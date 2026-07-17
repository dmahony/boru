# Offline and Direct Messaging: Current Lifecycle Audit

Status: code audit only. This document describes the implementation currently
present in the repository; no runtime behaviour was changed for this audit.

The repository contains two related direct-message designs:

1. The live Iced GUI path uses Whisper for an immediate text DM and falls back
to an encrypted mailbox envelope when Whisper fails.
2. `src/store.rs`, `src/storage.rs`, and `src/retry.rs` contain a newer SQLite
repository/retry design. It is exercised by storage tests, but the GUI offline
DM path still calls the legacy `MailboxStore`/`OutboxStore` APIs rather than
wiring this repository into message creation and acknowledgement handling.

Therefore the SQLite design must not be described as the currently active
end-to-end GUI delivery path.

## Executive summary

The live GUI lifecycle is:

```
/whisper <peer> <text>
  -> resolve peer key
  -> try WhisperHandle::send_dm()
  -> if Whisper succeeds: show local system line; stop
  -> if Whisper fails and a stored mailbox key exists:
       seal_for() (X25519 + AES-GCM + sender signature)
       MailboxStore::enqueue_outgoing() (in-memory)
       MailboxStore::save() (currently a no-op)
       try inbox::send_deliver() over /iroh-chat-inbox/1
       if send succeeds: report Delivered
       if send fails: report Queued, but no durable retry is scheduled
  -> if Whisper fails without a mailbox key: show error
```

Incoming inbox delivery is:

```
QUIC connection on /iroh-chat-inbox/1
  -> reject connection unless transport peer is in allowed_senders
  -> verify SignedInboxMessage and timestamp
  -> deduplicate an in-memory protocol message hash
  -> emit InboxEvent::EnvelopeReceived
  -> GUI calls MailboxStore::accept_incoming()
  -> verify sender allowlist, envelope signature, recipient and AEAD
  -> show plaintext in the GUI
  -> MailboxStore::save() (currently a no-op)
  -> fire-and-forget send_ack()
```

The current implementation does not provide a verified, durable, end-to-end
outbox/ack lifecycle for GUI offline DMs. In particular, the GUI does not
insert these messages into `MessageStore`/`Storage`, does not start
`RetryWorker` for them, and ignores `WhisperEvent::MailboxEnvelope` and
`WhisperEvent::MailboxAck`.

## Components inspected

The audit covered:

- `src/inbox.rs`: signed inbox wire format, authorization, replay window,
  deduplication, sync request handling, and direct send helpers.
- `src/outbox.rs`: legacy JSON reader and compatibility type. `save()` is a
  no-op; SQLite is the intended durable store.
- `src/mailbox.rs`: envelope cryptography plus legacy in-memory mailbox reader.
  `save()` is a no-op in the current source.
- `src/retry.rs`: SQLite `RetryWorker`, 60-second poll, trigger channel, and
  exponential backoff helper.
- `src/whisper/mod.rs`: direct QUIC actor, wire frames, connection lifecycle,
  and mailbox-frame events.
- `src/contact.rs`: signed contact actions and deterministic direct topic.
- `src/friends.rs`: persisted friend records and optional advertised mailbox
  key.
- `src/friend_request.rs`: JSON friend-request state and authorization rules.
- `src/conversations.rs`: JSON conversation records keyed by `TopicId`.
- `src/chat_history.rs`: legacy room-history event IDs and delivery state.
- `src/chat_core.rs`: shared chat protocol types and diagnostics integration.
- `examples/iced_chat/main.rs`: router registration, inbox startup, and
  initial allowlist seeding.
- `examples/iced_chat/app.rs`: GUI DM creation, fallback, inbox events,
  reconnect sync, and ack handling.
- `src/store.rs` and `src/storage.rs`: SQLite schemas, repositories,
  outbox state, tombstones, and migrations.
- DM, mailbox, lifecycle, storage, and acknowledgement tests under `tests/`.

## Message creation and identity

### Whisper text DM

