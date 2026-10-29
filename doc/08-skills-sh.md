# Skills.sh 技能目录适配

适配域名为 `skills.sh` 和 `www.skills.sh`。2026-10-28 核实时，`skill.sh` 是停放域名，不属于此适配器。

## 使用方式

```bash
# 技能详情：返回摘要、仓库、安装命令及网站呈现的技能正文
tkless https://skills.sh/vercel-labs/skills/find-skills

# JSON 包含 content、entity、extraction 与 warnings
tkless https://skills.sh/anthropics/skills/frontend-design -f json

# 仓库技能列表
tkless https://skills.sh/vercel-labs/skills -f json

# 站内搜索（URL 包含查询参数时用引号包围）
tkless "https://skills.sh/search?q=rust" --skills-limit 30 -f json
tkless "https://skills.sh/?q=react%20native" --skills-limit 50 -f json

# 首页及榜单自动翻页：默认最多 3 页、1000 个去重结果
tkless https://skills.sh/ -f json
tkless https://skills.sh/trending --skills-pages 5 --skills-limit 800 -f json
tkless https://skills.sh/hot -f json

# 从指定页继续（页号从 0 开始）
tkless "https://skills.sh/trending?page=3" --skills-pages 2 -f json

# 通用预算与输出选项仍可使用
tkless https://skills.sh/vercel-labs/skills/find-skills --max-tokens 1500
```

MCP 继续使用现有 `fetch_webpage`，无需新增工具或配置。动态模式下先发现或激活该工具：

```json
{
  "name": "fetch_webpage",
  "arguments": {
    "url": "https://skills.sh/trending",
    "skills_pages": 3,
    "skills_limit": 500,
    "format": "json"
  }
}
```

原生 Rust 调用使用 `TklessEngine::process_url`，自动选择 `SkillsAdapter`。`--no-adapter` 禁用此路径；显式 CSS `--selector` 提取原始页面 DOM，不恢复未渲染到 DOM 的折叠内容；显式 Schema 使用通用 Schema 提取。

MCP 搜索同样调用 `fetch_webpage`，将 `url` 改为 `https://skills.sh/search?q=rust`，用 `skills_limit` 控制最多返回的条数。Rust 使用 `ExtractionOptions.skills_pages` / `skills_limit`。页数范围 1–50，结果数范围 1–10000；两者均纳入缓存与批任务检查点身份。CLI 批量任务中把参数放在 `batch` 子命令前，可为每个 Skills.sh URL 应用相同限制。

## 提取的数据

| 页面 | JSON `entity` 主要字段 |
| --- | --- |
| 技能详情 `/owner/repo/slug` 或 `/site/domain/slug` | `page_type: skill`、`source`、`slug`、`source_type`、`repository_url`、`summary`、`install_command`、`readme_complete`、`content_kind`、`statistics` |
| 仓库 `/owner/repo` | `page_type: repository`、`scope: page_snapshot`、`count`、`skills[]` |
| 首页、`/trending`、`/hot` | `page_type: leaderboard`、`scope: paginated_catalog`、`view`、`count`、`skills[]`、`pagination` |
| `/search?q=...`、`/?q=...` | `page_type: search`、`scope: search_top_results`、`query`、`search_type`、`search_exhaustive: false`、`skills[]`、`pagination` |

`skills[]` 包括技能名、来源、标识、详情链接和安装量。公开接口提供的精确计数填写 `installs`；仓库 HTML 列表的 `metric_display` 保留展示文本，不把 `4.1M` 等近似值伪装成精确计数。榜单 `entity.view` 区分 `all-time`、`trending`、`hot`；`installs` 属于该视图的统计口径，不统一解释为累计安装量。Hot 额外保留上游 `change` 和 `installs_yesterday`，不把增长值当作安装数；技能详情的 `statistics.installs_display` 则是详情页安装量展示。字段不可用时为 `null` 或不包含对应统计项。

`content` 为正文，支持 Markdown、JSON、Text、HTML 和 Raw 等既有输出格式。技能详情的正文来自网站呈现的 `SKILL.md` HTML，保留标题、代码和链接；它不是仓库原始文件，不包含网站未展示的 Frontmatter、附件或其他文件。

`readme_complete` 表示网站提供的折叠正文已恢复，不代表获取了完整仓库。若仅能获取预览，设置 `readme_complete: false`、`truncated: true` 和 `skills_readme_preview` 警告。用户预算也可能截断 `content`，以文档顶层 `truncated` 为准。`max_tokens` 限制正文，不限制 `entity` 的大小。

## 搜索与翻页规则

榜单使用网站前端正在使用的 `/api/skills/{view}/{page}`，每页目前 200 条；从第 0 页开始，依据 `hasMore` 连续请求下一页。每页均校验返回页号，按来源和技能标识去重，直到上游结束、页数上限、结果上限、空的未结束页或重复页。仓库列表仍读取该仓库公开页面，不把全站榜单当作仓库列表。

