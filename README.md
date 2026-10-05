# Inspirum Terminal

Inspirum Terminal is an early, fully open-source Apache-2.0 slice of a native SSH terminal. It is not a complete terminal suite. It does not have WindTerm feature parity, and this repository does not claim full WindTerm parity.

Current scope:

- native `eframe`/egui desktop window;
- terminal rendering and PTY integration through `egui_term`;
- SSH sessions, interactive SFTP tabs, a graphical SFTP browser/transfer queue, and explicit SCP upload/download operations launched through the system OpenSSH client tools;
- saved, non-secret connection profiles with search, rename-on-save, editable duplication, confirmed deletion, and validated JSON import/export;
  Export refuses to overwrite an existing destination; replace-import requires explicit confirmation.
- first-class profile controls for identity-file paths, ProxyJump, structured HTTP CONNECT/SOCKS5 proxy routing, app-managed OpenSSH ControlMaster multiplexing/lifecycle, authentication-method policy, GSSAPI delegation policy, `IdentitiesOnly`, agent/X11 forwarding, compression, connect timeout, keepalive, remote auto-command, and local/remote/dynamic port forwarding;
- multiple SSH/SFTP terminal tabs, explicit disconnect by closing a tab, explicit reconnect after a remote/session exit, and explicit tmux discovery/attach/create with attach-only reconnect;
- host-key trust tools that resolve the effective OpenSSH host identity with `ssh -G`, inspect trusted entries with `ssh-keygen -F`, and require explicit confirmation before `ssh-keygen -R` removal;
- local-only `--diagnostics` reports with an allowlisted policy summary and no-clobber text export; no graphical desktop or server connection is required for this command;
- OpenSSH configuration, key material, agent, authentication prompts, host-key database, proxy transport, and forwarding implementation remain owned by OpenSSH.

## Verification status

Native CI for PR #5 completed successfully on Linux x86_64, Windows x64, and macOS Apple Silicon at commit `f52d6297c118da4b590526b8b027a6a8353656ff`. This is useful platform evidence, but it is not full SSH acceptance.

Verified evidence:

- Linux x86_64: formatting, build/check, Rust tests, clippy, helper-script tests, and the isolated authenticated `sshd` fixture passed. CI run #78 at `1a8716d0d9ed45e3c1d43c1db5918cc5725b2186` additionally verified password authentication, encrypted-key prompts, SSH-agent authentication, public-key + keyboard-interactive PAM MFA, disabled-method negative cases, password-prompt cancellation, and a stalled-handshake `ConnectTimeout` path. The fixture log contained the expected PASS markers and did not contain the synthetic fixture password.
- Windows x64: formatting, build/check, Rust tests, and clippy passed. The native suite includes the Windows PTY child-argument-boundary test and an actual OpenSSH child-exit smoke test.
- macOS Apple Silicon: formatting, build/check, Rust tests, and clippy passed natively on arm64. The suite includes an actual OpenSSH child-exit smoke test.
- License closure: Linux and Windows remain clear for the current dependency graph. The active `dispatch 0.2.0` notice still blocks macOS distribution; it does not block compilation or tests.

Not yet run:

- isolated authenticated SSH-server acceptance on Windows or macOS;
- GSSAPI authentication acceptance and the broader ProxyJump/forwarding, X11/agent-forwarding acceptance matrix on all three platforms;
- manual GUI/device acceptance, a release workflow, and downloaded-artifact verification.

The CI and release workflows pin Rust 1.95.0. Passing native CI proves the exercised code paths on those runners; it does not by itself establish complete SSH or WindTerm parity.

macOS release packaging is a bare executable in a `.tar.gz`, not an app bundle. It is unsigned and unnotarized. Do not distribute a macOS archive while the `dispatch 0.2.0` notice gap remains. No release artifact is code-signed.

## Release workflow targets

These are the targets the release workflow is defined to build. A row is not a statement that native execution has passed or that distribution is allowed.

| Platform | Rust target | Archive | Current limit |
| --- | --- | --- | --- |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.tar.gz` | native CI passed, including the isolated authenticated `sshd` fixture; broader SSH acceptance remains pending |
| Windows x64 | `x86_64-pc-windows-msvc` | `.zip` | native CI passed, including PTY argv-boundary and OpenSSH child-exit smoke tests; isolated authenticated server acceptance remains pending |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` bare executable, not an `.app` bundle | native arm64 CI passed; distribution is still blocked by `dispatch 0.2.0`; unsigned and unnotarized |

