# iroh API and feature-usage audit

Date/source baseline: current worktree on `main`, Cargo.lock-resolved `iroh 1.0.0` and package version `iroh-gossip 0.101.0`. The live documentation was fetched from https://docs.iroh.computer/ (the docs sitemap reports 2026-07-10 updates). This is an audit only; no production fixes are included.

## Executive summary

The core iroh-gossip integration follows the documented model: create an `Endpoint`, spawn `Gossip`, register it on an iroh `Router` under the gossip ALPN, create a 32-byte `TopicId`, subscribe with endpoint-ID bootstrap peers, and shut down the router before the endpoint. The application-specific room ticket is also a valid documented pattern: it packages a topic identifier plus `EndpointAddr` values rather than pretending to be an iroh endpoint ticket.

One actionable correctness issue was found in the TUI startup path: `examples/chat.rs` calls `subscribe_and_join` even when opening a room with no bootstrap peers. The documented `joined()` operation waits for an active connection, so a room creator with an empty peer list blocks before its UI can start. The GUI already avoids this by using `subscribe` for an empty bootstrap list.

A second issue is lifecycle/operational rather than an iroh API mismatch: the custom backfill actor and friend/whisper tasks are spawned independently of the router and do not expose an explicit shutdown/abort path. Router and endpoint shutdown are present, but these tasks rely on runtime teardown. This should be hardened if clean shutdown and deterministic test teardown are requirements.

The current uncommitted worktree does not compile independently of this audit: `cargo test --features net --lib --no-run` fails in `src/net/address_lookup.rs` because `Mutex` and `FriendsStore` are not imported and `GossipAddressLookup` initialisation omits its new `friends` field. This is recorded as a verification limitation, not attributed to an iroh API change.

## Findings

### F-01 — Incorrect: TUI creator blocks on `subscribe_and_join` with no peers

Evidence:

- `examples/chat.rs:274-307` opens/reuses a room and permits `peers = []`.
- `examples/chat.rs:537-549` unconditionally calls `gossip.subscribe_and_join(topic, peer_ids.clone())`.
- `src/api.rs:114-124` implements `subscribe_and_join` as `subscribe_with_opts(...); sub.joined().await?`; `joined` waits for at least one active connection.
- `src/api.rs:127-141` documents that `subscribe` returns without waiting.
- `examples/chat.rs:606-607` says an empty-peer creator should be “Waiting for peers to join us...”, which is unreachable until a peer already exists.
- The GUI has the correct split behavior at `examples/iced_chat/app.rs:1221-1230`: `subscribe` when bootstrap is empty, `subscribe_and_join` otherwise.

Why it matters: creating a new room with `cargo ... open` can hang before the TUI event loop starts. The room creator must be able to subscribe and wait for inbound joins.

Minimal recommendation: mirror the GUI branch in the TUI: use `gossip.subscribe(topic, peer_ids)` when `peer_ids.is_empty()`, and use `subscribe_and_join` only when there are bootstrap peers. If the intended UX is a bounded join wait, wrap the non-empty branch in an explicit timeout and keep the subscription alive for later retries.

Focused verification: `cargo test --features examples --test stale_bootstrap` (after the current compile errors are fixed), plus a two-process smoke test where the opener starts with no peers and the joiner connects later. A focused unit-level regression can assert that the empty-peer path returns a `GossipTopic` without awaiting `joined()`.

Relevant docs: https://docs.iroh.computer/connecting/gossip.md (subscribe/join example and `receiver.joined()` semantics); https://docs.iroh.computer/examples/chat.md (topic creation and subscription).

### F-02 — Ambiguous / operational: stale bootstrap addresses can wait indefinitely

Evidence:

- `examples/chat.rs:546-549` uses `subscribe_and_join` for all non-empty bootstrap lists.
- `examples/iced_chat/app.rs:1448-1451` likewise intentionally waits for a neighbor on ticket joins.
- The documented contract only says `joined()` waits until at least one endpoint is connected; it does not provide a timeout or guarantee that a stale endpoint will fail promptly.
- The application persists `EndpointAddr` values in `src/room.rs:43-48`, refreshes them after a join in `src/chat_core.rs:88-115`, and has a stale-bootstrap test, so this is partly mitigated.

