# Private Group Chat System

## Overview

Boru's private group chat system enables ephemeral or long-lived conversations
among a set of authenticated peers. The system is built on four core concepts
that together provide stable identity, secure transport, private discovery, and
membership lifecycle management.

## Core Concepts

### GroupId — Stable Conversation Identity

`GroupId` is a 32-byte cryptographically random identifier that uniquely
identifies a group conversation over its entire lifetime. It is generated at
group creation time and **never changes**, regardless of membership changes,
epoch rotations, or transport reconfiguration.

```
GroupId([u8; 32]) — generated via getrandom (CSPRNG)
```

- Generated with `GroupId::generate()`
- Immutable for the life of the group
- Used in signed control events (`MemberRemovedEvent`, `EpochChangedEvent`) to
  bind them to a specific group
- Format: hex-encoded when serialized (`Display`, `Serialize`)
- Defined in: `src/group_id.rs`

**Key distinction:** A `GroupId` is what a conversation *is*. A `TopicId` is
how it currently *travels* over the wire. They are independent types.

### TopicId — Current Gossip Transport Identity

`TopicId` is a 32-byte identifier that namespaces gossip traffic on the
iroh gossip mesh. In the private group system, the `TopicId` changes with
every epoch rotation so that removed members cannot eavesdrop on gossip.

- Each epoch has its own `TopicId`
- Used as the `group_id` field inside `GroupEventEnvelope` (gossip-scoped)
- The `EpochCredentials` struct bundles `(group_id, epoch, topic, discovery_secret)`
- Defined in: `src/proto/topic.rs` (re-exported as `TopicId`)

**Typical mapping:**

```
Group "Project X" (GroupId: 0xa1b2...)
│
├─ Epoch 0 → TopicId: T0, DiscoverySecret: DS0
├─ Epoch 1 → TopicId: T1, DiscoverySecret: DS1
├─ Epoch 2 → TopicId: T2, DiscoverySecret: DS2
└─ ...
```

### DiscoverySecret — Current Private Discovery Capability

`DiscoverySecret` is a 32-byte cryptographically random value that controls
who can find the group on the DHT. It is the bearer capability for private
room discovery: knowing the secret lets a peer:

- Derive the room's DHT namespace: `BLAKE3("private-room v1" || topic || secret)`
- Publish valid discovery records on that namespace
- Verify and decrypt records published by other members

Like the `TopicId`, the `DiscoverySecret` changes with every epoch rotation.

- Redacted in `Debug` output (only first 4 hex bytes visible)
- `Clone` intentionally documented as a sensitive operation
- Constant-time `PartialEq` comparison
- Defined in: `src/discovery_secret.rs`

**Security property:** A peer that knows the gossip `TopicId` but not the
`DiscoverySecret` cannot discover or impersonate room members on the DHT.

### Epoch — Membership / Security Generation

An epoch is a monotonically increasing `u64` counter representing one
generation of the group's membership and credentials. Epochs start at 0
and are incremented each time the owner removes a member.

Each epoch carries its own set of credentials (`EpochCredentials`):

```rust
pub struct EpochCredentials {
    group_id: GroupId,         // stable group identity
    epoch: u64,               // generation counter
    topic: TopicId,           // gossip transport for this epoch
    discovery_secret: DiscoverySecret,  // DHT discovery for this epoch
}
```

- Epoch 0: created when the group is first established
- Epoch N+1: created atomically when a member is removed (see Epoch Rotation)
- The `EpochChanged` group event records the advance
- Defined in: `src/group_epoch.rs`

## Group Membership Lifecycle

### Roles

| Role | Authority |
|------|-----------|
| **Owner** | Can invite, remove members, change metadata, advance epoch |
| **Member** | Can join (after invitation), leave voluntarily, send messages |

There is exactly one owner per group. The owner is established at creation time
and cannot be transferred (this is a current limitation).

### 1. Group Creation

The owner generates a fresh `GroupId`, creates `EpochCredentials` at epoch 0,
and initialises a `GroupState` with themselves as the sole member.

```text
Owner ──► GroupState::new(topic, owner_pubkey)
         ├── epoch = 0
         ├── members = {owner → Owner}
         ├── invited = {}
         └── seen = {}
```

### 2. Invitations

The owner invites a peer by creating a signed `MemberInvited` event. The
invited peer's public key is added to the `invited` set.

Invitations are shared out-of-band via one of two formats:

