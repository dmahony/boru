# Boru separate mobile-app contract

Status: authoritative v1 contract for a separate Android/iOS companion client.

This document describes the implemented desktop boundary in
`src/companion_protocol.rs` and the durable companion storage in
`src/store/companion.rs`. It is intentionally independent of the Iced desktop
UI and does not copy the desktop identity model. A mobile client may share
Boru's cryptographic/network libraries, but its UI, lifecycle, secure storage,
and notification implementation are platform-specific.

## 1. Transport and framing

- Connect to the desktop's existing authenticated Iroh endpoint using ALPN
  `/boru/companion/1`.
- The endpoint identity in the QR invitation is a routing/pinning hint; it is
  not authorization by itself. The client must verify the authenticated remote
  endpoint identity equals `host_endpoint_id` before continuing.
- Every bidirectional stream carries exactly one frame in each direction:
  `u32` big-endian byte length followed by one UTF-8 JSON value.
- Reject a frame before allocating when its length exceeds 65,536 bytes.
- The desktop applies a 10-second deadline to each frame. Treat timeout,
  truncated input, invalid JSON, and connection close as transport failure.
- JSON uses tagged enums: the discriminator is the lower-case snake-case
  `type` property. Unknown request types and unsupported methods return a stable
  error; clients must not infer success from a closed stream.
- Current wire version is integer `1`; negotiate it with `hello` before any
  other request. No account/profile/history data is available before approval.

The companion protocol is separate from the discovery control plane and chat
wire protocol. Discovery capabilities do not grant companion access.

## 2. Pairing transcript

### 2.1 Desktop invitation

The desktop creates one active invitation at a time. Creating a new one
invalidates the previous invitation. The invitation expires after 120 seconds,
and a desktop restart or disabling companion linking erases it and all pending
claims.

`CompanionInvitation` is serialized as JSON and then hex encoded for QR/text
transport:

```json
{
  "version": 1,
  "host_endpoint_id": "<iroh endpoint id>",
  "routing_hints": ["<opaque relay/direct hint>"],
  "invitation_id": [16 byte values],
  "secret": [32 byte values],
  "expires_at_ms": 1770000000000
}
```

The secret is bearer material. Never log it, put it in analytics, or include it
in a crash report. The desktop redacts it from debug output.

### 2.2 Client transcript

1. Client decodes the hex invitation, checks version `1`, exact 32-byte secret,
   16-byte invitation id, and expiry.
2. Client connects to the pinned host endpoint and sends `hello` with the
   versions and capability names it understands.
3. Client sends `pair_begin` with the original encoded invitation and a
   user-facing `device_name`.
4. Desktop validates the active invitation and binds the first claim to the
   authenticated client endpoint identity. It returns `pair_started` with the
   invitation id and a six-digit comparison `code`.
5. The desktop displays the same comparison code. The user compares both
   screens and explicitly approves or rejects the device on the desktop.
6. Client polls `pair_status` with the invitation id. `pending` means keep
   polling with bounded backoff; `approved` permits authenticated calls;
   `rejected` is terminal. `unknown` is terminal for this invitation.
7. Approval creates a durable registration. The client receives the
   registration id out-of-band from the approved pairing result in the host
   integration; it must persist it with the client device key. If the host
   integration does not return registration metadata, the client must remain in
   the approved-but-not-synced state and ask the user to relink rather than
   guessing identifiers.

The comparison code is a transcript check only. It is not a password, bearer
token, or replacement for the authenticated endpoint identity.

### 2.3 Pairing errors and suppression

Expired, malformed, superseded, already-used, or wrong-device invitations are
reported as `invalid_invitation`. A rejected claim is reported as `rejected`.
After rejection, suppress retries for that invitation id. Never retry an old
pairing after a new invitation is generated.

The unauthorised `pairing_request` and `invitation_claim` forms remain supported
for probes/older integrations, but a production mobile client should use
`pair_begin` and `pair_status`.

## 3. Message schemas

### Requests

```json
{"type":"hello","versions":[1],"capabilities":["pairing","history-v1"]}
{"type":"pairing_request","device_name":"Dan's phone"}
{"type":"pair_begin","invitation":"<hex invitation>","device_name":"Dan's phone"}
{"type":"pair_status","invitation_id":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]}
{"type":"host_status","request_id":"req-1","registration_id":[1],"device_id":[32 byte values],"grant_revision":1}
{"type":"snapshot_begin","request_id":"req-2","registration_id":[1],"device_id":[32 byte values],"grant_revision":1}
{"type":"changes_resume","request_id":"req-3","registration_id":[1],"device_id":[32 byte values],"grant_revision":1,"token":"<opaque snapshot token>","after_sequence":0,"limit":100}
{"type":"authenticated_request","request_id":"req-4","registration_id":[1],"device_id":[32 byte values],"grant_revision":1,"capability":"conversations.list","method":"conversations.list","params":{"limit":50}}
```

