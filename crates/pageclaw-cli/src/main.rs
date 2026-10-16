mod cli;
mod output;

use anyhow::{bail, Context, Result};
use clap::Parser;
use cli::{CliArgs, CliFormat, SubCommand};
use pageclaw_core::adapters::BilibiliAdapter;
use pageclaw_core::downloader::{format_bytes, DownloadProgressReporter};
use pageclaw_core::model::{ExtractionOptions, FetchOptions};
use pageclaw_core::{DocumentCache, McpServer, PageclawEngine, PageclawError};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;
use tracing_subscriber::EnvFilter;
use url::Url;

/// Real-time console progress reporter for media downloads writing strictly to stderr.
struct ConsoleProgressReporter {
    label: &'static str,
    start_time: Instant,
    last_update: Mutex<Instant>,
    quiet: bool,
}

impl ConsoleProgressReporter {
    fn new(label: &'static str, quiet: bool) -> Self {
        let now = Instant::now();
        Self {
            label,
            start_time: now,
            last_update: Mutex::new(now),
            quiet,
        }
    }
}

impl DownloadProgressReporter for ConsoleProgressReporter {
    fn on_start(&self, total_bytes: Option<u64>) {
        if self.quiet {
            return;
        }
        let total_str = match total_bytes {
            Some(t) => format!(" (total: {})", format_bytes(t)),
            None => String::new(),
        };
        eprintln!(
            "[PageClaw] Starting {} download{}...",
            self.label, total_str
        );
    }

    fn on_progress(&self, downloaded_bytes: u64, total_bytes: Option<u64>) {
        if self.quiet {
            return;
        }

        let mut last = self.last_update.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(*last).as_millis() < 120 {
            return;
        }
        *last = now;

        let elapsed_secs = self.start_time.elapsed().as_secs_f64().max(0.001);
        let speed = downloaded_bytes as f64 / elapsed_secs;
        let speed_str = format!("{}/s", format_bytes(speed as u64));

        let downloaded_str = format_bytes(downloaded_bytes);

        if let Some(total) = total_bytes {
            let percent = if total > 0 {
                (downloaded_bytes as f64 / total as f64 * 100.0).min(100.0)
            } else {
                0.0
            };
            let total_str = format_bytes(total);
            eprint!(
                "\r[PageClaw] Downloading {}: {:>5.1}% ({:>9} / {:>9}, {:>10})   ",
                self.label, percent, downloaded_str, total_str, speed_str
            );
        } else {
            eprint!(
                "\r[PageClaw] Downloading {}: {:>9} ({:>10})   ",
                self.label, downloaded_str, speed_str
            );
        }
        let _ = io::stderr().flush();
    }

    fn on_finish(&self, total_bytes: u64) {
        if self.quiet {
            return;
        }
        let elapsed = self.start_time.elapsed();
        let speed = if elapsed.as_secs_f64() > 0.0 {
            format!(
                "{}/s",
                format_bytes((total_bytes as f64 / elapsed.as_secs_f64()) as u64)
            )
        } else {
            "N/A".into()
        };
        eprintln!(
            "\r[PageClaw] {} download complete: {} in {:.2?} ({})                    ",
            self.label,
            format_bytes(total_bytes),
            elapsed,
            speed
        );
    }
}

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();
    if let Err(err) = run(args).await {
        let exit_code = if let Some(pe) = err.downcast_ref::<PageclawError>() {
            pe.exit_code()
        } else {
            1
        };
        eprintln!("Error: {err:#}");
        std::process::exit(exit_code);
    }
}

