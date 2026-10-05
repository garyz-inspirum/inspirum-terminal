# SSH-first roadmap

This checklist is a roadmap for the current early SSH terminal. It is not a WindTerm parity claim, and it is not a claim of full WindTerm parity. A checked item means only the limited statement in that item. Unchecked items are planned, blocked, or not yet run. SSH and terminal correctness come before additional protocols. The project remains Apache-2.0.

Legend: `[x]` present or evidenced only as that item states; `[ ]` planned, blocked, or not yet run.

## Evidence recorded with this docs snapshot

- [x] Source review of the implementation passed.
- [x] PR #5 native CI passed at `f52d6297c118da4b590526b8b027a6a8353656ff` on Linux x86_64, Windows x64, and macOS Apple Silicon.
- [x] Linux CI additionally passed the isolated authenticated `sshd` fixture for public-key login, I/O, resize, changed-host-key rejection, exit, and disconnect cleanup.
- [x] Windows CI passed the native PTY child-argument-boundary test and actual OpenSSH child-exit smoke test.
- [x] macOS Apple Silicon CI passed native build/tests/clippy and the OpenSSH child-exit smoke test.
- [x] License closure for Linux and Windows passed existing evidence.
- [x] CI and release workflows pin Rust 1.95.0. Their gates are unchanged by this documentation.
- [ ] Isolated authenticated SSH-server acceptance on Windows and macOS.
- [ ] macOS distribution. The active `dispatch 0.2.0` notice still blocks distribution, not compilation. The defined macOS artifact is a bare unsigned, unnotarized executable, not an app bundle.
- [ ] Full WindTerm parity. Not claimed.


## Foundation and release engineering

- [x] Native Rust/egui application shell.
- [x] Apache-2.0 repository license metadata and license text.
- [x] Locked dependency graph.
- [x] CI definitions for Linux x64, Windows x64, and macOS Apple Silicon, pinned to Rust 1.95.0. Definitions are not successful runs.
- [x] Release workflow definition for archives, README, full Apache-2.0 license, notices, and SHA-256 manifest. The macOS path packages a bare executable, not an app bundle.
- [x] Successful native CI run on Linux x64, Windows x64, and macOS Apple Silicon.
- [ ] Successful release run and downloaded artifact verification.
- [x] Native Windows x64 CI execution, including PTY argv-boundary and OpenSSH child-exit smoke tests; broader SSH acceptance is still pending.
- [x] Native macOS Apple Silicon CI execution, including the OpenSSH child-exit smoke test; broader SSH acceptance is still pending.
- [ ] macOS distribution while the `dispatch 0.2.0` notice remains unresolved. Compilation is not blocked.
- [ ] Code signing, Windows reputation, macOS signing, and notarization.
- [ ] Installer packages and automated update policy.
- [ ] Accessibility and localization audits.

## SSH connection core

- [x] System OpenSSH process in a native PTY.
- [x] Host/config alias, username, port, and optional config path.
- [x] Delegate identity/agent/authentication prompts to OpenSSH (not a claim every method is tested).
- [ ] Password, encrypted private keys, keyboard-interactive/MFA and agent authentication acceptance tests.
- [ ] GSSAPI authentication and credential-delegation policy, with explicit platform capability checks.
- [ ] ControlMaster/multiplexing workflow and platform limitations.
- [ ] HTTP and SOCKS proxy workflows and no-direct-fallback verification.
- [ ] SSH auto-execution after authentication, with explicit user opt-in.
- [ ] Tmux-aware integration (SSH-first scope; ordinary tmux inside a terminal is not equivalent).
- [x] Ask-before-trusting and already-trusted-only host-key modes.
- [x] Separate argv construction without a shell.
- [x] Disposable Linux sshd fixture: 3 ignored tests verified for public-key session I/O, resize, changed-host-key rejection, exit, and disconnect cleanup. Not password, MFA, or Windows/macOS acceptance.
- [x] Identity-file path editor plus validated profile persistence; private-key contents and passphrases are never stored. Native auth acceptance is still pending.
- [x] ProxyJump profile/UI and discrete `-J` argv support. ProxyCommand UI and safe preview remain planned.
- [x] Local, remote, and dynamic forwarding profile/UI with discrete `-L`/`-R`/`-D` argv and `ExitOnForwardFailure=yes`. Native forwarding acceptance and richer lifecycle controls remain pending.
- [x] Agent forwarding and X11 forwarding profile/UI with Inherit/Enable/Disable policy and explicit risk text. Native acceptance remains pending.
- [x] Connection timeout, server keepalive interval, and compression profile/UI; compression also has Inherit/Enable/Disable policy. Cipher/algorithm policy UI remains planned.
- [ ] Reconnect behavior with explicit user control.
- [ ] Connection diagnostics and sanitized support bundle.
- [ ] Native Windows and macOS isolated SSH-server acceptance tests for authentication, trust, ProxyJump/forwarding, and lifecycle behavior.

