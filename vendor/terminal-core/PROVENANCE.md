# Vendored egui_term provenance

- Upstream project: https://github.com/Harzu/egui_term
- Crate: `egui_term` 0.1.0
- crates.io checksum: `062329a2d44c767147e2bf56ebea1aa0ca25aea5e88e74b5feb956161c827c5a`
- Upstream license: MIT; preserved in `LICENSE`

This copy comes from the crates.io 0.1.0 package. Inspirum Terminal carries
focused changes for retained-focus input dispatch, Windows argument escaping
and executable-token serialization, and PTY event-loop/subscription shutdown.
The vendored code remains MIT-licensed and is not relicensed under the
surrounding project's Apache-2.0 license.

The toolkit-neutral backend is now maintained as `inspirum-terminal-core`.
The egui widgets, font and input bindings were removed. Color and geometry types
are plain Rust data; PTY callbacks have no GUI context dependency.
