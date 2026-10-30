# 变更日志 (Changelog)

本项目的所有显著变更均记录于此文件中。
遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/) 格式与 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/) 规范。

---

## [Unreleased]

### Added

- 微信账号历史采集：`wechat --history` 与 MCP 历史模式支持本地授权会话文件、历史游标翻页、头条/次条展开、检查点恢复及失败正文重试；会话失效明确返回 `session_required`，不将合集作为账号历史结果。
- 微信公众号公开内容采集：新增 `wechat` 命令与 `list_wechat_articles` MCP 工具，支持按名称发现、文章身份提取、关联合集与游标自动翻页，以及可选正文抓取。
- 合集增加账号约束、跨页去重、数量/页数预算、续取入口及部分失败保留；明确区分合集范围完成与全账号历史，验证页不会作为成功正文返回。
- 采集支持模板化请求：`CollectionConfig.request` 可配置 GET/POST/PUT、请求头与请求体，请求体可用 `{{page}}`、`{{offset}}`、`{{cursor}}`、`{{limit}}` 占位符，游标可放在请求体中；请求体上限 64 KiB，禁止在模板头中写凭据，模板请求不跟随重定向。
- 浏览器工作流新增 `scroll_until_stable` 步骤、`include_shadow_dom`/`include_frames` 序列化选项，以及受条数与字节上限约束的 XHR/fetch 响应捕获（默认关闭，带凭据请求不捕获）。
- 新增 `watch` 子命令：每轮按配置运行多个采集任务，输出新增/更新/删除事件，并支持文件或 Webhook 通知（可选 HMAC 签名、默认拒绝私网目标）；调度由 cron 等外部工具承担。
- MCP 新增可选 HTTP JSON-RPC 传输（`--http`、`POST /mcp`、`GET /healthz`），默认仅回环、非回环需显式授权与 Bearer Token；单次调用超时改由 `--tool-timeout-secs` 配置。
- 搜索新增 `--since`/`--until`/`--lang`/`--region`（及 MCP 对应参数），映射到 Bing 未公开参数；启用时报告附加 `search_filters_best_effort` 警告，非法日期或地区在发请求前报错。
- 新增 `tkless auth` 本地凭据库（`set` 从 stdin 读取、`list`、`remove`）：Unix 下以 `0600` 原子写入，单 Cookie ≤ 64 KiB、最多 32 平台；仅在未提供 `--cookie`/`--cookie-file` 时按平台使用。
- 新增 `--pdf`：通过 CDP `Page.printToPDF` 将渲染后的页面打印为 PDF（保留背景与 CSS 分页），与 `--screenshot` 共享导航、等待、自动滚动与拦截流程。
- 新增 `--retry-budget N`（整个运行共享重试上限）与 `--log-format json`（stderr 结构化日志，每行一个 JSON 对象）。

### Fixed

- 微信公开文章识别脚本内的合集链接，解码转义并核对账号；短链接与合集长链接按文章身份去重，避免入口文章重复计数。
- 自升级拒绝空文件、大小不符和非本机可执行产物；在 SHA-256 校验之外，增加落盘后的完整性检查及替换前后的 `--version` 启动检查。
- 使用独占升级锁和独立暂存目录，避免固定临时文件被并发升级覆盖；替换失败恢复原程序，回滚失败保留备份并报告路径。
- 发布工作流拒绝空产物，并在本机架构构建后检查程序版本；增加空重定向响应、文件截断、并发和回滚回归测试。
- 核实 0.4.0 升级器未处理 GitHub 的 302 重定向，会把空响应体写成程序并报告成功。从该版本恢复需手动安装；0.5.0 已支持逐跳重定向和哈希校验，本轮继续加固安装阶段。

## [0.5.0] - 2026-10-30

源码版本更新为 0.5.0；GitHub Release 的发布状态以远程标签和附件为准。

### Added

