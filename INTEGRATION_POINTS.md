# Integration Points — Files & Functions to Change

For adding public-room DHT discovery (next cards), these are the exact integration points.

## Files to modify (existing)

| File | Function(s) | Change |
|---|---|---|
| `Cargo.toml` | — | Add `distributed-topic-tracker = { version = "0.3.5", default-features = false }` |
| `src/lib.rs` | — | Add `pub mod discovery;`, `pub mod public_room;`, `pub mod public_room_config;` |
| `src/api.rs` | `GossipSender::join_peers()` | Uses new `our_endpoint_id` for self-filter (already in working tree) |
| `src/chat_core.rs` | New: `public_room_discovery_secret()` | Deterministic discovery key from topic + secret |
| `examples/chat.rs` | `main()` bootstrap section | Wire `TopicTracker::start()` + `start_continuous()` after subscribe |
| `examples/iced_chat/main.rs` | `block_on` endpoint section | Pass discovery config to app |
| `examples/iced_chat/app.rs` | `IcedChat::new()`, `OpenRoom`, `JoinFromTicket` | Wire background DHT discovery |

## Files to create (new)

| File | Purpose |
|---|---|
| `src/discovery/mod.rs` | Module header, re-exports |
| `src/discovery/topic_tracker.rs` | `TopicTracker` + `ContinuousDiscovery` — Mainline DHT publish/discover |
| `src/discovery/validation.rs` | `DiscoveryConfig`, `DiscoveryRecordValidator` — record validation |
| `src/discovery/public_record.rs` | `PublicDiscoveryRecord` — minimal self-signed discovery record |
| `src/discovery/backend.rs` | `TopicDiscoveryBackend` trait + `InMemoryDiscoveryBackend` + `DhtDiscoveryBackend` |
| `src/discovery/public_room_tracker.rs` | `PublicRoomTracker<B>` — backend-agnostic tracker |
| `src/discovery/namespace.rs` | Deterministic SHA-256 namespace derivation |
| `src/discovery/invite.rs` | Invite-based peer discovery and migration |
| `src/public_room.rs` | `PublicRoomIdentity`, `PublicNetwork` — canonical constants |
| `src/public_room_config.rs` | `PublicRoomConfig` — all safety limits in one struct |
| `src/topic_derivation.rs` | Deterministic TopicId derivation (already created) |
| `src/room_docs.rs` | Metadata/roster document sync (already exists) |
