# Boru event and state model

This document is the integration contract for the current chat, thread, reaction,
pin, search, notification, and media projections. SQLite is authoritative for
durable state; UI structs and in-memory protocol state are projections.

## Event identity and compatibility

Signed envelopes carry the sender, payload, timestamp, compression marker, and a
stable 32-byte message id. Receivers verify the signature before decoding and
use the embedded id when present, falling back to the legacy payload-derived id
for older envelopes. Unknown message variants are ignored by the shared event
handler rather than terminating the receive loop. New postcard fields must be
trailing and use the manual legacy-default deserializer pattern in
`chat_core/protocol.rs`.

The sender timestamp is an event timestamp, not a trusted wall clock. Durable
projections compare the timestamp together with the stable id (or actor/event
key) when reconciling equal or reordered operations. Remote timestamps must be
bounded before admission; they are never used as a local scheduling deadline.

## Durable projections

| Projection | Source of truth | Reconciliation key | Compatibility behavior |
| --- | --- | --- | --- |
| Chat message | `chat_messages` / message store | stable message id | ordinary `Message` remains valid |
| Thread | `chat_messages` plus `thread_state` | `(topic, thread_root_id)` | replies remain ordinary rows for delivery/backfill |
| Reaction | `reaction_events` | `(message_id, actor, emoji)` | legacy hash-only reactions still render |
| Pin | `pinned_messages` | `(topic, message_hash)` | unsupported peers ignore pin events |
| Local search | rebuildable FTS projection | local message row id | search never changes wire delivery |
| Outbox | SQLite outbox tables | local event id / payload hash | crash recovery resets in-flight work |

Schema migrations are forward-only. The current schema is v25; migrations v21–25
add reactions, threads, reply references, local search, and pins respectively.
Do not reuse a migration number or edit old migration semantics.

## Thread projection

Replies are retained in the main message table so gossip delivery and backfill do
not need a second transport. `thread_root_id` and `reply_to_message_id` let the
main timeline filter replies while a thread view projects the full reply set.
Missing roots are represented as unresolved summaries, and deleted roots use a
tombstone rather than deleting replies. Follow state, unread reply count, and
read time live in `thread_state`.

Notification decisions are made from visibility, follow state, mute state, and
message kind in one policy boundary. A visible, followed thread may mark replies
read; background replies must not silently mark themselves seen.

## Media lifecycle

Voice and screen sharing have independent track state (`Stopped`, `Starting`,
`Active`, `Reconnecting`) inside the shared realtime media presence projection.
Stopping or reconnecting screen sharing must not tear down voice. Screen-share
viewer admission is bounded, late/reconnecting viewers update the registry, and
removing the final viewer applies the resource-release policy. Capability hooks
are evaluated at the session boundary, not only by hiding UI controls.

The screen-share transport is a separate authenticated Iroh QUIC protocol from
ordinary chat and file transfer. A media failure should transition only the
media track and leave chat state intact.

## UI projection rules

Iced components read shared theme/layout tokens and should render from domain
state rather than mutate SQLite directly. A storage mutation emits or schedules
a UI refresh; a view rebuild must not duplicate a durable row. Search results
navigate to the owning topic and do not copy message bodies into the search
index beyond the local projection.

## Platform limitations

- Wayland capture depends on the desktop portal and PipeWire availability and
  user consent; automated Linux coverage cannot prove every compositor.
- X11 capture is a fallback and has different capture/input semantics.
- Windows capture uses the Windows Graphics Capture backend; cross-compilation
  does not replace runtime verification on a Windows desktop.
- Independent system/application audio and inline video playback depend on the
  selected platform backend and installed runtime packages.
- Real relay/DHT and multi-peer tests can be environment-sensitive. Prefer the
  deterministic/prewarm harness for state-machine coverage and record real
  transport coverage separately.
- Legacy peers may not understand threads, reactions, pins, typing, or media
  metadata. They must continue to receive ordinary chat and ignore extensions
  they cannot decode.
