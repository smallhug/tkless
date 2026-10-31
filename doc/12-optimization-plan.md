# 12 - 能力缺口与优化方案

> 对齐日期：2026-10-31。对应 0.6.0 源码（未发布；公众号授权历史采集已移除，公开文章/合集/名称发现保留）。
> 本文是规划文档，**不代表任何条目已经实现**；所有工作量与收益均为估算，不作为交付承诺。

---

## 1. 目标与非目标

### 1.1 目标

把 TkLess 从"单次抓取与提取工具"扩展为"可持续运行的数据采集管道"，同时保持现有的三条硬约束：

1. 纯 Safe Rust，不引入 Python/Node 运行时依赖；
2. 安全优先：SSRF 全链路校验、凭据作用域隔离、响应与状态大小上限、原子写入；
3. 结果语义诚实：区分"完成 / 部分完成 / 失败"，不把受限结果包装成成功。

### 1.2 非目标（设计取舍，不在本方案内）

- 验证码、Cloudflare/Turnstile、滑块等风控绕过；
- 在页面中执行用户提供的任意 JavaScript；
- 依赖 LLM 生成结论或摘要（保留本地启发式）；
- 把商业搜索 API Key 作为默认搜索路径；
- 内嵌 OCR/ASR 模型权重。

---

## 2. 现状基线

| 维度 | 当前实现 | 关键约束 |
| :--- | :--- | :--- |
| CLI | 18 个子命令 + 根命令单页模式 | 全部通过参数传入，无配置文件 |
| MCP | 31 个业务工具 + 4 个发现元工具，协议 `2026-07-28` | 仅 stdio；单次调用 300 秒超时 |
| 适配器 | 22 个内置平台适配器 | 编译期内置，无插件机制 |
| 请求 | 仅 GET；重定向逐跳校验；编码自动识别 | 无 POST/GraphQL/表单 |
| 渲染 | Chromium 系 CDP；步骤 `wait_for/fill/click/scroll` | 不展开 Shadow DOM / iframe |
| 提取 | Readability + CSS/regex Schema（嵌套、数组、类型、默认值） | 无 XPath、无 LLM |
| 搜索 | 仅 Bing；`limit ≤ 20`；最多取 2 页 | 无日期/语言/地区/垂直筛选 |
| 批量/爬取 | 进程内并发、单主机限流、checkpoint、失败重试 | crawl 硬编码同域；无总时间预算 |
| 采集/同步 | GET 分页三种；本地快照与增量事件 | 无请求模板；无调度 |
| 输出 | stdout / 文件（MD、JSON、JSONL、HTML、Text、SQL、CSV、TSV） | 无外部 sink |
| 观测 | stderr 日志 + `--stats` | 无 metrics / trace 导出 |

已具备且不应回退的能力：SSRF/DNS 全解析校验、凭据按初始 origin 隔离、响应与缓存大小上限、原子提交与独占锁、部分结果与停止原因、单页缓存条件请求、GBK/Shift-JIS 等编码识别、robots.txt（仅 `crawl` 可选）。

---

## 3. 缺口总表与优先级

优先级依据：**影响面 × 实现确定性 ÷ 成本**。

| ID | 工作流 | 缺口 | 优先级 | 估算（人日） |
| :--- | :--- | :--- | :--- | :--- |
| WS1 | 请求模型 | 仅 GET，无法调用 POST/GraphQL/表单接口 | P0 | 8–12 |
| WS2 | 页面结构 | 无 XHR 响应捕获、不展开 Shadow DOM/iframe、滚动不收敛 | P0 | 10–15 |
| WS3 | 调度监控 | 无常驻调度、无变更通知、无法监控任意 URL | P0 | 10–14 |
| WS4 | MCP 服务化 | 无持久化任务队列、SSE 通知流与会话回收 | P1 | 8–12 |
| WS5 | 搜索质量 | 无日期/语言/地区筛选、无多引擎可选 | P1 | 6–8 |
| WS6 | 凭据体系 | 无 cookie jar/多站点凭据库/会话刷新 | P1 | 8–12 |
| WS7 | 内容类型 | 无 OCR、Office、ASR | P1/P2 | 12–20 |
| WS8 | 稳定性 | 无每域名熔断、代理池、metrics | P2 | 8–12 |
| WS9 | 平台与扩展 | 适配器编译期内置，缺大量平台 | P2 | 按站点 1–3 |
| WS10 | 输出集成 | 无对象存储/数据库 sink | P2 | 6–8 |

> 本表保留立项时的基线，不代表当前待办。截至 2026-10-31，已完成或已舍弃的结论见各工作流「实施状态」与 §9.1；当前不再计划实现表中所有缺口。

---

## 4. 详细方案

