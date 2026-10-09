# Inspirum Terminal

Inspirum Terminal is a fully open-source Apache-2.0, cross-platform native SSH client at an SSH-first release-candidate stage. Its core SSH, transfer, workspace and native packaging workflows are functional and continuously verified, but it is not yet a complete terminal suite and does not claim full WindTerm feature parity.

Current scope:

- native Iced desktop window (the default build and launcher);
- Iced canvas terminal rendering with the shared `egui_term` PTY/state backend;
- SSH sessions, interactive SFTP tabs, a graphical SFTP browser/transfer queue, and explicit SCP upload/download operations launched through the system OpenSSH client tools;
- saved, non-secret connection profiles with search, rename-on-save, editable duplication, confirmed deletion, and validated JSON import/export;
  Export refuses to overwrite an existing destination; replace-import requires explicit confirmation.
- first-class profile controls for identity-file paths, ProxyJump, structured HTTP CONNECT/SOCKS5 proxy routing, app-managed OpenSSH ControlMaster multiplexing/lifecycle, authentication-method policy, GSSAPI delegation policy, `IdentitiesOnly`, agent/X11 forwarding, compression, connect timeout, keepalive, remote auto-command, and local/remote/dynamic port forwarding;
- multiple SSH/SFTP terminal tabs, explicit disconnect by closing a tab, explicit reconnect after a remote/session exit, and explicit tmux discovery/attach/create with attach-only reconnect;
- host-key trust tools that resolve the effective OpenSSH host identity with `ssh -G`, inspect trusted entries with `ssh-keygen -F`, and require explicit confirmation before `ssh-keygen -R` removal;
- privacy-safe diagnostics through both headless `--diagnostics` and the graphical Support diagnostics panel, with local OpenSSH/platform probes, allowlisted app-policy state, bounded sanitized in-memory error categories and no-clobber text export;
- OpenSSH configuration, key material, agent, authentication prompts, host-key database, proxy transport, and forwarding implementation remain owned by OpenSSH.

## Verification status

The current `main` release-candidate baseline is merge commit `17d353d3166f8c5a85f44329114530d3338462c9`. Post-merge CI run #214 (`37443065634`) passed the full native matrix on Linux x86_64, Windows x64, and macOS Apple Silicon.

Verified evidence:

- Linux x86_64: formatting, build/check, Rust tests, Clippy, target-specific third-party notice collection, native authenticated SSH smoke, the deeper disposable `sshd` integration suite, release-candidate packaging/upload, and downloaded clean-install verification passed.
- Windows x64: formatting, build/check, bounded Rust tests, Clippy, target-specific third-party notice collection, native authenticated SSH smoke, release-candidate packaging/upload, and downloaded clean-install verification passed.
- macOS Apple Silicon: formatting, build/check, Rust tests, Clippy, target-specific third-party notice collection, native authenticated SSH smoke, release-candidate packaging/upload, and downloaded clean-install verification passed natively on arm64.
- The native SSH smoke exercises the Inspirum -> system OpenSSH path for authentication, terminal I/O, PTY resize, changed-host-key rejection before authentication, reconnect, and process cleanup on all three supported platforms.
- The downloaded-artifact gate independently inspects the packaged executable bytes as ELF x86_64, PE32+ AMD64, or thin Mach-O ARM64, validates release metadata, clean-extracts the archive, and runs the extracted executable with `--version`.
- The previous macOS `dispatch 0.2.0` notice blocker is no longer in the distributed dependency closure; the compatibility edge is supplied by the project-owned Apache-2.0 `vendor/dispatch-compat` patch and the target-specific notice collector passes on Apple Silicon.

Known limits:

- release archives are intentionally unsigned and unnotarized until credential-backed signing is enabled and verified;
- macOS packaging is currently a bare executable in a `.tar.gz`, not an `.app`/DMG;
- GSSAPI controls delegate to the installed OpenSSH build and real Kerberos-environment compatibility is not claimed universally;
- keyboard/IME/accessibility, richer terminal/workspace polish, local file management and remote editing are Phase 3 work;
- full WindTerm parity is not claimed.

