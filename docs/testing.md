# Testing Infrastructure

boru-chat has a comprehensive test suite spanning unit tests, integration tests,
deterministic simulation, and GUI tests.

## Test Layout

```
tests/
├── sim.rs                              # Deterministic protocol simulation
├── room_e2e.rs                         # End-to-end room messaging
├── test_three_peer_mesh.rs             # Three-peer mesh connectivity
├── test_message_lifecycle.rs           # Full message send/receive/ack lifecycle
├── test_message_transfer.rs            # Message transfer between two peers
├── test_storage_integration.rs         # SQLite storage layer tests
├── test_file_library_integration.rs    # File library operations tests
├── test_conversation_integration.rs    # Conversation management tests
├── test_mcp_diagnostics_integration.rs # MCP diagnostic server tests
├── test_friend_request_e2e.rs          # Friend request end-to-end flow
├── test_friend_ticket_persistence.rs   # Friend ticket persistence
├── test_public_lobby_integration.rs    # Public lobby discovery tests
├── test_private_room_dht_discovery.rs  # Private room DHT tests
├── test_private_room_invitation_discovery.rs
├── test_performance_baseline.rs        # Performance baseline measurement
├── test_performance_regression.rs      # Performance regression detection
├── stress_test_comprehensive.rs        # Stress test suite
├── test_blob_size_enforcement.rs       # Blob transfer size limits
├── test_image_iced_gui_flow.rs         # Image send/download through GUI
├── test_image_receiver_download.rs     # Image download flow
├── test_image_send_download.rs         # Image send → download round trip
├── test_image_cache_persistence.rs     # Image cache durability
├── test_iced_chat_flow.rs              # GUI chat flow
├── test_full_chat_list_flow.rs         # Full chat list navigation
├── test_multi_image_burst.rs           # Burst image message test
├── test_online_user_list.rs            # Online user list test
├── test_signed_gossip_flow.rs          # Signed gossip message flow
├── test_serde_format.rs               # Serialization format stability
├── test_security.rs                    # Security edge cases
├── test_no_bootstrap.rs                # Bootstrap-free operation
├── test_stale_bootstrap.rs             # Stale bootstrap handling
├── test_local_address_lookup.rs        # mDNS address lookup tests
├── test_room_invite_v2.rs              # Room invite V2 flow
├── test_two_peers_exchange.rs          # Two-peer message exchange
├── test_two_peers_relay.rs             # Two-peer relay connectivity
├── stale_bootstrap.rs                  # Stale bootstrap edge cases
├── repro_two_iced_instances.rs         # Two concurrent GUI instances
├── mailbox.rs                          # Mailbox protocol tests (encryption, signatures, idempotent acceptance)
├── image_optimizer_integration.rs      # Image optimization tests
├── compression_integration.rs          # Compression integration tests
├── verify_gui_bootstrap.rs             # GUI bootstrap verification
├── gen_stress_data.rs                  # Stress test data generator
├── generate_test_images.py             # Script: generate test images
└── test_performance_baseline.rs        # Performance baseline
```

## Running Tests

```sh
# Run all tests (requires net + test-utils features)
cargo test --features net,test-utils

# Run GUI tests (requires gui feature)
cargo test --features gui

# Run a specific test
cargo test --test test_message_lifecycle --features net,test-utils

# Run with full output
cargo test -- --nocapture

# Run storage integration tests
cargo test --test test_storage_integration --features net
```

## Deterministic Simulation

The `sim` test at `tests/sim.rs` and the `sim` binary at `src/bin/sim.rs`
provide a deterministic simulation framework for the gossip protocol.

### Feature: `simulator`

The `simulator` feature enables the `sim` binary with:
- `tracing-subscriber` for structured logging
- `toml` for simulation configuration
- `clap` for CLI argument parsing
- `serde_json` for output serialization
- `rayon` for parallel simulation execution
- `comfy-table` for formatted output

### Running the Simulation

```sh
# Run the sim test
cargo test --test sim --features simulator

# Run the sim binary
cargo run --bin sim --features simulator -- --help

# With a config file
cargo run --bin sim --features simulator -- --config simulations/all.toml
```

## Test Patterns

### Delivery State Machine Tests

The delivery state machine in `src/delivery_state.rs` has exhaustive tests
covering:

