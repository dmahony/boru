#!/usr/bin/env bash
# Evidence for UI-HOME-08: Recent Activity refinement + Tunnels rail card.
#
# Captures the Figure 3 right rail in its two required states at the wide
# target window (1280x800):
#   empty     - fresh data dir with no activity events and no tunnels ->
#               truthful "No recent activity" empty state and the spec
#               Tunnels copy "No active tunnels. Create or join a tunnel to
#               securely route traffic."
#   populated - seeded fixture + real friend-status events routed through the
#               production handle_friend_event -> push_activity path, so the
#               rows are genuine app state, not invented entries. The seeded
#               long-named peer demonstrates ellipsis truncation.
#
# Also verifies the Tunnels "View all" header action is backed by a real
# destination: clicking it opens the Create Tunnel dialog (same route the
# previous Manage button used), then Cancel closes it.
#
# Output: docs/ui-redesign/evidence/t_a2c055ce/
#   t_a2c055ce_home_empty_1280x800.png
#   t_a2c055ce_home_populated_1280x800.png
#   t_a2c055ce_activity_populated_zoom_1280x800.png
#   t_a2c055ce_tunnels_empty_zoom_1280x800.png
#   t_a2c055ce_tunnels_viewall_after_1280x800.png
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_a2c055ce"
BINARY="$ROOT_DIR/target/debug/examples/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
TASK_ID="t_a2c055ce"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }
[[ -x "$MCP_CLIENT" ]] || { printf 'MCP helper not executable: %s\n' "$MCP_CLIENT" >&2; exit 1; }

# Global session state (one launch at a time).
DISPLAY_NUM=""
DATA_DIR=""
XVFB_PID=""
APP_PID=""
WIN_ID=""

cleanup() {
    set +e
    [[ -n "$APP_PID" ]] && kill "$APP_PID" 2>/dev/null
    [[ -n "$XVFB_PID" ]] && kill "$XVFB_PID" 2>/dev/null
    wait "$APP_PID" 2>/dev/null
    wait "$XVFB_PID" 2>/dev/null
    [[ -n "$DATA_DIR" ]] && rm -rf "$DATA_DIR"
}
trap cleanup EXIT

find_display() {
    local display
    for display in $(seq 220 240); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 220..240\n' >&2
    return 1
}

mcp() {
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$1" "$2" "$3"
}

wait_main_window() {
    local window_id=""
    # Skip the Tk splash (also titled "Boru"): the main window name contains
    # the version string ("v0.*").
    for _ in $(seq 1 60); do
        local candidate name
        candidate=$(DISPLAY=":$DISPLAY_NUM" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | tail -n 1 || true)
        if [[ -n "$candidate" ]]; then
            name=$(DISPLAY=":$DISPLAY_NUM" xdotool getwindowname "$candidate" 2>/dev/null || true)
            if [[ "$name" == *"v0."* ]]; then
                window_id="$candidate"
                break
            fi
        fi
        sleep 0.5
    done
    [[ -n "$window_id" ]] || { printf 'Boru main window never appeared\n' >&2; return 1; }
    # Wait for the splash to close so it cannot steal focus.
    for _ in $(seq 1 40); do
        local count
        count=$(DISPLAY=":$DISPLAY_NUM" xdotool search --onlyvisible --name '^Boru' 2>/dev/null | wc -l)
        if [[ "$count" -le 1 ]]; then
            break
        fi
        sleep 0.5
    done
    printf '%s\n' "$window_id"
}

ocr_has() {
    local img=$1 text=$2
    tesseract "$img" - 2>/dev/null | grep -qi -- "$text"
}

# Print the first TSV word-box (left top width height) matching a regex.
# Usage: word_box <src> <regex>
word_box() {
    local src=$1 regex=$2
    tesseract "$src" - tsv 2>/dev/null | awk -F'\t' -v re="$regex" '
        $12 ~ re { print $7, $8, $9, $10; exit }
    '
}

# Crop a padded region around the first TSV word-box that matches a regex.
# Usage: crop_around_text <src> <regex> <out> <pad>
crop_around_text() {
    local src=$1 regex=$2 out=$3 pad=${4:-40}
    local left top w h
    read -r left top w h <<<"$(word_box "$src" "$regex")"
    if [[ -z "${left:-}" ]]; then
        printf 'crop_around_text: word "%s" not found in %s\n' "$regex" "$src" >&2
        return 1
    fi
    local x y ww hh
    x=$((left - pad)); [[ $x -lt 0 ]] && x=0
    y=$((top - pad)); [[ $y -lt 0 ]] && y=0
    ww=$((w + pad * 2)); hh=$((h + pad * 2))
    convert "$src" -crop "${ww}x${hh}+${x}+${y}" +repage "$out"
}

# Crop a full-width rail-card region anchored on a header word box, extending
# downward to capture the card body (empty copy or several list rows).
# Usage: crop_card <src> <header-regex> <out> <height>
crop_card() {
    local src=$1 regex=$2 out=$3 height=$4
    local left top w h
    read -r left top w h <<<"$(word_box "$src" "$regex")"
    if [[ -z "${left:-}" ]]; then
        printf 'crop_card: header "%s" not found in %s\n' "$regex" "$src" >&2
        return 1
    fi
    local x y
    x=$((left - 12)); [[ $x -lt 0 ]] && x=0
    y=$((top - 24)); [[ $y -lt 0 ]] && y=0
    convert "$src" -crop "340x${height}+${x}+${y}" +repage "$out"
}

