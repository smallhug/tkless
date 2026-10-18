<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="TkLess Logo">
</p>

<h1 align="center">TkLess</h1>

<p align="center">
  <strong>Spend less tokens, perceive more web.</strong><br>
  专为 AI Agent（Claude Code、Cursor、AutoGPT）与 LLM 管道量身定制的毫秒级网页提取、结构化降噪与搜索引擎。
</p>

---

## 🌟 核心特性 (Key Features)

- **⚡ 毫秒级极速冷启动**：纯 Rust 编写，静态单二进制无外部运行时依赖，启动延迟仅数毫秒，空闲内存占用低（< 15MB）。
- **📉 激进降噪与 Token 预算控制**：
  - 基于 Mozilla Readability 启发式算法，自动过滤导航栏、页眉页脚、弹窗、广告与追踪脚本，平均为 LLM 节省 **70%~90% 的上下文 Token**。
  - 支持 `--max-tokens <N>` 智能截断：感知段落与句子边界，并在截断时**自动补全并闭合 Markdown 代码块（```` ``` ````）**。
- **🔌 Agent 管道友好 (Strict Stream Separation)**：
  - `stdout`：严格纯净，仅输出清洗后的目标数据（Markdown、JSON、SQL、CSV、Text）。
  - `stderr`：输出耗时指标、HTTP 状态、Token 统计与调试日志，杜绝任何数据流污染。
- **🤖 MCP 两阶段动态工具发现 (Two-Stage Dynamic Tool Discovery)**：
  - 针对大模型预置 System Prompt 膨胀问题，独创两阶段 MCP 协议；
  - 初始仅暴露 2 核心工具 + 4 发现元工具，握手 Token 开销从 6,724 tokens 骤降至 **1,700 tokens（节省 74.7%）**；
  - 支持运行时按需探查参数 Schema、单次即时调用 (`execute_tool`) 以及 `notifications/tools/list_changed` 动态拉起入库。
- **🧩 16+ 原生平台深度适配器矩阵**：
  - 原生适配 B站、知乎、微信公众号、小红书、GitHub、Hugging Face、Stack Overflow、V2EX、Reddit、维基百科、百度百科、ArXiv、Medium、CSDN、Substack、掘金等；
  - 提供 `tkless adapters` 命令行子命令与 `list_adapters` MCP 工具，一键检索支持平台与专属优化能力。
- **📊 独立表格提取与 SQL 导出 (`tkless tables`)**：
  - 深度解析网页中的 `<table>` 元素，自动推导列名并去重，一键导出为标准 **SQL DDL/INSERT** 建表与数据写入脚本、CSV、TSV 或 JSON。
- **🔄 文档版本差异比对 (`tkless diff`)**：
  - 支持对任意两个网页 URL 或本地文本执行 Myers 行级比对，输出标准 Unified Diff 格式与行变更比例。
- **💻 交互式探索 REPL 命令行 (`tkless shell`)**：
  - 内置交互式终端 Shell，支持以命令流交互调试抓取 (`fetch`)、搜索 (`search`)、订阅源解析 (`feed`)、表格提取 (`tables`) 与文档切块 (`chunk`)。
- **💡 本地提取式摘要与文档拆分**：
  - `--summarize`：本地启发式提炼 3~5 条核心观点速览（TL;DR），完全无需外部大模型 API Key；
  - `--out-dir`：按 Markdown 标题层级自动将长篇文档拆分为独立多文件目录并生成 `index.md` 导航索引。
- **💎 零成本先验结构化提取**：
  - 原生提取 `<script type="application/ld+json">`（Schema.org）、OpenGraph、Twitter Card 与 Favicon 图标元数据，无需消耗任何 LLM Token 即可获取高置信度结构化数据。
- **🌐 智能网络与企业级防御**：
  - 基于 `reqwest` + `rustls`，内置 SSRF 全网段地址防御拦截，支持多平台前缀 Cookie 隔离管理（防覆盖）；
  - 支持 Chrome DevTools Protocol (CDP) 远程无头调试 Sidecar，遇 Cloudflare/JS 挑战自动降级与高清网页截屏。

---

## 📚 开发与设计文档 (Documentation)

项目深度设计与开发规范已归档至 `doc/` 目录：

- 📖 **[doc/README.md](doc/README.md)**：开发文档导航与快速指引
- 🚀 **[01-ci-cd-and-release.md](doc/01-ci-cd-and-release.md)**：发布流程与 CI/CD 自动化构建指南（GitHub Actions 多平台交叉编译、Release 产物直出 `.exe` / `.dmg` 规范）
- 🔌 **[02-mcp-tools-extension.md](doc/02-mcp-tools-extension.md)**：Model Context Protocol (MCP) 工具扩展架构设计（22 个专用原子工具、进度推流与宿主配置）
- 🛠️ **[03-adapters-and-features.md](doc/03-adapters-and-features.md)**：平台适配器矩阵与功能特性实战手册（16+ 平台参数速查、SQL 表格导出、RAG 分块与实战案例）
- 📐 **[04-development-standards.md](doc/04-development-standards.md)**：核心开发规范与工程标准指南（架构分层、两阶段动态工具发现规范、安全防御红线及 Git 规范）

---

## 🏛️ 项目架构 (Workspace Layout)

```text
tkless/
├── Cargo.toml                          # Workspace 根配置与依赖版本锁定
├── README.md                           # 项目说明文档
├── .cursor/                            # Cursor IDE 集成配置
│   └── mcp.json                        # MCP 服务直连配置模版
├── .github/
│   └── workflows/                      # GitHub Actions CI/CD 自动化工作流
│       ├── ci.yml                      # PR/Push 三端测试与 Lint 门禁
│       └── release.yml                 # Git Tag 多平台二进制构建 (.exe / .dmg / standalone)
├── doc/                                # 核心开发、架构与使用文档
│   ├── README.md                       # 文档索引与快速指引
│   ├── 01-ci-cd-and-release.md         # CI/CD 与多平台构建发布指南
│   ├── 02-mcp-tools-extension.md       # MCP 工具扩展与协议规范
│   ├── 03-adapters-and-features.md     # 适配器矩阵与全量功能使用手册
│   └── 04-development-standards.md     # 核心开发规范与两阶段动态发现标准
└── crates/
    ├── tkless-core/                    # 核心提取与清洗引擎库 (可独立引入)
    │   ├── src/
    │   │   ├── lib.rs                  # TklessEngine 外观模式接口
    │   │   ├── model.rs                # 领域模型 (PageDocument, Metadata, Options)
    │   │   ├── error.rs                # 结构化错误类型 (TklessError via thiserror)
    │   │   ├── fetcher.rs              # 异步流式 HTTP 获取、编码探测与 SSRF 守卫
    │   │   ├── cleaner.rs              # DOM 降噪与 Readability 正文提取
    │   │   ├── markdown.rs             # HTML 转高纯净 GFM Markdown 与代码标签归一化
    │   │   ├── extractor.rs            # 元数据、超链接、图片与数据表格深度提取
    │   │   ├── token.rs                # BPE Token 计算与语法感知截断 (tiktoken)
    │   │   ├── summarizer.rs           # 本地启发式提取式摘要引擎 (TL;DR)
    │   │   ├── splitter.rs             # Markdown 标题层级物理拆分与索引生成
    │   │   ├── chunker.rs              # 语义感知分块 (Semantic Chunking)
    │   │   ├── diff.rs                 # 文本版本比对与变更比例分析 (diff)
    │   │   ├── dedup.rs                # 64位 SimHash 内容指纹去重
    │   │   ├── feed.rs                 # RSS 2.0 / Atom 1.0 XML 订阅源解析
    │   │   ├── search.rs               # 免 Key 全网搜索与生态定向检索
    │   │   ├── crawler.rs              # 整站递归爬虫与单文件 Markdown 编排
    │   │   ├── adapters/               # 16+ 原生平台深度适配器矩阵
    │   │   └── mcp/                    # Model Context Protocol (MCP) 原生服务实现
    │   │       ├── server.rs           # MCP 服务端与两阶段动态调度派发器
    │   │       ├── tools.rs            # 22 工具注册表、轻量描述与分类目录
    │   │       └── types.rs            # JSON-RPC 2.0 协议对象模型
    │   └── tests/                      # 集成测试与协议全生命周期验证
    └── tkless-cli/                     # 终端 CLI 二进制 (tkless)
        └── src/
            ├── main.rs                 # 异步 Runtime、子命令路由与 Stdio 流隔离
            ├── cli.rs                  # Clap 命令行定义与结构体
            └── output.rs               # Stdout 输出渲染 (Markdown/JSON/SQL/CSV)
