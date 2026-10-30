use super::{
    format_doc_result, ExtractionOptions, McpServer, McpToolCallResult, OutputFormat, TklessEngine,
    TklessError, Value,
};
use url::Url;

impl McpServer {
    pub(super) async fn call_browse_page(&self, args: &Value) -> McpToolCallResult {
        let result = async {
            let plan: crate::browser_workflow::BrowserPlan =
                serde_json::from_value(args["plan"].clone())
                    .map_err(|e| TklessError::InvalidArgument(e.to_string()))?;
            let url = args["url"]
                .as_str()
                .ok_or_else(|| TklessError::InvalidArgument("Missing url".into()))?;
            let target = Url::parse(url)?;
            let mut fetch = self.engine.fetcher().options().clone();
            fetch.allow_private_addresses = args
                .get("allow_private_addresses")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            fetch.initial_target_host = target.host_str().map(str::to_owned);
            fetch.initial_target_origin = Some(target.origin().ascii_serialization());
            crate::browser_workflow::browse_page(
                &TklessEngine::with_options(fetch)?,
                url,
                &plan,
                &ExtractionOptions::default(),
            )
            .await
        }
        .await;
        match result {
            Ok(doc) => format_doc_result(&doc, OutputFormat::Json),
            Err(error) => McpToolCallResult::from_error(&error),
        }
    }

    pub(super) async fn call_sync_collection(&self, args: &Value) -> McpToolCallResult {
        let result = async {
            let config: crate::collection::CollectionConfig =
                serde_json::from_value(args["config"].clone())
                    .map_err(|e| TklessError::InvalidArgument(e.to_string()))?;
            config.validate()?;
            let mut fetch = self.engine.fetcher().options().clone();
            fetch.allow_private_addresses = args
                .get("allow_private_addresses")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let url = Url::parse(&config.url)?;
            fetch.initial_target_host = url.host_str().map(str::to_owned);
            fetch.initial_target_origin = Some(url.origin().ascii_serialization());
            let engine = TklessEngine::with_options(fetch)?;
            let state = args["state_dir"]
                .as_str()
                .ok_or_else(|| TklessError::InvalidArgument("Missing state_dir".into()))?;
            let budget = crate::limits::sync_state_bytes(
                args.get("max_state_mb")
                    .and_then(Value::as_u64)
                    .unwrap_or(crate::limits::SYNC_DEFAULT_STATE_MB),
            )?;
            crate::sync::sync_collection(
                &engine,
                &config,
                &ExtractionOptions::default(),
                std::path::Path::new(state),
                budget,
            )
            .await
        }
        .await;
        report_result(result)
    }

    pub(super) async fn call_collect_pages(&self, args: &Value) -> McpToolCallResult {
        let result = async {
            let config: crate::collection::CollectionConfig =
                serde_json::from_value(args["config"].clone())
                    .map_err(|e| TklessError::InvalidArgument(e.to_string()))?;
            config.validate()?;
            let mut fetch = self.engine.fetcher().options().clone();
            fetch.allow_private_addresses = args
                .get("allow_private_addresses")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let url = Url::parse(&config.url)?;
            fetch.initial_target_host = url.host_str().map(str::to_owned);
            fetch.initial_target_origin = Some(url.origin().ascii_serialization());
            let engine = TklessEngine::with_options(fetch)?;
            crate::collection::collect_pages(&engine, &config, &ExtractionOptions::default()).await
        }
        .await;
        report_result(result)
    }
}

fn report_result<T: serde::Serialize>(result: Result<T, TklessError>) -> McpToolCallResult {
    match result.and_then(|report| serde_json::to_value(report).map_err(Into::into)) {
        Ok(value) => McpToolCallResult::success(value.to_string()).with_structured(value),
        Err(error) => McpToolCallResult::from_error(&error),
    }
}
