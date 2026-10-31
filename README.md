<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="TkLess Logo">
</p>

<h1 align="center">TkLess</h1>

<p align="center">
  <strong>Spend less tokens, perceive more web.</strong><br>
  面向 AI Agent 与 LLM 管道的 Rust 网页抓取、正文清洗、结构化提取与搜索工具。
</p>

## 核心能力

- **网页提取**：Readability 正文清洗、CSS 定向与排除选择器，输出 Markdown、JSON、纯文本或 HTML。
- **结构化数据**：JSON-LD、OpenGraph、Twitter Card、链接、图片、表格，以及声明式 Schema 字段提取和 DOM 脚手架。
- **Token 与文档处理**：`o200k` / `cl100k` / 启发式计数、按边界截断、Markdown 分块、本地提取式摘要、按标题导出文件。
- **22 个平台适配器**：优先使用平台接口或页面内置数据，提取文章、讨论、视频元数据、字幕及 Skills.sh 技能内容；Skills.sh 支持站内搜索和榜单自动翻页，见[适配说明](doc/08-skills-sh.md)。另可通过 `TKLESS_ADAPTERS_DIR` 加载声明式 JSON 适配器补全长尾站点。
- **Bing 搜索**：默认最多 10 条；支持平台定向、搜索后抓取正文和 SimHash 近似去重。
- **批量与站点抓取**：URL 列表并发提取、站点链接遍历、Sitemap、深度与页数限制、路径范围限制。
- **PDF**：识别远程、本地或 stdin 中的 PDF，提取文本和元数据，再用启发式规则整理为 Markdown。
- **Office**：识别远程 `.docx`/`.pptx`，用内置 ZIP + XML 解析提取标题、段落、标题层级与列表；`.xlsx` 需先导出为 CSV。
- **浏览器渲染**：通过 CDP 使用 Chrome / Edge / Chromium，支持挑战回退、自动滚动、资源拦截、截图与 PDF 打印。
- **MCP 服务**：stdio / 可选 HTTP JSON-RPC，31 个业务工具；动态模式初始暴露 2 个核心工具和 4 个发现元工具。
- **辅助工具**：表格 SQL / CSV / TSV 导出、文本差异、RSS / Atom、REPL、自检、缓存维护、Shell 补全和自升级。

普通抓取和转换不需要 Node.js、Python 或外部大模型服务。CDP 渲染及截图需要本机已安装的 Chromium 系浏览器，或可连接的远程 CDP 服务。

抓取类命令将结果写入 `stdout`，诊断和进度写入 `stderr`；MCP 模式的 `stdout` 用于单行 JSON-RPC 消息。实际耗时、内存和 Token 节省率随页面、网络及选项变化，可通过 `--stats` 与基准测试评估。

## 安装与快速开始

### 从源码安装

安装可用的 Rust 工具链后，在仓库根目录执行：

```bash
cargo install --locked --path crates/tkless-cli --force
tkless --version
tkless --help
tkless https://httpbin.org/html
```

安装位置通常为 `~/.cargo/bin`，请确保该目录在 `PATH` 中。开发时可以直接运行：

```bash
cargo run -p tkless-cli -- https://httpbin.org/html
```

### 预编译产物与升级

