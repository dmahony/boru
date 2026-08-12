# Boru Secure Tunnels: integration design note

Status: implementation design (Phase 12 complete). This note records the existing integration points, the transport concepts worth reusing from `n0-computer/dumbpipe`, and the persistence decision for secure tunnels.

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
6. subscribe to directory/discovery topics and start discovery/background trackers;
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

## Persistence decision (Phase 12)

Tunnel definitions are ephemeral in v1. A `TunnelService` owns its definitions in process memory, including the tunnel ID, owner and allowed peer identities, loopback target, expiry, lifecycle status, connection limits, and cancellation state. Creating a tunnel therefore does not write a capability, tunnel definition, target, or active-connection record to Boru's SQLite stores. Dropping the service (normally when Boru exits) removes all active definitions; a subsequent process starts with an empty tunnel service.

This is intentional rather than an accidental limitation:

- a capability is an authorisation secret and should not be copied into durable storage without a clear recovery and access-control design;
- an ephemeral definition cannot survive a restart with stale expiry, peer authorisation, or a target that the owner no longer intends to expose;
- revocation and shutdown have a simple meaning while all active state is owned by one service instance; and
- existing SQLite persistence remains unchanged and is not coupled to tunnel lifecycle operations.

Capability material must not be written to logs. Diagnostics and future UI status may expose non-secret metadata (for example a redacted tunnel ID, lifecycle state, expiry state, and counts), but must never print the capability token or serialized handshake containing it.

### Future persistence migration (design only; not implemented)

If restart survival becomes a product requirement, it must be introduced as a dedicated, reviewed SQLite migration rather than by adding writes to the v1 tunnel service. That migration should:

1. define a versioned table for encrypted or otherwise protected tunnel records, with explicit ownership and authorization fields;
2. specify key management and recovery before storing any capability secret (including what happens when the local identity changes);
3. store only the minimum required metadata, with expiry and revocation state, and never store plaintext capability material unless a security review explicitly approves it;
4. make startup restoration opt-in and reject expired, malformed, owner-mismatched, or revoked records;
5. define atomic revocation/deletion and migration rollback behaviour; and
6. add migration, restart, expiry, revocation, and secret-redaction tests before enabling the feature.

Until that migration exists, callers must treat tunnel definitions and capabilities as process-lifetime state and recreate them explicitly after restart.

## Optional CLI/debug interface (Phase 21)

The current Boru command surface is the GUI binary (`cargo run`), whose Clap commands select or join chat rooms before the long-lived GUI/network runtime is started. It does not provide a standalone process or a reusable command context for tunnel operations: tunnel definitions and capabilities are owned by the in-process `TunnelService`, and tunnel protocol handling is registered on the shared endpoint/router created by the GUI startup path.

Therefore Phase 21 does not add a separate `boru tunnel share <friend> <port>` or `boru tunnel connect <tunnel>` command. Adding those commands now would either create a second endpoint/identity and duplicate runtime setup, or require a new control API and lifecycle boundary for attaching short-lived CLI invocations to the running GUI. Neither is a small, natural extension of the existing architecture, and no new CLI framework is justified solely for this feature.

A future CLI/debug surface should reuse the existing endpoint, identity, friends store, `TunnelService`, and protocol router through an explicit service/control boundary. That boundary should define how a share command returns a redacted tunnel invitation/capability, how connect resolves the recipient and tunnel, and how the long-lived process remains alive while forwarding local TCP streams. Until then, tunnel control remains an in-process/API integration point rather than a supported shell command.

## Future use cases (Phase 23)

Tunnel v1 is a reliable byte-stream protocol. Each tunnel maps a local loopback TCP target (and on Unix, a local Unix socket target) onto one or more authenticated, recipient-bound Iroh QUIC streams. This section records how the same foundation could later support broader use cases. These are documented directions, not implemented features: nothing here changes `/boru-tunnel/1`, and none of it should be built until a concrete product need arrives.

### Scope boundary: v1 stays a reliable byte-stream protocol

`/boru-tunnel/1` is deliberately not a media protocol. The v1 contract is:

