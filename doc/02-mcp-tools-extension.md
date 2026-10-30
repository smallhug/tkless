# MCP 接入与工具扩展

本文对齐 2026-10-31 工作区。当前提供 31 个业务工具；`web_search` 与 `deep_research` 均仅使用 Bing。工具名称与完整参数以 [tools.rs](../crates/tkless-core/src/mcp/tools.rs) 及运行时 `tools/list` 为准。

## 启动与客户端配置

```powershell
tkless mcp
tkless mcp --dynamic
tkless mcp --profile dynamic

# HTTP 传输（可选），默认仅允许回环地址
tkless mcp --http 127.0.0.1:8787 --tool-timeout-secs 120

# 非回环绑定必须显式授权并提供 Bearer Token
TKLESS_MCP_TOKEN=... tkless mcp --http 0.0.0.0:8787 --allow-public-bind --auth-token-env TKLESS_MCP_TOKEN
```

完整模式返回 31 个业务工具；动态模式初始返回 2 个核心工具和 4 个发现元工具。环境变量 `TKLESS_MCP_DYNAMIC=1` 也可启用动态模式。MCP 本身不依赖 Node.js 或 Python；浏览器渲染和截图另需本机浏览器或远程 CDP 服务。

支持 mcpServers 的客户端可参考：

```json
{"mcpServers":{"tkless":{"command":"tkless","args":["mcp","--dynamic"]}}}
```

程序不在 PATH 时，将 command 替换为实际绝对路径，例如 `D:/projects/cli/target/release/tkless.exe`。替换二进制后重启服务，让客户端刷新工具声明。

## 传输与协议