**RoomInviteV2 (stable, preferred):**
```
boru1:<base32-nopad-lowercase of [version: u8, topic: [u8; 32], discovery_secret: [u8; 32]]>
```
- No endpoint/relay information
- ~105 characters + `boru1:` prefix
- Carries the `DiscoverySecret` so the invitee can find the room on the DHT
- Defined in: `src/chat_core.rs` (struct `RoomInviteV2`)

**Legacy Ticket:**
```
<base32-nopad-lowercase of postcard-encoded Ticket>
```
- Carries bootstrap peer addresses and optional discovery secret
- Backward compatible

### 3. Joining

An invited peer joins by creating a signed `MemberJoined` event. Validation
checks:

1. The event is in the correct epoch
2. The actor is in the `invited` set
3. The payload's `member` matches the actor
4. The signature is valid

On success, the peer moves from `invited` to `members`.

### 4. Leaving

A member voluntarily leaves by creating a signed `MemberLeft` event. The event
is valid when the actor is a current member and the payload's `member` matches
the actor. The member is removed from the `members` map.

### 5. Owner Removal and Epoch Rotation

When the owner removes a member, the system performs an **atomic** two-part
operation:

**Part A — Signed Control Events (gossip-safe):**

Two signed events are broadcast to the gossip mesh:

1. `MemberRemoved { group_id, epoch, member, actor, timestamp, signature }`
   — records the removal authorisation
2. `EpochChanged { group_id, old_epoch, new_epoch, actor, timestamp, signature }`
   — records the epoch advance

These events are public and safe to gossip because they carry no credential
material. They are **not** the same as the `MemberRemoved` and `EpochChanged`
variants in the `GroupEvent` enum (which apply to event-level state), though
they are semantically linked.

**Part B — Encrypted Credential Delivery (mailbox-sealed):**

The new `EpochCredentials` (with a fresh `TopicId` and `DiscoverySecret`) are
delivered only to surviving members through individually encrypted mailbox
envelopes:

```text
Owner
├── For each survivor S:
│   ├── Encrypt new EpochCredentials with S's mailbox public key
│   └── Create CredentialDelivery { recipient: S, envelope }
└── Removed member is explicitly excluded
```

Each survivor opens their delivery with `rotation.open_for(&identity)`.

The removed member:
- Receives the `MemberRemoved` and `EpochChanged` events (public gossip)
- Cannot decrypt the new epoch credentials
- Cannot participate in the new gossip topic
- Is still a member of epoch 0's `GroupState` but cannot access epoch 1's
  transport or discovery

**Implementation:** `EpochRotationState::rotate_after_removal()` in
`src/group_epoch.rs`

### 6. Backfill — History for Late Joiners

When a peer joins an existing group, or reconnects after a long absence, it may
have few or no messages. The backfill protocol lets it request history from any
connected peer:

**Protocol:** `/iroh-gossip-chat/backfill/1` (dedicated QUIC ALPN)

1. Requester opens a bi-directional QUIC stream to a responder
2. Sends a `BackfillRequest` asking for up to `max_messages` messages
3. Responder queries its `ChatHistoryStore` and returns raw signed message bytes
4. Requester verifies and decodes each message through the normal `NetEvent`
   channel

**Limits:**
- At most one backfill request per remote `PublicKey` served concurrently
- Server caps at 50 messages per response (`SERVER_MAX_BACKFILL`)
- Default trigger: request backfill when local log has fewer than 20 messages
  (`BACKFILL_TRIGGER_THRESHOLD`)
- 5-second timeout per exchange (`BACKFILL_REQUEST_TIMEOUT`)

**Safety:** Backfilled messages pass through the same verification pipeline as
live gossip messages, including signature verification and epoch checking.

Defined in: `src/backfill.rs`

## Group Event System

Group membership is governed by signed, versioned control events. Every event
is wrapped in a `GroupEventEnvelope`:

```rust
pub struct GroupEventEnvelope {
    version: u8,                      // GROUP_EVENT_VERSION = 1
    group_id: TopicId,                // gossip topic (epoch-scoped)
    event_id: [u8; 16],               // BLAKE3 hash for replay protection
    epoch: u64,                       // current group epoch
    actor: PublicKey,                 // signing peer
    timestamp: u64,                   // UNIX seconds
    payload: GroupEventPayload,       // the operation
    signature: [u8; 64],             // Ed25519 over all other fields
}
```

### Event Types

