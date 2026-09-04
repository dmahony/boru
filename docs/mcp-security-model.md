# Boru MCP security model

This catalogue is the security contract for the MCP diagnostic server. MCP is a
local operator interface by default; remote binding is an explicit deployment
choice and must not be used as an implicit authentication mechanism.

## Transport and capability policy

| Capability | Methods | Transport | Rate / resource policy | Sensitivity and redaction |
|---|---|---|---|---|
| Health | `boru_ping`, `boru_get_node_status` | local TCP or loopback TCP | bounded frame, 32 connections, 256 requests/connection, 32 requests/s | low sensitivity; identifiers are short/opaque and relay URLs are coarse |
| Network observation | `boru_get_room_status`, `boru_get_peer_status`, `boru_get_discovery_events`, `boru_get_failure_analysis`, `boru_wait_for_peer`, `boru_run_discovery_test` | local/loopback by default | every wait has a deadline; list limits are clamped by handlers | metadata only; no message bodies, credentials, raw payloads, or unbounded errors |
| Probe diagnostics | `boru_send_probe`, `boru_find_received_probe` | local/loopback; gossip is the only remote side effect | payload and identifiers are bounded; operation deadline applies | probe text is bounded and never executed; logs contain lengths/IDs only |
| Directory | `boru_list_public_rooms`, `boru_create_public_room`, `boru_delete_public_room`, `boru_join_lobby_room` | local/loopback; GUI gate for join | bounded names/descriptions and action queue | tickets and room metadata are returned only to the authenticated local operator; never log ticket contents |
| GUI observation | `boru_get_gui_snapshot`, `boru_get_iced_state`, `boru_get_iced_message_journal`, `boru_gui_get_action_status`, `boru_gui_wait_for_state` | explicit GUI-test mode, loopback only | bounded journal/snapshot output and deadlines | central GUI snapshot redaction excludes composer contents, clipboard, paths, and secrets |
| GUI mutation | `boru_send_gui_action`, `boru_gui_*` mutation methods, `boru_run_gui_message_test` | explicit GUI-test mode, loopback only | shared action rate limiter and bounded queue; invalid commands fail closed | command fields are typed/allow-listed; responses contain action IDs/status, not payloads |
| File/catalogue | `boru_browse_peer_catalogue`, `boru_download_file`, `boru_get_download_status`, `boru_add_peer_address`, `boru_get_local_shared_files`, `boru_gui_test_share_file`, `boru_grant_file_read_access` | local/loopback; peer operations require normal Iroh authentication | bounded hashes, paths, catalogue rows, downloads, and request deadlines | paths and hashes are validated; file content is never returned by diagnostics; errors are sanitized |
| Message delivery | `boru_get_message_store`, `boru_get_outbox_status`, `boru_get_delivery_telemetry`, `boru_wait_for_message_delivery` | local/loopback | query limits are clamped; waits are deadline-bound | delivery state and opaque IDs only; message text and signed/encrypted bytes are excluded |

## Framing and exhaustion controls

Requests are newline-delimited JSON-RPC 2.0 frames. The reader enforces a
128 KiB frame ceiling before deserialisation and rejects malformed input without
continuing a poisoned stream. A connection is closed after a parse failure,
oversized frame, request burst, or request-count limit. At most 32 connections
are active; each handler is deadline-bound to 30 seconds. These limits are
intentionally deterministic so callers receive a structured error or a closed
connection rather than an indefinitely growing buffer or task.

## Authentication and deployment rules

- The default bind address is `127.0.0.1`; no unauthenticated network listener
  is enabled by default.
- GUI-test capabilities additionally require `--enable-gui-test-actions` and a
  loopback bind. A non-loopback GUI configuration is rejected at startup.
- Any future remote mode must use an encrypted authenticated Iroh channel,
  credentials from a secret backend (never command-line logs or JSON), explicit
  capability scopes, credential rotation, and revocation. It must not reuse
  the unauthenticated TCP framing path.
- Privileged operations are auditable by method and outcome only. Audit records
  must use redaction helpers and must never contain secrets, message bodies,
  file bytes, clipboard content, raw addresses, or complete command JSON.

## Registry completeness

When adding a method, update all four surfaces together: this catalogue, the
module-level tool table, the dispatch match, and the bridge/tool definition.
Add a focused dispatch test covering the method's gate and malformed input.
