use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static SUBSTACK_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("h1.post-title, h1[data-testid='post-title'], h1, meta[property='og:title']")
        .unwrap()
});

static SUBSTACK_SUBTITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("h3.subtitle, h2.subtitle, meta[name='description']").unwrap()
});

static SUBSTACK_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("a.author, a[rel='author'], .byline a, meta[name='author']").unwrap()
});

static SUBSTACK_TIME_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("time.post-date, time, meta[property='article:published_time']").unwrap()
});

static SUBSTACK_CONTENT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("div.body.markup, div.available-content, article.post, .entry-content").unwrap()
});

static SUBSTACK_NOISE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<(?:div|section|aside)[^>]*?(?:class="[^"]*(?:subscription-widget|post-footer|post-ufi|paywall|subscribe-prompt|email-capture)[^"]*"|data-testid="[^"]*(?:subscribe|paywall)[^"]*")[^>]*>.*?</(?:div|section|aside)>"#).unwrap()
});

pub struct SubstackAdapter;

impl Default for SubstackAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SubstackAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Substack adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            )
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
    ) -> Result<PageDocument, TklessError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = doc
            .select(&SUBSTACK_TITLE_SELECTOR)
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
            .unwrap_or_else(|| "Substack Newsletter".to_string());

        // 2. Extract Subtitle
        let subtitle = doc
            .select(&SUBSTACK_SUBTITLE_SELECTOR)
            .next()
            .and_then(|el| {
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
            .select(&SUBSTACK_AUTHOR_SELECTOR)
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
            .unwrap_or_else(|| "Substack Writer".to_string());

        // 4. Extract Published Time
        let pub_time = doc.select(&SUBSTACK_TIME_SELECTOR).next().and_then(|el| {
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

        // 5. Extract and clean Content
        let body_html = if let Some(content_el) = doc.select(&SUBSTACK_CONTENT_SELECTOR).next() {
            preprocess_substack_html(&content_el.html())
        } else {
            let cleaned = crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)?;
            preprocess_substack_html(&cleaned.html)
        };

        // 6. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => substack_html_to_plain_text(&body_html),
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
                    if let Some(ref sub) = subtitle {
                        md.push_str(&format!("description: \"{}\"\n", sub.replace('"', "\\\"")));
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

impl PlatformAdapter for SubstackAdapter {
    fn name(&self) -> &'static str {
        "substack"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "substack.com")
            || host.ends_with(".substack.com")
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

/// Preprocesses Substack HTML: strips subscription widgets, paywall blocks, and footer reaction bars.
pub fn preprocess_substack_html(raw_html: &str) -> String {
    SUBSTACK_NOISE_RE.replace_all(raw_html, "").to_string()
}

fn substack_html_to_plain_text(html: &str) -> String {
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
    fn test_substack_adapter_matching() {
        let adapter = SubstackAdapter::new();
        let u1 = Url::parse("https://pragmaticengineer.substack.com/p/software-engineering-2026")
            .unwrap();
        let u2 = Url::parse("https://substack.com/browse").unwrap();
        let u3 = Url::parse("https://evilsubstack.com/p/fake").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
    }

    #[test]
    fn test_substack_noise_stripping() {
        let sample = r#"
            <article class="post">
                <p>Welcome to this edition of the newsletter.</p>
                <div class="subscription-widget-wrap">
                    <p>Subscribe to read more exciting articles!</p>
                </div>
                <div class="post-ufi">Like / Restack / Comment</div>
                <p>Here is the core technical architecture analysis.</p>
            </article>
        "#;
        let cleaned = preprocess_substack_html(sample);
        assert!(cleaned.contains("Welcome to this edition of the newsletter."));
        assert!(cleaned.contains("Here is the core technical architecture analysis."));
        assert!(!cleaned.contains("subscription-widget-wrap"));
        assert!(!cleaned.contains("post-ufi"));
    }

    #[test]
    fn test_substack_parse_article() {
        let adapter = SubstackAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <title>The Architecture of Scalable Systems</title>
                <meta property="og:title" content="The Architecture of Scalable Systems" />
                <meta name="author" content="Gergely Orosz" />
            </head>
            <body>
                <article class="post">
                    <h1 class="post-title">The Architecture of Scalable Systems</h1>
                    <h3 class="subtitle">How leading tech companies structure microservices</h3>
                    <div class="byline">
                        <a class="author">Gergely Orosz</a>
                    </div>
                    <time class="post-date" datetime="2026-10-18">Oct 18, 2026</time>
                    <div class="body markup">
                        <p>Scaling a system requires clear domain separation.</p>
                        <pre><code>pub struct ClusterNode;</code></pre>
                    </div>
                </article>
            </body>
            </html>
        "#;

        let url = Url::parse("https://newsletter.substack.com/p/scalable-systems").unwrap();
        let doc = adapter
            .parse_article(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(
            doc.title.as_deref(),
            Some("The Architecture of Scalable Systems")
        );
        assert_eq!(doc.byline.as_deref(), Some("Gergely Orosz"));
        assert!(doc
            .content
            .contains("How leading tech companies structure microservices"));
        assert!(doc
            .content
            .contains("Scaling a system requires clear domain separation."));
        assert!(doc.content.contains("pub struct ClusterNode"));
        assert!(doc.content.contains("**Author**: Gergely Orosz"));
        assert!(doc.content.contains("**Published**: 2026-10-18"));
    }
}
