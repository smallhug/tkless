use tkless_core::{
    adapters::{PlatformAdapter, WechatAdapter},
    wechat::identity,
    ExtractionOptions,
};
use url::Url;

#[test]
fn article_identity_and_related_albums_are_structured_and_scoped() {
    let url = Url::parse("https://mp.weixin.qq.com/s/example").unwrap();
    let html = r#"<h1 id="activity-name">测试文章</h1><a id="js_name">测试公众号</a><div id="js_content"><p>公开正文</p><img data-src="https://example.com/a.png"></div>
    <script>var biz = "abc=="; var user_name = "gh_example"; var appmsg_album_infos = [{"id":"42","title":"合集"}];</script>
    <a href="https://mp.weixin.qq.com/mp/appmsgalbum?__biz=other&amp;album_id=9">其他账号</a>"#;
    let doc = WechatAdapter::new()
        .parse_article(html, &url, &ExtractionOptions::default())
        .unwrap();
    let entity = doc.entity.unwrap();
    assert_eq!(entity["type"], "wechat_article");
    assert_eq!(entity["account"]["biz"], "abc==");
    assert_eq!(entity["account"]["nickname"], "测试公众号");
    assert_eq!(entity["albums"].as_array().unwrap().len(), 1);
    assert!(entity["albums"][0]
        .as_str()
        .unwrap()
        .contains("album_id=42"));
    assert!(doc.content.contains("公开正文"));
}

#[test]
fn script_album_links_decode_entities_deduplicate_and_enforce_account() {
    let html = r#"<script>
    var album_info_list = [{link: 'https://mp.weixin.qq.com/mp/appmsgalbum?__biz=abc==&amp;album_id=42#wechat_redirect'}];
    var info = {link: 'https://mp.weixin.qq.com/mp/appmsgalbum?__biz=abc==\x26amp;album_id=42'};
    var other = {link: 'https://mp.weixin.qq.com/mp/appmsgalbum?__biz=other&album_id=43'};
    </script>"#;
    let url = Url::parse("https://mp.weixin.qq.com/s/example").unwrap();
    let account = identity::Account {
        biz: Some("abc==".into()),
        ..Default::default()
    };
    let albums = identity::album_links(html, &url, &account);
    assert_eq!(albums.len(), 1);
    let album = Url::parse(&albums[0]).unwrap();
    assert_eq!(
        identity::parameter(&album, "album_id").as_deref(),
        Some("42")
    );
    assert_eq!(album.fragment(), None);
    assert!(identity::album_links(html, &url, &Default::default()).is_empty());
}

#[test]
fn short_and_query_article_urls_share_stable_identity() {
    let short = Url::parse("https://mp.weixin.qq.com/s/example").unwrap();
    let long = Url::parse("https://mp.weixin.qq.com/s?__biz=abc==&mid=123&idx=1").unwrap();
    let account = identity::Account {
        biz: Some("abc==".into()),
        ..Default::default()
    };
    let html = r#"<script>var mid = "123" || ""; var idx = "1" || "";</script>"#;
    assert_eq!(
        identity::article_id_from_html(html, &short, &account),
        identity::article_id(&long, &account)
    );
    assert_eq!(
        identity::article_id_from_html("", &short, &account),
        short.as_str()
    );
}

#[test]
fn unrelated_routes_lookalike_hosts_and_error_shells_are_rejected() {
    let adapter = WechatAdapter::new();
    assert!(!adapter.allow_html_fallback(&Url::parse("https://mp.weixin.qq.com/s/a").unwrap()));
    for raw in [
        "https://mp.weixin.qq.com/something",
        "https://weixin.sogou.com.evil.test/link?url=x",
    ] {
        assert!(!adapter.matches(&Url::parse(raw).unwrap()));
    }
    assert!(identity::public_url("https://mp.weixin.qq.com@evil.test/s/a").is_err());
    assert!(identity::public_url("https://mp.weixin.qq.com:8443/s/a").is_err());
    assert!(adapter
        .parse_article(
            "<title>未知错误</title><p>请求失败</p>",
            &Url::parse("https://mp.weixin.qq.com/s/a").unwrap(),
            &ExtractionOptions::default()
        )
        .is_err());
}

#[tokio::test]
async fn mcp_public_wechat_tool_is_discoverable_and_validates_limits() {
    let tools = tkless_core::mcp::tools::get_available_tools();
    let tool = tools
        .iter()
        .find(|t| t.name == "list_wechat_articles")
        .unwrap();
    assert!(tool.output_schema.is_some());
    let engine = tkless_core::TklessEngine::with_options(Default::default()).unwrap();
    let server = tkless_core::McpServer::new(engine, tkless_core::DocumentCache::memory_only(60));
    let result = server
        .dispatch_tool_execution(
            "list_wechat_articles",
            &serde_json::json!({"source":"测试公众号","max_pages":0}),
            None,
            None,
        )
        .await;
    assert!(result.is_error);
    assert_eq!(
        result.structured_content.unwrap()["error"]["code"],
        "invalid_argument"
    );
    let result = server
        .dispatch_tool_execution(
            "list_wechat_articles",
            &serde_json::json!({"source":"abc==","history":true}),
            None,
            None,
        )
        .await;
    assert!(!result.is_error);
    let report = result.structured_content.unwrap();
    assert_eq!(report["scope"], "account_history");
    assert_eq!(report["stop_reason"], "session_required");
    assert_eq!(report["complete"], false);
}
