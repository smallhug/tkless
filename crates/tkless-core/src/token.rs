use crate::model::TokenizerKind;

#[cfg(feature = "tiktoken")]
static O200K_BPE: std::sync::LazyLock<Option<tiktoken_rs::CoreBPE>> =
    std::sync::LazyLock::new(|| tiktoken_rs::o200k_base().ok());

#[cfg(feature = "tiktoken")]
static CL100K_BPE: std::sync::LazyLock<Option<tiktoken_rs::CoreBPE>> =
    std::sync::LazyLock::new(|| tiktoken_rs::cl100k_base().ok());

/// Counts tokens in the given text using the selected tokenizer encoding.
pub fn count_tokens(text: &str, kind: TokenizerKind) -> usize {
    match kind {
        TokenizerKind::Heuristic => estimate_tokens_heuristic(text),
        TokenizerKind::Cl100k => {
            #[cfg(feature = "tiktoken")]
            {
                if let Some(ref bpe) = *CL100K_BPE {
                    return bpe.encode_with_special_tokens(text).len();
                }
            }
            estimate_tokens_heuristic(text)
        }
        TokenizerKind::O200k => {
            #[cfg(feature = "tiktoken")]
            {
                if let Some(ref bpe) = *O200K_BPE {
                    return bpe.encode_with_special_tokens(text).len();
                }
            }
            estimate_tokens_heuristic(text)
        }
    }
}

/// Fallback token estimator: accounts for ASCII / English words vs CJK multibyte characters.
pub fn estimate_tokens_heuristic(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut tokens = 0;
    let mut in_word = false;

    for c in text.chars() {
        if c.is_ascii_whitespace() {
            in_word = false;
        } else if c.is_ascii() {
            if !in_word {
                in_word = true;
                tokens += 1;
            } else if c.is_ascii_punctuation() {
                tokens += 1;
            }
        } else {
            // CJK characters or non-ASCII emojis typically count as ~1 token each
            tokens += 1;
            in_word = false;
        }
    }

    tokens.max(1)
}

/// Truncates content to fit within `max_tokens`, ensuring:
/// 1. Cuts cleanly at paragraph or sentence boundaries.
/// 2. Closes unclosed Markdown code blocks (```).
/// 3. Appends a clear agent truncation notice.
pub fn truncate_to_tokens(text: &str, max_tokens: usize, kind: TokenizerKind) -> (String, bool) {
    let current_tokens = count_tokens(text, kind);
    if current_tokens <= max_tokens {
        return (text.to_string(), false);
    }

    if max_tokens == 0 {
        return (String::new(), true);
    }

    const NOTICE: &str = "\n\n[Content truncated by TkLess: token limit reached]";
    // Very small budgets cannot fit a notice; callers still have the truncated flag.
    let notice = if count_tokens(NOTICE, kind) <= max_tokens {
        NOTICE
    } else {
        ""
    };
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut low = 0;
    let mut high = boundaries.len() - 1;
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate = finish_truncated(&text[..boundaries[mid]], notice);
        if count_tokens(&candidate, kind) <= max_tokens {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let cut_byte_idx = boundaries[low];
    let slice = &text[..cut_byte_idx];

    // Find the best boundary: paragraph break (\n\n) > sentence end > newline > whitespace
    let best_cut = if let Some(p) = slice.rfind("\n\n") {
        if p > cut_byte_idx / 2 {
            p
        } else {
            find_sentence_or_line_boundary(slice, cut_byte_idx)
        }
    } else {
        find_sentence_or_line_boundary(slice, cut_byte_idx)
    };

    let mut cut = best_cut;
    loop {
        let truncated = finish_truncated(&text[..cut], notice);
        // BPE token counts are not strictly monotonic across prefix boundaries.
        if count_tokens(&truncated, kind) <= max_tokens {
            return (truncated, true);
        }
        cut = text[..cut]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }
}

fn finish_truncated(prefix: &str, notice: &str) -> String {
    let mut result = prefix.trim_end().to_string();
    let mut open: Option<(char, usize)> = None;
    for line in result.lines() {
        if line.starts_with('\t') || line.len() - line.trim_start_matches(' ').len() > 3 {
            continue;
        }
        let line = line.trim_start();
        let marker = line.chars().next().unwrap_or(' ');
        if marker != '`' && marker != '~' {
            continue;
        }
        let length = line.chars().take_while(|c| *c == marker).count();
        if length < 3 {
            continue;
        }
        match open {
            Some((c, n)) if c == marker && length >= n && line[length..].trim().is_empty() => {
                open = None
            }
            None => open = Some((marker, length)),
            _ => {}
        }
    }
    if let Some((marker, length)) = open {
        result.push('\n');
        result.extend(std::iter::repeat_n(marker, length));
    }
    result.push_str(notice);
    result
}

fn find_sentence_or_line_boundary(slice: &str, fallback_idx: usize) -> usize {
    // Sentence end punctuation: ". ", ".\n", "! ", "? ", "。\n", "。"
    let sentence_ends = [".\n", ".\r\n", "。\n", "。", "! ", "? ", "\n"];
    for sep in &sentence_ends {
        if let Some(pos) = slice.rfind(sep) {
            if pos > fallback_idx / 3 {
                return pos + sep.len();
            }
        }
    }
    fallback_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens() {
        let text = "Hello world! This is a test.";
        let count_o200k = count_tokens(text, TokenizerKind::O200k);
        let count_heuristic = count_tokens(text, TokenizerKind::Heuristic);
        assert!(count_o200k > 0 && count_o200k < 20);
        assert!(count_heuristic > 0 && count_heuristic < 20);
    }

    #[test]
    fn test_truncate_to_tokens_code_fence() {
        let text = "Here is some code:\n\n```rust\nfn main() {\n    println!(\"Hello\");\n}\n```\n\nMore paragraphs follow here.";
        let (truncated, is_truncated) = truncate_to_tokens(text, 10, TokenizerKind::default());

        assert!(is_truncated);
        assert!(count_tokens(&truncated, TokenizerKind::default()) <= 10);

        // If a code block was started, it must be closed
        let fence_count = truncated.matches("```").count();
        assert_eq!(fence_count % 2, 0);
    }

    #[test]
    fn strict_budget_includes_notices_and_fences() {
        for kind in [
            TokenizerKind::O200k,
            TokenizerKind::Cl100k,
            TokenizerKind::Heuristic,
        ] {
            for source in [
                "中文测试与 emoji 🦀 repeated words ".repeat(20),
                "Intro\n\n````rust\nlet text = \"```\";\n".repeat(10),
                "~~~text\ncode line\n".repeat(20),
            ] {
                for limit in 0..80 {
                    let (out, _) = truncate_to_tokens(&source, limit, kind);
                    assert!(
                        count_tokens(&out, kind) <= limit,
                        "{kind:?}, {limit}: {out}"
                    );
                }
            }
        }
    }
}