```

---

## 🚀 全局安装与可用性 (Global Installation & Usage)

`tkless` 是纯 Rust 编写的静态单二进制工具，无需 Node.js、Python 或任何外部运行时。将其安装为全局可用命令仅需一行：

### 1. 一键全局安装 (推荐)

在项目源码根目录下执行以下命令，即可将 `tkless` 自动编译并安装到全局 Cargo 目录（`~/.cargo/bin`，已默认位于系统 PATH 中）：

```bash
cargo install --path crates/tkless-cli --force
```

安装完成后，在**系统任意目录、任意终端窗口**中均可直接全局调用：

```bash
# 全局查看版本
tkless --version

# 全局查看帮助与 16+ 平台适配器矩阵
tkless -h
tkless adapters

# 全局抓取网页并提取为 Markdown
tkless https://httpbin.org/html
```

---

### 2. 下载预编译独立二进制放入口径 PATH

如果您在没有 Rust 开发环境的机器上部署，可直接从 [GitHub Releases](https://github.com/smallhug/tkless/releases/latest) 下载单文件直接使用（无需解压或安装运行时）：

| 操作系统 / 架构 | 发布产物名称 | 全局使用方式 |
| :--- | :--- | :--- |
| **Windows x86_64** | `tkless-windows-amd64.exe` | 下载并重命名为 `tkless.exe`，放置于任意系统 PATH 目录（如 `C:\Windows` 或自定义 PATH 文件夹） |
| **macOS Apple Silicon (M1~M4)** | `tkless-macos-arm64.dmg` | 打开 DMG 镜像，将 `tkless` 拖入 `/usr/local/bin` 或 `~/.cargo/bin` |
| **macOS Intel (x86_64)** | `tkless-macos-amd64.dmg` | 打开 DMG 镜像，将 `tkless` 拷贝至系统 PATH 路径 |
| **Linux x86_64** | `tkless-linux-amd64` | `sudo cp tkless-linux-amd64 /usr/local/bin/tkless && sudo chmod +x /usr/local/bin/tkless` |
| **Linux aarch64 (ARM64)** | `tkless-linux-arm64` | `sudo cp tkless-linux-arm64 /usr/local/bin/tkless && sudo chmod +x /usr/local/bin/tkless` |

---

### 3. 内置一键自升级命令 (`tkless upgrade`)

全局安装后，后续无需重新克隆或手动下载，随时直接在终端执行自更新：

```bash
# 自动检测 GitHub 最新版本并原地替换升级
tkless upgrade

# 或使用简短别名
tkless update

# 仅检查是否有更新 (不执行替换)
tkless upgrade --check
```

---

## 💻 常用命令行示例 (CLI Examples)

#### 基础网页抓取为 Markdown
```bash
tkless https://httpbin.org/html
```

#### 带 YAML 元数据前缀 (Title, URL, Published, Author)
```bash
tkless https://example.com/blog/article --frontmatter
```

#### 控制 Agent 上下文 Token 预算 (防止撑爆上下文)
```bash
tkless https://example.com/long-page --max-tokens 2000
```

#### 仅提取页面内置的 JSON-LD 结构化数据 (电商/新闻/知识库推荐)
```bash
tkless https://example.com/product/123 --json-ld
```

#### 完整结构化 JSON 输出 (供 Agent Function Calling 消费)
```bash
tkless https://example.com/article -f json | jq .metadata
```

#### 标准输入 (Stdin) 管道模式 (与其他抓取或渲染工具联动)
```bash
# 将本地 HTML 或其他工具抓取的 HTML 通过管道喂入
cat noisy_page.html | tkless - --format markdown

# Windows PowerShell 管道示例
Get-Content ./index.html -Raw | tkless - -f markdown
```

#### 进一步极限压缩 Token：剔除所有图片与链接 URL
```bash
tkless https://example.com/docs --no-images --no-links
```

#### ⚡ 主流平台原生适配器 (Platform Adapters)
针对现代复杂 SPA 网站（如 B站），自动拦截并调用原生接口，**在 150ms 左右直接输出 GFM Markdown 表格或结构化 JSON**，避免启动无头浏览器的巨额资源开销：

```bash
# 1. 抓取 B 站首页个性化推荐流 (150ms 极速获取视频推荐表格)
tkless https://www.bilibili.com/ --feed

