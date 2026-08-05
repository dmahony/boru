#!/usr/bin/env bash
# Capture UI-HOME-02 grid evidence on the HOME screen (Screen::ChatList).
#
# Verifies the UI-HOME-02 page geometry at three supported widths:
#   wide 1600x900, medium 1280x800 (reference), narrow 1024x720.
#
# Pattern: fresh temp data dir, --no-dht --no-relay, no subcommand -> the app
# lands on the ChatList (home) screen in the truthful fresh-launch Connecting
# state (no peers, mDNS not relied upon). Each instance is window-resized to
# the target viewport and captured with ImageMagick `import`.
#
# Output: docs/ui-redesign/evidence/<task-id>/<task-id>_home_<w>x<h>_connecting.png
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
TASK_ID="t_2e1588be"
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
    data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home02.XXXXXX")
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home02-xvfb-$w.log 2>&1 & xv=$!
    sleep 0.5
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-HOME-02 $w" \
        >/tmp/boru-home02-app-$w.log 2>&1 & app=$!
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
    DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_home_${w}x${h}_connecting.png"
    kill "$app" "$xv" 2>/dev/null || true
    wait "$app" 2>/dev/null || true
    wait "$xv" 2>/dev/null || true
    rm -rf "$data"
done

file "$OUT"/${TASK_ID}_home_*.png
