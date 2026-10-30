//! Public getalbum pagination. This is a website endpoint, not the authorized account API.
use super::{
    identity::{self, failure, Account},
    Article, StopReason, WechatOptions, WechatReport,
};
use crate::{fetcher::Fetcher, TklessError};
use serde_json::Value;
use std::{collections::HashSet, future::Future};
use url::Url;

fn scalar(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}
fn numeric(value: &Value) -> Option<u64> {
    scalar(value)?.parse().ok()
}

fn album_url(url: &Url) -> Result<Url, TklessError> {
    let source = identity::public_url(url.as_str())?;
    if source.path() != "/mp/appmsgalbum" {
        return Err(failure("Expected a WeChat album URL"));
    }
    let id = identity::parameter(&source, "album_id")
        .filter(|s| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| failure("Album URL requires a numeric album_id"))?;
    let biz = identity::parameter(&source, "__biz")
        .filter(|s| identity::valid_biz(s))
        .ok_or_else(|| failure("Album URL requires __biz"))?;
    let mut clean = Url::parse("https://mp.weixin.qq.com/mp/appmsgalbum")?;
    clean
        .query_pairs_mut()
        .append_pair("action", "getalbum")
        .append_pair("__biz", &biz)
        .append_pair("album_id", &id)
        .append_pair("f", "json");
    for key in ["begin_msgid", "begin_itemidx"] {
        if let Some(value) = identity::parameter(&source, key) {
            if value.is_empty() || value.len() > 32 || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(failure("Invalid album cursor"));
            }
            clean.query_pairs_mut().append_pair(key, &value);
        }
    }
    if identity::parameter(&clean, "begin_msgid").is_some()
        != identity::parameter(&clean, "begin_itemidx").is_some()
    {
        return Err(failure("Both album cursor fields are required"));
    }
    Ok(clean)
}

pub async fn collect(
    fetcher: &Fetcher,
    url: &Url,
    options: &WechatOptions,
) -> Result<WechatReport, TklessError> {
    collect_with(url, options, |url| async move {
        let raw = fetcher.fetch_api(url.as_str()).await?;
        if raw.truncated {
            return Err(failure("Album response is truncated"));
        }
        if !(200..300).contains(&raw.status) {
            return Err(failure(format!("Album HTTP status {}", raw.status)));
        }
        let final_url = identity::public_url(&raw.final_url)?;
        if final_url.path() != "/mp/appmsgalbum" {
            return Err(failure("Album redirected to verification or another route"));
        }
        serde_json::from_str(&raw.html)
            .map_err(|_| failure("Album returned a verification/error page instead of JSON"))
    })
    .await
}

