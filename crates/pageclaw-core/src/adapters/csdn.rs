use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::PageclawError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static CSDN_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "#articleContentId, h1.title-article, h1.article-title, meta[property='og:title']",
    )
    .unwrap()
});

static CSDN_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("a.follow-nickName, a.user-name, .profile-intro .name, span.name, .author-info-block .name, meta[name='author']").unwrap()
});

static CSDN_TIME_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("span.time, .time-factor, meta[property='article:published_time']").unwrap()
});

static CSDN_READ_COUNT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("span.read-count, .read-count").unwrap());

static CSDN_TAG_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("a.tag-link, .tag-link, .blog-tags-box a, .tags-box a").unwrap()
});

static CSDN_CONTENT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#content_views, #article_content, article.baidu_pl, .article_content").unwrap()
});

static CSDN_LINK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"https?://link\.csdn\.net/\?target=([^"'\s&]+)"#).unwrap()
});

static CSDN_NOISE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<div[^>]*class="[^"]*(?:hide-article-box|recommend-box|opt-box|login-mark|tool-box|csdn-side-toolbar)[^"]*"[^>]*>.*?</div>"#).unwrap()
});

static CSDN_LINE_NUM_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<ul[^>]*class="[^"]*pre-numbering[^"]*"[^>]*>.*?</ul>"#).unwrap()
});

static CSDN_COPY_BTN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<div[^>]*class="[^"]*hljs-button[^"]*"[^>]*>.*?</div>"#).unwrap()
});

pub struct CsdnAdapter;

