# Security Metadata & Authorization Review (BORU-SEC-001)

Status: review + documentation (no production behaviour changes).
Task: BORU-ARCH-39, Phase 8 (Security and Protocol Review) of the BORU-ARCH chain.
Scope: a focused metadata-and-authorization review of the post-refactor
architecture. It traces what metadata is visible on each wire path, verifies
that discovery/connectivity never implicitly grants friendship, room
membership or conversation authorization, verifies that room discovery cannot
expose private-room membership or identifiers unintentionally, verifies that
reconnect/backfill cannot cross authorization boundaries, and documents the
expected metadata exposure as a threat-model table.

This is an audit document. No protocol or authorization model was changed:
per PDF Section 14 (Stop Conditions) redesigning crypto/authz is out of scope
and any gap is recorded as a follow-up rather than fixed here.

---

## 1. Method

The review is grounded in the current implementation (commit `origin/main`
@ the start of this task), not in architecture prose. For each wire path the
relevant source was traced:

| Path | Files inspected |
|------|-----------------|
| Internal discovery topic (legacy) | `src/discovery_message.rs`, `src/discovery_service.rs` |
| Internal discovery topic (control plane) | `src/control_plane/message.rs`, `src/control_plane/privacy.rs` |
| Relay / transport | `src/net.rs`, `src/net/actor.rs` |
| Public room directory | `src/control_plane/advertisement.rs`, `src/room_directory.rs`, `src/discovery/presence_scheduler.rs` |
| Private room discovery | `src/private_room_tracker.rs`, `src/discovery_secret.rs` |
| Direct-message / data plane | `src/chat_core/protocol.rs`, `src/contact.rs` |
| Reconnect | `src/control_plane/reconnect.rs` |
| Backfill | `src/backfill/authorizer.rs`, `src/backfill/*` |
| Persisted authorization state | `src/storage.rs`, `src/storage/*` |

Each boundary below lists the metadata it exposes, to whom, its sensitivity,
whether the exposure is expected by design, and the regression test that pins
it.

---

## 2. Wire-path metadata trace

### 2.1 Internal discovery topic (legacy `DiscoveryMessage`, gossip)

The internal discovery topic is a fixed, network-wide gossip topic that every
node joins at startup (the "networking infrastructure" topic). It carries two
legacy wire formats, `DiscoveryMessage` (`src/discovery_message.rs:117`) and
control-plane `ControlEnvelope` (magic `BC`). The discovery topic is **not
protected by a secret**: it is the public rendezvous for the network.

- `Hello { header, event_id }` — sender node public key, protocol version,
  monotonic event id. `src/discovery_message.rs:119`.
- `Presence { header, event_id }` — same shape; periodic heartbeat.
- `PeerAdvertisement { header, advertised, event_id }` — sender public key
  plus the public key of a peer the sender is advertising for connectivity
  bootstrap.

Everything on this topic is **public** to anyone who can subscribe to the
network's discovery topic. The metadata exposed is limited to node public
keys, the protocol version, and per-node monotonic event ids (used only for
dedup). No display name, no profile, no room membership, no conversation
content is published here.

This topic never routes chat: `test_discovery_dm_isolation.rs`
(`assert_discovery_only`) and `test_discovery_group_isolation.rs` prove the
discovery topic only ever carries `DiscoveryMessage` / `ControlEnvelope`, and
that no `SignedMessage` chat payload ever crosses it.

### 2.2 Control-plane messages (magic `BC`, legacy topic)

`src/control_plane/message.rs` `ControlPayload`:
`Hello`, `Presence`, `Capabilities`, `DiagnosticHint`, `Extensions`,
`PublicRoomAdvertisement`, `PublicRoomWithdrawal`. These are broadcast on the
same internal discovery topic. Content is authenticated by the sender's
signature and screened by the privacy guard (`src/control_plane/privacy.rs`):
sender validation, dedup by sequence, rate limiting, and per-field bounds.