The `/whisper` command resolves an alias or public key and calls
`WhisperHandle::send_dm(peer, text)`. The Whisper wire frame contains the
sender string and text. It is carried on an authenticated QUIC connection,
but this path does not create a stable application message ID, conversation
ID, signed application envelope, or durable outbox row.

A successful Whisper send is reported as success to the GUI. The GUI adds a
system entry (`[Whisper to ...] ...`); it does not persist a direct-message
history record in `MessageStore`.

### Mailbox fallback

When Whisper returns an error and the peer has a cached
`MailboxPublicKey`, `seal_for()` creates a `MailboxEnvelope`:

- X25519 ephemeral-static Diffie-Hellman derives a key.
- BLAKE3 domain-separated key derivation feeds AES-256-GCM.
- The nonce is 12 random bytes.
- The sender signs the envelope fields.
- `message_id()` is BLAKE3 over the envelope signing bytes.

The plaintext is not sent in the inbox protocol. However, the fallback is
only reached after immediate Whisper failure and is not a general durable
queue.

### Conversation IDs

`contact::direct_topic(a, b)` deterministically hashes the ordered pair of
public keys with the `iroh-gossip-chat/direct/v1` domain. This is used by
conversation invitations/private rooms. The mailbox fallback itself does not
include a conversation ID: `MailboxEnvelope` has sender, recipient, ephemeral
key, nonce, ciphertext, creation time, and signature only.

### Other chat messages

Normal room messages use `SignedMessage::sign_and_encode`, a
`ChatHistoryStore` event ID, and a legacy JSON `OutboxEntry`. That is the room
broadcast lifecycle, not the `/whisper` offline-DM lifecycle.

## Durable storage and restart recovery

### Intended SQLite repository

`src/store.rs` defines `MessageStore` with `inbox`, `outbox`, `contacts`,
`sync_cursor`, `conversation_meta`, and `message_tombstones` tables. Its
outbox status enum is only:

```
Pending -> Sent -> Acked
                  \
                   -> Expired
```

`enqueue_outbox()` inserts a `(msg_id, recipient_device_id)` row. The inbox
uses `ON CONFLICT(msg_id) DO NOTHING`; conversation metadata updates unread
counts atomically; tombstones prevent resurrection; `mark_acked()` removes or
marks the recipient-specific outbox row. `src/storage.rs` provides the newer
`boru.db` repository and versioned migrations (currently schema version 4),
but the current GUI DM path does not call these methods.

### RetryWorker

`src/retry.rs` operates on `MessageStore`, not on `MailboxStore` or
`OutboxStore`. It processes due SQLite rows on a 60-second timer or an
explicit trigger. The backoff helper returns 5s, 30s, 2m, 10m, 30m, 2h, and
6h maximum. A successful network send records an attempt and leaves the row
awaiting an application ACK; an error records the error and schedules the
next attempt.

No call site was found that queues the GUI mailbox fallback into this worker.
Consequently, a fallback reported as `Queued` is not proven to survive process
restart or retry automatically.

### Legacy JSON stores

`OutboxStore` and `MailboxStore` still deserialize old JSON files and retain
some in-memory compatibility methods. Their current `save()` implementations
return success without writing. `MailboxStore::accept_incoming` and
`enqueue_outgoing` therefore do not establish durable restart recovery in the
current source. Existing tests that call older APIs such as `with_ttl`,
`enqueue`, `pending`, and `acknowledge` are stale relative to these structs.

## Transport and protocol behaviour

### Inbox protocol (single offline-delivery path)

`INBOX_ALPN` is `/iroh-chat-inbox/1`. Frames are a 4-byte big-endian length
followed by postcard bytes. `SignedInboxMessage` signs timestamp plus inner
payload and enforces a 24-hour timestamp skew window. The handler checks the
QUIC remote identity against `allowed_senders` before accepting streams.

Supported payloads are:

