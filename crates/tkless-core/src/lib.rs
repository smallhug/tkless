//! tkless-core: High-performance, token-efficient web content extraction engine for AI agents.

#![recursion_limit = "256"]

pub mod adapters;
pub mod auth;
pub mod batch;
pub mod browser_workflow;
pub mod cache;
pub mod cdp;
pub mod chunker;
pub mod cleaner;
pub mod collection;
pub mod crawler;
pub mod dedup;
pub mod diff;
pub mod downloader;
pub mod error;
pub mod extractor;
pub mod feed;
pub mod fetcher;
pub mod limits;
pub mod markdown;
pub mod mcp;
pub mod model;
pub mod pdf;
pub mod research;
pub mod schema;
pub mod search;
mod search_preflight;
pub mod splitter;
mod state_file;
pub mod summarizer;
pub mod sync;
pub mod token;
pub mod wechat;
pub mod workflow;

pub use adapters::{
    format_adapters_markdown, AdapterInfo, AdapterRegistry, ArxivAdapter, BaikeAdapter,
    BilibiliAdapter, CsdnAdapter, DiscourseAdapter, GithubAdapter, HackerNewsAdapter,
    HuggingfaceAdapter, JuejinAdapter, MediumAdapter, PapersWithCodeAdapter, PlatformAdapter,
    RedditAdapter, SkillsAdapter, StackoverflowAdapter, SubstackAdapter, TwitterAdapter,
    V2exAdapter, WechatAdapter, WikipediaAdapter, XiaohongshuAdapter, YoutubeAdapter, ZhihuAdapter,
};
pub use auth::{
    execute_login_flow, generate_qrcode, poll_qrcode_status, LoginOptions, QrPollStatus,
};
pub use cache::{CacheStats, DocumentCache};
pub use cdp::{capture_screenshot_via_cdp, encode_base64, fetch_html_via_cdp, CdpOptions};
pub use chunker::chunk_markdown;
pub use crawler::{
    crawl_site, format_crawl_report_jsonl, format_crawl_report_markdown, CrawlOptions, CrawledPage,
    SiteCrawlReport,
};
pub use dedup::{hamming_distance, is_near_duplicate, simhash, similarity_ratio};
pub use diff::{compute_diff, DiffReport};
pub use downloader::{
    download_and_rewrite_markdown_images, download_file, format_bytes, DownloadOptions,
    DownloadProgressReporter,
};
pub use error::TklessError;
pub use feed::{format_feed_markdown, is_feed_xml, parse_feed, FeedChannel, FeedItem};
pub use mcp::McpServer;
pub use model::{
    CommentEntry, ConclusionChapter, ConclusionKeypoint, DanmakuEntry, DiagnosticWarning,
    ExtractedImage, ExtractedLink, ExtractedTable, ExtractionMetrics, ExtractionMode,
    ExtractionOptions, ExtractionProvenance, FetchOptions, OnTooLarge, OutputFormat, PageDocument,
    PageMetadata, SubtitleLine, TableExportFormat, TokenizerKind, VideoConclusion, VideoQuality,
};
pub use pdf::{
    extract_pdf_metadata, extract_pdf_to_document, extract_raw_text_from_pdf, is_pdf_content,
    is_pdf_path, is_pdf_url, structure_pdf_text_to_markdown,
};
pub use research::{
    deep_research, format_deep_research_markdown, DeepResearchReport, ResearchCitation,
    ResearchDimension, ResearchOptions, ResearchTelemetry,
};
pub use schema::{
    extract_entities, scaffold_dom_skeleton, ExtractionSchema, FieldDef, FieldDetail,
    ScaffoldReport,
};
pub use search::{
    fetch_search_results, format_search_fetch_markdown, format_search_results_markdown,
    normalize_url_for_dedup, web_search, SearchFetchItem, SearchOptions, SearchPageDiagnostics,
    SearchReport, SearchResultItem, SearchStatus,
};
pub use splitter::{export_sections_to_dir, split_markdown_by_headings, DocumentSection};
pub use summarizer::{extract_key_takeaways, summarize_markdown};

use cleaner::{clean_html, extract_selector};
pub use extractor::{
    extract_clean_cell_text, extract_images, extract_json_ld, extract_links, extract_metadata,
    extract_tables, format_images_markdown, format_links_markdown,
};
use fetcher::Fetcher;
use markdown::MarkdownConverter;
use scraper::Html;
use std::collections::HashSet;
use std::time::Instant;
use token::{count_tokens, truncate_to_tokens};
use tracing::{debug, warn};
use url::Url;