从 [GitHub Releases](https://github.com/smallhug/tkless/releases) 查看已发布产物。当前发布工作流配置如下，具体以对应 Release 附件为准：

| 平台 | 产物 |
|---|---|
| Windows x86_64 | `tkless-windows-amd64.exe` |
| Linux x86_64 | `tkless-linux-amd64` |
| Linux ARM64 | `tkless-linux-arm64` |
| macOS Intel | `tkless-macos-amd64.dmg` |
| macOS Apple Silicon | `tkless-macos-arm64.dmg` |

Windows 将产物重命名为 `tkless.exe` 并放入用户自己的 `PATH` 目录；Linux 将产物重命名为 `tkless` 并添加执行权限；macOS 从 DMG 复制可执行文件到 `PATH` 目录。

```bash
tkless upgrade --check
tkless upgrade
```

升级下载会逐跳检查地址、限制产物大小，并在替换前验证 GitHub Release 的 SHA-256 digest 或配套 `.sha256` 文件。缺少校验信息或校验不一致时拒绝替换；版本比较采用语义化版本规则。

当前源码还会验证下载大小、可执行文件格式、落盘内容及替换前后的 `--version` 输出；使用独占升级锁和独立暂存目录，失败时恢复原程序。Windows 下若旧程序仍被占用，会显示保留的备份路径。上述安装加固尚未发布到 GitHub Release。

**从 0.4.0 更新的注意事项**：该版本升级器未跟随 GitHub 下载重定向，可能把 302 的空响应体安装成程序后报告成功。遇到此问题，请从 Release 页面手动下载对应二进制并核对 SHA-256，或在源码目录运行 `cargo install --path crates/tkless-cli --force --locked` 恢复。旧程序自身的升级代码不会因远程版本更新而自动修复。
## 常用工作流

示例中的 `example.com` URL、视频号和文件路径是占位符，需要替换为实际目标。不同子命令的选项支持范围请以各自的 `--help` 为准。

### 单页提取与本地输入

```bash
# Markdown、完整 PageDocument JSON、元数据前缀
tkless https://httpbin.org/html
tkless https://example.com/article -f json -o article.json
tkless https://example.com/article --frontmatter

# 控制正文 Token 用量；去除图片及链接 URL
tkless https://example.com/article --max-tokens 2000 --no-images --no-links

# CSS 定向与 DOM 排除；定向处理平台页时可禁用适配器
tkless https://example.com/article --no-adapter --selector "article" --remove-selector ".ads, .footer"

# 本地 HTML 文件或 stdin；base-url 用于解析相对链接
tkless ./page.html --base-url https://example.com/
cat ./page.html | tkless - --base-url https://example.com/

# 本地启发式摘要、Markdown 分块和按标题导出
tkless https://example.com/guide --summarize
tkless https://example.com/guide --chunk --chunk-size 500 --chunk-overlap 50 -f json
tkless https://example.com/handbook --out-dir ./handbook

# 页面统计与响应缓存（默认 TTL 为 900 秒）
tkless https://httpbin.org/html --stats
tkless https://httpbin.org/html --cache --cache-ttl 900
tkless cache --help
```

PowerShell 文本管道示例：

```powershell
Get-Content -Encoding UTF8 ./page.html -Raw | tkless - --format markdown
```

`--max-tokens` 限制正文 Token 数，并将截断提示及代码块闭合计入预算；预算太小时省略提示，JSON 的 `truncated` 仍会标记截断。单页 JSON 的元数据与响应包装不计入正文预算。`--chunk-size` 是目标块大小，完整代码块或表格可能超过该值。

缓存使用版本化 SHA-256 键，覆盖全部提取选项、Schema 文件内容、网络配置和目标凭据，按请求身份隔离。旧版缓存不会与新版键混用；需要维护时可运行 `tkless cache --clear`。

### 结构化字段、资产与表格

```bash
# 页面原生结构化数据、链接、图片、表格
tkless https://example.com/product --json-ld
tkless https://example.com/page --extract-links --extract-images --extract-tables -f json

# 从文件加载 Schema，避免不同 Shell 的内联 JSON 转义差异
tkless https://example.com/products --schema ./schema.json -f json
tkless https://example.com/products --schema ./schema.json --schema-strict -f json
tkless https://example.com/products --scaffold

# 仅导出表格
tkless tables https://example.com/data --format sql --table-name quarterly_report
tkless tables https://example.com/data --format csv -o tables.csv
tkless https://example.com/data --tables-only --table-format json

# 下载正文中的图片并回写本地链接
tkless https://example.com/guide --download-images --assets-dir ./assets -o guide.md
```

`schema.json` 示例：

```json
{
  "items_selector": ".product",
  "title": "h2",
  "price": ".price | number",
  "url": "a@href"
}
```

Schema 解析失败或未提取到有效值时，默认回退到正文，并在 JSON 的 `warnings[]` 中返回 `schema_fallback`。加上 `--schema-strict` 后直接报错，不回退；该选项要求 `--schema`，不表示所有字段都必须非空，也不替代完整 JSON Schema 校验。PDF 不支持 CSS Schema，严格模式会明确拒绝。

文档 JSON 的 `extraction` 记录实际提取方式（`html`、`schema`、`platform_adapter`、`feed`、`pdf`）、输入路径（`local`、`http`、`cdp`）和适配器名称。`warnings[]` 还会记录适配器失败及浏览器回退等情况；缓存命中保留这些信息。

### 搜索与搜索后提取

搜索统一使用 Bing，默认最多返回 10 条。无需指定引擎；用一个 `--scope` 参数选择平台。

```bash
tkless search "Rust 异步编程"
tkless search "Tokio runtime" --scope github -l 10 -f json
tkless search '"near-duplicate" -advertising 2026' --diagnostics -f json
tkless search "技术文章" --scope wechat
tkless search "光合作用" --scope baike

# 按需使用浏览器渲染，或连接已有 CDP 浏览器
tkless --auto-cdp search "DeepSeek" -f json
tkless search "DeepSeek" --cdp http://127.0.0.1:9222 -f json

# 并发抓取正文，并进行近似去重
tkless search "Tokio runtime" --fetch-content --dedup -l 3 --max-tokens-per-page 1500 -f json

# 时间/语言/地区筛选（尽力而为，报告会附加警告）
tkless search "Rust 2026" --since 2026-01-01 --until 2026-06-30 -f json
tkless search "Rust async" --lang zh-CN --region cn -f json
```

`--scope` 支持 `all`、`wechat`、`zhihu`、`github`、`juejin`、`csdn`、`medium`、`substack`、`arxiv`、`huggingface`、`wikipedia`、`reddit`、`stackoverflow`、`v2ex`、`baike`。平台定向转换为 Bing 的 `site:` 查询，并在本地校验结果 URL；关键词中已有 `site:` 过滤时优先保留原过滤。

`--limit` 范围为 1–100；`--dedup`、`--max-tokens-per-page` 要求 `--fetch-content`。`--since`/`--until`/`--lang`/`--region` 使用 Bing 未公开的参数，属于尽力而为：启用时报告附加 `search_filters_best_effort` 警告，请核验结果。CLI、Shell 和 MCP 均直接使用 Bing；旧的引擎选择参数及平台布尔开关已移除。搜索结果仍保留 `engine: "bing"` 来源字段。

搜索前默认在后台执行 HTTPS 连接预检，成功缓存 60 秒、失败缓存 5 秒。慢连接或状态未知仍尝试搜索，限流按 `Retry-After` 冷却。使用 `-vv` 查看诊断，结构化输出包含查询与页面诊断。`--retry-budget N` 是根选项，为整个运行设置共享重试上限，避免失败站点耗尽重试；`--log-format json` 可把 stderr 日志（含致命错误）改成每行一个 JSON 对象（便于采集）。详细用法见 [Bing 搜索说明](doc/06-bing-search.md)。

搜索保留原始查询的连字符、引号、排除词和年份，不再自动放宽查询。JSON 始终返回报告对象，JSONL 返回单行报告；`--diagnostics` 控制 Markdown 中是否展示详细诊断，包含原始与实际查询、页面状态、过滤计数及警告。未知页面结构会报解析错误，不会作为“无结果”成功返回。

### 批量抓取与站点知识库

`urls.txt` 每行填写一个 URL：

```bash
tkless batch ./urls.txt --concurrency 5 --max-tokens 1500 -f json -o batch.json
cat ./urls.txt | tkless batch - --concurrency 5

tkless crawl https://example.com/docs --max-depth 2 --max-pages 20 --same-path-prefix -o docs.md
tkless crawl https://example.com/sitemap.xml --sitemap --max-pages 50 -f json -o site.json
tkless crawl https://example.com/docs --dedup --delay-ms 500 -f jsonl -o site.jsonl

# 额外输出 Markdown 知识库压缩包
# 或同时导出 EPUB 3 电子书
tkless crawl https://example.com/docs --max-pages 50 --bundle ./site.zip --epub ./site.epub -o docs.md
```

`batch` 和 `crawl` 均支持有界并发、`--delay-ms` 请求启动间隔、检查点与失败重试。`crawl` 另支持时间、页面输出和待抓队列预算，以及可选 robots.txt 检查；Sitemap index 会递归遍历子地图并检测循环，单次运行最多检查 10,000 个地图。`crawl --bundle site.zip` 可额外输出 Markdown 知识库压缩包（README 索引 + 每页一个 `.md`），`--epub site.epub` 可导出 EPUB 3（标题目录 + 逐页 XHTML，正文保留原始 Markdown）。恢复及完成状态见 [采集工作流](doc/10-collection-workflows.md)。

**批量 JSON 输出**：返回 `{total, succeeded, failed, resumed, fetched, results}`，每项包含从 0 开始的 `index`、`url`、`document`、`error`、`exit_code` 和可选错误详情。JSONL 每项交付后刷新，默认按输入顺序；`--order completion` 按完成顺序交付。CLI 批量或站点任务只要有失败项，默认写出报告后非零退出；可加 `--allow-partial` 接受部分失败。MCP 批量任务在全部失败时返回 `isError: true`，部分失败保留在结果中。

```powershell
tkless batch ./urls.txt -f jsonl --order completion --checkpoint ./batch.state.jsonl -o ./results.jsonl
tkless batch ./urls.txt -f jsonl --checkpoint ./batch.state.jsonl --resume --retry-failed -o ./results.jsonl
```

新任务要求检查点不存在；恢复使用原 URL 列表和提取/请求配置。已保存结果会重新交付，输出文件重建，成功页面不重复抓取。详见 [批量任务说明](doc/07-batch-jobs.md)。

### 分页、增量同步与浏览器步骤

```bash
# 配置中的 URL、分页和字段规则需要按目标网站填写
tkless collect --config ./collect.json -o collection.json
tkless sync --config ./collect.json --state ./collection-state -o changes.json
tkless browse https://example.com/search --plan ./browser.json -o rendered.json
```

`collect` 支持下一页链接、页码与 JSON 游标，以及列表详情关联；接口不是 GET 时可通过配置中的 `request` 指定 POST/PUT、请求头与请求体模板。`sync` 保存快照并输出新增/更新/删除事件，采集不完整时不删除旧条目；`watch` 一轮执行多个采集任务并把非 `unchanged` 事件投递到文件或 Webhook（调度交给 cron）；`browse` 支持等待元素、填写文本、点击、滚动到底、可选展开 Shadow DOM/同源 iframe，以及按 MIME 与大小上限捕获 XHR JSON。配置示例与限制见 [采集工作流](doc/10-collection-workflows.md)。
### 平台适配器与视频

```bash
tkless adapters
tkless adapters -f json

tkless https://github.com/tokio-rs/tokio
tkless "https://news.ycombinator.com/item?id=39123456"
tkless "https://linux.do/t/topic-slug/12345"
tkless "https://www.youtube.com/watch?v=dQw4w9WgXcQ" --subtitles

# B站视频详情、字幕、弹幕、评论与平台提供的总结
tkless BV1xx411c7mD --subtitles --comments --comments-limit 10
tkless BV1xx411c7mD --danmaku --danmaku-limit 50
tkless BV1xx411c7mD --bili-sum
tkless https://www.bilibili.com/ --feed

# B站搜索、封面和渐进式 MP4 下载
tkless --bili-search "Rust" --bili-page 2 --bili-order click
tkless BV1xx411c7mD --cover
tkless BV1xx411c7mD --download-cover ./cover.jpg
tkless BV1xx411c7mD --download-video ./video.mp4 --quality 720p
```

当前内置平台：Bilibili、YouTube、微信、知乎、小红书、掘金、GitHub、Hacker News、Discourse、X / Twitter、Papers With Code、CSDN、Medium、Substack、arXiv、Hugging Face、Wikipedia、Reddit、Stack Overflow、V2EX、百度百科。

平台之间能力不同；适配器存在并不代表支持该站全部内容类型。字幕需源视频提供可访问的字幕轨，B站总结依赖平台接口；视频下载当前针对 B站渐进式 MP4，不包含通用视频下载或 DASH 音视频合并。

### Cookie 与登录

```bash
# B站扫码登录后自动合并保存 Cookie
tkless login bilibili
tkless login bilibili -o ./cookies.txt --no-browser

# 知乎当前打开登录页并提供手动复制 Cookie 的说明
tkless login zhihu
tkless https://zhuanlan.zhihu.com/p/123456 --cookie-file ./cookies.txt
```

默认会尝试读取当前目录的 `cookies.txt`。支持按平台前缀存放凭据，例如：

```text
bilibili: SESSDATA=...; bili_jct=...
zhihu: d_c0=...; z_c0=...
```

Cookie 文件为本地明文，请只在可信目录保存；仓库已忽略 `cookies.txt`。无前缀命令行 Cookie 按初始目标 Host 限制作用域，带前缀凭据按支持的平台匹配。

也可使用仅限本用户读取的凭据存储（Windows `%APPDATA%\tkless\credentials.json`，其他平台 `$XDG_CONFIG_HOME/tkless/credentials.json` 或 `~/.config/tkless/credentials.json`，可用 `TKLESS_CREDENTIALS_FILE` 覆盖）：

```bash
# 从 stdin 读取 Cookie，避免进入 shell 历史
printf '%s' 'SESSDATA=...; bili_jct=...' | tkless auth set bilibili
tkless auth list
tkless auth remove bilibili
```

存储优先使用 `--cookie`/`--cookie-file`；二者均未提供时才按平台读取凭据库。库文件在 Unix 下以 `0600` 写入，单平台 Cookie 上限 64 KiB、最多 32 个平台；进程启动时读取一次，修改后需重启长任务。

可选持久化 Cookie Jar：`--cookie-jar ./jar.json`（或环境变量 `TKLESS_COOKIE_JAR`）。启用后把响应的 `Set-Cookie` 按**精确主机**、路径、过期时间与 `Secure` 属性保存到 JSON，并在同一主机的后续请求中回放；`Domain` 属性被忽略，不做跨子域扩展，避免在没有公共后缀列表时把一个站点的 Cookie 注入到无关域。Jar 上限 1024 条、单条值 4 KiB、文件 4 MiB，Unix 下以 `0600` 写入；损坏或不支持版本的文件会让启动明确报错。它与 `--cookie`/`--cookie-file` 可同时使用，显式凭据在同名 cookie 上优先。

### 动态网页、截图与网络选项

```bash
# 显式 CDP；本地端点未启动时会尝试探测并启动系统浏览器
tkless https://example.com/app --no-adapter --cdp http://127.0.0.1:9222 --cdp-wait 1000

# 遇到支持识别的反爬挑战时尝试 CDP 回退
tkless https://example.com/app --auto-cdp
tkless https://example.com/feed --cdp http://127.0.0.1:9222 --cdp-auto-scroll
tkless https://example.com/gallery --cdp http://127.0.0.1:9222 --cdp-no-block-resources
tkless https://example.com/page --screenshot ./page.png

# 将渲染后的页面打印为完整 PDF（由浏览器的 Page.printToPDF 生成）
tkless https://example.com/report --pdf ./report.pdf

# 代理、移动端 UA、超时和重试
# 同一主机连续失败 3 次后熔断 30 秒
tkless https://example.com/page --proxy http://127.0.0.1:7890 --mobile --timeout 30 --retries 2 --circuit-breaker 3
```

常规 HTTP 抓取包含私网地址、DNS 结果和重定向检查，默认阻止本地及内网目标；`--allow-private-addresses` 可显式放开。CDP 会执行页面脚本，浏览器产生的后续网络请求不应视为已被普通 HTTP 抓取器的地址检查完全隔离。

`--circuit-breaker N` 启用每主机熔断：同一主机连续 N 次传输错误、超时、5xx 或 429 后，本进程后续请求直接返回 `circuit_open`（退出码 3，带 `retry_after_secs`），不再访问网络。冷却 30 秒后放行一个半开探测；成功则重置，失败则退避翻倍至最长 5 分钟。默认 `0` 禁用；MCP 服务读取 `TKLESS_CIRCUIT_BREAKER`。

### PDF、调研与辅助命令

```bash
tkless https://arxiv.org/pdf/1706.03762.pdf -o paper.md
tkless ./research_paper.pdf --chunk --summarize -f json -o paper.json
# POSIX Shell 中的二进制 stdin 示例；Windows 优先直接传入 PDF 路径
cat ./research_paper.pdf | tkless -

# 远程 Office 文档（DOCX/PPTX）自动转 Markdown
tkless https://example.com/reports/q3.docx -o q3.md

# 启发式维度拆解、搜索、最多两跳引链抓取与摘录汇编
tkless research "Rust Web 框架技术选型" --depth 2 --breadth 3 --max-pages 8 -o research.md
tkless research "长文本上下文压缩" -f json -o research.json

tkless diff ./old.md ./new.md -f json
tkless shell
tkless doctor
tkless doctor --proxy http://127.0.0.1:7890
tkless completions powershell
```

当前能力边界：

- PDF 支持 Token 截断、摘要和分块，并可走缓存路径；尚无扫描件 OCR，复杂多栏、表格及公式的排版还原有限。
- Office 支持 `.docx`（标题样式、段落、简单列表）与 `.pptx`（按页码逐页提取文本），不渲染图片、图表、批注、修订或嵌入对象；`.xlsx` 明确拒绝并提示转 CSV。归档读取限制 512 条目、单条目 32 MiB、总未压缩 64 MiB，拒绝加密与 ZIP64。
- `research` 使用本地启发式规则与提取式摘要，不调用 LLM。发现会附带来源编号，不自动生成未经证据支持的比较表；内容仍需交叉核验。最终报告应用 `--max-tokens` 预算，JSON 会保留合法结构和 `truncated` 标记；预算连基本结构也容纳不了时会明确报错。
- `diff` 使用 Myers 行级算法，生成包含 hunk 头的标准 unified diff；末尾换行的变化也属于差异。
- `--stream` 当前在内容处理完成后逐行写出并 flush，并非网络获取、解析和输出同时进行。
- 单页、`search`、`batch`、`crawl`、`adapters`、`diff` 支持 JSONL。列表型结果逐项输出，单文档输出一个完整 JSON 对象；`research` 仅支持 Markdown 和 JSON。子命令会拒绝未支持的格式。

以下能力明确不在计划内：第二搜索引擎、OCR/ASR 模型、XLSX 解析、代理池、OTLP、S3/对象存储或数据库 sink、MCP SSE/持久会话、批量编译式站点适配器。需要时请用外部工具预处理/后处理；决策记录见 [优化方案 §9.1](doc/12-optimization-plan.md#91-已舍弃的候选功能2026-10-31-决策)。

## MCP 接入

当前服务默认通过 stdio 传输，也可用 `--http ADDR` 提供无状态 HTTP JSON-RPC（`POST /mcp`、`GET /healthz`，默认仅回环，非回环需 `--allow-public-bind` 与 Bearer Token），仅实现 `2026-07-28` 协议：没有 `initialize` 握手，每个请求在 `params._meta` 中携带 `io.modelcontextprotocol/protocolVersion` 与 `clientCapabilities`；服务端实现 `server/discover`，所有结果带 `resultType`。HTTP 请求还必须携带 `MCP-Protocol-Version`、`Mcp-Method`（`tools/call` 另需 `Mcp-Name`）。工具调用并发执行，支持 `notifications/cancelled`，单次调用超时默认 300 秒（`--tool-timeout-secs` 可调），最多 64 个在途调用。长任务可用 `jobs_start` 转为后台执行并以 `jobs_status` 轮询。业务工具和参数见 [tools.rs](crates/tkless-core/src/mcp/tools.rs)。

支持 `mcpServers` 配置的客户端可使用以下配置；若客户端找不到 `PATH` 中的程序，将 `command` 改为可执行文件绝对路径：

```json
{
  "mcpServers": {
    "tkless": {
      "command": "tkless",
      "args": ["mcp", "--dynamic"]
    }
  }
}
```

```bash
# 全量模式：tools/list 返回 31 个业务工具
tkless mcp

# 动态模式：初始返回 6 个工具，按需激活业务工具
tkless mcp --dynamic
tkless mcp --profile dynamic
```

`web_search` 的调用参数示例：

```json
{"query":"Tokio","scope":"github","limit":10,"format":"json"}
```

无需传入 `engine` 或地区参数；旧参数会被拒绝。`limit` 范围为 1–20，默认 10。`auto_cdp` 默认 `false`；`dedup=true`、`max_tokens_per_page` 和 `schema` 要求 `fetch_content=true`。MCP 搜索支持 Markdown/JSON，CLI 搜索另支持 JSONL。协议响应、取消和进度说明见 [MCP 接入文档](doc/02-mcp-tools-extension.md)。

MCP 仅支持 `2026-07-28`；网页提取、HTML 清洗、搜索和批量抓取提供 `outputSchema` / `structuredContent`。旧协议降级、旧工具名和 `tkless_` 前缀已移除。工具错误提供稳定代码和重试信息；文档结构化结果位于 `structuredContent.document`，搜索和批量结果直接位于 `structuredContent`。

环境变量 `TKLESS_MCP_DYNAMIC=1` 也可启用动态模式。两个常驻业务工具是 `fetch_webpage` 和 `web_search`；四个发现元工具如下：

| 元工具 | 用途 |
|---|---|
| `discover_tools` | 按关键词或分类检索业务工具目录 |
| `get_tool_schema` | 获取指定工具的完整参数 Schema |
| `execute_tool` | 无需先激活，即时调用指定业务工具 |
| `activate_tools` | 激活工具；随后重新调用 `tools/list` 刷新声明（本服务不推送 `list_changed`） |

动态模式减少初始工具声明；具体 Token 开销取决于当前 Schema、序列化方式、分词器及宿主注入策略，此处不沿用旧版固定节省率。

31 个业务工具按目录分类如下，四个发现元工具不计入该数量：

| 分类 | 工具 |
|---|---|
| 内容（10） | `fetch_webpage`、`crawl_site`、`batch_fetch_webpages`、`clean_html`、`fetch_feed`、`scaffold_schema`、`collect_pages`、`sync_collection`、`browse_page`、`list_wechat_articles` |
| 搜索（5） | `web_search`、`search_bilibili`、`inspect_links`、`list_adapters`、`deep_research` |
| 媒体（3） | `fetch_bilibili_feed`、`download_media`、`take_screenshot` |
| 分析（6） | `export_tables`、`summarize_content`、`split_document`、`chunk_markdown`、`diff_documents`、`compare_similarity` |
| 系统（7） | `manage_cache`、`login_platform`、`check_auth`、`jobs_start`、`jobs_status`、`jobs_cancel`、`jobs_list` |

**后台任务**：`jobs_start` 接受 `tool`（`crawl_site`、`batch_fetch_webpages`、`collect_pages`、`sync_collection`）与嵌套 `arguments`，立即返回 `job_id`；`jobs_status` 返回状态、耗时、进度与最终结构化结果，`jobs_cancel` 中止任务，`jobs_list` 按状态筛选并以摘要形式列举。任务注册表仅存在于当前进程，最多 4 个排队/运行任务，结束后默认保留 3600 秒（`ttl_secs` 可调），重启即丢失。

```json
{"tool":"crawl_site","arguments":{"seed_url":"https://example.com/docs","max_pages":50,"checkpoint":"./docs.state.json"},"ttl_secs":3600}
```

`login_platform` 的 B站流程支持扫码及保存 Cookie，知乎流程目前返回手动操作说明。`check_auth` 对 B站、知乎、GitHub 尝试远程验证，阻断或无法识别的响应会明确表示状态未知；微信目前只报告已配置，并返回 `verified: false`，不会仅凭 Cookie 字段名判定登录有效。

## CLI 速查

```text
tkless [OPTIONS] [TARGET] [COMMAND]
```

| 子命令 | 用途 |
|---|---|
| `search` | Bing 平台定向搜索与搜索后正文提取 |
| `batch` | 从文件或 stdin 读取 URL 列表并发抓取 |
| `crawl` | 站点链接 / Sitemap 遍历与知识库汇编 |
| `wechat` | 微信公众号名称发现、文章身份与合集翻页；[使用说明](doc/11-wechat-public.md) |
| `collect` | 通用分页与列表详情采集 |
| `sync` | 保存采集快照并输出增量事件 |
| `watch` | 单轮或多轮监控采集，输出变更事件并投递文件/Webhook 通知 |
| `browse` | 执行受限浏览器步骤并提取正文 |
| `research` | 启发式两跳资料检索及摘录报告 |
| `tables` | 表格 Markdown / CSV / TSV / JSON / SQL 导出 |
| `diff` | 本地文本或网页的行级差异 |
| `mcp` | stdio / HTTP MCP 服务 |
| `login` | B站扫码登录或知乎登录指引 |
| `auth` | 管理本地凭据存储（`set` 从 stdin 读取 Cookie，`list`，`remove`） |
| `cache` | 缓存统计与清理 |
| `adapters` | 平台及能力目录 |
| `doctor` | 浏览器、缓存目录和网络连通性自检 |
| `shell` | 交互式 REPL |
| `completions` | Bash / Zsh / Fish / PowerShell / Elvish 补全 |
| `upgrade` | 检查及安装 GitHub Release |

根命令常用选项：

| 选项 | 说明 |
|---|---|
| `-f, --format` | 单页常用 `markdown`、`json`、`jsonl`、`text`、`html`、`raw`；子命令支持 `markdown`、`json`、`jsonl`（`research` 仅支持前两种），根选项会被子命令继承 |
| `-m, --mode` | `article`（默认）、`full`、`raw` |
| `-t, --max-tokens` | 正文截断目标 |
| `--tokenizer` | `o200k`（默认）、`cl100k`、`heuristic` |
| `--selector` / `--remove-selector` | 定向与排除 CSS 选择器 |
| `--schema` / `--scaffold` | 字段抽取 / DOM 骨架分析 |
| `--frontmatter` / `--json-ld` | YAML 元数据前缀 / 仅输出 JSON-LD |
| `--extract-links` / `--extract-images` / `--extract-tables` | 附带结构化资产 |
| `--chunk` / `--summarize` / `--out-dir` | 分块 / 摘要 / 按标题导出 |
| `--no-adapter` | 跳过平台接口，走通用抓取路径 |
| `--cookie` / `--cookie-file` / `--cookie-jar` | Cookie 字符串 / 凭据文件 / 可选持久化 Jar |
| `-H, --header` / `-u, --user-agent` | 自定义 HTTP 头 / UA |
| `--timeout` / `--connect-timeout` / `--retries` | 默认 15 秒 / 10 秒 / 0 次 |
| `--max-size` / `--on-too-large` | 默认 10 MB；超限默认报错，可选择截断 |
| `--cdp` / `--auto-cdp` / `--screenshot` | 浏览器端点 / 挑战回退 / 截图（另支持 `--pdf` 打印） |
| `--retry-budget N` | 根选项：整个运行共享的重试次数上限；省略时不限制 |
| `--circuit-breaker N` | 同一主机连续失败 N 次后熔断；默认 `0` 禁用 |
| `--log-format plain\|json` | stderr 日志格式；json 每行一个对象 |
| `-p, --proxy` / `--mobile` | 代理 / 移动端 UA |
| `--cache` / `--cache-ttl` / `--cache-dir` | 缓存开关 / TTL / 目录 |
| `-o, --output` | 输出文件 |
| `--stats` / `-v` / `-q` | 提取统计（写入 stderr，不污染 stdout）/ 增加日志详细度（可重复）/ 关闭诊断日志 |

完整且最新的参数说明由程序生成，避免依赖过期的帮助快照：

```bash
tkless --help
tkless search --help
tkless crawl --help
tkless research --help
tkless mcp --help
```

### 退出码

| 代码 | 含义 |
|---|---|
| `0` | 命令成功；使用 `--allow-http-errors` 时也可处理非 2xx 页面 |
| `1` | 通用、I/O、平台适配器、CDP、Schema、搜索反爬验证（`verification_required`）、状态与后台任务等未单独映射的错误 |
| `2` | 参数或 URL 格式错误 |
| `3` | 网络错误、超时、限流（`rate_limited`）或 HTTP 5xx |
| `4` | 解码、清洗、转换、PDF / Office / 搜索页解析（`search_parse_error`）错误或 HTTP 4xx |
| `5` | 响应体超过大小限制 |
| `6` | SSRF 地址检查拦截 |

退出码表示当前命令的执行结果；批量任务应同时检查逐项结果和诊断。映射实现见 [error.rs](crates/tkless-core/src/error.rs)。

## 作为 Rust 库使用

```toml
[dependencies]
tkless-core = { path = "path/to/tkless/crates/tkless-core" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use tkless_core::{ExtractionOptions, TklessEngine};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = TklessEngine::new()?;
    let options = ExtractionOptions {
        max_tokens: Some(2000),
        ..Default::default()
    };
    let doc = engine.process_url("https://httpbin.org/html", &options).await?;
    println!("{}", doc.content);
    Ok(())
}
```

`process_html` 用于已有 HTML，`process_url_with_cache` 可接入 `DocumentCache`。`tkless-core` 默认启用 `tiktoken` feature；关闭后对应计数回退到启发式估算。

原生搜索使用 `web_search(&engine, query, &SearchOptions)` 并返回 `SearchReport`；正文抓取使用 `fetch_search_results`。批量任务使用 `batch::run_batch` 逐项交付，或用 `batch::collect_batch` 收集完整报告。旧的裸数组搜索和批量入口已删除。

## 项目结构与开发

```text
crates/
  tkless-core/
    src/
      lib.rs                 # TklessEngine 统一入口
      model.rs / error.rs    # 配置、文档、错误与退出码
      fetcher.rs / cache.rs  # HTTP、地址及凭据检查、缓存
      cleaner.rs / markdown.rs / extractor.rs
      schema.rs / chunker.rs / token.rs / summarizer.rs / splitter.rs
      search.rs / search_preflight.rs / research.rs / crawler.rs / feed.rs
      cdp/                   # 浏览器连接预检与任务页面管理
      batch.rs / batch/      # 流式调度、单主机限制及持久检查点
      collection.rs / sync.rs / browser_workflow.rs
      workflow.rs / limits.rs / state_file.rs  # 类型化结果、共享约束及持久化
      pdf.rs / cdp.rs / auth.rs / downloader.rs / diff.rs / dedup.rs
      adapters/              # 22 个内置平台适配器与注册表
      mcp/                   # 工具契约、协议分发及 server/ 业务处理器和通用传输
    tests/                   # 提取、批量中断/恢复、CLI 子进程及 MCP 协议验证
    benches/                 # Criterion 合成 HTML 基准
  tkless-cli/
    src/
      main.rs / cli.rs / output.rs
      commands/              # 搜索、批处理、爬虫及采集工作流编排
      shell.rs / doctor.rs / progress.rs / upgrade.rs
    build.rs                 # 构建日期与 Windows 资源
doc/                         # 设计及开发文档
.github/workflows/           # 三平台 CI、多平台 Release
```

当前支持 Rust **1.98.1** 及以上。`scripts/check-docs.cjs` 用于开发时检查文档示例、链接和可执行程序的 MCP 工具目录；Node.js 不属于产品运行依赖。核心库宿主通过 `McpServer::run_transport` 传入异步输入输出流，CLI 负责绑定 stdin/stdout。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p tkless-cli
cargo bench -p tkless-core --bench extraction_bench
```

当前验证命令见上方；Bing 搜索的测试及在线结果见 [搜索说明](doc/06-bing-search.md)。在线搜索受出口网络和上游访问策略影响，不作为离线 CI 的成功断言。

变更与迁移：[工作区更新说明](RELEASE_NOTES.md)、[变更日志](CHANGELOG.md)。

进一步阅读：

- [开发文档索引](doc/README.md)
- [CI/CD 与发布](doc/01-ci-cd-and-release.md)
- [MCP 扩展设计](doc/02-mcp-tools-extension.md)
- [平台适配与功能手册](doc/03-adapters-and-features.md)
- [开发规范与工程标准](doc/04-development-standards.md)
- [搜索引擎在线实测与功能验证](doc/05-search-engine-validation.md)
- [批量流式任务与断点恢复](doc/07-batch-jobs.md)
- [安全加固与当前边界](doc/09-security-hardening.md)
- [分页、增量同步、爬虫恢复与浏览器步骤](doc/10-collection-workflows.md)
- [变更日志](CHANGELOG.md)

设计文档中部分数量、接口示例和性能描述尚未同步；实际可用命令及 Schema 以源码、`--help` 和 MCP `tools/list` 为准。
