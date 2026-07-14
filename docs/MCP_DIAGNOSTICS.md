# MCP Diagnostic Server

The MCP (Model Context Protocol) diagnostic server lets AI agents inspect
boru-chat's internal state — room discovery, peer connectivity, and probe
delivery — over a JSON-RPC 2.0 TCP interface.  It is designed for automated LAN
testing of gossip swarm behaviour without requiring a visual frontend or manual
log inspection.

## Quick start

Build the iced_chat GUI (which includes the MCP server):

```sh
cargo build --features gui --example iced_chat --release
```

Start with MCP enabled on the default address:

```sh
cargo run --features gui --example iced_chat --release -- --mcp
```

The server binds to `127.0.0.1:8765` by default.  To use a custom address:

```sh
cargo run --features gui --example iced_chat --release -- --mcp --mcp-bind 0.0.0.0:9876
```

## CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--mcp` | (disabled) | Enable the MCP diagnostic server |
| `--mcp-bind <ADDR>` | `127.0.0.1:8765` | Bind address for the MCP server |

## Protocol

The server speaks **newline-delimited JSON-RPC 2.0** over raw TCP.

Each request is a single JSON object followed by `\n`.  Each response is a
single JSON object followed by `\n`.  The connection is persistent — send
multiple requests over the same socket.

### Request format

```json
{"jsonrpc":"2.0","method":"boru_get_node_status","params":{},"id":1}
```

### Response format (success)

```json
{"jsonrpc":"2.0","id":1,"result":{...}}
```

### Response format (error)

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Internal error","data":"..."}}
```

### Testing with a raw TCP connection

```sh
echo '{"jsonrpc":"2.0","method":"boru_get_node_status","params":{},"id":1}' \
  | nc -w 3 127.0.0.1 8765
