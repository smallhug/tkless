use super::{McpServer, McpToolCallResult, Value};

impl McpServer {
    pub(super) async fn call_manage_cache(&self, args: &Value) -> McpToolCallResult {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("stats");
        match action {
            "clear" => match self.cache.clear() {
                Ok(_) => McpToolCallResult::success("TkLess 本地响应缓存已成功清空。"),
                Err(e) => McpToolCallResult::error(format!("Failed to clear cache: {e}")),
            },
            "prune_expired" => {
                let pruned = self.cache.prune_expired();
                McpToolCallResult::success(format!("已成功清理 {pruned} 条过期缓存。"))
            }
            _ => {
                let stats = self.cache.stats();
                let res = serde_json::json!({
                    "memory_entries": stats.memory_entries,
                    "disk_entries": stats.disk_entries,
                    "disk_bytes": stats.disk_bytes,
                    "disk_human": crate::downloader::format_bytes(stats.disk_bytes),
                    "cache_dir": stats.cache_dir
                });
                match serde_json::to_string_pretty(&res) {
                    Ok(json_str) => McpToolCallResult::success(json_str),
                    Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
                }
            }
        }
    }

    pub(super) async fn call_list_adapters(&self, args: &Value) -> McpToolCallResult {
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");
        let list = crate::adapters::AdapterRegistry::list_adapter_metadata();
        if format == "json" {
            match serde_json::to_string_pretty(&list) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
            }
        } else {
            McpToolCallResult::success(crate::adapters::format_adapters_markdown(&list))
        }
    }
}
