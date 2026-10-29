use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static JUEJIN_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1.article-title, h1.title, .article-title").unwrap());
static JUEJIN_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".author-name, .user-name, .username span, .author-info-block .name").unwrap()
});
static JUEJIN_TIME_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("time, .time, .meta-box time, meta[itemprop='datePublished']").unwrap()
});
static JUEJIN_CONTENT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("article.article-content, .markdown-body, .article-viewer").unwrap()
});

static JUEJIN_LINK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"https?://link\.juejin\.cn/\?target=([^"'\s&]+)"#).unwrap()
});

pub struct JuejinAdapter;

impl Default for JuejinAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl JuejinAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Juejin adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            )
            .header("Referer", "https://juejin.cn/")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
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
    ) -> Result<PageDocument, TklessError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = doc
            .select(&JUEJIN_TITLE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .or_else(|| extract_meta_property(&doc, "og:title"))
            .unwrap_or_else(|| "掘金文章".to_string());

        // 2. Extract Author
        let author = doc
            .select(&JUEJIN_AUTHOR_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .or_else(|| extract_meta_property(&doc, "author"))
            .unwrap_or_else(|| "掘金开发者".to_string());

        // 3. Extract Published Time
        let pub_time = doc
            .select(&JUEJIN_TIME_SELECTOR)
            .next()
            .and_then(|el| {
                el.value()
                    .attr("datetime")
                    .or_else(|| el.value().attr("content"))
            })
            .map(|s| s.trim().to_string())
            .or_else(|| {
                doc.select(&JUEJIN_TIME_SELECTOR)
                    .next()
                    .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            });

        // 4. Extract and clean Content (#article-content)
        let body_html = if let Some(content_el) = doc.select(&JUEJIN_CONTENT_SELECTOR).next() {
            preprocess_juejin_html(&content_el.html())
        } else {
            let cleaned = crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)?;
            preprocess_juejin_html(&cleaned.html)
        };

        // 5. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => juejin_html_to_plain_text(&body_html),
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
                if !header_parts.is_empty() {
                    md.push_str(&format!("> {}\n\n", header_parts.join(" | ")));
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
            title: Some(title.to_string()),
            byline: Some(author.to_string()),
            content,
            metadata: PageMetadata {
                author: Some(author.to_string()),
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

impl PlatformAdapter for JuejinAdapter {
    fn name(&self) -> &'static str {
        "juejin"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "juejin.cn") && url.path().starts_with("/post/")
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

/// Preprocesses Juejin HTML content:
/// Unpacks `link.juejin.cn/?target=...` tracking redirects to direct original URLs.
pub fn preprocess_juejin_html(raw_html: &str) -> String {
    JUEJIN_LINK_RE
        .replace_all(raw_html, |caps: &regex::Captures| {
            let target = caps.get(1).map_or("", |m| m.as_str());
            let decoded: String = url::form_urlencoded::byte_serialize(target.as_bytes()).collect();
            url::form_urlencoded::parse(target.as_bytes())
                .map(|(k, _)| k.to_string())
                .next()
                .unwrap_or(decoded)
        })
        .to_string()
}

fn extract_meta_property(doc: &Html, property: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[property='{property}']")).ok()?;
    let el = doc.select(&selector).next()?;
    el.value().attr("content").map(|s| s.trim().to_string())
}

fn juejin_html_to_plain_text(html: &str) -> String {
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
    fn test_juejin_adapter_matching() {
        let adapter = JuejinAdapter::new();
        let u1 = Url::parse("https://juejin.cn/post/7123456789012345678").unwrap();
        let u2 = Url::parse("https://eviljuejin.cn/post/123").unwrap();
        let u3 = Url::parse("https://juejin.cn/pins").unwrap();

        assert!(adapter.matches(&u1));
        assert!(!adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
    }

    #[test]
    fn test_juejin_link_unpacking() {
        let sample = r#"<p>查看 <a href="https://link.juejin.cn/?target=https%3A%2F%2Fgithub.com%2Ftokio-rs%2Ftokio">Tokio源码</a></p>"#;
        let cleaned = preprocess_juejin_html(sample);
        assert!(cleaned.contains("https://github.com/tokio-rs/tokio"));
        assert!(!cleaned.contains("link.juejin.cn/?target="));
    }

    #[test]
    fn test_juejin_parse_article() {
        let adapter = JuejinAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>Tokio 源码解析</title></head>
            <body>
                <h1 class="article-title">Tokio 核心架构解析</h1>
                <div class="author-name">Rust专家</div>
                <time datetime="2026-06-15 10:00:00">2026-06-15</time>
                <article class="article-content">
                    <p>Tokio 是一个异步事件驱动的运行时。</p>
                    <pre><code>async fn run() {}</code></pre>
                </article>
            </body>
            </html>
        "#;

        let url = Url::parse("https://juejin.cn/post/7123456789").unwrap();
        let doc = adapter
            .parse_article(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("Tokio 核心架构解析"));
        assert_eq!(doc.byline.as_deref(), Some("Rust专家"));
        assert!(doc.content.contains("Tokio 是一个异步事件驱动的运行时。"));
        assert!(doc.content.contains("async fn run()"));
        assert!(doc.content.contains("**作者**: Rust专家"));
    }
}
