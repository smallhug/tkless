//! Interactive REPL shell session for exploratory scraping, search, and feed inspection.

use anyhow::Result;
use std::io::{self, Write};
use tkless_core::model::ExtractionOptions;
use tkless_core::TklessEngine;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Interactive REPL shell session for exploratory scraping, search, and feed inspection.
pub async fn run_shell(engine: &TklessEngine, base_options: &ExtractionOptions) -> Result<()> {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║   TkLess Interactive Shell (REPL 探索会话模式)                 ║");
    println!("║   输入 'help' 查看指令指南，输入 'exit' 或 'quit' 退出会话      ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    loop {
        eprint!("tkless> ");
        let _ = io::stderr().flush();

        let line = match lines.next_line().await {
            Ok(Some(l)) => l.trim().to_string(),
            _ => break,
        };

        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();

        match cmd {
            "exit" | "quit" | "q" => {
                println!("再见！");
                break;
            }
            "help" | "?" => {
                println!("\n可用指令 (Available Commands):");
                println!("  fetch <url>         - 抓取网页并输出清洗后的 Markdown");
                println!("  search <query>      - 免 API Key 全网搜索并返回结果列表");
                println!("  research <topic>    - 多跳自主深度网络调研并生成出版级综述报告");
                println!("  feed <url>          - 解析并展示 RSS 2.0 / Atom 1.0 订阅源");
                println!("  tables <url>        - 提取页面中的全部表格并以 GFM Markdown 输出");
                println!("  links <url>         - 提取页面超链接清单并去重展示");
                println!("  cache [clear]       - 查看本地响应缓存统计或清空磁盘缓存");
                println!("  summarize <url>     - 本地启发式提取页面核心观点要点速览");
                println!("  chunk <url>         - 将长文档按标题与 Token 预算语义感知切块");
                println!("  diff <url1> <url2>  - 对比两个网页或文档版本的版本差异与变更比例");
                println!("  adapters            - 列出支持的全部 22 个平台适配器与能力");
                println!("  help                - 显示此帮助信息");
                println!("  exit / quit         - 退出交互式会话\n");
            }
            "fetch" => {
                if let Some(target_url) = rest.first() {
                    match engine.process_url(target_url, base_options).await {
                        Ok(doc) => {
                            println!("\n{}\n", doc.content);
                        }
                        Err(e) => {
                            eprintln!("抓取失败: {e}");
                        }
                    }
                } else {
                    eprintln!("用法: fetch <url>");
                }
            }
            "search" => {
                let query = rest.join(" ");
                if query.is_empty() {
                    eprintln!("用法: search <query>");
                } else {
                    let cdp = base_options.cdp_endpoint.as_ref().map(|endpoint| {
                        tkless_core::CdpOptions {
                            allow_private_addresses: engine
                                .fetcher()
                                .options()
                                .allow_private_addresses,
                            endpoint: endpoint.clone(),
                            wait_ms: base_options.cdp_wait_ms,
                            ..Default::default()
                        }
                    });
                    match tkless_core::web_search(
                        engine,
                        &query,
                        &tkless_core::SearchOptions {
                            cdp,
                            auto_cdp: base_options.auto_cdp,
                            ..Default::default()
                        },
                    )
                    .await
                    {
                        Ok(report) => {
                            let md = tkless_core::search::format_search_results_markdown(
                                &query,
                                &report.results,
                            );
                            println!("\n{md}\n");
                        }
                        Err(e) => eprintln!("搜索失败: {e}"),
                    }
                }
            }
            "feed" => {
                if let Some(feed_url) = rest.first() {
                    match engine.fetcher().fetch(feed_url).await {
                        Ok(resp) => match tkless_core::feed::parse_feed(&resp.html) {
                            Ok(channel) => {
                                println!(
                                    "\n{}\n",
                                    tkless_core::feed::format_feed_markdown(&channel)
                                );
                            }
                            Err(e) => eprintln!("解析订阅源失败: {e}"),
                        },
                        Err(e) => eprintln!("获取订阅源失败: {e}"),
                    }
                } else {
                    eprintln!("用法: feed <url>");
                }
            }
            "tables" | "table" => {
                if let Some(target_url) = rest.first() {
                    match engine.fetcher().fetch(target_url).await {
                        Ok(resp) => {
                            let doc = scraper::Html::parse_document(&resp.html);
                            let tables = tkless_core::extractor::extract_tables(&doc);
                            if tables.is_empty() {
                                eprintln!("未在页面中检测到 <table> 表格元素");
                            } else {
                                let formatted = tkless_core::model::ExtractedTable::format_tables(
                                    &tables,
                                    tkless_core::model::TableExportFormat::Markdown,
                                );
                                println!("\n{formatted}\n");
                            }
                        }
                        Err(e) => eprintln!("获取网页失败: {e}"),
                    }
                } else {
                    eprintln!("用法: tables <url>");
                }
            }
            "adapters" | "platforms" => {
                let list = tkless_core::adapters::AdapterRegistry::list_adapter_metadata();
                println!(
                    "\n{}\n",
                    tkless_core::adapters::format_adapters_markdown(&list)
                );
            }
            "cache" => {
                let sub = rest.first().copied().unwrap_or("stats");
                let cache = tkless_core::DocumentCache::new(None, 900);
                if sub == "clear" || sub == "clean" {
                    match cache.clear() {
                        Ok(_) => println!("已成功清空本地响应缓存磁盘目录。\n"),
                        Err(e) => eprintln!("清空缓存失败: {e}"),
                    }
                } else {
                    let stats = cache.stats();
                    println!("\n[TkLess 本地响应缓存状态]");
                    println!("--------------------------------------");
                    if let Some(ref dir) = stats.cache_dir {
                        println!("磁盘缓存目录 : {}", dir);
                    }
                    println!("内存驻留条目 : {} 项", stats.memory_entries);
                    println!("磁盘持久条目 : {} 项", stats.disk_entries);
                    println!(
                        "磁盘占用容量 : {}",
                        tkless_core::downloader::format_bytes(stats.disk_bytes)
                    );
                    println!("--------------------------------------\n");
                }
            }
            "links" => {
                if let Some(target_url) = rest.first() {
                    match engine.fetcher().fetch(target_url).await {
                        Ok(resp) => {
                            let doc = scraper::Html::parse_document(&resp.html);
                            let links =
                                tkless_core::extractor::extract_links(&doc, Some(target_url));
                            println!("\n页面超链接清单 (共提取 {} 项):\n", links.len());
                            for (i, l) in links.iter().take(20).enumerate() {
                                println!("{}. [{}]({})", i + 1, l.text.trim(), l.url);
                            }
                            if links.len() > 20 {
                                println!("... (其余 {} 项已折叠)\n", links.len() - 20);
                            } else {
                                println!();
                            }
                        }
                        Err(e) => eprintln!("提取链接失败: {e}"),
                    }
                } else {
                    eprintln!("用法: links <url>");
                }
            }
            "research" => {
                let topic = rest.join(" ");
                if topic.is_empty() {
                    eprintln!("用法: research <topic>");
                } else {
                    let opts = tkless_core::ResearchOptions {
                        depth: 1,
                        breadth: 2,
                        max_pages: 3,
                        ..Default::default()
                    };
                    match tkless_core::deep_research(engine, &topic, &opts).await {
                        Ok(report) => {
                            let md = tkless_core::format_deep_research_markdown(&report);
                            println!("\n{md}\n");
                        }
                        Err(e) => eprintln!("深度调研失败: {e}"),
                    }
                }
            }
            "summarize" | "summary" => {
                if let Some(target_url) = rest.first() {
                    match engine.process_url(target_url, base_options).await {
                        Ok(doc) => {
                            let bullets =
                                tkless_core::summarizer::extract_key_takeaways(&doc.content, 4);
                            println!("\n### 核心要点速览 (Key Takeaways):\n");
                            for b in bullets {
                                println!("- {}", b.trim());
                            }
                            println!();
                        }
                        Err(e) => eprintln!("提取失败: {e}"),
                    }
                } else {
                    eprintln!("用法: summarize <url>");
                }
            }
            "chunk" => {
                if let Some(target_url) = rest.first() {
                    match engine.process_url(target_url, base_options).await {
                        Ok(doc) => {
                            let chunks = tkless_core::chunker::chunk_markdown(
                                &doc.content,
                                500,
                                50,
                                tkless_core::model::TokenizerKind::O200k,
                            );
                            println!("\n# 语义分块输出 (共 {} 块):\n", chunks.len());
                            for c in chunks {
                                let path = if c.heading_path.is_empty() {
                                    "正文".to_string()
                                } else {
                                    c.heading_path.join(" > ")
                                };
                                println!(
                                    "### [Chunk {}] Tokens: {} | 标题: {}\n\n{}\n",
                                    c.index + 1,
                                    c.token_count,
                                    path,
                                    c.content
                                );
                            }
                        }
                        Err(e) => eprintln!("提取切块失败: {e}"),
                    }
                } else {
                    eprintln!("用法: chunk <url>");
                }
            }
            "diff" => {
                if rest.len() >= 2 {
                    let url1 = rest[0];
                    let url2 = rest[1];
                    let doc1 = engine.process_url(url1, base_options).await;
                    let doc2 = engine.process_url(url2, base_options).await;
                    match (doc1, doc2) {
                        (Ok(d1), Ok(d2)) => {
                            let rep = tkless_core::diff::compute_diff(&d1.content, &d2.content);
                            println!("\n# 页面差异比对结果:\n");
                            println!(
                                "> 新增: +{} | 删除: -{} | 变更比例: {:.1}%\n",
                                rep.added_lines, rep.removed_lines, rep.change_ratio
                            );
                            println!("```diff\n{}\n```\n", rep.unified_diff);
                        }
                        (Err(e), _) => eprintln!("抓取第一篇失败: {e}"),
                        (_, Err(e)) => eprintln!("抓取第二篇失败: {e}"),
                    }
                } else {
                    eprintln!("用法: diff <url1> <url2>");
                }
            }
            other => {
                eprintln!("未知指令: '{other}'。输入 'help' 查看可用指令。");
            }
        }
    }

    Ok(())
}
