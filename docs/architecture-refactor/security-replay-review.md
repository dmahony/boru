# Security Replay, Stale-Event & Downgrade Review (BORU-SEC-002)

Status: audit + regression tests (no production behaviour changes).
Task: BORU-ARCH-40, Phase 8 (Security and Protocol Review) of the BORU-ARCH chain.
Scope: verify that **old or repeated control messages cannot corrupt current
state** — that replay and stale-event handling is *defined, not accidental*,
and that capability/version negotiation cannot *silently downgrade*
security-sensitive behaviour. Adds explicit regression tests for each
accepted/rejected case.

This is an audit document built on the current implementation (commit
`origin/main` @ the start of this task). No protocol, storage format,
serialization, or authorization model was changed. Any gap is recorded as a
follow-up rather than fixed here (PDF Section 14 Stop Conditions).

---

## 1. Method

For each wire path the relevant source was traced, the mechanisms that guard
against replay / stale events / downgrade were identified, and the existing
regression tests that pin the accepted/rejected cases were enumerated. Two
explicit regression tests were added for the one previously-untested gap
(§5.4). The audit is grounded in code (file:line), not architecture prose.

| Wire path | Files inspected |
|-----------|-----------------|
| Internal discovery topic — legacy `DiscoveryMessage` | `src/discovery_message.rs`, `src/discovery/peer_registry.rs`, `src/discovery_service.rs` |
| Internal discovery topic — control plane (`BC` envelopes) | `src/control_plane/message.rs`, `src/control_plane/privacy.rs`, `src/control_plane/dispatch.rs` |
| Presence / capability / extensions re-announcement | `src/discovery/presence_scheduler.rs` |
| Public room directory | `src/control_plane/advertisement.rs`, `src/room_directory.rs`, `src/discovery/directory_lifecycle.rs` |
| Capability / version negotiation | `src/control_plane/capabilities.rs`, `src/discovery_service.rs` (`CapabilityGate`) |
| File-transfer capability gate | `src/bin/boru/app/files.rs` |

---

## 2. Recommended security model: monotonic counters + dedup + TTL

Boru's control plane is built on a single, coherent invariant:

> **Per-sender monotonic counters** (event id on legacy discovery, sequence on
> control envelopes) plus **idempotent dedup** plus **bounded TTL expiry**.
> A delivery is accepted only if it is *newer* than (or a refire of) the
> highest thing already seen from that sender. An *older* delivery — a replay
> or an out-of-order stale event — is rejected and cannot regress state.

This is enforced differently on the two discovery wire formats:

| Sender state | Legacy `DiscoveryMessage` | Control `ControlEnvelope` |
|--------------|---------------------------|---------------------------|
| Dedup key | `(node_id, event_id)` | `(sender_node_id, sequence)` |
| Counter type | per-node monotonic `u64` | per-sender monotonic `u64` |
| Counter seed | `rand::random::<u64>()` (BORU-DISC-17) | `unix_now_secs()` (BORU-DIR-23) |
| Replay rejection | exact-equal `event_id` → `Duplicate` | duplicate `(sender, seq)` → `Reject(Duplicate)` |
| Out-of-order (older) rejection | equal-id only (see §5.2) | `sequence <= last_sequence` → `Duplicate` |
| Staleness/liveness | `prune_older_than` TTL | TTL `expire_stale` + `get_active` fail-closed |
| Attribution | gossip `delivered_from` | Ed25519 signature (BORU-CP-17) |

Both counters being monotonic *within* a process means a replay of an already
seen event id or sequence is caught by exact-equality dedup, and a genuine
new event always advances the counter. The two different seeds handle the
restart case differently and deliberately: the control-plane sequence is
seeded with wall-clock seconds so a restarted advertiser's post-restart
sequence is *guaranteed higher* than anything the same identity sent before
(tested by the `BORU-DIR-23` matrix scenario); the legacy discovery event id
is seeded randomly so a restarted peer's first HELLO is distinguishable from
a byte-identical duplicate at the gossip layer (BORU-CP-07).

---

## 3. Event ids / nonces / timestamps / TTLs used by discovery and control-plane (§4.1 of objective)

### 3.1 Legacy `DiscoveryMessage` (`src/discovery_message.rs`)

- `Hello` / `Presence` / `PeerAdvertisement` each carry an optional per-node
  **`event_id: Option<u64>`** — a monotonic counter, `None` on the legacy
  wire format for backward compatibility (`deserialize_tolerant_opt_u64`
  backfills `None` on EOF so old payloads decode unchanged).
