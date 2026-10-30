# Bing 搜索

CLI、交互式 Shell、MCP 和深度调研统一使用 Bing。默认搜索数量为最多 10 条，实际数量取决于上游结果及过滤；深度调研按自身每轮检索预算执行。

## CLI 参数

```powershell
tkless search "DeepSeek"
tkless search "Rust runtime" --scope github -l 10 -f json
tkless search "技术文章" --scope wechat
tkless search "Tokio runtime" --fetch-content --dedup -l 3 --max-tokens-per-page 1500 -f json
tkless search "Rust 2026" --since 2026-01-01 --until 2026-06-30 -f json
tkless search "Rust async" --lang zh-CN --region cn -f json
tkless --auto-cdp search "DeepSeek" -f json
tkless search "DeepSeek" --cdp http://127.0.0.1:9222 --cdp-wait 1500 -f json
tkless research "Rust 异步运行时" -f json
```

| 参数 | 用途 |
|---|---|
| `-l, --limit N` | 正整数，默认 10；普通 CLI 搜索按需分页 |
| `--scope PLATFORM` | 统一的平台过滤；省略或 all 表示不限平台 |
| `--fetch-content` | 并发提取结果页正文 |
| `--dedup` | 正文近似去重，要求 fetch-content |
| `--max-tokens-per-page N` | 每页正文 token 上限，要求 fetch-content 且为正整数 |
| `--cdp ENDPOINT` | 显式使用已有浏览器渲染；也可使用命令前的全局参数 |
| `--auto-cdp` | HTTP 连接失败或遇到反爬挑战时尝试浏览器；不提供人工验证码解题流程 |
| `--cdp-wait MS` | 配合显式 CDP 端点等待 DOM 加载 |
| `-f, --format` | markdown、json 或 jsonl |
| `--diagnostics` | 在 Markdown 中展示详细诊断；JSON/JSONL 始终为含诊断的报告对象 |
| `--since YYYY-MM-DD` | 时间下界（含）；转换为 Bing 非公开筛选参数 |
| `--until YYYY-MM-DD` | 时间上界（含）；省略时使用当天 |
| `--lang LANG` | 结果语言（如 `zh-CN`），映射为 `setlang` |
| `--region CC` | 两字母地区码（如 `cn`），映射为 `cc` |

筛选参数属于**尽力而为**：它们使用 Bing 未公开的查询参数（`filters=ex1:"ez5_<startDays>_<endDays>"`、`setlang`、`cc`），上游可能忽略或变更。启用时报告会附加 `search_filters_best_effort` 警告；调用方应核验结果而不是仅凭参数假定已筛选。日期早于 1970-01-01、`--since` 晚于 `--until`、语言或地区格式非法都会在发请求前返回参数错误。

scope 支持 all、wechat、zhihu、github、juejin、csdn、medium、substack、arxiv、huggingface、wikipedia、reddit、stackoverflow、v2ex、baike。平台过滤转换为 Bing 的 site: 查询，已有 site: 查询保留原过滤。

结果还会在本地校验实际 URL 的主机名与路径，再进行数量截断。允许目标域名及其子域名，不接受相似后缀域名、用户信息段或查询参数中伪装的域名。上游符合过滤条件的链接不足 limit 时，返回实际数量。

查询中的连字符、排除词、引号和年份原样发送，不再自动去掉引号、日期或改写平台关键词。`site:` 的路径前缀保留大小写；`inurl:`、`intitle:`、`filetype:` 不会被误当成域名限制。

## 结果状态与诊断

```powershell
tkless search '"near-duplicate" -advertising 2026' --diagnostics -f json
tkless search "Tokio" --scope github --diagnostics -f jsonl
```

JSON 始终为报告对象，JSONL 为单行报告；`--diagnostics` 控制 Markdown 诊断展示。`query` 是用户输入，`effective_query` 包含 scope 添加的 site: 条件。`status` 为 `results`、`no_results`（识别到明确的空结果标记）或 `filtered`（存在结果候选但被过滤）。验证码返回 `verification_required` 错误，未识别或损坏的结果结构返回 `search_parse_error`；后两种不作为空结果成功返回。

每页 `diagnostics` 包含输入路径、候选数、无效项、域名过滤、重复项、广告过滤、严格/宽松匹配数、选中数和最终返回数。`selection` 为启发式选择方式，不是可信概率。跨页重复项不计入实际返回数；第二页普通读取/解析失败保留第一页并返回警告，429 则明确返回限流错误。CDP 当前只读取第一页。

MCP 的 JSON 和 `structuredContent` 总是提供这些诊断；`diagnostics: true` 让 Markdown 载荷带上诊断。搜索后提取的每项还包含实际提取来源、警告和可选 `error_details`。

