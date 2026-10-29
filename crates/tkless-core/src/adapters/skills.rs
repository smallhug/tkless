//! Public skills.sh pages. Next Flight data is decoded as data, never executed.
use super::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::fetcher::Fetcher;
use crate::model::{DiagnosticWarning, ExtractionOptions, OutputFormat, PageDocument};
use crate::token::{count_tokens, truncate_to_tokens};
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use url::Url;

mod catalog;

#[derive(Default)]
pub struct SkillsAdapter;

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Detail { source: String, slug: String },
    Repository,
    Leaderboard,
    Search(String),
}

fn route(url: &Url) -> Option<Route> {
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(url.host_str(), Some("skills.sh" | "www.skills.sh"))
    {
        return None;
    }
    let parts: Vec<_> = url
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() || parts.as_slice() == ["search"] {
        if let Some((_, query)) = url.query_pairs().find(|(key, _)| key == "q") {
            return Some(Route::Search(query.trim().into()));
        }
        if !parts.is_empty() {
            return Some(Route::Search(String::new()));
        }
    }
    if parts.is_empty() || matches!(parts.as_slice(), ["trending" | "hot"]) {
        return Some(Route::Leaderboard);
    }
    if matches!(
        parts[0],
        "docs"
            | "api"
            | "search"
            | "topic"
            | "topics"
            | "agents"
            | "packs"
            | "official"
            | "security"
    ) {
        return None;
    }
    match parts.as_slice() {
        ["site", domain, slug] if domain.contains('.') => Some(Route::Detail {
            source: (*domain).into(),
            slug: (*slug).into(),
        }),
        [owner, _] if !owner.contains('.') => Some(Route::Repository),
        [owner, repo, slug] => Some(Route::Detail {
            source: format!("{owner}/{repo}"),
            slug: (*slug).into(),
        }),
        _ => None,
    }
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("static skills selector")
}
fn text(el: ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_owned()
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn failure(message: &str) -> TklessError {
    TklessError::AdapterError {
        platform: "skills",
        message: message.into(),
    }
}

/// Only the JSON argument of Flight push calls is read. No JS evaluation or HTTP follow-ups.
fn flight_stream(doc: &Html) -> String {
    let mut stream = String::new();
    for script in doc.select(&sel("script")) {
        let script = script.text().collect::<String>();
        for (offset, marker) in script.match_indices("self.__next_f.push(") {
            let argument = &script[offset + marker.len()..];
            if let Some(Ok(Value::Array(parts))) = serde_json::Deserializer::from_str(argument)
                .into_iter::<Value>()
                .next()
            {
                if parts.first().and_then(Value::as_u64) == Some(1) {
                    if let Some(chunk) = parts.get(1).and_then(Value::as_str) {
                        stream.push_str(chunk);
                    }
                }
            }
        }
    }
    stream
}

#[derive(Default)]
struct FlightData {
    records: Vec<Value>,
    strings: HashMap<String, String>,
}

impl FlightData {
    fn parse(stream: &str) -> Self {
        let mut out = Self::default();
        let mut remaining = stream;
        while !remaining.is_empty() {
            remaining = remaining.trim_start_matches('\n');
            let Some(colon) = remaining.find(':') else {
                break;
            };
            let id = &remaining[..colon];
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                remaining = remaining.split_once('\n').map_or("", |(_, rest)| rest);
                continue;
            }
            let payload = &remaining[colon + 1..];
            if let Some(raw) = payload.strip_prefix('T') {
                if let Some((length, body)) = raw.split_once(',') {
                    if let Ok(length) = usize::from_str_radix(length, 16) {
                        if let (Some(value), Some(rest)) = (body.get(..length), body.get(length..))
                        {
                            out.strings.insert(id.into(), value.into());
                            remaining = rest;
                            continue;
                        }
                    }
                }
                break; // Incomplete length-delimited record: never consume a partial body.
            }
            let mut values = serde_json::Deserializer::from_str(payload).into_iter::<Value>();
            if let Some(Ok(value)) = values.next() {
                if let Some(s) = value.as_str() {
                    out.strings.insert(id.into(), s.into());
                }
                out.records.push(value);
                remaining = &payload[values.byte_offset()..];
                remaining = remaining.trim_start_matches('\n');
            } else {
                remaining = payload.split_once('\n').map_or("", |(_, rest)| rest);
            }
        }
        out
    }

    fn resolve(&self, value: &Value) -> Option<String> {
        let value = value.as_str()?;
        if let Some(literal) = value.strip_prefix("$$") {
            return Some(format!("${literal}"));
        }
        if let Some(id) = value.strip_prefix('$') {
            return self.strings.get(id).cloned();
        }
        Some(value.into())
    }

    fn object_with(&self, key: &str) -> Option<&serde_json::Map<String, Value>> {
        fn find<'a>(value: &'a Value, key: &str) -> Option<&'a serde_json::Map<String, Value>> {
            match value {
                Value::Object(object) if object.contains_key(key) => Some(object),
                Value::Object(object) => object.values().find_map(|v| find(v, key)),
                Value::Array(array) => array.iter().find_map(|v| find(v, key)),
                _ => None,
            }
        }
        self.records.iter().find_map(|value| find(value, key))
    }
}

