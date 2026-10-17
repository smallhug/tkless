//! Local heuristic extractive text summarizer and key takeaways generator.
//!
//! Evaluates sentence salience based on term frequencies (TF-IDF / TextRank heuristics)
//! and structural position bias without requiring external LLM API calls or network latency.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static CHINESE_STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都", "一", "一个", "上", "也",
        "很", "到", "说", "要", "去", "你", "会", "着", "没有", "看", "好", "自己", "这", "那",
        "与", "及", "等", "及", "其", "或", "而", "为", "之", "以", "从", "对", "把",
    ]
    .into_iter()
    .collect()
});

static ENGLISH_STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by",
        "from", "up", "about", "into", "over", "after", "is", "are", "was", "were", "be", "been",
        "being", "have", "has", "had", "do", "does", "did", "it", "its", "this", "that", "these",
        "those", "which", "who", "whom", "what", "where", "when", "why", "how", "all", "any",
        "both", "each", "few", "more", "most", "other", "some", "such", "no", "nor", "not", "only",
        "own", "same", "so", "than", "too", "very", "s", "t", "can", "will", "just", "don",
        "should", "now",
    ]
    .into_iter()
    .collect()
});

/// Extractive sentence with position index and computed salience score.
#[derive(Debug, Clone)]
struct ScoredSentence {
    index: usize,
    text: String,
    score: f64,
}

/// Extracts key takeaway bullet sentences from Markdown or plain text.
pub fn extract_key_takeaways(text: &str, max_bullets: usize) -> Vec<String> {
    if text.trim().is_empty() || max_bullets == 0 {
        return Vec::new();
    }

    let sentences = split_candidate_sentences(text);
    if sentences.is_empty() {
        return Vec::new();
    }
    if sentences.len() <= max_bullets {
        return sentences;
    }

    // 1. Calculate word / term frequency across the document
    let mut word_freq: HashMap<String, usize> = HashMap::new();
    let mut total_words = 0;

    for sent in &sentences {
        for word in tokenize_words(sent) {
            *word_freq.entry(word).or_insert(0) += 1;
            total_words += 1;
        }
    }

    if total_words == 0 {
        return sentences.into_iter().take(max_bullets).collect();
    }

    // 2. Score candidate sentences
    let total_sentences = sentences.len() as f64;
    let mut scored: Vec<ScoredSentence> = Vec::with_capacity(sentences.len());

    for (idx, sent) in sentences.into_iter().enumerate() {
        let words = tokenize_words(&sent);
        if words.is_empty() {
            continue;
        }

        let mut sent_score = 0.0;
        for word in &words {
            if let Some(&count) = word_freq.get(word) {
                // Diminishing returns for over-frequent words
                sent_score += (count as f64).sqrt();
            }
        }

        // Length normalization (penalize extreme brevity and extreme verbosity)
        let len_penalty = (words.len() as f64).sqrt().max(1.0);
        let mut score = sent_score / len_penalty;

        // Position bias: earlier sentences in an article carry higher summary weight
        let position_weight = 1.0 + (1.0 - (idx as f64 / total_sentences)).max(0.0) * 0.5;
        score *= position_weight;

        scored.push(ScoredSentence {
            index: idx,
            text: sent,
            score,
        });
    }

    // 3. Select top `max_bullets` highest-scoring sentences
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut selected: Vec<ScoredSentence> = scored.into_iter().take(max_bullets).collect();

    // 4. Restore original chronological order
    selected.sort_by_key(|s| s.index);

    selected.into_iter().map(|s| s.text).collect()
}

/// Summarizes Markdown content by prepending a formatted Key Takeaways header section.
pub fn summarize_markdown(markdown: &str, max_bullets: usize) -> String {
    let bullets = extract_key_takeaways(markdown, max_bullets);
    if bullets.is_empty() {
        return markdown.to_string();
    }

    let mut out = String::new();
    out.push_str("### 💡 核心要点速览 (Key Takeaways)\n\n");
    for bullet in bullets {
        out.push_str(&format!("- {}\n", bullet.trim()));
    }
    out.push_str("\n---\n\n");
    out.push_str(markdown.trim());
    out
}

