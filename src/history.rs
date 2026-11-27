//! In-memory retained terminal history indexing, marks and bounded folding.
use crate::terminal_ux::TextQuery;
use std::collections::{BTreeMap, BTreeSet};
use terminal_core::LogicalLine;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryLine {
    pub id: u64,
    /// Logical line: soft-wrapped grid rows joined (see `terminal_core::LogicalLine`).
    pub text: String,
    /// Where this line sits in the grid, for mapping a hit back to a scroll target.
    pub location: LogicalLine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryHit {
    pub line_id: u64,
    pub line_number: usize,
    pub column: usize,
    /// Zero-based grid row (from the oldest retained row) containing the match start.
    pub grid_row: usize,
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
    Folded {
        after_line_id: u64,
        hidden_lines: usize,
    },
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

/// How many trailing rows of the previous snapshot may have changed in place.
///
/// Rows on the live screen (prompt being typed, blank rows below the cursor, progress bars)
/// are rewritten in place; rows above the screen are immutable once scrolled into history.
/// The bound is larger than any realistic screen height; the alignment pass is a single
/// linear scan regardless of its value.
const LIVE_TAIL_ROWS: usize = 512;

/// Align `new` against `old` when up to `max_tail` trailing rows of `old` may have been
/// rewritten in place.
///
/// Returns `(overlap, tail)`: the first `overlap` rows of `new` equal the `overlap` rows of
/// `old` that end `tail` rows before its end. Picks the longest overlap, then the smallest
/// tail. `(0, 0)` means nothing aligned.
fn align_snapshot(old: &[HistoryLine], new: &[LogicalLine], max_tail: usize) -> (usize, usize) {
    if old.is_empty() || new.is_empty() {
        return (0, 0);
    }

    // KMP failure table over the new snapshot.
    let mut prefix = vec![0usize; new.len()];
    for index in 1..new.len() {
        let mut matched = prefix[index - 1];
        while matched > 0 && new[index].text != new[matched].text {
            matched = prefix[matched - 1];
        }
        if new[index].text == new[matched].text {
            matched += 1;
        }
        prefix[index] = matched;
    }

    // One pass over old: `matched` after old[i] is the longest prefix of new that is a
    // suffix of old[..=i], i.e. the overlap when old rows after i are the mutable tail.
    let first_candidate = old.len().saturating_sub(max_tail.saturating_add(1));
    let mut best = (0usize, 0usize);
    let mut matched = 0usize;
    for (index, line) in old.iter().enumerate() {
        while matched > 0 && line.text != new[matched].text {
            matched = prefix[matched - 1];
        }
        if line.text == new[matched].text {
            matched += 1;
        }
        if index >= first_candidate {
            let tail = old.len() - 1 - index;
            if matched > best.0 || (matched == best.0 && matched > 0 && tail < best.1) {
                best = (matched, tail);
            }
        }
        if matched == new.len() {
            matched = prefix[matched - 1];
        }
    }
    best
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

    pub fn selected_hit_index(&self) -> usize {
        self.selected_hit
    }

    pub fn line(&self, line_id: u64) -> Option<&HistoryLine> {
        self.lines.iter().find(|line| line.id == line_id)
    }

    pub fn truncated_lines(&self) -> u64 {
        self.truncated_lines
    }

    /// Test convenience: one unwrapped grid row per string.
    #[cfg(test)]
    pub fn update_snapshot(&mut self, snapshot: Vec<String>) {
        self.update_lines(
            snapshot
                .into_iter()
                .enumerate()
                .map(|(row, text)| LogicalLine {
                    text,
                    first_row: row,
                    segment_starts: vec![0],
                })
                .collect(),
        );
    }

    pub fn update_lines(&mut self, snapshot: Vec<LogicalLine>) {
        let anchor = self.selected_anchor();
        let (overlap, tail) = align_snapshot(&self.lines, &snapshot, LIVE_TAIL_ROWS);
        let aligned_end = self.lines.len() - tail;
        let retained_start = aligned_end - overlap;
        if retained_start > 0 {
            self.truncated_lines = self.truncated_lines.saturating_add(retained_start as u64);
        }

        // Rows above the mutable tail keep their ids. Rewritten tail rows keep the id of the
        // grid row they replaced in place (same position), so a mark on a prompt row being
        // typed into survives; genuinely new rows get fresh ids. Grid locations always come
        // from the new snapshot because rows shift as history grows.
        let reused: Vec<u64> = if overlap > 0 {
            self.lines[retained_start..]
                .iter()
                .map(|line| line.id)
                .collect()
        } else {
            Vec::new()
        };
        let mut next = Vec::with_capacity(snapshot.len());
        for (index, location) in snapshot.into_iter().enumerate() {
            let id = match reused.get(index) {
                Some(id) => *id,
                None => {
                    self.next_line_id = self.next_line_id.saturating_add(1).max(1);
                    self.next_line_id
                }
            };
            next.push(HistoryLine {
                id,
                text: location.text.clone(),
                location,
            });
        }
        if let Some(max_id) = next.iter().map(|line| line.id).max() {
            self.next_line_id = self.next_line_id.max(max_id);
        }
        self.lines = next;

        let retained_ids: BTreeSet<u64> = self.lines.iter().map(|line| line.id).collect();
        self.marks.retain(|id, _| retained_ids.contains(id));
        self.refresh_search(anchor);
    }

    /// `(line id, match column)` of the selected hit, used to keep the selection on the same
    /// match while lines arrive or roll off.
    fn selected_anchor(&self) -> Option<(u64, usize)> {
        self.hits
            .get(self.selected_hit)
            .map(|hit| (hit.line_id, hit.column))
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        let query = query.into();
        if self.query != query {
            self.query = query;
            self.selected_hit = 0;
            self.refresh_search(None);
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

    pub fn folded_count_after(&self, line_id: u64) -> usize {
        let Some(mark) = self.marks.get(&line_id) else {
            return 0;
        };
        if !mark.collapsed {
            return 0;
        }
        let next_mark = self.marks.keys().copied().find(|id| *id > line_id);
        let Some(index) = self.lines.iter().position(|line| line.id == line_id) else {
            return 0;
        };
        self.lines[index + 1..]
            .iter()
            .take_while(|line| next_mark.is_none_or(|next_id| line.id < next_id))
            .count()
    }

    /// Render rows for the history inspector. A collapsed mark hides lines until the next mark.
    pub fn display_rows(&self) -> Vec<HistoryRow> {
        let mut rows = Vec::new();
        let mark_ids: Vec<u64> = self.marks.keys().copied().collect();

        let mut index = 0usize;
        while index < self.lines.len() {
            let line = self.lines[index].clone();
            rows.push(HistoryRow::Line(line.clone()));

            let collapsed = self.marks.get(&line.id).is_some_and(|mark| mark.collapsed);
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

    /// Recompute hits. With `anchor`, reselect the same (line id, column) match; when it is
    /// gone, select the nearest later hit (the first hit when its line rolled off the top).
    fn refresh_search(&mut self, anchor: Option<(u64, usize)>) {
        self.hits.clear();
        let Some(query) = TextQuery::new(&self.query) else {
            self.selected_hit = 0;
            return;
        };

        // Search each retained logical row separately so stable line IDs survive viewport movement.
        let mut local = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            local.clear();
            query.find_in_line(&line.text, index + 1, 500 - self.hits.len(), &mut local);
            for hit in local.drain(..) {
                self.hits.push(HistoryHit {
                    line_id: line.id,
                    line_number: index + 1,
                    column: hit.column,
                    grid_row: line.location.row_for_char(hit.column - 1),
                    preview: hit.preview,
                });
            }
            if self.hits.len() >= 500 {
                break;
            }
        }
        if self.hits.is_empty() {
            self.selected_hit = 0;
            return;
        }
        if let Some((line_id, column)) = anchor {
            let anchor_line = self.lines.iter().position(|line| line.id == line_id);
            self.selected_hit = match anchor_line {
                // Hits are ordered by (line_number, column); first one at or after the anchor.
                Some(line_index) => self
                    .hits
                    .iter()
                    .position(|hit| (hit.line_number - 1, hit.column) >= (line_index, column))
                    .unwrap_or(self.hits.len() - 1),
                None => 0,
            };
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
        assert!(
            rows.iter()
                .any(|row| matches!(row, HistoryRow::Line(line) if line.id == second))
        );
    }

    #[test]
    fn large_scrollback_search_is_bounded_and_deterministic() {
        let mut state = HistoryState::default();
        let mut lines: Vec<String> = (0..50_000)
            .map(|n| format!("line-{n:05} ordinary"))
            .collect();
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

    fn ids_of(state: &HistoryState, texts: &[&str]) -> Vec<u64> {
        texts
            .iter()
            .map(|text| {
                state
                    .lines()
                    .iter()
                    .find(|line| line.text == *text)
                    .map(|line| line.id)
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn ids_survive_append_when_live_tail_row_changes() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["A".into(), "B".into(), "".into()]);
        let before = ids_of(&state, &["A", "B"]);
        state.update_snapshot(vec!["A".into(), "B".into(), "X".into(), "".into()]);
        assert_eq!(ids_of(&state, &["A", "B"]), before);
    }

    #[test]
    fn ids_survive_in_place_edit_of_live_last_row() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["A".into(), "B".into(), "pro".into()]);
        let before = ids_of(&state, &["A", "B"]);
        state.update_snapshot(vec!["A".into(), "B".into(), "prompt$ ".into()]);
        assert_eq!(ids_of(&state, &["A", "B"]), before);
    }

    #[test]
    fn ids_survive_roll_off_with_live_tail() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["A".into(), "B".into(), "C".into(), "".into()]);
        let before = ids_of(&state, &["B", "C"]);
        state.update_snapshot(vec!["B".into(), "C".into(), "D".into(), "".into()]);
        assert_eq!(ids_of(&state, &["B", "C"]), before);
        assert_eq!(state.truncated_lines(), 1);
    }

    #[test]
    fn selected_hit_follows_its_line_when_lines_arrive_and_roll_off() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["hit a".into(), "x".into(), "hit b".into(), "".into()]);
        state.set_query("hit");
        state.select_next_hit();
        let selected = state.selected_hit().unwrap().line_id;
        state.update_snapshot(vec![
            "hit a".into(),
            "x".into(),
            "hit b".into(),
            "hit c".into(),
            "".into(),
        ]);
        assert_eq!(state.selected_hit().unwrap().line_id, selected);
        // "hit a" rolls off: selection stays on "hit b", now at list index 0.
        state.update_snapshot(vec!["x".into(), "hit b".into(), "hit c".into(), "".into()]);
        assert_eq!(state.selected_hit().unwrap().line_id, selected);
        assert_eq!(state.selected_hit_index(), 0);
    }

    #[test]
    fn selection_moves_to_nearest_later_hit_when_its_line_rolls_off() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec!["hit a".into(), "x".into(), "hit b".into(), "".into()]);
        state.set_query("hit");
        assert_eq!(state.selected_hit().unwrap().preview, "hit a");
        state.update_snapshot(vec!["x".into(), "hit b".into(), "y".into(), "".into()]);
        assert_eq!(state.selected_hit().unwrap().preview, "hit b");
    }

    #[test]
    fn hit_in_wrapped_line_targets_the_row_holding_the_match_start() {
        let mut state = HistoryState::default();
        state.update_lines(vec![
            LogicalLine {
                text: "top".into(),
                first_row: 0,
                segment_starts: vec![0],
            },
            LogicalLine {
                text: "abcdefghi".into(),
                first_row: 1,
                segment_starts: vec![0, 6],
            },
        ]);
        state.set_query("efg");
        assert_eq!(state.selected_hit().unwrap().grid_row, 1);
        state.set_query("ghi");
        assert_eq!(state.selected_hit().unwrap().grid_row, 2);
        assert_eq!(state.selected_hit().unwrap().line_number, 2);
    }
}
