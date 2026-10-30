# TkLess 0.5.0 更新说明

更新日期：2026-10-30。本说明对应 0.5.0 源码，不代表已经发布到 GitHub Release。已发布版本以 Release 标签和附件为准。

## 0.5.0 新增工作流

- `collect` 支持下一页链接、页码、JSON 游标分页和列表详情关联。
- `sync` 保存快照并输出新增、更新、删除事件；不完整采集不会误删旧记录。
- `crawl` 增加持久检查点、恢复与失败重试、时间和输出预算、队列限制及可选 robots.txt 检查。
- `browse` 执行等待、填写、点击和滚动步骤，提取实际导航后的页面；不提供任意脚本或验证码自动处理。
- MCP 增加 `collect_pages`、`sync_collection`、`browse_page`。配置与边界见 [采集工作流](doc/10-collection-workflows.md)。

## 安全与工程改进

- 加固凭据作用域、DNS 校验、HTML 净化、响应大小限制、资源下载和缓存；具体边界见 [安全加固记录](doc/09-security-hardening.md)。
- CLI 命令与 MCP 处理器按职责拆分，分页和浏览器限制集中管理；状态损坏、锁冲突、存储预算超限使用独立错误码。
- 快照和检查点的磁盘操作移入阻塞任务，取消调用时运行中的写入仍保持锁直至完成。
- 最低支持 Rust 版本为 1.98.1；增加无默认特性、文档契约、支持基线和独立浏览器 CI 检查。

Rust 调用方将 `McpServer::run_stdio` 改为 `run_transport` 并传入输入输出流。停止原因与同步事件改为枚举，JSON 名称保持不变。缓存键升级为 v4；批量任务身份包含软件版本，跨版本恢复可能被拒绝，应新建任务。CLI/MCP 同步存储预算统一为 1–65536 MiB。

## Skills.sh 平台适配

- 新增第 22 个内置适配器，支持 `skills.sh` / `www.skills.sh` 的技能详情、仓库技能列表、站内搜索及首页、Trending、Hot 榜单自动翻页。
- 从公开 HTML 和 Next Flight 数据恢复折叠技能正文，无需 API Key、浏览器或新增运行时依赖；正文只作为数据提取，不执行安装命令。
- CLI `--skills-pages` / `--skills-limit` 和 MCP 同名下划线参数控制榜单页数及去重结果上限。默认最多 3 页、1000 条，保留停止原因与后续页失败时的部分结果；公共搜索最多 200 条且没有翻页游标。
- JSON `entity` 提供来源、技能标识、安装命令、正文完整性或技能列表。折叠内容缺失时明确返回预览警告；API 目录失败直接返回错误，不降级为无搜索结果的页面外壳。
- 当前不接入需要认证的 `/api/v1`，不提供完整仓库文件下载。详见 [Skills.sh 适配说明](doc/08-skills-sh.md)。

## 第一批可靠性改进

- 保留搜索查询的连字符、引号、排除词和年份，取消隐式查询放宽；`inurl:` 等操作符不再误作域名过滤。
- 搜索 JSON/JSONL 统一使用含查询、过滤数量和警告的报告对象；`--diagnostics` 控制 Markdown 诊断，未知页面结构明确报错。
- 增加 `--schema-strict` / MCP `schema_strict`，Schema 失败直接报错；默认回退会写入 `warnings[]`。文档 `extraction` 记录实际路径。
- 四个主要 MCP 工具提供结构化输出与输出 Schema。参数和结果集中校验，核心错误及逐项失败包含稳定错误码与重试信息。

## 第二阶段：批量任务与兼容清理

- CLI `batch -f jsonl` 逐项输出并刷新，默认输入顺序，`--order completion` 按完成顺序；有界缓冲支持背压。
- `--checkpoint` 保存完整结果，`--resume` 继续未完成项，`--retry-failed` 仅重跑失败项；支持尾部半行恢复、任务身份校验和独占文件锁。
- 单主机并发默认 2，支持单主机/全局任务启动间隔。完整 JSON 报告新增 `resumed` / `fetched`，每项新增从 0 开始的 `index`。
- MCP 增加相同恢复参数及 `stream_results`，通过带 `tkless/batch_item` 元数据的进度通知流式交付，最终只返回统计。
- MCP 仅实现 `2025-11-25`，移除旧协议输出降级、旧工具别名和前缀；CLI 长参数与子命令仅保留规范名称。
- 删除弃用的无作用域 Cookie 接口及旧搜索入口；原生搜索统一为 `SearchOptions` / `SearchReport`。缓存键升级为 v3，文档必须包含当前来源字段，不迁移旧结构。

示例及限制见 [批量任务](doc/07-batch-jobs.md)。当前恢复覆盖固定 URL 列表，站点爬虫队列恢复另行实现。