Metadata exposed (public, to the discovery swarm): sender node public key,
advertised feature-capability ids, structured connectivity hints, and
metadata-only extensions (`src/control_plane/extensions.rs`). None of these
grant any peer a privilege; the privacy guard rejects spoofed senders,
duplicates, out-of-order sequences, oversized fields and (critically) any
advertisement whose visibility is not `PublicDiscoverable`
(`policy_rejects_non_discoverable_room_advertisement`,
`src/control_plane/privacy.rs:1115`).

### 2.3 Relay / transport path

Connections (loopback, direct P2P, or relayed) are iroh QUIC — **end-to-end
encrypted between endpoints**. The iroh relay (`src/net/actor.rs` relays /
propagates QUIC streams) therefore sees connection-level metadata — endpoint
identities, connection tuples, timing, and byte volumes — but **cannot read
gossip payload plaintext**; those bytes are inside the E2E-encrypted QUIC
stream. This is standard iroh transport behaviour and is expected exposure
(traffic-analysis surface, not content disclosure).

### 2.4 Public room directory

The directory (public lobby + room-directory cache) is populated from
`PublicRoomAdvertisement` (`src/control_plane/advertisement.rs:505`). An
advertisement is public metadata about a **public discoverable** room:

| Field | Exposure | Sensitivity |
|-------|----------|-------------|
| `room_id` (TopicId) | deterministic public room identity (`topic_derivation::public_room_topic`) | public; not the invite secret, not membership |
| `room_name`, `short_description` | user-supplied card text | public |
| `owner_peer_id` | creator's public key | public; descriptive only, grants no privilege |
| `visibility` | must be `PublicDiscoverable` | enforced: private/unlisted never advertised |
| `expires_after_secs` | TTL | public |
| `tags`, `feature_flags` | bounded metadata | public |
| `last_active_hint_secs`, `approximate_member_count` | **untrusted** coarse hints | public; explicitly never an authorization/ranking signal |
| `room_avatar_hash` | BLAKE3 hash of avatar blob | public; no bytes/paths/tickets |
| `signature` | sender's Ed25519 signature | authentication only |

Only `RoomVisibility::PublicDiscoverable` rooms may emit advertisements.
The visibility model (`src/control_plane/advertisement.rs:207`) treats
`Private` and `PublicUnlisted` as **never advertised**, and
`plan_visibility_switch` forbids any switch involving `Private`
(`switch_involving_private_is_forbidden`). The publish sites gate on
`advert.visibility.is_discoverable()` (`src/discovery_service.rs:1406`,
`src/discovery/presence_scheduler.rs:633`). The advertisement never carries
private-room identifiers, invite secrets, membership lists, avatar bytes, or
paths (`no_private_data_fields_present`, `src/control_plane/advertisement.rs:1544`).

### 2.5 Private-room discovery

Private rooms do not use the public directory. They are found via a
`DiscoverySecret` (a random 32-byte secret, `src/discovery_secret.rs`) that
derives a private DHT/publish namespace; only holders of the secret can
publish or look up the room's presence. A party **without** the secret cannot
list or derive the private room from the public discovery surface
(`different_secrets_are_namespace_isolated`,
`tests/test_private_room_dht_discovery.rs:148`; the private namespace uses a
subkey distinct from the public v1 namespace,
`v2_subkeys_are_distinct_from_v1_namespace`). Private-room identifiers are
therefore not exposed by room discovery.

### 2.6 Direct-message path (data plane)

Direct conversations run on the deterministic pair topic
`direct_topic(pk_a, pk_b)` (`src/contact.rs`), derivable from the two
participants' public keys (order-independent; pinned by `test_discovery_dm_isolation.rs`).

- The **topic id** is public-by-derivation: both public keys are published on
  the discovery topic, so any observer can derive the pair topic for any two
  nodes.
