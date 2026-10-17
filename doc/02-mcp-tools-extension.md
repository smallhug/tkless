# 02 - Model Context Protocol (MCP) 工具扩展架构设计

本文档详细描述 TkLess 的原生 Model Context Protocol (MCP) 服务端架构，以及向主流 AI Agent（Claude Desktop、Cursor、Zed 等）暴露的高级工具扩展方案。
> **Spend less tokens, perceive more web.**

---

## 1. 架构总览

TkLess 内置原生轻量级 MCP 服务端（位于 `crates/tkless-core/src/mcp`），无需 Node.js 或 Python 运行时，直接通过标准输入输出（`stdio`）以 JSON-RPC 2.0 协议与 AI 宿主进程高效通信：

```text
┌────────────────────────────────────────────────────────┐
│   AI 宿主环境 (Claude Desktop / Cursor / Zed Agent)    │
└───────────────────────────┬────────────────────────────┘
                            │ JSON-RPC 2.0 (stdio)
┌───────────────────────────▼────────────────────────────┐
│              TkLess MCP Server (Rust)                  │
│  crates/tkless-core/src/mcp/server.rs                  │
├────────────────────────────────────────────────────────┤
│ 核心调度层:                                            │
│ - initialize / ping / tools/list / tools/call          │
│ - $/progress 进度通知实时推流                          │
├────────────────────────────────────────────────────────┤
│ 工具注册池 (Tool Registry - 21 Tools):                  │
│ 1. fetch_webpage (通用/全平台自适应抓取与降噪清洗)     │
│ 2. web_search (免 Key 双引擎全网搜索与定向生态检索)    │
│ 3. crawl_site (整站递归爬取与知识库聚合)               │
│ 4. export_tables (网页表格提取与 SQL/CSV/JSON 转换)    │
│ 5. chunk_markdown (语义分块与 RAG 向量切分)            │
│ 6. summarize_content (本地启发式无 Key 核心要点摘要)   │
│ 7. take_screenshot (CDP 无头浏览器长截图)              │
│ 8. diff_documents (Markdown 语义差异高亮比对)          │
│ ... 以及各类平台专属探测与清洗工具                    │
└────────────────────────────────────────────────────────┘
```

> 💡 **前缀自适应 (Prefix-Agnostic Dispatch)**：
> 服务端对所有工具调用均支持前缀自适应处理。无论是调用 `export_tables` 还是 `tkless_export_tables`（亦兼容历史 `tkless_export_tables` 前缀），服务端内部均会自动归一化分发，无缝兼容各类 MCP 宿主。

---

## 2. 核心专属 MCP 工具规范 (已全部落地)

为提升 Agent 调用时的决策精准度与参数收敛性，TkLess 提供了 5 个高频专用原子工具：

### 2.1 `export_tables` (表格提取与数据转换)
- **定位**：专门处理财经财报、数据榜单、维基百科信息表，直接产出开箱即用的 SQL 或 CSV，免除大模型繁重的数据格式重排负担。
- **支持输入**：支持直接传入 `url` 自动网络抓取，或传入本地 `html` 片段。
- **Schema 定义**：
```json
{
  "name": "export_tables",
  "description": "Extract all data tables from a webpage URL or raw HTML string into clean tabular datasets: SQL DDL + INSERT statements, CSV, TSV, GFM Markdown, or JSON records.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Target webpage URL to fetch and extract tables from" },
      "html": { "type": "string", "description": "Optional raw HTML string containing one or more <table> elements" },
      "format": {
        "type": "string",
        "enum": ["sql", "csv", "tsv", "json", "markdown"],
        "default": "sql",
        "description": "Target export format (default: 'sql' for SQLite/PostgreSQL compatibility)"
      },
      "table_name": { "type": "string", "default": "extracted_table", "description": "Custom SQL table name identifier" },
      "table_index": {
        "type": "integer",
        "description": "Optional 0-indexed specific table to extract. If omitted, extracts all tables."
      }
    }
  }
}
```