- 通用采集 `collect`：下一页链接、页码、JSON 游标分页和列表详情关联；不完整结果保留停止原因。
- 快照同步 `sync`：稳定记录标识、新增/更新/删除事件与存储预算；采集不完整时保留旧记录。
- 受限浏览器工作流 `browse`：等待、填写、点击、滚动和实际导航 URL；MCP 增加三个对应工具，完整模式共 26 个业务工具。
- 爬虫检查点恢复、失败重试、时间与输出预算、待抓队列限制和可选 robots.txt 检查。
- Skills.sh 适配器：公开技能详情及折叠正文、仓库技能列表、榜单快照与结构化元数据；当前内置适配器增至 22 个，不执行技能命令。
- Skills.sh 站内搜索与榜单自动翻页：CLI/MCP 可配置页数和结果上限，跨页去重、循环停止、部分失败保留和明确的完整性字段；公共搜索无游标，最多返回 200 条。
- 搜索 `--diagnostics` 与详细报告 API：区分明确空结果和过滤后无结果，记录逐页筛选计数、实际查询与警告。
- 提取 `--schema-strict` / MCP `schema_strict`：禁止 Schema 失败后回退；JSON 文档增加实际提取路径 `extraction` 和结构化 `warnings`。
- 网页、HTML、搜索及批量 MCP 工具增加 `outputSchema` / `structuredContent`，只实现当前 2025-11-25 协议。
- 核心错误增加稳定 `code`、`retryable`、`exit_code` 和可选冷却信息；搜索正文及批量结果保留逐项错误详情。
- 增加 Bing 页面样本、严格 Schema、回退来源、缓存隔离、MCP 契约及当前协议验证。
- 批量 JSONL 逐项输出、输入/完成顺序、背压、持久检查点、失败重试和单主机调度；MCP 支持带元数据的流式进度结果及最终统计。
- 增加子进程强制终止后恢复、文件锁、尾部半行恢复、身份校验、失败项重试与通知背压测试。

### Changed

- CLI 按命令职责拆分，MCP 按通用传输和业务处理器拆分；标准流绑定移至 CLI，Rust 调用方改用 `McpServer::run_transport`。
- 工作流停止原因和同步事件改为枚举，JSON 名称保持不变；CLI、核心与 MCP 共用分页、浏览器及同步存储预算限制。
- 快照提交和爬虫检查点 I/O 移到阻塞任务，取消调用时运行中的写入继续持锁；状态占用、损坏、预算超限与任务失败使用独立错误码。
- 声明 Rust 1.98.1 支持基线，禁止本项目 unsafe 代码；CI 增加无默认特性编译、文档契约检查、支持基线编译及独立浏览器测试。
- CLI、Shell、MCP 与深度调研统一使用 Bing，默认搜索上限 10 条。
- 平台过滤改为 --scope PLATFORM；结果在本地校验域名和路径后再应用数量上限。
- 增加后台 HTTPS 连接预检、连接与响应计时、连接复用识别、进程内缓存、代理隔离及限流冷却。
- 搜索支持 markdown/json/jsonl，调研支持 markdown/json；参数依赖和格式在执行前校验。
- 搜索与调研 MCP 声明拒绝未定义参数；更新后需重启 MCP 服务刷新声明。
- MCP 参数类型、枚举和已声明范围集中校验，规范工具名和动态调用使用同一路径；结构化输出按内置契约校验。
- 搜索 JSON/JSONL 统一为报告对象，不再返回旧的裸结果数组；CLI 长参数和子命令仅保留规范名称。
- 文档要求当前提取来源字段，缓存键升级为 v4，不迁移旧结构。
- CLI、MCP 和生成报告的提示与报错移除装饰表情，保留原始网页内容。

### Removed

- 非 Bing 搜索引擎、聚合路由和人工验证窗口。
- CLI --engine、--yandex-region、旧平台布尔开关，以及对应 MCP 引擎/地区参数。
- Rust SearchEngineKind、地区专用包装函数和多引擎合并接口。
- 未使用的 CLI 直接依赖及私有响应字段；删除弃用的无作用域 Cookie 接口和旧搜索包装函数。
- MCP 旧协议降级、旧工具名、`tkless_` 前缀和 CLI 参数/子命令别名。