- The **payload** is a `SignedMessage` (`src/chat_core/protocol.rs:599`):
  authenticated but **not** end-to-end encrypted for direct messages.
  `Message::Message { text }`, `ImageShare`, `FileShare { name, ticket, size }`,
  `AboutMe { name }`, read receipts, edits, reactions and `ProfileUpdate` are
  broadcast to the topic swarm in the clear (signed only).
- iroh gossip topics have **open membership**: any peer that learns the topic
  and can reach the mesh may subscribe and observe the plaintext
  conversation. There is no server/ACL layer limiting the pair topic to the
  two participants.

This is the single most important **expected** exposure and the reason
`Message::EncryptedGroupMessage` (p2panda forward-secure E2E encryption,
`src/chat_core/protocol.rs:207`) exists for **group** messages but is **not**
used for direct messages today. It is a deliberate, documented gap (see §4
Findings), not a regression introduced by the refactor.

### 2.7 Reconnect

The reconnect handle (`src/control_plane/reconnect.rs`) only re-establishes
reachability for **already-known** friends (queued by the app via
`ReconnectHandle::queue_reconnect`). It "never decides friend-ness (no
authorization)" — friendship/room membership ownership stays in the app
layer. Reconnect therefore cannot cross an authorization boundary: it only
reconnects topics/subscriptions that already existed and were already
authorized. `test_reconnect_asymmetric.rs` and the restart suites exercise
this path.

### 2.8 Backfill

History backfill is gated by `BackfillAuthorizer`
(`src/backfill/authorizer.rs`), which grants only to **current** members of a
topic. `tests/security/authorization.rs` (`backfill_authorization_matrix`,
`removed_member_denied_both_directions`) proves: current member/owner →
allowed; removed member → denied; stranger → denied; wrong connected peer →
denied; unknown topic → denied (**no information leak** on a topic the node
is not authorized for); direct-chat topics → only the two participants.

---

## 3. Threat-model table — expected metadata exposure

| # | Path | Metadata visible | Visible to | Readable content? | Expected? | Authorization mitigated by |
|---|------|------------------|------------|-------------------|-----------|---------------------------|
| M1 | Internal discovery topic (legacy) | node public keys, protocol version, monotonic event ids | whole network swarm (public) | no chat content (separate topics) | yes | topics separation + `test_discovery_dm/group_isolation` |
| M2 | Control plane (legacy topic) | node public key, capability ids, connectivity hints, metadata extensions, public-room ads | whole network swarm (public) | no chat content | yes | signature + privacy guard (`privacy.rs`) |
| M3 | Relay / QUIC transport | endpoint identities, connection tuples, timing, volume (traffic analysis) | relay operator / on-path observer | **no** (QUIC is E2E-encrypted) | yes | iroh QUIC E2E encryption |
| M4 | Public room directory | public room id/name/desc/owner-key/hints/feature flags (public room only) | anyone who can read the lobby topic | no member list, no invite secret, no private-room ids | yes | `RoomVisibility` gate + advert signature + bounds |
| M5 | Private room discovery | nothing without the `DiscoverySecret` | secret holders only | — | yes | secret-derived namespace (`discovery_secret.rs`) |
| M6 | Direct-message path | pair topic id (derivable from two public keys) + **message plaintext + metadata** | any peer that joins the pair-topic swarm | **yes — DM text and file/share/profile metadata** | **yes (documented gap)** | none today → follow-up (E2E for DMs) |
| M7 | Reconnect | none new | — | — | yes | app-owned friendship; no authz decision in reconnect |
| M8 | Backfill | only signed history for authorized topics | authorized members only | yes, but member-gated | yes | `BackfillAuthorizer` matrix |

Net: every path discloses only what the receiver is authorized for, with two
deliberate, documented public surfaces — (a) node public keys on the
discovery topic (needed for connectivity), and (b) unencrypted direct-message
plaintext on the pair topic (§4). Neither is new; both predate the BORU-ARCH
refactors and are preserved behaviour.

---

## 4. Verification of the three authorization boundaries