impl Default for CsdnAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CsdnAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        debug!(url = %url, "Executing CSDN adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            )
            .header("Referer", "https://blog.csdn.net/")
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            )
            .send()
            .await?;

        let html = resp.text().await?;
        self.parse_article(&html, url, options)
    }

    pub fn parse_article(
        &self,
        raw_html: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = doc
            .select(&CSDN_TITLE_SELECTOR)
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
            .unwrap_or_else(|| "CSDN 博客文章".to_string());

        // 2. Extract Author
        let author = doc
            .select(&CSDN_AUTHOR_SELECTOR)
            .next()
            .and_then(|el| {
                if el.value().name() == "meta" {
                    el.value().attr("content").map(|s| s.trim().to_string())
                } else {
                    let a = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    if a.is_empty() {
                        None
                    } else {
                        Some(a)
                    }
                }
            })
            .unwrap_or_else(|| "CSDN 创作者".to_string());

        // 3. Extract Published Time
        let pub_time = doc.select(&CSDN_TIME_SELECTOR).next().and_then(|el| {
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

        // 4. Extract Read Count
        let read_count = doc
            .select(&CSDN_READ_COUNT_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string());

        // 5. Extract Tags
        let tags: Vec<String> = doc
            .select(&CSDN_TAG_SELECTOR)
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // 6. Extract and clean Content
        let body_html = if let Some(content_el) = doc.select(&CSDN_CONTENT_SELECTOR).next() {
            preprocess_csdn_html(&content_el.html())
        } else {
            let cleaned = crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)?;
            preprocess_csdn_html(&cleaned.html)
        };

        // 7. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => csdn_html_to_plain_text(&body_html),
            OutputFormat::Markdown | OutputFormat::Json => {
                let converter = MarkdownConverter::new();
                let md_body =
                    converter.convert(&body_html, options.strip_images, options.strip_links)?;

                let mut md = String::new();
                if options.frontmatter && options.format == OutputFormat::Markdown {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("author: \"{}\"\n", author.replace('"', "\\\"")));
                    md.push_str(&format!("url: \"{url}\"\n"));
                    if let Some(ref t) = pub_time {
                        md.push_str(&format!("published: \"{t}\"\n"));
                    }
                    if !tags.is_empty() {
                        md.push_str(&format!(
                            "tags: [{}]\n",
                            tags.iter()
                                .map(|t| format!("\"{}\"", t.replace('"', "\\\"")))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {}\n\n", title));

                let mut header_parts = Vec::new();
                if !author.is_empty() {
                    header_parts.push(format!("**作者**: {author}"));
                }
                if let Some(ref t) = pub_time {
                    if !t.is_empty() {
                        header_parts.push(format!("**发布时间**: {t}"));
                    }
                }
                if let Some(ref rc) = read_count {
                    if !rc.is_empty() {
                        header_parts.push(format!("**阅读量**: {rc}"));
                    }
                }
                if !tags.is_empty() {
                    header_parts.push(format!("**标签**: {}", tags.join(", ")));
                }
                if !header_parts.is_empty() {
                    md.push_str(&format!("> {}\n\n", header_parts.join(" | ")));
                }

                md.push_str(&md_body);
                md
            }
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
            byline: Some(author.clone()),
            content,
            metadata: PageMetadata {
                author: Some(author),
                published_time: pub_time,
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
}

impl PlatformAdapter for CsdnAdapter {
    fn name(&self) -> &'static str {
        "csdn"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "csdn.net")
            && url.path().contains("/article/details/")
    }

    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, PageclawError>> {
        Box::pin(self.do_extract(fetcher, url, options))
    }
}

/// Preprocesses CSDN HTML content:
/// 1. Unpacks `link.csdn.net/?target=...` tracking redirects to direct original URLs.
/// 2. Strips paywall/login boxes, code-copy buttons, and line-number listings.
pub fn preprocess_csdn_html(raw_html: &str) -> String {
    // 1. Remove code line-numbering and copy buttons
    let without_line_nums = CSDN_LINE_NUM_RE.replace_all(raw_html, "");
    let without_copy_btn = CSDN_COPY_BTN_RE.replace_all(&without_line_nums, "");
    let without_noise = CSDN_NOISE_RE.replace_all(&without_copy_btn, "");

    // 2. Unpack `link.csdn.net/?target=...`
    CSDN_LINK_RE
        .replace_all(&without_noise, |caps: &regex::Captures| {
            let target = caps.get(1).map_or("", |m| m.as_str());
            let decoded: String = url::form_urlencoded::byte_serialize(target.as_bytes()).collect();
            url::form_urlencoded::parse(target.as_bytes())
                .map(|(k, _)| k.to_string())
                .next()
                .unwrap_or_else(|| decoded)
        })
        .to_string()
}

fn csdn_html_to_plain_text(html: &str) -> String {
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
    fn test_csdn_adapter_matching() {
        let adapter = CsdnAdapter::new();
        let u1 = Url::parse("https://blog.csdn.net/user123/article/details/123456789").unwrap();
        let u2 = Url::parse("https://csdn.net/user/article/details/999").unwrap();
        let u3 = Url::parse("https://evilcsdn.net/article/details/123").unwrap();
        let u4 = Url::parse("https://blog.csdn.net/user123/category_123.html").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
        assert!(!adapter.matches(&u4));
    }

    #[test]
    fn test_csdn_link_unpacking() {
        let sample = r#"<p>参考官方文档: <a href="https://link.csdn.net/?target=https%3A%2F%2Fdocs.rs%2Ftokio">Tokio Docs</a></p>"#;
        let cleaned = preprocess_csdn_html(sample);
        assert!(cleaned.contains("https://docs.rs/tokio"));
        assert!(!cleaned.contains("link.csdn.net/?target="));
    }

    #[test]
    fn test_csdn_noise_stripping() {
        let sample = r#"
            <div id="content_views">
                <p>正文内容开始</p>
                <div class="hide-article-box">展开阅读全文</div>
                <div class="hljs-button">复制代码</div>
                <ul class="pre-numbering"><li>1</li><li>2</li></ul>
                <p>正文内容结束</p>
            </div>
        "#;
        let cleaned = preprocess_csdn_html(sample);
        assert!(cleaned.contains("正文内容开始"));
        assert!(cleaned.contains("正文内容结束"));
        assert!(!cleaned.contains("hide-article-box"));
        assert!(!cleaned.contains("hljs-button"));
        assert!(!cleaned.contains("pre-numbering"));
    }

    #[test]
    fn test_csdn_parse_article() {
        let adapter = CsdnAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>Rust 并发编程指南</title></head>
            <body>
                <h1 class="title-article" id="articleContentId">Rust 并发编程完全指南</h1>
                <div class="profile-intro">
                    <a class="follow-nickName">Rust高手</a>
                </div>
                <span class="time">2026-05-20 14:30:00</span>
                <span class="read-count">阅读量 1.2w</span>
                <div class="blog-tags-box">
                    <a class="tag-link">Rust</a>
                    <a class="tag-link">并发</a>
                </div>
                <div id="content_views">
                    <p>Rust 的并发安全性基于所有权系统。</p>
                    <pre><code class="language-rust">std::thread::spawn(|| {});</code></pre>
                </div>
            </body>
            </html>
        "#;

        let url = Url::parse("https://blog.csdn.net/rust_dev/article/details/123456").unwrap();
        let doc = adapter
            .parse_article(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("Rust 并发编程完全指南"));
        assert_eq!(doc.byline.as_deref(), Some("Rust高手"));
        assert!(doc.content.contains("Rust 的并发安全性基于所有权系统。"));
        assert!(doc.content.contains("std::thread::spawn"));
        assert!(doc.content.contains("**作者**: Rust高手"));
        assert!(doc.content.contains("**阅读量**: 阅读量 1.2w"));
        assert!(doc.content.contains("**标签**: Rust, 并发"));
    }
}