impl SkillsAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Parse a fetched public page; `entity` describes the page, not a source SKILL.md file.
    pub fn parse_html(
        html: &str,
        page_url: &str,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        options.validate()?;
        let url = Url::parse(page_url)?;
        let page_route = route(&url).ok_or_else(|| failure("Unsupported skills.sh page"))?;
        if options.selector.is_some() {
            return crate::TklessEngine::new()?.process_html(html, Some(page_url), options);
        }
        let doc = Html::parse_document(html);
        let main = doc
            .select(&sel("main"))
            .next()
            .ok_or_else(|| failure("Page has no main content"))?;
        let flight = FlightData::parse(&flight_stream(&doc));
        let title_selector = if page_route == Route::Leaderboard {
            "h1, h2"
        } else {
            "h1"
        };
        let title = main
            .select(&sel(title_selector))
            .next()
            .map(text)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| failure("Page title missing; page layout may have changed"))?;
        let mut warnings = Vec::new();
        let (body, entity, incomplete) = match page_route {
            Route::Search(_) => {
                return Err(failure(
                    "Search results must be loaded from the public search endpoint",
                ))
            }
            Route::Detail { source, slug } => {
                let prose: Vec<_> = main.select(&sel(".prose")).collect();
                let summary = prose.first().map(|el| text(*el));
                let visible = prose
                    .last()
                    .filter(|_| prose.len() >= 2)
                    .map(|el| el.inner_html());
                let collapsed = main
                    .select(&sel("button"))
                    .any(|el| text(el).eq_ignore_ascii_case("Show more"));
                let (readme, complete) = if let Some(props) = flight.object_with("previewHtml") {
                    let preview = props.get("previewHtml").and_then(|v| flight.resolve(v));
                    let rest = props.get("restHtml").and_then(|v| flight.resolve(v));
                    match (preview, rest) {
                        (Some(preview), Some(rest)) => (format!("{preview}\n{rest}"), true),
                        (Some(preview), None) => (preview, false),
                        _ => (visible.unwrap_or_default(), false),
                    }
                } else {
                    (visible.unwrap_or_default(), !collapsed)
                };
                if readme.trim().is_empty() {
                    return Err(failure(
                        "Skill body missing; refusing to return a summary as SKILL.md",
                    ));
                }
                if !complete {
                    warnings.push(DiagnosticWarning::new("skills_readme_preview", "Only the rendered skill preview was available; the folded content could not be decoded."));
                }
                let command = flight
                    .object_with("command")
                    .and_then(|o| o.get("command"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        main.select(&sel("button code"))
                            .next()
                            .map(text)
                            .map(|s| s.trim_start_matches('$').trim().into())
                    });
                let repository_url = main
                    .select(&sel("a[href]"))
                    .filter_map(|el| el.value().attr("href"))
                    .find(|href| {
                        Url::parse(href).is_ok_and(|u| {
                            u.host_str() == Some("github.com")
                                && u.path().trim_matches('/') == source
                        })
                    })
                    .map(str::to_owned);
                let mut body = format!("<h1>{}</h1>", escape(&title));
                if let Some(summary) = &summary {
                    body.push_str(&format!("<h2>Summary</h2><p>{}</p>", escape(summary)));
                }
                if let Some(repository) = &repository_url {
                    body.push_str(&format!(
                        "<p>Repository: <a href=\"{}\">{}</a></p>",
                        escape(repository),
                        escape(&source)
                    ));
                }
                if let Some(command) = &command {
                    body.push_str(&format!(
                        "<h2>Install command</h2><pre><code>{}</code></pre>",
                        escape(command)
                    ));
                }
                let stats = detail_stats(main);
                for (key, label) in [
                    ("installs_display", "Installs"),
                    ("first_seen", "First seen"),
                ] {
                    if let Some(value) = stats.get(key).and_then(Value::as_str) {
                        body.push_str(&format!("<p>{label}: {}</p>", escape(value)));
                    }
                }
                body.push_str(&format!("<h2>SKILL.md (rendered)</h2>{readme}"));
                (
                    body,
                    json!({"platform":"skills.sh", "page_type":"skill", "source":source,"slug":slug,
                    "source_type":if source.contains('/') {"github"} else {"well-known"},
                    "repository_url":repository_url,"summary":summary,"install_command":command,
                    "readme_complete":complete,"content_kind":"rendered_skill","statistics":stats}),
                    !complete,
                )
            }
            Route::Repository | Route::Leaderboard => {
                let mut seen = HashSet::new();
                let initial_props = flight.object_with("initialSkills");
                let initial = initial_props
                    .and_then(|o| o.get("initialSkills"))
                    .and_then(Value::as_array);
                let view = initial_props
                    .and_then(|props| props.get("view"))
                    .and_then(Value::as_str)
                    .unwrap_or(match url.path().trim_matches('/') {
                        "hot" => "hot",
                        "trending" => "trending",
                        "" => "all-time",
                        _ => "repository",
                    });
                let mut skills = Vec::new();
                let mut body = format!("<h1>{} — {}</h1><ol>", escape(&title), escape(view));
                for link in main.select(&sel("a[href]")) {
                    let Some(href) = link.value().attr("href") else {
                        continue;
                    };
                    let Ok(target) = url.join(href) else {
                        continue;
                    };
                    let Some(Route::Detail { source, slug }) = route(&target) else {
                        continue;
                    };
                    if !seen.insert(target.to_string()) {
                        continue;
                    }
                    let name = link
                        .select(&sel("h2, h3"))
                        .next()
                        .map(text)
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| slug.clone());
                    let metric_display = link
                        .select(&sel("span"))
                        .last()
                        .map(text)
                        .filter(|v| !v.is_empty());
                    let record = initial.and_then(|rows| {
                        rows.iter()
                            .find(|row| row["source"] == source && row["skillId"] == slug)
                    });
                    let installs = record
                        .and_then(|row| row.get("installs"))
                        .and_then(Value::as_u64);
                    let change = record
                        .and_then(|row| row.get("change"))
                        .and_then(Value::as_i64);
                    let installs_yesterday = record
                        .and_then(|row| row.get("installsYesterday"))
                        .and_then(Value::as_u64);
                    body.push_str(&format!(
                        "<li><a href=\"{}\">{}</a> — {}{}{}</li>",
                        escape(target.as_str()),
                        escape(&name),
                        escape(&source),
                        installs
                            .map(|n| format!(" — {n} installs"))
                            .or_else(|| metric_display
                                .as_ref()
                                .map(|s| format!(" — {}", escape(s))))
                            .unwrap_or_default(),
                        change
                            .map(|value| format!(" (change {value:+})"))
                            .unwrap_or_default()
                    ));
                    skills.push(json!({"source":source,"slug":slug,"name":name,"url":target.as_str(),"installs":installs,"metric_display":metric_display,"change":change,"installs_yesterday":installs_yesterday}));
                }
                body.push_str("</ol>");
                if skills.is_empty() {
                    return Err(failure(
                        "No skill entries found; page may be empty or its layout has changed",
                    ));
                }
                let leaderboard = page_route == Route::Leaderboard;
                if leaderboard {
                    warnings.push(DiagnosticWarning::new("skills_list_snapshot", "Only skills embedded in this page response are included; additional client-side pages are not fetched."));
                }
                (
                    body,
                    json!({"platform":"skills.sh","page_type":if leaderboard {"leaderboard"} else {"repository"},"view":view,
                    "scope":"page_snapshot","count":skills.len(),"skills":skills}),
                    false,
                )
            }
        };
        finish_document(
            html,
            page_url,
            options,
            ParsedPage {
                title,
                body,
                entity,
                warnings,
                incomplete,
            },
        )
    }
}

