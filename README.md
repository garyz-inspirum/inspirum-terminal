# Inspirum Terminal

Inspirum Terminal is an early, fully open-source Apache-2.0 slice of a native SSH terminal. It is not a complete terminal suite. It does not have WindTerm feature parity, and this repository does not claim full WindTerm parity.

Current scope:

- native `eframe`/egui desktop window;
- terminal rendering and PTY integration through `egui_term`;
- SSH sessions launched as argument vectors through the system OpenSSH client;
- saved, non-secret connection profiles;
- multiple terminal tabs and explicit disconnect by closing a tab;
- OpenSSH configuration, keys, agent, prompts, and host-key database remain owned by OpenSSH.

## Verification status

Source review of the implementation passed. That does not mean every platform or acceptance row has been executed.

Verified evidence:

- Linux: a worker verified 17 normal Rust tests and 3 ignored fixture tests. The 17 are the non-ignored tests that run on Linux. They do not include the Windows-only PTY child-argument test, which has not been executed. The 3 fixture tests run only through `scripts/test-ssh-integration.sh`.
- Collector: a parent verified 11 tests in `scripts/tests/test_collect_third_party_notices.py`.
- License closure: Linux and Windows passed worker evidence. The active `dispatch 0.2.0` notice still blocks macOS distribution. It does not block compilation.

Not yet run:

- Windows native execution.
- macOS native execution.
- A completed native CI run, a release run, and downloaded-artifact verification.

The CI and release workflows pin Rust 1.95.0. A workflow definition, and compilation if it later succeeds, are not runtime validation and are not a support claim.

macOS release packaging is a bare executable in a `.tar.gz`, not an app bundle. It is unsigned and unnotarized. Do not distribute a macOS archive while the `dispatch 0.2.0` notice gap remains. No release artifact is code-signed.

## Release workflow targets

These are the targets the release workflow is defined to build. A row is not a statement that native execution has passed or that distribution is allowed.

| Platform | Rust target | Archive | Current limit |
| --- | --- | --- | --- |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.tar.gz` | 17 normal tests and 3 fixture tests verified on Linux; native CI has not yet run |
| Windows x64 | `x86_64-pc-windows-msvc` | `.zip` | license closure passed; native execution not yet run |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` bare executable, not an `.app` bundle | compilation is not blocked; distribution is blocked by `dispatch 0.2.0`; unsigned and unnotarized; native execution not yet run |

Early artifacts are unsigned prereleases because no code-signing or notarization credentials are available.

## Requirements

- a graphical desktop;
- the system `ssh` command from OpenSSH on `PATH`;
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

`--profiles` changes the JSON profile location. `--ssh-config` passes one explicit configuration file to OpenSSH. Otherwise OpenSSH uses its normal configuration and identity discovery.

Keep the pointer over the terminal while typing; this is a current `egui_term` interaction limitation. Verify new host-key fingerprints through an independent trusted channel before accepting them.

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

The SSH script requires `sshd`, `ssh-keygen`, and Python 3. It uses only an unprivileged loopback listener and temporary keys outside the checkout. It is Linux-only and is not Windows or macOS execution.

## Security and data model

Inspirum stores profile names, hosts, usernames, ports, and strict-host-key preference. It does not save passwords or private keys. Authentication occurs inside OpenSSH's PTY. Inputs are converted to separate process arguments rather than a shell command.

This is an early slice, not a security audit. Review `docs/ARCHITECTURE.md` for boundaries and `docs/PARITY.md` for the roadmap. The parity document is not a claim that WindTerm parity exists.

## Releases

Tags and manual release requests must use `vMAJOR.MINOR.PATCH` and must exactly match `package.version` in `Cargo.toml`. The workflow packages the executable with this README, the full Apache-2.0 `LICENSE`, and target-specific `THIRD_PARTY_NOTICES`, publishes `SHA256SUMS`, and marks the GitHub Release as an unsigned prerelease.

Workflow success would still not authorize macOS distribution while `dispatch 0.2.0` blocks that distribution. A macOS archive is a bare unsigned, unnotarized executable, not an app bundle. Windows and macOS native execution have not been run.

Verify downloads with the platform's SHA-256 tooling before running them. Checksums do not authenticate an unsigned publisher.

## Contributing and license

See `CONTRIBUTING.md`. Inspirum Terminal's own source is licensed under the Apache License 2.0; the full terms are in `LICENSE`. Third-party dependencies remain under their respective licenses and are not relicensed by the project. Release archives include their collected license and notice material in `THIRD_PARTY_NOTICES/`.
