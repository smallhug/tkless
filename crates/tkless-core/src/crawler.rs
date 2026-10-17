use crate::error::TklessError;
use crate::model::{ExtractionOptions, OutputFormat};
use crate::TklessEngine;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::sync::Semaphore;
use tracing::{debug, warn};
use url::Url;

static A_HREF_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a[href]").unwrap());

/// Configuration options for deep site crawling and sitemap-driven extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlOptions {
    /// Maximum link traversal depth (0: only seed URL, 1: direct child links, default: 2).
    pub max_depth: usize,
    /// Maximum number of unique pages to crawl [default: 20].
    pub max_pages: usize,
    /// Concurrency limit for simultaneous network requests [default: 5].
    pub concurrency: usize,
    /// Restrict crawling strictly to the same domain/origin as the seed URL [default: true].
    pub same_domain_only: bool,
    /// Restrict crawling strictly to URLs sharing the same path prefix as seed URL.
    pub same_path_prefix: bool,
    /// Regular expression patterns for excluding URLs (e.g. `\.(pdf|zip|png)$` or `/login`).
    pub exclude_patterns: Vec<String>,
    /// Whether to force sitemap XML parsing or automatically treat XML endpoints as sitemaps.
    pub sitemap: bool,
    /// Automatically skip near-duplicate pages based on 64-bit SimHash Hamming distance.
    pub dedup: bool,
    /// Delay between consecutive page requests in milliseconds to respect rate limits [default: 0].
    pub delay_ms: u64,
}

impl Default for CrawlOptions {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_pages: 20,
            concurrency: 5,
            same_domain_only: true,
            same_path_prefix: false,
            exclude_patterns: vec![
                r#"\.(png|jpg|jpeg|gif|svg|ico|webp|pdf|zip|tar|gz|exe|dmg|mp4|mp3|css|js|woff|woff2)$"#.to_string(),
                r#"/(login|signin|signup|logout|auth|register)"#.to_string(),
            ],
            sitemap: false,
            dedup: false,
            delay_ms: 0,
        }
    }
}

/// A single page crawled and extracted during the traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledPage {
    pub rank: usize,
    pub url: String,
    pub title: String,
    pub depth: usize,
    pub content: String,
    pub token_count: usize,
    pub status: u16,
    pub error: Option<String>,
}

/// The complete assembled site knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteCrawlReport {
    pub seed_url: String,
    pub total_pages: usize,
    pub total_tokens: usize,
    pub pages: Vec<CrawledPage>,
}

/// Executes a breadth-first deep site crawl starting from `seed_url`,
/// extracting and cleaning Markdown content for each discovered page.
pub async fn crawl_site(
    engine: &TklessEngine,
    seed_url: &str,
    crawl_options: &CrawlOptions,
    extraction_options: &ExtractionOptions,
) -> Result<SiteCrawlReport, TklessError> {
    crawl_site_with_progress::<fn(usize, usize, &str)>(
        engine,
        seed_url,
        crawl_options,
        extraction_options,
        None,
    )
    .await
}

