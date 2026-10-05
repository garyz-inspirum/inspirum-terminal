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
