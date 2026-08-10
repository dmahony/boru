# Boru protocol signing — one rule for all signed objects (BORU-AUDIT-27)

Status: implemented 2026-08-10 (BORU-AUDIT-27)

## The rule

Every Ed25519-authenticated Boru protocol object signs **one canonical byte
string**:

```
BORU/<protocol>/<version> || deterministic serialization of ALL security-relevant fields
```

Concretely, the canonical bytes are produced by a single shared function

```rust
protocol_signing::canonical_signed_bytes(protocol, version, fields)
```

which returns `postcard::to_stdvec((protocol, version, fields))`.  The tuple
is deterministic: postcard serializes fields in order and length-prefixes every
variable-length value, so two different objects can never produce the same
byte string with different meanings.

This replaces the historical bespoke layouts (raw `timestamp || data`
concatenation, bare postcard tuples, `extend_from_slice` hashes) that made it
easy for a new field to affect authorization or freshness without being
signed, or for two protocols to encode identities differently.

## Domain separation

`protocol` is a stable ASCII string unique per protocol object family:

| Object family              | protocol string              |
|----------------------------|------------------------------|
| Mailbox envelope V2        | `boru/mailbox`               |
| Mailbox acknowledgement    | `boru/mailbox-ack`           |
| Inbox signed message       | `boru/inbox`                 |
| Inbox author-delete proof  | `boru/inbox-delete`          |
| Tunnel capability          | `boru/tunnel-capability`     |
| Chat `SignedMessage`       | `boru/chat-message`          |
| Room advertisement         | `boru/room-advertisement`    |
| Short-code announcement    | `boru/short-code-announcement` |
| Contact control message    | `boru/contact`               |
| File catalogue             | `boru/file-catalogue`        |
| Room metadata (authorized) | `boru/room-metadata`         |
| Group event envelope       | `boru/group-event`           |
| Group epoch removed event  | `boru/group-epoch-removed`   |
| Group epoch changed event  | `boru/group-epoch-changed`   |
| Logical direct message     | `boru/logical-dm`            |
| File download descriptor   | `boru/file-descriptor`       |

A signature made over `boru/mailbox-ack` bytes can never be replayed as a
signature over `boru/group-event` bytes, and vice versa.

## Explicit version

`version` is an integer embedded **inside** the signed bytes (never only in an
unauthenticated header).  Unknown versions are rejected by verification, never
guessed.  Bump the version whenever the signed layout changes (a field is
added/removed/reordered, or a semantic rule changes).

## Field classification per object

For every signed type below: **security-relevant signed field** = must be in
the canonical bytes; **transport-local field** = deliberately excluded (never
influences authorization/freshness).

### Mailbox envelope (`src/mailbox.rs`, `MailboxEnvelopeV2`)

| Field | Classification |
|---|---|
| `version` | security-relevant (interpretation) |
| `from`, `recipient` | security-relevant (identity) |
| `ephemeral`, `nonce` | security-relevant (crypto material, nonce) |
| `created_at` | security-relevant (freshness) |
| `ciphertext` | security-relevant (AEAD bound as associated data + signed) |
| `signature` | signature itself |

The AEAD associated-data context includes the same domain/version/identity
fields as the signature, so external metadata is bound twice (belt and
braces).

### Mailbox acknowledgement (`MessageAcknowledgement`)

| Field | Classification |
|---|---|
| `version` | security-relevant (interpretation) |
| `message_id` | security-relevant (object reference) |
| `original_sender`, `recipient` | security-relevant (identity) |
| `acknowledged_at_ms` | security-relevant (freshness) |
| `status` | security-relevant (interpretation) |

### Inbox signed message (`SignedInboxMessage`)

| Field | Classification |
|---|---|
| `sender` | security-relevant (identity) |
| `sent_at_unix_secs` | security-relevant (freshness/replay) |
| `inner` | security-relevant (payload) |

### Inbox author-delete proof (`AuthorDeleteProof`)

| Field | Classification |
|---|---|
| `msg_id` | security-relevant (object reference) |
| `conversation_id` | security-relevant (routing/topic) |
| `created_at_unix_secs` | security-relevant (freshness/replay) |
| `author` | security-relevant (identity) |

### Tunnel capability (`TunnelCapability`)

| Field | Classification |
|---|---|
| `version` | security-relevant (interpretation) |
| `tunnel_id` | security-relevant (capability scope) |
| `owner_endpoint_id` | security-relevant (identity) |
| `allowed_peer_endpoint_id` | security-relevant (identity) |
| `created_at_ms`, `expires_at_ms` | security-relevant (freshness) |
| `nonce` | security-relevant (nonce) |

### Chat signed message (`SignedMessage`)

| Field | Classification |
|---|---|
| `from` | security-relevant (identity) |
| `data` | security-relevant (payload) |
| `sent_at` | security-relevant (freshness) |
| `compression` | security-relevant (interpretation) |

### Room advertisement (`RoomAdvertisement` via `sign_advertisement`)

