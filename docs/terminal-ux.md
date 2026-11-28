# SSH terminal UX

Feature #21 adds safety controls around the existing terminal selection and copy support.

## Copy

Selection remains owned by the terminal widget. Platform keyboard shortcuts continue
to work; **Edit → Copy** copies the focused pane's selection. Remote clipboard requests are
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

Open **Search**, choose **Edit > Find**, or press **Ctrl+F** (Cmd+F on macOS) to open
the Find strip for the focused pane. Search covers retained scrollback plus the visible
screen. Results are shown with retained line and column positions, and the selected
result is visibly highlighted in the result list.

Find shortcut rules:

- **Ctrl+F** opens Find, except when the focused pane is in alternate-screen mode
  (vim, less, htop and other full-screen programs). There Ctrl+F is sent to the
  program as before (byte 0x06, page-forward in vim/less), exactly once.
- **Ctrl/Cmd+Shift+F** always opens Find, including in alternate-screen programs.
- On macOS, **Cmd+F** always opens Find and **Ctrl+F** is always sent to the program.
- While a menu is open, Ctrl+F is ignored. While a dialog, the remote editor, the
  privacy lock, the Sender editor or a free-type draft owns the keyboard, Ctrl+F does
  what Ctrl+Shift+F does in that state, and is never sent to the terminal.
- Opening or re-focusing Find by any of these commands (or Edit > Find, the palette)
  keeps the previous query and selects it: typing replaces it, Enter/Next searches it.
  Clicking into the input places the caret without selecting.
- Pressing Ctrl+F (Cmd+F on macOS) or Ctrl/Cmd+Shift+F again while the Find input
  has the keyboard keeps focus there and selects the query again; no `f` is typed and
  nothing reaches the terminal. In any other text field (Sender, dialogs, editors)
  the chord stays with that field and does not open Find.

In the Find strip, **Enter** or **Next** selects the next result and **Prev** the
previous one; navigation wraps and scrolls the terminal viewport so the selected
retained line is visible.
**F3** and **Shift+F3** are not Find keys yet: they are sent to the terminal program.
Find-strip key navigation is left for a later stage.

**Mark output boundary** bookmarks the newest retained non-empty output line. Optional
timestamps store only a Unix timestamp alongside the in-memory mark. Marks survive
normal viewport movement and appended output while their retained line remains in
scrollback. When terminal scrollback truncation removes a marked line, that mark is
removed deterministically and the inspector reports the cumulative truncated-line
count. Once scrollback is full (10,000 rows) the terminal reports no eviction count, so
when the retained text could have scrolled by more than one amount (for example runs of
identical or blank lines) marks are dropped rather than moved to a possibly different
line, and the selected Find result returns to the first match.

A mark can fold the bounded region after it up to the next mark. Folding affects only
the **History inspector** presentation; it never changes terminal parser state, deletes
scrollback, reconnects a session, or replays input. The inspector limits rendering to
its last 200 presentation rows even when the retained terminal buffer is much larger.

Search is bounded to 500 displayed matches per active tab. Tests cover ASCII,
Unicode/CJK, combining-character and emoji queries, deterministic mark/truncation
behavior, bounded folding, navigation wrapping, and a full-pane retained-history regression (the
10,000-line default scrollback plus a screen).


## Terminal appearance and interaction preferences

Issue #58 adds a separate bounded appearance settings file next to the profile store.
Missing settings preserve the previous terminal defaults: 14 pt monospace text, the
existing dark ANSI palette, block cursor, normal selection behavior, and no mouse paste
shortcuts.

Global preferences provide the default for every terminal. An active profile can opt
into its own override; profiles without an override continue to inherit the global
settings. Profile overrides cover font family/size, palette, foreground/background,
cursor style, select-to-copy, mouse paste shortcuts and pointer hiding. Window opacity
is intentionally global only.

Built-in palettes are **Default dark**, **Light**, and **High contrast**. Optional custom
foreground/background colors must be `#RRGGBB`. The UI warns when the resulting
foreground/background contrast falls below 3:1. Invalid values are rejected on save or
load rather than silently substituted.

