<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="PageClaw Logo">
</p>

<h1 align="center">PageClaw (爪页)</h1>

<p align="center">
  <strong>High-Performance, Token-Efficient Web Content Extraction Engine & CLI for AI Agents.</strong><br>
  专为 AI Agent（Claude Code、Cursor、AutoGPT）与 LLM 管道量身定制的毫秒级网页提取与结构化降噪工具。
</p>

---

## 🌟 核心特性 (Key Features)

- **⚡ 毫秒级极速冷启动**：纯 Rust 编写，静态单二进制无依赖分发，启动延迟仅数毫秒，内存占用低（< 15MB）。
- **📉 激进降噪与 Token 预算控制**：
  - 基于 Mozilla Readability 启发式算法，自动过滤导航栏、页眉页脚、弹窗、广告与追踪脚本，平均为 LLM 节省 **70%~90% 的上下文 Token**。
  - 支持 `--max-tokens <N>` 智能截断：感知段落与句子边界，并在截断时**自动补全并闭合 Markdown 代码块（```` ``` ````）**。
- **🔌 Agent 管道友好 (Strict Stream Separation)**：
  - `stdout`：严格纯净，仅输出清洗后的目标数据（Markdown、JSON、Text）。
  - `stderr`：输出耗时指标、HTTP 状态、Token 统计与调试日志。
- **💎 零成本先验结构化提取**：
  - 原生提取 `<script type="application/ld+json">`（Schema.org）、OpenGraph、Twitter Card 与 Favicon 图标元数据，无需消耗任何 LLM Token 即可获取高置信度结构化数据。
- **💻 代码块语言标签归一化与语法高亮保障**：
  - 自动识别并归一化现代网页中各类复杂的代码块形式（`lang-py`, `language-rust`, `highlight-source-js`, `data-lang="ts"` 等），确保 Markdown 代码块拥有精准的标准语言标识符（如 ```` ```python ````、```` ```rust ````），完美契合 Agent 代码解析与代码执行工具。
- **🌐 智能网络获取**：
  - 基于 `reqwest` + `rustls`，原生支持 HTTP/2、连接池与多重压缩算法（Gzip / Brotli）。
  - 流式读取防内存溢出（OOM Guard），自动探测网页编码（UTF-8、GBK、Shift-JIS、Windows-1252 等）。
- **🏗️ 模块化多包架构**：采用 Cargo Workspace 组织，核心逻辑作为 `pageclaw-core` 库与 CLI 解耦，并预留了 MCP（Model Context Protocol）协议扩展点。

---

## 🏛️ 项目架构 (Workspace Layout)

```text
pageclaw/
├── Cargo.toml                          # Workspace 根配置与依赖版本锁定
├── README.md                           # 项目说明文档
└── crates/
    ├── pageclaw-core/                  # 核心提取与清洗引擎库 (可独立引入)
    │   ├── src/
    │   │   ├── lib.rs                  # PageclawEngine 外观模式接口
    │   │   ├── model.rs                # 领域模型 (PageDocument, Metadata, Options)
    │   │   ├── error.rs                # 结构化错误类型 (PageclawError via thiserror)
    │   │   ├── fetcher.rs              # 异步流式 HTTP 获取与编码自适应
    │   │   ├── cleaner.rs              # DOM 降噪与 Readability 正文提取 (scraper)
    │   │   ├── markdown.rs             # HTML 转高纯净 GFM Markdown (htmd)
    │   │   ├── extractor.rs            # 元数据与 JSON-LD 结构化提取
    │   │   └── token.rs                # Token 预算估算与代码块安全截断 (tiktoken)
    │   └── tests/
    │       └── integration_test.rs     # 自动化管道与集成测试
    │
    ├── pageclaw-cli/                   # 终端 CLI 二进制
    │   └── src/
    │       ├── main.rs                 # 异步 Runtime、退出码与 Stdio 流隔离
    │       ├── cli.rs                  # Clap 命令行定义
    │       └── output.rs               # Stdout 输出渲染 (Markdown/JSON/Text)
    │
    └── pageclaw-mcp/                   # (路线图) Model Context Protocol Server
```

---

## 🚀 快速上手 (Quick Start)

### 1. 编译与安装

```bash
# 调试构建
cargo build --workspace

# 生产级极速优化构建 (启用 LTO 与体积优化)
cargo build --release

