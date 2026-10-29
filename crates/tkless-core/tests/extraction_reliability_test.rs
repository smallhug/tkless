use tkless_core::{
    DocumentCache, ExtractionMode, ExtractionOptions, OutputFormat, TklessEngine, TklessError,
};

const HTML: &str = "<html><head><title>Sample</title></head><body><main><h1>Rust runtime</h1><p>Useful original content remains available.</p><div class='item'></div></main></body></html>";

fn options(schema: &str, strict: bool) -> ExtractionOptions {
    ExtractionOptions {
        schema: Some(schema.into()),
        schema_strict: strict,
        mode: ExtractionMode::Full,
        format: OutputFormat::Json,
        ..Default::default()
    }
}

#[test]
fn fallback_is_explicit_in_json_and_strict_mode_returns_errors() {
    let engine = TklessEngine::new().unwrap();
    for schema in [
        "{invalid JSON",
        r#"{"title":".missing"}"#,
        r#"{"items_selector":".item","fields":{"title":".missing"}}"#,
        r#"{"items_selector":".item","fields":{"tags":"[.missing]"}}"#,
    ] {
        let doc = engine
            .process_html(HTML, None, &options(schema, false))
            .unwrap();
        assert!(doc.entity.is_none());
        assert!(doc.content.contains("Useful original content"));
        let value = serde_json::to_value(&doc).unwrap();
        assert_eq!(value["warnings"][0]["code"], "schema_fallback");
        assert_eq!(value["extraction"]["method"], "html");
        assert!(matches!(
            engine.process_html(HTML, None, &options(schema, true)),
            Err(TklessError::SchemaError(_))
        ));
    }
}

#[test]
fn strict_success_preserves_entity_and_reports_schema_path() {
    let engine = TklessEngine::new().unwrap();
    let doc = engine
        .process_html(
            HTML,
            Some("https://example.com/article"),
            &options(r#"{"title":"h1"}"#, true),
        )
        .unwrap();
    assert_eq!(doc.entity.unwrap()["title"], "Rust runtime");
    assert!(doc.warnings.is_empty());
    assert_eq!(doc.extraction.method, "schema");
    assert_eq!(doc.extraction.transport, "local");
}

#[test]
fn strict_mode_is_validated_and_isolated_from_fallback_cache_entries() {
    let engine = TklessEngine::new().unwrap();
    assert!(matches!(
        engine.process_html(
            HTML,
            None,
            &ExtractionOptions {
                schema_strict: true,
                ..Default::default()
            }
        ),
        Err(TklessError::InvalidArgument(_))
    ));
    let soft = options(r#"{"title":".missing"}"#, false);
    let strict = options(r#"{"title":".missing"}"#, true);
    let cache = DocumentCache::memory_only(300);
    assert_ne!(
        engine.cache_key(&cache, "https://example.com", &soft),
        engine.cache_key(&cache, "https://example.com", &strict)
    );
    // Documents missing required current provenance fields are rejected.
    let doc = engine
        .process_html(HTML, None, &ExtractionOptions::default())
        .unwrap();
    let mut value = serde_json::to_value(doc).unwrap();
    value.as_object_mut().unwrap().remove("warnings");
    value.as_object_mut().unwrap().remove("extraction");
    assert!(serde_json::from_value::<tkless_core::PageDocument>(value).is_err());
}

#[test]
fn pdf_schema_is_explicitly_unsupported_in_strict_mode() {
    let error = tkless_core::extract_pdf_to_document(
        b"%PDF-invalid",
        None,
        &options(r#"{"title":"h1"}"#, true),
    )
    .unwrap_err();
    assert!(matches!(error, TklessError::SchemaError(_)));
}

#[test]
fn structured_errors_distinguish_retryable_failures() {
    let rate_limit = TklessError::SearchRateLimited {
        engine: "bing".into(),
        retry_after_secs: 42,
    }
    .details();
    assert_eq!(rate_limit.code, "rate_limited");
    assert!(rate_limit.retryable);
    assert_eq!(rate_limit.retry_after_secs, Some(42));
    for error in [
        TklessError::SearchParseError("layout changed".into()),
        TklessError::SearchAntiScraping("verification".into()),
        TklessError::SchemaError("no match".into()),
    ] {
        assert!(!error.details().retryable);
    }
}
