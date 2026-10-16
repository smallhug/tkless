use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::PageclawError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static ZHIHU_MATH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"<span[^>]*class="[^"]*ztext-math[^"]*"[^>]*data-tex="([^"]+)"[^>]*>.*?</span>"#)
        .unwrap()
});

static ZHIHU_IMG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"<img[^>]*data-actualsrc="([^"]+)"[^>]*>"#).unwrap()
});

static ZHIHU_INITIAL_DATA_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("script#js-initialData").unwrap());

/// Route types supported by ZhihuAdapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZhihuRoute {
    Question { question_id: String },
    Answer { question_id: String, answer_id: String },
    Article { article_id: String },
}

pub struct ZhihuAdapter;

impl Default for ZhihuAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZhihuAdapter {
    pub fn new() -> Self {
        Self
    }

    pub fn parse_route(url: &Url) -> Option<ZhihuRoute> {
        let host = url.host_str().unwrap_or("");
        if !host.ends_with("zhihu.com") {
            return None;
        }

        let path = url.path().trim_end_matches('/');

        // 1. Column article: zhuanlan.zhihu.com/p/{article_id}
        if host == "zhuanlan.zhihu.com" {
            if let Some(rest) = path.strip_prefix("/p/") {
                let id = rest.split('/').next()?;
                if !id.is_empty() {
                    return Some(ZhihuRoute::Article {
                        article_id: id.to_string(),
                    });
                }
            }
        }

        // 2. Specific question answer: /question/{qid}/answer/{aid}
        if let Some(pos) = path.find("/question/") {
            let after_q = &path[pos + 10..];
            let mut parts = after_q.split('/');
            let qid = parts.next()?;
            if let Some(next_part) = parts.next() {
                if next_part == "answer" {
                    if let Some(aid) = parts.next() {
                        if !aid.is_empty() {
                            return Some(ZhihuRoute::Answer {
                                question_id: qid.to_string(),
                                answer_id: aid.to_string(),
                            });
                        }
                    }
                }
            }
            // 3. Question overall: /question/{qid}
            if !qid.is_empty() {
                return Some(ZhihuRoute::Question {
                    question_id: qid.to_string(),
                });
            }
        }

        None
    }

    async fn do_extract(
        &self,
        client: &reqwest::Client,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let route = Self::parse_route(url).ok_or_else(|| PageclawError::AdapterError {
            platform: "zhihu",
            message: format!("Unsupported Zhihu URL: {url}"),
        })?;

        debug!(route = ?route, "Executing Zhihu adapter extraction");

        match route {
            ZhihuRoute::Article { article_id } => {
                self.extract_article(client, &article_id, url, options).await
            }
            ZhihuRoute::Answer {
                question_id,
                answer_id,
            } => {
                self.extract_single_answer(client, &question_id, &answer_id, url, options).await
            }
            ZhihuRoute::Question { question_id } => {
                self.extract_question_top(client, &question_id, url, options).await
            }
        }
    }

    /// Extracts a Zhihu Zhuanlan column article.
    async fn extract_article(
        &self,
        client: &reqwest::Client,
        article_id: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let target_url = format!("https://zhuanlan.zhihu.com/p/{article_id}");
        let html = fetch_zhihu_html(client, &target_url).await?;

        let doc = Html::parse_document(&html);

        // Try extracting from #js-initialData first
        if let Some(initial_json) = extract_initial_data(&doc) {
            if let Some(articles) = initial_json
                .get("initialState")
                .and_then(|s| s.get("entities"))
                .and_then(|e| e.get("articles"))
            {
                if let Some(art) = articles.get(article_id) {
                    let title = art.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let author = art
                        .get("author")
                        .and_then(|a| a.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let raw_content = art.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let cleaned_html = preprocess_zhihu_html(raw_content);
                    let voteup_count = art.get("voteupCount").and_then(|v| v.as_u64()).unwrap_or(0);
                    let created = art.get("created").and_then(|v| v.as_i64()).unwrap_or(0);
                    let pub_date = format_timestamp_date(created);

                    return self.build_article_doc(&title, &author, &cleaned_html, voteup_count, pub_date, url, options);
                }
            }
        }

        // Fallback: DOM parsing
        let title_selector = Selector::parse("h1.Post-Title").unwrap();
        let title = doc
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "知乎专栏".to_string());

        let author_selector = Selector::parse(".AuthorInfo-name").unwrap();
        let author = doc
            .select(&author_selector)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        let content_selector = Selector::parse(".Post-RichText").unwrap();
        let raw_content = doc
            .select(&content_selector)
            .next()
            .map(|el| el.html())
            .unwrap_or_default();

        let cleaned_html = preprocess_zhihu_html(&raw_content);
        self.build_article_doc(&title, &author, &cleaned_html, 0, None, url, options)
    }

