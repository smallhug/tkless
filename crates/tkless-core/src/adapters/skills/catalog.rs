//! Public endpoints used by the site's own search and infinite-scroll clients.
use super::{escape, failure, finish_document, route, ParsedPage, Route};
use crate::{fetcher::Fetcher, DiagnosticWarning, ExtractionOptions, PageDocument, TklessError};
use serde_json::{json, Value};
use std::{collections::HashSet, future::Future};
use url::Url;

pub(super) async fn extract(
    fetcher: &Fetcher,
    url: &Url,
    options: &ExtractionOptions,
) -> Result<PageDocument, TklessError> {
    let mut canonical = url.clone();
    canonical
        .set_scheme("https")
        .map_err(|_| failure("Invalid scheme"))?;
    canonical.set_host(Some("www.skills.sh"))?;
    canonical
        .set_port(None)
        .map_err(|_| failure("Invalid port"))?;
    canonical.set_fragment(None);
    canonical
        .set_username("")
        .map_err(|_| failure("Invalid username"))?;
    canonical
        .set_password(None)
        .map_err(|_| failure("Invalid password"))?;
    if matches!(route(&canonical), Some(Route::Search(_))) {
        canonical.set_path("/search");
    }
    let mut document = collect(&canonical, options, |target| async move {
        let response = fetcher.fetch_api(target.as_str()).await?;
        if !(200..300).contains(&response.status) {
            return Err(TklessError::HttpStatusError {
                status: response.status,
                url: response.final_url,
            });
        }
        if response.truncated {
            return Err(failure(
                "Catalog response was truncated before JSON decoding",
            ));
        }
        serde_json::from_str(&response.html)
            .map_err(|_| failure("Catalog endpoint returned invalid JSON"))
    })
    .await?;
    document.url = url.to_string();
    Ok(document)
}

fn number(row: &Value, key: &str) -> Result<Option<u64>, TklessError> {
    match row.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| failure("Invalid numeric catalog field")),
    }
}

fn normalized(row: &Value) -> Result<Value, TklessError> {
    let source = row
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| failure("Catalog entry has no source"))?;
    let slug = row
        .get("skillId")
        .or_else(|| row.get("slug"))
        .and_then(Value::as_str)
        .ok_or_else(|| failure("Catalog entry has no skill identifier"))?;
    let parts: Vec<_> = source.split('/').collect();
    let valid = |part: &str| {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && !part
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | '?' | '#'))
    };
    if !matches!(parts.len(), 1 | 2) || !parts.iter().all(|p| valid(p)) || !valid(slug) {
        return Err(failure("Catalog entry has an invalid source or identifier"));
    }
    let mut target = Url::parse("https://www.skills.sh/")?;
    {
        let mut path = target
            .path_segments_mut()
            .map_err(|_| failure("Invalid catalog base"))?;
        path.clear();
        if parts.len() == 1 {
            path.push("site");
        }
        for part in &parts {
            path.push(part);
        }
        path.push(slug);
    }
    let change = match row.get("change") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_i64()
                .ok_or_else(|| failure("Invalid catalog change value"))?,
        ),
    };
    Ok(
        json!({"id":format!("{source}/{slug}"),"source":source,"slug":slug,
        "source_type":if parts.len()==1 {"well-known"} else {"github"},
        "name":row.get("name").and_then(Value::as_str).unwrap_or(slug),"url":target.as_str(),
        "description":row.get("description").and_then(Value::as_str),
        "installs":number(row,"installs")?,"installs_yesterday":number(row,"installsYesterday")?,
        "change":change,"metric_display":Value::Null}),
    )
}

fn rows(payload: &Value) -> Result<Vec<Value>, TklessError> {
    payload
        .get("skills")
        .and_then(Value::as_array)
        .ok_or_else(|| failure("Catalog response has no skills array"))?
        .iter()
        .map(normalized)
        .collect()
}

fn body(title: &str, skills: &[Value], notice: &str) -> String {
    let mut out = format!("<h1>{}</h1><p>{}</p><ol>", escape(title), escape(notice));
    for row in skills {
        out.push_str(&format!(
            "<li><a href=\"{}\">{}</a> — {}",
            escape(row["url"].as_str().unwrap_or_default()),
            escape(row["name"].as_str().unwrap_or_default()),
            escape(row["source"].as_str().unwrap_or_default())
        ));
        if let Some(count) = row["installs"].as_u64() {
            out.push_str(&format!(" — {count} installs"));
        }
        if let Some(change) = row["change"].as_i64() {
            out.push_str(&format!(" (change {change:+})"));
        }
        if let Some(description) = row["description"].as_str() {
            out.push_str(&format!("<p>{}</p>", escape(description)));
        }
        out.push_str("</li>");
    }
    out.push_str("</ol>");
    if skills.is_empty() {
        out.push_str("<p>No skills found.</p>");
    }
    out
}