/// Executes deep site crawling with an optional real-time progress callback `F(crawled_count, max_pages, current_url)`.
pub async fn crawl_site_with_progress<F>(
    engine: &TklessEngine,
    seed_url: &str,
    crawl_options: &CrawlOptions,
    extraction_options: &ExtractionOptions,
    mut on_progress: Option<F>,
) -> Result<SiteCrawlReport, TklessError>
where
    F: FnMut(usize, usize, &str) + Send,
{
    let parsed_seed = Url::parse(seed_url)?;
    let seed_host = parsed_seed.host_str().unwrap_or("").to_lowercase();
    let seed_path = parsed_seed.path().to_string();

    let exclude_regexes: Vec<regex::Regex> = crawl_options
        .exclude_patterns
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect();

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(Url, usize)> = VecDeque::new();

    // Check if seed URL is a sitemap XML or if sitemap mode is enabled
    let is_sitemap_target = crawl_options.sitemap
        || seed_url.ends_with(".xml")
        || parsed_seed.path().ends_with("sitemap")
        || parsed_seed.path().ends_with("sitemap.xml");

    if is_sitemap_target {
        debug!(seed = seed_url, "Sitemap 模式：获取并解析站点地图 XML");
        if let Ok(resp) = engine.fetcher().fetch(seed_url).await {
            let mut extracted_urls = parse_sitemap_urls(&resp.html);

            // Handle sitemapindex by fetching child sitemaps (first 3)
            if resp.html.contains("<sitemapindex") {
                let child_sitemaps = extracted_urls.clone();
                extracted_urls.clear();
                for child_map in child_sitemaps.into_iter().take(3) {
                    if let Ok(child_resp) = engine.fetcher().fetch(&child_map).await {
                        let child_urls = parse_sitemap_urls(&child_resp.html);
                        extracted_urls.extend(child_urls);
                    }
                }
            }

            for loc in extracted_urls {
                if let Ok(parsed_loc) = Url::parse(&loc) {
                    if should_enqueue_url(
                        &parsed_loc,
                        &seed_host,
                        &seed_path,
                        crawl_options,
                        &exclude_regexes,
                    ) {
                        let norm = normalize_url(&parsed_loc);
                        if visited.insert(norm) {
                            queue.push_back((parsed_loc, 1));
                        }
                    }
                }
                if queue.len() >= crawl_options.max_pages {
                    break;
                }
            }
        }
    }

    if queue.is_empty() {
        let normalized_seed_str = normalize_url(&parsed_seed);
        visited.insert(normalized_seed_str);
        queue.push_back((parsed_seed.clone(), 0));
    }

    let mut crawled_pages: Vec<CrawledPage> = Vec::new();
    let mut seen_simhashes: Vec<u64> = Vec::new();
    let semaphore = Arc::new(Semaphore::new(crawl_options.concurrency.max(1)));

    while let Some((current_url, depth)) = queue.pop_front() {
        if crawled_pages.len() >= crawl_options.max_pages {
            debug!(
                limit = crawl_options.max_pages,
                "Reached max_pages limit, halting crawl"
            );
            break;
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let eng = engine.clone();
        let opts = extraction_options.clone();
        let url_str = current_url.to_string();
        let rank = crawled_pages.len() + 1;

        debug!(url = %url_str, depth = depth, rank = rank, "Crawling page");

        let fetch_res = eng.process_url(&url_str, &opts).await;
        drop(permit);

        match fetch_res {
            Ok(doc) => {
                // If deduplication is enabled, check for near-duplicates via SimHash
                if crawl_options.dedup {
                    let page_hash = crate::dedup::simhash(&doc.content);
                    if seen_simhashes
                        .iter()
                        .any(|&prev| crate::dedup::is_near_duplicate(prev, page_hash, 3))
                    {
                        debug!(url = %url_str, "跳过内容高度近似的重复页面 (SimHash near-duplicate)");
                        continue;
                    }
                    seen_simhashes.push(page_hash);
                }

                let page_title = doc.title.clone().unwrap_or_else(|| url_str.clone());

                // Discover child links if within depth limit
                if depth < crawl_options.max_depth && crawled_pages.len() < crawl_options.max_pages
                {
                    if let Ok(raw_resp) = eng
                        .process_url(
                            &url_str,
                            &ExtractionOptions {
                                format: OutputFormat::Raw,
                                ..Default::default()
                            },
                        )
                        .await
                    {
                        let links = extract_discovered_links(&raw_resp.content, &current_url);
                        for child_url in links {
                            if should_enqueue_url(
                                &child_url,
                                &seed_host,
                                &seed_path,
                                crawl_options,
                                &exclude_regexes,
                            ) {
                                let norm_child = normalize_url(&child_url);
                                if visited.insert(norm_child) {
                                    queue.push_back((child_url, depth + 1));
                                }
                            }
                        }
                    }
                }

                crawled_pages.push(CrawledPage {
                    rank,
                    url: url_str.clone(),
                    title: page_title,
                    depth,
                    content: doc.content,
                    token_count: doc.token_count,
                    status: doc.status,
                    error: None,
                });
                if let Some(ref mut cb) = on_progress {
                    cb(crawled_pages.len(), crawl_options.max_pages, &url_str);
                }
            }
            Err(err) => {
                warn!(url = %url_str, error = %err, "Failed to crawl page");
                crawled_pages.push(CrawledPage {
                    rank,
                    url: url_str.clone(),
                    title: url_str.clone(),
                    depth,
                    content: String::new(),
                    token_count: 0,
                    status: 0,
                    error: Some(err.to_string()),
                });
                if let Some(ref mut cb) = on_progress {
                    cb(crawled_pages.len(), crawl_options.max_pages, &url_str);
                }
            }
        }

        if crawl_options.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(crawl_options.delay_ms)).await;
        }
    }

    let total_tokens: usize = crawled_pages.iter().map(|p| p.token_count).sum();

    Ok(SiteCrawlReport {
        seed_url: seed_url.to_string(),
        total_pages: crawled_pages.len(),
        total_tokens,
        pages: crawled_pages,
    })
}