Classification is ambiguous rather than an API violation: waiting indefinitely may be an intentional “join until online” UX, but it can make a dead ticket or all-offline room appear hung.

Minimal recommendation: add an application-level timeout around the initial `joined()` wait, surface a retryable error, and retain the topic/subscription for later `join_peers` or a fresh subscription. Do not replace `EndpointAddr` persistence with endpoint IDs alone unless the desired behavior is to rely on the default DNS/Pkarr lookup.

Focused verification: run `cargo test --features net,test-utils --test stale_bootstrap`; add a test with an unreachable bootstrap address asserting the UI returns within the chosen timeout.

Relevant docs: https://docs.iroh.computer/connecting/gossip.md; https://docs.iroh.computer/concepts/tickets.md (tickets can go stale; long-lived connections should prefer endpoint IDs and runtime lookup).

### F-03 — Correct: topic identity and single-room persistence

Evidence:

- `src/proto/mod.rs`/`src/proto/topic.rs` use a 32-byte `TopicId`.
- `examples/chat.rs:295-303` creates a random `TopicId` for a new room and `examples/chat.rs:283-293` reuses the persisted topic.
- `examples/iced_chat/main.rs:318-338` does the same for the GUI.
- `src/room.rs:33-48` persists the topic and bootstrap addresses.
- `examples/chat.rs:546-549` and GUI subscription code pass the same topic to gossip.

This matches the current docs: a topic is a shared 32-byte identifier; an application may choose random or stable/hash-derived values. The single-room scope is preserved and is not itself a correctness problem.

Relevant docs: https://docs.iroh.computer/connecting/gossip.md#picking-a-topic-id and https://docs.iroh.computer/examples/chat.md.

### F-04 — Correct: bootstrap peer IDs are separated from address material

Evidence:

- `src/chat_core.rs:43-62` deduplicates `EndpointAddr` inputs into endpoint IDs for gossip and full addresses for lookup seeding.
- `examples/chat.rs:526-549` merges ticket and persisted addresses, seeds `MemoryLookup`, then passes only `peer_ids` to `subscribe_and_join`.
- `examples/iced_chat/app.rs:1217-1230` performs the same sequence for joins.

This is the correct distinction for the current gossip API: the subscription accepts `Vec<EndpointId>`, while the endpoint address lookup needs `EndpointAddr` data to resolve/dial those IDs. The docs describe tickets as carrying endpoint ID, relay, and direct addresses, and address lookup as the EndpointID-to-dialable-address layer.

Relevant docs: https://docs.iroh.computer/concepts/address-lookup.md and https://docs.iroh.computer/concepts/tickets.md.

### F-05 — Correct: custom room ticket is application-specific, not an invalid replacement for EndpointTicket

Evidence:

- `src/chat_core.rs:928-965` defines `Ticket { topic: TopicId, peers: Vec<EndpointAddr> }`, postcard/base32 encodes it, and decodes it symmetrically.
- `examples/chat.rs:493-497` creates it from the current topic and `local_peer_addr`.
- `examples/chat.rs:309-313` and GUI `examples/iced_chat/main.rs:343-353` parse it and recover both topic and peers.

The current iroh ticket docs explicitly say tickets are optional and application-specific ticket types may package content identifiers alongside endpoint addresses. Therefore using a local room ticket is correct. It is not wire-compatible with `iroh_tickets::endpoint::EndpointTicket`, but there is no requirement for that interoperability because the application needs the room topic as well.

Security/operational note: the ticket intentionally exposes the current direct addresses and relay information to whoever receives it. That is documented iroh behavior and should remain visible in the UI/help text.

Relevant docs: https://docs.iroh.computer/concepts/tickets.md.

