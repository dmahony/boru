# BORU-DISC-16: Old-lobby transition decision — no gossip compat listen; DHT member discovery is the compat path

Audit scope: committed worktree (`wt/t_632374de`) after `git fetch origin && git merge origin/main`
(origin/main @ 6cf16840, BORU-DISC-15). Phase 5 of the hidden-discovery refactor (PDF T14):
**decide how to handle the OLD lobby topic (`public_lobby_topic(PublicNetwork::Mainnet)`)
during the transition so that mixed-version peers can still discover one another — if
compatibility is required — without ever exposing the old lobby in the UI.**

This document records the decision, the rationale, and the transition window. **No code
changes are needed** for this task — the decision is option (b) from the task brief.

---

## 1. Decision

**Do NOT implement a compatibility gossip listen on the old lobby topic.** The old lobby
stays a Conversation-kind topic (unchanged, pinned by BORU-DISC-14's
`topic_kind_lobby_is_conversation`), is never exposed in the UI (already true since
BORU-DISC-12/15), and mixed-version discovery — **if** it is ever required — is served by the
existing DHT member-discovery layer (`PublicRoomTracker` / `PublicContinuousTracker` on the
canonical lobby discovery key), **not** by a gossip subscription on the old lobby topic.

This is option (b) from PDF T14 / the task body:

> rely on the existing DHT member-discovery layer for mixed-version discovery and document
> why no extra listen is needed.

Acceptance criterion from PDF T14: mixed-version peers can still discover one another during
the chosen transition period **if compatibility is required**, and the old lobby is never
exposed in the UI. Both halves hold:

- compatibility is **not** required for this project (no stated requirement; see §2.1), so
  the "if required" clause is not triggered — and the documented compat mechanism exists and
  is integration-tested for the day it is;
- the old lobby is **never** in the UI (no startup join since BORU-DISC-12, no user-facing
  "lobby" strings since BORU-DISC-15).

## 2. Rationale

### 2.1 Compatibility is not required

PDF T14 is explicitly conditional: *"optionally listen to the old lobby **only if needed** to
discover older Boru versions"* and *"IF compatibility is required"*.

- There is **no stated requirement** for old-version compatibility: the user's preference is
  normal Tor bootstrap and gating chat until Tor is ready; there is no deployed fleet of
  old-version nodes that must interoperate; the old auto-joined lobby is precisely the
  feature this refactor chain (BORU-DISC-01..15) is replacing.
- The internal discovery topic is already versioned
  (`BORU_DISCOVERY_PROTOCOL_VERSION`, `src/discovery_topic.rs`). Protocol evolution is
  handled by bumping the version byte, which deliberately changes the derived topic —
  mixed-version rendezvous is a versioned-protocol concern, matching the project's
  identifier-compat policy (`docs/compatibility-identifiers.md`: keep MUST-KEEP identifiers;
  dual-stack only for an *intentionally versioned* protocol).

### 2.2 The old lobby topic is a chat topic — unsafe as a discovery source

In old Boru versions the lobby topic carried **signed chat payloads** (`Message::Text`,
`Message::ImageShare`, presence, neighbor traffic) — see
`docs/discovery-refactor/00-architecture-note.md` §2 ("Outbound send + persistence for
lobby ... reachable", "Lobby traffic reaches every chat stage today").

A compat listen that classifies the legacy lobby topic as discovery/legacy would route
normal chat payloads onto the discovery path — directly violating the refactor's **hard
rule**:

> private direct messages and normal chat payloads must NEVER be routed through the
> discovery topic.

A routing guard would drop those payloads as undecodable (the DiscoveryService receive path
already does this), so it would be *safe in practice* — but the architecture would then be
literally carrying chat payloads on a topic treated as discovery infrastructure, a semantic
contradiction and a trap for future maintainers.

### 2.3 Reclassification would break explicit public-chat separation

`topic_kind()` is the Phase-4 routing guard applied at the forwarder-spawn boundary and at
`AppMessage::NetEvent`. Reclassifying the canonical lobby as Discovery/Legacy would make
`OpenRoom`/`RoomOpened` **refuse** the lobby topic (BORU-DISC-13 guards), so an explicit
user action — `boru open <lobby-topic>`, or the MCP diagnostic — could no longer open it.
That contradicts the guardrail *"keep public chat creation/joining explicit and
user-facing"* and undoes BORU-DISC-14's core separation (public rooms are ordinary
Conversation topics; the discovery topic is a *different derivation*).

### 2.4 The DHT member-discovery layer already covers mixed-version discovery

- Old peers publish presence to `public_discovery_key(Mainnet, "public-lobby", 1)` under
  `canonical_lobby_key(...)` every 5 minutes (the `PublicRoomTracker` publish loop — the
  identity is MUST-KEEP per `docs/compatibility-identifiers.md` §4, so old and new nodes
  derive the **same** key).
