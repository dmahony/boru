#!/usr/bin/env bash
# Capture UI-06 sidebar evidence: empty state (fresh data dir) and populated
# state (seeded data dir) at the required window sizes, on both the chat
# screen (selected conversation treatment) and the chat-list home screen
# (all sections + counts + empty states).
#
# Usage:
#   scripts/ui06_sidebar_screenshots.sh
#   scripts/ui06_sidebar_screenshots.sh --binary target/debug/boru
#
# Output goes to docs/ui-redesign/evidence/ui-06/ with immutable names:
#   t_4d13a7ac_<state>_<width>x<height>_<screen>.png
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-06-v4"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="${SEED_SCRIPT:-$ROOT_DIR/scripts/seed_boru_data.py}"
TASK_ID="t_4d13a7ac"

usage() {
    printf 'usage: %s [--binary PATH]\n' "$0" >&2
    exit 2
}
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) [[ $# -ge 2 ]] || usage; BINARY=$2; shift 2 ;;
        *) usage ;;
    esac
done

[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }
[[ -f "$SEED_SCRIPT" ]] || { printf 'seed script not found: %s\n' "$SEED_SCRIPT" >&2; exit 1; }

mkdir -p "$OUTPUT_DIR"

find_display() {
    local display
    for display in $(seq 99 119); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 99..119\n' >&2
    return 1
}

capture_window() {
    local output=$1 width=$2 height=$3
    local window_id
    window_id=$(xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    xdotool windowsize "$window_id" "$width" "$height"
    sleep 0.5
    import -window "$window_id" "$output"
}

# Redact mDNS-discovered peer rows in the DISCOVER section (they can expose
# real local peer IDs).  The rectangle covers the DISCOVER rows area of the
# sidebar; other sections remain unchanged.
redact_discover() {
    local output=$1
    convert "$output" -fill '#ffffff' -draw 'rectangle 0,540 279,640' "$output"
}

capture_state() {
    local state=$1 width=$2 height=$3 seed_dir=$4
    local output_home="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}_home.png"
    local output_chat="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}_chat.png"

    local display data_dir mcp_port xvfb_pid app_pid
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui06.XXXXXX")
    mcp_port=$((18000 + display))
    if [[ -n "$seed_dir" ]]; then
        cp -a "$seed_dir/." "$data_dir/"
    fi
    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "${app_pid:-}" 2>/dev/null
        wait "${xvfb_pid:-}" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui06-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "UI-06 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui06-app.log 2>&1 &
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

    # Chat screen first: open a seeded conversation so the sidebar shows the
    # selected conversation treatment (soft-green surface + primary border).
    # The `open` command lands in a local room whose topic is not in the
    # seeded conversation store, so use the GUI test action to open a seeded
    # conversation instead.
    if [[ "$state" == "populated" ]]; then
        DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" \
            boru_gui_open_conversation '{"conversation_id":"a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1"}' >/dev/null
        sleep 1
    fi
    DISPLAY=":$display" capture_window "$output_chat" "$width" "$height"
    redact_discover "$output_chat"

    # Then the chat-list home screen: full sidebar with all sections.
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1
    DISPLAY=":$display" capture_window "$output_home" "$width" "$height"
    redact_discover "$output_home"

    printf 'captured %s\n' "$output_chat"
    printf 'captured %s\n' "$output_home"
}

# Empty state: fresh data dir (no seed).
for size in '1280 800' '1024 720'; do
    # shellcheck disable=SC2086
    capture_state empty $size ""
done

# Populated state: seeded data dir.
SEED_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui06-seed.XXXXXX")
python3 "$SEED_SCRIPT" "$SEED_DIR" >/dev/null
for size in '1280 800' '1024 720'; do
    # shellcheck disable=SC2086
    capture_state populated $size "$SEED_DIR"
done
rm -rf "$SEED_DIR"