`registration_id`, `device_id`, and invitation ids are JSON arrays because that
is the serde representation of the Rust `Vec<u8>`/`[u8; 16]` fields. Preserve
bytes exactly; do not convert them to platform UUIDs.

Authenticated v1 methods currently implemented:

- `conversations.list`: params `limit` (default 50, maximum 100) and optional
  opaque `after` cursor. Returns conversation metadata, a snapshot value, a
  stable `next` cursor, `end`, and `state` (`loading` or `end`).
- `messages.get`: params `conversation_id` (64 lowercase hex), `limit`
  (default 50, maximum 100), optional `max_bytes` (capped at 262,144), and
  optional opaque `after` cursor. Returns message views, `snapshot`, `next`,
  `end`, and `state` (`loading`, `end`, or `unavailable`).

The server applies the registration's scope. An inaccessible conversation is
returned as an empty, ended page rather than an existence-revealing error.

### Responses

```json
{"type":"hello_accepted","version":1,"capabilities":["pairing","history-v1","snapshot-v1"]}
{"type":"approval_required"}
{"type":"pairing_pending"}
{"type":"pair_started","invitation_id":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15],"code":"042731"}
{"type":"pair_status","status":"pending"}
{"type":"host_status","request_id":"req-1","status":"ok","grant_revision":1}
{"type":"snapshot","request_id":"req-2","token":"<opaque>","watermark":42}
{"type":"changes","request_id":"req-3","changes":[],"next_sequence":0,"end":true}
{"type":"error","code":"stale_grant"}
{"type":"result","request_id":"req-4","value":{"snapshot":0,"items":[],"next":null,"end":true,"state":"end"}}
```

Current response implementations for `snapshot_begin` and `changes_resume`
use `result` with `value` objects (the dedicated `snapshot`/`changes` variants
are reserved for a compatible future handler). Clients must accept both shapes
when implementing a forward-compatible decoder.

## 4. Stable errors and retry rules

The complete v1 error enum is:

`malformed_frame`, `frame_too_large`, `incompatible_version`, `unknown_method`,
`not_approved`, `disabled`, `invalid_invitation`, `approval_pending`,
`rejected`, `revoked`, `stale_grant`.

- `malformed_frame`, `frame_too_large`, and `incompatible_version`: do not
  retry the same frame; fix the client or renegotiate.
- `approval_required`/`approval_pending`: no data was exposed. Poll pairing
  status or return to the linking screen; use bounded backoff.
- `stale_grant`/`revoked`: discard cached authorization, queued authenticated
  requests, and snapshot tokens; return to removed-device/relink UI.
- `disabled`/transport failure: retry only after lifecycle/network recovery,
  with exponential backoff and jitter. Do not spin while backgrounded.
- `unknown_method`: feature-gate the UI and do not retry.

Every request must have a stable client-generated `request_id` for correlation.
The durable store already reserves `(registration_id, operation_id)` and stores
request digests/results, but v1's public `authenticated_request` does not yet
expose an `operation_id` or mutation method. Therefore v1 is read-only over this
boundary. A future mutation extension MUST carry a stable opaque `operation_id`
and a request digest: retrying the same id with the same digest returns the
original result; reusing it with a different digest is a conflict; a pruned id
remains reserved. Never generate a new operation id merely because a network
request timed out.

## 5. Snapshot, pagination, and resumption

Use `snapshot_begin` after authorization to obtain an opaque token and immutable
watermark. The token is bound to registration id, device id, grant revision,
conversation scope, and the desktop sync epoch. It expires after 15 minutes.

- Conversation and message cursors are opaque strings; persist and return them
  unchanged. They encode a keyset boundary and must not be treated as offsets.
- Fetch pages until `end=true`; a `loading` state means more pages exist.
- A snapshot excludes arrivals after its watermark. Start a new snapshot for a
  fresh view.
- `changes_resume` returns body-free references (`sequence`, `change_id`,
  `entity_id`, `entity_revision`, `kind`, `tombstone`) only through the captured
  watermark. Fetch entity bodies through authorized reads.
- Cursor-pruned, expired, epoch-invalidated, or grant-invalidated snapshots
  fail closed. Restart with a new snapshot; do not advance a local cursor on a
  failed page.
- The host bounds change page limits to 1..500 and history pages to 100 records
  and 256 KiB.

## 6. Secure storage and local cache

The mobile app owns a separate device key. It must not copy or export the
Desktop's identity key.

Android:

- Generate a non-exportable key in Android Keystore where possible; require
  user/device authentication according to product policy.
