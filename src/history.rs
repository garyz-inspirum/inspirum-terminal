//! In-memory retained terminal history indexing, marks and bounded folding.
use crate::terminal_ux::TextQuery;
use std::collections::{BTreeMap, BTreeSet};
use terminal_core::{HistorySnapshot, LogicalLine};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryLine {
    pub id: u64,
    /// Logical line: soft-wrapped grid rows joined (see `terminal_core::LogicalLine`).
    pub text: String,
    /// Zero-based grid row (from the oldest retained row) of the first row of this line.
    pub first_row: usize,
    /// Char offsets into `text` at which each wrapped grid row starts; begins with `0`.
    /// Together with `first_row` this is all a jump needs; the text is stored only once.
    pub segment_starts: Vec<usize>,
}

impl HistoryLine {
    /// Grid row containing the char at `char_offset` (zero-based) of `text`.
    pub fn row_for_char(&self, char_offset: usize) -> usize {
        let segment = self
            .segment_starts
            .iter()
            .rposition(|start| *start <= char_offset)
            .unwrap_or(0);
        self.first_row + segment
    }
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
    /// Exact number of matches across all retained lines; `hits` stops at `HIT_LIST_CAP`.
    total_hits: usize,
    selected_hit: usize,
    truncated_lines: u64,
    /// Marks removed because their line identity became ambiguous or was reset.
    dropped_marks: u64,
}

