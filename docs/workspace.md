The default Iced frontend supports up to four panes. Saved layouts retain the exact split tree, axes and ratios. Older version-1 one/two-pane layouts remain readable; the optional `tree` field supplies the extended topology. Loading a layout, including startup metadata, does not connect. Restore requires both the saved reconnect opt-in and an explicit user action.

Open **Tools → Workspace** to save/load/restore a layout, find/reorder/color tabs, confirm bulk closes, select synchronized-input targets and arm input mirroring. Target edits, tab changes, reconnects and closes disarm mirroring. Input only reaches live selected panes in the active workspace. Delayed paste confirmations retain the original target IDs and are cancelled if focus or the target plan changes.

The following describes the previous frontend, retained with `--ui legacy`:

# SSH workspace

Feature #22 adds a bounded two-pane SSH workspace on top of the existing tab lifecycle.

## Split panes

A connected SSH tab can be split horizontally or vertically. The second pane opens a
separate OpenSSH PTY using the same validated non-secret session policy as the source
tab. Each pane remains an ordinary terminal tab underneath the workspace.

Closing a pane removes its tab and drops its `TerminalBackend`; the backend drop path
shuts down the PTY/process worker. Reconnecting a pane allocates a new terminal id and
always disarms synchronized input.

## Layout persistence

Workspace layouts are JSON schema version 1 and contain:

- split axis;
- one or two validated non-secret SSH session definitions;
- an explicit `reconnect_on_restore` permission flag.

Saving is atomic and bounded to 1 MiB. Loading a file only parses and validates metadata.
It never opens a connection. A loaded layout can only create PTYs after the user presses
**Restore & reconnect**, and that button is enabled only when the saved layout itself
contains `reconnect_on_restore: true`.

No workspace is restored automatically at application startup.

## Synchronized input

Synchronized input starts disarmed. Each destination pane must be selected explicitly.
At least two selected panes are required before the arm control becomes available, and
mirroring only occurs when the focused source pane is one of those selected targets.

While armed, the workspace displays the prominent text **SYNC INPUT ARMED**. Normal text,
single-line safe paste, Enter/Tab/Backspace and common navigation keys are mirrored.
Multiline paste remains subject to Feature #21's confirmation policy; a confirmed
multiline paste is mirrored only after the explicit confirmation.

Closing or reconnecting a participating pane disarms synchronization.

## Current scope

The first workspace implementation intentionally supports at most two simultaneous split
panes. Existing tabs outside the workspace remain available but are not synchronized
unless they are part of the current two-pane workspace.


## Advanced tab management

Feature #56 extends the workspace with deterministic tab-management controls:

- **Quick switcher** searches open tab names and is keyboard accessible with
  **Ctrl/Cmd+Shift+K**, Arrow Up/Down, Enter and Escape.
- Tabs can be moved left or right without reconnecting their PTYs.
- Each open tab can carry a runtime visual label. Labels are presentation-only and do
  not alter SSH policy or connection state.
- Checkboxes provide multi-select for **Close selected**. **Close right** and
  **Close others** provide bounded bulk actions around the active tab.
- Bulk close always presents the complete affected list and explicitly counts live
  sessions before any PTY is removed.
- Split-pane headers show a visible **FOCUSED** marker for the pane currently receiving
  terminal keyboard input.

All close paths use one ownership-removal path. Each removed tab owns exactly one
`TerminalBackend`; removing it drops that backend once, which performs the existing
PTY/process cleanup.

Changing synchronized-input target membership or closing a participating tab always
disarms synchronized input. Reordering tabs never replays input and never reconnects a
session.

Saved workspace metadata remains non-connecting on load. The explicit
**Restore & reconnect** safeguard and saved reconnect permission are unchanged.