- `event_id` is wire-tolerated: `skip_serializing_if = "Option::is_none"` +
  tolerant deserializer keep the BORU-DISC-06 34B/66B wire sizes intact.
- **No timestamp field** on legacy discovery messages — liveness is tracked
  by *arrival* time (`last_seen: Instant` in the registry), not by a sender
  clock. This is safe because legacy discovery grants only connectivity
  hints, never authorisation.
- **No TTL field** on legacy discovery messages — the registry prunes via
  `PeerRegistry::prune_older_than(max_age)` (wall-clock arrival age).

### 3.2 Control envelopes (`src/control_plane/message.rs`)

- `ControlEnvelope` carries **`sequence: u64`** (per-sender monotonic nonce)
  and **`timestamp_secs: u64`** (unix epoch creation time, `0` = unknown),
  plus `protocol_version` and `sender_node_id`.
- Presence carries an optional **`ttl_secs: Option<u32>`**, validated by the
  advert policy (`MAX_PRESENCE_TTL_SECS`, floor `PresenceTtlTooSmall`,
  ceiling `PresenceTtlTooLarge` — `src/control_plane/privacy.rs::policy`).
- Room advertisements carry **`advert_version: u8`**, **`expires_after_secs`**
  (TTL, bounded `[DEFAULT_MIN_ADVERT_TTL_SECS, DEFAULT_MAX_ADVERT_TTL_SECS]`),
  and **`timestamp_secs`**; the directory computes plan expiry when the
  receiver considers it stale (`src/control_plane/advertisement.rs:541`).
- Capabilities / extensions are whole-set re-announcements keyed by the
  envelope `sequence`, so a newer envelope replaces the cached set atomically.

### 3.3 Summary of the id/nonce/timestamp/TTL surface

| Field | Carried by | Purpose | Bounds / validation |
|-------|-----------|---------|---------------------|
| `event_id` | legacy discovery | dedup key | per-node monotonic, optional |
| `sequence` | control envelope | dedup + ordering nonce | per-sender monotonic, required |
| `timestamp_secs` | control envelope / advert | creation-time ordering | `0` = unknown |
| `ttl_secs` | control presence | liveness | `[min, max]` policy-checked |
| `expires_after_secs` | room advert | directory expiry | `[min, max]` policy-checked |
| `advert_version` | room advert | payload schema version | constant `ADVERTISEMENT_PAYLOAD_VERSION` |

---

## 4. Duplicate / replayed messages are idempotent or rejected (§4.2)

### 4.1 Legacy discovery (exact-equality dedup)

`PeerRegistry::upsert` (`src/discovery/peer_registry.rs:123`) returns:

- `New` — first time a node is seen.
- `Refreshed` — known node with a *different* event id (a real refresh).
- `Duplicate` — known node with the **same** event id re-delivered; the
  entry is left untouched (no last-seen / source / source-topic mutation),
  so replaying the same advertisement over two discovery paths has no effect.

`DiscoveryService::handle_incoming` (`src/discovery_service.rs:591`) maps
`Duplicate` to `IncomingOutcome::Duplicate` and emits no `PeerUpdate`, so a
replayed announcement cannot churn the connectivity state machine.

Pinned by: `registry_duplicate_event_id_ignored`,
`registry_event_id_survives_legacy_message`,
`handle_incoming_duplicate_event_id_ignored`.

### 4.2 Control plane (dedup + monotonic rejection)

`ControlPlaneGuard::admit` (`src/control_plane/privacy.rs:860`) is the gate:

1. **Rate limit** by authenticated delivery source (bounds log spam / churn).
2. **Attribution** — envelope sender must equal the authenticated gossip
   source, or carry a BORU-CP-17 signature that verifies against
   `sender_node_id`; a present-but-invalid signature is a spoof attempt →
   `Reject(SpoofedSender)`.
3. **Minimal-content policy** — whitelist + per-field bounds.
4. **Dedup** by `(sender_node_id, sequence)` in a bounded set (cleared when
   full as a resource valve) → duplicate `(sender, seq)` = `Reject(Duplicate)`.
5. **Presence state** — `PeerControlStateStore::record` rejects any
   `sequence <= last_sequence` as `Duplicate`, so an out-of-order **older**
   delivery never regresses presence / capabilities / protocol metadata.

Both duplicate refires *and* out-of-order older sequences are rejected, and
both are idempotent no-ops (no side effects, no UI churn).