### WS1 请求模型：从 GET-only 到模板化请求（P0，已实施）

> 实施状态（2026-10-30）：`CollectionConfig.request` 已落地，支持 GET/POST/PUT、请求体模板与 `cursor` 体分页；实现见 `crates/tkless-core/src/collection.rs`，说明见 `doc/10-collection-workflows.md`。

**现状与证据**：`crates/tkless-core/src/fetcher.rs` 原先全库无 POST 调用；`doc/10-collection-workflows.md` 曾明确"不包含任意 POST 请求模板"。

**方案**：在 `CollectionConfig` 增加可选 `request` 字段（`collection.rs`），向后兼容（`#[serde(default)]`，缺省即 GET）。

```json
{
  "url": "https://example.com/api/articles",
  "request": {
    "method": "POST",
    "content_type": "application/json",
    "headers": {"X-Requested-With": "XMLHttpRequest"},
    "body": "{\"page\":{{page}},\"cursor\":{{cursor}},\"size\":20}"
  },
  "pagination": {"kind": "cursor", "parameter": "cursor",
                 "items_pointer": "/data/items", "next_pointer": "/data/next_cursor"},
  "max_pages": 20
}
```

约束与安全边界：

- 方法白名单：`GET`、`POST`、`PUT`；`DELETE` 等破坏性方法不提供（§9 明确不做）；
- 模板变量白名单：`{{page}}`、`{{offset}}`、`{{cursor}}`、`{{limit}}`，其余占位符在校验阶段报错；
- body 上限 64 KiB，序列化后校验；不允许在 body 中出现凭据字段（与 `is_sensitive_header` 同源的黑名单校验）；
- 目标 origin 固定为 `config.url` 的 origin；跨源请求仍需 `allow_cross_origin=true`，且剥离全部自定义凭据；
- 请求仍走 SSRF 校验与凭据作用域；模板请求不跟随重定向（3xx 直接报错）；
- 不新增停止原因：上游 405 沿用 `HttpStatusError` / `page_error`，避免出现由常量配置触发的死分支。

交付物：

- `collection.rs`：`RequestSpec` 结构与校验；
- `collect_pages` MCP 参数的 `config` 子 Schema 更新（`crates/tkless-core/src/collection.rs::config_schema`）；
- `doc/10` 中"不包含任意 POST"表述更新为"支持同源模板化请求"；
- 本地 mock HTTP 服务测试：JSON body 分页、表单分页、405、跨源拒绝、body 超限。

验收：

1. mock 服务返回两页 JSON，`collect` 能跨页去重并标记 `listing_complete`；
2. 配置含 `{{unknown}}`、body > 64 KiB、跨源无 `allow_cross_origin` 均返回参数错误（退出码 2）；
3. `cargo test --workspace`、`clippy -D warnings`、`scripts/check-docs.cjs` 通过。

### WS2 现代页面结构：网络捕获与 DOM 穿透（P0，已实施；collect 消费捕获 JSON 已舍弃）

> 实施状态（2026-10-30）：已落地 `scroll_until_stable`、`include_shadow_dom`、`include_frames` 与可选 XHR 捕获（`capture`）；结果写入 `RenderedPage.network`，`browse` 输出 `entity.network`。真实浏览器用例为 `#[ignore]`。采集直接消费捕获 JSON 的方案已舍弃（浏览器捕获与采集器耦合、Pointer/分页语义难以稳定），需要接口数据时使用 WS1 的 `request` 模板，见 §9.1。

**现状与证据**：`browse_page` 只返回 `RenderedPage { html, final_url }`；`cdp/steps.rs` 仅 4 种动作；`cdp.rs` 的 DOM 读取基于 `document.documentElement.outerHTML`。

**方案**：

1. **网络响应捕获**（`cdp.rs` 已有 `Network.enable`，扩展为双通道读取）：
   - 新增 `capture: { mime_prefixes: ["application/json"], max_body_bytes, max_responses, same_origin_only }`；
   - 结果进入 `RenderedPage.network: Vec<CapturedResponse>`；`collect` 直接消费捕获结果的方案已舍弃，见 §9.1；
   - 响应体受 `max_bytes` 与条数上限约束，超限记录 `truncated` 而非静默裁剪。
2. **Shadow DOM 与 iframe**：
   - `DOM.getDocument(pierce: true)` + `DOM.getFlattenedDocument` 序列化影子树；
   - `Page.getFrameTree` 取同源 iframe，用对应 execution context 取值；跨源 frame 默认跳过并记警告；
   - 不引入用户脚本，仅使用固定内置表达式。
3. **滚动收敛**：新增 `scroll_until_stable { max_steps, step_px, idle_rounds, time_budget_ms }`，替换"固定 5 步"，达到高度连续 N 次不变或预算耗尽即停止，并报告 `scroll_stopped_reason`。

