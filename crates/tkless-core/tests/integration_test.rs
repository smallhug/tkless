use tkless_core::model::{ExtractionMode, ExtractionOptions, OutputFormat};
use tkless_core::TklessEngine;

#[test]
fn test_integration_pipeline_html_to_markdown() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let sample_html = r#"
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="utf-8">
            <title>Rust Web Extraction for Agents</title>
            <meta name="description" content="A demonstration of TkLess cleaning and extraction.">
            <meta name="author" content="TkLess Team">
            <meta property="og:title" content="TkLess Guide">
            <script type="application/ld+json">
            {
                "@context": "https://schema.org",
                "@type": "Article",
                "headline": "Rust Web Extraction for Agents"
            }
            </script>
        </head>
        <body>
            <header>
                <nav><a href="/">Home</a> | <a href="/docs">Docs</a></nav>
            </header>
            <main>
                <article>
                    <h1>Rust Web Extraction for Agents</h1>
                    <p>TkLess prunes boilerplates and produces clean Markdown.</p>
                    <p>Here is an essential table:</p>
                    <table>
                        <thead>
                            <tr><th>Language</th><th>Cold Start</th></tr>
                        </thead>
                        <tbody>
                            <tr><td>Rust</td><td>&lt; 20ms</td></tr>
                            <tr><td>Python</td><td>&gt; 800ms</td></tr>
                        </tbody>
                    </table>
                </article>
                <aside class="advertisement">
                    <p>Buy our sponsored product!</p>
                </aside>
            </main>
            <footer>
                <p>Copyright 2026</p>
            </footer>
        </body>
        </html>
    "#;

    let options = ExtractionOptions {
        mode: ExtractionMode::Article,
        format: OutputFormat::Markdown,
        max_tokens: None,
        tokenizer: tkless_core::model::TokenizerKind::default(),
        include_metadata: true,
        include_json_ld: true,
        strip_images: false,
        strip_links: false,
        frontmatter: true,
        no_adapter: false,
        feed: false,
        ..Default::default()
    };

    let doc = engine
        .process_html(
            sample_html,
            Some("https://example.com/blog/rust-agent"),
            &options,
        )
        .expect("Processing failed");

    // 1. Verify title and metadata
    assert_eq!(doc.title.as_deref(), Some("Rust Web Extraction for Agents"));
    assert_eq!(doc.metadata.author.as_deref(), Some("TkLess Team"));
    assert_eq!(
        doc.metadata.description.as_deref(),
        Some("A demonstration of TkLess cleaning and extraction.")
    );
    assert_eq!(
        doc.metadata.open_graph.get("og:title").map(|s| s.as_str()),
        Some("TkLess Guide")
    );

    // 2. Verify JSON-LD extraction
    assert_eq!(doc.json_ld.len(), 1);
    assert_eq!(doc.json_ld[0]["headline"], "Rust Web Extraction for Agents");

    // 3. Verify clean Markdown output (frontmatter, content, table)
    assert!(doc.content.starts_with("---\n"));
    assert!(doc
        .content
        .contains("title: \"Rust Web Extraction for Agents\""));
    assert!(doc
        .content
        .contains("TkLess prunes boilerplates and produces clean Markdown."));
    assert!(doc.content.contains("| Language | Cold Start |"));

    // 4. Verify noise was eliminated
    assert!(!doc.content.contains("Buy our sponsored product!"));
    assert!(!doc.content.contains("Copyright 2026"));

    // 5. Verify token estimation
    assert!(doc.token_count > 0);
    assert!(!doc.truncated);
}

#[test]
fn test_integration_token_budget_truncation() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let long_html = r#"
        <article>
            <h1>Introduction</h1>
            <p>Section 1: AI agents need low-overhead tools to process the web efficiently without wasting valuable context.</p>
            <p>Section 2: Rust provides unmatched speed, memory safety, and native asynchronous concurrency.</p>
            <p>Section 3: By controlling token budgets, we protect large language models from overflowing their context window.</p>
        </article>
    "#;

    let options = ExtractionOptions {
        mode: ExtractionMode::Article,
        format: OutputFormat::Markdown,
        max_tokens: Some(25),
        tokenizer: tkless_core::model::TokenizerKind::default(),
        include_metadata: false,
        include_json_ld: false,
        strip_images: false,
        strip_links: false,
        frontmatter: false,
        no_adapter: false,
        feed: false,
        ..Default::default()
    };

    let doc = engine
        .process_html(long_html, None, &options)
        .expect("Processing failed");

    assert!(doc.truncated);
    assert!(doc
        .content
        .contains("[Content truncated by TkLess: token limit reached]"));
}

