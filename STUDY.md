# STUDY: Boru network subsystems — patterns for the P2P call subsystem

Task: BORU-CALL-0.2 (Phase 0 research, no implementation).
Repo: iroh-gossip-chat (crate `boru-core`; GUI in `examples/iced_chat`).
Commit this study was written against: `f1751170` (TUN-02, 2026-08-08).

This document explains how Boru's existing network subsystems are built so the
planned call subsystem (CallBuilder / CallProtocol / CallHandle / CallManager)
can reuse established patterns. It covers: (1) whisper, (2) inbox / backfill /
file-access / net, (3) Iced GUI event flow, (4) peer authorization lifecycle,
(5) friend address → EndpointAddr construction.

---

## 1. Whisper — the direct-QUIC private messaging subsystem

Files: `src/whisper/mod.rs` (1049 lines), `src/whisper/session_manager.rs` (790 lines).

Whisper is the closest existing model for a call subsystem: a dedicated ALPN,
a builder → handle + event-receiver actor, a ProtocolHandler registered on the
Iroh Router, and per-connection reader tasks. It is a *direct* QUIC protocol,
separate from the gossip broadcast mesh, exactly like a call channel would be.

### 1.1 Wire protocol and ALPN

- `WHISPER_ALPN = b"/iroh-gossip-chat/whisper/1"` — `src/whisper/mod.rs:42`.
- Wire frames are a postcard-encoded enum `WhisperWireMessage` with variants
  `Text`, `Control` (opaque signed payload), `MailboxEnvelope`, `MailboxAck` —
  `src/whisper/mod.rs:53-69`. Length-prefixed framing: 4-byte LE length +
  payload over a bi-directional stream (`write_framed_message`,
  `src/whisper/mod.rs:635-662`; read side `read_connection_loop`,
  `src/whisper/mod.rs:755-813`). Max payload `MAX_WHISPER_PAYLOAD = 16 MB`
  (`src/whisper/mod.rs:48`).
- Every message is sent on a fresh `open_bi()` stream and finished with
  `send.finish()`; the connection stays open and multiplexes many streams.

### 1.2 Builder, handle, event receiver (the actor pattern to copy)

- `WhisperBuilder` (`src/whisper/mod.rs:332-394`):
  - `WhisperBuilder::new(endpoint: Endpoint, secret_key: SecretKey)` creates an
    mpsc command channel (`CMD_CHANNEL_CAP = 256`, `mod.rs:45`) and stores the
    receiver in `cmd_rx: Option<mpsc::Receiver<Cmd>>` so only `spawn()` can take it.
  - `protocol_handler()` (`mod.rs:368-373`) produces a `WhisperProtocol` clone
    that shares the same `cmd_tx` and the same denied-peers set. **The handler is
    not a separate actor — it is a thin transport shim that forwards incoming
    connections into the actor's command channel.**
  - `spawn(self) -> (WhisperHandle, mpsc::Receiver<WhisperEvent>)`
    (`mod.rs:376-393`): creates the 1024-capacity event channel, an
    `Arc<Mutex<HashMap<PublicKey, Connection>>>` connected-map, builds the
    handle, then `tokio::task::spawn(run_actor(...))`. The event receiver is
    handed to the caller (the GUI), never wrapped inside the actor.
- `WhisperHandle` (`mod.rs:168-290`): a cheap cloneable handle holding
  `cmd_tx: mpsc::Sender<Cmd>` and the shared denied-peers set. Public methods
  are async request/response calls that send a `Cmd` with a `oneshot` reply
  channel and await it — e.g. `send_dm`, `send_control`, `connect_to`,
  `disconnect` (`mod.rs:198-283`). `set_peer_authorized(peer, bool)`
  (`mod.rs:179-192`) mutates the shared denied set **and** sends
  `Cmd::RevokePeer(peer)` so a newly-denied peer's existing connection is torn
  down immediately.
- `Cmd` (internal, `mod.rs:119-154`): `SendDm`, `SendControl`,
  `SendMailboxEnvelope`, `SendMailboxAck`, `ConnectTo`, `Disconnect`,
  `RevokePeer`, `IncomingConnection(Connection)`. Every user-facing method
  carries a `oneshot::Sender<Result<...>>` for the reply.
