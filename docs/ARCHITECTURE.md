# Architecture

Inspirum Terminal uses Iced 0.14 as its only native frontend. The legacy egui windows, terminal widget, fallback launcher and egui/eframe runtime dependencies have been removed.

`src/main.rs` validates CLI arguments, resolves the profile directory, sets `TERM=xterm-256color` before threads start, and launches `src/iced_app.rs`. Diagnostics and proxy-helper modes exit before GUI initialization. `--ui iced` remains accepted for existing launch scripts; `--ui legacy` is rejected.

The Iced application owns pane grids, modal dialogs, focus, terminal rendering, transfer views and remote editors. `src/iced_app/tools.rs` and its SSH module implement profiles/startup, workspace/tab controls, history/logging, appearance, SSH management, SCP, diagnostics and multiline snippets. Blocking jobs run outside the UI thread. Confirmations capture concrete profile/pane/path targets and reject stale state. Profile load failures make the active profile store read-only until repaired.

`vendor/terminal-core` owns the Alacritty parser, native PTY/process lifecycle, retained history, selection and display snapshots. It contains no GUI widgets or egui context, color, geometry or input types. Events travel through callbacks/channels to Iced subscriptions. Closing a pane drops its backend and shuts down owned workers. This MIT-licensed backend derives from egui_term; its upstream attribution is retained in PROVENANCE.md and LICENSE.

`src/lib.rs` implements validated non-secret profiles and atomic persistence. `src/terminal.rs` builds argument vectors for system OpenSSH, SFTP, multiplexing, tunnels and host-key tooling. Credentials and protocol security remain owned by OpenSSH. The SFTP, SCP, remote-edit, proxy, history and workspace modules provide frontend-independent policies; Iced supplies their controls.

A root Iced widget enables native terminal IME only when no form/editor owns input. It positions candidates at the terminal cursor, displays preedit without writing it, captures composition keys, and sends committed text to the captured pane. Focus changes reset the native composition and reject stale commits.

Terminal output invalidates cached display rows and schedules a coalesced refresh. Clipboard and synchronized input use captured target IDs; changes to focus or armed targets cancel stale paste confirmations. Loading saved workspace metadata never connects. Restore requires saved reconnect permission and an explicit user action.

Linux and Windows use Iced WGPU with tiny-skia available; macOS uses native tiny-skia because the current WGPU Metal dependency graph contains historical crates without verified license text. Linux explicitly enables X11 and Wayland support. Release notices use package-local texts or exact-revision, hash-verified upstream supplements.

Tests cover policy/persistence, Iced interaction state, real PTY output/resize/history/cursor behavior, lifecycle failure cleanup and Windows argument boundaries. Native CI runs tests, strict Clippy, authenticated SSH smoke checks, release packaging and clean-install verification on Linux, Windows and macOS. The Iced workflow also captures Linux GUI screenshots for all eight Tools panels. Manual macOS IME and named-hardware latency acceptance remains tracked in issue #78; build/smoke success does not establish that acceptance.
