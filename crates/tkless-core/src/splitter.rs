//! Document heading splitter and multi-file directory exporter.
//!
//! Decomposes monolithic Markdown documents into modular sections split by heading hierarchy
//! (`#` or `##`), and optionally writes them into a structured directory with an `index.md` Table of Contents.

use crate::error::PageclawError;
use crate::token::count_tokens;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// A self-contained section of a Markdown document split by a heading.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentSection {
    pub index: usize,
    pub title: String,
    pub level: usize,
    pub slug: String,
    pub content: String,
    pub token_count: usize,
}

/// Splits a Markdown document into sections based on top-level (`#`) and second-level (`##`) headings.
pub fn split_markdown_by_headings(
    markdown: &str,
    max_heading_level: usize,
) -> Vec<DocumentSection> {
    let clean = markdown.trim();
    if clean.is_empty() {
        return Vec::new();
    }

    let level_limit = max_heading_level.clamp(1, 3);
    let mut sections = Vec::new();
    let mut current_title = String::new();
    let mut current_level = 1;
    let mut current_lines = Vec::new();
    let mut in_code_block = false;

    for line in clean.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            current_lines.push(line);
            continue;
        }

        if !in_code_block && trimmed.starts_with('#') {
            let heading_level = trimmed.chars().take_while(|&c| c == '#').count();
            if heading_level <= level_limit && trimmed.chars().nth(heading_level) == Some(' ') {
                // Flush previous section
                if !current_lines.is_empty() || !current_title.is_empty() {
                    let sec_content = current_lines.join("\n").trim().to_string();
                    let title = if current_title.is_empty() {
                        "Introduction".to_string()
                    } else {
                        current_title.clone()
                    };
                    let slug = slugify(&title);
                    let token_count =
                        count_tokens(&sec_content, crate::model::TokenizerKind::default());

                    sections.push(DocumentSection {
                        index: sections.len() + 1,
                        title,
                        level: current_level,
                        slug,
                        content: sec_content,
                        token_count,
                    });
                    current_lines.clear();
                }

                current_level = heading_level;
                current_title = trimmed[heading_level..].trim().to_string();
                current_lines.push(line);
                continue;
            }
        }

        current_lines.push(line);
    }

    // Flush final section
    if !current_lines.is_empty() || !current_title.is_empty() {
        let sec_content = current_lines.join("\n").trim().to_string();
        let title = if current_title.is_empty() {
            "Content".to_string()
        } else {
            current_title
        };
        let slug = slugify(&title);
        let token_count = count_tokens(&sec_content, crate::model::TokenizerKind::default());

        sections.push(DocumentSection {
            index: sections.len() + 1,
            title,
            level: current_level,
            slug,
            content: sec_content,
            token_count,
        });
    }

    sections
}

/// Exports an array of DocumentSections into a multi-file destination directory with an auto-generated `index.md`.
pub fn export_sections_to_dir(
    sections: &[DocumentSection],
    dir: &Path,
    doc_title: Option<&str>,
) -> Result<Vec<String>, PageclawError> {
    fs::create_dir_all(dir).map_err(PageclawError::Io)?;

    let mut written_files = Vec::new();
    let title_str = doc_title.unwrap_or("Document Index");

    // 1. Write individual section Markdown files
    for sec in sections {
        let filename = format!("{:02}-{}.md", sec.index, sec.slug);
        let file_path = dir.join(&filename);
        let mut file = File::create(&file_path).map_err(PageclawError::Io)?;
        file.write_all(sec.content.as_bytes())
            .map_err(PageclawError::Io)?;
        file.write_all(b"\n").map_err(PageclawError::Io)?;
        written_files.push(filename);
    }

    // 2. Generate and write index.md Table of Contents
    let mut index_md = String::new();
    index_md.push_str(&format!("# {}\n\n", title_str));
    index_md.push_str(&format!(
        "> 本文档已自动拆分为 {} 个独立章节文件。\n\n",
        sections.len()
    ));
    index_md.push_str("## 📑 章节目录导航 (Table of Contents)\n\n");

    for (idx, sec) in sections.iter().enumerate() {
        let filename = &written_files[idx];
        let indent = "  ".repeat(sec.level.saturating_sub(1));
        index_md.push_str(&format!(
            "{}- [{}](./{}) (Tokens: {})\n",
            indent, sec.title, filename, sec.token_count
        ));
    }

    let index_path = dir.join("index.md");
    let mut index_file = File::create(&index_path).map_err(PageclawError::Io)?;
    index_file
        .write_all(index_md.as_bytes())
        .map_err(PageclawError::Io)?;
    index_file.write_all(b"\n").map_err(PageclawError::Io)?;

    written_files.push("index.md".to_string());
    Ok(written_files)
}

fn slugify(text: &str) -> String {
    let mut clean = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            clean.push(ch.to_ascii_lowercase());
        } else if ch == ' ' || ch == '-' || ch == '_' {
            clean.push('-');
        }
    }
    let collapsed = clean
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if collapsed.is_empty() {
        "section".to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_markdown_by_headings() {
        let md = r#"
# Introduction
Welcome to this guide on Rust.

## Architecture
Rust uses ownership and borrowing.

```rust
fn main() {
    // # Not a real heading
    println!("Hello");
}
```

## Performance
Zero-cost abstractions ensure high throughput.
"#;

        let sections = split_markdown_by_headings(md, 2);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].title, "Introduction");
        assert_eq!(sections[0].level, 1);
        assert!(sections[0].content.contains("Welcome to this guide"));

        assert_eq!(sections[1].title, "Architecture");
        assert_eq!(sections[1].level, 2);
        assert!(sections[1].content.contains("println!(\"Hello\");"));

        assert_eq!(sections[2].title, "Performance");
        assert_eq!(sections[2].level, 2);
        assert!(sections[2].content.contains("Zero-cost abstractions"));
    }

    #[test]
    fn test_export_sections_to_temp_dir() {
        let md = "# Chapter 1\nContent 1\n# Chapter 2\nContent 2";
        let sections = split_markdown_by_headings(md, 1);
        let temp_dir = std::env::temp_dir().join(format!(
            "pageclaw_test_split_{}",
            std::time::SystemTime::now().elapsed().unwrap().as_nanos()
        ));

        let files = export_sections_to_dir(&sections, &temp_dir, Some("Test Book")).unwrap();
        assert_eq!(files.len(), 3); // 2 sections + index.md
        assert!(temp_dir.join("01-chapter-1.md").is_file());
        assert!(temp_dir.join("02-chapter-2.md").is_file());
        assert!(temp_dir.join("index.md").is_file());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