- `WhisperEvent` (public, `mod.rs:74-115`): `Message`, `Control`,
  `MailboxEnvelope`, `MailboxAck`, `Connected { peer }`, `Disconnected { peer }`.
  Frontends match on this enum to update state.

### 1.3 The actor loop (`run_actor`, `mod.rs:399-534`)

A single `tokio::select!` loop multiplexes two channels:

```rust
loop {
    tokio::select! {
        cmd = cmd_rx.recv() => { ... }        // handle commands from WhisperHandle
        Some(ev) = msg_rx.recv() => { ... }   // per-connection reader events
    }
}
```

- Commands perform one unit of work inline (send a message, connect,
  disconnect, revoke) and answer the oneshot reply.
- `IncomingConnection(conn)` (from the ProtocolHandler) registers the connection
  in the connected-map, emits `WhisperEvent::Connected`, and spawns a
  `read_connection_loop` task (`mod.rs:467-473`).
- `ConnectionEvent::Disconnected` removes the peer and emits
  `WhisperEvent::Disconnected` (`mod.rs:520-523`).
- On `cmd_rx` close (all handles dropped) the loop breaks and closes every
  connection, emitting final `Disconnected` events (`mod.rs:529-534`).
- Connection identity is enforced: `connect_to_peer` verifies
  `conn.remote_id() == peer` and closes on mismatch (`mod.rs:610-618`).

### 1.4 Connection acquisition (`get_or_connect`, `mod.rs:542-588`)

The actor's send paths call `get_or_connect` rather than requiring a pre-made
connection: check the connected-map first; otherwise resolve addresses from
`endpoint.remote_info(peer)` and build an `EndpointAddr { id, addrs }`
(`mod.rs:559-585`); fall back to ID-only `EndpointAddr::new(peer)` which
triggers DHT/mDNS/DNS during `connect()`. Then `connect_to_peer` runs
`endpoint.connect(addr, WHISPER_ALPN)` under a **15 s timeout**
(`mod.rs:604-609`).

### 1.5 ProtocolHandler (`WhisperProtocol`, `mod.rs:294-327`)

`impl ProtocolHandler for WhisperProtocol`:
- Checks the shared denied-peers set first; returns `AcceptError` if denied
  (`mod.rs:306-316`).
- Otherwise forwards the `Connection` to the actor via
  `cmd_tx.send(Cmd::IncomingConnection(connection))` (`mod.rs:320-323`).

This is the canonical pattern: **the protocol handler never touches the wire
itself — it authorizes at accept time, then hands the connection to the actor
that owns all connection state.**

### 1.6 Registration on the Router

In the GUI, whisper is wired up in `examples/iced_chat/main.rs`:

```rust
let whisper_builder = WhisperBuilder::new(endpoint.clone(), secret_key.clone());
let whisper_handler = whisper_builder.protocol_handler();
let (whisper_handle, whisper_events_rx_tmp) = whisper_builder.spawn();
// ...later, on the Router:
let router = iroh::protocol::Router::builder(endpoint.clone())
    .accept(WHISPER_ALPN, whisper_handler)   // main.rs:1019
    ...
    .spawn();                                 // main.rs:1024
```

The test helper `create_node` in `mod.rs:827-852` shows the minimal
registration: `Router::builder(endpoint).accept(WHISPER_ALPN, handler).spawn()`.

### 1.7 `SessionManager` — reconnect state machine (currently unused)

`src/whisper/session_manager.rs`:
- `SessionState` (`session_manager.rs:47-57`): `Disconnected`, `Connecting`,
  `Connected`, `Reconnecting`.
- `SessionEvent::StatusChanged { peer, state }` (`session_manager.rs:74-83`).
- `SessionManager::spawn(whisper_handle, local_public) -> (Self, Receiver<SessionEvent>)`
  (`session_manager.rs:180-198`) spawns `SessionManagerActor::run`.
- Per-peer `PeerSession` tracks state + exponential backoff
  (`BACKOFF_BASE = 1s`, `BACKOFF_MAX = 60s`, `MAX_RECONNECT_ATTEMPTS = 10`,
  `session_manager.rs:32-39`).
- Reconnect uses `wh.send_dm(peer, String::new())` as a "connect probe" —
  an empty DM triggers address discovery + connection in the whisper actor
  (`session_manager.rs:261-273`, reconnect loop `session_manager.rs:367-400`).