# 2. 抓取 B 站全站热门排行榜 Top 视频
tkless https://www.bilibili.com/v/popular/rank/all

# 3. 抓取 B 站某一频道排行榜 (如科技区、动画区、生活区)
tkless https://www.bilibili.com/v/popular/rank/tech
tkless https://www.bilibili.com/v/popular/rank/anime

# 4. 抓取 B 站具体视频详情页 (包含标题、作者、数据统计、简介及 Schema.org 结构化数据)
tkless https://www.bilibili.com/video/BV1xx411c7mD
# 也支持直接传入短 BV 号:
tkless BV1xx411c7mD

# 5. 提取 B 站高清视频封面图片 URL (输出纯净 URL 到 stdout)
tkless BV1xx411c7mD --cover

# 6. 下载 B 站视频封面到本地文件 (默认保存为 {bvid}_cover.jpg)
tkless BV1xx411c7mD --download-cover
# 或指定保存路径:
tkless BV1xx411c7mD --download-cover ./covers/my_cover.jpg

# 7. 下载 B 站渐进式 MP4 视频流 (零 ffmpeg 依赖，单文件直接播放)
tkless BV1xx411c7mD --download-video
# 指定保存名称与清晰度 (720p / 480p / 360p):
tkless BV1xx411c7mD --download-video my_video.mp4 --quality 720p

# 8. 禁用平台适配器，强制回退为标准纯静态 HTML 爬取
tkless https://www.bilibili.com/video/BV1xx411c7mD --no-adapter

# 9. 启用响应二级缓存与 HTTP 304 零流量再验证 (L1 内存 + L2 磁盘，自动携带 ETag/Last-Modified 协议头)
# 二次请求命中有效缓存 < 1ms 即时响应；缓存过期时自动发起条件验证，服务器返回 304 Not Modified 时以 0 字节负载即时续期：
tkless https://httpbin.org/html --cache --cache-ttl 900

# 10. 抓取 GitHub 仓库纯净 README 文档 (丢弃 90%+ 导航、Star、Commit DOM 噪音)
tkless https://github.com/tokio-rs/tokio
# 抓取 GitHub Issue 对话讨论链 (提取标题、状态、首楼描述及后续回复列表)
tkless https://github.com/tokio-rs/tokio/issues/1234

# 11. 抓取掘金技术社区博文 (自动解包 link.juejin.cn 外链，保留纯净技术代码块)
tkless https://juejin.cn/post/7123456789012345678

# 12. 抓取小红书图文笔记 (自动解析 __INITIAL_STATE__ 服务端状态树、提取无水印图集与点赞收藏数)
tkless https://www.xiaohongshu.com/explore/654321000

# 13. 启动原生 Model Context Protocol (MCP) 服务器 (stdio 传输)
tkless mcp

# 11. B 站全网视频关键词搜索 (自动清洗高亮标签，输出结构化 Markdown 表格)
tkless --search "Rust编程语言"

# 12. 搜索翻页与排序 (按最多播放 click、最新发布 pubdate、最多弹幕 dm、最多收藏 stow)
tkless --search "Rust" --page 2 --order click

# 13. 直接传入 B 站网页端搜索链接
tkless "https://search.bilibili.com/all?keyword=Rust"

# 14. 🎯 方向三：CSS 选择器定向提取 (DOM 阶段直接剪除 95%+ 无关内容，Token 消耗压缩至几百)
tkless https://httpbin.org/html --selector "h1"
tkless https://example.com/article --selector "article.content, #readme, table"

# 15. 🎯 方向三：全量超链接与图片资产清单提取 (支持 JSON 与 Markdown 两种形态)
tkless https://example.com/page --extract-links -f json
tkless https://example.com/page --extract-images -f json

# 16. 🎯 方向三：表格深度结构化解析 (自动提取 2D 数组与按表头索引的记录字典)
tkless https://example.com/data --extract-tables -f json

# 17. 🎬 方向二：B 站官方/AI 字幕轨提取 (毫秒级提取标准时间轴 [01:25 - 01:30] 内容)
tkless BV1xx411c7mD --subtitles

# 18. 🎬 方向二：B 站时间轴高能弹幕抓取 (按时间升序排版 GFM 表格，支持 --danmaku-limit 控制数量)
tkless BV1xx411c7mD --danmaku
tkless BV1xx411c7mD --danmaku-limit 50

# 19. 🎬 方向二：B 站视频热门精选高赞评论抓取 (支持 --comments-limit 控制数量，默认 20 条，支持跨页翻倍)
tkless BV1xx411c7mD --comments
tkless BV1xx411c7mD --comments-limit 10

# 20. 🧠 官方 AI 总结提纲提取 (直接获取官方大模型核心摘要与带时间戳的章节提纲，省去重度总结 Token)
tkless BV1xx411c7mD --bili-sum

# 21. 🎬 方向二：多平台前缀 Cookie 凭据管理与双格式载入 (自动按网站前缀区分，避免同一文件中多网站信息覆盖)
# cookies.txt 支持多平台前缀共存：
#   bilibili: SESSDATA=xxx; bili_jct=yyy
#   zhihu: d_c0="xyz"; z_c0="abc"
#   wechat: wx_token=...
tkless BV1xx411c7mD --cookie-file ./cookies.txt
tkless "https://zhuanlan.zhihu.com/p/123456" --cookie-file ./cookies.txt

# 22. 🔑 一行命令唤起本地浏览器扫码登录并自动提取保存 Cookie 凭据 (支持 bilibili, zhihu)
# 自动以平台前缀合并写入 cookies.txt，安全保留已有其它网站凭据，绝不覆盖冲掉历史配置
tkless login bilibili
tkless login zhihu
# 或指定凭据保存路径与禁止自动拉起浏览器（纯终端字符画二维码，单行文字提示）：
tkless login bilibili -o ./my_bilibili_cookies.txt --no-browser

