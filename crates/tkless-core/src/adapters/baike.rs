//! Baidu Baike Chinese encyclopedia knowledge adapter.
//!
//! Intercepts `baike.baidu.com/item/...` URLs, extracting entry title,
//! polysemy description, basic info key-value tables, and sectioned content
//! while stripping promotional carousels, editing widgets, and sidebar noise.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static BAIKE_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".lemma-title, .lemmaTitleH1, h1, meta[property='og:title']").unwrap()
});

static BAIKE_SUBTITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".lemma-desc, h2.lemma-desc, meta[name='description']").unwrap()
});

static BAIKE_CONTENT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".main-content, .lemmaWgt-lemmaTitle, .lemma-summary, .para").unwrap()
});

static BAIKE_INFOBOX_NAME_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("dt.basicInfo-item.name, .basicInfo-item.name").unwrap());

static BAIKE_INFOBOX_VALUE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("dd.basicInfo-item.value, .basicInfo-item.value").unwrap());

static BAIKE_SUP_REF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?is)<sup[^>]*class="[^"]*(?:sup-anchor|anchor-list)[^"]*"[^>]*>.*?</sup>"#,
    )
    .unwrap()
});

static BAIKE_NOISE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<(?:div|section|aside|nav)[^>]*?(?:class="[^"]*(?:top-tool|side-content|lemma-relation-table|edit-lemma|album-list|copyright|share-list)[^"]*"|id="side_box")[^>]*>.*?</(?:div|section|aside|nav)>"#).unwrap()
});

pub struct BaikeAdapter;

