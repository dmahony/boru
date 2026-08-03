#!/usr/bin/env python3
"""Assert PASS/FAIL for the scroll-behavior probe (t_727c1d5e).

Reads the five OCR dumps produced by scripts/scroll_probe.sh and checks the
visible "Seed msg NNN" range in each state against the acceptance criteria:

  state 1 after_open           — fresh conversation with history snaps to
                                 latest: newest messages (>= 50) visible.
  state 2 scrolled_up          — wheel up shows older messages: max seen
                                 drops well below the latest (<= 48).
  state 3 live_append_scrolled — live incoming appends while scrolled up must
                                 NOT move the reading position: still old
                                 messages, and within a small tolerance of
                                 state 2's position.
  state 4 back_to_bottom       — wheel down returns to the latest (>= 50).
  state 5 append_at_bottom     — live incoming while at bottom snaps to the
                                 newest (>= 50).

Usage: python3 scripts/scroll_probe_check.py <probe_out_dir>
Exit code 0 = all states PASS, 1 = at least one FAIL.
"""
import re
import sys
from pathlib import Path

# Seed bodies are "Seed msg {i:03d} - ..." (scripts/seed_chat_history.py).
# OCR often drops leading zeros, so match the number loosely.
SEED_MSG_RE = re.compile(r"Seed msg\s*(\d{1,3})")

# With 60 seeded entries, "latest" means messages in the 50s-60s are on
# screen; "scrolled up" (8 wheel notches up) lands in the 30s-40s band.
LATEST_MIN = 50
SCROLLED_UP_MAX = 48
POSITION_TOLERANCE = 3


def max_seed_msg(ocr_path: Path):
    if not ocr_path.exists():
        return None
    nums = [int(m) for m in SEED_MSG_RE.findall(ocr_path.read_text())]
    return max(nums) if nums else None


def main() -> int:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/scroll-probe")
    states = {
        "1 after_open": "after_open.ocr",
        "2 scrolled_up": "scrolled_up.ocr",
        "3 live_append_scrolled": "live_append_scrolled.ocr",
        "4 back_to_bottom": "back_to_bottom.ocr",
        "5 append_at_bottom": "append_at_bottom.ocr",
    }

    maxima = {}
    failures = []
    for label, filename in states.items():
        maxima[label] = max_seed_msg(out / filename)

    # State 1: fresh open lands at the latest message.
    m1 = maxima["1 after_open"]
    if m1 is None:
        failures.append("state 1: no seed messages OCR'd")
    elif m1 < LATEST_MIN:
        failures.append(f"state 1: expected latest (>= {LATEST_MIN}), saw max {m1}")

    # State 2: scrolled up into older history.
    m2 = maxima["2 scrolled_up"]
    if m2 is None:
        failures.append("state 2: no seed messages OCR'd")
    elif m2 > SCROLLED_UP_MAX:
        failures.append(f"state 2: expected older messages (<= {SCROLLED_UP_MAX}), saw max {m2}")

    # State 3: live appends while scrolled up must not move the reading
    # position — still old messages, within tolerance of state 2.
    m3 = maxima["3 live_append_scrolled"]
    if m3 is None:
        failures.append("state 3: no seed messages OCR'd")
    elif m3 > SCROLLED_UP_MAX:
        failures.append(f"state 3: reading position moved toward latest (max {m3})")
    elif m2 is not None and abs(m3 - m2) > POSITION_TOLERANCE:
        failures.append(
            f"state 3: reading position drifted (state2 max {m2} -> state3 max {m3})"
        )

    # State 4: wheel down returns to the latest.
    m4 = maxima["4 back_to_bottom"]
    if m4 is None:
        failures.append("state 4: no seed messages OCR'd")
    elif m4 < LATEST_MIN:
        failures.append(f"state 4: expected latest (>= {LATEST_MIN}), saw max {m4}")

    # State 5: append while at bottom snaps to the newest message.
    m5 = maxima["5 append_at_bottom"]
    if m5 is None:
        failures.append("state 5: no seed messages OCR'd")
    elif m5 < LATEST_MIN:
        failures.append(f"state 5: expected latest (>= {LATEST_MIN}), saw max {m5}")

    print(f"probe maxima: { {k: v for k, v in maxima.items()} }")
    if failures:
        for f in failures:
            print(f"FAIL  {f}")
        print("scroll_probe: FAIL")
        return 1
    print("scroll_probe: 5/5 PASS (states 1-5) — deterministic OCR check")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
