#!/usr/bin/env bash
# FS-24 visual QA screenshots: File Sharing dashboard at wide, reference, narrow.
# Uses MCP to navigate to the File Sharing screen.
# Output: docs/ui-redesign/evidence/fs-24/fs24_<width>x<height>.png
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/fs-24"
BINARY="$ROOT_DIR/target/debug/boru"
TASK_ID="t_f4f6f34d"
MCP_PORT=18765

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }

find_display() {
    for display in $(seq 190 230); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 190..230\n' >&2
    return 1
}

mcp_call() {
    local method=$1 params=${2:-'{}'}
    python3 -c "
import json, socket
req = {'jsonrpc': '2.0', 'method': '${method}', 'params': ${params}, 'id': 1}
payload = (json.dumps(req, separators=(',', ':')) + '\n').encode()
with socket.create_connection(('127.0.0.1', ${MCP_PORT}), timeout=15) as conn:
    conn.sendall(payload)
    line = conn.makefile('rb').readline()
print(line.decode() if line else '{}')
" 2>/dev/null || echo '{"error":"mcp call failed"}'
}

capture_window() {
    local display=$1 width=$2 height=$3 output=$4
    local window_id
    window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | head -n 1)
    if [[ -z "$window_id" ]]; then
        printf 'ERROR: no Boru window found on display :%s\n' "$display" >&2
        return 1
    fi
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 0.8
    DISPLAY=":$display" import -window "$window_id" "$output"
    printf 'Captured: %s\n' "$output"
}

for size in '1440 900' '1280 800' '1024 720'; do
    set -- $size
    width=$1
    height=$2

    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-fs24.XXXXXX")
    xvfb_pid=""
    app_pid=""

    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "$app_pid" 2>/dev/null
        wait "$xvfb_pid" 2>/dev/null
        rm -f "/tmp/.X${display}-lock" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-fs24-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5

    # Launch with MCP enabled
    DISPLAY=":$display" "$BINARY" --data-dir "$data_dir" --no-dht --no-relay \
        --name "FS-24 QA" --mcp --mcp-bind "127.0.0.1:${MCP_PORT}" \
        --enable-gui-test-actions >/tmp/boru-fs24-app.log 2>&1 &
    app_pid=$!
    sleep 6

    # Navigate to File Sharing via MCP
    mcp_call boru_gui_navigate '{"destination":"file_sharing"}'
    sleep 2

    capture_window "$display" "$width" "$height" \
        "$OUTPUT_DIR/${TASK_ID}_file_sharing_${width}x${height}.png"

    trap - RETURN
    cleanup
done

printf '\nCaptured FS-24 screenshots in %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR/"
