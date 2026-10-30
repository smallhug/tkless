# 04 - TkLess 核心开发规范与工程标准指南

> **Spend less tokens, perceive more web.**  
> 本文档定义 TkLess 项目的架构设计原则、Rust 编码与风格准则、安全防御硬约束、平台适配器扩展规程及 Git 提交规范，作为全仓库开发、代码审查（Code Review）与持续集成的法定参考基线。

---

## 🏛️ 一、架构设计与模块分层原则

TkLess 采用标准的 Cargo Workspace 多包模块化架构，严格遵循**关注点分离（Separation of Concerns）**与**零无用运行时依赖**：

```text
tkless/
├── crates/
│   ├── tkless-core/     # 提取、工作流与 MCP 通用传输；不绑定进程 stdin/stdout
│   └── tkless-cli/      # 参数解析、commands/ 命令编排、标准流绑定、输出与 REPL
├── doc/                 # 核心架构与开发规范文档中心
├── scripts/             # 开发检查脚本，不是产品运行时依赖
└── .github/workflows/   # 跨平台 CI/CD 自动化构建流水线
```

### 1. 核心库独立性约束 (`tkless-core`)
- `tkless-core` **严禁直接向标准输出/标准错误（`println!`, `eprintln!`）打印任何非必要内容**。所有运行时状态与诊断信息必须通过 `tracing` 宏（`trace!`, `debug!`, `info!`, `warn!`）记录，由上层宿主环境自由决定如何订阅。
- 保证可作为第三方 Rust 库被外部程序、Web 框架或本地智能体直接引入，无全局状态绑定。

### 2. 严格标准流隔离 (Strict Stream Separation)
- `stdout`（标准输出）：**仅且只能输出纯净的目标提取数据**（Markdown 正文、JSON、SQL DDL、纯文本）。任何 Agent、管道重定向（`|`）或自动化脚本抓取 stdout 时，绝不能混入任何非目标载荷。
- `stderr`（标准错误）：输出所有的网络耗时、HTTP 状态、Token 统计、进度条与调试日志。

### 3. 内存与便携性底线
- **纯 Safe Rust**：全库严禁引入自定义 `unsafe` 代码块或裸指针操作。所有内存安全与并发同步依托 Rust 编译器借用检查、Tokio 运行时与标准库同步原语。
- **分发依赖**：HTTP 使用 `rustls-tls`，不要求额外启用 `openssl/vendored`。各发布目标的系统库要求以构建产物为准；CDP 功能需要本机浏览器或可访问的 CDP 服务。
- **性能验证**：冷启动和内存数据必须注明平台、构建配置、输入规模、缓存和浏览器状态；未测量的性能目标不得写成已满足的保证。

---

## 💻 二、Rust 语言与编码规范

### 1. 命名与代码风格
- 严格遵循 Rust 官方命名规范：
  - 类型、特征、枚举：大驼峰（`TklessEngine`, `PageDocument`, `PlatformAdapter`）；
  - 函数、方法、变量、模块：蛇形命名（`process_url`, `extract_discovered_links`, `clean_html`）；
  - 常量、静态正则：全大写蛇形（`MAX_REDIRECTS`, `A_HREF_SELECTOR`）；
  - 静态编译期正则统一使用 `std::sync::LazyLock` 封装，杜绝重复解析编译损耗。
- 代码格式必须 100% 满足 `cargo fmt` 官方规范，行宽以 100 字符为软性基线。
- 当前支持的最低 Rust 工具链为 **1.98.1**，由 workspace `rust-version` 声明并通过 CI 验证；这是一条维护基线，不表示代码无法在更旧版本编译。
- Workspace 使用 `unsafe_code = "forbid"`，两个成员通过 `[lints] workspace = true` 继承。
- CLI 命令编排放在 `commands/`；MCP 传输和业务处理器放在 `mcp/server/`，按职责划分，避免入口文件聚集业务代码。
- 分页与浏览器限制集中于 `limits.rs`，CLI、核心校验和 MCP Schema 引用同一常量；业务停止原因与同步事件使用 `workflow.rs` 枚举，JSON 使用 snake_case。

