# 03 - 平台适配器矩阵与功能特性实战手册

本文档是 TkLess 的全量功能特性、22 个平台适配器与 CLI 命令行使用手册，旨在为开发者与 AI Agent 管道集成提供完整的参考标准。
> **Spend less tokens, perceive more web.**

---

## 1. 平台专属适配器矩阵 (22 Platforms)

TkLess 内置自动化平台识别调度器，当输入的 URL 命中受支持的主流平台时，会自动激活专属的数据提取与降噪策略：

| 平台名称 | 匹配域名规则 | 核心支持能力与专属选项 |
| :--- | :--- | :--- |
| **Skills.sh (Agent Skills)** | `skills.sh`, `www.skills.sh` | - 技能详情与折叠正文恢复<br>- 仓库技能列表、站内搜索与榜单自动翻页 (`--skills-pages` / `--skills-limit`)<br>- 来源、安装命令与正文完整性等结构化元数据；见 [适配说明](08-skills-sh.md) |
| **哔哩哔哩 (Bilibili)** | `bilibili.com`, `b23.tv` | - 提取 CC 官方/AI 字幕轨 (`--subtitles`)<br>- 提取 AI 视频大纲总结 (`--bili-sum`)<br>- 提取时间轴弹幕表 (`--danmaku`)<br>- 提取高赞精选热评 (`--comments`)<br>- 视频与高清封面下载 (`--download-video`, `--download-cover`) |
| **YouTube** | `youtube.com`, `youtu.be` | - 原生多语言字幕轨提纯 (`timedtext` 格式)<br>- 视频章节目录与时间戳 (Chapters)<br>- 频道名称、发布时间、时长与播放量元数据<br>- 视频简介提纯与 Markdown 结构化呈现 |
| **Hacker News** | `news.ycombinator.com` | - Top / 首页 30 条极客资讯榜单抽取<br>- 讨论帖官方 Firebase API + HTML 双通道<br>- 层级缩进评论树 (Comment Tree) 递归提取与清洗 |
| **Discourse / linux.do** | `linux.do`, `users.rust-lang.org`, `community.openai.com` 等 | - linux.do 技术社区深度适配<br>- 原生 `/t/{id}.json` REST API 免无头浏览器直出<br>- 提取主楼正文、楼层回复、点赞量、分类与标签 |
| **X (Twitter)** | `x.com`, `twitter.com` | - 免鉴权官方 Syndication API 接口（抗封锁、零 API Key）<br>- 单推文、长推文讨论串 (Threads) 提取<br>- 高清图集/视频媒体附件与引用推文 (Quoted Tweet) 嵌套解析 |
| **Papers With Code** | `paperswithcode.com` | - 论文 Abstract 摘要与作者列表提取<br>- arXiv 原文与直接 PDF 下载链接<br>- GitHub 开源实现与 Stars 关注度表格<br>- SOTA 基准评测结果表格结构化呈现 |
| **知乎 (Zhihu)** | `zhihu.com`, `zhuanlan.zhihu.com` | - 问题、回答、想法、专栏文章高保真纯净抓取<br>- 防外链重定向拦截解包（还原真实目标 URL）<br>- 自动替换高清原图与 LaTeX 数学公式标准化<br>- 智能探测知乎安全验证码与反爬挑战 |
| **微信公众号 (WeChat)** | `mp.weixin.qq.com` | - 自动解析 `data-src` 懒加载真实高清图片<br>- 提取精准作者名与发布时间戳<br>- 智能识别“该内容已被发布者删除”与反爬风控页面<br>- 公开文章身份、名称发现、合集游标翻页与部分结果续取，见[微信公众号说明](11-wechat-public.md) |
| **小红书 (Xiaohongshu)** | `xiaohongshu.com`, `xhslink.com` | - 提取笔记正文、话题标签与发布者信息<br>- 自动提取高清无水印多图画廊列表 |
| **GitHub** | `github.com` | - 仓库 README 深度降噪与代码块语言标识保留<br>- Release 发版说明解析<br>- **Trending 热榜抽取**（支持每日/每周/每月及编程语言过滤） |
| **Hugging Face** | `huggingface.co` | - 模型主页 Model Card、数据集主页与论文正文提取<br>- 提取头部 YAML Frontmatter 元数据（任务分类、下载量、许可证等） |
| **Stack Overflow** | `stackoverflow.com` | - 问题正文与精简代码块提纯<br>- 采纳答案（Accepted Answer）高亮标记与点赞得分统计 |
| **V2EX** | `v2ex.com` | - 楼主主楼与 Append 附言合并整合<br>- 楼层回复按 Markdown 结构化表格导出 |
| **Reddit** | `reddit.com` | - 帖子正文与 Markdown 格式转换<br>- 评论树递归扁平化与层级引用标注 |
| **Wikipedia (维基百科)** | `*.wikipedia.org` | - 正文层级标题保留，自动剔除编辑链接 `[edit]` 与上标引用 `[1][2]`<br>- 信息盒（Infobox）与维基表格结构化保留 |
| **百度百科 (Baike)** | `baike.baidu.com` | - 词条核心摘要（Lemma Summary）提纯<br>- 基础信息表格（Basic Info）键值化抽取<br>- 多义词列表识别与参考资料降噪 |
| **ArXiv** | `arxiv.org` | - 论文 Abstract 摘要、作者列表、分类标签与 PDF 下载链接提取<br>- 支持 ArXiv Atom 订阅源解析与论文快讯追踪 |
| **Medium & Substack** | `medium.com`, `*.substack.com` | - 专栏博文高保真图文排版提纯，剥离付费弹窗遮罩与推荐流广告 |
| **掘金 (Juejin) & CSDN** | `juejin.cn`, `blog.csdn.net` | - 外链跳转中间页自动解包<br>- 代码块行号、一键复制按钮与广告占位容器彻底清除 |

