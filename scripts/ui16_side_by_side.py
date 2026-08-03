#!/usr/bin/env python3
"""Build the UI-16 side-by-side comparison image (kanban t_dfbde136).

Places the Figure 4 target (docs/ui-redesign/evidence/ui-13-fixture/
target-figure4.png) next to the implementation capture at 1280x800, scaled
to a common chat-panel height with matching label strips.
"""
from __future__ import annotations

import os
import sys

from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TARGET = os.path.join(ROOT, "docs/ui-redesign/evidence/ui-13-fixture/target-figure4.png")
IMPL = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    ROOT, "docs/ui-redesign/evidence/ui-16/t_dfbde136_figure4_1280x800.png")
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
    ROOT, "docs/ui-redesign/evidence/ui-16/t_dfbde136_side_by_side_1280x800.png")

LABEL_H = 36
PAD = 20


def label_strip(text: str, width: int) -> Image.Image:
    im = Image.new("RGB", (width, LABEL_H), (240, 240, 240))
    d = ImageDraw.Draw(im)
    try:
        font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 18)
    except OSError:
        font = ImageFont.load_default()
    d.text((10, 8), text, fill=(40, 40, 40), font=font)
    return im


def main() -> int:
    target = Image.open(TARGET)
    impl = Image.open(IMPL)
    # Scale both to a common chat-panel height (target is full-window mockup).
    panel_h = 480
    target = target.resize((int(target.width * panel_h / target.height), panel_h), Image.LANCZOS)
    impl = impl.resize((int(impl.width * panel_h / impl.height), panel_h), Image.LANCZOS)

    t_strip = label_strip("Figure 4 — target", target.width)
    i_strip = label_strip(f"Implementation — {os.path.basename(IMPL)}", impl.width)
    gap = 24

    canvas_w = PAD + target.width + gap + impl.width + PAD
    canvas_h = LABEL_H + panel_h + PAD * 2
    canvas = Image.new("RGB", (canvas_w, canvas_h), (255, 255, 255))
    canvas.paste(t_strip, (PAD, PAD))
    canvas.paste(target, (PAD, PAD + LABEL_H))
    x0 = PAD + target.width + gap
    canvas.paste(i_strip, (x0, PAD))
    canvas.paste(impl, (x0, PAD + LABEL_H))
    canvas.save(OUT)
    print(f"side-by-side written: {OUT} ({canvas.width}x{canvas.height})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
