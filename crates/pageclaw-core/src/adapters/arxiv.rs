//! ArXiv research paper metadata and abstract extractor.
//!
//! Intercepts `arxiv.org/abs/...`, `arxiv.org/pdf/...`, and `arxiv.org/html/...` URLs,
//! extracting paper title, authors list, clean abstract with LaTeX math formulas preserved,
//! subjects/categories, publication dates, and direct PDF links.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::PageclawError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static ARXIV_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1.title, meta[name='citation_title']").unwrap());
static ARXIV_AUTHORS_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".authors a, meta[name='citation_author']").unwrap());
static ARXIV_ABSTRACT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("blockquote.abstract, meta[name='citation_abstract']").unwrap()
});
static ARXIV_SUBJECTS_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("td.tablecell.subjects, span.primary-subject").unwrap());
static ARXIV_DATELINE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".dateline, meta[name='citation_date']").unwrap());

pub struct ArxivAdapter;

impl Default for ArxivAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ArxivAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Extracts canonical arXiv ID from URL path (e.g. "2401.12345" or "cs/0601001").
    pub fn extract_arxiv_id(url: &Url) -> Option<String> {
        let path = url.path().trim_start_matches('/');
        let stripped = if let Some(rest) = path.strip_prefix("abs/") {
            rest
        } else if let Some(rest) = path.strip_prefix("pdf/") {
            rest.strip_suffix(".pdf").unwrap_or(rest)
        } else {
            path.strip_prefix("html/")?
        };

        let clean_id = stripped.trim_matches('/');
        if clean_id.is_empty() {
            None
        } else {
            Some(clean_id.to_string())
        }
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        debug!(url = %url, "Executing ArXiv adapter extraction");

        let arxiv_id = Self::extract_arxiv_id(url)
            .ok_or_else(|| PageclawError::InvalidArgument(format!("Invalid ArXiv URL: {url}")))?;

        // Canonicalize to abstract URL for structured metadata
        let abs_url = Url::parse(&format!("https://arxiv.org/abs/{arxiv_id}"))
            .map_err(|e| PageclawError::InvalidArgument(e.to_string()))?;

        let resp = fetcher
            .get(&abs_url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 PageClaw/0.1",
            )
            .send()
            .await?;

        let html = resp.text().await?;
        self.parse_paper(&html, url, &arxiv_id, options)
    }

    pub fn parse_paper(
        &self,
        raw_html: &str,
        original_url: &Url,
        arxiv_id: &str,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Title (stripping "Title:" or "[Title]" prefix)
        let title = doc
            .select(&ARXIV_TITLE_SELECTOR)
            .next()
            .and_then(|el| {
                if el.value().name() == "meta" {
                    el.value().attr("content").map(|s| s.trim().to_string())
                } else {
                    let raw = el.text().collect::<Vec<_>>().join(" ");
                    let clean = raw.trim().strip_prefix("Title:").unwrap_or(&raw).trim();
                    if clean.is_empty() {
                        None
                    } else {
                        Some(clean.to_string())
                    }
                }
            })
            .unwrap_or_else(|| format!("ArXiv Paper {arxiv_id}"));

        // 2. Extract Authors
        let mut authors: Vec<String> = Vec::new();
        for el in doc.select(&ARXIV_AUTHORS_SELECTOR) {
            if el.value().name() == "meta" {
                if let Some(name) = el.value().attr("content") {
                    let trimmed = name.trim().to_string();
                    if !trimmed.is_empty() && !authors.contains(&trimmed) {
                        authors.push(trimmed);
                    }
                }
            } else {
                let name = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                if !name.is_empty() && !authors.contains(&name) {
                    authors.push(name);
                }
            }
        }
        let author_str = if authors.is_empty() {
            "Unknown Authors".to_string()
        } else {
            authors.join(", ")
        };

        // 3. Extract Abstract (stripping "Abstract:" prefix)
        let abstract_text = doc
            .select(&ARXIV_ABSTRACT_SELECTOR)
            .next()
            .and_then(|el| {
                if el.value().name() == "meta" {
                    el.value().attr("content").map(|s| s.trim().to_string())
                } else {
                    let raw = el.text().collect::<Vec<_>>().join(" ");
                    let clean = raw.trim().strip_prefix("Abstract:").unwrap_or(&raw).trim();
                    if clean.is_empty() {
                        None
                    } else {
                        Some(clean.to_string())
                    }
                }
            })
            .unwrap_or_default();

        // 4. Extract Subjects / Categories
        let subjects = doc
            .select(&ARXIV_SUBJECTS_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string());

        // 5. Extract Dateline / Submission Date
        let dateline = doc.select(&ARXIV_DATELINE_SELECTOR).next().and_then(|el| {
            if el.value().name() == "meta" {
                el.value().attr("content").map(|s| s.trim().to_string())
            } else {
                Some(el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            }
        });

        // 6. Direct PDF link
        let pdf_url = format!("https://arxiv.org/pdf/{arxiv_id}.pdf");

        // 7. Format Content
        let content = match options.format {
            OutputFormat::Html => format!(
                "<h1>{}</h1><p><b>Authors:</b> {}</p><p><b>Abstract:</b> {}</p><p><a href=\"{}\">Download PDF</a></p>",
                html_escape(&title),
                html_escape(&author_str),
                html_escape(&abstract_text),
                pdf_url
            ),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => format!(
                "Title: {title}\nAuthors: {author_str}\nArXiv ID: {arxiv_id}\nPDF: {pdf_url}\n\nAbstract:\n{abstract_text}"
            ),
            OutputFormat::Markdown | OutputFormat::Json => {
                let converter = MarkdownConverter::new();
                let clean_abstract_md = converter
                    .convert(&abstract_text, options.strip_images, options.strip_links)?;

                let mut md = String::new();
                if options.frontmatter && options.format == OutputFormat::Markdown {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("arxiv_id: \"{arxiv_id}\"\n"));
                    md.push_str(&format!("authors: \"{}\"\n", author_str.replace('"', "\\\"")));
                    md.push_str(&format!("pdf_url: \"{pdf_url}\"\n"));
                    if let Some(ref d) = dateline {
                        md.push_str(&format!("submitted: \"{}\"\n", d.replace('"', "\\\"")));
                    }
                    if let Some(ref s) = subjects {
                        md.push_str(&format!("subjects: \"{}\"\n", s.replace('"', "\\\"")));
                    }
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {}\n\n", title));

                let mut meta_lines = Vec::new();
                meta_lines.push(format!("**ArXiv ID**: `{arxiv_id}`"));
                meta_lines.push(format!("**PDF Link**: [Download PDF]({pdf_url})"));
                meta_lines.push(format!("**Authors**: {author_str}"));

                if let Some(ref d) = dateline {
                    meta_lines.push(format!("**Submission**: {d}"));
                }
                if let Some(ref s) = subjects {
                    meta_lines.push(format!("**Subjects**: {s}"));
                }

                md.push_str(&meta_lines.iter().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n>\n"));
                md.push_str("\n\n## Abstract\n\n");
                md.push_str(clean_abstract_md.trim());
                md.push('\n');
                md
            }
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: original_url.to_string(),
            final_url: format!("https://arxiv.org/abs/{arxiv_id}"),
            status: 200,
            title: Some(title),
            byline: Some(author_str.clone()),
            content,
            metadata: PageMetadata {
                author: Some(author_str),
                published_time: dateline,
                canonical_url: Some(format!("https://arxiv.org/abs/{arxiv_id}")),
                description: Some(abstract_text),
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

impl PlatformAdapter for ArxivAdapter {
    fn name(&self) -> &'static str {
        "arxiv"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "arxiv.org")
            && (url.path().starts_with("/abs/")
                || url.path().starts_with("/pdf/")
                || url.path().starts_with("/html/"))
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

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arxiv_adapter_matching_and_id_extraction() {
        let adapter = ArxivAdapter::new();
        let u1 = Url::parse("https://arxiv.org/abs/2401.12345").unwrap();
        let u2 = Url::parse("https://arxiv.org/pdf/2401.12345.pdf").unwrap();
        let u3 = Url::parse("https://arxiv.org/html/2401.12345").unwrap();
        let u4 = Url::parse("https://evilarxiv.org/abs/2401.12345").unwrap();
        let u5 = Url::parse("https://arxiv.org/list/cs/recent").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(adapter.matches(&u3));
        assert!(!adapter.matches(&u4));
        assert!(!adapter.matches(&u5));

        assert_eq!(
            ArxivAdapter::extract_arxiv_id(&u1).as_deref(),
            Some("2401.12345")
        );
        assert_eq!(
            ArxivAdapter::extract_arxiv_id(&u2).as_deref(),
            Some("2401.12345")
        );
        assert_eq!(
            ArxivAdapter::extract_arxiv_id(&u3).as_deref(),
            Some("2401.12345")
        );
    }

    #[test]
    fn test_arxiv_parse_paper() {
        let adapter = ArxivAdapter::new();
        let sample_html = r##"
            <!DOCTYPE html>
            <html>
            <head><title>[2401.12345] Deep Residual Learning for Visual Recognition</title></head>
            <body>
                <h1 class="title mathjax"><span class="descriptor">Title:</span>Deep Residual Learning for Visual Recognition</h1>
                <div class="authors"><span class="descriptor">Authors:</span><a href="#">Kaiming He</a>, <a href="#">Xiangyu Zhang</a>, <a href="#">Shaoqing Ren</a>, <a href="#">Jian Sun</a></div>
                <blockquote class="abstract mathjax"><span class="descriptor">Abstract:</span>Deeper neural networks are more difficult to train. We present a residual learning framework to ease the training of networks that are substantially deeper than those used previously.</blockquote>
                <td class="tablecell subjects"><span class="primary-subject">Computer Vision and Pattern Recognition (cs.CV)</span>; Artificial Intelligence (cs.AI)</td>
                <div class="dateline">[v1] Thu, 10 Dec 2015 17:34:57 UTC</div>
            </body>
            </html>
        "##;

        let url = Url::parse("https://arxiv.org/abs/1512.03385").unwrap();
        let doc = adapter
            .parse_paper(
                sample_html,
                &url,
                "1512.03385",
                &ExtractionOptions::default(),
            )
            .unwrap();

        assert_eq!(
            doc.title.as_deref(),
            Some("Deep Residual Learning for Visual Recognition")
        );
        assert!(doc
            .content
            .contains("Kaiming He, Xiangyu Zhang, Shaoqing Ren, Jian Sun"));
        assert!(doc.content.contains("https://arxiv.org/pdf/1512.03385.pdf"));
        assert!(doc
            .content
            .contains("Deeper neural networks are more difficult to train."));
        assert!(doc
            .content
            .contains("Computer Vision and Pattern Recognition (cs.CV)"));
    }
}
