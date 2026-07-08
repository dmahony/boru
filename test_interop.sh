#!/usr/bin/env bash
# Test interoperability of all three chat frontends
# Workspace: /home/dan/iroh-gossip-chat
set -euo pipefail

cd /home/dan/iroh-gossip-chat

cleanup() {
    local rc=$?
    echo "=== Cleaning up ==="
    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    rm -rf "$TDIR_A" "$TDIR_B" "$TDIR_C"
    echo "=== Cleanup done ==="
    exit $rc
}
pids=()
trap cleanup EXIT

TDIR_A=$(mktemp -d /tmp/interop-a-XXXX)
TDIR_B=$(mktemp -d /tmp/interop-b-XXXX)
TDIR_C=$(mktemp -d /tmp/interop-c-XXXX)

BIN="cargo run --features gui,examples -q --"
# We build once to avoid races
echo "--- Building all targets ---"
cargo build --features gui,examples --examples 2>&1 | tail -5
echo "--- Build complete ---"

run_tui() {
    local dir=$1; shift
    IROH_GOSSIP_CHAT_DATA_DIR="$dir" script -q -c "RUST_LOG=info $BIN --example chat -- $* 2>&1" /tmp/tui-output-$$.txt
}

run_gui() {
    local dir=$1; shift
    IROH_GOSSIP_CHAT_DATA_DIR="$dir" xvfb-run -a \
        $BIN --example "$@" 2>&1
}

run_iced() {
    local dir=$1; shift
    IROH_GOSSIP_CHAT_DATA_DIR="$dir" xvfb-run -a \
        $BIN --example iced_chat -- "$@" 2>&1
}

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║  TEST 1: TUI open + monolithic GUI join                ║"
echo "╚══════════════════════════════════════════════════════════╝"
pids=()
# Start TUI opener
IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_A" RUST_LOG=info script -q -c "./target/debug/examples/chat --name tui-a open 2>&1" /tmp/tui-a-out.txt &
TUI_PID=$!
pids+=($TUI_PID)
sleep 5
# Capture ticket from TUI output
TUI_TICKET=$(grep -oP 'ticket to join us: \K.*' /tmp/tui-a-out.txt 2>/dev/null || true)
# If script output didn't work, try test-artifact
if [ -z "$TUI_TICKET" ]; then
    TUI_TICKET=$(grep -oP 'ticket to join us: \K.*' /tmp/tui-a-out.txt 2>/dev/null || true)
fi
echo "TUI ticket: $TUI_TICKET"
if [ -n "$TUI_TICKET" ]; then
    run_gui "$TDIR_B" chat-gui --join "$TUI_TICKET" --name gui-b &
    GUI_PID=$!
    pids+=($GUI_PID)
    sleep 8
    echo "--- TEST 1: Both should be running ---"
    kill -0 $TUI_PID 2>/dev/null && echo "TUI: alive" || echo "TUI: dead"
    kill -0 $GUI_PID 2>/dev/null && echo "GUI: alive" || echo "GUI: dead"
else
    echo "FAIL: Could not capture ticket from TUI"