- Connection-collision resolution: when a second `Connected` arrives while
  already `Connected`, the lower public-key peer keeps the outgoing connection
  and the higher-key peer closes its outgoing one (`session_manager.rs:317-335`).

**Important finding (documented in `docs/telepathy-room-dial-audit.md:229-236`):
`SessionManager` is dead code in the current app.** Nothing instantiates it or
calls `start_session`; the real whisper path does a single 15 s-timeout connect
per send and reconnects lazily on the next send via `get_or_connect`. The state
machine exists and is tested but is not wired into the GUI. A call subsystem
should either revive this pattern deliberately or not duplicate it.

---

## 2. Other protocol handlers — inbox, backfill, file-access, net

### 2.1 Inbox (`src/inbox.rs`, 1504 lines)

Offline-message delivery on its own ALPN `INBOX_ALPN = b"/iroh-chat-inbox/1"`
(`inbox.rs:49`). Security: every message is a `SignedInboxMessage` (sender +
timestamp + signature) for replay protection (`inbox.rs:136-146`); clock-skew
window 24 h (`inbox.rs:52`).

- `InboxInner` (`inbox.rs:257-280`) is the shared state:
  - `allowed_senders: HashSet<PublicKey>`
  - `authorization_fn: Option<Arc<dyn Fn(PublicKey) -> bool + Send + Sync>>` —
    a **live** callback consulted at connection time so contact changes take
    effect without restart (deliberately not a snapshot, `inbox.rs:262-264`).
  - `envelope_tx: mpsc::Sender<InboxEvent>` (forward events to frontend).
  - `pending_fn` / `record_sync_served_fn` — callbacks for sync responses.
- `InboxHandle` (`inbox.rs:333-416`): `new() -> (Self, mpsc::Receiver<InboxEvent>)`,
  `add_allowed_sender` / `remove_allowed_sender` / `set_allowed_senders`,
  `set_authorization_fn`, `set_pending_fn`, `set_record_sync_served_fn`.
  Note the different shape from whisper: **shared `Arc<Mutex<InboxInner>>`
  instead of a command channel.** The handle and the handler both point at the
  same inner state; handlers call back into it directly.
- `InboxProtocol` (`inbox.rs:423-454`): `new(inner)` + optional
  `with_secret_key(secret_key)` for signing SyncResponses.
- `impl ProtocolHandler for InboxProtocol` (`inbox.rs:456+`): checks
  authorization **before** accepting any streams (`inbox.rs:460-474`), then
  loops `connection.accept_bi()`, length-prefix framed postcard messages.
  Per-request dispatch is synchronous inside `handle_request` and produces an
  optional response written back on the same bi-stream (`inbox.rs:479-540`).

### 2.2 Backfill (`src/backfill.rs`, 1011 lines)

History backfill over `BACKFILL_ALPN`. Two sides:

- Server: `BackfillProtocolHandler` (`backfill.rs:183-203`) holds
  `storage: Arc<Storage>`, a per-peer `rate_limit`, and a global
  `backfill_semaphore: Arc<Semaphore>` capping concurrent serve tasks
  (`backfill.rs:189-201`). `impl ProtocolHandler` acquires the semaphore before
  work and returns early if full (`backfill.rs:205-226`).
- Client: `BackfillHandle` (`backfill.rs:419-502`) is another command-channel
  actor: `spawn(endpoint) -> Self` (`backfill.rs:428-432`); `request_history`
  sends `Cmd::RequestHistory` with a oneshot reply (`backfill.rs:445-469`);
  `backfill_actor` serializes requests so at most one outgoing backfill runs at
  a time (`backfill.rs:505+`).
- `backfill_round` (`backfill.rs:548-622`): `endpoint.connect(addr, BACKFILL_ALPN)`,
  `open_bi()`, length-prefixed request/response, wrapped in
  `tokio::time::timeout(BACKFILL_REQUEST_TIMEOUT, ...)` at the caller
  (`backfill.rs:651-664`).
- `try_backfill_from_peer` (`backfill.rs:480-501`) shows the
  remote-info → EndpointAddr path: `EndpointAddr::from_parts(peer, info.into_addrs())`.

### 2.3 File access (`src/file_access_handler.rs`, 3314 lines)

Signed download-descriptor issuance on `FILE_ACCESS_ALPN` (`net.rs:52-58`).

