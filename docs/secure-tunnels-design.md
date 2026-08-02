# Boru Secure Tunnels: integration design note

Status: architecture investigation only (Phase 1). This note records the existing integration points and the transport concepts worth reusing from `n0-computer/dumbpipe`; it does not implement the tunnel protocol.

## Existing Boru networking architecture

### One shared Iroh endpoint

The GUI creates the process-wide endpoint in `examples/iced_chat/main.rs` (the `runtime.block_on` startup block, around lines 704–755). The endpoint uses the persisted Boru `SecretKey`, a `Minimal` preset, the configured `RelayMode`, and the existing address-lookup chain:

- mDNS (`MdnsAddressLookup`), subscribed before endpoint binding;
- Pkarr/DNS lookups when configured;
- a `MemoryLookup` registered in the endpoint lookup registry;
- optional DHT address lookup registered later through `endpoint.address_lookup()`.

The endpoint binds the configured UDP port and is returned to the GUI together with the lookup handle, gossip service, and router. Secure tunnels must clone and reuse this endpoint; they must not create a second endpoint or identity.

The endpoint's public identity is `endpoint.id()` / `EndpointId` (the application commonly uses the `iroh::PublicKey` alias for peer identities). The endpoint address is obtained from `endpoint.addr()` and remote path information from `endpoint.remote_info(peer).await` where available.

### ALPN registration and incoming dispatch

Boru uses Iroh's `iroh::protocol::Router`, built in `examples/iced_chat/main.rs` around lines 949–958 with `Router::builder(endpoint.clone())`. Existing handlers are registered independently by ALPN:

- `/iroh-gossip/1` → `Gossip`
- Iroh blobs ALPN → blob protocol
- `/iroh-gossip-chat/friend-ping/1` → `PingHandler`
- backfill, whisper, inbox, catalogue, and file-access ALPNs → their dedicated handlers.

`src/net.rs` defines `GOSSIP_ALPN` and implements `ProtocolHandler for Gossip`; the handler forwards an accepted `Connection` to the gossip actor. `src/net.rs` also documents the older manual-dispatch shape, but the GUI's live path uses the Router. Other dedicated protocols follow the same `ProtocolHandler` + `.accept(ALPN, handler)` pattern (for example `src/inbox.rs`, `src/backfill.rs`, `src/whisper/mod.rs`, and `src/chat_core/friend_ping.rs`).

A tunnel implementation should add one new ALPN and one focused `ProtocolHandler` to this same router. It must not replace or multiplex the existing ALPN handlers. The handler should authenticate and route tunnel streams to a separate service/module.

### Outgoing connections

There are two relevant existing patterns:

1. Gossip owns its peer dials. `Gossip::builder().spawn(endpoint.clone())` starts the actor. The actor queues `EndpointAddr` + ALPN dials, calls `endpoint.connect`/the configured connect path, retries with bounded backoff, and tracks accepted/dialled connections internally (`src/net.rs`, `LocalActorMessage`, actor run loop, and dialer code).
2. Point protocols dial explicitly. For example, `src/catalogue_client.rs` constructs `EndpointAddr::new(server_pk)` and calls `client_ep.connect(addr, CATALOGUE_ALPN)` under a timeout, then opens request streams. `src/chat_core/friend_ping.rs` uses cached addresses or `Endpoint::remote_info`, then `connect_with_opts` with `FRIEND_PING_ALPN` and a short timeout.

A tunnel service should own its tunnel-specific outgoing connection/stream lifecycle, while using the shared endpoint and the owner's stored tunnel definition. It should not add a parallel peer-discovery or transport stack.

### Endpoint IDs, friends, and address resolution

`src/friends.rs` stores a stable `FriendId` backed by the peer public-key string. `FriendId::parse_public_key()` reconstructs the `PublicKey`; `FriendRecord` stores bounded, newest-first `known_addrs: Vec<EndpointAddr>` (capped at five) and relationship/status metadata. GUI startup loads friends from SQLite, falls back/migrates from JSON, and registers each friend's stored addresses with `FriendPingManager` (`examples/iced_chat/main.rs`, around lines 910–928 and 1272–1284).

`src/chat_core.rs` provides the address plumbing used by room joins: it deduplicates ticket/room addresses, seeds `MemoryLookup`, and refreshes room bootstrap peers from `endpoint.remote_info`. `src/net/address_lookup.rs` and `src/net/address_resolution.rs` integrate learned addresses with the endpoint lookup chain. `GossipAddressLookup` is added by `Gossip::builder().spawn` when a friends store is supplied.

For tunnels, the selected friend should be resolved to a `PublicKey`/`EndpointId`, and the tunnel owner's local target must remain in local tunnel state. The remote request must identify an existing tunnel, not supply an arbitrary host or port. Known friend addresses may be used as bootstrap hints; normal Iroh lookup/relay fallback remains authoritative for connection establishment.

### Direct and relay connections

The application retains the configured `RelayMode` on the endpoint and logs the endpoint address at startup. Endpoint addresses can contain relay URLs and direct address hints. Iroh's `Connection` exposes path information (`Connection::paths()` in the underlying Iroh API) and remote metadata, but Boru should only report Direct/Relay when reliable path information is available. Otherwise tunnel UI should say Connected/Unknown rather than infer a route.