- `Deliver(MailboxEnvelope)`
- `Ack(MailboxAck)`
- `SyncRequest { since_ms }`
- `SyncResponse { envelopes }`
- `DeleteTombstone(AuthorDeleteProof)`

For `Deliver`, transport-level replay tracking and durable application-level
idempotency are separate. The handler authenticates the sender and emits the
envelope to the GUI; the mailbox acceptance transaction is authoritative for
whether a message is new or a valid duplicate. A valid duplicate is not
reinserted, but still receives a regenerated signed ACK.

The incoming stream receives only a one-byte minimal response for normal
messages. The actual signed `MailboxAck` is sent later by the GUI through a
new outbound connection. Thus a successful handler return is not itself proof
that the sender received an application ACK.

### Sync

A reconnecting peer sends `SyncRequest { since_ms }`, where `since_ms` is a
resume hint for the last envelope timestamp it has processed. The request is
not an authorization mechanism and is not trusted as an unrestricted query:
the serving mailbox clamps it to its local retention window, filters by the
authenticated requester's recipient identity, and returns a deterministic page
ordered by `(created_at, message_id)`. Each page is capped at 64 envelopes and
512 KiB of encoded envelope data. A peer resumes pagination from the last
returned timestamp; equal-timestamp replay is safe because normal incoming
acceptance is idempotent by message ID.

`send_sync_request()` verifies the signed response before returning it. Every
returned envelope must then pass the same recipient, sender authorization,
expiry, signature, decryption, and durable idempotent acceptance path as an
ordinary `Deliver`; sync is replay/backfill, not a second delivery protocol.
The serving GUI installs the provider from the durable mailbox store before
starting the inbox protocol. If no provider is installed, a `SyncRequested`
event is only an observation and no response is produced.

### Whisper mailbox frames (compatibility only)

Whisper has `MailboxEnvelope` and `MailboxAck` wire variants for wire
compatibility and lower-level tests, and emits corresponding events. The GUI
does not use those variants for offline delivery. The sole active fallback is
the separate inbox ALPN helper `send_deliver`/`send_ack`; the same envelope must
not be queued in both transports.

## Authorization and key exchange

`ContactAction::MailboxAdvertise` exists and is signed by the contact layer.
`FriendRecord` can persist a `MailboxPublicKey`. At startup, `main.rs` seeds
inbox `allowed_senders` from stored friend records that already contain a
mailbox key.

The GUI event match handles friend requests, accepts/rejects, conversation
invites, and address updates, but no `MailboxAdvertise` branch was found in
`examples/iced_chat/app.rs`. Therefore the complete advertised-key exchange
is defined at the protocol type level but is not observed as wired through the
current GUI event path. A peer without a cached mailbox key cannot enter the
fallback path, and a peer not present in the startup allowlist is rejected at
inbox connection acceptance.

Removing an allowed sender affects future inbox connections. Already stored
legacy in-memory entries are not automatically removed. Whisper authorization
is separate: peers are allowed by default unless explicitly placed in the
Whisper denied set.

## Acknowledgements

`MailboxAck::sign()` signs `(recipient_public_key, message_id)`. `verify()`
checks both the expected recipient identity and signature.

The GUI recipient path sends an ack after successful envelope decryption and
plaintext conversion. The send is fire-and-forget: the result is discarded,
and no ack retry queue exists in this path. The GUI sender path receives
`InboxEvent::AckReceived`, removes an outgoing legacy mailbox entry when
`acknowledge_outgoing_and_save()` succeeds, and updates an in-memory pending
status entry. It does not verify the ACK with `MailboxAck::verify()` before
removal, and it does not call SQLite `mark_acked()`.

The SQLite repository does expose `mark_acked()`, but that is only relevant
when the message was inserted into the SQLite outbox. No GUI call chain from
`/whisper` to that repository was found.

## Test coverage and verification

The test suite contains useful lower-level coverage:

- `src/inbox.rs` tests signing, sender mismatch, tampering, topic derivation,
  and allowlist mutation.
