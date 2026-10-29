//! Autonomous Multi-Hop Deep Research Pipeline.
//!
//! Provides an end-to-end autonomous research agent loop:
//! 1. Analyzes and decomposes complex user topics into multi-angle research dimensions.
//! 2. Executes Bing search for each research sub-query (Hop 1).
//! 3. Extracts clean webpage content, token budgets, and local key takeaways.
//! 4. Performs reference tracking and citation exploration (Hop 2) for deep authoritative evidence.
//! 5. Synthesizes findings into a comprehensive, publication-grade research report with citations and telemetry.

use crate::error::TklessError;
use crate::model::{ExtractionOptions, OutputFormat};
use crate::search::{web_search, SearchOptions, SearchResultItem};
use crate::summarizer::extract_key_takeaways;
use crate::TklessEngine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Instant;
use tracing::{debug, info};

/// Options controlling autonomous multi-hop deep research.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchOptions {
    /// Depth of research hops (1: single-hop broad search, 2: two-hop follow-up citation tracking). Default: 2.
    pub depth: usize,
    /// Number of sub-queries/angles to decompose topic into (2-5). Default: 3.
    pub breadth: usize,
    /// Maximum number of total pages to fetch and analyze (3-20). Default: 8.
    pub max_pages: usize,
    /// Maximum tokens per page to extract during deep analysis. Default: 1500.
    pub max_tokens_per_page: usize,
    /// Overall token budget for the compiled research report. Default: 8000.
    pub max_tokens: Option<usize>,
    /// Deduplicate near-duplicate contents via SimHash. Default: true.
    pub dedup: bool,
    /// Presentation format: markdown (default) or json.
    pub format: OutputFormat,
}

impl Default for ResearchOptions {
    fn default() -> Self {
        Self {
            depth: 2,
            breadth: 3,
            max_pages: 8,
            max_tokens_per_page: 1500,
            max_tokens: Some(8000),
            dedup: true,
            format: OutputFormat::Markdown,
        }
    }
}

/// Cited source reference used in the research report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchCitation {
    pub id: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub hop: usize,
    pub key_points: Vec<String>,
}

/// Structured sub-query dimension with gathered findings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchDimension {
    pub angle: String,
    pub query: String,
    pub findings: Vec<String>,
    pub source_ids: Vec<usize>,
}

/// Telemetry metrics for research operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchTelemetry {
    pub total_queries: usize,
    pub pages_fetched: usize,
    pub total_tokens_extracted: usize,
    pub elapsed_ms: u64,
}

/// Comprehensive deep research report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepResearchReport {
    pub topic: String,
    #[serde(default)]
    pub truncated: bool,
    pub executive_summary: String,
    pub key_takeaways: Vec<String>,
    pub dimensions: Vec<ResearchDimension>,
    pub comparative_analysis: Option<String>,
    pub citations: Vec<ResearchCitation>,
    pub telemetry: ResearchTelemetry,
}

/// Cleans a research topic by removing parenthetical metadata and generic temporal/filler prefixes.
pub fn clean_core_entity(topic: &str) -> String {
    let mut clean = topic.trim();

    // 1. Strip parenthetical notes like (含其派生的 2 个深度探针) or (含...) or (...)
    if let Some(pos) = clean.find('(').or_else(|| clean.find('（')) {
        let prefix = clean[..pos].trim();
        if !prefix.is_empty() {
            clean = prefix;
        }
    }

    // 2. Strip common fluff / temporal leading modifiers
    let filler_prefixes = [
        "近期发布的",
        "最新发布的",
        "最近发布的",
        "近期",
        "最新",
        "最近",
        "关于",
        "相关的",
        "全面调研",
        "深度调研",
        "调研",
        "深度解析",
        "recently released",
        "latest",
        "recent",
        "state of the art",
        "overview of",
    ];

    let mut result = clean.to_string();
    for prefix in filler_prefixes {
        if result.to_lowercase().starts_with(prefix) {
            result = result[prefix.len()..].trim().to_string();
        }
    }

    if result.is_empty() {
        topic.trim().to_string()
    } else {
        result
    }
}