### Fixed

- 请求凭据按初始来源隔离，实际 DNS 解析失败时拒绝请求；修复 HTML 净化和资源下载边界。
- 为平台响应、图片、压缩数据、缓存及 MCP 输入增加大小限制，下载和状态文件使用原子提交。
- 浏览器请求检查覆盖任务页面导航与子资源；隔离边界及剩余限制见安全加固记录。
- 防止 Bing 忽略 site: 限制后混入其他域名结果；拦截相似域名、用户信息段及参数中的域名伪装。
- Chromium 拒绝隐藏 target 时，浏览器预检降级为任务专属后台标签页并清理自身 target。
- 清理帮助信息中的重复默认值，校正诊断级别说明。
- 搜索保留连字符、引号、排除词及年份，取消自动查询改写；修复 inurl 等操作符被误当成域名、site 路径大小写丢失的问题。
- 未识别或损坏的 Bing 页面明确返回解析错误，不伪装为空结果；Schema 列表全为空值时不再视为有效提取。

### Documentation

- 更新 README、发行说明、MCP 接入、Bing 搜索和参数迁移文档。
- 最近代码验证（2026-10-30）：307 项离线测试和 1 项真实浏览器测试通过；Clippy、格式、无默认特性编译、Release 构建及文档契约检查通过。

---

## 2026-10-19 工作区历史记录（部分功能已移除）

以下内容保留当时的实现记录。旧引擎、参数、性能数字和能力描述不代表当前版本；当前用法以 [README](README.md) 与 [Bing 搜索说明](doc/06-bing-search.md) 为准。

### 🌟 核心突破与重大更新

#### 1. 全球化核心平台适配器扩充 (21+ 原生平台矩阵)
- **YouTube 深度适配器 (`tkless-core::adapters::youtube`)**：
  - 自动识别并解析视频标准地址、短链接 (`youtu.be`) 与 Shorts 短视频；
  - 提取视频元数据（标题、频道作者、发布时间、时长、播放量与简介提纯）；
  - 智能提取视频章节时间戳（Chapters）；
  - 深度解析原生多语言字幕轨（`timedtext` XML/JSON3），结构化为带精确时间标签的 Markdown 文本实录。
- **Hacker News 极客社区适配器 (`tkless-core::adapters::hackernews`)**：
  - 官方 Firebase REST API + HTML 双通道支持；
  - 抽取 Top 30 极客资讯榜单（排名、标题、直达链接、得分与作者）；
  - 提取讨论帖正文与递归层级缩进评论树（Comment Tree）。
- **Discourse 通用引擎矩阵 / linux.do 适配器 (`tkless-core::adapters::discourse`)**：
  - 原生适配 `linux.do` 论坛及全球主流 Discourse 技术社区（`users.rust-lang.org`、`community.openai.com` 等）；
  - 优先调用原生 `/t/{id}.json` REST API 零无头浏览器直出，遇风控挑战时平滑回退至 HTML 提取；
  - 提纯主楼正文、楼层回复、点赞互动与完整代码块。
- **X / Twitter 适配器 (`tkless-core::adapters::twitter`)**：
  - 基于 Twitter 官方只读 Syndication API，免登录、免 API Key、抗风控挑战；
  - 提取单推文正文、高分辨率多图与视频、引用推文（Quoted Tweet）嵌套卡片及互动指标。
- **Papers With Code 学术与代码适配器 (`tkless-core::adapters::paperswithcode`)**：
  - 提取学术论文 Abstract 摘要与作者列表；
  - 自动提取 arXiv 原文直达链接与 PDF 下载地址；
  - 抽取官方/社区 GitHub 开源实现与 Stars 关注度；
  - 结构化提取 SOTA Benchmark 基准评测结果表格。

#### 2. 多源搜索引擎生态扩展 (Brave / Yandex / SearXNG / Google)
- **Brave Search 全球独立检索 (`search_brave`)**：
  - 支持官方 `BRAVE_API_KEY` 高速 REST API，无 Key 时自动切换为 Web HTML 结果解析；
  - 支持 auto-CDP 验证码挑战容灾。
