use super::*;

const RESULTS: &str = include_str!("../../tests/fixtures/bing/results.html");
const EMPTY: &str = include_str!("../../tests/fixtures/bing/no-results.html");

#[tokio::test]
async fn invalid_search_requests_fail_before_preflight() {
    let engine = TklessEngine::new().unwrap();
    for (query, limit) in [(" ", 10), ("Rust", 0)] {
        assert!(matches!(
            web_search(
                &engine,
                query,
                &SearchOptions {
                    limit,
                    ..Default::default()
                }
            )
            .await,
            Err(TklessError::InvalidArgument(_))
        ));
    }
}

#[test]
fn query_encoding_preserves_operators_phrases_dates_and_unicode() {
    for query in [
        r#""near-duplicate" -advertising RTX-4090 2026 site:github.com/Tokio"#,
        "C++ & C# 百分比 100% ? a+b",
        "中文 2026 -推广 site:mp.weixin.qq.com",
    ] {
        let url = url::Url::parse(&bing_search_url(query)).unwrap();
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
            query
        );
        assert_eq!(url.query_pairs().count(), 1);
    }
}

#[test]
fn snippets_and_scripts_mentioning_captcha_do_not_block_valid_results() {
    let html = format!(
        "{RESULTS}<script>var b_captcha = false;</script><p>Please solve the challenge below</p>"
    );
    assert_eq!(parse_bing_html_with_query(&html, "", 10).unwrap().len(), 2);
}

#[test]
fn http_failures_are_not_misclassified_as_verification_or_parse_errors() {
    for status in [403, 503] {
        let error = check_bing_http_status(
            status,
            "<h1>Service unavailable</h1>",
            "https://www.bing.com/search",
        )
        .unwrap_err();
        assert!(matches!(error, TklessError::HttpStatusError { .. }));
        assert_eq!(error.details().retryable, status == 503);
    }
    check_bing_http_status(
        403,
        include_str!("../../tests/fixtures/bing/challenge.html"),
        "https://www.bing.com/search",
    )
    .unwrap();
}

#[test]
fn filtering_counts_explain_scope_ads_duplicates_and_invalid_items() {
    let page = parse_bing_page(RESULTS, "site:github.com Tokio", 1).unwrap();
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].url, "https://github.com/tokio-rs/tokio");
    let counts = page.diagnostics;
    assert_eq!(counts.candidates, 6);
    assert_eq!(counts.invalid, 2);
    assert_eq!(counts.domain_filtered, 1);
    assert_eq!(counts.duplicates, 1);
    assert_eq!(counts.ads_filtered, 1);
    assert_eq!(counts.selected, 1);
    assert_eq!(counts.returned, 1);
}

#[test]
fn empty_filtered_verification_and_unknown_pages_are_distinct() {
    let mut empty = SearchReport::new("unfindable");
    empty.append(
        parse_bing_page(EMPTY, "unfindable", 10).unwrap(),
        1,
        "http",
        10,
    );
    assert_eq!(empty.status, SearchStatus::NoResults);
    let mut filtered = SearchReport::new("site:example.org Tokio");
    filtered.append(
        parse_bing_page(RESULTS, "site:example.org Tokio", 10).unwrap(),
        1,
        "http",
        10,
    );
    assert_eq!(filtered.status, SearchStatus::Filtered);
    assert!(matches!(
        parse_bing_html_with_query(
            include_str!("../../tests/fixtures/bing/challenge.html"),
            "",
            10
        ),
        Err(TklessError::SearchAntiScraping(_))
    ));
    for html in [
        include_str!("../../tests/fixtures/bing/changed-layout.html"),
        include_str!("../../tests/fixtures/bing/malformed-results.html"),
        "<html><body></body></html>",
        "<ol id='b_results'><li class='b_no'></li></ol>",
    ] {
        assert!(matches!(
            parse_bing_html_with_query(html, "", 10),
            Err(TklessError::SearchParseError(_))
        ));
    }
}

#[test]
fn domain_path_case_and_other_operators_do_not_change_host_filters() {
    let query = "SITE:github.com/Owner/Repo inurl:issues -advertisement filetype:html Tokio";
    let tokens = tokenize_query(query);
    assert_eq!(tokens.domain_filters, ["github.com/Owner/Repo"]);
    assert_eq!(tokens.core_tokens, ["tokio"]);
    assert!(matches_search_domains(
        "https://github.com/Owner/Repo/issues/1",
        &tokens.domain_filters
    ));
    assert!(!matches_search_domains(
        "https://github.com/owner/repo/issues/1",
        &tokens.domain_filters
    ));
    assert_eq!(apply_search_scope(query, Some("zhihu")).unwrap(), query);
}

#[test]
fn pagination_diagnostics_count_only_new_returned_urls() {
    let mut report = SearchReport::new("Tokio");
    report.append(
        parse_bing_page(RESULTS, "Tokio", 10).unwrap(),
        1,
        "http",
        10,
    );
    report.append(
        parse_bing_page(RESULTS, "Tokio", 10).unwrap(),
        2,
        "http",
        10,
    );
    assert_eq!(report.results.len(), 2);
    assert_eq!(report.diagnostics[0].returned, 2);
    assert_eq!(report.diagnostics[1].returned, 0);
    assert_eq!(report.diagnostics[1].duplicates, 3);
    assert_eq!(report.status, SearchStatus::Results);
}