### 2. 错误处理与退出码规范
- **领域错误统一定义**：`tkless-core` 使用 `thiserror` 派生枚举 `TklessError`，为每种错误类型提供精确的语义说明。
- **命令行退出码映射**：`TklessError` 必须实现 `exit_code(&self) -> i32`，严格映射为 Agent 调度友好的语义退出码：
  - `0`: 成功 (Success)；
  - `1`: 通用错误 (I/O, 未分类异常)；
  - `2`: 参数或 URL 格式错误；
  - `3`: 网络连接异常、DNS 失败、超时或服务端 5xx 故障；
  - `4`: 编码解码异常或客户端 HTTP 4xx 错误；
  - `5`: 响应体超出 `--max-size` 限制；
  - `6`: SSRF 拦截命中（拒绝私有/回环网络请求）。
- `anyhow` **仅允许**在 `tkless-cli` 二进制中使用，用于收集和丰富顶层调用上下文（`.context(...)`）。
- 参数错误与运行故障必须区分。状态锁竞争为 `state_busy`（可重试），损坏状态为 `state_corrupt`，存储预算超限为 `state_budget_exceeded`，后台任务失败为 `task_failed`；以上退出码均为 1，不能包装成参数错误 2。
- 同步磁盘遍历、快照序列化与持久化在 `spawn_blocking` 中执行。写入任务持有锁至提交结束；调用取消不会中止已经开始的阻塞写入，恢复时以已提交清单为准。
- 可恢复的序列化错误使用 `?` 传播；确属不变量的 `expect` 必须说明依据。不要将所有 `unwrap` 无差别替换成静默默认值。

### 3. Token 经济学与边界安全截断
- 任何正文截断操作必须通过 `tkless_core::token::truncate_to_tokens` 统一执行，**严禁使用粗暴的字符切片（`&str[..n]`）**。
- 截断算法必须感知段落（`\n\n`）与自然句子（句号/问号/感叹号）边界，且必须在截断处**自动检测并补齐未封闭的 Markdown 代码块（```` ``` ````）**，杜绝大模型在消费时产生未闭合代码块语法瘫痪。

---

## 🛡️ 三、网络安全与数据防御硬约束

作为需要直接面向未知互联网抓取页面的自动化引擎，必须坚守以下安全红线：

### 1. SSRF 全链路深度防御
- **全网请求强制前置校验**：所有面向用户传入 URL 的网络请求（普通抓取、整站爬虫、搜索结果即抓取、CDP 无头渲染、长截图、链接巡检），必须在出站前调用 `fetcher.validate_ssrf(&parsed_url)`。
- **全 DNS 解析遍历**：DNS 解析阶段若解析出多个 IP，必须遍历所有返回的 IP，只要存在任意一个命中私网（RFC 1918）、本地环回（`127.0.0.1`）、链路本地（`169.254.169.254` 云厂商元数据），必须全盘拦截。
- **重定向逐跳拦截 (Hop-by-Hop)**：网络层对 3xx 跳转重定向必须每一跳重新进行 SSRF 校验，防止攻击者利用公网域名 302 跳转至内网靶机。

### 2. 凭据作用域隔离与防泄漏
- **平台前缀存储隔离**：Cookie 文件持久化必须带平台标识符（如 `bilibili: ...`, `zhihu: ...`），同文件内多站点互不污染。
- **域名匹配出站**：出站请求仅根据当前请求的 URL 顶级私有域名匹配对应的平台凭据；无前缀命令行 Cookie 仅且只能绑定初始目标 Host，在发生跨域重定向时立即清空 Cookie，严防凭据被第三方捕获。

### 3. 防注入与数据净化
- **SQL DDL/INSERT 安全**：
  - 表名与列名必须通过 `sanitize_sql_identifier` 字符白名单清洗（仅允许 `[a-z0-9_]`）；
  - 若表头清洗后存在同名标识符，必须通过 `seen_col_names` 自动追加序号（`_1`, `_2`）消歧，确保生成的 DDL 语法合法；
  - 单元格数据必须包裹单引号，并对内部所有单引号严格执行 SQL 倍写转义（`'` ➔ `''`）。
- **XML/RSS/Atom 解析**：使用 `html5ever`，严禁开启外部实体展开与 DTD 处理，彻底免疫 XXE 攻击。
- **路径遍历防御**：长文档拆分文件名必须经过 `slugify` 正则清理；图片资产使用内部生成的 SHA-256 哈希名称，禁止直接使用远程文件名作为本地磁盘路径。