The existing GUI also uses relay-only versus direct-address publication policy for DHT address lookup. This policy is transport/address publication configuration, not a second connection mechanism.

### Shutdown and cancellation

The Router is stored in `IcedChat` as `_router` so it remains alive for the GUI lifetime; dropping it stops protocol acceptance. The endpoint is also retained in `IcedChat`. Individual services use explicit shutdown hooks or task lifetime ownership:

- `Gossip` has a shutdown command and abort-on-drop actor handle (`src/net.rs`);
- `FriendPingManager` is a spawned actor driven by an mpsc command channel;
- continuous room/DHT trackers use `CancellationToken` and await their task handles;
- download and transfer paths use bounded cancellation flags/tokens.

A tunnel service should use a shared `CancellationToken` for service shutdown and per-tunnel/per-stream child tokens. Cancellation must stop forwarding tasks, finish/reset QUIC send/receive sides appropriately, and avoid orphaned tasks. Endpoint/router shutdown should happen through the existing owner/lifetime rather than by closing the shared endpoint from a tunnel operation.

### Where networking services start

The startup order is in `examples/iced_chat/main.rs`:

1. load identity and configure discovery/address lookup;
2. bind the shared endpoint;
3. spawn gossip and initialize blobs/history;
4. construct backfill, whisper, inbox, catalogue, and file-access handlers;
5. build the shared Router with all existing ALPN handlers;
6. subscribe to lobby/directory rooms and start discovery/background trackers;
7. create `FriendPingManager`, register persisted friends, and construct `IcedChat`.

A future `TunnelService` should be constructed after the endpoint and friends store are available, before Router construction if its protocol handler is registered there. Its handle and event receiver should be passed into `IcedChat` similarly to the existing whisper/inbox/friend-ping services.

### GUI ↔ boru_core communication

`boru_core` is a library crate containing protocol/state/network code; `examples/iced_chat/app.rs` is the Iced frontend. `IcedChat::new` receives the shared endpoint, router, gossip handle, friend manager, protocol handles, and bounded mpsc receivers/senders. GUI subscriptions poll those receivers and translate core network events into `AppMessage`/state updates. Gossip events are forwarded through `forward_gossip_events` and conversation channels; core state mutation remains in the shared chat-core/event path rather than in transport handlers.

The diagnostic MCP server is also in-process (`examples/iced_chat/mcp_server.rs`). Startup gives it a clone of the endpoint and the shared diagnostics/journal state, so diagnostics observe the same live node. This is a useful model for future tunnel commands: expose a narrow service handle/API to the GUI and MCP, not raw protocol internals or socket state.

## Dumbpipe concepts worth reusing (not its CLI architecture)

The inspected dumbpipe implementation uses a single Iroh endpoint with an ALPN (`DUMBPIPEV0`) and a small fixed stream handshake (`hello`). The useful transport ideas are:

- separate control/handshake bytes from the opaque application byte stream;
- use `Connection::open_bi()` on the connecting side and `Connection::accept_bi()` on the accepting side;
- map one local TCP/Unix connection to one QUIC bidirectional stream;
- accept multiple local connections in a loop and spawn bounded per-connection forwarding tasks;
- use two directional copy operations and cancel both when either direction ends;
- on cancellation, reset/stop the corresponding QUIC stream; on successful local EOF, finish the send side;
- treat connection/stream/forwarding errors as per-connection failures so one bad stream does not crash the process;
- close the endpoint explicitly during standalone process shutdown.

Dumbpipe's CLI creates an endpoint per command and puts the destination in CLI arguments. Boru must not copy that architecture: Boru already has a long-lived endpoint, authenticated friends, a Router with many ALPNs, GUI/core channels, relay/address lookup, and application shutdown ownership. The Boru tunnel protocol therefore needs a versioned authenticated request/capability handshake before forwarding bytes, and its owner-side tunnel definition must select the loopback target locally.

## Proposed future integration boundary

Add a focused `tunnel` module (protocol, capability validation, service, and forwarding) in `boru_core`:

- define a dedicated versioned ALPN such as `/boru-tunnel/1`;
- register its handler alongside the existing Router accepts;
- validate peer identity, recipient-bound capability, expiry, tunnel ID, and local tunnel state before accepting a stream;
- keep loopback-only TCP target/listener policy in the service, never in remote request data;
- reuse the shared endpoint and Iroh connection paths/relay behavior;
- expose small service methods/events to the GUI and MCP through handles/channels;
- keep tunnel definitions ephemeral initially and do not persist capability secrets;
- add protocol, forwarding, cancellation, and resource-limit tests before GUI work.

The first implementation milestone should be a raw Boru-to-Boru authenticated bidirectional stream. TCP forwarding and GUI flows should be layered on only after that transport path is tested.

## Invariants

1. One primary Boru Iroh endpoint and identity.
2. Existing gossip, DM, inbox, backfill, blobs, file-access, and discovery ALPNs remain registered and unchanged.
3. A remote peer can select only an explicitly created tunnel, never an arbitrary destination.
4. Tunnel capabilities are recipient-bound, expiring, and validated before stream forwarding.
5. Loopback-only exposure/listening is the default.
6. Cancellation and shutdown are explicit, bounded, and free of orphan tasks.
7. Every networking change is accompanied by focused tests and the existing format/check/test gates.