struct ParsedPage {
    title: String,
    body: String,
    entity: Value,
    warnings: Vec<DiagnosticWarning>,
    incomplete: bool,
}

fn finish_document(
    html: &str,
    page_url: &str,
    options: &ExtractionOptions,
    page: ParsedPage,
) -> Result<PageDocument, TklessError> {
    let ParsedPage {
        title,
        body,
        entity,
        warnings,
        incomplete,
    } = page;
    let doc = Html::parse_document(html);
    let scoped = format!("<main>{body}</main>");
    let scoped = if let Some(remove) = &options.remove_selector {
        crate::cleaner::prune_unwanted_selectors(&scoped, remove)?
    } else {
        scoped
    };
    let cleaned = crate::cleaner::extract_selector(&scoped, "main", Some(page_url))?;
    let mut content = match options.format {
        OutputFormat::Markdown | OutputFormat::Json => crate::markdown::MarkdownConverter::new()
            .convert(&cleaned.html, options.strip_images, options.strip_links)?,
        OutputFormat::Text => crate::html_to_plain_text(&cleaned.html),
        OutputFormat::Html | OutputFormat::Raw => cleaned.html.clone(),
    };
    let metadata = if options.include_metadata {
        let mut metadata = crate::extract_metadata(&doc);
        if metadata.canonical_url.is_none() {
            metadata.canonical_url = Some(page_url.into());
        }
        metadata
    } else {
        Default::default()
    };
    if options.frontmatter && options.format == OutputFormat::Markdown {
        content = crate::prepend_frontmatter(&content, Some(&title), Some(page_url), &metadata);
    }
    let mut truncated = incomplete;
    if let Some(limit) = options.max_tokens {
        let (limited, was_truncated) = truncate_to_tokens(&content, limit, options.tokenizer);
        content = limited;
        truncated |= was_truncated;
    }
    let cleaned_doc = Html::parse_document(&cleaned.html);
    Ok(PageDocument {
        extraction: crate::ExtractionProvenance {
            method: "platform_adapter".into(),
            transport: "http".into(),
            adapter: Some("skills".into()),
        },
        warnings,
        url: page_url.into(),
        final_url: page_url.into(),
        status: 200,
        title: Some(title),
        byline: None,
        token_count: count_tokens(&content, options.tokenizer),
        tokenizer: options.tokenizer.as_str().into(),
        content,
        truncated,
        metadata,
        json_ld: if options.include_json_ld {
            crate::extract_json_ld(&doc)
        } else {
            Vec::new()
        },
        links: options
            .extract_links
            .then(|| crate::extract_links(&cleaned_doc, Some(page_url))),
        images: options
            .extract_images
            .then(|| crate::extract_images(&cleaned_doc, Some(page_url))),
        tables: (options.extract_tables || options.tables_only)
            .then(|| crate::extract_tables(&cleaned_doc)),
        entity: Some(entity),
        subtitles: None,
        danmaku: None,
        comments: None,
        conclusion: None,
        chunks: None,
        content_hash: None,
        metrics: None,
    })
}