- `FileAccessHandler` (`file_access_handler.rs:607-625`) holds storage, secret
  key, `profile_user_id`, a `FriendsStore`, a shared `NonceStore` for replay
  protection, and limiter Arcs.
- `impl ProtocolHandler` (`file_access_handler.rs:1312-1341`): runs
  `serve_file_access` under a request timeout, then **waits on
  `connection.closed().await`** so the client can finish reading before the
  handler returns (`file_access_handler.rs:1337-1339`).
- Authorization is request-time: `check_permission` (`file_access_handler.rs:741-924`)
  checks structural validity, offer-enabled, blocked relationship
  (`file_access_handler.rs:813-826`), ring access, per-grant
  allow/deny from storage, then friend relationship before issuing a
  signed descriptor.

### 2.4 Gossip/net (`src/net.rs`, 2788 lines)

The core gossip subsystem; a different but instructive shape.

- `Gossip` (`net.rs:97-100`) derefs to `GossipApi`; `Inner` (`net.rs:152-159`)
  holds `api`, `local_tx`, and **`_actor_handle: AbortOnDropHandle<()>`** — the
  actor task is aborted when the last Gossip handle drops (`net.rs:156, 259`).
- `impl ProtocolHandler for Gossip` (`net.rs:161-167`) forwards the connection
  via `handle_connection` → `local_tx.send(LocalActorMessage::HandleConnection(conn))`
  (`net.rs:311-320`).
- `Builder::spawn(endpoint)` (`net.rs:223-265`): creates `Actor`, registers a
  `GossipAddressLookup` on the endpoint, `task::spawn(actor.run())`, returns
  `Gossip` with the AbortOnDropHandle stored in `Inner`.
- `Actor` (`net.rs:342-377`) is the event loop: a `tokio::select!` over
  `local_rx`, `rpc_rx`, a `stream_group` of per-topic command streams, endpoint
  address updates, and the dialer's connection stream (`event_loop`,
  `net.rs:464-537`). Connection read/write loops are tracked in a
  `JoinSet<(EndpointId, Connection, Result<(), ConnectionLoopError>>)`
  (`net.rs:369`).
- Cancellation is cooperative: `RecvLoop::run` (`net/util.rs:113-122`) selects
  on `conn.closed()` alongside concurrent read futures; when the connection
  closes, the loop drains and exits, forwarding a disconnect into the actor's
  `in_event_tx`.
- The shutdown path is a command with a oneshot reply (`Gossip::shutdown`,
  `net.rs:326-334`; `LocalActorMessage::Shutdown`, `net.rs:124-132`).
- Test-only accept-loop pattern with a `CancellationToken`:
  `endpoint_loop(endpoint, gossip, cancel)` (`net.rs:2040-2070`) — select on
  `cancel.cancelled()` vs `endpoint.accept()`, then `gossip.handle_connection`.

### 2.5 Tunnel (`src/tunnel.rs`, 1410 lines) — newest subsystem

`TunnelProtocol` (`tunnel.rs:421-451`) demonstrates connection-level admission
control with a semaphore (`active_connections.try_acquire_owned()`,
`tunnel.rs:423-429`), then loops `connection.accept_bi()` and spawns a
per-stream handler task. ALPN `BORU_TUNNEL_ALPN = b"/boru-tunnel/1"`
(`tunnel.rs:360-361`).

### 2.6 Registration summary (the Router)

All handlers are registered on one Router in `examples/iced_chat/main.rs:1014-1024`:

```rust
Router::builder(endpoint.clone())
    .accept(GOSSIP_ALPN, gossip.clone())
    .accept(iroh_blobs::ALPN, blobs_protocol.clone())
    .accept(FRIEND_PING_ALPN, PingHandler)
    .accept(BACKFILL_ALPN, backfill_handler)
    .accept(WHISPER_ALPN, whisper_handler)
    .accept(INBOX_ALPN, inbox_protocol)
    .accept(CATALOGUE_ALPN, catalogue_handler)
    .accept(FILE_ACCESS_ALPN, file_access_handler)
    .accept(BORU_TUNNEL_ALPN, tunnel_handler)
    .spawn();
```

The ALL_ALPNS test list in `net.rs:1793-1802` enumerates all of them, useful
for a call ALPN addition.

---

## 3. Iced GUI event flow — how async events reach the frontend

Files: `examples/iced_chat/main.rs` (2585 lines), `examples/iced_chat/app.rs` (52833 lines).