The CI and release workflows pin Rust 1.95.0. Native CI proves the exercised paths above; it does not imply support for every OpenSSH build, desktop environment, locale, or future WindTerm-class feature.

## Release workflow targets

| Platform | Rust target | Archive | Verified state |
| --- | --- | --- | --- |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.tar.gz` | native SSH acceptance, package upload/download, binary architecture inspection and clean `--version` execution pass |
| Windows x64 | `x86_64-pc-windows-msvc` | `.zip` | native SSH acceptance, package upload/download, PE32+ AMD64 inspection and clean `--version` execution pass |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` bare executable | native SSH acceptance, package upload/download, thin Mach-O ARM64 inspection and clean `--version` execution pass; unsigned/unnotarized |

Early artifacts remain unsigned prereleases because code-signing and notarization credentials are not configured.

## Requirements

- a graphical desktop for interactive terminal windows (not required for diagnostics);
- the system `ssh` command from OpenSSH on `PATH`; SFTP workflows require `sftp`, SCP operations require `scp`, and host-key inspection/removal requires `ssh-keygen`;
- Rust 1.95.0 to match CI (the package uses edition 2024);
- on Linux, the normal X11/Wayland development packages needed by `eframe`.

OpenSSH Client is built into macOS, commonly packaged as `openssh-clients`/`openssh-client` on Linux, and available as a Windows Optional Feature. Availability of the system client is not application validation.

## Build and run

```text
cargo build --locked
cargo run --locked
```

Optional arguments:

```text
inspirum-terminal [--profiles PATH] [--ssh-config PATH] [--ui iced|legacy]
```

`--profiles` changes the JSON profile location. `--ssh-config` passes one explicit configuration file to OpenSSH. Otherwise OpenSSH uses its normal configuration and identity discovery. Tools → SSH tools opens interactive OpenSSH `sftp` terminals, manages trust, multiplexing, tunnels and tmux. The Files button opens the graphical SFTP browser and transfer queue. Tools → SCP opens explicit OpenSSH `scp` upload/download operations with overwrite safeguards; see [graphical SFTP](docs/sftp-browser.md) and [SCP operations](docs/scp.md). Structured HTTP CONNECT and SOCKS5 proxy transport currently supports no-auth proxies only; see [proxy transport](docs/proxy.md).

Click a terminal to give it keyboard focus. Moving the pointer away does not transfer that focus. The tmux controls are explicit and never run on ordinary Connect; see [tmux-aware SSH sessions](docs/tmux.md). Verify new host-key fingerprints through an independent trusted channel before accepting them. The Host key trust panel can inspect/remove entries from the default user `known_hosts` file or an explicitly selected file; it never auto-accepts a replacement key. If your SSH config uses a custom `UserKnownHostsFile`, select that file explicitly before inspecting or removing entries.

Open **Tools** or press **Ctrl+,** (Command+, on macOS) for profiles/startup, workspace/tab management, history/logging, appearance, SSH tools, SCP, diagnostics and snippets. In Tools, Ctrl/Command+1 through +8 selects the corresponding panel. Saved layouts preserve up to four panes, split directions and ratios; loading startup/workspace metadata never reconnects without an explicit restore. Synchronized input requires explicit pane selection and arming, and changing tabs or targets disarms it. The command palette provides quick switching, history search and sync control; snippets stage text until Send is pressed.

Appearance controls apply globally or to a saved profile, including colors, font, cursor, opacity and pointer behavior. History uses the backend's retained scrollback. Session logging writes output snapshots to an explicitly chosen new file; terminal output may contain secrets. `--ui legacy` retains the previous frontend for comparison. `cargo build --no-default-features` can build that comparison frontend alone.

## Local support diagnostics

```text
inspirum-terminal --diagnostics
inspirum-terminal --diagnostics --diagnostics-output support.txt
inspirum-terminal --diagnostics --profiles sessions.json --diagnostic-profile "Work laptop"
```

Headless diagnostics run before graphical initialization. The GUI Support diagnostics panel can add the current validated app launch policy and up to 12 sanitized in-memory error categories. Both surfaces query local OpenSSH tools with fixed arguments and never dump arbitrary environment variables or raw connection errors. Profile names, hosts, usernames, paths, endpoints, remote commands, passwords, passphrases, private-key material and authentication responses are omitted. Export refuses to overwrite an existing file.