# 安装到本地系统路径
cargo install --path crates/pageclaw-cli
```

### 2. 常用命令行示例 (CLI Examples)

#### 基础网页抓取为 Markdown
```bash
pageclaw https://httpbin.org/html
```

#### 带 YAML 元数据前缀 (Title, URL, Published, Author)
```bash
pageclaw https://example.com/blog/article --frontmatter
```

#### 控制 Agent 上下文 Token 预算 (防止撑爆上下文)
```bash
pageclaw https://example.com/long-page --max-tokens 2000
```

#### 仅提取页面内置的 JSON-LD 结构化数据 (电商/新闻/知识库推荐)
```bash
pageclaw https://example.com/product/123 --json-ld
```

#### 完整结构化 JSON 输出 (供 Agent Function Calling 消费)
```bash
pageclaw https://example.com/article -f json | jq .metadata
```

#### 标准输入 (Stdin) 管道模式 (与其他抓取或渲染工具联动)
```bash
# 将本地 HTML 或其他工具抓取的 HTML 通过管道喂入
cat noisy_page.html | pageclaw - --format markdown

# Windows PowerShell 管道示例
Get-Content ./index.html -Raw | pageclaw - -f markdown
```

#### 进一步极限压缩 Token：剔除所有图片与链接 URL
```bash
pageclaw https://example.com/docs --no-images --no-links
```

#### ⚡ 主流平台原生适配器 (Platform Adapters)
针对现代复杂 SPA 网站（如 B站），自动拦截并调用原生接口，**在 150ms 左右直接输出 GFM Markdown 表格或结构化 JSON**，避免启动无头浏览器的巨额资源开销：

```bash
# 1. 抓取 B 站首页个性化推荐流 (150ms 极速获取视频推荐表格)
pageclaw https://www.bilibili.com/ --feed

# 2. 抓取 B 站全站热门排行榜 Top 视频
pageclaw https://www.bilibili.com/v/popular/rank/all

# 3. 抓取 B 站某一频道排行榜 (如科技区、动画区、生活区)
pageclaw https://www.bilibili.com/v/popular/rank/tech
pageclaw https://www.bilibili.com/v/popular/rank/anime

# 4. 抓取 B 站具体视频详情页 (包含标题、作者、数据统计、简介及 Schema.org 结构化数据)
pageclaw https://www.bilibili.com/video/BV1xx411c7mD
# 也支持直接传入短 BV 号:
pageclaw BV1xx411c7mD

# 5. 提取 B 站高清视频封面图片 URL (输出纯净 URL 到 stdout)
pageclaw BV1xx411c7mD --cover

# 6. 下载 B 站视频封面到本地文件 (默认保存为 {bvid}_cover.jpg)
pageclaw BV1xx411c7mD --download-cover
# 或指定保存路径:
pageclaw BV1xx411c7mD --download-cover ./covers/my_cover.jpg

# 7. 下载 B 站渐进式 MP4 视频流 (零 ffmpeg 依赖，单文件直接播放)
pageclaw BV1xx411c7mD --download-video
# 指定保存名称与清晰度 (720p / 480p / 360p):
pageclaw BV1xx411c7mD --download-video my_video.mp4 --quality 720p

# 8. 禁用平台适配器，强制回退为标准纯静态 HTML 爬取
pageclaw https://www.bilibili.com/video/BV1xx411c7mD --no-adapter

# 9. 启用响应二级缓存 (二次访问毫秒级即时响应)
pageclaw https://httpbin.org/html --cache

# 10. 抓取 GitHub 仓库纯净 README 文档 (丢弃 90%+ 导航、Star、Commit DOM 噪音)
pageclaw https://github.com/tokio-rs/tokio
# 抓取 GitHub Issue 对话讨论链 (提取标题、状态、首楼描述及后续回复列表)
pageclaw https://github.com/tokio-rs/tokio/issues/1234

# 11. 抓取掘金技术社区博文 (自动解包 link.juejin.cn 外链，保留纯净技术代码块)
pageclaw https://juejin.cn/post/7123456789012345678

# 12. 抓取小红书图文笔记 (自动解析 __INITIAL_STATE__ 服务端状态树、提取无水印图集与点赞收藏数)
pageclaw https://www.xiaohongshu.com/explore/654321000

# 13. 启动原生 Model Context Protocol (MCP) 服务器 (stdio 传输)
pageclaw mcp

# 11. B 站全网视频关键词搜索 (自动清洗高亮标签，输出结构化 Markdown 表格)
pageclaw --search "Rust编程语言"