- Store the pinned host endpoint id, registration id, device id, grant revision,
  and invitation metadata in encrypted app storage. Keep bearer invitation
  secrets only until claim completion/expiry, then erase them.

 iOS:

- Generate a non-exportable Secure Enclave/Keychain key where supported; use
  Keychain accessibility appropriate for background reconnects.
- Store the same registration and pinning records in Keychain-backed encrypted
  storage. Erase expired invitation secrets and revoked registrations.

Both platforms:

- Cache conversations/messages in an encrypted, durable local database; cache
  only data allowed by the grant scope.
- Keep a durable outgoing queue for future mutation-capable protocol versions.
  Each queued item has stable operation id, request digest, grant revision,
  creation time, retry count, and explicit state. Queue entries are suppressed
  after revocation/stale grant and never replayed under a new registration.
- On schema migration, migrate forward transactionally. If migration cannot be
  proven safe, preserve the old database for export/diagnostics and reset the
  cache (not the secure key) with a visible resync state.
- A user-requested reset removes cache, queue, cursors, registrations, and
  pairing tokens; it does not silently delete the desktop's registration. The
  next launch requires relinking.
- The desktop's sync epoch reset invalidates all snapshots and grants. Treat it
  as a full resync, not as a transient page error.

## 7. Revocation and old-pairing suppression

Desktop revocation marks the registration revoked, increments its grant
revision, and closes tracked sessions. Every authenticated request must present
matching registration id, device id, and current grant revision. A revoked or
old revision cannot read cached operation results or commit queued work.

The client must:

1. stop network retries and notification actions for the revoked registration;
2. remove bearer tokens, snapshot tokens, cursors, and queued authenticated
   operations;
3. retain only a non-sensitive local tombstone sufficient to suppress stale
   background callbacks;
4. show “Device removed” with an explicit relink action.

A notification or delayed response from an old registration must be ignored by
checking registration id, grant revision, and request id before touching cache
or UI. Never auto-repair by accepting a new host key or silently pairing again.

## 8. Screen/lifecycle state model

The shared state names below are UI contract states, not Iced types:

- `linking`: no usable registration; show scan/import invitation and host-pin
  verification.
- `approval`: claim sent; show comparison code, pending/rejected/expired
  outcome, and desktop approval instructions.
- `cached_chat`: valid registration and encrypted cache available; render cache
  immediately with stale/offline indicator.
- `waiting_for_desktop`: cache is usable but host is unavailable/disabled;
  queue no authenticated work and show retry timing.
- `sync`: host authorized; show snapshot/page/change progress and keep cached
  rows visible.
- `update_required`: host and client share no supported version or the server
  reports an unsupported feature; preserve cache, disable that action, and
  provide an update path.
- `removed_device`: grant revoked/stale after authorization; suppress old
  responses/notifications and require explicit relinking.

Network changes, process death, and backgrounding must preserve encrypted cache,
registration metadata, and stable request ids. On foreground, revalidate host
pin and grant revision before draining any queue. A process restart must resume
from the last committed page/change cursor, never from an in-memory offset.

Notification permission is separate from companion authorization. Ask for
Android notification permission/iOS authorization only when the user enables
notifications. Store the platform token only in the mobile/provider boundary;
refresh it on token-change callbacks and app startup. The current Boru core
has no APNs/FCM provider, callback URL, or production background-push delivery:
`background_push=unavailable` is the honest state. Connected activity alerts
and future provider delivery must remain visibly distinct.

## 9. Platform separation and required future verification

Keep Android and iOS adapters separate behind the same protocol/client-core
interfaces. Do not share UI lifecycle code or assume identical secure-storage,
notification, camera, or background execution semantics.

Before claiming mobile interoperability, run real tests for:

- Android and iOS clients against the desktop implementation;
- direct and relay paths, including a network switch and sleep/wake;
- process death during pairing, page fetch, and queued work;
- background/foreground transitions and notification-token refresh;
- desktop restart, invitation expiry/regeneration, grant rotation, revocation,
  cache migration/reset, cursor pruning, and sync-epoch reset;
- malformed/oversized frames, invalid signatures/pins, stale responses, and
  duplicate retries.

Those mobile/provider tests are not implemented by this crate. This document
must not be read as evidence that APNs, FCM, Android UI, iOS UI, or background
execution already works.

## 10. Canonical fixtures

`docs/mobile-app-contract-fixtures.json` contains valid and invalid JSON
messages matching the serde tags in `CompanionRequest`, `CompanionResponse`,
and `CompanionErrorCode`. The Rust test `wire_shape_is_stable` additionally
pins the exact serialized hello request. Arrays in the fixtures are literal
JSON byte arrays, not base64 or UUIDs.
