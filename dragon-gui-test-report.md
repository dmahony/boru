# Second Ubuntu VM GUI test report

Target VM: `dragon` (Ubuntu 24.04.4 LTS, Linux 6.17.1-2-qcom, aarch64)
Date: 2026-07-15

## Scope attempted

The prescribed GUI/MCP workflow was attempted: node status, GUI state, room state, discovery events, then discovery/probe/message delivery tests if a peer was available.

## Environment evidence

- SSH: `dan@dragon` reachable with key authentication.
- GUI process: `/home/dan/boru-test/iced_chat-aarch64-linux --mcp --enable-gui-test-actions --mcp-bind 127.0.0.1:8765 --bind-port 0`, running under `xvfb-run`.
- Xvfb: display `:101`, 1280x1024x24; screenshot captured successfully.
- MCP listener: TCP `0.0.0.0:8765` observed listening on dragon.
- Binary SHA-256: `e9c5c3ace11b57d84a8b52212bbdf03eeef7d8805151dde5eef2c805b382246b`.
- GUI screenshot SHA-256: `519195f1ddc2aea229e1d8c65c88c51d9efef5492e9320ea22da976e4d37e138`.

## Timeline and results

1. SSH and process inspection passed: dragon, aarch64; Xvfb and iced_chat process running; port 8765 listening.
2. Remote MCP `boru_get_node_status` was attempted directly on dragon and through SSH forwarding. TCP connections were accepted, but no JSON-RPC response arrived before the timeout (prior readiness run: 5-second response timeout in both paths).
3. Because node status did not return, remote node ID, version, active room list, diagnostics availability, room ID, and peer IDs could not be obtained via MCP.
4. GUI screenshot succeeded. It shows Boru Chat open in room `9021bd1e...`; header reports `0 direct · 0 relay`; message area says `No messages yet`; no error dialog is visible; composer is present. This proves the GUI rendered and is interactive-looking, but does not prove network delivery.
5. Room status, peer status, discovery events, discovery test, probe send, probe receive, and GUI message test could not be executed on dragon because the MCP endpoint never produced the prerequisite node/room response and no expected peer ID was available.

## Diagnosis

- GUI launch/rendering: PASS (screenshot evidence).
- MCP TCP listener: PASS at transport/listener level.
- MCP JSON-RPC responsiveness: FAIL / timed out.
- Node discovery: Unknown / Not Observed through dragon MCP.
- Address resolution, peer connection, topic membership, probe broadcast, probe delivery, and symmetry: Unknown / Not Observed.
- This is classified as an MCP/application observability blocker, not an application message-delivery failure, because the diagnostic interface did not return structured state.

First blocking stage: MCP diagnostic response (before node-status observation). The distributed networking stages cannot be ranked from the remote perspective.

Confidence: High for the MCP responsiveness blocker (repeatable direct and SSH-forwarded timeouts, while the listener accepts TCP); Low for any claim about discovery or delivery because those stages were not observable.

Reproduction: start the shown command under `xvfb-run`, connect to `dragon:8765` (or SSH-forward it), send newline-delimited JSON-RPC `boru_get_node_status`, and wait 5 seconds; connection is accepted but no response is returned.