- a tunnel carries opaque, ordered, reliable bytes between the owner and one allowed peer;
- the owner chooses and stores the local target (loopback TCP or Unix socket path) locally; the remote peer never supplies an arbitrary host, port, or socket path;
- capabilities are recipient-bound, expiring, and validated before any stream is forwarded;
- one local connection maps to one QUIC bidirectional stream, with bounded per-tunnel and per-connection limits.

Realtime voice and video will not be bolted onto this protocol. They have fundamentally different requirements (bounded latency, loss tolerance, jitter buffering, codec framing) that a reliable byte-stream protocol cannot satisfy without turning every media packet into a head-of-line blocking hazard. If Boru later supports realtime media, it should use Iroh datagrams or a specialised transport design with its own ALPN and protocol handler, sharing the existing endpoint, identity, friends store, and capability/authorization model — never by extending `/boru-tunnel/1`.

The practical rule for future work: if a use case only needs a reliable byte stream, it can ride on tunnel v1 unchanged. If it needs realtime, loss-tolerant, or datagram semantics, it needs a new transport and must not be squeezed into the tunnel protocol.

### SSH forwarding

SSH is a natural fit for tunnel v1. An owner who runs `sshd` on a loopback port (or holds an SSH agent on a Unix socket) can create a tunnel whose target is that local listener; a trusted friend connects through the tunnel and gets an authenticated, encrypted SSH session that also rides Boru's existing identity, relay/direct connection, and capability validation. Future refinements (not required for v1):

- convenience flows that pre-fill the target port for common services (`22` for SSH, agent socket paths);
- per-tunnel metadata describing the service so the receiving side can offer sensible defaults;
- keep-alive tuning for long-lived SSH sessions, which already benefit from v1's explicit cancellation semantics.

No media or datagram support is needed; SSH is a byte-stream protocol.

### Game servers

TCP-based game servers (for example Minecraft-style dedicated servers, MUDs, or turn-based games) are already within v1's reach: expose the local server port as a tunnel target and let a friend connect. The constraints that matter are the same as for any long-lived byte stream: connection limits, idle behaviour, and cancellation. Future refinements:

- a documented "game server" tunnel profile that raises per-connection limits and tunes keep-alives;
- possibly a dedicated ALPN variant if the game protocol needs its own framing or pre-connection handshake.

Realtime action games that assume UDP will not fit v1; like voice/video they would need a datagram or low-latency transport design and are out of scope for the byte-stream tunnel.

### Terminal sharing

Terminal sharing (for example sharing a Zellij, tmux, or shell session with a friend) is another byte-stream use case. The architecture naturally points at the Unix socket support described in this plan (Step 22): an owner exposes a local Unix socket that the terminal multiplexer writes to, and a friend connects through the tunnel. Because Unix socket targets are already part of the design surface (conditional compilation, owner-chosen path, no remote path exposure), terminal sharing can be layered on without a protocol change.

Future work to consider:

- a control channel or out-of-band signal for resize and session control, if the multiplexer protocol requires it;
- read-only versus interactive sharing modes enforced on the owner side;
- careful handling of terminal escape sequences, which are just bytes from the tunnel's point of view.

### Remote support