# 23. 🌐 免 Key 通用网页搜索与平台定向搜索 (并发多引擎 Bing + DuckDuckGo 智能去重聚合，直供 AI Agent 全流程搜索闭环)
# 默认启用双引擎并发聚合检索 (Bing + DuckDuckGo 交叉去重排名)
tkless search "Rust 异步编程最佳实践" --limit 5
tkless search "Tokio 教程" --limit 5 -f json
# 指定独立搜索引擎 (--engine all / bing / duckduckgo)
tkless search "Python asyncio" --engine duckduckgo --limit 3
tkless search "Linux kernel" --engine bing --limit 5
# 专享生态定向：一键限定抓取微信公众号、百度百科、知乎、GitHub、掘金平台
tkless search "开源项目" --wechat --limit 5
tkless search "光合作用" --baike --limit 3
tkless search "Rust 异步运行时" --zhihu --limit 5
tkless search "mini-redis" --github --limit 5
tkless search "Vue3 源码解析" --juejin --limit 5
# ⚡ 搜索即抓取一体化 (Search & Fetch All-in-One，单次交互并发抓取并清洗各页面正文，Token 预算严格可控)
tkless search "Rust 异步运行时" --fetch-content --limit 3 --max-tokens-per-page 1500
tkless search "Tokio runtime" --fetch-content --limit 2 -f json --max-tokens-per-page 300

# 24. ⚡ 批量并发抓取与多 URL 流水线 (通过多协程并发池并发处理，支持从文件或管道批量输入)
tkless batch ./urls.txt --concurrency 5 -o ./batch_results.md
cat ./urls.txt | tkless batch - --concurrency 10 -f json

# 25. 📚 整站文档递归爬取与知识库汇编 (BFS 遍历站点/同路径超链接，自动构建带 TOC 目录索引的单文件 Markdown 知识库)
# 从入口 URL 深度递归爬取整套技术文档（限制深度 2，最多 20 页，并发 5，保存为集成知识库）：
tkless crawl "https://tokio.rs/tokio/tutorial" --max-depth 2 --max-pages 20 -o ./tokio_docs.md
# 仅爬取相同路径前缀的子文档（例如仅抓取 /docs/ 路径下的页面）：
tkless crawl "https://example.com/docs" --same-path-prefix -f json -o ./docs_kb.json
# ⚡ Sitemap 站点地图驱动提速模式：直接解析 sitemap.xml 提取全量规范文档 URL，提速 10 倍：
tkless crawl "https://docs.rs/tokio/sitemap.xml" --sitemap --max-pages 50 -o ./tokio_sitemap.md

# 26. 🌐 Chrome DevTools Protocol (CDP) 无头浏览器渲染 Sidecar (针对重度 SPA 与反爬挑战绕过)
# 连接本地运行的 Chrome/Edge 远程调试端口 (如 google-chrome --remote-debugging-port=9222)
# 自动在浏览器中执行 JavaScript、等待 DOM 水合渲染，并安全提取完整的 outerHTML：
tkless "https://zhuanlan.zhihu.com/p/123456" --cdp http://127.0.0.1:9222 --cdp-wait 1000
# 与搜索即抓取、整站爬虫协同使用：
tkless search "Rust 教程" --fetch-content --cdp http://127.0.0.1:9222
# ⚡ CDP 性能飞跃与动态懒加载扩展 (Direction 3)：
# - 默认启用网络层静态重资产与跟踪器拦截 (自动阻断图片、字体、音视频、分析脚本)，提速 3~5 倍
# - 支持 --cdp-auto-scroll 自动平滑滚动加载动态无限滚动页面 (如推特/微博/小红书)，完成后自动提取：
tkless "https://example.com/dynamic-feed" --cdp http://127.0.0.1:9222 --cdp-auto-scroll
# 若需要完整视觉资产加载，可通过 --cdp-no-block-resources 禁用拦截：
tkless "https://example.com/gallery" --cdp http://127.0.0.1:9222 --cdp-no-block-resources

# 27. 🔬 多跳自主深度网络调研流水线 (Autonomous Deep Research Loop，Direction 4)
# 自动化研报闭环：自动拆解维度 -> 跨引擎并发检索 (Hop-1) -> 权威正文提取 -> 引文链接跟踪追踪 (Hop-2) -> 汇编出版级 Markdown 报告
tkless research "Polars 与 Pandas 2.0 架构原理与内存性能对比" --depth 2 --breadth 3 --max-pages 8
# 导出为结构化 JSON 供下游 LLM 或工作流使用，或保存为完整长篇综述报告：
tkless research "Rust Web 框架技术选型 Axum vs Actix-web" -f markdown -o ./rust_web_research.md
tkless research "大模型上下文压缩与长文本推理优化" --max-pages 10 --max-tokens 10000 -f json

# 27. 🎯 结构化实体提取 Schema (Schema-Driven Entity Extraction，极简声明式字段抽取与 Token 极致压缩)
# 通过极简平铺 JSON 直接从 DOM 抽取指定字段与属性，直出纯净结构化 JSON 或 Markdown 附录：
tkless "https://store.example.com/item/1" --schema '{"title":"h1","price":".price-val | number","in_stock":".stock | bool","link":"a@href"}' -f json
# 支持 items_selector 列表循环模式，一次性抽取整页商品卡片或文章列表（亦完美兼容 --scaffold 自动生成的模板直接回灌）：
tkless "https://news.example.com/" --schema '{"items_selector":".post-item","title":"h2","url":"a@href","author":".author"}' -f json

# 28. 🔍 页面 DOM 结构骨架分析与 Schema 自动脚手架 (Scaffolding，超低 Token 探索未知页面)
# 自动探测目标页面内的重复列表容器 (如商品、文章、卡片) 并输出就绪的 JSON Schema 模版：
tkless "https://store.example.com/items" --scaffold
# 也可以对 stdin 管道传入的 HTML 进行结构脚手架分析：
cat ./page.html | tkless - --scaffold

# 29. ✂️ 语义感知分块 (Semantic Chunking，专供 RAG 向量检索与本地知识库构建)
# 基于标题层级 (#, ##, ###) 自适应切块，绝不切断代码块与 Markdown 表格，并记录标题面包屑路径：
tkless "https://example.com/long-guide" --chunk --chunk-size 500 --chunk-overlap 50
# 输出为结构化 JSON (包含 chunks 数组，各切块附带 heading_path、title、token_count)：
tkless "https://example.com/long-guide" --chunk -f json

