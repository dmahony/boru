#!/usr/bin/env bash
# Root UI-10 evidence: populated right-rail cards (Online Peers, Recent
# Activity, Tunnels) plus live online->offline update evidence.
#
# Closes the UI-10 reviewer gate gaps for t_42abbf42:
#   1. populated screenshot at the wide target window (empty already exists),
#   2. a >=15 peer / >=15 activity fixture demonstrating bounded overflow,
#   3. a live online->offline transition capture.
#
# Fixture strategy (truthful, no sample data in the render path):
#   - Seed friends.json with 16 deterministic friends, each with a
#     last_offline_at_unix_ms value so the production handle_friend_event
#     treats them as "has_been_seen" and pushes genuine activity events.
#   - Drive boru_gui_set_peer_presence online:true for all 16 via the MCP
#     action. That action routes through FriendEvent::StatusChanged — the
#     same path real network events use — so the Online Peers card renders
#     real presence rows and the Recent Activity card receives 16 real
#     "came online" events through push_activity.
#   - Capture the populated rail, then flip 3 peers offline and capture
#     again for the live online->offline evidence (badge 13/16, activity
#     gains "went offline" rows).
#   - Tunnels stays in its truthful empty state: TunnelService is in-memory
#     only and an isolated run has no real friend/tunnel to display (already
#     accepted in t_5f03f97d).
#
# Output: docs/ui-redesign/evidence/ui-10/
#   t_42abbf42_populated_1280x800.png    - 16 peers online, 16 activity rows
#   t_42abbf42_populated_600x720.png     - compact responsive populated rail
#   t_42abbf42_live_before_1280x800.png  - 16/16 just before the transition
#   t_42abbf42_live_after_1280x800.png   - 13/16 after 3 peers go offline
#   t_42abbf42_zoom_1280x800.png         - zoomed crop of the populated rail
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/ui-10"
BINARY="$ROOT_DIR/target/debug/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
TASK_ID="t_42abbf42"
NUM_PEERS=16

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }
[[ -x "$MCP_CLIENT" ]] || { printf 'MCP helper not executable: %s\n' "$MCP_CLIENT" >&2; exit 1; }

find_display() {
    local display
    for display in $(seq 200 220); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 200..220\n' >&2
    return 1
}

mcp() {
    local display=$1 port=$2 method=$3 params=$4
    DISPLAY=":$display" python3 "$MCP_CLIENT" "$port" "$method" "$params"
}

wait_main_window() {
    local display=$1
    local window_id=""
    local candidate name
    for _ in $(seq 1 60); do
        candidate=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' 2>/dev/null | tail -n 1 || true)
        if [[ -n "$candidate" ]]; then
            name=$(DISPLAY=":$display" xdotool getwindowname "$candidate" 2>/dev/null || true)
            if [[ "$name" == *"v0."* ]]; then
                window_id="$candidate"
                break
            fi
        fi
        sleep 0.5
    done
    [[ -n "$window_id" ]] || { printf 'Boru main window never appeared\n' >&2; return 1; }
    printf '%s\n' "$window_id"
}