服务通过 stdio 读取和输出单行 JSON-RPC 2.0，诊断日志写入 stderr。仅实现 `2025-11-25`，初始化始终声明该版本；不模拟旧协议，也不省略结构化输出。请求其他版本时仍返回自身支持的版本，客户端不支持该版本时应断开，遵循 [MCP 版本协商规范](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle#version-negotiation)。

## HTTP 传输

指定 `--http ADDR` 时改用 HTTP JSON-RPC：`POST /mcp` 每次发送一个 JSON-RPC 消息，`GET /healthz` 返回健康状态。该传输为无状态实现，单连接单请求（`Connection: close`），不提供 SSE 或服务端主动通知流；需要进度通知或取消的长任务仍应使用 stdio。

- 默认只允许绑定回环地址；非回环地址必须同时指定 `--allow-public-bind` 与 `--auth-token-env`，否则启动失败。
- 设置 Token 后，每个请求都必须携带 `Authorization: Bearer <token>`，否则返回 401。
- 请求头上限 16 KiB、请求体上限 1 MiB，拒绝分块传输；单个 IP 未限制，但全局最多 32 个并发请求（超出排队）。
- 单次请求超时由 `--tool-timeout-secs`（默认 300，1–3600）控制；超时返回 JSON-RPC 错误 `-32001`。
- 不实现会话、鉴权刷新或多租户隔离；暴露到公网前应置于反向代理与 TLS 之后。

```jsonl
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"example-client","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
```

工具调用并发执行，最多 64 个在途调用，单次超时默认 300 秒（`--tool-timeout-secs` 可调，1–3600）。调用方可发送 `notifications/cancelled`，并在 `params.requestId` 中指定原请求 ID。JSON-RPC 协议错误和工具执行错误是两类响应，调用方都应处理。

## Bing 搜索

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_search","arguments":{"query":"Tokio","scope":"github","limit":10,"format":"json"}}}
```

可选筛选参数：`since` / `until`（`YYYY-MM-DD`）、`language`（如 `zh-CN`）、`region`（两字母地区码）。它们使用 Bing 未公开参数，属于尽力而为；报告会附加 `search_filters_best_effort` 警告，宿主不应假定已严格生效。

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

文档工具结构为 `{document: PageDocument}`；搜索结构为 `{query, effective_query, status, results, diagnostics, warnings?}`；批量结构为 `{total, succeeded, failed, resumed, fetched, results}`。`diagnostics` 仅控制 Markdown 诊断展示。服务端按内置契约校验这些结构化结果。远程 PDF、DOCX、PPTX 响应由原生解析器处理（`extraction.method` 分别为 `pdf`、`office`）；XLSX 返回 `office_error`，需先导出为 CSV。

服务进程读取 `TKLESS_COOKIE_JAR`：设置后 `Set-Cookie` 按精确主机/路径保存到本地 Jar 并在后续请求回放，不扩展到子域；`Domain` 属性被忽略以避免公共后缀列表缺失带来的 cookie 注入风险。`TKLESS_CIRCUIT_BREAKER=N` 可为 `fetch_webpage` 等服务端请求启用每主机熔断（0 禁用）。

参数统一检查声明中的类型、枚举、必填项及数值范围，工具别名和 `execute_tool` 使用同一校验路径。四个工具及搜索/调研拒绝未声明参数。Schema 严格模式禁止回退，但不要求所有字段非空。

工具错误返回 `result.isError=true`；`structuredContent.error` 包含 `code`、`message`、`retryable`、`exit_code`，限流错误另有 `retry_after_secs`。常见 code 包括 `invalid_argument`、`schema_error`、`verification_required`、`search_parse_error`、`rate_limited`、`office_error`。部分批量失败保留在 `results[].error` 和 `results[].error_details`，全部失败时 `isError=true` 且结构化内容仍是批量报告。单个请求失败不等同于协议连接中断。

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

## 异步任务（jobs_*）

四个工具把长时间运行的工具调用移出单次请求超时：

| 工具 | 参数与用途 |
|---|---|
| `jobs_start` | `tool`、`arguments`、可选 `ttl_secs`；启动后台任务并立即返回 `job_id` |
| `jobs_status` | `job_id`；返回状态、耗时、进度、错误和完成后的结构化结果 |
| `jobs_cancel` | `job_id`；取消排队或运行中的任务；对已完成任务幂等 |
| `jobs_list` | 可选 `state`、`limit`；按新到旧返回摘要，不含结果体 |

`tool` 仅接受 `crawl_site`、`batch_fetch_webpages`、`collect_pages`、`sync_collection`；`arguments` 与该工具在 `tools/list` 中的 `inputSchema` 一致，并在启动时同步校验。任务状态为 `queued`、`running`、`completed`、`failed` 或 `cancelled`；失败任务的错误文本上限 4096 字符，完整结构化结果仍需通过 `jobs_status` 获取。

```json
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"jobs_start","arguments":{"tool":"crawl_site","arguments":{"seed_url":"https://example.com/docs","max_pages":50},"ttl_secs":600}}}
```

任务注册表仅存在于当前进程：最多 4 个排队/运行任务，结束后默认保留 3600 秒（60–86400 可调），进程重启即丢失；不跨进程共享，也不做持久化。结果为内存副本，大报告的保留会占用进程内存。HTTP 传输虽为无状态，但同一进程内可先 `POST` 启动、后续请求轮询；不提供多租户隔离，`job_id` 不是授权凭据，公网部署仍需 Bearer Token 与反向代理。

## 进度与业务工具目录

批量抓取和爬虫在调用 `params._meta.progressToken` 存在时，可以发送 `notifications/progress`，携带同一 `progressToken`、`progress`、`total` 和 `message`。

```json
{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"crawl-1","progress":2,"total":10,"message":"Page processed"}}
```

31 个业务工具的分类清单见 [README](../README.md#mcp-接入)。返回文件、图片、认证或缓存信息的工具，应按各自 Schema 和响应处理；适配器存在不代表站点所有内容均可访问。

新增工具需同步更新 `get_available_tools`、`get_tool_catalog`、`dispatch_tool_execution` 及协议测试；实现与规范见 [开发标准](04-development-standards.md)。