---

## 🧩 四、平台专属适配器开发规程

当引入新的网站适配器（如 Reddit、Wikipedia、V2EX）时，必须遵守以下规范：

1. **实现标准特征 (`PlatformAdapter`)**：
   ```rust
   pub trait PlatformAdapter: Send + Sync {
       fn name(&self) -> &'static str;
       fn matches(&self, url: &Url) -> bool;
       fn extract<'a>(
           &'a self,
           fetcher: &'a Fetcher,
           url: &'a Url,
           options: &'a ExtractionOptions,
       ) -> BoxFuture<'a, Result<PageDocument, TklessError>>;
   }
   ```
2. **轻量与高保真优先**：
   - 优先通过提取页面内置数据树（如 `__INITIAL_STATE__`、JSON-LD）或轻量公开 API 解析，耗时应压制在 100~200ms 以内，非必要不启动无头浏览器；
   - 自动解包平台外链中转页（如 `link.zhihu.com/?target=...`、`link.juejin.cn/?target=...`）；
   - 图片地址自动替换为平台最高清无水印原始资源链接。
3. **反爬与降级意识**：
   - 必须实现反爬挑战探测（如 Cloudflare 滑块、平台图形验证码），当遭遇风控拦截时返回清晰的错误诊断或触发 CDP 降级。
4. **适配器注册**：
   - 在 `crates/tkless-core/src/adapters/mod.rs` 中的 `AdapterRegistry::default()` 完成注册。
5. **配套单元测试**：
   - 每个适配器必须在所属模块的 `tests` 模块中提供：
     1. URL 路由匹配测试（支持主站、子域、短链接、移动端链接）；
     2. 基于离线 HTML 片段或 JSON 夹具的解析测试（严禁依赖外部真实网络）。

---

## 📋 五、CLI 命令行帮助信息 (-h/--help) 输出规范

TkLess 作为兼具人类开发者直接交互与 AI Agent 自动探索（Command Line Discovery / Function Calling）双重场景的生产级工具，其命令行帮助信息的清晰度与规范性至关重要：

### 1. 结构化功能分组 (`help_heading`)
- 顶层命令及拥有较多参数的子命令（如 `tkless` 根命令、`crawl`、`search`、`batch`），**严禁将所有参数平铺堆砌**。
- 必须使用 `help_heading` 属性将选项清晰归类到明确的业务维度：
  - `通用提取选项 (General Options)`：输出格式、正文模式、Token 上限、正负向 CSS 选择器、Schema 抽取、分块、表格过滤等；
  - `哔哩哔哩专属 (Bilibili Options)`：仅影响特定站点的专属功能参数；
  - `网络与身份认证 (Network & Auth)`：Cookie、请求头、代理、超时重试、CDP 无头端点、移动端模拟排版等；
  - `缓存与运行诊断 (Cache & Diagnostics)`：本地二级缓存开关、TTL、分词器选择、遥测统计、详细日志调试等。

### 2. 参数命名与短选项唯一性硬约束
- **短选项 (`short`) 全局唯一**：在同一命令或同一子命令结构体（如 `CrawlArgs`）内部，短选项必须严格唯一，**严禁不同参数使用相同的字母**（例如严禁 `--max-pages` 与 `--proxy` 同时占用 `-p`，应分别采用 `-P` 与 `-p`），否则将触发 Clap 运行期冲突断言 Panic。
- **长选项 (`long`) 风格**：统一使用小写连字符 `kebab-case`（如 `--max-tokens`, `--table-format`），禁止驼峰或下划线命名。
- **参数值占位符 (`value_name`)**：必须提供大写且具备清晰业务含义的名词（如 `N`, `DIR`, `CSS_SELECTOR`, `PROXY_URL`, `FILE`），严禁使用无意义的 `<val>`。