launch_state() {
    local state=$1
    DISPLAY_NUM=$(find_display)
    DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui08.XXXXXX")
    local mcp_port=$((18600 + DISPLAY_NUM))

    if [[ "$state" == "populated" ]]; then
        python3 "$SEED_SCRIPT" "$DATA_DIR" >/dev/null
    fi

    Xvfb ":$DISPLAY_NUM" -screen 0 1280x800x24 -nolisten tcp >/tmp/boru-ui08-xvfb.log 2>&1 &
    XVFB_PID=$!
    sleep 0.5
    kill -0 "$XVFB_PID"

    DISPLAY=":$DISPLAY_NUM" "$BINARY" \
        --data-dir "$DATA_DIR" --no-dht --no-relay --name "UI-HOME-08 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui08-app.log 2>&1 &
    APP_PID=$!

    local attempt
    for attempt in $(seq 1 60); do
        if DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
            break
        fi
        sleep 0.25
    done
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null
    sleep 2

    WIN_ID=$(wait_main_window)
    DISPLAY=":$DISPLAY_NUM" xdotool windowsize "$WIN_ID" 1280 800
    sleep 1
    DISPLAY=":$DISPLAY_NUM" xdotool windowfocus --sync "$WIN_ID"
    sleep 0.5

    # Home (chat list) shows the Figure 3 rail.
    mcp "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1

    if [[ "$state" == "populated" ]]; then
        # Route real friend-status events through the production
        # handle_friend_event path (has_been_seen=true for seeded friends),
        # which pushes genuine activity events into recent_activity.
        ALICE_PK=$(printf 'a1%.0s' {1..32})
        BOB_PK=$(printf 'b2%.0s' {1..32})
        LONG_PK=$(printf 'c3%.0s' {1..32})
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
        sleep 0.5
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$BOB_PK\",\"online\":false}" >/dev/null
        sleep 0.5
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$LONG_PK\",\"online\":true}" >/dev/null
        sleep 1
    fi
}

# ── Empty state (fresh launch, truthful) ─────────────────────────────
launch_state empty
EMPTY="$OUTPUT_DIR/${TASK_ID}_home_empty_1280x800.png"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$EMPTY"
printf 'captured %s\n' "$EMPTY"

# Zoomed crop of the Tunnels card (empty copy region) — works on the PNG.
crop_around_text "$EMPTY" 'TUNNELS' "$OUTPUT_DIR/${TASK_ID}_tunnels_empty_zoom_1280x800.png" 60
crop_card "$EMPTY" 'TUNNELS' "$OUTPUT_DIR/${TASK_ID}_tunnels_empty_card_1280x800.png" 210

# Click-through: the Tunnels "View all" header action opens the Create
# Tunnel dialog (a real destination). The Tunnels card is the LAST rail
# card, so its "View all" is the last "View" token in TSV reading order;
# take that one and click it, then verify the dialog renders.
TSV=$(tesseract "$EMPTY" - tsv 2>/dev/null)
read -r vx vy vw vh <<<"$(awk -F'\t' '
    $12 ~ /^View$/ { vx=$7; vy=$8; vw=$9; vh=$10 }
    END { print vx, vy, vw, vh }
' <<<"$TSV")"
if [[ -n "${vx:-}" ]]; then
    printf 'clicking Tunnels "View all" at %s,%s\n' $((vx + vw / 2)) $((vy + vh / 2))
    DISPLAY=":$DISPLAY_NUM" xdotool mousemove $((vx + vw / 2)) $((vy + vh / 2))
    sleep 0.5
    DISPLAY=":$DISPLAY_NUM" xdotool click 1
    sleep 1.5
    DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$OUTPUT_DIR/${TASK_ID}_tunnels_viewall_after_1280x800.png"
    if ocr_has "$OUTPUT_DIR/${TASK_ID}_tunnels_viewall_after_1280x800.png" 'Cancel'; then
        printf 'View all -> Create Tunnel dialog: OK (Cancel visible)\n'
        # Close the dialog by clicking Cancel.
        read -r cx cy cw ch <<<"$(tesseract "$OUTPUT_DIR/${TASK_ID}_tunnels_viewall_after_1280x800.png" - tsv 2>/dev/null | awk -F'\t' '$12 ~ /^Cancel$/ { print $7, $8, $9, $10; exit }')"
        if [[ -n "${cx:-}" ]]; then
            DISPLAY=":$DISPLAY_NUM" xdotool mousemove $((cx + cw / 2)) $((cy + ch / 2))
            sleep 0.5
            DISPLAY=":$DISPLAY_NUM" xdotool click 1
            sleep 0.8
        fi
    else
        printf 'View all -> dialog: Cancel NOT found in capture\n' >&2
    fi
else
    printf 'Tunnels "View all" action not found via TSV\n' >&2
fi

# Teardown the empty-state session before launching the populated one.
cleanup
DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""

# ── Populated state (seeded fixture + real presence events) ──────────
launch_state populated
POPULATED="$OUTPUT_DIR/${TASK_ID}_home_populated_1280x800.png"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$POPULATED"
printf 'captured %s\n' "$POPULATED"

# Zoomed crop of the Recent Activity card — works on the PNG.
crop_around_text "$POPULATED" 'RECENT' "$OUTPUT_DIR/${TASK_ID}_activity_populated_zoom_1280x800.png" 60
crop_card "$POPULATED" 'RECENT' "$OUTPUT_DIR/${TASK_ID}_activity_populated_card_1280x800.png" 300

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