/// Decomposes a research topic into multiple sub-queries / angles.
pub fn decompose_topic(topic: &str, breadth: usize) -> Vec<(String, String)> {
    let raw = topic.trim();
    let core = clean_core_entity(raw);
    let b = breadth.clamp(2, 5);

    let is_cjk = core.chars().any(|c| (c as u32) >= 0x4E00);

    if is_cjk {
        let mut angles = Vec::new();
        angles.push((
            "核心架构与原理 (Architecture & Fundamentals)".to_string(),
            format!("{core} 核心架构 原理"),
        ));
        angles.push((
            "优缺点对比与性能基准 (Tradeoffs & Benchmarks)".to_string(),
            format!("{core} 优缺点 性能基准"),
        ));
        if b >= 3 {
            angles.push((
                "生产实践与避坑指南 (Best Practices & Pitfalls)".to_string(),
                format!("{core} 生产实践 最佳实践"),
            ));
        }
        if b >= 4 {
            angles.push((
                "生态体系与前沿演进 (Ecosystem & Future Roadmap)".to_string(),
                format!("{core} 生态发展 最新进展"),
            ));
        }
        if b >= 5 {
            angles.push((
                "行业落地与典型案例 (Case Studies & Applications)".to_string(),
                format!("{core} 行业应用 典型案例"),
            ));
        }
        angles
    } else {
        let mut angles = Vec::new();
        angles.push((
            "Architecture & Fundamentals".to_string(),
            format!("{core} architecture internals"),
        ));
        angles.push((
            "Tradeoffs & Performance Benchmarks".to_string(),
            format!("{core} benchmarks comparison"),
        ));
        if b >= 3 {
            angles.push((
                "Production Best Practices".to_string(),
                format!("{core} production best practices"),
            ));
        }
        if b >= 4 {
            angles.push((
                "Ecosystem & Future Roadmap".to_string(),
                format!("{core} ecosystem trends roadmap"),
            ));
        }
        if b >= 5 {
            angles.push((
                "Case Studies & Industry Adoption".to_string(),
                format!("{core} real world case studies"),
            ));
        }
        angles
    }
}