安全与兼容：

- 捕获结果不写入缓存以外位置；凭据请求的响应体默认不捕获（按 `is_sensitive_header` 判断请求头）；
- `RenderedPage` 新增字段使用 `#[serde(default)]`，旧调用方不受影响；
- `browse_page` 的 MCP 参数 `additionalProperties:false`，新增字段需同步 `tools.rs` 与 `contracts.rs`。

验收：

- fixture 站点覆盖：仅 XHR 返回数据的列表、Shadow DOM 内的正文、同源 iframe 正文、无限滚动 3 屏；
- 断言捕获条数/大小上限、跨源 frame 警告、滚动停止原因；
- 作为 `#[ignore]` 真实浏览器测试在 CI 之外可手动运行。

### WS3 调度与变更监控（P0，已实施；MCP watch / stdout sink 已舍弃）

> 实施状态（2026-10-30）：已落地 `watch` 子命令与 `tkless_core::watch::run_once`，支持多 job 单轮执行、轮次预算、文件/Webhook 通知（可选 HMAC-SHA256）、通知去重与失败隔离；调度采用外部 cron/计划任务，不内建守护进程。MCP 侧 `watch` 工具与 `stdout` sink 已舍弃（见 §9.1）；进程内长任务注册表由 WS4 的 `jobs_*` 提供，不含 watch。

**现状与证据**：无调度命令；`sync` 只对 `collect` 配置做快照，不提供通知；`doc/10` 说明其"不是跳过未变页面的网络抓取"。

**方案**：新增 `watch` 子命令，采用"单轮执行 + 外部调度"模型，避免内建常驻进程：

```
tkless watch --config ./watch.json --state ./watch-state -o events.jsonl
tkless watch --config ./watch.json --state ./watch-state --interval 3600
```

省略 `--interval` 时只执行一轮。

`watch.json`：

```json
{
  "jobs": [
    {
      "id": "rust-blog",
      "target": { "url": "https://example.com/blog", "pagination": {"kind": "next_link", "selector": "a[rel=next]"}, "schema": {"items_selector": "article", "fields": {"title": "h2", "url": "a@href"}} },
      "id_pointer": "/url",
      "max_pages": 3,
      "diff": { "mode": "content_hash", "ignore_paths": ["/fetched_at"] },
      "notify": { "sink": "file", "path": "./events.jsonl" }
    }
  ]
}
```

设计要点：

- 复用 `sync.rs` 的快照与事件模型（`added/updated/removed/unchanged/error`），状态文件沿用 `state_file` 的锁与原子写；
- 省略 `--interval` 时只执行一轮，供 cron / Task Scheduler / systemd timer 调用；`--interval` 仅在本地前台运行，不注册系统服务；
- 通知 sink 首期支持 `stdout`、`file`、`http`（POST JSON）；HTTP sink 必须做 SSRF 校验，重试与超时受限，失败不影响本轮 `complete` 语义但写入 `errors`；
- webhook 签名可选：`HMAC-SHA256(secret, body)`，密钥仅从环境变量或文件读取，不落日志与报告；
- 预算：每个 job 有 `max_pages`/`max_items`/`time_budget_ms`；整轮有总时长上限，超时保留已完成 job 并标记 `partial`。

验收：

- 两次运行之间制造"新增/修改/删除"各一项，事件类型与 ID 稳定；
- 单轮超时、webhook 5xx、状态文件损坏分别返回可区分的错误码；
- 单轮退出码语义：本轮全部 job 完整为 0，存在部分结果为 1（可 `--allow-partial`）。

### WS4 MCP 传输、异步任务与协议演进（P1，已实施；SSE / 会话已舍弃）

> 实施状态（2026-10-31）：已落地可选 HTTP JSON-RPC 传输（`--http`、`POST /mcp`、`GET /healthz`、Bearer Token、回环默认）、可配置工具超时（`--tool-timeout-secs`）与进程内异步任务（`jobs_start`/`jobs_status`/`jobs_cancel`/`jobs_list`）。异步任务采用显式启动，支持 `crawl_site`/`batch_fetch_webpages`/`collect_pages`/`sync_collection`，最多 4 个活动、TTL 回收、重启即丢失。SSE 通知流、`Mcp-Session-Id` 会话回收、“超时自动转后台”与 `run_watch_once` job 已舍弃（见 §9.1）；任务注册表仅保存在当前进程。

**现状与证据**：`mcp/server/transport.rs` 只有 `run_transport`（stdin/stdout）；工具调用硬编码 300 秒超时；MCP 只声明 `tools` 能力。

**方案**：

