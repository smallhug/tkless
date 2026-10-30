use super::{
    format_doc_result, ExtractionMode, ExtractionOptions, McpServer, McpToolCallResult,
    OutputFormat, TableExportFormat, TokenizerKind, Value,
};
use url::Url;

impl McpServer {
    pub(super) async fn call_clean_html(&self, args: &Value) -> McpToolCallResult {
        let html = match args.get("html").and_then(|v| v.as_str()) {
            Some(h) if !h.trim().is_empty() => h,
            _ => return McpToolCallResult::error("Missing required parameter 'html'"),
        };

        let base_url = args.get("base_url").and_then(|v| v.as_str());

        let format = match args.get("format").and_then(|v| v.as_str()) {
            Some("json") => OutputFormat::Json,
            Some("text") => OutputFormat::Text,
            Some("html") => OutputFormat::Html,
            _ => OutputFormat::Markdown,
        };

        let mode = match args.get("mode").and_then(|v| v.as_str()) {
            Some("full") => ExtractionMode::Full,
            Some("raw") => ExtractionMode::Raw,
            _ => ExtractionMode::Article,
        };

        let selector = args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let remove_selector = args
            .get("remove_selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let schema = args.get("schema").map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        });

        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let strip_images = args
            .get("strip_images")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let strip_links = args
            .get("strip_links")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let frontmatter = args
            .get("frontmatter")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let chunk = args.get("chunk").and_then(|v| v.as_bool()).unwrap_or(false);
        let chunk_size = args
            .get("chunk_size")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let chunk_overlap = args
            .get("chunk_overlap")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let tables_only = args
            .get("tables_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let table_format = match args.get("table_format").and_then(|v| v.as_str()) {
            Some("csv") => TableExportFormat::Csv,
            Some("tsv") => TableExportFormat::Tsv,
            Some("json") => TableExportFormat::Json,
            Some("sql") => TableExportFormat::Sql,
            _ => TableExportFormat::Markdown,
        };
        let stats = args.get("stats").and_then(|v| v.as_bool()).unwrap_or(false);
        let summarize = args
            .get("summarize")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let options = ExtractionOptions {
            mode,
            format,
            selector,
            schema,
            schema_strict: args
                .get("schema_strict")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            max_tokens,
            strip_images,
            strip_links,
            frontmatter,
            chunk,
            chunk_size,
            chunk_overlap,
            tables_only,
            table_format,
            stats,
            remove_selector,
            summarize,
            ..Default::default()
        };

        match self.engine.process_html(html, base_url, &options) {
            Ok(doc) => format_doc_result(&doc, format),
            Err(e) => McpToolCallResult::from_error(&e),
        }
    }

    pub(super) async fn call_scaffold_schema(&self, args: &Value) -> McpToolCallResult {
        let raw_html = if let Some(html) = args.get("html").and_then(|v| v.as_str()) {
            html.to_string()
        } else if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
            let parsed_url = match url::Url::parse(url) {
                Ok(u) => u,
                Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
            };

            if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
                return McpToolCallResult::error(format!("SSRF blocked: {e}"));
            }

            if let Some(cdp_endpoint) = args.get("cdp_endpoint").and_then(|v| v.as_str()) {
                let cdp_opts = crate::cdp::CdpOptions {
                    allow_private_addresses: self
                        .engine
                        .fetcher()
                        .options()
                        .allow_private_addresses,
                    endpoint: cdp_endpoint.to_string(),
                    wait_ms: Some(1000),
                    timeout_ms: None,
                    user_agent: self.engine.fetcher().options().user_agent.clone(),
                    cookie: self.engine.fetcher().resolve_cookie_for_url(&parsed_url),
                    ..Default::default()
                };
                match crate::cdp::fetch_html_via_cdp(url, &cdp_opts).await {
                    Ok(h) => h,
                    Err(e) => return McpToolCallResult::error(format!("CDP fetch failed: {e}")),
                }
            } else {
                match self.engine.fetcher().fetch(url).await {
                    Ok(resp) => resp.html,
                    Err(e) => return McpToolCallResult::error(format!("Fetch URL failed: {e}")),
                }
            }
        } else {
            return McpToolCallResult::error(
                "Either 'url' or 'html' parameter must be provided to scaffold_schema",
            );
        };

