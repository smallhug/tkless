# 微信公众号公开内容

支持其他公众号的公开文章发现、合集采集，以及携带用户授权会话的账号历史列表采集。无需公众号管理员 API 密钥，不使用后台登录接口。历史模式需要有效微信会话，不能匿名读取。实现于当前未发布工作区，不属于已发布的 0.5.0 二进制。

## 账号历史模式

账号历史与合集是两条独立流程。使用 `--history`，输入文章 URL 或已确认的 `biz`；此模式不接受公众号名称或合集 URL，也不会回退成合集结果。

```powershell
# 首次执行：解析账号、枚举历史并保存进度（示例会话文件需先准备）
tkless wechat "https://mp.weixin.qq.com/s/dFzHMB6pFyjKFd50_gHkQQ" --history --session-file .\wechat-session.json --checkpoint .\insar-history.json --limit 1000 --max-pages 100 --allow-partial -o .\articles.json

# 更新会话后恢复，或继续下一批；同时获取正文并重试失败文章
tkless wechat "MzkyMDcyMzc3OA==" --history --session-file .\wechat-session.json --checkpoint .\insar-history.json --resume --limit 1000 --max-pages 100 --fetch-content --allow-partial -o .\articles.json
```

`--limit` 和 `--max-pages` 在历史模式中限制每次调用新增文章数和请求页数；检查点累计保存所有已采集文章，恢复后的报告包含累计列表。一次群发可能含多篇文章，达到条数预算时保留当前页游标，下次重读该页并去重，避免漏掉次条。达到页数/条数预算后继续 `--resume`，直到 `listing_complete=true`。`pages_fetched` 为累计成功解析页数，重读部分页也计数。

会话 JSON 示例（值均为占位符，不能直接用于访问）：

```json
{
  "biz": "MzkyMDcyMzc3OA==",
  "cookie": "请替换为本人授权的历史请求Cookie",
  "parameters": {
    "uin": "请替换为请求中的uin",
    "key": "请替换为请求中的key",
    "pass_ticket": "请替换为请求中的pass_ticket"
  }
}
```

在微信中打开目标公众号的历史页面，从你授权取得的历史请求中准备会话文件。仅保留实际存在的字段；参数允许 `uin`、`key`、`pass_ticket`、`appmsg_token`、`poc_token`、`poc_sid`。不同客户端可能使用不同组合，提供字段不代表会话仍然有效，工具以真实历史响应验证。文件中的 `biz` 必须与目标账号一致；文件上限 64 KiB。当前实现不自动扫描微信缓存、不安装抓包证书、不自动打开登录窗口，也不承诺普通浏览器登录可以取得该会话。

凭据只发送到固定 HTTPS 历史接口，不跟随重定向，不写入报告或检查点；错误信息不会回显上游响应或包含凭据的请求 URL。文件本身仍含敏感信息，请保存在本地受保护位置，不要提交仓库或粘贴到聊天。正文请求不携带历史模式额外接入的会话参数。

没有会话时返回 `stop_reason=session_required`，不会发出历史请求。上游返回 `no session`、登录重定向或验证页面时保留索引和游标，提示刷新会话并恢复。检查点含版本、账号、游标及已采集文章，使用独占锁和原子替换；已有文件必须显式 `--resume`，不能与会话或输出文件重合。检查点上限 64 MiB；含正文时更容易达到上限，建议先完成索引，再分批获取正文。

历史模式 `scope=account_history`，`listing_complete=true` 表示从起点遍历至接口明确返回末尾，覆盖当前会话可见列表；`complete` 另考虑所请求正文是否全部成功。`account_history_complete` 仍固定为 `false`，因为无法证明已删除、不可见、未进入历史索引的文章都已恢复。索引完成后可以继续 `--resume --fetch-content`；已有正文跳过，失败正文重新尝试。`next_urls` 仅提供不含凭据的游标诊断信息，恢复必须使用检查点，不能将该 URL 当作普通文章入口。

MCP 复用 `list_wechat_articles`：设置 `history: true`，同时传入 `session_file`、`checkpoint`，恢复时增加 `resume: true`，其余预算与正文参数共用。路径均位于 MCP 服务进程所在机器。