    /// Extracts a specific question answer.
    async fn extract_single_answer(
        &self,
        client: &reqwest::Client,
        question_id: &str,
        answer_id: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let api_url = format!(
            "https://www.zhihu.com/api/v4/answers/{answer_id}?include=content%2Cvoteup_count%2Cauthor%2Cquestion"
        );

        let resp = client
            .get(&api_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .header("Referer", format!("https://www.zhihu.com/question/{question_id}"))
            .send()
            .await;

        if let Ok(res) = resp {
            if let Ok(ans) = res.json::<ZhihuAnswerItem>().await {
                let q_title = ans.question.map(|q| q.title).unwrap_or_else(|| "知乎问答".into());
                let author = ans.author.map(|a| a.name).unwrap_or_else(|| "知乎用户".into());
                let cleaned_html = preprocess_zhihu_html(&ans.content);

                return self.build_single_answer_doc(&q_title, &author, &cleaned_html, ans.voteup_count, url, options);
            }
        }

        // Fallback: Fetch question page HTML
        let target_url = format!("https://www.zhihu.com/question/{question_id}/answer/{answer_id}");
        let html = fetch_zhihu_html(client, &target_url).await?;
        let doc = Html::parse_document(&html);

        if let Some(initial_json) = extract_initial_data(&doc) {
            let title = initial_json
                .get("initialState")
                .and_then(|s| s.get("entities"))
                .and_then(|e| e.get("questions"))
                .and_then(|q| q.get(question_id))
                .and_then(|q| q.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("知乎问答")
                .to_string();

            if let Some(answers) = initial_json
                .get("initialState")
                .and_then(|s| s.get("entities"))
                .and_then(|e| e.get("answers"))
            {
                if let Some(ans) = answers.get(answer_id) {
                    let author = ans
                        .get("author")
                        .and_then(|a| a.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("知乎用户")
                        .to_string();
                    let raw_content = ans.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let voteup_count = ans.get("voteupCount").and_then(|v| v.as_u64()).unwrap_or(0);
                    let cleaned_html = preprocess_zhihu_html(raw_content);

                    return self.build_single_answer_doc(&title, &author, &cleaned_html, voteup_count, url, options);
                }
            }
        }

        Err(PageclawError::AdapterError {
            platform: "zhihu",
            message: "Failed to extract Zhihu answer content".to_string(),
        })
    }

    /// Extracts the top N answers for a question.
    async fn extract_question_top(
        &self,
        client: &reqwest::Client,
        question_id: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let limit = 5;
        let api_url = format!(
            "https://www.zhihu.com/api/v4/questions/{question_id}/answers?include=content%2Cvoteup_count%2Cauthor&offset=0&limit={limit}&sort_by=default"
        );

        let resp = client
            .get(&api_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .header("Referer", format!("https://www.zhihu.com/question/{question_id}"))
            .send()
            .await;

        if let Ok(res) = resp {
            if let Ok(data) = res.json::<ZhihuAnswersResponse>().await {
                if !data.data.is_empty() {
                    let q_title = data
                        .data
                        .first()
                        .and_then(|a| a.question.as_ref())
                        .map(|q| q.title.clone())
                        .unwrap_or_else(|| "知乎问答".to_string());

                    return self.build_question_answers_doc(&q_title, data.data, url, options);
                }
            }
        }

        // Fallback: Fetch question page and parse #js-initialData
        let target_url = format!("https://www.zhihu.com/question/{question_id}");
        let html = fetch_zhihu_html(client, &target_url).await?;
        let doc = Html::parse_document(&html);

        if let Some(initial_json) = extract_initial_data(&doc) {
            let title = initial_json
                .get("initialState")
                .and_then(|s| s.get("entities"))
                .and_then(|e| e.get("questions"))
                .and_then(|q| q.get(question_id))
                .and_then(|q| q.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("知乎问答")
                .to_string();

            if let Some(answers_map) = initial_json
                .get("initialState")
                .and_then(|s| s.get("entities"))
                .and_then(|e| e.get("answers"))
                .and_then(|v| v.as_object())
            {
                let mut answers = Vec::new();
                for (_, ans_val) in answers_map {
                    if let Ok(item) = serde_json::from_value::<ZhihuAnswerItem>(ans_val.clone()) {
                        answers.push(item);
                    }
                }
                answers.sort_by_key(|a| std::cmp::Reverse(a.voteup_count));
                answers.truncate(limit);

                if !answers.is_empty() {
                    return self.build_question_answers_doc(&title, answers, url, options);
                }
            }
        }

        Err(PageclawError::AdapterError {
            platform: "zhihu",
            message: "Failed to extract Zhihu question answers".to_string(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_article_doc(
        &self,
        title: &str,
        author: &str,
        cleaned_html: &str,
        voteup: u64,
        pub_date: Option<String>,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let converter = MarkdownConverter::new();
        let md_body = converter.convert(cleaned_html, options.strip_images, options.strip_links)?;

        let mut content = String::new();
        if options.frontmatter && options.format == OutputFormat::Markdown {
            content.push_str("---\n");
            content.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
            content.push_str(&format!("author: \"{}\"\n", author.replace('"', "\\\"")));
            content.push_str(&format!("url: \"{url}\"\n"));
            if let Some(ref p) = pub_date {
                content.push_str(&format!("published: \"{p}\"\n"));
            }
            content.push_str(&format!("votes: {voteup}\n"));
            content.push_str("---\n\n");
        }

        content.push_str(&format!("# {}\n\n", title));
        content.push_str(&format!("> **作者**: {} | **赞同**: {}\n\n", author, format_count(voteup)));
        content.push_str(&md_body);

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title.to_string()),
            byline: Some(author.to_string()),
            content,
            metadata: PageMetadata {
                author: Some(author.to_string()),
                published_time: pub_date,
                canonical_url: Some(url.to_string()),
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
        })
    }

    fn build_single_answer_doc(
        &self,
        question_title: &str,
        author: &str,
        cleaned_html: &str,
        voteup: u64,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let converter = MarkdownConverter::new();
        let md_body = converter.convert(cleaned_html, options.strip_images, options.strip_links)?;

        let mut content = String::new();
        content.push_str(&format!("# {}\n\n", question_title));
        content.push_str(&format!("> **回答作者**: {} | **赞同数**: {}\n\n", author, format_count(voteup)));
        content.push_str(&md_body);

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(question_title.to_string()),
            byline: Some(author.to_string()),
            content,
            metadata: PageMetadata {
                author: Some(author.to_string()),
                canonical_url: Some(url.to_string()),
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
        })
    }

    fn build_question_answers_doc(
        &self,
        question_title: &str,
        answers: Vec<ZhihuAnswerItem>,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let converter = MarkdownConverter::new();
        let mut content = String::new();

        content.push_str(&format!("# {}\n\n", question_title));
        content.push_str(&format!("> 本问题精选了前 {} 个高赞回答：\n\n", answers.len()));

        for (idx, ans) in answers.iter().enumerate() {
            let author = ans.author.as_ref().map(|a| a.name.as_str()).unwrap_or("知乎用户");
            let cleaned = preprocess_zhihu_html(&ans.content);
            let md = converter.convert(&cleaned, options.strip_images, options.strip_links)?;

            content.push_str(&format!(
                "## 回答 #{} by {} (赞同: {})\n\n",
                idx + 1,
                author,
                format_count(ans.voteup_count)
            ));
            content.push_str(&md);
            content.push_str("\n\n---\n\n");
        }

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(question_title.to_string()),
            byline: Some("知乎".to_string()),
            content,
            metadata: PageMetadata {
                canonical_url: Some(url.to_string()),
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
        })
    }
}

impl PlatformAdapter for ZhihuAdapter {
    fn name(&self) -> &'static str {
        "zhihu"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        host.ends_with("zhihu.com")
    }

    fn extract<'a>(
        &'a self,
        client: &'a reqwest::Client,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, PageclawError>> {
        Box::pin(self.do_extract(client, url, options))
    }
}

/// Preprocesses Zhihu HTML content:
/// 1. Converts `<span class="ztext-math" data-tex="...">` to `$$...$$` LaTeX syntax
/// 2. Promotes `data-actualsrc` or `data-original` to `src`
pub fn preprocess_zhihu_html(raw_html: &str) -> String {
    // 1. Math formulas
    let with_math = ZHIHU_MATH_RE.replace_all(raw_html, |caps: &regex::Captures| {
        let tex = caps.get(1).map_or("", |m| m.as_str());
        format!(" $${tex}$$ ")
    });

    // 2. Images
    let with_images = ZHIHU_IMG_RE.replace_all(&with_math, |caps: &regex::Captures| {
        let src = caps.get(1).map_or("", |m| m.as_str());
        format!("<img src=\"{src}\" />")
    });

    with_images.to_string()
}

async fn fetch_zhihu_html(client: &reqwest::Client, target_url: &str) -> Result<String, PageclawError> {
    let resp = client
        .get(target_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header("Referer", "https://www.zhihu.com/")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .send()
        .await?;

    Ok(resp.text().await?)
}

fn extract_initial_data(doc: &Html) -> Option<serde_json::Value> {
    let script_el = doc.select(&ZHIHU_INITIAL_DATA_SELECTOR).next()?;
    let text = script_el.text().collect::<Vec<_>>().join("");
    serde_json::from_str(&text).ok()
}

fn format_timestamp_date(ts: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn format_count(count: u64) -> String {
    if count >= 10_000 {
        format!("{:.1}万", count as f64 / 10_000.0)
    } else {
        count.to_string()
    }
}

// -----------------------------------------------------------------------------
// Upstream Zhihu API Payloads
// -----------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct ZhihuAnswersResponse {
    #[serde(default)]
    data: Vec<ZhihuAnswerItem>,
}

#[derive(Deserialize, Debug)]
pub struct ZhihuAnswerItem {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub voteup_count: u64,
    #[serde(default)]
    pub author: Option<ZhihuAuthor>,
    #[serde(default)]
    pub question: Option<ZhihuQuestionSnippet>,
}

#[derive(Deserialize, Debug)]
pub struct ZhihuAuthor {
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize, Debug)]
pub struct ZhihuQuestionSnippet {
    #[serde(default)]
    pub title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zhihu_route_matching() {
        let u1 = Url::parse("https://www.zhihu.com/question/123456").unwrap();
        let u2 = Url::parse("https://www.zhihu.com/question/123456/answer/78910").unwrap();
        let u3 = Url::parse("https://zhuanlan.zhihu.com/p/654321").unwrap();

        assert_eq!(
            ZhihuAdapter::parse_route(&u1),
            Some(ZhihuRoute::Question {
                question_id: "123456".into()
            })
        );
        assert_eq!(
            ZhihuAdapter::parse_route(&u2),
            Some(ZhihuRoute::Answer {
                question_id: "123456".into(),
                answer_id: "78910".into()
            })
        );
        assert_eq!(
            ZhihuAdapter::parse_route(&u3),
            Some(ZhihuRoute::Article {
                article_id: "654321".into()
            })
        );
    }

    #[test]
    fn test_zhihu_math_and_image_preprocessing() {
        let raw = r#"
            <p>欧拉恒等式为：<span class="ztext-math" data-tex="e^{i\pi} + 1 = 0">math</span>很神奇。</p>
            <p><img class="origin_image" src="data:image/svg+xml;..." data-actualsrc="https://pic1.zhimg.com/v2-abc.jpg" /></p>
        "#;

        let processed = preprocess_zhihu_html(raw);
        assert!(processed.contains("$$e^{i\\pi} + 1 = 0$$"));
        assert!(processed.contains("https://pic1.zhimg.com/v2-abc.jpg"));
        assert!(!processed.contains("data:image/svg+xml"));
    }
}