Remote support (temporary assistance on a friend's machine) fits the v1 model well: the person being helped creates a short-lived, recipient-bound tunnel to a local service (a shell, a diagnostic tool, or a screen-viewer service), and the helper connects through it. The existing capability model already supports expiry and revocation, which are exactly the properties remote-support sessions need: access is temporary, explicit, and can be cut off at any time.

Future work to consider:

- UI flows for "start a support session" that create a tunnel with a short expiry and clear lifecycle status;
- audit/event reporting of who connected and when (already anticipated by the diagnostics and lifecycle-logging direction in this plan);
- integration with the network-doctor diagnostics so a support session can observe the same live node state.

### Screen sharing

Screen sharing can be built in two very different ways, and v1 is only suited to one of them:

- Frame/byte-stream screen sharing (for example streaming encoded still frames, or a tool that emits a byte stream a remote viewer renders) can ride v1 unchanged: it is just a reliable byte stream.
- Realtime interactive screen sharing (low-latency region updates, cursor movement, continuous video-like updates) has the same constraints as video and should not be forced through a reliable byte-stream protocol.

Future work should first decide which experience is wanted. If it is the realtime one, it belongs on the future datagram/specialised transport, not on `/boru-tunnel/1`.

### Voice and video

Explicitly out of scope for tunnel v1. The plan's guidance is unambiguous: do not prematurely turn `/boru-tunnel/1` into a media protocol, because realtime voice/video may eventually require datagrams or a specialised transport design.

If Boru later adds realtime media:

- introduce a new ALPN (for example `/boru-media/1` or a datagram-based protocol) rather than extending the tunnel ALPN;
- reuse the existing endpoint, identity, friends store, capability/authorization model, and relay/direct connection machinery;
- treat media signalling (session setup, codec negotiation, keying) separately from media data, and keep both out of the byte-stream tunnel;
- apply the same abuse-protection, limit, expiry, and revocation thinking used for tunnel v1.

Until such a transport exists, voice/video remain future work and the tunnel protocol stays a byte-stream protocol.

### Developer web servers

Developer web servers (for example a local `localhost:3000` dev server shared with a collaborator) are a classic tunnel use case and already work with v1's TCP target model. The owner creates a tunnel whose target is the local dev server port; the friend connects and sees the same HTTP server. Future refinements:

- documented profiles for common dev-server ports and a UI that makes sharing a dev server a one-click flow;
- optional per-tunnel display metadata describing the service (for example "dev server on port 3000") without exposing target details to the remote peer;
- consideration of host-header or SNI handling if multiple dev servers are shared at once — this is application-level, not transport-level.

### Local dashboards

Local dashboards (metrics, admin UIs, home-automation panels, personal tools bound to loopback) share the same shape as developer web servers: the owner binds a local dashboard, creates a tunnel to it, and selects a trusted friend to view it. v1's loopback-only default is exactly the right safety property: dashboards are not exposed to the network at large, only through an authenticated, expiring, recipient-bound tunnel. No protocol change is required.

### Device management

Device management (administering a remote device: a router, server, or embedded board) is a broad future category. The byte-stream tunnel can already provide the secure transport for device management protocols such as SSH to the device, a local admin web UI, or a device-specific control protocol. Future work to consider:

- a device-management orientation in the GUI that groups tunnels by device and shows lifecycle/connection state;
- integration with diagnostics and the network doctor so a device's tunnel health is observable;
- policy controls (who may create device tunnels, for how long, to which local services) built on the existing capability model.

Anything that needs realtime telemetry streaming at high rates should again be evaluated against the datagram/specialised-transport path rather than the byte-stream tunnel.

### Summary of the boundary

- v1 (`/boru-tunnel/1`) carries reliable byte streams only; it does not carry media.
- Byte-stream use cases (SSH, TCP game servers, terminal sharing, remote support, screen sharing by frame stream, dev web servers, local dashboards, device-management transports) can be layered on v1 without protocol changes; the natural hooks are target profiles, metadata, UI flows, and diagnostics.
- Realtime use cases (interactive screen sharing, voice, video, UDP game traffic) require a future datagram or specialised transport with its own ALPN; they must not be implemented inside `/boru-tunnel/1`.
- All future transports should reuse the existing endpoint, identity, friends store, capability/authorization model, limits, and cancellation/shutdown semantics rather than inventing a parallel stack.

## Invariants

1. One primary Boru Iroh endpoint and identity.
2. Existing gossip, DM, inbox, backfill, blobs, file-access, and discovery ALPNs remain registered and unchanged.
3. A remote peer can select only an explicitly created tunnel, never an arbitrary destination.
4. Tunnel capabilities are recipient-bound, expiring, and validated before stream forwarding.
5. Loopback-only exposure/listening is the default.
6. Cancellation and shutdown are explicit, bounded, and free of orphan tasks.
7. Every networking change is accompanied by focused tests and the existing format/check/test gates.