- A node that runs a `PublicRoomTracker`/`PublicContinuousTracker` on that key discovers
  old peers' `EndpointId`s and joins them into the mesh via `DynamicPeerJoiner` / the
  `DiscoveryService` connectivity loop — the same plumbing mDNS and per-room DHT discovery
  use today.
- This is **lobby-independent** (a DHT namespace, not the gossip topic) and produces **zero
  chat payloads** — exactly the "legacy-discovery source" the PDF describes, but over DHT
  where it is safe. The mechanism is integration-tested without a live DHT by
  `tests/test_public_lobby_integration.rs` (8 tests, `InMemoryDiscoveryBackend`, peers
  discover each other without tickets).

## 3. Transition window

| Aspect | Window |
|---|---|
| Gossip compat listen on the old lobby topic | **Never** (recommended). No transition-period gossip subscription to the old lobby topic will be added. |
| Canonical lobby identity (topic + discovery key) | MUST-KEEP and Conversation-kind for as long as the public-room identity derivation is supported — through the v0.200.x line and until an intentional protocol bump (`BORU_DISCOVERY_PROTOCOL_VERSION` / public-room `PROTOCOL_VERSION`) changes the derivation. |
| If an old-version compat requirement ever appears | Start a compat `PublicRoomTracker` on `public_room_identity(PublicNetwork::Mainnet)` (the canonical lobby key), gated behind this same window; wire discovered `EndpointId`s into `DynamicPeerJoiner`/`DiscoveryJoiner`. No gossip listen, no reclassification, no UI exposure. |

### Compat-path recipe (if ever required)

```rust
// In main.rs startup, when old-version compat is enabled (documented transition gate):
let dht = distributed_topic_tracker::Dht::new(&Default::default());
let tracker = PublicRoomTracker::start(
    Box::new(dht),
    PublicNetwork::Mainnet,   // canonical lobby key = public_discovery_key(Mainnet, "public-lobby", 1)
    local_endpoint_id,
    secret_key,
).await?;
// loop: tracker.publish_once() every 5 min; tracker.discover_once() every 30 s
// -> discovered EndpointIds -> DynamicPeerJoiner::discovery_tx / DiscoveryService joiner
```

No conversation state, no UI, no gossip subscription on the old lobby topic.

## 4. What stays as-is (guardrail compliance)

- **`topic_kind()`**: the canonical lobby remains `Conversation`-kind (pinned by the
  `topic_kind_lobby_is_conversation` test in `src/discovery_topic.rs`). No classification
  change shipped.
- **Startup**: no subscription to the old lobby topic (already true since BORU-DISC-12;
  `main.rs` joins only the internal discovery topic, the directory topic, and mDNS).
- **UI**: the old lobby is never exposed — no sidebar row, no mesh text, no unread count
  (BORU-DISC-12/15).
- **MCP `boru_join_lobby_room`** (`mcp_server.rs:2439-2498`): uses the **stale literal**
  `b"iroh-gossip-chat/default-lobby/v1"` — a *different* topic from the canonical lobby. It
  is a Conversation-kind diagnostic id and stays untouched (BORU-DISC-20 diagnostics scope).
- **Deterministic derivations**: unchanged (guardrail: do not change the deterministic topic
  derivation unless demonstrably wrong).
- **Discovery state vs conversation state**: unchanged — DiscoveryService owns the internal
  discovery topic only; the old lobby remains conversation-owned.

## 5. Evidence

| Check | Command | Result |
|---|---|---|
| Lib compile (net) | `rb check --lib --features net` | PASS (exit 0) |
| Lib unit (classifier + identity) | `rb test --lib --features net -- discovery_topic public_room` | PASS (incl. `topic_kind_lobby_is_conversation`, known-answer discovery keys) |
| DHT member-discovery integration | `rb test --features net --test test_public_lobby_integration` | 8/8 PASS (compat-path proof) |

No code changes shipped in this task; the decision is recorded here and cross-referenced
from the refactor doc chain.

## 6. References

- `docs/discovery-refactor/00-architecture-note.md` §5.1 decision 1 — *"classify the old
  lobby topic as Discovery during a transition **if mixed-version compatibility is
  required**"* — this task resolves: **not required**; DHT is the documented mechanism.
- `docs/discovery-refactor/14-public-chats-explicit.md` — the canonical lobby stays
  Conversation-kind; public rooms are explicit user features.
- `docs/compatibility-identifiers.md` §4 — lobby identity inputs (`PUBLIC_ROOM_NAME`,
  `PROTOCOL_VERSION`, network bytes, domain separators) are MUST-KEEP; kept unchanged.
- `docs/protocol-layers.md` "Known Limitations" — no always-on startup lobby tracker; DHT
  trackers are per user-created room (BORU-DISC-12/14).
- `docs/plans/public-rooms-hybrid-registry.md` — tracker-per-user-room model that the DHT
  compat path reuses.
- `tests/test_public_lobby_integration.rs` — DHT publish/discover proof for the canonical
  lobby key.