1. **HTTP 传输**：`tkless mcp --http 127.0.0.1:8787`，JSON-RPC over HTTP POST。传输保持无状态；SSE 通知流与 `Mcp-Session-Id` 会话回收已舍弃（见 §9.1）。
   - 默认仅绑定回环地址；绑定非回环需显式 `--allow-public-bind`；
   - 可选 Bearer Token（`TKLESS_MCP_TOKEN`），缺失时拒绝非回环请求。
2. **异步任务**：新增 `jobs_start` / `jobs_status` / `jobs_cancel` / `jobs_list`；
   - 长任务通过显式 `jobs_start` 启动；不支持按超时阈值自动转后台，也不支持 `run_watch_once`（见 §9.1）；
   - 任务注册表进程内保存，带 TTL 与并发上限；
   - 超时阈值可由 `--tool-timeout-secs` 配置，默认保持 300 秒。
3. **协议策略**：继续只实现 `2026-07-28`（无 `initialize` 握手、每请求 `_meta`、`server/discover`），不恢复旧协议降级；新增能力通过 `capabilities.extensions` 声明，宿主不支持时返回明确错误。

验收：

- MCP 契约测试覆盖：未知参数拒绝、Token 缺失、非回环绑定拒绝、`jobs_*` 生命周期、取消后资源释放；
- `scripts/check-docs.cjs` 的工具目录与实际 `tools/list` 一致。

### WS5 搜索质量与多引擎（P1，已实施；多引擎已舍弃）

> 实施状态（2026-10-30）：已落地时间/语言/地区筛选（CLI `--since`/`--until`/`--lang`/`--region`、MCP 同名参数），映射为 Bing 未公开参数并在报告附加 `search_filters_best_effort` 警告；非法日期与地区在发请求前拒绝。多引擎抽象、跨查询持久指纹、结果缓存 TTL 与每域名结果上限已舍弃（见 §9.1），固定使用 Bing。

**现状与证据**：`search.rs::SearchOptions` 只有 `scope/limit/cdp/auto_cdp`；`apply_search_scope` 仅做 `site:`；最多取 2 页。

**方案**：

- 增加时间与语言参数：`--since`/`--until`（日期）、`--lang`、`--region`，转换为 Bing 的筛选参数；该层属于非公开接口，必须：
  - 用固定 SSR 样本做解析回归；
  - 失败时降级为无筛选并在 `warnings` 明确说明，不静默改变语义；
- 结果级能力：跨查询/跨批次去重使用单次请求内的 SimHash；持久指纹、结果缓存 TTL 与每域名结果上限已舍弃（见 §9.1）；
- 引擎抽象：固定使用 Bing；`trait SearchBackend` 与第二引擎已舍弃（见 §9.1），不提供引擎选择参数。

验收：日期筛选在固定样本上命中预期 URL；筛选失败产生 `warning` 且 `complete` 语义不变。

### WS6 凭据与会话体系（P1，已实施；刷新/加密已舍弃）

> 实施状态（2026-10-31）：已落地本地凭据库（`tkless auth set|list|remove`）、按平台的请求时回退解析、`0600` 原子写入与大小/平台数限制，以及可选持久化 Cookie Jar（`--cookie-jar` / `TKLESS_COOKIE_JAR`）。Jar 按精确主机、路径、过期与 `Secure` 回放，限制 1024 条与 4 MiB，损坏文件明确报错。跨子域共享、Cookie 自动刷新、`LoginFlow` 抽象与加密/密钥环已舍弃（见 §9.1）；当前依靠文件权限保护。

**现状与证据**：`FetchOptions.cookie/cookie_file` 为明文；`login` 仅 B 站扫码；`check_auth` 对微信只返回 `configured`。

**方案**：

- **凭据存储**：`tkless auth set <platform>` 从 stdin 读取，写入用户目录（Unix `0600`）；可选的 `argon2` + `chacha20poly1305` 加密已舍弃（见 §9.1），不引入密钥环依赖；
- **Cookie Jar**：按精确主机持久化与更新（Set-Cookie），遵循路径、过期与 `Secure`；跨子域扩展与自动刷新已舍弃（见 §9.1）；
- **登录流程抽象**：不新增 `trait LoginFlow`；B 站扫码为现有实现，其余平台提供"导入 cookie"路径（见 §9.1）；
- **会话健康**：`check_auth` 返回 `verified` 三态（true/false/unknown），并在报告中说明判断依据。

验收：明文文件权限测试（Unix）、跨 origin 不发送、会话失效可区分于网络错误、`--no-cache` 下不写盘。

### WS7 内容类型扩展（P1/P2，已实施；OCR/ASR/XLSX 已舍弃）

