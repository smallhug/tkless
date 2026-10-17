# 02 - Model Context Protocol (MCP) 工具扩展架构设计

本文档详细描述 PageClaw 的原生 Model Context Protocol (MCP) 服务端架构，以及向主流 AI Agent（Claude Desktop、Cursor、Zed 等）暴露的高级工具扩展方案。

---

## 1. 架构总览

PageClaw 内置原生轻量级 MCP 服务端（位于 `crates/pageclaw-core/src/mcp`），无需 Node.js 或 Python 运行时，直接通过标准输入输出（`stdio`）以 JSON-RPC 2.0 协议与 AI 宿主进程高效通信：

```text
┌────────────────────────────────────────────────────────┐
│   AI 宿主环境 (Claude Desktop / Cursor / Zed Agent)    │
└───────────────────────────┬────────────────────────────┘
                            │ JSON-RPC 2.0 (stdio)
┌───────────────────────────▼────────────────────────────┐
│              PageClaw MCP Server (Rust)                │
│  crates/pageclaw-core/src/mcp/server.rs                │
├────────────────────────────────────────────────────────┤
│ 核心调度层:                                            │
│ - initialize / ping / tools/list / tools/call          │
│ - $/progress 进度通知实时推流                          │
├────────────────────────────────────────────────────────┤
│ 工具注册池 (Tool Registry):                            │
│ 1. fetch_webpage (通用/全平台自适应抓取与降噪清洗)     │
│ 2. search_web (免 Key 双引擎全网搜索与定向生态检索)    │
│ 3. crawl_site (整站递归爬取与知识库聚合)               │
│                                                        │
│ [规划中新增扩展工具 (Planned Extensions)]:              │
│ 4. export_tables (网页表格提取与 SQL/CSV 转换)         │
│ 5. chunk_markdown (语义分块与 RAG 向量切分)            │
│ 6. summarize_content (本地启发式无 Key 核心要点摘要)    │
│ 7. take_screenshot (CDP 无头浏览器长截图)              │
│ 8. diff_documents (Markdown 语义差异高亮比对)          │
└────────────────────────────────────────────────────────┘
```

---

## 2. 规划扩展的专属 MCP 工具定义

为提升 Agent 调用时的决策精准度与参数收敛性，计划在现有大而全的 `fetch_webpage` 之外，拆分并暴露以下 5 个高频专用原子工具：

### 2.1 `pageclaw_export_tables` (表格提取与数据转换)
- **定位**：专门处理财经财报、数据榜单、维基百科信息表，直接产出开箱即用的 SQL 或 CSV，免除大模型繁重的数据格式重排负担。
- **Schema 定义**：
```json
{
  "name": "pageclaw_export_tables",
  "description": "Extract all data tables from a webpage into SQL DDL/INSERT statements, CSV, TSV, or JSON records.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Target webpage URL" },
      "format": {
        "type": "string",
        "enum": ["sql", "csv", "tsv", "json", "markdown"],
        "default": "sql",
        "description": "Target export format (default: 'sql' for SQLite/PostgreSQL compatibility)"
      },
      "table_index": {
        "type": "integer",
        "description": "Optional 0-indexed specific table to extract. If omitted, extracts all tables."
      }
    },
    "required": ["url"]
  }
}
```

### 2.2 `pageclaw_chunk_markdown` (RAG 语义感知切块)
- **定位**：对目标技术文档进行标题感知分块（H1-H6 继承与代码块闭合保护），输出包含层级 Breadcrumbs 的切块数组供向量库入库。
- **Schema 定义**：
```json
{
  "name": "pageclaw_chunk_markdown",
  "description": "Fetch webpage and split its content into semantic chunks respecting heading hierarchy and protecting code fences.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Webpage URL to fetch and chunk" },
      "chunk_size": { "type": "integer", "default": 500, "description": "Target token size per chunk" },
      "chunk_overlap": { "type": "integer", "default": 50, "description": "Token overlap between chunks" }
    },
    "required": ["url"]
  }
}
```

### 2.3 `pageclaw_summarize` (本地启发式无 Key 核心要点摘要)
- **定位**：在零 API 消耗的前提下，运用 TextRank 与词频算法快速提取 3~5 条核心结论，帮助 Agent 预先研判页面价值。
- **Schema 定义**：
```json
{
  "name": "pageclaw_summarize",
  "description": "Quickly extract 3-5 key takeaways from any article without consuming LLM API credits.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Article URL" },
      "max_points": { "type": "integer", "default": 5, "description": "Max bullet points" }
    },
    "required": ["url"]
  }
}
```

### 2.4 `pageclaw_screenshot` (CDP 页面高清快照)
- **定位**：利用 Chrome DevTools Protocol 对目标复杂 SPA 或仪表盘页面截取完整高清视口图片（Base64 或本地文件路径）。
- **Schema 定义**：
```json
{
  "name": "pageclaw_screenshot",
  "description": "Capture a full-page or viewport screenshot of a webpage using headless Chrome DevTools Protocol.",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Target webpage URL" },
      "output_path": { "type": "string", "description": "Local path to save the screenshot (.png / .jpg)" },
      "full_page": { "type": "boolean", "default": true, "description": "Whether to capture the entire scrollable height" }
    },
    "required": ["url", "output_path"]
  }
}
```

---

## 3. 宿主集成配置示例

### 3.1 Claude Desktop 配置 (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "pageclaw": {
      "command": "pageclaw",
      "args": ["mcp"]
    }
  }
}
```

### 3.2 Cursor 配置 (`.cursor/mcp.json`)

```json
{
  "mcpServers": {
    "pageclaw": {
      "command": "pageclaw",
      "args": ["mcp"]
    }
  }
}
```

---

## 4. MCP 流式进度通知规范 (`$/progress`)

在处理长耗时任务（如整站爬取 `crawl_site`、大批量并发检索或 CDP 截图时），PageClaw 服务端会通过标准 JSON-RPC 2.0 异步推流进度事件：

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
