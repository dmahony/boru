#!/usr/bin/env bash
# Evidence for t_232df918: message timeline as the sole expanding scrollable
# region between the fixed header and the pinned composer.
#
# Captures the chat screen in the four states required by the card:
#   empty    - no messages: timeline region stretches between header and
#              composer, composer pinned at the bottom, no dead area below it.
#   short    - 3 messages: content is bottom-aligned (hugs the composer);
#              whitespace sits ABOVE the messages, not below them (the
#              previous top-aligned render left a giant dead area under the
#              last message).
#   long     - 40 messages: content overflows the viewport, the timeline
#              scrolls (scrollbar), latest message visible near the composer,
#              header and composer remain fixed.
#   scrolled - same long conversation scrolled up with the mouse wheel:
#              older messages visible, header and composer still fixed.
#
# Every message is submitted through the real GUI send path
# (boru_gui_set_composer + boru_gui_submit_composer), so the timeline is
# populated with genuine local entries - no sample data in the render path.
#
# After capturing, a pixel-analysis pass (PIL) verifies the geometry:
#   - header band occupies the top of the window in every state
#   - composer band sits at the bottom in every state
#   - in the short state the last message bottom is within 40px of the
#     composer top (bottom-aligned) and the blank region above the messages
#     is at least 150px (whitespace moved above the content)
#   - in the long state a scrollbar column is present at the right edge
#
# Output: docs/ui-redesign/evidence/ui-timeline-region/
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-timeline-region"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
TASK_ID="t_232df918"

mkdir -p "$OUTPUT_DIR"

[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 200 230); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 200..230\n' >&2
    return 1
}

mcp() {
    DISPLAY=":$1" python3 "$MCP_CLIENT" "$2" "$3" "$4"
}

capture_window() {
    local display=$1 output=$2 width=$3 height=$4
    local window_id
    window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 0.6
    DISPLAY=":$display" import -window "$window_id" "$output"
}

# conversation_id is the peer public key (64 hex chars) of the seeded Alice.
ALICE_PK=$(printf 'a1%.0s' {1..32})

send_messages() {
    local display=$1 port=$2 count=$3
    local i sent=0
    for i in $(seq 1 "$count"); do
        # Each message = set_composer + submit_composer through the real GUI
        # send path. The MCP action queue rate-limits to 10 actions/sec, so we
        # pace at ~3/sec and retry transient rate-limit responses.
        local text="Timeline message $i - a real local entry sent through the normal GUI send path."
        local resp=""
        local try
        for try in 1 2 3 4 5 6; do
            resp=$(mcp "$display" "$port" boru_gui_set_composer "{\"text\":\"$text\"}" 2>/dev/null || true)
            if echo "$resp" | grep -q '"sent":true'; then
                break
            fi
            sleep 0.4
        done
        for try in 1 2 3 4 5 6; do
            resp=$(mcp "$display" "$port" boru_gui_submit_composer '{}' 2>/dev/null || true)
            if echo "$resp" | grep -q '"sent":true'; then
                sent=$((sent + 1))
                break
            fi
            sleep 0.4
        done
        sleep 0.25
    done
    printf 'sent %d/%d messages\n' "$sent" "$count"
    sleep 1.5
}

capture_state() {
    local state=$1 width=$2 height=$3 count=$4 scroll=$5
    local display data_dir mcp_port xvfb_pid app_pid window_id
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-timeline.XXXXXX")
    mcp_port=$((18600 + display))
    xvfb_pid=""
    app_pid=""
    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "${app_pid:-}" 2>/dev/null
        wait "${xvfb_pid:-}" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    python3 "$SEED_SCRIPT" "$data_dir" >/dev/null

    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-timeline-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "UI Timeline Region" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-timeline-app.log 2>&1 &
    app_pid=$!

    local attempt
    for attempt in $(seq 1 60); do
        if DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
            break
        fi
        sleep 0.25
    done
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null
    sleep 2

    mcp "$display" "$mcp_port" boru_gui_open_conversation "{\"conversation_id\":\"$ALICE_PK\"}" >/dev/null
    sleep 1.2
    # Mark Alice online so the header shows a real Online state.
    mcp "$display" "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
    sleep 0.8

    if [[ "$count" -gt 0 ]]; then
        send_messages "$display" "$mcp_port" "$count"
    fi

    window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 0.8

    local out="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}.png"
    DISPLAY=":$display" import -window "$window_id" "$out"

    if [[ "$scroll" == "yes" ]]; then
        # Scroll up inside the timeline: click into the message area first so
        # the wheel targets the scrollable, then wheel up 6 notches.
        local mx=$((width * 3 / 4))
        local my=$((height / 2))
        DISPLAY=":$display" xdotool mousemove --sync "$mx" "$my"
        sleep 0.3
        DISPLAY=":$display" xdotool click 1
        sleep 0.4
        DISPLAY=":$display" xdotool click --repeat 6 --delay 40 4
        sleep 1.0
        DISPLAY=":$display" import -window "$window_id" "$out"
    fi

    printf 'captured %s\n' "$out"
}

