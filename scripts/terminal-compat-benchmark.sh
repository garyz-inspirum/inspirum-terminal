#!/usr/bin/env bash
set -euo pipefail

cmd=(cargo test --locked --release --test terminal_compat terminal_high_volume_benchmark -- --ignored --nocapture)

case "$(uname -s)" in
  Linux)
    exec /usr/bin/time -v "${cmd[@]}"
    ;;
  Darwin)
    exec /usr/bin/time -l "${cmd[@]}"
    ;;
  *)
    echo "terminal compatibility memory benchmark currently supports Linux and macOS" >&2
    exit 2
    ;;
esac