# 12. 搜索翻页与排序 (按最多播放 click、最新发布 pubdate、最多弹幕 dm、最多收藏 stow)
pageclaw --search "Rust" --page 2 --order click

# 13. 直接传入 B 站网页端搜索链接
pageclaw "https://search.bilibili.com/all?keyword=Rust"

# 14. 🎯 方向三：CSS 选择器定向提取 (DOM 阶段直接剪除 95%+ 无关内容，Token 消耗压缩至几百)
pageclaw https://httpbin.org/html --selector "h1"
pageclaw https://example.com/article --selector "article.content, #readme, table"

# 15. 🎯 方向三：全量超链接与图片资产清单提取 (支持 JSON 与 Markdown 两种形态)
pageclaw https://example.com/page --extract-links -f json
pageclaw https://example.com/page --extract-images -f json

# 16. 🎯 方向三：表格深度结构化解析 (自动提取 2D 数组与按表头索引的记录字典)
pageclaw https://example.com/data --extract-tables -f json

# 17. 🎬 方向二：B 站官方/AI 字幕轨提取 (毫秒级提取标准时间轴 [01:25 - 01:30] 内容)
pageclaw BV1xx411c7mD --subtitles

# 18. 🎬 方向二：B 站时间轴高能弹幕抓取 (按时间升序排版 GFM 表格，支持 --danmaku-limit 控制数量)
pageclaw BV1xx411c7mD --danmaku
pageclaw BV1xx411c7mD --danmaku-limit 50

# 19. 🎬 方向二：B 站视频热门精选高赞评论抓取 (支持 --comments-limit 控制数量，默认 20 条，支持跨页翻倍)
pageclaw BV1xx411c7mD --comments
pageclaw BV1xx411c7mD --comments-limit 10

# 20. 🧠 官方 AI 总结提纲提取 (直接获取官方大模型核心摘要与带时间戳的章节提纲，省去重度总结 Token)
pageclaw BV1xx411c7mD --bili-sum

# 21. 🎬 方向二：多平台前缀 Cookie 凭据管理与双格式载入 (自动按网站前缀区分，避免同一文件中多网站信息覆盖)
# cookies.txt 支持多平台前缀共存：
#   bilibili: SESSDATA=xxx; bili_jct=yyy
#   zhihu: d_c0="xyz"; z_c0="abc"
#   wechat: wx_token=...
pageclaw BV1xx411c7mD --cookie-file ./cookies.txt
pageclaw "https://zhuanlan.zhihu.com/p/123456" --cookie-file ./cookies.txt

# 22. 🔑 一行命令唤起本地浏览器扫码登录并自动提取保存 Cookie 凭据 (支持 bilibili, zhihu)
# 自动以平台前缀合并写入 cookies.txt，安全保留已有其它网站凭据，绝不覆盖冲掉历史配置
pageclaw login bilibili
pageclaw login zhihu
# 或指定凭据保存路径与禁止自动拉起浏览器（纯终端字符画二维码，单行文字提示）：
pageclaw login bilibili -o ./my_bilibili_cookies.txt --no-browser

# 23. 🌐 免 Key 通用网页搜索与平台定向搜索 (集成 Bing & Google 双引擎，直供 AI Agent 全流程搜索闭环)
# 默认使用 Bing 搜索引擎（国内网络访问极佳，自动解包去除重定向追踪）
pageclaw search "Rust 异步编程最佳实践" --limit 5
# 显式指定使用 Google 或 Bing 引擎
pageclaw search "Tokio 教程" --engine bing -f json
pageclaw search "Rust compilation optimization" --engine google --limit 5
# 自动容灾模式（首选 Bing，遇阻断自动平滑降级到 Google）
pageclaw search "DeepSeek V3" --engine auto --limit 5
# 专享生态定向：一键限定抓取微信公众号、知乎、GitHub、掘金平台
pageclaw search "开源项目" --wechat --limit 5
pageclaw search "Rust 异步运行时" --zhihu --limit 5
pageclaw search "mini-redis" --github --limit 5
pageclaw search "Vue3 源码解析" --juejin --limit 5
# ⚡ 搜索即抓取一体化 (Search & Fetch All-in-One，单次交互并发抓取并清洗各页面正文，Token 预算严格可控)
pageclaw search "Rust 异步运行时" --fetch-content --limit 3 --max-tokens-per-page 1500
pageclaw search "Tokio runtime" --fetch-content --limit 2 -f json --max-tokens-per-page 300

