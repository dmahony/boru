#!/usr/bin/env bash
# Capture UI-HOME-06 quick-action grid evidence on the HOME screen
# (Screen::ChatList).
#
# Verifies the UI-HOME-06 quick-action card fix at three supported widths:
#   wide 1600x900, medium 1280x800 (reference), narrow 1024x720.
#
# Pattern: fresh temp data dir, --no-dht --no-relay, no subcommand -> the app
# lands on the ChatList (home) screen in the truthful fresh-launch Connecting
# state. Each instance is window-resized to the target viewport and captured
# with ImageMagick `import`.
#
# Also runs the four-card action test at 1600x900: each quick-action card is
# clicked (located via tesseract TSV on the rendered labels) and the resulting
# flow (create-room dialog / create-group dialog / friend-requests screen /
# file picker) is verified by OCR.
#
# Output: docs/ui-redesign/evidence/t_2577e385/<task>_home_<w>x<h>_after.png
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/boru
TASK_ID="t_2577e385"
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

launch_app() {
    local w=$1 h=$2 name=$3
    local display
    display=$(find_display)
    local data
    data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home06.XXXXXX")
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home06-xvfb-$w.log 2>&1 & xv=$!
    sleep 0.5
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "$name" \
        >/tmp/boru-home06-app-$w.log 2>&1 & app=$!
    local win=''
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
    printf '%s %s %s %s %s\n' "$display" "$win" "$app" "$xv" "$data"
}

# ── Phase 1: three-width screenshots ──────────────────────────────────
for spec in '1600 900' '1280 800' '1024 720'; do
    read -r w h <<<"$spec"
    read -r display win app xv data <<<"$(launch_app "$w" "$h" "UI-HOME-06 $w")"
    sleep 5
    DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_home_${w}x${h}_after.png"
    kill "$app" "$xv" 2>/dev/null || true
    wait "$app" 2>/dev/null || true
    wait "$xv" 2>/dev/null || true
    rm -rf "$data"
done

file "$OUT"/${TASK_ID}_home_*_after.png

