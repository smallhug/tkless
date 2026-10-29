use super::SearchResultItem;
use serde::{Deserialize, Serialize};

/// Diagnostic counts describe filtering decisions, not confidence probabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchPageDiagnostics {
    pub page: usize,
    pub transport: String,
    pub candidates: usize,
    pub invalid: usize,
    pub domain_filtered: usize,
    pub duplicates: usize,
    pub ads_filtered: usize,
    pub strict_matches: usize,
    pub relaxed_matches: usize,
    pub selected: usize,
    pub returned: usize,
    pub selection: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchReport {
    pub query: String,
    pub effective_query: String,
    pub status: SearchStatus,
    pub results: Vec<SearchResultItem>,
    pub diagnostics: Vec<SearchPageDiagnostics>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::model::DiagnosticWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchStatus {
    Results,
    NoResults,
    Filtered,
}

pub(super) struct ParsedPage {
    pub results: Vec<SearchResultItem>,
    pub diagnostics: SearchPageDiagnostics,
}

impl SearchReport {
    pub fn markdown_diagnostics(&self) -> String {
        let details = serde_json::json!({"query":self.query,"effective_query":self.effective_query,"status":self.status,"pages":self.diagnostics,"warnings":self.warnings});
        format!(
            "\n\n## Search diagnostics\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&details).expect("Search diagnostics are serializable")
        )
    }

    pub(super) fn new(query: &str) -> Self {
        Self {
            query: query.into(),
            effective_query: query.into(),
            status: SearchStatus::NoResults,
            results: Vec::new(),
            diagnostics: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub(super) fn append(
        &mut self,
        mut page: ParsedPage,
        number: usize,
        transport: &str,
        limit: usize,
    ) {
        let mut seen: std::collections::HashSet<_> = self
            .results
            .iter()
            .map(|item| super::normalize_url_for_dedup(&item.url))
            .collect();
        page.diagnostics.page = number;
        page.diagnostics.transport = transport.into();
        page.diagnostics.returned = 0;
        for item in page.results {
            if !seen.insert(super::normalize_url_for_dedup(&item.url)) {
                page.diagnostics.duplicates += 1;
            } else if self.results.len() < limit {
                self.results.push(item);
                page.diagnostics.returned += 1;
            }
        }
        self.diagnostics.push(page.diagnostics);
        self.status = if !self.results.is_empty() {
            SearchStatus::Results
        } else if self.diagnostics.iter().any(|p| p.candidates > 0) {
            SearchStatus::Filtered
        } else {
            SearchStatus::NoResults
        };
    }
}
