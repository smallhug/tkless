# 代码审计报告：安全漏洞、逻辑缺陷与 CLI/文档规范

## 文档信息

| 项目 | 内容 |
| :--- | :--- |
| 审查日期 | 2026-10-31 |
| 审查对象 | `tkless-core`、`tkless-cli` 及 `README.md`、`doc/` |
| 审查方式 | 静态代码审查；逐条核对实现与文档 |
| 验证状态 | 审查阶段为静态审查；2026-10-31 安全与逻辑修复后已通过 `cargo test --workspace --all-targets`、严格 Clippy 与 `cargo fmt --check` |
| 关联文档 | [09-security-hardening.md](./09-security-hardening.md)、[04-development-standards.md](./04-development-standards.md) |

## 1. 结论摘要

本次审查共整理 47 项问题：安全 13 项、逻辑 16 项、CLI 规范 9 项、文档一致性 9 项。其中需要优先处理的是：

1. MCP HTTP 回环服务默认无认证且不校验 `Host`/`Origin`，同时暴露文件写入与内网访问工具。
2. 自定义请求头采用 7 项黑名单，`Private-Token`、`X-Goog-Api-Key` 等凭据头会发往所有主机。
3. `--cookie` 按 apex 域放行，凭据库按硬编码平台映射回放，存在跨子域/跨业务域泄露。
4. 登录写出的 `cookies.txt` 非原子写入且 Unix 下默认 `0644`。
5. 搜索查询包含 `"20年"` 等中英混合 token 时会 panic。
6. RSS/Atom 分支绕过 `--max-tokens`、`--chunk`、`--summarize`，核心预算保证失效。
7. 根选项放在子命令前会被静默忽略，`--format` 暴露实际不支持的值，退出码与 README 不一致。
8. README/doc 中存在多个不存在的别名与参数（`platforms`、`update`、`--once`、`--history`）。

## 1.1 安全项修复状态（2026-10-31）

| ID | 状态 | 落地说明 |
| :--- | :--- | :--- |
| SEC-01 | 部分修复 | 增加 `Host`、`Origin`、`Content-Type` 校验；完成任务保留上限 64。任意路径写入限制尚未实现，回环服务仍信任本机进程。 |
| SEC-02 | 已修复 | 自定义头改为白名单，非默认头仅在 `initial_target_origin` 匹配时发送。 |
| SEC-03 | 已修复 | 移除 apex 域回退；引入更窄的凭据平台映射；显式 Cookie 来源存在时不再回退凭据库。 |
| SEC-04 | 已修复 | `cookies.txt` 改为临时文件 + rename，Unix 下 `0600`，并增加权限测试。 |
| SEC-05 | 已实现（待配置密钥） | 升级器改为验证 `SHA256SUMS` + `SHA256SUMS.minisig`，内置 Minisign 公钥；发布流水线在受保护 `release-signing` Environment 中签名。仓库需替换公钥占位文件并配置 Environment Secrets 后生效。 |
| SEC-06 | 已修复 | 补齐 IPv6 转换前缀与 `192.88.99.0/24`；下载器增加协议/私网字面量检查、16 GiB 上限与饱和 Range 运算；`DownloadOptions` 新增 `allow_private_addresses`。 |
| SEC-07 | 部分修复 | HTTP 路径改为 `spawn_blocking` + 60 秒超时；同步路径与 `panic = "abort"` 下的解析器 panic 仍存在。 |
| SEC-08 | 已修复 | CDP 配置目录改为每进程随机路径并设置 `0700`；端点判定改为 URL 主机精确解析。 |
| SEC-09 | 已修复 | 新增 `MAX_RETAINED_JOBS = 64`，按序号淘汰最旧的非活动任务。 |
| SEC-10 | 已修复 | `Retry-After` 单次上限由 300 秒降为 60 秒。 |
| SEC-11 | 已修复 | Cookie 值拒绝 `;` 与 `,`。 |
| SEC-12 | 已修复 | 缓存维护对单条目应用 16 MiB 上限；`clear()` 仅删除 `*.json`，不再递归删除目录。 |
| SEC-13 | 已修复 | PDF frontmatter 使用 YAML 转义，标题换行归一化。 |

### 1.2 逻辑项修复状态（2026-10-31）

| ID | 状态 | 落地说明 |
| :--- | :--- | :--- |
| LOG-01 | 已修复 | 年份前缀按字符边界处理，`20年`/`20日` 不再 panic，新增回归测试。 |
| LOG-02 | 已修复 | RSS/Atom 接入统一 `finalize_content` 管线，预算、分块、摘要、哈希与指标不再被绕过。 |
| LOG-03 | 已修复 | robots 缓存锁不再跨网络请求持有。 |
| LOG-04 | 已修复 | `--selector` 零匹配返回错误；嵌套匹配只渲染最外层。 |
| LOG-05 | 已修复 | 数学占位符按 id 降序恢复，≥10 个公式不再前缀冲突。 |
| LOG-06 | 已修复 | 同步状态保留最近 64 个事件并回收未引用的快照对象，预算检查前先 GC。 |
| LOG-07 | 已修复 | 通知载荷构建失败写入 `notify_errors`，不再中止整轮 watch。 |
| LOG-08 | 已修复 | research 严格执行 3–20 页预算；Hop 2 复用 SimHash 去重；Markdown 截断设置 `truncated`。 |
| LOG-09 | 已修复 | 表头按最大列数补齐；`headers` 与 records/CSV/TSV 使用同一套清洗后的列名。 |
| LOG-10 | 已修复 | GitHub 适配器只为已识别的仓库路由匹配；保留路径/登录/设置页走通用抽取；releases 选择 release 条目并尝试使用发布标题。 |
| LOG-11 | 已修复 | 模板占位符拒绝带空格写法；ItemLimit 发生在页中时不再把当前页作为可续跑 URL。 |
| LOG-12 | 已修复 | chunk_size 不再静默放大；分节记录代码围栏类型并使用调用方 tokenizer；小数不再被拆成句子。 |
| LOG-13 | 已修复 | 爬虫预算触发前先提交已抓取页面，避免丢弃并在续跑时重复抓取。 |
| LOG-14 | 部分修复 | CDP 路径使用真实 `final_url`；主文档 HTTP 状态仍未从 CDP 获取，继续沿用 200 标记。 |
| LOG-15 | 已修复 | 无特征文本使用原始文本哈希，不再全部折叠为 0。 |
| LOG-16 | 未修复 | 检查点 `fsync` 仍在异步任务内同步执行。尝试移入 `spawn_blocking` 会延长文件锁生命周期并导致立即续跑竞态，暂保留原实现。 |

