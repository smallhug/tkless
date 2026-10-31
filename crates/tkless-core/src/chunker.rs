//! Semantic Markdown Chunking for RAG and Vector Database Ingestion.
//!
//! Breaks cleaned Markdown documents into cohesive semantic chunks based on heading hierarchies (`#`, `##`, `###`),
//! guaranteeing that code fences (```) and Markdown tables are never split in half, while maintaining
//! breadcrumb heading paths for LLM contextual grounding.

use crate::model::{DocumentChunk, TokenizerKind};
use crate::token::count_tokens;

/// A parsed atomic block of Markdown (heading, paragraph, code fence, or table).
#[derive(Debug, Clone)]
struct MarkdownBlock {
    heading_level: Option<usize>,
    heading_text: Option<String>,
    content: String,
    is_heading: bool,
}

/// Segments a Markdown document into semantic chunks bounded by token limits.
pub fn chunk_markdown(
    markdown: &str,
    target_chunk_tokens: usize,
    overlap_tokens: usize,
    tokenizer: TokenizerKind,
) -> Vec<DocumentChunk> {
    let trimmed = markdown.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if target_chunk_tokens == 0 {
        return Vec::new();
    }
    let target_tokens = target_chunk_tokens;
    let overlap = overlap_tokens.min(target_tokens / 2);

    let blocks = parse_markdown_blocks(trimmed);
    if blocks.is_empty() {
        return Vec::new();
    }

    let mut chunks: Vec<DocumentChunk> = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();

    let mut current_blocks: Vec<String> = Vec::new();
    let mut current_tokens: usize = 0;
    let mut current_heading_path: Vec<String> = Vec::new();
    let mut current_title: Option<String> = None;

    for block in blocks {
        let block_tokens = count_tokens(&block.content, tokenizer);

        // Check if adding this block exceeds target token budget, or if entering a new major section
        let should_split = (current_tokens + block_tokens > target_tokens
            || (block.is_heading && current_tokens >= target_tokens / 2))
            && !current_blocks.is_empty();

        if should_split {
            // Emit current chunk with its accurate heading path
            let chunk_content = current_blocks.join("\n\n");
            let chunk_tokens = count_tokens(&chunk_content, tokenizer);

            chunks.push(DocumentChunk {
                index: chunks.len(),
                heading_path: current_heading_path.clone(),
                title: current_title.clone(),
                content: chunk_content,
                token_count: chunk_tokens,
            });

            // Handle overlap: retain trailing block(s) if within overlap budget (unless splitting at a new heading)
            let mut retained_blocks = Vec::new();
            let mut retained_tokens = 0;

            if overlap > 0 && !block.is_heading {
                for b in current_blocks.iter().rev() {
                    let b_tokens = count_tokens(b, tokenizer);
                    if retained_tokens + b_tokens <= overlap {
                        retained_blocks.push(b.clone());
                        retained_tokens += b_tokens;
                    } else {
                        break;
                    }
                }
                retained_blocks.reverse();
            }

            current_blocks = retained_blocks;
            current_tokens = retained_tokens;
        }

        if block.is_heading {
            let level = block.heading_level.unwrap_or(1);
            let text = block.heading_text.clone().unwrap_or_default();

            // Pop any heading of equal or deeper level
            while let Some((last_level, _)) = heading_stack.last() {
                if *last_level >= level {
                    heading_stack.pop();
                } else {
                    break;
                }
            }
            heading_stack.push((level, text));

            current_heading_path = heading_stack.iter().map(|(_, t)| t.clone()).collect();
            current_title = heading_stack.last().map(|(_, t)| t.clone());
        }

        current_blocks.push(block.content);
        current_tokens += block_tokens;
    }

    // Flush remaining chunk
    if !current_blocks.is_empty() {
        let chunk_content = current_blocks.join("\n\n");
        let chunk_tokens = count_tokens(&chunk_content, tokenizer);

        chunks.push(DocumentChunk {
            index: chunks.len(),
            heading_path: current_heading_path,
            title: current_title,
            content: chunk_content,
            token_count: chunk_tokens,
        });
    }

    chunks
}