```

## Tools

### `boru_get_node_status`

Fetch the local node's identity and status.

*Parameters:* none

*Response:*
```json
{
  "node_id": "f1a2b3c4...",
  "node_id_short": "f1a2b3…",
  "version": "0.101.0",
  "active_room_count": 1,
  "latest_event_sequence": 42,
  "relay_url": null
}
```

---

### `boru_get_room_status`

Fetch room membership and peer summary for a given room.

*Parameters:*

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `room_id` | string | **yes** | 64-hex-char topic ID |

*Response:*
```json
{
  "node_id": "f1a2b3c4...",
  "room_id": "abcd...",
  "joined": true,
  "subscribed": true,
  "peer_count": 2,
  "peers": [
    {
      "peer_id": "peer1...",
      "discovery_sources": ["mdns", "mainline_dht"],
      "addresses": ["192.168.1.5:12345"],
      "connected": true,
      "topic_member": true,
      "last_error": null
    }
  ],
  "discovery_sources_enabled": ["mdns", "mainline_dht", "bootstrap"],
  "last_error": null,
  "local_room_joined": true
}
```

---

### `boru_get_discovery_events`

Return recent diagnostic events, optionally filtered by room and sequence.

*Parameters:*

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `since_sequence` | number | no | `0` | Only return events with sequence > this value |
| `limit` | number | no | `200` | Maximum events to return |
| `room_id` | string | no | (all rooms) | 64-hex-char topic ID to filter by |

*Response:*
```json
{
  "events": [
    {
      "sequence": 1,
      "timestamp": "2026-07-14T19:00:00Z",
      "room_id": "abcd...",
      "peer_id": "peer1...",
      "kind": {
        "type": "peer_discovered_with_addr",
        "source": "mdns",
        "addresses": ["192.168.1.5:12345"]
      }
    }
  ],
  "latest_sequence": 42,
  "returned_count": 1
}
```

---

### `boru_send_probe`

Broadcast a diagnostic probe through gossip.  The probe is a signed message
that travels through the gossip mesh like any other message, but invisible to
the chat UI.  The response includes the probe ID and message hash for later
verification.

*Parameters:*

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `room_id` | string | **yes** | — | 64-hex-char topic ID to broadcast on |
| `probe_id` | string | no | (auto-generated) | Custom probe identifier |
| `payload` | string | no | (empty) | Arbitrary diagnostic payload text |

*Response:*
```json
{
  "probe_id": "probe_abc123",
  "room_id": "abcd...",
  "sender_id": "f1a2b3c4...",
  "message_hash": "e3b0c442...",
  "sent_at_ms": 1740949200000,
  "broadcast_accepted": true
}
```

---

### `boru_find_received_probe`

Look up a received probe by its probe ID.  The receiving node records probes
that arrive through the gossip mesh with full metadata.

*Parameters:*

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `probe_id` | string | **yes** | The probe identifier to look up |

*Response (found):*
```json
{
  "received": true,
  "probe": {
    "probe_id": "probe_abc123",
    "room_id": "abcd...",
    "sender_id": "f1a2b3c4...",
    "sent_at_ms": 1740949200000,
    "received_at_ms": 1740949200500,
    "latency_ms": 500,
    "message_hash": "e3b0c442...",
    "duplicate_count": 0
  }
}
```

*Response (not found):*
```json
{
  "received": false,
  "probe_id": "probe_abc123"
}
```

---

### `boru_get_peer_status`

Fetch the full diagnostic state for a specific peer.

*Parameters:*

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `peer_id` | string | **yes** | Peer public key (hex) |

*Response:*
```json
{
  "found": true,
  "peer_id": "peer1...",
  "discovered": true,
  "address_lookup_state": "Succeeded",
  "addresses": ["192.168.1.5:12345"],
  "connection_state": "Connected",
  "subscription_state": "Succeeded",
  "topic_member": true,
  "discovery_sources": ["mdns"],
  "last_error": null,
  "first_seen_ms": 1740949200000
}
```

---

### `boru_wait_for_peer`

Wait asynchronously for a peer to reach a target diagnostic state.  Polls the
diagnostics watch channel and returns when the state is satisfied or the
timeout elapses.

*Parameters:*

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `peer_id` | string | **yes** | — | Peer public key (hex) |
| `target_state` | string | **yes** | — | One of: `discovered`, `address_resolved`, `connected`, `subscription_joined`, `topic_member` |
| `timeout_ms` | number | no | `15000` | Maximum wait (clamped to 30s) |

*Response (reached):*
```json
{
  "reached": true,
  "target_state": "connected",
  "timed_out": false,
  "peer": { ... }
}
```

*Response (timeout):*
```json
{
  "reached": false,
  "target_state": "connected",
  "timed_out": true,
  "peer": { ... }
}
```

---

### `boru_run_discovery_test`

Orchestrated end-to-end discovery and probe delivery test against a specific
peer.  This tool automates the full LAN test workflow:

1. Validates that the local room is joined
2. Waits for the peer to progress through all stages (discovered →
   address_resolved → connected → subscription_joined → topic_member)
3. Optionally sends a diagnostic probe and waits briefly for delivery
4. Collects evidence and classifies the outcome with a human-readable summary
5. Returns all relevant events for further analysis

*Parameters:*

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `room_id` | string | **yes** | — | 64-hex-char topic ID |
| `expected_peer_id` | string | **yes** | — | Peer public key (hex) to test |
| `timeout_ms` | number | no | `20000` | Maximum wait (clamped to 30s) |
| `send_probe` | bool | no | `true` | Whether to send a diagnostic probe |
| `probe_payload` | string | no | `"automatic LAN discovery test"` | Payload text for the probe |

*Response (success):*
```json
{
  "success": true,
  "room_id": "abcd...",
  "local_node_id": "f1a2b3c4...",
  "expected_peer_id": "peer1...",
  "failed_stage": null,
  "summary": "All stages completed successfully.",
  "evidence": {
    "local_room_joined": true,
    "peer_discovered": true,
    "address_lookup_observed": true,
    "address_resolved": true,
    "connection_attempted": true,
    "connection_established": true,
    "subscription_started": true,
    "subscription_joined": true,
    "peer_in_topic": true,
    "probe_broadcast": true,
    "probe_received_or_acknowledged": true
  },
  "peer": { ... },
  "event_sequence_start": 10,
  "event_sequence_end": 25,
  "relevant_events": [ ... ],
  "probe": {
    "probe_id": "probe_abc123",
    "broadcast_accepted": true,
    "delivery_confirmed": true,
    "latency_ms": 500
  }
}
```

*Response (failure — peer never discovered):*
```json
{
  "success": false,
  "failed_stage": "Discovery",
  "summary": "Expected peer was never discovered.",
  "evidence": { ... },
  "relevant_events": [ ... ]
}
```

## Security considerations

- **Loopback by default.**  The server binds to `127.0.0.1:8765`.  Only
  processes on the same machine can connect.

- **No secrets exposed.**  The MCP tools expose diagnostic metadata (peer IDs,
  connection states, event counts, room IDs, probe delivery status).  Private
  keys, mailbox tokens, room tickets, and message contents are never returned.

- **Probe payloads are inert text.**  Probe payload strings are diagnostic
  labels — never executed or interpreted as commands.

- **Binding to non-loopback addresses.**  If you set `--mcp-bind 0.0.0.0:8765`
  the server warns loudly and becomes accessible to all machines on the
  network.  Only do this on trusted LANs where all hosts are under your
  control.

- **No authentication.**  There is no authentication layer on the TCP socket.
  Any process that can reach the bind address can call all tools.

## Limitations

- **One-shot state.**  Tools return point-in-time snapshots.  Watching for
  state transitions requires polling or the dedicated `boru_wait_for_peer`
  tool.

- **No chat message injection.**  The MCP server can *read* diagnostic state
  and *send* diagnostic probes, but it cannot send chat messages or impersonate
  a user in a room.

- **Event capacity.**  The internal event ring buffer holds up to 5 000 events
  (oldest dropped first).  Probe storage holds up to 1 000 probes.  These
  limits are hard-coded at compile time.

- **Latency accuracy.**  Latency is computed as `received_at_ms - sent_at_ms`,
  which is only accurate when both nodes' system clocks are roughly
  synchronised.  If `received_at_ms < sent_at_ms`, latency is reported as
  `null`.

- **No persistence.**  Events and probes exist only in memory and are lost on
  process restart.

## LAN testing workflow

This section describes the manual step-by-step verification that an AI agent
can perform to confirm peer discovery and probe delivery across two LAN nodes.

### Prerequisites

- Two machines on the same LAN (Node A and Node B).
- Both machines have the boru-chat iced_chat binary built with the `gui` feature.
- Both machines share a valid room ticket for the same topic.

### 1. Start both nodes with MCP enabled

**Node A:**
```sh
cargo run --features gui --example iced_chat --release -- --mcp --mcp-bind 127.0.0.1:8765
```

**Node B:**
```sh
cargo run --features gui --example iced_chat --release -- --mcp --mcp-bind 127.0.0.1:8766
```

Both nodes should now have an MCP server listening on their respective ports
(both on loopback, since MCP connects are local to each machine).

### 2. Configure both MCP connections in the agent

In the AI agent's MCP configuration (for example, `config.yaml` in Hermes),
add two TCP-based MCP servers:

```yaml
# config.yaml (Hermes agent)
mcp_servers:
  chat_node_a:
    transport: tcp
    url: "127.0.0.1:8765"
  chat_node_b:
    transport: tcp
    url: "127.0.0.1:8766"