### 1.3 CLI 规范与文档一致性修复状态（2026-10-31）

| ID | 状态 | 落地说明 |
| :--- | :--- | :--- |
| CLI-01 | 已修复 | 根 `--format`/`-o` 由结构化子命令继承；对不支持根格式/输出的命令以及子命令上的 `--stats` 给出显式告警，不再静默忽略。 |
| CLI-02 | 已修复 | `batch`/`crawl`/`diff`/`adapters` 使用受限 `value_parser`；`research` 只接受 markdown/json；根格式在子命令执行前校验。 |
| CLI-03 | 已修复 | `--config` 统一为 `-C`，`--concurrency` 为 `-j`，`research --max-pages` 为 `-P`；移除 `cache --clear`/`upgrade --force` 的短选项，消除 `-c`/`-m`/`-f` 语义冲突。 |
| CLI-04 | 已修复 | 在 clap 层校验搜索/采集/调研数量、`--max-size`、超时、`--cache-ttl`、`--max-tokens`、B 站页数/条数、`--since`/`--until` 日期与 SQL 表名；`--delay-ms` 保留批处理 0ms 与公众号 300ms 的场景差异，均由 clap 显示默认值。 |
| CLI-05 | 已修复 | 声明 `--screenshot`/`--pdf`、`--out-dir`/`-o` 互斥，`--chunk-size`/`--chunk-overlap`/`--assets-dir`/`--mcp-dynamic` 依赖；`--cdp-wait` 在单页、搜索、批量、爬取路径执行前校验。 |
| CLI-06 | 已修复 | 缺少 TARGET、URL 解析失败与 `auth` 参数错误返回 2；`--stats` 改写到 stderr；`--log-format json` 覆盖致命错误；README 补充 `verification_required`。 |
| CLI-07 | 已修复 | `doctor` 实际执行缓存目录写入/读取/删除探测，汇总网络与缓存失败并返回非零退出码。 |
| CLI-08 | 已修复 | `fetch_webpage` 增加 `jsonl` 与 `max_size_mb`；`batch_fetch_webpages` 增加链接/表格/去图去链参数；`deep_research` 增加 `proxy`；MCP 缓存默认改为 opt-in。 |
| CLI-09 | 已修复 | 修正 `--download-video`/`--download-cover` 提示；`--bili-order` 改为 `ValueEnum`。 |
| DOC-01 | 已修复 | 删除 README 中不存在的 `platforms`/`update` 别名。 |
| DOC-02 | 已修复 | 删除 `watch --once` 示例，明确省略 `--interval` 即单轮。 |
| DOC-03 | 已修复 | 将 `wechat --history`/`session_required` 移入「计划中」，声明当前未实现。 |
| DOC-04 | 已修复 | README、doc/02、`cli.rs`、RELEASE_NOTES 统一为 31 个业务工具。 |
| DOC-05 | 已修复 | README 补充 `jsonl` 与根级 `--retry-budget`；doc/03 修正 `--chunk` 输出与 REPL 命令列表。 |
| DOC-06 | 已修复（降低规范要求） | doc/04 明确根命令必须分组、子命令按规模补齐；`value_name` 对路径/数量类参数强制，布尔开关可省略。 |
| DOC-07 | 已修复 | doc/09 改为说明预检查与连接 resolver 的两次解析及 DNS 重绑定窗口；凭据库回退描述与 SEC-03 实现一致。 |
| DOC-08 | 已修复 | doc/01 与当前 `release.yml` 对齐；doc/06 补充 `verification_required`、`search_parse_error`、`rate_limited` 的退出码。 |
| DOC-09 | 已修复 | 版本统一为 0.6.0，CHANGELOG 的 Unreleased 更名为 0.6.0 并补充 CLI 变更记录。 |

验证：`cargo test --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` 均已通过。新增 `tests/cli_contract_test.rs` 覆盖退出码、`--stats` 输出与根格式继承。

验证：`cargo test --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` 均已通过。`DownloadOptions` 新增 `allow_private_addresses` 字段，库调用者需要显式设置（B 站适配器已传递 Fetcher 配置）。

## 2. 严重度定义

| 级别 | 含义 |
| :--- | :--- |
| 严重 | 在常见配置下可导致凭据泄露、任意文件写入或进程崩溃，应优先修复 |
| 高 | 明确的错误行为或安全边界缺失，需要环境配合但影响显著 |
| 中 | 特定输入/平台/并发条件下出现的正确性、性能或安全问题 |
| 低 | 边界情况、体验问题或防御纵深建议 |

---

## 3. 安全漏洞

### SEC-01 MCP HTTP 回环服务无认证且不校验来源

