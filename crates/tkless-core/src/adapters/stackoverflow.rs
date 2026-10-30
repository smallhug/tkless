//! Stack Overflow and Stack Exchange network Q&A content adapter.
//!
//! Intercepts `stackoverflow.com/questions/...`, `*.stackexchange.com/questions/...`,
//! `superuser.com/questions/...`, `serverfault.com/questions/...`, and `askubuntu.com/questions/...` URLs,
//! extracting question title, tags, question body, accepted answer, and top community answers.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static SO_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#question-header h1 a.question-hyperlink, h1.question-hyperlink, h1[itemprop='name'] a, h1").unwrap()
});

static SO_QUESTION_BODY_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#question .js-post-body, #question .post-text, .question .js-post-body")
        .unwrap()
});

static SO_TAGS_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("ul.post-taglist a.post-tag, a.post-tag").unwrap());

static SO_QUESTION_SCORE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#question .js-vote-count, #question [itemprop='upvoteCount']").unwrap()
});

static SO_QUESTION_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "#question .post-signature.owner .user-details a, #question .user-info .user-details a",
    )
    .unwrap()
});

static SO_ANSWER_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("#answers .answer, .answer").unwrap());

static SO_ANSWER_BODY_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".js-post-body, .post-text").unwrap());

static SO_ANSWER_SCORE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".js-vote-count, [itemprop='upvoteCount']").unwrap());

static SO_ANSWER_AUTHOR_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".user-info .user-details a").unwrap());

static SO_ACCEPTED_INDICATOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".js-accepted-answer-indicator:not(.d-none), [itemprop='acceptedAnswer']")
        .unwrap()
});

pub struct StackoverflowAdapter;

