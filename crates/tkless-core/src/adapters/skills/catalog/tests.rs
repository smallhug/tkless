use super::*;
use std::{collections::VecDeque, future::ready};

fn item(slug: &str) -> Value {
    json!({"source":"example/tools","skillId":slug,"name":slug,"installs":7})
}
fn page(index: usize, skills: Vec<Value>, more: bool) -> Value {
    json!({"page":index,"skills":skills,"hasMore":more,"total":8})
}

async fn run(
    url: &str,
    options: ExtractionOptions,
    replies: Vec<Result<Value, TklessError>>,
) -> (Result<PageDocument, TklessError>, Vec<Url>) {
    let mut queue: VecDeque<_> = replies.into();
    let mut requests = Vec::new();
    let result = collect(&Url::parse(url).unwrap(), &options, |target| {
        requests.push(target);
        ready(queue.pop_front().expect("Unexpected extra request"))
    })
    .await;
    (result, requests)
}

#[tokio::test]
async fn paging_uses_actual_page_numbers_and_deduplicates_overlap() {
    let (result, requests) = run(
        "https://www.skills.sh/",
        ExtractionOptions::default(),
        vec![
            Ok(page(0, vec![item("a"), item("b")], true)),
            Ok(page(1, vec![item("b"), item("c")], false)),
        ],
    )
    .await;
    let doc = result.unwrap();
    let e = doc.entity.unwrap();
    assert_eq!(
        requests.iter().map(|u| u.path()).collect::<Vec<_>>(),
        vec!["/api/skills/all-time/0", "/api/skills/all-time/1"]
    );
    assert_eq!(e["count"], 3);
    assert_eq!(e["pagination"]["duplicates_removed"], 1);
    assert_eq!(e["pagination"]["complete"], true);
    assert_eq!(e["pagination"]["next_page"], Value::Null);
    assert!(!doc.truncated);
    assert!(doc.warnings.is_empty());
}

#[tokio::test]
async fn caps_stop_without_requesting_an_extra_page() {
    let (result, requests) = run(
        "https://www.skills.sh/trending?page=2",
        ExtractionOptions {
            skills_pages: 1,
            ..Default::default()
        },
        vec![Ok(page(2, vec![item("a")], true))],
    )
    .await;
    let doc = result.unwrap();
    assert_eq!(requests[0].path(), "/api/skills/trending/2");
    assert_eq!(
        doc.entity.as_ref().unwrap()["pagination"]["stop_reason"],
        "max_pages"
    );
    assert!(doc.truncated);
    let (result, requests) = run(
        "https://www.skills.sh/hot",
        ExtractionOptions {
            skills_limit: 1,
            ..Default::default()
        },
        vec![Ok(page(0, vec![item("a"), item("b")], false))],
    )
    .await;
    let doc = result.unwrap();
    assert_eq!(requests.len(), 1);
    let e = doc.entity.unwrap();
    assert_eq!(e["count"], 1);
    assert_eq!(e["pagination"]["stop_reason"], "result_limit");
    assert_eq!(e["pagination"]["complete"], false);
    assert_eq!(e["pagination"]["next_page"], Value::Null);
}

