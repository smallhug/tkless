mod batch;
mod cli;
mod commands;
mod doctor;
mod output;
mod paths;
mod progress;
mod shell;
mod upgrade;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser};
use cli::{CliArgs, CliFormat, SubCommand};
use progress::ConsoleProgressReporter;
use std::io::{self, Read};
use std::path::Path;
use std::time::Instant;
use tkless_core::adapters::BilibiliAdapter;
use tkless_core::model::{ExtractionOptions, FetchOptions};
use tkless_core::{DocumentCache, McpServer, TklessEngine, TklessError};
use tracing_subscriber::EnvFilter;
use url::Url;

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
    let structured_command_format = match &args.command {
        Some(SubCommand::Search(a)) => Some(a.format),
        Some(SubCommand::Batch(a)) => Some(a.format),
        Some(SubCommand::Crawl(a)) => Some(a.format),
        Some(SubCommand::Research(a)) => Some(a.format),
        Some(SubCommand::Diff(a)) => Some(a.format),
        Some(SubCommand::Adapters(a)) => Some(a.format),
        _ => None,
    };
    if structured_command_format
        .is_some_and(|f| !matches!(f, CliFormat::Markdown | CliFormat::Json | CliFormat::Jsonl))
    {
        return Err(TklessError::InvalidArgument(
            "This subcommand supports markdown, json, or jsonl".into(),
        )
        .into());
    }
    // Configure tracing logger to output strictly to stderr
    if !args.quiet {
        let filter_str = match args.verbose {
            0 => "off",
            1 => "tkless=warn,tkless_core=warn",
            2 => "tkless=info,tkless_core=info",
            3 => "tkless=debug,tkless_core=debug",
            _ => "tkless=trace,tkless_core=trace",
        };

        let env_filter = EnvFilter::try_new(filter_str).unwrap_or_else(|_| EnvFilter::new("off"));

        use std::io::IsTerminal;
        let is_tty = io::stderr().is_terminal();
        let use_ansi = std::env::var("NO_COLOR").is_err() && is_tty;

        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_env_filter(env_filter)
            .with_ansi(use_ansi)
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
                        if !args.quiet && args.verbose >= 1 {
                            eprintln!(
                                "[TkLess] Automatically loaded [{platform}] credentials from {cf}"
                            );
                        }
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
        initial_target_origin: target_url_ref
            .and_then(|t| url::Url::parse(t).ok())
            .map(|u| u.origin().ascii_serialization()),
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
        include_json_ld: args.json_ld_only
            || matches!(args.format, CliFormat::Json | CliFormat::Jsonl),
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
        schema_strict: args.schema_strict,
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
        cdp_block_resources: !args.cdp_no_block_resources,
        cdp_auto_scroll: args.cdp_auto_scroll,
        summarize: args.summarize,
        skills_pages: args.skills_pages as usize,
        skills_limit: args.skills_limit as usize,
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
            println!("本地缓存已成功清空。");
            return Ok(());
        }

        let stats = cache.stats();
        println!("# TkLess 本地响应缓存状态 (Cache Status)\n");
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
    if let Some(command) = &args.command {
        if commands::workflows::run(command, &fetch_options, &extraction_options).await? {
            return Ok(());
        }
    }
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

        let is_dynamic = args.mcp_dynamic
            || match args.command {
                Some(SubCommand::Mcp(ref mcp_args)) => {
                    mcp_args.dynamic || mcp_args.profile.eq_ignore_ascii_case("dynamic")
                }
                _ => false,
            }
            || std::env::var("TKLESS_MCP_DYNAMIC")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

        let mcp_mode = if is_dynamic {
            tkless_core::mcp::McpMode::Dynamic
        } else {
            tkless_core::mcp::McpMode::Full
        };

        let mcp_server = McpServer::new_with_mode(engine, cache, mcp_mode);
        return mcp_server
            .run_transport(tokio::io::stdin(), tokio::io::stdout())
            .await
            .map_err(Into::into);
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.2: Interactive Exploratory REPL Shell
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Shell(_)) = args.command {
        let engine = TklessEngine::with_options(fetch_options)
            .context("Failed to initialize TkLess engine")?;
        return shell::run_shell(&engine, &extraction_options).await;
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.25: List Built-in Platform Adapters Matrix
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Adapters(ref adapters_args)) = args.command {
        let list = tkless_core::adapters::AdapterRegistry::list_adapter_metadata();
        let payload = if matches!(adapters_args.format, CliFormat::Json | CliFormat::Jsonl) {
            output::serialize_rows(&list, adapters_args.format)?
        } else {
            tkless_core::adapters::format_adapters_markdown(&list)
        };
        output::write_payload(
            &payload,
            adapters_args
                .output
                .as_deref()
                .or(args.output_file.as_deref()),
        )?;
        return Ok(());
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.28: Self-Upgrade from GitHub Releases (like bun upgrade)
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Upgrade(ref upgrade_args)) = args.command {
        let engine = TklessEngine::with_options(fetch_options)
            .context("Failed to initialize TkLess engine")?;
        return upgrade::run_upgrade(&engine, upgrade_args).await;
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.29: Environment Diagnostics & System Doctor
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Doctor(ref doc_args)) = args.command {
        return doctor::run_doctor(doc_args, &fetch_options, args.output_file.as_deref()).await;
    }

    // Execution Branch 1.3: Document Version Diff Analysis
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Diff(ref diff_args)) = args.command {
        let engine = TklessEngine::with_options(fetch_options)
            .context("Failed to initialize TkLess engine")?;
        let old_text = load_target_text(&diff_args.old_target, &engine).await?;
        let new_text = load_target_text(&diff_args.new_target, &engine).await?;
        let report = tkless_core::diff::compute_diff(&old_text, &new_text);

        let payload = if matches!(diff_args.format, CliFormat::Json | CliFormat::Jsonl) {
            output::serialize_json(&report, diff_args.format)?
        } else {
            format!(
                "# 文档版本差异与变更分析 (Content Diff Report)\n\n> 新增行数: `+{}` | 删除行数: `-{}` | 未变行数: `{}` | 变更比例: `{:.1}%`\n\n```diff\n{}\n```",
                report.added_lines,
                report.removed_lines,
                report.unchanged_lines,
                report.change_ratio,
                report.unified_diff
            )
        };
        output::write_payload(
            &payload,
            diff_args.output.as_deref().or(args.output_file.as_deref()),
        )?;
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
        let payload = if table_format == tkless_core::model::TableExportFormat::Sql {
            tables
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
                .join("\n\n")
        } else {
            tkless_core::model::ExtractedTable::format_tables(&tables, table_format)
        };

        output::write_payload(
            &payload,
            tables_args
                .output
                .as_deref()
                .or(args.output_file.as_deref()),
        )?;
        return Ok(());
    }

    if commands::jobs::run(&args, &fetch_options, &extraction_options).await? {
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
        let parsed_url = url::Url::parse(&target)?;
        engine
            .fetcher()
            .validate_ssrf(&parsed_url)
            .await
            .context("SSRF blocked")?;
        let cdp_ep = tkless_core::cdp::resolve_cdp_endpoint(args.cdp.as_deref(), true)
            .unwrap_or_else(|| "http://127.0.0.1:9222".to_string());
        let cdp_opts = tkless_core::cdp::CdpOptions {
            allow_private_addresses: args.allow_private_addresses,
            endpoint: cdp_ep,
            wait_ms: args.cdp_wait_ms.or(Some(1000)),
            timeout_ms: Some(30_000),
            user_agent: args.user_agent.clone(),
            accept_language: None,
            cookie: engine.fetcher().resolve_cookie_for_url(&parsed_url),
            block_resources: Some(false),
            auto_scroll: Some(args.cdp_auto_scroll),
            scroll_count: Some(5),
            ..Default::default()
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
            engine
                .fetcher()
                .validate_ssrf(&parsed_url)
                .await
                .context("SSRF blocked")?;
            let cdp_opts = tkless_core::cdp::CdpOptions {
                allow_private_addresses: args.allow_private_addresses,
                endpoint: cdp_endpoint.clone(),
                wait_ms: args.cdp_wait_ms.or(Some(1000)),
                timeout_ms: None,
                user_agent: args.user_agent.clone(),
                accept_language: None,
                cookie: engine.fetcher().resolve_cookie_for_url(&parsed_url),
                block_resources: Some(!args.cdp_no_block_resources),
                auto_scroll: Some(args.cdp_auto_scroll),
                scroll_count: Some(5),
                ..Default::default()
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
    // Standard Document Extraction / Fetch Pipeline (HTML & Native PDF)
    // -------------------------------------------------------------------------
    let doc = if target == "-" {
        let mut stdin_bytes = Vec::new();
        io::stdin()
            .read_to_end(&mut stdin_bytes)
            .context("Failed to read content from standard input")?;

        if tkless_core::pdf::is_pdf_content(&stdin_bytes) {
            tkless_core::pdf::extract_pdf_to_document(&stdin_bytes, None, &extraction_options)?
        } else {
            let raw_html = String::from_utf8_lossy(&stdin_bytes).to_string();
            engine
                .process_html(&raw_html, args.base_url.as_deref(), &extraction_options)
                .context("Failed to process HTML content")?
        }
    } else if std::path::Path::new(&target).is_file() {
        let file_bytes = std::fs::read(&target)
            .with_context(|| format!("Failed to read local file: {target}"))?;

        if tkless_core::pdf::is_pdf_content(&file_bytes) || tkless_core::pdf::is_pdf_path(&target) {
            tkless_core::pdf::extract_pdf_to_document(
                &file_bytes,
                Some(&target),
                &extraction_options,
            )?
        } else {
            let raw_html = String::from_utf8_lossy(&file_bytes).to_string();
            engine
                .process_html(&raw_html, args.base_url.as_deref(), &extraction_options)
                .context("Failed to process local file content")?
        }
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

    // Print diagnostic execution metrics to stderr only if verbose >= 1 (keeps stderr clean for PowerShell / agent pipelines)
    if !args.quiet && args.verbose >= 1 {
        let title_disp = doc.title.as_deref().unwrap_or("<no title>");
        let status_disp = if target == "-" {
            "200 (stdin)".to_string()
        } else {
            doc.status.to_string()
        };
        eprintln!(
            "[TkLess] Finished in {:.2?}: \"{}\" (Status: {}, Tokens: {}, Truncated: {})",
            elapsed, title_disp, status_disp, doc.token_count, doc.truncated
        );
    }

    // --stats is decoupled from -q: when user explicitly asks for --stats, telemetry is rendered to stdout
    if args.stats {
        if let Some(ref m) = doc.metrics {
            println!("\n[TkLess Telemetry & Token Economy]");
            println!("----------------------------------------------------------");
            println!("Target URL        : {}", doc.url);
            println!(
                "Pipeline Latency  : {}ms (Fetch: {}ms, Clean: {}ms)",
                m.total_time_ms, m.fetch_time_ms, m.clean_time_ms
            );
            println!(
                "Payload Reduction : {} -> {} (降噪率: {:.1}%)",
                tkless_core::downloader::format_bytes(m.raw_bytes as u64),
                tkless_core::downloader::format_bytes(m.cleaned_bytes as u64),
                m.noise_reduction_percent
            );
            println!(
                "Token Economy     : ~{} (Raw) -> {} (Clean) [节省 {} Tokens, 节省率: {:.1}%]",
                m.raw_token_estimate,
                m.final_token_count,
                m.raw_token_estimate.saturating_sub(m.final_token_count),
                m.token_savings_percent
            );
            println!("----------------------------------------------------------\n");
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
        args.format,
        args.json_ld_only,
        args.output_file.as_deref(),
        args.stream,
        &extraction_options,
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

        // Token measurements for CLI help outputs
        use tkless_core::model::TokenizerKind;
        use tkless_core::token::count_tokens;

        let mut h_buf = Vec::new();
        let _ = cmd.write_help(&mut h_buf);
        let h_str = String::from_utf8_lossy(&h_buf);
        println!("\n=== CLI Help Token Metrics ===");
        println!(
            "tkless --help (Full): chars={}, o200k={}, cl100k={}",
            h_str.len(),
            count_tokens(&h_str, TokenizerKind::O200k),
            count_tokens(&h_str, TokenizerKind::Cl100k)
        );
    }
}
