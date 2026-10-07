# Inspirum: native Iced design preview

This is a **design and interaction prototype**, not the production terminal.
It has no SSH, PTY, SFTP, credential storage, production-profile access or real file operations. All server names, terminal output and file listings are fixtures. The production egui application is untouched.

## Run

```sh
cargo run --manifest-path prototypes/iced-shell/Cargo.toml
```

Open a sample session in the left sidebar. Try session search, New connection, form validation, independent tabs, close confirmation, split right/down, dragging pane headers/dividers, the Files dock, selecting a sample remote file and queuing a preview transfer. A-/A+ changes the UI scale. Ctrl/Cmd+N opens the connection form; Ctrl/Cmd+Shift+P opens Commands. Escape dismisses dialogs.

`--preview=terminal`, `--preview=split`, `--preview=files` and `--preview=dialog` create deterministic screenshot fixtures. Pass after `--` when using cargo run.

The Iced design preview workflow builds/tests Linux, Windows and macOS, and captures native Linux windows. Its artifacts are explicitly **preview** executables, separate from production releases. Native macOS/Windows visual verification is still required even when their builds pass.

## Deliberate limits

- Up to four panes for this interaction study, not a production feature limit.
- Sidebar width, advanced-connection controls and full keyboard focus trapping need a further implementation pass.
- The terminal-shaped content is plain sample text, not a terminal emulator or a benchmark.
- The file pane demonstrates target visibility, selection and queue placement; it is not a file manager.
- No claims of complete WindTerm parity, migration, accessibility conformance or superior performance.
- Tiny-skia is used for this first deterministic native-layout study. GPU rendering and terminal latency must be assessed in the next real-PTY Iced spike.

## Acceptance before replacing the production frontend

A reviewed task-based design; real SSH and PTY integration; existing policy, host-key and clipboard/paste safeguards retained; complete feature mapping; native interaction checks on all three operating systems; measured startup/input/scroll performance. Keep issue #78 open.
