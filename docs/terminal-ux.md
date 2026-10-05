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
