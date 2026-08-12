# BORU-DISC-27: Mixed-version discovery — old/new peer dedup verification (PDF T24)

PDF Phase 7 task 24: *"If compatibility with the old lobby is implemented, test
old/new peer discovery and deduplication. No duplicate peer records or
duplicate UI entries occur."*

The condition — *"if compatibility with the old lobby is implemented"* — was
resolved **no** by BORU-DISC-16 (`docs/discovery-refactor/16-old-lobby-transition.md`):
Boru ships **no compatibility gossip listen on the old lobby topic**. Per the
task scope, this task is therefore **documentation-only**: no test code was
added. This document records why, and what mechanism covers mixed-version
discovery and duplicate-peer risk today.

## 1. Decision from BORU-DISC-16 (recap)

- **No compat gossip listen** on the legacy lobby topic
  (`public_lobby_topic(PublicNetwork::Mainnet)`). The old lobby stays a
  Conversation-kind topic and is never subscribed at startup, never exposed in
  the UI, and never joined by any discovery service.
- Mixed-version discovery — if it is ever required — is served by the existing
  **DHT member-discovery layer** (`PublicRoomTracker` /
  `PublicContinuousTracker` on `public_discovery_key(Mainnet, "public-lobby",
  1)`, the canonical lobby discovery key), **not** by a gossip subscription on
  the old lobby topic.
- The DHT layer is **topic-independent**: it is a DHT namespace (peer presence
  records keyed by the canonical lobby identity), not the gossip topic, so it
  produces zero chat payloads and zero gossip-topic subscription state.

## 2. Why no mixed-version test was added

The PDF's test scenario simulates "an old peer announcing on the legacy lobby
topic AND a new peer announcing on the internal discovery topic, and asserts
dedup yields ONE peer record / UI entry." That scenario presumes a compat
listen exists to receive the old peer's legacy-lobby announcement. It does not:

- A Boru node joins **exactly one** gossip discovery topic at startup — the
  internal discovery topic (`discovery_topic(network)`,
  `BORU_DISCOVERY_TOPIC_V1`), via `DiscoveryService::join` in
  `examples/iced_chat/main.rs` (BORU-DISC-08/12/21). The only other startup
  gossip subscriptions are the directory topic (BORU-DISC-01, unchanged) and
  user-facing conversation topics.
- The legacy lobby topic is never subscribed — verified by
  `tests/test_discovery_startup.rs` (startup joins discovery, creates no
  conversation) and the grep audit in §5. There is no second gossip source
  that could announce the same peer, so the "one peer on two topics" duplicate
  scenario cannot arise from the gossip layer.

A test that fabricates a legacy-lobby announcement would be testing a path that
does not exist in the product — a false positive harness, not a regression
guard. That is why scope item 2 directs a documentation task instead.

## 3. The actual dedup mechanisms

Even though only one discovery topic is joined, dedup is enforced at every
layer that *could* see a peer from multiple sources:

1. **Peer registry dedup (BORU-DISC-17)** — `DiscoveryService`'s
   `PeerRegistry` maps `node_id` → last-seen/source-topic metadata and is the
   dedup anchor: a node already registered is not re-announced as new. Dedup is
   keyed by `(node_id, event_id)`:
   - by node identity — the map key itself, so the same peer discovered on two
     paths (e.g. internal discovery topic + a hypothetical compat path that
     forwards the same advertisement) occupies a single entry;
   - by event id — re-delivering the same event (same node, same id) leaves the
     registry untouched (`UpsertOutcome::Duplicate`);
   - legacy senders (no event id on the wire) always refresh, never
     deduplicated — preserving BORU-DISC-06 behaviour.
2. **Connectivity-loop dedup** — the wiring task that dials newly seen peers
   into the discovery mesh deduplicates by endpoint id
   (`connectivity_loop_deduplicates_peer_dials` test in
   `src/discovery_service.rs`), so even if a peer surfaced through both the
   gossip mesh and the DHT joiner it is dialed once.
