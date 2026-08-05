#!/usr/bin/env python3
"""UI-HOME-18 hard gate: quick-action description completeness.

Uses the exact method proven by UI-HOME-06 (scripts/ui_home06_quick_actions_
evidence.sh): crop the main panel (x>=300) band, 4x upscale, OCR, then check
significant-word containment — every content word (>=3 chars) of each
approved description must appear in the OCR text. OCR of 13 px supporting
text is imperfect and multi-column reading order scrambles phrase order, so
word containment (not substring) is the robust check; a clipped trailing
word ("join." / "chat." / "key.") would be missing and fail.

Per width, a description passes if it passes in ANY shot of that width
(one-column layouts show one card per screenful; scrolled series covers the
fold). Exit 0 = PASS, 1 = FAIL.
"""
import os
import re
import subprocess
import sys
import tempfile

from PIL import Image

from glob import glob

TASK_ID = "t_266bfba3"
WIDTHS = [1600, 1280, 1024, 800]
DESCRIPTIONS = [
    "Open a public room for anyone to join",
    "Start a private group conversation",
    "Connect with a friend by public key",
    "Choose a file to share in a chat",
]


def alnum(s):
    return re.sub(r"[^a-z0-9]", "", s.lower())


def ocr_main_panel(shot):
    """Crop the main panel (x 300..) with 4x upscale, OCR, return normalized
    text with spaces preserved."""
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
        crop_path = tmp.name
    try:
        img = Image.open(shot).convert("RGB")
        w, h = img.size
        # Crop the whole main panel (x from 300) — y from 20 so narrow-width
        # grid cards that render high in the window are included. (UI-HOME-06
        # used y>=200 for 1600 top shots where the grid sits low; scanning
        # the full column is safer and word-containment is noise-tolerant.)
        x1 = min(w, 1200)
        y1 = max(0, h - 40)
        if x1 <= 300 or y1 <= 20:
            return ""
        band = img.crop((300, 20, x1, y1))
        band = band.resize((band.width * 4, band.height * 4), Image.LANCZOS)
        band.save(crop_path)
        out = subprocess.run(
            ["tesseract", crop_path, "-", "--psm", "6"], capture_output=True, text=True
        ).stdout
        # Keep letters/digits/spaces only, lowercase.
        norm = re.sub(r"[^a-z0-9 ]", " ", out.lower())
        norm = re.sub(r"\s+", " ", norm).strip()
        return norm
    finally:
        os.unlink(crop_path)


def words_in(text):
    return set(w for w in text.lower().split() if len(w) >= 3)


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "docs/ui-redesign/evidence/t_266bfba3"
    failed = 0
    report = []
    report.append("UI-HOME-18 quick-action description completeness (HARD GATE)")
    report.append("Method (UI-HOME-06 proven): main-panel crop, 4x upscale, OCR, significant-word")
    report.append("(>=3 chars) containment. Every content word of each approved description must")
    report.append("be present in at least one shot of the width (scrolled series covers the fold).")
    report.append("")
    for width in WIDTHS:
        shots = sorted(
            glob(os.path.join(out_dir, f"{TASK_ID}_home_{width}x*.png"))
            + glob(os.path.join(out_dir, f"{TASK_ID}_home_{width}x*_series", "*.png"))
        )
        report.append(f"[{width}] {len(shots)} shot(s)")
        width_ok = {d: False for d in DESCRIPTIONS}
        per_shot = {}
        for shot in shots:
            name = os.path.basename(shot)
            norm = ocr_main_panel(shot)
            got = words_in(norm)
            per_shot[name] = []
            for desc in DESCRIPTIONS:
                want = words_in(desc)
                missing = want - got
                if not missing:
                    per_shot[name].append(f"  OK   {desc}")
                    width_ok[desc] = True
                else:
                    per_shot[name].append(f"  CHK  {desc} (missing words: {sorted(missing)})")
        for name, rows in per_shot.items():
            report.append(f"  {name}:")
            report.extend(rows)
        for desc in DESCRIPTIONS:
            if not width_ok[desc]:
                report.append(f"  >>> FAIL: {desc!r} NOT fully visible in ANY shot at width {width}")
                failed = 1
        report.append("")
    if failed:
        report.append("RESULT: CLIPPING DETECTED — REJECT")
    else:
        report.append("RESULT: all four descriptions fully visible at every width (in-view shots)")
    out = "\n".join(report) + "\n"
    with open(os.path.join(out_dir, "quick_action_clip_check.txt"), "w") as f:
        f.write(out)
    print(out)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
