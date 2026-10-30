# 通用采集、增量同步与浏览器工作流

本轮按顺序实现了通用分页、快照同步、单轮监控通知、爬虫恢复与预算、受限浏览器步骤。CLI 与 MCP 共用 Rust 核心，无新增 Python 或 Node.js 运行依赖。浏览器功能需要 Chrome/Edge/Chromium 或远程 CDP。

## 1. 通用分页和列表详情

将下列内容保存为 `collect.json`。URL 与 CSS 选择器都是示例，使用前按实际网站修改。

```json
{
  "url": "https://example.com/articles",
  "schema": {
    "items_selector": ".article-card",
    "fields": {"title": "h2", "url": "a@href"}
  },
  "id_pointer": "/url",
  "pagination": {"kind": "next_link", "selector": "a[rel=next]"},
  "details": {"url_pointer": "/url"},
  "max_pages": 10,
  "max_items": 1000,
  "delay_ms": 200
}
```

```bash
tkless collect --config ./collect.json -o collection.json
# 接受部分结果；报告仍保留 complete=false 与停止原因
tkless collect --config ./collect.json --allow-partial -o collection.json
```

输出 JSON 包含 `records`、`pages_fetched`、`duplicates`、`listing_complete`、`complete`、`stop_reason`、`next_url` 和 `errors`。每条记录保留稳定 ID、列表来源 URL、字段数据、可选详情文档及独立错误。

- `next_link`：按选择器读取下一页的 `href`。下一页元素消失表示列表结束；元素存在但没有 `href` 会报错。
- `page_number`：修改指定查询参数，每次加一；使用显式空结果标记判断结束，避免将页面结构变化当作正常结束。
- `cursor`：通过 JSON Pointer 提取 API 数组和下一游标；游标作为查询参数编码，不直接拼接 URL。`next_pointer` 必须存在，值为 `null` 或空字符串表示结束。
- `none`：只读取一页，是省略 `pagination` 时的默认行为。

页码模式的 `pagination` 示例：

```json
{"kind":"page_number","parameter":"page","start":1,"empty_selector":".no-results"}
```

游标 API 配置示例（此模式不需要 HTML Schema）：

```json
{
  "url":"https://example.com/api/articles",
  "id_pointer":"/id",
  "pagination":{"kind":"cursor","parameter":"cursor","items_pointer":"/data/items","next_pointer":"/data/next_cursor"},
  "max_pages":20
}
```

默认不跟随跨源分页或详情 URL；`allow_cross_origin=true` 可显式放开发现的链接范围。HTTP 请求仍进行 SSRF 检查，并拒绝 API 请求的跨源重定向。重复 URL、重复条目页、空页带后续游标、页数/条目上限以及中途错误都不会被标为完整采集。`next_url` 是诊断信息，不是可独立保证无遗漏的续传游标。

详情使用通用 HTML 提取或 `details.schema`，不自动调用平台专属适配器。列表字段规则与详情字段规则分开配置。HTML 与游标 API 均支持模板化请求（见下节）；不自动猜网站分页协议，也不提供全文数据库检索。

### 模板化请求（GET / POST / PUT）

分页接口不是 GET 时，用 `request` 描述方法、请求体与分页变量。占位符仅支持 `{{page}}`、`{{offset}}`、`{{cursor}}`、`{{limit}}`，其中 `{{limit}}` 取自 `request.page_size`（默认 20）。

```json
{
  "url": "https://example.com/api/articles",
  "request": {
    "method": "POST",
    "content_type": "application/json",
    "headers": {"X-Requested-With": "XMLHttpRequest"},
    "body": "{\"cursor\":\"{{cursor}}\",\"size\":{{limit}}}",
    "page_size": 50
  },
  "pagination": {"kind": "cursor", "parameter": "cursor", "items_pointer": "/data/items", "next_pointer": "/data/next_cursor"},
  "max_pages": 20
}
```

约束：