## 搜索统一使用 Bing

CLI、Shell、MCP `web_search` 和深度调研均直接使用 Bing。默认搜索上限为 10 条，实际返回数量取决于上游响应与本地过滤；深度调研使用自身每轮检索预算。

- 移除其他搜索引擎实现、聚合搜索路由及人工验证窗口。
- 删除 CLI 的 `--engine`、`--yandex-region`，以及 MCP 的 `engine`、`yandex_region` 参数。
- 平台定向统一为 `--scope` PLATFORM，替代 `--github`、`--wechat`、`--zhihu` 等布尔开关。
- 将平台限制转换为 site: 查询，并按实际 URL 主机名和路径过滤，先过滤再截取数量。
- 返回项保留 `engine`: "bing"，不以其他域名的链接补齐数量。

```powershell
tkless search "DeepSeek"
tkless search "Tokio" --scope github -l 10 -f json
tkless search "Rust runtime" --fetch-content --dedup -l 3 --max-tokens-per-page 1500 -f json
tkless research "Rust 异步运行时" -f json
```

`--limit` 必须为正整数；`--dedup` 和 `--max-tokens-per-page` 要求 `--fetch-content`。搜索输出支持 markdown、json、jsonl；调研输出支持 markdown、json。

## 后台 HTTPS 连接预检

搜索前检查 Bing 的 HTTPS 连接。HTTP 沿用搜索客户端和代理；CDP 在实际浏览器上下文中检测。

- 使用无关键词、无 Cookie/Authorization 的根路径 HEAD 请求，不执行系统 ping。
- 分开记录连接建立与响应头耗时；复用连接时不伪造新的握手耗时。
- 成功缓存 60 秒，失败或未知状态缓存 5 秒；缓存限于当前进程，并按客户端或浏览器上下文隔离。
- HTTP 连接预算默认 3 秒、预检总预算 5 秒；CDP 使用整体 5 秒预算。
- 慢连接只告警，未知状态仍尝试搜索；429 按 Retry-After 冷却。
- 使用 -vv 查看诊断，标准输出继续保留结果格式。

```powershell
tkless -vv search "Rust programming language" -f json
tkless --auto-cdp search "DeepSeek" -f json
tkless search "DeepSeek" --cdp http://127.0.0.1:9222 --cdp-wait 1500 -f json
```

浏览器预检无法创建隐藏 target 时，降级为任务专属的普通后台标签页，并只清理任务创建的标签页。普通后台标签页不保证完全隐藏。浏览器渲染不提供自动解验证码或人工验证窗口流程。

## MCP 参数迁移

更新二进制后重启 MCP 服务，刷新宿主缓存的工具声明。调用 `web_search` 时省略引擎选择参数：

```json
{"query":"Tokio","scope":"github","limit":10,"format":"json"}
```

MCP 搜索 `limit` 范围为 1–20，默认 10；`auto_cdp` 默认 false。搜索与调研拒绝未声明参数。`dedup`=true、`max_tokens_per_page`、`schema` 要求 `fetch_content`=true。

完整模式提供 26 个业务工具；动态模式初始提供 2 个核心工具和 4 个发现元工具。完整 Schema 以 `tools/list` 或动态工具 `get_tool_schema` 返回为准。

## 其他当前能力与边界

- 原生 Rust PDF 文本提取、元数据和启发式 Markdown 重排，不包含扫描件 OCR。
- 22 个内置平台适配器，以及网页、表格、Schema、字幕等提取能力。
- 本地启发式多跳调研与来源摘录，不调用外部 LLM 自动生成结论。
- 批量与站点抓取保留逐项错误，默认部分失败也非零退出；可用 `--allow-partial` 接受部分失败。
- 自升级使用语义化版本比较，替换前验证 SHA-256；发布工作流生成配套 .sha256 文件。

## 最近代码验证记录

2026-10-30：307 项离线测试通过，覆盖分页与同步、任务恢复、预算、锁竞争、取消期间写入、参数边界和 MCP 协议。严格 Clippy、无默认特性编译、格式检查、Release 构建及文档契约检查通过。1 项真实 Chrome 无头浏览器集成测试单独通过；远程 CI 状态需提交后确认。

此前 HTTP 在线实测：DeepSeek 返回 10 条 Bing 结果；Tokio runtime `--scope github` 返回 1 条符合条件的链接，诊断记录 10 个候选中有 9 个域名不符而被过滤。此结果不是本次版本更新的新测试，在线数量不作为离线 CI 的成功断言。

详细说明见 [README](README.md)、[Bing 搜索](doc/06-bing-search.md) 和 [MCP 接入](doc/02-mcp-tools-extension.md)。
