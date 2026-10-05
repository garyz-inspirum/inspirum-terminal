# tmux-aware SSH sessions

Inspirum provides explicit tmux session discovery, attach and create actions for SSH profiles. Ordinary SSH **Connect** does not inspect, attach, create, detach, rename or destroy tmux sessions.

## Server requirements

The remote host must provide `tmux` on the account's normal `PATH`. The account must be permitted to allocate a PTY for attach/create operations.

**Refresh tmux** uses a short non-interactive SSH command so the UI can capture the session list. That refresh therefore requires non-interactive authentication such as a usable key/agent or an already established ControlMaster. Interactive password, passphrase, MFA and first-host-trust prompts are not displayed by the refresh operation.

Attach and Create use the normal Inspirum SSH PTY, so normal interactive OpenSSH authentication remains available for those actions.

A profile-level custom remote command is intentionally incompatible with the tmux controls. Clear that command before using tmux integration so Inspirum never silently replaces user-configured remote-command intent.

## Explicit behavior

- **Refresh tmux** runs a fixed remote probe that lists session name and attached-client count.
- **Attach selected** attaches only the selected existing session.
- **Create tmux** creates only the explicitly entered session name.
- Inspirum does not expose `kill-session` or `kill-server` actions.
- Supported session names are deliberately conservative: 1-64 ASCII letters, digits, `_` or `-`, and they may not start with `-`. This keeps generated remote commands unambiguous and prevents user text from becoming remote shell syntax.

The discovery probe contains fixed shell syntax because OpenSSH remote commands are interpreted by the remote account's shell. No user-supplied session name is inserted into that probe. Attach/create commands contain only session names accepted by the conservative validator. Inspirum does not invoke a **local** shell.

## Disconnect, detach and reconnect semantics

Closing an Inspirum tmux terminal closes the SSH/tmux client. The tmux server-side session remains running; Inspirum does not send a destructive tmux command.

When a tmux tab is initially created, the live connection uses `tmux new-session -s NAME`, but the tab stores `tmux attach-session -t NAME` as its reconnect action. Therefore **Reconnect never recreates a missing tmux session implicitly**. If the server-side session was removed independently, reconnect fails and the user must explicitly choose Create again.

## Verification

Portable tests validate session-name restrictions, non-destructive attach/create command construction, parsing of selectable session state, and the non-interactive discovery argv.

The isolated Linux SSH fixture installs tmux and starts a dedicated loopback sshd without a ForceCommand. Acceptance creates and attaches a unique tmux session, verifies discovery reports it attached, closes the SSH backend and verifies the tmux session remains detached, attaches it again, repeats attach-only reconnect, and finally performs an explicit fixture-only cleanup. The production application has no cleanup/kill command.

Windows x64 and macOS Apple Silicon CI compile and run the portable tests natively. The isolated authenticated tmux lifecycle fixture currently runs on Linux only.
