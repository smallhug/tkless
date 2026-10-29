use tkless_core::{ExtractionOptions, OutputFormat, PlatformAdapter, SkillsAdapter};
use url::Url;

const URL: &str = "https://www.skills.sh/example/tools/demo";

fn page_with_flight(rest: Option<&str>) -> String {
    let preview = "<h1>Demo Skill</h1><p>Preview text.</p>";
    let props = serde_json::json!(["$", "component", null, {
        "previewHtml":preview,"restHtml":"$a", "command":"npx skills add example/tools --skill demo"
    }]);
    let mut stream = format!("1:{props}\n");
    if let Some(rest) = rest {
        stream.push_str(&format!("a:T{:x},{rest}", rest.len()));
    }
    // Flight chunks can divide a length-delimited text record anywhere.
    let midpoint = stream.find("Preview").unwrap();
    let script = [&stream[..midpoint], &stream[midpoint..]]
        .iter()
        .map(|chunk| {
            format!(
                "<script>self.__next_f.push({})</script>",
                serde_json::json!([1, chunk])
                    .to_string()
                    .replace('<', "\\u003c")
            )
        })
        .collect::<String>();
    format!(
        r#"<html><head><meta name="description" content="Demo description"></head><body>
    <nav>Global navigation</nav><main><h1>demo</h1><div class="prose"><p>Skill summary.</p></div>
    <a href="https://github.com/example/tools">Repository</a><div class="prose">{preview}</div>
    <button>Show more</button><section><h2>Related skills</h2><p>Noise from another skill</p></section>
    <div class="bg-background"><div><span>Installs</span></div><div>1.2K</div></div>
    <div class="bg-background"><div><span>First Seen</span></div><div>Oct 20, 2026</div></div>
    </main><footer>Footer noise</footer>{script}</body></html>"#
    )
}

#[test]
fn matches_only_supported_hosts_and_routes() {
    let adapter = SkillsAdapter::new();
    for url in [
        URL,
        "https://skills.sh/",
        "https://skills.sh/hot",
        "https://www.skills.sh/trending",
        "https://skills.sh/example/tools",
        "https://skills.sh/site/domain.example/demo",
        "https://skills.sh/search?q=rust",
        "https://skills.sh/?q=rust",
    ] {
        assert!(adapter.matches(&Url::parse(url).unwrap()), "{url}");
    }
    for url in [
        "https://skill.sh/example/tools/demo",
        "https://skills.sh.evil.example/example/tools/demo",
        "https://evil.skills.sh/example/tools/demo",
        "https://skills.sh/docs/api",
        "https://skills.sh/example/tools/demo/security/socket",
        "ftp://skills.sh/",
    ] {
        assert!(!adapter.matches(&Url::parse(url).unwrap()), "{url}");
    }
    assert!(!adapter.allow_html_fallback(&Url::parse("https://skills.sh/search?q=rust").unwrap()));
    assert!(!adapter.allow_html_fallback(&Url::parse("https://skills.sh/hot").unwrap()));
    assert!(adapter.allow_html_fallback(&Url::parse(URL).unwrap()));
}

#[tokio::test]
async fn mcp_catalog_bounds_are_validated_before_fetching() {
    let server = tkless_core::McpServer::new(
        tkless_core::TklessEngine::new().unwrap(),
        tkless_core::DocumentCache::memory_only(30),
    );
    for (field, value) in [
        ("skills_pages", 0),
        ("skills_pages", 51),
        ("skills_limit", 0),
        ("skills_limit", 10001),
    ] {
        let mut args = serde_json::json!({"url":"https://skills.sh/"});
        args[field] = value.into();
        let result = server
            .dispatch_tool_execution("fetch_webpage", &args, None, None)
            .await;
        assert!(result.is_error);
        assert_eq!(
            result.structured_content.unwrap()["error"]["code"],
            "invalid_argument"
        );
    }
}