The task names three invariants. Each is verified below with the enforcement
site and the regression test that pins it.

1. **Discovery never grants friendship, room membership, or conversation
   authorization.**
   - Enforcement: `src/discovery_service.rs` module invariant ("connectivity
     ONLY: it never creates a friendship, a group membership, a
     conversation"); the discovery service has no writer access to the
     friends store / conversation store (it cannot create a friend).
   - Tests: `test_discovery_ui_isolation.rs` (discovery payloads produce no
     chat rows/history/unread/notification/attachment state and the
     conversation forwarder drops discovery-topic events);
     `test_discovery_dm_isolation.rs` (DMs never route via discovery);
     `test_discovery_group_isolation.rs` (group membership stays exactly
     {A, B} — discovery traffic never changes it).
   - Result: **holds**; no implicit trust is created by discovery alone.

2. **Room discovery cannot expose private-room membership or identifiers
   unintentionally.**
   - Enforcement: `RoomVisibility::is_discoverable` gate at both publish
     sites (`discovery_service.rs:1406`, `presence_scheduler.rs:633`);
     privacy-policy rejection of non-discoverable advertisements
     (`policy_rejects_non_discoverable_room_advertisement`);
     `plan_visibility_switch` forbids Private involvement;
     private rooms use a secret-derived namespace.
   - Tests: `policy_rejects_non_discoverable_room_advertisement`,
     `no_private_data_fields_present`, `switch_involving_private_is_forbidden`,
     `different_secrets_are_namespace_isolated`,
     `v2_subkeys_are_distinct_from_v1_namespace`.
   - Result: **holds**; private rooms are structurally never advertised with
     membership or identifiers.

3. **Reconnect/backfill cannot cross authorization boundaries.**
   - Enforcement: reconnect never decides authorization (app-owned);
     `BackfillAuthorizer` member-gates every request.
   - Tests: `backfill_authorization_matrix`,
     `removed_member_denied_both_directions` (removed member denied in both
     directions; unknown topic returns forbidden with no information leak);
     reconnect/restart suites.
   - Result: **holds**.

---

## 5. Findings

**No defect was found in this review**; the refactored architecture enforces
the three authorization boundaries, and every named boundary already has a
regression test. The only notable privacy characteristic is the **expected**,
pre-existing exposure of unencrypted direct-message plaintext on the
deterministic pair topic (M6).

**Follow-up (out of scope for this task — documentation only):**
- F1 — Consider end-to-end encryption for direct messages (today only group
  messages use `Message::EncryptedGroupMessage`). Extending the forward-secure
  p2panda envelope, or a lightweight DH/ratchet, to the direct pair topic would
  remove the M6 plaintext exposure. This is a protocol-authorization-model
  change and belongs in a dedicated task (per PDF Section 14).

No protocol bytes or persistent-storage bytes were changed by this task.

---

## 6. Regression tests that enforce these boundaries

- `tests/security/authorization.rs` — backfill authorization matrix,
  download-descriptor matrix, group-event matrix, short-code freshness,
  tunnel capability, room advert + withdrawal signature matrices.
- `tests/test_discovery_dm_isolation.rs` — direct-vs-discovery topic
  separation.
- `tests/test_discovery_group_isolation.rs` — group membership explicit, not
  discovery-granted; group-vs-discovery topic separation.
- `tests/test_discovery_ui_isolation.rs` — discovery packets never render as
  chat / never grant UI or conversation state; forwarder drops discovery
  events; malformed discovery rejected.
- `tests/test_private_room_dht_discovery.rs` /
  `tests/test_private_room_invitation_discovery.rs` — private-room
  confidentiality via secret-derived namespace.
- `tests/test_public_room_directory.rs` — public directory advert/withdrawal
  authenticity.
- `src/control_plane/privacy.rs` / `src/control_plane/advertisement.rs`
  (unit tests) — non-discoverable advert rejection, bounds, spoof/duplicate/
  rate-limit rejection.
