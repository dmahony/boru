# Boru MCP — Agent Instructions

Quick-start reference for using the boru-chat MCP diagnostic tools from within
Hermes. This document is written for AI agents (not humans) — it focuses on
what the tools do, how to invoke them, and the pitfalls that will waste your
time if you don't know about them.

## Architecture

```
Hermes MCP client (stdio)
  └─ boru-mcp-bridge.py (Python MCP → JSON-RPC proxy)
       └─ TCP → app's JSON-RPC diagnostic server (--mcp --mcp-bind IP:PORT)
```

The bridge script lives at `~/.hermes/scripts/boru-mcp-bridge.py`. It defines
27 tools and forwards each call as a JSON-RPC request over TCP to the app.

**Two namespaces** — one per test VM:

| Namespace prefix | Local bridge port | Remote VM      | App MCP port |
|------------------|-------------------|----------------|--------------|
| `mcp__boru_a__`  | 8765              | 172.16.0.54    | 9054         |
| `mcp__boru_b__`  | 8766              | 172.16.0.55    | 9055         |

Both namespaces expose the **exact same set of 27 tools**. The prefix
determines which VM you're querying.

## Prerequisites (all three must be true)

1. **App running with `--mcp`** on the VM. Without it there is no JSON-RPC
   server to connect to.
2. **SSH tunnels up** forwarding the bridge ports to the app's MCP ports:
   ```
   ssh -fNL 8765:127.0.0.1:9054 dan@172.16.0.54
   ssh -fNL 8766:127.0.0.1:9055 dan@172.16.0.55
   ```
3. **Bridge processes alive.** The bridge connects on first tool call. If it
   was killed (e.g. after a deploy), kill stale processes so the watchdog
   restarts them:
   ```
   pkill -f 'boru-mcp-bridge'
   # Wait 2-3s, then verify with boru_ping
   ```

   **After a deploy that stops both apps**, kill all bridges + watchdogs:
   ```
   pkill -f 'mcp_stdio_watchdog.*boru'
   pkill -f 'boru-mcp-bridge'
   sleep 3
   ```

## Quick health check

Always start with these three calls to confirm the bridge and app are alive:

```
mcp__boru_a__boru_ping()
mcp__boru_a__boru_get_node_status()
mcp__boru_b__boru_ping()
mcp__boru_b__boru_get_node_status()
```

`boru_get_node_status` returns `node_id_short`, version, relay URL, and event
count. Use these IDs to cross-reference with `boru_get_peer_status`.

## Tool catalog

### Category 1: Read-only diagnostics (always available — no special flags needed)

These 10 tools work as long as the app was started with `--mcp`.

| Tool | Parameters | Returns |
|------|-----------|---------|
| `boru_ping` | none | `{"status": "ok"}` |
| `boru_get_node_status` | none | node_id, version, relay_url, event_count |
| `boru_get_peer_status` | `peer_id` (hex or base58) | discovery state per peer: discovered, address_resolved, connected, topic_member, etc. |
| `boru_get_room_status` | `room_id` (64-char hex) | subscribed, joined, peers[] with connection state |
| `boru_get_discovery_events` | `since_sequence?`, `limit?`, `room_id?` | recent protocol events (peer_discovered, connection_established, message_received, probe_received) |
| `boru_get_failure_analysis` | none | network_failure, iced_update_failure, etc. (⚠ limited scope — see pitfalls) |
| `boru_send_probe` | `room_id`, `probe_id?`, `payload?` | diagnostic probe sent through gossip (uses separate code path from normal messages) |
| `boru_find_received_probe` | `probe_id` | probe data if received |
| `boru_wait_for_peer` | `peer_id`, `target_state`, `timeout_ms?` | blocks until peer reaches state or timeout |
| `boru_run_discovery_test` | `peer_id`, `room_id?`, `timeout_ms?` | orchestrated discovery test result |

### Category 2: GUI test actions (requires `--enable-gui-test-actions`)

These 17 tools manipulate or inspect the Iced GUI. Without the flag they
return `{"code":-32601, "message":"Method not found", "data":"GUI test actions are not enabled."}`.