> 💡 **提示**：若希望禁用平台内置适配器并强制走通用 Readability HTML 清洗流水线，只需追加 `--no-adapter` 参数。

---

## 2. 核心功能与 CLI 命令速查

### 2.1 基础网页抓取与输出控制
```bash
# 抓取网页并输出纯净 GFM Markdown
tkless https://example.com/article

# 限制上下文 Token 预算（段落安全断句，自动闭合未封闭的代码块）
tkless https://example.com/long-page --max-tokens 1500

# 输出带 YAML 前置元数据的 Markdown (Title, URL, Date, Author)
tkless https://example.com/article --frontmatter

# 剔除正文中的图片或超链接（大幅压减 Token 消耗）
tkless https://example.com/article --no-images --no-links

# 输出完整的结构化 JSON（包含 content, metadata, tokens 等字段）
tkless https://example.com/article -f json
```

### 2.2 DOM 定向提取与负向剪枝
```bash
# 仅提取特定 CSS 选择器容器内容
tkless https://docs.rs/tokio/latest/tokio/ --selector "div.docblock"

# 剔除正文中的指定噪音容器（负向选择器，支持逗号分隔多个选择器）
tkless https://example.com/article -R ".advertisement, .comment-box, #author-sidebar"
```

### 2.3 网页表格一键提取与 SQL/CSV 转换
```bash
# 仅提取网页中的全部数据表，并转为 SQLite/PostgreSQL 兼容的 DDL 与 INSERT 语句
tkless https://finance.example.com/report --tables-only --table-format sql

# 将数据表导出为标准 CSV 格式
tkless https://en.wikipedia.org/wiki/List_of_countries --tables-only --table-format csv
```

### 2.4 Schema 驱动的结构化实体抽取 (免 LLM 消耗)
```bash
# 自动探测页面的重复列表容器并生成推荐的 Schema 模版
tkless https://news.ycombinator.com/ --scaffold

# 使用内联 Schema 直接抽取特定结构化实体数组
tkless https://news.ycombinator.com/ --schema '{
  "items": {
    "selector": "tr.athing",
    "fields": {
      "title": ".titleline > a",
      "url": ".titleline > a@href",
      "rank": ".rank | default: 0"
    }
  }
}'
```

### 2.5 语义感知分块与长文档目录拆分 (RAG 增强)
```bash
# 启用语义切块（保护代码块与段落完整性；默认输出带标题路径与 Token 统计的 Markdown 分块）
tkless https://docs.rs/tokio/latest/tokio/ --chunk --chunk-size 500

# 需要机器可读结果时配合 -f json 输出完整文档结构

# 将长篇文档按标题拆分为多个独立 Markdown 文件并生成 index.md 导航索引
tkless https://en.wikipedia.org/wiki/Rust_(programming_language) --out-dir ./rust_doc
```

### 2.6 CDP 无头浏览器渲染与高清长截图
```bash
# 针对复杂 SPA 动态页面，调用本地 Chrome 渲染并抓取高清整页截图
tkless https://dashboard.example.com/app --screenshot ./dashboard.png

# 将渲染后的页面打印为 PDF（背景图形与 CSS 分页设置保留）
tkless https://dashboard.example.com/report --pdf ./report.pdf
```

### 2.7 免 API Key 全网搜索与即搜即抓
```bash
# 使用 Bing 检索关键词，默认最多 10 条
tkless search "Rust async tokio tutorial"

# 针对微信公众号定向检索
tkless search "大模型 Agent 架构设计" --scope wechat

# GitHub 平台定向，并输出结构化结果
tkless search "Tokio" --scope github -l 10 -f json

# 搜索前 3 条结果并在一次调用中直接抓取其清洗后的正文内容
tkless search "DeepSeek v3 架构解析" --limit 3 --fetch-content
```