- 方法仅限 `GET`、`POST`、`PUT`；`GET` 不允许请求体；不支持 `DELETE` 等破坏性方法。
- 请求体上限 64 KiB，展开后再校验一次；header 最多 16 个，单个值不超过 8 KiB。
- 不得在 header 中写 `Cookie`、`Authorization` 等凭据；凭据仍由 `--cookie` 或 cookie 文件按 origin 注入。
- `{{page}}` 只能用于 `page_number` 分页，`{{cursor}}` 只能用于 `cursor` 分页；`{{offset}}` 为已请求页序号（从 0 开始）。
- 模板请求不跟随重定向（3xx 直接报错），也不能与 `browser` 渲染同时使用。
- 游标位于请求体时，`next_url` 仅作诊断信息，不能当作可续传入口。
- `page_number` + 请求体仍需显式空页标记才会标记 `listing_complete`；纯 JSON 接口建议使用 `cursor` 模式。

未配置 `request` 时沿用原有 GET 行为，旧配置无需迁移。

## 2. 快照与增量同步

```bash
tkless sync --config ./collect.json --state ./article-state -o changes.json
tkless sync --config ./collect.json --state ./article-state -o changes-next.json
```

每次运行会重新检查配置范围内的内容，输出 `added`、`updated`、`removed`、`unchanged` 或 `error` 事件。它是增量结果与快照维护，不是跳过所有未变页面的网络抓取。

同步要求 `id_pointer` 或 `details.url_pointer`。ID 基于稳定业务字段或规范化详情 URL，而非列表位置；内容哈希排除抓取耗时等波动字段。配置、提取参数和请求身份绑定状态目录；改变身份或提取范围需要使用另一个目录，页数/条目限制和请求间隔可以调整。

目录内容：

```text
article-state/
  manifest.json          当前版本、配置指纹及记录索引
  manifest.lock          系统文件锁的载体，不需要手动删除
  objects/               按记录 ID 与内容哈希保存的 JSON 快照
  events/1.json          第一次同步的事件报告
  events/2.json          后续同步的事件报告
```

只有完整成功的采集才能删除本次未出现的旧条目。触及上限、详情失败或后续页失败时，成功记录仍可更新，但缺失记录会保留。对象和事件先写入，清单最后原子替换；操作系统锁防止同时写入，并在进程退出时释放。崩溃可能留下尚未被清单引用的对象或事件，以清单的 `generation` 为准。

`--max-state-mb` 默认 256，约束保留的快照、事件及本次待写数据的保守估算；单个状态 JSON 上限 64 MiB。达到上限会停止推进清单，不自动删除历史。当前没有内置定时器、通知投递、向量数据库写入或历史回收命令；可以由外部调度器周期调用 `sync`。

## 3. 爬虫检查点和预算

```bash
tkless crawl https://example.com/docs --same-path-prefix --checkpoint ./docs.state.json --max-pages 100 --max-seconds 120 --max-output-bytes 16777216 --respect-robots -f json -o docs.json

# 扩大预算后恢复；成功处理的页面不会重复抓取
tkless crawl https://example.com/docs --same-path-prefix --checkpoint ./docs.state.json --resume --max-pages 500 --max-seconds 120 --max-output-bytes 33554432 --respect-robots -f json -o docs.json

# 同时重试上次失败的页面
tkless crawl https://example.com/docs --same-path-prefix --checkpoint ./docs.state.json --resume --retry-failed --max-pages 500 --respect-robots -f json -o docs.json
```

检查点保存待抓 URL、已发现集合、处理结果和请求身份。每次任务完成后原子保存；未完成请求保留在恢复队列中。强制退出时正在进行的 GET 可能会在恢复后重新执行，已保存的成功结果不会重抓。

状态读写在阻塞任务中执行，不占用异步网络工作线程。已经开始的持久化不会因调用取消而中止，任务会持锁至提交结束；恢复以实际状态文件为准。锁竞争返回可重试的 `state_busy`，状态损坏为 `state_corrupt`，存储预算超限为 `state_budget_exceeded`，均与参数错误区分。

参数语义：