#[tokio::test]
async fn later_failure_retains_results_and_retry_page_but_first_failure_is_terminal() {
    let error = || TklessError::HttpStatusError {
        status: 429,
        url: "https://www.skills.sh/api/skills/hot/1".into(),
    };
    let (result, requests) = run(
        "https://www.skills.sh/hot",
        ExtractionOptions::default(),
        vec![Ok(page(0, vec![item("a")], true)), Err(error())],
    )
    .await;
    let doc = result.unwrap();
    assert_eq!(requests.len(), 2);
    let e = doc.entity.unwrap();
    assert_eq!(e["count"], 1);
    assert_eq!(e["pagination"]["next_page"], 1);
    assert_eq!(e["pagination"]["pages_fetched"], 1);
    assert_eq!(e["pagination"]["stop_reason"], "page_error");
    assert_eq!(e["pagination"]["page_error"]["retryable"], true);
    assert!(doc.truncated);
    assert_eq!(doc.warnings[0].code, "skills_page_failed");
    let (result, _) = run(
        "https://www.skills.sh/hot",
        ExtractionOptions::default(),
        vec![Err(error())],
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn repeats_and_empty_nonterminal_pages_stop_the_loop() {
    for entries in [vec![item("a")], vec![]] {
        let (result, requests) = run(
            "https://www.skills.sh/",
            ExtractionOptions::default(),
            vec![
                Ok(page(0, vec![item("a")], true)),
                Ok(page(1, entries.clone(), true)),
            ],
        )
        .await;
        let doc = result.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            doc.entity.unwrap()["pagination"]["stop_reason"],
            if entries.is_empty() {
                "empty_page"
            } else {
                "repeated_page"
            }
        );
        assert!(doc.truncated);
    }
}

#[tokio::test]
async fn invalid_page_shapes_do_not_masquerade_as_empty_results() {
    for bad in [
        json!({"error":"offline"}),
        json!({"page":0,"hasMore":false,"skills":[{"name":"bad"}]}),
        page(9, vec![], false),
    ] {
        let (result, _) = run(
            "https://www.skills.sh/",
            ExtractionOptions::default(),
            vec![Ok(bad)],
        )
        .await;
        assert!(result.is_err());
    }
    let (result, _) = run(
        "https://www.skills.sh/",
        ExtractionOptions::default(),
        vec![Ok(page(0, vec![], false))],
    )
    .await;
    let doc = result.unwrap();
    assert_eq!(doc.entity.unwrap()["count"], 0);
    assert!(!doc.truncated);
}

#[tokio::test]
async fn search_encodes_unicode_queries_and_does_not_fabricate_a_cursor() {
    let (result, requests) = run(
        "https://www.skills.sh/search?q=Rust%20%E5%B7%A5%E5%85%B7%20%26%20CLI",
        ExtractionOptions {
            skills_limit: 5,
            ..Default::default()
        },
        vec![Ok(json!({"skills":[item("a")],"searchType":"semantic"}))],
    )
    .await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path(), "/api/search");
    let pairs: std::collections::HashMap<_, _> = requests[0].query_pairs().collect();
    assert_eq!(pairs["q"], "Rust 工具 & CLI");
    assert_eq!(pairs["limit"], "5");
    assert!(!pairs.contains_key("page"));
    let doc = result.unwrap();
    let e = doc.entity.unwrap();
    assert_eq!(e["page_type"], "search");
    assert_eq!(e["search_type"], "semantic");
    assert_eq!(e["pagination"]["supported"], false);
    assert_eq!(e["search_exhaustive"], false);
    assert!(!doc.truncated);
}

#[tokio::test]
async fn search_caps_and_legitimate_empty_results_are_explicit() {
    let entries = (0..200).map(|n| item(&format!("s{n}"))).collect::<Vec<_>>();
    let (result, requests) = run(
        "https://www.skills.sh/?q=rust",
        ExtractionOptions::default(),
        vec![Ok(json!({"skills":entries}))],
    )
    .await;
    assert!(requests[0]
        .query_pairs()
        .any(|(k, v)| k == "limit" && v == "200"));
    let doc = result.unwrap();
    assert!(doc.truncated);
    assert_eq!(
        doc.entity.unwrap()["pagination"]["stop_reason"],
        "search_limit"
    );
    let (result, _) = run(
        "https://www.skills.sh/search?q=missing",
        ExtractionOptions::default(),
        vec![Ok(json!({"skills":[]}))],
    )
    .await;
    let doc = result.unwrap();
    assert_eq!(doc.entity.unwrap()["count"], 0);
    assert!(!doc.truncated);
}

#[tokio::test]
async fn invalid_inputs_fail_before_network() {
    for url in [
        "https://www.skills.sh/search?q=a",
        "https://www.skills.sh/search",
        "https://www.skills.sh/search?q=rust&page=1",
        "https://www.skills.sh/?page=-1",
    ] {
        let (result, requests) = run(url, ExtractionOptions::default(), vec![]).await;
        assert!(result.is_err(), "{url}");
        assert!(requests.is_empty());
    }
    for options in [
        ExtractionOptions {
            skills_limit: 0,
            ..Default::default()
        },
        ExtractionOptions {
            skills_pages: 51,
            ..Default::default()
        },
    ] {
        let (result, requests) = run("https://www.skills.sh/", options, vec![]).await;
        assert!(result.is_err());
        assert!(requests.is_empty());
    }
}

#[test]
fn source_urls_are_constructed_locally_and_cache_isolates_limits() {
    let row = normalized(
        &json!({"source":"domain.example","skillId":"demo","url":"http://127.0.0.1/secret"}),
    )
    .unwrap();
    assert_eq!(row["url"], "https://www.skills.sh/site/domain.example/demo");
    assert!(normalized(&json!({"source":"../bad","skillId":"demo"})).is_err());
    let cache = crate::DocumentCache::memory_only(60);
    let options = ExtractionOptions::default();
    let original = cache.generate_key("https://skills.sh/", &options);
    assert_ne!(
        original,
        cache.generate_key(
            "https://skills.sh/",
            &ExtractionOptions {
                skills_pages: 1,
                ..options.clone()
            }
        )
    );
    assert_ne!(
        original,
        cache.generate_key(
            "https://skills.sh/",
            &ExtractionOptions {
                skills_limit: 5,
                ..options
            }
        )
    );
}
