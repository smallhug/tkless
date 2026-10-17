//! Content and Markdown change detection and unified diff module.
//!
//! Computes line-level differences between two document versions without external dependencies,
//! providing change metrics (added, removed, unchanged lines, change ratio) and unified diffs
//! for AI agents tracking documentation drift, news updates, or release changes over time.

use serde::{Deserialize, Serialize};

/// Detailed change detection and unified diff report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffReport {
    pub added_lines: usize,
    pub removed_lines: usize,
    pub unchanged_lines: usize,
    pub change_ratio: f64,
    pub unified_diff: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditOp {
    Equal(usize, usize),
    Insert(usize),
    Delete(usize),
}

/// Computes line-level diff between `old_text` and `new_text`.
pub fn compute_diff(old_text: &str, new_text: &str) -> DiffReport {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    if old_lines.is_empty() && new_lines.is_empty() {
        return DiffReport {
            added_lines: 0,
            removed_lines: 0,
            unchanged_lines: 0,
            change_ratio: 0.0,
            unified_diff: String::new(),
        };
    }

    let script = compute_lcs_script(&old_lines, &new_lines);

    let mut added = 0;
    let mut removed = 0;
    let mut unchanged = 0;
    let mut diff_output = String::new();

    diff_output.push_str("--- a/original\n+++ b/updated\n");

    for op in &script {
        match op {
            EditOp::Equal(old_idx, _) => {
                unchanged += 1;
                diff_output.push(' ');
                diff_output.push_str(old_lines[*old_idx]);
                diff_output.push('\n');
            }
            EditOp::Insert(new_idx) => {
                added += 1;
                diff_output.push('+');
                diff_output.push_str(new_lines[*new_idx]);
                diff_output.push('\n');
            }
            EditOp::Delete(old_idx) => {
                removed += 1;
                diff_output.push('-');
                diff_output.push_str(old_lines[*old_idx]);
                diff_output.push('\n');
            }
        }
    }

    let total_lines = (old_lines.len() + new_lines.len()).max(1) as f64;
    let change_ratio = (added + removed) as f64 / total_lines;

    DiffReport {
        added_lines: added,
        removed_lines: removed,
        unchanged_lines: unchanged,
        change_ratio: (change_ratio * 100.0).round() / 100.0,
        unified_diff: diff_output.trim_end().to_string(),
    }
}

/// Computes edit script using Longest Common Subsequence (LCS).
fn compute_lcs_script(old_lines: &[&str], new_lines: &[&str]) -> Vec<EditOp> {
    let n = old_lines.len();
    let m = new_lines.len();

    // Fast-path: identical
    if old_lines == new_lines {
        return (0..n).map(|i| EditOp::Equal(i, i)).collect();
    }

    // Dynamic programming matrix for LCS
    let mut dp = vec![vec![0usize; m + 1]; n + 1];

    for i in 0..n {
        for j in 0..m {
            if old_lines[i] == new_lines[j] {
                dp[i + 1][j + 1] = dp[i][j] + 1;
            } else {
                dp[i + 1][j + 1] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    // Backtrack to build the edit script
    let mut script = Vec::new();
    let mut i = n;
    let mut j = m;

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            script.push(EditOp::Equal(i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            script.push(EditOp::Insert(j - 1));
            j -= 1;
        } else if i > 0 && (j == 0 || dp[i][j - 1] < dp[i - 1][j]) {
            script.push(EditOp::Delete(i - 1));
            i -= 1;
        }
    }

    script.reverse();
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_diff_basic() {
        let old = "Alpha\nBeta\nGamma";
        let new = "Alpha\nBeta 2.0\nGamma\nDelta";

        let report = compute_diff(old, new);
        assert_eq!(report.added_lines, 2);
        assert_eq!(report.removed_lines, 1);
        assert_eq!(report.unchanged_lines, 2);
        assert!(report.unified_diff.contains("-Beta"));
        assert!(report.unified_diff.contains("+Beta 2.0"));
        assert!(report.unified_diff.contains("+Delta"));
        assert!(report.change_ratio > 0.0);
    }

    #[test]
    fn test_compute_diff_identical() {
        let text = "Line 1\nLine 2";
        let report = compute_diff(text, text);
        assert_eq!(report.added_lines, 0);
        assert_eq!(report.removed_lines, 0);
        assert_eq!(report.unchanged_lines, 2);
        assert_eq!(report.change_ratio, 0.0);
    }
}
