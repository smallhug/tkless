//! Papers With Code research paper, code implementations, and benchmark evaluation adapter.
//!
//! Intercepts `paperswithcode.com/paper/...`, `paperswithcode.com/task/...`, and `paperswithcode.com/method/...`,
//! extracting paper abstract, author list, arXiv / PDF direct links, GitHub code implementations with Stars,
//! and SOTA benchmark evaluation results tables.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::model::{ExtractionOptions, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static PWC_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1.paper-title, .paper-title h1, h1").unwrap());
static PWC_AUTHORS_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".author-span a, .paper-authors a, span.author-span").unwrap()
});
static PWC_ABSTRACT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".paper-abstract p, div.paper-abstract").unwrap());
static PWC_ARXIV_LINK_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "a.badge-dark[href*='arxiv.org'], a[href*='arxiv.org/abs'], a[href*='arxiv.org/pdf']",
    )
    .unwrap()
});
static PWC_CODE_ROW_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".paper-implementations tr, #implementations-short-list .row, div.entity")
        .unwrap()
});
static PWC_CODE_LINK_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.code-table-link, a[href*='github.com']").unwrap());
static PWC_CODE_STARS_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".stars-accumulated, .badge-stars, span.stars").unwrap());

pub struct PapersWithCodeAdapter;

impl Default for PapersWithCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PapersWithCodeAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Parses a Papers With Code paper HTML document into structured Markdown.
    pub fn parse_paper_html(html: &str, page_url: &str) -> (String, String, Option<String>) {
        let doc = Html::parse_document(html);

        let title = doc
            .select(&PWC_TITLE_SELECTOR)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| "Papers With Code Research Paper".to_string());

        let mut authors: Vec<String> = Vec::new();
        for el in doc.select(&PWC_AUTHORS_SELECTOR) {
            let name = el.text().collect::<String>().trim().to_string();
            if !name.is_empty() && !authors.contains(&name) {
                authors.push(name);
            }
        }

        let abstract_text = doc
            .select(&PWC_ABSTRACT_SELECTOR)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        let arxiv_url = doc
            .select(&PWC_ARXIV_LINK_SELECTOR)
            .next()
            .and_then(|e| e.value().attr("href"))
            .unwrap_or("")
            .trim();

        let authors_line = if authors.is_empty() {
            "未知".to_string()
        } else {
            authors.join(", ")
        };

        let mut out = String::new();
        out.push_str(&format!("# 📄 {title}\n\n"));
        out.push_str(&format!("> 👥 **作者**: {authors_line}\n"));
        if !arxiv_url.is_empty() {
            out.push_str(&format!(
                "> 📚 **ArXiv 原文**: [{arxiv_url}]({arxiv_url})\n"
            ));
        }
        out.push_str(&format!(
            "> 🔗 **PWC 页面**: [{page_url}]({page_url})\n\n---\n\n"
        ));

        if !abstract_text.is_empty() {
            out.push_str("### 📋 论文摘要 (Abstract)\n\n");
            out.push_str(&abstract_text);
            out.push_str("\n\n---\n\n");
        }

        // Code Implementations
        let mut code_repos = Vec::new();
        for row in doc.select(&PWC_CODE_ROW_SELECTOR) {
            if let Some(link_el) = row.select(&PWC_CODE_LINK_SELECTOR).next() {
                let repo_name = link_el.text().collect::<String>().trim().to_string();
                let repo_href = link_el
                    .value()
                    .attr("href")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let stars = row
                    .select(&PWC_CODE_STARS_SELECTOR)
                    .next()
                    .map(|e| e.text().collect::<String>().trim().to_string())
                    .unwrap_or_else(|| "-".to_string());

                if repo_href.contains("github.com") && !repo_name.is_empty() {
                    code_repos.push((repo_name, repo_href, stars));
                }
            }
        }

        if !code_repos.is_empty() {
            out.push_str("### 💻 开源代码实现 (Official / Community Implementations)\n\n");
            out.push_str("| 代码仓库 (GitHub) | Stars 关注度 |\n");
            out.push_str("|:---|:---|\n");
            for (name, href, stars) in &code_repos {
                out.push_str(&format!("| [{name}]({href}) | ⭐ {stars} |\n"));
            }
            out.push_str("\n---\n\n");
        }

        // Benchmark Evaluation tables
        let tables = crate::extractor::extract_tables(&doc);
        if !tables.is_empty() {
            out.push_str("### 📊 基准评测表现 (Benchmark Evaluation Results)\n\n");
            for t in &tables {
                out.push_str(&t.to_markdown());
                out.push('\n');
            }
        }

        (
            out,
            title,
            if authors.is_empty() {
                None
            } else {
                Some(authors_line)
            },
        )
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Papers With Code adapter extraction");

        let resp = fetcher.get(url).send().await?;
        let html = resp.text().await?;

        let (content, title, author) = Self::parse_paper_html(&html, url.as_str());
        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
            byline: author.clone(),
            content,
            metadata: PageMetadata {
                author,
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

impl PlatformAdapter for PapersWithCodeAdapter {
    fn name(&self) -> &'static str {
        "paperswithcode"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("").to_lowercase();
        crate::fetcher::is_subdomain_or_exact(&host, "paperswithcode.com")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pwc_matching() {
        let u1 =
            Url::parse("https://paperswithcode.com/paper/deepseek-v3-technical-report").unwrap();
        let adapter = PapersWithCodeAdapter::new();
        assert!(adapter.matches(&u1));

        let u2 = Url::parse("https://github.com/deepseek-ai").unwrap();
        assert!(!adapter.matches(&u2));
    }

    #[test]
    fn test_pwc_parse_paper_html() {
        let html = r#"
        <h1 class="paper-title">DeepSeek-V3 Technical Report</h1>
        <div class="paper-authors">
            <span class="author-span"><a href="/author/deepseek">DeepSeek-AI</a></span>
        </div>
        <a class="badge-dark" href="https://arxiv.org/abs/2412.19437">arXiv</a>
        <div class="paper-abstract">
            <p>We present DeepSeek-V3, a strong Mixture-of-Experts language model.</p>
        </div>
        <div class="paper-implementations">
            <table>
                <tr>
                    <td><a class="code-table-link" href="https://github.com/deepseek-ai/DeepSeek-V3">deepseek-ai/DeepSeek-V3</a></td>
                    <td><span class="stars-accumulated">45.2k</span></td>
                </tr>
            </table>
        </div>
        "#;

        let (md, title, author) = PapersWithCodeAdapter::parse_paper_html(
            html,
            "https://paperswithcode.com/paper/deepseek-v3",
        );
        assert_eq!(title, "DeepSeek-V3 Technical Report");
        assert_eq!(author, Some("DeepSeek-AI".to_string()));
        assert!(md.contains("DeepSeek-V3 Technical Report"));
        assert!(md.contains("ArXiv") || md.contains("arXiv"));
        assert!(md.contains("deepseek-ai/DeepSeek-V3"));
        assert!(md.contains("45.2k"));
    }
}