### 3. 信息密度与文案标准
- **首行摘要简明扼要**：用于 `-h` 快速展示，首行不得超过 40 字；
- **默认值与取值范围**：优先使用 Clap 的 default_value、value_parser 和 value_enum 自动展示默认值及允许值；不要在描述中重复手写默认值。依赖参数用 requires 或执行前校验明确限制，CLI 与 MCP 同步维护。
- **版本号与品牌 Banner**：帮助信息 Banner 必须完整嵌入产品定位与口号：
  ```rust
  version = concat!(env!("CARGO_PKG_VERSION"), " (built ", env!("TKLESS_BUILD_DATE"), ")"),
  about = "TkLess: Spend less tokens, perceive more web.",
  long_about = "TkLess — Spend less tokens, perceive more web.\n专为大语言模型与 AI Agent 量身定制的高性能网页抓取、降噪清洗与搜索提取引擎。..."
  ```

---

## 🤖 六、Model Context Protocol (MCP) 服务端交互与输出规范

TkLess 原生实现了基于标准 stdio 的 JSON-RPC 2.0 MCP 协议，为确保被 Cursor、Claude Desktop、Zed 及各大 Agent 框架稳定调用，必须遵守以下通信与输出数据契约：

### 1. stdio 通道绝对纯净原则 (Zero Stream Pollution)
- **严格的标准流隔离**：在 MCP 模式（`tkless mcp`）下，**标准输出 (`stdout`) 必须且只能输出严格符合 JSON-RPC 2.0 规范的单行 JSON 字符串**。
- **严禁向 stdout 输出裸文本**：全库任何模块、第三方依赖的 `println!`、调试追踪或 Panic 栈追踪严禁流向 stdout。任何非法字符（如裸字符串、警告、未封装的 JSON）都会直接导致 Cursor 或 Claude Desktop 的 MCP 客户端解析失败并强制断连。
- 所有的运行日志、调试 Trace 必须经由 `tracing` 输出到 `stderr`。

### 2. 工具调用响应格式与数据契约 (`tools/call`)
- **成功响应 (`isError: false`)**：
  - 必须符合标准结构：`{"content": [{"type": "text", "text": "..."}], "isError": false}`；
  - 返回的文本内容必须高密度提纯，根据请求格式自适应输出纯净 Markdown、格式化 JSON、SQL DDL/INSERT 或 CSV，**绝不包裹冗余的客套欢迎语或对话型废话**，最大限度为宿主大模型节约上下文 Token。
- **失败响应 (`isError: true`) 与自愈引导**：
  - 工具执行异常（网络失败、解析失败、参数非法、SSRF 阻断）时，必须设置 `"isError": true`；
  - 错误文本必须遵循**“故障定位 + 可操作指引”**原则，指导 AI Agent 自行调用修复工具或调整参数重试，例如：
    - 凭据失效时：`"⚠️ [BILIBILI] 凭据已失效或已过期。建议操作: 调用 login_platform(platform='bilibili') 或执行 tkless login bilibili 重新授权。"`
    - 反爬阻断时：`"❌ 遭遇目标站点反爬挑战。建议操作: 在参数中配置 cdp_endpoint (如 'http://127.0.0.1:9222') 启用无头浏览器渲染绕过。"`
    - SSRF 拦截时：明确提示目标地址属于私有保留网段，严禁内网嗅探。

### 3. 流式进度通知规范 (`notifications/progress`)
- **Token 关联性**：仅当客户端在请求的 `_meta.progressToken` 中提供了标记时，服务端才触发进度通知。通知必须携带对应的 `progressToken`。
- **推流频次节流 (Throttle)**：
  - 对于整站爬虫（`crawl_site`）或大批量并发抓取（`batch_fetch_webpages`），必须进行进度推流频率控制（节流间隔不低于 `100ms` 或按完成条数步进）；
  - **严禁毫秒级或按字节密集狂刷进度通知**，避免引发宿主 IDE/客户端的 UI 渲染卡死。
- **进度消息语义化**：
  - 必须提供明确的数值指标：`progress: 当前已完成数`, `total: 目标总数`；
  - `message` 必须具备上下文含义，例如 `"Crawling page 15/50: https://example.com/docs/api"`。

### 4. 规范工具名与当前协议
- 调度器仅接受目录中的规范名称（如 `export_tables`），直接调用、动态执行、Schema 查询和激活使用相同名称。不得恢复旧工具别名或前缀转换。
- MCP 只实现 `2025-11-25`；初始化始终声明该版本，不剥离输出 Schema 或结构化结果。CLI 参数只保留一个规范长名称及必要的短参数。

