use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::PageclawError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static GITHUB_README_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("article.markdown-body, #readme article, div[data-target='readme.content']")
        .unwrap()
});
static GITHUB_REPO_DESC_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".f4.my-3, p.f4, meta[name='description']").unwrap());
static GITHUB_ISSUE_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1.gh-header-title, bdi.js-issue-title").unwrap());
static GITHUB_ISSUE_STATE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("span[title^='Status:'], .State, span.IssueLabel").unwrap());
static GITHUB_COMMENT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".timeline-comment-group, .timeline-comment").unwrap());

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubRoute {
    /// Repository root or README view (e.g. /owner/repo or /owner/repo/blob/.../README.md)
    Repo { owner: String, repo: String },
    /// Issue or Pull Request discussion thread (/owner/repo/issues/123 or /owner/repo/pull/123)
    Issue {
        owner: String,
        repo: String,
        number: String,
        is_pr: bool,
    },
    /// Release notes (/owner/repo/releases)
    Releases { owner: String, repo: String },
}

pub struct GithubAdapter;

impl Default for GithubAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GithubAdapter {
    pub fn new() -> Self {
        Self
    }

    pub fn parse_route(url: &Url) -> Option<GithubRoute> {
        let host = url.host_str().unwrap_or("");
        if !crate::fetcher::is_subdomain_or_exact(host, "github.com") {
            return None;
        }

        let segments: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() < 2 {
            return None;
        }

        let owner = segments[0].to_string();
        let repo = segments[1].to_string();

        if segments.len() == 2 {
            return Some(GithubRoute::Repo { owner, repo });
        }

        match segments[2] {
            "issues" => {
                if let Some(num) = segments.get(3) {
                    Some(GithubRoute::Issue {
                        owner,
                        repo,
                        number: num.to_string(),
                        is_pr: false,
                    })
                } else {
                    Some(GithubRoute::Repo { owner, repo })
                }
            }
            "pull" => {
                if let Some(num) = segments.get(3) {
                    Some(GithubRoute::Issue {
                        owner,
                        repo,
                        number: num.to_string(),
                        is_pr: true,
                    })
                } else {
                    Some(GithubRoute::Repo { owner, repo })
                }
            }
            "releases" => Some(GithubRoute::Releases { owner, repo }),
            _ => Some(GithubRoute::Repo { owner, repo }),
        }
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        debug!(url = %url, "Executing GitHub adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Referer", "https://github.com/")
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
    ) -> Result<PageDocument, PageclawError> {
        let route = Self::parse_route(url).unwrap_or(GithubRoute::Repo {
            owner: "GitHub".into(),
            repo: "Repository".into(),
        });

        match route {
            GithubRoute::Repo { owner, repo } => {
                self.extract_repo_readme(raw_html, &owner, &repo, url, options)
            }
            GithubRoute::Issue {
                owner,
                repo,
                number,
                is_pr,
            } => {
                self.extract_issue_discussion(raw_html, &owner, &repo, &number, is_pr, url, options)
            }
            GithubRoute::Releases { owner, repo } => {
                self.extract_repo_readme(raw_html, &owner, &repo, url, options)
            }
        }
    }