#[tokio::test]
async fn api_only_adapter_failures_do_not_fall_back_to_an_html_shell() {
    struct ApiOnly;
    impl PlatformAdapter for ApiOnly {
        fn name(&self) -> &'static str {
            "api_only"
        }
        fn matches(&self, _: &Url) -> bool {
            true
        }
        fn allow_html_fallback(&self, _: &Url) -> bool {
            false
        }
        fn extract<'a>(
            &'a self,
            _: &'a tkless_core::fetcher::Fetcher,
            _: &'a Url,
            _: &'a ExtractionOptions,
        ) -> tkless_core::adapters::BoxFuture<
            'a,
            Result<tkless_core::PageDocument, tkless_core::TklessError>,
        > {
            Box::pin(async {
                Err(tkless_core::TklessError::AdapterError {
                    platform: "api_only",
                    message: "No catalog data".into(),
                })
            })
        }
    }
    let mut adapters = tkless_core::AdapterRegistry::new();
    adapters.register(std::sync::Arc::new(ApiOnly));
    let engine = tkless_core::TklessEngine::with_options_and_adapters(
        tkless_core::FetchOptions {
            allow_private_addresses: true,
            ..Default::default()
        },
        adapters,
    )
    .unwrap();
    assert!(matches!(
        engine
            .process_url(
                "http://127.0.0.1/never-requested",
                &ExtractionOptions::default()
            )
            .await,
        Err(tkless_core::TklessError::AdapterError { .. })
    ));
}