/// Executes an autonomous multi-hop deep web research loop.
pub async fn deep_research(
    engine: &TklessEngine,
    topic: &str,
    options: &ResearchOptions,
) -> Result<DeepResearchReport, TklessError> {
    let start_time = Instant::now();
    let clean_topic = topic.trim();
    if clean_topic.is_empty() {
        return Err(TklessError::InvalidArgument(
            "Research topic cannot be empty".to_string(),
        ));
    }

    if !(1..=2).contains(&options.depth)
        || !(2..=5).contains(&options.breadth)
        || options.max_pages == 0
    {
        return Err(TklessError::InvalidArgument(
            "Research depth must be 1-2, breadth 2-5, and max_pages positive".into(),
        ));
    }
    let core_entity = clean_core_entity(clean_topic);

    info!(
        topic = clean_topic,
        core_entity = %core_entity,
        depth = options.depth,
        breadth = options.breadth,
        "启动多跳自主深度调研流水线"
    );

    // Phase 1: Query Decomposition
    let angles = decompose_topic(clean_topic, options.breadth);
    let total_queries = angles.len();

    // Phase 2: Hop 1 - Concurrently search for each angle
    let mut dimension_search_results: Vec<(String, String, Vec<SearchResultItem>)> = Vec::new();
    let mut seen_urls = HashSet::new();

    for (angle_name, sub_query) in &angles {
        debug!(angle = %angle_name, query = %sub_query, "检索研究维度候选资料");
        let mut unique_items = Vec::new();

        match web_search(
            engine,
            sub_query,
            &SearchOptions {
                limit: 4,
                ..Default::default()
            },
        )
        .await
        {
            Ok(results) => {
                for item in results.results {
                    let norm = crate::search::normalize_url_for_dedup(&item.url);
                    if seen_urls.insert(norm) {
                        unique_items.push(item);
                    }
                }
            }
            Err(e) => {
                debug!(query = %sub_query, error = %e, "维度检索异常");
            }
        }

        // Graceful fallback: If angle query returned 0 items (e.g. Due to search term drop), retry with clean core entity!
        if unique_items.is_empty() {
            debug!(
                angle = %angle_name,
                core_entity = %core_entity,
                "维度检索未命中，使用核心实体保底检索"
            );
            if let Ok(fallback_items) = web_search(
                engine,
                &core_entity,
                &SearchOptions {
                    limit: 4,
                    ..Default::default()
                },
            )
            .await
            {
                for item in fallback_items.results {
                    let norm = crate::search::normalize_url_for_dedup(&item.url);
                    if seen_urls.insert(norm) {
                        unique_items.push(item);
                    }
                }
            }
        }

        dimension_search_results.push((angle_name.clone(), sub_query.clone(), unique_items));
    }

    // Phase 3: Parallel Content Fetching & Extraction (Hop 1 Pages)
    let max_hop1_pages = options.max_pages.min(8);
    let mut page_fetch_queue = Vec::new();

    for (dim_idx, (_angle, _q, items)) in dimension_search_results.iter().enumerate() {
        for item in items.iter().take(2) {
            if page_fetch_queue.len() < max_hop1_pages {
                page_fetch_queue.push((dim_idx, item.clone(), 1usize));
            }
        }
    }

    let mut extraction_opts = ExtractionOptions {
        max_tokens: Some(options.max_tokens_per_page),
        ..Default::default()
    };
    extraction_opts.include_metadata = true;
    extraction_opts.extract_links = true;

    let mut citations = Vec::new();
    let mut seen_simhashes = Vec::new();
    let mut total_tokens = 0usize;
    let mut candidate_hop2_urls: Vec<(usize, String, String)> = Vec::new();

    let mut dimensions: Vec<ResearchDimension> = angles
        .into_iter()
        .map(|(angle, query)| ResearchDimension {
            angle,
            query,
            findings: Vec::new(),
            source_ids: Vec::new(),
        })
        .collect();

    // Concurrently fetch Hop 1 pages
    let mut join_set = tokio::task::JoinSet::new();

    for (dim_idx, item, hop) in page_fetch_queue {
        let eng = engine.clone();
        let opts = extraction_opts.clone();
        join_set.spawn(async move {
            let res = eng.process_url(&item.url, &opts).await;
            (dim_idx, item, hop, res)
        });
    }

    while let Some(res) = join_set.join_next().await {
        if let Ok((dim_idx, item, hop, doc_res)) = res {
            match doc_res {
                Ok(doc) => {
                    if options.dedup {
                        let h = crate::dedup::simhash(&doc.content);
                        if seen_simhashes
                            .iter()
                            .any(|&prev| crate::dedup::is_near_duplicate(prev, h, 3))
                        {
                            debug!(url = %item.url, "过滤近似重复页面");
                            continue;
                        }
                        seen_simhashes.push(h);
                    }

                    let points = extract_key_takeaways(&doc.content, 4);
                    total_tokens += doc.token_count;

                    // Collect potential Hop 2 external citation targets
                    if let Some(ref links) = doc.links {
                        for link in links {
                            if (link.url.starts_with("http://") || link.url.starts_with("https://"))
                                && !link.url.contains("login")
                                && !link.url.contains("signup")
                                && link.text.len() >= 6
                            {
                                candidate_hop2_urls.push((
                                    dim_idx,
                                    link.text.clone(),
                                    link.url.clone(),
                                ));
                            }
                        }
                    }

                    let citation_id = citations.len() + 1;
                    let citation = ResearchCitation {
                        id: citation_id,
                        title: if item.title.is_empty() {
                            doc.title.unwrap_or_else(|| "文献源".into())
                        } else {
                            item.title
                        },
                        url: item.url,
                        snippet: item.snippet,
                        hop,
                        key_points: points.clone(),
                    };

                    if let Some(dim) = dimensions.get_mut(dim_idx) {
                        dim.source_ids.push(citation_id);
                        for pt in points {
                            let pt = format!("{pt} [文献 #{citation_id}]");
                            if !dim.findings.contains(&pt) {
                                dim.findings.push(pt);
                            }
                        }
                    }

                    citations.push(citation);
                }
                Err(err) => {
                    debug!(url = %item.url, error = %err, "研究页面获取异常");
                }
            }
        }
    }

    // Phase 4: Hop 2 Deep Citation Tracking (Follow Authoritative External Links)
    if options.depth >= 2 && citations.len() < options.max_pages && !candidate_hop2_urls.is_empty()
    {
        debug!(
            count = candidate_hop2_urls.len(),
            "Hop 2: 追踪权威引文链接深度延伸"
        );
        let remaining_budget = options.max_pages.saturating_sub(citations.len()).min(3);
        let mut hop2_fetch_queue = Vec::new();

        for (dim_idx, title, url) in candidate_hop2_urls {
            let norm = crate::search::normalize_url_for_dedup(&url);
            if seen_urls.insert(norm) && hop2_fetch_queue.len() < remaining_budget {
                hop2_fetch_queue.push((
                    dim_idx,
                    SearchResultItem {
                        title,
                        url,
                        snippet: "深度追踪引用源".to_string(),
                        engine: "citation".to_string(),
                    },
                    2usize,
                ));
            }
        }

        let mut hop2_join_set = tokio::task::JoinSet::new();
        for (dim_idx, item, hop) in hop2_fetch_queue {
            let eng = engine.clone();
            let opts = extraction_opts.clone();
            hop2_join_set.spawn(async move {
                let res = eng.process_url(&item.url, &opts).await;
                (dim_idx, item, hop, res)
            });
        }

        while let Some(res) = hop2_join_set.join_next().await {
            if let Ok((dim_idx, item, hop, Ok(doc))) = res {
                let points = extract_key_takeaways(&doc.content, 3);
                total_tokens += doc.token_count;

                let citation_id = citations.len() + 1;
                citations.push(ResearchCitation {
                    id: citation_id,
                    title: if item.title.is_empty() {
                        doc.title.unwrap_or_else(|| "延伸参考源".into())
                    } else {
                        item.title
                    },
                    url: item.url,
                    snippet: item.snippet,
                    hop,
                    key_points: points.clone(),
                });

                // Keep citation findings attached to their originating research dimension.
                if let Some(last_dim) = dimensions.get_mut(dim_idx) {
                    last_dim.source_ids.push(citation_id);
                    for pt in points {
                        let pt = format!("{pt} [文献 #{citation_id}]");
                        if !last_dim.findings.contains(&pt) {
                            last_dim.findings.push(pt);
                        }
                    }
                }
            }
        }
    }

    // Phase 5: Synthesis of Key Takeaways and Executive Summary
    let mut all_key_takeaways = Vec::new();
    for dim in &dimensions {
        for f in &dim.findings {
            if !all_key_takeaways.contains(f) && all_key_takeaways.len() < 8 {
                all_key_takeaways.push(f.clone());
            }
        }
    }

    let executive_summary = if all_key_takeaways.is_empty() {
        format!("课题「{clean_topic}」未获取到足够正文证据，无法形成结论。")
    } else {
        format!(
            "以下为已抓取来源的提取式要点，未经独立交叉核验：\n\n{}",
            all_key_takeaways
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n")
        )
    };

    let elapsed_ms = start_time.elapsed().as_millis() as u64;

    let telemetry = ResearchTelemetry {
        total_queries,
        pages_fetched: citations.len(),
        total_tokens_extracted: total_tokens,
        elapsed_ms,
    };

    Ok(DeepResearchReport {
        topic: clean_topic.to_string(),
        truncated: false,
        executive_summary,
        key_takeaways: all_key_takeaways,
        dimensions,
        comparative_analysis: None,
        citations,
        telemetry,
    })
}