搜索固定使用 Bing，无需引擎参数。`--scope` 会添加 `site:` 查询并在本地校验结果 URL；过滤后的数量可以少于上限。`--dedup` 与 `--max-tokens-per-page` 要求同时启用 `--fetch-content`。连接预检、浏览器回退和迁移方式见 [Bing 搜索说明](06-bing-search.md)。

### 2.8 交互式 REPL 探索会话命令行 (tkless shell)
```bash
# 启动交互式终端 REPL，无需重复冷启动进程即可执行一系列探索操作
tkless shell

# 交互式会话支持指令：
#   fetch <url>         - 抓取目标网页并输出清洗后的 Markdown
#   search <query>      - 全网免 Key 检索并以 Markdown 呈现结果列表
#   research <topic>    - 多跳自主深度调研并生成摘录报告
#   feed <url>          - 解析并排版输出 RSS 2.0 / Atom 1.0 订阅源列表
#   tables <url>        - 提取页面中的全部表格并以 GFM Markdown 表格输出
#   links <url>         - 提取页面超链接清单并去重展示
#   cache [clear]       - 查看本地响应缓存统计或清空磁盘缓存
#   summarize <url>     - 本地启发式提取页面核心要点速览
#   chunk <url>         - 将长文档按标题与 Token 预算进行语义感知切块
#   diff <url1> <url2>  - 对比两个网页版本的差异与行级变更比例
#   adapters            - 列出支持的全部平台适配器与能力
#   help / exit         - 查看帮助或退出会话
```

### 2.9 网页表格提取与格式转换子命令 (tkless tables)
```bash
# 提取网页中的全部表格并直接导出为 SQLite/PostgreSQL 兼容的 SQL DDL 与 INSERT
tkless tables https://finance.example.com/report --format sql

# 从本地 HTML 文件或管道提取表格并转为 CSV
tkless tables ./report.html --format csv --index 0

# 指定自定义 SQL 数据表名称
tkless tables https://example.com/stats --format sql --table-name "annual_metrics"
```

### 2.10 文档版本差异分析子命令 (tkless diff)
```bash
# 对比两个网页版本，输出统一 Diff 视图与增删行级指标
tkless diff https://example.com/v1.0 https://example.com/v2.0

# 对比本地两份 Markdown 文档并输出结构化 JSON 报表
tkless diff ./spec_v1.md ./spec_v2.md -f json
```

### 2.11 终端自动补全脚本生成 (tkless completions)
```bash
# 生成 Bash 自动补全脚本并加载
tkless completions bash > ~/.local/share/bash-completion/completions/tkless

# 生成 Zsh 自动补全脚本
tkless completions zsh > "${fpath[1]}/_tkless"

# 生成 Fish 自动补全脚本
tkless completions fish > ~/.config/fish/completions/tkless.fish

# 在 PowerShell 当前会话中实时加载自动补全
tkless completions powershell | Out-String | Invoke-Expression
```

---

## 2.12 配置式适配器（无需改代码）

设置 `TKLESS_ADAPTERS_DIR` 指向一个目录，启动时加载其中的 `*.json`。不设置时不加载任何配置适配器，内置适配器优先匹配。

```json
{
  "name": "my-blog",
  "hosts": ["blog.example.com"],
  "path_prefix": "/posts/",
  "schema": {"fields": {"title": "h1", "author": ".author"}}
}
```

- `hosts` 支持精确域和子域（`example.com` 匹配 `news.example.com`），最多 8 个；`path_prefix` 可选。
- `schema` 为普通 [ExtractionSchema](04-development-standards.md) 根字段定义，结果写入 `entity`。
- 适配器仅执行 HTTP 获取 + 通用清洗 + 可选 Schema 抽取，**不加载任何代码**；单文件 ≤ 64 KiB、最多 32 个。
- 配置非法（名称/域/前缀/Schema 错误、未知字段）会记录警告并跳过整个目录，不会静默加载部分内容。
- 内置适配器始终优先；配置适配器适合补全长尾站点，不替代需要签名/API 的平台适配器。

## 3. Agent 管道典型集成范例

### 3.1 Claude Code / Cursor 自动化调研 Prompt 模板
```text
请使用 tkless 命令行工具，针对用户提出的技术问题进行深度调研：
1. 优先使用 `tkless search "<关键词>" --limit 3 --fetch-content` 获取前 3 篇核心文章。
2. 若遇到长篇文档，使用 `tkless "<URL>" --max-tokens 2000 --summarize` 提取要点。
3. 若需提取对比表格，使用 `tkless "<URL>" --tables-only --table-format markdown`。
```
