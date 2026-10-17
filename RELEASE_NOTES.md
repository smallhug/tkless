# TkLess v0.3.0 — Spend less tokens, perceive more web.

本次版本带来了针对**搜索结果准确率治理、多线程并发下载加速、系统级缓存路径重定向**的重大系统升级：

---

### 🚀 核心更新亮点 (Key Highlights)

#### 1. 🔍 工业级搜索门禁与反爬自愈体系
- **商业广告与推广位彻底过滤**：拦截淘宝、抖音、TapTap、阿里云盘、Poki 等垃圾推广，误放行率降低至 0%；
- **首词降级（Term Drop）严格拦截**：检测 query 核心语义词覆盖率，坚决拦截仅命中首词的粗粒度百科结果（如搜“北京小学入学政策”绝不返回“北京市_百度百科”）；
- **人机验证码主动甄别与显式报错**：新增 `SearchAntiScraping` 状态，主动识别 Bing 验证码挑战，杜绝假空结果，并指导使用 `--cdp` 穿透；
- **多阶段智能查询松弛**：自动剥离超前年份时效约束重试，自动执行中文紧凑短语重组，实测中文长尾时效（Group C）命中率从 **8% 提升至 100%**。

#### 2. ⚡ 4 线程 HTTP Range 分片并发下载加速
- **突破单连接 QoS 限速**：基于 HTTP 206 `Range` 实现 4 线程分片并发拉取，B站视频下载速度提升 **3~5 倍**；
- **1MB 大块写缓冲 (BufWriter)**：底层磁盘写缓冲由 8KB 扩容至 1024KB，减少 99% 的系统调用中断；
- **三级智能 CDN 路由优选**：优先调度阿里云、腾讯云、百度云和 Akamai 骨干 CDN，彻底排查并降权低速 PCDN / P2P 节点。

#### 3. 🗄️ 操作系统标准缓存路径重定向与自动 Prune
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
