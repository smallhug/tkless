use super::{
    get_available_tools, get_dynamic_discovery_tools, get_tool_catalog, McpServer,
    McpToolCallResult, Value,
};

impl McpServer {
    pub(super) async fn call_discover_tools(&self, args: &Value) -> McpToolCallResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_lowercase());
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("all")
            .trim()
            .to_lowercase();

        let catalog = get_tool_catalog();
        let active_set = self.active_dynamic_tools.read().await;

        let filtered: Vec<_> = catalog
            .into_iter()
            .filter(|entry| {
                if category != "all" && !entry.category.eq_ignore_ascii_case(&category) {
                    return false;
                }
                if let Some(ref q) = query {
                    if q.is_empty() {
                        return true;
                    }
                    entry.name.to_lowercase().contains(q)
                        || entry.summary.to_lowercase().contains(q)
                        || entry.category.to_lowercase().contains(q)
                        || entry.keywords.iter().any(|k| k.to_lowercase().contains(q))
                } else {
                    true
                }
            })
            .collect();

        if filtered.is_empty() {
            let msg = format!(
                "No specialized tools matched query '{:?}' in category '{}'. Available categories: content, search, media, analysis, system, all.",
                query.unwrap_or_default(),
                category
            );
            return McpToolCallResult::success(msg);
        }

        let mut md = String::new();
        md.push_str(&format!(
            "### TkLess 动态工具发现目录 (匹配到 {} 个工具)\n\n",
            filtered.len()
        ));
        md.push_str("| 工具名称 (Tool) | 分类 (Category) | 运行状态 | 功能概述 (Summary) |\n");
        md.push_str("| :--- | :--- | :--- | :--- |\n");

        for entry in &filtered {
            let status = if entry.is_core {
                "核心常驻"
            } else if active_set.contains(entry.name) {
                "已动态激活"
            } else {
                "按需拉取"
            };
            md.push_str(&format!(
                "| `{}` | `{}` | {} | {} |\n",
                entry.name, entry.category, status, entry.summary
            ));
        }

        md.push_str("\n> **两阶段调用建议 (Usage Guidelines)**:\n");
        md.push_str("> 1. **单次即时调用**: 直接调用 `execute_tool(tool_name: \"...\", arguments: {...})` 立即执行。\n");
        md.push_str("> 2. **查看完整参数**: 调用 `get_tool_schema(tool_name: \"...\")` 获取 inputSchema 与参数描述。\n");
        md.push_str("> 3. **会话动态注册**: 调用 `activate_tools(tool_names: [\"...\"])` 加入 tools/list；本服务不发送 `list_changed`，请随后重新调用 `tools/list`。\n");

        McpToolCallResult::success(md)
    }

    pub(super) async fn call_get_tool_schema(&self, args: &Value) -> McpToolCallResult {
        let tool_name = match args.get("tool_name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => n.trim(),
            _ => return McpToolCallResult::error("Missing required parameter 'tool_name'"),
        };

        let mut all_tools = get_available_tools();
        all_tools.extend(get_dynamic_discovery_tools());

        if let Some(tool) = all_tools.into_iter().find(|t| t.name == tool_name) {
            let schema_pretty =
                serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default();
            let mut response = format!(
                "### 工具参数定义: `{}`\n\n**描述**: {}\n\n**inputSchema**:\n```json\n{}\n```\n",
                tool.name, tool.description, schema_pretty
            );
            if let Some(ref output_schema) = tool.output_schema {
                response.push_str(&format!(
                    "\n**outputSchema**:\n```json\n{}\n```\n",
                    serde_json::to_string_pretty(output_schema).unwrap_or_default()
                ));
            }
            McpToolCallResult::success(response).with_structured(&tool)
        } else {
            McpToolCallResult::error(format!(
                "Tool '{tool_name}' not found. Call 'discover_tools' to search available tools."
            ))
        }
    }

    pub(super) async fn call_activate_tools(&self, args: &Value) -> McpToolCallResult {
        let tool_names = match args.get("tool_names").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr,
            _ => return McpToolCallResult::error("Missing or empty 'tool_names' array"),
        };

        let available = get_available_tools();
        if tool_names.iter().any(|item| {
            !item
                .as_str()
                .is_some_and(|name| available.iter().any(|tool| tool.name == name))
        }) {
            return McpToolCallResult::invalid_argument(
                "tool_names must contain canonical business tool names from tools/list",
            );
        }
        let mut newly_activated = Vec::new();
        let mut active_set = self.active_dynamic_tools.write().await;
        for item in tool_names {
            if let Some(name) = item.as_str() {
                active_set.insert(name.to_string());
                newly_activated.push(name.to_string());
            }
        }

        drop(active_set);

        let resp = format!(
            "成功激活工具: [{}]。请重新调用 'tools/list' 刷新声明；本服务不发送 notifications/tools/list_changed，您也可以直接调用。",
            newly_activated.join(", ")
        );
        McpToolCallResult::success(resp)
    }
}
