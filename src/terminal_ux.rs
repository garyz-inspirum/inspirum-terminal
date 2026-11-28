//! Policy helpers for interactive terminal UX.
//!
//! This module deliberately keeps paste classification, text search and log-file
//! lifecycle independent from the GUI toolkit so the security-sensitive behavior is easy to test.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PastePolicy {
    /// Single-line pastes are sent immediately. Any payload containing CR or LF
    /// requires an explicit confirmation.
    #[default]
    ConfirmMultiline,
    /// Every paste requires an explicit confirmation.
    ConfirmAll,
    /// Single-line pastes are sent immediately; multiline pastes are rejected.
    BlockMultiline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteDecision {
    Send,
    Confirm,
    Block,
}

pub fn is_multiline_paste(text: &str) -> bool {
    text.as_bytes()
        .iter()
        .any(|byte| matches!(byte, b'\r' | b'\n'))
}

pub fn classify_paste(policy: PastePolicy, text: &str) -> PasteDecision {
    if text.contains('\0') {
        return PasteDecision::Block;
    }
    let multiline = is_multiline_paste(text);
    match (policy, multiline) {
        (PastePolicy::ConfirmAll, _) => PasteDecision::Confirm,
        (PastePolicy::ConfirmMultiline, true) => PasteDecision::Confirm,
        (PastePolicy::BlockMultiline, true) => PasteDecision::Block,
        _ => PasteDecision::Send,
    }
}