| Field | Classification |
|---|---|
| `room_name`, `description` | security-relevant (interpretation/display) |
| `topic` | security-relevant (routing/topic) |
| `ticket` | security-relevant (authorization context) |
| `member_count`, `last_activity` | security-relevant (freshness/interpretation) |

### Short-code announcement (`SignedShortCodeAnnouncement`)

| Field | Classification |
|---|---|
| `from` | security-relevant (identity) |
| `sent_at_unix_secs` | security-relevant (freshness) |
| `data` (announcement) | security-relevant (payload) |

### Contact control message (`SignedContactMessage`)

| Field | Classification |
|---|---|
| `from` | security-relevant (identity) |
| `sent_at_unix_secs` | security-relevant (freshness) |
| `data` (action) | security-relevant (payload) |

### File catalogue (`SignedFileCatalogue`)

| Field | Classification |
|---|---|
| `owner_id` | security-relevant (identity) |
| `revision` | security-relevant (freshness/ordering) |
| `generated_at_ms` | security-relevant (freshness) |
| `collections`, `files` | security-relevant (payload) |

### Room metadata (authorized wire, `room_docs.rs`)

| Field | Classification |
|---|---|
| `version` | security-relevant (interpretation) |
| `owner` | security-relevant (identity) |
| `payload` (`RoomMetadata`) | security-relevant (payload) |

### Group event envelope (`GroupEventEnvelope`)

| Field | Classification |
|---|---|
| `version` | security-relevant (interpretation) |
| `group_id` | security-relevant (routing/topic) |
| `event_id` | security-relevant (replay protection) |
| `nonce` | security-relevant (nonce) |
| `epoch` | security-relevant (interpretation) |
| `actor` | security-relevant (identity) |
| `timestamp` | security-relevant (freshness) |
| `payload` | security-relevant (payload) |

### Group epoch removed/changed (`MemberRemovedEvent`, `EpochChangedEvent`)

| Field | Classification |
|---|---|
| `group_id` | security-relevant (routing/topic) |
| `epoch` / `old_epoch` / `new_epoch` | security-relevant (interpretation) |
| `member` | security-relevant (identity/scope) |
| `actor` | security-relevant (identity) |
| `timestamp` | security-relevant (freshness) |

### Logical direct message (`LogicalDm` in `storage.rs`)

| Field | Classification |
|---|---|
| `conversation_id` | security-relevant (routing/topic) |
| `sender`, `recipient` | security-relevant (identity) |
| `sequence`, `message_id` | security-relevant (ordering/reference) |
| `plaintext` | security-relevant (payload) |

### File download descriptor (`DescriptorSignedPayloadV2`)

Standardized earlier by BORU-AUDIT-05; already follows this rule.  See
`docs/file-access-descriptor-signing.md`.

## Not covered by this rule (documented exceptions)

- `spake2_pairing::AuthenticatedInvitation` is **not** an Ed25519 signature:
  it is an HMAC-SHA256 MAC under a SPAKE2-derived key.  The MAC covers the
  whole postcard-encoded invitation payload, which includes the version and
  identity, so its integrity coverage is complete for its purpose.
- `discovery_record` uses the `distributed-topic-tracker` crate's native
  `Record` signing format (external crate), which already embeds the topic,
  unix minute, publisher key and content in its signature.
- `group_encryption` pre-key bundles use p2panda's `XSignature` (external
  crate), whose format is defined upstream.
- Transport-local fields (connection IDs, QUIC stream metadata, gossip
  neighbor identities used purely for routing the *current hop*) are never
  part of the signed representation.  Anything that a peer later uses to
  decide authorization, ordering or freshness MUST be signed.

## Implementation notes

- `src/protocol_signing.rs` exposes `canonical_signed_bytes` and
  `verify_canonical_or_legacy`.
- Each object family keeps its own `fn signing_bytes`/`unsigned_bytes`/
  `signing_payload` helper so the field order stays visible at the call site;
  the helpers all delegate to `canonical_signed_bytes` rather than hand-rolling
  byte concatenation.  The one exception is `MailboxEnvelopeV2`, whose
  `signing_bytes`/`context_bytes` framing was standardized by an earlier audit
  (BORU-AUDIT-02 hardening): it already embeds the `boru/mailbox` domain tag
  and the version inside the signed bytes and binds the same context into the
  AEAD associated data, so it satisfies the same invariant without routing
  through the shared helper.
- **Migration compatibility**: verification uses
  `verify_canonical_or_legacy` — it tries the new canonical bytes first and,
  only if that fails, the *legacy* framing (raw concatenation / bare tuple)
  produced by the pre-AUDIT-27 code.  This lets old persisted/wire objects
  continue to verify while new objects get the full domain-separated layout.
  New objects are always signed with the canonical layout.

## Tests

- Field-mutation test for every signed type: changing any security-relevant
  field invalidates verification.
- Golden-vector canonical-bytes tests: fixed inputs produce fixed bytes
  (postcard is deterministic).
- Cross-version tests: legacy framing still verifies (migration); unknown
  version values are rejected.
- Key round-trip tests: public keys serialize/deserialize to identical bytes
  (no string-normalization ambiguity).
