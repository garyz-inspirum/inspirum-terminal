# SSH-first roadmap

This checklist is a roadmap for the current early SSH terminal. It is not a WindTerm parity claim, and it is not a claim of full WindTerm parity. A checked item means only the limited statement in that item. Unchecked items are planned, blocked, or not yet run. SSH and terminal correctness come before additional protocols. The project remains Apache-2.0.

Legend: `[x]` present or evidenced only as that item states; `[ ]` planned, blocked, or not yet run.

## Evidence recorded with this docs snapshot

- [x] Source review of the implementation passed.
- [x] PR #5 native CI passed at `f52d6297c118da4b590526b8b027a6a8353656ff` on Linux x86_64, Windows x64, and macOS Apple Silicon.
- [x] Linux CI additionally passed the isolated authenticated `sshd` fixture for public-key login, I/O, resize, changed-host-key rejection, exit, and disconnect cleanup.
- [x] Windows CI passed the native PTY child-argument-boundary test and actual OpenSSH child-exit smoke test.
- [x] macOS Apple Silicon CI passed native build/tests/clippy and the OpenSSH child-exit smoke test.
- [x] PR #30 profile-import safety regressions passed all three native targets in CI run `37289535816` at `f5662def628728735eaef7caa3d66b388e90dc06`.
- [x] PR #32 SFTP policy and IPv6 argument regressions passed all three native targets in CI run `37290183042` at `03d07bf50f0c9d3e6eef4edb0b13cbc15cd5e7db`; Linux additionally passed three SFTP policy/trust fixture tests. This is not IPv6 network or Windows/macOS authenticated-server acceptance.
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
- [x] Public-key, password, keyboard-interactive/MFA and `IdentitiesOnly` profile policy controls with Inherit/Enable/Disable semantics. Authentication prompts and secrets remain inside OpenSSH; complete cross-platform method acceptance remains pending.
- [x] GSSAPI authentication and credential-delegation profile policy controls with Inherit/Enable/Disable semantics and an explicit delegation risk warning. Runtime support is platform/OpenSSH-build dependent and capability acceptance remains pending.
- [x] Linux CI run #78 at `1a8716d0d9ed45e3c1d43c1db5918cc5725b2186` verified password authentication, encrypted private key, SSH agent, public-key + keyboard-interactive PAM MFA, disabled-method negative cases, password-prompt cancellation, and stalled-handshake timeout through the application PTY path.
- [ ] GSSAPI authentication acceptance with a real Kerberos realm, plus isolated authenticated-server acceptance for applicable methods on Windows and macOS.
- [ ] ControlMaster/multiplexing workflow and platform limitations.
- [x] Structured no-auth HTTP CONNECT and SOCKS5 proxy workflows use the built-in Inspirum proxy helper; Linux fixture coverage verifies actual HTTP proxy routing and hard failure without direct fallback. Proxy authentication, raw ProxyCommand compatibility and Windows/macOS authenticated-proxy acceptance remain open (#14/#35).
- [x] Optional per-profile remote command executed by OpenSSH after authentication; it is stored as profile metadata, passed after the SSH destination, and never invoked through a local shell. Full remote-command acceptance remains pending.
- [x] Tmux-aware integration with explicit discovery, selectable attach, explicit create, disconnect-as-detach behavior, attach-only reconnect, no production kill action, portable policy tests and isolated Linux tmux lifecycle acceptance (#19).
- [x] Ask-before-trusting and already-trusted-only host-key modes.
- [x] Host-key trust panel resolves effective `Hostname`/`Port`/`HostKeyAlias` with `ssh -G`, inspects `known_hosts` via `ssh-keygen -F`, and requires explicit confirmation before `ssh-keygen -R` removal. It does not auto-accept replacement keys; custom `UserKnownHostsFile` paths must be selected explicitly.
- [x] Separate argv construction without a shell.
- [x] Disposable Linux sshd fixture covers public-key session I/O, resize, changed-host-key rejection, exit, disconnect cleanup, encrypted-key and agent authentication, password/PAM-MFA policy cases, password-prompt cancellation, and stalled-handshake timeout. Password/PAM cases use disposable users only when passwordless sudo is available; CI run #78 passed all 15 fixture tests. Windows/macOS isolated-server acceptance remains pending.
- [x] Identity-file path editor plus validated profile persistence; private-key contents and passphrases are never stored. Linux encrypted-key and agent authentication are verified; Windows/macOS isolated auth acceptance remains pending.
- [x] ProxyJump profile/UI and discrete `-J` argv support. ProxyCommand UI and safe preview remain planned.
- [x] Local, remote, and dynamic forwarding profile/UI with discrete `-L`/`-R`/`-D` argv and `ExitOnForwardFailure=yes`. Complete native forwarding acceptance and richer lifecycle controls remain pending.
- [x] Agent forwarding and X11 forwarding profile/UI with Inherit/Enable/Disable policy and explicit risk text. Native acceptance remains pending.
- [x] Connection timeout, server keepalive interval, and compression profile/UI; compression also has Inherit/Enable/Disable policy. Cipher/algorithm policy UI remains planned.
- [x] Explicit reconnect button for an exited SSH tab; reconnect starts a fresh OpenSSH/PTy session from the tab's original profile and does not replay terminal input. Network-loss and host-key-change reconnect acceptance remains pending.
- [x] Headless and graphical support diagnostics share one privacy-safe report core: local OpenSSH/platform capability probes, allowlisted current app launch-policy summary, bounded sanitized in-memory recent-error categories, deterministic/redaction tests and no-clobber export. No passwords, passphrases, private-key material, authentication responses, arbitrary environment dump, terminal contents or raw error text are included (#20/#33). See [diagnostics](diagnostics.md).
- [ ] Native Windows and macOS isolated SSH-server acceptance tests for authentication, trust, ProxyJump/forwarding, and lifecycle behavior.

## Sessions and workspace

- [x] Non-secret JSON profiles with atomic replacement.
- [x] Saved profile selection and update by profile name.
- [x] Multiple terminal tabs and explicit close/disconnect.
- [x] Search, rename-on-save, editable duplicate, and confirmed delete for saved profiles; operations preserve atomic persistence and do not disconnect already-open tabs.
- [x] Validated non-secret JSON profile import/export with collision-safe merge and confirmed replace; missing import sources are errors and the complete merged candidate must fit the file-size limit before any store change. Imported local identity paths may require adjustment on another machine.
- [ ] Group and tag profiles.
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

- [x] Interactive SFTP terminal tabs backed by the system OpenSSH `sftp` client, reusing host trust, SSH config, identity, ProxyJump and connection-policy inputs. A disposable Linux upload/download fixture is present; green verification is tracked separately.
- [x] All six explicit authentication policies apply to SFTP; inherited policies remain inherited. IPv6/scoped IPv6 destinations are bracketed for the SFTP grammar. Portable regressions and Linux success/disabled-key/changed-trust fixtures passed in PR #32; broader platform acceptance remains open. See [SFTP](sftp.md).
- [x] Integrated graphical SFTP browser, transfer queue, progress state, cancel/retry, and overwrite/conflict UX. Resume remains a broader transfer-workflow roadmap item.
- [x] Explicit SCP upload/download through system OpenSSH with shared trust/auth/proxy policy, staged no-partial success semantics, overwrite safeguards, portable argv tests and Linux binary SHA-256 acceptance (#18).
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

- [x] Structured ControlMaster Inherit/Disabled/Auto policy, explicit ControlPath/ControlPersist, status/close lifecycle controls, portable argv tests and Linux disposable-sshd lifecycle acceptance (#15).

- [x] First-class SSH tunnel manager lifecycle for profile local/remote/dynamic forwards, including live per-forward status, listener startup failure reporting, explicit stop/cleanup, non-loopback risk acknowledgement, portable policy tests and Linux disposable-sshd acceptance (#16).

- [x] Graphical SFTP browser and transfer queue with local/remote navigation, remote mutation, explicit overwrite handling, staged verified downloads, verified uploads, progress state, cancel/retry and isolated binary-transfer/negative acceptance (#17).
