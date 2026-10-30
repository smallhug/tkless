//! Shared, typed outcomes for bounded collection workflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Exhausted,
    PageErrors,
    PageError,
    ParseError,
    PageLimit,
    ItemLimit,
    TimeBudget,
    OutputBudget,
    QueueBudget,
    SitemapLimit,
    OriginBoundary,
    RepeatedUrl,
    RepeatedPage,
    EmptyPage,
}
impl StopReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exhausted => "exhausted",
            Self::PageErrors => "page_errors",
            Self::PageError => "page_error",
            Self::ParseError => "parse_error",
            Self::PageLimit => "page_limit",
            Self::ItemLimit => "item_limit",
            Self::TimeBudget => "time_budget",
            Self::OutputBudget => "output_budget",
            Self::QueueBudget => "queue_budget",
            Self::SitemapLimit => "sitemap_limit",
            Self::OriginBoundary => "origin_boundary",
            Self::RepeatedUrl => "repeated_url",
            Self::RepeatedPage => "repeated_page",
            Self::EmptyPage => "empty_page",
        }
    }
}
impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncEventKind {
    Added,
    Updated,
    Removed,
    Unchanged,
    Error,
}
impl SyncEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Updated => "updated",
            Self::Removed => "removed",
            Self::Unchanged => "unchanged",
            Self::Error => "error",
        }
    }
}
impl std::fmt::Display for SyncEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_are_stable_and_unknown_states_are_rejected() {
        assert_eq!(
            serde_json::to_string(&StopReason::OutputBudget).unwrap(),
            "\"output_budget\""
        );
        assert_eq!(
            serde_json::from_str::<SyncEventKind>("\"removed\"").unwrap(),
            SyncEventKind::Removed
        );
        assert!(serde_json::from_str::<StopReason>("\"typo\"").is_err());
        assert!(serde_json::from_str::<SyncEventKind>("\"typo\"").is_err());
    }
}
