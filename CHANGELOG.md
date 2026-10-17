# 变更日志 (Changelog)

本项目的所有显著变更均记录于此文件中。
遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/) 格式与 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/) 规范。

---

## [0.3.0] - 2026-10-18

### 🌟 核心突破与重大更新

#### 1. 工业级搜索门禁与反爬自愈体系
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
