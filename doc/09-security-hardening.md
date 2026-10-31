# 安全加固记录（2026-10-30）

本次修复针对代码审计发现的凭据跨源转发、HTML 转义、下载覆盖和资源消耗问题。回归测试使用本地服务及虚构凭据，不依赖搜索引擎在线状态。

## 请求与凭据

- `Authorization`、`Proxy-Authorization`、`Cookie`、`Cookie2`、`X-API-Key`、`Api-Key`、`X-Auth-Token` 不再进入客户端默认请求头；仅发送至 `FetchOptions.initial_target_origin` 对应的协议、主机和端口。CLI 单 URL 请求自动设置该值。库调用者需要显式设置规范化 origin，例如 `https://example.com`；未设置时不发送这些自定义凭据。
- 同源重定向保留凭据，跨源、端口变化和 HTTPS 降级不携带上述自定义凭据。`--cookie` 和 Cookie 文件仍使用原有的独立作用域规则。
- 普通 HTTP 请求逐跳校验目标；DNS 查询失败不再放行。预检查会解析并校验主机的全部 `A`/`AAAA` 记录，实际连接再由自定义 resolver 解析一次；两次解析之间仍存在 DNS 重绑定窗口，严格防护需要系统级 DNS 固定或出口代理。私有地址、回环地址和组播地址默认受限。
- 适配器 GET 构造额外拒绝私有 IP 字面量；YouTube 字幕、Bilibili 封面及视频候选地址补上校验。
- CDP 在导航前启用 `Fetch` 拦截并确认成功，检查当前页面会话中的导航、重定向及子资源请求；等待页面渲染期间继续处理拦截事件。截图先创建空白页再导航。`allow_private_addresses` 显式传入浏览器配置。
- 新增本地凭据库（`tkless auth set|list|remove`）：写入采用临时文件+rename，Unix 下权限 `0600`；单 Cookie ≤ 64 KiB、最多 32 个平台；错误信息与日志不回显 Cookie；库只按平台名查询，仅在未提供 `--cookie`/`--cookie-file` 时使用。（2026-10-31 复审补充：`tkless login` / MCP `login_platform` 写出的 `cookies.txt` 也已改为临时文件+rename，Unix 下 `0600`。）
- 新增可选 Cookie Jar（`--cookie-jar` / `TKLESS_COOKIE_JAR`）：保存 `Set-Cookie` 到 JSON，仅回放至**精确主机**且满足路径、过期与 `Secure` 规则；`Domain` 属性被忽略，避免在缺少公共后缀列表时发生跨域 cookie 注入。Jar 限制 1024 条、单条值 4 KiB、文件 4 MiB；损坏文件启动时报错，不静默丢弃。

## HTML 与下载

- 清理路径统一转义文本和属性，移除危险 URL 协议、事件属性及可执行嵌入元素；Readability 的结果也经过清理。选择器裁剪保持文本语义。
- Raw 模式保留原始 HTML；内容提取器不应被当作通用浏览器沙箱。将任何抓取结果嵌入网站时仍需按展示环境隔离。
- 图片使用 SHA-256 内容哈希命名，通过临时文件发布，避免多个文档共用目录时互相覆盖。单张图片受 `max_bytes` 限制，每页最多处理 128 个唯一图片 URL，累计读取预算为 `8 × max_bytes`。下载失败或超限的图片保留远程链接。
- Range 下载要求 `206`、精确的 `Content-Range` 和完整字节数；失败后终止并等待并发任务，再清理分片、回退单流下载。
- 文件下载完成后才替换目标文件，失败时保留已有文件。

## 资源与缓存

- 适配器 HTML、JSON、字幕等响应统一采用有大小上限的读取方法。结构化 API 超限报错，不将截断 JSON 当作有效结果。Bilibili 弹幕解压后的大小同样受限。
- MCP 单条输入消息上限为 1 MiB（包含换行）；超限或无效 UTF-8 终止该传输并返回错误。读取状态在异步任务切换时保留。
- L1 缓存最多 64 条，单条序列化数据不超过 1 MiB；这不是精确的进程内存上限。较大文档仅使用磁盘缓存，磁盘单条上限 16 MiB，写入采用临时文件替换。
- 缓存键升级为 v4，旧提取结果不再命中；旧磁盘文件仍可通过缓存清理命令删除。
- 含 `skills_page_failed` 的临时分页失败结果不缓存；用户主动设置页数或结果数量造成的截断仍可缓存。
- 可选 `--retry-budget N` 为整个运行设置共享重试上限（默认不限）；预算耗尽后不再重试，直接返回当次错误，避免故障站点拖长任务。
- 可选 `--circuit-breaker N` 按主机熔断：连续 N 次传输错误、超时、5xx 或 429 后冷却 30 秒，期间本地拒绝并返回 `circuit_open`；冷却后仅放行一个半开探测，再次失败将冷却翻倍至 5 分钟。默认禁用，避免改变既有重试行为。
- Office 归档只在扩展名或媒体类型匹配时解析，普通 ZIP 不会进入该路径。读取限制 512 条目、单条目 32 MiB、总未压缩 64 MiB，拒绝加密、ZIP64 与未知压缩方法；仅读取固定的 XML 部件，不执行宏、外部关系、嵌入对象或任何脚本。

## 2026-10-31 复审修复

本轮针对 [代码审计报告](13-code-audit.md) 中的安全项完成以下修复：