- `src/mailbox.rs` contains an envelope round-trip unit test.
- `src/whisper/mod.rs` contains live two-peer mailbox-frame coverage.
- `src/store.rs` tests SQLite inbox/outbox idempotency, ACK state, expiry,
  conversation metadata, and tombstones.
- `tests/test_offline_delivery_integration.rs` tests the SQLite storage model
  with deterministic simulated delivery, restart, duplicates, expiry, and
  ACK scenarios.
- `tests/test_message_lifecycle.rs` tests the legacy JSON room-history and
  outbox state model.
- `tests/mailbox.rs` describes the older persistent MailboxStore contract,
  but currently references APIs removed from the implementation (`with_ttl`,
  `enqueue`, `pending`, `acknowledge`, and related methods).

Verification command run during this audit:

```
cargo check --all-targets
```

It did not complete. The current checkout fails first with duplicate module
registration for `abuse_controls` in `src/lib.rs` (at lines 235 and 337).
The command also reports stale/unused-code warnings. This compile result is
repository-wide and is not evidence that a live network DM succeeds.

## Evidence-based stage matrix

| Stage | Current evidence | Result |
|---|---|---|
| Peer key resolution | `/whisper` resolves alias/public key | Observed in code |
| Whisper connection | `send_dm` attempts direct connection/discovery | Attempted; runtime success unknown |
| Immediate Whisper delivery | GUI treats `send_dm == Ok` as success | Reported by caller, no recipient ACK |
| Mailbox key availability | Optional friend field | Conditional; exchange not wired in GUI |
| Envelope encryption/signing | `seal_for` implementation and unit test | Observed |
| Durable fallback insertion | GUI calls legacy `enqueue_outgoing`; `save` is no-op | Not durable |
| Inbox authorization | `allowed_senders` checked at connection accept | Observed when configured |
| Inbox envelope verification | Signed wrapper plus mailbox validation in GUI | Observed in code |
| Persistent recipient insertion | GUI uses legacy in-memory MailboxStore | Not observed |
| Recipient-visible plaintext | GUI adds a chat entry after decrypt | Implemented; runtime delivery unverified |
| ACK signature verification | Type method exists; GUI ack path does not call it | Not observed in active GUI path |
| ACK persistence/removal | Legacy in-memory removal only | Not durable |
| SQLite outbox/retry integration | Repository and worker exist | Not connected to GUI DM path |
| Restart recovery | SQLite tests cover model; GUI fallback save is no-op | Not observed for live GUI fallback |
| Reconnect sync response | Requires `pending_fn`; no setup found | Not observed |
| Symmetric delivery | No two-node live probe performed in this audit | Unknown |

## First unresolved failure stage

For a live `/whisper` offline message, the furthest reliably established
stage from source inspection is envelope construction and a best-effort
inbox send. The first stage that cannot be established as reliable is
**durable queue insertion**: the GUI writes only to the legacy in-memory
`MailboxStore`, whose `save()` is a no-op. If that send fails, retry and
restart recovery are not demonstrated.

For reconnect-based mailbox replay, the first unestablished stage is
**SyncResponse production** because `pending_fn` is optional and no startup
configuration was found. For ACK completion, the first unestablished stage
is **verified, durable ACK processing** because the GUI discards verification
errors/results and does not update the SQLite outbox.

## Remaining unknowns

This is a static audit plus a compile check; it does not establish runtime
peer discovery, connection success, topic membership, or symmetric delivery.
To close those gaps, use the running diagnostic MCP interface to collect both
nodes' node/room/peer status and discovery timelines, then perform a two-way
probe with unique IDs. The required runtime evidence is:

- both node IDs and active protocol/room state;
- each node's view of the other's discovery source, addresses, and connection;
- inbox allowlist/key state on both sides;
- ordered connection, delivery, sync, and ACK diagnostic events;
- probe IDs and received-probe records in both directions.

Without that live evidence, runtime delivery is Unknown rather than failed or
successful by assumption.