# 24. ⚡ 批量并发抓取与多 URL 流水线 (通过多协程并发池并发处理，支持从文件或管道批量输入)
pageclaw batch ./urls.txt --concurrency 5 -o ./batch_results.md
cat ./urls.txt | pageclaw batch - --concurrency 10 -f json

# 25. 📚 整站文档递归爬取与知识库汇编 (BFS 遍历站点/同路径超链接，自动构建带 TOC 目录索引的单文件 Markdown 知识库)
# 从入口 URL 深度递归爬取整套技术文档（限制深度 2，最多 20 页，并发 5，保存为集成知识库）：
pageclaw crawl "https://tokio.rs/tokio/tutorial" --max-depth 2 --max-pages 20 -o ./tokio_docs.md
# 仅爬取相同路径前缀的子文档（例如仅抓取 /docs/ 路径下的页面）：
pageclaw crawl "https://example.com/docs" --same-path-prefix -f json -o ./docs_kb.json
# ⚡ Sitemap 站点地图驱动提速模式：直接解析 sitemap.xml 提取全量规范文档 URL，提速 10 倍：
pageclaw crawl "https://docs.rs/tokio/sitemap.xml" --sitemap --max-pages 50 -o ./tokio_sitemap.md

# 26. 🌐 Chrome DevTools Protocol (CDP) 无头浏览器渲染 Sidecar (针对重度 SPA 与反爬挑战绕过)
# 连接本地运行的 Chrome/Edge 远程调试端口 (如 google-chrome --remote-debugging-port=9222)
# 自动在浏览器中执行 JavaScript、等待 DOM 水合渲染，并安全提取完整的 outerHTML：
pageclaw "https://zhuanlan.zhihu.com/p/123456" --cdp http://127.0.0.1:9222 --cdp-wait 1000
# 与搜索即抓取、整站爬虫协同使用：
pageclaw search "Rust 教程" --fetch-content --cdp http://127.0.0.1:9222

# 27. 🎯 结构化实体提取 Schema (Schema-Driven Entity Extraction，极简声明式字段抽取与 Token 极致压缩)
# 通过内联 JSON 或模式文件，直接从 DOM 抽取指定字段与属性，直出纯净结构化 JSON 或 Markdown 附录：
pageclaw "https://store.example.com/item/1" --schema '{"fields":{"title":"h1","price":".price-val","link":"a@href","tags":"[.tag]"}}' -f json
# 支持 items_selector 列表循环模式，一次性抽取整页商品卡片或文章列表：
pageclaw "https://news.example.com/" --schema '{"items_selector":".post-item","fields":{"title":"h2","url":"a@href","author":".author"}}' -f json

# 28. 🔍 页面 DOM 结构骨架分析与 Schema 自动脚手架 (Scaffolding，超低 Token 探索未知页面)
# 自动探测目标页面内的重复列表容器 (如商品、文章、卡片) 并输出就绪的 JSON Schema 模版：
pageclaw "https://store.example.com/items" --scaffold
# 也可以对 stdin 管道传入的 HTML 进行结构脚手架分析：
cat ./page.html | pageclaw - --scaffold

# 29. ✂️ 语义感知分块 (Semantic Chunking，专供 RAG 向量检索与本地知识库构建)
# 基于标题层级 (#, ##, ###) 自适应切块，绝不切断代码块与 Markdown 表格，并记录标题面包屑路径：
pageclaw "https://example.com/long-guide" --chunk --chunk-size 500 --chunk-overlap 50
# 输出为结构化 JSON (包含 chunks 数组，各切块附带 heading_path、title、token_count)：
pageclaw "https://example.com/long-guide" --chunk -f json

# 30. 🌐 显式 HTTP/HTTPS 代理配置 (-p, --proxy)
# 支持为单页提取、全网搜索、批量流水线与整站爬虫指定外部代理服务：
pageclaw "https://example.com" -p http://127.0.0.1:7890
pageclaw search "Rust news" --proxy http://127.0.0.1:7890

# 31. 🧬 64 位 SimHash 内容指纹去重 (--dedup)
# 递归爬虫自动计算页面指纹，跳过近似度高（Hamming 距离 <= 3）的镜像或重复页面：
pageclaw crawl "https://example.com/docs" --dedup -o ./docs_dedup.md