`entity.pagination` 返回 `pages_fetched`、`start_page`、`complete`、`has_more`、`next_page`、`stop_reason`、`total_reported` 和 `duplicates_removed`。`complete` 只表示是否消费完这次 API 列表，不意味着抓到了全站所有技能；`total_reported` 为上游报告的当前列表总量。达到结果上限且只消费了当前页的一部分时，`next_page` 为 `null`，避免生成会跳过未返回条目的错误续页号。`next_page` 是 API 续页提示，不是精确恢复检查点。

后续页请求失败时，保留前面结果，写入 `skills_page_failed`、`pagination.page_error` 及待重试页号。页数、结果数或循环保护导致停止时，写入 `skills_pagination_incomplete`。这些情况顶层 `truncated: true`；Markdown 中也显示页数、结果数、停止原因及部分目录提示。首个 API 请求失败、返回结构无效时直接报错，不回退为看起来成功的客户端页面外壳。详情和仓库页面仍可按原流程回退并记录 `adapter_fallback`。

搜索使用网站公开 `/api/search?q=...&limit=...`，查询须为 2–2000 个字符，正确编码 Unicode 和特殊字符。当前接口返回排名靠前的结果，不提供 `hasMore`、游标或下一页，且实测会忽略 `page` / `offset`。因此最多请求 200 条，不伪造搜索翻页；带这两个参数的搜索 URL 会明确报错。`skills_pages` 对搜索无效。合法的空 `skills` 数组表示无结果，缺少该字段或非法 JSON 则为接口错误。

搜索结果有 `skills_search_unpaged` 提示，`pagination.supported: false`，`complete` / `has_more` 为 `null`，`search_exhaustive: false`。若达到请求条数，设置 `truncated: true` 和 `stop_reason: search_limit`；未达到时为 `search_response`，也不声称已经穷尽所有相关技能。

## 数据获取与边界

实现使用既有 Rust HTTP 客户端抓取公开页面，沿用重定向、SSRF、大小限制和请求配置。Next Flight 中的 JSON、字符串及长度分帧文本只作为数据解析，绝不执行页面脚本或技能命令。无需新增 Python、Node.js、API Key 或浏览器运行时依赖。

[官方 API 文档](https://www.skills.sh/docs/api)规定 `/api/v1` 需要 Vercel OIDC 认证。本实现使用公开网页及前端公开使用的搜索、翻页接口，不依赖 `/api/v1`，不自动读取或申请认证凭据。这些前端接口可能变化，因此保留严格结构校验和停止原因。不提供技能安装、认证 API、文档/审计子页面专门适配或仓库文件下载。API 请求经已有 `fetch_api` 发出，拒绝跨源重定向，并沿用 SSRF、响应大小和请求配置；不跟随返回数据里的外部地址抓取资源。

验证包括合成页面回归测试：域名与路径识别、折叠正文及 Unicode 分帧、缺失正文警告、列表去重与安装量、输出格式、预算、CSS 选择器，以及公开页面的实际抓取验证。

2026-10-28 验证记录：新增 13 项测试，全工作区 264 项测试通过；Clippy 严格检查和无默认特性检查通过。最终 Release 的公开页面实测覆盖 `find-skills`、`frontend-design`、`/site/uizze.sh/ui-taste`、`vercel-labs/skills` 仓库、首页、Trending 和 Hot。三篇详情均恢复了网站提供的完整折叠正文，仓库页返回 3 个技能；榜单数量随页面变化，不作为固定全站总量。Release 的 MCP `fetch_webpage` 调用已验证 `structuredContent.document` 返回同一适配器的完整正文，CLI 目录确认注册 22 个适配器。

2026-10-29 搜索与翻页验证：新增 12 项测试，全工作区 276 项测试通过；Clippy 严格检查、无默认特性检查通过。覆盖实际页号请求、跨页去重、页数/条数上限、重复/空页停止、后续页失败保留、非法返回结构、Unicode 查询、搜索上限、MCP/CLI 参数边界、缓存隔离与 API 失败禁止回退。联网实测 `rust` 搜索返回指定的 20 条、`Rust 工具` 返回指定的 5 条，首页和 Trending 连续两页各 400 条，Hot 两页在 250 条上限停止；MCP 搜索返回指定的 7 条，榜单两页在 220 条上限停止。以上数量为本次实测，不是固定上游结果保证。

本机代理将公共域名解析为虚拟内网 IPv6，实测命令为这些固定公开网址添加了已有的 `--allow-private-addresses` 选项；MCP 则在该次 `fetch_webpage` 调用参数中设置 `allow_private_addresses: true`。默认 SSRF 检查仍启用；普通公共 DNS 环境无需此选项。
