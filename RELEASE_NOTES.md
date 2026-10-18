# TkLess v0.4.0 — Spend less tokens, perceive more web.

TkLess v0.4.0 带来重大功能演进：**原生 PDF 智能解析与重排**、**Autonomous Deep Research 多跳自主深度调研体系**、**CDP 协议级资源拦截加速**、**DuckDuckGo 双引擎聚合**、**全方言 Schema 抽取与容错回退**、**维基内联 CSS 噪声根除**、**全链路 Doctor 环境自检**以及 **CLI 与 Windows PowerShell 体验深度净化**。

---

### 🚀 核心更新亮点 (Key Highlights)

#### 1. 📄 原生 PDF 智能解析与 Markdown 重排 (`tkless-core::pdf`)
- **纯 Rust 零外部依赖**：基于 `pdf-extract` 与 `lopdf` 构建纯 Rust PDF 解析引擎，完全脱离 Poppler、PDFium 及系统 C/C++ 动态链接库与 OpenSSL 编译绑定，全平台（Windows / macOS / Linux）免环境配置开箱即用；
- **全通道输入无缝支持**：自动感知输入形态，无缝支持远程 `.pdf` URL、本地 `.pdf` 文件路径以及管道 `stdin` 输入流（如 `curl -s ... | tkless -`）；
- **页眉/页脚/页码智能去噪**：内置高频页眉、页脚及纯数字/罗马数字页码模式识别器，剔除跨页重复出现的版权声明与页码杂讯；
- **Markdown 结构化重排**：智能重组 PDF 断行碎片，恢复自然段落缩进与多级标题层级（H1/H2/H3），生成高密度、低 Token 消耗的纯净结构化文档。

```bash
# 抓取远程学术/技术 PDF 并直接输出纯净 Markdown
tkless fetch https://arxiv.org/pdf/1706.03762.pdf -o attention.md

# 解析本地 PDF 文件
tkless fetch ./annual_report.pdf --stats
```

#### 2. 🔬 Autonomous Deep Research 多跳自主深度调研体系
- **多轮多跳递归探索 (`tkless research` & MCP `deep_research`)**：支持设置调研主题、最大探索跳数（hops）与分支发散度。基于前置抓取结果自动生成后续追问子查询，形成发散与收敛闭环；
- **多源并发抓取与交叉比对**：自动化并行检索多个引擎并抓取高权重网页，通过交叉比对不同信源消除幻觉；
- **结构化研报生成与思维链留痕**：最终自动沉淀包含“核心摘要”、“关键发现矩阵”、“详细论证”与“参考信源列表”的 Markdown 深度研究报告，并完整记录研究跳数与决策过程。

```bash
# 执行 2 跳深度调研，发散探索前沿进展
tkless research "Rust 异步运行时性能优化最新实践" --max-hops 2 -o research_report.md
```

#### 3. ⚡ CDP 资源拦截加速 (3~5x) 与自动懒加载滚动
- **DevTools 协议级静态资源拦截**：新增 `--cdp-block-images`、`--cdp-block-media`、`--cdp-block-fonts` 核心拦截策略，在 CDP 层拦截图片、音频、视频与字体静态资源，网络吞吐量降低 70% 以上，首屏渲染提速 3~5 倍；
- **平滑渐进式页面滚动 (`--cdp-auto-scroll`)**：支持自适应步长平滑滚动到底部，彻底触发现代 SPA 与瀑布流站点的无限懒加载与动态水合，确保动态长文本完整捕获。

```bash
# 开启图片与媒体拦截，极速渲染重度动态页面
tkless fetch https://example.com --cdp --cdp-block-images --cdp-block-media --cdp-auto-scroll
```

#### 4. 🌐 DuckDuckGo 引擎集成与双引擎并发去重检索
- **全球化与海外开发者生态支持**：在原有 Bing 基础之上，全新引入 DuckDuckGo 搜索源，支持 `-e all`（并发聚合）、`-e bing`、`-e duckduckgo` 灵活指定；
- **双引擎并发与 SimHash 语义去重**：`all` 模式下双引擎并行发起查询，基于规范化 URL 签名与 SimHash 汉明距离进行双重去重，消除冗余并提供更均衡的搜索覆盖率；
- **弹性容灾降级与显式告警**：引擎遇到反爬验证或网络超时时保持优雅降级，并在结果中明确标识可能受阻的信源，杜绝空结果假象。

```bash
# 聚合 Bing 与 DuckDuckGo 双引擎结果
tkless search "Rust web framework benchmark" -e all -l 10
```

#### 5. 🎯 声明式 Schema 抽取全方言统一与容错回退
- **全方言模式通用支持**：统一兼容平铺键值对（`{"title": "h1", "price": ".amount"}`）、嵌套 `properties` 规范以及标准 JSON Schema 格式；
- **智能注解剥离与选择器派生**：自动剥离类型提示（如 `| number`、`| string`），并根据字段语义启发式推导最优 CSS 选择器；
- **容错降级与明确诊断**：当选择器未匹配到任何字段时，不再抛出异常崩溃，而是输出清晰的字段诊断提示（`matched: 0`）并平滑回退至正文全文抽取。

```bash
# 使用任意 JSON Schema 方言抽取结构化数据
tkless fetch https://news.ycombinator.com --schema '{"title": ".titleline > a", "rank": ".rank"}' -f json
```

#### 6. 🧹 维基百科与信息框内联 CSS 噪声彻底根除
- **精准限定内容边界**：维基百科适配器正文抽取根严格锁定 `#mw-content-text`，彻底剔除 Vector 2022 皮肤的目录侧栏（`.vector-toc`）与 50+ 门语言切换长列表；
- **内联样式穿透剥离**：彻底清除正文和 Infobox 表格中内嵌的 `<style>`、`@media` 以及 `.mw-parser-output` CSS 类声明，使提取出的正文及导出的 CSV/SQL 数据恢复极致纯净。

#### 7. 🩺 全链路环境自检诊断 (`tkless doctor`)
- **一键诊断子命令**：新增 `tkless doctor` 命令，全面探测系统运行环境：
  - 系统架构与操作系统版本；
  - 本地 Chrome / Microsoft Edge / Chromium 无头浏览器就绪状态与路径探测；
  - 本地响应缓存（Response Cache）读写权限、容量及磁盘配额统计；
  - 核心搜索引擎与外部代理服务器的网络连通性及延时探测。

```bash
# 运行环境自检
tkless doctor
```

#### 8. 🛠️ CLI 体验净化、参数体系标准化与架构高内聚解耦
- **PowerShell 与非 TTY 体验净化**：仅在终端为 TTY 时展示动态进度；管道/重定向模式下自动抑制实时刷新并支持 `NO_COLOR`，彻底根除 Windows PowerShell 下 exit 0 误报红色 `NativeCommandError` 的视觉痛点；
- **全局选项覆盖与子命令统一**：全局统一支持 `-o/--output`、`-q/--quiet`、`-v/--verbose`、`--stats`，并在 `tables` 等子命令中统一支持 `--table-format`；
- **架构模块解耦与极速轻量化**：拆分 `tkless-cli` 为 `doctor.rs`、`progress.rs`、`shell.rs`、`upgrade.rs` 单一职责模块；剔除 `readability` 默认附带的重复旧版依赖，减少 46 个上游依赖包，大幅提升编译速度并压缩编译后二进制体积。

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

已安装用户可直接在终端中执行自更新至 v0.4.0：
```bash
tkless upgrade
```
