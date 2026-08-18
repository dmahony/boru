# Room Withdrawal / Tombstone (BORU-DIR-09, PDF Task 3.3)

Status: implemented
Chain: BORU-DIR-08 (TTL) → **BORU-DIR-09 (this doc)** → BORU-DIR-10 (bounded cache)

## Goal

When a room is deleted, made unlisted, or intentionally removed from
discovery, the advertiser must be able to remove it from other directories
**immediately**, instead of waiting for the advertisement TTL. A
withdrawal/tombstone is a small signed message broadcast over the directory
gossip topic; directory clients that verify it remove the matching
advertisement right away. TTL expiry (BORU-DIR-08) remains the safety net for
withdrawals that are missed (offline peers, packet loss).

## Design: two transports, one authority rule

The directory has two transports that coexist today:

1. **Legacy chat path** (the one the iced GUI actually uses): a
   `SignedMessage`-wrapped `Message::RoomAdvertisement` broadcast over the
   directory gossip topic (`discovery_topic()`). The withdrawal is a new
   sibling `Message::RoomWithdrawal { topic, signature }`.
2. **Control plane** (BORU-DIR-01; protocol-complete, consumed by the Phase 4
   cache, BORU-DIR-10): `PUBLIC_ROOM_WITHDRAWAL` on the
   `ControlEnvelope`/`DiscoveryService` channel, alongside
   `PUBLIC_ROOM_ADVERTISEMENT`.

Both require **the same authoritative identity rules used for
advertisements**:

- The withdrawal must be signed by the node that published the advertisement
  being withdrawn (the advertisement's author is its designated authority in
  the legacy path; in the control plane the authority is `owner_peer_id`,
  so only the room's designated owner may withdraw the canonical entry).
- A withdrawal for a room the signer never advertised removes nothing
  (spoofed withdrawals cannot remove unrelated rooms).
- TTL expiry remains the final cleanup mechanism: an advertisement whose
  withdrawal was missed still expires via `evict_expired` (BORU-DIR-08).

## Legacy path (functional in the GUI)

### Wire format

`Message::RoomWithdrawal` (appended at the END of the `Message` enum for wire
compatibility, `src/chat_core/protocol.rs`):

```rust
RoomWithdrawal {
    topic: TopicId,          // the room whose advertisement is withdrawn
    signature: Vec<u8>,      // ed25519 over canonical signing bytes
}
```

Signing/verification uses the same canonical-signing machinery as
advertisements, with a distinct protocol tag so a withdrawal signature can
never be replayed as an advertisement signature:

| Constant | Value | Meaning |
|---|---|---|
| `ROOM_WITHDRAWAL_PROTOCOL` | `"boru/room-withdrawal"` | canonical signing tag |
| `ROOM_WITHDRAWAL_VERSION` | `1` | signed payload layout version |

Public helpers (re-exported from `boru_core::chat_core`):

- `sign_room_withdrawal(&topic, &sk) -> Vec<u8>`
- `verify_room_withdrawal(&topic, &signature, author) -> bool`

### Publisher side (emit)

The iced app emits a fire-and-forget withdrawal (`broadcast_room_withdrawal`)
whenever a room stops being discoverable:

- **Made unlisted** (`VisibilitySwitchOutcome::Unlisted` branch of
  `apply_room_directory_visibility`): the local advertisement entry is
  removed and a signed `Message::RoomWithdrawal` is broadcast.
- **Deleted** (`AppMessage::ConfirmDeleteRoom` in `src/bin/boru/app/chat.rs`):
  if the deleted room was advertised (`advertised_rooms`), the local entry is
  dropped and a signed withdrawal is broadcast.

The broadcast is best-effort: if the directory sender is not yet available
(startup race) or the broadcast fails, remote directories fall back to TTL
expiry — the withdrawal is an optimization, not a correctness requirement.

### Directory-client side (consume)

Two receive paths both apply a verified withdrawal via
`DirectoryStore::withdraw(topic, author)` (which removes exactly the
`(topic, author)` entry — the withdrawal signal the Phase 4 cache consumes):

1. **main.rs directory receiver loop**: decodes the
   `SignedMessage`; on `Message::RoomWithdrawal`, verifies
   `verify_room_withdrawal`; if valid, forwards
   `app::DirectoryRoomEvent::Withdrawal(topic, from)`; otherwise drops with a
   debug log. The app drains this on `ConnMonitorTick` and withdraws from the
   store.
2. **app.rs NetEvent handler**: `Message::RoomWithdrawal` arriving through the
   normal per-topic event pipeline is verified and applied directly.

`DirectoryRoomEvent` is the channel type that replaced the old
`DirectoryRoomUpdate` struct (the `Advertisement` variant preserves the
previous behaviour).

## Control-plane path (protocol-complete, BORU-DIR-10 consumes)

`PUBLIC_ROOM_WITHDRAWAL` is message type `6` on the control plane
(`src/control_plane/message.rs`), carried in the same signed
`ControlEnvelope` as advertisements:

| Item | Location |
|---|---|
| `ControlMessageType::PublicRoomWithdrawal = 6` | `src/control_plane/message.rs` |
| `ControlPayload::PublicRoomWithdrawal(PublicRoomWithdrawalPayload)` | `src/control_plane/message.rs` |
| `PublicRoomWithdrawal` struct | `src/control_plane/advertisement.rs` |
| `ControlEnvelope::public_room_withdrawal(...)` | `src/control_plane/message.rs` |
| `DiscoveryService::announce_room_withdrawal(...)` + public handle wrapper | `src/discovery_service.rs` |
| `ControlEvent::RoomWithdrawal(RoomWithdrawalEvent)` | `src/discovery_service.rs` |

The payload mirrors the advertisement authority model
(BORU-DIR-03): `withdrawal_version`, `room_id`, `owner_peer_id`,
`timestamp_secs`, `signature`. Verification reuses `AdvertisementAuth`
(`Verified` / `InvalidSignature` / `MissingSignature`) with a distinct
signing protocol tag (`"boru/public-room-withdrawal/v1"`). Incoming handling:

- **Verified + authoritative** (`sender == owner_peer_id`): emit
  `ControlEvent::RoomWithdrawal` → the Phase 4 cache removes the room's
  canonical entry.
- **Verified but not authoritative**: drop (`WithdrawalNotAuthoritative`) —
  a member cannot withdraw the owner's canonical entry.
- **Invalid / missing signature**: drop (`WithdrawalAuthRejected`) — forged
  withdrawals are discarded before they reach any cache.

The privacy guard (BORU-CP-16) accepts the tiny fixed-size payload within the
same bounds as advertisements (`AdvertViolation::Withdrawal`).

## Tests

- `tests/security/authorization.rs::room_withdrawal_signature_matrix`:
  round-trip sign/verify; wrong key, tampered topic, and truncated signature
  all fail.
- `src/directory.rs`:
  - `directory_store_withdrawal_removes_matching_ad` — `withdraw` removes
    exactly `(topic, author)`.
  - `directory_store_withdrawal_cannot_remove_unrelated_room` — other
    author's ad and other room's ad survive.
  - `directory_store_missed_withdrawal_still_expires_via_ttl` — TTL expiry
    remains the safety net when no withdrawal arrives.
- `src/control_plane/advertisement.rs`:
  - withdrawal sign/verify round-trip;
  - wrong-key signature rejected;
  - non-authoritative (member) withdrawal rejected.
- `src/control_plane/privacy.rs::policy_accepts_public_room_withdrawal`:
  bounded withdrawal passes the privacy guard.
- `src/discovery_service.rs`:
  - verified authoritative withdrawal → `ControlEvent::RoomWithdrawal`;
  - tampered withdrawal → `WithdrawalAuthRejected`, no event;
  - member (non-authoritative) withdrawal → `WithdrawalNotAuthoritative`, no
    event;
  - `announce_room_withdrawal` broadcasts a signed envelope with the
    `PublicRoomWithdrawal` message type.
- `src/bin/boru/app.rs`:
  - `directory_room_withdrawal_removes_matching_advertisement` — feeding a
    withdrawal through the same channel main.rs uses removes the room.
  - `directory_room_withdrawal_cannot_remove_other_authors_ad` — spoofed
    withdrawal cannot remove an unrelated author's advertisement.
  - `directory_room_withdrawal_for_unadvertised_room_is_noop` — no spurious
    churn.

## Acceptance criteria (PDF Task 3.3)

- [x] Intentional unlisting/deletion removes the room from other directories
  quickly (verified withdrawal → immediate `DirectoryStore::withdraw`).
- [x] Spoofed withdrawals cannot remove unrelated rooms (keyed by the
  signer's own `(topic, author)` entry; control plane additionally requires
  the designated `owner_peer_id`).
- [x] TTL remains the final cleanup mechanism: a missed withdrawal still
  expires via BORU-DIR-08 `evict_expired`.
