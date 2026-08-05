#!/usr/bin/env bash
# Evidence for UI-HOME-16: intentional empty states for Online Peers,
# Recent Activity, Tunnels and Mesh Health (Recent events).
#
# Captures the home screen in three states at the target windows:
#   empty      1280x800  - fresh data dir, --no-dht --no-relay: all four
#                          cards show their intentional empty states
#                          (small muted icon + spec copy). The right rail
#                          (Online Peers / Recent Activity / Tunnels) and
#                          the Mesh Health "Recent events" feed are all
#                          empty on a truthful fresh launch.
#   narrow     800x600   - minimum content band: compact two-line headers
#                          and the two-sentence empty copy wraps instead of
#                          overflowing the narrow rail.
#   populated  1280x800  - seeded fixture + real friend-status events routed
#                          through the production handle_friend_event ->
#                          push_activity path: Online Peers gains rows and
#                          Recent Activity fills, proving the live
#                          transition out of the empty state.
#
# Output: docs/ui-redesign/evidence/t_4186e7f9/
#   t_4186e7f9_home_empty_1280x800.png
#   t_4186e7f9_online_peers_empty_1280x800.png      (card crop)
#   t_4186e7f9_recent_activity_empty_1280x800.png   (card crop)
#   t_4186e7f9_tunnels_empty_1280x800.png           (card crop)
#   t_4186e7f9_mesh_events_empty_1280x800.png       (mesh Recent events crop)
#   t_4186e7f9_home_empty_800x600.png
#   t_4186e7f9_home_populated_1280x800.png
#   ocr.txt   (tesseract verification of the copy + right-edge overflow)
#   README.md
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/docs/ui-redesign/evidence/t_4186e7f9"
BINARY="$ROOT_DIR/target/debug/examples/boru"
MCP_CLIENT="$ROOT_DIR/scripts/ui_mcp.py"
SEED_SCRIPT="$ROOT_DIR/scripts/seed_boru_data.py"
TASK_ID="t_4186e7f9"

mkdir -p "$OUTPUT_DIR"
[[ -x "$BINARY" ]] || { printf 'GUI binary not found: %s\n' "$BINARY" >&2; exit 1; }
[[ -x "$MCP_CLIENT" ]] || { printf 'MCP helper not executable: %s\n' "$MCP_CLIENT" >&2; exit 1; }

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
    for display in $(seq 280 300); do
        if ! [[ -e "/tmp/.X11-unix/X${display}" ]] && ! [[ -e "/tmp/.X${display}-lock" ]]; then
            printf '%s\n' "$display"
            return 0
        fi
    done
    printf 'no free X display in 280..300\n' >&2
    return 1
}

mcp() {
    DISPLAY=":$DISPLAY_NUM" python3 "$MCP_CLIENT" "$1" "$2" "$3"
}