/// Render the complete report within its budget, retaining valid JSON and explicit truncation.
pub fn render_deep_research(
    report: &DeepResearchReport,
    options: &ResearchOptions,
) -> Result<String, TklessError> {
    let tokenizer = crate::model::TokenizerKind::default();
    if options.format != OutputFormat::Json {
        let markdown = format_deep_research_markdown(report);
        return Ok(match options.max_tokens {
            Some(limit) => crate::token::truncate_to_tokens(&markdown, limit, tokenizer).0,
            None => markdown,
        });
    }
    let mut bounded = report.clone();
    loop {
        let json = serde_json::to_string(&bounded)
            .map_err(|e| TklessError::InvalidArgument(e.to_string()))?;
        if options
            .max_tokens
            .is_none_or(|limit| crate::token::count_tokens(&json, tokenizer) <= limit)
        {
            return Ok(json);
        }
        bounded.truncated = true;
        if bounded.comparative_analysis.take().is_some() {
            continue;
        }
        if let Some(citation) = bounded.citations.iter_mut().find(|c| !c.snippet.is_empty()) {
            citation.snippet.clear();
            continue;
        }
        if bounded.executive_summary.len() > 100 {
            bounded.executive_summary = "报告已按 Token 预算缩减，请查看保留的来源摘录。".into();
            continue;
        }
        if bounded.key_takeaways.pop().is_some() {
            continue;
        }
        if let Some(dim) = bounded
            .dimensions
            .iter_mut()
            .find(|d| !d.findings.is_empty())
        {
            dim.findings.pop();
            continue;
        }
        if let Some(citation) = bounded.citations.pop() {
            for dim in &mut bounded.dimensions {
                dim.source_ids.retain(|id| *id != citation.id);
            }
            continue;
        }
        if bounded.dimensions.pop().is_some() {
            continue;
        }
        return Err(TklessError::InvalidArgument("Research token budget is too small for the JSON report envelope; increase --max-tokens".into()));
    }
}

