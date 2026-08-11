#!/usr/bin/env bash
# UI-15 evidence capture (t_929dfe1a).
#
# Launches the real Boru GUI under Xvfb, opens the seeded conversation through
# the test-action MCP endpoint, then captures the composer in four states:
#   empty, typed (green circular send), attachment-hover (paperclip hover),
#   and sending (held by a slow link-preview fetch so the transient state is
#   visible).  Mirrors scripts/ui14_states_evidence.sh.
set -euo pipefail

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BINARY=${BORU_BINARY:-$ROOT_DIR/target/debug/boru}
MCP_CLIENT=$ROOT_DIR/scripts/ui_mcp.py
FIXTURE=$ROOT_DIR/scripts/figure4_fixture.py
SPEC=${BORU_SPEC:-$ROOT_DIR/docs/ui-redesign/evidence/ui-14/ui14-states-spec.json}
EVIDENCE_DIR=$ROOT_DIR/docs/ui-redesign/evidence/ui-15
WIDTH=${BORU_WIDTH:-1280}
HEIGHT=${BORU_HEIGHT:-800}
SLOW_PORT=18731

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
[[ -x "$BINARY" ]] || fail "GUI binary not found: $BINARY (build with: cargo build --features gui --bin boru)"
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
http_pid=""
cleanup() {
    set +e
    [[ -n "$http_pid" ]] && kill "$http_pid" 2>/dev/null || true
    [[ -n "$app_pid" ]] && kill "$app_pid" 2>/dev/null || true
    [[ -n "$xvfb_pid" ]] && kill "$xvfb_pid" 2>/dev/null || true
    [[ -n "$app_pid" ]] && wait "$app_pid" 2>/dev/null || true
    [[ -n "$xvfb_pid" ]] && wait "$xvfb_pid" 2>/dev/null || true
    [[ -n "$data_dir" ]] && python3 "$FIXTURE" cleanup "$data_dir" >/dev/null 2>&1 || rm -rf "$data_dir"
}
trap cleanup EXIT

for candidate in $(seq 260 299); do
    if [[ ! -e "/tmp/.X11-unix/X${candidate}" && ! -e "/tmp/.X${candidate}-lock" ]]; then
        display=$candidate
        break
    fi
done
[[ -n "$display" ]] || fail "no free X display in 260..299"

data_dir=$(mktemp -d "${TMPDIR:-/tmp}/boru-ui15.XXXXXX")
python3 "$FIXTURE" inject "$data_dir" --spec "$SPEC" --now-ms "$(date +%s%3N)" >/dev/null

mcp_port=$((19000 + display))
Xvfb ":$display" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp >/tmp/boru-ui15-xvfb.log 2>&1 &
xvfb_pid=$!
sleep 0.5
kill -0 "$xvfb_pid" 2>/dev/null || fail "Xvfb failed to start"

DISPLAY=":$display" "$BINARY" \
    --data-dir "$data_dir" --no-dht --no-relay --name "UI-15 Composer" \
    --mcp --enable-gui-test-actions --mcp-bind "127.0.0.1:$mcp_port" open \
    >/tmp/boru-ui15-app.log 2>&1 &
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

window_id=$(DISPLAY=":$display" xdotool search --sync --onlyvisible --name '^Boru' | head -n 1)
[[ -n "$window_id" ]] || fail "Boru window not found"
DISPLAY=":$display" xdotool windowsize "$window_id" "$WIDTH" "$HEIGHT"
sleep 0.8

settle_prev=""
settled=0
for settle_attempt in $(seq 1 30); do
    DISPLAY=":$display" import -window "$window_id" /tmp/boru-ui15-settle.png 2>/dev/null || true
    if [[ -n "$settle_prev" ]] && cmp -s /tmp/boru-ui15-settle.png "$settle_prev"; then
        settled=1
        break
    fi
    cp /tmp/boru-ui15-settle.png "$settle_prev" 2>/dev/null || true
    sleep 0.5
done
[[ "$settled" == "1" ]] && echo "timeline settled" || echo "timeline did not fully settle; capturing anyway"

# 1) Empty composer state.
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_clear_composer '{}' >/dev/null || true
sleep 0.4
DISPLAY=":$display" import -window "$window_id" "$EVIDENCE_DIR/ui15_empty_${WIDTH}x${HEIGHT}.png"
echo "captured empty"

# 2) Typed state — green circular send button active.
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_set_composer \
    '{"text":"A modern message composer"}' >/dev/null
sleep 0.4
DISPLAY=":$display" import -window "$window_id" "$EVIDENCE_DIR/ui15_typed_${WIDTH}x${HEIGHT}.png"
echo "captured typed"

# 3) Attachment-hover state — mouse over the paperclip button (leading edge of
#    the composer in the chat panel; ~x=348, ~38px up from the window bottom).
attach_x=348
attach_y=$((HEIGHT - 38))
DISPLAY=":$display" xdotool mousemove "$attach_x" "$attach_y"
sleep 0.5
DISPLAY=":$display" import -window "$window_id" "$EVIDENCE_DIR/ui15_attach_hover_${WIDTH}x${HEIGHT}.png"
echo "captured attachment-hover"

# 4) Sending state — submit a message containing a URL served by a slow local
#    HTTP endpoint.  The link-preview fetch keeps the broadcast task in flight
#    (and therefore composer_sending=true) for the ~1.6s the preview takes,
#    giving a stable capture window for the transient sending state.
#
#    The send is triggered with the REAL keyboard path (focus the composer,
#    then press Enter) rather than the MCP `boru_gui_submit_composer` action:
#    headlessly there are no neighbors, so `sender_ready` stays false and the
#    GUI-test validation rejects SubmitComposer (RoomInactive) before the send
#    task can start.  The keyboard path also directly demonstrates the
#    preserved Enter-to-send shortcut.
printf '#!/usr/bin/env python3\nimport http.server, time\nclass H(http.server.BaseHTTPRequestHandler):\n    def do_GET(self):\n        time.sleep(1.6)\n        body=b"<html><head><title>Boru UI-15</title></head><body>ok</body></html>"\n        self.send_response(200); self.send_header("Content-Type","text/html"); self.send_header("Content-Length",str(len(body))); self.end_headers(); self.wfile.write(body)\n    def log_message(self,*a): pass\nhttp.server.HTTPServer(("127.0.0.1", 18731), H).serve_forever()\n' > /tmp/boru-ui15-slow-server.py
python3 /tmp/boru-ui15-slow-server.py >/tmp/boru-ui15-http.log 2>&1 &
http_pid=$!
sleep 0.3

DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_set_composer \
    '{"text":"Check http://127.0.0.1:18731/ soon"}' >/dev/null
sleep 0.3
DISPLAY=":$display" python3 "$MCP_CLIENT" "$mcp_port" boru_gui_focus_composer '{}' >/dev/null
sleep 0.3
DISPLAY=":$display" xdotool windowfocus "$window_id" 2>/dev/null || true
DISPLAY=":$display" xdotool key Return
# The preview fetch takes ~1.6s; capture ~0.5s after submit while the flag is
# still set.
sleep 0.5
DISPLAY=":$display" import -window "$window_id" "$EVIDENCE_DIR/ui15_sending_${WIDTH}x${HEIGHT}.png"
echo "captured sending"
sleep 2.5

echo "ALL DONE"
file "$EVIDENCE_DIR"/ui15_*_"${WIDTH}x${HEIGHT}".png