# 30. 🌐 显式 HTTP/HTTPS 代理配置 (-p, --proxy)
# 支持为单页提取、全网搜索、批量流水线与整站爬虫指定外部代理服务：
tkless "https://example.com" -p http://127.0.0.1:7890
tkless search "Rust news" --proxy http://127.0.0.1:7890

# 31. 🧬 64 位 SimHash 内容指纹去重 (--dedup)
# 递归爬虫自动计算页面指纹，跳过近似度高（Hamming 距离 <= 3）的镜像或重复页面：
tkless crawl "https://example.com/docs" --dedup -o ./docs_dedup.md

# 32. 🖼️ 离线图片资产自动下载与 Markdown 相对路径回写 (--download-images)
# 下载 Markdown 中的全部远程图片到本地资产目录并自动将链接回写为本地相对路径：
tkless "https://example.com/guide" --download-images --assets-dir ./assets

# 33. 📊 独立表格数据精准抽取与多格式导出 (tkless tables / --tables-only)
# 跳过全部正文与噪音，仅精准抽取网页中的所有数据表格，直接导出为标准 SQL DDL 与 INSERT 写入语句：
tkless tables "https://example.com/financials" --table-format sql --table-name quarterly_report
# 导出为 CSV / TSV 文件或结构化 JSON 字典记录数组：
tkless tables "https://finance.example.com/earnings" --table-format csv
tkless "https://en.wikipedia.org/wiki/List_of_countries_by_GDP" -T --table-format tsv
tkless "https://example.com/spec-comparison" -T --table-format json

# 34. 🔄 网页与文档版本差异比对 (tkless diff)
# 对比两个网页 URL 或本地文件，输出标准统一 Diff 格式、新增/删除行数与变更比例：
tkless diff "https://example.com/v1" "https://example.com/v2"
tkless diff ./old_docs.md ./new_docs.md -f json

# 35. 🧭 查看内置 16+ 平台原生适配器矩阵 (tkless adapters)
# 以 GFM Markdown 表格或 JSON 输出支持的平台域名匹配规则、专属优化能力与状态：
tkless adapters
tkless adapters -f json

# 36. 💻 启动交互式探索 REPL 命令行 (tkless shell)
# 启动交互式会话，支持连续交互执行 fetch/search/feed/tables/diff/chunk 等指令：
tkless shell

# 37. 💡 本地启发式核心观点速览提取 (--summarize)
# 本地启发式提取 3~5 条核心观点 (TL;DR)，完全无需配置或消耗外部大模型 API Key：
tkless "https://example.com/long-article" --summarize

# 38. 📁 按标题层级物理拆分长文档为多文件目录 (--out-dir)
# 自动将长篇 Markdown 按标题拆分为多文件结构，并在根目录生成带 TOC 导航的 index.md：
tkless "https://example.com/handbook" --out-dir ./handbook_docs/

# 39. 📱 移动端排版与设备模拟模式 (--mobile)
# 模拟 iPhone / Safari 访问以触发轻量移动端响应式排版，大幅消除桌面端冗余侧边栏与广告：
tkless "https://example.com/article" --mobile
tkless search "技术资讯" --mobile --fetch-content

# 40. 📈 Token 经济性与管道耗时基准遥测报告 (--stats)
# 输出完整的管道耗时（获取、清洗、转 Markdown、分词）与实测 Token 节省率、降噪比指标：
tkless "https://example.com/long-article" --stats

# 41. 🩺 运行环境与核心服务自检诊断 (tkless doctor)
# 自检系统 Chrome/Edge 无头浏览器路径、本地缓存目录读写权限、核心网络服务 (Bing, DuckDuckGo, Bilibili) 连通性：
tkless doctor
# 也可搭配代理参数测试代理网络环境：
tkless doctor -p http://127.0.0.1:7890

