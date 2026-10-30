use super::{
    ExtractionOptions, McpServer, McpToolCallResult, NotificationSender, OutputFormat,
    TklessEngine, TklessError, TokenizerKind, Value, INTERNAL_ERROR,
};

impl McpServer {
    pub(super) async fn call_crawl_site(
        &self,
        args: &Value,
        notifier: Option<&NotificationSender>,
        progress_token: Option<&Value>,
    ) -> McpToolCallResult {
        let seed_url = match args.get("seed_url").and_then(|v| v.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'seed_url'"),
        };

        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .clamp(0, 5) as usize;
        let max_pages = args
            .get("max_pages")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .clamp(1, 100) as usize;
        let same_path_prefix = args
            .get("same_path_prefix")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let sitemap = args
            .get("sitemap")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let dedup = args.get("dedup").and_then(|v| v.as_bool()).unwrap_or(false);

        let is_jsonl = args.get("format").and_then(|v| v.as_str()) == Some("jsonl");
        let delay_ms = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };

        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let cdp_endpoint = args
            .get("cdp_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let cdp_wait_ms = args.get("cdp_wait_ms").and_then(|v| v.as_u64());
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mobile = args
            .get("mobile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let schema = args.get("schema").map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });

        let crawl_options = crate::crawler::CrawlOptions {
            retry_failed: args
                .get("retry_failed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            checkpoint: args
                .get("checkpoint")
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from),
            resume: args.get("resume").and_then(Value::as_bool).unwrap_or(false),
            max_seconds: args.get("max_seconds").and_then(Value::as_u64),
            max_output_bytes: args.get("max_output_bytes").and_then(Value::as_u64),
            max_pending_urls: args
                .get("max_pending_urls")
                .and_then(Value::as_u64)
                .unwrap_or(10000) as usize,
            respect_robots: args
                .get("respect_robots")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            max_depth,
            max_pages,
            concurrency: args
                .get("concurrency")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 32) as usize,
            same_domain_only: true,
            same_path_prefix,
            exclude_patterns: crate::crawler::CrawlOptions::default().exclude_patterns,
            sitemap,
            dedup,
            delay_ms,
        };

        let extraction_options = ExtractionOptions {
            cdp_endpoint,
            cdp_wait_ms,
            schema,
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ..Default::default()
        };

        let custom_engine;
        let engine_ref = if proxy.is_some() || mobile {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            if let Some(ref p) = proxy {
                fetch_opts.proxy = Some(p.clone());
            }
            if mobile {
                fetch_opts.mobile = true;
            }
            match TklessEngine::with_options(fetch_opts) {
                Ok(eng) => {
                    custom_engine = eng;
                    &custom_engine
                }
                Err(_) => &self.engine,
            }
        } else {
            &self.engine
        };

        let notifier_clone = notifier.cloned();
        let progress_token_clone = progress_token.cloned();

        let on_progress = if let (Some(tx), Some(token)) = (notifier_clone, progress_token_clone) {
            Some(
                move |crawled_count: usize, max_pages: usize, current_url: &str| {
                    let notif = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": {
                            "progressToken": token,
                            "progress": crawled_count,
                            "total": max_pages,
                            "url": current_url
                        }
                    });
                    let _ = tx.try_send(notif);
                },
            )
        } else {
            None
        };

        match crate::crawler::crawl_site_with_progress(
            engine_ref,
            seed_url,
            &crawl_options,
            &extraction_options,
            on_progress,
        )
        .await
        {
            Ok(report) => {
                let mut payload = if is_jsonl {
                    crate::crawler::format_crawl_report_jsonl(&report)
                } else if format == OutputFormat::Json {
                    match serde_json::to_string(&report) {
                        Ok(text) => text,
                        Err(e) => return McpToolCallResult::error(e.to_string()),
                    }
                } else {
                    crate::crawler::format_crawl_report_markdown(&report)
                };
                if let Some(limit) = max_tokens {
                    if (is_jsonl || format == OutputFormat::Json)
                        && crate::token::count_tokens(&payload, TokenizerKind::default()) > limit
                    {
                        return McpToolCallResult::error("JSON report exceeds max_tokens; increase the budget or reduce max_pages");
                    }
                    if !is_jsonl && format != OutputFormat::Json {
                        payload = crate::token::truncate_to_tokens(
                            &payload,
                            limit,
                            TokenizerKind::default(),
                        )
                        .0;
                    }
                }
                if !report.pages.is_empty() && report.pages.iter().all(|page| page.error.is_some())
                {
                    McpToolCallResult::error(payload).with_structured(&report)
                } else {
                    McpToolCallResult::success(payload).with_structured(&report)
                }
            }
            Err(e) => McpToolCallResult::error(format!("Site crawl failed: {e}")),
        }
    }

    pub(super) async fn call_batch_fetch_webpages(
        &self,
        args: &Value,
        notifier: Option<&NotificationSender>,
        progress_token: Option<&Value>,
    ) -> McpToolCallResult {
        let urls = match args.get("urls").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>(),
            _ => return McpToolCallResult::error("Missing or empty 'urls' array"),
        };

        if urls.is_empty() || urls.iter().any(String::is_empty) {
            return McpToolCallResult::invalid_argument(
                "'urls' entries must be non-empty URL strings",
            );
        }

        let concurrency = args
            .get("concurrency")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 20) as usize;

        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };

        let max_tokens = args
            .get("max_tokens_per_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let remove_selector = args
            .get("remove_selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let delay_ms = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(0);

        let schema = args.get("schema").map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });

        let cdp_endpoint = args
            .get("cdp_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let cdp_wait_ms = args.get("cdp_wait_ms").and_then(|v| v.as_u64());
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mobile = args
            .get("mobile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let custom_engine;
        let engine_ref = if proxy.is_some() || mobile {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            if let Some(ref p) = proxy {
                fetch_opts.proxy = Some(p.clone());
            }
            if mobile {
                fetch_opts.mobile = true;
            }
            match TklessEngine::with_options(fetch_opts) {
                Ok(eng) => {
                    custom_engine = eng;
                    std::sync::Arc::new(custom_engine)
                }
                Err(_) => self.engine.clone(),
            }
        } else {
            self.engine.clone()
        };

        let base_options = ExtractionOptions {
            format,
            max_tokens,
            selector,
            remove_selector,
            schema,
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cdp_endpoint,
            cdp_wait_ms,
            ..Default::default()
        };

        let job = crate::batch::BatchOptions {
            concurrency,
            delay_ms,
            per_host_concurrency: args
                .get("per_host_concurrency")
                .and_then(Value::as_u64)
                .unwrap_or(2) as usize,
            per_host_delay_ms: args
                .get("per_host_delay_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            order: if args.get("order").and_then(Value::as_str) == Some("completion") {
                crate::batch::BatchOrder::Completion
            } else {
                crate::batch::BatchOrder::Input
            },
            checkpoint: args
                .get("checkpoint")
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from),
            resume: args.get("resume").and_then(Value::as_bool).unwrap_or(false),
            retry_failed: args
                .get("retry_failed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let stream_results = args
            .get("stream_results")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if stream_results && (notifier.is_none() || progress_token.is_none()) {
            return McpToolCallResult::invalid_argument(
                "stream_results requires an active notification receiver and _meta.progressToken",
            );
        }
        let mut completed = 0usize;
        let total = urls.len();
        let mut results = Vec::new();
        let summary = crate::batch::run_batch(&engine_ref, &urls, &base_options, &job, |item| {
            completed += 1;
            let notification = if stream_results || completed == total || completed.is_multiple_of(total.div_ceil(10).max(1)) {
                progress_token.map(|token| {
                    let mut params = serde_json::json!({"progressToken":token,"progress":completed,"total":total,"message":item.url});
                    if stream_results { params["_meta"] = serde_json::json!({"tkless/batch_item":item}); }
                    serde_json::json!({"jsonrpc":"2.0","method":"notifications/progress","params":params})
                })
            } else { None };
            if !stream_results { results.push(item); }
            let tx = notifier.cloned();
            async move {
                if let (Some(tx), Some(notification)) = (tx, notification) {
                    tx.send(notification).await.map_err(|_| TklessError::McpError { code: INTERNAL_ERROR, message: "Batch notification receiver disconnected".into() })?;
                }
                Ok(())
            }
        }).await;
        let summary = match summary {
            Ok(summary) => summary,
            Err(error) => return McpToolCallResult::from_error(&error),
        };
        let failed = summary.failed > 0 && summary.succeeded == 0;
        let (structured, markdown) = if stream_results {
            let mut value = serde_json::to_value(&summary).expect("BatchSummary is serializable");
            value["streamed"] = serde_json::json!(true);
            let markdown = format!("Batch finished: {} total, {} succeeded, {} failed, {} restored, {} fetched. Results were streamed through progress notifications.", summary.total, summary.succeeded, summary.failed, summary.resumed, summary.fetched);
            (value, markdown)
        } else {
            let report = crate::batch::BatchReport { summary, results };
            (
                serde_json::to_value(&report).expect("BatchReport is serializable"),
                report.markdown(),
            )
        };
        let text = if format == OutputFormat::Json {
            serde_json::to_string_pretty(&structured).unwrap_or_default()
        } else {
            markdown
        };
        let response = if failed {
            McpToolCallResult::error(text)
        } else {
            McpToolCallResult::success(text)
        };
        response.with_structured(structured)
    }
}