impl Default for BaikeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl BaikeAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Baidu Baike adapter extraction");

        let fetch_res = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
            )
            .header("Referer", "https://baike.baidu.com/")
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            )
            .send()
            .await;

        let html = match fetch_res {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let text = fetcher.read_text(resp).await?;
                    if text.contains("__PARIS_CHALLENGE_START__")
                        || text.contains("百度安全验证")
                        || text.contains("mkdjump")
                    {
                        // Challenge page detected, attempt OpenAPI lemma recovery
                        if let Some(doc) = self.try_openapi_recovery(fetcher, url, options).await {
                            return Ok(doc);
                        }
                        return Err(TklessError::AdapterError {
                            platform: "baike",
                            message: "Baidu Baike Paris/Heicha WAF anti-scraping challenge triggered (百度安全验证)".to_string(),
                        });
                    }
                    text
                } else if status == reqwest::StatusCode::FORBIDDEN || status.as_u16() == 429 {
                    if let Some(doc) = self.try_openapi_recovery(fetcher, url, options).await {
                        return Ok(doc);
                    }
                    return Err(TklessError::AdapterError {
                        platform: "baike",
                        message: format!("Baidu Baike blocked with HTTP status {status}"),
                    });
                } else {
                    return Err(TklessError::HttpStatusError {
                        status: status.as_u16(),
                        url: url.to_string(),
                    });
                }
            }
            Err(err) => {
                if let Some(doc) = self.try_openapi_recovery(fetcher, url, options).await {
                    return Ok(doc);
                }
                return Err(TklessError::Network(err));
            }
        };

        self.parse_page(&html, url, options)
    }

    /// Attempts to recover article content directly via Baidu Baike's LemmaCard OpenAPI
    /// when the standard web page is intercepted by WAF 403 or Paris anti-scraping challenges.
    async fn try_openapi_recovery(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Option<PageDocument> {
        let lemma_name = extract_lemma_name_from_url(url)?;
        debug!(lemma = %lemma_name, "Attempting Baidu Baike OpenAPI recovery for lemma");

        let encoded =
            url::form_urlencoded::byte_serialize(lemma_name.as_bytes()).collect::<String>();
        let api_url = format!(
            "https://baike.baidu.com/api/openapi/BaikeLemmaCardApi?scope=103&format=json&appid=379020&bk_key={encoded}"
        );

        let resp = fetcher.client().get(&api_url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let data = fetcher.read_json::<serde_json::Value>(resp).await.ok()?;
        let title = data.get("title").and_then(|v| v.as_str())?.to_string();
        let abstract_text = data
            .get("abstract")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let desc = data
            .get("desc")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();

        let mut markdown = format!("# {title}\n\n");
        if !desc.is_empty() {
            markdown.push_str(&format!("> {desc}\n\n"));
        }
        if !abstract_text.is_empty() {
            markdown.push_str("## 概述\n\n");
            markdown.push_str(abstract_text);
            markdown.push_str("\n\n");
        }

        // Add infobox card if present
        if let Some(card_arr) = data.get("card").and_then(|v| v.as_array()) {
            if !card_arr.is_empty() {
                markdown.push_str("## 基本信息\n\n");
                for item in card_arr {
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim();
                    let val = item
                        .get("value")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join("、")
                        })
                        .unwrap_or_default();
                    if !name.is_empty() && !val.is_empty() {
                        markdown.push_str(&format!("- **{name}**: {val}\n"));
                    }
                }
                markdown.push('\n');
            }
        }

        let token_count = count_tokens(&markdown, options.tokenizer);

        Some(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(format!("{title}_百度百科")),
            byline: Some("百度百科".to_string()),
            content: markdown,
            metadata: PageMetadata {
                description: Some(abstract_text.to_string()),
                canonical_url: Some(url.to_string()),
                ..Default::default()
            },
            json_ld: Vec::new(),
            token_count,
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }

    pub fn parse_page(
        &self,
        raw_html: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = doc
            .select(&BAIKE_TITLE_SELECTOR)
            .next()
            .and_then(|el| {
                if el.value().name() == "meta" {
                    el.value().attr("content").map(|s| s.trim().to_string())
                } else {
                    let t = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }
            })
            .unwrap_or_else(|| "百度百科词条".to_string());

        // 2. Extract Subtitle / Description
        let subtitle = doc.select(&BAIKE_SUBTITLE_SELECTOR).next().and_then(|el| {
            if el.value().name() == "meta" {
                el.value().attr("content").map(|s| s.trim().to_string())
            } else {
                let s = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
        });

        // 3. Extract Basic Info Box (Key-Value pairs)
        let mut infobox = Vec::new();
        let names: Vec<String> = doc
            .select(&BAIKE_INFOBOX_NAME_SELECTOR)
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join("")
                    .trim()
                    .replace('\u{a0}', "")
                    .to_string()
            })
            .collect();
        let values: Vec<String> = doc
            .select(&BAIKE_INFOBOX_VALUE_SELECTOR)
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .collect();

        for (k, v) in names.into_iter().zip(values) {
            if !k.is_empty() && !v.is_empty() {
                infobox.push((k, v));
            }
        }

        // 4. Extract Main Content
        let cleaned_html = preprocess_baike_html(raw_html);
        let cleaned_doc = Html::parse_document(&cleaned_html);

        let body_html = if let Some(content_el) = cleaned_doc.select(&BAIKE_CONTENT_SELECTOR).next()
        {
            content_el.html()
        } else {
            let cleaned =
                crate::cleaner::clean_html(&cleaned_html, Some(url.as_str()), options.mode)?;
            cleaned.html
        };

        // 5. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => baike_html_to_plain_text(&body_html),
            OutputFormat::Markdown | OutputFormat::Json => {
                let converter = MarkdownConverter::new();
                let md_body =
                    converter.convert(&body_html, options.strip_images, options.strip_links)?;

                let mut md = String::new();
                if options.frontmatter && options.format == OutputFormat::Markdown {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("url: \"{url}\"\n"));
                    md.push_str("source: \"Baidu Baike\"\n");
                    if let Some(ref sub) = subtitle {
                        md.push_str(&format!("subtitle: \"{}\"\n", sub.replace('"', "\\\"")));
                    }
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {}\n\n", title));

                if let Some(ref sub) = subtitle {
                    if !sub.is_empty() && sub != &title {
                        md.push_str(&format!("*{}*\n\n", sub));
                    }
                }

                md.push_str(&format!("> **来源**: [百度百科]({url})\n\n"));

                // If infobox is present, format as a clean GFM table
                if !infobox.is_empty() && !options.tables_only {
                    md.push_str("### 基本信息 (Basic Info)\n\n");
                    md.push_str("| 属性 (Property) | 内容 (Value) |\n");
                    md.push_str("|---|---|\n");
                    for (k, v) in &infobox {
                        md.push_str(&format!("| **{}** | {} |\n", k.trim(), v.trim()));
                    }
                    md.push_str("\n---\n\n");
                }

                md.push_str(&md_body);
                md
            }
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
            byline: Some("百度百科".to_string()),
            content,
            metadata: PageMetadata {
                author: Some("百度百科".to_string()),
                canonical_url: Some(url.to_string()),
                description: subtitle,
                ..Default::default()
            },
            json_ld: Vec::new(),
            token_count,
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }
}

impl PlatformAdapter for BaikeAdapter {
    fn name(&self) -> &'static str {
        "baike"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "baike.baidu.com")
            && url.path().starts_with("/item/")
    }

    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, TklessError>> {
        Box::pin(self.do_extract(fetcher, url, options))
    }
}