Early artifacts are unsigned prereleases because no code-signing or notarization credentials are available.

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
inspirum-terminal [--profiles PATH] [--ssh-config PATH]
```

`--profiles` changes the JSON profile location. `--ssh-config` passes one explicit configuration file to OpenSSH. Otherwise OpenSSH uses its normal configuration and identity discovery. The SFTP button opens an interactive OpenSSH `sftp` tab. The Files button opens the graphical SFTP browser and transfer queue. The SCP button opens explicit OpenSSH `scp` upload/download operations with overwrite safeguards; see [graphical SFTP](docs/sftp-browser.md) and [SCP operations](docs/scp.md). Structured HTTP CONNECT and SOCKS5 proxy transport currently supports no-auth proxies only; see [proxy transport](docs/proxy.md).

Click a terminal to give it keyboard focus. Moving the pointer away does not transfer that focus. The tmux controls are explicit and never run on ordinary Connect; see [tmux-aware SSH sessions](docs/tmux.md). Verify new host-key fingerprints through an independent trusted channel before accepting them. The Host key trust panel can inspect/remove entries from the default user `known_hosts` file or an explicitly selected file; it never auto-accepts a replacement key. If your SSH config uses a custom `UserKnownHostsFile`, select that file explicitly before inspecting or removing entries.

## Local support diagnostics

```text
inspirum-terminal --diagnostics
inspirum-terminal --diagnostics --diagnostics-output support.txt
inspirum-terminal --diagnostics --profiles sessions.json --diagnostic-profile "Work laptop"
```

Diagnostics run before graphical initialization. They query local OpenSSH tools with fixed arguments; they never connect, authenticate or evaluate SSH configuration. An optional profile contributes only policy states, boolean flags and forwarding counts. Profile names, hosts, usernames, paths, remote commands, credentials, terminal contents and raw error text are omitted. Export refuses to overwrite an existing file.

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

The SSH script requires `sshd`, `ssh-keygen`, and Python 3. Core fixtures use unprivileged loopback listeners and temporary keys outside the checkout. Password and PAM-backed MFA acceptance additionally use passwordless `sudo` when available to create disposable OS users and root-owned loopback sshd processes; those accounts and processes are removed in cleanup. It is Linux-only and is not Windows or macOS execution.

## Security and data model

Inspirum stores profile names, hosts, usernames, ports, strict-host-key preference, identity-file paths, ProxyJump routes, forwarding specifications, keepalive/timeout values, optional remote commands, and tri-state SSH/authentication policies that can inherit, enable, or disable selected OpenSSH behavior. It does not save passwords, passphrases, private-key contents, or authentication responses. Authentication occurs inside OpenSSH's PTY. Every configured SSH value is passed as a separate local process argument rather than being interpolated into a local shell command. An optional remote command is sent only after the SSH destination and is interpreted by the remote account's shell after authentication.

Authentication policy controls do not store credentials. Public-key, password, keyboard-interactive/MFA, GSSAPI, GSSAPI credential delegation, and `IdentitiesOnly` are passed to OpenSSH as explicit options only when the profile overrides the inherited setting. GSSAPI support depends on the platform and the installed OpenSSH build; enabling GSSAPI credential delegation should be limited to trusted hosts where delegation is required.

This is an early slice, not a security audit. Review `docs/ARCHITECTURE.md` for boundaries and `docs/PARITY.md` for the roadmap. The parity document is not a claim that WindTerm parity exists.

## Releases

Tags and manual release requests must use `vMAJOR.MINOR.PATCH` and must exactly match `package.version` in `Cargo.toml`. The workflow packages the executable with this README, the full Apache-2.0 `LICENSE`, and target-specific `THIRD_PARTY_NOTICES`, publishes `SHA256SUMS`, and marks the GitHub Release as an unsigned prerelease.

Workflow success still does not authorize macOS distribution while `dispatch 0.2.0` blocks that distribution. A macOS archive is a bare unsigned, unnotarized executable, not an app bundle. Native CI has run on Windows and Apple Silicon, but manual GUI/device acceptance and isolated authenticated SSH-server acceptance on those platforms remain pending.

Verify downloads with the platform's SHA-256 tooling before running them. Checksums do not authenticate an unsigned publisher.

## Contributing and license

See `CONTRIBUTING.md`. Inspirum Terminal's own source is licensed under the Apache License 2.0; the full terms are in `LICENSE`. Third-party dependencies remain under their respective licenses and are not relicensed by the project. Release archives include their collected license and notice material in `THIRD_PARTY_NOTICES/`.

- [SSH connection multiplexing](docs/controlmaster.md)
- [SSH tunnel manager](docs/tunnels.md)
- [Graphical SFTP browser and transfer queue](docs/sftp-browser.md)