fn detail_stats(main: ElementRef<'_>) -> Value {
    let mut stats = serde_json::Map::new();
    for section in main.select(&sel("div.bg-background")) {
        let nodes: Vec<_> = section.children().filter_map(ElementRef::wrap).collect();
        if nodes.len() < 2 {
            continue;
        }
        let key = text(nodes[0]);
        let field = match key.as_str() {
            "Installs" | "Total Installs" => "installs_display",
            "First Seen" => "first_seen",
            _ => continue,
        };
        stats.insert(field.into(), Value::String(text(nodes[1])));
    }
    Value::Object(stats)
}

impl PlatformAdapter for SkillsAdapter {
    fn name(&self) -> &'static str {
        "skills"
    }
    fn matches(&self, url: &Url) -> bool {
        route(url).is_some()
    }
    fn allow_html_fallback(&self, url: &Url) -> bool {
        !matches!(route(url), Some(Route::Search(_) | Route::Leaderboard))
    }
    fn extract<'a>(
        &'a self,
        fetcher: &'a Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, TklessError>> {
        Box::pin(async move {
            options.validate()?;
            if options.selector.is_none()
                && matches!(route(url), Some(Route::Search(_) | Route::Leaderboard))
            {
                return catalog::extract(fetcher, url, options).await;
            }
            let response = fetcher.fetch(url.as_str()).await?;
            if !(200..300).contains(&response.status) {
                return Err(TklessError::HttpStatusError {
                    status: response.status,
                    url: response.final_url,
                });
            }
            let mut doc = Self::parse_html(&response.html, &response.final_url, options)?;
            doc.url = response.requested_url;
            doc.status = response.status;
            doc.truncated |= response.truncated;
            Ok(doc)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_text_records_use_utf8_byte_lengths_and_do_not_parse_body_as_records() {
        let body = "你好\nb:{\"previewHtml\":\"not a component\"}";
        let stream = format!(
            "1:I[42]\na:T{:x},{body}\nc:{{\"restHtml\":\"$a\"}}\n",
            body.len()
        );
        let data = FlightData::parse(&stream);
        assert_eq!(data.resolve(&json!("$a")).as_deref(), Some(body));
        assert!(data.object_with("previewHtml").is_none());
        assert!(data.object_with("restHtml").is_some());
    }

    #[test]
    fn incomplete_or_invalid_unicode_text_records_never_claim_a_complete_body() {
        for stream in ["a:T20,short", "a:T1,你好", "a:Txyz,body"] {
            assert!(FlightData::parse(stream).resolve(&json!("$a")).is_none());
        }
        assert_eq!(
            FlightData::parse("a:\"plain body\"\n")
                .resolve(&json!("$a"))
                .as_deref(),
            Some("plain body")
        );
        assert_eq!(
            FlightData::default()
                .resolve(&json!("$$literal"))
                .as_deref(),
            Some("$literal")
        );
    }
}