### 5. 两阶段动态工具发现规范 (Two-Stage Dynamic Tool Discovery)
针对 MCP 客户端默认将所有工具定义注入 System Prompt 导致上下文过度消耗的问题，TkLess 提供了两阶段动态工具发现架构（`--dynamic` 或 `--profile dynamic` 或环境变量 `TKLESS_MCP_DYNAMIC=1`）：

- **第一阶段：轻量常驻核心集 (Stage 1: Resident Core + Meta Discovery Tools)**
  - **初次暴露工具数**：从全量 26 个业务工具大幅精简为 **6 个常驻工具**；
  - **核心基础工具 (Core Tools)**：`fetch_webpage`（多格式抓取与清洗）、`web_search`（免 Key 全网搜索）；
  - **发现元工具 (Meta-Tools)**：
    1. `discover_tools(query, category)`：按关键词或分类（`content`, `search`, `media`, `analysis`, `system`, `all`）检索专门工具目录及简述；
    2. `get_tool_schema(tool_name)`：按需获取指定工具的完整 `inputSchema` 和参数枚举定义；
    3. `execute_tool(tool_name, arguments)`：即使该工具未在 `tools/list` 声明中暴露，Agent 也可通过元工具立即无缝执行；
    4. `activate_tools(tool_names)`：将会话所需工具动态载入当前激活列表，并通过 `notifications/tools/list_changed` 通知 MCP 宿主重新获取工具列表。
- **第二阶段：按需探查与执行 (Stage 2: On-Demand Discovery & Execution)**
  - Agent 在识别出特殊需求（如提取数据表格并转为 SQL、音视频字幕抓取、文档比对、语义切块）时，先调用 `discover_tools` / `get_tool_schema` 确定工具签名，随后既可直接通过 `execute_tool` 单次即时运行，也可调用 `activate_tools` 动态入库；
  - **直接调用**：动态模式也允许直接调用未激活的规范业务工具名称（如 `export_tables`），参数与输出仍经过同样的校验。
- **Token 开销**：以当前 Schema、分词器和宿主注入策略实测，不将历史固定比例作为验收保证。

---

## 🧪 七、质量门禁与测试标准

在向仓库提交任何代码之前，必须确保在工作区根目录下执行并通过以下检查：

```bash
# 1. 代码格式一致性检查 (禁止提交未格式化的代码)
cargo fmt --all -- --check

# 2. 严苛静态分析 (警告视为错误，强制 0 告警通过)
cargo clippy --workspace --all-targets -- -D warnings

# 3. 全工作区自动化测试全量通过
cargo test --workspace

# 4. 生产优化构建验证
cargo build --release -p tkless-cli

# 5. 可选特性与文档契约
cargo check -p tkless-core --no-default-features --all-targets --locked
node scripts/check-docs.cjs target/release/tkless

# 6. 浏览器相关改动（需要本机 Chrome/Edge）
cargo test -p tkless-core --test bing_cdp_test -- --ignored
```

Windows 的文档检查传入 `target/release/tkless.exe`。Node.js 仅用于开发检查，不是 TkLess 的运行依赖。CI 分别运行三平台常规检查、Rust 1.98.1 编译和 Windows 浏览器测试；本地通过不能替代远程 CI 结果。

---

## 📝 八、Git 提交信息规范 (Commit Guidelines)

遵循清晰、简明且规范的 Git 提交风格：

- **语言**：使用中文精炼总结代码改动。
- **标题行 (Subject Line)**：
  - 尽量限制在 50 个字符以内；
  - 首字母大写（英文）或祈使动词开头（如：`支持...`、`修复...`、`重构...`、`优化...`）；
  - **末尾严禁添加任何标点符号**（如句号、感叹号）。
- **正文 (Body - 可选)**：
  - 仅在标题行无法完整表达上下文时补充，说明“为什么改”以及“核心影响”；
  - 标题与正文之间空一行，单行字数折行限制在 72 字符以内。
- **示例**：
  - 好的示例：`支持表格提取与文档比对CLI子命令并增强REPL会话`
  - 好的示例：`修复Linux交叉编译OpenSSL缺失并启用vendored静态编译`
  - 避免的示例：`fix: bug.` 或 `更新代码`