impl Default for StackoverflowAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl StackoverflowAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Stack Overflow adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 TkLess/0.1",
            )
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            )
            .send()
            .await?;

        let html = fetcher.read_text(resp).await?;
        self.parse_page(&html, url, options)
    }

    pub fn parse_page(
        &self,
        raw_html: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Question Title
        let title = doc
            .select(&SO_TITLE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "Stack Overflow Question".to_string());

        // 2. Extract Question Tags
        let mut tags = Vec::new();
        for el in doc.select(&SO_TAGS_SELECTOR) {
            let t = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
            if !t.is_empty() && !tags.contains(&t) {
                tags.push(t);
            }
        }

        // 3. Extract Question Score and Author
        let q_score = doc
            .select(&SO_QUESTION_SCORE_SELECTOR)
            .next()
            .and_then(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .parse::<i64>()
                    .ok()
            })
            .unwrap_or(0);

        let q_author = doc
            .select(&SO_QUESTION_AUTHOR_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "Anonymous".to_string());

        // 4. Extract Question Body
        let converter = MarkdownConverter::new();
        let q_body_html = doc
            .select(&SO_QUESTION_BODY_SELECTOR)
            .next()
            .map(|el| el.html())
            .unwrap_or_default();
        let q_body_md =
            converter.convert(&q_body_html, options.strip_images, options.strip_links)?;

        // 5. Extract Answers
        let mut answers_output = Vec::new();
        let mut answers_json = Vec::new();

        for answer_el in doc.select(&SO_ANSWER_SELECTOR).take(10) {
            let is_accepted = answer_el.value().classes().any(|c| c == "accepted-answer")
                || answer_el.select(&SO_ACCEPTED_INDICATOR).next().is_some();

            let a_score = answer_el
                .select(&SO_ANSWER_SCORE_SELECTOR)
                .next()
                .and_then(|el| {
                    el.text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .parse::<i64>()
                        .ok()
                })
                .unwrap_or(0);

            let a_author = answer_el
                .select(&SO_ANSWER_AUTHOR_SELECTOR)
                .next()
                .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .unwrap_or_else(|| "Community Member".to_string());

            let a_body_html = answer_el
                .select(&SO_ANSWER_BODY_SELECTOR)
                .next()
                .map(|el| el.html())
                .unwrap_or_default();

            if a_body_html.trim().is_empty() {
                continue;
            }

            let a_body_md =
                converter.convert(&a_body_html, options.strip_images, options.strip_links)?;

            answers_json.push(serde_json::json!({
                "author": a_author,
                "score": a_score,
                "is_accepted": is_accepted,
                "body": a_body_md
            }));

            let badge = if is_accepted {
                "**采纳答案 (Accepted Answer)**"
            } else {
                "**社区回答**"
            };

            answers_output.push(format!(
                "### {badge} | 得分: {a_score} | 回答者: `{a_author}`\n\n{}\n",
                a_body_md.trim()
            ));
        }

        // 6. Generate Content according to format
        let content = match options.format {
            OutputFormat::Json => {
                let json_obj = serde_json::json!({
                    "title": title,
                    "url": url.as_str(),
                    "tags": tags,
                    "question": {
                        "author": q_author,
                        "score": q_score,
                        "body": q_body_md
                    },
                    "answers": answers_json
                });
                serde_json::to_string_pretty(&json_obj).unwrap_or_default()
            }
            _ => {
                let mut md = String::new();
                if options.frontmatter {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("url: \"{url}\"\n"));
                    md.push_str(&format!("author: \"{q_author}\"\n"));
                    md.push_str(&format!("score: {q_score}\n"));
                    if !tags.is_empty() {
                        md.push_str(&format!(
                            "tags: [{}]\n",
                            tags.iter()
                                .map(|t| format!("\"{}\"", t.replace('"', "\\\"")))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {title}\n\n"));
                md.push_str(&format!("> **提问者**: `{q_author}` | **得分**: {q_score}"));
                if !tags.is_empty() {
                    md.push_str(&format!(" | **标签**: `{}`", tags.join("`, `")));
                }
                md.push_str("\n\n## 问题描述\n\n");
                md.push_str(q_body_md.trim());
                md.push_str("\n\n---\n\n## 解决方案与回答 (Answers)\n\n");

                if answers_output.is_empty() {
                    md.push_str("> *[当前暂无社区回答]*\n\n");
                } else {
                    for ans in answers_output {
                        md.push_str(&ans);
                        md.push_str("\n---\n\n");
                    }
                }

                md
            }
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
            byline: Some(format!("Asked by {q_author}")),
            content,
            metadata: PageMetadata {
                author: Some(q_author),
                canonical_url: Some(url.to_string()),
                keywords: tags,
                ..Default::default()
            },
            json_ld: Vec::new(),
            token_count,
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }
}

impl PlatformAdapter for StackoverflowAdapter {
    fn name(&self) -> &'static str {
        "stackoverflow"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        let path = url.path();

        let is_so_host = crate::fetcher::is_subdomain_or_exact(host, "stackoverflow.com")
            || crate::fetcher::is_subdomain_or_exact(host, "stackexchange.com")
            || crate::fetcher::is_subdomain_or_exact(host, "superuser.com")
            || crate::fetcher::is_subdomain_or_exact(host, "serverfault.com")
            || crate::fetcher::is_subdomain_or_exact(host, "askubuntu.com");

        is_so_host && (path.starts_with("/questions/") || path.starts_with("/q/"))
    }

    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, TklessError>> {
        Box::pin(self.do_extract(fetcher, url, options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stackoverflow_adapter_matching() {
        let adapter = StackoverflowAdapter::new();
        let u1 =
            Url::parse("https://stackoverflow.com/questions/21747136/how-to-read-a-file-in-rust")
                .unwrap();
        let u2 = Url::parse("https://superuser.com/questions/12345/command-line-tool").unwrap();
        let u3 = Url::parse("https://serverfault.com/questions/999/nginx-config").unwrap();
        let u4 = Url::parse("https://askubuntu.com/questions/555/install-package").unwrap();
        let u5 = Url::parse("https://unix.stackexchange.com/questions/4321/bash-script").unwrap();
        let u6 = Url::parse("https://evilstackoverflow.com/questions/123").unwrap();
        let u7 = Url::parse("https://stackoverflow.com/tags").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(adapter.matches(&u3));
        assert!(adapter.matches(&u4));
        assert!(adapter.matches(&u5));
        assert!(!adapter.matches(&u6));
        assert!(!adapter.matches(&u7));
    }

    #[test]
    fn test_stackoverflow_parse_page() {
        let adapter = StackoverflowAdapter::new();
        let sample_html = r##"
            <!DOCTYPE html>
            <html>
            <head><title>How to read a file in Rust? - Stack Overflow</title></head>
            <body>
                <div id="question-header">
                    <h1><a href="/questions/21747136" class="question-hyperlink">How to read a file into a string in Rust?</a></h1>
                </div>
                <div id="question">
                    <div class="js-vote-count">142</div>
                    <ul class="post-taglist">
                        <li><a class="post-tag">rust</a></li>
                        <li><a class="post-tag">io</a></li>
                    </ul>
                    <div class="user-info">
                        <div class="user-details"><a href="#">coder123</a></div>
                    </div>
                    <div class="js-post-body">
                        <p>I want to read the entire contents of a file into a String in Rust.</p>
                        <pre><code>let contents = fs::read_to_string("foo.txt")?;</code></pre>
                    </div>
                </div>
                <div id="answers">
                    <div class="answer accepted-answer" id="answer-1">
                        <div class="js-vote-count">256</div>
                        <span class="js-accepted-answer-indicator"></span>
                        <div class="user-info">
                            <div class="user-details"><a href="#">rust_expert</a></div>
                        </div>
                        <div class="js-post-body">
                            <p>You can use <code>std::fs::read_to_string</code> which is standard since Rust 1.26.</p>
                            <pre><code class="language-rust">use std::fs;

fn main() -> std::io::Result<()> {
    let data = fs::read_to_string("file.txt")?;
    println!("{}", data);
    Ok(())
}</code></pre>
                        </div>
                    </div>
                </div>
            </body>
            </html>
        "##;

        let url =
            Url::parse("https://stackoverflow.com/questions/21747136/how-to-read-a-file-in-rust")
                .unwrap();
        let doc = adapter
            .parse_page(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(
            doc.title.as_deref(),
            Some("How to read a file into a string in Rust?")
        );
        assert!(doc.content.contains("coder123"));
        assert!(doc.content.contains("rust_expert"));
        assert!(doc.content.contains("采纳答案 (Accepted Answer)"));
        assert!(doc.content.contains("std::fs::read_to_string"));
        assert!(doc.content.contains("`rust`"));
        assert!(doc.content.contains("`io`"));
    }
}
