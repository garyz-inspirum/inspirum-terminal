# SSH terminal UX

Feature #21 adds safety controls around the existing terminal selection and copy support.

## Copy

Selection remains owned by the terminal widget. Platform keyboard shortcuts continue
to work, and the toolbar provides **Copy selection**. Remote clipboard requests are
not forwarded to the desktop clipboard.

## Paste

Paste is classified before it reaches the PTY. The default policy confirms every
payload containing CR or LF. A NUL-containing payload is rejected. Users may choose
**confirm all** or **block multiline**; there is intentionally no mode that sends
multiline clipboard content without confirmation.

The confirmation dialog supports Enter to send and Escape to cancel.

## Search

Ctrl/Cmd-Shift-F opens case-insensitive local search of the current rendered terminal
viewport and shows bounded line/column previews. It does not send search text to the
remote host. Full retained-scrollback navigation and highlighting remain follow-up work.

## Session logging

Logging is off by default and is enabled explicitly for the active tab. The destination
must be a new file, so existing files are not overwritten. The logger records changed
rendered-screen snapshots and does not record local input keystrokes. Logging stops when
the owning tab closes or when writing fails.

Terminal output may contain sensitive material. The UI and log header warn about this;
users should protect and review log files before sharing them. Prompt text is not parsed
or inferred. Normally hidden interactive input is not captured from the local input side,
but anything printed by the remote program can appear in a screen snapshot.

## Keyboard-only flows

- Copy: Command-C on macOS; Ctrl-Shift-C on Windows/Linux.
- Paste: Command-V on macOS; Ctrl-Shift-V on Windows/Linux.
- Search: Ctrl/Cmd-Shift-F.
- Confirmation: Enter sends; Escape cancels.


## Retained history search, marks and navigation

Issue #57 extends search from the visible viewport to the terminal emulator's existing
in-memory scrollback buffer. Inspirum does not create a parallel keystroke log and does
not write retained history to disk. Disk output still requires the separate explicit
**Start logging** action.

Open **Search** or press **Ctrl/Cmd+Shift+F**. Search covers retained scrollback plus the
visible screen. Results are shown with retained line and column positions, and the
selected result is visibly highlighted in the result list. **F3** selects the next
result and **Shift+F3** selects the previous result; navigation wraps and scrolls the
terminal viewport so the selected retained line is visible.

**Mark output boundary** bookmarks the newest retained non-empty output line. Optional
timestamps store only a Unix timestamp alongside the in-memory mark. Marks survive
normal viewport movement and appended output while their retained line remains in
scrollback. When terminal scrollback truncation removes a marked line, that mark is
removed deterministically and the inspector reports the cumulative truncated-line
count.

A mark can fold the bounded region after it up to the next mark. Folding affects only
the **History inspector** presentation; it never changes terminal parser state, deletes
scrollback, reconnects a session, or replays input. The inspector limits rendering to
its last 200 presentation rows even when the retained terminal buffer is much larger.

Search is bounded to 500 displayed matches per active tab. Tests cover ASCII,
Unicode/CJK, combining-character and emoji queries, deterministic mark/truncation
behavior, bounded folding, navigation wrapping, and a 50,000-line retained-history
regression.
