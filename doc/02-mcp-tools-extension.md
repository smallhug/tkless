# MCP 接入与工具扩展

本文对齐 2026-10-30 工作区。当前提供 27 个业务工具；`web_search` 与 `deep_research` 均仅使用 Bing。工具名称与完整参数以 [tools.rs](../crates/tkless-core/src/mcp/tools.rs) 及运行时 `tools/list` 为准。

## 启动与客户端配置

```powershell
tkless mcp
tkless mcp --dynamic
tkless mcp --profile dynamic
```

完整模式返回 27 个业务工具；动态模式初始返回 2 个核心工具和 4 个发现元工具。环境变量 `TKLESS_MCP_DYNAMIC=1` 也可启用动态模式。MCP 本身不依赖 Node.js 或 Python；浏览器渲染和截图另需本机浏览器或远程 CDP 服务。

支持 mcpServers 的客户端可参考：

```json
{"mcpServers":{"tkless":{"command":"tkless","args":["mcp","--dynamic"]}}}
```

程序不在 PATH 时，将 command 替换为实际绝对路径，例如 `D:/projects/cli/target/release/tkless.exe`。替换二进制后重启服务，让客户端刷新工具声明。

## 传输与协议

服务通过 stdio 读取和输出单行 JSON-RPC 2.0，诊断日志写入 stderr。仅实现 `2025-11-25`，初始化始终声明该版本；不模拟旧协议，也不省略结构化输出。请求其他版本时仍返回自身支持的版本，客户端不支持该版本时应断开，遵循 [MCP 版本协商规范](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle#version-negotiation)。

```jsonl
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"example-client","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
```

工具调用并发执行，最多 64 个在途调用，单次超时 300 秒。调用方可发送 `notifications/cancelled`，并在 `params.requestId` 中指定原请求 ID。JSON-RPC 协议错误和工具执行错误是两类响应，调用方都应处理。

## Bing 搜索

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_search","arguments":{"query":"Tokio","scope":"github","limit":10,"format":"json"}}}
```

| 参数 | 当前行为 |
|---|---|
| `query` | 必填，非空关键词 |
| `limit` | 整数 1–20，默认 10；实际数量可少于上限 |
| `scope` | 默认 all；平台过滤，完整值见工具 Schema |
| `format` | markdown 或 json |
| `fetch_content` | 默认 false；启用后并发提取结果正文 |
| `dedup` / `max_tokens_per_page` / `schema` | 需要 `fetch_content`=true；每页默认预算 1500 tokens |
| `cdp_endpoint` | 显式使用浏览器搜索 |
| `auto_cdp` | 默认 false；HTTP 连接失败或遇到反爬挑战时尝试浏览器 |
| `diagnostics` | 默认 false；启用后 Markdown 包含诊断，JSON 始终包含诊断 |
| `schema_strict` | 默认 false；要求 schema，解析或提取失败时直接报错 |

搜索和调研不再接受 `engine`、`yandex_region`；未声明参数会明确报错。结果项保留 `engine`: "bing"。平台限制通过 site: 查询和本地 URL 校验共同执行，已有 site: 过滤优先。HTTPS 预检沿用实际 HTTP/浏览器路径，不会弹出人工验证窗口；更多配置见 [Bing 搜索](06-bing-search.md)。

`format`=json 时，文本载荷位于 `result.content[].text`。`fetch_webpage`、`clean_html`、`web_search` 和 `batch_fetch_webpages` 提供 `outputSchema` 和 `structuredContent`，调用方可直接读取对象；不提供旧协议降级。搜索 JSON 文本与结构化结果使用同一报告结构，不返回旧的裸数组。

文档工具结构为 `{document: PageDocument}`；搜索结构为 `{query, effective_query, status, results, diagnostics, warnings?}`；批量结构为 `{total, succeeded, failed, resumed, fetched, results}`。`diagnostics` 仅控制 Markdown 诊断展示。服务端按内置契约校验这些结构化结果。

参数统一检查声明中的类型、枚举、必填项及数值范围，工具别名和 `execute_tool` 使用同一校验路径。四个工具及搜索/调研拒绝未声明参数。Schema 严格模式禁止回退，但不要求所有字段非空。

工具错误返回 `result.isError=true`；`structuredContent.error` 包含 `code`、`message`、`retryable`、`exit_code`，限流错误另有 `retry_after_secs`。常见 code 包括 `invalid_argument`、`schema_error`、`verification_required`、`search_parse_error`、`rate_limited`。部分批量失败保留在 `results[].error` 和 `results[].error_details`，全部失败时 `isError=true` 且结构化内容仍是批量报告。单个请求失败不等同于协议连接中断。

```json
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\"query\":\"example\",\"effective_query\":\"example\",\"status\":\"no_results\",\"results\":[],\"diagnostics\":[]}"}],"structuredContent":{"query":"example","effective_query":"example","status":"no_results","results":[],"diagnostics":[]},"isError":false}}
```

上例仅展示响应结构，不代表实际搜索结果。

## 批量流式结果与恢复

`batch_fetch_webpages` 支持 `checkpoint`、`resume`、`retry_failed`、`order` 及单主机并发/延迟配置。默认返回完整报告；`stream_results=true` 要求调用携带 `_meta.progressToken`，每项通过 `notifications/progress` 的 `params._meta["tkless/batch_item"]` 交付，最终仅返回统计和 `streamed: true`。通知队列有界，消费者变慢会施加背压。

检查点保存请求身份指纹和完整结果；恢复可复用完成项，仅重试失败项。宿主需要读取流式扩展元数据，普通进度提示不能代替逐项结果。完整示例与限制见 [批量任务](07-batch-jobs.md)。

## 深度调研

```json
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"deep_research","arguments":{"topic":"Rust 异步运行时","depth":2,"breadth":3,"max_pages":8,"format":"json"}}}
```

调研通过 Bing 检索、网页抓取和本地启发式摘录构建带来源的报告，不调用外部 LLM 生成结论。`format` 支持 markdown、json，搜索页数与最终报告由各自预算控制。

## 动态工具发现

| 工具 | 参数与用途 |
|---|---|
| `discover_tools` | `query`、category；检索工具目录 |
| `get_tool_schema` | `tool_name`；获取工具定义、`inputSchema` 和可选 `outputSchema` |
| `execute_tool` | `tool_name`、`arguments`；无需先激活即可执行 |
| `activate_tools` | `tool_names`；激活工具，并通知客户端刷新列表 |

```json
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_tool_schema","arguments":{"tool_name":"export_tables"}}}
```

激活后发送 `notifications/tools/list_changed`。动态模式支持直接调用未激活业务工具；`execute_tool` 不能递归调用自身。所有路径只接受目录中的规范名称，例如 `web_search`；旧别名和 `tkless_` 前缀会报错。初始声明的 Token 开销取决于 Schema、分词器和宿主。

## 进度与业务工具目录

批量抓取和爬虫在调用 `params._meta.progressToken` 存在时，可以发送 `notifications/progress`，携带同一 `progressToken`、`progress`、`total` 和 `message`。

```json
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"crawl-1","progress":2,"total":10,"message":"Page processed"}}
```

27 个业务工具的分类清单见 [README](../README.md#mcp-接入)。返回文件、图片、认证或缓存信息的工具，应按各自 Schema 和响应处理；适配器存在不代表站点所有内容均可访问。

新增工具需同步更新 `get_available_tools`、`get_tool_catalog`、`dispatch_tool_execution` 及协议测试；实现与规范见 [开发标准](04-development-standards.md)。
