# 搜索与功能验证

当前搜索仅使用 Bing；默认最多 10 条。搜索前执行 HTTPS 连接预检，浏览器渲染按需启用。当前参数和搜索验证见 [Bing 搜索说明](06-bing-search.md)。

以下保留此前与搜索引擎无关的功能补齐记录。

## 其他源码补齐

| 范围 | 当前行为 |
|---|---|
| Token 预算 | 截断提示及 Markdown 围栏闭合计入预算；极小预算可省略提示；最终 Markdown 输出再次检查预算 |
| 缓存 | SHA-256 版本化键覆盖提取配置、Schema 文件内容及请求身份；保留过期验证头供跨进程再验证；不再由 CLI 重复写入覆盖验证头 |
| 批量提取 | 有界并发，保留每个 URL 的文档或错误及退出码，应用启动间隔；默认有失败项即非零退出 |
| 站点爬取 | 实际并发调度、启动间隔、递归 Sitemap index、错误记录、重定向后相对链接解析；支持 `--allow-partial` |
| HTTP 重试 | 处理 HTTP 429，并读取秒数或 HTTP 日期形式的 `Retry-After`；等待上限为 300 秒 |
| PDF | 本地与缓存抓取路径均支持摘要、分块和正文预算 |
| 调研报告 | 摘录标注来源，二跳内容关联原研究维度；移除通用比较模板；输出预算保留合法 JSON 或明确报错 |
| MCP | 支持版本协商、动态列表能力声明、并发工具调用、取消及 300 秒超时；最多 64 个在途调用 |
| Diff | Myers 算法与标准 unified diff hunk；末尾换行变化计入差异 |
| 认证状态 | 知乎、GitHub 尝试远程验证；未验证状态不再仅凭 Cookie 名称判为有效；微信明确标记 `verified: false` |
| 升级 | 语义化版本比较、逐跳下载检查、100 MB 大小限制、替换前 SHA-256 验证；发布工作流上传配套校验文件 |
| JSONL | 单页、搜索、批量、爬虫、适配器和差异生成合法逐行 JSON；调研仅支持 Markdown/JSON，结构化响应不会被直接切成损坏的 JSON |
| 查询保真 | 不再自动替换连字符、去掉引号/年份或改写平台查询；域名操作符保留路径大小写 |
| 搜索诊断 | 区分结果、明确空结果、过滤后为空、验证页及未知结构；记录逐页过滤和返回数量 |
| Schema 严格模式 | 解析/提取失败时直接报错；默认回退在 JSON 中保留结构化警告；全空列表不算有效提取 |
| 来源与错误详情 | 文档记录实际提取方式和 HTTP/CDP/本地路径；批量与搜索后抓取保留逐项错误码和重试信息 |
| MCP 输出契约 | 网页、HTML、搜索和批量工具提供输出 Schema 和结构化内容；仅实现 2025-11-25，参数与输出集中校验 |
| 批量持久任务 | JSONL 流式交付、输入/完成顺序、背压、持久检查点、失败重试、单主机并发和启动间隔 |

### 批量输出

`batch -f json` 和 MCP `batch_fetch_webpages(format="json")` 现在返回包含统计和逐项结果的对象：

```json
{
  "total": 1,
  "succeeded": 0,
  "failed": 1,
  "resumed": 0,
  "fetched": 1,
  "results": [
    {
      "index": 0,
      "url": "http://127.0.0.1/example",
      "document": null,
      "error": "SSRF blocked: ...",
      "exit_code": 6
    }
  ]
}
```

原先直接读取文档数组的调用方需要改读 `results[].document`，同时处理 `results[].error`。CLI `batch` / `crawl` 默认报告部分失败并非零退出；如果工作流允许部分失败，可显式使用 `--allow-partial`。


## 复测命令

```powershell
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo build --release --offline -p tkless-cli
tkless -vv search "Rust programming language" -f json
```

在线返回数量受上游响应和过滤影响，不作为离线 CI 的成功断言。