### F-06 — Correct: Endpoint presets, relays, and address lookup composition

Evidence:

- `examples/chat.rs:394-445` uses `presets::N0` for default relays/discovery and `presets::N0DisableRelay` when relays are disabled, then applies the selected `RelayMode`.
- `examples/iced_chat/main.rs:453-492` follows the same pattern.
- Both add `MemoryLookup` for ticket/persisted addresses and mDNS/DHT lookups as optional supplemental services (`examples/chat.rs:457-483`; GUI `examples/iced_chat/main.rs:498-525`).
- The current mDNS docs show adding mDNS on top of `presets::N0`, and the DNS docs identify `N0` as the default DNS/Pkarr lookup preset.

No outdated Endpoint/Relay API usage was found. The repeated builder code is verbose but semantically consistent with the current documentation. `RelayMode::Disabled` for Tor mode is deliberate because Tor is installed as a custom transport.

Relevant docs: https://docs.iroh.computer/connecting/creating-endpoint.md, https://docs.iroh.computer/connecting/dns-address-lookup.md, https://docs.iroh.computer/connecting/local-address-lookup.md, https://docs.iroh.computer/concepts/relays.md, and https://docs.iroh.computer/transports/tor.md.

### F-07 — Correct: protocol registration and ALPN routing

Evidence:

- `examples/chat.rs:518-524` registers gossip, blobs, friend ping, whisper, and backfill handlers on one `iroh::protocol::Router`.
- `examples/iced_chat/main.rs:556-562` does the equivalent.
- `src/net.rs:129-141` implements `ProtocolHandler` for `Gossip` and its shutdown hook.
- `src/backfill.rs:175-176` and `src/whisper/mod.rs` implement protocol handlers for their respective ALPNs.

This matches the current iroh protocol model: the Router dispatches incoming connections by ALPN and should be shut down cleanly. No ALPN collision or direct accept-loop mismatch was found.

Relevant docs: https://docs.iroh.computer/connecting/gossip.md and https://docs.iroh.computer/concepts/protocols.md.

### F-08 — Correct: shutdown ordering for Router and Endpoint; ambiguous for auxiliary tasks

Evidence:

- TUI: `examples/chat.rs:1172-1174` awaits `router.shutdown()` then `endpoint.close()`.
- GUI: `examples/iced_chat/main.rs:699-704` explicitly awaits `endpoint.close()` after GUI teardown.
- `src/net.rs:251-263` implements gossip shutdown by leaving topics and sending disconnects.
- `src/backfill.rs:336-339` and friend/whisper managers spawn independent tasks; no corresponding application-wide shutdown call is visible in the inspected paths.

Router/endpoint ordering is correct and agrees with the docs' clean-shutdown guidance. Auxiliary task lifetime is ambiguous: runtime teardown will eventually drop them, but deterministic cancellation is preferable and avoids relying on process/runtime shutdown to stop background work.

Minimal recommendation: add explicit shutdown/cancellation handles for backfill, friend ping, whisper, and Tor monitor tasks, and invoke them before endpoint close. Keep the existing router-before-endpoint ordering.

Focused verification: run the relevant e2e tests under `RUST_LOG=warn` and assert no background-task or “endpoint dropped without calling close” diagnostics after the process exits.

Relevant docs: https://docs.iroh.computer/protocols/using-quic.md#closing-connections and https://docs.iroh.computer/connecting/gossip.md.

## Verification record

Commands run:

- `grep`/source inspection of `Cargo.toml`, `Cargo.lock`, `src/`, `examples/`, and `tests/`.
- Live HTTP fetches of the linked iroh documentation pages and `https://docs.iroh.computer/llms.txt`.
- `cargo test --features net --lib --no-run` — **failed before tests** due current uncommitted errors in `src/net/address_lookup.rs`: missing `Mutex`/`FriendsStore` imports and missing `friends` field initialisation. The compiler also emitted an unrelated `rpc` cfg warning.

No production source fixes were made for this audit.