> 实施状态（2026-10-31）：已落地 CDP `Page.printToPDF`（`tkless ... --pdf` 与 `capture_pdf_via_cdp`），复用截图相同的导航/等待/自动滚动/请求拦截流程；并新增 `crawl --epub` 导出静态 EPUB 3（stored ZIP，无新依赖，正文以转义 `<pre>` 保留 Markdown 原文）与 DOCX/PPTX 原生解析（内置 ZIP+XML，无新依赖；识别标题样式、段落、简单列表与逐页文本）。OCR、XLSX、ASR 与 Markdown 渲染式 EPUB 已舍弃（见 §9.1），改由外部工具预处理或转 CSV。

**原则**：默认构建不引入重依赖与模型权重；全部通过 feature 开关，并在 `--help`/文档中明确"需要外部程序或额外特性"。

| 能力 | 结论 | 说明 |
| :--- | :--- | :--- |
| PDF 打印 | 已实施 | CDP `Page.printToPDF`；复用截图流程 |
| Office 解析 | 已实施（DOCX/PPTX） | 内置 ZIP+XML；不渲染格式、图片、图表、批注或修订 |
| EPUB 导出 | 已实施（原文） | `crawl --epub` 保留 Markdown 原文，不做渲染 |
| 元素 / 区域截图 | 已舍弃 | 全页截图 + 外部裁剪；不引入 `clip` 定位（见 §9.1） |
| 扫描件 PDF / 图片 OCR | 已舍弃 | 外部 `tesseract` 等工具先转文本/PDF，再交给 tkless |
| XLSX | 已舍弃 | 先导出为 CSV |
| 音视频转写 | 已舍弃 | 外部 `whisper` 或云转写先产出文本/字幕 |

验收：已实施能力需在缺少输入或格式不支持时明确报错（不得 panic 或静默降级为空内容）；舍弃项不在 CLI/MCP 中提供参数或承诺。

### WS8 稳定性、限流与可观测性（P2，已实施；代理池/OTLP 已舍弃）

> 实施状态（2026-10-31）：已落地共享重试预算（`--retry-budget`）、JSON 结构化日志（`--log-format json`）与可选每主机熔断（`--circuit-breaker N`，默认关闭；连续失败后 30 秒冷却、半开探测与退避上限 5 分钟，错误码 `circuit_open`）。代理池、OTLP 导出、任务级磁盘总配额、缓存容量淘汰与运行统计快照已舍弃（见 §9.1）；磁盘预算仍列为已知边界（见 `doc/09`）。

- **每域名熔断**：已实现连续失败阈值 + 半开探测 + 退避；默认关闭，避免改变既有延迟特性；
- **重试策略**：`retries` 默认保持 0；新增 `--retry-budget` 按任务限制总重试次数；
- **代理池**：已舍弃（见 §9.1）；继续使用单一 `--proxy`，复杂出口管理交给代理软件；
- **可观测性**：已实现 `--log-format json`；`--stats` 计数器快照与 OTLP 已舍弃（见 §9.1）；
- **磁盘预算**：任务级下载总配额与缓存容量淘汰已舍弃（见 §9.1）；单文件 `max_bytes`、爬虫 `--max-output-bytes` 与 `manage_cache` 继续生效。

验收：熔断行为可由 `circuit_open` 与重试时间观察；JSON 日志字段稳定；已舍弃项不在 CLI/MCP 中提供参数。

### WS9 平台覆盖与扩展机制（P2，已实施；批量编译式适配器已舍弃）

> 实施状态（2026-10-31）：已落地配置式适配器（`TKLESS_ADAPTERS_DIR/*.json`，域/路径匹配 + 可选 Schema，无代码加载、大小与数量限制、内置优先）。批量编译式站点适配器与配置适配器的 `api`/`map` 映射已舍弃（见 §9.1）；按真实需求逐站点添加，不预先承诺候选清单。

**方案 A（已实施，有限）**：配置式适配器 `adapters.d/*.json`，仅描述 `hosts`、`path_prefix` 与可选 `schema`；`api`/`map` 映射已舍弃（见 §9.1），接口数据请使用 WS1 的 `request` 模板。

```json
{
  "name": "example",
  "hosts": ["example.com"],
  "path_prefix": "/blog/",
  "schema": {"fields": {"title": "h1"}}
}
```

**方案 B（按需）**：编译期新增适配器，按现有 `PlatformAdapter` trait，逐站点 1–3 人日。不预先批量实现微博、抖音、豆瓣、简书、36 氪、TikTok、Instagram、LinkedIn、Notion、Confluence、GitLab、Dev.to 等候选（见 §9.1），只在有明确需求时逐个评估。

说明：无适配器不等于不能抓取；通用管线仍可处理静态页面，适配器主要提升"接口优先/内嵌数据/分页"的成功率。文档需保留这句边界声明，避免夸大。

