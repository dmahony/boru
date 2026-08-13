# Local Relationship to Discovered Rooms (BORU-DIR-12, PDF Task 4.3)

Status: implemented
Chain: BORU-DIR-10 (bounded cache) → BORU-DIR-11 (dedupe/conflict) → **BORU-DIR-12 (this doc)** → BORU-DIR-13 (Discover Rooms UI)

## Goal

Let the directory distinguish discoverable rooms from rooms the user already
joined, is joining, hid, or cannot join. For every cached `room_id` the
directory derives a local relationship state — `NotJoined`, `Joined`,
`JoinPending`, `Blocked/Hidden`, or `Incompatible` — using the **real local
room database** as the source of truth for `Joined` (never the
advertisement), and never re-shows locally hidden rooms unless the user
explicitly resets that preference.

## Core rule

The directory **stores** local relationship state; it never decides it. The
app layer owns the source of truth (the persisted conversation store + the
persisted hide preference) and pushes the facts in via
[`RoomDirectory::sync_local_states`]. The directory can therefore never
create, duplicate, or mutate local membership records (PDF Core rule).

## Derivation

`RoomDirectory` keeps a [`LocalRoomFacts`](crate::room_directory::LocalRoomFacts)
struct — three sets of `TopicId`s:

- `joined` — room ids the user has joined, from the local room database
  (in Boru: every topic in the persisted `ConversationStore`).
- `pending` — room ids with a join attempt in flight (Phase 6 join flow;
  empty until then).
- `hidden` — room ids the user has hidden/blocked locally (persisted).

`sync_local_states(facts)` replaces the facts and re-derives every cached
entry's `local_join_state`. New entries added later (including a hidden room
re-advertised after eviction) are derived immediately from the stored facts.

Precedence ([`derive_local_state`](crate::room_directory::derive_local_state)):

1. hidden → `Blocked` (never re-shown)
2. joined → `Joined` (Open rather than Join)
3. pending → `JoinPending`
4. incompatible (advertised protocol newer/unsupported) → `Incompatible`
5. otherwise → `NotJoined`

## Browse surface

[`RoomDirectory::snapshot`] is the browse surface the future Discover Rooms
UI renders: it **excludes** `Blocked` entries, so hidden rooms never reappear
across advertisement refreshes. [`RoomDirectory::snapshot_all`] includes them
for diagnostics (Phase 8).

[`DirectoryEntry::offered_action`] exposes what the UI should offer:
`RoomAction::Open` for joined rooms, `Join` for genuinely joinable rooms,
`Hidden` for blocked rooms, `Incompatible` for unjoinable rooms.

## Persistence hook

[`Storage::room_hidden_ids`] / [`Storage::set_room_hidden`]
(`src/storage.rs`, kv_store key `room_hidden_ids`, JSON array of hex room
ids) persist the hide preference so it survives advertisement refreshes and
application restarts. BORU-DIR-20's user-facing Hide/Block controls write
through these methods; this task only provides the cache-side derivation +
the persistence hook.

## App wiring

`IcedChat` holds a read handle to the directory (`app.room_directory`, set
from `main.rs` after construction). On every `ConnMonitorTick` (1 Hz)
`sync_directory_local_states()` rebuilds `LocalRoomFacts` from
`conversation_store` (joined) + `Storage::room_hidden_ids` (hidden) and calls
`sync_local_states`, so joins/hides are reflected promptly.

## Acceptance criteria

- The directory does not offer Join for an already joined room
  (`offered_action() == Open`).
- Directory state cannot duplicate local membership records (facts are fed
  in, never materialised; `sync_local_states` never inserts entries).
- Hidden rooms stay hidden across advertisement refreshes when the
  preference is persisted.