- **请求头作用域**：自定义头改为白名单。仅 `Accept`、`Accept-Language`、`Referer`、`Sec-*` 等非凭据头进入客户端默认头；其余自定义头（包括 `Private-Token`、`X-Goog-Api-Key`、`X-Amz-Security-Token` 等）仅在 `initial_target_origin` 匹配时发送。未设置 `initial_target_origin` 的调用（例如部分搜索/批量路径）不会发送非白名单自定义头；需要携带自定义头的场景请使用单 URL 提取或显式设置 origin 的库调用。
- **Cookie 作用域**：无前缀 `--cookie` 不再按 apex 域放行，改为精确主机、同一凭据平台或初始主机子域；凭据平台选择比适配器识别更窄，`bilivideo.com` 不再获得 B 站会话，`qq.com` 不再整体映射为微信，仅 `mp.weixin.qq.com` / `weixin.qq.com`。显式提供 `--cookie` 或 `--cookie-file` 时不再回退到凭据库。
- **凭据文件**：`save_platform_cookies_to_file` 改为同目录临时文件 + `rename`，Unix 下 `0600`，并新增权限回归测试。
- **MCP HTTP**：回环绑定也校验 `Host`（必须为回环名称/IP 且端口匹配）、`Origin`（存在时必须为同端口回环来源）、`Content-Type: application/json`；Bearer 比较改为定长遍历；已完成任务增加 `MAX_RETAINED_JOBS = 64` 上限。
- **SSRF / 下载器**：补齐 `192.88.99.0/24`、NAT64 `64:ff9b::/96`、6to4 `2002::/16`、IPv4-translated、`fec0::/10` 判定；`download_file` 拒绝非 HTTP(S) 与私网 IP 字面量，累计大小上限 16 GiB，Range 数学改为饱和运算，`DownloadOptions` 新增 `allow_private_addresses`。
- **PDF**：HTTP 路径的 PDF 解析移入 `spawn_blocking`，增加 60 秒超时；frontmatter 使用 YAML 转义，标题换行归一化。
- **CDP**：自动浏览器配置目录改为每进程随机路径并在 Unix 下 `0700`；本地端点判定改为 URL 主机精确解析。
- **资源限制**：`Retry-After` 单次上限降为 60 秒；Cookie Jar 拒绝值内 `;` 与 `,`；缓存维护对单条目 16 MiB 上限，`clear()` 只删除 `*.json` 而非递归删除目录。

验证入口：`cargo test --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` 均已通过。

### 本轮未消除的边界

- 升级已改为 Minisign 签名清单（`SHA256SUMS` + `SHA256SUMS.minisig`），发布签名在受保护 `release-signing` Environment 中执行。仓库仍需把真实公钥写入 `crates/tkless-cli/assets/tkless-upgrade.pub` 并配置 `MINISIGN_SECRET_KEY`/`MINISIGN_PASSWORD`；未配置时升级会 fail closed。
- MCP HTTP 回环绑定仍允许本机进程无 Token 调用；本轮阻断的是浏览器跨源/DNS rebinding，本机恶意进程不在防护模型内。
- PDF 解析器在 `panic = "abort"` 的 release 配置下仍可能因恶意输入终止进程；60 秒超时只能限制挂起，不能回收阻塞线程。
- 代理与 CDP 浏览器的实际出口 IP 仍不受预检查约束；严格隔离需要出口代理或系统级网络隔离。

## 尚未覆盖的边界

- HTTP 代理和浏览器可能自行解析目标 DNS。当前 CDP 拦截并不约束浏览器实际连接的 IP，也未覆盖独立 worker、跨进程 iframe 或 WebSocket 等所有网络路径。严格隔离不可信页面仍需要浏览器出口代理或系统级网络隔离；本轮不能宣称完全消除了浏览器 SSRF。
- `allow_private_addresses=true` 明确放开私网访问；仅应在可信任务中使用。
- 已评估并舍弃任务级下载总配额与缓存容量淘汰：单文件仍受 `max_bytes` 约束，爬虫使用 `--max-output-bytes`，缓存可用 `manage_cache` 清理。
- 代理池、OTLP、S3/对象存储与数据库 sink、OCR/ASR/XLSX 等已明确舍弃，不再作为待办；决策记录见 [`doc/12` §9.1](12-optimization-plan.md#91-已舍弃的候选功能2026-10-31-决策)。
- 熔断仅在本进程内生效且按主机名区分，不区分端口、代理出口或 CDN；DNS 轮换到同一主机的其他地址也会一并熔断。MCP 单次 `fetch_webpage` 会新建引擎，因此其熔断状态不跨调用累积，批量与爬虫任务内共享。
- 凭据库为明文文件（依赖文件权限），未提供加密或系统密钥环集成；Cookie Jar 支持持久化但仅限精确主机，不实现跨子域共享、公共后缀判别或会话失效自动刷新。
- Office 解析不渲染格式、图片、图表、批注或修订；超出大小上限的文档直接报错，不会部分静默降级。XLSX 未实现，需要先转为 CSV。
- 本轮未完成第三方依赖 CVE 扫描，不据此认定依赖无漏洞。

## 验证入口

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p tkless-core --no-default-features --all-targets
cargo test -p tkless-core --test bing_cdp_test -- --ignored
```

最后一项需要本地 Chrome/Edge，使用临时配置启动无头浏览器，验证页面提取及任务标签页清理。其他安全回归覆盖跨源凭据、HTML 注入文本、图片重名和超限、Range 回退、已有文件保护、MCP 超长消息、缓存上限及 CDP 请求拦截协议。

本轮结果：288 项工作区测试通过；默认忽略的真实浏览器测试单独执行通过，覆盖提取、PNG 截图和标签页清理；严格 Clippy、无默认特性编译、格式检查及 Release 构建通过。新 Release 可执行文件重新运行三个原始攻击样例，确认跨主机凭据未转发、两张图片分别保留、编码后的 HTML 仍是文本。沙箱内 Chrome 曾因 GPU 子进程权限失败，真实浏览器测试改在沙箱外使用临时配置完成。
