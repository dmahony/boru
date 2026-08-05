#!/usr/bin/env python3
"""Find the pixel centre of a phrase (one or more words) in a screenshot.

Used by the UI-HOME-17 evidence harness to calibrate click targets
dynamically, because the home layout is content-driven and card positions
shift run to run (mesh card height depends on the live event log).

Phrase matching is line-aware: all words must appear on the same OCR line
(y within LINE_TOL px) in left-to-right order, so "Group Chat" matches the
card title, never "Group" from a sidebar label plus a later "Chat".

Usage: ui17_click_calibrate.py <shot.png> <word1> [word2 ...] [--xmin N] [--ymin N] [--xmax N] [--ymax N]
Prints "X Y" (integer pixel centre) or "0 0" if not found.
"""
from __future__ import annotations

import os
import subprocess
import sys
from PIL import Image

RESAMPLE = getattr(Image, "Resampling", Image).LANCZOS if hasattr(Image, "Resampling") else Image.LANCZOS
LINE_TOL = 10

def main() -> int:
    args = sys.argv[1:]
    if len(args) < 2:
        print("usage: ui17_click_calibrate.py shot.png word [word ...] [--xmin N] ...", file=sys.stderr)
        return 2
    shot = args[0]
    words = []
    bands = {"xmin": 0, "ymin": 0, "xmax": 99999, "ymax": 99999}
    i = 1
    while i < len(args):
        a = args[i]
        if a.startswith("--") and i + 1 < len(args):
            bands[a[2:]] = int(args[i + 1])
            i += 2
        else:
            words.append(a.lower().rstrip(".,;:"))
            i += 1
    if not words:
        print("0 0")
        return 0

    scale = 3
    img = Image.open(shot).convert("RGB")
    p = "/tmp/ui17_calib.png"
    img.resize((img.width * scale, img.height * scale), RESAMPLE).save(p)
    t = p.replace(".png", "")
    subprocess.run(["tesseract", p, t, "--psm", "6", "tsv"], capture_output=True)
    rows = []
    if os.path.exists(t + ".tsv"):
        for line in open(t + ".tsv").read().splitlines()[1:]:
            f = line.split("\t")
            if len(f) == 12 and f[11].strip() and f[6].isdigit():
                rows.append((f[11].lower().rstrip(".,;:"), int(f[6]) // scale, int(f[7]) // scale,
                             int(f[8]) // scale, int(f[9]) // scale))
    rows = [r for r in rows if bands["xmin"] <= r[1] <= bands["xmax"] and bands["ymin"] <= r[2] <= bands["ymax"]]

    # Word match with OCR-noise tolerance: exact match preferred; otherwise
    # accept a one-character edit (e.g. tesseract reads "Greate" for
    # "Create" in the Tunnels header action).
    def word_matches(ocr_word: str, target: str) -> bool:
        if ocr_word == target:
            return True
        if len(target) < 4 or len(ocr_word) != len(target):
            return False
        diffs = sum(1 for a, b in zip(ocr_word, target) if a != b)
        return diffs <= 1

    # First word: first match in reading order (y, then x).
    first = next(
        (r for r in sorted(rows, key=lambda r: (r[2], r[1])) if word_matches(r[0], words[0])),
        None,
    )
    if first is None:
        print("0 0")
        return 0
    line_y = first[2]
    line = [r for r in rows if abs(r[2] - line_y) <= LINE_TOL]
    line.sort(key=lambda r: r[1])
    # Greedy left-to-right scan for the full phrase on this line.
    match = []
    j = 0
    for r in line:
        if j < len(words) and word_matches(r[0], words[j]):
            match.append(r)
            j += 1
    if j != len(words):
        print("0 0")
        return 0
    cx = sum(r[1] + r[3] // 2 for r in match) // len(match)
    cy = sum(r[2] + r[4] // 2 for r in match) // len(match)
    print(f"{cx} {cy}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
