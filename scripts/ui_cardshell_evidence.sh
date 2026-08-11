#!/usr/bin/env bash
# Evidence for the reusable card shell component (t_67cfe73b).
#
# Opens the developer component gallery (Ctrl+Shift+G) and captures the new
# "Card Shell (Figure 3 rail)" section, which demonstrates:
#   - the empty state (title + count badge + caller-provided message)
#   - the populated state (8 rows at 48 px inside a bounded 140 px scrollable,
#     so a vertical scrollbar appears instead of an unbounded card)
#   - the optional "View all" header action
#
# Notes for a bare-Xvfb (no window manager) run:
#   - winit ignores synthetic `--window` key events; set real input focus with
#     `xdotool windowfocus`.
#   - The Tk splash window is also titled "Boru" and grabs focus; wait for it
#     to close before pressing keys.
#   - Use `xdotool key --clearmodifiers` so stuck modifier state cannot eat
#     the Ctrl+Shift+G chord.
#
# Output: docs/ui-redesign/evidence/ui-cardshell/
#   t_67cfe73b_cardshell_1280x800.png      — gallery with the Card Shell section
#   t_67cfe73b_cardshell_zoom_1280x800.png — zoomed crop of the two shells
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-cardshell"
BINARY="$ROOT_DIR/target/debug/boru"
TASK_ID="t_67cfe73b"

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
data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-cardshell.XXXXXX")
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

Xvfb ":$display" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-cardshell-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5

DISPLAY=":$display" "$BINARY" --data-dir "$data_dir" --no-dht --no-relay \
    --name "Card Shell Evidence" >/tmp/boru-cardshell-app-$display.log 2>&1 &
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
    DISPLAY=":$display" import -window "$window_id" /tmp/boru-cardshell-check.png
    if tesseract /tmp/boru-cardshell-check.png - 2>/dev/null | grep -q "Component Gallery"; then
        gallery_open=1
        break
    fi
done
if [[ "$gallery_open" != "1" ]]; then
    printf 'Could not open the component gallery\n' >&2
    exit 1
fi
DISPLAY=":$display" import -window "$window_id" "$OUTPUT_DIR/${TASK_ID}_cardshell_1280x800.png"

# Zoomed crop of the card shell section (right of the sidebar). The section
# sits under the "Card Shell (Figure 3 rail)" heading; crop the main panel.
convert "$OUTPUT_DIR/${TASK_ID}_cardshell_1280x800.png" \
    -crop 720x560+420+180 +repage \
    "$OUTPUT_DIR/${TASK_ID}_cardshell_zoom_1280x800.png"

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
