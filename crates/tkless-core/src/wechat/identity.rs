//! Public WeChat URL and article identity parsing; no script execution.
use crate::TklessError;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use url::Url;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub biz: Option<String>,
    pub nickname: Option<String>,
    pub username: Option<String>,
}

pub(crate) fn failure(message: impl Into<String>) -> TklessError {
    TklessError::AdapterError {
        platform: "wechat",
        message: message.into(),
    }
}

pub fn public_url(raw: &str) -> Result<Url, TklessError> {
    let mut url = Url::parse(&raw.replace("&amp;", "&"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("mp.weixin.qq.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(TklessError::InvalidArgument(
            "Expected a public mp.weixin.qq.com URL".into(),
        ));
    }
    url.set_scheme("https")
        .map_err(|_| failure("Invalid URL scheme"))?;
    url.set_fragment(None);
    Ok(url)
}

pub fn is_article(url: &Url) -> bool {
    url.host_str() == Some("mp.weixin.qq.com")
        && (url.path() == "/s" || url.path().starts_with("/s/"))
}

pub fn parameter(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

pub(super) fn valid_biz(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=_-".contains(&b))
}

pub fn article_id(url: &Url, account: &Account) -> String {
    if let (Some(biz), Some(mid), Some(idx)) = (
        account.biz.as_ref(),
        parameter(url, "mid"),
        parameter(url, "idx"),
    ) {
        format!("{biz}:{mid}:{idx}")
    } else {
        let mut normalized = url.clone();
        normalized.set_query(None);
        // Query-style links without enough identity fields must not collapse to /s.
        if url.path() == "/s" {
            url.to_string()
        } else {
            normalized.to_string()
        }
    }
}

static IDENTITY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"\b(?:var\s+)?(biz|nickname|user_name)\s*=\s*['"]([^'"\r\n]*)['"]"#)
        .expect("static identity regex")
});
static ARTICLE_IDS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"\bvar\s+(mid|idx)\s*=\s*['"]([0-9]{1,32})['"]"#)
        .expect("static article identity regex")
});

pub fn article_id_from_html(html: &str, url: &Url, account: &Account) -> String {
    let mut identity_url = url.clone();
    let doc = Html::parse_document(html);
    for script in doc.select(&SCRIPTS) {
        for captures in ARTICLE_IDS.captures_iter(&script.inner_html()) {
            if parameter(&identity_url, &captures[1]).is_none() {
                identity_url
                    .query_pairs_mut()
                    .append_pair(&captures[1], &captures[2]);
            }
        }
    }
    if parameter(&identity_url, "mid").is_some() && parameter(&identity_url, "idx").is_some() {
        article_id(&identity_url, account)
    } else {
        article_id(url, account)
    }
}
static NAME: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#js_name, .profile_nickname, .rich_media_meta_nickname")
        .expect("static selector")
});
static LINKS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").expect("static selector"));
static SCRIPTS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("script").expect("static selector"));
static ALBUM_LITERAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"['"](https?://mp\.weixin\.qq\.com/mp/appmsgalbum\?[^'"\r\n<>]+)['"]"#)
        .expect("static album URL regex")
});

pub fn article_identity(html: &str, url: &Url) -> Account {
    let doc = Html::parse_document(html);
    let mut account = Account {
        biz: parameter(url, "__biz").filter(|v| valid_biz(v)),
        nickname: doc
            .select(&NAME)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_owned())
            .filter(|s| !s.is_empty()),
        username: None,
    };
    for captures in IDENTITY.captures_iter(html) {
        let value = captures[2]
            .replace("\\x3d", "=")
            .replace("\\x3D", "=")
            .replace("\\u003d", "=")
            .replace("&amp;", "&");
        match &captures[1] {
            "biz" if account.biz.is_none() && valid_biz(&value) => account.biz = Some(value),
            "nickname" if account.nickname.is_none() && !value.is_empty() => {
                account.nickname = Some(value)
            }
            "user_name" if !value.is_empty() => account.username = Some(value),
            _ => {}
        }
    }
    account
}

pub fn album_links(html: &str, base: &Url, account: &Account) -> Vec<String> {
    let doc = Html::parse_document(html);
    let mut urls = Vec::new();
    for element in doc.select(&LINKS) {
        if let Some(raw) = element.value().attr("href") {
            if let Ok(url) = base.join(raw).and_then(|url| Url::parse(url.as_str())) {
                if let Ok(url) = public_url(url.as_str()) {
                    if url.path() == "/mp/appmsgalbum"
                        && parameter(&url, "album_id").is_some()
                        && account
                            .biz
                            .as_ref()
                            .is_some_and(|biz| parameter(&url, "__biz").as_ref() == Some(biz))
                        && !urls.contains(&url.to_string())
                    {
                        urls.push(url.to_string());
                    }
                }
            }
        }
    }
    // Some public article templates expose a JSON array instead of rendered album anchors.
    if let (Some(biz), Some(start)) = (&account.biz, html.find("appmsg_album_infos")) {
        if let Some((_, tail)) = html[start..].split_once('=') {
            if let Some(Ok(serde_json::Value::Array(items))) =
                serde_json::Deserializer::from_str(tail.trim_start())
                    .into_iter::<serde_json::Value>()
                    .next()
            {
                for item in items {
                    let id = item.get("id").or_else(|| item.get("album_id"));
                    let id = id.and_then(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    });
                    if let Some(id) =
                        id.filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
                    {
                        let mut url = base.clone();
                        url.set_path("/mp/appmsgalbum");
                        url.set_query(None);
                        url.query_pairs_mut()
                            .append_pair("__biz", biz)
                            .append_pair("album_id", &id);
                        if !urls.contains(&url.to_string()) {
                            urls.push(url.to_string());
                        }
                    }
                }
            }
        }
    }
    // Current templates embed album_info_list / appmsgalbuminfo as JavaScript,
    // not JSON. Read only URL literals; never evaluate the page's script.
    for script in doc.select(&SCRIPTS) {
        for captures in ALBUM_LITERAL.captures_iter(&script.inner_html()) {
            let raw = captures[1].replace("\\x26", "&").replace("\\u0026", "&");
            if let Ok(url) = public_url(&raw) {
                urls.push(url.to_string());
            }
        }
    }
    let mut normalized = Vec::new();
    for raw in urls {
        let Ok(url) = public_url(&raw) else { continue };
        let (Some(biz), Some(id)) = (parameter(&url, "__biz"), parameter(&url, "album_id")) else {
            continue;
        };
        if account.biz.as_ref() != Some(&biz)
            || id.is_empty()
            || id.len() > 32
            || !id.bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        let mut clean = Url::parse("https://mp.weixin.qq.com/mp/appmsgalbum").expect("static URL");
        clean
            .query_pairs_mut()
            .append_pair("__biz", &biz)
            .append_pair("album_id", &id);
        let value = clean.to_string();
        if !normalized.contains(&value) {
            normalized.push(value);
        }
        if normalized.len() == 20 {
            break;
        }
    }
    normalized
}
