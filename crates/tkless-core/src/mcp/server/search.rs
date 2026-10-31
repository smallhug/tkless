use super::{ExtractionOptions, McpServer, McpToolCallResult, OutputFormat, TklessEngine, Value};

impl McpServer {
    pub(super) async fn call_web_search(&self, args: &Value) -> McpToolCallResult {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'query'"),
        };

        let limit = match args.get("limit") {
            None => 10,
            Some(value) => match value.as_u64().filter(|value| (1..=20).contains(value)) {
                Some(value) => value as usize,
                None => {
                    return McpToolCallResult::error("limit must be an integer between 1 and 20")
                }
            },
        };
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };
        let scope = match args.get("scope") {
            None => None,
            Some(Value::String(value)) => Some(value.as_str()),
            _ => return McpToolCallResult::error("scope must be a platform name"),
        };
        let effective_scope = scope;
        let effective_query = query.to_string();

        let fetch_content = args
            .get("fetch_content")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_tokens_per_page = args
            .get("max_tokens_per_page")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let mobile = args
            .get("mobile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let dedup = args.get("dedup").and_then(|v| v.as_bool()).unwrap_or(false);

        let allow_private = args.get("allow_private_addresses").and_then(Value::as_bool);
        let custom_engine;
        let engine_ref = if proxy.is_some() || mobile || allow_private.is_some() {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            if let Some(ref p) = proxy {
                fetch_opts.proxy = Some(p.clone());
            }
            if mobile {
                fetch_opts.mobile = true;
            }
            if let Some(allow) = allow_private {
                fetch_opts.allow_private_addresses = allow;
            }
            match TklessEngine::with_options(fetch_opts) {
                Ok(eng) => {
                    custom_engine = eng;
                    &custom_engine
                }
                Err(error) => return McpToolCallResult::error(error.to_string()),
            }
        } else {
            &self.engine
        };

        let options = ExtractionOptions {
            cdp_endpoint: args
                .get("cdp_endpoint")
                .and_then(Value::as_str)
                .map(str::to_owned),
            cdp_wait_ms: args.get("cdp_wait_ms").and_then(Value::as_u64),
            auto_cdp: args
                .get("auto_cdp")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            schema: args.get("schema").map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            }),
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ..Default::default()
        };
        if let Err(error) = options.validate() {
            return McpToolCallResult::from_error(&error);
        }
        let cdp = options
            .cdp_endpoint
            .as_ref()
            .map(|endpoint| crate::CdpOptions {
                allow_private_addresses: engine_ref.fetcher().options().allow_private_addresses,
                endpoint: endpoint.clone(),
                wait_ms: options.cdp_wait_ms,
                ..Default::default()
            });
        let report = match crate::web_search(
            engine_ref,
            &effective_query,
            &crate::SearchOptions {
                scope: effective_scope.map(str::to_owned),
                limit,
                cdp,
                auto_cdp: options.auto_cdp,
                filters: crate::search::SearchFilters {
                    since: args.get("since").and_then(Value::as_str).map(str::to_owned),
                    until: args.get("until").and_then(Value::as_str).map(str::to_owned),
                    language: args
                        .get("language")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    region: args
                        .get("region")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                },
            },
        )
        .await
        {
            Ok(report) => report,
            Err(error) => return McpToolCallResult::from_error(&error),
        };
        let diagnostics = args
            .get("diagnostics")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let diagnostic_md = if diagnostics {
            report.markdown_diagnostics()
        } else {
            String::new()
        };
        let (structured, text) = if fetch_content {
            let items = match crate::search::fetch_search_results(
                engine_ref,
                report.results.clone(),
                &options,
                max_tokens_per_page,
                dedup,
            )
            .await
            {
                Ok(items) => items,
                Err(error) => return McpToolCallResult::from_error(&error),
            };
            let md = crate::search::format_search_fetch_markdown(&effective_query, &items)
                + &diagnostic_md;
            let text = md;
            let value = serde_json::json!({"query":report.query,"effective_query":report.effective_query,"status":report.status,
                "results":items,"diagnostics":report.diagnostics,"warnings":report.warnings});
            (value, text)
        } else {
            let text =
                crate::search::format_search_results_markdown(&effective_query, &report.results)
                    + &diagnostic_md;
            (
                serde_json::to_value(&report).expect("SearchReport is serializable"),
                text,
            )
        };
        let text = if format == OutputFormat::Json {
            serde_json::to_string_pretty(&structured).unwrap_or_default()
        } else {
            text
        };
        McpToolCallResult::success(text).with_structured(structured)
    }

    pub(super) async fn call_deep_research(&self, args: &Value) -> McpToolCallResult {
        let topic = match args.get("topic").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'topic'"),
        };

        let depth = args
            .get("depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .clamp(1, 2) as usize;
        let breadth = args
            .get("breadth")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(2, 5) as usize;
        let max_pages = args
            .get("max_pages")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .clamp(3, 20) as usize;
        let max_tokens_per_page = args
            .get("max_tokens_per_page")
            .and_then(|v| v.as_u64())
            .unwrap_or(1500) as usize;
        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .or(Some(8000));
        let dedup = args.get("dedup").and_then(|v| v.as_bool()).unwrap_or(true);
        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            _ => OutputFormat::Markdown,
        };

        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let options = crate::research::ResearchOptions {
            depth,
            breadth,
            max_pages,
            max_tokens_per_page,
            max_tokens,
            dedup,
            format,
        };

        let custom_engine;
        let engine = if let Some(proxy) = proxy {
            let mut fetch_opts = self.engine.fetcher().options().clone();
            fetch_opts.proxy = Some(proxy);
            match TklessEngine::with_options(fetch_opts) {
                Ok(engine) => {
                    custom_engine = engine;
                    &custom_engine
                }
                Err(e) => {
                    return McpToolCallResult::error(format!(
                        "Failed to initialize engine with proxy: {e}"
                    ))
                }
            }
        } else {
            &self.engine
        };

        match crate::research::deep_research(engine, topic, &options).await {
            Ok(report) => match crate::research::render_deep_research(&report, &options) {
                Ok(text) => McpToolCallResult::success(text),
                Err(e) => McpToolCallResult::error(e.to_string()),
            },
            Err(e) => McpToolCallResult::error(format!("Deep research failed: {e}")),
        }
    }
}