```

When the two nodes are on different machines, replace `127.0.0.1` with the
respective LAN IP addresses (and bind the MCP servers to `0.0.0.0` on each
node).  Remember that binding to non-loopback addresses exposes the diagnostic
tools to the LAN.

### 3. Agent workflow

The AI agent follows these steps to verify end-to-end discovery and delivery:

#### 3.1. Check node status on both hosts

Call `boru_get_node_status` on Node A and Node B.  Record each node's
`node_id` and `latest_event_sequence`.

#### 3.2. Get room status on both hosts

Call `boru_get_room_status({"room_id": "<hex-topic-id>"})` on both nodes.
Confirm that both report `"joined": true` and that the room IDs match.

#### 3.3. Confirm each node sees the other peer

On each node, call `boru_get_room_status({"room_id": "<hex-topic-id>"})` and
inspect the `peers` array.  Node A's peer list should contain Node B's node_id,
and vice versa.  Each peer entry should show `"connected": true` and
`"topic_member": true`.

If peers are not yet visible, use `boru_wait_for_peer`:

```json
{"method":"boru_wait_for_peer","params":{"peer_id":"<peer-id>","target_state":"topic_member","timeout_ms":15000}}
```

#### 3.4. Send a diagnostic probe from Node A

Call `boru_send_probe({"room_id": "<hex-topic-id>"})` on Node A.  Save the
returned `probe_id` and `message_hash`.

#### 3.5. Verify probe delivery on Node B

Poll `boru_find_received_probe({"probe_id": "<probe-id>"})` on Node B.
Retry every 1–2 seconds for up to 15 seconds.  When the probe arrives,
the response will show `"received": true` with the full probe metadata.

#### 3.6. Compare metadata

Confirm that the `probe_id` and `message_hash` from Node A's `boru_send_probe`
response match the values in Node B's `boru_find_received_probe` response.
Check that `latency_ms` is a reasonable positive value (indicating successful
gossip propagation).

#### 3.7. Fetch discovery events from both nodes

Call `boru_get_discovery_events({"since_sequence": 0})` on both nodes.
Inspect the events for `PeerDiscoveredWithAddr`, `PeerJoinedRoom`,
`MessageReceived`, and `ProbeBroadcast`/`ProbeReceived` entries.

#### 3.8. Report

Combine the observations into a pass/fail verdict:

- **Pass:** Both nodes see each other as connected topic members, the probe
  was sent and received, probe ID and message hash match, and discovery events
  are present on both sides.
- **Fail:** Identify which stage failed (discovery, connection, subscription,
  or probe delivery) using the event logs.

### 4. Automated test with `boru_run_discovery_test`

The single-call orchestrated test automates steps 3.1–3.8 above:

```json
{
  "method": "boru_run_discovery_test",
  "params": {
    "room_id": "<hex-topic-id>",
    "expected_peer_id": "<node-b-peer-id>",
    "timeout_ms": 20000,
    "send_probe": true
  }
}
```

Call this on Node A with Node B's peer ID.  The tool waits for discovery
progression, sends a probe, checks for delivery, and returns a structured
result with `success: true/false`, a `failed_stage` if applicable, a
`summary`, and the full `evidence` and `relevant_events` for debugging.

## Error codes

| Code | Message | Description |
|------|---------|-------------|
| `-32700` | Parse error | Invalid JSON |
| `-32601` | Method not found | Unknown tool name |
| `-32000` | Internal error | Tool execution failed (see `data` for details) |

## Example sessions

### Successful two-node LAN verification

```
→ Node A: boru_get_node_status
← {"node_id":"A_abc...","active_room_count":1,...}