- **Yandex 独立搜索引擎 (`search_yandex`)**：
  - 支持 Yandex Web 搜索结果解析，覆盖泛欧与技术正文，配备 SmartCaptcha 防爬感知。
- **SearXNG 聚合元搜索引擎 (`search_searxng`)**：
  - 支持通过环境变量 `SEARXNG_URL` 或 CLI 参数 `--searxng-url` 指定私有化实例（默认回退 `https://searx.be`）；
  - 直接对接标准 JSON API，提供零追踪、高保真的纯净搜索数据流。
- **Google 搜索源 (`search_google`)**：
  - 支持 Google 网页搜索结果结构化解析与重定向解包（`unwrap_google_url`）。
- **多引擎并发轮询聚合 (`merge_multi_engine_search_results`)**：
  - 升级 `SearchEngineKind::All`，支持在 Bing、DuckDuckGo、Brave 等多个可用引擎间并发查询，基于 URL 规范化与 SimHash 汉明距离双重去重，全面提升信息覆盖率与召回准确度。

### 🌟 核心突破与重大更新

#### 1. 原生 PDF 智能解析与 Markdown 重排 (`tkless-core::pdf`)
- **纯 Rust 零外部依赖解析**：基于 `pdf-extract` 与 `lopdf` 构建纯 Rust PDF 解析引擎，无需安装 Poppler、PDFium 或外部 C/C++ 动态链接库，摆脱系统级 OpenSSL 编译绑定，全平台（Windows / macOS / Linux）开箱即用。
- **全通道输入无缝支持**：自动感知输入形态，无缝支持远程 `.pdf` URL、本地 `.pdf` 文件路径以及管道 `stdin` 输入流（如 `curl -s ... | tkless -`）。
- **页眉/页脚/页码智能去噪**：内置高频页眉、页脚及纯数字/罗马数字页码模式识别器，剔除跨页重复出现的版权声明与页码杂讯。
- **Markdown 结构化重排**：智能重组 PDF 断行碎片，恢复自然段落缩进与多级标题层级（H1/H2/H3），生成高密度、低 Token 消耗的纯净结构化文档。

#### 2. Autonomous Deep Research 多跳自主深度调研体系
- **多轮多跳递归探索 (`tkless research` & MCP `deep_research`)**：支持设置调研主题、最大探索跳数（hops）与分支发散度。基于前置抓取结果自动生成后续追问子查询，形成发散与收敛闭环。
- **多源并发抓取与交叉比对**：自动化并行检索多个引擎并抓取高权重网页，通过交叉比对不同信源消除幻觉。
- **结构化研报生成与思维链留痕**：最终自动沉淀包含“核心摘要”、“关键发现矩阵”、“详细论证”与“参考信源列表”的 Markdown 深度研究报告，并完整记录研究跳数与决策过程。

#### 3. CDP 资源拦截加速 (3~5x) 与自动懒加载滚动
- **智能资源拦截策略 (`--cdp-block-images`, `--cdp-block-media`, `--cdp-block-fonts`)**：在 Chrome DevTools 协议层拦截大体积图片、音频、视频、Web 字体等非必要静态资源加载，网络吞吐量降低 70% 以上，首屏渲染提速 3~5 倍。
- **平滑渐进式页面滚动 (`--cdp-auto-scroll`)**：支持自适应步长平滑滚动到底部，彻底触发现代 SPA / 瀑布流站点的无限懒加载与动态水合，确保动态长文本完整捕获。

#### 4. DuckDuckGo 引擎集成与双引擎并发去重检索
- **全球化与海外开发者生态支持**：在原有 Bing 基础之上，全新引入 DuckDuckGo 搜索源，支持 `-e all`（并发聚合）、`-e bing`、`-e duckduckgo` 灵活指定。
- **双引擎并发与 SimHash 语义去重**：`all` 模式下双引擎并行发起查询，基于规范化 URL 签名与 SimHash 汉明距离进行双重去重，消除冗余并提供更均衡的搜索覆盖率。
- **弹性容灾降级与显式告警**：引擎遇到反爬验证或网络超时时保持优雅降级，并在结果中明确标识可能受阻的信源，杜绝空结果假象。