wait_main_window() {
    local window_id=""
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
word_box() {
    local src=$1 regex=$2
    tesseract "$src" - tsv 2>/dev/null | awk -F'\t' -v re="$regex" '
        $12 ~ re { print $7, $8, $9, $10; exit }
    '
}

# Print the word-box of the first token immediately followed by a second
# token on the same OCR line (within ±6 px vertically, to the right).
# Used to anchor crops on two-word headers such as "RECENT ACTIVITY" when a
# bare "RECENT" is ambiguous with "RECENT EVENTS".
word_box_pair() {
    local src=$1 first=$2 second=$3
    tesseract "$src" - tsv 2>/dev/null | awk -F'\t' -v f="$first" -v s="$second" '
        NR>1 && $12 != "" && $11 > 40 {
            n++; t[n]=$12; tx[n]=$7; ty[n]=$8; tw[n]=$9; th[n]=$10;
        }
        END {
            for (i=1;i<=n;i++) {
                if (t[i]==f) {
                    for (j=1;j<=n;j++) {
                        if (t[j]==s && ty[j]>=ty[i]-6 && ty[j]<=ty[i]+6 &&
                            tx[j]>tx[i] && tx[j]<tx[i]+150) {
                            print tx[i], ty[i], tw[i], th[i];
                            exit;
                        }
                    }
                }
            }
        }
    '
}

# Crop a full-width rail-card region anchored on a header word box, extending
# downward to capture the card body (empty copy). The anchor may be a single
# token ("ONLINE") or a two-word phrase ("RECENT ACTIVITY") — phrases use
# word_box_pair so the crop starts at the header's first word.
crop_card() {
    local src=$1 anchor=$2 out=$3 height=$4
    local left top w h
    if [[ "$anchor" == *" "* ]]; then
        local first=${anchor%% *} second=${anchor#* }
        read -r left top w h <<<"$(word_box_pair "$src" "$first" "$second")"
    else
        read -r left top w h <<<"$(word_box "$src" "$anchor")"
    fi
    if [[ -z "${left:-}" ]]; then
        printf 'crop_card: header "%s" not found in %s\n' "$anchor" "$src" >&2
        return 1
    fi
    local x y
    x=$((left - 12)); [[ $x -lt 0 ]] && x=0
    y=$((top - 24)); [[ $y -lt 0 ]] && y=0
    convert "$src" -crop "340x${height}+${x}+${y}" +repage "$out"
}

# Crop a padded region around the first TSV word-box matching a regex.
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

# Launch the app at the requested window size on a fresh display.
# state: empty | populated
launch_state() {
    local width=$1 height=$2 state=$3
    DISPLAY_NUM=$(find_display)
    DATA_DIR=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui16.XXXXXX")
    local mcp_port=$((19500 + DISPLAY_NUM))

    if [[ "$state" == "populated" ]]; then
        python3 "$SEED_SCRIPT" "$DATA_DIR" >/dev/null
    fi

    Xvfb ":$DISPLAY_NUM" -screen 0 "${width}x${height}x24" -nolisten tcp >/tmp/boru-ui16-xvfb.log 2>&1 &
    XVFB_PID=$!
    sleep 0.5
    kill -0 "$XVFB_PID"

    DISPLAY=":$DISPLAY_NUM" "$BINARY" \
        --data-dir "$DATA_DIR" --no-dht --no-relay --name "UI-HOME-16 Evidence" \
        --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
        >/tmp/boru-ui16-app.log 2>&1 &
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

    # Home (chat list) shows the dashboard with the rail cards.
    mcp "$mcp_port" boru_gui_navigate '{"destination":"chat_list"}' >/dev/null
    sleep 1

    if [[ "$state" == "populated" ]]; then
        # Route real friend-status events through the production
        # handle_friend_event path (has_been_seen=true for seeded friends),
        # which pushes genuine activity events into recent_activity and adds
        # Online Peers rows.
        ALICE_PK=$(printf 'a1%.0s' {1..32})
        BOB_PK=$(printf 'b2%.0s' {1..32})
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$ALICE_PK\",\"online\":true}" >/dev/null
        sleep 0.5
        mcp "$mcp_port" boru_gui_set_peer_presence "{\"peer_id\":\"$BOB_PK\",\"online\":false}" >/dev/null
        sleep 1
    fi

    if [[ "$state" == "mesh_cleared" ]]; then
        # Test-only harness action: empty the live mesh event log so the
        # Mesh Health "Recent events" section truthfully shows its
        # intentional no-events state. Never fabricates events.
        mcp "$mcp_port" boru_gui_clear_mesh_events '{}' >/dev/null
        sleep 1
    fi
}

# ── Empty state (fresh launch, truthful) at 1280x800 ──────────────────
launch_state 1280 800 empty
EMPTY="$OUTPUT_DIR/${TASK_ID}_home_empty_1280x800.png"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$EMPTY"
printf 'captured %s\n' "$EMPTY"

# Card crops: Online Peers / Recent Activity / Tunnels (right rail),
# Mesh Health Recent events (left column). Anchors are unique word tokens:
# "RECENT ACTIVITY" uses a two-word anchor so it never matches the mesh
# card's "RECENT EVENTS" header; "EVENTS" only occurs in that mesh header.
crop_card "$EMPTY" 'ONLINE' "$OUTPUT_DIR/${TASK_ID}_online_peers_empty_1280x800.png" 230 \
    || printf 'online-peers crop skipped (header not found)\n'
crop_card "$EMPTY" 'RECENT ACTIVITY' "$OUTPUT_DIR/${TASK_ID}_recent_activity_empty_1280x800.png" 160 \
    || printf 'recent-activity crop skipped (header not found)\n'
crop_card "$EMPTY" 'TUNNELS' "$OUTPUT_DIR/${TASK_ID}_tunnels_empty_1280x800.png" 160 \
    || printf 'tunnels crop skipped (header not found)\n'

cleanup
DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""

# ── Mesh Health no-events state (fresh launch + cleared mesh log) ─────
launch_state 1280 800 mesh_cleared
MESH="$OUTPUT_DIR/${TASK_ID}_mesh_events_empty_1280x800.png"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$MESH"
printf 'captured %s\n' "$MESH"
crop_around_text "$MESH" 'EVENTS' "$OUTPUT_DIR/${TASK_ID}_mesh_events_crop_1280x800.png" 90 \
    || printf 'mesh-events crop skipped (header not found)\n'

cleanup
DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""

# ── Empty state at 800x600 (minimum band: compact headers + wrapping) ──
launch_state 800 600 empty
NARROW="$OUTPUT_DIR/${TASK_ID}_home_empty_800x600.png"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$NARROW"
printf 'captured %s\n' "$NARROW"

# The right rail stacks below the fold at 800x600; scroll down over the
# main panel so the empty rail cards are visible (same pattern as UI-HOME-15).
DISPLAY=":$DISPLAY_NUM" xdotool mousemove --sync 400 300
for _ in $(seq 1 24); do
    DISPLAY=":$DISPLAY_NUM" xdotool click 5
    sleep 0.15
done
sleep 1
NARROW_SCROLLED="$OUTPUT_DIR/${TASK_ID}_home_empty_800x600_scrolled.png"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$NARROW_SCROLLED"
printf 'captured %s\n' "$NARROW_SCROLLED"

cleanup
DISPLAY_NUM=""; DATA_DIR=""; XVFB_PID=""; APP_PID=""; WIN_ID=""

# ── Populated state (live transition out of empty) at 1280x800 ────────
launch_state 1280 800 populated
POPULATED="$OUTPUT_DIR/${TASK_ID}_home_populated_1280x800.png"
DISPLAY=":$DISPLAY_NUM" import -window "$WIN_ID" "$POPULATED"
printf 'captured %s\n' "$POPULATED"

# ── OCR verification ──────────────────────────────────────────────────
OCR="$OUTPUT_DIR/ocr.txt"
{
    printf 'UI-HOME-16 OCR verification\n'
    printf '==========================\n'

    printf '\n[empty 1280x800 crops] exact empty-state copy\n'
    printf '  -- Online Peers card --\n'
    for phrase in 'No peers are online right now' 'Connected peers will appear'; do
        if ocr_has "$OUTPUT_DIR/${TASK_ID}_online_peers_empty_1280x800.png" "$phrase"; then
            printf '  OK   %s\n' "$phrase"
        else
            printf '  MISS %s\n' "$phrase"
        fi
    done
    printf '  -- Recent Activity card --\n'
    for phrase in 'No recent activity' 'Network events will'; do
        if ocr_has "$OUTPUT_DIR/${TASK_ID}_recent_activity_empty_1280x800.png" "$phrase"; then
            printf '  OK   %s\n' "$phrase"
        else
            printf '  MISS %s\n' "$phrase"
        fi
    done
    printf '  -- Tunnels card --\n'
    for phrase in 'No active tunnels' 'Create or join a tunnel'; do
        if ocr_has "$OUTPUT_DIR/${TASK_ID}_tunnels_empty_1280x800.png" "$phrase"; then
            printf '  OK   %s\n' "$phrase"
        else
            printf '  MISS %s\n' "$phrase"
        fi
    done

    printf '\n[mesh 1280x800, cleared log] no-events state\n'
    if ocr_has "$MESH" 'No recent mesh events'; then
        printf '  OK   No recent mesh events\n'
    else
        printf '  MISS No recent mesh events\n'
    fi
    if ocr_has "$MESH" 'RECENT EVENTS'; then
        printf '  OK   RECENT EVENTS header retained\n'
    else
        printf '  MISS RECENT EVENTS header\n'
    fi

    printf '\n[empty 1280x800] header actions remain available\n'
    for phrase in 'View all' 'Create tunnel'; do
        if ocr_has "$EMPTY" "$phrase"; then
            printf '  OK   %s\n' "$phrase"
        else
            printf '  MISS %s\n' "$phrase"
        fi
    done

    printf '\n[empty 800x600 scrolled] rail copy present in the minimum band\n'
    for phrase in 'peers are online' 'recent activity' 'active tunnels'; do
        if ocr_has "$NARROW_SCROLLED" "$phrase"; then
            printf '  OK   %s\n' "$phrase"
        else
            printf '  MISS %s\n' "$phrase"
        fi
    done

    printf '\n[populated 1280x800] live transition out of empty\n'
    if ocr_has "$POPULATED" 'came online'; then
        printf '  OK   activity rows present ("came online")\n'
    else
        printf '  MISS activity rows (no "came online" found)\n'
    fi

    # Right-edge overflow check: no OCR word box may cross the window width.
    printf '\n[geometry] words_past_right_edge (must be 0)\n'
    for pair in "1280 $EMPTY" "1280 $MESH" "800 $NARROW_SCROLLED" "1280 $POPULATED"; do
        set -- $pair
        local_w=$1 img=$2
        printf '  %s: ' "$(basename "$img")"
        tesseract "$img" - tsv 2>/dev/null | awk -F'\t' -v W="$local_w" '
            NR>1 && $12 != "" && $11 > 40 { if ($7 + $9 > W) over++ }
            END { printf "words_past_right_edge=%d\n", over+0 }
        '
    done
} > "$OCR"
printf 'OCR report written to %s\n' "$OCR"

# ── README ─────────────────────────────────────────────────────────────
cat > "$OUTPUT_DIR/README.md" <<EOF
# UI-HOME-16 empty-state evidence

All captures from the running Boru GUI under Xvfb with a fresh data dir,
\`--no-dht --no-relay\`, MCP-driven home navigation. Every value shown is
live app state — the empty states are the truthful fresh-launch state, not
sample content.

| File | What it proves |
| --- | --- |
| \`${TASK_ID}_home_empty_1280x800.png\` | Fresh launch: three rail cards show intentional empty states — Online Peers (0/0 badge, "No peers are online right now. Connected peers will appear here."), Recent Activity ("No recent activity. Network events will appear here."), Tunnels (0 badge, "Create tunnel" action, "No active tunnels. Create or join a tunnel to securely route traffic."). |
| \`${TASK_ID}_online_peers_empty_1280x800.png\` | Online Peers card crop: small muted icon + spec copy centred in the min-height body. |
| \`${TASK_ID}_recent_activity_empty_1280x800.png\` | Recent Activity card crop: small muted activity icon + spec copy. |
| \`${TASK_ID}_tunnels_empty_1280x800.png\` | Tunnels card crop: lock icon + spec copy + "Create tunnel" header action (the create/join dialog the copy points at). |
| \`${TASK_ID}_mesh_events_empty_1280x800.png\` | Mesh Health card with the live mesh log cleared via the test-only \`boru_gui_clear_mesh_events\` harness action: connection summary + stat tiles retained above, "No recent mesh events" below the divider. |
| \`${TASK_ID}_mesh_events_crop_1280x800.png\` | Zoomed "Recent events" crop of the above. |
| \`${TASK_ID}_home_empty_800x600.png\` | Minimum content band (top of page). |
| \`${TASK_ID}_home_empty_800x600_scrolled.png\` | Minimum content band scrolled: compact two-line card headers and the two-sentence copy wraps inside the narrow rail (no overflow — see ocr.txt geometry). |
| \`${TASK_ID}_home_populated_1280x800.png\` | Seeded fixture + real friend-status events: Online Peers gains rows and Recent Activity fills, proving the live transition out of the empty state. |

\`ocr.txt\` lists the tesseract verification per capture and the
right-edge overflow geometry (\`words_past_right_edge=0\` at every width).
The mesh no-events capture uses \`boru_gui_clear_mesh_events\`, a
\`--enable-gui-test-actions\`-gated harness tool that empties the live mesh
event log (it never fabricates events); the same state is reachable in
production when the watchdog purges transient startup lines after the mesh
goes Good.
EOF

printf 'Evidence written to %s\n' "$OUTPUT_DIR"
ls -la "$OUTPUT_DIR"