| 参数 | 含义 |
|---|---|
| `--max-pages` | 整个任务累计的页面处理上限，恢复时可增加；Sitemap 文件不计为正文页面 |
| `--max-seconds` | 本次调用的时间预算，范围 1–86400 秒；预算到期取消未完成异步任务，保留队列 |
| `--max-output-bytes` | 已保存页面 JSON 的累计字节预算，不包含报告包装、锁文件等开销；恢复时可增加 |
| `--max-pending-urls` | 待抓 URL 上限，默认 10000，最大 100000；超限的当前页面保持待处理状态 |
| `--checkpoint` / `--resume` | 新建任务 / 恢复任务，输出文件必须与检查点分开 |
| `--retry-failed` | 恢复时仅将失败页面重新加入队列 |
| `--respect-robots` | 显式启用 robots.txt 检查；默认关闭 |

robots.txt 首版支持 `TkLess` / `*` 分组、Allow/Disallow、`*` 和尾部 `$`，采用最长匹配和同长度 Allow 优先。同源规则在一次运行中复用；404/410 视为无规则，其余获取错误及不完整响应会阻止对应页面。尚不包含 Crawl-delay、特殊编码的完整标准化等所有扩展。

报告包含 `complete`、`stop_reason`、`pending_urls`。JSONL 最后一条新增 `type=summary` 记录，前面的页面行保持页面字段；Markdown 开头包含相同完成状态。不完整结果默认返回非零退出码，`--allow-partial` 可以接受部分结果。深度、域名和排除规则定义的是采集范围，`complete` 表示该范围内任务已完成。

## 4. 受限浏览器交互

保存为 `browser.json`：

```json
{
  "timeout_ms":30000,
  "include_shadow_dom":true,
  "include_frames":true,
  "capture":{"mime_prefixes":["application/json"],"max_responses":10,"max_body_bytes":262144,"same_origin_only":true},
  "steps":[
    {"action":"wait_for","selector":"#query","timeout_ms":5000},
    {"action":"fill","selector":"#query","value":"Rust"},
    {"action":"click","selector":"button.search"},
    {"action":"wait_for","selector":".results","timeout_ms":10000},
    {"action":"scroll_until_stable","max_steps":20,"idle_rounds":2,"step_px":400}
  ]
}
```

```bash
tkless browse https://example.com/search --plan ./browser.json -o rendered.json
```

`browse` 输出 PageDocument JSON，正文默认 Markdown，`final_url` 来自浏览器实际页面。选择器必须指向唯一、可见、可用的点击或填写目标；`wait_for` 等待匹配元素可见。点击会触发导航或异步加载时，应紧接 `wait_for` 等待目标内容。可通过 `endpoint` 指定已有 CDP 服务。

- 最多 32 步；单次等待最多 30 秒，整个浏览器调用最多 120 秒。
- 支持文本类输入框及 textarea；不支持密码、文件上传、表单提交按钮、下载链接或打开新窗口的链接。
- 不接受任意 JavaScript。默认拦截 GET/HEAD/OPTIONS 之外的 HTTP 请求，并限制跨源文档导航；因此依赖 POST 搜索接口的网站可能无法使用此模式。
- 滚动支持固定像素与 `scroll_until_stable`（按高度是否停止增长判定），只截断按高度判定；报告 `scroll_stopped_reason` 为 `stable` 或 `max_steps`。
- `include_shadow_dom=true` 时把开放 shadow root 的内容序列化进宿主元素；`include_frames=true` 时把同源 iframe 内容内联为 `<div data-frame-url>`。二者默认关闭，关闭时保持原始 `outerHTML`。
- `capture` 可选捕获 XHR/fetch 响应（默认只保留 `application/json`），受条数、单条字节数与同源限制；结果位于 `browse` 输出 `entity.network`，`PageDocument` 正文不受影响。请求头含 `Cookie`/`Authorization` 等凭据时该响应不捕获（登录态接口需改用直接请求模板并自行处理凭据）；首页导航尚未完成的慢响应可能不在快照内。
- 仍不包含多标签页流程、跨源 iframe 读取与验证码自动处理；新增的 JSON 列表采集优先使用 `collect` 的 `request` 模板直接取接口。
- 可将 BrowserPlan 放在采集配置的 `browser` 中，作用于每个 HTML 列表页；详情页独立使用 `details.browser`。游标 JSON API 仍走 HTTP。
- 浏览器网络隔离仍有前述边界，见 [安全加固记录](09-security-hardening.md)。固定动作和 HTTP 方法限制并不能保证网站本身没有 GET 副作用，也不替代系统级出口隔离。