- **严重度**：严重（仅在启用 `mcp --http` 时）
- **位置**：`crates/tkless-core/src/mcp/server/http.rs:29-37`、`:46-58`、`:109-137`
- **问题**：`HttpTransportOptions::default()` 的 `auth_token` 为 `None`；只有非回环绑定才强制 Token。请求处理只检查 method/path/Bearer，从未检查 `Host`、`Origin`、`Referer` 或 `Content-Type`。
- **影响**：启用 `--http` 后，本机任意进程均可调用全部工具；浏览器页面可通过 DNS rebinding 或对回环地址发送 `text/plain` 的简单 POST（不触发 CORS 预检）调用高权限工具。可写入任意路径的工具包括：
  - `crates/tkless-core/src/mcp/server/inspection.rs:294-299`（截图 `output_path`）
  - `crates/tkless-core/src/mcp/server/auth.rs:14-35`（`output_cookie_file`）
  - `crates/tkless-core/src/mcp/tools.rs:472-477`（`download_media.destination`）
  - `crates/tkless-core/src/mcp/tools.rs:337-340`（`sync_collection.state_dir`）
- **补充**：`fetch_webpage` 公开 `allow_private_addresses`（`crates/tkless-core/src/mcp/tools.rs:391-397`），可直接访问 localhost/内网。
- **建议**：
  1. 回环绑定也默认要求 Token，或启动时生成一次性随机 Token。
  2. 校验 `Host`/`Origin` 白名单。
  3. 将 `output_path`/`destination`/`state_dir` 规范化并限制在配置的 workspace 根目录内。
  4. 要求 `Content-Type: application/json`。

### SEC-02 自定义敏感头黑名单不完整

- **严重度**：高
- **位置**：`crates/tkless-core/src/fetcher.rs:69-76`、`:861-872`
- **问题**：`--header` 中除敏感头外全部进入 `Client::default_headers`；敏感头仅 7 项：`authorization`、`proxy-authorization`、`cookie`、`cookie2`、`x-api-key`、`api-key`、`x-auth-token`。
- **影响**：`Private-Token`、`X-Goog-Api-Key`、`X-Subscription-Key`、`X-Amz-Security-Token`、`X-RapidAPI-Key`、`X-Vault-Token` 等会随搜索、重定向、图片/媒体下载发往第三方主机。CDP 抓包复用同一判定（`crates/tkless-core/src/cdp.rs:839-848`）。
- **建议**：改为“白名单 + 显式 origin 绑定”；凭据类头仅在 `initial_target_origin` 匹配时发送。

### SEC-03 Cookie 作用域按 apex 域与平台映射放行

- **严重度**：高
- **位置**：`crates/tkless-core/src/fetcher.rs:286-303`、`:1037-1067`、`:1070-1128`、`:312-317`
- **问题**：
  - 无前缀 `--cookie` 只要请求主机是初始主机 apex 域的子域就放行。
  - `extract_apex_domain` 仅硬编码 `.co.uk` 等少数双段后缀，没有 Public Suffix List。
  - 无显式 Cookie 时按 `detect_platform_from_url` 从凭据库取；`bilibili` 覆盖 `bilivideo.com`，`wechat` 覆盖整个 `qq.com`。
- **影响**：抓取 `https://myapp.vercel.app` 并带 `--cookie` 时，重定向到 `https://attacker.vercel.app` 仍会发送 Cookie；`github.io`、`herokuapp.com`、`pages.dev`、`s3.amazonaws.com` 同理。抓取任意 `*.qq.com` 或 `*.bilibili.com` 页面会使用凭据库中的对应平台会话。
- **建议**：引入 PSL 或至少禁止公共后缀；无前缀 Cookie 默认只发往精确主机；平台映射限定为初始主机所属平台。

### SEC-04 登录 Cookie 文件非原子写入且默认权限过宽

- **严重度**：高
- **位置**：`crates/tkless-core/src/fetcher.rs:1223`、`crates/tkless-core/src/auth.rs:457-461`
- **问题**：`save_platform_cookies_to_file` 使用 `std::fs::write`，无临时文件、无 `0600`、先截断后写入。登录流程正是通过该函数保存 B 站会话。
- **影响**：Unix 多用户机器上生成 `0644` 的 `cookies.txt`，其他本地用户可读取 `SESSDATA`、`bili_jct`、`z_c0`；写入中途崩溃会丢失全部已保存平台 Cookie。`doc/09-security-hardening.md:12` 的“临时文件+rename，Unix 0600”对该路径不成立。
- **建议**：复用 `state_file::write` 的原子写与权限逻辑；Windows 下至少使用继承目录 ACL 并避免写入共享目录。

### SEC-05 升级缺少独立签名且安装前执行候选二进制

- **严重度**：中
- **位置**：`crates/tkless-cli/src/upgrade.rs:101-122`、`:129-135`；`crates/tkless-cli/src/upgrade/install.rs:189-190`
- **问题**：SHA-256 来自同一 Release 的 `asset["digest"]` 或同源 `<asset>.sha256`；macOS 解包 DMG 后重新计算二进制哈希再校验，属于自证循环。安装器在替换前调用 `probe_version` 执行候选二进制。
- **影响**：哈希只证明元数据与二进制一致，不能证明发布者可信；任何能控制 Release 的人都能获得代码执行，回滚也无法避免一次执行。
- **建议**：使用 minisign/cosign 对校验文件签名并将公钥内置；安装前改为静态版本解析，或明确提示“即将执行未签名二进制”。

### SEC-06 SSRF 覆盖盲区与下载器缺陷

- **严重度**：中
- **位置**：`crates/tkless-core/src/fetcher.rs:905-935`；`crates/tkless-core/src/downloader.rs:42-78`、`:166-171`、`:195-198`
- **问题**：
  - IPv6 仅覆盖 IPv4-mapped、loopback、unspecified、multicast、ULA、link-local；未覆盖 NAT64 `64:ff9b::/96`、6to4 `2002::/16`、IPv4-translated、site-local `fec0::/10`。
  - IPv4 未覆盖 `192.88.99.0/24`、`198.18.0.0/15`（测试还显式认为其公网可访问，见 `fetcher.rs:1681`）。
  - `download_file` 不调用 `validate_ssrf`、不限制累计大小；直接信任 `Content-Range` 的 total；`(i + 1) * chunk_size - 1` 在极大 total 下会溢出 `u64`。