# 32. 🖼️ 离线图片资产自动下载与 Markdown 相对路径回写 (--download-images)
# 下载 Markdown 中的全部远程图片到本地资产目录并自动将链接回写为本地相对路径：
pageclaw "https://example.com/guide" --download-images --assets-dir ./assets

# 33. 📊 独立表格数据精准抽取与多格式导出 (--tables-only, --table-format csv|tsv|markdown|json)
# 跳过全部正文与噪音，仅精准抽取网页中的所有数据表格，直接导出为 CSV 格式：
pageclaw "https://finance.example.com/earnings" --tables-only --table-format csv
# 导出为 TSV 制表符分隔文本或结构化 JSON 字典记录数组：
pageclaw "https://en.wikipedia.org/wiki/List_of_countries_by_GDP" -T --table-format tsv
pageclaw "https://example.com/spec-comparison" -T --table-format json

# 34. 📱 移动端排版与设备模拟模式 (--mobile)
# 模拟 iPhone / Safari 访问以触发轻量移动端响应式排版，大幅消除桌面端冗余侧边栏与广告：
pageclaw "https://example.com/article" --mobile
pageclaw search "技术资讯" --mobile --fetch-content
```

---

## 🤖 Model Context Protocol (MCP) 原生服务模式

PageClaw 实现了符合 **Anthropic MCP 2024-11-05 协议规范** 的原生 stdio JSON-RPC 2.0 服务端。
AI Agent（如 **Claude Desktop**、**Cursor**、**Windsurf** 等）可以直接将 PageClaw 挂载为原生系统工具，获得开箱即用的网页抓取与媒体下载能力。

### 1. 接入配置指南 (Configuration)

#### 在 Claude Desktop 中使用 (`claude_desktop_config.json`)
```json
{
  "mcpServers": {
    "pageclaw": {
      "command": "pageclaw",
      "args": ["mcp"]
    }
  }
}
```

#### 在 Cursor 中使用 (`.cursor/mcp.json`)
```json
{
  "mcpServers": {
    "pageclaw": {
      "command": "pageclaw",
      "args": ["mcp"]
    }
  }
}
```

### 2. 暴露给大模型的原生 MCP Tools 清单

| MCP 工具名 | 功能说明 | 核心入参 |
|---|---|---|
| **`fetch_webpage`** | 全网网页/B站视频智能抓取与清洗，输出 Markdown 或结构化 JSON，支持 Token 预算截断与缓存，全功能支持 CSS 选择器定向提取、链接/图片/表格结构化资产解析、CDP 浏览器渲染与 Schema 结构化实体提取、语义感知分块 (Chunking)、离线图片资产下载与链接回写、独立表格抽取 (CSV/TSV/JSON)、移动端设备模拟排版、代理配置，以及 B站 字幕/弹幕/评论/官方AI大纲提纲提取与 Cookie 鉴权 | `url` (必填), `format`, `mode`, `selector`, `schema`, `chunk`, `chunk_size`, `chunk_overlap`, `download_images`, `assets_dir`, `tables_only`, `table_format`, `mobile`, `cdp_endpoint`, `cdp_wait_ms`, `proxy`, `extract_links`, `extract_images`, `extract_tables`, `subtitles`, `danmaku`, `comments`, `conclusion`, `cookie`, `cookie_file`, `max_tokens`, `use_cache` |
| **`batch_fetch_webpages`** | **并发抓取多网页流水线**，使用异步协程并发池并发处理一组 URL 列表，一次性聚合返回清洗后的 Markdown 或 JSON 文档数组 | `urls` (必填字符串数组), `concurrency` (默认 5, 最大 20), `format` (markdown/json), `max_tokens_per_page`, `selector`, `schema`, `cdp_endpoint`, `cdp_wait_ms`, `proxy`, `mobile` |
| **`clean_html`** | **本地直接清洗已有 HTML 字符串**，剥离广告/脚本/导航噪音，转化为纯净 Markdown、纯文本或执行 Schema/CSS 选择器结构化抽取、独立表格抽取 (CSV/TSV/JSON) 与语义分块 | `html` (必填), `base_url`, `format` (markdown/json/text/html), `mode` (article/full/raw), `selector`, `schema`, `chunk`, `chunk_size`, `chunk_overlap`, `tables_only`, `table_format`, `max_tokens`, `strip_images`, `strip_links`, `frontmatter` |
| **`web_search`** | **免 API Key 的通用全网搜索**（支持 Bing、Google 与 Auto 容灾），支持生态定向标签（微信公众号、知乎、GitHub、掘金）与 `--fetch-content` 搜索即抓取一体化调研（可叠加 CDP 渲染、移动端排版、Schema 抽取与代理） | `query` (必填), `scope` (all/wechat/zhihu/github/juejin), `engine` (bing/google/auto, 默认 bing), `fetch_content` (可选布尔值), `max_tokens_per_page` (可选整数), `cdp_endpoint`, `schema`, `proxy`, `mobile`, `limit` (默认 5), `format` (markdown/json) |
| **`crawl_site`** | **整站文档递归爬取与知识库汇编**（从种子 URL 广度优先发现超链接或通过 sitemap.xml 解析规范地址），支持 CDP 无头渲染、Token 总预算上限截断、SimHash 内容指纹去重（`dedup`）、移动端排版与 Schema 抽取，自动生成带目录索引（TOC）的单文件集成 Markdown 知识库或结构化 JSON | `seed_url` (必填), `max_depth` (默认 2, 最大 5), `max_pages` (默认 20, 最大 100), `same_path_prefix` (可选布尔值), `sitemap` (可选布尔值), `dedup` (可选布尔值), `proxy`, `mobile`, `max_tokens`, `cdp_endpoint`, `schema`, `format` (markdown/json) |
| **`login_platform`** | **平台凭据扫码授权与登录辅助**，发起 B站/知乎 交互式登录提取会话 Cookie 并安全隔离写入凭据文件（带平台前缀，防多网站覆盖） | `platform` (bilibili/zhihu, 必填), `output_cookie_file` (默认 ./cookies.txt), `no_browser` (默认 true), `timeout_secs` (默认 180) |
| **`scaffold_schema`** | **DOM 结构骨架探测与 Schema 自动生成**，分析 URL 或 raw HTML，自动识别重复卡片容器与字段选择器，输出就绪的提取模板（极大降低 Agent 前期试错 Token） | `url`, `html`, `cdp_endpoint` |
| **`fetch_bilibili_feed`** | 毫秒级拉取 B站 首页推荐流、全站热榜或分区排行榜 GFM 表格 | `feed_type` (recommendations/popular/ranking), `partition`, `format` |
| **`search_bilibili`** | 按关键词搜索 B站 视频，输出结构化 GFM 表格或 JSON，支持分页与排序 | `keyword` (必填), `page`, `order`, `format`, `use_cache` |
| **`download_media`** | 下载 B站 视频高清封面图片或免登录渐进式 MP4 视频流至本地磁盘 | `url_or_bvid` (必填), `media_type` (video/cover), `quality`, `destination` |

---

## 📋 命令行参数详解 (CLI Reference)

```text
USAGE:
    pageclaw [OPTIONS] [TARGET] [COMMAND]

