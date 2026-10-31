# 14 - DeepSeek Harness 插件

本仓库根目录同时是一个 DSH（DeepSeek Harness）bundle 包：`package.json` 声明 `dsh.bundle.patch`，`cordis.patch.yml` 覆盖 DSH 的 agent preset，`dsh/index.js` 负责下载 TkLess 二进制并把 `tkless mcp` 暴露为原生工具。

该集成解决两件事：

1. **真正移除内置网络搜索**：覆盖 `standard`、`ptc`、`cordis` 三个 shipped preset，把 `tool-web` 的 `search` 设为 `false`。新会话不会注册 `web_search`，搜索提示词也不再出现；`web_fetch` 保留。
2. **bridge + guard**：`dsh/index.js` 启动 `tkless mcp --dynamic`，把工具注册为 `mcp__tkless__<name>`；同时注册 guard，拒绝安装前就已打开的旧会话对 `web_search` 的调用。关闭或卸载 bundle 时，MCP 子进程、工具、guard 和 preset 覆盖一起移除。

bridge 使用 MCP `2026-07-28` 无握手协议：启动时调用 `server/discover`，每个请求在 `params._meta` 中携带协议版本、客户端信息与空能力对象；每次 `tools/call` 注入 `progressToken` 并收集批量流式项；`activate_tools`（含 `execute_tool` 间接调用）后重新调用 `tools/list` 刷新工具注册（服务端不推送 `list_changed`）。

> 版本锁定：preset 覆盖重述了完整插件列表，因此只能匹配生成它时的 DSH 版本。当前锁定 `0.2.1-rc.5`；升级 DSH 后必须重新生成并发布新版本插件。

## 安装

要求：DSH `0.2.1-rc.5`，Node 已随 DSH 提供。

在 DSH 侧边栏 **Plugins → Add plugin** 粘贴：

```text
github:smallhug/tkless#v0.7.0
```

或使用 CLI（参数顺序以本机 `dsh plugin --help` 为准）：

```bash
dsh plugin add github:smallhug/tkless#v0.7.0
```

> `v0.7.0` 尚未发布；发布前请用 `#<commit>` 或本地目录安装。`v0.6.0` 不包含这个 bundle。

安装后插件默认启用。**必须新开一个会话**才能看到 `web_search` 被移除；当前已打开的会话仍持有旧 preset revision，只能由 guard 拒绝调用。

## 二进制来源

- Windows x64：`tkless-windows-amd64.exe`
- Linux x64 / ARM64：`tkless-linux-amd64` / `tkless-linux-arm64`
- macOS：Release 只提供 `.dmg`，插件不自动解包；请安装后设置 `binaryPath` 指向可执行文件

首次启用时，插件从 `https://github.com/smallhug/tkless/releases/download/v<version>/` 下载对应资产：

1. 读取并解析 `SHA256SUMS`；清单缺失或不包含该资产时拒绝安装。
2. 下载资产，校验大小上限与 SHA-256。
3. 写入 `<包目录>/bin/tkless(.exe)` 并设置执行权限。
4. 执行 `tkless --version` 自检。

二进制位于插件包目录内，所以 `pnpm remove` 删除包时二进制一并删除。DSH 升级或 profile 修复可能清理包目录，下次启用会重新下载。

安全边界：这里只校验 SHA-256，不验证 `tkless upgrade` 使用的 Minisign 签名；HTTPS 之外没有额外信任锚。需要更强校验时请预装二进制并设置 `binaryPath`。下载走 Node 的全局 `fetch`，不读取 npm/pnpm 代理配置；受限网络请使用 `binaryPath` 或 `downloadBaseUrl`。

## 配置

`cordis.patch.yml` 里的默认配置：

```yaml
- insert:
    - id: tkless-dsh
      name: './dsh/index.js'
      config:
        autoDownload: true
```

可用字段：