See [diagnostics and privacy boundaries](docs/diagnostics.md), [saved profiles and safe import/export](docs/profile-library.md), and [interactive SFTP policy](docs/sftp.md). A diagnostic report is not proof of SSH connectivity or complete platform acceptance.

## Testing

Match CI, which pins Rust 1.95.0:

```text
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

`cargo test --locked` does not run the ignored fixture tests. On Linux, CI also runs:

```text
python3 -B -m unittest discover -s scripts/tests -p 'test_*.py'
scripts/test-ssh-integration.sh
```

Terminal compatibility coverage for Unicode/wide/combining/emoji text, alternate-screen and SGR mouse modes, representative VT behavior, and high-volume scrollback is documented in [terminal compatibility](docs/terminal-compatibility.md). Run it with:

```text
cargo test --locked --test terminal_compat
bash scripts/terminal-compat-benchmark.sh
```

The SSH script requires `sshd`, `ssh-keygen`, and Python 3. Core fixtures use unprivileged loopback listeners and temporary keys outside the checkout. Password and PAM-backed MFA acceptance additionally use passwordless `sudo` when available to create disposable OS users and root-owned loopback sshd processes; those accounts and processes are removed in cleanup. It is Linux-only and is not Windows or macOS execution.

## Security and data model

Inspirum stores profile names, hosts, usernames, ports, strict-host-key preference, identity-file paths, ProxyJump routes, forwarding specifications, keepalive/timeout values, optional remote commands, and tri-state SSH/authentication policies that can inherit, enable, or disable selected OpenSSH behavior. It does not save passwords, passphrases, private-key contents, or authentication responses. Authentication occurs inside OpenSSH's PTY. Every configured SSH value is passed as a separate local process argument rather than being interpolated into a local shell command. An optional remote command is sent only after the SSH destination and is interpreted by the remote account's shell after authentication.

Authentication policy controls do not store credentials. Public-key, password, keyboard-interactive/MFA, GSSAPI, GSSAPI credential delegation, and `IdentitiesOnly` are passed to OpenSSH as explicit options only when the profile overrides the inherited setting. GSSAPI support depends on the platform and the installed OpenSSH build; enabling GSSAPI credential delegation should be limited to trusted hosts where delegation is required.

This is an early slice, not a security audit. Review `docs/ARCHITECTURE.md` for boundaries and `docs/PARITY.md` for the roadmap. The parity document is not a claim that WindTerm parity exists.

## Releases

Tags and manual release requests must use `vMAJOR.MINOR.PATCH` and must exactly match `package.version` in `Cargo.toml`. The workflow packages the executable with this README, the full Apache-2.0 `LICENSE`, target-specific `THIRD_PARTY_NOTICES`, and `RELEASE_METADATA.json`; it publishes `SHA256SUMS` and marks the GitHub Release as an unsigned prerelease.

Before publication, each platform archive is uploaded as a workflow artifact and then downloaded into a separate native verification job. That job clean-extracts the archive, rejects unsafe archive layouts, verifies the executable architecture from its bytes, validates release metadata, and runs the extracted executable with `--version`. Publication depends on all three verification jobs.

Current archives are **not code-signed**. macOS artifacts are **not notarized** and remain bare executable archives rather than app bundles/DMGs. The repository includes signing-readiness hooks, but no release should be described as signed or notarized until credential-backed signing is enabled and the resulting signatures/notarization are independently verified.

Verify downloads with the platform's SHA-256 tooling before running them. Checksums detect changed bytes relative to the published manifest; while releases are unsigned, checksums do not independently authenticate the publisher.

## Contributing and license

See `CONTRIBUTING.md`. Inspirum Terminal's own source is licensed under the Apache License 2.0; the full terms are in `LICENSE`. Third-party dependencies remain under their respective licenses and are not relicensed by the project. Release archives include their collected license and notice material in `THIRD_PARTY_NOTICES/`.

- [SSH connection multiplexing](docs/controlmaster.md)
- [SSH tunnel manager](docs/tunnels.md)
- [Graphical SFTP browser and transfer queue](docs/sftp-browser.md)
