#!/usr/bin/env bash
# Capture UI-08 home hero evidence on the HOME screen (Screen::ChatList).
#
# The home hero only renders on the chat-list / landing screen. Launching with
# a subcommand (`open <topic>`) lands on the CHAT screen, so we launch with NO
# subcommand: the lobby opens and the app auto-returns to the chat list, where
# the greeting + connection hero card live.
#
# State isolation: every instance subscribes to the shared Mainnet lobby and
# discovers peers via mDNS on 224.0.0.251:5353. To capture truthful single-
# instance states (connecting, degraded) we temporarily drop mDNS multicast
# with iptables so the instance has NO peers; for ready we restore mDNS so two
# of our own instances discover each other.
#
#   connecting - fresh launch, no peers yet (amber pill "Connecting")
#   degraded   - fresh launch, wait past the 30s mesh watchdog -> Degraded
#   ready      - two instances on the same lobby topic (mDNS connects them)
#
# Output: docs/ui-redesign/evidence/ui-08/t_ed8af7fe_<state>_<width>x<height>.png
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-08"
BINARY="$ROOT_DIR/target/debug/examples/boru"
TASK_ID="t_ed8af7fe"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }

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

mdns_block() {
    sudo iptables -C OUTPUT -d 224.0.0.251/32 -p udp -m udp --dport 5353 -j DROP 2>/dev/null ||
        sudo iptables -I OUTPUT 1 -d 224.0.0.251/32 -p udp -m udp --dport 5353 -j DROP
    sudo iptables -C INPUT -s 224.0.0.251/32 -p udp -m udp --sport 5353 -j DROP 2>/dev/null ||
        sudo iptables -I INPUT 1 -s 224.0.0.251/32 -p udp -m udp --sport 5353 -j DROP
}

mdns_unblock() {
    sudo iptables -D OUTPUT -d 224.0.0.251/32 -p udp -m udp --dport 5353 -j DROP 2>/dev/null || true
    sudo iptables -D INPUT -s 224.0.0.251/32 -p udp -m udp --sport 5353 -j DROP 2>/dev/null || true
}

launch_no_cmd() {
    # $1 = display, $2 = data_dir, $3 = name
    local display=$1 data_dir=$2 name=$3 app_pid
    DISPLAY=":$display" "$BINARY" --data-dir "$data_dir" --no-dht --no-relay \
        --name "$name" >/tmp/boru-ui08-app-$display.log 2>&1 &
    app_pid=$!
    printf '%s\n' "$app_pid"
}

capture_window() {
    local display=$1 width=$2 height=$3 output=$4
    local window_id
    window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 0.6
    DISPLAY=":$display" import -window "$window_id" "$output"
}

cleanup_xvfb() {
    set +e
    [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
    wait "$xvfb_pid" 2>/dev/null
    rm -f /tmp/.X${xvfb_display:-}-lock 2>/dev/null
}

start_xvfb() {
    local display=$1 width=$2 height=$3
    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui08-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
}

# Cleanup any stray rules if we exit mid-run.
trap mdns_unblock EXIT

for size in '1280 800'; do
    set -- $size
    width=$1
    height=$2

    # ── Connecting: fresh launch, mDNS blocked, captured before the 30s watchdog ──
    mdns_block
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui08-connect.XXXXXX")
    xvfb_pid=""
    app_pid=""
    cleanup() {
        set +e
        [[ -n "$app_pid" ]] && kill "$app_pid" 2>/dev/null
        cleanup_xvfb
        rm -rf "$data_dir"
    }
    trap cleanup RETURN
    start_xvfb "$display" "$width" "$height"
    app_pid=$(launch_no_cmd "$display" "$data_dir" "UI-08 Connecting")
    sleep 6
    capture_window "$display" "$width" "$height" \
        "$OUTPUT_DIR/${TASK_ID}_connecting_${width}x${height}.png"
    trap - RETURN
    cleanup

    # ── Degraded: fresh launch, mDNS blocked, wait past the 30s watchdog ──
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui08-degraded.XXXXXX")
    xvfb_pid=""
    app_pid=""
    trap cleanup RETURN
    start_xvfb "$display" "$width" "$height"
    app_pid=$(launch_no_cmd "$display" "$data_dir" "UI-08 Degraded")
    sleep 38
    capture_window "$display" "$width" "$height" \
        "$OUTPUT_DIR/${TASK_ID}_degraded_${width}x${height}.png"
    trap - RETURN
    cleanup
    mdns_unblock

    # ── Ready: two instances on the same lobby topic (mDNS connects them) ──
    display_a=$(find_display)
    display_b=$(find_display)
    data_a=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui08-ready-a.XXXXXX")
    data_b=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui08-ready-b.XXXXXX")
    xvfb_pid=""
    app_pid=""
    app_pid_b=""
    cleanup_two() {
        set +e
        [[ -n "${app_pid_b:-}" ]] && kill "$app_pid_b" 2>/dev/null
        [[ -n "$app_pid" ]] && kill "$app_pid" 2>/dev/null
        cleanup_xvfb
        [[ -n "${xvfb_pid_b:-}" ]] && kill "$xvfb_pid_b" 2>/dev/null
        rm -rf "$data_a" "$data_b"
    }
    trap cleanup_two RETURN
    start_xvfb "$display_a" "$width" "$height"
    Xvfb ":$display_b" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui08-xvfb-b.log 2>&1 &
    xvfb_pid_b=$!
    sleep 0.5
    app_pid=$(launch_no_cmd "$display_a" "$data_a" "UI-08 Ready A")
    app_pid_b=$(launch_no_cmd "$display_b" "$data_b" "UI-08 Ready B")
    sleep 30
    capture_window "$display_a" "$width" "$height" \
        "$OUTPUT_DIR/${TASK_ID}_ready_${width}x${height}.png"
    trap - RETURN
    cleanup_two
done

trap - EXIT
printf 'captured UI-08 home-hero evidence in %s\n' "$OUTPUT_DIR"