| Event | Actor | Effect |
|-------|-------|--------|
| `MemberInvited` | Owner | Adds target public key to `invited` set |
| `MemberJoined` | Invited peer | Moves from `invited` to `members` |
| `MemberLeft` | Member | Removes self from `members` |
| `MemberRemoved` | Owner | Removes target from `members` |
| `MetadataChanged` | Owner | Updates room name/description |
| `EpochChanged` | Owner | Advances `state.epoch` counter |

### Validation Pipeline

Every event passes through `GroupEvent::verify()` which checks in order:

1. **Version** — must match `GROUP_EVENT_VERSION` (1)
2. **Group** — `envelope.group_id` must match `state.group_id`
3. **Epoch** — `envelope.epoch` must match `state.epoch` (or be strictly greater for
   `EpochChanged` payloads)
4. **Payload variant** — envelope payload must match the event enum variant
5. **Payload size** — encoded payload must not exceed 16 KiB
6. **Timestamp** — must be within 24 hours of the local clock
7. **Signature** — Ed25519 signature verification against the actor's public key
8. **Replay** — event ID must not have been seen before
9. **Membership** — actor must be in the appropriate role for the operation
10. **Permission** — actor must be authorised to perform the operation (owner-only
    operations require `Role::Owner`)

### Authoritative State

`GroupState` is the authoritative source of membership truth:

```rust
pub struct GroupState {
    group_id: TopicId,
    owner: PublicKey,
    epoch: u64,
    members: HashMap<PublicKey, Role>,   // includes owner
    invited: HashSet<PublicKey>,
    seen: HashSet<[u8; 16]>,             // replay protection
}
```

**Important:** A roster snapshot (e.g. from `members()`) is only a projection.
The `GroupState` itself, as derived from verified events, is what determines
membership. The roster is never consulted to grant access.

### Wire Protocol Constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `GROUP_EVENT_VERSION` | 1 | Current wire format version |
| `MAX_GROUP_EVENT_PAYLOAD` | 16 KiB | Maximum encoded payload size |
| `MAX_GROUP_EVENT_CLOCK_SKEW_SECS` | 86,400 (24h) | Allowed clock drift |
| Event ID length | 16 bytes | BLAKE3 truncated hash |

## Relationship Summary

```
GroupId (stable, never changes)
  │
  └── serves as the durable identity for the conversation
      └── each Epoch adds a (TopicId, DiscoverySecret) pair
           │
           ├── TopicId: the gossip topic for message transport
           │   - changes every epoch
           │   - removed members cannot listen on the new topic
           │
           └── DiscoverySecret: the DHT discovery capability
               - changes every epoch
               - removed members cannot find the room on the DHT
               - HPKE-encrypted records on a per-room DHT namespace
```

### How they interact in practice

| Operation | GroupId | TopicId | DiscoverySecret | Epoch |
|-----------|---------|---------|-----------------|-------|
| Create group | Generated | Generated at epoch 0 | Generated at epoch 0 | Set to 0 |
| Invite member | Referenced | Used as `group_id` in event | Referenced in ticket | Checked |
| Join group | Referenced | Used to subscribe to gossip | Used for DHT discovery | Checked |
| Send message | Referenced | Determines which gossip mesh | N/A (gossip = authenticated) | Checked |
| Remove member | Referenced | Rotated to new value | Rotated to new value | Incremented |
| Rejoin after removal | Referenced | Must get new credentials | Must get new credentials | Must be current |

## Security Assumptions

### What the system guarantees

1. **Authenticated membership** — Every group event is Ed25519-signed. Only the
   owner can invite, remove, or change metadata. Only invited peers can join.

2. **Replay protection** — Each event carries a unique `event_id` derived from
   a BLAKE3 hash of (actor, group_id, epoch, timestamp, payload). Once applied,
   the ID is recorded and replayed events are rejected.

3. **Byzantine fault tolerance for events** — All peers independently validate
   every event against the same `GroupState` rules. A compromised or malicious
   peer cannot forge owner signatures or inject unauthorised state changes.

4. **Malleability resistance** — Signatures cover every field of the envelope.
   No field can be modified without invalidating the signature.

5. **Clock bounding** — Events with timestamps more than 24 hours away from the
   local clock are rejected, limiting temporal replay windows.

6. **Backfill safety** — Backfilled messages pass through the same
   verification pipeline as live messages. A malformed or forged backfill
   response cannot inject unverified state.

### What the system does NOT guarantee

1. **Forward secrecy after removal** — A removed member who **persisted** the
   epoch-0 `TopicId` and `DiscoverySecret` may still be able to:
   - Decrypt previously captured gossip traffic from epoch 0
   - Discover epoch-0 peers on the DHT

   This is a known limitation: earlier epoch credentials are not rotated
   proactively. The system rotates credentials atomically *at removal time*,
   but old credentials are not revoked retroactively.