wait_mcp() {
    local display=$1 port=$2
    local attempt
    for attempt in $(seq 1 180); do
        if mcp "$display" "$port" boru_ping '{}' >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

set_presence() {
    local display=$1 port=$2 key=$3 online=$4
    local resp attempt
    for attempt in 1 2 3; do
        resp=$(mcp "$display" "$port" boru_gui_set_peer_presence \
            "{\"peer_id\":\"$key\",\"online\":$online}" 2>&1 || true)
        if printf '%s' "$resp" | grep -q '"error"'; then
            sleep 0.2
        else
            return 0
        fi
    done
    return 1
}

seed_friends() {
    local data_dir=$1
    # Writes friends.json with 16 friends keyed by REAL Ed25519 public keys
    # (generated the same way seed_boru_data.py creates the local identity).
    # iroh PublicKey hex parsing validates the decoded Ed25519 point, so
    # arbitrary hex strings are rejected by the friends loader — the keys
    # must be genuine public keys. Also writes peer_keys.txt (one key per
    # line) for the MCP presence calls below.
    python3 - "$data_dir" <<'PY'
import json, os, sys
data_dir = sys.argv[1]
os.makedirs(data_dir, exist_ok=True)
now_ms = int(__import__("time").time() * 1000)
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

friends = {"schema_version": 4, "friends": {}}
keys = []
for i in range(1, 17):
    # Deterministic seed so re-runs produce the same friend set.
    seed = bytes([i]) * 32
    priv = Ed25519PrivateKey.from_private_bytes(seed)
    pk = priv.public_key().public_bytes_raw().hex()
    keys.append(pk)
    friends["friends"][pk] = {
        "label": f"Peer {i:02d}",
        # last_offline_at makes handle_friend_event treat the friend as
        # has_been_seen, so presence changes push real activity events.
        "status": {
            "online": False,
            "last_offline_at_unix_ms": now_ms - 60_000,
        },
        "relationship": "friends",
    }
with open(os.path.join(data_dir, "friends.json"), "w") as f:
    json.dump(friends, f, indent=2)
with open(os.path.join(data_dir, "peer_keys.txt"), "w") as f:
    f.write("\n".join(keys) + "\n")
print(f"seeded {len(friends['friends'])} friends into {data_dir}")
PY
}

capture_state() {
    local state=$1 width=$2 height=$3
    local display data_dir mcp_port xvfb_pid app_pid window_id
    display=$(find_display)
    data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui10.XXXXXX")
    mcp_port=$((18800 + display))
    xvfb_pid=""
    app_pid=""
    cleanup() {
        set +e
        [[ -n "${app_pid:-}" ]] && kill "$app_pid" 2>/dev/null
        [[ -n "${xvfb_pid:-}" ]] && kill "$xvfb_pid" 2>/dev/null
        wait "${app_pid:-}" 2>/dev/null
        wait "${xvfb_pid:-}" 2>/dev/null
        rm -rf "$data_dir"
    }
    trap cleanup RETURN

    seed_friends "$data_dir"

    Xvfb ":$display" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui10-xvfb.log 2>&1 &
    xvfb_pid=$!
    sleep 0.5
    kill -0 "$xvfb_pid"

    DISPLAY=":$display" "$BINARY" \
        --data-dir "$data_dir" --no-dht --no-relay --name "UI-10 Rail Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" \
        >/tmp/boru-ui10-app-$display.log 2>&1 &
    app_pid=$!

    wait_mcp "$display" "$mcp_port" || { printf 'MCP never came up\n' >&2; return 1; }
    sleep 2

    # Home (chat list) shows the Figure 3 rail.
    mcp "$display" "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null 2>&1 || true
    sleep 1

    # Mark all NUM_PEERS friends online through the production path. The GUI
    # action queue rate-limits to 10 actions/sec, so pace the calls.
    local i key
    local -a peer_keys=()
    mapfile -t peer_keys < "$data_dir/peer_keys.txt"
    for key in "${peer_keys[@]}"; do
        set_presence "$display" "$mcp_port" "$key" true || printf 'presence for %s failed\n' "${key:0:8}"
        sleep 0.15
    done
    sleep 3

    window_id=$(wait_main_window "$display") || return 1
    DISPLAY=":$display" xdotool windowsize "$window_id" "$width" "$height"
    sleep 1
    DISPLAY=":$display" xdotool windowactivate --sync "$window_id" 2>/dev/null || true
    sleep 0.8

    local out="$OUTPUT_DIR/${TASK_ID}_${state}_${width}x${height}.png"
    DISPLAY=":$display" import -window "$window_id" "$out"
    printf 'captured %s\n' "$out"

    if [[ "$state" == "live_before" ]]; then
        # Live online->offline transition: take the last 3 peers offline.
        local -a offline_keys=("${peer_keys[@]: -3}")
        for key in "${offline_keys[@]}"; do
            set_presence "$display" "$mcp_port" "$key" false || printf 'presence for %s offline failed\n' "${key:0:8}"
            sleep 0.15
        done
        sleep 3
        local out_after="$OUTPUT_DIR/${TASK_ID}_live_after_${width}x${height}.png"
        DISPLAY=":$display" import -window "$window_id" "$out_after"
        printf 'captured %s\n' "$out_after"
    fi
}

# ── States ────────────────────────────────────────────────────────────
capture_state populated 1280 800
capture_state populated 600 720
capture_state live_before 1280 800

# Zoomed crop of the populated rail (wide capture, right column region).
convert "$OUTPUT_DIR/${TASK_ID}_populated_1280x800.png" \
    -crop 360x700+900+40 +repage \
    "$OUTPUT_DIR/${TASK_ID}_zoom_1280x800.png"
printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