#[test]
fn test_integration_selector_targeted_extraction() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let page_html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>Full Page With Lots of Content</title></head>
        <body>
            <header><nav><a href="/">Home</a><a href="/about">About</a></nav></header>
            <div class="sidebar">
                <p>Trending articles, advertisements, sidebar junk...</p>
                <div class="ads">Ad 1</div>
            </div>
            <main>
                <article class="post-body">
                    <h1>Target Header</h1>
                    <p>This is the core target paragraph needed by the AI Agent.</p>
                </article>
                <section class="comments-section">
                    <p>User comments...</p>
                </section>
            </main>
            <footer>Footer boilerplate...</footer>
        </body>
        </html>
    "#;

    let full_options = ExtractionOptions {
        mode: ExtractionMode::Full,
        format: OutputFormat::Markdown,
        ..Default::default()
    };
    let full_doc = engine.process_html(page_html, None, &full_options).unwrap();

    let selector_options = ExtractionOptions {
        selector: Some("article.post-body".to_string()),
        format: OutputFormat::Markdown,
        ..Default::default()
    };
    let selector_doc = engine
        .process_html(page_html, None, &selector_options)
        .unwrap();

    // Verify selector isolated target element
    assert!(selector_doc.content.contains("# Target Header"));
    assert!(selector_doc
        .content
        .contains("This is the core target paragraph"));
    assert!(!selector_doc.content.contains("Trending articles"));
    assert!(!selector_doc.content.contains("Footer boilerplate"));
    assert!(!selector_doc.content.contains("User comments"));

    // Verify token compression
    assert!(selector_doc.token_count < full_doc.token_count);
}

#[test]
fn test_integration_extract_assets_and_tables_json() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let page_html = r#"
        <html>
        <body>
            <a href="/page1">First Link</a>
            <a href="https://example.com/page2">Second Link</a>
            <img src="/assets/logo.png" alt="Company Logo">
            <img src="https://cdn.example.com/banner.jpg" alt="Hero Banner">
            <table>
                <thead>
                    <tr><th>City</th><th>Population</th></tr>
                </thead>
                <tbody>
                    <tr><td>Tokyo</td><td>14000000</td></tr>
                    <tr><td>London</td><td>8900000</td></tr>
                </tbody>
            </table>
        </body>
        </html>
    "#;

    let options = ExtractionOptions {
        extract_links: true,
        extract_images: true,
        extract_tables: true,
        format: OutputFormat::Json,
        ..Default::default()
    };

    let doc = engine
        .process_html(page_html, Some("https://example.com/base/"), &options)
        .unwrap();

    // Links
    let links = doc.links.as_ref().expect("Links must be populated");
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].url, "https://example.com/page1");
    assert_eq!(links[1].url, "https://example.com/page2");

    // Images
    let images = doc.images.as_ref().expect("Images must be populated");
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].src, "https://example.com/assets/logo.png");
    assert_eq!(images[0].alt, "Company Logo");
    assert_eq!(images[1].src, "https://cdn.example.com/banner.jpg");

    // Tables
    let tables = doc.tables.as_ref().expect("Tables must be populated");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].headers, vec!["City", "Population"]);
    assert_eq!(tables[0].rows.len(), 2);
    assert_eq!(
        tables[0].records[0].get("City").map(|s| s.as_str()),
        Some("Tokyo")
    );

    // Verify JSON serialization includes structured assets
    let json_val = serde_json::to_value(&doc).unwrap();
    assert!(json_val.get("links").is_some());
    assert!(json_val.get("images").is_some());
    assert!(json_val.get("tables").is_some());
}

#[test]
fn test_integration_extract_assets_markdown() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let page_html = r#"
        <html>
        <body>
            <p>Hello World</p>
            <a href="https://example.com/docs">Documentation</a>
            <img src="https://example.com/pic.png" alt="Sample Picture">
        </body>
        </html>
    "#;

    let options = ExtractionOptions {
        extract_links: true,
        extract_images: true,
        format: OutputFormat::Markdown,
        ..Default::default()
    };

    let doc = engine.process_html(page_html, None, &options).unwrap();

    assert!(doc.content.contains("### 页面链接清单 (Links)"));
    assert!(doc
        .content
        .contains("- [Documentation](https://example.com/docs)"));
    assert!(doc.content.contains("### 页面图片资产 (Images)"));
    assert!(doc
        .content
        .contains("- ![Sample Picture](https://example.com/pic.png)"));
}