## MCP

```json
{"query":"Rust runtime","scope":"github","limit":10,"format":"json"}
```

工具名为 web_search。省略 limit 时默认 10，范围 1–20；fetch_content、dedup、max_tokens_per_page、cdp_endpoint、auto_cdp 等能力继续保留。deep_research 同样直接使用 Bing。两个工具都拒绝未声明参数；dedup=true、max_tokens_per_page、schema 需要 fetch_content=true。更新后重启 MCP 服务刷新工具声明。

## 搜索前连接检查

目标为 https://www.bing.com:443。HTTP 沿用搜索客户端和代理，使用无关键词、无 Cookie/Authorization 的根路径 HEAD 请求；CDP 在实际浏览器上下文中发起 HEAD。连接计时与响应头耗时分别记录，连接复用时不伪造新的握手耗时。

成功状态缓存 60 秒，连接失败或未知状态缓存 5 秒，并合并并发预检。缓存仅在进程内共享，HTTP 按引擎客户端及源站隔离，CDP 按浏览器实例及上下文隔离。搜索统一经过 TklessEngine 的请求池与预检状态。

HTTP 新连接预算 3 秒，预检总预算 5 秒；CDP 使用整体 5 秒预算。慢连接只告警，未知状态允许继续搜索；403、405、5xx 不算网络离线。429 按 Retry-After 冷却，缺省 5 秒、最长 24 小时；实际 HTTP 搜索连接失败或被限流会更新缓存。

```powershell
tkless -vv search "Rust programming language" -f json
$env:TKLESS_SEARCH_CONNECT_TIMEOUT_MS = "3000"       # 最大 60000
$env:TKLESS_SEARCH_PREFLIGHT_TIMEOUT_MS = "5000"     # 最大 120000
$env:TKLESS_SEARCH_PREFLIGHT_SUCCESS_TTL_SECS = "60" # 最大 3600
$env:TKLESS_SEARCH_PREFLIGHT_FAILURE_TTL_SECS = "5"  # 最大 300
```

这些配置均为正整数，连接预算不能大于预检总预算。诊断写入标准错误，结果 JSON/MCP 格式不混入日志。

浏览器预检优先创建隐藏页面；Chromium 拒绝隐藏 target 时降级为本次任务专属的普通后台标签页。只关闭任务创建的 target，不关闭用户已有标签或整个浏览器。普通后台标签页不保证完全隐藏。

## 第一页全部链接

```powershell
cargo run --offline -p tkless-core --example first_page_links -- "OpenAI Codex"
```

该示例读取 Bing 第一页结果区及子链接，按 URL 去重，不分页、不改写查询。未识别到有效结果页时明确失败。

## 参数迁移

引擎选择、地区参数、聚合模式和人工验证窗口已移除。旧平台布尔开关改为统一的 --scope 参数。返回项的 engine 字段继续固定为 bing，用于标明结果来源。

| 旧参数 | 当前写法 |
|---|---|
| `--engine ...` | 删除该参数，直接调用 search 或 research |
| `--yandex-region ...` | 已移除，无对应参数 |
| `--github` / `--wechat` / `--zhihu` 等 | `--scope github` / `--scope wechat` / `--scope zhihu` |

Rust 搜索只保留 `web_search(&TklessEngine, query, &SearchOptions) -> SearchReport`。`SearchOptions` 包含 scope、limit（默认 10）、cdp 和 auto_cdp；旧的裸数组搜索入口及组合包装函数已移除。需要正文时，将 `report.results` 传给 `fetch_search_results`，无需重复检索。

在已创建 engine 的异步上下文中：

```rust
let report = tkless_core::web_search(
    &engine,
    "Tokio runtime",
    &tkless_core::SearchOptions {
        scope: Some("github".into()),
        ..Default::default()
    },
).await?;
println!("{} results", report.results.len());
```

## 验证记录

2026-10-28：251 项离线测试通过。严格 Clippy、关闭默认 feature 的编译检查、格式检查与 release 构建通过；发布版 CLI/MCP 检查覆盖默认数量、scope、参数依赖、输出格式、旧参数拒绝、Schema 严格/回退模式、来源诊断及结构化结果。真实 Chrome 无头浏览器测试单独通过。

本轮 HTTP 在线实测：`DeepSeek --diagnostics` 返回 10 条 Bing 结果；`Tokio runtime --scope github --diagnostics` 的 10 个候选中有 9 个域名不符而被过滤，最终返回 1 条 GitHub 链接。默认最多 10 条，不以其他域名链接补齐平台过滤后的数量。
