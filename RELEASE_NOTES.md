# TkLess v0.3.0 — Spend less tokens, perceive more web.

本次版本带来了针对**垂直生态搜索管道、CDP无头浏览器自动托管、反爬无缝容灾回退、中间重定向解密**的重大系统升级：

---

### 🚀 核心更新亮点 (Key Highlights)

#### 1. 🌐 垂直生态专属搜索管道与多引擎解耦
- **百度百科原生词条直连 (`search_baike`)**：使用 `--baike` 时，直接路由至百度百科原生 OpenAPI 词条接口，彻底解决通用搜索引擎将词条降级为官网导航或首词百科的问题，响应时间 < 100ms，命中率 100%；
- **微信公众号文章专属检索 (`search_wechat_sogou`)**：使用 `--wechat` 时，自动路由至微信文章专用检索管道，配合会话级 Cookie 保持与动态 JS 解密，直接返回真实可读的公众号文章列表。

#### 2. 🤖 内置无头浏览器守护进程与 CDP 自动托管 (Auto-Managed CDP)
- **本地系统浏览器自探测 (`locate_system_browser`)**：自动定位 Windows、macOS 与 Linux 系统中已安装的 Chrome、Edge 或 Chromium；
- **全自动开箱即用**：指定 `--cdp` 或触发动态渲染时，无需用户手动在外部命令行启动浏览器。若端口未监听，TkLess 会自动以 `--headless=new` 静默拉起独立守护进程并接管，彻底告别“无法连接到 Chrome CDP 端点”崩溃。

#### 3. 🛡️ 反爬与 CDP 容灾回退（Fallback）无缝化
- **深度防爬指纹识别**：精准识别 HTTP 403/429/503，以及百度 Paris/Heicha WAF、搜狗反爬、Cloudflare 5秒盾等验证码拦截页；
- **自适应无缝降级**：轻量 HTTP 遇到阻断时不再直接中断报错，而是自动将目标页面无缝交由 CDP 真实浏览器执行 JS 水合加载，对上层 Agent 保持完全透明。

#### 4. 🔑 平台适配器前置防盗链中间跳转解密
- **搜狗微信跳转页逆向解密 (`decrypt_sogou_wechat_redirect`)**：针对微信搜索结果中的动态 JS 拼接跳转页（`/link?url=...`），内置会话预热与 JS 切片还原引擎，直接提取真实的 `https://mp.weixin.qq.com/s?...` 直达文章链接，避免二次验证码阻断。

#### 5. 🔍 工业级搜索门禁与反爬自愈体系
- **商业广告与推广位彻底过滤**：拦截淘宝、抖音、TapTap、阿里云盘、Poki 等垃圾推广，误放行率降低至 0%；
- **首词降级（Term Drop）严格拦截**：检测 query 核心语义词覆盖率，坚决拦截仅命中首词的粗粒度百科结果；
- **多阶段智能查询松弛**：自动剥离超前年份时效约束重试，自动执行中文紧凑短语重组，实测中文长尾时效（Group C）命中率达 100%。

#### 6. ⚡ 4 线程 HTTP Range 分片并发下载加速
- **突破单连接 QoS 限速**：基于 HTTP 206 Range 实现 4 线程分片并发拉取，B站视频下载速度提升 3~5 倍；
- **1MB 大块写缓冲 (BufWriter)**：底层磁盘写缓冲由 8KB 扩容至 1024KB，减少 99% 的系统调用中断；
- **三级智能 CDN 路由优选**：优先调度阿里云、腾讯云、百度云和 Akamai 骨干 CDN，彻底排查并降权低速 PCDN / P2P 节点。

#### 7. 🗄️ 操作系统标准缓存路径重定向与自动 Prune
- **消除工程目录污染**：默认缓存目录由当前工程下的 `.tkless_cache` 重定向至跨平台标准用户目录（Windows: `%LOCALAPPDATA%\tkless\cache`，macOS: `~/Library/Caches/tkless`，Linux: `~/.cache/tkless`）；
- **启动时自动过期清理**：每次启动时静默清理超过 TTL（默认 15 分钟）的过期文件，防止磁盘无限膨胀。

---

### 📦 跨平台独立二进制下载 (Direct Standalone Downloads)

| 操作系统 / 架构 | 发布产物名称 | 说明 |
| :--- | :--- | :--- |
| **Windows x86_64** | `tkless-windows-amd64.exe` | 独立免安装程序，直接放入口径 PATH 或供 Cursor / IDE 直连 |
| **macOS Apple Silicon (M1~M4)** | `tkless-macos-arm64.dmg` | 原生 Apple Silicon DMG 镜像包，拖入 Applications 即可使用 |
| **macOS Intel (x86_64)** | `tkless-macos-amd64.dmg` | Intel 架构 Mac DMG 镜像包 |
| **Linux x86_64** | `tkless-linux-amd64` | 静态编译独立 ELF 二进制 (内置 OpenSSL vendored, 零 glibc 依赖冲突) |
| **Linux aarch64 (ARM64)** | `tkless-linux-arm64` | ARM64 Linux 云服务器与树莓派独立二进制 |

---

### 🛠️ 一键自升级命令

已安装用户可直接在终端中执行自更新至 v0.3.0：
```bash
tkless upgrade
```
