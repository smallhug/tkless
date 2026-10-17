//! Hugging Face Hub (Model Cards, Papers, Datasets, and Blog) content adapter.
//!
//! Intercepts `huggingface.co/papers/...`, `huggingface.co/blog/...`, `huggingface.co/{org}/{model}`,
//! and `huggingface.co/datasets/{org}/{dataset}`, extracting clean model cards, research discussions,
//! paper abstracts, and technical blog posts without React navigation boilerplate.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static HF_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1, h1 a, meta[property='og:title']").unwrap());

static HF_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "a[href*='/papers?author='], a.flex.items-center span, .author, meta[name='author']",
    )
    .unwrap()
});

static HF_PROSE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("article.prose, .blog-content, section.prose, [data-target='ModelCard'], div.prose, main article").unwrap()
});

static HF_TAG_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("a[href*='pipeline_tag='], a[href*='dataset_tag='], .tag, a.tag, span.tag")
        .unwrap()
});

static HF_GITHUB_LINK_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href*='github.com']").unwrap());

pub struct HuggingfaceAdapter;

impl Default for HuggingfaceAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HuggingfaceAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Hugging Face adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 TkLess/0.1",
            )
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            )
            .send()
            .await?;

        let html = resp.text().await?;
        self.parse_page(&html, url, options)
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
            .select(&HF_TITLE_SELECTOR)
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
            .unwrap_or_else(|| "Hugging Face Resource".to_string());

        // 2. Extract Authors / Org
        let mut authors = Vec::new();
        for el in doc.select(&HF_AUTHOR_SELECTOR) {
            let name = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
            if !name.is_empty() && !authors.contains(&name) {
                authors.push(name);
            }
        }
        let author_str = if authors.is_empty() {
            "Hugging Face Community".to_string()
        } else {
            authors.join(", ")
        };

        // 3. Extract Tags / Tasks
        let mut tags = Vec::new();
        for el in doc.select(&HF_TAG_SELECTOR) {
            let t = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
            if !t.is_empty() && !tags.contains(&t) {
                tags.push(t);
            }
        }

        // 4. Extract GitHub reference if present
        let github_url = doc
            .select(&HF_GITHUB_LINK_SELECTOR)
            .next()
            .and_then(|el| el.value().attr("href").map(|s| s.trim().to_string()));

        // 5. Extract Main Content
        let body_html = if let Some(content_el) = doc.select(&HF_PROSE_SELECTOR).next() {
            content_el.html()
        } else {
            let cleaned = crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)?;
            cleaned.html
        };

        // 6. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => hf_html_to_plain_text(&body_html),
            OutputFormat::Markdown | OutputFormat::Json => {
                let converter = MarkdownConverter::new();
                let md_body =
                    converter.convert(&body_html, options.strip_images, options.strip_links)?;

                let mut md = String::new();
                if options.frontmatter && options.format == OutputFormat::Markdown {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("url: \"{url}\"\n"));
                    if !author_str.is_empty() {
                        md.push_str(&format!(
                            "author: \"{}\"\n",
                            author_str.replace('"', "\\\"")
                        ));
                    }
                    if let Some(ref gh) = github_url {
                        md.push_str(&format!("github: \"{gh}\"\n"));
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
                if !author_str.is_empty() {
                    header_parts.push(format!("**Author / Org**: {author_str}"));
                }
                if let Some(ref gh) = github_url {
                    header_parts.push(format!("**GitHub**: [{gh}]({gh})"));
                }
                if !tags.is_empty() {
                    header_parts.push(format!("**Tags**: {}", tags.join(", ")));
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
            byline: Some(author_str.clone()),
            content,
            metadata: PageMetadata {
                author: Some(author_str),
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

impl PlatformAdapter for HuggingfaceAdapter {
    fn name(&self) -> &'static str {
        "huggingface"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "huggingface.co")
            || crate::fetcher::is_subdomain_or_exact(host, "hf.co")
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

fn hf_html_to_plain_text(html: &str) -> String {
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
    fn test_huggingface_adapter_matching() {
        let adapter = HuggingfaceAdapter::new();
        let u1 = Url::parse("https://huggingface.co/papers/2401.12345").unwrap();
        let u2 = Url::parse("https://huggingface.co/blog/smolagents").unwrap();
        let u3 = Url::parse("https://huggingface.co/meta-llama/Llama-3.1-8B-Instruct").unwrap();
        let u4 = Url::parse("https://hf.co/deepseek-ai/DeepSeek-R1").unwrap();
        let u5 = Url::parse("https://evilhuggingface.co/model").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(adapter.matches(&u3));
        assert!(adapter.matches(&u4));
        assert!(!adapter.matches(&u5));
    }

    #[test]
    fn test_huggingface_parse_model_page() {
        let adapter = HuggingfaceAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>DeepSeek-R1 - Hugging Face</title></head>
            <body>
                <header>
                    <h1>DeepSeek-R1</h1>
                    <div class="author"><span>DeepSeek AI</span></div>
                    <a class="tag" href="/models?pipeline_tag=text-generation">text-generation</a>
                    <a href="https://github.com/deepseek-ai/DeepSeek-R1">GitHub Repo</a>
                </header>
                <article class="prose">
                    <h2>Model Summary</h2>
                    <p>DeepSeek-R1 is an open-source reasoning model trained with large-scale reinforcement learning.</p>
                    <pre><code class="language-python">from transformers import AutoModelForCausalLM</code></pre>
                </article>
            </body>
            </html>
        "#;

        let url = Url::parse("https://huggingface.co/deepseek-ai/DeepSeek-R1").unwrap();
        let doc = adapter
            .parse_page(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("DeepSeek-R1"));
        assert!(doc.content.contains("DeepSeek AI"));
        assert!(doc
            .content
            .contains("https://github.com/deepseek-ai/DeepSeek-R1"));
        assert!(doc.content.contains("text-generation"));
        assert!(doc
            .content
            .contains("DeepSeek-R1 is an open-source reasoning model"));
        assert!(doc
            .content
            .contains("from transformers import AutoModelForCausalLM"));
    }
}
