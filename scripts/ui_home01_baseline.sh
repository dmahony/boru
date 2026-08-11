#!/usr/bin/env bash
# Capture Boru home-screen baseline screenshots for t_a0b1f82f.
#
# Captures the home screen at normal (1280x800) and maximized (1920x1080)
# sizes using Xvfb + MCP. No code changes — read-only capture.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_a0b1f82f"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
TASK_ID="t_a0b1f82f"

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

[[ -x "$BINARY" ]] || {
    printf 'GUI binary not found: %s\nBuild with: rb build --features gui,video-playback,terminal\n' "$BINARY" >&2
    exit 1
}
[[ -x "$MCP_CLIENT" ]] || { printf 'MCP helper is not executable: %s\n' "$MCP_CLIENT" >&2; exit 1; }

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
    # Redact mDNS sidebar rows if any local peer appears
    convert "$output" -fill '#ffffff' -draw 'rectangle 0,430 279,490' "$output"
}

capture_size() {
    local width=$1 height=$2
    local prefix="${TASK_ID}_"
    local home="$OUTPUT_DIR/${prefix}home_${width}x${height}_baseline.png"
    [[ ! -e "$home" ]] || {
        printf 'refusing to overwrite: %s\n' "$home" >&2
        exit 1
    }

    local display data_dir mcp_port xvfb_pid app_pid
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui-baseline.XXXXXX")
    mcp_port=$((18000 + display))
    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "${app_pid:-}" 2>/dev/null
        wait "${xvfb_pid:-}" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui-home01-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "UI Baseline" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui-home01-app.log 2>&1 &
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

    # Navigate to chat_list (home screen) and capture
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1
    DISPLAY=":$display" capture_window "$home" "$width" "$height"

    printf 'captured %s\n' "$home"
}

for size in '1280 800' '1920 1080'; do
    # shellcheck disable=SC2086
    capture_size $size
done