/// Parses Markdown text into discrete atomic blocks (preserving code blocks and tables).
fn parse_markdown_blocks(markdown: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut in_code_fence = false;
    let mut in_table = false;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in markdown.lines() {
        let trimmed_line = line.trim();

        // 1. Code fence boundary check
        if trimmed_line.starts_with("```") || trimmed_line.starts_with("~~~") {
            if in_code_fence {
                // Closing code fence
                current_lines.push(line);
                let content = current_lines.join("\n");
                blocks.push(MarkdownBlock {
                    heading_level: None,
                    heading_text: None,
                    content,
                    is_heading: false,
                });
                current_lines.clear();
                in_code_fence = false;
                continue;
            } else {
                // Opening code fence: flush any preceding paragraph
                if !current_lines.is_empty() {
                    let content = current_lines.join("\n");
                    blocks.push(MarkdownBlock {
                        heading_level: None,
                        heading_text: None,
                        content,
                        is_heading: false,
                    });
                    current_lines.clear();
                }
                in_code_fence = true;
                current_lines.push(line);
                continue;
            }
        }

        if in_code_fence {
            current_lines.push(line);
            continue;
        }

        // 2. Table boundary check
        let is_table_row = trimmed_line.starts_with('|') && trimmed_line.ends_with('|');
        if is_table_row {
            if !in_table {
                // Flush preceding text before table
                if !current_lines.is_empty() {
                    let content = current_lines.join("\n");
                    blocks.push(MarkdownBlock {
                        heading_level: None,
                        heading_text: None,
                        content,
                        is_heading: false,
                    });
                    current_lines.clear();
                }
                in_table = true;
            }
            current_lines.push(line);
            continue;
        } else if in_table {
            // End of table
            let content = current_lines.join("\n");
            blocks.push(MarkdownBlock {
                heading_level: None,
                heading_text: None,
                content,
                is_heading: false,
            });
            current_lines.clear();
            in_table = false;
        }

        // 3. Heading check
        if trimmed_line.starts_with('#') {
            if let Some((level, text)) = parse_heading_line(trimmed_line) {
                // Flush preceding text
                if !current_lines.is_empty() {
                    let content = current_lines.join("\n");
                    blocks.push(MarkdownBlock {
                        heading_level: None,
                        heading_text: None,
                        content,
                        is_heading: false,
                    });
                    current_lines.clear();
                }

                blocks.push(MarkdownBlock {
                    heading_level: Some(level),
                    heading_text: Some(text),
                    content: line.to_string(),
                    is_heading: true,
                });
                continue;
            }
        }

        // 4. Blank line paragraph boundary
        if trimmed_line.is_empty() {
            if !current_lines.is_empty() {
                let content = current_lines.join("\n");
                blocks.push(MarkdownBlock {
                    heading_level: None,
                    heading_text: None,
                    content,
                    is_heading: false,
                });
                current_lines.clear();
            }
            continue;
        }

        current_lines.push(line);
    }

    // Flush any trailing lines
    if !current_lines.is_empty() {
        let content = current_lines.join("\n");
        blocks.push(MarkdownBlock {
            heading_level: None,
            heading_text: None,
            content,
            is_heading: false,
        });
    }

    blocks
}

/// Parses a heading line into (level, title_text).
fn parse_heading_line(line: &str) -> Option<(usize, String)> {
    let mut level = 0;
    for c in line.chars() {
        if c == '#' {
            level += 1;
        } else {
            break;
        }
    }
    if (1..=6).contains(&level) {
        let text = line[level..].trim().to_string();
        Some((level, text))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_markdown_heading_hierarchy() {
        let md = r#"# Main Guide

Introduction paragraph for the main guide.

## Section 1: Setup

Details about setting up the project and configuration.

### Subsection 1.1: Installation

Run `cargo install tkless` to install the tool.

```bash
cargo build --release
tkless --version
```

## Section 2: Usage

Here is how you use TkLess for extraction.

| Command | Description |
|---|---|
| search | Web search |
| crawl | Site crawl |
"#;

        let chunks = chunk_markdown(md, 50, 10, TokenizerKind::Heuristic);
        assert!(!chunks.is_empty());

        // Verify heading paths are captured
        let has_subsection = chunks.iter().any(|c| {
            c.heading_path.contains(&"Main Guide".to_string())
                && c.heading_path.contains(&"Section 1: Setup".to_string())
        });
        assert!(has_subsection, "Should track heading breadcrumb hierarchy");

        // Verify code block is not split
        let has_code = chunks.iter().any(|c| {
            c.content
                .contains("```bash\ncargo build --release\ntkless --version\n```")
        });
        assert!(has_code, "Code block must remain intact within a chunk");

        // Verify table is not split
        let has_table = chunks.iter().any(|c| {
            c.content.contains("| Command | Description |")
                && c.content.contains("| search | Web search |")
        });
        assert!(has_table, "Table must remain intact within a chunk");
    }

    #[test]
    fn test_chunk_markdown_empty_or_small() {
        let chunks = chunk_markdown("", 500, 50, TokenizerKind::Heuristic);
        assert!(chunks.is_empty());

        let short_md = "# Hello\n\nShort text.";
        let chunks = chunk_markdown(short_md, 500, 50, TokenizerKind::Heuristic);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].title.as_deref(), Some("Hello"));
        assert_eq!(chunks[0].heading_path, vec!["Hello"]);
    }
}