### WS10 输出与企业集成（P2，已实施；S3/数据库 sink 已舍弃）

- Sink：不再引入通用 Sink 抽象；保留 CLI 输出、`crawl --bundle`/`--epub` 打包与 `watch` 的 file/http 通知；`stdout`（作为库 sink）、S3/对象存储与数据库写入已舍弃（见 §9.1）；
- 交付格式：Markdown 打包、JSONL 事件流、EPUB、PDF（CDP 打印）；
- 与外部系统：webhook 事件（复用 WS3）、退出码契约保持不变。

> 实施状态（2026-10-31）：已落地 `crawl --bundle FILE`，输出 `README.md` 索引与逐页 Markdown 的 ZIP（stored 实现，512 条目 / 64 MiB 上限）。已存在 JSONL 事件流、webhook 通知与 CDP PDF；EPUB 由 WS7 的 `crawl --epub` 提供（正文保留 Markdown 原文，不做渲染）。通用 Sink 抽象、S3/对象存储与数据库写入已舍弃（见 §9.1）。

---

## 5. 契约与兼容策略

### 5.1 新增或修改 MCP 工具的必改清单

依据 `doc/02-mcp-tools-extension.md`：

1. `mcp/tools.rs`：`get_available_tools` 声明 + `get_tool_catalog` 目录项；
2. `mcp/contracts.rs`：`output_schema`（若返回结构化结果）；
3. `mcp/server.rs`：`dispatch_tool_execution` 分发；
4. `mcp/server/*.rs`：业务处理器；
5. `tests/mcp_test.rs` 或专项协议测试；
6. `scripts/check-docs.cjs` 与相关文档；
7. 动态模式：`get_dynamic_discovery_tools` 的关键词覆盖。

### 5.2 参数与状态兼容

- 输入参数只增不改；删除参数视为破坏性变更，需要版本说明与迁移文档；
- 使用 `deny_unknown_fields` 的结构新增字段时，同步更新 MCP `input_schema`，否则会被 `validate_arguments` 拒绝；
- 缓存键变更（如新增请求模板语义）必须升级前缀版本，旧缓存不迁移；
- 状态文件（`state_file`/`sync`/`watch`）使用版本号，跨版本恢复拒绝并提示新建目录；
- 停止原因与事件枚举新增值必须同步 `workflow.rs`、报告文档与 CLI 帮助，JSON 名称保持 snake_case。

### 5.3 文档同步

每个工作流落地时必须同步：

| 文档 | 需更新内容 |
| :--- | :--- |
| `README.md` | 子命令表、MCP 工具数、能力边界 |
| `doc/02` | 新工具、参数、协议能力 |
| `doc/03` | 适配器与能力矩阵 |
| `doc/04` | 新增硬约束（如请求模板规则） |
| `doc/09` | 新增或收窄的安全边界 |
| `doc/10` | 采集/同步/调度能力与限制 |
| `doc/README.md` | 索引表 |
| `CHANGELOG.md` | 用户可见变更 |
| `RELEASE_NOTES.md` | 迁移与风险提示 |

---

## 6. 测试与质量门禁

每个工作流必须满足同一套完成定义（DoD）：

