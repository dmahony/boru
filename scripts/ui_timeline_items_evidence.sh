#!/usr/bin/env bash
# Evidence for the reusable date-separator and system-event-chip components
# (t_ead7de5f).
#
# Opens the developer component gallery (Ctrl+Shift+G) and captures the new
# "Timeline (Figure 4)" section, which demonstrates:
#   - centered, muted date separators ("Today", "Yesterday", full date)
#   - centered system-event chips across every accent the timeline uses
#     (MEMBER/NAME/HELP/NOTICE/INFO) with the caller-supplied label+accent
#   - the same components in a sample timeline stack
#
# Notes for a bare-Xvfb (no window manager) run:
#   - winit ignores synthetic `--window` key events; set real input focus with
#     `xdotool windowfocus`.
#   - The Tk splash window is also titled "Boru" and grabs focus; wait for it
#     to close before pressing keys.
#   - Use `xdotool key --clearmodifiers` so stuck modifier state cannot eat
#     the Ctrl+Shift+G chord.
#
# Output: docs/ui-redesign/evidence/ui-timeline-items/
#   t_ead7de5f_timeline_1280x800.png      — gallery with the Timeline section
#   t_ead7de5f_timeline_1024x720.png      — alternate viewport
#   t_ead7de5f_timeline_zoom_1280x800.png — zoomed crop of the sample timeline
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-timeline-items"
BINARY="$ROOT_DIR/target/debug/boru"
TASK_ID="t_ead7de5f"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 240 270); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 240..270\n' >&2
    return 1
}

display=$(find_display)
data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-timeline-items.XXXXXX")
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
trap cleanup EXIT

Xvfb ":$display" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-timeline-items-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5

DISPLAY=":$display" "$BINARY" --data-dir "$data_dir" --no-dht --no-relay \
    --name "Timeline Items Evidence" >/tmp/boru-timeline-items-app-$display.log 2>&1 &
app_pid=$!

# Wait for the MAIN window. The splash window is also titled "Boru", so poll
# until the remaining window name contains the version tag ("Boru — v0.…").
window_id=""
for _ in $(seq 1 60); do
    candidate=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | tail -n 1 || true)
    if [[ -n "$candidate" ]]; then
        name=$(DISPLAY=":$display" xdotool getwindowname "$candidate" 2>/dev/null || true)
        if [[ "$name" == *"v0."* ]]; then
            window_id="$candidate"
            break
        fi
    fi
    sleep 0.5
done
[[ -n "$window_id" ]] || { printf 'Boru main window never appeared\n' >&2; exit 1; }

# Wait for the Tk splash (also titled "Boru") to close — it grabs input focus,
# so keys pressed while it is alive go to the splash and are lost.
for _ in $(seq 1 40); do
    count=$(DISPLAY=":$display" xdotool search --onlyvisible --name '^Boru' 2>/dev/null | wc -l)
    if [[ "$count" -le 1 ]]; then
        break
    fi
    sleep 0.5
done

capture_gallery() {
    local width="$1"
    local height="$2"
    local suffix="$3"
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 1
    # The gallery screen persists across resizes (verified when it was first
    # toggled open); do not re-check for the "Component Gallery" heading here
    # because after scrolling it is no longer on screen.
    # Scroll the gallery until the "Timeline (Figure 4)" section is visible
    # (it sits below Buttons/Cards/Card Shell/List Rows/Avatars/…). Iced's
    # scrollable does not handle Page_Down, and `xdotool click --window`
    # (synthesized window-relative events) stops being processed after a few
    # events — use the REAL pointer position and plain clicks instead.
    # Detection uses distinctive sample strings that only exist inside the
    # Timeline section ("Kitchen", "Chat joined", "Invite sent") — OCR of the
    # "Figure 4" heading itself is unreliable because "Figure 3 rail" is
    # frequently misread.
    geometry=$(DISPLAY=":$display" xdotool getwindowgeometry "$window_id" | awk '/Position:/{print $2}')
    win_x=${geometry%,*}
    win_y=${geometry#*,}
    DISPLAY=":$display" xdotool windowfocus --sync "$window_id"
    # Pointer over the main panel (right of the ~304 px sidebar), inside the
    # window — use window coords + window origin since there is no WM.
    DISPLAY=":$display" xdotool mousemove $((win_x + width * 2 / 3)) $((win_y + height / 2))
    sleep 0.3
    for _ in $(seq 1 60); do
        DISPLAY=":$display" import -window "$window_id" /tmp/boru-timeline-check.png
        if tesseract /tmp/boru-timeline-check.png - 2>/dev/null \
            | grep -qE "Kitchen|Chat joined|Invite sent"; then
            break
        fi
        DISPLAY=":$display" xdotool click 5
        sleep 0.15
    done
    # Nudge the section heading toward the top of the panel.
    DISPLAY=":$display" xdotool click 5
    sleep 0.3
    DISPLAY=":$display" xdotool click 5
    sleep 0.8
    DISPLAY=":$display" import -window "$window_id" "$OUTPUT_DIR/${TASK_ID}_timeline_${suffix}.png"
    return 0
}

sleep 1   # let the UI settle
DISPLAY=":$display" xdotool windowsize "$window_id" 1280 800
sleep 1

# Toggle into the developer gallery with Ctrl+Shift+G, verifying via OCR on a
# scratch capture. The toggle flips screens, so verify after each press before
# pressing again.
DISPLAY=":$display" xdotool windowfocus --sync "$window_id"
sleep 0.5
gallery_open=0
for _ in $(seq 1 4); do
    DISPLAY=":$display" xdotool key --clearmodifiers ctrl+shift+g
    sleep 1.5
    DISPLAY=":$display" import -window "$window_id" /tmp/boru-timeline-check.png
    if tesseract /tmp/boru-timeline-check.png - 2>/dev/null | grep -q "Component Gallery"; then
        gallery_open=1
        break
    fi
done
if [[ "$gallery_open" != "1" ]]; then
    printf 'Could not open the component gallery\n' >&2
    exit 1
fi

capture_gallery 1280 800 "1280x800" || { printf 'gallery capture 1280x800 failed\n' >&2; exit 1; }
capture_gallery 1024 720 "1024x720" || { printf 'gallery capture 1024x720 failed\n' >&2; exit 1; }

# Zoomed crop of the "Timeline (Figure 4)" section. The gallery scrolls, so
# the section may sit below the fold; capture the whole window then crop the
# main panel region where the sample timeline renders.
convert "$OUTPUT_DIR/${TASK_ID}_timeline_1280x800.png" \
    -crop 720x560+420+180 +repage \
    "$OUTPUT_DIR/${TASK_ID}_timeline_zoom_1280x800.png"

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
