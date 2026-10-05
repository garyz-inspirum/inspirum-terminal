# Architecture

## Status

Inspirum Terminal is an early SSH-only native desktop slice. The architecture deliberately delegates protocol and credential handling to the operating system's OpenSSH client. It does not yet implement the broader roadmap in `PARITY.md`.

## Components

### Native application shell

`src/main.rs` parses `--profiles` and `--ssh-config`, resolves the default platform configuration directory, sets `TERM=xterm-256color` before threads start, and launches an `eframe` window.

`src/app.rs` owns the egui connection form, saved-profile list, active terminal tabs, error display, explicit keyboard-focus ownership, and PTY event channel. A terminal acquires keyboard ownership only after connect, tab selection, or a click in the terminal; pointer hover cannot take focus from a form field. Closing a tab drops its terminal backend and is the current disconnect operation.

### Profiles and launch policy

`src/lib.rs` defines the non-secret `Session` model and JSON persistence. Profile fields are validated before save and load. Both directions enforce the same 1 MiB serialized-file limit. Writes validate and serialize before creating a temporary file, then use replacement so a failed save does not truncate a prior profile file. Unknown JSON fields are rejected.

A profile can contain a display name, host/config alias, username, port, strict-host-key flag, identity-file path, ProxyJump route, tri-state agent/X11 forwarding and compression policies (inherit/enable/disable), connection timeout, server keepalive interval, and local/remote/dynamic forwarding specifications. It cannot contain a password, passphrase, private-key contents, arbitrary OpenSSH option, remote command, or shell fragment. Forwarding is launched with `ExitOnForwardFailure=yes` so a requested listener failure is surfaced instead of silently producing a partially configured session.

### SSH and terminal boundary

`src/terminal.rs` verifies that `ssh -V` reports OpenSSH, creates a vector of process arguments, and starts `ssh` in the PTY supplied by `egui_term`. No shell is involved.

The minimally patched `egui_term` 0.1.0 source is vendored under `vendor/egui_term` with its upstream MIT license and provenance. It provides the terminal widget, Alacritty terminal parser, and native PTY abstraction. The local patch enables Alacritty's Windows argument escaping so one Rust argument remains one child argument. Because Alacritty 0.25 passes a null application name to `CreateProcessW` and otherwise emits the executable program unquoted, the Windows boundary also validates that the program contains neither a quote nor NUL and serializes it as one quoted command-line token. POSIX program handling is unchanged. It also routes keyboard events according to retained widget focus while requiring pointer containment for mouse events. PTY construction installs a rollback guard before the fallible subscription-thread spawn, the subscription stops on channel closure or forwarding failure, and backend drop joins it. These dependency changes do not alter Inspirum Terminal's Apache-2.0 project license. OpenSSH provides transport, authentication, configuration parsing, proxy/jump behavior configured by the user, host-key storage, and agent integration.

Remote terminal output is untrusted. PTY title or clipboard events are currently not forwarded to host APIs.

## Data and process flow

1. The user selects or enters a profile.
2. Inspirum validates profile tokens and converts them to separate OpenSSH arguments.
3. Inspirum adds validated first-class SSH options as discrete argv values, then optionally adds one `-F` configuration path.
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

Normal tests cover profile validation, the shared serialized-size limit and preservation of the prior file after an oversized save, argument placement, missing-OpenSSH errors, headless egui rendering, and a headless PTY observing an actual OpenSSH exit. Real egui widget dispatch tests verify that a focused terminal receives text, paste, and Enter after the pointer leaves its rectangle, while a focused form does not leak the same event classes into a hovered terminal. Linux tests inject subscription-thread spawn failure after a child has started and assert bounded child cleanup, and cover forwarding-channel disconnection. Platform-independent regressions verify Windows program-token serialization and compile the native argv-dump source from a spaced filename with an explicit crate name. A Windows-only PTY regression launches that helper from an executable path containing spaces, requires a bounded exit event, and compares exact argument count and values for spaces, a quote, and a trailing backslash. The opt-in Linux fixture starts a disposable unprivileged loopback `sshd` to test authenticated input/output, resize, host-key rejection, exit, repeated disconnect cleanup, and subscription-thread teardown.

Linux unit and authenticated fixture tests can be run locally. Native CI has exercised the Windows child-argument regression and OpenSSH child-exit smoke path, and has exercised the OpenSSH child-exit smoke path on Apple Silicon. Windows/macOS isolated authenticated-server tests and manual GUI/device acceptance remain pending. Compilation or smoke testing alone is not full runtime acceptance.

## Release model

CI builds natively on Ubuntu x64, Windows x64, and macOS Apple Silicon. Each archive contains one native executable, `LICENSE`, and `README.md`. A separate publish job creates `SHA256SUMS` and an unsigned prerelease. Tag and manual workflows require an existing `vMAJOR.MINOR.PATCH` tag whose version equals `Cargo.toml`.