Cursor styles are block, underline and beam. **Select to copy** remains disabled by
default. Middle-click and right-click paste are explicit opt-ins. A mouse paste shortcut
only asks the native window for clipboard paste; the resulting paste event still passes
through the same **confirm multiline / confirm all / block multiline** policy used by
keyboard paste, so mouse shortcuts cannot bypass guarded paste.

**Hide pointer while typing** hides the native cursor after terminal text/key input and
shows it again on pointer movement.

The opacity preference is persisted, validated and applied to the Iced terminal surface. It does not set operating-system window opacity.

Appearance settings are JSON schema version 1, limited to 64 KiB, validated before use,
and replaced atomically. Missing fields migrate to defaults; unknown fields and unknown
schema versions are rejected.


## Command palette, command sender and snippets

Issue #60 adds keyboard-first operator shortcuts without making convenience features execute
remote commands implicitly.

Press **Ctrl/Cmd+Shift+P** to open the command palette. Palette entries are explicitly
separated into **[APP]** actions and **[REMOTE TEXT]** snippets:

- application actions run only when the user explicitly activates the selected palette item;
- snippet selection or completion only copies the snippet body into the command sender;
- staged snippet text is never sent merely because it was selected, searched, or completed.

The command sender supports an explicit target choice:

- **Current pane** sends only to the active terminal;
- **Selected sync panes** resolves the currently selected synchronized-input targets and sends
  only to that deterministic set.

Every remote-text send goes through the existing paste-safety classifier. Multiline content
still requires confirmation (or is blocked, depending on the configured paste policy), and
NUL-containing payloads remain blocked. Confirmation dialogs suppress application shortcuts so
they cannot be bypassed by the command palette.

Snippets are stored in a separate bounded JSON file beside the profile store. The library is
validated before persistence, writes are atomic, duplicate names are rejected, individual
snippet bodies are bounded, and exports refuse to overwrite an existing destination. Snippet
metadata is intended for non-secret operational text; passwords, private keys, passphrases and
other secrets should not be stored in snippets.

Import and export never auto-execute snippet bodies. Completion by snippet name only stages text
in the sender and requires a separate **Send** action.


## WindTerm-style workspace shell

Issue #78 reorganizes the default application surface around the operator workflow rather than around configuration forms. The primary layout follows WindTerm's published IDE-style arrangement: a narrow Resource Manager on the left, a compact application menu and session tab strip above the terminal workspace, optional utility panes in the central workspace, and a small status bar at the bottom.

Advanced profile, authentication, forwarding, appearance, keyboard and snippet controls remain available but are hidden by default behind **Settings** or the application menus. This keeps the normal workspace terminal-centric while preserving all previously implemented capabilities and safety confirmations.

The UI uses compact split-pane headers, restrained separators and clear focus states; pane headers show mode labels and close/reconnect controls, not sync or mode action buttons. Application menus hold Copy/Paste (**Edit**), Files/Shell (**View**), Free Type Mode/Remote Keys/Sync Input... (**Mode**) and workspace controls (**Window**). Existing keyboard shortcuts remain available for command palette, quick switching, retained-history search and tab management.


### Split-pane chrome

The split-terminal workspace uses compact pane headers rather than full control panels. The focused pane is visually distinguished; close is a small pane-local control. Split creation is under **Window**. Save/restore and per-pane synchronized-input target checkboxes are under **Window → Workspace**, together with **ARM synchronized input**. **Mode → Sync Input...** opens Workspace when fewer than two targets are selected and otherwise toggles arm/disarm. Focusing an unselected pane while other targets are armed does not mirror its input; it receives its own input only.

When sync is armed, the status-bar badge counts N selected targets. A selected focused source is eligible to mirror to selected live targets, but delivery is not guaranteed if its source write fails. An unselected focused source receives input only to itself, even while the other targets remain armed. Changing selected targets disarms sync; it is never armed automatically.