### 3.1 Startup wiring (main.rs)

Everything network-related is constructed inside one `runtime.block_on` closure
(`main.rs:743+`) and handed into the GUI:

- Endpoint with layered address lookups (mDNS + PkarrResolver + DnsAddressLookup
  + optional DHT) — `main.rs:755-855`. Relay `online()` is bounded by a 15 s
  timeout so a dead relay cannot hang startup (`main.rs:780-790`).
- Whisper: `WhisperBuilder::new` → `protocol_handler()` → `spawn()` →
  `(whisper_handle, whisper_events_rx)` (`main.rs:902-907`).
- Inbox: `InboxHandle::new()` → callback wiring → `InboxProtocol::new(inner)`
  (`main.rs:909-968`).
- Friends load (SQLite first, JSON fallback/migration) — `main.rs:970-984`.
- Catalogue + file-access handlers (`main.rs:990-1007`), tunnel service
  (`main.rs:1009-1012`).
- Router registration (`main.rs:1014-1024`, quoted above).
- Backfill actor: `BackfillHandle::spawn(endpoint)` (`main.rs:1316`).
- Friend ping manager: `FriendPingManager::spawn(endpoint, interval, timeout)`
  (`main.rs:1329-1333`), then all persisted friends registered via
  `add_friend_addrs(peer, record.known_addrs.clone())` (`main.rs:1341-1350`).
- Event receivers are wrapped in `Arc<Mutex<...>>` before entering the GUI —
  e.g. `whisper_events_rx` (`main.rs:1319`), `friend_events_rx`
  (`main.rs:1335`), `inbox_events_rx` (`main.rs:967`), `net_rx` (`main.rs:1325`).
  The GUI holds handles too: `whisper_handle.clone()`, `friend_mgr`,
  `backfill_handle`, `endpoint`, `gossip`, `blob_store`, `tunnel_service`.
- `IcedChat::new(...)` (`main.rs:1522-1563`) takes all of these; the
  constructor stores them on the app struct (`app.rs:7552-7591`).

### 3.2 Event → AppMessage bridge (the subscription stream)

`IcedChat::subscription(...)` (`app.rs:35640+`) returns an
`iced::Subscription<AppMessage>` composed of:

- time-based subscriptions (`app.rs:35650-35661`): ConnMonitorTick,
  MeshWatchdogTick, OutboxRetryTick, IdleTick, window resize;
- `iced::event::listen()` for file drag/drop and IME (`app.rs:35667-35700`);
- `subscription_stream(...)` (`app.rs:35467-35638`).

`subscription_stream` is the bridge that turns tokio channels into Iced
messages. It uses `n0_future::stream::unfold` with a state tuple of the
receiver Arcs, and inside one iteration locks **all** receivers with
`tokio::select!` over `rx_guard.recv()`, `friend_guard.recv()`,
`whisper_guard.recv()`, `inbox_guard.recv()`, `discovered_guard.recv()`,
`gui_action_guard.recv()`, `transfer_guard.recv()` (`app.rs:35521-35633`).
Each branch maps a channel item to an `AppMessage` variant:

- `AppMessage::NetEvent(ConversationNetEvent)` (`app.rs:35531`)
- `AppMessage::FriendEvent(FriendEvent)` (`app.rs:35544`)
- `AppMessage::WhisperEvent(WhisperEvent)` (`app.rs:35557`)
- `AppMessage::InboxEvent(InboxEvent)` (`app.rs:35570`)
- `AppMessage::NewDiscoveredPeers(...)` (`app.rs:35583`)
- `AppMessage::GuiTestActionReceived(...)` (`app.rs:35596-35599`)
- `AppMessage::TransferProjectionUpdate(...)` (`app.rs:35618`)

Per-channel `*_open` booleans disable closed channels instead of ending the
stream (`app.rs:35506-35512`), so an optional subsystem closing its sender
(inbox/discovery in headless launches) does not kill the whole subscription.

### 3.3 update() handlers

- `AppMessage::NetEvent(conv_event)` (`app.rs:15057+`): routes by topic into
  `ConversationLive` state, updates neighbor sets, presence, sidebar previews.
- `AppMessage::FriendEvent(event)` (`app.rs:15208-15212`) → `handle_friend_event`
  (`app.rs:24777-24843`): marks online/offline in `FriendsStore`, updates
  `peer_presence_map`, and on `AddressUpdated` records the addr
  (`app.rs:24836-24841`).