/// Deliver input to its original focused PTY *before* any opted-in mirrors.
///
/// If the source fails (exited/locked/changed focus), no destination receives
/// the input. Targets contain stable pane IDs, never tab indices. This helper
/// is used by the Iced GUI and by real OpenSSH native smoke acceptance.
pub fn source_first_synced_write(
    source: u64,
    targets: &[u64],
    mut write: impl FnMut(u64) -> bool,
) -> bool {
    if !write(source) {
        return false;
    }
    for &target in targets {
        if target != source {
            let _ = write(target);
        }
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

pub fn find_text(haystack: &str, needle: &str, limit: usize) -> Vec<SearchHit> {
    let Some(query) = TextQuery::new(needle) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for (line_index, line) in haystack.lines().enumerate() {
        query.find_in_line(line, line_index + 1, limit, &mut hits);
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

/// A case-insensitive literal query, lowercased once per search.
///
/// Invariants:
/// - The query and each line are folded the same way (`str::to_lowercase`), and matching is
///   done on the folded text.
/// - Matches are non-overlapping, scanned left to right over the folded text (after a match
///   the scan resumes after its end).
/// - `count_in_line` and `find_in_line` (unlimited) agree on the number of matches per line.
/// - `column` is the 1-based char index in the ORIGINAL line of the char whose fold produced
///   the match's first folded char. Folding can add chars ('İ' -> "i\u{307}"), so folded
///   offsets are mapped back; a match starting inside an expansion (query begins with
///   U+0307) reports the expanding char. Callers use `column - 1` against original-text
///   char offsets (soft-wrap segment starts).
/// - Folded text is only sliced at offsets returned by `str::find` (match boundaries), and
///   the original line is only walked by `chars()`, so no slice can split a char.
///
/// ASCII query against an ASCII line compares bytes with ASCII case folding and allocates
/// nothing per line (there folded and original offsets coincide); any non-ASCII on either
/// side uses Unicode `to_lowercase` (e.g. ASCII "k" still finds KELVIN SIGN U+212A).
pub struct TextQuery {
    lower: String,
    ascii: bool,
}

impl TextQuery {
    /// `None` for an empty (after trimming) query.
    pub fn new(needle: &str) -> Option<Self> {
        let query = needle.trim();
        if query.is_empty() {
            return None;
        }
        Some(Self {
            lower: query.to_lowercase(),
            ascii: query.is_ascii(),
        })
    }

    /// Number of non-overlapping matches in `line`, without allocating per match (an
    /// ASCII query on an ASCII line allocates nothing; otherwise one lowercase copy per line).
    pub fn count_in_line(&self, line: &str) -> usize {
        if self.ascii && line.is_ascii() {
            let needle = self.lower.as_bytes();
            let bytes = line.as_bytes();
            let mut count = 0;
            let mut start = 0;
            while start + needle.len() <= bytes.len() {
                if bytes[start..start + needle.len()].eq_ignore_ascii_case(needle) {
                    count += 1;
                    start += needle.len();
                } else {
                    start += 1;
                }
            }
            return count;
        }
        line.to_lowercase().matches(self.lower.as_str()).count()
    }

    /// Append hits in `line` (reported as line number `line_number`) until `hits` holds
    /// `limit` entries.
    pub fn find_in_line(
        &self,
        line: &str,
        line_number: usize,
        limit: usize,
        hits: &mut Vec<SearchHit>,
    ) {
        if hits.len() >= limit {
            return;
        }
        if self.ascii && line.is_ascii() {
            let needle = self.lower.as_bytes();
            let bytes = line.as_bytes();
            let mut start = 0;
            while start + needle.len() <= bytes.len() {
                if bytes[start..start + needle.len()].eq_ignore_ascii_case(needle) {
                    hits.push(SearchHit {
                        line: line_number,
                        column: start + 1,
                        preview: preview_of(line),
                    });
                    if hits.len() >= limit {
                        return;
                    }
                    start += needle.len();
                } else {
                    start += 1;
                }
            }
            return;
        }
        let line_lower = line.to_lowercase();
        // Implicit folded-byte -> original-char map, walked forward once: hits arrive in
        // increasing folded byte order, so the cursor never rewinds (O(line) for all hits).
        let mut originals = line.chars();
        // Chars of `line` consumed so far, and the folded byte where the next one starts.
        let mut consumed = 0;
        let mut folded_end = 0;
        let mut start = 0;
        while start <= line_lower.len() {
            let Some(found) = line_lower[start..].find(&self.lower) else {
                break;
            };
            let byte_column = start + found;
            // Consume until the last consumed char's fold covers `byte_column`; that char
            // (index `consumed - 1`) produced the match's first folded char.
            while folded_end <= byte_column {
                let Some(original) = originals.next() else {
                    break;
                };
                folded_end += folded_len(original);
                consumed += 1;
            }
            hits.push(SearchHit {
                line: line_number,
                column: consumed.max(1),
                preview: preview_of(line),
            });
            if hits.len() >= limit {
                return;
            }
            start = byte_column + self.lower.len();
        }
    }
}

/// UTF-8 length of `c` in `str::to_lowercase` output. `char::to_lowercase` differs from the
/// string form only for final sigma ('Σ' -> 'ς' vs 'σ'), and both are 2 bytes.
fn folded_len(c: char) -> usize {
    c.to_lowercase().map(char::len_utf8).sum()
}

fn preview_of(line: &str) -> String {
    if line.chars().count() > 180 {
        line.chars().take(177).collect::<String>() + "..."
    } else {
        line.to_owned()
    }
}

/// Creates a new session log without overwriting an existing file.
pub fn start_session_log(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "session log path is required");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create new session log {}", path.display()))?;
    file.write_all(
        b"# Inspirum Terminal session log\n# WARNING: terminal output can contain passwords, tokens and other secrets.\n\n",
    )?;
    file.sync_all()?;
    Ok(())
}

/// Appends a changed screen snapshot. Input keystrokes are intentionally not logged.
/// Returns true when bytes were appended.
pub fn append_screen_snapshot(path: &Path, previous: &mut String, screen: &str) -> Result<bool> {
    if screen == previous {
        return Ok(false);
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("append session log {}", path.display()))?;
    file.write_all(b"--- screen snapshot ---\n")?;
    file.write_all(screen.as_bytes())?;
    if !screen.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    file.flush()?;
    *previous = screen.to_owned();
    Ok(true)
}

pub fn log_exists(path: &Path) -> bool {
    File::open(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronized_input_never_writes_a_mirror_if_source_fails() {
        let mut observed = Vec::new();
        assert!(!source_first_synced_write(30, &[10, 20, 30], |id| {
            observed.push(id);
            false
        }));
        assert_eq!(observed, vec![30]);
        observed.clear();
        assert!(source_first_synced_write(30, &[10, 20, 30], |id| {
            observed.push(id);
            true
        }));
        assert_eq!(observed, vec![30, 10, 20]);
    }

    #[test]
    fn multiline_detection_covers_lf_cr_and_crlf() {
        for payload in ["one\ntwo", "one\rtwo", "one\r\ntwo"] {
            assert!(is_multiline_paste(payload));
            assert_eq!(
                classify_paste(PastePolicy::ConfirmMultiline, payload),
                PasteDecision::Confirm
            );
        }
        assert!(!is_multiline_paste("one line"));
    }

    #[test]
    fn no_policy_sends_multiline_without_confirmation() {
        let payload = "rm -rf /\necho unsafe";
        assert_ne!(
            classify_paste(PastePolicy::ConfirmMultiline, payload),
            PasteDecision::Send
        );
        assert_ne!(
            classify_paste(PastePolicy::ConfirmAll, payload),
            PasteDecision::Send
        );
        assert_ne!(
            classify_paste(PastePolicy::BlockMultiline, payload),
            PasteDecision::Send
        );
    }

    #[test]
    fn nul_is_always_blocked() {
        for policy in [
            PastePolicy::ConfirmMultiline,
            PastePolicy::ConfirmAll,
            PastePolicy::BlockMultiline,
        ] {
            assert_eq!(classify_paste(policy, "hello\0world"), PasteDecision::Block);
        }
    }

    #[test]
    fn search_is_case_insensitive_and_bounded() {
        let hits = find_text("Alpha beta\nalpha gamma\nALPHA", "alpha", 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].column, 1);
        assert_eq!(hits[1].line, 2);

        let unicode = find_text("Ångström 東京", "ång", 5);
        assert_eq!(unicode.len(), 1);
        assert_eq!(unicode[0].column, 1);
    }

    /// Every hit column for `query` in `line`, unlimited.
    fn hit_columns(line: &str, query: &str) -> Vec<usize> {
        let Some(query) = TextQuery::new(query) else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        query.find_in_line(line, 1, usize::MAX, &mut hits);
        hits.iter().map(|hit| hit.column).collect()
    }

    #[test]
    fn folded_offset_reviewer_case_reports_original_column() {
        // 'İ' lowercases to "i\u{307}" (two chars); `X` is still the 2nd original char.
        assert_eq!(hit_columns("İXYZ", "x"), vec![2]);
        assert_eq!(hit_columns("İXYZ", "yz"), vec![3]);
    }

    #[test]
    fn folded_offset_survives_several_expansions_and_multiple_hits() {
        assert_eq!(hit_columns("İİİ needle", "needle"), vec![5]);
        assert_eq!(hit_columns("İİİ NEEDLE", "Needle"), vec![5]);
        // a b ' ' İ ' ' a b
        assert_eq!(hit_columns("ab İ ab", "AB"), vec![1, 6]);
        assert_eq!(hit_columns("İabİİabİ", "ab"), vec![2, 6]);
    }

    #[test]
    fn folded_offset_for_queries_touching_the_expansion() {
        // Query "i" matches the first folded char of 'İ'.
        assert_eq!(hit_columns("İ", "i"), vec![1]);
        assert_eq!(hit_columns("aİbİ", "I"), vec![2, 4]);
        // A match beginning inside the expansion maps to the char that produced it.
        assert_eq!(hit_columns("İ", "\u{307}"), vec![1]);
        assert_eq!(hit_columns("aİX", "\u{307}x"), vec![2]);
        // The query is folded the same way as the line.
        assert_eq!(hit_columns("aİXİx", "İx"), vec![2, 4]);
        assert_eq!(hit_columns("i\u{307}x", "İX"), vec![1]);
        assert_eq!(hit_columns("ix", "İx"), Vec::<usize>::new());
    }

    #[test]
    fn folded_offset_cjk_combining_and_emoji_columns() {
        assert_eq!(hit_columns("東京 network 東京", "東京"), vec![1, 12]);
        assert_eq!(hit_columns("東京 İ 東京", "東京"), vec![1, 6]);
        assert_eq!(
            hit_columns("Cafe\u{301} CAFE\u{301}", "cafe\u{301}"),
            vec![1, 7]
        );
        assert_eq!(hit_columns("e\u{301}İcafe\u{301}", "CAFE\u{301}"), vec![4]);
        assert_eq!(hit_columns("status 🚀 ready", "READY"), vec![10]);
        assert_eq!(hit_columns("🚀İ🚀 Ready", "ready"), vec![5]);
    }

    #[test]
    fn folded_offset_count_agrees_with_find() {
        let cases: &[(&str, &str)] = &[
            ("İXYZ", "x"),
            ("İİİ needle", "needle"),
            ("ab İ ab", "ab"),
            ("İabİİabİ", "ab"),
            ("İ", "i"),
            ("İ", "\u{307}"),
            ("aİXİx", "İx"),
            ("i\u{307}x", "İX"),
            ("ix", "İx"),
            ("東京 İ 東京", "東京"),
            ("e\u{301}İcafe\u{301}", "CAFE\u{301}"),
            ("🚀İ🚀 Ready", "ready"),
            ("aaaa", "aa"),
            ("İİİİ", "İİ"),
            ("plain ascii", "ÀB"),
            ("\u{212A}elvin kelvin", "K"),
            ("kelvin KELVIN", "\u{212A}"),
        ];
        for (line, needle) in cases {
            let query = TextQuery::new(needle).unwrap();
            assert_eq!(
                query.count_in_line(line),
                hit_columns(line, needle).len(),
                "{line:?} / {needle:?}"
            );
        }
        assert_eq!(hit_columns("İİİİ", "İİ"), vec![1, 3]);
    }

    #[test]
    fn folded_offset_selects_the_correct_soft_wrapped_row() {
        // "İXYZ" soft-wrapped into rows "İX" and "YZ": segment starts are char offsets.
        let segment_starts = [0usize, 2];
        let segment_for = |char_offset: usize| {
            segment_starts
                .iter()
                .rposition(|start| *start <= char_offset)
                .unwrap_or(0)
        };
        let x = hit_columns("İXYZ", "x");
        assert_eq!(x.len(), 1);
        assert_eq!(segment_for(x[0] - 1), 0);
        let y = hit_columns("İXYZ", "y");
        assert_eq!(y.len(), 1);
        assert_eq!(segment_for(y[0] - 1), 1);
    }

    #[test]
    fn mouse_triggered_paste_uses_the_same_multiline_guard() {
        let payload = "first command\nsecond command";
        assert_eq!(
            classify_paste(PastePolicy::ConfirmMultiline, payload),
            PasteDecision::Confirm
        );
        assert_eq!(
            classify_paste(PastePolicy::BlockMultiline, payload),
            PasteDecision::Block
        );
        assert_eq!(
            classify_paste(PastePolicy::ConfirmAll, "safe single line"),
            PasteDecision::Confirm
        );
    }

    #[test]
    fn session_log_is_create_new_and_snapshot_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.log");
        start_session_log(&path).unwrap();
        assert!(log_exists(&path));
        assert!(start_session_log(&path).is_err());

        let mut previous = String::new();
        assert!(append_screen_snapshot(&path, &mut previous, "hello").unwrap());
        assert!(!append_screen_snapshot(&path, &mut previous, "hello").unwrap());
        assert!(append_screen_snapshot(&path, &mut previous, "hello\nworld").unwrap());

        let saved = std::fs::read_to_string(path).unwrap();
        assert!(saved.contains("WARNING"));
        assert_eq!(saved.matches("--- screen snapshot ---").count(), 2);
    }
}

#[cfg(test)]
mod text_query_tests {
    use super::*;

    /// The pre-TextQuery implementation, kept as the reference.
    fn find_text_reference(haystack: &str, needle: &str, limit: usize) -> Vec<SearchHit> {
        let query = needle.trim();
        if query.is_empty() || limit == 0 {
            return Vec::new();
        }
        let query_lower = query.to_lowercase();
        let mut hits = Vec::new();
        for (line_index, line) in haystack.lines().enumerate() {
            let line_lower = line.to_lowercase();
            let mut start = 0;
            while start <= line_lower.len() {
                let Some(found) = line_lower[start..].find(&query_lower) else {
                    break;
                };
                let byte_column = start + found;
                let column = line_lower[..byte_column].chars().count() + 1;
                hits.push(SearchHit {
                    line: line_index + 1,
                    column,
                    preview: preview_of(line),
                });
                if hits.len() >= limit {
                    return hits;
                }
                start = byte_column + query_lower.len().max(1);
            }
        }
        hits
    }

    #[test]
    fn matches_reference_on_table() {
        let long = "x".repeat(200) + "Needle";
        let cases: &[(&str, &str)] = &[
            ("Alpha beta ALPHA", "alpha"),
            ("aaaa", "aa"),
            ("aaa", "aa"),
            ("東京 network 東京", "東京"),
            ("Cafe\u{301} CAFE\u{301}", "cafe\u{301}"),
            ("ÀB àb", "àb"),
            ("status 🚀 ready", "READY"),
            ("plain ascii", "ÀB"),
            ("line one\nLINE two\nnone", "line"),
            ("anything", ""),
            ("anything", "   "),
            (long.as_str(), "needle"),
        ];
        for (haystack, needle) in cases {
            for limit in [0, 1, 2, 500] {
                assert_eq!(
                    find_text(haystack, needle, limit),
                    find_text_reference(haystack, needle, limit),
                    "{haystack:?} / {needle:?} / {limit}"
                );
            }
        }
    }

    #[test]
    fn matches_do_not_overlap() {
        let hits = find_text("aaaa", "aa", 10);
        assert_eq!(
            hits.iter().map(|hit| hit.column).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }
}
