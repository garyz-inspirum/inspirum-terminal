# Architecture

## Status

Inspirum Terminal is an early SSH-only native desktop slice. The architecture deliberately delegates protocol and credential handling to the operating system's OpenSSH client. It does not yet implement the broader roadmap in `PARITY.md`.

## Components

### Native application shell

`src/main.rs` parses `--profiles` and `--ssh-config`, resolves the default platform configuration directory, sets `TERM=xterm-256color` before threads start, and launches an `eframe` window.

`src/app.rs` owns the egui connection form, saved-profile list, active terminal tabs, error display, explicit keyboard-focus ownership, and PTY event channel. A terminal acquires keyboard ownership only after connect, tab selection, or a click in the terminal; pointer hover cannot take focus from a form field. Closing a tab drops its terminal backend and is the current disconnect operation.

### Profiles and launch policy

`src/lib.rs` defines the non-secret `Session` model and JSON persistence. Profile fields are validated before save and load. Both directions enforce the same 1 MiB serialized-file limit. Writes validate and serialize before creating a temporary file, then use replacement so a failed save does not truncate a prior profile file. Unknown JSON fields are rejected.

A profile can contain a display name, host/config alias, username, port, and strict-host-key flag. It cannot contain a password, private key, command, arbitrary OpenSSH option, or shell fragment.

### SSH and terminal boundary

`src/terminal.rs` verifies that `ssh -V` reports OpenSSH, creates a vector of process arguments, and starts `ssh` in the PTY supplied by `egui_term`. No shell is involved.

The minimally patched `egui_term` 0.1.0 source is vendored under `vendor/egui_term` with its upstream MIT license and provenance. It provides the terminal widget, Alacritty terminal parser, and native PTY abstraction. The local patch enables Alacritty's Windows argument escaping so one Rust argument remains one child argument, and makes the PTY subscription thread terminate on channel closure or forwarding failure and join during backend drop. These dependency changes do not alter Inspirum Terminal's Apache-2.0 project license. OpenSSH provides transport, authentication, configuration parsing, proxy/jump behavior configured by the user, host-key storage, and agent integration.

Remote terminal output is untrusted. PTY title or clipboard events are currently not forwarded to host APIs.

## Data and process flow

1. The user selects or enters a profile.
2. Inspirum validates profile tokens and converts them to separate OpenSSH arguments.
3. Inspirum optionally adds one `-F` configuration path.
4. `egui_term` starts system `ssh` in a native PTY.
5. Input and resize commands flow from the terminal widget to the PTY.
6. Parsed terminal state flows back into the egui widget; exit events mark the tab exited.
7. Authentication prompts and responses stay inside the OpenSSH PTY.

## Trust boundaries

- Profile JSON is local but treated as untrusted input when loaded.
- Profile fields cannot become shell syntax or arbitrary command-line options.
- OpenSSH configuration is trusted according to normal OpenSSH rules; users remain responsible for its contents and permissions.
- Host-key acceptance is security-sensitive. The default asks through OpenSSH; strict profiles require an already trusted key.
- Terminal escape sequences originate remotely and must not silently gain host capabilities.
- Release artifacts are currently unsigned. SHA-256 checksums provide integrity checking after obtaining `SHA256SUMS`, but not publisher identity.

## Tests

Normal tests cover profile validation, the shared serialized-size limit and preservation of the prior file after an oversized save, argument placement, explicit form/terminal focus ownership for typing, paste and Enter, missing-OpenSSH errors, headless egui rendering, and a headless PTY observing an actual OpenSSH exit. A Windows-only PTY regression launches PowerShell through a path containing spaces and checks actual child arguments containing spaces, a quote, and a trailing backslash. The opt-in Linux fixture starts a disposable unprivileged loopback `sshd` to test authenticated input/output, resize, host-key rejection, exit, repeated disconnect cleanup, and subscription-thread teardown.

Linux unit and authenticated fixture tests can be run locally. The Windows child-argument regression requires native Windows CI; Windows GUI behavior and all macOS GUI, PTY, OpenSSH, packaging, and lifecycle behavior remain unverified until native CI and human testing run. Compilation alone is not runtime proof.

## Release model

CI builds natively on Ubuntu x64, Windows x64, and macOS Apple Silicon. Each archive contains one native executable, `LICENSE`, and `README.md`. A separate publish job creates `SHA256SUMS` and an unsigned prerelease. Tag and manual workflows require an existing `vMAJOR.MINOR.PATCH` tag whose version equals `Cargo.toml`.