### 2.2 `chunk_markdown` (RAG 语义感知切块)
- **定位**：对目标技术文档进行标题感知分块（H1-H6 继承与代码块闭合保护），输出包含层级 Breadcrumbs 的切块数组供向量库入库。
- **支持输入**：支持直接传入 `url` 自动网络抓取并清洗，或直接传入 `markdown` 文本。
- **Schema 定义**：
```json
{
  "name": "chunk_markdown",
  "description": "Fetches a webpage URL or takes a Markdown string, and segments content into semantic chunks respecting heading hierarchy and protecting code fences for RAG vector DB ingestion.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Webpage URL to fetch and chunk" },
      "markdown": { "type": "string", "description": "Optional Markdown content string to chunk directly" },
      "chunk_size": { "type": "integer", "default": 500, "description": "Target token size per chunk" },
      "chunk_overlap": { "type": "integer", "default": 50, "description": "Token overlap between chunks" },
      "tokenizer": { "type": "string", "enum": ["o200k", "cl100k", "heuristic"], "default": "o200k" },
      "format": { "type": "string", "enum": ["json", "markdown"], "default": "json" }
    }
  }
}
```

### 2.3 `summarize_content` (本地启发式无 Key 核心要点摘要)
- **定位**：在零 API 消耗的前提下，运用 TextRank 与词频算法快速提取 3~5 条核心结论，帮助 Agent 预先研判页面价值。
- **支持输入**：支持直接传入 `url` 自动抓取分析，或直接传入 `text`。
- **Schema 定义**：
```json
{
  "name": "summarize_content",
  "description": "Distills salient key takeaway bullet points (TL;DR) from a webpage URL or raw text/Markdown using term frequency and positional heuristics without requiring external LLM API calls.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Article URL to fetch and summarize" },
      "text": { "type": "string", "description": "Optional raw text or Markdown content to summarize directly" },
      "max_bullets": { "type": "integer", "default": 3, "description": "Max bullet points" },
      "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown" }
    }
  }
}
```

### 2.4 `take_screenshot` (CDP 页面高清快照)
- **定位**：利用 Chrome DevTools Protocol 对目标复杂 SPA 或仪表盘页面截取完整高清视口图片（Base64 或本地文件路径）。
- **Schema 定义**：
```json
{
  "name": "take_screenshot",
  "description": "Capture a full-page or viewport screenshot of a webpage using headless Chrome DevTools Protocol.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Target webpage URL" },
      "output_path": { "type": "string", "description": "Local path to save the screenshot (.png / .jpg)" },
      "format": { "type": "string", "enum": ["png", "jpeg"], "default": "png" },
      "full_page": { "type": "boolean", "default": true, "description": "Whether to capture the entire scrollable height" },
      "cdp_endpoint": { "type": "string", "description": "CDP endpoint (default: http://127.0.0.1:9222)" },
      "cdp_wait_ms": { "type": "integer", "default": 1000 }
    },
    "required": ["url"]
  }
}
```

### 2.5 `diff_documents` (Markdown 语义差异高亮比对)
- **定位**：快速对比两份 Markdown 文档或网页版本，输出带行号与统计比例的统一差异报表。
- **Schema 定义**：
```json
{
  "name": "diff_documents",
  "description": "Computes line-level differences, change metrics, and unified diff between two versions of text or Markdown documents.",
  "parameters": {
    "type": "object",
    "properties": {
      "old_text": { "type": "string", "description": "Original text version" },
      "new_text": { "type": "string", "description": "Updated text version" },
      "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown" }
    },
    "required": ["old_text", "new_text"]
  }
}
```

---

## 3. 宿主集成配置示例

### 3.1 Claude Desktop 配置 (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "tkless": {
      "command": "tkless",
      "args": ["mcp"]
    }
  }
}
```

### 3.2 Cursor 配置 (`.cursor/mcp.json`)

```json
{
  "mcpServers": {
    "tkless": {
      "command": "tkless",
      "args": ["mcp"]
    }
  }
}
```

---

## 4. MCP 流式进度通知规范 (`$/progress`)

在处理长耗时任务（如整站爬取 `crawl_site`、大批量并发检索或 CDP 截图时），TkLess 服务端会通过标准 JSON-RPC 2.0 异步推流进度事件：

```json
{
  "jsonrpc": "2.0",
  "method": "$/progress",
  "params": {
    "progressToken": "req-1024",
    "progress": 45,
    "total": 100,
    "message": "Crawling page 45/100: https://docs.rs/tokio/latest/tokio/sync/index.html"
  }
}
```
宿主界面（如 Cursor 或 Claude）可实时展示动态进度条，杜绝长耗时任务导致的假死与超时断连。