#### 5. 声明式 Schema 抽取全方言统一与容错回退
- **全方言模式通用支持**：统一兼容平铺键值对（`{"title": "h1", "price": ".amount"}`）、嵌套 `properties` 规范以及标准 JSON Schema 格式。
- **智能注解剥离与选择器派生**：自动剥离类型提示（如 `| number`、`| string`），并根据字段语义启发式推导最优 CSS 选择器。
- **容错降级与明确诊断**：当选择器未匹配到任何字段时，不再抛出异常崩溃，而是输出清晰的字段诊断提示（`matched: 0`）并平滑回退至正文全文抽取。

#### 6. 维基百科与信息框内联 CSS 噪声彻底根除
- **精准限定内容边界**：维基百科适配器正文抽取根严格锁定 `#mw-content-text`，彻底剔除 Vector 2022 皮肤的目录侧栏（`.vector-toc`）与 50+ 门语言切换长列表。
- **内联样式穿透剥离**：彻底清除正文和 Infobox 表格中内嵌的 `<style>`、`@media` 以及 `.mw-parser-output` CSS 类声明，使提取出的正文及导出的 CSV/SQL 数据恢复极致纯净。

#### 7. 全链路环境自检诊断 (`tkless doctor`)
- **一键诊断子命令**：新增 `tkless doctor` 命令，全面探测系统运行环境：
  - 系统架构与操作系统版本；
  - 本地 Chrome / Microsoft Edge / Chromium 无头浏览器就绪状态与路径探测；
  - 本地响应缓存（Response Cache）读写权限、容量及磁盘配额统计；
  - 核心搜索引擎与外部代理服务器的网络连通性及延时探测。

#### 8. CLI 体验净化、参数体系标准化与架构高内聚解耦
- **PowerShell 与非 TTY 体验净化**：仅在终端为 TTY 时展示动态进度；管道/重定向模式下自动抑制实时刷新并支持 `NO_COLOR`，彻底根除 Windows PowerShell 下 exit 0 误报红色 `NativeCommandError` 的视觉痛点。
- **全局选项覆盖与子命令统一**：全局统一支持 `-o/--output`、`-q/--quiet`、`-v/--verbose`、`--stats`，并在 `tables` 等子命令中统一支持 `--table-format`。
- **架构模块解耦与极速轻量化**：拆分 `tkless-cli` 为 `doctor.rs`、`progress.rs`、`shell.rs`、`upgrade.rs` 单一职责模块；剔除 `readability` 默认附带的重复旧版依赖，减少 46 个上游依赖包，大幅提升编译速度并压缩编译后二进制体积。

#### 9. 搜索引擎语法解析、相关性弹性门禁与深度调研探针升级
- **语法操作符与引号洗净**：自动识别并剥离 `site:`、`domain:`、`inurl:` 以及短语双引号，提取为结构化过滤规则，彻底杜绝语法词污染 Core Tokens 导致的字面量误判。
- **空间折叠与实体归一化**：支持去空格与连接符折叠比对（如 `huggingface` 自动匹配 `Hugging Face`），解决复合名词切分断裂问题。
- **双层弹性相关性门禁与保底召回**：引入 Tier 1 严格门禁 + Tier 2 弹性召回双层模型，在严格模式无召回时自动启用弹性打分，防止多词和冷门词查询产生假死空结果。
- **Bing 品牌意图对抗与平台定向重写**：针对国内 Bing 索引对 `site:` 语法的降级缺陷，自动注入反向排除与中文平台词（`知乎`、`知乎专栏`、`CSDN`）改写，直达第三方高质量技术正文。
- **Deep Research 调研探针短词化与防饥饿机制**：剥离课题修饰性定语，提炼 2~4 词精炼探针防止搜索引擎 Term Drop；首轮维度遇冷时自动启用核心实体保底检索，保障研究链路 100% 具备有效证据。
- **CDP 全局命令行参数就绪**：将 `--auto-cdp`、`--cdp-auto-scroll` 与 `--cdp-no-block-resources` 升级为全局参数，全子命令无缝透传。