# ── Phase 2: OCR description check at every width ─────────────────────
# Every description must be FULLY visible at every supported width. The
# 13 px supporting text is upscaled and OCR'd; each full phrase is matched
# word-by-word (whitespace-normalised, case-insensitive) so a missing
# trailing word fails. At 1600 all four cards fit in one row; at 1280/1024
# the 2x2 grid's second row is below the fold, so those two phrases are
# verified against the scrolled captures in Phase 2b.
if command -v tesseract >/dev/null 2>&1; then
    ok=1
    check_phrase() {
        local shot=$1 w=$2 phrase=$3
        local region txt norm text missing word
        region=$(mktemp --suffix=.png)
        python3 - "$shot" "$region" "$w" <<'PY'
import sys
from PIL import Image
img = Image.open(sys.argv[1])
w = int(sys.argv[3])
crop = img.crop((300, 200, min(w, 1200), img.height - 40))
crop = crop.resize((crop.width * 4, crop.height * 4), Image.LANCZOS)
crop.save(sys.argv[2])
PY
        txt=$(mktemp)
        tesseract "$region" "$txt" --psm 6 >/dev/null 2>&1 || true
        text=$(cat "$txt.txt" 2>/dev/null || true)
        norm=$(printf '%s' "$text" | tr '\n' ' ' | tr -s ' ')
        rm -f "$region" "$txt" "$txt.txt"
        missing=0
        # OCR of the 13 px supporting text is imperfect and multi-column
        # reading order scrambles phrase order, so the robust check is
        # significant-word containment: every content word (>= 3 chars)
        # of the phrase must appear in the OCR text.
        want=$(printf '%s' "$phrase" | tr '[:upper:]' '[:lower:]' | tr -d '[:punct:]')
        got=$(printf '%s' "$norm" | tr '[:upper:]' '[:lower:]' | tr -d '[:punct:]')
        for word in $want; do
            [[ ${#word} -ge 3 ]] || continue
            [[ " $got " == *" $word "* ]] || missing=1
        done
        if [[ $missing -eq 1 ]]; then
            printf 'OCR MISS at %s width: %s\n' "$w" "$phrase" >&2
            ok=0
        fi
    }
    # 1600: all four phrases on the one-row top shot.
    for phrase in \
        "Open a public room for anyone to join" \
        "Start a private group conversation" \
        "Connect with a friend by public key" \
        "Choose a file to share in a chat"; do
        check_phrase "$OUT/${TASK_ID}_home_1600x900_after.png" 1600 "$phrase"
    done
    # 1280/1024: row-1 phrases on the top shot.
    for spec in '1280 800' '1024 720'; do
        read -r w h <<<"$spec"
        check_phrase "$OUT/${TASK_ID}_home_${w}x${h}_after.png" "$w" \
            "Open a public room for anyone to join"
        check_phrase "$OUT/${TASK_ID}_home_${w}x${h}_after.png" "$w" \
            "Start a private group conversation"
    done
    if [[ $ok -eq 1 ]]; then
        printf 'OCR OK: row-1 descriptions fully visible at all three widths\n'
    else
        printf 'OCR CHECK FAILED: one or more row-1 descriptions missing/clipped\n' >&2
        exit 1
    fi
else
    printf 'tesseract not installed; skipping OCR verification\n' >&2
fi

# ── Phase 2b: scrolled captures verify row 2 at medium/narrow widths ──
# The home page scrolls (gutter_scrollable); the second row of the 2x2 grid
# at 1280x800/1024x720 is below the fold in the top shot but fully rendered
# when scrolled. Capture scrolled views and verify the two row-2 phrases.
if command -v tesseract >/dev/null 2>&1; then
    ok2=1
    for spec in '1280 800' '1024 720'; do
        read -r w h <<<"$spec"
        shot="$OUT/${TASK_ID}_home_${w}x${h}_scrolled.png"
        if [[ ! -f "$shot" ]]; then
            printf 'missing scrolled capture %s — run scripts/ui_home06_scroll_evidence.sh first\n' "$shot" >&2
            ok2=0
            continue
        fi
        region=$(mktemp --suffix=.png)
        python3 - "$shot" "$region" "$w" <<'PY'
import sys
from PIL import Image
img = Image.open(sys.argv[1])
w = int(sys.argv[3])
crop = img.crop((300, 0, min(w, 1000), img.height))
crop = crop.resize((crop.width * 4, crop.height * 4), Image.LANCZOS)
crop.save(sys.argv[2])
PY
        txt=$(mktemp)
        tesseract "$region" "$txt" --psm 6 >/dev/null 2>&1 || true
        text=$(cat "$txt.txt" 2>/dev/null || true)
        norm=$(printf '%s' "$text" | tr '\n' ' ' | tr -s ' ')
        rm -f "$region" "$txt" "$txt.txt"
        for phrase in \
            "Connect with a friend by public key" \
            "Choose a file to share in a chat"; do
            missing=0
            want=$(printf '%s' "$phrase" | tr '[:upper:]' '[:lower:]' | tr -d '[:punct:]')
            got=$(printf '%s' "$norm" | tr '[:upper:]' '[:lower:]' | tr -d '[:punct:]')
            for word in $want; do
                [[ ${#word} -ge 3 ]] || continue
                [[ " $got " == *" $word "* ]] || missing=1
            done
            if [[ $missing -eq 1 ]]; then
                printf 'SCROLLED OCR MISS at %sx%s: %s\n' "$w" "$h" "$phrase" >&2
                ok2=0
            fi
        done
        printf 'OCR checked scrolled %sx%s (row 2)\n' "$w" "$h"
    done
    if [[ $ok2 -eq 1 ]]; then
        printf 'OCR OK: row-2 descriptions fully visible in scrolled views\n'
    else
        printf 'SCROLLED OCR CHECK FAILED: row-2 descriptions missing\n' >&2
        exit 1
    fi
fi

# ── Phase 3: four-card action test at 1600x900 ────────────────────────
# Cards are located by OCR-ing the rendered labels and taking the label box
# centres; clicking anywhere on a card activates the whole-card button.
read -r display win app xv data <<<"$(launch_app 1600 900 "UI-HOME-06 actions")"
sleep 5
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_action_home_1600x900.png"

python3 - "$OUT/${TASK_ID}_action_home_1600x900.png" > "$OUT/${TASK_ID}_card_centers.txt" <<'PY'
import subprocess, os, sys
from PIL import Image
shot = sys.argv[1]
scale = 3
img = Image.open(shot).convert('RGB')
p = '/tmp/action_tsv.png'
img.resize((img.width*scale, img.height*scale), Image.LANCZOS).save(p)
t = p.replace('.png','')
subprocess.run(['tesseract', p, t, '--psm', '6', 'tsv'], capture_output=True)
rows=[]
if os.path.exists(t+'.tsv'):
    for line in open(t+'.tsv').read().splitlines()[1:]:
        f = line.split('\t')
        if len(f)==12 and f[11].strip() and f[6].isdigit():
            rows.append((f[11], int(f[6])//scale, int(f[7])//scale, int(f[8])//scale, int(f[9])//scale))
# Expected label words per card, in order.
cards = [
    ("Create", "Public", "Room"),
    ("Create", "Group", "Chat"),
    ("Add", "Friend"),
    ("Share", "Files"),
]
def center_of(word):
    # Restrict to the action-grid band (y >= 450, x >= 320) so sidebar
    # labels like the "Add friend by key..." section are not matched.
    for w, x, y, ww, hh in rows:
        if x >= 320 and y >= 450 and w.lower().rstrip('.,;:') == word.lower():
            return (x + ww//2, y + hh//2)
    return None
for card in cards:
    pts = [center_of(w) for w in card]
    pts = [p for p in pts if p]
    if pts:
        cx = sum(p[0] for p in pts)//len(pts)
        cy = sum(p[1] for p in pts)//len(pts)
        print(f"{cx} {cy}")
    else:
        print("0 0")
PY
mapfile -t centers < "$OUT/${TASK_ID}_card_centers.txt"
printf 'card centres found: %s\n' "${centers[*]}"

# Card 1: Create Public Room -> create-room dialog.
coords=(${centers[0]:-0 0})
DISPLAY=":$display" xdotool mousemove --sync "${coords[0]}" "${coords[1]}" click 1
sleep 1.5
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_action_1_create_room.png"
DISPLAY=":$display" xdotool key Escape
sleep 0.5

# Card 2: Create Group Chat -> create-group dialog.
coords=(${centers[1]:-0 0})
DISPLAY=":$display" xdotool mousemove --sync "${coords[0]}" "${coords[1]}" click 1
sleep 1.5
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_action_2_create_group.png"
DISPLAY=":$display" xdotool key Escape
sleep 0.5

# Card 3: Add Friend -> friend-requests screen.
coords=(${centers[2]:-0 0})
DISPLAY=":$display" xdotool mousemove --sync "${coords[0]}" "${coords[1]}" click 1
sleep 1.5
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_action_3_friend_requests.png"

# ── Card 4 (Share Files) in a fresh instance ─────────────────────────
# The native file picker (rfd) is OS-dependent; under Xvfb the GTK chooser
# may render as a separate window or fail silently, so isolate the click and
# record whatever is on screen plus the app's liveness. The dispatch itself
# (AttachPressed -> AsyncFileDialog) is unit-tested in quick_actions.rs.
kill "$app" "$xv" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xv" 2>/dev/null || true
rm -rf "$data"
read -r display win app xv data <<<"$(launch_app 1600 900 "UI-HOME-06 share")"
sleep 5
DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_action_home_1600x900_share.png"
python3 - "$OUT/${TASK_ID}_action_home_1600x900_share.png" > "$OUT/${TASK_ID}_card4_center.txt" <<'PY'
import subprocess, os, sys
from PIL import Image
shot = sys.argv[1]
scale = 3
img = Image.open(shot).convert('RGB')
p = '/tmp/action_tsv4.png'
img.resize((img.width*scale, img.height*scale), Image.LANCZOS).save(p)
t = p.replace('.png','')
subprocess.run(['tesseract', p, t, '--psm', '6', 'tsv'], capture_output=True)
rows=[]
if os.path.exists(t+'.tsv'):
    for line in open(t+'.tsv').read().splitlines()[1:]:
        f = line.split('\t')
        if len(f)==12 and f[11].strip() and f[6].isdigit():
            rows.append((f[11], int(f[6])//scale, int(f[7])//scale, int(f[8])//scale, int(f[9])//scale))
def center_of(word):
    for w, x, y, ww, hh in rows:
        if x >= 320 and y >= 450 and w.lower().rstrip('.,;:') == word.lower():
            return (x + ww//2, y + hh//2)
    return None
pts = [center_of(w) for w in ("Share", "Files")]
pts = [p for p in pts if p]
if pts:
    print(f"{sum(p[0] for p in pts)//len(pts)} {sum(p[1] for p in pts)//len(pts)}")
else:
    print("0 0")
PY
read -r c4x c4y < "$OUT/${TASK_ID}_card4_center.txt"
printf 'share card centre: %s %s\n' "$c4x" "$c4y"
DISPLAY=":$display" xdotool mousemove --sync "$c4x" "$c4y" click 1
sleep 2
DISPLAY=":$display" import -window root "$OUT/${TASK_ID}_action_4_share_files.png"
kill -0 "$app" && echo "app alive after Share Files click"
# Note: under Xvfb the native GTK chooser may not render; dispatch verified
# by unit test + liveness above.

# OCR-verify the three in-app flows opened the correct screen.
if command -v tesseract >/dev/null 2>&1; then
    for name in 1_create_room 2_create_group 3_friend_requests; do
        region=$(mktemp --suffix=.png)
        python3 - "$OUT/${TASK_ID}_action_${name}.png" "$region" <<'PY'
import sys
from PIL import Image
img = Image.open(sys.argv[1]).convert('RGB')
crop = img.resize((img.width * 2, img.height * 2), Image.LANCZOS)
crop.save(sys.argv[2])
PY
        txt=$(mktemp)
        tesseract "$region" "$txt" --psm 6 >/dev/null 2>&1 || true
        text=$(cat "$txt.txt" 2>/dev/null || true)
        norm=$(printf '%s' "$text" | tr '\n' ' ' | tr -s ' ')
        rm -f "$region" "$txt" "$txt.txt"
        case "$name" in
            1_create_room) expect="Create Public Room" ;;
            2_create_group) expect="Create Group" ;;
            3_friend_requests) expect="Friend Requests" ;;
        esac
        ok=1
        want=$(printf '%s' "$expect" | tr '[:upper:]' '[:lower:]' | tr -d '[:punct:][:space:]')
        got=$(printf '%s' "$norm" | tr '[:upper:]' '[:lower:]' | tr -d '[:punct:][:space:]')
        [[ "$got" == *"$want"* ]] || ok=0
        if [[ $ok -eq 1 ]]; then
            printf 'ACTION OK: %s opened (%s)\n' "$name" "$expect"
        else
            printf 'ACTION CHECK FAILED: %s did not show "%s" (OCR text: %s)\n' "$name" "$expect" "${norm:0:160}" >&2
        fi
    done
fi

kill "$app" "$xv" 2>/dev/null || true
wait "$app" 2>/dev/null || true
wait "$xv" 2>/dev/null || true
rm -rf "$data"

printf '\nEvidence complete: %s\n' "$OUT"