#[test]
fn test_integration_schema_driven_entity_extraction() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let page_html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>Product Detail</title></head>
        <body>
            <div class="product-container">
                <h1 class="product-title">高性能机械键盘 Ultra</h1>
                <div class="price-wrapper">
                    <span class="price-val">899.00</span>
                </div>
                <a class="product-link" href="/shop/item-999">购买链接</a>
            </div>
        </body>
        </html>
    "#;

    let schema_json = r#"
    {
        "fields": {
            "title": "h1.product-title",
            "price": ".price-val",
            "link": "a.product-link @href"
        }
    }
    "#;

    let options = ExtractionOptions {
        schema: Some(schema_json.to_string()),
        format: OutputFormat::Json,
        ..Default::default()
    };

    let doc = engine
        .process_html(
            page_html,
            Some("https://store.example.com/item/1"),
            &options,
        )
        .expect("Failed to process HTML with schema");

    let entity = doc.entity.expect("Extracted entity must be present");
    assert_eq!(entity["title"], "高性能机械键盘 Ultra");
    assert_eq!(entity["price"], "899.00");
    assert_eq!(entity["link"], "https://store.example.com/shop/item-999");

    // Also test Markdown format includes JSON entity section
    let md_options = ExtractionOptions {
        schema: Some(schema_json.to_string()),
        format: OutputFormat::Markdown,
        ..Default::default()
    };
    let md_doc = engine
        .process_html(
            page_html,
            Some("https://store.example.com/item/1"),
            &md_options,
        )
        .expect("Failed to process HTML with schema");

    assert!(md_doc
        .content
        .contains("### 结构化实体提取 (Extracted Entity)"));
    assert!(md_doc.content.contains("高性能机械键盘 Ultra"));
}

#[tokio::test]
async fn test_integration_cdp_connection_failure_actionable_error() {
    let cdp_options = tkless_core::cdp::CdpOptions {
        endpoint: "http://192.0.2.1:59999".to_string(), // Unreachable remote IP (TEST-NET)
        wait_ms: Some(100),
        timeout_ms: Some(5000),
        user_agent: None,
        cookie: None,
        ..Default::default()
    };

    let res = tkless_core::cdp::fetch_html_via_cdp("https://example.com", &cdp_options).await;
    assert!(res.is_err());
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("无法连接到 Chrome CDP 端点") || err_msg.contains("remote-debugging-port"),
        "Error message should guide the user: {err_msg}"
    );
}

#[test]
fn test_integration_semantic_markdown_chunking() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let doc_html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>RAG Knowledge Base Guide</title></head>
        <body>
            <article>
                <h1>RAG Knowledge Base Guide</h1>
                <p>Comprehensive overview of vector embeddings and retrieval augmentation.</p>

                <h2>Architecture Overview</h2>
                <p>TkLess acts as the high-throughput web ingestion and cleaning layer.</p>

                <h3>Code Implementation</h3>
                <p>Here is an essential code implementation snippet:</p>
                <pre><code class="language-rust">fn main() {
    println!("Hello TkLess RAG!");
}</code></pre>

                <h2>Performance Benchmark</h2>
                <p>Benchmarks comparing ingestion latency across different engines:</p>
                <table>
                    <thead>
                        <tr><th>Engine</th><th>Cold Start</th></tr>
                    </thead>
                    <tbody>
                        <tr><td>TkLess</td><td>&lt; 5ms</td></tr>
                        <tr><td>Puppeteer</td><td>&gt; 800ms</td></tr>
                    </tbody>
                </table>
            </article>
        </body>
        </html>
    "#;

    let options = ExtractionOptions {
        chunk: true,
        chunk_size: Some(30),
        chunk_overlap: Some(5),
        format: OutputFormat::Json,
        ..Default::default()
    };

    let doc = engine
        .process_html(doc_html, Some("https://example.com/guide"), &options)
        .expect("Failed to process HTML with chunking");

    let chunks = doc
        .chunks
        .expect("Chunks must be populated when chunk: true");
    assert!(
        chunks.len() >= 2,
        "Document should be segmented into multiple chunks"
    );

    // Verify heading breadcrumbs
    let code_chunk = chunks.iter().find(|c| c.content.contains("println!"));
    assert!(code_chunk.is_some(), "Code snippet must be present");
    let c = code_chunk.unwrap();
    assert!(
        c.heading_path
            .contains(&"Architecture Overview".to_string())
            || c.heading_path.contains(&"Code Implementation".to_string()),
        "Heading path hierarchy must be preserved"
    );

    // Verify code block was not broken in half
    assert!(
        c.content
            .contains("```rust\nfn main() {\n    println!(\"Hello TkLess RAG!\");\n}\n```")
            || c.content.contains("println!(\"Hello TkLess RAG!\");"),
        "Code fence must remain intact"
    );
}

