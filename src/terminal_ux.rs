//! Policy helpers for interactive terminal UX.
//!
//! This module deliberately keeps paste classification, text search and log-file
//! lifecycle independent from egui so the security-sensitive behavior is easy to test.
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

pub fn classify_mouse_paste(policy: PastePolicy, text: &str) -> PasteDecision {
    // Pointer shortcuts intentionally share the exact same policy as keyboard/OS paste.
    classify_paste(policy, text)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

pub fn find_text(haystack: &str, needle: &str, limit: usize) -> Vec<SearchHit> {
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
            let preview = if line.chars().count() > 180 {
                line.chars().take(177).collect::<String>() + "..."
            } else {
                line.to_owned()
            };
            hits.push(SearchHit {
                line: line_index + 1,
                column,
                preview,
            });
            if hits.len() >= limit {
                return hits;
            }
            start = byte_column + query_lower.len().max(1);
        }
    }
    hits
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
    fn mouse_triggered_paste_cannot_bypass_guarded_multiline_policy() {
        let multiline = "echo one\necho two";
        assert_eq!(
            classify_mouse_paste(PastePolicy::ConfirmMultiline, multiline),
            PasteDecision::Confirm
        );
        assert_eq!(
            classify_mouse_paste(PastePolicy::BlockMultiline, multiline),
            PasteDecision::Block
        );
        assert_eq!(
            classify_mouse_paste(PastePolicy::ConfirmAll, "single line"),
            PasteDecision::Confirm
        );
        assert_eq!(
            classify_mouse_paste(PastePolicy::ConfirmMultiline, "hello\0world"),
            PasteDecision::Block
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