- `AppMessage::WhisperEvent(event)` (`app.rs:15214+`): the richest handler —
  verifies `SignedContactMessage::Control` payloads (friend requests /
  acceptances / conversation invites / address updates / mailbox ads),
  parses private-chat and group invites from `Message`, ignores empty DMs
  (connect probes), and drives `BackgroundSubscribe` on the deterministic
  private topic (`app.rs:15217-15593`).
- `AppMessage::InboxEvent(event)` (`app.rs:15738+`): accepts envelope /
  ack / sync / tombstone events from the inbox protocol.

**Key pattern for calls:** a new `CallEvent` channel should be added as another
Arc<Mutex<Receiver<...>>> field on `IcedChat`, another branch in
`subscription_stream` mapping to `AppMessage::CallEvent(...)`, and a handler in
`update()`.

---

## 4. Peer authorization lifecycle — block/unblock/friend/unfriend

Files: `src/friends.rs` (770 lines), `src/chat_core/friend_ping.rs` (1006 lines),
wiring in `main.rs` and `app.rs`.

### 4.1 The friends store

- `FriendRecord.relationship: FriendRelationship` (`friends.rs:186-187`), enum
  `NotFriend | Friends | Blocked` (deprecated pending variants reset on load)
  (`friends.rs:97-133`).
- `FriendRelationship::can_message()` returns true only for `Friends`
  (`friends.rs:135-140`). This is the **authorization predicate** used by
  inbox authorization and by the file-access permission check.
- `set_relationship(id, rel)` mutates and logs changes (`friends.rs:596-609`).
- Persistence: `save()` (atomic JSON, `friends.rs:441-452`) and
  `save_to_sqlite` / `load_from_sqlite` (`friends.rs:454+`, used at
  `main.rs:971-984`).

### 4.2 What happens on friend / unfriend / block / unblock

- **Add friend** (manual `/friend add <key> [alias]`, `app.rs:14027-14058`):
  `friend_mgr.add_friend(peer, addr)` starts ping tracking; the GUI also stores
  the relationship (e.g. `FriendRelationship::Friends` on accepted invites,
  `app.rs:15287`).
- **Remove friend** (`ConfirmRemoveFriend`, `app.rs:19902-19921`): calls
  `friend_mgr.remove_friend(&peer)` which stops ping tracking
  (`friend_ping.rs:309-312`). The inbox authorization callback re-loads the
  friends store on every connection, so the peer is immediately unauthorized
  for inbox traffic (`main.rs:1357-1372`).
- **Block friend** (`ConfirmBlockFriend`, `app.rs:19939-19952`): sets
  `record.relationship = Blocked` in the store and bumps the sidebar revision.
  Blocked peers are denied by:
  - file access: `check_permission` returns `PermissionDenied` for Blocked
    (`file_access_handler.rs:813-826`);
  - inbox: the authorization_fn predicate `can_message()` returns false for
    Blocked (`main.rs:1359-1371`);
  - the whisper denied-peers set if it were wired (see 4.3).
- **Unblock**: no dedicated UI path exists yet (grep for `Unblock` in
  `examples/` returns nothing); the mechanism would be `set_relationship(..., NotFriend)`
  or `Friends`, which the same authorization checks would honor on next request.

### 4.3 The two authorization mechanisms and the gap

1. **Whisper denied-peers set** (`WhisperHandle::set_peer_authorized`,
   `mod.rs:179-192`, plus `WhisperBuilder::with_denied_peers`, `mod.rs:356-362`).
   It rejects at the ProtocolHandler accept and revokes live connections. Tests
   prove the semantics (`mod.rs:894-953`). **However, the GUI never calls
   `set_peer_authorized`** — grep of `examples/iced_chat` for
   `set_peer_authorized` finds nothing. In the current app the deny set stays
   empty, so whisper admission is effectively "everyone"; application-level
   filtering happens in the GUI handlers.
2. **Inbox authorization_fn** (`main.rs:1357-1372`): a closure that re-loads
   `FriendsStore` from disk on every connection and checks
   `can_message()` + mailbox-key identity. This is the canonical "live
   authorization at request time" pattern — it requires no restart and no
   explicit revocation call.

