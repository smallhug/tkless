//! Content deduplication and SimHash near-duplicate detection.
//!
//! Calculates 64-bit SimHash fingerprints for web documents using word and character n-grams.
//! Enables crawlers and batch pipelines to identify near-duplicate pages (e.g. syndicated articles,
//! mirrors, repetitive boilerplates) and calculate Hamming distances.

use std::collections::HashMap;

/// Calculates a 64-bit SimHash fingerprint for arbitrary text content.
pub fn simhash(text: &str) -> u64 {
    let features = extract_features(text);
    if features.is_empty() {
        return 0;
    }

    let mut bit_counts = [0i32; 64];

    for (feature, weight) in features {
        let hash = fnv1a_64(feature.as_bytes());
        for i in 0..64 {
            let bit = (hash >> i) & 1;
            if bit == 1 {
                bit_counts[i] += weight as i32;
            } else {
                bit_counts[i] -= weight as i32;
            }
        }
    }

    let mut result = 0u64;
    for (i, count) in bit_counts.iter().enumerate() {
        if *count > 0 {
            result |= 1 << i;
        }
    }

    result
}

/// Computes the Hamming distance (number of differing bits) between two 64-bit SimHash values.
pub fn hamming_distance(h1: u64, h2: u64) -> u32 {
    (h1 ^ h2).count_ones()
}

/// Calculates the similarity ratio between two 64-bit SimHash values (0.0 to 1.0).
pub fn similarity_ratio(h1: u64, h2: u64) -> f64 {
    let dist = hamming_distance(h1, h2);
    1.0 - (dist as f64 / 64.0)
}

/// Determines whether two documents are near-duplicates given a maximum Hamming distance threshold.
/// Standard threshold is 3 (corresponding to ~95% content similarity).
pub fn is_near_duplicate(h1: u64, h2: u64, max_distance: u32) -> bool {
    hamming_distance(h1, h2) <= max_distance
}

/// 64-bit FNV-1a non-cryptographic hash function (fast, zero external dependencies).
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Extracts features with frequency weights:
/// - Words (for Latin / alphanumeric languages) with elevated weight
/// - CJK character 2-grams (for non-ASCII CJK texts lacking space delimiters)
fn extract_features(text: &str) -> HashMap<String, usize> {
    let mut features = HashMap::new();
    let lower = text.to_lowercase();

    // 1. Word tokens (for words of length >= 2)
    for word in lower.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let trimmed = word.trim();
        if trimmed.len() >= 2 {
            *features.entry(trimmed.to_string()).or_insert(0) += 2;
        }
    }

    // 2. CJK character 2-grams (specifically for CJK text lacking space delimiters)
    let cjk_chars: Vec<char> = lower
        .chars()
        .filter(|c| !c.is_ascii() && !c.is_whitespace() && !c.is_ascii_punctuation())
        .collect();
    if cjk_chars.len() >= 2 {
        for window in cjk_chars.windows(2) {
            let gram: String = window.iter().collect();
            *features.entry(gram).or_insert(0) += 1;
        }
    }

    features
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simhash_identical_content() {
        let text1 = "PageClaw is a token-efficient web content extraction engine for AI agents.";
        let text2 = "PageClaw is a token-efficient web content extraction engine for AI agents.";

        let h1 = simhash(text1);
        let h2 = simhash(text2);

        assert_eq!(h1, h2);
        assert_eq!(hamming_distance(h1, h2), 0);
        assert_eq!(similarity_ratio(h1, h2), 1.0);
        assert!(is_near_duplicate(h1, h2, 3));
    }

    #[test]
    fn test_simhash_near_duplicate_content() {
        let text1 = "PageClaw is a token-efficient web content extraction engine for AI agents with fast Rust speed.";
        let text2 = "PageClaw is a token-efficient web extraction engine for AI agents with fast Rust speed and low RAM.";

        let h1 = simhash(text1);
        let h2 = simhash(text2);

        let dist = hamming_distance(h1, h2);
        assert!(
            dist <= 12,
            "Near duplicate documents should have small Hamming distance: {dist}"
        );
        assert!(similarity_ratio(h1, h2) >= 0.80);

        // Test on realistic article paragraphs
        let article1 = r#"
            PageClaw is a high-performance, token-efficient web content extraction engine designed specifically
            for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and
            boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown.
            Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead.
        "#;
        let article2 = r#"
            PageClaw is a high-performance, token-efficient web content extraction engine designed specifically
            for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and
            boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown.
            Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with low memory.
        "#;
        let art_h1 = simhash(article1);
        let art_h2 = simhash(article2);
        let art_dist = hamming_distance(art_h1, art_h2);
        assert!(
            art_dist <= 3,
            "Real article near-duplicates must have Hamming distance <= 3: {art_dist}"
        );
        assert!(is_near_duplicate(art_h1, art_h2, 3));
    }

    #[test]
    fn test_simhash_completely_different_content() {
        let text1 = "The weather in Tokyo is sunny today with mild spring temperatures.";
        let text2 =
            "Kubernetes deployment configuration with replica sets and ingress controllers.";

        let h1 = simhash(text1);
        let h2 = simhash(text2);

        let dist = hamming_distance(h1, h2);
        assert!(
            dist >= 20,
            "Completely different documents should have large Hamming distance: {dist}"
        );
        assert!(!is_near_duplicate(h1, h2, 3));
    }

    #[test]
    fn test_simhash_cjk_support() {
        let text1 =
            "PageClaw 爪页是一个专为大语言模型和 AI Agent 设计的高性能网页抓取与降噪清洗工具。";
        let text2 =
            "PageClaw 爪页是一个专为大语言模型与 AI Agent 设计的高性能网页抓取降噪清洗工具。";

        let h1 = simhash(text1);
        let h2 = simhash(text2);

        let dist = hamming_distance(h1, h2);
        assert!(
            dist <= 12,
            "Near identical Chinese texts should have small distance: {dist}"
        );
        assert!(similarity_ratio(h1, h2) >= 0.80);

        // Test on realistic Chinese article paragraphs
        let cjk_article1 = r#"
            PageClaw（爪页）是一个专为大语言模型与 AI Agent 设计的高性能网页抓取、降噪清洗与搜索提取引擎。
            自动剥离导航、脚本与广告噪音，将正文转化为 LLM 友好的 Markdown 或结构化 JSON，支持段落断句平滑截断与 Token 预算控制。
            原生集成 B站、知乎、微信公众号、小红书、掘金与 GitHub 平台适配器，内置 Bing 和 Google 双引擎检索。
        "#;
        let cjk_article2 = r#"
            PageClaw（爪页）是一个专为大语言模型与 AI Agent 设计的高性能网页抓取、降噪清洗与搜索提取引擎。
            自动剥离导航、脚本与广告噪音，将正文转化为 LLM 友好的 Markdown 或结构化 JSON，支持段落断句平滑截断与 Token 预算控制。
            原生集成 B站、知乎、微信公众号、小红书、掘金与 GitHub 平台适配器，内置 Bing 和 Google 双引擎搜索。
        "#;
        let cjk_h1 = simhash(cjk_article1);
        let cjk_h2 = simhash(cjk_article2);
        let cjk_dist = hamming_distance(cjk_h1, cjk_h2);
        assert!(
            cjk_dist <= 3,
            "Real Chinese article near-duplicates must have Hamming distance <= 3: {cjk_dist}"
        );
        assert!(is_near_duplicate(cjk_h1, cjk_h2, 3));
    }
}