# 42. 📄 原生 PDF 文档智能解析与 Markdown 重排 (Native PDF to LLM Markdown)
# 纯 Rust 原生零外部 C 库依赖，支持远程 URL、本地 .pdf 文件与 stdin 输入流自动识别与排版提纯：
tkless "https://arxiv.org/pdf/1706.03762.pdf" -o ./transformer_paper.md
tkless ./research_paper.pdf --chunk --chunk-size 500
cat ./annual_report.pdf | tkless - --summarize
```

---

## 🤖 Model Context Protocol (MCP) 原生服务模式

TkLess 实现了符合 **Anthropic MCP 2024-11-05 协议规范** 的原生 stdio JSON-RPC 2.0 服务端。
AI Agent（如 **Claude Desktop**、**Cursor**、**Windsurf** 等）可以直接将 TkLess 挂载为原生系统工具，获得开箱即用的网页抓取、全网搜索、表格导出、文档切块与音视频媒体能力。

### 1. 🌟 两阶段动态工具发现架构 (Two-Stage Dynamic Tool Discovery)

传统 MCP 客户端在建立握手时会通过 `tools/list` 将所有工具的 JSON Schema 一次性全部灌入 System Prompt 中。当工具数达到 22 个时，**每轮对话尚未开始就会持续吃掉 6,700 多 Token 的常驻空间**，导致大模型上下文膨胀与首字延迟上升。

为此，TkLess 创新设计了**两阶段动态工具发现架构**（通过 `--dynamic` 或 `--profile dynamic` 或环境变量 `TKLESS_MCP_DYNAMIC=1` 启用）：

- **第一阶段（极简常驻）**：`tools/list` 仅暴露 2 个基础核心工具（`fetch_webpage`、`web_search`）+ 4 个发现元工具（`discover_tools`、`get_tool_schema`、`execute_tool`、`activate_tools`），常驻开销骤降至 **1,700 tokens（净省 74.7%）**！
- **第二阶段（按需探查与执行）**：
  - **检索专门工具**：Agent 遇到特定任务（如表格转 SQL）时，先调用 `discover_tools(query="table")` 检索可用工具；
  - **按需索取 Schema**：调用 `get_tool_schema(tool_name="export_tables")` 查阅参数定义；
  - **单次即时执行**：调用 `execute_tool(tool_name="export_tables", arguments={...})` 即时运行，无需写入当前工具列表；
  - **动态入库扩容**：调用 `activate_tools(tool_names=["export_tables"])` 动态激活工具，并通过 `notifications/tools/list_changed` 协议通知宿主自动刷新扩容。

#### 实测 Token 消耗与节省基准对比 (`o200k_base` 分词器)

| 运行模式 / 阶段 | 暴露工具数 | 声明字符数 | OpenAI o200k (GPT-4o) | 相较原版节省比例 |
| :--- | :---: | :---: | :---: | :---: |
| **全量原始模式 (Baseline)** | 22 个 | 30,128 (Compact) | 6,724 tokens | 基准 (0%) |
| **轻量化全量模式 (Lightweight)** | 22 个 | 19,823 (Compact) | 4,368 tokens | **-35.0%** (净降 2,356) |
| **两阶段动态发现模式 (Dynamic)** | **6 个** | **7,711 (Compact)** | **1,700 tokens** | **-74.7% (净降 5,024)** |

---

### 2. 接入配置指南 (Configuration)

#### 在 Claude Desktop 中使用 (`claude_desktop_config.json`)

**推荐：启用两阶段动态工具发现模式（节省 >74% Token）**：
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

若需全量暴露 22 个工具，使用标准模式：
```json
{
  "mcpServers": {
    "tkless": {
      "command": "tkless",
      "args": ["mcp"]
    }
  }
}
```

#### 在 Cursor 中使用 (`.cursor/mcp.json`)

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

---

### 3. 全量 MCP 工具分类目录 (23 工具矩阵)

#### 🔍 动态发现元工具 (Dynamic Discovery Meta-Tools，动态模式 Stage 1 常驻)
| 工具名 | 功能概述 | 核心参数 |
|---|---|---|
| **`discover_tools`** | 按关键词或分类检索专门的 TkLess 工具目录与简介 | `query` (可选关键词), `category` (content/search/media/analysis/system/all) |
| **`get_tool_schema`** | 按需索取指定专门工具的完整 JSON inputSchema 与参数描述 | `tool_name` (必填) |
| **`execute_tool`** | 单次无缝即时执行任何专门工具（无需预先在 `tools/list` 声明） | `tool_name` (必填), `arguments` (匹配该工具 schema 的入参对象) |
| **`activate_tools`** | 动态将工具激活入库，并向客户端广播 `notifications/tools/list_changed` | `tool_names` (工具名数组) |

#### 📄 内容抓取与知识汇编 (Content)
| 工具名 | 功能概述 | 核心参数 |
|---|---|---|
| **`fetch_webpage`** *(Core)* | 全网网页/PDF论文抓取与清洗为纯净 Markdown/JSON，支持平台适配器、CSS 定向、Token 预算、CDP 浏览器渲染与表格独立提取 | `url` (必填), `format`, `mode`, `selector`, `schema`, `chunk`, `tables_only`, `table_format`, `mobile`, `stats`, `proxy` |
| **`batch_fetch_webpages`** | 多协程并发抓取多网页流水线，聚合返回清洗后的 Markdown 或 JSON 文档数组 | `urls` (必填), `concurrency` (默认 5), `format`, `max_tokens_per_page`, `selector`, `schema` |
| **`crawl_site`** | 整站递归爬取与知识库汇编，支持 Sitemap、深度遍历、SimHash 去重与 TOC 目录构建 | `seed_url` (必填), `max_depth`, `max_pages`, `sitemap`, `dedup`, `same_path_prefix`, `max_tokens` |
| **`clean_html`** | 本地直接清洗已有 HTML 字符串，剥离广告脚本噪点，执行 Schema 实体提取或转为 Markdown | `html` (必填), `base_url`, `format`, `mode`, `selector`, `schema`, `chunk`, `tables_only` |
| **`fetch_feed`** | 解析 RSS 2.0 或 Atom 1.0 XML 订阅源并输出 Markdown 列表或 JSON 结构 | `url`, `xml`, `format`, `limit` |
| **`scaffold_schema`** | 分析页面 DOM 结构骨架，自动探测重复列表容器并生成提取 Schema 模版 | `url`, `html`, `cdp_endpoint` |

#### 🌐 定向检索与生态发现 (Search)
| 工具名 | 功能概述 | 核心参数 |
|---|---|---|
| **`web_search`** *(Core)* | 免 API Key 的通用全网搜索（支持 Bing + DuckDuckGo 双引擎并发去重聚合，支持生态定向与 `--fetch-content` 一体化） | `query` (必填), `scope`, `engine` (all/bing/duckduckgo), `fetch_content`, `max_tokens_per_page`, `limit` |
| **`deep_research`** | 多跳自主深度网络调研流水线（自动拆解维度、跨引擎并发检索、跟踪外部权威引链、汇编出版级综述报告） | `topic` (必填), `depth` (默认 2), `breadth` (默认 3), `max_pages` (默认 8), `max_tokens`, `engine`, `format` |
| **`search_bilibili`** | 搜索 B站 视频，输出结构化 GFM 表格或 JSON，支持分页与排序（播放/最新/弹幕/收藏） | `keyword` (必填), `page`, `order`, `format` |
| **`inspect_links`** | 批量探测超链接可达性、HTTP 状态码、延迟与重定向，具备内网 SSRF 拦截防御 | `urls` (必填数组), `concurrency`, `timeout_secs`, `format` |
| **`list_adapters`** | 列出当前支持的全部 16+ 平台原生适配器、匹配域名与专属优化能力 | `format` (markdown/json) |

#### 🎬 富媒体与音视频解析 (Media)
| 工具名 | 功能概述 | 核心参数 |
|---|---|---|
| **`fetch_bilibili_feed`** | 毫秒级拉取 B站 首页推荐流、全站热榜或分区排行榜 GFM 表格 | `feed_type` (recommendations/popular/ranking), `partition`, `format` |
| **`download_media`** | 下载 B站 视频高清封面图片或免登录渐进式 MP4 视频流至本地磁盘 | `url_or_bvid` (必填), `media_type` (video/cover), `quality`, `destination` |
| **`take_screenshot`** | 通过 Chrome DevTools Protocol (CDP) 对动态渲染网页截取高清快照图片 (PNG/JPEG) | `url` (必填), `output_path`, `format`, `full_page`, `cdp_endpoint` |

#### 📊 结构化与语义分析 (Analysis)
| 工具名 | 功能概述 | 核心参数 |
|---|---|---|
| **`export_tables`** | 深度解析网页中的 `<table>` 数据表格，导出为标准 SQL DDL/INSERT、CSV、TSV 或 JSON | `url`, `html`, `format` (sql/csv/tsv/json/markdown), `table_name`, `table_index` |
| **`summarize_content`** | 本地启发式提取 3~5 条核心观点速览 (TL;DR)，完全无需外部 LLM API Key | `url`, `text`, `max_bullets` (默认 3), `format` |
| **`split_document`** | 按照 Markdown 标题层级结构将长文档拆分为树状章节段落数组 | `markdown` (必填), `max_heading_level` (默认 2) |
| **`chunk_markdown`** | 语义感知分块 (Semantic Chunking)，保护表格与代码块完整性，专供 RAG 向量入库 | `url`, `markdown`, `chunk_size` (默认 500), `chunk_overlap` (默认 50), `tokenizer` |
| **`diff_documents`** | 对比两个网页或文本版本的变更差异，输出统一 Diff 格式与行级变更比例 | `old_text` (必填), `new_text` (必填), `format` (markdown/json) |
| **`compare_similarity`** | 计算两段文本的 64 位 SimHash 内容指纹与 Hamming 汉明距离（用于查重去重） | `text_a` (必填), `text_b` (必填) |

#### 🔑 系统与凭据维护 (System)
| 工具名 | 功能概述 | 核心参数 |
|---|---|---|
| **`manage_cache`** | 查阅本地响应缓存统计（内存/磁盘条目、字节数）或清空过期缓存 | `action` (stats/clear/prune_expired), `cache_dir` |
| **`login_platform`** | 唤起本地浏览器登录提取会话 Cookie 并安全隔离写入凭据文件（带平台前缀防覆盖） | `platform` (bilibili/zhihu), `output_cookie_file`, `no_browser`, `timeout_secs` |
| **`check_auth`** | 校验平台 Cookie 凭据有效性与登录态（安全无风控校验） | `platform` (必填), `cookie_file`, `cookie` |

---

## 📋 命令行参数详解 (CLI Reference)

```text
TkLess: Spend less tokens, perceive more web.