**Recommendation for calls:** a call subsystem should use the same
request-time authorization pattern as inbox (authorization_fn / check on each
incoming call request), because voice/video calls are high-impact
privacy events; a stale snapshot is not acceptable.

---

## 5. Friend endpoint addresses → remote info / EndpointAddr

### 5.1 Persisted addresses

`FriendRecord.known_addrs: Vec<EndpointAddr>` — newest-first, deduped, capped
at `MAX_KNOWN_ADDRS = 5` (`friends.rs:33, 179-184`; `record_addrs`,
`friends.rs:227-243`). Addresses arrive from:

- `ContactAction::ConversationInvite { addrs }` — validated, persisted via
  `record.record_addrs(persisted_addrs)` (`app.rs:15321-15342`).
- `ContactAction::AddressUpdate { addrs }` — recorded (`app.rs:15405-15412`).
- `FriendEvent::AddressUpdated { addr }` from the ping manager
  (`app.rs:24836-24841`).

### 5.2 Seeding the ping manager and the endpoint

At startup, every friend's `known_addrs` is pushed into the ping manager:
`friend_mgr.add_friend_addrs(peer, addrs)` (`main.rs:1341-1350`).
The endpoint itself learns addresses through:

- mDNS / Pkarr / DNS / DHT lookup layers registered at build time
  (`main.rs:755-855`);
- `GossipAddressLookup::with_friends` when the gossip builder is given the
  friends store (`net.rs:216-220`, `net.rs:225-228`).

### 5.3 remote_info → EndpointAddr at connect time

Three concrete construction sites, all equivalent:

1. **Whisper** `get_or_connect` (`mod.rs:559-585`):
   ```rust
   let info = endpoint.remote_info(*peer).await;
   // Some(info) => EndpointAddr { id: *peer, addrs: info.addrs().map(|a| a.addr().clone()).collect() }
   // None       => EndpointAddr::new(*peer)   // ID-only, resolves via lookup chain
   ```
2. **Backfill** `try_backfill_from_peer` (`backfill.rs:492-496`):
   `EndpointAddr::from_parts(peer, info.into_addrs().map(|addr| addr.into_addr()))`.
3. **Friend ping** `resolve_addrs` (`friend_ping.rs:393-418`): merges cached
   `state.addrs` + `endpoint.remote_info(peer)` addrs, then appends
   `EndpointAddr::new(peer)` as the ID-only fallback. `try_connect` then splits
   the transport addrs into per-address candidates via
   `EndpointAddr::from_parts(peer, [transport])` (`friend_ping.rs:426-435`).

**Pattern:** always end with an ID-only `EndpointAddr::new(peer)` fallback so
the endpoint's lookup chain (DHT/mDNS/DNS) can resolve the peer even when no
cached transport address exists.

---

## Patterns to reuse for calls

Concrete recommendations for `CallBuilder` / `CallProtocol` / `CallHandle` /
`CallManager`, modeled on whisper and the other subsystems.

### A. Module layout and types

- New module `src/call.rs` (or `src/call/` with `mod.rs` + `session_manager.rs`),
  feature-gated `#[cfg(feature = "net")]` in `lib.rs` like whisper
  (`lib.rs:228-230`).
- `pub const CALL_ALPN: &[u8] = b"/boru-call/1";` — add to the ALL_ALPNS test
  list (`net.rs:1793-1802`).
- Wire enum `CallWireMessage` with postcard + 4-byte LE length framing, copied
  from `WhisperWireMessage` (`mod.rs:53-69`, `write_framed_message`
  `mod.rs:635-662`). For real-time media this is the control plane; media
  (RTP/WebRTC-like) would ride separate QUIC streams or datagrams, but the
  signaling frames follow this exact shape.

### B. CallBuilder / CallHandle (mirror whisper's actor)

- `CallBuilder::new(endpoint: Endpoint, secret_key: SecretKey)` creates the
  command channel and stores `cmd_rx: Option<mpsc::Receiver<Cmd>>`
  (mirror `mod.rs:332-353`).
- `protocol_handler(&self) -> CallProtocol` sharing `cmd_tx` + the
  authorization set (mirror `mod.rs:368-373`).
- `spawn(self) -> (CallHandle, mpsc::Receiver<CallEvent>)` with an
  `Arc<Mutex<HashMap<PublicKey, Connection>>>` for active call connections and
  `tokio::task::spawn(run_actor(...))` (mirror `mod.rs:376-393`).