---

## [0.3.0] - 2026-10-18

### 🌟 核心突破与重大更新

#### 1. 垂直生态专属搜索管道与多引擎解耦
- **百度百科官方 OpenAPI 垂直管道 (`search_baike`)**：当使用 `--baike` 或搜索百科词条时，自动路由至百度百科原生词条接口（`BaikeLemmaCardApi`），彻底解决通用搜索引擎（如必应）将词条粗暴改写为官网导航或首词降级百科的弊端，响应耗时 < 100ms，真值命中率 100%。
- **微信公众号专属搜索管道 (`search_wechat_sogou`)**：当使用 `--wechat` 时，自动路由至微信文章专用检索管道，配合会话级 Cookie 保持与动态 JS 解密，直接返回真实的微信公众号文章列表。

#### 2. 内置无头浏览器守护进程与 CDP 自动进程托管 (Auto-Managed CDP)
- **系统浏览器自动探测 (`locate_system_browser`)**：自动扫描并定位宿主机上已安装的 Google Chrome、Microsoft Edge 或 Chromium 可执行文件路径（覆盖 Windows / macOS / Linux 默认安装路径及系统 PATH）。
- **零手动配置守护进程 (`ensure_cdp_daemon_ready`)**：调用 `--cdp` 或触发动态渲染时，工具优先探测目标端口；若未监听且为本地环回地址，自动以 `--headless=new` 静默拉起独立的无头浏览器守护进程接管调试端口，并在端口就绪后执行渲染，无需用户手动拉起 Chrome。

#### 3. 反爬与 CDP 容灾回退（Fallback）无缝化
- **深度防爬挑战感知体系 (`is_anti_scraping_challenge`)**：全面识别 HTTP 403/429/503 异常状态，以及百度百科 Paris/Heicha WAF（`__PARIS_CHALLENGE_START__`）、搜狗反爬验证码（`antispider.min.js`）、Cloudflare 5秒盾与必应/谷歌人机验证码。
- **全自动容灾回退闭环**：当轻量 HTTP Client 遇到上述拦截时，系统不再直接中断报错，而是自动无缝切换至 CDP 无头浏览器执行真实的 DOM 水合渲染与正文提取，实现对上层业务与 AI Agent 完全透明的抗封锁能力。

#### 4. 平台适配器中间重定向解密
- **微信适配器前置防盗链解密 (`decrypt_sogou_wechat_redirect`)**：针对搜狗微信搜索结果中带有动态 JS 混淆与签名的中间跳转页（`/link?url=...`），内置会话预热与 JS 字符串切片拼接引擎，自动剔除 `@` 混淆字符并还原为微信永久/临时直达文章（`https://mp.weixin.qq.com/s?...`），杜绝因裸请求跳转页引发的二次验证码阻断。

#### 5. 工业级搜索门禁与反爬自愈体系
- **商业广告与垃圾推广彻底过滤**：新增 `is_bing_ad_or_promotion` 判定，对淘宝、抖音、TapTap、阿里云盘、Poki 等常见推广域名以及广告容器进行一票否决剔除，广告误放行率降低至 0%。
- **首词降级（Term Drop）严格拦截**：新增 `calculate_relevance` 门禁，检测 query 核心语义分词覆盖率。针对多词复合查询，坚决拦截仅命中首词的粗粒度降级结果（如搜“北京小学入学政策”绝不返回“北京市_百度百科”）。
- **反爬人机验证码主动甄别与报错**：新增 `TklessError::SearchAntiScraping` 状态，主动识别 Bing 验证码挑战（“最后一步 请解决以下难题以继续”），杜绝静默假空结果，并显式指导 Agent 或用户使用 `--cdp` 或切换节点。
- **多阶段智能查询松弛（Multi-Stage Query Relaxation）**：
  - Stage 1: 原始输入全词精确检索；
  - Stage 2: 自动剥离未来年份（如 2025/2026）时效约束重试真实主体政策；
  - Stage 3: 自动进行中文自然紧凑词重组（如将“新能源汽车 补贴 政策”拼接为“新能源汽车补贴政策”），消除 Bing 中文空格分词的 Term Drop 缺陷。
  - 实测中文长尾年份测试集（Group C）真值命中率从 **8% 提升至 100%**。
