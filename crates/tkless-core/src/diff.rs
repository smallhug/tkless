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

/// Computes a bounded-memory Myers line diff and a standard unified patch.
pub fn compute_diff(old_text: &str, new_text: &str) -> DiffReport {
    let diff = similar::TextDiff::from_lines(old_text, new_text);
    let mut added = 0;
    let mut removed = 0;
    let mut unchanged = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal => unchanged += 1,
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
        }
    }
    let total = (old_text.lines().count() + new_text.lines().count()).max(1) as f64;
    DiffReport {
        added_lines: added,
        removed_lines: removed,
        unchanged_lines: unchanged,
        change_ratio: (((added + removed) as f64 / total) * 100.0).round() / 100.0,
        unified_diff: diff
            .unified_diff()
            .context_radius(3)
            .header("a/original", "b/updated")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_diff_basic() {
        let old = "Alpha\nBeta\nGamma\n";
        let new = "Alpha\nBeta 2.0\nGamma\nDelta\n";

        let report = compute_diff(old, new);
        assert_eq!(report.added_lines, 2);
        assert_eq!(report.removed_lines, 1);
        assert_eq!(report.unchanged_lines, 2);
        assert!(report.unified_diff.contains("-Beta"));
        assert!(report.unified_diff.contains("+Beta 2.0"));
        assert!(report.unified_diff.contains("+Delta"));
        assert!(report.unified_diff.contains("@@ -1,3 +1,4 @@"));
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