/// Normalizes URL by removing fragment and trailing slashes for deduplication.
fn normalize_url(url: &Url) -> String {
    let mut cloned = url.clone();
    cloned.set_fragment(None);
    let mut s = cloned.to_string();
    if s.ends_with('/') && s != "http://" && s != "https://" {
        s.pop();
    }
    s
}

/// Evaluates if a discovered child URL satisfies domain, path, and exclusion filters.
fn should_enqueue_url(
    child: &Url,
    seed_host: &str,
    seed_path: &str,
    options: &CrawlOptions,
    exclude_regexes: &[regex::Regex],
) -> bool {
    // Only HTTP/HTTPS
    if child.scheme() != "http" && child.scheme() != "https" {
        return false;
    }

    let child_host = child.host_str().unwrap_or("").to_lowercase();

    // Same domain filter
    if options.same_domain_only
        && child_host != seed_host
        && !crate::fetcher::is_subdomain_or_exact(&child_host, seed_host)
    {
        return false;
    }

    // Path prefix filter (e.g. only crawl inside /docs/)
    if options.same_path_prefix {
        let child_path = child.path();
        if !child_path.starts_with(seed_path) {
            return false;
        }
    }

    // Exclusion patterns filter
    let child_str = child.as_str();
    for re in exclude_regexes {
        if re.is_match(child_str) {
            return false;
        }
    }

    true
}

/// Extracts all candidate hyperlinks from raw HTML content and resolves them to absolute URLs.
fn extract_discovered_links(html: &str, base_url: &Url) -> Vec<Url> {
    let doc = Html::parse_fragment(html);
    let mut links = Vec::new();

    for el in doc.select(&A_HREF_SELECTOR) {
        if let Some(href) = el.value().attr("href") {
            let trimmed = href.trim();
            if trimmed.is_empty()
                || trimmed.starts_with('#')
                || trimmed.starts_with("javascript:")
                || trimmed.starts_with("mailto:")
            {
                continue;
            }

            if let Ok(resolved) = base_url.join(trimmed) {
                links.push(resolved);
            }
        }
    }

    links
}

/// Extracts all canonical `<loc>` URLs from a Sitemap XML (`<urlset>` or `<sitemapindex>`).
pub fn parse_sitemap_urls(xml_content: &str) -> Vec<String> {
    static LOC_REGEX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)<loc>\s*(https?://[^\s<]+)\s*</loc>").unwrap());

    let mut urls = Vec::new();
    for caps in LOC_REGEX.captures_iter(xml_content) {
        if let Some(matched) = caps.get(1) {
            let u = matched.as_str().trim();
            if !u.is_empty() {
                urls.push(u.to_string());
            }
        }
    }
    urls
}

/// Formats the crawled pages report into an assembled single-file Markdown knowledge base with TOC.
pub fn format_crawl_report_markdown(report: &SiteCrawlReport) -> String {
    let mut out = String::new();

    out.push_str(&format!("# 整站知识库汇编: {}\n\n", report.seed_url));
    out.push_str(&format!(
        "> 爬取页面总数: {} | 累计估算 Tokens: {}\n\n",
        report.total_pages, report.total_tokens
    ));

    out.push_str("## 📚 知识库目录索引 (Table of Contents)\n\n");
    for page in &report.pages {
        let indent = "  ".repeat(page.depth);
        out.push_str(&format!(
            "{}- [{}](#章节-{}-{}) (Tokens: {})\n",
            indent,
            page.title.trim(),
            page.rank,
            slugify(&page.title),
            page.token_count
        ));
    }
    out.push_str("\n---\n\n");

    for page in &report.pages {
        out.push_str(&format!(
            "<a id=\"章节-{}-{}\"></a>\n\n## 章节 {}. [{}]({})\n\n",
            page.rank,
            slugify(&page.title),
            page.rank,
            page.title.trim(),
            page.url
        ));
        out.push_str(&format!(
            "> 来源链接: {} | 爬取深度: {}\n\n",
            page.url, page.depth
        ));

        if page.content.trim().is_empty() {
            if let Some(ref err) = page.error {
                out.push_str(&format!("> *[页面提取异常: {err}]*\n\n"));
            } else {
                out.push_str("> *[无有效正文内容]*\n\n");
            }
        } else {
            out.push_str(page.content.trim());
            out.push_str("\n\n");
        }

        out.push_str("---\n\n");
    }

    out
}

