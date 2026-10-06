//! In-memory retained terminal history indexing, marks and bounded folding.
use crate::terminal_ux::{SearchHit, find_text};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryLine {
    pub id: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryHit {
    pub line_id: u64,
    pub line_number: usize,
    pub column: usize,
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryMark {
    pub line_id: u64,
    pub timestamp_epoch_seconds: Option<u64>,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryRow {
    Line(HistoryLine),
    Folded { after_line_id: u64, hidden_lines: usize },
}

#[derive(Clone, Debug, Default)]
pub struct HistoryState {
    lines: Vec<HistoryLine>,
    marks: BTreeMap<u64, HistoryMark>,
    next_line_id: u64,
    query: String,
    hits: Vec<HistoryHit>,
    selected_hit: usize,
    truncated_lines: u64,
}

fn overlap_len(old: &[HistoryLine], new: &[String]) -> usize {
    if old.is_empty() || new.is_empty() {
        return 0;
    }

    // KMP: longest prefix of the new snapshot equal to a suffix of the old snapshot.
    let mut prefix = vec![0usize; new.len()];
    for index in 1..new.len() {
        let mut matched = prefix[index - 1];
        while matched > 0 && new[index] != new[matched] {
            matched = prefix[matched - 1];
        }
        if new[index] == new[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }

    let mut matched = 0usize;
    for line in old {
        while matched > 0 && (matched == new.len() || line.text != new[matched]) {
            matched = prefix[matched - 1];
        }
        if matched < new.len() && line.text == new[matched] {
            matched += 1;
        }
        if matched == new.len() {
            // Keep a full match only when it reaches the end of the old snapshot; otherwise
            // continue searching for the longest suffix match.
            if line.id != old.last().map(|line| line.id).unwrap_or_default() {
                matched = prefix[matched - 1];
            }
        }
    }
    matched
}

impl HistoryState {
    pub fn lines(&self) -> &[HistoryLine] {
        &self.lines
    }

    pub fn marks(&self) -> impl Iterator<Item = &HistoryMark> {
        self.marks.values()
    }

    pub fn hits(&self) -> &[HistoryHit] {
        &self.hits
    }

    pub fn selected_hit(&self) -> Option<&HistoryHit> {
        self.hits.get(self.selected_hit)
    }

    pub fn truncated_lines(&self) -> u64 {
        self.truncated_lines
    }

    pub fn update_snapshot(&mut self, snapshot: Vec<String>) {
        let overlap = overlap_len(&self.lines, &snapshot);
        let removed = self.lines.len().saturating_sub(overlap);
        if removed > 0 {
            self.truncated_lines = self.truncated_lines.saturating_add(removed as u64);
        }

        let retained_start = self.lines.len().saturating_sub(overlap);
        let mut next = self.lines[retained_start..].to_vec();
        for text in snapshot.into_iter().skip(overlap) {
            self.next_line_id = self.next_line_id.saturating_add(1).max(1);
            next.push(HistoryLine {
                id: self.next_line_id,
                text,
            });
        }
        if let Some(max_id) = next.iter().map(|line| line.id).max() {
            self.next_line_id = self.next_line_id.max(max_id);
        }
        self.lines = next;

        let retained_ids: BTreeSet<u64> = self.lines.iter().map(|line| line.id).collect();
        self.marks.retain(|id, _| retained_ids.contains(id));
        self.refresh_search();
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        let query = query.into();
        if self.query != query {
            self.query = query;
            self.selected_hit = 0;
            self.refresh_search();
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn select_next_hit(&mut self) {
        if !self.hits.is_empty() {
            self.selected_hit = (self.selected_hit + 1) % self.hits.len();
        }
    }

    pub fn select_previous_hit(&mut self) {
        if !self.hits.is_empty() {
            self.selected_hit = (self.selected_hit + self.hits.len() - 1) % self.hits.len();
        }
    }

    pub fn select_hit(&mut self, index: usize) {
        if index < self.hits.len() {
            self.selected_hit = index;
        }
    }

    pub fn mark_newest_nonempty(&mut self, timestamp_epoch_seconds: Option<u64>) -> Option<u64> {
        let line_id = self
            .lines
            .iter()
            .rev()
            .find(|line| !line.text.trim().is_empty())
            .or_else(|| self.lines.last())
            .map(|line| line.id)?;
        self.marks.entry(line_id).or_insert(HistoryMark {
            line_id,
            timestamp_epoch_seconds,
            collapsed: false,
        });
        Some(line_id)
    }

    pub fn remove_mark(&mut self, line_id: u64) {
        self.marks.remove(&line_id);
    }

    pub fn toggle_fold(&mut self, line_id: u64) -> bool {
        let Some(mark) = self.marks.get_mut(&line_id) else {
            return false;
        };
        mark.collapsed = !mark.collapsed;
        true
    }

    /// Render rows for the history inspector. A collapsed mark hides lines until the next mark.
    pub fn display_rows(&self) -> Vec<HistoryRow> {
        let mut rows = Vec::new();
        let mark_ids: Vec<u64> = self.marks.keys().copied().collect();

        let mut index = 0usize;
        while index < self.lines.len() {
            let line = self.lines[index].clone();
            rows.push(HistoryRow::Line(line.clone()));

            let collapsed = self
                .marks
                .get(&line.id)
                .is_some_and(|mark| mark.collapsed);
            if collapsed {
                let next_mark = mark_ids.iter().copied().find(|id| *id > line.id);
                let mut end = index + 1;
                while end < self.lines.len()
                    && next_mark.is_none_or(|next_id| self.lines[end].id < next_id)
                {
                    end += 1;
                }
                let hidden = end.saturating_sub(index + 1);
                if hidden > 0 {
                    rows.push(HistoryRow::Folded {
                        after_line_id: line.id,
                        hidden_lines: hidden,
                    });
                    index = end;
                    continue;
                }
            }

            index += 1;
        }

        rows
    }

    fn refresh_search(&mut self) {
        self.hits.clear();
        let query = self.query.trim();
        if query.is_empty() {
            self.selected_hit = 0;
            return;
        }

        // Search each retained logical row separately so stable line IDs survive viewport movement.
        for (index, line) in self.lines.iter().enumerate() {
            let local: Vec<SearchHit> = find_text(&line.text, query, 500 - self.hits.len());
            for hit in local {
                self.hits.push(HistoryHit {
                    line_id: line.id,
                    line_number: index + 1,
                    column: hit.column,
                    preview: hit.preview,
                });
            }
            if self.hits.len() >= 500 {
                break;
            }
        }
        if self.hits.is_empty() {
            self.selected_hit = 0;
        } else {
            self.selected_hit = self.selected_hit.min(self.hits.len() - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_handles_unicode_cjk_combining_and_emoji() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec![
            "ASCII Alpha".into(),
            "東京 network".into(),
            "Cafe\u{301} decomposed".into(),
            "status 🚀 ready".into(),
        ]);

        for query in ["alpha", "東京", "Cafe\u{301}", "🚀"] {
            state.set_query(query);
            assert_eq!(state.hits().len(), 1, "{query:?}");
        }
    }

    #[test]
    fn marks_survive_viewport_only_changes_and_appends() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["one".into(), "two".into(), "three".into()]);
        let marked = state.mark_newest_nonempty(Some(123)).unwrap();

        state.update_snapshot(vec![
            "one".into(),
            "two".into(),
            "three".into(),
            "four".into(),
        ]);
        assert!(state.marks().any(|mark| mark.line_id == marked));
    }

    #[test]
    fn truncation_preserves_retained_ids_and_drops_old_marks() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["one".into(), "two".into(), "three".into()]);
        let old = state.lines()[0].id;
        state.marks.insert(
            old,
            HistoryMark {
                line_id: old,
                timestamp_epoch_seconds: None,
                collapsed: false,
            },
        );
        let retained = state.lines()[2].id;

        state.update_snapshot(vec!["three".into(), "four".into()]);
        assert_eq!(state.lines()[0].id, retained);
        assert!(!state.marks().any(|mark| mark.line_id == old));
        assert_eq!(state.truncated_lines(), 2);
    }

    #[test]
    fn folding_is_bounded_by_next_mark() {
        let mut state = HistoryState::default();
        state.update_snapshot((1..=6).map(|n| format!("line {n}")).collect());
        let first = state.lines()[0].id;
        let second = state.lines()[4].id;
        state.marks.insert(
            first,
            HistoryMark {
                line_id: first,
                timestamp_epoch_seconds: None,
                collapsed: true,
            },
        );
        state.marks.insert(
            second,
            HistoryMark {
                line_id: second,
                timestamp_epoch_seconds: None,
                collapsed: false,
            },
        );

        let rows = state.display_rows();
        assert!(matches!(
            rows[1],
            HistoryRow::Folded {
                hidden_lines: 3,
                ..
            }
        ));
        assert!(rows.iter().any(|row| matches!(row, HistoryRow::Line(line) if line.id == second)));
    }

    #[test]
    fn large_scrollback_search_is_bounded_and_deterministic() {
        let mut state = HistoryState::default();
        let mut lines: Vec<String> = (0..50_000).map(|n| format!("line-{n:05} ordinary")).collect();
        lines[49_999] = "line-49999 NEEDLE".into();
        state.update_snapshot(lines);
        state.set_query("needle");
        assert_eq!(state.hits().len(), 1);
        assert_eq!(state.hits()[0].line_number, 50_000);
    }

    #[test]
    fn hit_navigation_wraps_both_directions() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["hit".into(), "none".into(), "hit".into()]);
        state.set_query("hit");
        assert_eq!(state.selected_hit().unwrap().line_number, 1);
        state.select_previous_hit();
        assert_eq!(state.selected_hit().unwrap().line_number, 3);
        state.select_next_hit();
        assert_eq!(state.selected_hit().unwrap().line_number, 1);
    }
}
