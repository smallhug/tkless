mod cli;
mod output;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser};
use cli::{CliArgs, CliFormat, SubCommand};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;
use tkless_core::adapters::BilibiliAdapter;
use tkless_core::downloader::{format_bytes, DownloadProgressReporter};
use tkless_core::model::{ExtractionOptions, FetchOptions};
use tkless_core::{DocumentCache, McpServer, TklessEngine, TklessError};
use tokio::io::{AsyncBufReadExt, BufReader};
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
        eprintln!("[TkLess] Starting {} download{}...", self.label, total_str);
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
                "\r[TkLess] Downloading {}: {:>5.1}% ({:>9} / {:>9}, {:>10})   ",
                self.label, percent, downloaded_str, total_str, speed_str
            );
        } else {
            eprint!(
                "\r[TkLess] Downloading {}: {:>9} ({:>10})   ",
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
            "-".to_string()
        };
        eprintln!(
            "\r[TkLess] {} download complete: {} in {:.2?} ({})                    ",
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
        let exit_code = if let Some(pe) = err.downcast_ref::<TklessError>() {
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
            0 => "tkless=warn,tkless_core=warn",
            1 => "tkless=info,tkless_core=info",
            2 => "tkless=debug,tkless_core=debug",
            _ => "tkless=trace,tkless_core=trace",
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
                "[TkLess warning] Skipping invalid header format (expected Key: Value): '{h}'"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Execution Branch 0: Shell Auto-Completions Generation
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Completions(ref comp_args)) = args.command {
        let shell = comp_args.shell;
        std::thread::Builder::new()
            .name("tkless-completions".to_string())
            .stack_size(4 * 1024 * 1024)
            .spawn(move || {
                let mut cmd = cli::CliArgs::command();
                clap_complete::generate(shell, &mut cmd, "tkless", &mut io::stdout());
            })
            .expect("Failed to spawn completion thread")
            .join()
            .expect("Completion thread failed");
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Execution Branch 0.1: Interactive Browser QR Code Login
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Login(ref login_args)) = args.command {
        let platform_clean = login_args.platform.trim().to_ascii_lowercase();
        match platform_clean.as_str() {
            "bilibili" | "bili" => {
                let login_options = tkless_core::auth::LoginOptions {
                    output_path: login_args.output.clone(),
                    no_browser: login_args.no_browser,
                    timeout_secs: login_args.timeout_secs,
                };

                tkless_core::auth::execute_login_flow(None, login_options).await?;
                return Ok(());
            }
            "zhihu" => {
                let login_url = "https://www.zhihu.com/signin";
                eprintln!("打开浏览器授权登录zhihu：{login_url}");
                if !login_args.no_browser {
                    if let Err(e) = tkless_core::auth::open_browser(login_url) {
                        eprintln!("[TkLess warning] Failed to launch browser: {e}");
                    }
                }
                eprintln!("[TkLess] 登录完成后，可将 Cookie 凭据按前缀格式保存至 cookies.txt（自动区分多网站，互不覆盖）：");
                eprintln!("  zhihu: d_c0=\"...\"; z_c0=\"...\"");
                eprintln!("或直接使用命令行参数: tkless <URL> --cookie \"...\"");
                return Ok(());
            }
            other => {
                bail!("Unsupported login platform: '{other}'. Currently supported platforms: bilibili, zhihu");
            }
        }
    }

    let default_cookie_path = std::path::Path::new("./cookies.txt");
    let target_url_ref = args.target.as_deref();

    let cookie_file_path = if let Some(ref cf) = args.cookie_file {
        Some(cf.to_string_lossy().to_string())
    } else if default_cookie_path.is_file() {
        Some(default_cookie_path.to_string_lossy().to_string())
    } else {
        None
    };

    // Print friendly loading log if target URL is specified upfront
    if !args.quiet && args.cookie.is_none() {
        if let Some(ref cf) = cookie_file_path {
            if let Ok(c) = tkless_core::fetcher::load_cookie_from_file_for_url(
                std::path::Path::new(cf),
                target_url_ref,
            ) {
                if !c.trim().is_empty() {
                    if let Some(platform) =
                        target_url_ref.and_then(tkless_core::fetcher::detect_platform_from_url)
                    {
                        eprintln!(
                            "[TkLess] Automatically loaded [{platform}] credentials from {cf}"
                        );
                    }
                }
            }
        }
    }

    let initial_target_host = target_url_ref
        .and_then(|t| url::Url::parse(t).ok())
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()));

    // Build network fetch options
    let fetch_options = FetchOptions {
        user_agent: args.user_agent.clone(),
        timeout_secs: args.timeout_secs,
        connect_timeout_secs: args.connect_timeout,
        retries: args.retries,
        max_bytes: args.max_size_mb * 1024 * 1024,
        on_too_large: args.on_too_large.into(),
        headers: custom_headers,
        cookie: args.cookie.clone(),
        cookie_file: cookie_file_path,
        initial_target_host,
        allow_private_addresses: args.allow_private_addresses,
        allow_http_errors: args.allow_http_errors,
        proxy: args.proxy.clone(),
        mobile: args.mobile,
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
        cdp_endpoint: args.cdp.clone(),
        cdp_wait_ms: args.cdp_wait_ms,
        schema: args.schema.clone(),
        chunk: args.chunk,
        chunk_size: args.chunk_size,
        chunk_overlap: args.chunk_overlap,
        download_images: args.download_images,
        assets_dir: args.assets_dir.clone(),
        tables_only: args.tables_only,
        table_format: args.table_format.into(),
        stats: args.stats,
        remove_selector: args.remove_selector.clone(),
        auto_cdp: args.auto_cdp,
        summarize: args.summarize,
    };

    // -------------------------------------------------------------------------
    // Execution Branch 0.5: Local Cache Inspection and Management
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Cache(ref cache_args)) = args.command {
        let dir = cache_args
            .cache_dir
            .clone()
            .or_else(|| args.cache_dir.clone());
        let cache = DocumentCache::new(dir, args.cache_ttl);
        if cache_args.clear {
            cache.clear().context("Failed to clear cache")?;
            println!("✅ 本地缓存已成功清空。");
            return Ok(());
        }

        let stats = cache.stats();
        println!("# 🗄️ TkLess 本地响应缓存状态 (Cache Status)\n");
        println!(
            "- **存储路径**: {}",
            stats.cache_dir.unwrap_or_else(|| "内存缓存".to_string())
        );
        println!("- **内存活跃条目数 (L1)**: {}", stats.memory_entries);
        println!("- **磁盘条目总数 (L2)**: {}", stats.disk_entries);
        println!(
            "- **磁盘总占用空间**: {}",
            tkless_core::downloader::format_bytes(stats.disk_bytes)
        );
        return Ok(());
    }

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

        let engine = TklessEngine::with_options(fetch_options)
            .context("Failed to initialize TkLess engine")?;
        let mcp_server = McpServer::new(engine, cache);
        return mcp_server.run_stdio().await.map_err(Into::into);
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.2: Interactive Exploratory REPL Shell
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Shell(_)) = args.command {
        let engine = TklessEngine::with_options(fetch_options)
            .context("Failed to initialize TkLess engine")?;
        return run_shell(&engine, &extraction_options).await;
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.3: Document Version Diff Analysis
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Diff(ref diff_args)) = args.command {
        let engine = TklessEngine::with_options(fetch_options)
            .context("Failed to initialize TkLess engine")?;
        let old_text = load_target_text(&diff_args.old_target, &engine).await?;
        let new_text = load_target_text(&diff_args.new_target, &engine).await?;
        let report = tkless_core::diff::compute_diff(&old_text, &new_text);

        if matches!(diff_args.format, CliFormat::Json | CliFormat::Jsonl) {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("# 🔄 文档版本差异与变更分析 (Content Diff Report)\n");
            println!(
                "> 新增行数: `+{}` | 删除行数: `-{}` | 未变行数: `{}` | 变更比例: `{:.1}%`\n",
                report.added_lines,
                report.removed_lines,
                report.unchanged_lines,
                report.change_ratio
            );
            println!("```diff\n{}\n```", report.unified_diff);
        }
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.4: Webpage Tables Extraction & SQL/CSV Export
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Tables(ref tables_args)) = args.command {
        let engine = TklessEngine::with_options(fetch_options)
            .context("Failed to initialize TkLess engine")?;
        let html = load_target_html(&tables_args.target, &engine).await?;
        let doc = scraper::Html::parse_document(&html);
        let mut tables = tkless_core::extractor::extract_tables(&doc);

        if tables.is_empty() {
            bail!("未在目标网页或 HTML 中检测到有效的 <table> 表格元素");
        }

        if let Some(idx) = tables_args.table_index {
            if idx >= tables.len() {
                bail!(
                    "table_index {} 超出范围，页面仅包含 {} 个表格 (有效索引: 0-{})",
                    idx,
                    tables.len(),
                    tables.len().saturating_sub(1)
                );
            }
            tables = vec![tables.swap_remove(idx)];
        }

        let table_format: tkless_core::model::TableExportFormat = tables_args.format.into();
        if table_format == tkless_core::model::TableExportFormat::Sql {
            let ddl = tables
                .iter()
                .enumerate()
                .map(|(idx, t)| {
                    let name = if tables.len() > 1 {
                        format!("{}_{}", tables_args.table_name, idx + 1)
                    } else {
                        tables_args.table_name.clone()
                    };
                    t.to_sql(&name)
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            println!("{ddl}");
        } else {
            let formatted =
                tkless_core::model::ExtractedTable::format_tables(&tables, table_format);
            println!("{formatted}");
        }
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.5: Zero-API-Key Web Search
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Search(ref search_args)) = args.command {
        let mut search_fetch_options = fetch_options.clone();
        if search_args.proxy.is_some() {
            search_fetch_options.proxy = search_args.proxy.clone();
        }
        search_fetch_options.mobile = search_args.mobile || args.mobile;
        let engine = TklessEngine::with_options(search_fetch_options)
            .context("Failed to initialize TkLess engine")?;

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
        } else if search_args.csdn {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:blog.csdn.net {clean}")
            }
        } else if search_args.medium {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:medium.com {clean}")
            }
        } else if search_args.substack {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:substack.com {clean}")
            }
        } else if search_args.arxiv {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:arxiv.org {clean}")
            }
        } else if search_args.hf {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:huggingface.co {clean}")
            }
        } else if search_args.wiki {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:wikipedia.org {clean}")
            }
        } else if search_args.reddit {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:reddit.com {clean}")
            }
        } else if search_args.so {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:stackoverflow.com {clean}")
            }
        } else if search_args.v2ex {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:v2ex.com {clean}")
            }
        } else if search_args.baike {
            let clean = search_args.query.trim();
            if clean.starts_with("site:") {
                clean.to_string()
            } else {
                format!("site:baike.baidu.com {clean}")
            }
        } else {
            search_args.query.clone()
        };

        if search_args.fetch_content {
            let mut search_opts = extraction_options.clone();
            if search_args.cdp.is_some() {
                search_opts.cdp_endpoint = search_args.cdp.clone();
                search_opts.cdp_wait_ms = search_args.cdp_wait_ms;
            }

            let items = tkless_core::web_search_and_fetch(
                &engine,
                &effective_query,
                search_args.limit,
                search_args.engine.into(),
                &search_opts,
                search_args.max_tokens_per_page,
                search_args.dedup,
            )
            .await
            .context("Failed to perform search and fetch")?;

            let mut payload = match search_args.format {
                CliFormat::Json => serde_json::to_string_pretty(&items)
                    .context("Failed to serialize search & fetch items to JSON")?,
                _ => tkless_core::format_search_fetch_markdown(&effective_query, &items),
            };

            // Enforce overall max-tokens limit if specified
            if let Some(total_limit) = args.max_tokens {
                let initial_tokens =
                    tkless_core::token::count_tokens(&payload, args.tokenizer.into());
                if initial_tokens > total_limit {
                    let (truncated_payload, _) = tkless_core::token::truncate_to_tokens(
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

        let results = tkless_core::web_search_with_engine(
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
            _ => tkless_core::format_search_results_markdown(&effective_query, &results),
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

        let mut batch_fetch_options = fetch_options.clone();
        if batch_args.proxy.is_some() {
            batch_fetch_options.proxy = batch_args.proxy.clone();
        }
        batch_fetch_options.mobile = batch_args.mobile || args.mobile;

        let engine = std::sync::Arc::new(
            TklessEngine::with_options(batch_fetch_options)
                .context("Failed to initialize TkLess engine")?,
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
            cdp_endpoint: batch_args.cdp.clone().or_else(|| args.cdp.clone()),
            cdp_wait_ms: batch_args.cdp_wait_ms.or(args.cdp_wait_ms),
            schema: batch_args.schema.clone().or_else(|| args.schema.clone()),
            chunk: args.chunk,
            chunk_size: args.chunk_size,
            chunk_overlap: args.chunk_overlap,
            download_images: false,
            assets_dir: None,
            tables_only: false,
            table_format: tkless_core::model::TableExportFormat::default(),
            stats: false,
            remove_selector: args.remove_selector.clone(),
            auto_cdp: args.auto_cdp,
            summarize: args.summarize,
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
                                "[TkLess Batch] [{done}/{total}] OK: \"{title_disp}\" ({url})"
                            );
                        }
                        Err(e) => {
                            eprintln!("[TkLess Batch] [{done}/{total}] FAILED ({url}): {e}");
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
        let mut crawl_fetch_options = fetch_options.clone();
        if crawl_args.proxy.is_some() {
            crawl_fetch_options.proxy = crawl_args.proxy.clone();
        }
        crawl_fetch_options.mobile = crawl_args.mobile || args.mobile;
        let engine = TklessEngine::with_options(crawl_fetch_options)
            .context("Failed to initialize TkLess engine")?;

        let crawl_options = tkless_core::CrawlOptions {
            max_depth: crawl_args.max_depth,
            max_pages: crawl_args.max_pages,
            concurrency: crawl_args.concurrency,
            same_domain_only: true,
            same_path_prefix: crawl_args.same_path_prefix,
            exclude_patterns: tkless_core::CrawlOptions::default().exclude_patterns,
            sitemap: crawl_args.sitemap,
            dedup: crawl_args.dedup,
            delay_ms: crawl_args.delay_ms,
        };

        let mut crawl_extraction_options = extraction_options.clone();
        if crawl_args.cdp.is_some() {
            crawl_extraction_options.cdp_endpoint = crawl_args.cdp.clone();
            crawl_extraction_options.cdp_wait_ms = crawl_args.cdp_wait_ms;
        }

        let report = tkless_core::crawl_site(
            &engine,
            &crawl_args.url,
            &crawl_options,
            &crawl_extraction_options,
        )
        .await
        .context("Failed to crawl site")?;

        let mut payload = match crawl_args.format {
            CliFormat::Json => serde_json::to_string_pretty(&report)
                .context("Failed to serialize crawl report to JSON")?,
            CliFormat::Jsonl => tkless_core::format_crawl_report_jsonl(&report),
            _ => tkless_core::format_crawl_report_markdown(&report),
        };

        if let Some(total_limit) = args.max_tokens {
            let initial_tokens = tkless_core::token::count_tokens(&payload, args.tokenizer.into());
            if initial_tokens > total_limit {
                let (truncated_payload, _) = tkless_core::token::truncate_to_tokens(
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
            None => bail!("Missing target URL, '--bili-search <KEYWORD>', or '-'. See 'tkless --help' for available options."),
        }
    };

    let start_time = Instant::now();
    let engine =
        TklessEngine::with_options(fetch_options).context("Failed to initialize TkLess engine")?;

    // Auto-normalize target (e.g. bare BV1xx411c7mD -> https://www.bilibili.com/video/BV1xx411c7mD)
    let target = tkless_core::adapters::bilibili::normalize_bilibili_url(&raw_target);

    // -------------------------------------------------------------------------
    // Dedicated Media Action 1: Download Video Stream
    // -------------------------------------------------------------------------
    if let Some(ref dest_val) = args.bili_dl {
        let (bvid, default_page) = tkless_core::adapters::bilibili::extract_video_info(&target)
            .context("The --bili-dl option requires a valid Bilibili video URL or BV ID")?;

        let page_num = args.bili_page.map(|p| p as usize).unwrap_or(default_page);

        let user_dest = if dest_val.is_empty() {
            None
        } else {
            Some(Path::new(dest_val))
        };

        let reporter = ConsoleProgressReporter::new("Video", args.quiet);
        let saved_path = BilibiliAdapter::download_video(
            engine.fetcher(),
            &bvid,
            page_num,
            args.bili_quality.into(),
            user_dest,
            Some(&reporter),
        )
        .await
        .context("Failed to download video stream")?;

        if !args.quiet {
            eprintln!("[TkLess] Saved video to: {}", saved_path.display());
        }
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Dedicated Media Action 2: Download Video Cover
    // -------------------------------------------------------------------------
    if let Some(ref dest_val) = args.bili_dl_cover {
        let (bvid, _) = tkless_core::adapters::bilibili::extract_video_info(&target)
            .context("The --bili-dl-cover option requires a valid Bilibili video URL or BV ID")?;

        let user_dest = if dest_val.is_empty() {
            None
        } else {
            Some(Path::new(dest_val))
        };

        let reporter = ConsoleProgressReporter::new("Cover", args.quiet);
        let saved_path =
            BilibiliAdapter::download_cover(engine.fetcher(), &bvid, user_dest, Some(&reporter))
                .await
                .context("Failed to download video cover image")?;

        if !args.quiet {
            eprintln!("[TkLess] Saved cover to: {}", saved_path.display());
        }
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Dedicated Visual Action: Capture Screenshot via CDP Headless Browser
    // -------------------------------------------------------------------------
    if let Some(ref shot_path) = args.screenshot {
        let cdp_ep = tkless_core::cdp::resolve_cdp_endpoint(args.cdp.as_deref(), true)
            .unwrap_or_else(|| "http://127.0.0.1:9222".to_string());
        let parsed_url = url::Url::parse(&target)?;
        let cdp_opts = tkless_core::cdp::CdpOptions {
            endpoint: cdp_ep,
            wait_ms: args.cdp_wait_ms.or(Some(1000)),
            timeout_ms: None,
            user_agent: args.user_agent.clone(),
            cookie: engine.fetcher().resolve_cookie_for_url(&parsed_url),
        };
        let img_bytes = tkless_core::capture_screenshot_via_cdp(&target, &cdp_opts, None).await?;
        std::fs::write(shot_path, &img_bytes)?;
        if !args.quiet {
            eprintln!(
                "[TkLess] Successfully saved visual screenshot ({}) to: {}",
                tkless_core::downloader::format_bytes(img_bytes.len() as u64),
                shot_path.display()
            );
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
                    "[TkLess] Cache hit: \"{}\" (Tokens: {})",
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
                args.stream,
            );
        }
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.8: DOM Structure Skeleton & Schema Scaffolding
    // -------------------------------------------------------------------------
    if args.scaffold {
        let raw_html = if target == "-" {
            let mut stdin_content = String::new();
            io::stdin()
                .read_to_string(&mut stdin_content)
                .context("Failed to read HTML content from standard input")?;
            stdin_content
        } else if let Some(ref cdp_endpoint) = args.cdp {
            let parsed_url = url::Url::parse(&target)?;
            let cdp_opts = tkless_core::cdp::CdpOptions {
                endpoint: cdp_endpoint.clone(),
                wait_ms: args.cdp_wait_ms.or(Some(1000)),
                timeout_ms: None,
                user_agent: args.user_agent.clone(),
                cookie: engine.fetcher().resolve_cookie_for_url(&parsed_url),
            };
            tkless_core::cdp::fetch_html_via_cdp(&target, &cdp_opts).await?
        } else {
            let resp = engine.fetcher().fetch(&target).await?;
            resp.html
        };

        let report = tkless_core::schema::scaffold_dom_skeleton(&raw_html);
        output::write_payload(&report.summary_markdown, args.output_file.as_deref())?;
        return Ok(());
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
        // Fetch and process remote URL (with HTTP 304 conditional revalidation when caching is enabled)
        if let Some(ref c) = cache {
            engine
                .process_url_with_cache(&target, &extraction_options, c)
                .await
                .context(format!("Failed to fetch and process URL: {target}"))?
        } else {
            engine
                .process_url(&target, &extraction_options)
                .await
                .context(format!("Failed to fetch and process URL: {target}"))?
        }
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
            "[TkLess] Finished in {:.2?}: \"{}\" (Status: {}, Tokens: {}, Truncated: {})",
            elapsed, title_disp, doc.status, doc.token_count, doc.truncated
        );

        if args.stats {
            if let Some(ref m) = doc.metrics {
                eprintln!("\n[TkLess Telemetry & Token Economy]");
                eprintln!("----------------------------------------------------------");
                eprintln!("Target URL        : {}", doc.url);
                eprintln!(
                    "Pipeline Latency  : {}ms (Fetch: {}ms, Clean: {}ms)",
                    m.total_time_ms, m.fetch_time_ms, m.clean_time_ms
                );
                eprintln!(
                    "Payload Reduction : {} -> {} (降噪率: {:.1}%)",
                    tkless_core::downloader::format_bytes(m.raw_bytes as u64),
                    tkless_core::downloader::format_bytes(m.cleaned_bytes as u64),
                    m.noise_reduction_percent
                );
                eprintln!(
                    "Token Economy     : ~{} (Raw) -> {} (Clean) [节省 {} Tokens, 节省率: {:.1}%]",
                    m.raw_token_estimate,
                    m.final_token_count,
                    m.raw_token_estimate.saturating_sub(m.final_token_count),
                    m.token_savings_percent
                );
                eprintln!("----------------------------------------------------------\n");
            }
        }
    }

    // Multi-file directory export divided by headings if --out-dir is specified
    if let Some(ref out_dir) = args.out_dir {
        let sections = tkless_core::split_markdown_by_headings(&doc.content, 2);
        let _written =
            tkless_core::export_sections_to_dir(&sections, out_dir, doc.title.as_deref())?;
        if !args.quiet {
            eprintln!(
                "[TkLess] Successfully exported {} sections and index.md to: {}",
                sections.len(),
                out_dir.display()
            );
        }
        return Ok(());
    }

    // Write final content to stdout or designated file
    output::render_output(
        &doc,
        extraction_options.format,
        args.json_ld_only,
        args.output_file.as_deref(),
        args.stream,
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

async fn load_target_text(target: &str, engine: &TklessEngine) -> Result<String> {
    if target.starts_with("http://") || target.starts_with("https://") {
        let doc = engine
            .process_url(
                target,
                &ExtractionOptions {
                    mode: tkless_core::model::ExtractionMode::Article,
                    format: tkless_core::model::OutputFormat::Markdown,
                    ..Default::default()
                },
            )
            .await
            .context("Failed to process URL")?;
        Ok(doc.content)
    } else {
        std::fs::read_to_string(target)
            .with_context(|| format!("Failed to read target file '{target}'"))
    }
}

async fn load_target_html(target: &str, engine: &TklessEngine) -> Result<String> {
    if target == "-" {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .context("Failed to read HTML from stdin")?;
        Ok(buffer)
    } else if target.starts_with("http://") || target.starts_with("https://") {
        let parsed = Url::parse(target).context("Invalid target URL")?;
        engine
            .fetcher()
            .validate_ssrf(&parsed)
            .await
            .context("SSRF blocked")?;
        let resp = engine
            .fetcher()
            .fetch(target)
            .await
            .context("Failed to fetch target URL")?;
        Ok(resp.html)
    } else {
        std::fs::read_to_string(target)
            .with_context(|| format!("Failed to read HTML file '{target}'"))
    }
}

/// Interactive REPL shell session for exploratory scraping, search, and feed inspection.
async fn run_shell(engine: &TklessEngine, base_options: &ExtractionOptions) -> Result<()> {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║   TkLess Interactive Shell (REPL 探索会话模式)                 ║");
    println!("║   输入 'help' 查看指令指南，输入 'exit' 或 'quit' 退出会话      ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    loop {
        eprint!("tkless> ");
        let _ = io::stderr().flush();

        let line = match lines.next_line().await {
            Ok(Some(l)) => l.trim().to_string(),
            _ => break,
        };

        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();

        match cmd {
            "exit" | "quit" | "q" => {
                println!("再见！");
                break;
            }
            "help" | "?" => {
                println!("\n可用指令 (Available Commands):");
                println!("  fetch <url>         - 抓取网页并输出清洗后的 Markdown");
                println!("  search <query>      - 免 API Key 全网搜索并返回结果列表");
                println!("  feed <url>          - 解析并展示 RSS 2.0 / Atom 1.0 订阅源");
                println!("  tables <url>        - 提取页面中的全部表格并以 GFM Markdown 输出");
                println!("  summarize <url>     - 本地启发式提取页面核心观点要点速览");
                println!("  chunk <url>         - 将长文档按标题与 Token 预算语义感知切块");
                println!("  diff <url1> <url2>  - 对比两个网页或文档版本的版本差异与变更比例");
                println!("  help                - 显示此帮助信息");
                println!("  exit / quit         - 退出交互式会话\n");
            }
            "fetch" => {
                if let Some(target_url) = rest.first() {
                    match engine.process_url(target_url, base_options).await {
                        Ok(doc) => {
                            println!("\n{}\n", doc.content);
                        }
                        Err(e) => {
                            eprintln!("❌ 抓取失败: {e}");
                        }
                    }
                } else {
                    eprintln!("用法: fetch <url>");
                }
            }
            "search" => {
                let query = rest.join(" ");
                if query.is_empty() {
                    eprintln!("用法: search <query>");
                } else {
                    match tkless_core::search::web_search_with_engine(
                        engine.client(),
                        &query,
                        5,
                        tkless_core::model::SearchEngineKind::Bing,
                    )
                    .await
                    {
                        Ok(results) => {
                            let md = tkless_core::search::format_search_results_markdown(
                                &query, &results,
                            );
                            println!("\n{md}\n");
                        }
                        Err(e) => eprintln!("❌ 搜索失败: {e}"),
                    }
                }
            }
            "feed" => {
                if let Some(feed_url) = rest.first() {
                    match engine.fetcher().fetch(feed_url).await {
                        Ok(resp) => match tkless_core::feed::parse_feed(&resp.html) {
                            Ok(channel) => {
                                println!(
                                    "\n{}\n",
                                    tkless_core::feed::format_feed_markdown(&channel)
                                );
                            }
                            Err(e) => eprintln!("❌ 解析订阅源失败: {e}"),
                        },
                        Err(e) => eprintln!("❌ 获取订阅源失败: {e}"),
                    }
                } else {
                    eprintln!("用法: feed <url>");
                }
            }
            "tables" | "table" => {
                if let Some(target_url) = rest.first() {
                    match engine.fetcher().fetch(target_url).await {
                        Ok(resp) => {
                            let doc = scraper::Html::parse_document(&resp.html);
                            let tables = tkless_core::extractor::extract_tables(&doc);
                            if tables.is_empty() {
                                eprintln!("⚠️ 未在页面中检测到 <table> 表格元素");
                            } else {
                                let formatted = tkless_core::model::ExtractedTable::format_tables(
                                    &tables,
                                    tkless_core::model::TableExportFormat::Markdown,
                                );
                                println!("\n{formatted}\n");
                            }
                        }
                        Err(e) => eprintln!("❌ 获取网页失败: {e}"),
                    }
                } else {
                    eprintln!("用法: tables <url>");
                }
            }
            "summarize" | "summary" => {
                if let Some(target_url) = rest.first() {
                    match engine.process_url(target_url, base_options).await {
                        Ok(doc) => {
                            let bullets =
                                tkless_core::summarizer::extract_key_takeaways(&doc.content, 4);
                            println!("\n### 💡 核心要点速览 (Key Takeaways):\n");
                            for b in bullets {
                                println!("- {}", b.trim());
                            }
                            println!();
                        }
                        Err(e) => eprintln!("❌ 提取失败: {e}"),
                    }
                } else {
                    eprintln!("用法: summarize <url>");
                }
            }
            "chunk" => {
                if let Some(target_url) = rest.first() {
                    match engine.process_url(target_url, base_options).await {
                        Ok(doc) => {
                            let chunks = tkless_core::chunker::chunk_markdown(
                                &doc.content,
                                500,
                                50,
                                tkless_core::model::TokenizerKind::O200k,
                            );
                            println!("\n# ✂️ 语义分块输出 (共 {} 块):\n", chunks.len());
                            for c in chunks {
                                let path = if c.heading_path.is_empty() {
                                    "正文".to_string()
                                } else {
                                    c.heading_path.join(" > ")
                                };
                                println!(
                                    "### 📦 [Chunk {}] Tokens: {} | 标题: {}\n\n{}\n",
                                    c.index + 1,
                                    c.token_count,
                                    path,
                                    c.content
                                );
                            }
                        }
                        Err(e) => eprintln!("❌ 提取切块失败: {e}"),
                    }
                } else {
                    eprintln!("用法: chunk <url>");
                }
            }
            "diff" => {
                if rest.len() >= 2 {
                    let url1 = rest[0];
                    let url2 = rest[1];
                    let doc1 = engine.process_url(url1, base_options).await;
                    let doc2 = engine.process_url(url2, base_options).await;
                    match (doc1, doc2) {
                        (Ok(d1), Ok(d2)) => {
                            let rep = tkless_core::diff::compute_diff(&d1.content, &d2.content);
                            println!("\n# 🔄 页面差异比对结果:\n");
                            println!(
                                "> 新增: +{} | 删除: -{} | 变更比例: {:.1}%\n",
                                rep.added_lines, rep.removed_lines, rep.change_ratio
                            );
                            println!("```diff\n{}\n```\n", rep.unified_diff);
                        }
                        (Err(e), _) => eprintln!("❌ 抓取第一篇失败: {e}"),
                        (_, Err(e)) => eprintln!("❌ 抓取第二篇失败: {e}"),
                    }
                } else {
                    eprintln!("用法: diff <url1> <url2>");
                }
            }
            other => {
                eprintln!("未知指令: '{other}'。输入 'help' 查看可用指令。");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_command_build() {
        let cmd = cli::CliArgs::command();
        assert_eq!(cmd.get_name(), "tkless");
    }

    #[test]
    fn test_completions_generation() {
        let mut cmd = cli::CliArgs::command();
        let mut buf = Vec::new();
        clap_complete::generate(clap_complete::Shell::Bash, &mut cmd, "tkless", &mut buf);
        assert!(!buf.is_empty());
    }
}