/// Formats the crawled pages report into line-delimited JSON (JSONL / ndjson)
/// for streaming ingestion into vector databases and RAG knowledge pipelines.
pub fn format_crawl_report_jsonl(report: &SiteCrawlReport) -> String {
    let mut out = String::new();
    for page in &report.pages {
        if let Ok(line) = serde_json::to_string(page) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn slugify(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_url() {
        let u1 = Url::parse("https://docs.rs/tokio/latest/tokio/#functions").unwrap();
        let u2 = Url::parse("https://docs.rs/tokio/latest/tokio/").unwrap();

        assert_eq!(normalize_url(&u1), "https://docs.rs/tokio/latest/tokio");
        assert_eq!(normalize_url(&u2), "https://docs.rs/tokio/latest/tokio");
    }

    #[test]
    fn test_should_enqueue_url() {
        let options = CrawlOptions {
            same_domain_only: true,
            same_path_prefix: true,
            ..Default::default()
        };
        let exclude_regexes = vec![regex::Regex::new(r#"\.png$"#).unwrap()];

        let seed_host = "tokio.rs";
        let seed_path = "/tokio/tutorial";

        let valid_child = Url::parse("https://tokio.rs/tokio/tutorial/hello-world").unwrap();
        let diff_path_child = Url::parse("https://tokio.rs/blog/2026-news").unwrap();
        let diff_domain_child = Url::parse("https://github.com/tokio-rs/tokio").unwrap();
        let png_child = Url::parse("https://tokio.rs/tokio/tutorial/arch.png").unwrap();

        assert!(should_enqueue_url(
            &valid_child,
            seed_host,
            seed_path,
            &options,
            &exclude_regexes
        ));
        assert!(!should_enqueue_url(
            &diff_path_child,
            seed_host,
            seed_path,
            &options,
            &exclude_regexes
        ));
        assert!(!should_enqueue_url(
            &diff_domain_child,
            seed_host,
            seed_path,
            &options,
            &exclude_regexes
        ));
        assert!(!should_enqueue_url(
            &png_child,
            seed_host,
            seed_path,
            &options,
            &exclude_regexes
        ));
    }

    #[test]
    fn test_format_crawl_report_markdown() {
        let report = SiteCrawlReport {
            seed_url: "https://example.com/docs".to_string(),
            total_pages: 2,
            total_tokens: 350,
            pages: vec![
                CrawledPage {
                    rank: 1,
                    url: "https://example.com/docs".to_string(),
                    title: "Documentation Overview".to_string(),
                    depth: 0,
                    content: "# Overview\nWelcome to docs.".to_string(),
                    token_count: 150,
                    status: 200,
                    error: None,
                },
                CrawledPage {
                    rank: 2,
                    url: "https://example.com/docs/getting-started".to_string(),
                    title: "Getting Started".to_string(),
                    depth: 1,
                    content: "# Quickstart\nInstall and run.".to_string(),
                    token_count: 200,
                    status: 200,
                    error: None,
                },
            ],
        };

        let md = format_crawl_report_markdown(&report);
        assert!(md.contains("# 整站知识库汇编: https://example.com/docs"));
        assert!(md.contains("爬取页面总数: 2"));
        assert!(md.contains("## 📚 知识库目录索引 (Table of Contents)"));
        assert!(md.contains("Documentation Overview"));
        assert!(md.contains("Getting Started"));
        assert!(md.contains("## 章节 1. [Documentation Overview]"));
        assert!(md.contains("## 章节 2. [Getting Started]"));
    }

    #[test]
    fn test_parse_sitemap_urls() {
        let sample_sitemap = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url>
                <loc>https://docs.rs/tokio/latest/tokio/</loc>
                <lastmod>2026-03-01</lastmod>
            </url>
            <url>
                <loc>https://docs.rs/tokio/latest/tokio/sync/index.html</loc>
            </url>
        </urlset>"#;

        let urls = parse_sitemap_urls(sample_sitemap);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://docs.rs/tokio/latest/tokio/");
        assert_eq!(
            urls[1],
            "https://docs.rs/tokio/latest/tokio/sync/index.html"
        );
    }
}