| Tool | Parameters | What it does |
|------|-----------|-------------|
| `boru_get_iced_state` | none | full Iced UI state snapshot |
| `boru_get_iced_message_journal` | `filter?`, `limit?` | recent AppMessage processing history |
| `boru_get_gui_snapshot` | none | GUI application state snapshot |
| `boru_join_lobby_room` | `timeout_ms?` | open + join the stable diagnostic lobby room |
| `boru_gui_navigate` | `destination` | navigate to a screen by name |
| `boru_gui_open_room` | `room_id` | open a room (alphanumeric, 1-128 chars) |
| `boru_gui_open_conversation` | `conversation_id` | open direct conversation with peer (64-hex key) |
| `boru_gui_set_composer` | `text` | set text in message input field |
| `boru_gui_clear_composer` | none | clear message input field |
| `boru_gui_focus_composer` | none | focus the message input field |
| `boru_gui_submit_composer` | none | submit composer text through normal send path |
| `boru_gui_toggle_dark_mode` | `enabled` (bool) | toggle dark/light mode |
| `boru_gui_close_dialog` | none | close current dialog or overlay |
| `boru_send_gui_action` | `command` (JSON), `idempotency_key?` | send arbitrary GUI command |
| `boru_gui_get_action_status` | `action_id` | status of a prior GUI action by idempotency key |
| `boru_gui_wait_for_state` | `condition` (JSON), `timeout_ms?` | wait for a GUI state condition |
| `boru_run_gui_message_test` | `room_id`, `message_text`, `expected_peer_id`, `timeout_ms?` | test local GUI message pipeline end-to-end |

## Quick-start recipes

### Verify both nodes are alive and connected

```
1. mcp__boru_a__boru_get_node_status()   → note node_id_short for VM-A
2. mcp__boru_b__boru_get_node_status()   → note node_id_short for VM-B
3. mcp__boru_a__boru_get_peer_status(peer_id=<vm-b-id>)
4. mcp__boru_b__boru_get_peer_status(peer_id=<vm-a-id>)
```

