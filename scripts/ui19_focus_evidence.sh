#!/usr/bin/env bash
# UI-19 evidence (t_8a0db9ba): accessibility — focus, contrast, tooltips.
#
# Captures the real GUI under Xvfb for:
#   1. Focus states — composer focused (focus ring), chat search focused,
#      new-chat dialog focused.
#   2. Icon-only button tooltips — composer attach / send.
#   3. Status-not-colour-only — presence label + dot, online dot tooltip.
#   4. Keyboard-only traversal — Tab / Shift+Tab / Escape / Ctrl+N.
#
# Mirrors scripts/ui15_composer_evidence.sh launch + fixture pattern.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY=${BORU_BINARY:-$ROOT_DIR/target/debug/examples/boru}
MCP_CLIENT=$ROOT_DIR/scripts/ui_mcp.py
FIXTURE=$ROOT_DIR/scripts/figure4_fixture.py
SPEC=${BORU_SPEC:-$ROOT_DIR/docs/ui-redesign/evidence/ui-14/ui14-states-spec.json}
EVIDENCE_DIR=$ROOT_DIR/docs/ui-redesign/evidence/ui-19
WIDTH=${BORU_WIDTH:-1280}
HEIGHT=${BORU_HEIGHT:-800}

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
[[ -x "$BINARY" ]] || fail "GUI binary not found: $BINARY (build with: cargo build --features gui --example boru)"
[[ -x "$MCP_CLIENT" ]] || fail "MCP client is not executable: $MCP_CLIENT"
[[ -f "$SPEC" ]] || fail "states spec not found: $SPEC"
command -v Xvfb >/dev/null || fail "Xvfb is required for headless capture"
command -v xdotool >/dev/null || fail "xdotool is required for headless capture"
command -v import >/dev/null || fail "ImageMagick import is required for screenshot capture"

mkdir -p "$EVIDENCE_DIR"
display=""
data_dir=""
xvfb_pid=""
app_pid=""
cleanup() {
    set +e
    [[ -n "$app_pid" ]] && kill "$app_pid" 2>/dev/null || true
    [[ -n "$xvfb_pid" ]] && kill "$xvfb_pid" 2>/dev/null || true
    [[ -n "$app_pid" ]] && wait "$app_pid" 2>/dev/null || true
    [[ -n "$xvfb_pid" ]] && wait "$xvfb_pid" 2>/dev/null || true
    [[ -n "$data_dir" ]] && rm -rf "$data_dir"
}
trap cleanup EXIT

for candidate in $(seq 260 299); do
    if [[ ! -e "/tmp/.X11-unix/X${candidate}" && ! -e "/tmp/.X${candidate}-lock" ]]; then
        display=$candidate
        break
    fi
done
[[ -n "$display" ]] || fail "no free X display in 260..299"

data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui19.XXXXXX")
python3 "$FIXTURE" inject "$data_dir" --spec "$SPEC" --now-ms "$(date +%s%3N)" >/dev/null

mcp_port=$((19000 + display))
Xvfb ":$display" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp >/tmp/boru-ui19-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5
kill -0 "$xvfb_pid" 2>/dev/null || fail "Xvfb failed to start"

DISPLAY=":$display" "$BINARY" \
    --data-dir "$data_dir" --no-dht --no-relay --name "UI-19 Focus" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
    >/tmp/boru-ui19-app.log 2>&1 &
app_pid=$!

for attempt in $(seq 1 80); do
    if DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null 2>&1; then
        break
    fi
    sleep 0.25
done
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_ping '{}' >/dev/null \
    || fail "GUI MCP endpoint did not become ready"

remote_pk=28d7ee8656$(printf 'ab%.0s' {1..27})
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_open_conversation \
    "{\"conversation_id\":\"$remote_pk\"}" >/dev/null

shot() { # $1=name
    DISPLAY=":$display" import -window root "$EVIDENCE_DIR/$1.png"
    echo "captured $1"
}
mcp() { DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" "$1" "$2"; }

# Wait for the window to actually map and render (blank-root check).
sleep 3.0
for _ in $(seq 1 10); do
    sz=$(DISPLAY=":$display" import -window root png:- 2>/dev/null | wc -c)
    if [[ "$sz" -gt 50000 ]]; then break; fi
    sleep 0.5
done

# ── 1. Focus states ───────────────────────────────────────────────────
mcp boru_gui_focus_composer '{}' >/dev/null 2>&1
sleep 0.6
shot "01-composer-focused"

mcp boru_gui_set_composer '{"text":"hello ui19"}' >/dev/null 2>&1
sleep 0.4
shot "02-composer-text-send-enabled"

# ── 2. Tooltips on icon-only buttons (hover via xdotool) ──────────────
# Paperclip is the leftmost composer icon; send is rightmost.
DISPLAY=":$display" xdotool mousemove 60 745
sleep 1.0
shot "03-tooltip-attach"

DISPLAY=":$display" xdotool mousemove 1150 745
sleep 1.0
shot "04-tooltip-send"

# ── 3. Keyboard-only traversal ────────────────────────────────────────
mcp boru_gui_focus_composer '{}' >/dev/null 2>&1
sleep 0.4
DISPLAY=":$display" xdotool key Tab
sleep 0.4
shot "05-after-tab"

DISPLAY=":$display" xdotool key shift+Tab
sleep 0.4
shot "06-after-shift-tab"

DISPLAY=":$display" xdotool key Escape
sleep 0.4
shot "07-after-escape"

# ── 4. Status not colour-only: presence label + dot ───────────────────
shot "08-presence-label-and-dot"

# ── 5. New chat dialog (focused input) ────────────────────────────────
DISPLAY=":$display" xdotool key ctrl+n
sleep 0.6
shot "09-new-chat-dialog"

DISPLAY=":$display" xdotool key Escape
sleep 0.4
shot "10-after-escape-dialog"

# ── 6. Home (chat list) ───────────────────────────────────────────────
mcp boru_gui_navigate '{"destination":"chat_list"}' >/dev/null 2>&1
sleep 0.8
shot "11-home-chat-list"

echo "---"
echo "UI-19 screenshots written to $EVIDENCE_DIR"
ls -la "$EVIDENCE_DIR"