## Sessions and workspace

- [x] Non-secret JSON profiles with atomic replacement.
- [x] Saved profile selection and update by profile name.
- [x] Multiple terminal tabs and explicit close/disconnect.
- [ ] Delete, duplicate, rename, group, tag, search, and import/export profiles.
- [ ] Split panes and flexible layouts.
- [ ] Restore selected layouts with opt-in reconnect.
- [ ] Tab search, tab color, close-right/others, and bulk actions.
- [ ] Sync input with prominent target and safety controls.
- [ ] Startup session selection.

## Terminal experience

- [x] `egui_term` terminal rendering backed by the Alacritty parser.
- [x] PTY resize propagation exercised on Linux by the fixture. Not Windows or macOS execution.
- [ ] Cross-platform keyboard/IME audit.
- [ ] Selection, copy, paste, and safe paste confirmation policy.
- [ ] Search, marks, timestamps, folding, and outlining.
- [ ] Configurable fonts, colors, themes, opacity, and cursor.
- [ ] Mouse protocol and alternate-screen compatibility matrix.
- [ ] Unicode, emoji, wide-character, combining-character, and bidi test matrix.
- [ ] VT/xterm compatibility suite and published results.
- [ ] Performance and memory benchmarks with reproducible workloads.
- [ ] Session logging with secret/redaction policy.
- [ ] Command palette, command sender, and quick bar.
- [ ] Local/remote editing modes, completion, and snippets.

## Files and remote workflows

- [ ] Integrated SFTP browser and transfers.
- [ ] SCP operations.
- [ ] Local file browser and drag/drop policy.
- [ ] Transfer queue, progress, resume, conflict handling, and integrity checks.
- [ ] Remote editor workflow with safe temporary-file handling.

## Additional protocols — only after SSH quality gates

- [ ] Local shell profiles.
- [ ] Telnet.
- [ ] Raw TCP.
- [ ] Serial.
- [ ] ZModem/XModem/YModem transfer compatibility.

## Additional WindTerm-class UI roadmap

Unchecked items below are not implemented and are not a WindTerm parity claim.

- [ ] Free-type and focus modes; local/remote vim-style keyboard modes.
- [ ] Explorer/shell panes, enhanced paste dialog, screen lock.
- [ ] Delimiter highlighting, configurable syntax/color schemes and online text search.
- [ ] Select-to-copy and configurable middle/right-click paste.
- [ ] Hide pointer while typing, layout persistence and session restoration.
- [ ] Complete advertised WindTerm feature inventory reconciled against the [source README](https://github.com/garyz-inspirum/WindTerm/blob/master/README.md); any proposed exclusion requires an explicit scope decision.

## Quality gates before calling a feature supported

- [ ] User-visible behavior documented in the same change.
- [ ] Automated tests cover policy and failure paths.
- [ ] Native runtime validation on every claimed platform.
- [ ] Security boundaries and stored data documented.
- [ ] Resource cleanup verified for success, failure, cancel, and window close.
- [ ] Accessibility and keyboard-only behavior reviewed.
- [ ] No unsupported feature is implied by marketing or release notes.
