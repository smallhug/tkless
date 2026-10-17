use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::PageclawError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static MEDIUM_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("h1[data-testid='storyTitle'], h1.pw-post-title, h1, meta[property='og:title']")
        .unwrap()
});

static MEDIUM_SUBTITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("h2.subtitle, h2.pw-subtitle-paragraph, meta[name='description']").unwrap()
});

static MEDIUM_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "a[data-testid='authorName'], a[rel='author'], .pw-author-name, meta[name='author']",
    )
    .unwrap()
});

static MEDIUM_TIME_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "span[data-testid='storyPublishDate'], time, meta[property='article:published_time']",
    )
    .unwrap()
});

static MEDIUM_READ_TIME_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "span[data-testid='storyReadTime'], .pw-reading-time, meta[name='twitter:data1']",
    )
    .unwrap()
});

static MEDIUM_CONTENT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("article, section.meteredContent, main article, .postArticle-content").unwrap()
});

static MEDIUM_NOISE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<(?:div|section|aside)[^>]*?(?:data-testid="[^"]*(?:paywall|responses-drawer|bottom-banner)[^"]*"|class="[^"]*(?:speechify-container|pw-multi-vote-icon|meteredProgressBlock)[^"]*")[^>]*>.*?</(?:div|section|aside)>"#).unwrap()
});

pub struct MediumAdapter;

impl Default for MediumAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MediumAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        debug!(url = %url, "Executing Medium adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            )
            .header("Referer", "https://medium.com/")
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
            .select(&MEDIUM_TITLE_SELECTOR)
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
            .unwrap_or_else(|| "Medium Article".to_string());

        // 2. Extract Subtitle
        let subtitle = doc.select(&MEDIUM_SUBTITLE_SELECTOR).next().and_then(|el| {
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

        // 3. Extract Author
        let author = doc
            .select(&MEDIUM_AUTHOR_SELECTOR)
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
            .unwrap_or_else(|| "Medium Author".to_string());

        // 4. Extract Published Time
        let pub_time = doc.select(&MEDIUM_TIME_SELECTOR).next().and_then(|el| {
            if el.value().name() == "meta" {
                el.value().attr("content").map(|s| s.trim().to_string())
            } else {
                el.value()
                    .attr("datetime")
                    .map(|s| s.trim().to_string())
                    .or_else(|| {
                        let t = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    })
            }
        });

        // 5. Extract Read Time
        let read_time = doc
            .select(&MEDIUM_READ_TIME_SELECTOR)
            .next()
            .and_then(|el| {
                if el.value().name() == "meta" {
                    el.value().attr("content").map(|s| s.trim().to_string())
                } else {
                    let rt = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    if rt.is_empty() {
                        None
                    } else {
                        Some(rt)
                    }
                }
            });

        // 6. Extract and clean Content
        let body_html = if let Some(content_el) = doc.select(&MEDIUM_CONTENT_SELECTOR).next() {
            preprocess_medium_html(&content_el.html())
        } else {
            let cleaned = crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)?;
            preprocess_medium_html(&cleaned.html)
        };

        // 7. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => medium_html_to_plain_text(&body_html),
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
                    if let Some(ref rt) = read_time {
                        md.push_str(&format!("reading_time: \"{rt}\"\n"));
                    }
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {}\n\n", title));

                if let Some(ref sub) = subtitle {
                    if !sub.is_empty() && sub != &title {
                        md.push_str(&format!("*{}*\n\n", sub));
                    }
                }

                let mut header_parts = Vec::new();
                if !author.is_empty() {
                    header_parts.push(format!("**Author**: {author}"));
                }
                if let Some(ref t) = pub_time {
                    if !t.is_empty() {
                        header_parts.push(format!("**Published**: {t}"));
                    }
                }
                if let Some(ref rt) = read_time {
                    if !rt.is_empty() {
                        header_parts.push(format!("**Reading Time**: {rt}"));
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

impl PlatformAdapter for MediumAdapter {
    fn name(&self) -> &'static str {
        "medium"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "medium.com") || host.ends_with(".medium.com")
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

/// Preprocesses Medium HTML content:
/// Strips paywall blocks, clapping widgets, speechify audio containers, and popups.
pub fn preprocess_medium_html(raw_html: &str) -> String {
    MEDIUM_NOISE_RE.replace_all(raw_html, "").to_string()
}

fn medium_html_to_plain_text(html: &str) -> String {
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
    fn test_medium_adapter_matching() {
        let adapter = MediumAdapter::new();
        let u1 = Url::parse("https://medium.com/@author/building-an-async-engine-in-rust").unwrap();
        let u2 = Url::parse(
            "https://towardsdatascience.medium.com/deep-learning-transformers-explained",
        )
        .unwrap();
        let u3 = Url::parse("https://evilmedium.com/story").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
    }

    #[test]
    fn test_medium_noise_stripping() {
        let sample = r#"
            <article>
                <p>Welcome to this guide on Rust.</p>
                <div data-testid="paywall-wrapper">
                    <p>Upgrade to member to read more...</p>
                </div>
                <div class="speechify-container">Listen to audio</div>
                <p>Here is the real conclusion.</p>
            </article>
        "#;
        let cleaned = preprocess_medium_html(sample);
        assert!(cleaned.contains("Welcome to this guide on Rust."));
        assert!(cleaned.contains("Here is the real conclusion."));
        assert!(!cleaned.contains("Upgrade to member"));
        assert!(!cleaned.contains("Listen to audio"));
    }

    #[test]
    fn test_medium_parse_article() {
        let adapter = MediumAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <title>Building High-Performance Systems with Rust</title>
                <meta property="og:title" content="Building High-Performance Systems with Rust" />
                <meta name="author" content="Jane Doe" />
            </head>
            <body>
                <article>
                    <h1 data-testid="storyTitle">Building High-Performance Systems with Rust</h1>
                    <h2 class="subtitle">A complete architectural breakdown</h2>
                    <a data-testid="authorName">Jane Doe</a>
                    <span data-testid="storyPublishDate">Oct 20, 2026</span>
                    <span data-testid="storyReadTime">6 min read</span>
                    <div>
                        <p>Rust provides memory safety without a garbage collector.</p>
                        <p>Zero-cost abstractions make high throughput possible.</p>
                    </div>
                </article>
            </body>
            </html>
        "#;

        let url = Url::parse("https://medium.com/@janedoe/building-systems-rust-1234").unwrap();
        let doc = adapter
            .parse_article(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(
            doc.title.as_deref(),
            Some("Building High-Performance Systems with Rust")
        );
        assert_eq!(doc.byline.as_deref(), Some("Jane Doe"));
        assert!(doc.content.contains("A complete architectural breakdown"));
        assert!(doc
            .content
            .contains("Zero-cost abstractions make high throughput possible."));
        assert!(doc.content.contains("**Author**: Jane Doe"));
        assert!(doc.content.contains("**Published**: Oct 20, 2026"));
        assert!(doc.content.contains("**Reading Time**: 6 min read"));
    }
}
