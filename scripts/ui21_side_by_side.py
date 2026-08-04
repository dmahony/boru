#!/usr/bin/env python3
"""UI-21 side-by-side comparisons (kanban t_8960f71c).

Builds target-vs-implementation montages at 1280x800 for the home screen
(Figure 3) and the chat screen (Figure 4) using the authoritative targets
extracted from the implementation plan PDF, placed next to the running-app
captures produced by scripts/ui21_final_evidence.sh.

Both halves are scaled to a common panel height so hierarchy/spacing can be
compared directly; each half carries a label strip.
"""
from __future__ import annotations

import os
import sys

from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EVIDENCE = os.path.join(ROOT, "docs/ui-redesign/evidence/final")

# Authoritative targets extracted from the plan PDF (pages 5 & 6).
TARGET_HOME = "/tmp/boru_targets/fig-000.png"   # Figure 3 - target home screen
TARGET_CHAT = "/tmp/boru_targets/fig-001.png"   # Figure 4 - target chat screen

IMPL_HOME = os.path.join(EVIDENCE, "final_home_1280x800.png")
IMPL_CHAT = os.path.join(EVIDENCE, "final_chat_1280x800.png")
OUT_HOME = os.path.join(EVIDENCE, "side_by_side_home_1280x800.png")
OUT_CHAT = os.path.join(EVIDENCE, "side_by_side_chat_1280x800.png")

LABEL_H = 36
PAD = 20
PANEL_H = 620


def label_strip(text: str, width: int) -> Image.Image:
    im = Image.new("RGB", (width, LABEL_H), (235, 238, 236))
    d = ImageDraw.Draw(im)
    try:
        font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 18)
    except OSError:
        font = ImageFont.load_default()
    d.text((10, 8), text, fill=(30, 34, 31), font=font)
    return im


def montage(target_path: str, impl_path: str, target_label: str, impl_label: str, out: str) -> None:
    target = Image.open(target_path).convert("RGB")
    impl = Image.open(impl_path).convert("RGB")
    target = target.resize((int(target.width * PANEL_H / target.height), PANEL_H), Image.LANCZOS)
    impl = impl.resize((int(impl.width * PANEL_H / impl.height), PANEL_H), Image.LANCZOS)

    t_strip = label_strip(target_label, target.width)
    i_strip = label_strip(impl_label, impl.width)
    gap = 28

    canvas_w = PAD + target.width + gap + impl.width + PAD
    canvas_h = LABEL_H + PANEL_H + PAD * 2
    canvas = Image.new("RGB", (canvas_w, canvas_h), (255, 255, 255))
    canvas.paste(t_strip, (PAD, PAD))
    canvas.paste(target, (PAD, PAD + LABEL_H))
    x0 = PAD + target.width + gap
    canvas.paste(i_strip, (x0, PAD))
    canvas.paste(impl, (x0, PAD + LABEL_H))
    canvas.save(out)
    print(f"written: {out} ({canvas.width}x{canvas.height})")


def main() -> int:
    for target, impl, tlabel, ilabel, out in (
        (TARGET_HOME, IMPL_HOME, "Figure 3 — target home", "Implementation — home 1280x800", OUT_HOME),
        (TARGET_CHAT, IMPL_CHAT, "Figure 4 — target chat", "Implementation — chat 1280x800", OUT_CHAT),
    ):
        if not os.path.exists(impl):
            print(f"SKIP: implementation capture missing: {impl}", file=sys.stderr)
            continue
        montage(target, impl, tlabel, ilabel, out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
