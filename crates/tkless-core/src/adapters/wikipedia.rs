//! Wikipedia encyclopedia content adapter.
//!
//! Intercepts `*.wikipedia.org/wiki/...` URLs, extracting clean encyclopedia articles
//! with citation footnote links (`[1]`, `[2]`), edit section buttons (`[edit]`),
//! navigation boxes (`.navbox`), and disambiguation notices stripped out.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static WIKI_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#firstHeading, h1.firstHeading, h1#firstHeading, h1").unwrap()
});

static WIKI_CONTENT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#mw-content-text .mw-parser-output, #bodyContent, #content").unwrap()
});

static WIKI_NOISE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<(?:span|div|table|aside|sup)[^>]*?(?:class="[^"]*(?:mw-editsection|reference|navbox|vertical-navbox|sidebar|catlinks|hatnote|ambox|noprint|metadata)[^"]*"|id="toc")[^>]*>.*?</(?:span|div|table|aside|sup)>"#).unwrap()
});

static WIKI_SUP_REF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<sup[^>]*class="[^"]*reference[^"]*"[^>]*>.*?</sup>"#).unwrap()
});

static WIKI_EDIT_SECTION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<span[^>]*class="[^"]*mw-editsection[^"]*"[^>]*>.*?</span>"#).unwrap()
});

pub struct WikipediaAdapter;

impl Default for WikipediaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl WikipediaAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Wikipedia adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 TkLess/0.1 (https://github.com/tkless/tkless)",
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
    ) -> Result<PageDocument, TklessError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = doc
            .select(&WIKI_TITLE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "Wikipedia Article".to_string());

        // 2. Extract and clean Content
        let body_html = if let Some(content_el) = doc.select(&WIKI_CONTENT_SELECTOR).next() {
            preprocess_wikipedia_html(&content_el.html())
        } else {
            let cleaned = crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)?;
            preprocess_wikipedia_html(&cleaned.html)
        };

        // 3. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => wiki_html_to_plain_text(&body_html),
            OutputFormat::Markdown | OutputFormat::Json => {
                let converter = MarkdownConverter::new();
                let md_body =
                    converter.convert(&body_html, options.strip_images, options.strip_links)?;

                let mut md = String::new();
                if options.frontmatter && options.format == OutputFormat::Markdown {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("url: \"{url}\"\n"));
                    md.push_str("source: \"Wikipedia\"\n");
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {}\n\n", title));
                md.push_str(&format!("> **来源**: [维基百科 (Wikipedia)]({url})\n\n"));
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
            byline: Some("Wikipedia Contributors".to_string()),
            content,
            metadata: PageMetadata {
                author: Some("Wikipedia Contributors".to_string()),
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

impl PlatformAdapter for WikipediaAdapter {
    fn name(&self) -> &'static str {
        "wikipedia"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        (crate::fetcher::is_subdomain_or_exact(host, "wikipedia.org")
            || host.ends_with(".wikipedia.org"))
            && url.path().starts_with("/wiki/")
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

/// Preprocesses Wikipedia HTML: strips edit buttons, reference citations, navboxes, and TOC.
pub fn preprocess_wikipedia_html(raw_html: &str) -> String {
    let s1 = WIKI_SUP_REF_RE.replace_all(raw_html, "");
    let s2 = WIKI_EDIT_SECTION_RE.replace_all(&s1, "");
    WIKI_NOISE_RE.replace_all(&s2, "").to_string()
}

fn wiki_html_to_plain_text(html: &str) -> String {
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
    fn test_wikipedia_adapter_matching() {
        let adapter = WikipediaAdapter::new();
        let u1 = Url::parse("https://en.wikipedia.org/wiki/Rust_(programming_language)").unwrap();
        let u2 = Url::parse("https://zh.wikipedia.org/wiki/%E4%BA%BA%E5%B7%A5%E6%99%BA%E8%83%BD")
            .unwrap();
        let u3 = Url::parse("https://evilwikipedia.org/wiki/Fake").unwrap();
        let u4 = Url::parse("https://en.wikipedia.org/w/index.php?search=test").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
        assert!(!adapter.matches(&u4));
    }

    #[test]
    fn test_wikipedia_noise_stripping() {
        let sample = r##"
            <div class="mw-parser-output">
                <p>Rust is a multi-paradigm programming language.<sup class="reference"><a href="#cite_note-1">[1]</a></sup></p>
                <h2>History<span class="mw-editsection"><a href="#">[edit]</a></span></h2>
                <div class="navbox">Navigation box junk</div>
                <div id="toc">Table of contents</div>
                <p>The language was designed by Graydon Hoare.<sup class="reference"><a href="#cite_note-2">[2]</a></sup></p>
            </div>
        "##;

        let cleaned = preprocess_wikipedia_html(sample);
        assert!(cleaned.contains("Rust is a multi-paradigm programming language."));
        assert!(cleaned.contains("The language was designed by Graydon Hoare."));
        assert!(!cleaned.contains("[1]"));
        assert!(!cleaned.contains("[2]"));
        assert!(!cleaned.contains("[edit]"));
        assert!(!cleaned.contains("Navigation box junk"));
        assert!(!cleaned.contains("Table of contents"));
    }

    #[test]
    fn test_wikipedia_parse_article() {
        let adapter = WikipediaAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>Rust (programming language) - Wikipedia</title></head>
            <body>
                <h1 id="firstHeading">Rust (programming language)</h1>
                <div id="mw-content-text">
                    <div class="mw-parser-output">
                        <p>Rust is a general-purpose programming language emphasizing performance and safety.<sup class="reference">[1]</sup></p>
                        <h2>Syntax and semantics<span class="mw-editsection">[edit]</span></h2>
                        <pre><code>fn main() { println!("Hello!"); }</code></pre>
                    </div>
                </div>
            </body>
            </html>
        "#;

        let url = Url::parse("https://en.wikipedia.org/wiki/Rust_(programming_language)").unwrap();
        let doc = adapter
            .parse_article(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("Rust (programming language)"));
        assert!(doc.content.contains("emphasizing performance and safety."));
        assert!(doc.content.contains("fn main() { println!(\"Hello!\"); }"));
        assert!(!doc.content.contains("[1]"));
        assert!(!doc.content.contains("[edit]"));
        assert!(doc.content.contains("**来源**: [维基百科 (Wikipedia)]"));
    }
}