Usage: tkless [OPTIONS] [TARGET] [COMMAND]

Commands:
  mcp          启动原生 Model Context Protocol (MCP) 服务端 (供 Claude Desktop / Cursor 直连)
  login        交互式登录并提取保存平台凭据 (自动带平台前缀隔离存储至 cookies.txt，多网站互不覆盖)
  search       免 API Key 全网搜索与生态定向检索 (支持 Bing 及一键 --fetch-content 搜索即抓取)
  batch        并发抓取与批量 URL 提取流水线 (多协程并发池高效处理文件或 stdin 列表)
  crawl        整站文档递归爬取与知识库汇编 (自动构建带 TOC 目录索引的单文件 Markdown 知识库)
  cache        本地响应缓存查看与清理维护 (查看磁盘占用/条目统计，或清空缓存)
  shell        启动交互式 REPL 会话命令行 (支持 fetch/search/cache/links/tables/diff 快速交互)
  diff         对比两个文本文件或网页版本，输出统一 Diff 格式与行级变更比例
  tables       提取网页或 HTML 中的全部数据表格，并直接导出为 SQL DDL/INSERT、CSV、TSV 或 JSON
  completions  生成 Shell (Bash, Zsh, Fish, PowerShell, Elvish) 终端自动补全脚本
  adapters     列出支持的全部 16+ 平台原生适配器、匹配域名与专属优化能力 (别名: platforms)
  upgrade      检查并自动升级 TkLess 二进制至 GitHub 最新版本 (别名: update, 类似 bun upgrade)
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help (see more with '--help')
  -V, --version  Print version

通用提取选项 (General Options):
  -f, --format <FORMAT>                 Output presentation format [default: markdown] [values: markdown, json, text, html, raw, jsonl]
  -m, --mode <MODE>                     Content extraction mode [default: article] [values: article, full, raw]
  -t, --max-tokens <N>                  Maximum token budget (safely truncated at paragraph/sentence boundaries)
      --frontmatter                     Prepend YAML frontmatter with title, URL, date, and metadata (Markdown format only)
      --json-ld                         Extract and output only JSON-LD structured data blocks
      --selector <CSS_SELECTOR>         CSS selector for targeted DOM extraction (e.g. 'article.content, #readme')
  -R, --remove-selector <CSS_SELECTOR>  CSS 排除选择器: 剔除指定匹配的 DOM 元素容器 (例如 '.ads, .footer, #comments')
      --extract-links                   Extract and structure all hyperlinks from the document
      --extract-images                  Extract and structure all image assets from the document
      --extract-tables                  Extract and structure all tables into 2D arrays and row dictionary objects
      --no-images                       Strip images from Markdown to conserve token budget
      --no-links                        Strip link URLs from Markdown, keeping only the anchor text
      --no-metadata                     Do not extract document metadata (OpenGraph, Twitter Cards, description)
      --no-adapter                      Disable platform-specific fast API adapters and force standard HTML scraping
      --schema <JSON_OR_PATH>           结构化实体提取 Schema (支持内联 JSON 或 JSON 模式文件路径，直接抽取所需字段)
      --scaffold                        分析目标页面 DOM 结构骨架，自动探测重复列表容器并生成建议的 Schema 模版
      --chunk                           启用语义感知切块 (Semantic Chunking，按标题层级切分，保护代码块与表格，供 RAG 向量入库)
      --chunk-size <N>                  目标切块 Token 大小 [默认: 500]
      --chunk-overlap <N>               相邻切块间的 Token 重叠量 [默认: 50]
      --download-images                 下载 Markdown 中的全部远程图片到本地资产目录并自动回写本地相对链接
      --assets-dir <DIR>                本地图片资产保存目录 [默认: ./assets]
  -T, --tables-only                     仅提取并输出页面中的全部数据表格，跳过正文与噪音
      --table-format <TABLE_FORMAT>     表格导出呈现格式: markdown (默认), csv, tsv, json, sql [default: markdown]
      --stream                          启用终端流式按行输出 (实时 flush 降低首字延迟)
      --summarize                       启用本地启发式提取式摘要 (生成 3-5 条核心观点速览，无需外部大模型 API Key)
      --out-dir <DIR>                   将提取的 Markdown 按标题层级自动拆分为多文件目录并生成 index.md 导航索引
  [TARGET]                              Target webpage URL or '-' to read HTML from standard input (stdin)