- **URL 深度净化与官方 Edge 指纹注入**：移除干扰 Bing 路由的 `&setlang=zh-hans` 参数，注入 Microsoft Edge 官方 Desktop User-Agent 与 Sec-Ch-Ua 请求头。

#### 2. 4 线程 HTTP Range 分片并发下载加速
- **突破单连接 QoS 限速**：新增基于 HTTP 206 `Range: bytes=start-end` 的 4 线程分片并发下载管线，总下载速度直接提升 3~5 倍。
- **轻量探针与平滑降级**：通过 `Range: bytes=0-0` 轻量探测文件总大小；若文件小于 4MB 或不支持分片，自动安全降级至单流高吞吐下载。
- **1MB 大吞吐内存写缓冲区**：将底层磁盘写缓冲由默认 8KB 扩容至 1024KB，减少 99% 的磁盘写入系统调用与 CPU 上下文切换。
- **三级智能 CDN 路由优选**：优先调度阿里云、腾讯云、百度云和 Akamai 骨干 CDN，彻底排查并降权低速 PCDN / P2P 节点。

#### 3. 操作系统标准缓存路径重定向与自动 Prune
- **消除工程目录污染**：默认缓存目录由当前工程下的 `.tkless_cache` 重定向至跨平台标准用户目录：
  - Windows: `%LOCALAPPDATA%\tkless\cache`
  - macOS: `~/Library/Caches/tkless`
  - Linux: `$XDG_CACHE_HOME/tkless` 或 `~/.cache/tkless`
- **启动时自动过期清理**：在 `DocumentCache::new()` 实例化时自动静默清理超过 TTL（默认 15 分钟）的过期文件，防止磁盘空间无限制膨胀。

---

## [0.2.0] - 2026-10-18

### Added
- **独创 MCP 两阶段动态工具发现架构**：
  - 第一阶段常驻上下文仅保留 2 核心工具（`fetch_webpage`、`web_search`）+ 4 发现元工具（`discover_tools`、`get_tool_schema`、`execute_tool`、`activate_tools`），初始 Prompt Token 消耗骤降 **74.7%**（从 6,724 tokens 降至 1,700 tokens）。
  - 支持运行时通过 `execute_tool` 免注册即时执行，或通过 `activate_tools` 触发 `notifications/tools/list_changed` 动态扩容。
- **全生态平台矩阵查看子命令**：新增 `tkless adapters`（别名 `platforms`），支持以 Markdown 表格或 JSON 结构展示全部 16+ 平台适配器能力。
- **一键自升级子命令**：新增 `tkless upgrade`（别名 `update`），支持自动检查 GitHub Releases 并热替换更新本地二进制。

### Changed
- **精简搜索引擎架构**：全面移除因机房 IP 风控频发的 Google 搜索，统一标准化为国内网络高可用的 Bing 搜索引擎。
- **电报化轻量化描述**：全量精简 22 个 MCP 工具与数十项入参的描述文本，减少 35% 常驻 Token 开销。

---

## [0.1.0] - 2026-10-17

### Added
- **品牌全面确立**：正式命名为 **TkLess**（*“Spend less tokens, perceive more web.”*）。
- **多平台免解压单二进制发行**：GitHub Actions 发布工作流支持直出 Windows `.exe`、macOS 原生 `.dmg` 与 Linux 静态链接独立二进制。
- **16+ 原生平台深度适配器**：全面适配 B站、知乎、微信、小红书、GitHub、Hugging Face、ArXiv 等平台。
- **核心数据提取工具链**：支持 `tkless tables`（数据表格转标准 SQL DDL/INSERT）、`tkless diff`（文本统一差异对比）与 `tkless shell`（交互式 REPL 会话）。