async fn run(args: CliArgs) -> Result<()> {
    // Configure tracing logger to output strictly to stderr
    if !args.quiet {
        let filter_str = match args.verbose {
            0 => "pageclaw=warn,pageclaw_core=warn",
            1 => "pageclaw=info,pageclaw_core=info",
            2 => "pageclaw=debug,pageclaw_core=debug",
            _ => "pageclaw=trace,pageclaw_core=trace",
        };

        let env_filter = EnvFilter::try_new(filter_str).unwrap_or_else(|_| EnvFilter::new("warn"));

        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_env_filter(env_filter)
            .init();
    }

    // Parse custom headers
    let mut custom_headers = Vec::new();
    for h in &args.headers {
        if let Some((k, v)) = h.split_once(':') {
            custom_headers.push((k.trim().to_string(), v.trim().to_string()));
        } else {
            eprintln!(
                "[PageClaw warning] Skipping invalid header format (expected Key: Value): '{h}'"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Execution Branch 0: Interactive Browser QR Code Login
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Login(ref login_args)) = args.command {
        let platform_clean = login_args.platform.trim().to_ascii_lowercase();
        match platform_clean.as_str() {
            "bilibili" | "bili" => {
                let login_options = pageclaw_core::auth::LoginOptions {
                    output_path: login_args.output.clone(),
                    no_browser: login_args.no_browser,
                    timeout_secs: login_args.timeout_secs,
                };

                pageclaw_core::auth::execute_login_flow(None, login_options).await?;
                return Ok(());
            }
            "zhihu" => {
                let login_url = "https://www.zhihu.com/signin";
                eprintln!("打开浏览器授权登录zhihu：{login_url}");
                if !login_args.no_browser {
                    if let Err(e) = pageclaw_core::auth::open_browser(login_url) {
                        eprintln!("[PageClaw warning] Failed to launch browser: {e}");
                    }
                }
                eprintln!("[PageClaw] 登录完成后，可将 Cookie 凭据按前缀格式保存至 cookies.txt（自动区分多网站，互不覆盖）：");
                eprintln!("  zhihu: d_c0=\"...\"; z_c0=\"...\"");
                eprintln!("或直接使用命令行参数: pageclaw <URL> --cookie \"...\"");
                return Ok(());
            }
            other => {
                bail!("Unsupported login platform: '{other}'. Currently supported platforms: bilibili, zhihu");
            }
        }
    }

    // Resolve cookies from --cookie-file and --cookie with CLI priority (auto-fallback to ./cookies.txt if present)
    let default_cookie_path = std::path::Path::new("./cookies.txt");
    let target_url_ref = args.target.as_deref();
    let cookie_from_file = if let Some(ref cf) = args.cookie_file {
        Some(
            pageclaw_core::fetcher::load_cookie_from_file_for_url(cf, target_url_ref)
                .with_context(|| format!("Failed to read cookie file: {}", cf.display()))?,
        )
    } else if args.cookie.is_none() && default_cookie_path.is_file() {
        if let Ok(c) = pageclaw_core::fetcher::load_cookie_from_file_for_url(
            default_cookie_path,
            target_url_ref,
        ) {
            if !args.quiet && !c.trim().is_empty() {
                if let Some(platform) =
                    target_url_ref.and_then(pageclaw_core::fetcher::detect_platform_from_url)
                {
                    eprintln!("[PageClaw] Automatically loaded [{platform}] credentials from ./cookies.txt");
                } else {
                    eprintln!(
                        "[PageClaw] Automatically loaded default credentials from ./cookies.txt"
                    );
                }
            }
            Some(c)
        } else {
            None
        }
    } else {
        None
    };

    let effective_cookie = match (cookie_from_file, args.cookie) {
        (Some(fc), Some(cc)) => Some(pageclaw_core::fetcher::merge_cookies(&fc, &cc)),
        (Some(fc), None) => Some(fc),
        (None, Some(cc)) => Some(cc),
        (None, None) => None,
    };

    // Build network fetch options
    let fetch_options = FetchOptions {
        user_agent: args.user_agent.clone(),
        timeout_secs: args.timeout_secs,
        connect_timeout_secs: args.connect_timeout,
        retries: args.retries,
        max_bytes: args.max_size_mb * 1024 * 1024,
        on_too_large: args.on_too_large.into(),
        headers: custom_headers,
        cookie: effective_cookie,
        cookie_file: None,
        allow_private_addresses: args.allow_private_addresses,
        allow_http_errors: args.allow_http_errors,
    };

    // Build extraction options
    let extraction_options = ExtractionOptions {
        mode: args.mode.into(),
        format: args.format.into(),
        max_tokens: args.max_tokens,
        tokenizer: args.tokenizer.into(),
        include_metadata: !args.no_metadata,
        include_json_ld: args.json_ld_only || matches!(args.format, CliFormat::Json),
        strip_images: args.no_images,
        strip_links: args.no_links,
        frontmatter: args.frontmatter,
        no_adapter: args.no_adapter,
        feed: args.bili_feed,
        selector: args.selector.clone(),
        extract_links: args.extract_links,
        extract_images: args.extract_images,
        extract_tables: args.extract_tables,
        subtitles: args.bili_sub,
        danmaku: args.bili_dm || args.bili_dm_limit.is_some(),
        danmaku_limit: args.bili_dm_limit,
        comments: args.bili_cm || args.bili_cm_limit.is_some(),
        comments_limit: args.bili_cm_limit,
        conclusion: args.bili_sum,
    };

    // -------------------------------------------------------------------------
    // Execution Branch 1: Native MCP (Model Context Protocol) Server over Stdio
    // -------------------------------------------------------------------------
    let is_mcp = args.mcp || matches!(args.command, Some(SubCommand::Mcp(_)));
    if is_mcp {
        let (no_cache, cache_ttl, cache_dir) = match args.command {
            Some(SubCommand::Mcp(ref mcp_args)) => (
                mcp_args.no_cache,
                mcp_args.cache_ttl,
                mcp_args.cache_dir.clone(),
            ),
            _ => (args.no_cache, args.cache_ttl, args.cache_dir.clone()),
        };

        let cache = if no_cache {
            DocumentCache::memory_only(cache_ttl)
        } else {
            DocumentCache::new(cache_dir, cache_ttl)
        };

        let engine = PageclawEngine::with_options(fetch_options)
            .context("Failed to initialize PageClaw engine")?;
        let mcp_server = McpServer::new(engine, cache);
        return mcp_server.run_stdio().await.map_err(Into::into);
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.5: Zero-API-Key Web Search
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Search(ref search_args)) = args.command {
        let engine = PageclawEngine::with_options(fetch_options)
            .context("Failed to initialize PageClaw engine")?;

        let effective_query = if search_args.wechat {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:mp.weixin.qq.com {clean}")
            }
        } else if search_args.zhihu {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:zhihu.com {clean}")
            }
        } else if search_args.github {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:github.com {clean}")
            }
        } else if search_args.juejin {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:juejin.cn {clean}")
            }
        } else {
            search_args.query.clone()
        };

        if search_args.fetch_content {
            let items = pageclaw_core::web_search_and_fetch(
                &engine,
                &effective_query,
                search_args.limit,
                search_args.engine.into(),
                &extraction_options,
                search_args.max_tokens_per_page,
            )
            .await
            .context("Failed to perform search and fetch")?;

            let mut payload = match search_args.format {
                CliFormat::Json => serde_json::to_string_pretty(&items)
                    .context("Failed to serialize search & fetch items to JSON")?,
                _ => pageclaw_core::format_search_fetch_markdown(&effective_query, &items),
            };

            // Enforce overall max-tokens limit if specified
            if let Some(total_limit) = args.max_tokens {
                let initial_tokens =
                    pageclaw_core::token::count_tokens(&payload, args.tokenizer.into());
                if initial_tokens > total_limit {
                    let (truncated_payload, _) = pageclaw_core::token::truncate_to_tokens(
                        &payload,
                        total_limit,
                        args.tokenizer.into(),
                    );
                    payload = truncated_payload;
                }
            }

            output::write_payload(&payload, search_args.output.as_deref())?;
            return Ok(());
        }

        let results = pageclaw_core::web_search_with_engine(
            engine.client(),
            &effective_query,
            search_args.limit,
            search_args.engine.into(),
        )
        .await
        .context("Failed to perform web search")?;

        let payload = match search_args.format {
            CliFormat::Json => serde_json::to_string_pretty(&results)
                .context("Failed to serialize search results to JSON")?,
            _ => pageclaw_core::format_search_results_markdown(&effective_query, &results),
        };

        output::write_payload(&payload, search_args.output.as_deref())?;
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.6: Concurrent Batch URL Fetch & Extraction
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Batch(ref batch_args)) = args.command {
        let raw_lines = if batch_args.file == "-" {
            let mut stdin_content = String::new();
            io::stdin()
                .read_to_string(&mut stdin_content)
                .context("Failed to read URL list from stdin")?;
            stdin_content
        } else {
            std::fs::read_to_string(&batch_args.file)
                .with_context(|| format!("Failed to read batch URL file: {}", batch_args.file))?
        };

        let urls: Vec<String> = raw_lines
            .lines()
            .map(|l| l.trim().trim_matches('\u{feff}').to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();

        if urls.is_empty() {
            bail!("No valid URLs found in '{}'", batch_args.file);
        }

        let engine = std::sync::Arc::new(
            PageclawEngine::with_options(fetch_options)
                .context("Failed to initialize PageClaw engine")?,
        );

        let batch_extraction_opts = std::sync::Arc::new(ExtractionOptions {
            mode: args.mode.into(),
            format: batch_args.format.into(),
            max_tokens: batch_args.max_tokens,
            tokenizer: args.tokenizer.into(),
            include_metadata: !args.no_metadata,
            include_json_ld: matches!(batch_args.format, CliFormat::Json),
            strip_images: batch_args.no_images,
            strip_links: batch_args.no_links,
            frontmatter: args.frontmatter,
            no_adapter: args.no_adapter,
            feed: false,
            selector: batch_args.selector.clone(),
            extract_links: batch_args.extract_links,
            extract_images: false,
            extract_tables: batch_args.extract_tables,
            subtitles: false,
            danmaku: false,
            danmaku_limit: None,
            comments: false,
            comments_limit: None,
            conclusion: false,
        });

        let semaphore =
            std::sync::Arc::new(tokio::sync::Semaphore::new(batch_args.concurrency.max(1)));
        let total = urls.len();
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let mut tasks = Vec::with_capacity(total);

        for url in urls {
            let sem = semaphore.clone();
            let eng = engine.clone();
            let opts = batch_extraction_opts.clone();
            let cnt = counter.clone();
            let quiet = args.quiet;

            tasks.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let res = eng.process_url(&url, &opts).await;
                let done = cnt.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if !quiet {
                    match &res {
                        Ok(doc) => {
                            let title_disp = doc.title.as_deref().unwrap_or("<no title>");
                            eprintln!(
                                "[PageClaw Batch] [{done}/{total}] OK: \"{title_disp}\" ({url})"
                            );
                        }
                        Err(e) => {
                            eprintln!("[PageClaw Batch] [{done}/{total}] FAILED ({url}): {e}");
                        }
                    }
                }
                res
            }));
        }

        let mut docs = Vec::new();
        for task in tasks {
            if let Ok(Ok(doc)) = task.await {
                docs.push(doc);
            }
        }

        let payload = match batch_args.format {
            CliFormat::Json => serde_json::to_string_pretty(&docs)
                .context("Failed to serialize batch documents to JSON")?,
            _ => docs
                .iter()
                .map(|d| d.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n---\n\n"),
        };

        output::write_payload(&payload, batch_args.output.as_deref())?;
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.7: Deep Site Crawl & Knowledge Base Assembly
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Crawl(ref crawl_args)) = args.command {
        let engine = PageclawEngine::with_options(fetch_options)
            .context("Failed to initialize PageClaw engine")?;

        let crawl_options = pageclaw_core::CrawlOptions {
            max_depth: crawl_args.max_depth,
            max_pages: crawl_args.max_pages,
            concurrency: crawl_args.concurrency,
            same_domain_only: true,
            same_path_prefix: crawl_args.same_path_prefix,
            exclude_patterns: pageclaw_core::CrawlOptions::default().exclude_patterns,
        };

        let report = pageclaw_core::crawl_site(
            &engine,
            &crawl_args.url,
            &crawl_options,
            &extraction_options,
        )
        .await
        .context("Failed to crawl site")?;

        let mut payload = match crawl_args.format {
            CliFormat::Json => serde_json::to_string_pretty(&report)
                .context("Failed to serialize crawl report to JSON")?,
            _ => pageclaw_core::format_crawl_report_markdown(&report),
        };

        if let Some(total_limit) = args.max_tokens {
            let initial_tokens =
                pageclaw_core::token::count_tokens(&payload, args.tokenizer.into());
            if initial_tokens > total_limit {
                let (truncated_payload, _) = pageclaw_core::token::truncate_to_tokens(
                    &payload,
                    total_limit,
                    args.tokenizer.into(),
                );
                payload = truncated_payload;
            }
        }

        output::write_payload(&payload, crawl_args.output.as_deref())?;
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Execution Branch 2: Standard CLI Mode
    // -------------------------------------------------------------------------
    let raw_target = if let Some(ref kw) = args.bili_search {
        let page = args.bili_page.unwrap_or(1);
        let encoded_kw: String = url::form_urlencoded::byte_serialize(kw.as_bytes()).collect();
        let mut s_url = format!("https://search.bilibili.com/all?keyword={encoded_kw}&page={page}");
        if let Some(ref ord) = args.bili_order {
            if !ord.is_empty() {
                s_url.push_str(&format!("&order={ord}"));
            }
        }
        s_url
    } else if args.bili_feed && args.target.is_none() {
        "https://www.bilibili.com/".to_string()
    } else {
        match args.target {
            Some(ref t) => {
                let mut t_str = t.clone();
                if t_str.contains("search.bilibili.com") && (args.bili_page.is_some() || args.bili_order.is_some()) {
                    t_str = apply_search_params(&t_str, args.bili_page, args.bili_order.as_deref());
                }
                t_str
            }
            None => bail!("Missing target URL, '--bili-search <KEYWORD>', or '-'. See 'pageclaw --help' for available options."),
        }
    };

    let start_time = Instant::now();
    let engine = PageclawEngine::with_options(fetch_options)
        .context("Failed to initialize PageClaw engine")?;

    // Auto-normalize target (e.g. bare BV1xx411c7mD -> https://www.bilibili.com/video/BV1xx411c7mD)
    let target = pageclaw_core::adapters::bilibili::normalize_bilibili_url(&raw_target);

    // -------------------------------------------------------------------------
    // Dedicated Media Action 1: Download Video Stream
    // -------------------------------------------------------------------------
    if let Some(ref dest_val) = args.bili_dl {
        let (bvid, default_page) =
            pageclaw_core::adapters::bilibili::extract_video_info(&target)
                .context("The --bili-dl option requires a valid Bilibili video URL or BV ID")?;

        let page_num = args.bili_page.map(|p| p as usize).unwrap_or(default_page);

        let user_dest = if dest_val.is_empty() {
            None
        } else {
            Some(Path::new(dest_val))
        };

        let reporter = ConsoleProgressReporter::new("Video", args.quiet);
        let saved_path = BilibiliAdapter::download_video(
            engine.client(),
            &bvid,
            page_num,
            args.bili_quality.into(),
            user_dest,
            Some(&reporter),
        )
        .await
        .context("Failed to download video stream")?;

        if !args.quiet {
            eprintln!("[PageClaw] Saved video to: {}", saved_path.display());
        }
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Dedicated Media Action 2: Download Video Cover
    // -------------------------------------------------------------------------
    if let Some(ref dest_val) = args.bili_dl_cover {
        let (bvid, _) = pageclaw_core::adapters::bilibili::extract_video_info(&target)
            .context("The --bili-dl-cover option requires a valid Bilibili video URL or BV ID")?;

        let user_dest = if dest_val.is_empty() {
            None
        } else {
            Some(Path::new(dest_val))
        };

        let reporter = ConsoleProgressReporter::new("Cover", args.quiet);
        let saved_path =
            BilibiliAdapter::download_cover(engine.client(), &bvid, user_dest, Some(&reporter))
                .await
                .context("Failed to download video cover image")?;

        if !args.quiet {
            eprintln!("[PageClaw] Saved cover to: {}", saved_path.display());
        }
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Caching Check for Document Extraction
    // -------------------------------------------------------------------------
    let cache = if args.cache && !args.no_cache {
        Some(DocumentCache::new(args.cache_dir.clone(), args.cache_ttl))
    } else {
        None
    };

    let cache_key = if target != "-" {
        cache
            .as_ref()
            .map(|c| c.generate_key(&target, &extraction_options))
    } else {
        None
    };

    if let (Some(c), Some(k)) = (cache.as_ref(), cache_key.as_ref()) {
        if let Some(cached_doc) = c.get(k) {
            if !args.quiet {
                let title_disp = cached_doc.title.as_deref().unwrap_or("<no title>");
                eprintln!(
                    "[PageClaw] Cache hit: \"{}\" (Tokens: {})",
                    title_disp, cached_doc.token_count
                );
            }
            if args.bili_cover {
                if let Some(ref cover_url) = cached_doc.metadata.cover_image {
                    output::render_cover(cover_url, args.output_file.as_deref())?;
                    return Ok(());
                } else {
                    bail!("No cover image found for the specified target");
                }
            }
            return output::render_output(
                &cached_doc,
                extraction_options.format,
                args.json_ld_only,
                args.output_file.as_deref(),
            );
        }
    }

    // -------------------------------------------------------------------------
    // Standard Document Extraction / Fetch Pipeline
    // -------------------------------------------------------------------------
    let doc = if target == "-" {
        // Read HTML from stdin
        let mut raw_html = String::new();
        io::stdin()
            .read_to_string(&mut raw_html)
            .context("Failed to read HTML content from standard input")?;

        engine
            .process_html(&raw_html, args.base_url.as_deref(), &extraction_options)
            .context("Failed to process HTML content")?
    } else {
        // Fetch and process remote URL
        engine
            .process_url(&target, &extraction_options)
            .await
            .context(format!("Failed to fetch and process URL: {target}"))?
    };

    let elapsed = start_time.elapsed();

    // Store in cache if enabled
    if let (Some(c), Some(k)) = (cache.as_ref(), cache_key.as_ref()) {
        let _ = c.set(k, &doc, None);
    }

    // -------------------------------------------------------------------------
    // Dedicated Media Action 3: Output Cover URL Only
    // -------------------------------------------------------------------------
    if args.bili_cover {
        if let Some(ref cover_url) = doc.metadata.cover_image {
            output::render_cover(cover_url, args.output_file.as_deref())?;
            return Ok(());
        } else {
            bail!("No cover image found for the specified target");
        }
    }

    // Print diagnostic execution metrics to stderr (leaves stdout clean for agent pipelines)
    if !args.quiet {
        let title_disp = doc.title.as_deref().unwrap_or("<no title>");
        eprintln!(
            "[PageClaw] Finished in {:.2?}: \"{}\" (Status: {}, Tokens: {}, Truncated: {})",
            elapsed, title_disp, doc.status, doc.token_count, doc.truncated
        );
    }

    // Write final content to stdout or designated file
    output::render_output(
        &doc,
        extraction_options.format,
        args.json_ld_only,
        args.output_file.as_deref(),
    )?;

    Ok(())
}

/// Injects or overrides page and order query parameters on a search URL.
fn apply_search_params(url_str: &str, page: Option<u32>, order: Option<&str>) -> String {
    if let Ok(mut parsed) = Url::parse(url_str) {
        let mut pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();

        if let Some(p) = page {
            pairs.retain(|(k, _)| k != "page");
            pairs.push(("page".to_string(), p.to_string()));
        }

        if let Some(ord) = order {
            if !ord.is_empty() {
                pairs.retain(|(k, _)| k != "order");
                pairs.push(("order".to_string(), ord.to_string()));
            }
        }

        parsed.query_pairs_mut().clear().extend_pairs(pairs);
        parsed.to_string()
    } else {
        url_str.to_string()
    }
}