| 字段 | 默认 | 说明 |
|---|---|---|
| `autoDownload` | `true` | 包内没有二进制时是否自动下载 |
| `binaryPath` | — | 指定现成二进制；优先级高于自动下载，也可用环境变量 `TKLESS_DSH_BINARY` |
| `releaseTag` | `v<包版本>` | 下载使用的 tag |
| `downloadBaseUrl` | GitHub Release URL | 覆盖下载基地址（镜像/内网） |
| `args` | `['mcp', '--dynamic']` | 传给 `tkless` 的参数 |
| `env` | — | 追加给子进程的环境变量 |
| `toolNamePrefix` | `mcp__tkless__` | 工具名前缀 |
| `disableNativeSearch` | `true` | 是否安装 `web_search` guard |
| `protocolVersion` | `2026-07-28` | 每个请求 `_meta` 中声明的 MCP 协议版本 |

## 状态与恢复

- `tkless_dsh_status`：返回 bridge 状态、协议版本、工具数量、二进制路径、错误信息，以及**当前 agent** 是否还能看到 `web_search`。新会话应为 `false`，旧会话为 `true`。
- `tkless_dsh_retry`：启动失败（网络、校验、二进制损坏）后重新下载并连接。

启动失败时的边界：preset 覆盖在组合层独立生效，所以**新会话仍然没有 `web_search`**，即使 bridge 没起来；旧会话因为没有安装 guard，仍可使用原生搜索。修复下载或设置 `binaryPath` 后运行 `tkless_dsh_retry`，或重新启用插件。

如果新会话里 `webSearchVisible` 仍为 `true`，说明 profile 中存在优先级更高的 preset 覆盖（例如用 Web 的 preset 编辑器保存过修改），它会压过 bundle 的覆盖；需要删除该 profile 覆盖或改用干净的 preset。

bridge 为每次 `tools/call` 注入 `_meta.progressToken`，并收集 `notifications/progress` 中的 `tkless/batch_item`：

- `batch_fetch_webpages` 的 `stream_results: true` 可用；流式项以 JSONL 追加到 `text`，同时写入 `structured.streamedItems`，`structured.summary` 是服务端最终摘要。
- 其他工具的进度通知只用于驱动收集，不实时推送到 UI。

工具返回值是 `{ text, structured? }`：`text` 供模型阅读；`structured` 对应 MCP 的 `structuredContent`（批量流式时为 `{ summary, streamedItems }`），PTC / workflow 可直接读取。`activate_tools` 或 `execute_tool({ tool_name: 'activate_tools' })` 执行后会同步刷新工具注册。

服务端的 `outputSchema` 使用 `anyOf`，而 DSH 的输出 schema 只接受 `oneOf` 等子集，因此不直接映射；`structured` 已由服务端在返回前校验。

## 关闭与卸载

- **关闭 bundle**：MCP 子进程停止，TkLess 工具注销，guard 解除；新会话恢复原生 `web_search`。旧会话的 guard 解除后立即恢复调用，但启动于禁用状态的会话不会重新注册工具。
- **卸载**：与关闭相同，且包目录（含下载的二进制）被删除。

不要单独关闭 bundle 内的行：preset 覆盖行在设计上是只读的，但 `tkless-dsh` 行可以单独关闭，会造成 TkLess 工具消失而 `web_search` 仍被移除。

## 维护 preset 覆盖

DSH 升级后：

```bash
# 指向新的 DSH 安装，重新生成 cordis.patch.yml 与 dsh/upstream.json
node scripts/sync-dsh-presets.cjs --asar "/path/to/resources/app.asar"

# 校验已提交内容是否匹配当前安装
node scripts/sync-dsh-presets.cjs --check

# 静态检查 bundle 结构
node scripts/check-dsh-bundle.cjs
```

`sync-dsh-presets.cjs` 直接读取 `app.asar` 里的 `@deepseek-ai/dsh-web-app/presets/*.patch.yml`，把 `- insert:` 声明转换成 patch 覆盖，并且只修改 `tool-web.config.search`。如果上游 preset 增加或删除插件，重新生成会自动同步。

生成的 `cordis.patch.yml` 约 400 行，不要手改；`dsh/upstream.json` 记录上游版本与三个 preset 的哈希，用于排查漂移。

只覆盖 DSH 自带的三个 preset；其他 bundle 声明的自建 preset 如果挂载了 `tool-web`，不在本插件的覆盖范围内。
