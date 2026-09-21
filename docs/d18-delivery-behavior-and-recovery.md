# D18 delivery behavior, recovery, and receipt contract

Date: 2026-09-21
Status: documented contract; live offline-queue acceptance remains unverified

This note is the hand-off contract for the intended direct-message delivery
behavior. It describes what an operator may conclude from each state and how to
reproduce the required failure/recovery demonstration without turning a fixture
or screenshot into a loss-free claim. The current GUI `/whisper` fallback is
still documented as using legacy in-memory mailbox APIs; the SQLite ownership
rules below are the required architecture/evidence boundary, not a claim that
that GUI path has already been proven end to end.

## User-visible states and reasons

The sender renders one bubble per stable `message_id`. Delivery state is held in
SQLite and projected into the UI; the bubble must not be recreated for retries.

| State | Meaning | Valid reasons / transitions |
| --- | --- | --- |
| `Queued` | The message is durably admitted locally but has not been confirmed by the recipient. | Recipient offline, no usable route, transient transport failure, or sender restart recovery. It is not proof that the recipient has the message. |
| `Sent` | An attempt transmitted the envelope, but the application receipt is still absent. | The peer may have received it, the connection may have failed after transmission, or the receipt may have been lost. This is intentionally ambiguous and remains retryable. |
| `Delivered` | A verified, recipient-bound ACK for this exact message ID was committed. | Only `MailboxAck::verify` plus sender/recipient/message-ID binding may produce this state. A successful socket write, handler return, or local timeout cannot produce it. |
| `Failed` / `Expired` | Delivery will not continue automatically. | Permanent authorization/identity error, malformed or invalid envelope, explicit cancellation, retention deadline, or retry policy exhaustion. The UI must retain the reason code and must not imply delivery. |

`Read`/`Seen` is separate from delivery. `Delivered` means the recipient's
application accepted the message; `Read` means the recipient UI reported that
the conversation was visible and the message was actually observed. Receiving,
persisting, syncing, or acknowledging in the background must not mark `Read`.
Older peers that do not support receipts stay unconfirmed rather than being
promoted by inference.

## ACK semantics and ambiguity

The receiver ACK is an application receipt, not a transport receipt. It is
signed over the recipient identity and stable message ID and is accepted only
when the sender verifies the signature, expected recipient, and exact message
binding. The receiver's durable insert is idempotent (`INSERT ... ON CONFLICT`
or equivalent); a retransmission after a lost ACK therefore produces one local
message row and one UI bubble, while allowing the sender to retry.

A sender in `Sent` must retry because it cannot distinguish “recipient did not
receive” from “recipient received but the ACK was lost.” This is at-least-once
transport with exactly-once local presentation, not exactly-once network
transmission. Invalid, late, forged, or mismatched receipts are ignored or
recorded as failures and never advance the state.

## Ownership and single source of truth

1. The SQLite outbox owns sender admission, stable message identity, attempt
   count, retry deadline, lease, expiry, and terminal state.
2. One delivery worker owns a claimed outbox row at a time. Leases expire and
   crash recovery returns `Sending`/`Sent` rows to retryable `Pending`.
3. The envelope and message ID are bound at admission. A retry reuses the same
   envelope and ID; a user-created new send gets a new intent and ID.
4. The receiver's SQLite inbox/message table is authoritative for deduplication
   and presentation. Sync/replay is another input to the same acceptance
   transaction, not a second message path.
5. Receipt processing is committed before the UI projects `Delivered`. UI state,
   diagnostics, and screenshots are projections and cannot be authoritative.
6. Startup recovery runs before delivery workers; migration is forward-only and
   must preserve queued identities. A failed migration leaves the previous
   transaction intact and is retried on the next open. Rollback means restoring a
   compatible database backup/application version, not downgrading schema in
   place.

## Limits, expiry, and retention

Room broadcast limits and direct-message retention are separate policies. A room
may cap members, encoded frame/page size, or queued work; exceeding a limit is a
bounded failure and must not silently drop an admitted local row. Inbox sync is
paged and bounded (the current protocol contract documents 64 envelopes and
512 KiB per page). Direct envelopes expire according to their authenticated
creation time and local retention policy. Expired rows are terminal and are not
revived by reconnect, replay, or restart. Tombstones/deduplication records must
outlive the replay window so a late duplicate cannot create a second bubble.

Retention is not an availability guarantee: a recipient that remains offline
past expiry may never receive the message. The UI should distinguish `Expired`
from `Failed` and include a bounded reason code. Do not claim unlimited offline
storage or loss-free delivery.

## Reproducible acceptance demonstration

Use two isolated data directories and two real Boru processes (or two operator
controlled hosts) with a seeded direct-message/mailbox identity. Record process
IDs, stable message ID, SQLite rows, ACK events, and UI bubble counts; do not
use a mock-only workflow as transport evidence.

1. Start recipient B, then stop it cleanly. Start sender A and submit one unique
   marker through the normal composer. Verify exactly one sender outbox row in
   `Queued`/`Pending`, with its stable ID and body/envelope persisted.
2. Kill and restart A using the same data directory, without resubmitting. Verify
   the same outbox ID and envelope are recovered and the UI still has one bubble.
3. Start B and allow the normal reconnect/retry path. Verify B has one durable
   receiver row and one visible bubble. Verify A reaches `Sent` only until a
   genuine verified ACK is observed, then `Delivered`.
4. To exercise the receipt-loss boundary, arrange for B to commit the receive and
   then stop before its ACK reaches A. Restart B. Let A retry automatically.
   Verify B's deduplication leaves one row/one bubble, while A eventually reaches
   `Delivered` after a fresh valid ACK.
5. Verify `Read` remains unset until B's conversation is visible and the message
   is actually observed. Close or background the conversation and repeat; a
   background ACK must not create `Read`.
6. Capture the negative cases separately: forged/wrong-ID ACK, expired envelope,
   permanent authorization failure, and a room/page limit. Each must leave a
   bounded failure/expiry reason and no duplicate bubble.

Acceptance is **UNVERIFIED** in this checkout for the full real offline queue,
receipt-loss, and GUI bubble sequence: D16 explicitly ran real process/restart
and two-process probes but did not inject a queued direct message or prove a
genuine `Delivered` receipt. D18 therefore supplies the reproducible procedure
and evidence requirements, not a fabricated pass.

## Platform boundary

The procedure must be repeated on every release platform intended for support.
Linux/X11 process evidence does not prove Windows or macOS runtime behavior;
relay/DHT, sleep/resume, route changes, and older-peer compatibility require
separate real-network evidence. Deterministic storage and crash-boundary tests
are valuable evidence, but they do not replace the two-process demonstration.
