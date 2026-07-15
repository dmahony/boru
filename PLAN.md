# CARD 13 — Dynamic peer join API

## Changes

### 1. `src/api.rs`
- Add `JoinSummary` struct (attempted, joined, skipped_self, skipped_duplicate, errors)
- Store `our_endpoint_id` in `GossipApi` → passed to `GossipTopic` → `GossipSender`
- `GossipSender::join_peers` accepts `impl IntoIterator<Item = EndpointId>`, does self-filter + batch-dedup, returns `Result<JoinSummary, ApiError>`

### 2. `src/net.rs`
- `Builder::spawn` passes `endpoint.id()` to `GossipApi::local()`
- Actor `handle_command` adds structured tracing for `Command::JoinPeers`
- Actor filters already-neighbor peers silently with trace

### 3. `src/chat_core.rs`
- Minor: `spawn_discovery_forwarder` call still works (Result<JoinSummary, ApiError>)

### 4. Tests
- Unit test in `api.rs` for join_peers self-filter + dedup
- Integration test for late peer join via join_peers