→ Node B: boru_get_node_status
← {"node_id":"B_def...","active_room_count":1,...}

→ Node A: boru_get_room_status({"room_id":"<topic>"})
← {"peers":[{"peer_id":"B_def...","connected":true,"topic_member":true}],...}

→ Node B: boru_get_room_status({"room_id":"<topic>"})
← {"peers":[{"peer_id":"A_abc...","connected":true,"topic_member":true}],...}

→ Node A: boru_send_probe({"room_id":"<topic>"})
← {"probe_id":"probe_x1y2","message_hash":"abc123...","broadcast_accepted":true}

→ Node B: boru_find_received_probe({"probe_id":"probe_x1y2"})
← {"received":true,"probe":{"latency_ms":420,"message_hash":"abc123...",...}}

→ Node A: boru_run_discovery_test({"room_id":"<topic>","expected_peer_id":"B_def..."})
← {"success":true,"summary":"All stages completed successfully.","probe":{"delivery_confirmed":true,"latency_ms":420}}
```

### Failure: peer never discovered

```
→ Node A: boru_get_room_status({"room_id":"<topic>"})
← {"peers":[],...}

→ Node A: boru_wait_for_peer({"peer_id":"B_def...","target_state":"discovered","timeout_ms":10000})
← {"reached":false,"timed_out":true,...}

→ Node A: boru_run_discovery_test({"room_id":"<topic>","expected_peer_id":"B_def..."})
← {"success":false,"failed_stage":"Discovery","summary":"Expected peer was never discovered.","evidence":{"peer_discovered":false,...}}
```