3. **UI-level separation** — discovery state never reaches conversation or
   rendering state (BORU-DISC-13/14/25: `topic_kind()` routing guard,
   `tests/test_discovery_ui_isolation.rs`). No discovery event can create a
   duplicate UI entry because discovery events never create UI entries at all.

## 4. Mixed-version discovery: how old and new peers meet

Old-version Boru nodes publish presence to the **same** canonical lobby
discovery key (`public_discovery_key(Mainnet, "public-lobby", 1)` — MUST-KEEP
per `docs/compatibility-identifiers.md` §4, so old and new derive the same
key), every 5 minutes via their `PublicRoomTracker` publish loop. A new node
that runs a tracker on that key discovers old peers' `EndpointId`s and joins
them into the mesh via `DynamicPeerJoiner` / the `DiscoveryService`
connectivity loop — the same plumbing mDNS and per-room DHT discovery use
today. This is **lobby-independent** (a DHT namespace, not the gossip topic)
and produces **zero chat payloads**. The mechanism is integration-tested
without a live DHT by `tests/test_public_lobby_integration.rs` (8 tests,
`InMemoryDiscoveryBackend`).

Because the DHT member-discovery layer is topic-independent, mixed-version
discovery does not depend on which gossip topics each version subscribes to —
the two versions rendezvous on the shared discovery key, not on the old lobby
gossip topic. And because a node joins only one gossip discovery topic, there
is no duplicate-peer risk from topic overlap: the DHT path feeds the same
`PeerRegistry`/connectivity dedup as the gossip path.

## 5. Verification audit (no code shipped)

| Check | Command / evidence | Result |
|---|---|---|
| BORU-DISC-16 decision | `docs/discovery-refactor/16-old-lobby-transition.md` | No compat gossip listen; DHT is the compat path |
| Startup joins only internal discovery topic | `examples/iced_chat/main.rs` `DiscoveryService::join(discovery_topic(...))` (BORU-DISC-08/12) | Only discovery topic + directory topic subscribed at startup |
| No legacy-lobby gossip subscribe anywhere | `grep -rn "public_lobby_topic\|legacy_lobby_topic" src/ examples/iced_chat/` (non-test, non-derivation) | Only `lobby_migration.rs` (persistence prune), `backfill.rs` (guards), `directory.rs` (reference) — no subscription |
| Dedup anchor | `src/discovery_service.rs` `PeerRegistry` keyed by `(node_id, event_id)` (BORU-DISC-17) | Duplicate events → `UpsertOutcome::Duplicate`, single record |
| No duplicate UI entries | `tests/test_discovery_ui_isolation.rs` (3 tests, BORU-DISC-25) | Discovery payloads never render as chat |
| Single-topic discovery regression | `tests/test_discovery_startup.rs` (4 tests, BORU-DISC-21) | One discovery subscription, zero conversations created |

No `Cargo.toml` change, no `tests/test_discovery_mixed_version.rs`, no runtime
code. The mixed-version test from PDF T24 is intentionally not added; if a
future protocol bump (BORU_DISCOVERY_PROTOCOL_VERSION) or an explicit compat
requirement ever introduces a second discovery source, revisit this task: add
the dual-source dedup test then, keyed on the `PeerRegistry` anchor above.

## 6. Guardrail compliance

- **Deterministic topic derivation unchanged** — no direct/groups/derivations
  touched; the internal discovery topic derivation is untouched.
- **Discovery state not merged with conversation state** — no change; discovery
  stays in `DiscoveryService`/`PeerRegistry`.
- **No hidden chat object** — no code added at all.
- **Public chat creation/joining explicit** — unchanged.
- **Hard rule** — private direct messages and normal chat payloads never routed
  through the discovery topic; no compat listen means no legacy-lobby chat
  payloads can be misclassified as discovery (the exact risk BORU-DISC-16 §2.2
  documents).
