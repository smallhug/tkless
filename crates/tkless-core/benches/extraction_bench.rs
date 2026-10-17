use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tkless_core::chunker::chunk_markdown;
use tkless_core::cleaner::clean_html;
use tkless_core::dedup::simhash;
use tkless_core::markdown::MarkdownConverter;
use tkless_core::model::ExtractionMode;

fn generate_synthetic_html(target_size_kb: usize) -> String {
    let mut html = String::with_capacity(target_size_kb * 1024);
    html.push_str(
        "<!DOCTYPE html><html><head><title>TkLess Benchmark Document</title></head><body>",
    );
    html.push_str("<header><nav><a href=\"/\">Home</a><a href=\"/docs\">Docs</a></nav></header>");
    html.push_str("<main><article>");
    html.push_str("<h1>TkLess 高性能网页提取引擎基准测试</h1>");
    html.push_str("<div class=\"author\">TkLess 团队</div>");
    html.push_str("<time>2026-10-20</time>");

    let sample_paragraph = "<p>TkLess 是专为大模型智能体设计的超轻量、低 Token 消耗的网页内容提取引擎。它通过确定性 DOM 启发式剪枝算法与 Readability 分数系统，能够有效剥离无用的脚本、样式表、导航抽屉与广告容器，提取出最干净的核心 Markdown 知识库内容。</p>";
    let sample_code_block = "<pre><code class=\"language-rust\">async fn run_pipeline() -> Result<(), TklessError> {\n    let engine = TklessEngine::new()?;\n    let doc = engine.process_url(\"https://example.com\", &opts).await?;\n    println!(\"Tokens: {}\", doc.token_count);\n    Ok(())\n}</code></pre>";
    let sample_table = "<table><thead><tr><th>Metric</th><th>Before</th><th>After</th><th>Savings</th></tr></thead><tbody><tr><td>Payload Size</td><td>1.2 MB</td><td>18 KB</td><td>98.5%</td></tr><tr><td>Token Count</td><td>45,000</td><td>1,250</td><td>97.2%</td></tr></tbody></table>";
    let sample_ad = "<div class=\"advertisement\"><a href=\"#\">Upgrade to Pro plan today for special offers!</a></div>";

    let mut section_idx = 1;
    while html.len() < target_size_kb * 1024 {
        html.push_str(&format!(
            "<h2>Section {}: 架构解析与并发流水线</h2>",
            section_idx
        ));
        html.push_str(sample_paragraph);
        html.push_str(sample_code_block);
        html.push_str(sample_table);
        html.push_str(sample_ad);
        html.push_str("<p>Here is an English technical excerpt explaining zero-copy buffering and memory locality in asynchronous runtimes.</p>");
        section_idx += 1;
    }

    html.push_str("</article></main>");
    html.push_str("<footer><p>&copy; 2026 TkLess Benchmark</p></footer>");
    html.push_str("</body></html>");
    html
}

fn bench_clean_html(c: &mut Criterion) {
    let mut group = c.benchmark_group("clean_html");

    for size_kb in [50, 200, 1000].iter() {
        let html = generate_synthetic_html(*size_kb);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}KB", size_kb)),
            &html,
            |b, input| {
                b.iter(|| {
                    let cleaned = clean_html(
                        black_box(input),
                        Some("https://example.com/bench"),
                        ExtractionMode::Article,
                    )
                    .unwrap();
                    black_box(cleaned);
                });
            },
        );
    }
    group.finish();
}

fn bench_simhash(c: &mut Criterion) {
    let mut group = c.benchmark_group("simhash_64bit");

    for size_kb in [10, 100, 500].iter() {
        let html = generate_synthetic_html(*size_kb);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}KB", size_kb)),
            &html,
            |b, input| {
                b.iter(|| {
                    let hash = simhash(black_box(input));
                    black_box(hash);
                });
            },
        );
    }
    group.finish();
}

fn bench_markdown_converter(c: &mut Criterion) {
    let mut group = c.benchmark_group("markdown_converter");
    let converter = MarkdownConverter::new();

    for size_kb in [50, 200, 1000].iter() {
        let raw_html = generate_synthetic_html(*size_kb);
        let cleaned = clean_html(
            &raw_html,
            Some("https://example.com/bench"),
            ExtractionMode::Article,
        )
        .unwrap();

        group.throughput(Throughput::Bytes(cleaned.html.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}KB", size_kb)),
            &cleaned.html,
            |b, input| {
                b.iter(|| {
                    let md = converter.convert(black_box(input), false, false).unwrap();
                    black_box(md);
                });
            },
        );
    }
    group.finish();
}

fn bench_semantic_chunker(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_chunker");
    let raw_html = generate_synthetic_html(200);
    let cleaned = clean_html(
        &raw_html,
        Some("https://example.com/bench"),
        ExtractionMode::Article,
    )
    .unwrap();
    let converter = MarkdownConverter::new();
    let md = converter.convert(&cleaned.html, false, false).unwrap();

    group.throughput(Throughput::Bytes(md.len() as u64));
    group.bench_function("chunk_200kb_markdown", |b| {
        b.iter(|| {
            let chunks = chunk_markdown(
                black_box(&md),
                400,
                50,
                tkless_core::model::TokenizerKind::Heuristic,
            );
            black_box(chunks);
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_clean_html,
    bench_simhash,
    bench_markdown_converter,
    bench_semantic_chunker
);
criterion_main!(benches);
