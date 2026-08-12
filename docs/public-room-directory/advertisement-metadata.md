# Room Advertisement Metadata (BORU-DIR-02, PDF Task 1.2)

Status: implemented (src/control_plane/advertisement.rs)
Chain: BORU-DIR-01 (wire type) → **BORU-DIR-02 (this doc)** → BORU-DIR-03 (auth)

## Goal

Define a **versioned `PublicRoomAdvertisement` payload** that advertises
enough information to browse a room and decide whether to join, without
exposing unnecessary metadata. The advertisement is compact, bounded, and
metadata-only: the discovery network advertises that a room **exists**; it
does not join the room, subscribe to its chat topic, download its history,
or grant permission (PDF Core rule).

## Payload

```rust
pub struct PublicRoomAdvertisement {
    // Required
    pub advert_version: u8,            // advertisement payload version (1)
    pub room_id: TopicId,              // stable room identity (gossip topic)
    pub room_name: String,             // display name (bounded)
    pub short_description: String,     // short description (bounded)
    pub room_protocol_version: u8,     // room chat protocol version
    pub owner_peer_id: [u8; 32],       // creator/owner iroh Ed25519 key
    pub visibility: RoomVisibility,    // must be PublicDiscoverable
    pub expires_after_secs: u32,       // TTL: the expiry/refresh mechanism

    // Optional
    pub tags: Vec<String>,             // searchable tags (bounded)
    pub last_active_hint_secs: Option<u32>,   // coarse activity timestamp
    pub approximate_member_count: Option<u32>, // untrusted member hint
    pub room_avatar_hash: Option<[u8; 32]>,    // content-addressed avatar ref
    pub feature_flags: Vec<String>,    // compatible feature flag ids
}
```

### Field semantics and size limits

| Field | Required | Semantics | Size limit (default bounds) |
|---|---|---|---|
| `advert_version` | yes | Advertisement payload version. Receivers treat unknown future versions as metadata to cache — never as an authorisation signal (PDF Task 1.3 step 4). | `u8` (currently 1) |
| `room_id` | yes | Stable room identity — the room's gossip `TopicId` raw bytes (see `src/topic_derivation.rs`). The directory keys entries by it; a joiner subscribes to it. Not the room name; leaks no invite secret or membership. | 32 bytes (fixed) |
| `room_name` | yes | Human-readable name shown in the directory card. | ≤ 64 Unicode chars (`DEFAULT_MAX_ROOM_NAME_LEN`) |
| `short_description` | yes | Short description shown in the directory card. | ≤ 256 Unicode chars (`DEFAULT_MAX_DESCRIPTION_LEN`) |
| `room_protocol_version` | yes | Room chat protocol version for compatibility checks before join (PDF Phase 6 Task 6.2). | `u8` |
| `owner_peer_id` | yes | Creator/owner peer id (raw iroh Ed25519 public key bytes). **Descriptive metadata only** until BORU-DIR-03 signs advertisements — it grants no moderation or join privileges (PDF Task 1.3 step 3). | 32 bytes (fixed), must be a valid Ed25519 point |
| `visibility` | yes | Room visibility. Must be `PublicDiscoverable` for a valid advertisement: Private and PublicUnlisted rooms are never advertised (PDF visibility model). | enum (`Private`=0, `PublicUnlisted`=1, `PublicDiscoverable`=2) |
| `expires_after_secs` | yes | Advertisement TTL in seconds — the expiry/refresh mechanism. Receiver considers the ad stale at `envelope.timestamp_secs + expires_after_secs`; publisher must refresh before expiry (PDF Phase 3 Task 3.2). Absurd TTLs are rejected (PDF Phase 7 Task 7.1). | 60 .. 604800 s (1 min .. 7 days) |
| `tags` | no | Searchable category tags. | ≤ 8 tags (`DEFAULT_MAX_TAGS`), each ≤ 24 Unicode chars (`DEFAULT_MAX_TAG_LEN`), non-empty, no control chars |
| `last_active_hint_secs` | no | Coarse activity timestamp (unix seconds). Coarse and **untrusted** — a hint, not verified activity. | `u32` |
| `approximate_member_count` | no | Approximate member count. An **untrusted self-reported hint** — never an authorisation or ranking signal (PDF Phase 7 Task 7.3). | `u32` |
| `room_avatar_hash` | no | Content-addressed room avatar/blob reference: a BLAKE3 hash fetched through Boru's existing blob-transfer path. Never carries avatar bytes, paths, URLs, or tickets. | 32 bytes (fixed) |
| `feature_flags` | no | Compatible feature flag ids (e.g. `files-v2`). Namespaced, versioned identifiers using the metadata charset `[A-Za-z0-9._-]`. | ≤ 8 flags (`DEFAULT_MAX_FEATURE_FLAGS`), each ≤ 48 Unicode chars (`DEFAULT_MAX_FEATURE_FLAG_LEN`) |

### Total size bound

Beyond the per-field limits, the **postcard-encoded payload** is capped at
`max_encoded_len = 2048` bytes (`AdvertisementBounds::max_encoded_len`).
This keeps the advertisement compact and well inside the control-plane
envelope cap (`MAX_CONTROL_PAYLOAD_LEN = 4096`), leaving headroom for the
envelope header and future appended fields.

## Privacy guardrails (enforced, not aspirational)

The payload **cannot carry** (structurally impossible — there is no field
for them, and the `no_private_data_fields_present` test pins the Debug
field names):

- Member lists / member identities
- Chat message history / chat previews
- Filenames / attachment content
- Invite secrets / tickets
- Moderation state / moderator ids
- Private keys / secret keys
- `signature_or_auth_proof` (BORU-DIR-03)

Additional policy checks in the privacy layer (`ControlAdvertPolicy`):

- `visibility` must be `PublicDiscoverable` — private/unlisted rooms are
  never advertised.
- `owner_peer_id` must be a valid iroh Ed25519 public key.
- All free-form metadata rejects ASCII control characters (log-injection /
  display-injection defence).
- Feature flags use the metadata charset `[A-Za-z0-9._-]`.

## Versioning and wire compatibility

- `advert_version` is the advertisement **payload** version (currently 1),
  independent of the control-plane envelope version and the room protocol
  version.
- New payload fields are appended at the **end** of the struct: the
  envelope decoder uses `postcard::take_from_bytes`, so older clients
  decode the known prefix and ignore trailing bytes (forward compatible —
  proven by `unknown_future_advertisement_fields_tolerated`).
- Receivers treat an unknown `advert_version` as metadata to cache, never
  as an authorisation signal.

## Out of scope (later tasks)

- Signing / authentication of advertisements (`signature_or_auth_proof`) —
  BORU-DIR-03.
- Visibility state, publish/refresh/expiry loops, directory cache — PDF
  Phases 2–4.
- Discover Rooms UI — PDF Phase 5.
- Join flow / compatibility UI — PDF Phase 6.

## Tests

- Round-trip (full + minimal) through postcard.
- Optional-field presence/absence.
- Size-limit enforcement for every field (name, description, tags, flags,
  TTL, total encoded size).
- Visibility guardrail (non-discoverable rejected).
- Owner peer id validity.
- No private data fields present (Debug-field pin).
- Compactness (minimal advertisement < 200 bytes).
- Policy-level rejection (oversized / non-discoverable) in privacy.rs.
- End-to-end decode into `ControlEvent::RoomAdvertisement` with the full
  payload in discovery_service.rs.