async fn collect_with<F, Fut>(
    url: &Url,
    options: &WechatOptions,
    mut fetch: F,
) -> Result<WechatReport, TklessError>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<Value, TklessError>>,
{
    options.validate()?;
    let mut current = album_url(url)?;
    let biz = identity::parameter(&current, "__biz").ok_or_else(|| failure("Missing biz"))?;
    if options
        .expected_biz
        .as_ref()
        .is_some_and(|value| value != &biz)
    {
        return Err(failure("Album does not belong to expected_biz"));
    }
    let resumed = identity::parameter(&current, "begin_msgid").is_some();
    let mut report = WechatReport::new(
        url.as_str(),
        if resumed { "album_remaining" } else { "album" },
    );
    report.stop_reason = StopReason::PageLimit;
    report.albums.push(url.to_string());
    let mut ids = HashSet::new();
    let mut cursors = HashSet::new();
    for _ in 0..options.max_pages {
        let count = 20.min(options.limit - report.articles.len());
        let pairs: Vec<_> = current
            .query_pairs()
            .filter(|(key, _)| key != "count")
            .map(|(a, b)| (a.into_owned(), b.into_owned()))
            .collect();
        current.set_query(None);
        current
            .query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("count", &count.to_string());
        tokio::time::sleep(std::time::Duration::from_millis(options.delay_ms)).await;
        let page = match fetch(current.clone())
            .await
            .and_then(|data| parse_page(data, &biz))
        {
            Ok(page) => page,
            Err(error) => {
                report.stop_reason = StopReason::FetchFailed;
                report.errors.push(error.to_string());
                report.next_urls.push(current.to_string());
                return Ok(report);
            }
        };
        report.pages_fetched += 1;
        report.album_title = page.title.or(report.album_title);
        if report.total_hint.is_none() {
            report.total_hint = page.total;
        }
        if report.accounts.is_empty() {
            report.accounts.push(page.account);
        }
        let before = report.articles.len();
        let mut last = None;
        let mut overflow = false;
        for (article, cursor) in page.articles {
            if ids.contains(&article.id) {
                report.duplicates += 1;
                last = Some(cursor);
                continue;
            }
            if report.articles.len() == options.limit {
                overflow = true;
                break;
            }
            ids.insert(article.id.clone());
            report.articles.push(article);
            last = Some(cursor);
        }
        if !page.more && !overflow {
            report.stop_reason = StopReason::Exhausted;
            report.listing_complete = true;
            if !resumed
                && report
                    .total_hint
                    .is_some_and(|n| n != report.articles.len() as u64)
            {
                report.listing_complete = false;
                report.stop_reason = StopReason::InvalidPage;
                report.errors.push("Album total differs from collected unique articles; the album may have changed or omitted entries".into());
            }
            report.complete = report.listing_complete;
            break;
        }
        if report.articles.len() == before {
            report.stop_reason = StopReason::RepeatedPage;
            report.next_urls.push(current.to_string());
            break;
        }
        let (msgid, idx) = last.ok_or_else(|| failure("Nonterminal album page has no cursor"))?;
        if !cursors.insert((msgid.clone(), idx.clone())) {
            report.stop_reason = StopReason::RepeatedCursor;
            report.next_urls.push(current.to_string());
            break;
        }
        let pairs: Vec<_> = current
            .query_pairs()
            .filter(|(k, _)| k != "begin_msgid" && k != "begin_itemidx")
            .map(|(a, b)| (a.into_owned(), b.into_owned()))
            .collect();
        current.set_query(None);
        current
            .query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair("begin_msgid", &msgid)
            .append_pair("begin_itemidx", &idx);
        report.next_urls = vec![current.to_string()];
        if report.articles.len() >= options.limit {
            report.stop_reason = StopReason::ItemLimit;
            break;
        }
        report.next_urls.clear();
    }
    if !report.listing_complete && report.next_urls.is_empty() {
        report.next_urls.push(current.to_string());
    }
    if report.listing_complete {
        report.next_urls.clear();
    }
    report.warnings.push(
        "Album coverage is limited to this public album; it is not the account's full history."
            .into(),
    );
    Ok(report)
}

struct Page {
    account: Account,
    title: Option<String>,
    total: Option<u64>,
    more: bool,
    articles: Vec<(Article, (String, String))>,
}
fn parse_page(data: Value, biz: &str) -> Result<Page, TklessError> {
    if numeric(&data["base_resp"]["ret"]) != Some(0) {
        return Err(failure(format!(
            "WeChat album request failed (ret={})",
            data["base_resp"]["ret"]
        )));
    }
    let album = &data["getalbum_resp"];
    let more = match scalar(&album["continue_flag"]).as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => return Err(failure("Missing or invalid album continue_flag")),
    };
    let account = Account {
        biz: Some(biz.into()),
        nickname: album["base_info"]["nickname"].as_str().map(str::to_owned),
        username: album["base_info"]["username"].as_str().map(str::to_owned),
    };
    let rows = album["article_list"]
        .as_array()
        .ok_or_else(|| failure("Album article_list is missing"))?;
    if rows.len() > 100 {
        return Err(failure("Unexpected album page size"));
    }
    if rows.is_empty() && more {
        return Err(failure("Empty album page still claims continuation"));
    }
    let mut articles = Vec::new();
    for row in rows {
        let url = identity::public_url(
            row["url"]
                .as_str()
                .ok_or_else(|| failure("Album article URL missing"))?,
        )?;
        if !identity::is_article(&url) || identity::parameter(&url, "__biz").as_deref() != Some(biz)
        {
            return Err(failure("Album contains an off-account article URL"));
        }
        let msgid = scalar(&row["msgid"])
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| failure("Invalid msgid"))?;
        let idx = scalar(&row["itemidx"])
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| failure("Invalid itemidx"))?;
        if identity::parameter(&url, "mid").as_ref() != Some(&msgid)
            || identity::parameter(&url, "idx").as_ref() != Some(&idx)
        {
            return Err(failure("Article URL and cursor identity disagree"));
        }
        articles.push((
            Article {
                id: identity::article_id(&url, &account),
                url: url.to_string(),
                title: row["title"]
                    .as_str()
                    .ok_or_else(|| failure("Article title missing"))?
                    .into(),
                account: account.clone(),
                published_at: scalar(&row["create_time"]),
                cover: row["cover_img_1_1"].as_str().map(str::to_owned),
                document: None,
                error: None,
            },
            (msgid, idx),
        ));
    }
    Ok(Page {
        account,
        title: album["base_info"]["title"].as_str().map(str::to_owned),
        total: numeric(&album["base_info"]["article_count"]),
        more,
        articles,
    })
}

#[cfg(test)]
mod tests;
