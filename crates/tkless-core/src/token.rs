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

    // Rough target char count based on current token density
    let ratio = max_tokens as f64 / current_tokens as f64;
    let target_char_count = ((text.chars().count() as f64) * ratio * 0.95) as usize;

    let char_indices: Vec<(usize, char)> = text.char_indices().collect();
    if target_char_count >= char_indices.len() {
        return (text.to_string(), false);
    }

    let cut_byte_idx = char_indices[target_char_count].0;
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

    let mut truncated = text[..best_cut].trim_end().to_string();

    // Check for unclosed Markdown code fences (```)
    let fence_count = truncated.matches("```").count();
    if !fence_count.is_multiple_of(2) {
        truncated.push_str("\n```");
    }

    truncated.push_str("\n\n[Content truncated by TkLess: token limit reached]");

    (truncated, true)
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
        assert!(truncated.contains("[Content truncated by TkLess"));

        // If a code block was started, it must be closed
        let fence_count = truncated.matches("```").count();
        assert_eq!(fence_count % 2, 0);
    }
}
