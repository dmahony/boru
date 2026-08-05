#!/usr/bin/env python3
"""FS-23 helper: locate the green 'Share Files or Folder' button in a boru
window screenshot and (optionally) click it via xdotool.

Usage:
  find_share_button.py <screenshot.png> [--click [--open-menu]]
"""
from __future__ import annotations
import subprocess
import sys
from collections import deque

from PIL import Image


def green_blobs(path: str):
    img = Image.open(path).convert("RGB")
    w, h = img.size
    pixels = img.load()
    mask = [[False] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            r, g, b = pixels[x, y]
            if g > 90 and g < 200 and g > r + 40 and g > b + 40 and r < 120:
                mask[y][x] = True
    seen = [[False] * w for _ in range(h)]
    blobs = []
    for y in range(h):
        for x in range(w):
            if mask[y][x] and not seen[y][x]:
                q = deque([(x, y)])
                seen[y][x] = True
                xs, ys = [], []
                while q:
                    cx, cy = q.popleft()
                    xs.append(cx)
                    ys.append(cy)
                    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                        nx, ny = cx + dx, cy + dy
                        if 0 <= nx < w and 0 <= ny < h and mask[ny][nx] and not seen[ny][nx]:
                            seen[ny][nx] = True
                            q.append((nx, ny))
                if len(xs) > 200:
                    blobs.append((len(xs), min(xs), max(xs), min(ys), max(ys)))
    blobs.sort(reverse=True)
    return blobs, (w, h)


def main() -> int:
    path = sys.argv[1]
    blobs, (w, h) = green_blobs(path)
    # The share button: a wide green blob in the lower half (below the header).
    candidates = [b for b in blobs if b[2] - b[1] > 150 and b[4] > 250]
    if not candidates:
        print(f"NO_BUTTON blobs={blobs[:6]}")
        return 1
    n, x0, x1, y0, y1 = candidates[0]
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    print(f"BUTTON center=({cx},{cy}) bbox=({x0},{y0})-({x1},{y1}) size={n}")
    if "--click" in sys.argv:
        subprocess.run(["xdotool", "mousemove", str(cx), str(cy), "click", "1"], check=True)
        print(f"CLICKED ({cx},{cy})")
        if "--open-menu" in sys.argv:
            # 'Share Files...' sits ~53px below the button center, slightly right.
            mx, my = cx + 20, cy + 53
            subprocess.run(["xdotool", "mousemove", str(mx), str(my), "click", "1"], check=True)
            print(f"CLICKED_MENU_ITEM ({mx},{my})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
