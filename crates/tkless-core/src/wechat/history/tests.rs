use super::*;
use serde_json::json;

fn account() -> Account {
    Account {
        biz: Some("abc==".into()),
        nickname: Some("Test".into()),
        username: None,
    }
}
fn item(mid: u64, idx: u64) -> Value {
    json!({"title":format!("{mid}-{idx}"),"content_url":format!("http://mp.weixin.qq.com/s?__biz=abc==&mid={mid}&idx={idx}&sn=signature&pass_ticket=SECRET")})
}
fn page(mid: u64, more: bool, next: u64) -> Value {
    let mut main = item(mid, 1);
    main["multi_app_msg_item_list"] = json!([item(mid, 2)]);
    json!({"ret":0,"can_msg_continue":u8::from(more),"next_offset":next,
        "general_msg_list":json!({"list":[{"comm_msg_info":{"datetime":1234},"app_msg_ext_info":main}]}).to_string()})
}
fn options() -> WechatOptions {
    WechatOptions {
        delay_ms: 0,
        ..Default::default()
    }
}

#[test]
fn expands_secondary_articles_and_strips_session_parameters() {
    let parsed =
        parse_page(page(1, false, 1), &account()).unwrap_or_else(|_| panic!("parse failed"));
    assert_eq!(parsed.articles.len(), 2);
    assert_eq!(parsed.articles[1].id, "abc==:1:2");
    assert_eq!(parsed.articles[1].published_at.as_deref(), Some("1234"));
    assert!(!serde_json::to_string(&parsed.articles)
        .unwrap()
        .contains("SECRET"));
}

#[tokio::test]
async fn expired_session_retains_previous_page_and_resumes_at_saved_offset() {
    let mut state = Checkpoint::new(account());
    collect_with(&mut state, &options(), None, |offset| async move {
        if offset == 0 {
            Ok(page(1, true, 10))
        } else {
            Ok(json!({"ret":-3,"errmsg":"SECRET"}))
        }
    })
    .await
    .unwrap();
    assert_eq!(state.report.articles.len(), 2);
    assert_eq!(state.offset, 10);
    assert_eq!(state.report.stop_reason, StopReason::SessionRequired);
    assert!(!state.report.listing_complete);
    assert!(!serde_json::to_string(&state).unwrap().contains("SECRET"));
    collect_with(&mut state, &options(), None, |offset| async move {
        assert_eq!(offset, 10);
        Ok(page(2, false, 20))
    })
    .await
    .unwrap();
    assert_eq!(state.report.articles.len(), 4);
    assert!(state.report.listing_complete);
    assert!(!state.report.account_history_complete);
}

#[tokio::test]
async fn item_budget_inside_multi_article_message_does_not_skip_secondary_on_resume() {
    let mut state = Checkpoint::new(account());
    let options = WechatOptions {
        limit: 1,
        ..options()
    };
    collect_with(&mut state, &options, None, |_| async {
        Ok(page(1, false, 10))
    })
    .await
    .unwrap();
    assert_eq!(state.offset, 0);
    assert_eq!(state.report.articles.len(), 1);
    assert!(!state.report.listing_complete);
    collect_with(&mut state, &options, None, |_| async {
        Ok(page(1, false, 10))
    })
    .await
    .unwrap();
    assert_eq!(state.report.articles.len(), 2);
    assert!(state.report.listing_complete);
}

#[tokio::test]
async fn repeated_cursor_and_page_budget_do_not_report_completion() {
    let mut state = Checkpoint::new(account());
    collect_with(&mut state, &options(), None, |_| async {
        Ok(page(1, true, 0))
    })
    .await
    .unwrap();
    assert_eq!(state.report.stop_reason, StopReason::RepeatedCursor);
    assert!(!state.report.listing_complete);
    let mut repeated = Checkpoint::new(account());
    collect_with(&mut repeated, &options(), None, |offset| async move {
        Ok(page(1, true, offset + 10))
    })
    .await
    .unwrap();
    assert_eq!(repeated.report.stop_reason, StopReason::RepeatedPage);
    assert!(!repeated.report.listing_complete);
    let mut state = Checkpoint::new(account());
    collect_with(
        &mut state,
        &WechatOptions {
            max_pages: 1,
            ..options()
        },
        None,
        |_| async { Ok(page(1, true, 10)) },
    )
    .await
    .unwrap();
    assert_eq!(state.offset, 10);
    assert_eq!(state.report.stop_reason, StopReason::PageLimit);
    assert!(!state.report.listing_complete);
}

#[test]
fn malformed_and_off_account_history_are_rejected() {
    assert!(parse_page(json!({"ret":0}), &account()).is_err());
    assert!(parse_page(
        json!({"ret":0,"can_msg_continue":0,"general_msg_list":{"list":[{}]}}),
        &account()
    )
    .is_err());
    let other = Account {
        biz: Some("other".into()),
        ..Default::default()
    };
    assert!(parse_page(page(1, false, 10), &other).is_err());
    let mut state = Checkpoint::new(account());
    state.report.accounts.clear();
    assert!(state.validate("abc==").is_err());
    let session = Session {
        biz: "abc==".into(),
        cookie: String::new(),
        parameters: BTreeMap::from([("redirect".into(), "SECRET".into())]),
    };
    assert!(session.validate("abc==").is_err());
}

#[tokio::test]
async fn checkpoint_roundtrip_and_missing_session_report_are_explicit() {
    let path = std::env::temp_dir().join(format!("tkless-history-{}.json", std::process::id()));
    let engine = TklessEngine::with_options(Default::default()).unwrap();
    let history = HistoryOptions {
        checkpoint: Some(path.clone()),
        ..Default::default()
    };
    let report = list_history(
        &engine,
        "abc==",
        &options(),
        &history,
        &ExtractionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.stop_reason, StopReason::SessionRequired);
    assert!(!report.complete);
    let state: Checkpoint = state_file::read(&path).unwrap();
    state.validate("abc==").unwrap();
    assert!(state.validate("other").is_err());
    assert!(list_history(
        &engine,
        "abc==",
        &options(),
        &history,
        &ExtractionOptions::default()
    )
    .await
    .is_err());
    let history = HistoryOptions {
        resume: true,
        ..history
    };
    assert!(list_history(
        &engine,
        "abc==",
        &options(),
        &history,
        &ExtractionOptions::default()
    )
    .await
    .is_ok());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(path.with_extension("lock")).unwrap();
}

#[test]
fn windows_utf8_bom_session_is_accepted_without_leaking_parse_errors() {
    let path = std::env::temp_dir().join(format!("tkless-session-bom-{}.json", std::process::id()));
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend(br#"{"biz":"abc==","cookie":"example=SECRET"}"#);
    std::fs::write(&path, bytes).unwrap();
    assert!(Session::read(&path, "abc==").is_ok());
    std::fs::write(&path, br#"{"SECRET":"bad schema"}"#).unwrap();
    let error = Session::read(&path, "abc==").err().unwrap();
    assert!(!error.to_string().contains("SECRET"));
    std::fs::remove_file(path).unwrap();
}
