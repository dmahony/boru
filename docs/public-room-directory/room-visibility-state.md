# Room visibility state (BORU-DIR-04)

PDF Phase 2 Task 2.1: make discoverability a deliberate room property rather
than an automatic side effect of being public.

## Visibility model

Every room has an explicit visibility, stored on the durable room metadata
(`ConversationEntry` in `src/conversations.rs`):

| Visibility              | Advertised?                  | Join method                     |
|-------------------------|------------------------------|---------------------------------|
| `Private`               | No                           | Invite / authorization only     |
| `PublicUnlisted`        | No                           | Room ID / invite / link         |
| `PublicDiscoverable`    | Yes                          | Directory + explicit Join       |

The enum lives in `src/control_plane/advertisement.rs` (`RoomVisibility`,
wire-stable postcard tags `Private = 0`, `PublicUnlisted = 1`,
`PublicDiscoverable = 2`). The same type is reused for persisted room
metadata so there is exactly one visibility model.

## Persistence

- `ConversationEntry.visibility: RoomVisibility` — `#[serde(default)]`
  (legacy entries without the field load as `Private`, so nothing is
  accidentally advertised after an upgrade).
- Persisted through `ConversationStore` (conversations.json + SQLite
  `kv_store`), exactly like every other room-metadata field.
- Direct chats and new groups default to `Private`.
- New rooms created with the "Advertise in Directory" checkbox are
  `PublicDiscoverable` — the checkbox is the explicit discoverability
  choice.
- `ToggleAdvertiseRoom` keeps the field in sync: enabling advertising →
  `PublicDiscoverable`, disabling → `PublicUnlisted`.

## Conservative migration

At startup (`IcedChat::new`) rooms that the local node advertised into the
directory under the legacy model (local-authored rows in the persisted
`directory_store`) are migrated to `PublicUnlisted`:

- `ConversationStore::migrate_legacy_public_rooms(&HashSet<TopicId>)`
  only touches entries that still carry the legacy `Private` default, so a
  room the user has already made discoverable is never downgraded.
- The migration is idempotent and runs once per load.

This satisfies "existing public rooms are not unexpectedly exposed": a
legacy public room becomes shareable-but-unlisted rather than discoverable.

## Emit-site guard

Only `PublicDiscoverable` rooms may emit directory advertisements.

1. **Control-plane path** — `DiscoveryService::announce_room_advertisement`
   (and its internal `ControlAnnounceHandle` counterpart) refuse
   non-discoverable advertisements with `AnnounceOutcome::NotDiscoverable`;
   nothing is broadcast. This is the guard at the advertisement emit site.
2. **Legacy directory path** — the app's periodic `RoomAdvertisement`
   broadcast skips topics whose persisted visibility is not
   `PublicDiscoverable`.

The existing receive-path validation
(`PublicRoomAdvertisement::validate` →
`AdvertisementViolation::NotDiscoverable`) remains as the second layer:
even if a non-discoverable advertisement were constructed, the privacy
layer drops it before it enters the directory view.

## Room identity / history

Visibility is metadata only. Changing discoverability never changes the
room's topic identity (`ConversationEntry.topic` / group id / epoch) and
never destroys message history — the entry is updated in place, not
recreated. `visibility_change_keeps_topic_identity_and_history` verifies
this.

## Tests

- `conversations::tests::visibility_defaults_to_private_for_new_entries`
- `conversations::tests::visibility_round_trips_json_persistence`
- `conversations::tests::legacy_json_without_visibility_defaults_to_private`
- `conversations::tests::migrate_legacy_public_rooms_sets_unlisted`
- `conversations::tests::visibility_change_keeps_topic_identity_and_history`
- `discovery_service::tests::announce_room_advertisement_refuses_non_discoverable`
- `control_plane::advertisement::tests::rejects_non_discoverable_visibility`
