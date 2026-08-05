#!/usr/bin/env bash
# Capture UI-HOME-05 Mesh Health card evidence on the HOME screen
# (Screen::ChatList).
#
# Usage: scripts/ui_home05_mesh_health_evidence.sh [populated|noevents|details|all]
#
# Populated state: two seeded instances (seed_two_instances.py) that connect
# directly over localhost QUIC, so the Mesh Health card shows real values
# (neighbors / direct / relayed) and REAL mesh events ("Connected to lobby — 1
# peer online", "Discovered 1 direct, 0 relayed peers").
#
# No-events state: a fresh --no-dht --no-relay instance whose mesh event log is
# cleared through the test-only boru_send_gui_action clear_mesh_event_log
# command, capturing the card's intentional empty state.
#
# View-details: OCR-locates the "View details" button in the card header,
# clicks it, and captures the opened connection-details dialog.
#
# Output: docs/ui-redesign/evidence/t_faa541a0/t_faa541a0_<state>_<w>x<h>.png
set -euo pipefail

MODE="${1:-all}"
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BIN=$ROOT/target/debug/examples/boru
TASK_ID="t_faa541a0"
OUT="$ROOT/docs/ui-redesign/evidence/$TASK_ID"
MCP=$ROOT/scripts/ui_mcp.py
SEED=$ROOT/scripts/seed_two_instances.py
mkdir -p "$OUT"
[[ -x "$BIN" ]] || { printf 'GUI binary not found: %s\n' "$BIN" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 230 290); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 230..290\n' >&2
    return 1
}

wait_window() { # $1=display $2=logprefix
    local display=$1 prefix=$2 win=''
    for _ in $(seq 1 100); do
        win=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1 || true)
        [[ -n "$win" ]] && break
        sleep 0.25
    done
    printf '%s\n' "$win"
}

ocr_find() { # $1=png $2=text  -> prints "x y" center of the matched box
    local png=$1 text=$2 x y
    tesseract "$png" stdout tsv 2>/dev/null | awk -F'\t' -v want="$text" '
        $12 != "" && index(tolower($12), tolower(want)) > 0 {
            cx = $7 + $9 / 2; cy = $8 + $10 / 2;
            printf "%d %d\n", cx, cy; exit
        }'
}

# ── Populated: two instances that discover each other ──────────────────────
capture_populated() {
    local w=$1 h=$2
    local display_a display_b mcp_a mcp_b data_a data_b xv_a xv_b app_a app_b win
    display_a=$(find_display)
    data_a=$(mktemp -d "${TMPDIR:-/tmp}/boru-home05a.XXXXXX")
    data_b=$(mktemp -d "${TMPDIR:-/tmp}/boru-home05b.XXXXXX")
    # Deterministic keys + direct QUIC addresses so the two instances connect
    # without DHT/relay and produce real "Discovered ... peers" events.
    python3 "$SEED" "$data_a" "$data_b" \
        --bind-port-a $((20000 + display_a)) --bind-port-b $((20100 + display_a)) >/dev/null
    Xvfb ":$display_a" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home05-xvfb-a.log 2>&1 & xv_a=$!
    sleep 0.5
    # Instance B must use a DIFFERENT display: only probe after A's Xvfb has
    # created its socket, so find_display cannot hand back display_a again.
    display_b=$(find_display)
    mcp_a=$((18800 + display_a))
    mcp_b=$((18810 + display_b))
    DISPLAY=":$display_a" "$BIN" --data-dir "$data_a" --no-dht --no-relay --name "UI-HOME-05 A" \
        --bind-port $((20000 + display_a)) --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_a" \
        >/tmp/boru-home05-app-a.log 2>&1 & app_a=$!
    sleep 6
    Xvfb ":$display_b" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home05-xvfb-b.log 2>&1 & xv_b=$!
    DISPLAY=":$display_b" "$BIN" --data-dir "$data_b" --no-dht --no-relay --name "UI-HOME-05 B" \
        --bind-port $((20100 + display_a)) --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_b" \
        >/tmp/boru-home05-app-b.log 2>&1 & app_b=$!
    win=$(wait_window "$display_a" a)
    if [[ -n "$win" ]]; then
        DISPLAY=":$display_a" xdotool windowsize "$win" "$w" "$h"
        # Let the direct connection + discovery events land in the log.
        sleep 14
        DISPLAY=":$display_a" import -window "$win" "$OUT/${TASK_ID}_populated_${w}x${h}.png"
        printf 'OK populated %sx%s\n' "$w" "$h"
    else
        printf 'FAIL populated %sx%s: window not found\n' "$w" "$h" >&2
    fi
    kill "$app_a" "$app_b" "$xv_a" "$xv_b" 2>/dev/null || true
    wait "$app_a" "$app_b" "$xv_a" "$xv_b" 2>/dev/null || true
    rm -rf "$data_a" "$data_b"
}

