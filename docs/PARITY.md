# SSH-first roadmap

This checklist is the roadmap for the current SSH-first release-candidate baseline. It is not a WindTerm parity claim, and it is not a claim of full WindTerm parity. A checked item means only the limited statement in that item. Unchecked items are planned, blocked, or not yet run. SSH and terminal correctness come before additional protocols. The project remains Apache-2.0.

Legend: `[x]` present or evidenced only as that item states; `[ ]` planned, blocked, or not yet run.

## Evidence recorded with this docs snapshot

- [x] Original SSH-first release baseline: `17d353d3166f8c5a85f44329114530d3338462c9`. This is historical, **not** the current main commit.
- [x] Post-merge CI run #214 (`37443065634`) passed Linux x86_64, Windows x64, and macOS Apple Silicon.
- [x] Native authenticated SSH smoke passes on all three targets through the real Inspirum -> system OpenSSH path, covering public-key authentication, terminal I/O, PTY resize, changed-host-key rejection before authentication, reconnect, and cleanup.
- [x] Linux additionally passes the deeper disposable `sshd` integration suite covering password, encrypted private key, SSH agent, public-key + keyboard-interactive MFA, disabled-method negative cases, prompt cancellation, timeout, forwarding, SFTP/SCP, tmux, and transfer workflows.
- [x] Target-specific dependency notice collection passes on Linux, Windows, and Apple Silicon. The former macOS crates.io `dispatch 0.2.0` blocker is no longer in the distributed closure; the compatibility edge is supplied by the project-owned Apache-2.0 `vendor/dispatch-compat` patch.
- [x] CI builds and uploads release-candidate archives for all three targets, then separate native jobs download, clean-extract, architecture-inspect, metadata-check, and execute each packaged binary with `--version`.
- [x] Packaged architecture is verified from executable bytes: ELF x86_64, PE32+ AMD64, and thin Mach-O ARM64.
- [x] CI and release workflows pin Rust 1.95.0.
- [ ] Releases remain unsigned/unnotarized until credential-backed signing is enabled and independently verified.
- [x] Isolated Linux Kerberos/GSSAPI runtime acceptance (authentication, delegation on/off, missing and expired tickets) merged in #89/#92. macOS/Windows runtime GSSAPI has **not** been verified. Issue #65 is closed with these documented limitations.
- [x] Advanced SSH acceptance (#66): ProxyJump and TCP -L/-R/-D pass on Linux/macOS/Windows with failure, trust, reconnect and teardown checks; real agent forwarding passes on Linux and macOS with enable/disable/re-enable isolation; Linux X11 passes through Xvfb/xauth. Windows agent and Windows/macOS X11 remain explicitly unverified in the connection UI and documentation, so no runtime support is claimed.
- [ ] Full WindTerm parity. Not claimed.

## Updated native acceptance evidence — 10 October 2026

**Production GUI:** Iced 0.14 is the sole application UI since merged #84; egui/eframe fallback is removed. Focus, privacy and paste guards are tested. PR #95 enables Metal/WGPU with tiny-skia fallback on Apple Silicon with full license-notice and downloaded-artifact CI checks, but **a compiled renderer is not evidence of lower click latency**. Hardware click-to-caret, native IME/CJK, connected SSH full-screen apps, split panes and SFTP visuals remain open in #78.

**Advanced SSH:** #88 verified ProxyJump routing with changed-key/failing-jump rejection and real local/remote/SOCKS forwarding, same-port reconnect and listener cleanup on Linux x64, Windows x64 and macOS arm64. #93 verified Linux agent forwarding; the native Unix fixture also verifies enable → disable → re-enable, identity visibility and session cleanup on Linux and macOS. #99 verified X11 forwarding on/off through Linux Xvfb/xauth with a system OpenSSH control. The UI explicitly labels Windows agent and Windows/macOS X11 runtime as unverified, and those platforms are not claimed. This completes bounded issue #66 without extrapolating unsupported runtime evidence. Linux Kerberos/GSSAPI (#89/#92) remains a Linux-only runtime claim.

**Interactions:** #86 privacy view, #87 multiline paste confirmation, #90 focus mode, #91 isolated local navigation, #97 optional vi bindings and #98 nested delimiter matching are merged. Free-type local draft (#100) is a separate in-review PR and must not be counted as merged before green CI.

**Scope still open:** #64 interaction/explorer parity, #78 live GUI usability and #1 full WindTerm functional parity including local shell, Telnet, raw TCP, serial and X/Y/ZModem. Unsigned prerelease packages must not be described as signed or notarized.

## Foundation and release engineering

- [x] Native Rust/Iced application shell.
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
- [x] GSSAPI authentication and credential-delegation profile policy controls with Inherit/Enable/Disable semantics and explicit delegation risk warning. Linux real-realm authentication/delegation and absent/expired ticket failure tested in #89/#92; other OSes not runtime verified.
- [x] Linux CI run #78 at `1a8716d0d9ed45e3c1d43c1db5918cc5725b2186` verified password authentication, encrypted private key, SSH agent, public-key + keyboard-interactive PAM MFA, disabled-method negative cases, password-prompt cancellation, and stalled-handshake timeout through the application PTY path.
- [x] Linux real Kerberos/GSSAPI acceptance complete in #89/#92; cross-platform non-GSSAPI authentication/trust/lifecycle smoke passes on all three platforms. See closed #65 for Linux-only runtime caveat.
- [ ] ControlMaster/multiplexing workflow and platform limitations.
- [x] Structured no-auth HTTP CONNECT and SOCKS5 proxy workflows use the built-in Inspirum proxy helper; Linux fixture coverage verifies actual HTTP proxy routing and hard failure without direct fallback. Proxy authentication, raw ProxyCommand compatibility and Windows/macOS authenticated-proxy acceptance remain open (#14/#35).
- [x] Optional per-profile remote command executed by OpenSSH after authentication; it is stored as profile metadata, passed after the SSH destination, and never invoked through a local shell. Full remote-command acceptance remains pending.
- [x] Tmux-aware integration with explicit discovery, selectable attach, explicit create, disconnect-as-detach behavior, attach-only reconnect, no production kill action, portable policy tests and isolated Linux tmux lifecycle acceptance (#19).
- [x] Ask-before-trusting and already-trusted-only host-key modes.
- [x] Host-key trust panel resolves effective `Hostname`/`Port`/`HostKeyAlias` with `ssh -G`, inspects `known_hosts` via `ssh-keygen -F`, and requires explicit confirmation before `ssh-keygen -R` removal. It does not auto-accept replacement keys; custom `UserKnownHostsFile` paths must be selected explicitly.
- [x] Separate argv construction without a shell.
- [x] Disposable Linux sshd fixture covers public-key session I/O, resize, changed-host-key rejection, exit, disconnect cleanup, encrypted-key and agent authentication, password/PAM-MFA policy cases, password-prompt cancellation, and stalled-handshake timeout. Cross-platform native public-key authentication/trust/lifecycle smoke additionally passes on Windows and macOS.
- [x] Identity-file path editor plus validated profile persistence; private-key contents and passphrases are never stored. Linux encrypted-key and agent authentication are verified; cross-platform native public-key authentication/trust/lifecycle smoke passes on all three targets.
- [x] ProxyJump profile/UI and discrete `-J` argv support with isolated native three-platform jump routing and no-direct-fallback negative checks in #88. Structured proxy controls are separately implemented.
- [x] Local, remote, and dynamic forwarding profile/UI with discrete `-L`/`-R`/`-D`, `ExitOnForwardFailure=yes`, and real TCP/SOCKS bytes plus listener cleanup on Linux/macOS/Windows in #88.
- [x] Agent/X11 forwarding UI has Inherit/Enable/Disable policy and risk text. Disposable Linux ssh-agent forwarding (#93) and Xvfb/xauth X11 forwarding (#99) runtime opt-in/opt-out passed; cross-platform runtime tests remain pending under #66.
- [x] Connection timeout, server keepalive interval, and compression profile/UI; compression also has Inherit/Enable/Disable policy. Cipher/algorithm policy UI remains planned.
- [x] Explicit reconnect button for an exited SSH tab; reconnect starts a fresh OpenSSH/PTy session from the tab's original profile and does not replay terminal input. Network-loss and host-key-change reconnect acceptance remains pending.
- [x] Headless and graphical support diagnostics share one privacy-safe report core: local OpenSSH/platform capability probes, allowlisted current app launch-policy summary, bounded sanitized in-memory recent-error categories, deterministic/redaction tests and no-clobber export. No passwords, passphrases, private-key material, authentication responses, arbitrary environment dump, terminal contents or raw error text are included (#20/#33). See [diagnostics](diagnostics.md).
- [x] Native Windows/macOS authentication, trust, terminal I/O, resize, reconnect and cleanup smoke plus ProxyJump and -L/-R/-D forwarding byte roundtrips in #88. Agent and X11 runtime support on Windows/macOS is **not** verified (#66).

## Sessions and workspace

- [x] Non-secret JSON profiles with atomic replacement.
- [x] Saved profile selection and update by profile name.
- [x] Multiple terminal tabs and explicit close/disconnect.
- [x] Search, rename-on-save, editable duplicate, and confirmed delete for saved profiles; operations preserve atomic persistence and do not disconnect already-open tabs.
- [x] Validated non-secret JSON profile import/export with collision-safe merge and confirmed replace; missing import sources are errors and the complete merged candidate must fit the file-size limit before any store change. Imported local identity paths may require adjustment on another machine.
- [x] Folder/tag/favorite profile organization and startup-session selection completed under closed #55.
- [x] Bounded two-pane horizontal/vertical SSH splits with independent PTY lifecycle (#22).
- [x] Persisted workspace layouts load as metadata only and reconnect only after saved opt-in plus an explicit restore action (#22).
- [x] Advanced tab search/color and workspace actions completed under closed #56.
- [x] Synchronized input requires explicit pane targets plus a prominent armed state; close/reconnect disarms it and multiline paste remains confirmation-gated (#22).
- [x] Startup session selection implemented under closed #55.

## Terminal experience

- [x] Iced terminal rendering backed by the toolkit-neutral Alacritty terminal core.
- [x] PTY resize propagation is exercised by native authenticated SSH smoke on Linux, Windows and macOS.
- [x] Phase 3 keyboard/IME/focus/accessibility code audit closed as #59; **live native IME/CJK and physical Mac click-to-focus performance acceptance is still pending in #78**.
- [x] Selection/copy plus guarded paste policy: multiline CR/LF payloads cannot reach the PTY without explicit confirmation (or are blocked), with keyboard confirmation/cancel controls (#21).
- [x] Retained history navigation/search/marks/timestamps/folding completed under closed #57, with additional read-only vi scrollback in #97. Live native GUI acceptance remains #78.
- [x] Terminal appearance, fonts, colors, opacity, cursor and pointer settings implemented under closed #58. Native rendering/contrast approval remains #78.
- [x] Bounded alternate-screen (`?1049`) and SGR mouse-mode (`?1000`/`?1006`) compatibility regressions execute on Unix CI as part of #23; this is not a complete mouse-protocol matrix.
- [x] Bounded Unicode regression covers CJK wide cells, combining-mark input and emoji without replacement-character corruption on the real Unix PTY/grid path (#23). Bidi, IME and font-shaping audits remain open.
- [x] Bounded VT/xterm compatibility suite covers alternate screen, SGR mouse mode, cursor-back overwrite and erase-line behavior, with commands/results scope documented in `terminal-compatibility.md` (#23).
- [x] Reproducible 5,000-line high-volume scrollback regression plus opt-in 20,000-line release benchmark and `/usr/bin/time` memory wrapper are documented for #23; no universal latency or memory guarantee is claimed.
- [x] Opt-in per-tab screen-snapshot session logging is off by default, creates a new file without overwrite, never records local keystrokes, and documents that remote output can contain sensitive material (#21).
- [x] Command palette, explicit command sender, quick bar, snippets and completion implemented under closed #60.
- [x] Focus/scrollback modes (#90/#91/#97), preview/lock (#87/#86) and delimiter emphasis (#98) merged. Local free-type compose draft is under CI in #100; remote-cursor and explorer/shell parity remain #64.

## Files and remote workflows

- [x] Interactive SFTP terminal tabs backed by the system OpenSSH `sftp` client, reusing host trust, SSH config, identity, ProxyJump and connection-policy inputs. A disposable Linux upload/download fixture is present; green verification is tracked separately.
- [x] All six explicit authentication policies apply to SFTP; inherited policies remain inherited. IPv6/scoped IPv6 destinations are bracketed for the SFTP grammar. Portable regressions and Linux success/disabled-key/changed-trust fixtures passed in PR #32; broader platform acceptance remains open. See [SFTP](sftp.md).
- [x] Integrated graphical SFTP browser, transfer queue, progress state, cancel/retry/resume, and overwrite/conflict UX. Resume uses OpenSSH `reget`/`reput` with queue-owned partial state and final size verification.
- [x] Explicit SCP upload/download through system OpenSSH with shared trust/auth/proxy policy, staged no-partial success semantics, overwrite safeguards, portable argv tests and Linux binary SHA-256 acceptance (#18).
- [x] Local file manager and drag/drop integration implemented under closed #61.
- [x] Transfer queue, progress, resume, conflict handling, and integrity checks for the integrated SFTP workflow.
- [x] Safe remote editor workflow with temporary-file/conflict handling implemented under closed #62.

## Phase 3 — SSH workflow and product polish

Active bounded queue:

- [x] #55 SSH profile organization: folders, tags, favorites and startup sessions
- [x] #56 Advanced tab and workspace management
- [x] #57 Terminal history search, marks, timestamps, folding and navigation
- [x] #58 Terminal appearance, themes and pointer/copy interaction settings
- [x] #59 Cross-platform keyboard, IME, focus and accessibility audit
- [x] #60 Command palette, quick bar, command sender, snippets and completion
- [x] #61 Local file manager and drag/drop integration for SSH transfers
- [x] #62 Safe remote editor workflow over SFTP
- [x] #63 Advanced SSH algorithm policy and authenticated proxy support
- [ ] #64 WindTerm-class interaction modes, panes, enhanced paste and screen lock
- [x] #65 Real Kerberos/GSSAPI authentication acceptance (Linux isolated KDC; other OSes not runtime verified)
- [x] #66 Cross-platform advanced SSH acceptance: ProxyJump, forwarding, agent and X11

Phase 3 intentionally stays SSH-first. Local shell, Telnet, raw TCP, serial and X/Y/ZModem begin only after these product-polish and evidence gaps are completed or explicitly descoped.

## Additional protocols — only after SSH quality gates

- [ ] Local shell profiles.
- [ ] Telnet.
- [ ] Raw TCP.
- [ ] Serial.
- [ ] ZModem/XModem/YModem transfer compatibility.

## Additional WindTerm-class UI roadmap

Unchecked items below are not implemented and are not a WindTerm parity claim.

- [x] Alt+Enter focus (#90) and per-pane Shift+Enter local navigation (#91/#97) with vi-like scrollback and remote-input isolation. Inline local free-type composer is in PR #100 under CI; full remote cursor editing remains outside the implemented scope (#64).
- [x] Guarded paste preview/optional newline normalization (#87) and in-app privacy curtain (#86) have verified Linux/Windows/macOS CI. Explorer/shell side-pane parity remains incomplete (#64).
- [x] Visible nested-bracket matching emphasis and targeted iced row-cache invalidation (#98). General text highlighting beyond existing search and offscreen delimiter matching remain tracked in #64/#57.
- [x] Select-to-copy and configurable middle/right-click paste implemented under closed #58; verify live OS-specific mouse acceptance under GUI issue #78.
- [x] Hide-pointer setting and advanced workspace/tab management implemented under closed #58/#56; device-specific rendering/focus acceptance remains #78.
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
