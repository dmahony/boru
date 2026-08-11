#!/usr/bin/env bash
# Scroll the home page via mouse-wheel (over the main panel) so the lower
# quick-action row is fully in view, then capture evidence that every card
# description is fully rendered (nothing clipped by the card itself).
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/boru
TASK_ID="t_2577e385"
OUT="$ROOT/docs/ui-redesign/evidence/$TASK_ID"
mkdir -p "$OUT"

find_display() {
    local display
    for display in $(seq 190 230); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    return 1
}

for spec in '1280 800' '1024 720'; do
    read -r w h <<<"$spec"
    display=$(find_display)
    data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home06wheel.XXXXXX")
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home06w-xvfb-$w.log 2>&1 & xv=$!
    sleep 0.5
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-HOME-06 wheel $w" \
        >/tmp/boru-home06w-app-$w.log 2>&1 & app=$!
    win=''
    for _ in $(seq 1 80); do
        win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
        [[ -n "$win" ]] && break
        sleep 0.25
    done
    [[ -n "$win" ]] || { echo "no window $w"; kill "$app" "$xv" 2>/dev/null || exit 1; }
    DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
    sleep 5
    # Move pointer over the main content panel (right of the sidebar) and
    # scroll down several notches with the wheel.
    DISPLAY=":$display" xdotool mousemove --sync $((w/2)) $((h/2))
    for _ in $(seq 1 8); do
        DISPLAY=":$display" xdotool click 5
        sleep 0.2
    done
    sleep 1
    DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_home_${w}x${h}_scrolled.png"
    kill "$app" "$xv" 2>/dev/null || true
    wait "$app" 2>/dev/null || true
    wait "$xv" 2>/dev/null || true
    rm -rf "$data"
done
file "$OUT"/${TASK_ID}_home_*_scrolled.png