/// Formats a complete DeepResearchReport as clean, structured publication-grade Markdown.
pub fn format_deep_research_markdown(report: &DeepResearchReport) -> String {
    let mut out = String::new();

    out.push_str(&format!("# 深度研究综述: \"{}\"\n\n", report.topic));
    out.push_str(&format!(
        "> 调研维度: {} 个 | 深入分析文献: {} 篇 (含 Hop-2 引链) | 累计处理 Tokens: {} | 耗时: {:.2}s\n\n---\n\n",
        report.dimensions.len(),
        report.telemetry.pages_fetched,
        report.telemetry.total_tokens_extracted,
        report.telemetry.elapsed_ms as f64 / 1000.0
    ));

    out.push_str("## 执行摘要 (Executive Summary)\n\n");
    out.push_str(&report.executive_summary);
    out.push_str("\n\n");

    out.push_str("## 核心发现与关键结论 (Key Takeaways)\n\n");
    if report.key_takeaways.is_empty() {
        out.push_str("- 未提取到显著核心结论。\n\n");
    } else {
        for (idx, takeaway) in report.key_takeaways.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", idx + 1, takeaway));
        }
        out.push('\n');
    }

    out.push_str("## 调研维度多角度深度剖析 (In-Depth Dimension Analysis)\n\n");
    for (idx, dim) in report.dimensions.iter().enumerate() {
        out.push_str(&format!("### {}. {}\n\n", idx + 1, dim.angle));
        out.push_str(&format!("> **检索探针**: `{}`\n\n", dim.query));

        if dim.findings.is_empty() {
            out.push_str("*该维度暂未提取到足够篇幅的独立论据。*\n\n");
        } else {
            for finding in &dim.findings {
                out.push_str(&format!("- {}\n", finding));
            }
            out.push('\n');
        }

        if !dim.source_ids.is_empty() {
            let cited_str: Vec<String> = dim
                .source_ids
                .iter()
                .map(|id| format!("[文献 #{id}]"))
                .collect();
            out.push_str(&format!("*证据链参考*: {}\n\n", cited_str.join(" ")));
        }
    }

    if let Some(ref matrix) = report.comparative_analysis {
        out.push_str("## 综合对比与选型评估矩阵 (Evaluation Matrix)\n\n");
        out.push_str(matrix);
        out.push_str("\n\n");
    }

    out.push_str("## 权威参考源与证据链引注 (References & Citation Evidence)\n\n");
    if report.citations.is_empty() {
        out.push_str("未收录到有效参考源。\n\n");
    } else {
        for c in &report.citations {
            let hop_tag = if c.hop == 2 {
                " `[Hop-2 引文延伸]`"
            } else {
                ""
            };
            out.push_str(&format!("{}. [{}]({}){}\n", c.id, c.title, c.url, hop_tag));
            if !c.snippet.is_empty() {
                out.push_str(&format!("   > **摘要**: {}\n", c.snippet));
            }
            if !c.key_points.is_empty() {
                out.push_str(&format!(
                    "   > **要点支持**: {}\n",
                    c.key_points.join(" | ")
                ));
            }
            out.push('\n');
        }
    }

    out.push_str("---\n\n## 调研遥测报告 (Telemetry)\n\n");
    out.push_str(&format!(
        "- **检索探针总数**: {} 组\n- **精读解析网页数**: {} 篇\n- **累计提取 Tokens**: {}\n- **全流程耗时**: {} ms\n",
        report.telemetry.total_queries,
        report.telemetry.pages_fetched,
        report.telemetry.total_tokens_extracted,
        report.telemetry.elapsed_ms
    ));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompose_topic_cjk() {
        let angles = decompose_topic("Rust 异步编程机制", 3);
        assert_eq!(angles.len(), 3);
        assert!(angles[0].0.contains("架构"));
        assert!(angles[0].1.contains("Rust 异步编程机制"));
        assert!(angles[1].0.contains("优缺点"));
        assert!(angles[2].0.contains("生产实践"));
    }

    #[test]
    fn test_decompose_topic_en() {
        let angles = decompose_topic("Polars vs Pandas 2.0", 4);
        assert_eq!(angles.len(), 4);
        assert!(angles[0].0.contains("Architecture"));
        assert!(angles[1].0.contains("Tradeoffs"));
        assert!(angles[2].0.contains("Best Practices"));
        assert!(angles[3].0.contains("Ecosystem"));
    }

    #[test]
    fn test_format_deep_research_markdown() {
        let report = DeepResearchReport {
            topic: "Rust Web 开发选型".to_string(),
            truncated: false,
            executive_summary: "系统对比了 Axum 与 Actix-web 的性能与生态表现。".to_string(),
            key_takeaways: vec![
                "Axum 与 Tokio 生态深度结合，API 更加人体工学。".to_string(),
                "Actix-web 历史悠久，在极低延迟基准中略微占优。".to_string(),
            ],
            dimensions: vec![ResearchDimension {
                angle: "生态与架构".to_string(),
                query: "Rust Web 框架对比".to_string(),
                findings: vec!["Axum 成为目前主流首选。".to_string()],
                source_ids: vec![1],
            }],
            comparative_analysis: Some("| 框架 | 优点 |\n|---|---|\n| Axum | 现代化 |".to_string()),
            citations: vec![ResearchCitation {
                id: 1,
                title: "Axum 官方仓库".to_string(),
                url: "https://github.com/tokio-rs/axum".to_string(),
                snippet: "Ergonomic and modular web framework built with Tokio, Tower, and Hyper."
                    .to_string(),
                hop: 1,
                key_points: vec!["模块化设计".to_string()],
            }],
            telemetry: ResearchTelemetry {
                total_queries: 1,
                pages_fetched: 1,
                total_tokens_extracted: 350,
                elapsed_ms: 1200,
            },
        };

        let md = format_deep_research_markdown(&report);
        assert!(md.contains("# 深度研究综述: \"Rust Web 开发选型\""));
        assert!(md.contains("## 执行摘要"));
        assert!(md.contains("Axum 官方仓库"));
        assert!(md.contains("## 调研遥测报告"));
    }

    #[test]
    fn test_clean_core_entity_and_probe_simplification() {
        let topic = "近期发布的小参数语言模型 SLM (含其派生的 2 个深度探针)";
        let clean = clean_core_entity(topic);
        assert_eq!(clean, "小参数语言模型 SLM");

        let angles = decompose_topic(topic, 3);
        assert_eq!(angles.len(), 3);
        // Ensure probes are crisp and don't carry the parenthetical notes or fluff
        for (_name, query) in &angles {
            assert!(
                !query.contains("近期发布的"),
                "Probe '{query}' must strip temporal filler prefix"
            );
            assert!(
                !query.contains("深度探针"),
                "Probe '{query}' must strip parenthetical notes"
            );
            assert!(query.contains("小参数语言模型 SLM"));
        }
    }
}