#[test]
fn detail_restores_folded_unicode_body_without_related_noise() {
    let html = page_with_flight(Some("<h2>完整正文</h2><p>Rust 工具</p><pre><code class=\"language-rust\">fn main() {}</code></pre><a href=\"./other\">Other</a>"));
    let doc = SkillsAdapter::parse_html(
        &html,
        URL,
        &ExtractionOptions {
            extract_links: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(doc.content.contains("完整正文"));
    assert!(doc.content.contains("fn main()"));
    assert!(doc
        .content
        .contains("https://www.skills.sh/example/tools/other"));
    assert!(!doc.content.contains("Footer noise"));
    assert!(!doc.content.contains("Noise from another skill"));
    let entity = doc.entity.unwrap();
    assert_eq!(entity["readme_complete"], true);
    assert_eq!(entity["statistics"]["installs_display"], "1.2K");
    assert_eq!(entity["statistics"]["first_seen"], "Oct 20, 2026");
    assert_eq!(entity["repository_url"], "https://github.com/example/tools");
    assert!(doc
        .links
        .unwrap()
        .iter()
        .any(|link| link.url.ends_with("/other")));
    assert!(!doc.truncated);
    assert!(doc.warnings.is_empty());
}

#[test]
fn missing_folded_record_is_explicit_preview() {
    let doc =
        SkillsAdapter::parse_html(&page_with_flight(None), URL, &ExtractionOptions::default())
            .unwrap();
    assert!(doc.truncated);
    assert_eq!(doc.entity.unwrap()["readme_complete"], false);
    assert_eq!(doc.warnings[0].code, "skills_readme_preview");
    assert!(doc.content.contains("Preview text"));
}

#[test]
fn static_complete_body_and_well_known_source() {
    let html = "<main><h1>demo</h1><div class='prose'>Summary</div><div class='prose'><h2>Body</h2><p>All instructions</p></div></main>";
    let doc = SkillsAdapter::parse_html(
        html,
        "https://skills.sh/site/domain.example/demo",
        &ExtractionOptions::default(),
    )
    .unwrap();
    assert_eq!(doc.entity.as_ref().unwrap()["source_type"], "well-known");
    assert_eq!(doc.entity.unwrap()["readme_complete"], true);
    assert!(doc.content.contains("All instructions"));
}

#[test]
fn list_is_deduplicated_with_exact_embedded_counts_and_scope() {
    let props = serde_json::json!({"initialSkills":[{"source":"example/tools","skillId":"demo","installs":1234}]});
    let html = format!(
        r#"<main><h2>Skills Leaderboard</h2>
    <a href="/example/tools/demo"><h3>demo</h3><span>1.2K</span></a>
    <a href="https://www.skills.sh/example/tools/demo"><h3>duplicate</h3></a>
    <a href="/site/domain.example/other"><h3>other</h3><span>52</span></a>
    <a href="https://evil.example/a/b/c">Foreign</a><a href="/docs/api">Docs</a></main>
    <script>self.__next_f.push({})</script>"#,
        serde_json::json!([1, format!("1:{props}\n")])
    );
    let doc = SkillsAdapter::parse_html(
        &html,
        "https://www.skills.sh/",
        &ExtractionOptions::default(),
    )
    .unwrap();
    let entity = doc.entity.unwrap();
    assert_eq!(entity["count"], 2);
    assert_eq!(entity["skills"][0]["installs"], 1234);
    assert_eq!(entity["skills"][1]["installs"], serde_json::Value::Null);
    assert_eq!(entity["scope"], "page_snapshot");
    assert_eq!(doc.warnings[0].code, "skills_list_snapshot");
    assert!(!doc.content.contains("Foreign"));
}

#[test]
fn repository_is_a_list_not_a_skill() {
    let doc=SkillsAdapter::parse_html("<main><h1>example/tools</h1><a href='/example/tools/demo'><h3>demo</h3><span>7</span></a></main>",
        "https://skills.sh/example/tools",&ExtractionOptions::default()).unwrap();
    assert_eq!(doc.entity.unwrap()["page_type"], "repository");
    assert!(doc.warnings.is_empty());
}

#[test]
fn changed_or_empty_pages_fail_instead_of_inventing_content() {
    for html in [
        "<main><h1>Missing body</h1><div class='prose'>Summary only</div></main>",
        "<h1>Outside main</h1>",
        "<main><h1>empty</h1></main>",
    ] {
        assert!(SkillsAdapter::parse_html(html, URL, &ExtractionOptions::default()).is_err());
    }
    assert!(SkillsAdapter::parse_html(
        "<main><h1>Empty</h1></main>",
        "https://skills.sh/",
        &ExtractionOptions::default()
    )
    .is_err());
}

#[test]
fn formats_filters_and_token_budget_are_honored() {
    let html=page_with_flight(Some("<h2>Body</h2><p>Hello <a href='/example/tools/demo'>Link</a></p><script>BAD_SCRIPT()</script>"));
    for format in [
        OutputFormat::Markdown,
        OutputFormat::Json,
        OutputFormat::Text,
        OutputFormat::Html,
        OutputFormat::Raw,
    ] {
        let doc = SkillsAdapter::parse_html(
            &html,
            URL,
            &ExtractionOptions {
                format,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(doc.content.contains("Body"));
        assert!(!doc.content.contains("BAD_SCRIPT"));
        if format == OutputFormat::Text {
            assert!(!doc.content.contains("<h2>"));
        }
    }
    let doc = SkillsAdapter::parse_html(
        &html,
        URL,
        &ExtractionOptions {
            max_tokens: Some(12),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(doc.truncated);
    assert!(doc.token_count <= 12);
    let doc = SkillsAdapter::parse_html(
        &html,
        URL,
        &ExtractionOptions {
            strip_links: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!doc.content.contains("](https://"));
}

#[test]
fn selectors_use_original_dom_without_claiming_complete_skill() {
    let doc = SkillsAdapter::parse_html(
        &page_with_flight(None),
        URL,
        &ExtractionOptions {
            selector: Some("section".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(doc.content.contains("Noise from another skill"));
    assert!(!doc.content.contains("Preview text"));
    assert!(doc.entity.is_none());
}

#[test]
fn builtin_registry_and_catalog_include_skills() {
    let registry = tkless_core::AdapterRegistry::with_builtins();
    assert_eq!(
        registry
            .find_adapter(&Url::parse(URL).unwrap())
            .unwrap()
            .name(),
        "skills"
    );
    assert!(tkless_core::AdapterRegistry::list_adapter_metadata()
        .iter()
        .any(|entry| entry.name == "skills"));
}

#[test]
fn hot_view_preserves_count_and_change_as_distinct_metrics() {
    let props = serde_json::json!({"view":"hot","initialSkills":[{"source":"domain.example","skillId":"demo","installs":365,"installsYesterday":185,"change":180}]});
    let html = format!(
        r#"<main><h2>Skills Leaderboard</h2><a href='/site/domain.example/demo'><h3>demo</h3><span>365</span><span>+180</span></a></main><script>self.__next_f.push({})</script>"#,
        serde_json::json!([1, format!("1:{props}\n")])
    );
    let doc = SkillsAdapter::parse_html(
        &html,
        "https://www.skills.sh/hot",
        &ExtractionOptions::default(),
    )
    .unwrap();
    let entity = doc.entity.unwrap();
    assert_eq!(entity["view"], "hot");
    assert_eq!(entity["skills"][0]["source"], "domain.example");
    assert_eq!(entity["skills"][0]["installs"], 365);
    assert_eq!(entity["skills"][0]["change"], 180);
    assert_eq!(entity["skills"][0]["installs_yesterday"], 185);
    assert_eq!(entity["skills"][0]["metric_display"], "+180");
    assert!(doc.content.contains("365 installs (change +180)"));
}
