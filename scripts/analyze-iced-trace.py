#!/usr/bin/env python3
"""Summarize opt-in Iced terminal click-to-canvas timing without exposing logs.

This is CPU event-delivery-to-canvas timing, NOT physical click-to-caret.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import re

CLICK = re.compile(r"^iced slow click_to_canvas_draw: ([0-9]+(?:\.[0-9]+)?) ms \(threshold [0-9]+ ms\)$")


def click_samples(lines: list[str]) -> list[float]:
    values = []
    for line in lines:
        match = CLICK.fullmatch(line.strip())
        if match:
            values.append(float(match.group(1)))
    return values


def percentile(values: list[float], percentage: int) -> float:
    """Nearest-rank quantile; no artificial interpolated frame latency."""
    if not values:
        raise ValueError("no timing observations")
    ordered = sorted(values)
    rank = (len(ordered) * percentage + 99) // 100
    return ordered[max(1, rank) - 1]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path, help="Iced stderr capture from a local run")
    args = parser.parse_args()
    try:
        samples = click_samples(args.log.read_text(encoding="utf-8").splitlines())
    except (OSError, UnicodeError) as error:
        parser.error(f"cannot read timing log: {error}")
    if not samples:
        print("No click-to-canvas timings. Enable INSPIRUM_ICED_TRACE_MS=1 and click a terminal.")
        return 1
    print(f"terminal_click_samples={len(samples)}")
    print(f"click_to_canvas_p50_ms={percentile(samples, 50):.1f}")
    print(f"click_to_canvas_p95_ms={percentile(samples, 95):.1f}")
    if len(samples) < 30:
        print("Warning: fewer than 30 observations; collect more clicks for a stable p95.")
    print("These are in-process CPU-stage durations, not physical click-to-visible-caret timings.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