2. **Epoch rotation is not yet mandatory** — At this stage, epoch rotation
   happens only as part of the owner removal flow
   (`rotate_after_removal`). There is no periodic or policy-driven rotation
   mechanism. Groups without removals remain in epoch 0 indefinitely.

3. **No forward secrecy for messages** — Individual messages are not
   ephemeral-key encrypted. A peer that is a member at time T can read all
   messages from time T onward while it remains a member. Messages are
   encrypted in transit by QUIC/TLS 1.3 but not with per-message forward
   secrecy.

4. **No post-removal credential revocation** — Once a removed peer has
   received the `MemberRemoved` and `EpochChanged` events (which are public
   gossip), they know a rotation happened. But if they captured the epoch-0
   `TopicId` before removal and can still connect to a misconfigured peer
   still using the old topic, they might receive traffic. Peers are expected
   to drop the old topic subscription after processing the epoch change.

5. **Owner is a single point of trust** — The owner can unilaterally remove
   any member. There is no multisig, quorum, or appeals mechanism. The owner
   secret key must be protected accordingly.

6. **No message integrity after member removal** — Removed members retain
   any messages they received before removal. The system does not support
   remote message deletion or recall.

7. **The database is not encrypted at the file level** — See
   [`security-model.md`](security-model.md) for full details on at-rest
   storage protections and their limits.

8. **No disclosure control for the `MemberRemoved` event** — The signed
   `MemberRemovedEvent` and `EpochChangedEvent` are broadcast on the public
   gossip mesh. Any peer on the topic mesh (including the removed member)
   receives and can verify these events. Removal is not deniable.

### Security architecture at epoch boundaries

```
                   Epoch N                                  Epoch N+1
  ┌──────────────────────────────┐      ┌──────────────────────────────┐
  │ TopicId: T_N                 │      │ TopicId: T_{N+1} (new)       │
  │ DiscoverySecret: DS_N        │      │ DiscoverySecret: DS_{N+1}    │
  │ Members: {A (owner), B, C}   │ ──►  │ Members: {A (owner), C}      │
  │                              │      │                              │
  │ B removed via signed event   │      │ B cannot decrypt new creds   │
  │ Public: MemberRemoved{ B }   │      │ B cannot subscribe to T_{N+1}│
  │ Public: EpochChanged{ N→N+1 }│      │ B cannot discover DS_{N+1}   │
  └──────────────────────────────┘      └──────────────────────────────┘

  Credential delivery (encrypted, not gossiped):
    A → C: seal(DS_N+1 || T_N+1, mailbox_pubkey(C))
    B:    (no delivery — explicitly excluded)
```

## BLEP (Boru Limitations and Extension Points)

### Current limitations

- **Single owner model** — No support for co-owners or multi-sig operations
- **No owner transfer** — The group dissolves if the owner's key is lost
- **No epoch rotation triggers other than removal** — No periodic rotation,
  no rotation-on-leave, no rotation for key compromise
- **No offline delivery for epoch credentials** — Credentials are delivered
  over mailbox, but if a survivor is unreachable at rotation time, they must
  obtain the new credentials through a sync/reconnect path
- **No group merging or splitting** — Groups cannot be combined or partitioned
- **No membership expiration** — Invitations and memberships are permanent
  until explicitly revoked

### Extension points (not yet implemented)

- **V2 DiscoverySecret subkeys** — Domain-separated subkeys for namespace,
  encryption, and signing are designed (functions `subkey_namespace`,
  `subkey_encryption`, `subkey_signing` exist in `discovery_secret.rs`) but
  not deployed on the wire. V1 still uses the raw secret for all three
  purposes.
- **Periodic epoch rotation** — Infrastructure exists for rotation, but no
  timer-driven policy is wired in.
- **Message-level forward secrecy** — Could be added with per-message
  ephemeral keys and ratcheting.

## Related Documentation

- [`protocol-layers.md`](protocol-layers.md) — Wire protocols including
  gossip, inbox, backfill, and discovery
- [`security-model.md`](security-model.md) — Transport security, at-rest
  storage, and cryptographic properties
- [`privacy-model.md`](privacy-model.md) — Metadata exposure, DHT privacy,
  and local-path protection
- [`discovery-architecture.md`](discovery-architecture.md) — Full DHT
  discovery architecture for public and private rooms
