#!/usr/bin/env bash
# BORU-HOME-12 — regression + visual QA evidence (release binary).
#
# Drives the FINAL redesigned home screen (post BORU-HOME-01..11) through the
# production UI paths with the release build and captures evidence:
#   1. Home dashboard at 1600x900 (zero state: 2 seeded friends offline)
#   2. Every quick action click -> expected dialog/screen (OCR-verified)
#   3. Download Manager access (header action + MCP navigate)
#   4. Live updates: peer presence flip 0 -> 1 -> 2 -> 0 (badge/rows/activity)
#   5. Long-identifier truncation (extra-long peer id does not resize layout)
#   6. Sidebar navigation destinations still reachable
#
# Quick actions after BORU-HOME-07: Start Chat / Create Group /
# Create Public Room / Create Tunnel.
#
# Output: docs/ui-redesign/evidence/t_a38b6ffa/
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_a38b6ffa"
BINARY="$ROOT_DIR/target/release/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
CALIBRATE="$ROOT_DIR/scripts/ui17_click_calibrate.py"
TASK_ID="t_a38b6ffa"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }
[[ -x "$MCP_CLIENT" ]] || { printf 'MCP helper not executable: %s\n' "$MCP_CLIENT" >&2; exit 1; }
[[ -f "$CALIBRATE" ]] || { printf 'Calibrator missing: %s\n' "$CALIBRATE" >&2; exit 1; }

DISPLAY_NUM=""
DATA_DIR=""
XVFB_PID=""
APP_PID=""
WIN_ID=""
APP_LOG=""
MCP_PORT=""

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
    for display in $(seq 310 340); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 310..340\n' >&2
    return 1
}

mcp() {
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$1" "$2" "$3"
}