async fn collect<F, Fut>(
    url: &Url,
    options: &ExtractionOptions,
    mut get: F,
) -> Result<PageDocument, TklessError>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<Value, TklessError>>,
{
    options.validate()?;
    let kind = route(url).ok_or_else(|| failure("Unsupported catalog URL"))?;
    if let Route::Search(query) = kind {
        if url
            .query_pairs()
            .any(|(key, _)| matches!(key.as_ref(), "page" | "offset"))
        {
            return Err(TklessError::InvalidArgument("Skills.sh public search does not support page or offset; use skills_limit up to 200".into()));
        }
        if !(2..=2000).contains(&query.chars().count()) {
            return Err(TklessError::InvalidArgument(
                "Skills.sh search query must contain 2..=2000 characters".into(),
            ));
        }
        let limit = options.skills_limit.min(200);
        let mut target = url.join("/api/search")?;
        target
            .query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("limit", &limit.to_string());
        let payload = get(target).await?;
        let raw = rows(&payload)?;
        let capped = raw.len() >= limit;
        let mut seen = HashSet::new();
        let skills: Vec<_> = raw
            .into_iter()
            .filter(|row| seen.insert(row["id"].clone()))
            .take(limit)
            .collect();
        let title = format!("Skills.sh search: {query}");
        let entity = json!({"platform":"skills.sh","page_type":"search","data_source":"public_site_api",
            "scope":"search_top_results","query":query,"search_type":payload.get("searchType"),
            "count":skills.len(),"skills":skills,"search_exhaustive":false,
            "pagination":{"supported":false,"pages_fetched":1,"complete":Value::Null,"has_more":Value::Null,
                "request_limit":limit,"stop_reason":if capped {"search_limit"} else {"search_response"}}});
        let notice = format!(
            "{} ranked results; request limit {limit}. Public site search has no next-page cursor.",
            skills.len()
        );
        return finish_document("",url.as_str(),options,ParsedPage {body:body(&title,&skills,&notice),title,entity,
            warnings:vec![DiagnosticWarning::new("skills_search_unpaged","The public site search returns ranked top results (at most 200) and exposes no pagination cursor; page/offset are not supported.")],incomplete:capped});
    }
    if kind != Route::Leaderboard {
        return Err(failure("Not a pageable leaderboard"));
    }
    let view = match url.path().trim_matches('/') {
        "hot" => "hot",
        "trending" => "trending",
        _ => "all-time",
    };
    let start = url
        .query_pairs()
        .find(|(key, _)| key == "page")
        .map(|(_, value)| value.parse::<usize>())
        .transpose()
        .map_err(|_| {
            TklessError::InvalidArgument("Skills.sh page must be a nonnegative integer".into())
        })?
        .unwrap_or(0);
    if start > 50000 {
        return Err(TklessError::InvalidArgument(
            "Skills.sh page must be at most 50000".into(),
        ));
    }
    let mut skills = Vec::new();
    let mut seen = HashSet::new();
    let mut warnings = Vec::new();
    let mut fetched = 0;
    let mut total = None;
    let mut next_page = Some(start);
    let mut duplicates = 0;
    let mut has_more = true;
    let mut reason = "max_pages";
    let mut page_error = None;
    for page in start..start + options.skills_pages {
        let target = url.join(&format!("/api/skills/{view}/{page}"))?;
        let result = async {
            let payload = get(target).await?;
            if payload.get("page").and_then(Value::as_u64) != Some(page as u64) {
                return Err(failure(
                    "Leaderboard endpoint returned an unexpected page number",
                ));
            }
            let more = payload
                .get("hasMore")
                .and_then(Value::as_bool)
                .ok_or_else(|| failure("Leaderboard response has no hasMore flag"))?;
            Ok((
                rows(&payload)?,
                more,
                payload.get("total").and_then(Value::as_u64),
            ))
        }
        .await;
        let (entries, more, reported_total) = match result {
            Ok(result) => result,
            Err(error) if fetched == 0 => return Err(error),
            Err(error) => {
                page_error = Some(error.details());
                reason = "page_error";
                warnings.push(DiagnosticWarning::new(
                    "skills_page_failed",
                    format!(
                        "Page {page} failed; previously collected results were retained: {error}"
                    ),
                ));
                break;
            }
        };
        fetched += 1;
        total = reported_total.or(total);
        has_more = more;
        next_page = more.then_some(page + 1);
        let raw_len = entries.len();
        let before = skills.len();
        for (index, row) in entries.into_iter().enumerate() {
            if seen.insert(row["id"].clone()) {
                skills.push(row);
            } else {
                duplicates += 1;
            }
            if skills.len() == options.skills_limit {
                reason = "result_limit";
                if index + 1 < raw_len {
                    has_more = true;
                    next_page = None;
                }
                break;
            }
        }
        if skills.len() == options.skills_limit {
            if !has_more {
                reason = "exhausted";
            }
            break;
        }
        if !more {
            reason = "exhausted";
            break;
        }
        if skills.len() == before {
            reason = if raw_len == 0 {
                "empty_page"
            } else {
                "repeated_page"
            };
            break;
        }
    }
    let complete = !has_more && reason == "exhausted";
    if !complete && page_error.is_none() {
        warnings.push(DiagnosticWarning::new(
            "skills_pagination_incomplete",
            format!("Leaderboard collection stopped: {reason}; this is a partial catalog."),
        ));
    }
    let title = format!("Skills.sh leaderboard — {view}");
    let entity = json!({"platform":"skills.sh","page_type":"leaderboard","data_source":"public_site_api",
        "scope":"paginated_catalog","view":view,"count":skills.len(),"skills":skills,
        "pagination":{"supported":true,"start_page":start,"pages_fetched":fetched,"complete":complete,
            "has_more":has_more,"next_page":next_page,"stop_reason":reason,"total_reported":total,
            "duplicates_removed":duplicates,"page_error":page_error}});
    let notice = format!(
        "Fetched {fetched} pages; {} unique skills; stop: {reason}. {}",
        skills.len(),
        if complete {
            "All pages in this API listing were consumed."
        } else {
            "This is a partial catalog."
        }
    );
    finish_document(
        "",
        url.as_str(),
        options,
        ParsedPage {
            body: body(&title, &skills, &notice),
            title,
            entity,
            warnings,
            incomplete: !complete,
        },
    )
}

#[cfg(test)]
mod tests;