/// Most hits kept in the selectable list; matches beyond it are counted, not listed.
pub const HIT_LIST_CAP: usize = 500;

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

    /// Exact number of matches across all retained lines (may exceed `hits().len()`).
    pub fn total_hits(&self) -> usize {
        self.total_hits
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

    /// Running total of marks removed because their line identity became ambiguous or was
    /// reset. Marks whose line rolled off with a uniquely known shift, and marks the user
    /// removed, are not counted.
    pub fn dropped_marks(&self) -> u64 {
        self.dropped_marks
    }

    /// Test convenience: one unwrapped grid row per string, modelling a pane at scrollback
    /// capacity whose last row is the live screen (`history_rows = len - 1 = capacity`).
    /// Every row but the last is immutable scrollback and ids follow text alignment only.
    #[cfg(test)]
    pub fn update_snapshot(&mut self, snapshot: Vec<String>) {
        let history_rows = snapshot.len().saturating_sub(1);
        self.update_lines(HistorySnapshot {
            lines: snapshot
                .into_iter()
                .enumerate()
                .map(|(row, text)| LogicalLine {
                    text,
                    first_row: row,
                    segment_starts: vec![0],
                })
                .collect(),
            history_rows,
            capacity: history_rows,
            columns: 80,
        });
    }

    pub fn update_lines(&mut self, snapshot: HistorySnapshot) {
        let snapshot = snapshot.lines;
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
        for (index, logical) in snapshot.into_iter().enumerate() {
            let id = match reused.get(index) {
                Some(id) => *id,
                None => {
                    self.next_line_id = self.next_line_id.saturating_add(1).max(1);
                    self.next_line_id
                }
            };
            let LogicalLine {
                text,
                first_row,
                segment_starts,
            } = logical;
            next.push(HistoryLine {
                id,
                text,
                first_row,
                segment_starts,
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

    /// Select the listed hit at exactly (`line_id`, `column`). Returns `false` and leaves the
    /// selection unchanged when no such hit is listed.
    pub fn select_hit_at(&mut self, line_id: u64, column: usize) -> bool {
        match self
            .hits
            .iter()
            .position(|hit| hit.line_id == line_id && hit.column == column)
        {
            Some(index) => {
                self.selected_hit = index;
                true
            }
            None => false,
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
        self.total_hits = 0;
        let Some(query) = TextQuery::new(&self.query) else {
            self.selected_hit = 0;
            return;
        };

        // Search each retained logical row separately so stable line IDs survive viewport movement.
        // Every match is counted; only the first `HIT_LIST_CAP` are listed.
        let mut local = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            self.total_hits += query.count_in_line(&line.text);
            if self.hits.len() >= HIT_LIST_CAP {
                continue;
            }
            local.clear();
            query.find_in_line(
                &line.text,
                index + 1,
                HIT_LIST_CAP - self.hits.len(),
                &mut local,
            );
            for hit in local.drain(..) {
                self.hits.push(HistoryHit {
                    line_id: line.id,
                    line_number: index + 1,
                    column: hit.column,
                    grid_row: line.row_for_char(hit.column - 1),
                    preview: hit.preview,
                });
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
        // The trailing "" is the live screen row; "three" is immutable scrollback.
        state.update_snapshot(vec!["one".into(), "two".into(), "three".into(), "".into()]);
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

        state.update_snapshot(vec!["three".into(), "four".into(), "".into()]);
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
        // A full pane: the scrollback limit plus a screen.
        let rows = terminal_core::SCROLLBACK_LINES + 50;
        let mut lines: Vec<String> = (0..rows).map(|n| format!("line-{n:05} ordinary")).collect();
        lines[rows - 1] = "last NEEDLE".into();
        state.update_snapshot(lines);
        state.set_query("needle");
        assert_eq!(state.hits().len(), 1);
        assert_eq!(state.total_hits(), 1);
        assert_eq!(state.hits()[0].line_number, rows);
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

    fn lines_snapshot(
        lines: Vec<LogicalLine>,
        history_rows: usize,
        capacity: usize,
        columns: usize,
    ) -> HistorySnapshot {
        HistorySnapshot {
            lines,
            history_rows,
            capacity,
            columns,
        }
    }

    fn line_at(text: &str, first_row: usize, segment_starts: &[usize]) -> LogicalLine {
        LogicalLine {
            text: text.to_owned(),
            first_row,
            segment_starts: segment_starts.to_vec(),
        }
    }

    /// One unwrapped 80-column row per string.
    fn rows_snapshot(texts: &[&str], history_rows: usize, capacity: usize) -> HistorySnapshot {
        let lines = texts
            .iter()
            .enumerate()
            .map(|(row, text)| line_at(text, row, &[0]))
            .collect();
        lines_snapshot(lines, history_rows, capacity, 80)
    }

    fn line_ids(state: &HistoryState) -> Vec<u64> {
        state.lines().iter().map(|line| line.id).collect()
    }

    fn mark_at(state: &mut HistoryState, index: usize) -> u64 {
        let line_id = state.lines()[index].id;
        state.marks.insert(
            line_id,
            HistoryMark {
                line_id,
                timestamp_epoch_seconds: None,
                collapsed: false,
            },
        );
        line_id
    }

    fn mark_ids(state: &HistoryState) -> Vec<u64> {
        state.marks().map(|mark| mark.line_id).collect()
    }

    fn assert_all_ids_fresh(state: &HistoryState, before: &[u64]) {
        assert!(
            line_ids(state).iter().all(|id| !before.contains(id)),
            "an old id was reused on a line whose identity is ambiguous"
        );
    }

    #[test]
    fn line_identity_identical_rows_rolling_off_at_capacity_drop_the_mark() {
        // Reviewer case: at capacity, three identical retained rows, mark on the newest.
        let mut state = HistoryState::default();
        let screen = ["same", "same", "same", "$"];
        state.update_lines(rows_snapshot(&screen, 3, 3));
        let before = line_ids(&state);
        mark_at(&mut state, 2);
        state.set_query("same");
        state.select_hit(2);

        // One row evicted at the top and one more "same" row scrolled in: the text is
        // unchanged, so shifts 0, 1 and 2 all fit the immutable history.
        state.update_lines(rows_snapshot(&screen, 3, 3));
        assert!(
            mark_ids(&state).is_empty(),
            "the mark must not stay on a different row"
        );
        assert_eq!(state.dropped_marks(), 1);
        assert_all_ids_fresh(&state, &before);
        assert_eq!(state.truncated_lines(), 0, "the shift is unknown");
        assert_eq!(state.selected_hit_index(), 0, "anchor dropped: first hit");
    }

    #[test]
    fn line_identity_no_scroll_control_at_capacity() {
        // Identical rows: a refresh with no scrolling is indistinguishable from the
        // roll-off above, so it is handled the same way (ambiguous, mark dropped).
        let mut identical = HistoryState::default();
        let screen = ["same", "same", "same", "$"];
        identical.update_lines(rows_snapshot(&screen, 3, 3));
        let before = line_ids(&identical);
        mark_at(&mut identical, 2);
        identical.update_lines(rows_snapshot(&screen, 3, 3));
        assert!(mark_ids(&identical).is_empty());
        assert_eq!(identical.dropped_marks(), 1);
        assert_all_ids_fresh(&identical, &before);

        // Distinct rows: shift 0 is the only one that fits, so ids and the mark stay.
        let mut distinct = HistoryState::default();
        let screen = ["a", "b", "c", "$"];
        distinct.update_lines(rows_snapshot(&screen, 3, 3));
        let before = line_ids(&distinct);
        let marked = mark_at(&mut distinct, 1);
        distinct.update_lines(rows_snapshot(&screen, 3, 3));
        assert_eq!(line_ids(&distinct), before);
        assert_eq!(mark_ids(&distinct), vec![marked]);
        assert_eq!(
            (distinct.dropped_marks(), distinct.truncated_lines()),
            (0, 0)
        );
    }

    #[test]
    fn line_identity_blank_run_rolling_off_at_capacity_drops_the_mark() {
        let mut state = HistoryState::default();
        let screen = ["", "", "", "$"];
        state.update_lines(rows_snapshot(&screen, 3, 3));
        let before = line_ids(&state);
        mark_at(&mut state, 2);
        state.update_lines(rows_snapshot(&screen, 3, 3));
        assert!(mark_ids(&state).is_empty());
        assert_eq!(state.dropped_marks(), 1);
        assert_all_ids_fresh(&state, &before);
        assert_eq!(state.truncated_lines(), 0);

        // A distinct row above the run does not help: it is the row that was evicted.
        let mut state = HistoryState::default();
        state.update_lines(rows_snapshot(&["x", "", "", "", "$"], 4, 4));
        let before = line_ids(&state);
        mark_at(&mut state, 3);
        state.update_lines(rows_snapshot(&["", "", "", "$", "out"], 4, 4));
        assert!(mark_ids(&state).is_empty());
        assert_eq!(state.dropped_marks(), 1);
        assert_all_ids_fresh(&state, &before);
    }

    #[test]
    fn line_identity_blank_run_no_scroll_control_at_capacity() {
        let mut blank = HistoryState::default();
        let screen = ["", "", "", "$"];
        blank.update_lines(rows_snapshot(&screen, 3, 3));
        let before = line_ids(&blank);
        mark_at(&mut blank, 1);
        blank.update_lines(rows_snapshot(&screen, 3, 3));
        assert!(mark_ids(&blank).is_empty(), "same as the blank roll-off");
        assert_eq!(blank.dropped_marks(), 1);
        assert_all_ids_fresh(&blank, &before);

        // Blank-separated distinct rows have one consistent shift.
        let mut separated = HistoryState::default();
        let screen = ["a", "", "b", "$"];
        separated.update_lines(rows_snapshot(&screen, 3, 3));
        let before = line_ids(&separated);
        let marked = mark_at(&mut separated, 1);
        separated.update_lines(rows_snapshot(&screen, 3, 3));
        assert_eq!(line_ids(&separated), before);
        assert_eq!(mark_ids(&separated), vec![marked]);
        assert_eq!(separated.dropped_marks(), 0);
    }

    #[test]
    fn line_identity_short_blank_border_makes_an_ordinary_log_ambiguous() {
        // The cost of refusing to guess: the true shift is 1, but the oldest retained line
        // and the newest history line are both blank, so shift 3 also fits.
        let mut state = HistoryState::default();
        state.update_lines(rows_snapshot(&["x", "", "a", "", "$"], 4, 4));
        mark_at(&mut state, 2);
        state.update_lines(rows_snapshot(&["", "a", "", "y", "$"], 4, 4));
        assert!(mark_ids(&state).is_empty());
        assert_eq!(state.dropped_marks(), 1);
    }

    #[test]
    fn line_identity_below_capacity_identical_and_blank_rows_keep_every_id_and_mark() {
        let mut state = HistoryState::default();
        state.update_lines(rows_snapshot(&["same", "same", "", "", "same"], 3, 100));
        let before = line_ids(&state);
        for index in [1, 2, 4] {
            mark_at(&mut state, index);
        }

        // Two rows scroll into history; the live last row was rewritten before it scrolled.
        state.update_lines(rows_snapshot(
            &["same", "same", "", "", "same more", "", "same"],
            5,
            100,
        ));
        let after = line_ids(&state);
        assert_eq!(after[..5], before[..]);
        assert!(after[5..].iter().all(|id| !before.contains(id)));
        assert_ne!(after[5], after[6]);
        assert_eq!(mark_ids(&state), vec![before[1], before[2], before[4]]);

        // One more identical row appended, then the live last row rewritten in place.
        let mut rows = ["same", "same", "", "", "same more", "", "same", "same"];
        state.update_lines(rows_snapshot(&rows, 6, 100));
        let appended = line_ids(&state);
        assert_eq!(appended[..7], after[..]);
        rows[7] = "same!";
        state.update_lines(rows_snapshot(&rows, 6, 100));
        assert_eq!(line_ids(&state), appended);
        assert_eq!(mark_ids(&state), vec![before[1], before[2], before[4]]);
        assert_eq!((state.dropped_marks(), state.truncated_lines()), (0, 0));
    }

    #[test]
    fn line_identity_below_capacity_cleared_and_regrown_history_is_a_reset() {
        let mut state = HistoryState::default();
        state.update_lines(rows_snapshot(&["a", "b", "c", "$"], 3, 100));
        let before = line_ids(&state);
        mark_at(&mut state, 1);
        state.set_query("c");

        // History was cleared and regrew past its old size with text that looks like a
        // one-line roll-off. Below capacity nothing can be evicted, so this is a reset.
        state.update_lines(rows_snapshot(&["b", "c", "d", "e", "$"], 4, 100));
        assert!(mark_ids(&state).is_empty());
        assert_eq!(state.dropped_marks(), 1);
        assert_all_ids_fresh(&state, &before);
        assert_eq!(state.truncated_lines(), 0);
        assert_eq!(state.selected_hit().unwrap().line_number, 2);
    }

    #[test]
    fn line_identity_unique_shift_at_capacity_keeps_ids_and_selection() {
        let mut state = HistoryState::default();
        state.update_lines(rows_snapshot(
            &["l0", "l1", "l2", "l3", "l4", "l5", "$"],
            6,
            6,
        ));
        let before = line_ids(&state);
        let marked = mark_at(&mut state, 3);
        state.set_query("l");
        assert!(state.select_hit_at(before[4], 1));
        assert_eq!(state.selected_hit_index(), 4);

        state.update_lines(rows_snapshot(
            &["l2", "l3", "l4", "l5", "l6", "l7", "$"],
            6,
            6,
        ));
        assert_eq!(line_ids(&state)[..4], before[2..6]);
        assert_eq!(state.truncated_lines(), 2);
        assert_eq!(mark_ids(&state), vec![marked]);
        assert_eq!(state.dropped_marks(), 0);
        assert_eq!(state.selected_hit().unwrap().line_id, before[4]);
        assert_eq!(state.selected_hit_index(), 2);
    }

    #[test]
    fn line_identity_cut_wrapped_top_line_at_capacity() {
        // Four columns; the oldest logical line spans rows 0-2 and loses row 0.
        let mut state = HistoryState::default();
        state.update_lines(lines_snapshot(
            vec![
                line_at("aaaabbbbcc", 0, &[0, 4, 8]),
                line_at("d", 3, &[0]),
                line_at("e", 4, &[0]),
                line_at("$", 5, &[0]),
            ],
            5,
            5,
            4,
        ));
        let before = line_ids(&state);
        mark_at(&mut state, 0);
        let marked = mark_at(&mut state, 1);

        state.update_lines(lines_snapshot(
            vec![
                line_at("bbbbcc", 0, &[0, 4]),
                line_at("d", 2, &[0]),
                line_at("e", 3, &[0]),
                line_at("f", 4, &[0]),
                line_at("$", 5, &[0]),
            ],
            5,
            5,
            4,
        ));
        let after = line_ids(&state);
        assert!(!before.contains(&after[0]), "cut line gets a fresh id");
        assert_eq!(after[1..4], before[1..4]);
        assert_eq!(state.truncated_lines(), 1);
        assert_eq!(mark_ids(&state), vec![marked], "cut line's mark rolled off");
        assert_eq!(state.dropped_marks(), 0);

        // The remainder loses one more row on the next refresh.
        state.update_lines(lines_snapshot(
            vec![
                line_at("cc", 0, &[0]),
                line_at("d", 1, &[0]),
                line_at("e", 2, &[0]),
                line_at("f", 3, &[0]),
                line_at("g", 4, &[0]),
                line_at("$", 5, &[0]),
            ],
            5,
            5,
            4,
        ));
        assert_eq!(line_ids(&state)[1..4], after[1..4]);
        assert_eq!(state.truncated_lines(), 2);
        assert_eq!(mark_ids(&state), vec![marked]);
        assert_eq!(state.dropped_marks(), 0);
    }

    #[test]
    fn line_identity_full_roll_off_at_capacity_is_not_counted_as_dropped() {
        let mut state = HistoryState::default();
        state.update_lines(rows_snapshot(&["a", "b", "$"], 2, 2));
        let before = line_ids(&state);
        mark_at(&mut state, 1);
        state.update_lines(rows_snapshot(&["c", "d", "$"], 2, 2));
        assert!(mark_ids(&state).is_empty());
        assert_all_ids_fresh(&state, &before);
        assert_eq!(state.truncated_lines(), 3);
        assert_eq!(state.dropped_marks(), 0);
    }

    #[test]
    fn line_identity_select_hit_at_exact_hit_only() {
        let mut state = HistoryState::default();
        state.update_snapshot(vec![
            "hit a".into(),
            "x".into(),
            "hit b hit c".into(),
            "$".into(),
        ]);
        state.set_query("hit");
        let first = state.lines()[0].id;
        let third = state.lines()[2].id;
        assert!(state.select_hit_at(third, 7));
        assert_eq!(state.selected_hit_index(), 2);
        assert!(!state.select_hit_at(third, 2), "no hit at that column");
        assert!(!state.select_hit_at(u64::MAX, 1), "unknown line");
        assert_eq!(state.selected_hit_index(), 2, "selection unchanged");
        assert!(state.select_hit_at(first, 1));
        assert_eq!(state.selected_hit_index(), 0);
    }

    #[test]
    fn line_identity_column_change_aligns_by_text() {
        // Eight columns to four: the wrapped first line reflows from two rows to three.
        let mut state = HistoryState::default();
        state.update_lines(lines_snapshot(
            vec![
                line_at("abcdefghij", 0, &[0, 8]),
                line_at("beta", 2, &[0]),
                line_at("gamma", 3, &[0]),
                line_at("$", 4, &[0]),
            ],
            4,
            100,
            8,
        ));
        let before = line_ids(&state);
        let marked = mark_at(&mut state, 1);
        state.update_lines(lines_snapshot(
            vec![
                line_at("abcdefghij", 0, &[0, 4, 8]),
                line_at("beta", 3, &[0]),
                line_at("gamma", 4, &[0]),
                line_at("$", 5, &[0]),
            ],
            5,
            100,
            4,
        ));
        assert_eq!(line_ids(&state), before);
        assert_eq!(mark_ids(&state), vec![marked]);
        assert_eq!(state.line(marked).unwrap().first_row, 3);
        assert_eq!((state.dropped_marks(), state.truncated_lines()), (0, 0));

        // With no immutable history there is no evidence across a reflow: reset.
        let mut screen_only = HistoryState::default();
        screen_only.update_lines(rows_snapshot(&["$"], 0, 100));
        mark_at(&mut screen_only, 0);
        let mut reflowed = rows_snapshot(&["$"], 0, 100);
        reflowed.columns = 40;
        screen_only.update_lines(reflowed);
        assert!(mark_ids(&screen_only).is_empty());
        assert_eq!(screen_only.dropped_marks(), 1);
    }

    #[test]
    fn hit_in_wrapped_line_targets_the_row_holding_the_match_start() {
        let mut state = HistoryState::default();
        state.update_lines(lines_snapshot(
            vec![
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
            ],
            2,
            100,
            6,
        ));
        state.set_query("efg");
        assert_eq!(state.selected_hit().unwrap().grid_row, 1);
        state.set_query("ghi");
        assert_eq!(state.selected_hit().unwrap().grid_row, 2);
        assert_eq!(state.selected_hit().unwrap().line_number, 2);
    }

    #[test]
    fn history_line_holds_its_text_once() {
        // Compile-level guarantee: destructuring lists every field, so a second String
        // (e.g. a nested `LogicalLine`) would fail to compile here.
        let mut state = HistoryState::default();
        state.update_lines(lines_snapshot(
            vec![LogicalLine {
                text: "abcdefgh".into(),
                first_row: 3,
                segment_starts: vec![0, 4],
            }],
            3,
            100,
            4,
        ));
        let HistoryLine {
            id: _,
            text,
            first_row,
            segment_starts,
        } = state.lines()[0].clone();
        assert_eq!(text, "abcdefgh");
        assert_eq!((first_row, segment_starts), (3, vec![0, 4]));
        assert_eq!(state.lines()[0].row_for_char(5), 4);
    }

    #[test]
    fn total_hits_counts_past_the_listed_cap() {
        let mut state = HistoryState::default();
        state.update_snapshot((0..300).map(|_| "aa xx aa".to_owned()).collect());
        state.set_query("aa");
        assert_eq!(state.total_hits(), 600);
        assert_eq!(state.hits().len(), HIT_LIST_CAP);
        state.select_previous_hit();
        assert_eq!(
            state.selected_hit_index(),
            HIT_LIST_CAP - 1,
            "wraps within the list"
        );
        state.select_next_hit();
        assert_eq!(state.selected_hit_index(), 0);
        state.set_query("AA XX");
        assert_eq!((state.total_hits(), state.hits().len()), (300, 300));
        state.set_query("zz");
        assert_eq!((state.total_hits(), state.hits().len()), (0, 0));
        state.set_query("");
        assert_eq!(state.total_hits(), 0);
    }
}
