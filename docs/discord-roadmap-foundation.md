# Discord-style feature roadmap: Boru foundation audit

Status: implementation note for the ordered feature slices. This document records the
current tree's boundaries; it does not add product behavior.

## Executive decisions

1. **Separate wire message identity from local row identity.** `MessageHash` is currently
   `type [u8; 32]` and `message_hash()` hashes postcard(`Message`) in
   `src/chat_core/protocol.rs:481-490`. It is useful for deduplication and existing edit,
   delete, and receipt targets, but it is not an immutable message identity: changing a
   body changes the hash, and two equal payloads have the same hash. The SQLite
   `messages.id` at `src/store/mod.rs:254-269` is an AUTOINCREMENT local row id and must
   not be put on the wire. Future addressable features should introduce an explicit
   immutable `MessageId` in the signed envelope (generated once by the sender), retain
   `MessageHash` only as content/dedup evidence, and map `MessageId -> messages.id`
   locally. References (reply, reaction, edit, delete, notification actions) target
   `MessageId`; they never copy the parent body as authority.
2. **Keep protocol evolution additive and authenticated.** New optional metadata must
   use `#[serde(default)]` with postcard, not `skip_serializing_if` (postcard needs the
   option tag). A new trailing field requires a manual deserializer like
   `SignedMessage` (`src/chat_core/protocol.rs:597-660`), because postcard reports EOF
   before serde defaults can apply. New authenticated event families use
   `protocol_signing::canonical_signed_bytes(protocol, version, fields)` and a distinct
   domain tag; signers always emit the current version, while verifiers may retain an
   explicit, bounded legacy fallback. Unknown versions fail closed rather than being
   guessed. See `src/protocol_signing.rs:1-74` and `docs/protocol-signing.md`.
3. **SQLite is authoritative for durable state.** The versioned store is at schema 20
   (`src/storage/mod.rs:55-58`), with migrations dispatched in
   `src/storage/schema.rs:194-260`. Add one forward migration per durable feature and
   leave old rows readable. The `messages` table is the richer UI history projection;
   the legacy/backfill `chat_messages` table is created by v19
   (`src/storage/schema.rs:783-809`) and must not become a second authority. JSON
   conversation data is read-only upgrade fallback; `ConversationStore` writes SQLite
   (`src/conversations.rs:1-23`).
4. **Ephemeral state stays out of durable history.** Selection, composer text, hover and
   context menus, unresolved-reference queues, live notification toasts, online/session
   state, typing/presence, media frames, and in-memory navigation/reveal targets are
   process/UI state. Persist only restart-meaningful facts: messages and immutable IDs,
   conversation metadata, unread/mute/archive flags, reactions/edits/deletion tombstones,
   delivery/outbox state, and resumable transfer/session metadata where an existing
   subsystem already defines it.

## Current canonical paths and extension anchors

| Concern | Current authority | Roadmap extension point |
| --- | --- | --- |
| Wire messages and signed envelope | `src/chat_core/protocol.rs` (`Message`, `NetEvent`, `SignedMessage`, `message_hash`) | Add versioned, signed immutable message identity and optional metadata here; keep old variants decodable. |
| Authenticated routing and safety | `src/chat_core/net_event.rs:127-180` and `src/chat_callbacks.rs` | Validate signature/version before callback dispatch; route by `ConversationNetEvent` topic and preserve dedup/order semantics. |
| Conversation routing/navigation | `src/conversations.rs:96-115`; `ConversationEntry` includes topic, group epoch mappings, unread/archive metadata | Use `TopicId`/`GroupId` as navigation keys, not display names; resolve message references after the target conversation is loaded. |
| Durable chat rows | `src/store/mod.rs:250-269` (`messages`) and `src/storage/*` | Add migrations/indexes for message ID, references, reactions, and tombstones; make upserts idempotent for direct, room, offline, and backfill paths. |
| Backfill and replay | `src/backfill/{client,server,wire}.rs`; v19 `chat_messages` migration | Carry immutable IDs and event metadata through backfill; accept reordering and deduplicate without rewriting canonical message content. |
| Delivery/outbox | `src/store/mod.rs:187-196`; `src/outbox.rs`; storage outbox migrations | Keep delivery state separate from message content; retries update durable state and never create a second message identity. |
| Notification policy | `src/bin/boru/notification/event.rs:1-24,131-181`; `src/bin/boru/app/notifications.rs:1-24` | Emit structured events with stable IDs/action targets. Suppress/group using focus, mute, conversation visibility, and user policy; render privacy-sensitive text only at the edge. |
| Iced shell and chat UI | `src/bin/boru/app.rs:2563` (`IcedChat`), `AppMessage` at `:3856`, routing at `:4296`; `src/bin/boru/app/chat.rs` (`view_chat_log`, `view_composer`) | Add context actions and reveal/navigation messages to the existing shell/domain pattern; keep UI state a projection of storage/network events. |
| Media and sessions | `src/call/media.rs` (`MEDIA_VERSION`, `CallId`, sequence/fragments); `src/whisper/session_manager.rs` (`SessionState`, `SessionEvent`); `src/screen_share/session.rs` (`ScreenShareSessionId`, versioned lifecycle) | Reuse call/session IDs and explicit lifecycle state. Never persist live media payloads; persist only resumable metadata if a feature requires restart recovery. |
| Tests | `src/chat_core/tests.rs`, `src/storage/tests.rs`; integration tests under `tests/` including `test_message_lifecycle.rs`, `test_offline_delivery_integration.rs`, `test_iced_chat_flow.rs`, image/transfer and serde tests | Every slice needs protocol roundtrip/legacy tests, migration tests, duplicate/reorder/backfill tests, and direct + room + offline coverage; UI tests should assert actions route to stable IDs. |

## Ordered implementation guidance

The dependency order from the roadmap should remain: establish immutable message IDs and
compatibility helpers first; then replies/reference resolution; then reactions and other
message mutations; then threads/search/navigation projections; then notification policy;
and finally media/session-facing UI integrations. Each slice should extend the existing
protocol, storage, router, and Iced domain rather than create parallel stores or event
buses. The following invariants apply to every slice:

- old peers ignore optional metadata and still display the base message;
- signed fields include identity, routing target, timestamp/nonce, version, and every
  field that changes interpretation;
- duplicate, reordered, replayed, offline, and backfilled events converge to one state;
- a missing parent/reference is retained as unresolved and resolved when the message
  arrives, never treated as permission to duplicate stale body text;
- deletion is represented by durable tombstones so backfill/restart cannot resurrect it;
- navigation and notification actions carry structured `TopicId`/`MessageId` targets;
- all UI mutations follow the existing storage/event projection boundary.

## Verification checklist for later slices

- Protocol: current roundtrip, legacy decode, unknown-version rejection, signature mutation,
  malformed input, and size-limit tests.
- Storage: fresh migration, upgrade from the previous schema, idempotent replay, indexes,
  tombstones, and restart/reopen tests.
- Routing: direct, room, offline mailbox/outbox, backfill, duplicate, reorder, and
  multi-peer convergence tests through `handle_net_event*`.
- UI: composer/context action, quoted preview, reveal selected message, unread/mute/focus
  notification policy, and no duplicate local-vs-remote rendering.
- Media/session: explicit consent and lifecycle transitions; no media payload in chat
  history or notifications.

This audit was checked against the current worktree at commit `fd2e3e1b`; line numbers are
anchors for the current tree and should be refreshed when a slice changes those files.
