use crate::{
    batch,
    cli::{CliArgs, CliFormat, SubCommand},
    output, paths,
};
use anyhow::{bail, Context, Result};
use std::io::{self, Read};
use std::path::Path;
use tkless_core::{ExtractionOptions, FetchOptions, TklessEngine, TklessError};
pub async fn run(
    args: &CliArgs,
    fetch_options: &FetchOptions,
    extraction_options: &ExtractionOptions,
) -> Result<bool> {
    // -------------------------------------------------------------------------
    // Execution Branch 1.5: Zero-API-Key Web Search
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Search(ref search_args)) = args.command {
        let search_cdp_endpoint = tkless_core::cdp::resolve_cdp_endpoint(
            search_args.cdp.as_deref().or(args.cdp.as_deref()),
            false,
        );
        let search_cdp_wait = search_args.cdp_wait_ms.or(args.cdp_wait_ms);
        if search_cdp_wait.is_some() && search_cdp_endpoint.is_none() {
            return Err(TklessError::InvalidArgument(
                "--cdp-wait requires an explicit --cdp endpoint".into(),
            )
            .into());
        }
        let mut search_fetch_options = fetch_options.clone();
        if search_args.proxy.is_some() {
            search_fetch_options.proxy = search_args.proxy.clone();
        }
        search_fetch_options.mobile = search_args.mobile || args.mobile;
        let engine = TklessEngine::with_options(search_fetch_options)
            .context("Failed to initialize TkLess engine")?;

        let effective_scope = search_args.scope.as_ref().map(|scope| scope.as_str());
        let effective_query = search_args.query.trim().to_string();

        extraction_options.validate()?;

        if !search_args.fetch_content && (args.schema.is_some() || args.schema_strict) {
            return Err(TklessError::InvalidArgument(
                "search --schema requires --fetch-content".into(),
            )
            .into());
        }
        let cdp_opts = search_cdp_endpoint
            .as_ref()
            .map(|endpoint| tkless_core::CdpOptions {
                allow_private_addresses: args.allow_private_addresses,
                endpoint: endpoint.clone(),
                wait_ms: search_cdp_wait,
                ..Default::default()
            });
        let report = tkless_core::web_search(
            &engine,
            &effective_query,
            &tkless_core::SearchOptions {
                scope: effective_scope.map(str::to_owned),
                limit: search_args.limit,
                cdp: cdp_opts,
                auto_cdp: args.auto_cdp,
                filters: tkless_core::search::SearchFilters {
                    since: search_args.since.clone(),
                    until: search_args.until.clone(),
                    language: search_args.language.clone(),
                    region: search_args.region.clone(),
                },
            },
        )
        .await?;
        let diagnostic_md = if search_args.diagnostics {
            report.markdown_diagnostics()
        } else {
            String::new()
        };
        let payload = if search_args.fetch_content {
            let mut options = extraction_options.clone();
            options.cdp_endpoint = search_cdp_endpoint;
            options.cdp_wait_ms = search_cdp_wait;
            let items = tkless_core::search::fetch_search_results(
                &engine,
                report.results.clone(),
                &options,
                search_args.max_tokens_per_page,
                search_args.dedup,
            )
            .await?;
            match search_args.format {
                CliFormat::Json | CliFormat::Jsonl => {
                    let value = serde_json::json!({"query":report.query,"effective_query":report.effective_query,
                        "status":report.status,"results":items,"diagnostics":report.diagnostics,"warnings":report.warnings});
                    if search_args.format == CliFormat::Jsonl {
                        serde_json::to_string(&value)?
                    } else {
                        serde_json::to_string_pretty(&value)?
                    }
                }
                _ => {
                    tkless_core::format_search_fetch_markdown(&effective_query, &items)
                        + &diagnostic_md
                }
            }
        } else {
            match search_args.format {
                CliFormat::Json | CliFormat::Jsonl => {
                    if search_args.format == CliFormat::Jsonl {
                        serde_json::to_string(&report)?
                    } else {
                        serde_json::to_string_pretty(&report)?
                    }
                }
                _ => {
                    tkless_core::format_search_results_markdown(&effective_query, &report.results)
                        + &diagnostic_md
                }
            }
        };
        output::write_payload(&payload, search_args.output.as_deref())?;
        return Ok(true);
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.55: Autonomous Multi-Hop Deep Web Research
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Research(ref res_args)) = args.command {
        let mut res_fetch_options = fetch_options.clone();
        if res_args.proxy.is_some() {
            res_fetch_options.proxy = res_args.proxy.clone();
        }
        res_fetch_options.mobile = args.mobile;
        let engine = TklessEngine::with_options(res_fetch_options)
            .context("Failed to initialize TkLess engine for deep research")?;

        let options = tkless_core::ResearchOptions {
            depth: res_args.depth,
            breadth: res_args.breadth,
            max_pages: res_args.max_pages,
            max_tokens_per_page: res_args.max_tokens_per_page,
            max_tokens: Some(res_args.max_tokens),
            dedup: !res_args.no_dedup,
            format: res_args.format.into(),
        };

        let report = tkless_core::deep_research(&engine, &res_args.topic, &options)
            .await
            .context("Failed to execute autonomous deep research")?;

        let payload = tkless_core::research::render_deep_research(&report, &options)?;

        output::write_payload(&payload, res_args.output.as_deref())?;
        return Ok(true);
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
            include_json_ld: matches!(batch_args.format, CliFormat::Json | CliFormat::Jsonl),
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
            schema_strict: batch_args.schema_strict || args.schema_strict,
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
            cdp_block_resources: !args.cdp_no_block_resources,
            cdp_auto_scroll: args.cdp_auto_scroll,
            summarize: args.summarize,
            skills_pages: args.skills_pages as usize,
            skills_limit: args.skills_limit as usize,
        });

        let job = tkless_core::batch::BatchOptions {
            concurrency: batch_args.concurrency,
            delay_ms: batch_args.delay_ms,
            per_host_concurrency: batch_args.per_host_concurrency,
            per_host_delay_ms: batch_args.per_host_delay_ms,
            order: batch_args.order.into(),
            checkpoint: batch_args.checkpoint.clone(),
            resume: batch_args.resume,
            retry_failed: batch_args.retry_failed,
        };
        let summary = batch::execute(
            &engine,
            &urls,
            &batch_extraction_opts,
            &job,
            batch::BatchOutput {
                format: batch_args.format,
                path: batch_args.output.as_deref(),
                input: (batch_args.file != "-").then(|| Path::new(&batch_args.file)),
                quiet: args.quiet,
            },
        )
        .await?;
        if !args.quiet {
            eprintln!(
                "[TkLess Batch] Completed: {} succeeded, {} failed, {} restored, {} fetched",
                summary.succeeded, summary.failed, summary.resumed, summary.fetched
            );
        }
        if summary.failed > 0 && !batch_args.allow_partial {
            bail!("Batch completed with {} failed URL(s) out of {}. See per-URL results; use --allow-partial to accept partial failure.", summary.failed, summary.total);
        }
        return Ok(true);
    }

    // -------------------------------------------------------------------------
    // Execution Branch 1.7: Deep Site Crawl & Knowledge Base Assembly
    // -------------------------------------------------------------------------
    if let Some(SubCommand::Crawl(ref crawl_args)) = args.command {
        paths::distinct(
            crawl_args.output.as_deref(),
            crawl_args.checkpoint.as_deref(),
        )?;
        let mut crawl_fetch_options = fetch_options.clone();
        if crawl_args.proxy.is_some() {
            crawl_fetch_options.proxy = crawl_args.proxy.clone();
        }
        crawl_fetch_options.mobile = crawl_args.mobile || args.mobile;
        let engine = TklessEngine::with_options(crawl_fetch_options)
            .context("Failed to initialize TkLess engine")?;

        let crawl_options = tkless_core::CrawlOptions {
            retry_failed: crawl_args.retry_failed,
            checkpoint: crawl_args.checkpoint.clone(),
            resume: crawl_args.resume,
            max_seconds: crawl_args.max_seconds,
            max_output_bytes: crawl_args.max_output_bytes,
            max_pending_urls: crawl_args.max_pending_urls,
            respect_robots: crawl_args.respect_robots,
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

        payload = output::apply_budget(
            payload,
            crawl_args.format,
            args.max_tokens,
            args.tokenizer.into(),
        )?;

        output::write_payload(&payload, crawl_args.output.as_deref())?;
        let failed = report
            .pages
            .iter()
            .filter(|page| page.error.is_some())
            .count();
        if (!report.complete || failed > 0) && !crawl_args.allow_partial {
            bail!("Crawl incomplete ({}), {failed} failed page(s), {} pending URL(s); use --allow-partial to accept partial results.", report.stop_reason, report.pending_urls);
        }
        return Ok(true);
    }

    Ok(false)
}
