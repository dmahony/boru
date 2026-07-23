#!/usr/bin/env bash
# Lifecycle supervisor for Boru test instances on a VM.
# All state is confined to ~/boru-test and ~/boru-test/runs.
#
# Two launch modes:
#   start  — headless xvfb (virtual display, for automated testing)
#   desktop — native desktop (DISPLAY=:0, for user-facing GUI)
set -euo pipefail

STATE_DIR="${HOME}/boru-test"
PID_FILE="${STATE_DIR}/instance.pid"
DESKTOP_PID_FILE="${STATE_DIR}/desktop-instance.pid"

usage() {
    echo "usage: $0 start|desktop BINARY MCP_PORT DISPLAY DATA_DIR [RELAY_FLAG [RELAY_URL]] | stop [--desktop] | status MCP_PORT" >&2
    exit 2
}

cleanup_legacy_xvfb() {
    # Displays 98-127 are reserved for Boru test instances. This also removes
    # Xvfb left behind by older xvfb-run wrappers that lost their SSH parent.
    local signal stale_pid
    for signal in TERM KILL; do
        for stale_pid in $(pgrep -u "$(id -u)" -f '^Xvfb :(9[89]|1[01][0-9]|12[0-7]) ' 2>/dev/null || true); do
            kill -"$signal" "$stale_pid" 2>/dev/null || true
        done
        sleep 0.2
        [[ -z "$(pgrep -u "$(id -u)" -f '^Xvfb :(9[89]|1[01][0-9]|12[0-7]) ' 2>/dev/null || true)" ]] && break
    done
    pkill -u "$(id -u)" -f 'xvfb-run.*iced_chat' 2>/dev/null || true
}

stop_instance() {
    local pid=""
    # ── Kill by PID file (supervisor-managed instance) ────────────────
    for pf in "$PID_FILE" "$DESKTOP_PID_FILE"; do
        if [[ -s "$pf" ]]; then
            pid=$(<"$pf")
            if [[ "$pid" =~ ^[0-9]+$ ]] && kill -0 "$pid" 2>/dev/null; then
                # start uses setsid, so the supervisor is the process-group leader.
                kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
                for _ in {1..50}; do
                    kill -0 "$pid" 2>/dev/null || break
                    sleep 0.1
                done
                if kill -0 "$pid" 2>/dev/null; then
                    kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
                fi
            fi
            rm -f "$pf"
        fi
    done

    # ── Aggressively kill ALL iced_chat-x86_64-linux processes ──────
    # This catches instances launched from any path, with any flags,
    # including ones that predate the supervisor or were started manually.
    local stale
    for stale_pid in $(pgrep -u "$(id -u)" -f 'iced_chat-x86_64-linux' 2>/dev/null || true); do
        [[ "$stale_pid" == "$$" ]] && continue
        kill -TERM "$stale_pid" 2>/dev/null || true
    done
    # Also kill any bare 'iced_chat' debug binary or other-named variants
    for stale_pid in $(pgrep -u "$(id -u)" -x 'iced_chat' 2>/dev/null || true); do
        [[ "$stale_pid" == "$$" ]] && continue
        kill -TERM "$stale_pid" 2>/dev/null || true
    done
    # Give them a moment to exit gracefully, then SIGKILL survivors
    sleep 0.5
    for stale_pid in $(pgrep -u "$(id -u)" -f 'iced_chat' 2>/dev/null || true); do
        [[ "$stale_pid" == "$$" ]] && continue
        kill -KILL "$stale_pid" 2>/dev/null || true
    done

    cleanup_legacy_xvfb
}

# ── Headless xvfb run ─────────────────────────────────────────────────

run_instance() {
    local binary="$1" port="$2" display="$3" data_dir="$4" relay_flag="${5:-}" relay_url="${6:-}"
    local child
    local -a app_args=(--mcp --mcp-bind "127.0.0.1:${port}" --enable-gui-test-actions
        --data-dir "$data_dir" --bind-port 0)
    if [[ -n "$relay_flag" ]]; then
        if [[ "$relay_flag" == "--relay" && -n "$relay_url" ]]; then
            app_args=("$relay_flag" "$relay_url" "${app_args[@]}")
        else
            app_args=("$relay_flag" "${app_args[@]}")
        fi
    fi
    trap 'kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true; exit 143' TERM INT
    # Keep xvfb-run in the foreground so it owns and reaps Xvfb on shutdown.
    DISPLAY=":${display}" xvfb-run -a -n "$display" -s '-screen 0 1280x720x24' \
        "$binary" "${app_args[@]}" &
    child=$!
    wait "$child"
}

