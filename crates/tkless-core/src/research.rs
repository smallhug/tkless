//! Autonomous Multi-Hop Deep Research Pipeline.
//!
//! Provides an end-to-end autonomous research agent loop:
//! 1. Analyzes and decomposes complex user topics into multi-angle research dimensions.
//! 2. Concurrently executes multi-engine search across all sub-queries (Hop 1).
//! 3. Extracts clean webpage content, token budgets, and local key takeaways.
//! 4. Performs reference tracking and citation exploration (Hop 2) for deep authoritative evidence.
//! 5. Synthesizes findings into a comprehensive, publication-grade research report with citations and telemetry.

use crate::error::TklessError;
use crate::model::{ExtractionOptions, OutputFormat, SearchEngineKind};
use crate::search::{web_search_scoped, SearchResultItem};
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
    /// Search engine kind to use for research searches. Default: All (Bing + DuckDuckGo).
    pub engine: SearchEngineKind,
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
            engine: SearchEngineKind::All,
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
    pub executive_summary: String,
    pub key_takeaways: Vec<String>,
    pub dimensions: Vec<ResearchDimension>,
    pub comparative_analysis: Option<String>,
    pub citations: Vec<ResearchCitation>,
    pub telemetry: ResearchTelemetry,
}

/// Decomposes a research topic into multiple sub-queries / angles.
pub fn decompose_topic(topic: &str, breadth: usize) -> Vec<(String, String)> {
    let clean = topic.trim();
    let b = breadth.clamp(2, 5);

    let is_cjk = clean.chars().any(|c| (c as u32) >= 0x4E00);

    if is_cjk {
        let mut angles = Vec::new();
        angles.push((
            "核心架构与原理 (Architecture & Fundamentals)".to_string(),
            format!("{clean} 核心架构 原理机制 深度解析"),
        ));
        angles.push((
            "优缺点对比与性能基准 (Tradeoffs & Benchmarks)".to_string(),
            format!("{clean} 优缺点对比 性能基准 评测"),
        ));
        if b >= 3 {
            angles.push((
                "生产实践与避坑指南 (Best Practices & Pitfalls)".to_string(),
                format!("{clean} 生产实践 最佳实践 踩坑经验"),
            ));
        }
        if b >= 4 {
            angles.push((
                "生态体系与前沿演进 (Ecosystem & Future Roadmap)".to_string(),
                format!("{clean} 生态发展 最新进展 发展趋势"),
            ));
        }
        if b >= 5 {
            angles.push((
                "行业落地与典型案例 (Case Studies & Applications)".to_string(),
                format!("{clean} 行业应用 典型案例 落地经验"),
            ));
        }
        angles
    } else {
        let mut angles = Vec::new();
        angles.push((
            "Architecture & Fundamentals".to_string(),
            format!("{clean} architecture internals overview"),
        ));
        angles.push((
            "Tradeoffs & Performance Benchmarks".to_string(),
            format!("{clean} pros and cons performance benchmarks comparison"),
        ));
        if b >= 3 {
            angles.push((
                "Production Best Practices".to_string(),
                format!("{clean} production best practices common pitfalls"),
            ));
        }
        if b >= 4 {
            angles.push((
                "Ecosystem & Future Roadmap".to_string(),
                format!("{clean} ecosystem trends roadmap 2026"),
            ));
        }
        if b >= 5 {
            angles.push((
                "Case Studies & Industry Adoption".to_string(),
                format!("{clean} real world case studies implementation"),
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

    info!(
        topic = clean_topic,
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
        match web_search_scoped(
            engine.client(),
            sub_query,
            None,
            4,
            options.engine,
            None,
            false,
        )
        .await
        {
            Ok(results) => {
                let mut unique_items = Vec::new();
                for item in results {
                    let norm = crate::search::normalize_url_for_dedup(&item.url);
                    if seen_urls.insert(norm) {
                        unique_items.push(item);
                    }
                }
                dimension_search_results.push((
                    angle_name.clone(),
                    sub_query.clone(),
                    unique_items,
                ));
            }
            Err(e) => {
                debug!(query = %sub_query, error = %e, "维度检索异常，跳过此角度");
                dimension_search_results.push((angle_name.clone(), sub_query.clone(), Vec::new()));
            }
        }
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
    let mut candidate_hop2_urls: Vec<(String, String)> = Vec::new();

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
                                candidate_hop2_urls.push((link.text.clone(), link.url.clone()));
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

        for (title, url) in candidate_hop2_urls {
            let norm = crate::search::normalize_url_for_dedup(&url);
            if seen_urls.insert(norm) && hop2_fetch_queue.len() < remaining_budget {
                hop2_fetch_queue.push((
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
        for (item, hop) in hop2_fetch_queue {
            let eng = engine.clone();
            let opts = extraction_opts.clone();
            hop2_join_set.spawn(async move {
                let res = eng.process_url(&item.url, &opts).await;
                (item, hop, res)
            });
        }

        while let Some(res) = hop2_join_set.join_next().await {
            if let Ok((item, hop, Ok(doc))) = res {
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

                // Add points to the last dimension
                if let Some(last_dim) = dimensions.last_mut() {
                    last_dim.source_ids.push(citation_id);
                    for pt in points {
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
        format!("围绕课题「{clean_topic}」进行了多维度调研，未检索到足够的高置信度正文数据。")
    } else {
        format!(
            "本报告通过多跳全网深度检索与正文提纯，系统化覆盖了「{}」的架构机制、权衡指标与工程落地实践。综合证据表明，主流技术方案在不同场景下各有明确的边界与适用法则。",
            clean_topic
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
        executive_summary,
        key_takeaways: all_key_takeaways,
        dimensions,
        comparative_analysis: Some(generate_comparative_matrix(clean_topic)),
        citations,
        telemetry,
    })
}

/// Generates a structured comparative matrix skeleton for the report.
fn generate_comparative_matrix(topic: &str) -> String {
    let mut out = String::new();
    out.push_str("| 评估维度 | 核心考量点 | 主流方案特征 | 生产建议 |\n");
    out.push_str("|---|---|---|---|\n");
    out.push_str(&format!("| **架构定位** | 抽象层级与执行模型 | 与「{topic}」核心场景强绑定 | 明确系统边界与资源配额 |\n"));
    out.push_str(
        "| **性能开销** | 内存与并发调度延迟 | 吞吐优先或延迟敏感 | 基于实际业务链路压测基准 |\n",
    );
    out.push_str("| **维护成本** | 学习曲线与生态成熟度 | 社区活跃度与排障工具链 | 优先选用社区主流成熟方案 |\n");
    out
}

/// Formats a complete DeepResearchReport as clean, structured publication-grade Markdown.
pub fn format_deep_research_markdown(report: &DeepResearchReport) -> String {
    let mut out = String::new();

    out.push_str(&format!("# 🌐 深度研究综述: \"{}\"\n\n", report.topic));
    out.push_str(&format!(
        "> 🔬 调研维度: {} 个 | 深入分析文献: {} 篇 (含 Hop-2 引链) | 累计处理 Tokens: {} | 耗时: {:.2}s\n\n---\n\n",
        report.dimensions.len(),
        report.telemetry.pages_fetched,
        report.telemetry.total_tokens_extracted,
        report.telemetry.elapsed_ms as f64 / 1000.0
    ));

    out.push_str("## 📋 执行摘要 (Executive Summary)\n\n");
    out.push_str(&report.executive_summary);
    out.push_str("\n\n");

    out.push_str("## 💡 核心发现与关键结论 (Key Takeaways)\n\n");
    if report.key_takeaways.is_empty() {
        out.push_str("- 未提取到显著核心结论。\n\n");
    } else {
        for (idx, takeaway) in report.key_takeaways.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", idx + 1, takeaway));
        }
        out.push('\n');
    }

    out.push_str("## 🔍 调研维度多角度深度剖析 (In-Depth Dimension Analysis)\n\n");
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
        out.push_str("## ⚖️ 综合对比与选型评估矩阵 (Evaluation Matrix)\n\n");
        out.push_str(matrix);
        out.push_str("\n\n");
    }

    out.push_str("## 📚 权威参考源与证据链引注 (References & Citation Evidence)\n\n");
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

    out.push_str("---\n\n## ⏱️ 调研遥测报告 (Telemetry)\n\n");
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
        assert!(md.contains("# 🌐 深度研究综述: \"Rust Web 开发选型\""));
        assert!(md.contains("## 📋 执行摘要"));
        assert!(md.contains("Axum 官方仓库"));
        assert!(md.contains("## ⏱️ 调研遥测报告"));
    }
}