/// Preprocesses Baidu Baike HTML: strips citation anchors and promotional noise blocks.
pub fn preprocess_baike_html(raw_html: &str) -> String {
    let s1 = BAIKE_SUP_REF_RE.replace_all(raw_html, "");
    BAIKE_NOISE_RE.replace_all(&s1, "").to_string()
}

/// Extracts and URL-decodes the lemma entry name from a Baidu Baike item URL path.
pub fn extract_lemma_name_from_url(url: &Url) -> Option<String> {
    let path = url.path();
    let stripped = path.strip_prefix("/item/")?;
    let first_segment = stripped.split('/').next()?;
    if first_segment.is_empty() {
        return None;
    }
    let decoded = url::form_urlencoded::parse(first_segment.as_bytes())
        .map(|(k, _)| k.to_string())
        .next()
        .unwrap_or_else(|| first_segment.to_string());
    Some(decoded)
}

fn baike_html_to_plain_text(html: &str) -> String {
    let doc = Html::parse_fragment(html);
    let mut text_parts = Vec::new();

    for text in doc.root_element().text() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            text_parts.push(trimmed);
        }
    }

    text_parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_baike_adapter_matching() {
        let adapter = BaikeAdapter::new();
        let u1 =
            Url::parse("https://baike.baidu.com/item/Rust%E8%AF%AD%E8%A8%80/20349806").unwrap();
        let u2 = Url::parse("https://baike.baidu.com/item/%E4%BA%BA%E5%B7%A5%E6%99%BA%E8%83%BD")
            .unwrap();
        let u3 = Url::parse("https://evilbaike.baidu.com/item/test").unwrap();
        let u4 = Url::parse("https://baike.baidu.com/tashuo/browse/content").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
        assert!(!adapter.matches(&u4));
    }

    #[test]
    fn test_baike_parse_page() {
        let adapter = BaikeAdapter::new();
        let sample_html = r##"
            <!DOCTYPE html>
            <html>
            <head><title>Rust语言 - 百度百科</title></head>
            <body>
                <h1 class="lemma-title">Rust语言</h1>
                <h2 class="lemma-desc">通用系统级编程语言</h2>
                <div class="basic-info">
                    <dl>
                        <dt class="basicInfo-item name">中文名</dt>
                        <dd class="basicInfo-item value">Rust编程语言</dd>
                        <dt class="basicInfo-item name">设计者</dt>
                        <dd class="basicInfo-item value">Graydon Hoare</dd>
                    </dl>
                </div>
                <div class="main-content">
                    <p class="para">Rust是一门由Mozilla主导开发的注重安全、并发与性能的高性能语言。<sup class="sup-anchor">[1]</sup></p>
                    <div class="edit-lemma">编辑词条</div>
                </div>
            </body>
            </html>
        "##;

        let url = Url::parse("https://baike.baidu.com/item/Rust").unwrap();
        let doc = adapter
            .parse_page(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("Rust语言"));
        assert!(doc.content.contains("通用系统级编程语言"));
        assert!(doc.content.contains("基本信息 (Basic Info)"));
        assert!(doc.content.contains("**中文名**"));
        assert!(doc.content.contains("Rust编程语言"));
        assert!(doc.content.contains("**设计者**"));
        assert!(doc.content.contains("Graydon Hoare"));
        assert!(doc.content.contains("Rust是一门由Mozilla主导开发"));
        assert!(!doc.content.contains("[1]"));
        assert!(!doc.content.contains("编辑词条"));
    }

    #[test]
    fn test_extract_lemma_name_from_url() {
        let u1 =
            Url::parse("https://baike.baidu.com/item/Rust%E8%AF%AD%E8%A8%80/20349806").unwrap();
        assert_eq!(
            extract_lemma_name_from_url(&u1),
            Some("Rust语言".to_string())
        );

        let u2 = Url::parse("https://baike.baidu.com/item/%E5%85%89%E5%90%88%E4%BD%9C%E7%94%A8")
            .unwrap();
        assert_eq!(
            extract_lemma_name_from_url(&u2),
            Some("光合作用".to_string())
        );

        let u3 = Url::parse("https://baike.baidu.com/item/Python").unwrap();
        assert_eq!(extract_lemma_name_from_url(&u3), Some("Python".to_string()));

        let u4 = Url::parse("https://baike.baidu.com/other/path").unwrap();
        assert_eq!(extract_lemma_name_from_url(&u4), None);
    }
}
