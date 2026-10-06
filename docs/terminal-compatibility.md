# Terminal compatibility and performance suite

Issue #23 tracks bounded compatibility evidence for the existing `egui_term` / Alacritty-backed PTY path. This suite is not a claim of complete xterm, VT, Unicode or WindTerm parity.

## Automated coverage

`tests/terminal_compat.rs` exercises the real terminal backend on Unix CI runners with:

- UTF-8 text containing CJK wide characters, a combining-mark sequence and emoji, including a replacement-character regression check;
- alternate-screen enter/leave using xterm `?1049`;
- normal mouse tracking plus SGR mouse encoding mode `?1000` / `?1006`;
- representative VT cursor-back and erase-line behavior;
- 5,000-line high-volume output followed by scrollback navigation.

The test binary is also compiled by native Windows CI. The PTY runtime cases currently use `/bin/sh`, so they execute on Linux and macOS only. This distinction is intentional and must not be presented as Windows runtime acceptance.

Run the compatibility suite:

```text
cargo test --locked --test terminal_compat
```

## Reproducible throughput and memory benchmark

The opt-in benchmark drives 20,000 terminal lines through the same backend, reports wall-clock time to the final marker, and verifies retained scrollback remains navigable:

```text
cargo test --locked --release --test terminal_compat terminal_high_volume_benchmark -- --ignored --nocapture
```

For process-level peak-memory evidence on Linux or macOS, use:

```text
bash scripts/terminal-compat-benchmark.sh
```

The script wraps the release-mode benchmark with the platform `/usr/bin/time` implementation. Results depend on runner hardware, OS, Rust toolchain, font/rendering environment and concurrent load. Record the exact commit and environment with any published number; do not turn a single CI timing into a product-wide latency or memory guarantee.

## Evidence policy

A green run proves only the exercised parser/PTTY/grid behaviors on that runner. It does not establish complete terminal emulation compatibility, bidi correctness, IME behavior, font shaping correctness, every mouse protocol, or interactive GUI performance under all workloads.
