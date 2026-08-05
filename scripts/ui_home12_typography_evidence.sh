#!/usr/bin/env bash
# Capture UI-HOME-12 typography evidence on the HOME screen (Screen::ChatList).
#
# Usage: scripts/ui_home12_typography_evidence.sh <before|after>
#
# Verifies the home-screen typography migration at three supported widths:
#   wide 1600x900, medium 1280x800 (reference), narrow 1024x720.
#
# Pattern: fresh temp data dir, --no-dht --no-relay, no subcommand -> the app
# lands on the ChatList (home) screen in the truthful fresh-launch Connecting
# state. Each instance is window-resized to the target viewport and captured
# with ImageMagick `import`. The 1280x800 reference shot is additionally
# OCR-checked for the quick-action descriptions (must remain unclipped).
#
# Output: docs/ui-redesign/evidence/t_4c86d88c/t_4c86d88c_home_<w>x<h>_<phase>.png
set -euo pipefail

PHASE="${1:-after}"
case "$PHASE" in
    before|after) ;;
    *) printf 'usage: %s <before|after>\n' "$0" >&2; exit 2 ;;
esac

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
TASK_ID="t_4c86d88c"
OUT="$ROOT/docs/ui-redesign/evidence/$TASK_ID"
mkdir -p "$OUT"
[[ -x "$BIN" ]] || { printf 'GUI binary not found: %s\n' "$BIN" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 190 230); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 190..230\n' >&2
    return 1
}

for spec in '1600 900' '1280 800' '1024 720'; do
    read -r w h <<<"$spec"
    display=$(find_display)
    data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home12.XXXXXX")
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home12-xvfb-$w-$PHASE.log 2>&1 & xv=$!
    sleep 0.5
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-HOME-12 $w" \
        >/tmp/boru-home12-app-$w-$PHASE.log 2>&1 & app=$!
    win=''
    for _ in $(seq 1 80); do
        win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
        [[ -n "$win" ]] && break
        sleep 0.25
    done
    if [[ -z "$win" ]]; then
        kill "$app" "$xv" 2>/dev/null || true
        rm -rf "$data"
        printf 'window not found for %sx%s\n' "$w" "$h" >&2
        exit 1
    fi
    DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
    sleep 5
    DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_home_${w}x${h}_${PHASE}.png"
    kill "$app" "$xv" 2>/dev/null || true
    wait "$app" 2>/dev/null || true
    wait "$xv" 2>/dev/null || true
    rm -rf "$data"
done

file "$OUT"/${TASK_ID}_home_*_${PHASE}.png

# ── Clipping check (after phase only): OCR the 1280 reference for the four
# quick-action descriptions. 13 px supporting text is too small for tesseract
# at native scale, so the left-column region is cropped and upscaled 3x first.
# Descriptions may wrap to two lines, so match per-line fragments.
if [[ "$PHASE" == "after" ]] && command -v tesseract >/dev/null 2>&1; then
    shot="$OUT/${TASK_ID}_home_1280x800_after.png"
    region=$(mktemp --suffix=.png)
    python3 - "$shot" "$region" <<'PY'
import sys
from PIL import Image
img = Image.open(sys.argv[1])
crop = img.crop((24, 360, 900, 780))  # left column: hero/mesh/quick actions
crop = crop.resize((crop.width * 3, crop.height * 3), Image.LANCZOS)
crop.save(sys.argv[2])
PY
    txt=$(mktemp)
    tesseract "$region" "$txt" >/dev/null 2>&1 || true
    text=$(cat "$txt.txt" 2>/dev/null || true)
    rm -f "$region" "$txt" "$txt.txt"
    ok=1
    for fragment in "anyone to join" "group conversation" "public key" "share in a chat"; do
        if ! grep -qF "$fragment" <<<"$text"; then
            printf 'OCR MISS: quick-action description fragment not found: %s\n' "$fragment" >&2
            ok=0
        fi
    done
    if [[ $ok -eq 1 ]]; then
        printf 'OCR OK: all four quick-action descriptions visible at 1280x800 (no clipping)\n'
    else
        printf 'OCR CHECK FAILED: one or more quick-action descriptions missing/clipped\n' >&2
        exit 1
    fi
fi