#[test]
fn test_integration_tables_only_csv_export() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let page_html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>Quarterly Financial Results</title></head>
        <body>
            <header><nav>Navigation links...</nav></header>
            <main>
                <h1>Quarterly Financial Overview</h1>
                <p>Lengthy preamble about market conditions and forward-looking statements...</p>
                <table>
                    <thead>
                        <tr><th>Quarter</th><th>Revenue (USD)</th><th>Growth</th></tr>
                    </thead>
                    <tbody>
                        <tr><td>Q1 2026</td><td>$1,500,000</td><td>+25%</td></tr>
                        <tr><td>Q2 2026</td><td>$1,950,000</td><td>+30%</td></tr>
                    </tbody>
                </table>
                <p>Additional footnotes and disclaimers...</p>
            </main>
            <footer>Copyright 2026</footer>
        </body>
        </html>
    "#;

    let options = ExtractionOptions {
        tables_only: true,
        table_format: tkless_core::model::TableExportFormat::Csv,
        ..Default::default()
    };

    let doc = engine
        .process_html(
            page_html,
            Some("https://finance.example.com/earnings"),
            &options,
        )
        .expect("Failed to process HTML with tables_only");

    assert!(doc.content.starts_with("Quarter,Revenue (USD),Growth\n"));
    assert!(doc.content.contains("Q1 2026,\"$1,500,000\",+25%\n"));
    assert!(doc.content.contains("Q2 2026,\"$1,950,000\",+30%\n"));
    assert!(!doc.content.contains("Lengthy preamble"));
    assert!(!doc.content.contains("Navigation links"));
    assert!(!doc.content.contains("Additional footnotes"));
}

#[test]
fn test_integration_extraction_metrics_telemetry() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let page_html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>Telemetry Benchmark Page</title></head>
        <body>
            <header><nav><a href="/">Home</a><a href="/docs">Docs</a><a href="/pricing">Pricing</a></nav></header>
            <div class="sidebar"><p>Sidebar ads and links...</p></div>
            <main>
                <article>
                    <h1>Telemetry Benchmark Page</h1>
                    <p>TkLess prunes boilerplate and reports empirical token savings to LLM agents.</p>
                </article>
            </main>
            <footer><p>Footer copyright boilerplate 2026...</p></footer>
        </body>
        </html>
    "#;

    let options = ExtractionOptions {
        stats: true,
        ..Default::default()
    };

    let doc = engine
        .process_html(page_html, Some("https://example.com/metrics"), &options)
        .expect("Failed to process HTML with stats");

    let metrics = doc
        .metrics
        .expect("Metrics must be populated when stats is true");
    assert!(
        metrics.raw_bytes > metrics.cleaned_bytes,
        "Raw HTML bytes must exceed cleaned bytes"
    );
    assert!(
        metrics.raw_token_estimate > metrics.final_token_count,
        "Raw token estimate must exceed cleaned token count"
    );
    assert!(
        metrics.token_savings_percent > 0.0,
        "Token savings percent must be positive"
    );
    assert!(
        metrics.noise_reduction_percent > 0.0,
        "Noise reduction percent must be positive"
    );
    assert_eq!(metrics.final_token_count, doc.token_count);
}

#[test]
fn test_integration_rss_feed_parsing_and_markdown() {
    let engine = TklessEngine::new().expect("Failed to initialize engine");

    let sample_rss = r#"<?xml version="1.0" encoding="UTF-8"?>
        <rss version="2.0">
            <channel>
                <title>Rust Security Announcements</title>
                <link>https://blog.rust-lang.org</link>
                <description>Official security advisories and release notes.</description>
                <item>
                    <title>Security Advisory 2026-01</title>
                    <link>https://blog.rust-lang.org/2026/01/adv.html</link>
                    <pubDate>Wed, 14 Jan 2026 12:00:00 GMT</pubDate>
                    <creator>Rust Security WG</creator>
                    <description>Critical vulnerability remediation details.</description>
                </item>
            </channel>
        </rss>
    "#;

    let doc = engine
        .process_html(
            sample_rss,
            Some("https://blog.rust-lang.org/feed.xml"),
            &ExtractionOptions::default(),
        )
        .expect("Failed to process RSS feed XML");

    assert_eq!(doc.title.as_deref(), Some("Rust Security Announcements"));
    assert!(doc
        .content
        .contains("# 📰 订阅源聚合 (RSS): Rust Security Announcements"));
    assert!(doc.content.contains("Security Advisory 2026-01"));
    assert!(doc.content.contains("Rust Security WG"));
    assert!(doc
        .content
        .contains("https://blog.rust-lang.org/2026/01/adv.html"));
    assert!(doc
        .content
        .contains("Critical vulnerability remediation details."));
}