        let report = crate::schema::scaffold_dom_skeleton(&raw_html);
        McpToolCallResult::success(report.summary_markdown)
    }

    pub(super) async fn call_compare_similarity(&self, args: &Value) -> McpToolCallResult {
        let text1 = match args.get("text1").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t,
            _ => return McpToolCallResult::error("Missing required parameter 'text1'"),
        };
        let text2 = match args.get("text2").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t,
            _ => return McpToolCallResult::error("Missing required parameter 'text2'"),
        };

        let max_dist = args
            .get("max_hamming_distance")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as u32;

        let hash1 = crate::dedup::simhash(text1);
        let hash2 = crate::dedup::simhash(text2);
        let dist = crate::dedup::hamming_distance(hash1, hash2);
        let is_duplicate = dist <= max_dist;
        let similarity_pct = (1.0 - (dist as f64 / 64.0)) * 100.0;

        let res = serde_json::json!({
            "simhash1": format!("0x{:016x}", hash1),
            "simhash2": format!("0x{:016x}", hash2),
            "hamming_distance": dist,
            "threshold": max_dist,
            "is_near_duplicate": is_duplicate,
            "similarity_percentage": (similarity_pct * 100.0).round() / 100.0
        });

        match serde_json::to_string_pretty(&res) {
            Ok(json_str) => McpToolCallResult::success(json_str),
            Err(e) => McpToolCallResult::error(format!("Failed to serialize result: {e}")),
        }
    }

    pub(super) async fn call_summarize_content(&self, args: &Value) -> McpToolCallResult {
        let text = if let Some(t) = args.get("text").and_then(|v| v.as_str()) {
            t.to_string()
        } else if let Some(u) = args.get("url").and_then(|v| v.as_str()) {
            let fetch_opts = ExtractionOptions {
                mode: ExtractionMode::Article,
                format: OutputFormat::Markdown,
                ..Default::default()
            };
            match self.engine.process_url(u, &fetch_opts).await {
                Ok(doc) => doc.content,
                Err(e) => {
                    return McpToolCallResult::error(format!(
                        "Failed to fetch and process URL '{u}': {e}"
                    ))
                }
            }
        } else {
            return McpToolCallResult::error(
                "Missing required parameter: either 'url' or 'text' must be provided",
            );
        };

        let max_bullets = args
            .get("max_bullets")
            .or_else(|| args.get("max_points"))
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(1, 10) as usize;

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let bullets = crate::summarizer::extract_key_takeaways(&text, max_bullets);

        match format {
            "json" => match serde_json::to_string_pretty(&bullets) {
                Ok(json_str) => McpToolCallResult::success(json_str),
                Err(e) => {
                    McpToolCallResult::error(format!("Failed to serialize summary to JSON: {e}"))
                }
            },
            _ => {
                let mut md = String::new();
                md.push_str("### 核心要点速览 (Key Takeaways)\n\n");
                for bullet in bullets {
                    md.push_str(&format!("- {}\n", bullet.trim()));
                }
                McpToolCallResult::success(md)
            }
        }
    }

    pub(super) async fn call_chunk_markdown(&self, args: &Value) -> McpToolCallResult {
        let markdown = if let Some(m) = args.get("markdown").and_then(|v| v.as_str()) {
            m.to_string()
        } else if let Some(u) = args.get("url").and_then(|v| v.as_str()) {
            let fetch_opts = ExtractionOptions {
                mode: ExtractionMode::Article,
                format: OutputFormat::Markdown,
                ..Default::default()
            };
            match self.engine.process_url(u, &fetch_opts).await {
                Ok(doc) => doc.content,
                Err(e) => {
                    return McpToolCallResult::error(format!(
                        "Failed to fetch and process URL '{u}': {e}"
                    ))
                }
            }
        } else {
            return McpToolCallResult::error(
                "Missing required parameter: either 'url' or 'markdown' must be provided",
            );
        };

        let chunk_size = args
            .get("chunk_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(500) as usize;
        let chunk_overlap = args
            .get("chunk_overlap")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;
        let tokenizer = match args.get("tokenizer").and_then(|v| v.as_str()) {
            Some("cl100k") => TokenizerKind::Cl100k,
            Some("heuristic") => TokenizerKind::Heuristic,
            _ => TokenizerKind::O200k,
        };
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("json");

        let chunks =
            crate::chunker::chunk_markdown(&markdown, chunk_size, chunk_overlap, tokenizer);

        if format == "markdown" {
            let mut out = String::new();
            out.push_str(&format!("# 语义切块结果 (共 {} 块)\n\n", chunks.len()));
            for chunk in &chunks {
                let breadcrumbs = if chunk.heading_path.is_empty() {
                    "正文".to_string()
                } else {
                    chunk.heading_path.join(" > ")
                };
                out.push_str(&format!(
                    "### 切块 #{}: {} ({} Tokens / {} 字符)\n\n{}\n\n---\n\n",
                    chunk.index + 1,
                    breadcrumbs,
                    chunk.token_count,
                    chunk.content.chars().count(),
                    chunk.content
                ));
            }
            McpToolCallResult::success(out)
        } else {
            match serde_json::to_string_pretty(&chunks) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => {
                    McpToolCallResult::error(format!("Failed to serialize chunks to JSON: {e}"))
                }
            }
        }
    }

    pub(super) async fn call_split_document(&self, args: &Value) -> McpToolCallResult {
        let markdown = match args.get("markdown").and_then(|v| v.as_str()) {
            Some(m) if !m.trim().is_empty() => m,
            _ => return McpToolCallResult::error("Missing required parameter 'markdown'"),
        };

        let max_level = args
            .get("max_heading_level")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;

        let sections = crate::splitter::split_markdown_by_headings(markdown, max_level);
        match serde_json::to_string_pretty(&sections) {
            Ok(json_str) => McpToolCallResult::success(json_str),
            Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
        }
    }

    pub(super) async fn call_diff_content(&self, args: &Value) -> McpToolCallResult {
        let old_text = match args.get("old_text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return McpToolCallResult::error("Missing required parameter 'old_text'"),
        };
        let new_text = match args.get("new_text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return McpToolCallResult::error("Missing required parameter 'new_text'"),
        };

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let report = crate::diff::compute_diff(old_text, new_text);

        if format == "json" {
            match serde_json::to_string_pretty(&report) {
                Ok(s) => McpToolCallResult::success(s),
                Err(e) => McpToolCallResult::error(format!("Serialization error: {e}")),
            }
        } else {
            let mut md = String::new();
            md.push_str("# 文档版本差异与变更分析 (Content Diff Report)\n\n");
            md.push_str(&format!(
                "> 新增行数: `+{}` | 删除行数: `-{}` | 未变行数: `{}` | 变更比例: `{:.1}%`\n\n",
                report.added_lines,
                report.removed_lines,
                report.unchanged_lines,
                report.change_ratio
            ));
            md.push_str("```diff\n");
            md.push_str(&report.unified_diff);
            md.push_str("\n```");
            McpToolCallResult::success(md)
        }
    }

    pub(super) async fn call_export_tables(&self, args: &Value) -> McpToolCallResult {
        let html = if let Some(h) = args.get("html").and_then(|v| v.as_str()) {
            h.to_string()
        } else if let Some(u) = args.get("url").and_then(|v| v.as_str()) {
            let parsed_url = match Url::parse(u) {
                Ok(p) => p,
                Err(e) => return McpToolCallResult::error(format!("Invalid URL: {e}")),
            };
            if let Err(e) = self.engine.fetcher().validate_ssrf(&parsed_url).await {
                return McpToolCallResult::error(format!("SSRF blocked: {e}"));
            }
            match self.engine.fetcher().fetch(u).await {
                Ok(res) => res.html,
                Err(e) => {
                    return McpToolCallResult::error(format!("Failed to fetch URL '{u}': {e}"))
                }
            }
        } else {
            return McpToolCallResult::error(
                "Missing required parameter: either 'url' or 'html' must be provided",
            );
        };

        let format_str = args.get("format").and_then(|v| v.as_str()).unwrap_or("sql");

        let table_format = match format_str {
            "sql" => TableExportFormat::Sql,
            "csv" => TableExportFormat::Csv,
            "tsv" => TableExportFormat::Tsv,
            "json" => TableExportFormat::Json,
            _ => TableExportFormat::Markdown,
        };

        let table_name = args
            .get("table_name")
            .and_then(|v| v.as_str())
            .unwrap_or("extracted_table");

        let doc = scraper::Html::parse_document(&html);
        let mut tables = crate::extractor::extract_tables(&doc);

        if tables.is_empty() {
            return McpToolCallResult::error(
                "未在提供的页面或 HTML 中检测到有效的 <table> 表格元素",
            );
        }

        if let Some(idx) = args
            .get("table_index")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
        {
            if idx >= tables.len() {
                return McpToolCallResult::error(format!(
                    "table_index {} 超出范围，页面仅包含 {} 个表格 (有效索引: 0-{})",
                    idx,
                    tables.len(),
                    tables.len().saturating_sub(1)
                ));
            }
            tables = vec![tables.swap_remove(idx)];
        }

        if table_format == TableExportFormat::Sql {
            let sql_ddl = tables
                .iter()
                .enumerate()
                .map(|(idx, t)| {
                    let name = if tables.len() > 1 {
                        format!("{table_name}_{}", idx + 1)
                    } else {
                        table_name.to_string()
                    };
                    t.to_sql(&name)
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            McpToolCallResult::success(sql_ddl)
        } else {
            let formatted = crate::model::ExtractedTable::format_tables(&tables, table_format);
            McpToolCallResult::success(formatted)
        }
    }
}