# ── No-events: fresh instance + test-only clear ─────────────────────────────
capture_noevents() {
    local w=$1 h=$2
    local display mcp data xv app win
    display=$(find_display)
    mcp=$((18820 + display))
    data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home05n.XXXXXX")
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home05-xvfb-n.log 2>&1 & xv=$!
    sleep 0.5
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-HOME-05 N" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp" \
        >/tmp/boru-home05-app-n.log 2>&1 & app=$!
    win=$(wait_window "$display" n)
    if [[ -n "$win" ]]; then
        DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
        # Let startup events ("Starting up...", "Connecting to lobby...") land
        # and the lobby subscription settle BEFORE clearing, so the card's
        # empty state is captured rather than a mid-startup log.
        sleep 8
        # Test-only command: clear the mesh event log so the card shows its
        # intentional no-events state (never fabricates events). The outer
        # params object's "command" key carries the serialized internally
        # tagged GuiTestCommand.
        DISPLAY=":$display" python3 "$MCP" "$mcp" boru_send_gui_action \
            '{"command": {"command": "clear_mesh_event_log"}}' >/dev/null 2>&1 || true
        sleep 2
        DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_noevents_${w}x${h}.png"
        printf 'OK no-events %sx%s\n' "$w" "$h"
    else
        printf 'FAIL no-events %sx%s: window not found\n' "$w" "$h" >&2
    fi
    kill "$app" "$xv" 2>/dev/null || true
    wait "$app" "$xv" 2>/dev/null || true
    rm -rf "$data"
}

# ── View details interaction ───────────────────────────────────────────────
capture_details() {
    local w=$1 h=$2
    local display mcp data xv app win pos
    display=$(find_display)
    mcp=$((18830 + display))
    data=$(mktemp -d "${TMPDIR:-/tmp}/boru-home05d.XXXXXX")
    Xvfb ":$display" -screen 0 "${w}x${h}x24" -nolisten tcp >/tmp/boru-home05-xvfb-d.log 2>&1 & xv=$!
    sleep 0.5
    DISPLAY=":$display" "$BIN" --data-dir "$data" --no-dht --no-relay --name "UI-HOME-05 D" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp" \
        >/tmp/boru-home05-app-d.log 2>&1 & app=$!
    win=$(wait_window "$display" d)
    if [[ -n "$win" ]]; then
        DISPLAY=":$display" xdotool windowsize "$win" "$w" "$h"
        sleep 4
        DISPLAY=":$display" import -window "$win" /tmp/boru-home05-detect.png
        # OCR is sometimes noisy ("View details" -> "View details"/"View
        # detail"), so fall back to a fuzzy "details" match.
        pos=$(ocr_find /tmp/boru-home05-detect.png "View details")
        if [[ -z "$pos" ]]; then
            pos=$(ocr_find /tmp/boru-home05-detect.png "etails")
        fi
        if [[ -n "$pos" ]]; then
            read -r px py <<<"$pos"
            DISPLAY=":$display" xdotool mousemove --sync "$px" "$py" click 1
            sleep 2
            DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_details_${w}x${h}.png"
            printf 'OK details %sx%s (clicked %s)\n' "$w" "$h" "$pos"
        else
            printf 'WARN details %sx%s: "View details" not found by OCR\n' "$w" "$h" >&2
            DISPLAY=":$display" import -window "$win" "$OUT/${TASK_ID}_details_${w}x${h}.png"
        fi
    else
        printf 'FAIL details %sx%s: window not found\n' "$w" "$h" >&2
    fi
    kill "$app" "$xv" 2>/dev/null || true
    wait "$app" "$xv" 2>/dev/null || true
    rm -rf "$data"
}

case "$MODE" in
    populated)
        capture_populated 1600 900
        capture_populated 1280 800
        ;;
    noevents)
        capture_noevents 1280 800
        capture_noevents 1600 900
        ;;
    details)
        capture_details 1280 800
        ;;
    all)
        capture_populated 1600 900
        capture_populated 1280 800
        capture_noevents 1280 800
        capture_details 1280 800
        ;;
    *)
        printf 'usage: %s [populated|noevents|details|all]\n' "$0" >&2
        exit 2
        ;;
esac

printf 'Evidence in %s\n' "$OUT"
ls -1 "$OUT" 2>/dev/null | sed 's/^/  /' || true