- **影响**：在启用 NAT64/6to4 等环境可绕过私网限制；恶意 CDN 可声称超大对象导致溢出或长时间下载直至磁盘耗尽。代理与 CDP 的 DNS/连接由代理或浏览器完成，现有预检查不能约束实际出口。
- **建议**：使用成熟的 IP 分类库补齐特殊段；`download_file` 增加 URL 校验与最大字节数；Range 数学使用 `checked_mul`/`saturating` 并限制 total 上限。

### SEC-07 PDF 解析同步执行且缺少隔离

- **严重度**：中
- **位置**：`crates/tkless-core/src/lib.rs:514-520`、`crates/tkless-core/src/pdf.rs:69-74`、`:94-97`
- **问题**：在 async 路径直接调用 `lopdf`/`pdf-extract` 同步解析并解压流，没有 `spawn_blocking`、超时或 `catch_unwind`。
- **影响**：恶意 PDF 可阻塞 tokio worker、放大内存或触发解析库 panic。Office 路径已有条目/大小/总解压预算，PDF 没有同等保护。
- **建议**：放入 `spawn_blocking`，增加超时与解压总量限制，并隔离第三方解析器 panic。

### SEC-08 CDP 临时目录可预测且端点判定使用子串

- **严重度**：中
- **位置**：`crates/tkless-core/src/cdp.rs:179-180`、`:139`、`:545-567`
- **问题**：临时配置目录为 `temp_dir()/tkless_cdp_profile_{port}`，创建失败被忽略；本地端点判定使用 `contains("127.0.0.1") || contains("localhost")`。
- **影响**：多用户 Unix 下可预创建同名目录影响浏览器配置；`http://127.0.0.1.attacker.tld:9222` 会被误判为本地端点，用户 Cookie 会被推送过去。
- **建议**：使用随机高熵目录并校验权限；用 URL 主机精确比较替代子串判断。

### SEC-09 MCP 已完成任务保留时间过长

- **严重度**：低-中
- **位置**：`crates/tkless-core/src/mcp/server/jobs_registry.rs:274-297`、`:339-343`
- **问题**：只限制活动任务数（`MAX_ACTIVE_JOBS = 4`），已完成任务保留到 TTL（最长 24 小时），每次 reserve 仅清理过期项。
- **影响**：客户端可串行完成大量 `crawl_site`/`batch_fetch_webpages` 任务，每个任务保留完整结果 JSON，造成内存膨胀。
- **建议**：增加已完成任务数量上限或结果总字节上限，按 LRU 淘汰。

### SEC-10 Retry-After 可拉长任务

- **严重度**：低
- **位置**：`crates/tkless-core/src/fetcher.rs:891-903`
- **问题**：`Retry-After` 单次最多 300 秒，默认重试预算无限（`fetcher.rs:100`）。
- **影响**：恶意站点配合 `--retries N` 可将任务拖长数小时。
- **建议**：增加单次任务总等待上限，或在共享预算中计入等待时间。

### SEC-11 Cookie Jar 允许值内分号

- **严重度**：低
- **位置**：`crates/tkless-core/src/cookie_jar.rs:300-308`
- **问题**：`valid_cookie` 拒绝控制字符但不拒绝 `;`，而 `parse_set_cookie` 会拒绝。手工修改的 jar 可注入额外 Cookie 对。
- **建议**：加载与回放时统一拒绝 `;`、`,`、空白等分隔字符。

### SEC-12 缓存维护无大小上限

- **严重度**：低
- **位置**：`crates/tkless-core/src/cache.rs:250-252`、`:280`
- **问题**：维护/统计时对任意 `*.json` 无大小上限反序列化；`clear()` 会 `remove_dir_all` 配置目录。
- **影响**：共享缓存目录下可造成内存尖峰；`--cache-dir` 指向宽泛路径时存在误删风险。
- **建议**：维护路径复用 `get` 的 16 MiB 上限；`clear()` 增加目录安全校验。

### SEC-13 PDF frontmatter 转义不完整