Pinned by: `guard_rejects_duplicate_sequence`,
`guard_rejects_out_of_order_sequence`, `guard_accepts_different_sequences`,
`presence_store_stale_capabilities_not_current`.

### 4.3 Chat-layer replay floods

`tests/test_hostile_input.rs` pins the data-plane replay boundary:
`replay_flood_identical_messages_suppressed`, `replay_flood_about_me_capped_at_one`,
`duplicate_message_suppressed_by_dedup`, `duplicate_about_me_suppressed_by_dedup`,
`same_content_different_sender_not_deduped`, `rejected_input_does_not_cause_unbounded_dedup_set`.

### 4.4 Room directory duplicate merge

`src/room_directory.rs::decide_update` step 1 returns `Duplicate` for an
identical advertisement (same publisher + envelope sequence + advert version
+ content digest) → pure liveness refresh, no new card, no UI event
(`duplicate_advertisements_merge_into_one_entry`, `identical_advertisement_is_deduped_no_ui_churn`).

---

## 5. Stale presence / advertisements cannot override newer state (§4.3)

### 5.1 Control-plane presence TTL + stale fail-closed

`PeerControlStateStore` is bounded and TTL-expiring. `expire_stale` removes
entries beyond their TTL, and — crucially for security — the negotiation
lookup uses **`get_active(node, now)`** which returns `None` for any peer
whose presence has gone stale, so **stale capability/extension data is never
treated as current** (`presence_store_stale_capabilities_not_current`,
`guard_expires_stale_presence`). Capabilities die with presence: when the
entry is expired, the cached capability set goes with it.

### 5.2 Legacy discovery: deliberate exact-equality (documented, not stale-override)

One nuance worth recording: the legacy discovery `PeerRegistry` dedups by
*exact* `event_id` equality only, not by monotonic comparison. A **lower**
(non-equal) event id from a known, still-online peer is treated as
`Refreshed` (it bumps `last_seen`). This is intentional and safe:

- The discovery counter is seeded randomly per process (BORU-CP-07), so a
  restarted peer legitimately starts from a fresh random (possibly lower)
  id-space; monotonic rejection would silently drop a legitimate
  post-restart re-announcement. The exact-equality dedup is what lets the
  `refresh_after_restart` rediscovery path (BORU-CP-07) distinguish a genuine
  restart from a duplicate delivery while a peer is alive/online.
- Legacy discovery grants **connectivity hints only** (a dial candidate,
  never friendship / membership / authorisation — proven in BORU-SEC-001), so
  bumping `last_seen` on a distinct event id cannot corrupt any
  security-relevant state.

Pinned by: `handle_incoming_distinct_event_ids_refresh`,
`handle_incoming_restart_rediscovery_when_peer_lost`,
`registry_same_peer_two_topics_is_one_entry`.

### 5.3 Room directory stale-replay rejection

`decide_update` (`src/room_directory.rs:1020`), same-publisher refresh rule
(step 3):

> `if incoming_sequence < existing.sequence { return Keep }` — "Stale replay:
> cannot downgrade the cached metadata."

