# Public lobby identity

The canonical public-lobby identity is derived by `public_room_identity` in
`src/public_room.rs`. It uses `PUBLIC_ROOM_NAME`, `PROTOCOL_VERSION`, and the
`PublicNetwork` discriminator. The gossip topic is BLAKE3 with the
`boru-chat public-room v1` domain separator; the DHT discovery key is a
separate domain-separated BLAKE3 value.

Call sites:

- `PublicRoomTracker::start` derives the identity through
  `public_room_identity(network)` and uses its discovery key for publication
  and lookup.
- `examples/iced_chat/main.rs` starts the tracker with `PublicNetwork::Mainnet`.
- `examples/iced_chat/app.rs` opens and compares the default lobby through
  `public_lobby_topic(PublicNetwork::Mainnet)`.
- `examples/dht_harness.rs` uses `public_room_identity` directly.

The old GUI-only seed `iroh-gossip-chat/default-lobby/v1` was removed. Existing
users with data or peers keyed to that topic must re-open the lobby after this
migration; the new identity intentionally does not alias the old topic. Network
and protocol-version changes also intentionally create disjoint rooms.
