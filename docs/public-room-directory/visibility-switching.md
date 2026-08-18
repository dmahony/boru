# Directory Visibility Switching (BORU-DIR-06)

Owner/admin controls for switching a public room between
**Public-Discoverable** and **Public-Unlisted**, gated by the existing room
permission model. Implements PDF Task 2.3.

## Permission model

The local user may change a room's directory visibility only when they own
the room. In Boru's existing room permission model a room is locally owned
when:

1. The room is in the advertised set (`advertised_rooms`) — the user
   explicitly advertised it, **or**
2. The room's persisted `ConversationEntry.visibility` is not `Private` —
   the user created it as a public room via the create-room dialog.

Rooms merely **joined** from the directory keep the `Private` default and
are **not** locally owned. Non-authorized users cannot change directory
visibility: the UI shows a muted notice instead of controls, the room
settings dialog refuses to open, and the switch handler rejects the request
with a system message and no side effects (PDF Task 2.3 "Do not: let
non-authorized users change directory visibility").

The gate is enforced in three places:

- **UI** (`view_chat_options_popover` in `app/chat.rs`): directory controls
  are only rendered for `is_room_directory_owner(topic)`.
- **Dialog** (`OpenRoomSettings` handler): refuses to open for non-owners.
- **Switch** (`apply_room_directory_visibility`): the permission gate runs
  first; a rejected switch leaves all state untouched.

## Switching to Public-Discoverable

`plan_visibility_switch(current, requested, is_owner)` returns
`VisibilitySwitchOutcome::Published`, which the app handler applies as:

1. Persist `PublicDiscoverable` on the conversation entry (SQLite included).
2. Add the topic to `advertised_rooms` so the periodic tick (~60 s) keeps
   the advertisement fresh.
3. Upsert the room into the **local** directory store so the creator sees
   their own room in the PUBLIC ROOMS sidebar immediately (the gossip mesh
   does not echo our own broadcasts back).
4. **Immediately** broadcast a fresh signed `RoomAdvertisement` (no waiting
   for the periodic tick). If no directory sender is connected yet, the app
   subscribes to the directory topic so the periodic tick can take over.
5. Surface `"You announced public room …"` in Recent Activity.

## Switching to Public-Unlisted

`plan_visibility_switch` returns `VisibilitySwitchOutcome::Unlisted`, applied
as:

1. Persist `PublicUnlisted` on the conversation entry.
2. Remove the topic from `advertised_rooms` — **refreshes stop**.
3. Stop any per-room DHT tracker (`room_trackers`) so nothing re-publishes
   the room.
4. Remove the room from the **local** directory store + SQLite so it
   disappears from the PUBLIC ROOMS sidebar immediately.
5. Surface `"You unlisted a public room …"` in Recent Activity.

### Withdrawal / TTL caveat (BORU-DIR-09)

There is **no withdrawal/tombstone message yet** (BORU-DIR-09, out of
scope). Remote directories that already hold the advertisement will keep it
until the advertisement TTL expires, so an unlisted room can take up to the
TTL to disappear everywhere. This is documented in the dialog helper text and
the Recent Activity message so the owner is not surprised.

## Metadata edits republish

The room-settings dialog also lets the owner edit the advertised metadata
(name / description / tags). On save:

- The metadata is validated + normalized with `normalize_room_metadata`
  (same bounds as the create flow). Oversized/invalid input is rejected
  inline and nothing is persisted.
- The normalized metadata + the chosen visibility are persisted on the
  conversation entry. **Room identity (topic) never changes** — metadata
  edits propagate without changing the room's ID.
- The visibility switch is applied (see above), and when the room is
  discoverable a fresh advertisement is immediately broadcast so the edited
  name/description reach peers without waiting for the periodic tick.

## Programmatic access (GuiTestCommand)

The same controls are exposed to the GUI test / MCP layer:

- `SetRoomDirectoryVisibility { room_id, visibility }` — direct owner-gated
  switch (PublicDiscoverable <-> PublicUnlisted).
- `OpenRoomSettings { room_id }` / `SetRoomSettingsName { name }` /
  `SetRoomSettingsDescription { description }` /
  `SetRoomSettingsTags { tags }` / `SetRoomSettingsVisibility { visibility }`
  / `ConfirmRoomSettings` — drive the room-settings dialog.

## Tests

- Library: `plan_visibility_switch` unit tests in
  `src/control_plane/advertisement.rs` (owner switch both directions,
  no-change, non-owner forbidden, Private transitions forbidden).
- GUI (`src/bin/boru/app.rs`, `vr_*` tests): owner switch to
  discoverable persists + advertises; non-owner switch and dialog are
  rejected with no side effects; switch to unlisted stops advertising and
  removes the local directory entry; room-settings edits persist metadata
  without changing identity and keep the room advertised; oversized edits
  are rejected inline.
