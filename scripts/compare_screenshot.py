#!/usr/bin/env python3
"""Tolerant RGB screenshot comparison with a failure diff image.

This is intentionally dependency-light (Pillow only) so it can run in CI. A
pixel is considered equal when every channel differs by at most --tolerance.
The comparison passes when the fraction of unequal pixels is <= --max-mismatch.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from PIL import Image, ImageChops


def compare(actual_path: Path, baseline_path: Path, diff_path: Path, tolerance: int,
            max_mismatch: float) -> dict:
    actual = Image.open(actual_path).convert("RGB")
    baseline = Image.open(baseline_path).convert("RGB")
    if actual.size != baseline.size:
        result = {
            "ok": False,
            "reason": "size_mismatch",
            "actual_size": list(actual.size),
            "baseline_size": list(baseline.size),
            "mismatched_pixels": None,
            "mismatch_fraction": 1.0,
            "tolerance": tolerance,
            "max_mismatch": max_mismatch,
        }
        # Produce a visible artifact even when dimensions differ.
        diff = Image.new("RGB", actual.size, (255, 0, 0))
        diff.save(diff_path)
        return result

    width, height = actual.size
    # Pillow 14 renamed getdata(); retain compatibility with older CI images.
    if hasattr(actual, "get_flattened_data"):
        actual_data = actual.get_flattened_data()
        baseline_data = baseline.get_flattened_data()
    else:
        actual_data = actual.getdata()
        baseline_data = baseline.getdata()
    actual_pixels = list(actual_data)  # type: ignore[arg-type]
    baseline_pixels = list(baseline_data)  # type: ignore[arg-type]
    diff = Image.new("RGB", actual.size, (0, 0, 0))

    mismatched = 0
    max_channel_delta = 0
    for y in range(height):
        for x in range(width):
            index = y * width + x
            ap = tuple(int(channel) for channel in actual_pixels[index])
            bp = tuple(int(channel) for channel in baseline_pixels[index])
            deltas = [abs(a - b) for a, b in zip(ap, bp)]
            max_channel_delta = max(max_channel_delta, *deltas)
            if max(deltas) > tolerance:
                mismatched += 1
                # Red marks changed pixels; unchanged pixels are dimmed so the
                # diff remains useful when opened in a CI artifact viewer.
                diff.putpixel((x, y), (255, min(255, max(deltas) * 4), 0))
            else:
                grey = sum(bp) // 3
                diff.putpixel((x, y), (grey // 4, grey // 4, grey // 4))

    total = width * height
    fraction = mismatched / total
    result = {
        "ok": fraction <= max_mismatch,
        "reason": "within_tolerance" if fraction <= max_mismatch else "pixel_mismatch",
        "actual_size": [width, height],
        "baseline_size": [width, height],
        "mismatched_pixels": mismatched,
        "total_pixels": total,
        "mismatch_fraction": fraction,
        "max_channel_delta": max_channel_delta,
        "tolerance": tolerance,
        "max_mismatch": max_mismatch,
    }
    diff.save(diff_path)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("actual", type=Path)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("--diff", type=Path, required=True)
    parser.add_argument("--metrics", type=Path)
    parser.add_argument("--tolerance", type=int, default=16)
    parser.add_argument("--max-mismatch", type=float, default=0.005)
    args = parser.parse_args()
    if not 0 <= args.tolerance <= 255:
        parser.error("--tolerance must be between 0 and 255")
    if not 0 <= args.max_mismatch <= 1:
        parser.error("--max-mismatch must be between 0 and 1")
    try:
        result = compare(args.actual, args.baseline, args.diff, args.tolerance,
                         args.max_mismatch)
    except (OSError, ValueError) as exc:
        print(f"ERROR: screenshot comparison failed: {exc}", file=sys.stderr)
        return 2
    if args.metrics:
        args.metrics.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
    if not result["ok"]:
        print(f"FAIL: screenshot differs from baseline; diff: {args.diff}", file=sys.stderr)
        return 1
    print("PASS: screenshot is within the configured pixel tolerance")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
