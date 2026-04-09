//! Fuzzy search overlay for any list in the TUI.
//!
//! Activated by `/` in any pane. The search is a transient view filter —
//! it doesn't change underlying data. When the user selects a match,
//! the selection flows through the normal state setters (set_branch, etc).

use nucleo::Matcher;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};

use crate::component::{BottomTab, Pane};
use crate::store::{DataStore, LoadState};

/// Build the list of searchable tree node paths from the store.
/// Shared between App::search_candidates and UI rendering.
pub fn tree_candidates(store: &DataStore) -> Vec<String> {
    let mut paths = Vec::new();
    for state in store.node_children.values() {
        if let LoadState::Loaded(nodes) = state {
            for node in nodes {
                paths.push(node.path.clone());
            }
        }
    }
    paths
}

/// Which list the search is targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchTarget {
    Tree,
    Snapshots,
    Branches,
    Tags,
}

/// Active search session.
pub struct SearchState {
    /// What we're searching.
    pub target: SearchTarget,
    /// The search query string (what the user is typing).
    pub query: String,
    /// Indices into the source list that match the query, best match first.
    pub matches: Vec<usize>,
    /// Which match is currently highlighted (index into `matches`).
    pub cursor: usize,
}

impl SearchState {
    pub fn new(pane: Pane, tab: BottomTab) -> Self {
        let target = match pane {
            Pane::Sidebar => SearchTarget::Tree,
            Pane::Bottom => match tab {
                BottomTab::Snapshots => SearchTarget::Snapshots,
                BottomTab::Branches => SearchTarget::Branches,
                BottomTab::Tags => SearchTarget::Tags,
            },
            Pane::Detail => unreachable!("search not supported in Detail pane"),
        };
        Self {
            target,
            query: String::new(),
            matches: Vec::new(),
            cursor: 0,
        }
    }

    /// Update matches against a list of candidate strings.
    pub fn update_matches(&mut self, candidates: &[&str]) {
        if self.query.is_empty() {
            // Empty query = show all items
            self.matches = (0..candidates.len()).collect();
            self.cursor = 0;
            return;
        }

        let pattern = Pattern::new(
            &self.query,
            CaseMatching::Smart,
            Normalization::Smart,
            nucleo::pattern::AtomKind::Fuzzy,
        );
        let mut matcher = Matcher::new(nucleo::Config::DEFAULT);

        let mut scored: Vec<(usize, u32)> = candidates
            .iter()
            .enumerate()
            .filter_map(|(i, candidate)| {
                let mut buf = Vec::new();
                let haystack = nucleo::Utf32Str::new(candidate, &mut buf);
                pattern
                    .score(haystack, &mut matcher)
                    .map(|score| (i, score))
            })
            .collect();

        // Sort by score descending (best match first)
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        self.matches = scored.into_iter().map(|(i, _)| i).collect();

        // Clamp cursor
        if self.cursor >= self.matches.len() {
            self.cursor = 0;
        }
    }

    /// Push a character to the query and recompute.
    pub fn push_char(&mut self, c: char, candidates: &[&str]) {
        self.query.push(c);
        self.update_matches(candidates);
    }

    /// Remove last character from query and recompute.
    pub fn pop_char(&mut self, candidates: &[&str]) {
        self.query.pop();
        self.update_matches(candidates);
    }

    /// Move to next match.
    pub fn next(&mut self) {
        if !self.matches.is_empty() {
            self.cursor = (self.cursor + 1) % self.matches.len();
        }
    }

    /// Move to previous match.
    pub fn prev(&mut self) {
        if !self.matches.is_empty() {
            self.cursor = if self.cursor == 0 {
                self.matches.len() - 1
            } else {
                self.cursor - 1
            };
        }
    }

    /// Get the source-list index of the currently highlighted match.
    pub fn selected_index(&self) -> Option<usize> {
        self.matches.get(self.cursor).copied()
    }
}