- All happy-path transitions (Queued → Sending → SentAwaitingAck → Acknowledged)
- Retry loop (Sending → RetryScheduled → Sending)
- Failure / expiry / cancellation from every non-terminal state
- Identity (no-op) transitions for every state
- Invalid backwards transitions from every state
- Invalid skip transitions (e.g. Queued → Acknowledged)
- Terminal-state immutability (Acknowledged, FailedPermanent, Expired, Cancelled)

These tests use the FSM's `can_transition_to()` function directly without IO
or networking, making them fast and deterministic.

### Integration Tests (two peers)

Most integration tests follow a pattern:
1. Spawn two (or more) iroh endpoints with in-memory storage
2. Subscribe both to the same gossip topic
3. Send a message from peer A
4. Assert peer B receives it
5. Verify delivery state (ACKs, storage)

Example from `test_message_transfer.rs`:
```rust
#[tokio::test]
async fn test_basic_message_transfer() -> Result<()> {
    let (ep1, gossip1, _) = make_endpoint_and_gossip(1).await?;
    let (ep2, gossip2, _) = make_endpoint_and_gossip(2).await?;
    // ... subscribe, send, verify
}
```

### Storage Tests

Storage tests use in-memory or temp-directory SQLite databases to verify:
- CRUD operations on inbox, outbox, file objects
- Migration correctness (V1 → V2 → V3 → V4)
- Crash recovery (simulated crashes via incomplete operations)
- Tombstone semantics (local/remote deletion, re-insertion prevention)
- File library lifecycle (import, reference, change detection, cleanup)

### MCP Tests

The MCP diagnostic server is tested via TCP connections in
`test_mcp_diagnostics_integration.rs`:
1. Start the application with `--mcp` enabled
2. Connect to the MCP port
3. Send JSON-RPC 2.0 requests
4. Assert responses match expected shapes

### Performance Tests

- `test_performance_baseline.rs` — measures message throughput, latency, and
  memory usage on reference hardware
- `test_performance_regression.rs` — compares current results against stored
  baseline, failing if degradation exceeds configured thresholds
- `stress_test_comprehensive.rs` — multi-peer message burst tests

## Test Features

| Feature | Required For |
|---------|--------------|
| `net` | All networking tests (gossip, inbox, backfill, whisper) |
| `test-utils` | Deterministic RNG (`chacha`), `humantime-serde` for config |
| `gui` | GUI tests (image optimization, iced integration) |
| `simulator` | Deterministic simulation binary |

## Remote file-sharing tests

The remote catalogue and transfer tests should verify the stages independently:

1. **Catalogue signing** — a generated catalogue verifies with the owner's key;
   changing any signed field, owner, revision, or file metadata is rejected.
2. **Requester filtering** — friends see enabled available offers, explicit
   `read` grants expose only granted files to non-friends, blocked peers receive
   access denied, and disabled/unavailable files are omitted. Build catalogues
   for two requesters and assert their projections are isolated.
3. **Notification/cache behaviour** — a revision notice triggers one fetch,
   duplicate notices are coalesced, `NotModified` preserves a verified cache,
   and stale/manual/missing-file triggers refresh without a polling loop.
4. **Authorisation** — access is re-evaluated after catalogue creation;
   changed hashes/versions, disabled offers, missing data, blocked peers, and
   expired descriptors are denied. A descriptor must be bound to the transport
   requester and fail signature/expiry verification when tampered with.
5. **Blob transfer** — exercise successful iroh-blobs download, wrong hash,
   wrong size, provider failure, cancellation, pause/resume, atomic destination
   installation, and cleanup of temporary output. Verify that resumed output is
   re-streamed and that only verified BLAKE3 content is installed.
6. **Abuse limits** — test request/response/catalogue size caps, pagination
   bounds, per-peer/global concurrency, rate limits, timeouts, and upload
   preparation limits. Assertions should use structured protocol errors rather
   than log text.

A complete end-to-end scenario is: publish an enabled local file, notify a peer,
fetch and verify its requester-filtered catalogue, request authorisation, fetch
through iroh-blobs, and assert size/hash plus durable download state. Run the
same scenario from both peers where applicable; a successful catalogue fetch is
not evidence that transfer authorisation or blob delivery succeeded.

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/lint.sh` | Wrapper for clippy that handles edition correctly |
| `scripts/flamegraph.sh` | Generate CPU flamegraphs (requires `cargo-flamegraph`) |
| `scripts/boru-test-instance.sh` | Launch a test GUI instance for manual testing |