    fn extract_repo_readme(
        &self,
        raw_html: &str,
        owner: &str,
        repo: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let doc = Html::parse_document(raw_html);
        let repo_title = format!("{owner}/{repo}");

        let desc = doc
            .select(&GITHUB_REPO_DESC_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .or_else(|| extract_meta_name(&doc, "description"));

        let readme_html = doc
            .select(&GITHUB_README_SELECTOR)
            .next()
            .map(|el| el.html())
            .unwrap_or_else(|| {
                if let Ok(cleaned) =
                    crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)
                {
                    cleaned.html
                } else {
                    raw_html.to_string()
                }
            });

        let converter = MarkdownConverter::new();
        let md_body = converter.convert(&readme_html, options.strip_images, options.strip_links)?;

        let mut content = String::new();
        if options.frontmatter && options.format == OutputFormat::Markdown {
            content.push_str("---\n");
            content.push_str(&format!("repo: \"{repo_title}\"\n"));
            content.push_str(&format!("url: \"{url}\"\n"));
            if let Some(ref d) = desc {
                content.push_str(&format!("description: \"{}\"\n", d.replace('"', "\\\"")));
            }
            content.push_str("---\n\n");
        }

        content.push_str(&format!("# {}\n\n", repo_title));
        if let Some(ref d) = desc {
            if !d.is_empty() {
                content.push_str(&format!("> {}\n\n", d));
            }
        }
        content.push_str(&md_body);

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(repo_title.clone()),
            byline: Some(owner.to_string()),
            content,
            metadata: PageMetadata {
                description: desc,
                author: Some(owner.to_string()),
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
        })
    }

    fn extract_issue_discussion(
        &self,
        raw_html: &str,
        owner: &str,
        repo: &str,
        number: &str,
        is_pr: bool,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let doc = Html::parse_document(raw_html);
        let kind_label = if is_pr { "Pull Request" } else { "Issue" };

        let title = doc
            .select(&GITHUB_ISSUE_TITLE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| format!("{owner}/{repo} #{number}"));

        let state = doc
            .select(&GITHUB_ISSUE_STATE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "Open".into());

        let converter = MarkdownConverter::new();
        let mut content = String::new();

        content.push_str(&format!("# [{kind_label} #{number}] {}\n\n", title));
        content.push_str(&format!(
            "> **状态**: {} | **仓库**: {}/{}\n\n",
            state, owner, repo
        ));

        let author_sel = Selector::parse(".author, strong a.author").unwrap();
        let body_sel = Selector::parse(".comment-body, .edit-comment-hide").unwrap();

        let mut comment_count = 0;
        for (idx, comment_el) in doc.select(&GITHUB_COMMENT_SELECTOR).enumerate() {
            let author = comment_el
                .select(&author_sel)
                .next()
                .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .unwrap_or_else(|| "github-user".into());

            if let Some(body_el) = comment_el.select(&body_sel).next() {
                let md = converter.convert(
                    &body_el.html(),
                    options.strip_images,
                    options.strip_links,
                )?;
                if !md.trim().is_empty() {
                    comment_count += 1;
                    if idx == 0 {
                        content.push_str(&format!("## 描述 (by @{author}):\n\n"));
                    } else {
                        content.push_str(&format!("## 回复 #{} (by @{author}):\n\n", idx));
                    }
                    content.push_str(md.trim());
                    content.push_str("\n\n---\n\n");
                }
            }
        }

        if comment_count == 0 {
            // Fallback: Readability extraction if comments timeline layout changed
            if let Ok(cleaned) =
                crate::cleaner::clean_html(raw_html, Some(url.as_str()), options.mode)
            {
                let md =
                    converter.convert(&cleaned.html, options.strip_images, options.strip_links)?;
                content.push_str(&md);
            }
        }

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(format!("#{number} {title}")),
            byline: Some(owner.to_string()),
            content,
            metadata: PageMetadata {
                author: Some(owner.to_string()),
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
        })
    }
}

impl PlatformAdapter for GithubAdapter {
    fn name(&self) -> &'static str {
        "github"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "github.com")
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

fn extract_meta_name(doc: &Html, name: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[name='{name}']")).ok()?;
    let el = doc.select(&selector).next()?;
    el.value().attr("content").map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_adapter_matching_and_route() {
        let adapter = GithubAdapter::new();
        let u1 = Url::parse("https://github.com/tokio-rs/tokio").unwrap();
        let u2 = Url::parse("https://github.com/tokio-rs/tokio/issues/1234").unwrap();
        let u3 = Url::parse("https://github.com/tokio-rs/tokio/pull/5678").unwrap();
        let u4 = Url::parse("https://evilgithub.com/tokio-rs/tokio").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(adapter.matches(&u3));
        assert!(!adapter.matches(&u4));

        assert_eq!(
            GithubAdapter::parse_route(&u1),
            Some(GithubRoute::Repo {
                owner: "tokio-rs".into(),
                repo: "tokio".into()
            })
        );
        assert_eq!(
            GithubAdapter::parse_route(&u2),
            Some(GithubRoute::Issue {
                owner: "tokio-rs".into(),
                repo: "tokio".into(),
                number: "1234".into(),
                is_pr: false
            })
        );
    }

    #[test]
    fn test_github_extract_readme() {
        let adapter = GithubAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>tokio-rs/tokio</title></head>
            <body>
                <h1 class="public"><strong itemprop="name"><a href="/tokio-rs/tokio">tokio</a></strong></h1>
                <p class="f4 my-3">A runtime for writing reliable, asynchronous applications with Rust.</p>
                <article class="markdown-body">
                    <h2>Overview</h2>
                    <p>Tokio provides event-driven non-blocking I/O.</p>
                </article>
            </body>
            </html>
        "#;

        let url = Url::parse("https://github.com/tokio-rs/tokio").unwrap();
        let doc = adapter
            .parse_page(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("tokio-rs/tokio"));
        assert!(doc.content.contains("# tokio-rs/tokio"));
        assert!(doc.content.contains("A runtime for writing reliable"));
        assert!(doc.content.contains("## Overview"));
        assert!(doc.content.contains("Tokio provides event-driven"));
    }
}