start_instance() {
    local binary="$1" port="$2" display="$3" data_dir="$4" relay_flag="${5:-}" relay_url="${6:-}"
    mkdir -p "$STATE_DIR" "$data_dir"
    stop_instance
    nohup setsid "$0" run "$binary" "$port" "$display" "$data_dir" "$relay_flag" "$relay_url" \
        >"${data_dir}/instance.log" 2>&1 < /dev/null &
    local pid=$!
    printf '%s\n' "$pid" >"$PID_FILE"
    printf '%s\n' "$pid"
}

# ── Native desktop run (DISPLAY=:0, no xvfb) ──────────────────────────

run_desktop() {
    local binary="$1" port="$2" data_dir="$3" relay_flag="${4:-}" relay_url="${5:-}"
    local child
    local -a app_args=(--mcp --mcp-bind "127.0.0.1:${port}" --enable-gui-test-actions
        --data-dir "$data_dir" --bind-port 0)
    if [[ -n "$relay_flag" ]]; then
        if [[ "$relay_flag" == "--relay" && -n "$relay_url" ]]; then
            app_args=("$relay_flag" "$relay_url" "${app_args[@]}")
        else
            app_args=("$relay_flag" "${app_args[@]}")
        fi
    fi
    # Use the user's Xauthority if available, fall back to the display manager's.
    local xauth=""
    if [[ -f "$HOME/.Xauthority" ]]; then
        xauth="$HOME/.Xauthority"
    elif [[ -f "/run/user/$(id -u)/Xauthority" ]]; then
        xauth="/run/user/$(id -u)/Xauthority"
    elif [[ -f "/run/sddm/xauth_"* ]]; then
        xauth=$(ls /run/sddm/xauth_* 2>/dev/null | head -1)
    fi
    trap 'kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true; exit 143' TERM INT
    export DISPLAY=:0
    [[ -n "$xauth" ]] && export XAUTHORITY="$xauth"
    "$binary" "${app_args[@]}" &
    child=$!
    # Bring window to front after it appears
    for _ in $(seq 1 20); do
        if DISPLAY=:0 xdotool search --name 'boru' > /dev/null 2>&1; then
            DISPLAY=:0 wmctrl -a 'boru-test' 2>/dev/null || true
            DISPLAY=:0 xdotool windowactivate "$(DISPLAY=:0 xdotool search --name 'boru')" 2>/dev/null || true
            break
        fi
        sleep 0.2
    done
    wait "$child"
}

start_desktop() {
    local binary="$1" port="$2" data_dir="$3" relay_flag="${4:-}" relay_url="${5:-}"
    mkdir -p "$STATE_DIR" "$data_dir"
    stop_instance
    nohup setsid "$0" run-desktop "$binary" "$port" "$data_dir" "$relay_flag" "$relay_url" \
        >"${data_dir}/instance.log" 2>&1 < /dev/null &
    local pid=$!
    printf '%s\n' "$pid" >"$DESKTOP_PID_FILE"
    printf '%s\n' "$pid"
}

# ── Status ────────────────────────────────────────────────────────────

status_instance() {
    local port="$1"
    printf 'pidfile: '
    [[ -s "$PID_FILE" ]] && cat "$PID_FILE" || echo none
    printf 'desktop_pidfile: '
    [[ -s "$DESKTOP_PID_FILE" ]] && cat "$DESKTOP_PID_FILE" || echo none
    printf 'iced_chat: '
    pgrep -u "$(id -u)" -af "iced_chat.*--mcp" || true
    printf 'iced_chat_binary_count: '
    ps -u "$(id -u)" -o args= | awk '$0 ~ /iced_chat-x86_64-linux/ && $0 !~ /xvfb-run/ && $0 !~ /awk/ {n++} END {print n+0}'
    printf 'xvfb_reserved: '
    pgrep -u "$(id -u)" -af '^Xvfb :(9[89]|1[01][0-9]|12[0-7]) ' || true
    printf 'mcp_port_%s: ' "$port"
    ss -ltnp 2>/dev/null | grep -E ":${port} " || true
}

[[ $# -ge 1 ]] || usage
case "$1" in
    start)     [[ $# -ge 5 ]] || usage; start_instance "${@:2}" ;;
    desktop)   [[ $# -ge 4 ]] || usage; start_desktop "${@:2}" ;;
    run)       [[ $# -ge 5 ]] || usage; run_instance "${@:2}" ;;
    run-desktop) [[ $# -ge 4 ]] || usage; run_desktop "${@:2}" ;;
    stop)      stop_instance ;;
    status)    [[ $# -eq 2 ]] || usage; status_instance "$2" ;;
    *)         usage ;;
esac