1. **单元测试**：纯逻辑（校验、解析、去重、模板展开）；
2. **集成测试**：本地 mock HTTP 服务（不依赖公网）；
3. **契约测试**：MCP 参数校验、输出 Schema、错误码；
4. **安全回归**：SSRF、凭据作用域、大小上限、原子写、锁竞争、日志不含凭据；
5. **真实站点验证**：`#[ignore]` 测试或手工记录，标注日期与结果，不得作为离线 CI 断言；
6. **门禁命令**：`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`node scripts/check-docs.cjs`；
7. **文档**：按 5.3 更新，并在 PR 描述中列出"未验证部分"。

---

## 7. 里程碑与路线图

| 版本 | 内容 | 出口标准 |
| :--- | :--- | :--- |
| v0.6（P0） | WS1 请求模板、WS2 网络捕获与 DOM 穿透、WS3 watch 单轮模式 | 三类真实站点（POST 接口、Shadow DOM、滚动列表）各有可复现验证记录；既有测试零回退 |
| v0.7（P1） | WS4 MCP HTTP + jobs、WS5 搜索筛选、WS6 凭据体系 | MCP 契约测试全绿；Token 与非回环绑定行为符合预期 |
| v0.8（P1/P2） | WS7 内容类型（OCR/PDF 打印）、WS8 熔断与可观测 | 缺依赖时报错明确；熔断与预算可在报告中观察 |
| v0.9+（P2） | WS9 配置式适配器、WS10 sink | 新增站点无需改核心代码；sink 失败不影响主流程语义 |

说明：版本号与顺序是建议，不绑定发布时间；每项应独立可回滚。截至 2026-10-31，上述里程碑的主要条目已落地或已明确舍弃（见 §9.1），路线图不再按原候选条目排期；具体实现边界以各工作流「实施状态」为准。

---

## 8. 风险与缓解

| 风险 | 影响 | 缓解 |
| :--- | :--- | :--- |
| 上游页面结构变更 | 搜索/采集解析失效 | 固定样本回归 + 明确解析错误，不伪装为空结果 |
| 新增请求模板被滥用 | 对内网或第三方发起越权请求 | origin 固定、方法白名单、body 上限、SSRF 逐跳校验、凭据黑名单 |
| 网络捕获带来内存压力 | 大响应导致内存膨胀 | 条数与字节双上限、超限标记 truncated、默认只捕获 JSON |
| HTTP 传输暴露服务 | 未授权调用 | 默认回环绑定、可选 Bearer Token、非回环需显式开关 |
| 调度误报 | 变更通知噪声 | 内容哈希排除易变字段、`unchanged` 不通知、支持忽略路径 |
| 新依赖膨胀 | 编译体积与 CVE 面 | 全部可选 feature、优先纯 Rust、记录依赖理由 |
| 文档漂移 | 用户按错误能力规划 | `scripts/check-docs.cjs` + 每项工作的文档清单 |

---

## 9. 明确不做

- 绕过验证码或风控、伪造设备指纹以规避检测；
- 执行用户提供的任意脚本；
- 内嵌大模型或模型权重；
- 承诺"取得某平台全部历史/全量数据"这类无法证明的完整性声明；
- 以破坏性手段（`DELETE`）操作第三方资源。

### 9.1 已舍弃的候选功能（2026-10-31 决策）

以下条目曾出现在 §4 的方案中，经技术瓶颈、维护成本、可验证性与替代方案评估后**不再计划实现**。代码中不应为其预留参数、Schema 或未使用的抽象；后续若要重启，需重新立项并更新本节。

| 类别 | 舍弃项 | 主要原因 | 替代路径 |
| :--- | :--- | :--- | :--- |
| 搜索 | 第二搜索引擎与 `trait SearchBackend`、跨查询持久指纹、结果缓存 TTL、每域名结果上限 | 非公开页面解析需长期跟随改版；持久缓存/指纹会模糊"结果完整"语义 | 固定 Bing；交叉验证交给外部搜索工具 |
| MCP | SSE 通知流、`Mcp-Session-Id` 会话回收、超时自动转后台、`run_watch_once` job | 无状态 HTTP 传输更简单；隐式转后台使调用语义不稳定；watch 自带状态目录 | stdio progress + 显式 `jobs_start` |
| 调度 | MCP `watch` 工具、`stdout` 通知 sink | watch 需要状态目录与外部调度；核心库禁止直接打印 | cron/Task Scheduler + file/http sink |
| 浏览器 | `collect` 直接消费捕获的 XHR JSON、跨源 iframe 跳过警告 | 捕获与采集器耦合，JSON Pointer、分页与多响应选择难以稳定；跨源 frame 无法读取，警告需额外状态管道且不影响正文 | `CollectionConfig.request` 模板直连接口；同源 frame 合并，跨源静默跳过 |
| 凭据 | Cookie 自动刷新、`LoginFlow` 抽象、跨子域 Cookie、加密/密钥环 | 平台刷新机制不公开；无公共后缀列表时跨子域有注入风险；无密钥环的加密仅转移密码问题 | 重新登录；`0600` 明文 + 精确主机 Jar |
| 内容类型 | OCR、ASR、XLSX、Markdown 渲染式 EPUB、元素/区域截图 | 依赖外部二进制/模型或额外 crate，质量与平台差异不可控；元素截图需 box-model 与穿透状态，收益低 | 外部 OCR/ASR；XLSX 转 CSV；EPUB 保留 Markdown 原文；全页截图 + 外部裁剪 |
| 稳定性 | 代理池、OTLP、任务级下载总配额、缓存容量淘汰、运行统计快照 | 与代理管理器/可观测性平台职责重叠；状态机与依赖成本高 | `--proxy`、JSON 日志、`--circuit-breaker`、`manage_cache` |
| 平台 | 批量编译式站点适配器、配置适配器的 `api`/`map` 映射 | 逐站点持续逆向与合规成本；与 WS1 请求模板重叠 | 22 个内置适配器 + `TKLESS_ADAPTERS_DIR`（hosts/path_prefix/schema）+ 按需单站点 |
| 输出 | 通用 Sink 抽象、S3/对象存储、数据库写入 | 签名、幂等、凭据与供应商差异属于数据平台能力 | CLI 输出管道给 rclone/aws cli/数据库导入工具 |

---

## 10. 附录：差距自查表

| 场景 | 当前结论 | 对应工作流 |
| :--- | :--- | :--- |
| 静态/服务端渲染正文 | 支持 | — |
| 简单 JS 交互后取正文 | 支持（`browse`） | WS2 增强 |
| 数据仅在 XHR JSON | 部分（`browse` 捕获 + `request` 模板直连接口；`collect` 直接消费已舍弃） | WS2 |
| 列表页 GET 分页 | 支持（`collect`） | — |
| 列表页 POST/表单分页 | 支持（`CollectionConfig.request`） | WS1 |
| Shadow DOM / iframe 正文 | 支持（同源、显式开启） | WS2 |
| 无限滚动列表 | 支持（`scroll_until_stable`） | WS2 |
| 登录后内容 | 支持（凭据库 + 可选持久化 Cookie Jar） | WS6 |
| 扫码/2FA 自动登录 | 仅 B 站 | WS6 |
| 验证码/Cloudflare | 不支持（设计取舍） | — |
| 扫描 PDF / 图片文字 | 不做（外部 OCR） | WS7（已舍弃） |
| Office 文档 | DOCX/PPTX 支持；XLSX 不做，转 CSV | WS7 |
| 视频理解（无字幕） | 不做（外部 ASR） | WS7（已舍弃） |
| 日期/语言/地区检索 | 支持（Bing 未公开参数，尽力而为） | WS5 |
| 多引擎交叉验证 | 不做（固定 Bing） | WS5（已舍弃） |
| 定时监控与变更通知 | 支持（`watch` 单轮/循环，外部调度；MCP watch 工具已舍弃） | WS3 |
| 分布式多机抓取 | 不支持 | 不在近期范围 |
| 远程 MCP 接入 | 支持无状态 HTTP；SSE/会话已舍弃 | WS4 |
| MCP 长任务 | 支持显式 `jobs_*`（进程内、TTL、非持久） | WS4 |
| 代理池 / OTLP / 全局磁盘配额 | 不做 | WS8（已舍弃） |
| 对象存储/数据库写入 | 不做（外部工具） | WS10（已舍弃） |
| 大规模站点平台覆盖 | 22 个内置适配器 + 配置式扩展；批量编译式已舍弃 | WS9 |

---

## 11. 变更记录

| 日期 | 变更 |
| :--- | :--- |
| 2026-10-30 | 建立文档：现状基线、10 个优化工作流、优先级与验收标准 |
| 2026-10-30 | WS1 实施：模板化请求（GET/POST/PUT）、请求体占位符、游标体分页、校验与测试 |
| 2026-10-30 | WS2 部分实施：滚动收敛、Shadow DOM/同源 iframe 序列化、可选 XHR 捕获与边界 |
| 2026-10-30 | WS3 部分实施：`watch` 单轮/循环、轮次预算、文件与 Webhook 通知、HMAC 签名与失败隔离 |
| 2026-10-30 | WS4 部分实施：可选 HTTP JSON-RPC 传输、Bearer Token、可配置工具超时 |
| 2026-10-30 | WS5 部分实施：搜索时间/语言/地区筛选与尽力而为告警 |
| 2026-10-30 | WS6 部分实施：本地凭据库（权限、限制、请求时回退） |
| 2026-10-30 | WS7 部分实施：CDP PDF 打印（与截图共享流程） |
| 2026-10-30 | WS8 部分实施：共享重试预算与 JSON 结构化日志 |
| 2026-10-30 | WS9 部分实施：声明式配置适配器（TKLESS_ADAPTERS_DIR） |
| 2026-10-31 | WS10 部分实施：`crawl --bundle` Markdown 知识库打包（stored ZIP） |
| 2026-10-31 | WS4 部分实施：MCP `jobs_*` 进程内异步任务（限流、TTL、取消） |
| 2026-10-31 | WS7 部分实施：`crawl --epub` 静态 EPUB 3 导出（无新依赖） |
| 2026-10-31 | WS7 部分实施：DOCX/PPTX 原生解析（内置 ZIP+XML，无新依赖） |
| 2026-10-31 | WS6 部分实施：可选持久化 Cookie Jar（精确主机、无新依赖） |
| 2026-10-31 | WS8 部分实施：可选每主机熔断（阈值、冷却、半开探测） |
| 2026-10-31 | 同步 §7 路线图与 §10 差距自查表至当前实施状态 |
| 2026-10-31 | 决策：新增 §9.1，明确舍弃第二引擎、SSE/会话、OCR/ASR/XLSX、代理池、OTLP、S3/数据库 sink、批量编译式适配器等候选功能；同步各 WS 状态与差距表 |