/// The central engine coordinating web fetching, sanitization, markdown conversion,
/// metadata extraction, and token budget management.
#[derive(Clone)]
pub struct TklessEngine {
    fetcher: Fetcher,
    markdown_converter: MarkdownConverter,
    adapter_registry: AdapterRegistry,
}

impl Default for TklessEngine {
    fn default() -> Self {
        Self::new().expect("Failed to initialize default TklessEngine")
    }
}

impl TklessEngine {
    /// Creates a new engine instance with default network options.
    pub fn new() -> Result<Self, TklessError> {
        Self::with_options(FetchOptions::default())
    }

    /// Creates a new engine instance with custom network and request options.
    pub fn with_options(fetch_options: FetchOptions) -> Result<Self, TklessError> {
        Self::with_options_and_adapters(fetch_options, AdapterRegistry::default())
    }

    /// Creates an engine instance with custom network options and a customized adapter registry.
    pub fn with_options_and_adapters(
        fetch_options: FetchOptions,
        adapter_registry: AdapterRegistry,
    ) -> Result<Self, TklessError> {
        let fetcher = Fetcher::new(fetch_options)?;
        let markdown_converter = MarkdownConverter::new();
        Ok(Self {
            fetcher,
            markdown_converter,
            adapter_registry,
        })
    }