So a replayed room advertisement with a lower envelope sequence is a
deterministic no-op (`AdvertiseOutcome::Unchanged`); the newer metadata wins.
Pinned by `duplicate_advertisements_merge_into_one_entry` (the "stale replay
cannot downgrade" assertion, lines 1222-1231) and the conflict/authority
matrix (`older_conflicting_advertisement_keeps_winner_marks_conflict`,
`verified_authority_resolves_conflict`, `untrusted_update_cannot_rename_verified_room`).

---

## 6. Capability / version negotiation cannot silently downgrade (§4.4)

### 6.1 Fail-closed capability negotiation

`compatible_version(local, remote, feature)` (`src/control_plane/capabilities.rs:352`)
returns the **highest** version both sides share, or **`None`** if they share
none / a side does not advertise the feature. `DiscoveryCapabilityGate::peer_supports`
(`src/discovery_service.rs:850`) wires this into the app: `None` means "no
compatible version → do not initiate the feature" (fail closed).

Pinned by: `test_compatible_version`, `test_files_protocol_version_negotiation`,
`peer_capabilities_and_peer_supports_query`.

### 6.2 Negotiation follows the latest *honest* claim, never a stale one

Two complementary rules pin the downgrade boundary (the regression tests
added in this task):

- **REJECTED (no silent downgrade via replay):** a stale, lower-sequence
  `CAPABILITIES` envelope cannot regress the cached capability set. In
  `guard_rejects_stale_capability_downgrade`, a peer advertising `files-v2`
  at sequence 10, then re-announcing `files-v1` at sequence 9, is rejected
  (`Reject(Duplicate)`) and the cached `files-v2` set is preserved. A
  replayed older envelope cannot force the legacy BlobTicket path onto a
  peer that honestly supports the direct `files-v2` path.
- **ACCEPTED (honest re-announcement):** a genuinely newer sequence *may*
  change the advertised set — the caller negotiates against the latest honest
  capability claim (`guard_accepts_newer_capability_update`).

### 6.3 Version negotiation

- **Control-plane protocol version** (`CONTROL_PLANE_PROTOCOL_VERSION = 1`):
  a frame with an unknown `protocol_version` is dropped at decode
  (`UnsupportedVersion`) and never interpreted
  (`src/control_plane/dispatch.rs:140`).
- **Discovery protocol version** (`BORU_DISCOVERY_PROTOCOL_VERSION = 1`):
  `check_discovery_version` rejects unknown versions; the receive path drops
  `Unsupported` messages (logged, counted — `DiscoveryVersionCheck`).
- **App protocol version** (`BORU_APP_PROTOCOL_VERSION = 1`, the HELLO field):
  capabilities are **never inferred from the app version string** — feature
  availability comes only from the capability set
  (`test_availability_not_inferred_from_app_version`,
  `capabilities.rs` designer notes). So a peer or relay cannot downgrade
  feature negotiation by misreporting an app version.
- **Room compatibility**: `RoomCompatibility::for_room_protocol` is a
  forward-compat ladder (same/older joinable; exactly-one-newer requires
  upgrade; more-than-one-newer is incompatible — `src/room_directory.rs:150`),
  so an advertisement cannot silently downgrade a room's joinable-ness.

### 6.4 Additive (cannot silently remove) security properties already covered

These are adjacent but were verified to hold by inspection so this review can
state downgrade-resistance exhaustively:

- **Presence never grants authorisation** — the control-plane store is a
  metadata cache only; `presence_never_grants_authorisation`.
- **Spoofed sender rejected** — `guard_rejects_spoofed_sender`,
  `spooffed_sender_rejected_by_signature` (hostile-input).
- **Capability gate at file send** — a direct-peer file transfer is blocked
  (toast + no-op) when `negotiated_feature_version(peer, FILES)` is `None`
  (`src/bin/boru/app/files.rs:5952-5974`), and only a `files-v2` peer gets
  the direct FileOffer path (`files.rs:5992`).

---

## 7. Regression tests added in this task

All tests below are new in this task and build on the existing
replay/hostile-input/authorization suites (BORU-AUDIT-28). They live in
`src/control_plane/privacy.rs` and run under `rb test --lib`.

| Test | Case | Verdict |
|------|------|---------|
| `guard_rejects_stale_capability_downgrade` | older lower-sequence `CAPABILITIES` re-announcement tries to regress v2→v1 | **REJECTED** (Duplicate); cached set unchanged |
| `guard_accepts_newer_capability_update` | genuinely newer sequence re-announces a changed capability set | **ACCEPTED** (negotiation follows latest honest claim) |

---

## 8. Findings

- **No defect found.** The replay, stale-event, and downgrade handling is
  already defined (not accidental) across every wire path: per-sender
  monotonic counters, exact-equality + bounded-set dedup, monotonic
  out-of-order rejection, TTL expiry with stale-fail-closed negotiation, and
  fail-closed capability/version negotiation.
- **One previously-untested case pinned by new tests:** the "stale capability
  re-announcement cannot downgrade the cached set" invariant was enforced by
  the store but had no explicit regression test. The two tests in §7 now pin
  both the rejected (stale downgrade) and accepted (honest newer claim) cases.
- **Recorded nuance (not a defect):** the legacy discovery registry dedups by
  exact event-id equality rather than monotonic comparison. This is
  intentional (randomised per-process seed + restart rediscovery) and safe
  because legacy discovery grants connectivity hints only, never
  authorisation (§5.2).

## 9. Follow-ups

None blocking. (No protocol, wire, or storage behaviour changed; the audit is
purely additive: two regression tests + this document.)

---

## 10. Verification

- `git diff --check` clean.
- `rb test --lib control_plane::privacy::tests` → 44/44 pass (includes the two
  new regression tests).
- `rb check` green for the Boru target (pre-existing warnings only).
- Only new/changed regions formatted by hand (repo is not rustfmt-clean at
  HEAD; no repo-wide `cargo fmt` was run).
