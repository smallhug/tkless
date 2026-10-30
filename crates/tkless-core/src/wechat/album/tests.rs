use super::*;
use serde_json::json;

fn url() -> Url {
    Url::parse(
        "https://mp.weixin.qq.com/mp/appmsgalbum?__biz=abc%3D%3D&album_id=123&pass_ticket=secret",
    )
    .unwrap()
}
fn options() -> WechatOptions {
    WechatOptions {
        delay_ms: 0,
        ..Default::default()
    }
}
fn page(ids: &[(u64, u64)], more: u8, total: usize) -> Value {
    json!({"base_resp":{"ret":0},"getalbum_resp":{"continue_flag":more.to_string(),"base_info":{"nickname":"测试公众号","username":"gh_test","title":"测试合集","article_count":total.to_string()},
        "article_list":ids.iter().map(|(mid,idx)| json!({"msgid":mid.to_string(),"itemidx":idx.to_string(),"title":format!("文章{mid}/{idx}"),"create_time":"1700000000","url":format!("http://mp.weixin.qq.com/s?__biz=abc%3D%3D&mid={mid}&idx={idx}&sn=signature#rd")})).collect::<Vec<_>>()}})
}

#[tokio::test]
async fn follows_both_cursor_fields_and_deduplicates_without_claiming_full_history() {
    let mut calls = vec![];
    let report = collect_with(&url(), &options(), |target| {
        calls.push(target.clone());
        let value = if calls.len() == 1 {
            page(&[(11, 1), (11, 2)], 1, 3)
        } else {
            page(&[(11, 2), (12, 1)], 0, 3)
        };
        std::future::ready(Ok(value))
    })
    .await
    .unwrap();
    assert!(report.complete && report.listing_complete);
    assert!(!report.account_history_complete);
    assert_eq!(report.articles.len(), 3);
    assert_eq!(report.duplicates, 1);
    assert_eq!(
        identity::parameter(&calls[1], "begin_msgid").as_deref(),
        Some("11")
    );
    assert_eq!(
        identity::parameter(&calls[1], "begin_itemidx").as_deref(),
        Some("2")
    );
    assert!(calls
        .iter()
        .all(|url| identity::parameter(url, "pass_ticket").is_none()));
    assert!(report.next_urls.is_empty());
}

#[tokio::test]
async fn retains_previous_pages_on_later_error() {
    let mut calls = 0;
    let report = collect_with(&url(), &options(), |_| {
        calls += 1;
        std::future::ready(if calls == 1 {
            Ok(page(&[(1, 1)], 1, 2))
        } else {
            Err(failure("verification required"))
        })
    })
    .await
    .unwrap();
    assert_eq!(report.articles.len(), 1);
    assert!(!report.complete);
    assert_eq!(report.stop_reason, StopReason::FetchFailed);
    assert_eq!(report.next_urls.len(), 1);
}

#[tokio::test]
async fn repeated_pages_and_budget_limits_remain_partial() {
    let report = collect_with(&url(), &options(), |_| {
        std::future::ready(Ok(page(&[(1, 1)], 1, 10)))
    })
    .await
    .unwrap();
    assert_eq!(report.stop_reason, StopReason::RepeatedPage);
    assert_eq!(report.pages_fetched, 2);
    assert!(!report.complete);
    for (limit, pages, reason) in [
        (1, 5, StopReason::ItemLimit),
        (100, 1, StopReason::PageLimit),
    ] {
        let opts = WechatOptions {
            limit,
            max_pages: pages,
            ..options()
        };
        let report = collect_with(&url(), &opts, |_| {
            std::future::ready(Ok(page(&[(1, 1)], 1, 10)))
        })
        .await
        .unwrap();
        assert_eq!(report.stop_reason, reason);
        assert!(!report.complete);
        assert!(report.next_urls[0].contains("begin_msgid=1"));
    }
}

#[test]
fn rejects_errors_missing_continuation_and_off_account_urls() {
    assert!(parse_page(json!({"base_resp":{"ret":-1}}), "abc==").is_err());
    let mut value = page(&[(1, 1)], 0, 1);
    value["getalbum_resp"]
        .as_object_mut()
        .unwrap()
        .remove("continue_flag");
    assert!(parse_page(value, "abc==").is_err());
    let mut value = page(&[(1, 1)], 0, 1);
    value["getalbum_resp"]["article_list"][0]["url"] =
        json!("https://mp.weixin.qq.com.evil.test/s?__biz=abc==&mid=1&idx=1");
    assert!(parse_page(value, "abc==").is_err());
    assert!(parse_page(page(&[], 1, 9), "abc==").is_err());
}

#[tokio::test]
async fn total_mismatch_and_expected_biz_are_not_silently_accepted() {
    let report = collect_with(&url(), &options(), |_| {
        std::future::ready(Ok(page(&[(1, 1)], 0, 2)))
    })
    .await
    .unwrap();
    assert!(!report.complete);
    assert_eq!(report.stop_reason, StopReason::InvalidPage);
    let opts = WechatOptions {
        expected_biz: Some("other".into()),
        ..options()
    };
    assert!(collect_with(&url(), &opts, |_| async {
        panic!("must reject before fetching")
    })
    .await
    .is_err());
}