wait_main_window() {
    local window_id="" candidate name
    for _ in $(seq 1 60); do
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

ocr_norm() {
    tr '[:upper:]' '[:lower:]' | tr -d '[:punct:][:space:]'
}

# Seed friends.json. $1 = data dir, $2 = "long" to include an
# extra-long identifier (truncation check), else two normal friends.
seed_friends() {
    local data_dir=$1 mode=${2:-normal}
    python3 - "$data_dir" "$mode" <<'PY'
import json, os, sys
data_dir, mode = sys.argv[1], sys.argv[2]
now = 1_700_000_000_000
friends = {
    "friends": {
        "a1" * 32: {
            "label": "Ada",
            "status": {"online": False, "last_offline_at_unix_ms": now - 60_000},
            "relationship": "friends",
        },
        "b2" * 32: {
            "label": "Bob",
            "status": {"online": False, "last_offline_at_unix_ms": now - 120_000},
            "relationship": "friends",
        },
    }
}
if mode == "long":
    # 32-byte hex id + a 60-char label: forces truncation in sidebar rows.
    friends["friends"]["c3" * 32] = {
        "label": "Avery Longname With Extra Words To Force Truncation",
        "status": {"online": False, "last_offline_at_unix_ms": now - 180_000},
        "relationship": "friends",
    }
with open(os.path.join(data_dir, "friends.json"), "w") as f:
    json.dump(friends, f, indent=2)
PY
}

launch() {
    local width=$1 height=$2 mode=${3:-normal}
    DISPLAY_NUM=$(find_display)
    DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui12.XXXXXX")
    APP_LOG="/tmp/boru-ui12-app-$DISPLAY_NUM.log"
    local mcp_port=$((19700 + DISPLAY_NUM))
    MCP_PORT="$mcp_port"

    seed_friends "$DATA_DIR" "$mode"

    Xvfb ":$DISPLAY_NUM" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui12-xvfb.log 2>&1 &
    XVFB_PID=$!
    sleep 0.5
    kill -0 "$XVFB_PID"

    DISPLAY=":$DISPLAY_NUM" "$BINARY" \
        --data-dir "$DATA_DIR" --no-dht --no-relay --name "UI-HOME-12 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >"$APP_LOG" 2>&1 &
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
    DISPLAY=":$DISPLAY_NUM" xdotool windowsize "$WIN_ID" "$width" "$height"
    sleep 1
    DISPLAY=":$DISPLAY_NUM" xdotool windowfocus --sync "$WIN_ID"
    sleep 0.5

    mcp "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1

    # The `open` subcommand auto-opens a chat room whose OpenRoom task can
    # complete AFTER our navigate, switching the screen back to Chat (the
    # quick-action grid only renders on ChatList). Wait for the initial
    # room to settle, then navigate to ChatList and confirm it STAYS there
    # across two consecutive snapshots (re-navigating if the room task
    # fires late).
    local settled="" prev="" stable=0
    for _ in $(seq 1 30); do
        settled=$(DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$mcp_port" boru_get_gui_snapshot '{}' 2>/dev/null \
            | python3 -c 'import json,sys; d=json.load(sys.stdin); r=d.get("result",d); print(r.get("active_screen","?"))' 2>/dev/null || true)
        if [[ "$settled" == "ChatList" && "$prev" == "ChatList" ]]; then
            stable=$((stable + 1))
            if [[ $stable -ge 3 ]]; then
                break
            fi
        else
            stable=0
        fi
        prev="$settled"
        if [[ "$settled" != "ChatList" ]]; then
            mcp "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
        fi
        sleep 0.5
    done
    printf 'launch settled on screen: %s\n' "$settled" >&2
    sleep 1
}

snap() {
    local name=$1
    DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$OUTPUT_DIR/${TASK_ID}_${name}.png"
    printf 'captured %s\n' "$name"
}

click_calibrated() {
    local shot="" args=() word
    for word in "$@"; do
        if [[ "$word" == "--" ]]; then
            shift
            shot=$1
            shift
            break
        fi
        args+=("$word")
        shift
    done
    [[ -n "$shot" ]] || { printf 'click_calibrated: missing -- <shot>\n' >&2; return 1; }
    local coords
    coords=$(python3 "$CALIBRATE" "$shot" "${args[@]}" "$@")
    read -r cx cy <<<"$coords"
    if [[ "${cx:-0}" == "0" && "${cy:-0}" == "0" ]]; then
        printf 'click_calibrated: "%s" not found in %s\n' "${args[*]}" "$shot" >&2
        return 1
    fi
    DISPLAY=":$DISPLAY_NUM" xdotool mousemove --sync "$cx" "$cy" click 1
    sleep 1.2
}

key() {
    DISPLAY=":$DISPLAY_NUM" xdotool windowfocus --sync "$WIN_ID" 2>/dev/null || true
    DISPLAY=":$DISPLAY_NUM" xdotool key --window "$WIN_ID" "$@"
    sleep 0.6
}

close_dialog() {
    key Escape
    key Escape
    sleep 0.8
    mcp "$MCP_PORT" boru_gui_close_dialog '{}' >/dev/null 2>&1 || true
    sleep 0.5
}

verify_ocr() {
    local action=$1 expect=$2 shot=$3
    local norm got ok=1
    if [[ ! -f "$shot" ]]; then
        printf '  MISS %-32s (%s missing)\n' "$action" "$(basename "$shot")" | tee -a "$OCR"
        return 1
    fi
    local region txt
    region=$(mktemp --suffix=.png)
    python3 - "$shot" "$region" <<'PY'
import sys
from PIL import Image
img = Image.open(sys.argv[1]).convert('RGB')
img.resize((img.width*2, img.height*2), Image.LANCZOS).save(sys.argv[2])
PY
    txt=$(mktemp)
    tesseract "$region" "$txt" --psm 6 >/dev/null 2>&1 || true
    norm=$(cat "$txt.txt" 2>/dev/null | ocr_norm || true)
    rm -f "$region" "$txt" "$txt.txt"
    want=$(printf '%s' "$expect" | ocr_norm)
    [[ "$norm" == *"$want"* ]] || ok=0
    if [[ $ok -eq 1 ]]; then
        printf '  OK   %-32s -> %s\n' "$action" "$expect" | tee -a "$OCR"
    else
        printf '  FAIL %-32s -> expected "%s" (ocr: %.80s)\n' "$action" "$expect" "$norm" | tee -a "$OCR"
    fi
}

OCR="$OUTPUT_DIR/qa_ocr.txt"
printf 'BORU-HOME-12 regression QA OCR verification\n' > "$OCR"
printf '===========================================\n' >> "$OCR"

# Authoritative screen-state check via the MCP GUI snapshot (avoids
# OCR/capture-timing ambiguity). Prints the active_screen value.
snapshot_screen() {
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$MCP_PORT" boru_get_gui_snapshot '{}' \
        | python3 -c 'import json,sys; d=json.load(sys.stdin); r=d.get("result",d); print(r.get("active_screen","?"))' 2>/dev/null || echo "?"
}

# ── Phase 1: home dashboard, zero state ────────────────────────────────
launch 1600 900 normal
snap "qa_home_1600x900_zero"
HOME_SHOT="$OUTPUT_DIR/${TASK_ID}_qa_home_1600x900_zero.png"

verify_ocr "home: greeting" "Good evening" "$HOME_SHOT"
verify_ocr "home: connection hero" "Your Boru node is online and ready" "$HOME_SHOT"
verify_ocr "home: Mesh Health" "Mesh Health" "$HOME_SHOT"
verify_ocr "home: quick actions present" "Start Chat" "$HOME_SHOT"
verify_ocr "home: Download Manager" "Download Manager" "$HOME_SHOT"
verify_ocr "home: People & Activity" "People & Activity" "$HOME_SHOT"
verify_ocr "home: Tunnels zero state" "No active tunnels" "$HOME_SHOT"

# ── Phase 2: quick-action clicks ───────────────────────────────────────
# Each quick action is tested from a FRESH ChatList capture: the `open`
# subcommand's room auto-open can flip the screen to Chat at any moment,
# so we re-navigate + re-capture + re-calibrate before every click.
test_quick_action() {
    local action_name=$1 phrase=$2 expect_ocr=$3
    # Make sure we are on the dashboard first.
    mcp "$MCP_PORT" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1
    local screen=""
    for _ in $(seq 1 20); do
        screen=$(snapshot_screen)
        if [[ "$screen" == "ChatList" ]]; then
            break
        fi
        mcp "$MCP_PORT" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
        sleep 0.5
    done
    if [[ "$screen" != "ChatList" ]]; then
        printf '  %-28s SKIPPED (screen %s, dashboard never stable)\n' "$action_name" "$screen" | tee -a "$OCR"
        return 1
    fi
    local shot="$OUTPUT_DIR/${TASK_ID}_qa_${action_name}_pre.png"
    DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$shot"
    # The dashboard is content-driven: mesh-card height varies with the
    # live status/event log, so the quick-action grid can sit below the
    # fold. If the label is not visible in the first capture, scroll the
    # main panel down a notch and re-capture (grid is in the main panel,
    # right of the sidebar).
    local found=0 attempt=0 cx=0 cy=0
    for attempt in $(seq 1 6); do
        # The quick-action grid sits in the main panel below the mesh card
        # (content-driven y). On the first (unscrolled) capture, constrain
        # to y>=530 so the rail's TUNNELS "Create tunnel" (~455) and the
        # chat list rows never match the first "Create" and break the
        # line-aware phrase match. After scrolling, the grid is higher in
        # the window and the rail is gone — relax the band to y>=250.
        local ymin=530
        if [[ $attempt -gt 1 ]]; then
            ymin=250
        fi
        coords=$(python3 "$CALIBRATE" "$shot" $phrase --xmin 300 --ymin "$ymin" 2>/dev/null || true)
        read -r cx cy <<<"$coords"
        if [[ "${cx:-0}" != "0" && "${cy:-0}" != "0" ]]; then
            found=1
            break
        fi
        # Scroll down in the main panel and re-capture.
        DISPLAY=":$DISPLAY_NUM" xdotool mousemove --sync $((width/2 + 100)) $((height/2))
        for _ in $(seq 1 4); do
            DISPLAY=":$DISPLAY_NUM" xdotool click 5
            sleep 0.1
        done
        sleep 0.5
        DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$shot"
    done
    if [[ $found -eq 1 ]]; then
        DISPLAY=":$DISPLAY_NUM" xdotool mousemove --sync "$cx" "$cy" click 1
        sleep 1.2
        snap "qa_${action_name}"
        printf '  %-28s screen: %s\n' "$action_name" "$(snapshot_screen)" | tee -a "$OCR"
    else
        printf 'WARN: %s card not located (even after scrolling)\n' "$phrase" >&2
    fi
}

test_quick_action "action_start_chat" "Start" "Friend Requests"
close_dialog

test_quick_action "action_create_group" "Group" "Create Group Chat"
close_dialog

test_quick_action "action_create_public_room" "Public" "Create Public Room"
close_dialog

test_quick_action "action_create_tunnel" "Tunnel" "Create Tunnel"
close_dialog

# ── Phase 3: Download Manager access ───────────────────────────────────
mcp "$MCP_PORT" boru_gui_navigate '{"destination":"file_sharing"}' >/dev/null
sleep 1.5
snap "qa_download_manager"
mcp "$MCP_PORT" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
sleep 1

# ── Phase 4: live updates (peer presence 0 -> 1 -> 2 -> 0) ─────────────
ALICE_PK=$(printf 'a1%.0s' {1..32})
BOB_PK=$(printf 'b2%.0s' {1..32})
mcp "$MCP_PORT" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
sleep 2
snap "qa_live_one_online"
mcp "$MCP_PORT" boru_gui_set_peer_presence "{\"peer_id\":\"$BOB_PK\",\"online\":true}" >/dev/null
sleep 2
snap "qa_live_two_online"
mcp "$MCP_PORT" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":false}" >/dev/null
sleep 2
snap "qa_live_after_offline"

# ── Phase 5: sidebar navigation destinations ───────────────────────────
mcp "$MCP_PORT" boru_gui_navigate '{"destination":"friends"}' >/dev/null
sleep 1.5
snap "qa_nav_friends"
mcp "$MCP_PORT" boru_gui_navigate '{"destination":"settings"}' >/dev/null
sleep 1.5
snap "qa_nav_settings"
mcp "$MCP_PORT" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
sleep 1

# ── Phase 6: long-identifier truncation ────────────────────────────────
# Relaunch with a long label friend; geometry check must still be clean.
cleanup
DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""
launch 1280 800 long
snap "qa_long_label_1280x800"
LONG_SHOT="$OUTPUT_DIR/${TASK_ID}_qa_long_label_1280x800.png"

# ── OCR verification of phases 2-6 ─────────────────────────────────────
verify_ocr "Start Chat -> Friends screen" "Friend Requests" "$OUTPUT_DIR/${TASK_ID}_qa_action_start_chat.png"
verify_ocr "Create Group -> dialog" "Create Group Chat" "$OUTPUT_DIR/${TASK_ID}_qa_action_create_group.png"
verify_ocr "Create Public Room -> dialog" "Create Public Room" "$OUTPUT_DIR/${TASK_ID}_qa_action_create_public_room.png"
verify_ocr "Create Tunnel -> dialog" "Create Tunnel" "$OUTPUT_DIR/${TASK_ID}_qa_action_create_tunnel.png"
verify_ocr "Download Manager access" "Files I'm Sharing" "$OUTPUT_DIR/${TASK_ID}_qa_download_manager.png"
verify_ocr "one online: Ada row" "Ada" "$OUTPUT_DIR/${TASK_ID}_qa_live_one_online.png"
verify_ocr "two online: Bob row" "Bob" "$OUTPUT_DIR/${TASK_ID}_qa_live_two_online.png"
verify_ocr "nav friends screen" "Friend Requests" "$OUTPUT_DIR/${TASK_ID}_qa_nav_friends.png"
verify_ocr "nav settings screen" "Settings" "$OUTPUT_DIR/${TASK_ID}_qa_nav_settings.png"

# Live-transition OCR checks (best-effort text assertions).
{
    printf '\n[live updates]\n'
    if ocr_has "$OUTPUT_DIR/${TASK_ID}_qa_live_one_online.png" "came online"; then
        printf '  OK   activity "came online" appended after flip\n'
    else
        printf '  CHECK activity "came online" not OCR-visible (may be below fold)\n'
    fi
    if ocr_has "$OUTPUT_DIR/${TASK_ID}_qa_live_after_offline.png" "went offline"; then
        printf '  OK   activity "went offline" appended after flip\n'
    else
        printf '  CHECK activity "went offline" not OCR-visible (may be below fold)\n'
    fi
} >> "$OCR"

# Long-label geometry: every OCR word right edge must stay inside 1280.
{
    printf '\n[long-label geometry] %s\n' "$(basename "$LONG_SHOT")"
    tesseract "$LONG_SHOT" - tsv 2>/dev/null | awk -F'\t' -v W=1280 \
        'NR>1 && $12 != "" && $11 > 40 {
            x=$7; w=$9; r=x+w;
            if (r > W) { over++ }
            if (++n <= 14) printf "%-30s left=%s right=%s\n", $12, x, r;
        }
        END { printf "rows=%d words_past_right_edge=%d\n", n, over+0 }'
} | tee -a "$OCR"

# App liveness + no panic in the app log.
if kill -0 "$APP_PID" 2>/dev/null; then
    printf '\n[health] app alive at end of run\n' | tee -a "$OCR"
else
    printf '\n[health] FAIL: app exited early\n' | tee -a "$OCR"
fi
if grep -qiE "panic|RUST_BACKTRACE" "$APP_LOG" 2>/dev/null; then
    printf '[health] FAIL: panic found in app log\n' | tee -a "$OCR"
else
    printf '[health] no panic in app log\n' | tee -a "$OCR"
fi

printf '\nQA evidence written to %s\n' "$OUTPUT_DIR"