Both should show `discovered: true`, `connected: true`, `topic_member: true`
(or at least `address_resolved: true` if they haven't joined a room yet).

### Check room state

```
mcp__boru_a__boru_get_room_status(room_id="<64-char-hex>")
```

Returns `subscribed`, `joined`, and `peers[]`. Each peer has `connected`,
`topic_member`, `node_id`.

### Send a message through the GUI (needs --enable-gui-test-actions)

```
1. mcp__boru_a__boru_gui_open_room(room_id="lobby")
2. mcp__boru_a__boru_gui_set_composer(text="hello from MCP")
3. mcp__boru_a__boru_gui_submit_composer()
```

Or use the higher-level test:
```
mcp__boru_a__boru_run_gui_message_test(
  room_id="lobby",
  message_text="hello",
  expected_peer_id="<vm-b-peer-id>"
)
```

### Debug one-way discovery

When VM-A sees VM-B but not vice versa:
```
# Check both directions
mcp__boru_a__boru_get_discovery_events()
mcp__boru_b__boru_get_discovery_events()

# Send a probe from the "blind" side to trigger address exchange
mcp__boru_b__boru_send_probe(room_id="<lobby-topic>", payload="discovery-trigger")

# Wait 10s, then check if peer is now visible
mcp__boru_b__boru_get_peer_status(peer_id="<vm-a-id>")
```

## Critical pitfalls

### 1. `boru_get_room_status` returns "Room not found" for the lobby

The lobby subscription runs at the gossip protocol layer and is NOT
registered in the app-level room-status map. `boru_get_room_status` only
knows about rooms joined through `OpenRoom`. To verify lobby connectivity,
check the app log instead:
```
grep "subscribed to lobby topic" <data_dir>/logs/boru.log
```

### 2. `boru_get_discovery_events` returns 0 events despite active mDNS

mDNS discovery events (peer seen, advertisement expired) are NOT recorded
in this buffer. Only gossip-level protocol events appear. Empty events does
NOT mean mDNS is broken. Verify mDNS separately:
```
grep -c "join_peers succeeded" <data_dir>/logs/boru.log
```

### 3. `boru_get_failure_analysis` reports all-clear despite broken gossip

This tool tracks explicit app-level failures (iced UI update failures,
state-update failures). Gossip-level dial timeouts, failed QUIC handshakes,
and relay fallback failures are handled silently and do NOT set these flags.
"All false" does not mean the gossip layer is healthy.

### 4. Probes ≠ Messages

`boru_send_probe` uses a **separate diagnostic code path** from normal
gossip messages. Successful probe delivery (28ms bidirectional!) does NOT
mean regular chat messages will flow. Probes create a fresh
`Gossip::subscribe` + `broadcast` per call; persistent room subscriptions
may have broken discovery even when probes work.

### 5. `boru_wait_for_peer` timeout is silent

When `boru_wait_for_peer(target_state="connected")` times out, it produces
no diagnostic event on either VM. Neither `boru_get_discovery_events` nor
`boru_get_failure_analysis` will show the attempt.

### 6. GUI tools return "Method not found" without the flag

All 17 GUI tools require `--enable-gui-test-actions` on the app. Without it
they return error code -32601. The read-only diagnostic tools still work.

### 7. `boru_gui_submit_composer` rejected with "room_inactive"

This means the conversation's `GossipSender` is `None` — the gossip
subscription completed at the protocol level, but `RoomOpened` never fired
because `gossip.subscribe()` inside a `runtime_handle.spawn()` hung.
The iced `Task::perform` future never completed.

### 8. MCP bridge "Connection refused" / stale after deploy

After stopping and restarting the app on a VM, the bridge process holds a
stale TCP connection. Kill it:
```
pkill -f 'boru-mcp-bridge.py.*:8766'   # for VM-B
```
The watchdog auto-restarts within 1-2 seconds.

### 9. SSH tunnel port mismatch

If the tunnel forwards to the wrong port (e.g. `-L 8765:127.0.0.1:8765`
instead of `-L 8765:127.0.0.1:9054`), the bridge connects but the app
never responds. Verify:
```
ssh dan@172.16.0.54 "ss -tlnp | grep boru"   # shows actual MCP port
ss -tlnp | grep 8765                          # shows tunnel listener
```

### 10. No MCP tool exposes individual ChatEntry delivery state

There is no tool to check whether a specific message reached
Queued→Sent→Delivered→Seen. To check delivery state, you must either:
- Look at the GUI via VNC (label icons: 🔄/✓/✓✓/👁)
- Check `boru_get_iced_message_journal()` for NetEvent entries
- Add debug logging to `process_net_event_sync`

### 11. GUI freeze from file dialog (ashpd / XDG Desktop Portal)

The GUI may become unresponsive after a file share/open dialog. The process
stays alive (state S), MCP read-only tools still respond, but the GUI thread
blocks on a D-Bus portal race. No code fix; restart the app.

## Cross-VM verification workflow (most common pattern)

```
# 1. Confirm both apps are reachable
mcp__boru_a__boru_ping()
mcp__boru_b__boru_ping()

# 2. Get identities
mcp__boru_a__boru_get_node_status()    → id_a, relay_a
mcp__boru_b__boru_get_node_status()    → id_b, relay_b

# 3. Cross-check peer visibility
mcp__boru_a__boru_get_peer_status(peer_id=<id_b>)
mcp__boru_b__boru_get_peer_status(peer_id=<id_a>)

# 4. If both sides see each other, check room state
mcp__boru_a__boru_get_room_status(room_id="<topic-hex>")
mcp__boru_b__boru_get_room_status(room_id="<topic-hex>")
# Both should show peer_count >= 1 and peers[].connected == true

# 5. For detailed timeline
mcp__boru_a__boru_get_discovery_events()
mcp__boru_b__boru_get_discovery_events()

# 6. If discovery is one-way, send a probe from the blind side
mcp__boru_b__boru_send_probe(room_id="<topic-hex>")
```

## When to use MCP vs log inspection

| Question | Use MCP | Use logs |
|----------|---------|----------|
| Is the app running? | `boru_ping` | — |
| What's the node ID? | `boru_get_node_status` | — |
| Does peer X see peer Y? | `boru_get_peer_status` | — |
| Is room R active? | `boru_get_room_status` | — |
| What discovery events happened? | `boru_get_discovery_events` | — |
| Is the lobby subscribed? | — | `grep "subscribed to lobby"` |
| Is mDNS working? | — | `grep "join_peers succeeded"` |
| Did a message get delivered? | — | VNC or debug logging |
| What crashed? | — | `grep "ERROR\|panic" boru.log` |
| Is the GUI frozen? | `boru_get_iced_state` (needs flag) | `cat /proc/<pid>/wchan` |