    /// Access the underlying HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        self.fetcher.client()
    }

    /// Access the underlying Fetcher engine.
    pub fn fetcher(&self) -> &Fetcher {
        &self.fetcher
    }

    /// Cache identity includes request configuration and the current target's resolved credentials.
    pub fn cache_key(
        &self,
        cache: &DocumentCache,
        url: &str,
        options: &ExtractionOptions,
    ) -> String {
        let cookie = Url::parse(url)
            .ok()
            .and_then(|u| self.fetcher.resolve_cookie_for_url(&u));
        let scope = serde_json::to_vec(&(self.fetcher.options(), cookie))
            .expect("Fetch options are JSON serializable");
        cache.generate_scoped_key(url, options, &scope)
    }

    /// Fetches a remote URL and transforms it according to extraction options.
    /// Intercepts platform-specific URLs when a matching adapter exists unless disabled.
    pub async fn process_url(
        &self,
        url: &str,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        options.validate()?;
        let mut fallback_warnings = Vec::new();
        let parsed_url = Url::parse(url)?;
        let fetch_start = std::time::Instant::now();

        // Enforce SSRF validation upfront for all URLs before entering platform adapters or standard fetching
        self.fetcher.validate_ssrf(&parsed_url).await?;

        // 0. If no_adapter is explicitly requested and CDP is specified, go directly to CDP
        if options.no_adapter {
            if let Some(ref cdp_endpoint) = options.cdp_endpoint {
                debug!(
                    endpoint = %cdp_endpoint,
                    url = url,
                    "使用 CDP 无头浏览器渲染抓取页面 (no_adapter)"
                );
                return self
                    .fetch_via_cdp_and_process(url, &parsed_url, options, cdp_endpoint)
                    .await;
            }
        }

        // 1. Attempt platform adapter fast path if enabled (bypass if explicit DOM schema extraction is requested)
        if !options.no_adapter && options.schema.is_none() {
            if let Some(adapter) = self.adapter_registry.find_adapter(&parsed_url) {
                debug!(
                    platform = adapter.name(),
                    url = url,
                    "Intercepting with platform adapter"
                );
                match adapter.extract(&self.fetcher, &parsed_url, options).await {
                    Ok(mut doc) => {
                        doc.extraction.method = "platform_adapter".into();
                        doc.extraction.transport = "http".into();
                        doc.extraction.adapter = Some(adapter.name().into());
                        if options.summarize
                            && matches!(options.format, OutputFormat::Markdown | OutputFormat::Json)
                        {
                            doc.content = summarizer::summarize_markdown(&doc.content, 3);
                        }
                        // Enforce token counting and budget truncation
                        let initial_token_count = count_tokens(&doc.content, options.tokenizer);
                        if let Some(limit) = options.max_tokens {
                            if initial_token_count > limit {
                                let (truncated_content, was_truncated) =
                                    truncate_to_tokens(&doc.content, limit, options.tokenizer);
                                doc.content = truncated_content;
                                doc.truncated = was_truncated;
                            }
                        }
                        doc.token_count = if doc.truncated {
                            count_tokens(&doc.content, options.tokenizer)
                        } else {
                            initial_token_count
                        };
                        doc.tokenizer = options.tokenizer.as_str().to_string();
                        if options.chunk {
                            doc.chunks = Some(chunker::chunk_markdown(
                                &doc.content,
                                options.chunk_size.unwrap_or(500),
                                options.chunk_overlap.unwrap_or(50),
                                options.tokenizer,
                            ));
                        }
                        if options.download_images && !doc.content.is_empty() {
                            let assets_path = std::path::Path::new(
                                options.assets_dir.as_deref().unwrap_or("./assets"),
                            );
                            if let Ok(rewritten) = downloader::download_and_rewrite_markdown_images(
                                &self.fetcher,
                                &doc.content,
                                assets_path,
                                Some(url),
                            )
                            .await
                            {
                                doc.content = rewritten;
                            }
                        }
                        if options.tables_only {
                            if let Some(ref tables) = doc.tables {
                                doc.content =
                                    ExtractedTable::format_tables(tables, options.table_format);
                            }
                        }

                        // Ensure extract_links is honored on adapter pages
                        if options.extract_links {
                            let mut extracted = Vec::new();
                            let parsed_doc = Html::parse_document(&doc.content);
                            for l in extract_links(&parsed_doc, Some(url)) {
                                extracted.push(l);
                            }
                            // Also parse Markdown links: [text](url)
                            let md_link_re =
                                regex::Regex::new(r"\[([^\]]+)\]\(((?:https?://|/)[^\)]+)\)")
                                    .unwrap();
                            let mut seen_urls: HashSet<String> =
                                extracted.iter().map(|l| l.url.clone()).collect();
                            for cap in md_link_re.captures_iter(&doc.content) {
                                if let (Some(text), Some(u)) = (cap.get(1), cap.get(2)) {
                                    let resolved = if let Ok(base) = Url::parse(url) {
                                        base.join(u.as_str())
                                            .map(|j| j.to_string())
                                            .unwrap_or_else(|_| u.as_str().to_string())
                                    } else {
                                        u.as_str().to_string()
                                    };
                                    if seen_urls.insert(resolved.clone()) {
                                        extracted.push(ExtractedLink {
                                            text: text.as_str().trim().to_string(),
                                            url: resolved,
                                        });
                                    }
                                }
                            }
                            if !extracted.is_empty() {
                                doc.links = Some(extracted.clone());
                                if (options.format == OutputFormat::Markdown
                                    || options.format == OutputFormat::Text)
                                    && !doc.content.contains("## Links")
                                {
                                    doc.content.push_str(&format_links_markdown(&extracted));
                                }
                            }
                        }

                        // Ensure extract_images is honored on adapter pages
                        if options.extract_images {
                            let mut extracted_imgs = Vec::new();
                            let parsed_doc = Html::parse_document(&doc.content);
                            for img in extract_images(&parsed_doc, Some(url)) {
                                extracted_imgs.push(img);
                            }
                            // Also parse Markdown images: ![alt](src)
                            let md_img_re =
                                regex::Regex::new(r"!\[([^\]]*)\]\(((?:https?://|/)[^\)]+)\)")
                                    .unwrap();
                            let mut seen_srcs: HashSet<String> =
                                extracted_imgs.iter().map(|i| i.src.clone()).collect();
                            for cap in md_img_re.captures_iter(&doc.content) {
                                if let (Some(alt), Some(src)) = (cap.get(1), cap.get(2)) {
                                    let resolved = if let Ok(base) = Url::parse(url) {
                                        base.join(src.as_str())
                                            .map(|j| j.to_string())
                                            .unwrap_or_else(|_| src.as_str().to_string())
                                    } else {
                                        src.as_str().to_string()
                                    };
                                    if seen_srcs.insert(resolved.clone()) {
                                        extracted_imgs.push(ExtractedImage {
                                            alt: alt.as_str().trim().to_string(),
                                            src: resolved,
                                        });
                                    }
                                }
                            }
                            if !extracted_imgs.is_empty() {
                                doc.images = Some(extracted_imgs.clone());
                                if (options.format == OutputFormat::Markdown
                                    || options.format == OutputFormat::Text)
                                    && !doc.content.contains("## Images")
                                {
                                    doc.content
                                        .push_str(&format_images_markdown(&extracted_imgs));
                                }
                            }
                        }

                        if let Some(limit) = options.max_tokens {
                            let (content, truncated) =
                                truncate_to_tokens(&doc.content, limit, options.tokenizer);
                            doc.content = content;
                            doc.truncated |= truncated;
                        }
                        doc.token_count = count_tokens(&doc.content, options.tokenizer);
                        if options.chunk {
                            doc.chunks = Some(chunker::chunk_markdown(
                                &doc.content,
                                options.chunk_size.unwrap_or(500),
                                options.chunk_overlap.unwrap_or(50),
                                options.tokenizer,
                            ));
                        }
                        return Ok(doc);
                    }
                    Err(err) => {
                        if !adapter.allow_html_fallback(&parsed_url) {
                            return Err(err);
                        }
                        fallback_warnings.push(DiagnosticWarning::new(
                            "adapter_fallback",
                            format!("{} adapter failed: {err}", adapter.name()),
                        ));
                        warn!(
                            platform = adapter.name(),
                            error = %err,
                            "Platform adapter extraction failed, attempting fallback"
                        );
                        // Seamless CDP fallback: If adapter failed (e.g. anti-scraping challenge, 403 WAF) and CDP is enabled, fallback to CDP!
                        let cdp_ep = cdp::resolve_cdp_endpoint(
                            options.cdp_endpoint.as_deref(),
                            options.auto_cdp,
                        );
                        if let Some(ep) = cdp_ep {
                            debug!(endpoint = %ep, "平台适配器受阻，自动无缝回退至 CDP 渲染会话");
                            if let Ok(mut doc) = self
                                .fetch_via_cdp_and_process(url, &parsed_url, options, &ep)
                                .await
                            {
                                doc.warnings.extend(fallback_warnings);
                                return Ok(doc);
                            }
                        }
                    }
                }
            }
        }

        // 2. Standard HTML fetch or direct CDP if configured and no adapter matched
        if let Some(ref cdp_endpoint) = options.cdp_endpoint {
            debug!(
                endpoint = %cdp_endpoint,
                url = url,
                "使用 CDP 无头浏览器会话渲染页面"
            );
            return self
                .fetch_via_cdp_and_process(url, &parsed_url, options, cdp_endpoint)
                .await
                .map(|mut doc| {
                    doc.warnings.extend(fallback_warnings);
                    doc
                });
        }

        let response = match self.fetcher.fetch(url).await {
            Ok(resp) => {
                let is_challenge = cdp::is_anti_scraping_challenge(&resp.html, resp.status);
                let cdp_ep =
                    cdp::resolve_cdp_endpoint(options.cdp_endpoint.as_deref(), options.auto_cdp);

                if is_challenge {
                    fallback_warnings.push(DiagnosticWarning::new(
                        "http_challenge",
                        "HTTP response contains an access challenge",
                    ));
                    if let Some(ep) = cdp_ep {
                        debug!(
                            url = url,
                            endpoint = %ep,
                            "Static fetch encountered anti-scraping challenge; auto falling back to CDP headless browser"
                        );
                        return self
                            .fetch_via_cdp_and_process(url, &parsed_url, options, &ep)
                            .await
                            .map(|mut doc| {
                                doc.warnings.extend(fallback_warnings);
                                doc
                            });
                    }
                }
                resp
            }
            Err(err) => {
                fallback_warnings.push(DiagnosticWarning::new("http_fallback", err.to_string()));
                let cdp_ep =
                    cdp::resolve_cdp_endpoint(options.cdp_endpoint.as_deref(), options.auto_cdp);
                // If CDP endpoint is configured or auto_cdp is enabled, seamlessly fall back to CDP on HTTP client/server errors
                if let Some(ep) = cdp_ep {
                    debug!(
                        url = url,
                        endpoint = %ep,
                        error = %err,
                        "Static fetch failed with HTTP or network error; auto falling back to CDP headless browser"
                    );
                    return self
                        .fetch_via_cdp_and_process(url, &parsed_url, options, &ep)
                        .await
                        .map(|mut doc| {
                            doc.warnings.extend(fallback_warnings);
                            doc
                        });
                }
                return Err(err);
            }
        };

        self.process_response(response, options, fetch_start)
            .await
            .map(|mut doc| {
                doc.warnings.extend(fallback_warnings);
                doc
            })
    }

    async fn process_response(
        &self,
        response: fetcher::FetchedResponse,
        options: &ExtractionOptions,
        fetch_start: Instant,
    ) -> Result<PageDocument, TklessError> {
        let source_url = response.requested_url.clone();
        let url = source_url.as_str();
        // Native PDF extraction fast-path when server returns PDF bytes
        if let Some(ref bytes) = response.raw_bytes {
            if pdf::is_pdf_content(bytes) {
                debug!(
                    url = url,
                    "Directing response to native PDF extraction pipeline"
                );
                let mut doc =
                    pdf::extract_pdf_to_document(bytes, Some(&response.final_url), options)?;
                doc.extraction.transport = "http".into();
                doc.url = response.requested_url;
                doc.final_url = response.final_url;
                doc.status = response.status;
                doc.truncated = doc.truncated || response.truncated;
                return Ok(doc);
            }
        }

        let mut doc = self.process_html(&response.html, Some(&response.final_url), options)?;
        doc.extraction.transport = "http".into();
        doc.url = response.requested_url;
        doc.final_url = response.final_url;
        doc.status = response.status;
        doc.truncated = doc.truncated || response.truncated;

        if options.download_images && !doc.content.is_empty() {
            let assets_path =
                std::path::Path::new(options.assets_dir.as_deref().unwrap_or("./assets"));
            if let Ok(rewritten) = downloader::download_and_rewrite_markdown_images(
                &self.fetcher,
                &doc.content,
                assets_path,
                Some(url),
            )
            .await
            {
                doc.content = rewritten;
            }
        }

        if let Some(ref mut m) = doc.metrics {
            m.fetch_time_ms = fetch_start.elapsed().as_millis() as u64;
            m.total_time_ms = m.fetch_time_ms + m.clean_time_ms;
        }

        Ok(doc)
    }

    /// Helper to fetch rendered HTML via CDP and run it through the extraction pipeline.
    async fn fetch_via_cdp_and_process(
        &self,
        url: &str,
        parsed_url: &Url,
        options: &ExtractionOptions,
        cdp_endpoint: &str,
    ) -> Result<PageDocument, TklessError> {
        let cdp_opts = cdp::CdpOptions {
            allow_private_addresses: self.fetcher.options().allow_private_addresses,
            endpoint: cdp_endpoint.to_string(),
            wait_ms: options.cdp_wait_ms,
            timeout_ms: None,
            user_agent: self.fetcher.options().user_agent.clone(),
            accept_language: self
                .fetcher
                .options()
                .headers
                .iter()
                .rev()
                .find(|(name, _)| name.eq_ignore_ascii_case("Accept-Language"))
                .map(|(_, value)| value.clone()),
            cookie: self.fetcher.resolve_cookie_for_url(parsed_url),
            block_resources: Some(options.cdp_block_resources),
            auto_scroll: Some(options.cdp_auto_scroll),
            scroll_count: Some(5),
            ..Default::default()
        };
        let html = cdp::fetch_html_via_cdp(url, &cdp_opts).await?;
        let mut doc = self.process_html(&html, Some(url), options)?;
        doc.extraction.transport = "cdp".into();
        doc.url = url.to_string();
        doc.final_url = url.to_string();
        doc.status = 200;
        Ok(doc)
    }

    /// Fetches and extracts a webpage with intelligent two-level caching and HTTP 304 conditional revalidation.
    /// If a fresh cache entry exists, returns it immediately (< 1ms).
    /// If expired, issues a conditional HTTP request with ETag / Last-Modified headers. Upon receiving HTTP 304,
    /// re-arms the cache and returns the cached document with zero body bytes transferred.
    pub async fn process_url_with_cache(
        &self,
        url: &str,
        options: &ExtractionOptions,
        cache: &DocumentCache,
    ) -> Result<PageDocument, TklessError> {
        options.validate()?;
        self.fetcher.validate_ssrf(&Url::parse(url)?).await?;
        let key = self.cache_key(cache, url, options);

        // Platform and browser paths must use the same extraction behavior with or without caching.
        let specialized = (!options.no_adapter
            && options.schema.is_none()
            && self
                .adapter_registry
                .find_adapter(&Url::parse(url)?)
                .is_some())
            || options.cdp_endpoint.is_some()
            || options.auto_cdp
            || options.download_images;
        if specialized {
            if let Some(doc) = cache.get(&key) {
                return Ok(doc);
            }
            let doc = self.process_url(url, options).await?;
            cache.set(&key, &doc, None)?;
            return Ok(doc);
        }
        if let Some(entry) = cache.get_entry(&key) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            if !entry.is_expired(now) {
                debug!(url = url, "L1/L2 cache hit (fresh)");
                return Ok(entry.document);
            }

            // Entry expired: attempt conditional HTTP 304 revalidation
            if entry.etag.is_some() || entry.last_modified.is_some() {
                debug!(
                    url = url,
                    etag = ?entry.etag,
                    last_modified = ?entry.last_modified,
                    "Cache entry expired, issuing conditional HTTP 304 revalidation"
                );
                match self
                    .fetcher
                    .fetch_with_revalidation(
                        url,
                        entry.etag.as_deref(),
                        entry.last_modified.as_deref(),
                    )
                    .await
                {
                    Ok(resp) if resp.not_modified => {
                        debug!(
                            url = url,
                            "Server confirmed HTTP 304 Not Modified; re-arming cache with 0 body bytes"
                        );
                        cache.touch(&key, None);
                        return Ok(entry.document);
                    }
                    Ok(resp) => {
                        let etag = resp.etag.clone();
                        let last_modified = resp.last_modified.clone();
                        let doc = self.process_response(resp, options, Instant::now()).await?;
                        let _ = cache.set_with_validators(&key, &doc, None, etag, last_modified);
                        return Ok(doc);
                    }
                    Err(err) => {
                        warn!(
                            url = url,
                            error = %err,
                            "Conditional revalidation failed"
                        );
                        return Err(err);
                    }
                }
            }
        }

        // Cache miss: fetch full document and store with validators
        let resp = self.fetcher.fetch(url).await?;
        let etag = resp.etag.clone();
        let last_modified = resp.last_modified.clone();
        let doc = self.process_response(resp, options, Instant::now()).await?;
        let _ = cache.set_with_validators(&key, &doc, None, etag, last_modified);
        Ok(doc)
    }

    /// Processes already-available raw HTML (e.g. from stdin, file, or headless browser).
    pub fn process_html(
        &self,
        raw_html: &str,
        base_url: Option<&str>,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        options.validate()?;
        // Check if raw content is a PDF document
        if pdf::is_pdf_content(raw_html.as_bytes()) {
            debug!("Directing raw input to native PDF extraction pipeline");
            return pdf::extract_pdf_to_document(raw_html.as_bytes(), base_url, options);
        }
        let clean_start = Instant::now();

        // 0. Optional negative DOM pruning via remove_selector
        let pruned_html_holder;
        let effective_html = if let Some(ref rem_sel) = options.remove_selector {
            match cleaner::prune_unwanted_selectors(raw_html, rem_sel) {
                Ok(h) => {
                    pruned_html_holder = h;
                    &pruned_html_holder
                }
                Err(e) => {
                    warn!(error = %e, "Failed to prune unwanted selectors, continuing with raw HTML");
                    raw_html
                }
            }
        } else {
            raw_html
        };

        // 0.5 Auto-detection or explicit request for RSS 2.0 / Atom 1.0 Syndication Feeds
        if options.schema.is_none() && (options.feed || feed::is_feed_xml(effective_html)) {
            if let Ok(channel) = feed::parse_feed(effective_html) {
                let content = match options.format {
                    OutputFormat::Json => serde_json::to_string_pretty(&channel)
                        .unwrap_or_else(|_| feed::format_feed_markdown(&channel)),
                    _ => feed::format_feed_markdown(&channel),
                };
                let token_count = count_tokens(&content, options.tokenizer);
                let title = Some(channel.title.clone());
                let metadata = PageMetadata {
                    description: Some(channel.description.clone()),
                    canonical_url: Some(channel.link.clone()),
                    ..Default::default()
                };

                return Ok(PageDocument {
                    extraction: ExtractionProvenance {
                        method: "feed".into(),
                        ..Default::default()
                    },
                    warnings: Vec::new(),
                    url: base_url.unwrap_or(&channel.link).to_string(),
                    final_url: base_url.unwrap_or(&channel.link).to_string(),
                    status: 200,
                    title,
                    byline: None,
                    content,
                    metadata,
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
                });
            }
        }

        let parsed_doc = Html::parse_document(effective_html);

        // 1. Extract metadata and JSON-LD
        let metadata = if options.include_metadata {
            extract_metadata(&parsed_doc)
        } else {
            PageMetadata::default()
        };

        let json_ld = if options.include_json_ld {
            extract_json_ld(&parsed_doc)
        } else {
            Vec::new()
        };

        // 2. Extract structured assets (links, images, tables)
        let links = if options.extract_links {
            Some(extract_links(&parsed_doc, base_url))
        } else {
            None
        };

        let images = if options.extract_images {
            Some(extract_images(&parsed_doc, base_url))
        } else {
            None
        };

        let tables = if options.extract_tables || options.tables_only {
            Some(extract_tables(&parsed_doc))
        } else {
            None
        };

        // 2b. Extract structured entities if schema is provided (with graceful fallback on selector mismatch)
        let (entity, schema_warning) = if let Some(ref schema_input) = options.schema {
            match schema::ExtractionSchema::parse(schema_input) {
                Ok(schema) => match schema::extract_entities(effective_html, &schema, base_url) {
                    Ok(val) => (Some(val), None),
                    Err(err) => {
                        if options.schema_strict {
                            return Err(err);
                        }
                        warn!(
                            error = %err,
                            "Schema 实体提取未匹配到有效字段，自动回退至全文正文提取"
                        );
                        (None, Some(err.to_string()))
                    }
                },
                Err(err) => {
                    if options.schema_strict {
                        return Err(err);
                    }
                    warn!(error = %err, "解析 Schema 定义失败，自动回退至全文正文提取");
                    (None, Some(err.to_string()))
                }
            }
        } else {
            (None, None)
        };

        // 3. Clean and isolate target content (targeted CSS selector or heuristic cleaning)
        let cleaned = if let Some(ref sel) = options.selector {
            extract_selector(effective_html, sel, base_url)?
        } else {
            clean_html(effective_html, base_url, options.mode)?
        };

        let title = cleaned.title.or_else(|| {
            metadata
                .open_graph
                .get("og:title")
                .cloned()
                .or_else(|| metadata.description.clone())
        });

        // 4. Format into desired output presentation
        let mut content = if options.tables_only {
            let table_list = tables.as_deref().unwrap_or(&[]);
            ExtractedTable::format_tables(table_list, options.table_format)
        } else if let Some(ref ent) = entity {
            match options.format {
                OutputFormat::Json => {
                    serde_json::to_string_pretty(ent).unwrap_or_else(|_| "{}".to_string())
                }
                OutputFormat::Markdown => {
                    let mut s = String::new();
                    s.push_str("### 结构化实体提取 (Extracted Entity)\n\n```json\n");
                    s.push_str(&serde_json::to_string_pretty(ent).unwrap_or_default());
                    s.push_str("\n```\n");
                    s
                }
                _ => serde_json::to_string(ent).unwrap_or_default(),
            }
        } else {
            let mut formatted = match options.format {
                OutputFormat::Markdown | OutputFormat::Json => {
                    let md = self.markdown_converter.convert(
                        &cleaned.html,
                        options.strip_images,
                        options.strip_links,
                    )?;
                    if options.frontmatter && options.format == OutputFormat::Markdown {
                        prepend_frontmatter(&md, title.as_deref(), base_url, &metadata)
                    } else {
                        md
                    }
                }
                OutputFormat::Text => html_to_plain_text(&cleaned.html),
                OutputFormat::Raw | OutputFormat::Html => cleaned.html,
            };

            if let Some(ref warn_msg) = schema_warning {
                if options.format == OutputFormat::Markdown {
                    formatted.push_str(&format!("\n\n> **Schema 提取提示**: {}\n", warn_msg));
                }
            }
            formatted
        };

        // Append structured link and image assets to Markdown/Text output if requested
        if !options.tables_only && options.extract_links {
            if let Some(ref link_list) = links {
                if !link_list.is_empty()
                    && (options.format == OutputFormat::Markdown
                        || options.format == OutputFormat::Text)
                    && !content.contains("## Links")
                {
                    content.push_str(&format_links_markdown(link_list));
                }
            }
        }

        if !options.tables_only && options.extract_images {
            if let Some(ref image_list) = images {
                if !image_list.is_empty()
                    && (options.format == OutputFormat::Markdown
                        || options.format == OutputFormat::Text)
                    && !content.contains("## Images")
                {
                    content.push_str(&format_images_markdown(image_list));
                }
            }
        }

        // Append structured entity JSON to Markdown if extracted (avoid duplicate if already present)
        if let Some(ref ent) = entity {
            if options.format == OutputFormat::Markdown && !content.contains("结构化实体提取")
            {
                content.push_str("\n\n### 结构化实体提取 (Extracted Entity)\n\n```json\n");
                content.push_str(&serde_json::to_string_pretty(ent).unwrap_or_default());
                content.push_str("\n```\n");
            }
        }

        // 4.5 Prepend heuristic extractive summary if requested
        if options.summarize
            && (options.format == OutputFormat::Markdown || options.format == OutputFormat::Json)
            && !content.is_empty()
        {
            content = summarizer::summarize_markdown(&content, 3);
        }

        // 5. Token counting and truncation
        let initial_token_count = count_tokens(&content, options.tokenizer);
        let mut truncated = false;

        if let Some(limit) = options.max_tokens {
            if initial_token_count > limit {
                debug!(
                    current = initial_token_count,
                    limit = limit,
                    "Truncating content to token limit"
                );
                let (truncated_content, was_truncated) =
                    truncate_to_tokens(&content, limit, options.tokenizer);
                content = truncated_content;
                truncated = was_truncated;
            }
        }

        let final_token_count = if truncated {
            count_tokens(&content, options.tokenizer)
        } else {
            initial_token_count
        };

        let chunks = if options.chunk {
            Some(chunker::chunk_markdown(
                &content,
                options.chunk_size.unwrap_or(500),
                options.chunk_overlap.unwrap_or(50),
                options.tokenizer,
            ))
        } else {
            None
        };

        let content_hash = Some(format!("{:016x}", dedup::simhash(&content)));

        let metrics = if options.stats {
            let raw_bytes = raw_html.len();
            let cleaned_bytes = content.len();
            let raw_token_estimate = count_tokens(raw_html, options.tokenizer);
            let token_savings_percent = ((raw_token_estimate.saturating_sub(final_token_count))
                as f64
                / raw_token_estimate.max(1) as f64
                * 100.0)
                .clamp(0.0, 100.0);
            let noise_reduction_percent = ((raw_bytes.saturating_sub(cleaned_bytes)) as f64
                / raw_bytes.max(1) as f64
                * 100.0)
                .clamp(0.0, 100.0);
            let clean_time_ms = clean_start.elapsed().as_millis() as u64;

            Some(ExtractionMetrics {
                fetch_time_ms: 0,
                clean_time_ms,
                total_time_ms: clean_time_ms,
                raw_bytes,
                cleaned_bytes,
                raw_token_estimate,
                final_token_count,
                token_savings_percent,
                noise_reduction_percent,
            })
        } else {
            None
        };

        Ok(PageDocument {
            extraction: ExtractionProvenance {
                method: if entity.is_some() { "schema" } else { "html" }.into(),
                ..Default::default()
            },
            warnings: schema_warning
                .into_iter()
                .map(|message| DiagnosticWarning::new("schema_fallback", message))
                .collect(),
            url: base_url.unwrap_or("").to_string(),
            final_url: base_url.unwrap_or("").to_string(),
            status: 200,
            title,
            byline: cleaned.byline.or_else(|| metadata.author.clone()),
            content,
            metadata,
            json_ld,
            token_count: final_token_count,
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links,
            images,
            tables,
            entity,
            chunks,
            content_hash,
            metrics,
        })
    }
}