历史接口使用 `/mp/profile_ext?action=getmsg`，读取 `general_msg_list`（字符串或对象）、主文章与 `multi_app_msg_item_list`，按 `next_offset` 和 `can_msg_continue` 翻页。这是内部网站协议，未来可能变化。协议字段参考[公开实现的说明](https://github.com/tingaidehua/wechat-article-downloader-skill/blob/main/docs/protocol-principles.md)，本实现不执行网页脚本。

## CLI

```bash
# 名称发现：Bing 索引候选，再读取文章核对公众号名称及身份
tkless wechat "公众号名称" --limit 10 --allow-partial -o articles.json

# 文章入口：提取账号，自动枚举页面暴露的同账号合集
tkless wechat "https://mp.weixin.qq.com/s/文章标识" --allow-partial

# 合集入口：自动翻页，最多 5 页、100 篇
tkless wechat "https://mp.weixin.qq.com/mp/appmsgalbum?__biz=账号标识&album_id=合集编号" --allow-partial

# 同时获取正文，单篇正文预算默认 4000 token
tkless wechat "https://mp.weixin.qq.com/mp/appmsgalbum?__biz=账号标识&album_id=合集编号" --fetch-content --max-tokens-per-article 4000 --allow-partial
```

此命令固定输出 JSON。`--limit` 为 1–1000（默认 100），`--max-pages` 为 1–100（默认 5），`--delay-ms` 为 0–60000（默认 300）。名称搜索受 Bing 单次最多 20 个候选的限制，核实后数量可能更少；不会自动扩展名称发现结果的合集。文章入口可用 `--no-follow-albums` 关闭合集扩展。`--expected-biz` 指定目标账号身份，避免同名账号混入；名称相同本身不能证明账号唯一。

不完整时先输出已获取的数据，再返回非零退出码。`--allow-partial` 接受部分结果并返回成功。名称及文章入口均无法证明全账号历史完整，因此需要按部分结果处理。接口失败或验证失败不会转换成成功文章正文。

## 返回与恢复

| 字段 | 含义 |
|---|---|
| `scope` | `indexed_articles` 名称发现；`article_and_linked_albums` 文章入口；`album` 单合集；`album_remaining` 从游标续取；`account_history` 授权历史 |
| `account_history_complete` | 固定为 `false`；公开合集和搜索索引不能证明账号历史完整 |
| `listing_complete` | 指定合集、剩余范围或授权历史是否枚举完成 |
| `complete` | 指定范围枚举完成，且请求的正文抓取没有失败；不保证单篇正文未受 token 预算裁剪 |
| `stop_reason` | 完成、数量/页数上限、重复页/游标、响应异常、抓取失败、身份不符或仅发现 |
| `accounts` / `articles` | 账号 biz、昵称、用户名；文章 ID、链接、标题、时间、封面及可选正文/错误 |
| `albums` / `next_urls` | 发现的合集与可继续请求的入口 |
| `errors` / `warnings` | 错误与覆盖范围提示；正文失败另见各文章的 `error` |

将 `next_urls` 中的 URL 重新作为 `tkless wechat` 输入即可继续，调用方合并结果时按文章 `id` 去重。续取报告只描述剩余范围。`pages_fetched` 记录成功解析的合集响应页数，不包括搜索或正文请求；`total_hint` 是上游报告的合集总数，可能随内容变化。

文章返回 `entity.type = wechat_article`、账号身份、关联合集和文章标识。直接使用通用 `tkless URL` 抓取合集也受支持，默认最多 5 页、100 篇，实体类型为 `wechat_album`；精确控制分页应使用 `wechat` 子命令。

## MCP

新增 `list_wechat_articles`，支持动态工具发现。参数示例：

```json
{
  "source": "公众号名称",
  "limit": 10,
  "max_pages": 5,
  "delay_ms": 300,
  "fetch_content": false,
  "follow_albums": true,
  "max_tokens_per_article": 4000
}
```

报告通过 `structuredContent` 返回；宿主应检查 `complete`、`stop_reason` 和文章错误，不能只依赖传输成功。输入校验错误使用 MCP 错误响应。

## 接口与边界

合集读取网站的 `/mp/appmsgalbum?action=getalbum&f=json`，携带 `__biz`、`album_id`、`count`；翻页使用末条记录的 `begin_msgid` 与 `begin_itemidx`。只保留必要参数，不转发入口中的会话参数。校验响应状态、续页标记、账号与文章链接，跨页去重，异常时保留已完成页。

这是公开网站接口，并非承诺稳定的公众号开放平台 API。协议交叉参考公开实现 [WeChat Album Downloader](https://github.com/0xbbdd/WeChat-Album-Downloader/blob/26de5bde51173bcf888f6b6841bda46d6ba489b3/appmsgalbum_downloader.py) 与 [feedgrab](https://github.com/iBigQiang/feedgrab/blob/9f691a18c7f3245d52261494405c561762aa8981/feedgrab/fetchers/mpweixin_album.py)，并以公开合集实测响应核对字段。

无法保证取得账号全部历史、未索引文章、未公开合集、已删除或付费正文、留言、阅读量等。遇到微信验证或访问限制会报告失败，不自动绕过验证，也不自动打开验证窗口。正文和合集可能分别受限，列表成功不代表每篇正文都可访问。

## 验证记录

2026-10-30 账号历史模式：全工作区 333 项测试通过，历史模块包含 7 项测试；严格 Clippy、格式检查和 Release 构建通过。验证主条/次条展开、预算中断续取、重复页/游标、会话过期、检查点隔离与 Windows UTF-8 BOM。真实接口使用无效测试会话返回 `session_required`，CLI 非零退出，检查点保留；最高诊断级别日志、结果和检查点均未包含测试凭据。尚未取得有效微信会话，因此没有宣称真实账号历史分页成功或完成全账号采集。

2026-10-30 补充实测“無厘InSAR”：从用户提供的文章确认账号及“InSAR开源软件实战手册”合集，获取合集全部 17 篇；随机抽取 3 篇均成功获取正文且未截断。实测修复了脚本中的合集链接漏识别，以及文章短链接与合集长链接重复计数问题，增加对应回归测试。此结果只代表该合集，不是全账号历史。

2026-10-30：全工作区 324 项测试通过，严格 Clippy、无默认特性编译、格式检查、Release 构建和文档/MCP 目录检查通过。离线测试覆盖游标双字段、跨页去重、预算停止、后续页失败保留、异常响应、账号约束及验证页面拒绝。

公开实测 `COMPASS Lab` 的 `Compass Paper Reading` 合集：连续 2 页获取 25 篇，上游总数提示为 662，按数量上限停止；使用续取 URL 再获取 2 篇，与前批没有重复。首篇文章《论文分享 | Cache Me, Catch You》成功提取身份与正文。按名称 `COMPASS Lab` 搜索本次未取得可核实结果，名称发现仍受 Bing 索引及访问条件限制。以上为当次实测，不保证上游数量或可访问性始终相同。
