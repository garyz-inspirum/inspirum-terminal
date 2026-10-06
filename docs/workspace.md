# SSH workspace and tab management

The workspace builds a bounded two-pane SSH layout on top of the ordinary tab lifecycle. Phase 3 issue #56 adds day-to-day tab management without changing the PTY ownership model.

## Split panes

A connected SSH tab can be split horizontally or vertically. The second pane opens a
separate OpenSSH PTY using the same validated non-secret session policy as the source
tab. Each pane remains an ordinary terminal tab underneath the workspace.

Closing a pane removes its tab and drops its `TerminalBackend`; the backend drop path
shuts down the PTY/process worker. Reconnecting a pane allocates a new terminal id and
always disarms synchronized input.

## Tab management

Open terminal tabs can be searched with the **Quick switcher** or **Ctrl/Cmd+P**. The switcher supports typing to filter, Up/Down to change selection, Enter to activate, and Escape to close without switching. Search results preserve the current tab order.

Each tab can carry an in-memory visual label: Work, Prod, Dev, Critical, or no label. Labels identify the tab only and do not alter SSH connection policy.

Tabs can be moved one position left or right. Checkboxes provide explicit multi-selection. Bulk actions support **Close selected**, **Close right**, and **Close others**. Bulk close always shows every affected tab, marks which sessions are still live, and requires confirmation before any PTY is dropped.

All close paths share one cleanup transition. A removed tab is deleted from the tab vector once, so its `TerminalBackend` is dropped once. Workspace-pane membership, selected-tab state, pending paste state, logging state, active focus, and synchronized-input targets are repaired at the same point. Any tab close disarms synchronized input before removal.

Split-pane headers visibly mark the keyboard-focused pane with **FOCUSED** so the input target is clear.

## Layout persistence

Workspace layouts are JSON schema version 1 and contain:

- split axis;
- one or two validated non-secret SSH session definitions;
- an explicit `reconnect_on_restore` permission flag.

Saving is atomic and bounded to 1 MiB. Loading a file only parses and validates metadata.
It never opens a connection. A loaded layout can only create PTYs after the user presses
**Restore & reconnect**, and that button is enabled only when the saved layout itself
contains `reconnect_on_restore: true`.

A configured startup workspace may load its **metadata** automatically, but startup never reconnects it. Creating PTYs still requires the separate explicit **Restore & reconnect** action and a saved layout with reconnect permission.

## Synchronized input

Synchronized input starts disarmed. Each destination pane must be selected explicitly.
At least two selected panes are required before the arm control becomes available, and
mirroring only occurs when the focused source pane is one of those selected targets.

While armed, the workspace displays the prominent text **SYNC INPUT ARMED**. Normal text,
single-line safe paste, Enter/Tab/Backspace and common navigation keys are mirrored.
Multiline paste remains subject to Feature #21's confirmation policy; a confirmed
multiline paste is mirrored only after the explicit confirmation.

Any change to the synchronized-input target set disarms synchronization and requires a fresh explicit arm action. Closing any tab also disarms synchronization before cleanup. Reconnecting a participating pane remains disarmed.

## Current scope

The first workspace implementation intentionally supports at most two simultaneous split
panes. Existing tabs outside the workspace remain available but are not synchronized
unless they are part of the current two-pane workspace.