/// Prepends YAML frontmatter to Markdown content for structured LLM reading.
fn prepend_frontmatter(
    content: &str,
    title: Option<&str>,
    url: Option<&str>,
    metadata: &PageMetadata,
) -> String {
    let mut fm = String::from("---\n");

    if let Some(t) = title {
        fm.push_str(&format!("title: \"{}\"\n", t.replace('"', "\\\"")));
    }
    if let Some(u) = url {
        fm.push_str(&format!("url: \"{u}\"\n"));
    }
    if let Some(ref pub_time) = metadata.published_time {
        fm.push_str(&format!("published: \"{pub_time}\"\n"));
    }
    if let Some(ref author) = metadata.author {
        fm.push_str(&format!("author: \"{}\"\n", author.replace('"', "\\\"")));
    }
    if let Some(ref desc) = metadata.description {
        fm.push_str(&format!("description: \"{}\"\n", desc.replace('"', "\\\"")));
    }
    if let Some(ref cover) = metadata.cover_image {
        fm.push_str(&format!("cover: \"{cover}\"\n"));
    }
    fm.push_str("---\n\n");
    fm.push_str(content);

    fm
}

/// Converts HTML string to clean, whitespace-collapsed plain text.
fn html_to_plain_text(html: &str) -> String {
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