- **严重度**：低
- **位置**：`crates/tkless-core/src/pdf.rs:165-176`
- **问题**：只转义双引号，标题以 `\` 结尾或含换行可破坏 YAML frontmatter；标题还会原样进入 Markdown 标题。
- **建议**：使用 YAML 序列化器生成 frontmatter，或至少转义反斜杠与换行。

---

## 4. 逻辑与正确性缺陷

### LOG-01 搜索查询 panic：`"20年"` 触发非字符边界切片

- **严重度**：高
- **位置**：`crates/tkless-core/src/search.rs:519-526`
- **问题**：`t.len()` 是字节长度；`"20年"` 为 5 字节，`t[..4]` 落在 `年` 的 3 字节编码中间，触发 `panic: byte index 4 is not a char boundary`。`tokenize_query` 被 `apply_search_scope`（`search.rs:311-316`）和 `parse_bing_page`（`search.rs:1318-1324`）调用。
- **影响**：`tkless search "20年"` 会直接崩溃。
- **建议**：使用 `char_indices`/`chars().take(4)` 截取，并补充中文日期回归测试。

### LOG-02 RSS/Atom 分支绕过内容后处理

- **严重度**：高
- **位置**：`crates/tkless-core/src/lib.rs:726-771`
- **问题**：feed 分支在通用流程前直接返回：不执行 `max_tokens` 截断（`lib.rs:937-953`）、不生成 `chunks`（`lib.rs:961-970`）、不应用 `summarize`/`links`/`images`，`truncated` 硬编码为 `false`。
- **影响**：`is_feed_xml` 自动识别 feed，即使用户未传 `--feed` 也会命中；核心 Token 预算保证对任何 feed URL 失效。
- **建议**：feed 仅负责解析为 Markdown，随后进入统一后处理管线；或在 feed 分支补齐预算、分块与摘要。

### LOG-03 robots.txt 互斥锁跨网络请求持有

- **严重度**：中
- **位置**：`crates/tkless-core/src/crawler/job.rs:396-407`
- **问题**：全局 `tokio::Mutex` 在发起 HTTP 请求前持有，直到请求完成才释放。
- **影响**：慢主机或超时可阻塞所有主机的 robots 检查，使 `--concurrency N` 在预检阶段退化为 1。
- **建议**：锁只用于缓存读写；释放锁后请求，再回写缓存（可用 per-host 单飞）。

### LOG-04 `--selector` 零匹配静默成功且嵌套匹配重复

- **严重度**：中
- **位置**：`crates/tkless-core/src/cleaner.rs:178-202`
- **问题**：`matched_count == 0` 仍返回 `Ok` 与空 `html`；`process_html` 继续生成 `content=""`、`error=None` 的文档。`document.select()` 同时返回嵌套的祖先与后代，导致内容重复。
- **影响**：选择器拼写错误会被批量/爬虫记录为成功；嵌套选择器输出重复正文。与 `schema.rs:273-293` 的零匹配报错行为不一致。
- **建议**：零匹配返回明确错误；过滤 `select` 结果中互为祖先/后代的重叠节点。

### LOG-05 数学占位符前缀冲突

- **严重度**：中
- **位置**：`crates/tkless-core/src/markdown.rs:202-216`
- **问题**：占位符为 `TKLESSMATHBLOCK{id}` / `TKLESSMATHINLINE{id}`。`TKLESSMATHBLOCK1` 是 `TKLESSMATHBLOCK10` 的前缀，按 id 升序替换时 id=1 会先替换掉 id=10 token 的前半部分。
- **影响**：页面包含 ≥11 个公式时第 10 个公式被替换为第 1 个，并残留 `0`。
- **建议**：占位符加分隔符（如 `TKLESSMATHBLOCK_{id}_`），或按 id 降序替换。

### LOG-06 增量同步历史从不回收

- **严重度**：中（长期 `watch` 场景为高）
- **位置**：`crates/tkless-core/src/sync.rs:184-205`
- **问题**：每代写入新的 `events/{n}.json` 与 `objects/*.json`，没有删除旧历史的逻辑；`directory_bytes` 统计全部历史，超过 `max_state_bytes` 后每次运行都在“未推进 manifest”处失败。
- **影响**：状态目录达到预算后永久失败，只能手工删除。
- **建议**：保留最近 N 代/时间窗口 GC；或在预算检查前删除旧 generation，并提供可恢复的错误信息。

### LOG-07 单个超大通知载荷中止整轮 watch

- **严重度**：中
- **位置**：`crates/tkless-core/src/watch.rs:256-257`
- **问题**：`notification_payload(...)?` 在载荷超限时直接逃出 `run_once`，已完成的 job 报告被丢弃，后续 job 不执行。
- **影响**：一轮采集因一个通知失败而整体失败，且 `JobReport.notify_errors` 设计未被使用。
- **建议**：将载荷构建错误写入 `notify_errors`，继续其他 job。

### LOG-08 research 预算、去重与截断标记不一致

- **严重度**：中
- **位置**：`crates/tkless-core/src/research.rs:309`、`:424-435`、`:527`、`:543-548`、`:227-231`
- **问题**：
  - `max_pages.min(8)`，但 CLI 帮助写 3–20（`cli.rs:1253-1255`），核心只拒绝 0。
  - Hop 2 只用 URL 规范化去重，没有 Hop 1 的 SimHash 近重复过滤。
  - `truncated` 恒为 `false`；Markdown 截断后也不设置标记。
- **影响**：用户请求 20 页最多得到 8 + 3 页；镜像/转载页膨胀引用；消费者无法判断报告被截断。
- **建议**：按声明范围校验并生效 `max_pages`；Hop 2 复用近重复过滤；截断时设置 `truncated`。

### LOG-09 表格 Markdown 表头未补齐，CSV 表头与 records 不一致

- **严重度**：中
- **位置**：`crates/tkless-core/src/model.rs:530-548`；`crates/tkless-core/src/extractor.rs:466-496`
- **问题**：`col_count` 取表头与行的最大值，但表头未补齐到 `col_count`，分隔行却是 `col_count` 列；`headers` 保存原始值，`records` 使用 `clean_headers` 生成的 `col_N`。
- **影响**：ragged table 生成非法 GFM；CSV/TSV 表头为空而记录使用 `col_1`。
- **建议**：表头补齐到 `col_count`；CSV 与 records 使用同一套清洗后的表头。

### LOG-10 GitHub 适配器吞掉所有 github.com 路径

- **严重度**：中
- **位置**：`crates/tkless-core/src/adapters/github.rs:105-131`、`:164-167`
- **问题**：任意第三个路径段都回退为 `Repo { owner, repo }`；解析失败时伪造 `owner: "GitHub", repo: "Repository"`。`/releases` 也走 README 抽取。
- **影响**：`github.com/login`、`/settings/tokens`、`/orgs/x/people` 被当作仓库页，标题错误并抑制通用抽取；发布说明无法正确解析。
- **建议**：仅为合法的 `/{owner}/{repo}` 形态启用适配器，其余返回 `None`；为 releases 实现独立解析。

### LOG-11 采集模板与续跑指针错误

- **严重度**：中
- **位置**：`crates/tkless-core/src/collection.rs:162-179`、`:126-132`、`:684-687`
- **问题**：
  - 校验时 trim 占位符，允许 `{{ page }}`；但替换只匹配 `{{page}}`，带空格的模板永远不会被替换。
  - `ItemLimit` 时 `next_url` 指向当前已消费页面，续跑会重复输出该页条目。
- **建议**：校验时拒绝带空格占位符（或替换时统一 trim）；`ItemLimit` 保存下一页或页内游标。

### LOG-12 分块、分节与摘要细节问题

- **严重度**：低-中
- **位置**：`crates/tkless-core/src/chunker.rs:31-32`；`crates/tkless-core/src/splitter.rs:44-45`、`:62-63`、`:95`；`crates/tkless-core/src/summarizer.rs:169-176`
- **问题**：
  - `target_chunk_tokens.max(50)` 静默放大请求，`--chunk-size 20` 实际得到 50。
  - 分节只用一个布尔值记录代码块状态，混用反引号围栏与 `~~~` 围栏会错乱。
  - 分节 `token_count` 固定使用 `TokenizerKind::default()`，与用户 `--tokenizer` 不一致。
  - 摘要器 `'.'` 分支无条件切分，`3.14` 会被拆开并可能被长度阈值丢弃。
- **建议**：校验 `chunk_size` 并与用户参数一致；记录代码围栏字符；分节使用传入 tokenizer；修正小数切分。

### LOG-13 爬虫预算丢弃已抓取页面

- **严重度**：低
- **位置**：`crates/tkless-core/src/crawler/job.rs:282-299`
- **问题**：队列/输出预算的 `break` 发生在 `pending.remove` 与 `processed += 1` 之前。
- **影响**：已成功抓取并提取的页面被丢弃，续跑时重复抓取；`stop_reason` 掩盖了已完成页面。
- **建议**：先提交当前页面，再判断是否停止。

### LOG-14 CDP 分支伪造 200 状态

- **严重度**：低
- **位置**：`crates/tkless-core/src/crawler/job.rs:384-388`；`crates/tkless-core/src/collection.rs:733-745`；`crates/tkless-core/src/lib.rs:588-594`
- **问题**：CDP 路径返回合成 `200` 与原始 URL。
- **影响**：404/410 页面被记为成功；客户端重定向页面的相对链接按原 URL 解析。
- **建议**：从 CDP 响应中读取真实状态与最终 URL。

### LOG-15 空/短文本 SimHash 冲突

- **严重度**：低
- **位置**：`crates/tkless-core/src/dedup.rs:10-14`
- **问题**：无特征文本统一返回 `0`，两个不相关短页面会被判定为近重复并被丢弃。
- **建议**：特征为空时返回唯一值或跳过去重。

### LOG-16 checkpoint 同步 I/O 阻塞异步执行器

- **严重度**：低
- **位置**：`crates/tkless-core/src/batch.rs:158-163`；`crates/tkless-core/src/batch/checkpoint.rs:168-185`
- **问题**：在异步任务中持有 `std::sync::Mutex` 执行 `write_all` + `fsync`。
- **影响**：每次检查点写入都会阻塞 worker，削弱并发收益。
- **建议**：使用 `tokio::task::spawn_blocking` 写盘，或改为异步文件 I/O。

---

## 5. CLI 命令格式规范

### CLI-01 根选项放在子命令前会被静默忽略

- **严重度**：高
- **位置**：`crates/tkless-cli/src/cli.rs:447-450`、`:1376-1380`、`:1482-1487`、`:1682-1687`；`crates/tkless-cli/src/commands/jobs.rs:110-121`
- **问题**：根命令与 `search`/`research`/`batch`/`crawl` 都有 `--format`；分发时只读取子命令字段。
- **影响**：`tkless -f json search q` 解析成功但输出 Markdown；`-o`、`--stats` 等根选项也有同样问题。
- **建议**：真正通用的选项设为 `global = true`，或在子命令中拒绝冲突用法。

### CLI-02 `--format` 暴露命令实际不支持的值

- **严重度**：中
- **位置**：`crates/tkless-cli/src/cli.rs:1484`、`:1684`、`:1312`、`:1230`；`crates/tkless-cli/src/main.rs:48-55`
- **问题**：`batch`/`crawl`/`diff`/`adapters` 使用完整 `CliFormat`，帮助与补全提供 `text/html/raw`，但解析后统一拒绝。
- **影响**：用户按帮助输入得到运行时错误，退出码为 2 但时机较晚；与 `search`/`research` 的受限解析器不一致。
- **建议**：为这些命令定义专用枚举或 `value_parser`。

### CLI-03 短选项语义冲突与命名漂移

- **严重度**：中
- **位置**：`crates/tkless-cli/src/cli.rs:720`、`:1475`、`:1670`、`:1115`、`:1148`、`:1355`、`:453`、`:1254`、`:1246`、`:1655`
- **问题**：`-c` 分别表示 cookie / concurrency / config / clear；`-m` 表示 mode / max-pages；`-d` 表示 depth / max-depth。
- **影响**：跨子命令记忆成本高，易误用。
- **建议**：统一长选项命名与单位；短选项只在无歧义时使用。

### CLI-04 默认值、范围与校验时机不一致

- **严重度**：中
- **位置**：`crates/tkless-cli/src/cli.rs:204-209`、`:1461`、`:1667-1674`、`:1177-1180`；`crates/tkless-core/src/mcp/tools.rs:512`、`:567`、`:599`；`crates/tkless-core/src/crawler/job.rs:52-68`
- **问题**：`search --limit` 无上限；`batch` 1–64、`crawl` 1–32、MCP batch 1–20；`wechat` 参数在 clap 层无校验；`--delay-ms` 默认值 0/300 不一致；`--max-size 0`、`--timeout 0`、`--max-tokens 0` 均被接受。
- **建议**：在 clap 层统一加非零/范围校验。

### CLI-05 互斥/依赖参数缺少声明

- **严重度**：中
- **位置**：`crates/tkless-cli/src/main.rs:646-647`、`:840-852`；`crates/tkless-cli/src/cli.rs:556-569`、`:578-584`、`:845-851`
- **问题**：
  - `--screenshot` 与 `--pdf` 可同时给出，路径取 screenshot，内容按 pdf 决定，PDF 会写入截图路径。
  - `--out-dir` 与 `-o` 同时给出时，`-o` 被静默忽略。
  - `--chunk-size`/`--chunk-overlap` 不要求 `--chunk`；`--assets-dir` 不要求 `--download-images`；根 `--cdp-wait` 不要求 `--cdp`。
  - `--mcp-dynamic` 单独使用无效果。
- **建议**：使用 `conflicts_with`、`requires`、`required_unless_present` 在解析期解决。

### CLI-06 退出码与机器可读输出不符合文档

- **严重度**：高
- **位置**：`README.md:473-483`；`crates/tkless-cli/src/main.rs:28-32`、`:574`、`:648`、`:813-837`、`:855-862`、`:919`；`crates/tkless-cli/src/commands/workflows.rs:79`；`crates/tkless-core/src/error.rs:204`
- **问题**：
  - URL 格式错误承诺退出 2，但 `url::Url::parse(...)?` 不是 `TklessError`，最终退出 1。
  - 缺少 TARGET 也退出 1。
  - `--stats` 用 `println!` 写 stdout，随后 payload 也写 stdout，`-f json --stats` 不是合法 JSON。
  - `--log-format json` 只覆盖 tracing，致命错误仍是纯文本。
  - `verification_required` 映射为 1 且未出现在 README 表。
- **建议**：统一将参数错误包装为 `TklessError`；`--stats` 改写到 stderr；致命错误遵循 `--log-format`。

### CLI-07 doctor 无条件报告成功

- **严重度**：中
- **位置**：`crates/tkless-cli/src/doctor.rs:54-60`、`:134`
- **问题**：未做缓存写测试就输出“缓存存储读写正常”；两个网络探针都失败仍输出“所有核心子系统均已完成自检”，且始终退出 0。
- **建议**：实际执行缓存写入/读取/删除；汇总失败并返回非零退出码。

### CLI-08 MCP 工具 schema 与 CLI 不对齐

- **严重度**：中
- **位置**：`crates/tkless-core/src/mcp/tools.rs:359-364`、`:396`、`:597-618`、`:543-556`；`crates/tkless-core/src/mcp/server/content.rs:264`
- **问题**：
  - `fetch_webpage.format` 缺少 `jsonl`。
  - `use_cache` 默认 `true`，CLI 默认关闭缓存。
  - `batch_fetch_webpages` 缺少 `extract_links`/`extract_tables`/`no_images`/`no_links`。
  - `deep_research` 没有 `proxy`。
  - `fetch_webpage` 硬编码 10 MB 响应上限，CLI 有 `--max-size`。
- **建议**：以 CLI 选项为单一事实来源生成 MCP schema，或增加契约测试。

### CLI-09 过时提示与未校验值

- **严重度**：低
- **位置**：`crates/tkless-cli/src/main.rs:590`、`:623`；`crates/tkless-cli/src/cli.rs:674-680`
- **问题**：`--download-cover`/`--download-video` 的错误信息提示不存在的 `--bili-dl`/`--bili-dl-cover`；`--bili-order` 是自由字符串，直接拼进 URL。
- **建议**：修正提示；为 `--bili-order` 定义 `ValueEnum` 并编码参数。

---

## 6. 文档一致性

### DOC-01 README 中存在不存在的别名

- **严重度**：高
- **位置**：`README.md:430`、`:434`；`crates/tkless-cli/src/cli.rs:1083-1095`
- **问题**：`adapters`（`platforms`）与 `upgrade`（`update`）没有 alias 声明；全文件无 `alias` 属性，测试还断言 `tkless update --check` 失败（`cli.rs:364`）。
- **建议**：删除别名描述，或实现 `visible_alias`。

### DOC-02 `watch --once` 不存在

- **严重度**：高
- **位置**：`doc/10-collection-workflows.md:233`、`doc/12-optimization-plan.md:155/179/188`；`crates/tkless-cli/src/cli.rs:1146-1161`
- **问题**：`WatchArgs` 只有 `--config/--state/--interval/--output/--allow-partial`，默认就是单轮。
- **建议**：删除 `--once`，或实现该参数。

### DOC-03 `wechat --history` 与 `session_required` 不存在

- **严重度**：高
- **位置**：`CHANGELOG.md:12`；`crates/tkless-cli/src/cli.rs:1173-1199`
- **问题**：文档描述历史采集、MCP 历史模式与 `session_required`，代码中均不存在。
- **建议**：将描述移到“计划中”，或删除。

### DOC-04 MCP 工具数量互相矛盾

- **严重度**：中
- **位置**：`README.md:25/360/389`；`crates/tkless-cli/src/cli.rs:1597`；`RELEASE_NOTES.md:95`；`CHANGELOG.md:25/47`
- **问题**：README 与 doc/02 写 31，`cli.rs` 写 27，RELEASE_NOTES 写 27，CHANGELOG 写 26/31。
- **建议**：以工具目录实际数量为准，统一所有文案。

### DOC-05 README 选项说明与实际不符

- **严重度**：中
- **位置**：`README.md:440`、`:181`、`:30`；`doc/03-adapters-and-features.md:98`、`:144`
- **问题**：
  - `--format` 未列 `jsonl`。
  - `--retry-budget` 被描述为搜索级选项，实际是根选项。
  - 文档说诊断写 stderr，但 `--stats` 写 stdout。
  - `doc/03` 说 `--chunk` 输出“JSON stream”，默认 Markdown 下实际输出 Markdown 分块。
  - `doc/03` 的 REPL 命令列表少于 `shell.rs:42-56` 实际支持的命令。
- **建议**：按当前实现更新示例与说明。

### DOC-06 开发规范文档未被 CLI 自身遵守

- **严重度**：中
- **位置**：`doc/04-development-standards.md:132-146`；`crates/tkless-cli/src/cli.rs:1131`、`:1154`、`:1463-1465`、`:1638-1643`、`:1177-1180`、`:646`、`:1483`、`:1654`、`:1667`、`:1683`
- **问题**：要求 `help_heading`、显式 `value_name`、不在描述中重复默认值；`BatchArgs`/`SearchSubArgs`/`CrawlArgs` 未遵守。
- **建议**：补齐 headings 与 value names，或降低规范要求。

### DOC-07 安全文档部分结论与实现不符

- **严重度**：高
- **位置**：`doc/09-security-hardening.md:9`、`:12`
- **问题**：
  - 称凭据写入“临时文件+rename，Unix 0600”，但 `cookies.txt` 实际为 `std::fs::write`（见 SEC-04）。
  - 称凭据库“仅在未提供 `--cookie`/`--cookie-file` 时使用”，实际前缀不匹配时仍回退到凭据库（`fetcher.rs:312-317`）。
  - 称“直接连接使用校验过的 DNS 解析结果”，实现是两次独立解析，连接使用 resolver 结果（`fetcher.rs:842`、`:879`）。
  - 私有/回环/组播受限的表述未覆盖 IPv6 特殊段（见 SEC-06）。
- **建议**：修正安全文档措辞，或补齐实现以匹配声明。

### DOC-08 其他过时文档

- **严重度**：中
- **位置**：`doc/01-ci-cd-and-release.md:24-115`；`doc/06-bing-search.md:51`
- **问题**：发布流水线片段与实际 `.github/workflows/release.yml` 不一致；搜索错误退出码描述与 `error.rs:190`、`:204` 不一致。
- **建议**：同步文档与工作流/错误映射。

### DOC-09 版本信息漂移

- **严重度**：低
- **位置**：`Cargo.toml:8`；`README.md:12`；`CHANGELOG.md:8-29`
- **问题**：代码与 README 为 0.5.0，但 README 宣称已具备的 `watch`、`auth`、`--pdf`、`--cookie-jar` 等仍在 Unreleased 段落。
- **建议**：发布前统一版本、CHANGELOG 与 README 状态。

---

## 7. 修复优先级

### P0（立即）

- [ ] SEC-01：MCP HTTP 回环认证、Host/Origin 校验、写入目录限制
- [ ] SEC-02：请求头改为白名单 + origin 绑定
- [ ] SEC-03：Cookie 作用域改为精确主机/PSL
- [ ] SEC-04：登录 Cookie 原子写入与 `0600`
- [ ] LOG-01：修复搜索 token 切片 panic
- [ ] LOG-02：feed 分支接入统一预算/分块/摘要

### P1（本迭代）

- [ ] SEC-05：升级产物独立签名
- [ ] SEC-06：SSRF 特殊段与下载器上限/溢出
- [ ] LOG-04/05/06/07：selector、公式、sync GC、watch 通知隔离
- [ ] LOG-09/10/11：表格、GitHub 适配器、采集模板/续跑
- [ ] CLI-01/02/05/06：根选项、format 枚举、互斥声明、退出码与 stats 输出
- [ ] DOC-01/02/03/07：不存在的别名/参数与安全声明

### P2（后续）

- [ ] SEC-07/08/09/10/11/12/13
- [ ] LOG-03/08/12/13/14/15/16
- [ ] CLI-03/04/07/08/09
- [ ] DOC-04/05/06/08/09

## 8. 建议的回归测试

1. `tokenize_query("20年")`、`"20日"`、`"2026年"` 不 panic。
2. 带 feed 的 URL 在 `--max-tokens`、`--chunk`、`--summarize` 下行为与普通 HTML 一致。
3. `--selector` 零匹配返回错误；嵌套选择器不重复输出。
4. ≥11 个公式的页面恢复顺序与数量正确。
5. 同步状态达到预算后可自动回收并继续运行。
6. watch 单个 job 通知超限不影响其他 job。
7. `tkless -f json search q` 输出 JSON，或明确报错。
8. `tkless URL -f json --stats` 输出仍是合法 JSON。
9. MCP HTTP 在回环绑定时拒绝无 Token 或非白名单 Origin 的请求。
10. 自定义 `Private-Token` 在跨主机重定向时不发送。
11. `tkless -f json search q` 与 `tkless -f text adapters` 分别继承格式/在解析期报错。
12. 缺 TARGET 与 URL 解析失败退出码为 2；`--log-format json` 的致命错误是合法 JSON。
13. `tkless URL -f json --stats` 的 stdout 仍是合法 JSON。
14. `--screenshot --pdf`、`--out-dir -o`、`--chunk-size` 无 `--chunk` 等在解析期失败。
15. `--bili-order`、搜索日期与数量范围在 clap 层拒绝非法值。

## 9. 审查边界

- 适配器仅抽查 `wechat`、`bilibili`、`youtube`、`github` 及通用入口，其余适配器未逐行审计。
- 未运行测试、Clippy、真实网络或浏览器回归；并发与超时行为建议通过 `cargo test --workspace` 与 `doc/09` 的真实浏览器用例复验。
- 已验证的正面设计包括：重定向逐跳凭据策略、Office ZIP 解压预算、缓存键包含提取选项、batch checkpoint 的索引/指纹恢复、robots 最长匹配规则。