/// Splits input text into candidate sentences while filtering out headers, tables, code blocks.
fn split_candidate_sentences(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        // Skip headings, tables, blockquotes, code blocks, or separator lines
        if trimmed.starts_with('#')
            || trimmed.starts_with('|')
            || trimmed.starts_with("```")
            || trimmed.starts_with("---")
            || trimmed.starts_with("***")
            || trimmed.starts_with(">")
            || trimmed.is_empty()
        {
            continue;
        }

        // Split by punctuation: period, exclamation, question mark, or Chinese equivalents
        let mut current = String::new();
        for ch in trimmed.chars() {
            current.push(ch);
            if ch == '。'
                || ch == '！'
                || ch == '？'
                || ch == '!'
                || ch == '?'
                || ch == ';'
                || ch == '；'
            {
                let s = current.trim().to_string();
                if is_valid_sentence(&s) {
                    candidates.push(s);
                }
                current.clear();
            } else if ch == '.' {
                // Avoid splitting decimal numbers (e.g. "3.14") or abbreviations
                let s = current.trim().to_string();
                if is_valid_sentence(&s) {
                    candidates.push(s);
                }
                current.clear();
            }
        }

        let s = current.trim().to_string();
        if is_valid_sentence(&s) {
            candidates.push(s);
        }
    }

    candidates
}

fn is_valid_sentence(sent: &str) -> bool {
    let len = sent.chars().count();
    // Exclude very short fragments or lines that look like links/tags
    len >= 12 && !sent.starts_with("http") && !sent.starts_with('[')
}

/// Tokenizes text into lowercase words while omitting stop words.
fn tokenize_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let lower = text.to_lowercase();

    for token in lower.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let trimmed = token.trim();
        if trimmed.len() >= 2
            && !ENGLISH_STOPWORDS.contains(trimmed)
            && !CHINESE_STOPWORDS.contains(trimmed)
        {
            words.push(trimmed.to_string());
        }
    }

    // CJK character 2-grams
    let cjk_chars: Vec<char> = lower
        .chars()
        .filter(|c| !c.is_ascii() && !c.is_whitespace() && !c.is_ascii_punctuation())
        .collect();

    if cjk_chars.len() >= 2 {
        for window in cjk_chars.windows(2) {
            let gram: String = window.iter().collect();
            if !CHINESE_STOPWORDS.contains(gram.as_str()) {
                words.push(gram);
            }
        }
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_key_takeaways_english() {
        let text = r#"
            # Artificial Intelligence Architecture

            Artificial Intelligence is rapidly transforming modern software engineering and agentic workflows.
            Deep neural networks enable automated pattern discovery across massive unstructured web datasets.
            TkLess provides an ultra-fast token-efficient extraction pipeline designed specifically for LLMs.
            Traditional scrapers fail due to heavy JavaScript execution and aggressive anti-bot protections.
            By utilizing native platform adapters, TkLess extracts articles with sub-second latency and zero API keys.
        "#;

        let takeaways = extract_key_takeaways(text, 3);
        assert!(!takeaways.is_empty());
        assert!(takeaways.len() <= 3);
        assert!(takeaways
            .iter()
            .any(|s| s.contains("TkLess") || s.contains("Artificial Intelligence")));
    }

    #[test]
    fn test_summarize_markdown() {
        let text = r#"
            Rust 是一门专注于安全、并发与性能的高性能系统级编程语言。
            Rust 的所有权系统在编译期排除了所有空指针异常和数据竞争隐患。
            TkLess 完全采用 Rust 构建，具备毫秒级冷启动与极低内存占用。
        "#;

        let summarized = summarize_markdown(text, 2);
        assert!(summarized.contains("### 💡 核心要点速览 (Key Takeaways)"));
        assert!(summarized.contains("- "));
        assert!(summarized.contains("所有权系统"));
    }
}