- `CallHandle`: cloneable, `cmd_tx` + oneshot replies; methods like
  `start_call(peer, addr)`, `accept_call(peer)`, `end_call(peer)`,
  `set_peer_authorized(peer, bool)` (mirror `mod.rs:168-290`).
- `CallEvent`: `IncomingCall { from }`, `CallAccepted`, `CallEnded`,
  `Connected`, `Disconnected` — consumed by the GUI like `WhisperEvent`
  (`mod.rs:74-115`).

### C. CallProtocol (ProtocolHandler)

- `impl ProtocolHandler for CallProtocol` checks authorization at accept time
  and forwards `Connection` to the actor via `Cmd::IncomingConnection`
  (mirror `mod.rs:294-327`). For calls, prefer the **request-time
  authorization_fn pattern from inbox** (`main.rs:1357-1372`) — a closure that
  re-loads the friends store — so block/unfriend takes effect immediately
  without an explicit revocation call.

### D. CallManager (state machine)

- The `SessionManager` (`session_manager.rs`) is the natural blueprint: a
  per-peer `CallSession` with `Idle | Ringing | Connecting | InCall | Ended`
  states, an event channel of `CallEvent::StatusChanged { peer, state }`
  (`session_manager.rs:47-83`), exponential backoff for reconnect
  (`BACKOFF_BASE`/`BACKOFF_MAX`, `session_manager.rs:32-39`), and the
  deterministic collision-resolution rule (lower key keeps outgoing,
  `session_manager.rs:317-335`).
- **Decide deliberately:** either wire the manager into the GUI (call
  `start_session`/`stop_session`/`notice_call_event` from `update()` like
  the friend ping manager), or drop it. The whisper `SessionManager` is
  currently dead code (`docs/telepathy-room-dial-audit.md:229-236`) — do not
  replicate that mistake. Call sessions must be driven by real user actions
  (accept/end) rather than a silent reconnect loop, though a bounded
  re-invite/retry with the same backoff constants is reasonable for
  "call dropped, reconnecting".

### E. GUI wiring

- Add `call_events_rx: Arc<Mutex<Receiver<CallEvent>>>` and
  `call_handle: CallHandle` fields to `IcedChat` (`app.rs:3775-3780` shows the
  whisper analogues), constructed in `main.rs` inside the `runtime.block_on`
  block and passed through `IcedChat::new` (`main.rs:1522-1563`).
- Register `CallProtocol` on the Router in `main.rs:1014-1024`.
- Add a branch to `subscription_stream` mapping
  `call_guard.recv()` → `AppMessage::CallEvent(...)` with its own `*_open`
  flag (`app.rs:35467-35638`), plus a handler in `update()`.
- Incoming call notification should reuse the `push_system` / toast patterns
  (`app.rs:15480`, `app.rs:19948`) and a modal accept/decline dialog (the
  existing `boru_dialog` / Card overlay patterns).

### F. Cancellation, timeouts, and resource bounds

- Bound every connect with `tokio::time::timeout` (whisper 15 s,
  `mod.rs:604-609`; backfill `BACKFILL_REQUEST_TIMEOUT`,
  `backfill.rs:651-664`; endpoint `online()` 15 s, `main.rs:780-790`).
- Cooperative cancellation via `conn.closed()` in reader loops
  (`net/util.rs:113-122`, `file_access_handler.rs:1337-1339`).
- Admission control via semaphores (backfill `backfill_semaphore`,
  `backfill.rs:189-201`; tunnel `active_connections`, `tunnel.rs:423-429`) —
  use one for concurrent call sessions (e.g. max 1 active call, N ringing).
- Identity check after connect (`conn.remote_id() == peer`,
  `mod.rs:610-618`).
- Emit `Disconnected` events on clean shutdown so the GUI never shows a
  stuck "in call" state (`mod.rs:529-534`).

### G. Security defaults for calls

- Deny by default at the application layer; authorize only friends with
  `can_message()` (the inbox authorization_fn pattern), and re-check at
  request time — never trust a startup snapshot.
- Keep media-plane details out of logs: no payload dumps, short peer ids
  (`fmt_short()`), no local paths (see `access_diag`, `file_access_handler.rs:704-724`).
- Rate-limit incoming call requests (mirror `BackfillRateLimit` /
  `UploadLimiter` patterns) to avoid call-spam resource exhaustion.
