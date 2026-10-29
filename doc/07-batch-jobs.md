# 批量流式任务与断点恢复

当前 `batch` 使用同一套 Rust 调度与检查点实现，供 CLI 和 MCP 调用。输入顺序由 URL 列表确定，结果的 `index` 从 0 开始；重复 URL 也保留独立索引。

## CLI

```powershell
# JSONL 逐项写出并刷新；默认保持输入顺序
tkless batch ./urls.txt -f jsonl --checkpoint ./batch.state.jsonl -o ./results.jsonl

# 完成顺序可让快页面先输出
tkless batch ./urls.txt -f jsonl --order completion --checkpoint ./batch.state.jsonl -o ./results.jsonl

# 中断后继续：使用原 URL 列表与提取/网络配置
tkless batch ./urls.txt -f jsonl --checkpoint ./batch.state.jsonl --resume -o ./results.jsonl

# 已失败项也重新抓取；已成功项继续复用
tkless batch ./urls.txt -f jsonl --checkpoint ./batch.state.jsonl --resume --retry-failed -o ./results.jsonl

# 有界并发与任务启动间隔
tkless batch ./urls.txt -f jsonl --concurrency 8 --per-host-concurrency 2 --per-host-delay-ms 300
```

前两条是两个独立的启动示例，使用时选择一条；新任务的检查点文件必须不存在。`--resume` 要求 `--checkpoint`，`--retry-failed` 要求 `--resume`。JSONL 每完成一项就交付；输入顺序模式会等待前面的慢页面，完成顺序模式按就绪顺序交付。

JSONL 不累积完整结果集，内存中的在途页面数量受并发数限制，写入端变慢时会施加背压。JSON 和 Markdown 为完整报告，会收集结果。URL 列表和检查点索引仍按任务规模占用内存。

JSON 报告包含 `total`、`succeeded`、`failed`、`resumed`、`fetched` 和 `results`。`resumed` 是本次复用的记录数，`fetched` 是本次重新执行的条目数，包含本次失败项。每项包含 `index`、`url`、`document`、`error`、`exit_code` 和可选 `error_details`。

默认只要有失败项就写出报告并非零退出；`--allow-partial` 接受部分或全部失败，逐项错误仍保留。输入文件、输出文件和检查点必须是不同路径。恢复输出会重新交付所有条目；指定 `-o` 时重建完整输出文件，不追加旧的部分输出。

## 检查点约定

检查点是专用 JSONL：首行为任务头，后续每行保存一个完整结果。完成记录在交付给消费者之前调用 `sync_data`；即使输入顺序下暂时无法输出某个快页面，它也可以先保存。

- 任务指纹覆盖 URL 列表及顺序、提取配置、Schema 文件内容、网络配置和请求身份；凭据只参与哈希，不作为配置明文写入任务头。结果正文和 URL 本身会保存在文件中。
- 可以调整并发、输出顺序和延迟后恢复；改变影响提取内容或请求身份的配置会被拒绝。
- 默认复用成功和失败记录；`--retry-failed` 重跑失败索引。尚未保存的在途条目会重新执行。
- 恢复时只丢弃末尾未完成的一行；完整但损坏的记录、未知版本或任务不匹配均明确报错。
- 检查点持有操作系统独占文件锁，禁止两个进程同时恢复/写入同一任务。Unix 新文件权限为 `0600`。
- 新任务不会覆盖既有检查点；不迁移旧格式。

当前恢复针对固定 URL 列表的 `batch`，站点爬虫的队列恢复另行实现。

## MCP

`batch_fetch_webpages` 支持 `checkpoint`、`resume`、`retry_failed`、`order`、`per_host_concurrency`、`per_host_delay_ms` 和 `stream_results`。路径指 MCP 服务所在机器的本地路径。MCP 并发参数范围为 1–20；CLI / 核心范围为 1–64。默认总并发 5、单主机并发 2，延迟为 0；延迟上限为 3,600,000 毫秒。延迟限制作用于页面任务启动，不逐条控制适配器内部请求。

默认仍返回完整批量报告。启用流式结果时，调用必须带 `_meta.progressToken`：

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "batch_fetch_webpages",
    "_meta": {"progressToken": "batch-1"},
    "arguments": {
      "urls": ["https://example.com/a", "https://example.com/b"],
      "stream_results": true,
      "order": "completion",
      "checkpoint": "./batch.state.jsonl",
      "format": "json"
    }
  }
}
```

每项通过 `notifications/progress` 的 `params._meta["tkless/batch_item"]` 交付，包含完整条目；这是服务在 `capabilities.experimental` 中声明的扩展。宿主需要读取该扩展元数据，普通进度提示不等同于完整流式结果。

结果通知使用有界队列并等待消费，全部条目通知写出后再写最终响应。最终 `structuredContent` 仅包含统计和 `streamed: true`，不包含 `results` 数组。全部失败时 `isError` 为 true；逐项错误已经在通知中交付。中断连接或取消调用后，可用同一检查点重新发起任务。

任务取消遵循 MCP `notifications/cancelled`；恢复不会补发原请求的响应，而是执行一个新的调用。协议及工具定义见 [MCP 接入](02-mcp-tools-extension.md)，数据结构和实现见 [batch.rs](../crates/tkless-core/src/batch.rs)。
