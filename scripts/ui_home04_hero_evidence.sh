#!/usr/bin/env bash
# Capture UI-HOME-04 connection overview card evidence on the HOME screen
# (Screen::ChatList).
#
# Usage: scripts/ui_home04_hero_evidence.sh
#
# States captured (truthful, live):
#   connecting - fresh launch with --no-dht --no-relay: no peers yet, amber
#                "Connecting" hero (non-connected state evidence).
#   ready      - two instances on the same lobby topic: mDNS connects them,
#                one direct peer -> green "Connected"/Ready hero.
#
# Medium width is captured in both states (1280x800 is the reference
# viewport; 1024x720 shows the stacked layout).
#
# Output: docs/ui-redesign/evidence/t_f6df86db/t_f6df86db_home_<w>x<h>_<state>.png
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
TASK_ID="t_f6df86db"
OUT="$ROOT/docs/ui-redesign/evidence/$TASK_ID"
mkdir -p "$OUT"
[[ -x "$BIN" ]] || { printf 'GUI binary not found: %s\n' "$BIN" >&2; exit 1; }

find_display() {
    # $1 (optional) = comma-separated displays already reserved this run.
    local used="${1:-}"
    local display
    for display in $(seq 190 230); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            if [[ -n "$used" ]] && [[ ",$used," == *",$display,"* ]]; then
                continue
            fi
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 190..230\n' >&2
    return 1
}

launch() {
    # $1 = display, $2 = data_dir, $3 = name
    DISPLAY=":$1" "$BIN" --data-dir "$2" --no-dht --no-relay --name "$3" \
        >/tmp/boru-home04-app-$1.log 2>&1 &
    printf '%s\n' "$!"
}

capture() {
    # $1 = display, $2 = width, $3 = height, $4 = output
    local win=''
    for _ in $(seq 1 80); do
        win=$(DISPLAY=":$1" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
        [[ -n "$win" ]] && break
        sleep 0.25
    done
    if [[ -z "$win" ]]; then
        printf 'window not found on display %s\n' "$1" >&2
        return 1
    fi
    DISPLAY=":$1" xdotool windowsize "$win" "$2" "$3"
    sleep 1
    # Retry until the capture is a real image (blank windows come out as
    # tiny 1-bit grayscale PNGs ~200-300 bytes).
    local attempt=0
    while [[ $attempt -lt 5 ]]; do
        DISPLAY=":$1" import -window "$win" "$4"
        if [[ -f "$4" ]] && [[ $(stat -c %s "$4") -gt 10000 ]]; then
            return 0
        fi
        printf '  retry %d for %s (size %s)\n' "$attempt" "$4" "$(stat -c %s "$4" 2>/dev/null || echo 0)"
        sleep 3
        attempt=$((attempt + 1))
    done
    printf 'capture still blank after retries: %s\n' "$4" >&2
    return 1
}

cleanup_instance() {
    set +e
    [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
    [[ -n "${app_pid_b:-}" ]] && kill "$app_pid_b" 2>/dev/null
    [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
    [[ -n "${xvfb_pid_b:-}" ]] && kill "$xvfb_pid_b" 2>/dev/null
    rm -rf "${data_a:-}" "${data_b:-}"
    set -e
}

for spec in '1600 900' '1280 800' '1024 720'; do
    read -r w h <<<"$spec"

    # ── Connecting (non-connected state) ──
    display=$(find_display)
    data_a=$(mktemp -d "${TMPDIR:-/tmp}/boru-home04-connect.XXXXXX")
    xvfb_pid=""
    app_pid=""
    app_pid_b=""
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home04-xvfb-connect.log 2>&1 & xvfb_pid=$!
    sleep 0.5
    app_pid=$(launch "$display" "$data_a" "UI-HOME-04 Connecting $w")
    sleep 6
    if capture "$display" "$w" "$h" "$OUT/${TASK_ID}_home_${w}x${h}_connecting.png"; then
        printf 'captured connecting %sx%s\n' "$w" "$h"
    fi
    cleanup_instance

    # ── Ready (connected state): two instances discover each other via mDNS ──
    # Reserve both displays up front (before starting either Xvfb) so the
    # two instances never land on the same display — the second Xvfb would
    # fail to bind and the capture would be a blank window.
    display=$(find_display)
    display_b=$(find_display "$display")
    data_a=$(mktemp -d "${TMPDIR:-/tmp}/boru-home04-ready-a.XXXXXX")
    data_b=$(mktemp -d "${TMPDIR:-/tmp}/boru-home04-ready-b.XXXXXX")
    xvfb_pid=""
    xvfb_pid_b=""
    app_pid=""
    app_pid_b=""
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home04-xvfb-ready.log 2>&1 & xvfb_pid=$!
    Xvfb ":$display_b" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home04-xvfb-ready-b.log 2>&1 & xvfb_pid_b=$!
    sleep 0.5
    app_pid=$(launch "$display" "$data_a" "UI-HOME-04 Ready A $w")
    app_pid_b=$(launch "$display_b" "$data_b" "UI-HOME-04 Ready B $w")
    sleep 20
    if capture "$display" "$w" "$h" "$OUT/${TASK_ID}_home_${w}x${h}_ready.png"; then
        printf 'captured ready %sx%s\n' "$w" "$h"
    fi
    cleanup_instance
done

file "$OUT"/${TASK_ID}_home_*.png