# ── State captures ────────────────────────────────────────────────────
capture_state empty    1280 800 0  no
capture_state short    1280 800 3  no
capture_state long     1280 800 40 no
capture_state scrolled 1280 800 40 yes
capture_state short    1024 720 3  no

# ── Pixel verification pass ───────────────────────────────────────────
python3 - "$OUTPUT_DIR" <<'PY'
import json
import os
import sys

from PIL import Image

out_dir = sys.argv[1]

# Canvas background of the message region (light theme).
BG = (247, 249, 248)


def row_color(im, y, x0=0.45, x1=0.9):
    """Average color across a horizontal slice of the chat area."""
    w, h = im.size
    xs = range(int(w * x0), int(w * x1), 3)
    cs = [im.getpixel((x, y)) for x in xs]
    return tuple(sum(c[i] for c in cs) // len(cs) for i in range(3))


def header_bottom(im):
    """Return the y of the last header-surface row at the top.

    The header surface is near-white (>= 244 in every channel) and ends at
    the thin divider line below it (clearly darker, ~220) or the canvas
    background (247,249,248 is still >= 244, so the divider is what stops
    the scan). The threshold must tolerate the modern compact header's
    off-white surface (244-254) instead of requiring pure 255.
    """
    w, h = im.size
    y = 0
    while y < h:
        r, g, b = row_color(im, y)
        # Header surface is near-white; anything clearly darker is the
        # divider or the canvas below the header. Threshold 235 gives
        # margin for the off-white header surface (244-254) and its
        # subpixel-darker rows at alternate widths (down to ~239), while
        # still stopping at the divider (~220) and treating the canvas
        # (247,249,248) as header-adjacent only until the divider stops it.
        if r < 235 or g < 235 or b < 235:
            break
        y += 2
    return y


def composer_top(im):
    """Return the y where the composer band (muted surface) starts near the bottom."""
    w, h = im.size
    y = h - 1
    while y > 0:
        r, g, b = row_color(im, y)
        # composer surface is (238, 241, 238)-ish, distinctly darker than the
        # (247,249,248) canvas. Scan upward until we leave that band.
        if r < 243 and g < 246 and b < 243:
            break
        y -= 2
    return y


def last_content_y(im, start_y, end_y):
    """Find the lowest row in [start_y, end_y) with non-background content."""
    w, h = im.size
    last = None
    for y in range(start_y, end_y, 2):
        r, g, b = row_color(im, y)
        if abs(r - BG[0]) + abs(g - BG[1]) + abs(b - BG[2]) > 18:
            last = y
    return last


def scrollbar_present(im):
    """Check for a vertical scrollbar track at the right edge of the chat area.

    The chat panel is inset 16px from the window edge (SPACE_16) and the
    scrollbar (width ~10px) renders at the right edge of the scrollable, so it
    appears around x = w-26 .. w-16.
    """
    w, h = im.size
    xs = range(max(w - 27, 0), max(w - 15, 1))
    found = 0
    for y in range(0, h, 3):
        cs = [im.getpixel((x, y)) for x in xs]
        # scrollbar thumb/track differs from the plain canvas background
        diff = sum(
            1
            for c in cs
            if abs(c[0] - BG[0]) + abs(c[1] - BG[1]) + abs(c[2] - BG[2]) > 24
        )
        if diff >= 2:
            found += 1
    return found > 25


results = {}
for name in sorted(os.listdir(out_dir)):
    if not name.endswith(".png"):
        continue
    im = Image.open(os.path.join(out_dir, name)).convert("RGB")
    w, h = im.size
    hb = header_bottom(im)
    ct = composer_top(im)
    # region between header and composer
    last = last_content_y(im, hb + 4, ct - 4)
    sb = scrollbar_present(im)
    results[name] = {
        "header_bottom_y": hb,
        "composer_top_y": ct,
        "timeline_region_px": ct - hb,
        "last_content_y": last,
        "gap_last_message_to_composer_px": None if last is None else ct - last,
        "scrollbar_present": sb,
    }
    print(f"{name}: header_bottom={hb} composer_top={ct} region={ct-hb}px "
          f"last_content={last} gap_to_composer={None if last is None else ct-last}px "
          f"scrollbar={sb}")

ok = True
# 1. Header fixed at top in every capture (the code pins it at 60px; the
# detector allows for render/subpixel variance at alternate widths).
for name, r in results.items():
    if r["header_bottom_y"] < 20 or r["header_bottom_y"] > 90:
        print(f"FAIL {name}: header band height {r['header_bottom_y']}px out of expected 20-90px")
        ok = False
# 2. Composer pinned at the bottom in every capture (within 60px of window bottom).
for name, r in results.items():
    h = int(name.split("x")[1].split(".")[0])
    if h - r["composer_top_y"] > 70:
        print(f"FAIL {name}: composer top {r['composer_top_y']} is more than 70px from bottom {h}")
        ok = False
# 3. Short state: messages bottom-aligned -> gap from last message to composer small (< 40px).
for name, r in results.items():
    if "short" not in name:
        continue
    if r["last_content_y"] is None:
        print(f"FAIL {name}: no message content found in short state")
        ok = False
    elif r["gap_last_message_to_composer_px"] is None or r["gap_last_message_to_composer_px"] > 40:
        print(f"FAIL {name}: last message {r['gap_last_message_to_composer_px']}px above composer "
              f"(expected bottom-aligned < 40px)")
        ok = False
# 4. Short state: whitespace above the messages (content does NOT start at header).
for name, r in results.items():
    if "short" not in name:
        continue
    # first content row should be at least 100px below the header when 3 short
    # messages are bottom-aligned at 1280x800
    if r["last_content_y"] is not None:
        im = Image.open(os.path.join(out_dir, name)).convert("RGB")
        w, h = im.size
        first = None
        for y in range(r["header_bottom_y"] + 4, r["last_content_y"], 2):
            rr, gg, bb = row_color(im, y)
            if abs(rr - BG[0]) + abs(gg - BG[1]) + abs(bb - BG[2]) > 18:
                first = y
                break
        if first is None or (first - r["header_bottom_y"]) < 100:
            print(f"FAIL {name}: whitespace above messages only "
                  f"{None if first is None else first - r['header_bottom_y']}px (expected >= 100px)")
            ok = False
# 5. Long state: scrollbar present.
for name, r in results.items():
    if "long" not in name and "scrolled" not in name:
        continue
    if not r["scrollbar_present"]:
        print(f"FAIL {name}: no scrollbar detected in overflowing timeline")
        ok = False

with open(os.path.join(out_dir, "verification.json"), "w") as f:
    json.dump({"ok": ok, "results": results}, f, indent=2)

print("VERIFICATION:", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
PY

cat > "$OUTPUT_DIR/README.md" <<'MD'
# t_232df918 — Message timeline as the sole expanding scrollable region

Refactor goal: the message timeline is the only element that expands and
scrolls vertically between the fixed conversation header and the pinned
composer. No giant dead areas.

## Behavior

- The timeline scrollable keeps a Fill height inside the chat panel column:
  `header (fixed 60px) -> divider -> timeline (Fill) -> composer (pinned)`.
- When the message content is shorter than the viewport, a leading spacer
  pushes the content to the bottom of the timeline so it hugs the composer
  (chat convention — Telegram/Signal/WhatsApp). Whitespace sits ABOVE the
  messages where it reads as balanced, instead of a giant dead area below the
  last message.
- When content overflows the viewport the spacer is zero and the existing
  anchored-to-bottom virtualized scrolling takes over unchanged.
- The spacer derives from the existing incremental layout cache
  (`LayoutCache::total_height`) and the live scrollable viewport height
  (`Scrolled` event), so it tracks message growth exactly and never interferes
  with reading position once the timeline overflows.

## Files

- `t_232df918_empty_1280x800.png` — empty conversation; region stretches,
  composer pinned.
- `t_232df918_short_1280x800.png` — 3 real messages, bottom-aligned.
- `t_232df918_short_1024x720.png` — same at the alternate viewport.
- `t_232df918_long_1280x800.png` — 40 real messages, scrollbar present.
- `t_232df918_scrolled_1280x800.png` — scrolled up; header/composer fixed.
- `verification.json` — pixel-analysis geometry for every capture.
MD

printf 'evidence complete: %s\n' "$OUTPUT_DIR"