网络与身份认证 (Network & Auth):
  -c, --cookie <COOKIE>              Cookie header string
      --cookie-file <PATH>           Path to cookie credential file (supports multi-platform prefix format)
  -u, --user-agent <UA>              Custom User-Agent header
  -H, --header <KEY:VALUE>           Custom HTTP request headers in "Key: Value" format
      --timeout <SECS>               Network request timeout in seconds [default: 15]
      --connect-timeout <SECS>       Network connection handshake timeout in seconds [default: 10]
      --retries <N>                  Number of retry attempts with exponential backoff on 5xx or network drops [default: 0]
      --max-size <MB>                Maximum response download size in megabytes [default: 10]
      --on-too-large <ON_TOO_LARGE>  Behavior when response size exceeds max-size: 'error' (default) or 'truncate' [default: error]
      --allow-private-addresses      Allow fetching private IP addresses, localhost, and internal endpoints (bypasses SSRF guard)
      --cdp <ENDPOINT>               Chrome DevTools Protocol (CDP) 远程调试端点 (如 http://127.0.0.1:9222)
      --cdp-wait <MS>                CDP 页面加载后额外等待 DOM 动态渲染完成的时间 (毫秒) [默认: 1000]
      --auto-cdp                     自动探测 Cloudflare / 验证码等反爬挑战，受阻时自动降级到 CDP 无头浏览器渲染
      --screenshot <PATH>            通过 CDP 无头浏览器截取渲染后的页面高清快照并保存为 PNG/JPEG 图像
      --allow-http-errors            Do not fail with non-zero exit code on HTTP 4xx or 5xx error responses
  -p, --proxy <PROXY_URL>            HTTP/HTTPS 代理服务器地址 (例如 http://127.0.0.1:7890)
      --mobile                       模拟移动端设备 (iPhone / Safari) 访问以触发轻量移动端排版，消除冗余噪音
      --base-url <URL>               Base URL to resolve relative links when reading HTML from stdin

缓存与运行诊断 (Cache & Diagnostics):
      --mcp                    Run as native Model Context Protocol (MCP) server over stdio
      --mcp-dynamic            启用 MCP 两阶段动态工具发现模式 (极简常驻上下文，节省 >74% 初始提示词 Token)
      --cache                  Enable local response caching
      --no-cache               Explicitly disable local response caching
      --cache-ttl <SECS>       Cache TTL in seconds (default: 900 = 15 minutes) [default: 900]
      --cache-dir <DIR>        Custom directory for persistent file cache
      --tokenizer <TOKENIZER>  Tokenizer model encoding scheme for token counting and truncation [default: o200k]
  -o, --output <FILE>          Write output payload to file instead of stdout
  -v, --verbose...             Verbose diagnostic output on stderr (-v for debug, -vv for trace)
  -q, --quiet                  Silence all diagnostic logs and progress messages on stderr
      --stats                  输出详细的 Token 经济性与管道耗时基准遥测报告 (显示 Token 节省比例与降噪率)
```

---

## 🚦 进程退出码规范 (Exit Codes)

为了便于在 AI Agent 编排流水线与 Shell 脚本中准确捕获异常原因，TkLess 实现了标准化的语义退出码体系：

| 退出码 | 含义 | 说明 |
|---|---|---|
| `0` | 成功 (Success) | 抓取、清洗、搜索或多媒体下载完成，或使用 `--allow-http-errors` 提取非 2xx 页面 |
| `2` | 命令行参数错误 | 传入非法参数、URL 语法有误 |
| `3` | 网络连接/超时/服务端错误 | 网络不通、DNS 无法解析、连接被拒绝、握手超时或 HTTP 5xx 错误 |
| `4` | 内容解析或客户端 HTTP 错误 | 字符集解码异常、HTML 严重破损或 HTTP 4xx 错误 (如 404/403) |
| `5` | 响应体超出限制 | 下载内容大小超出 `--max-size` |
| `6` | SSRF 安全拦截 | 目标 URL 指向回环、私网或云元数据地址被拦截 |
| `1` | 通用错误 | 文件 I/O 错误或其他未分类故障 |

---

## 📦 作为 Rust 库使用 (Library Usage)

在您的 Rust 项目 `Cargo.toml` 中引入 `tkless-core`：

```toml
[dependencies]
tkless-core = { path = "path/to/crates/tkless-core" }
tokio = { version = "1", features = ["full"] }
```

调用代码示例：

```rust
use tkless_core::{TklessEngine, ExtractionOptions, OutputFormat, ExtractionMode};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = TklessEngine::new()?;

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
# 运行全部单元测试与集成测试 (146 项测试全量通过)
cargo test --workspace

# 代码规范格式化检查
cargo fmt --all -- --check

# 严苛代码质量与静态分析 (0 告警通过)
cargo clippy --workspace --all-targets -- -D warnings
```

---

## 🗺️ 后续路线图 (Roadmap)

- [x] 基于 Readability 的核心正文提取与 DOM 噪点过滤
- [x] GFM 格式 Markdown 高保真转换 (保留表格、代码高亮归一化)
- [x] 零 Token 成本的 JSON-LD 与 OpenGraph 元数据抽取
- [x] BPE Token 计数与语法感知安全截断
- [x] 严格的 Stdout / Stderr 隔离与管道支持
- [x] **Chrome DevTools Protocol (CDP) 远程无头调试与快照截屏** (`--cdp`, `--screenshot`)
- [x] **Model Context Protocol (MCP) 原生集成** (符合 2024-11-05 协议，支持 stdio 通信与进度推流)
- [x] **MCP 两阶段动态工具发现架构** (`--dynamic`, `--profile dynamic`，节省 >74% 预置 Prompt Token)
- [x] **16+ 原生平台深度适配器矩阵** (B站、知乎、微信公众号、小红书、GitHub、Hugging Face、ArXiv 等)
- [x] **独立数据表格深度抽取与 SQL DDL/INSERT / CSV / TSV 导出** (`tkless tables`)
- [x] **网页与文档版本差异比对** (`tkless diff`)
- [x] **交互式 REPL 会话探索模式** (`tkless shell`)
- [x] **本地启发式提取式摘要与核心观点速览** (`--summarize`)
- [x] **按 Markdown 标题层级物理拆分长文档** (`--out-dir`)
- [x] **免 API Key 通用网页搜索与平台生态定向检索** (Bing 引擎)
- [x] **整站递归爬虫与单文件 Markdown 知识库自动编排** (`tkless crawl`)
- [x] **自动化 GitHub Actions 多平台二进制直接发版** (`.exe` / `.dmg` / standalone binary)
