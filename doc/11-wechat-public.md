# 微信公众号公开内容

支持其他公众号的公开文章发现与合集采集。无需公众号管理员 API 密钥，不使用后台登录接口，也不读取或依赖任何账号会话。实现于当前未发布工作区，不属于已发布的 0.5.0 二进制。

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
| `scope` | `indexed_articles` 名称发现；`article_and_linked_albums` 文章入口；`album` 单合集；`album_remaining` 从游标续取 |
| `account_history_complete` | 固定为 `false`；公开合集和搜索索引不能证明账号历史完整 |
| `listing_complete` | 指定合集或剩余范围是否枚举完成 |
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

2026-10-30 补充实测“無厘InSAR”：从用户提供的文章确认账号及“InSAR开源软件实战手册”合集，获取合集全部 17 篇；随机抽取 3 篇均成功获取正文且未截断。实测修复了脚本中的合集链接漏识别，以及文章短链接与合集长链接重复计数问题，增加对应回归测试。此结果只代表该合集，不是全账号历史。

2026-10-30：全工作区 324 项测试通过，严格 Clippy、无默认特性编译、格式检查、Release 构建和文档/MCP 目录检查通过。离线测试覆盖游标双字段、跨页去重、预算停止、后续页失败保留、异常响应、账号约束及验证页面拒绝。

公开实测 `COMPASS Lab` 的 `Compass Paper Reading` 合集：连续 2 页获取 25 篇，上游总数提示为 662，按数量上限停止；使用续取 URL 再获取 2 篇，与前批没有重复。首篇文章《论文分享 | Cache Me, Catch You》成功提取身份与正文。按名称 `COMPASS Lab` 搜索本次未取得可核实结果，名称发现仍受 Bing 索引及访问条件限制。以上为当次实测，不保证上游数量或可访问性始终相同。