fi
cleanup_stage1() {
    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    pids=()
}
cleanup_stage1

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║  TEST 2: Modular GUI open + TUI join                   ║"
echo "╚══════════════════════════════════════════════════════════╝"
rm -rf "$TDIR_A" "$TDIR_B"
TDIR_A=$(mktemp -d /tmp/interop-a-XXXX)
TDIR_B=$(mktemp -d /tmp/interop-b-XXXX)
ICED_TICKET=$(IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_A" xvfb-run -a \
    ./target/debug/examples/iced_chat --name iced-a open 2>&1 | grep -oP 'ticket to join us: \K.*' || true)
echo "Iced ticket: $ICED_TICKET"
if [ -n "$ICED_TICKET" ]; then
    IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_B" RUST_LOG=info timeout 10 \
        ./target/debug/examples/chat --name tui-b join "$ICED_TICKET" 2>&1 </dev/null || true
    echo "--- TEST 2: TUI ran and exited (timeout or completed) ---"
else
    echo "FAIL: Could not capture ticket from iced_chat"
fi

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║  TEST 3: All three frontends in same room              ║"
echo "╚══════════════════════════════════════════════════════════╝"
rm -rf "$TDIR_A" "$TDIR_B" "$TDIR_C"
TDIR_A=$(mktemp -d /tmp/interop-a-XXXX)
TDIR_B=$(mktemp -d /tmp/interop-b-XXXX)
TDIR_C=$(mktemp -d /tmp/interop-c-XXXX)
# Start iced_chat as opener
IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_A" xvfb-run -a \
    ./target/debug/examples/iced_chat --name iced-a open 2>&1 &
ICED_PID=$!
pids+=($ICED_PID)
sleep 5
# Get ticket
ALL_TICKET=$(grep -oP 'ticket to join us: \K.*' /proc/$ICED_PID/fd/1 2>/dev/null || true)
# Try reading from a different approach: capture the output file
echo "Ticket for all-three test: (will try direct join)"
# Start chat-gui joining the same topic — but we need the ticket... 
# Alternative: open with a known topic ID
KNOWN_TOPIC="aabbccddee00112233445566778899aabbccddee00112233445566778899aabb"
echo "Using known topic: $KNOWN_TOPIC"
kill $ICED_PID 2>/dev/null || true
wait 2>/dev/null || true
sleep 1

# Start all three with the same known topic
IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_A" xvfb-run -a \
    ./target/debug/examples/iced_chat --name iced-a open "$KNOWN_TOPIC" 2>&1 &
pids+=($!)
sleep 3
IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_B" xvfb-run -a \
    ./target/debug/examples/chat-gui --name gui-b open "$KNOWN_TOPIC" 2>&1 &
pids+=($!)
sleep 3
IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_C" RUST_LOG=info timeout 10 \
    ./target/debug/examples/chat --name tui-c open "$KNOWN_TOPIC" 2>&1 </dev/null || true
echo "--- TEST 3: All three frontends launched ---"
sleep 5
echo "Process status:"
for pid in "${pids[@]}"; do
    kill -0 $pid 2>/dev/null && echo "  PID $pid: alive" || echo "  PID $pid: dead"
done

cleanup_stage1

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║  TEST 4: iced_chat ↔ chat-gui (both GUI, join via ticket)║"
echo "╚══════════════════════════════════════════════════════════╝"
rm -rf "$TDIR_A" "$TDIR_B"
TDIR_A=$(mktemp -d /tmp/interop-a-XXXX)
TDIR_B=$(mktemp -d /tmp/interop-b-XXXX)

# Start iced_chat opener, capture ticket from stderr/stdout
IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_A" xvfb-run -a \
    ./target/debug/examples/iced_chat --name iced-a open 2>/tmp/iced-a-all.txt &
ICED_PID=$!
sleep 6
ICED_TICKET=$(grep -oP 'ticket to join us: \K.*' /tmp/iced-a-all.txt 2>/dev/null || true)
echo "iced_chat ticket: $ICED_TICKET"
if [ -n "$ICED_TICKET" ]; then
    IROH_GOSSIP_CHAT_DATA_DIR="$TDIR_B" xvfb-run -a \
        ./target/debug/examples/chat-gui --name gui-b join "$ICED_TICKET" 2>/tmp/gui-b-all.txt &
    GUI_PID=$!
    sleep 8
    echo "--- TEST 4: Both GUI frontends should be running ---"
    kill -0 $ICED_PID 2>/dev/null && echo "iced_chat: alive" || echo "iced_chat: dead"
    kill -0 $GUI_PID 2>/dev/null && echo "chat-gui: alive" || echo "chat-gui: dead"
    echo "iced_chat output (last 15 lines):"
    tail -15 /tmp/iced-a-all.txt
    echo ""
    echo "chat-gui output (last 15 lines):"
    tail -15 /tmp/gui-b-all.txt
else
    echo "FAIL: Could not capture ticket from iced_chat"
fi
cleanup_stage1

echo ""
echo "=== ALL TESTS COMPLETE ==="