## 5. 定时监控与变更通知

`watch` 把 `sync` 的快照能力扩展到多个任务，一次运行一轮，调度交给 cron / Task Scheduler / systemd timer，不内建守护进程。

```json
{
  "round_timeout_ms": 300000,
  "jobs": [
    {
      "id": "rust-blog",
      "target": {
        "url": "https://example.com/blog",
        "schema": {"items_selector": "article", "fields": {"title": "h2", "url": "a@href"}},
        "id_pointer": "/url",
        "pagination": {"kind": "next_link", "selector": "a[rel=next]"},
        "max_pages": 3
      },
      "max_state_mb": 64,
      "notify": [
        {"kind": "file", "path": "./changes.jsonl"},
        {"kind": "http", "url": "https://hooks.example.com/tkless", "timeout_ms": 10000, "max_attempts": 2, "secret_env": "TKLESS_WEBHOOK_SECRET"}
      ]
    }
  ]
}
```

```bash
# 单轮（推荐由 cron 调用），报告写到 stdout 或 -o 文件
tkless watch --config ./watch.json --state ./watch-state --once -o ./round.json

# 前台循环，每 3600 秒一轮；每轮输出一行 JSON
TKLESS_WEBHOOK_SECRET=... tkless watch --config ./watch.json --state ./watch-state --interval 3600
```

行为与边界：

- 每个 job 使用 `state/<job-id>` 独立状态目录：身份校验、独占锁、原子提交、状态预算均沿用 `sync`；某个 job 失败不会删除或阻塞其他 job 的记录。
- 只通知非 `unchanged` 事件（`send_unchanged=true` 可改成全量）；无变更时不发出任何通知。
- `file` sink 按 JSONL 追加一行 `{job, generation, captured_at, complete, stop_reason, events}`；父目录不存在时自动创建。
- `http` sink 以 POST JSON 发送同一载荷，不跟随重定向；默认拒绝私网/回环地址，需显式 `allow_private_addresses=true`；设 `secret_env` 时附带 `X-Tkless-Signature: sha256=<HMAC-SHA256>`，环境变量缺失则拒绝发送而不是降级为无签名。
- 通知失败写入该 job 的 `notify_errors`，不改变轮次 `complete`（采集语义与投递结果分开）；退出码只看采集结果。
- 轮次预算 `round_timeout_ms` 为 1s–1h；超时的 job 返回 `complete=false` 与错误，已完成 job 的结果保留。
- job `id` 仅允许 `[a-z0-9][a-z0-9_-]{0,63}`，同时作为状态子目录名，阻止路径穿越；最多 64 个 job、每个 job 最多 4 个 sink。
- `--interval` 与 `--output` 不能同时使用（循环模式每轮一行 JSON 到 stdout）。

## MCP 与验证

新增 `collect_pages`、`sync_collection`、`browse_page` 三个工具，完整模式共 27 个业务工具；动态模式仍初始返回 6 个工具，可通过发现接口按需启用。新工具返回 `structuredContent`，参数 JSON Schema 与核心校验共同限制配置。`crawl_site` 扩展了对应检查点与预算参数，成功返回的结构化报告包含完成状态。MCP 传输层单次工具调用仍受 300 秒超时约束，长爬取任务可通过检查点分次恢复。

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"collect_pages","arguments":{"config":{"url":"https://example.com/articles","schema":{"items_selector":".article-card","fields":{"title":"h2","url":"a@href"}},"pagination":{"kind":"next_link","selector":"a[rel=next]"}}}}}
```

本地回归覆盖分页与详情、游标编码、重复页和错误保留、同步不误删、监控轮次事件与通知去重、通知失败不回写采集结论、不可达 job 的诚实报告、爬虫恢复、时间与输出预算、robots.txt、CLI 文件保护及 MCP 参数校验。真实浏览器测试覆盖填写、点击、等待、实际导航 URL、截图、标签页清理及 WS2 的 shadow DOM/iframe/滚动收敛/XHR 捕获（默认 `#[ignore]`）：

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tkless-core --test bing_cdp_test -- --ignored
```
