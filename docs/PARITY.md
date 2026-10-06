# SSH-first roadmap

This checklist is the roadmap for the current SSH-first release-candidate baseline. It is not a WindTerm parity claim, and it is not a claim of full WindTerm parity. A checked item means only the limited statement in that item. Unchecked items are planned, blocked, or not yet run. SSH and terminal correctness come before additional protocols. The project remains Apache-2.0.

Legend: `[x]` present or evidenced only as that item states; `[ ]` planned, blocked, or not yet run.

## Evidence recorded with this docs snapshot

- [x] Current `main` baseline is merge commit `17d353d3166f8c5a85f44329114530d3338462c9`.
- [x] Post-merge CI run #214 (`37443065634`) passed Linux x86_64, Windows x64, and macOS Apple Silicon.
- [x] Native authenticated SSH smoke passes on all three targets through the real Inspirum -> system OpenSSH path, covering public-key authentication, terminal I/O, PTY resize, changed-host-key rejection before authentication, reconnect, and cleanup.
- [x] Linux additionally passes the deeper disposable `sshd` integration suite covering password, encrypted private key, SSH agent, public-key + keyboard-interactive MFA, disabled-method negative cases, prompt cancellation, timeout, forwarding, SFTP/SCP, tmux, and transfer workflows.
- [x] Target-specific dependency notice collection passes on Linux, Windows, and Apple Silicon. The former macOS crates.io `dispatch 0.2.0` blocker is no longer in the distributed closure; the compatibility edge is supplied by the project-owned Apache-2.0 `vendor/dispatch-compat` patch.
- [x] CI builds and uploads release-candidate archives for all three targets, then separate native jobs download, clean-extract, architecture-inspect, metadata-check, and execute each packaged binary with `--version`.
- [x] Packaged architecture is verified from executable bytes: ELF x86_64, PE32+ AMD64, and thin Mach-O ARM64.
- [x] CI and release workflows pin Rust 1.95.0.
- [ ] Releases remain unsigned/unnotarized until credential-backed signing is enabled and independently verified.
- [ ] Real Kerberos/GSSAPI acceptance remains Phase 3 work (#65); support depends on the installed OpenSSH/platform environment.
- [ ] Advanced cross-platform ProxyJump/forwarding/agent/X11 runtime acceptance remains Phase 3 work (#66).
- [ ] Full WindTerm parity. Not claimed.

## Foundation and release engineering

- [x] Native Rust/egui application shell.
- [x] Apache-2.0 repository license metadata and license text.
- [x] Locked dependency graph.
- [x] CI definitions and successful native execution for Linux x64, Windows x64, and macOS Apple Silicon, pinned to Rust 1.95.0.
- [x] Release workflow definition for archives, README, full Apache-2.0 license, target-specific notices, release metadata, and SHA-256 manifest.
- [x] Release-candidate packaging and downloaded-artifact verification on all three native targets.
- [x] Native Windows x64 SSH acceptance for authentication, terminal I/O, resize, host-key rejection, reconnect and cleanup.
- [x] Native macOS Apple Silicon SSH acceptance for authentication, terminal I/O, resize, host-key rejection, reconnect and cleanup.
- [x] macOS dependency-notice closure passes; the former `dispatch 0.2.0` distribution blocker is removed from the distributed dependency closure.
- [ ] Code signing, Windows publisher reputation, macOS Developer ID signing and notarization. Hooks/readiness are implemented, credentials are not configured.
- [ ] Installer/app-bundle packages and automated update policy.
- [ ] Accessibility/localization audit (#59 plus later multilingual work).

## SSH connection core

- [x] System OpenSSH process in a native PTY.
- [x] Host/config alias, username, port, and optional config path.
- [x] Delegate identity/agent/authentication prompts to OpenSSH (not a claim every method is tested).
- [x] Public-key, password, keyboard-interactive/MFA and `IdentitiesOnly` profile policy controls with Inherit/Enable/Disable semantics. Authentication prompts and secrets remain inside OpenSSH; native public-key acceptance passes on all three platforms while the deeper password/MFA matrix remains Linux-focused.
- [x] GSSAPI authentication and credential-delegation profile policy controls with Inherit/Enable/Disable semantics and an explicit delegation risk warning. Runtime support is platform/OpenSSH-build dependent and capability acceptance remains pending.
- [x] Linux CI run #78 at `1a8716d0d9ed45e3c1d43c1db5918cc5725b2186` verified password authentication, encrypted private key, SSH agent, public-key + keyboard-interactive PAM MFA, disabled-method negative cases, password-prompt cancellation, and stalled-handshake timeout through the application PTY path.
- [ ] Real Kerberos/GSSAPI authentication acceptance is tracked by #65. Native non-GSSAPI authentication/trust/lifecycle smoke passes on all three platforms.
- [ ] ControlMaster/multiplexing workflow and platform limitations.
- [x] Structured no-auth HTTP CONNECT and SOCKS5 proxy workflows use the built-in Inspirum proxy helper; Linux fixture coverage verifies actual HTTP proxy routing and hard failure without direct fallback. Proxy authentication, raw ProxyCommand compatibility and Windows/macOS authenticated-proxy acceptance remain open (#14/#35).
- [x] Optional per-profile remote command executed by OpenSSH after authentication; it is stored as profile metadata, passed after the SSH destination, and never invoked through a local shell. Full remote-command acceptance remains pending.
- [x] Tmux-aware integration with explicit discovery, selectable attach, explicit create, disconnect-as-detach behavior, attach-only reconnect, no production kill action, portable policy tests and isolated Linux tmux lifecycle acceptance (#19).
- [x] Ask-before-trusting and already-trusted-only host-key modes.
- [x] Host-key trust panel resolves effective `Hostname`/`Port`/`HostKeyAlias` with `ssh -G`, inspects `known_hosts` via `ssh-keygen -F`, and requires explicit confirmation before `ssh-keygen -R` removal. It does not auto-accept replacement keys; custom `UserKnownHostsFile` paths must be selected explicitly.
- [x] Separate argv construction without a shell.
- [x] Disposable Linux sshd fixture covers public-key session I/O, resize, changed-host-key rejection, exit, disconnect cleanup, encrypted-key and agent authentication, password/PAM-MFA policy cases, password-prompt cancellation, and stalled-handshake timeout. Cross-platform native public-key authentication/trust/lifecycle smoke additionally passes on Windows and macOS.
- [x] Identity-file path editor plus validated profile persistence; private-key contents and passphrases are never stored. Linux encrypted-key and agent authentication are verified; cross-platform native public-key authentication/trust/lifecycle smoke passes on all three targets.
- [x] ProxyJump profile/UI and discrete `-J` argv support. ProxyCommand UI and safe preview remain planned.
- [x] Local, remote, and dynamic forwarding profile/UI with discrete `-L`/`-R`/`-D` argv and `ExitOnForwardFailure=yes`. Complete native forwarding acceptance and richer lifecycle controls remain pending.
- [x] Agent forwarding and X11 forwarding profile/UI with Inherit/Enable/Disable policy and explicit risk text. Native acceptance remains pending.
- [x] Connection timeout, server keepalive interval, and compression profile/UI; compression also has Inherit/Enable/Disable policy. Cipher/algorithm policy UI remains planned.
- [x] Explicit reconnect button for an exited SSH tab; reconnect starts a fresh OpenSSH/PTy session from the tab's original profile and does not replay terminal input. Network-loss and host-key-change reconnect acceptance remains pending.
- [x] Headless and graphical support diagnostics share one privacy-safe report core: local OpenSSH/platform capability probes, allowlisted current app launch-policy summary, bounded sanitized in-memory recent-error categories, deterministic/redaction tests and no-clobber export. No passwords, passphrases, private-key material, authentication responses, arbitrary environment dump, terminal contents or raw error text are included (#20/#33). See [diagnostics](diagnostics.md).
- [x] Native Windows/macOS authentication, trust, terminal I/O, resize, reconnect and cleanup smoke. Advanced ProxyJump/forwarding/agent/X11 cross-platform evidence remains tracked by #66.

## Sessions and workspace

- [x] Non-secret JSON profiles with atomic replacement.
- [x] Saved profile selection and update by profile name.
- [x] Multiple terminal tabs and explicit close/disconnect.
- [x] Search, rename-on-save, editable duplicate, and confirmed delete for saved profiles; operations preserve atomic persistence and do not disconnect already-open tabs.
- [x] Validated non-secret JSON profile import/export with collision-safe merge and confirmed replace; missing import sources are errors and the complete merged candidate must fit the file-size limit before any store change. Imported local identity paths may require adjustment on another machine.
- [ ] Group/tag/favorite profile organization and startup-session selection (#55).
- [x] Bounded two-pane horizontal/vertical SSH splits with independent PTY lifecycle (#22).
- [x] Persisted workspace layouts load as metadata only and reconnect only after saved opt-in plus an explicit restore action (#22).
- [ ] Tab search, tab color, close-right/others, reorder and bulk actions (#56).
- [x] Synchronized input requires explicit pane targets plus a prominent armed state; close/reconnect disarms it and multiline paste remains confirmation-gated (#22).
- [ ] Startup session selection is included in #55.

## Terminal experience

- [x] `egui_term` terminal rendering backed by the Alacritty parser.
- [x] PTY resize propagation is exercised by native authenticated SSH smoke on Linux, Windows and macOS.
- [ ] Cross-platform keyboard/IME/focus/accessibility audit (#59).
- [x] Selection/copy plus guarded paste policy: multiline CR/LF payloads cannot reach the PTY without explicit confirmation (or are blocked), with keyboard confirmation/cancel controls (#21).
- [ ] Retained-scrollback search/navigation, marks, timestamps and bounded folding (#57). Current-viewport case-insensitive search is already implemented.
- [ ] Configurable fonts, colors, themes, opacity, cursor and pointer/copy interaction settings (#58).
- [x] Bounded alternate-screen (`?1049`) and SGR mouse-mode (`?1000`/`?1006`) compatibility regressions execute on Unix CI as part of #23; this is not a complete mouse-protocol matrix.
- [x] Bounded Unicode regression covers CJK wide cells, combining-mark input and emoji without replacement-character corruption on the real Unix PTY/grid path (#23). Bidi, IME and font-shaping audits remain open.
- [x] Bounded VT/xterm compatibility suite covers alternate screen, SGR mouse mode, cursor-back overwrite and erase-line behavior, with commands/results scope documented in `terminal-compatibility.md` (#23).
- [x] Reproducible 5,000-line high-volume scrollback regression plus opt-in 20,000-line release benchmark and `/usr/bin/time` memory wrapper are documented for #23; no universal latency or memory guarantee is claimed.
- [x] Opt-in per-tab screen-snapshot session logging is off by default, creates a new file without overwrite, never records local keystrokes, and documents that remote output can contain sensitive material (#21).
- [ ] Command palette, command sender, quick bar, snippets and completion (#60).
- [ ] Free-type/focus/editing-mode workflows are tracked by #64; snippets/completion are tracked by #60.

## Files and remote workflows

- [x] Interactive SFTP terminal tabs backed by the system OpenSSH `sftp` client, reusing host trust, SSH config, identity, ProxyJump and connection-policy inputs. A disposable Linux upload/download fixture is present; green verification is tracked separately.
- [x] All six explicit authentication policies apply to SFTP; inherited policies remain inherited. IPv6/scoped IPv6 destinations are bracketed for the SFTP grammar. Portable regressions and Linux success/disabled-key/changed-trust fixtures passed in PR #32; broader platform acceptance remains open. See [SFTP](sftp.md).
- [x] Integrated graphical SFTP browser, transfer queue, progress state, cancel/retry/resume, and overwrite/conflict UX. Resume uses OpenSSH `reget`/`reput` with queue-owned partial state and final size verification.
- [x] Explicit SCP upload/download through system OpenSSH with shared trust/auth/proxy policy, staged no-partial success semantics, overwrite safeguards, portable argv tests and Linux binary SHA-256 acceptance (#18).
- [ ] Local file browser and drag/drop integration (#61).
- [x] Transfer queue, progress, resume, conflict handling, and integrity checks for the integrated SFTP workflow.
- [ ] Safe remote editor workflow with temporary-file/conflict handling (#62).

## Phase 3 — SSH workflow and product polish

Active bounded queue:

- [ ] #55 SSH profile organization: folders, tags, favorites and startup sessions
- [ ] #56 Advanced tab and workspace management
- [ ] #57 Terminal history search, marks, timestamps, folding and navigation
- [ ] #58 Terminal appearance, themes and pointer/copy interaction settings
- [ ] #59 Cross-platform keyboard, IME, focus and accessibility audit
- [ ] #60 Command palette, quick bar, command sender, snippets and completion
- [ ] #61 Local file manager and drag/drop integration for SSH transfers
- [ ] #62 Safe remote editor workflow over SFTP
- [ ] #63 Advanced SSH algorithm policy and authenticated proxy support
- [ ] #64 WindTerm-class interaction modes, panes, enhanced paste and screen lock
- [ ] #65 Real Kerberos/GSSAPI authentication acceptance
- [ ] #66 Cross-platform advanced SSH acceptance: ProxyJump, forwarding, agent and X11

Phase 3 intentionally stays SSH-first. Local shell, Telnet, raw TCP, serial and X/Y/ZModem begin only after these product-polish and evidence gaps are completed or explicitly descoped.

## Additional protocols — only after SSH quality gates

- [ ] Local shell profiles.
- [ ] Telnet.
- [ ] Raw TCP.
- [ ] Serial.
- [ ] ZModem/XModem/YModem transfer compatibility.

## Additional WindTerm-class UI roadmap

Unchecked items below are not implemented and are not a WindTerm parity claim.

- [ ] Free-type/focus and optional vim-style interaction modes (#64).
- [ ] Explorer/shell panes, enhanced paste dialog and screen lock (#64).
- [ ] Delimiter/text highlighting hooks are included in #64; terminal appearance is #58 and retained history search is #57.
- [ ] Select-to-copy and configurable middle/right-click paste (#58).
- [ ] Hide-pointer interaction setting (#58); advanced tab/workspace management and restoration polish (#56).
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

- [x] Graphical SFTP browser and transfer queue with local/remote navigation, remote mutation, explicit overwrite handling, staged verified downloads, verified uploads, progress state, cancel/retry/resume and isolated binary-transfer/negative acceptance (#17 plus #3 completion work).