COMMANDS:
    mcp                        启动原生 Model Context Protocol (MCP) stdio 服务端
    login <PLATFORM>           唤起本地浏览器登录提取 Cookie 凭据并保存至文件 (支持 bilibili, zhihu)
    search <QUERY>             免 API Key 的全网通用搜索，直接输出 Markdown 编号列表或 JSON (支持 -l, -f, -o, -e/--engine, --fetch-content, --max-tokens-per-page, --wechat, --zhihu, --github, --juejin)
    batch <FILE>               并发抓取与批量 URL 提取流水线 (支持 -c, -f, -t, --selector, -o)
    crawl <URL>                整站文档递归爬取与知识库汇编 (支持 -d/--max-depth, -p/--max-pages, -c/--concurrency, --same-path-prefix, --sitemap, --dedup, -p/--proxy, -f, -o, -t)

ARGS:
    <TARGET>                   目标 URL、短 BV 号或 '-' (从标准输入读取 HTML)

OPTIONS:
        --search <KEYWORD>     按关键词搜索 B站 视频
        --page <PAGE>          搜索结果页码或多 P 视频分 P 编号 (1-indexed)
        --order <ORDER>        搜索排序规则: totalrank (默认), click (播放), pubdate (最新), dm (弹幕), stow (收藏)
        --mcp                  以原生 MCP 服务器模式运行
        --cache                启用响应二级缓存 (L1 内存 + L2 磁盘，二次请求 < 10ms)
        --no-cache             显式禁用响应缓存
        --cache-ttl <SECS>     缓存有效时长 (秒) [默认: 900 (15 分钟)]
        --cache-dir <DIR>      自定义磁盘缓存存储目录
    -f, --format <FORMAT>      输出格式: markdown (默认), json, text, html, raw
    -m, --mode <MODE>          提取模式: article (默认 Readability 正文), full (全量清洗), raw
    -t, --max-tokens <N>       限制最大输出 Token 数量 (段落边界自适应截断)
        --frontmatter          在 Markdown 顶部添加 YAML 元数据 Frontmatter
        --json-ld              仅提取并输出页面内的 JSON-LD 结构化数据
        --selector <CSS>       CSS 选择器定向提取 (DOM 阶段剥离无关节点，如 'article.content, #readme')
        --schema <JSON_OR_PATH> 结构化实体提取 Schema (支持内联 JSON 或 JSON 模式文件路径)
        --scaffold             分析页面 DOM 结构骨架并生成建议的 Schema 模版 (节省 Agent 探索 Token)
        --chunk                启用语义感知切块 (按标题层级切分，保护代码块与表格，供 RAG 向量入库)
        --chunk-size <N>       目标切块 Token 大小 [默认: 500]
        --chunk-overlap <N>    相邻切块间的 Token 重叠量 [默认: 50]
        --extract-links        提取页面全量去重超链接清单 (在 -f json 中为 links 数组，在 md 中为尾部清单)
        --extract-images       提取页面全量图片资产清单 (在 -f json 中为 images 数组，在 md 中为尾部清单)
        --extract-tables       深度解析页面中的 table 为二维数组与字典对象 records
    -T, --tables-only          仅提取并输出页面中的全部数据表格，跳过正文与噪音 (别名: --table-only)
        --table-format <FMT>   表格导出呈现格式: markdown (默认), csv, tsv, json
        --bili-sub             提取 B站 视频官方/AI 生成的 CC 字幕轨
        --bili-sum             提取 B站 官方 AI 视频总结核心摘要与章节大纲
        --bili-dm              提取 B站 视频时间轴高能弹幕 (按出现时间升序排序为 GFM 表格)
        --bili-dm-limit <N>    限制提取的时间轴弹幕最大条数 (如 --bili-dm-limit 50)
        --bili-cm              提取 B站 视频高赞精选热评 (包含用户名、点赞数、评论内容与发布时间)
        --bili-cm-limit <N>    限制提取的高赞热评最大条数 [默认: 20] (支持自动跨页获取，如 --bili-cm-limit 50)
        --bili-feed            拉取 B站 首页推荐视频流表格
        --bili-search <KEY>    按关键词搜索 B站 视频
        --bili-cover           仅输出 B站 视频高清封面图片 URL
        --bili-dl [<PATH>]     下载 B站 渐进式 MP4 视频流
        --bili-quality <QN>    B站 视频清晰度选择: 720p (默认), 480p, 360p
    -p, --proxy <PROXY_URL>    配置 HTTP/HTTPS 网络代理地址 (如 http://127.0.0.1:7890)
        --mobile               模拟移动端设备 (iPhone / Safari) 访问以触发轻量移动端排版
        --dedup                在整站爬取中启用 SimHash 内容去重 (自动跳过近似重复页面)
        --download-images      下载 Markdown 中的全部远程图片到本地资产目录并自动回写本地相对链接
        --assets-dir <DIR>     本地图片资产保存目录 [默认: ./assets]
        --cookie <COOKIE>      指定 Cookie 字符串凭据
        --cookie-file <PATH>   指定 Cookie 凭证文件路径 (支持多平台前缀格式，如 bilibili: ..., zhihu: ...)
        --no-images            移除所有图片以节省 Token
        --no-links             移除超链接 URL，仅保留锚点文本
        --no-metadata          跳过文档元数据 (OG/Twitter/Meta) 提取
        --no-adapter           禁用平台适配器，强制走原生 HTML 抓取
        --feed                 拉取平台推荐视频流 (如 B站首页推荐)
        --cover                仅输出视频高清封面图片 URL
        --download-cover [P]   下载视频封面图片到本地 (默认: {bvid}_cover.jpg)
        --download-video [P]   下载渐进式 MP4 视频流 (别名: --download，默认: {title}.mp4)
        --quality <QUALITY>    视频清晰度选择: 720p (默认), 480p, 360p
        --allow-private-addresses 允许访问内网私有 IP、localhost 与本地链路地址 (放行 SSRF)
        --allow-http-errors    在 HTTP 4xx 或 5xx 时不以非零退出码报错，直接返回响应体 (别名: --no-fail)
        --base-url <URL>       从 stdin 读取 HTML 时用于补全相对链接的基础 URL
        --retries <N>          网络故障或 5xx 错误时的指数退避重试次数 [默认: 0]
        --connect-timeout <S>  网络连接握手超时时间 (秒) [默认: 10]
        --on-too-large <MODE>  响应超出 max-size 时的处理行为: error (默认), truncate (优雅截断继续)
        --tokenizer <MODEL>    Token 计算模型口径: o200k (默认), cl100k, heuristic
    -u, --user-agent <UA>      自定义 User-Agent 请求头
    -H, --header <KEY:VALUE>   添加自定义 HTTP 请求头 (可多次指定)
        --timeout <SECS>       网络请求超时时间 (秒) [默认: 15]
        --max-size <MB>        单次下载最大限制 (MB) [默认: 10]
    -o, --output <FILE>        将处理结果保存至文件而非 stdout
    -v, --verbose              详细日志输出到 stderr (-v: INFO, -vv: DEBUG)
    -q, --quiet                静默模式，关闭 stderr 上的全部日志输出
    -h, --help                 查看帮助信息
    -V, --version              查看版本信息
    -V, --version              查看版本信息
```

---

## 🚦 进程退出码规范 (Exit Codes)

为了便于在 AI Agent 编排流水线与 Shell 脚本中准确捕获异常原因，PageClaw 实现了标准化的语义退出码体系：

| 退出码 | 含义 | 说明 |
|---|---|---|
| `0` | 成功 (Success) | 抓取、清洗或多媒体下载完成，或使用 `--allow-http-errors` 提取非 2xx 页面 |
| `2` | 命令行参数错误 | 传入非法参数、URL 语法有误 |
| `3` | 网络连接/超时/服务端错误 | 网络不通、DNS 无法解析、连接被拒绝、握手超时或 HTTP 5xx 错误 |
| `4` | 内容解析或客户端 HTTP 错误 | 字符集解码异常、HTML 严重破损或 HTTP 4xx 错误 (如 404/403) |
| `5` | 响应体超出限制 | 下载内容大小超出 `--max-size` |
| `6` | SSRF 安全拦截 | 目标 URL 指向回环、私网或云元数据地址被拦截 |
| `1` | 通用错误 | 文件 I/O 错误或其他未分类故障 |

---

## 📦 作为 Rust 库使用 (Library Usage)

在您的 Rust 项目 `Cargo.toml` 中引入 `pageclaw-core`：

```toml
[dependencies]
pageclaw-core = { path = "path/to/crates/pageclaw-core" }
tokio = { version = "1", features = ["full"] }
```

调用代码示例：

```rust
use pageclaw_core::{PageclawEngine, ExtractionOptions, OutputFormat, ExtractionMode};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = PageclawEngine::new()?;

    let options = ExtractionOptions {
        mode: ExtractionMode::Article,
        format: OutputFormat::Markdown,
        max_tokens: Some(3000),
        frontmatter: true,
        ..Default::default()
    };

    let doc = engine.process_url("https://httpbin.org/html", &options).await?;

    println!("Title: {:?}", doc.title);
    println!("Tokens: {}", doc.token_count);
    println!("Clean Markdown:\n{}", doc.content);

    Ok(())
}
```

---

## 🧪 自动化测试 (Testing)

```bash
# 运行全部单元测试与集成测试
cargo test --workspace

# 代码质量检查与严格静态分析
cargo clippy --workspace --all-targets -- -D warnings
```

---

## 🗺️ 后续路线图 (Roadmap)

- [x] 基于 Readability 的核心正文提取与 DOM 噪点过滤
- [x] GFM 格式 Markdown 高保真转换 (保留表格、代码高亮)
- [x] 零 Token 成本的 JSON-LD 与 OpenGraph 元数据抽取
- [x] BPE Token 计数与语法感知安全截断
- [x] 严格的 Stdout / Stderr 隔离与管道支持
- [ ] **Tier 2: 动态渲染扩展 (`pageclaw-mcp` / `chromiumoxide`)**：当目标页面为复杂 SPA 时，可选无缝唤起本地无头 Chrome 完成客户端 JS 渲染。
- [ ] **Model Context Protocol (MCP) 原生集成**：实现基于 `stdio` 的原生 MCP Server，作为 Claude Code 或 Cursor 的专用 Agent 工具直接加载。
