# BORU-DISC-14: Public chats stay explicit user features — separate from discovery

Audit scope: committed worktree (`wt/t_23c6d799`) after `git fetch origin && git merge origin/main`
(origin/main @ 86113c5e, BORU-DISC-13). Phase 3 of the hidden-discovery refactor
(PDF T12): **removing the old auto-joined lobby must NOT remove public chats as a
feature — public rooms remain explicit user-created / user-joined conversations
with their own topic and metadata, never confused with the internal discovery
topic.**

This document records the verification and the small code changes shipped in
this task.

---

## 1. Objective (from PDF T12)

> If Boru has a public-chat feature, retain it as an ordinary user-created /
> user-joined conversation with its own topic and metadata. Removing the old
> lobby must NOT remove public chats as a feature.

Boru does have a public-chat feature (user-created rooms with DHT member
discovery + directory-topic advertisement). BORU-DISC-12 removed the
**auto-joined lobby conversation** at startup. This task confirms every
**explicit** public-chat entry point still works, that public rooms are ordinary
conversations (own topic, metadata, history, list entry), and that nothing in
the public-room path is confused with the internal discovery topic
(`BORU_DISCOVERY_TOPIC_V1`, `src/discovery_topic.rs`).

## 2. What "explicit public chat" means today (verified live in this tree)

| Entry point | Path | Explicit? |
|---|---|---|
| UI create room | `CreateNewRoom` dialog → `ConfirmCreateNewRoom` with "advertise" on (`app.rs:11255-11415`) | ✅ user clicks + names the room |
| UI join from directory | Discover screen "Public Rooms" → `DirectoryRoomJoin(ad)` parses ticket → `OpenRoom(topic)` (`app/discover.rs:1652-1666`) | ✅ user clicks Join on an advertised room |
| CLI open | `boru open <topic>` → `initial_room` → `OpenRoom` (`main.rs:544-568`) | ✅ explicit subcommand |
| CLI join | `boru join <ticket>` → parse ticket → `OpenRoom(ticket.topic)` (`main.rs:569-579`) | ✅ explicit subcommand |
| Member discovery | `PublicRoomTracker::new_with_metadata` + `PublicContinuousTracker::start` keyed by the created room's topic (`app.rs:11333-11364`) | ✅ per-room, explicit identity |

The create path (`app.rs:11269-11364`) is the canonical public-room flow:
- random `TopicId` (own topic),
- `RoomStore` entry (own persisted peers),
- archived `ConversationEntry` (own name/metadata, unarchived into CHATS on open),
- `DirectoryStore` upsert + directory-topic `RoomAdvertisement` broadcast (own
  listing),
- `PublicRoomTracker` with `PublicRoomIdentity::new(topic, discovery_key(name))`
  — **explicit identity for that room**, not the canonical lobby identity.

## 3. Domain separation: public rooms vs the discovery topic (BORU-DISC-05 check)

The internal discovery topic is a **different derivation** (`src/discovery_topic.rs`:
`DISCOVERY_TOPIC_DOMAIN_SEPARATOR = b"boru-chat internal-discovery v1"`, no room
name component). The classifier (`topic_kind`, BORU-DISC-10) treats it as
Discovery and **everything else — including every public-room topic and the
canonical lobby — as Conversation**. Verified by unit tests:

- `discovery_topic_differs_from_public_room_topic` — discovery ≠ lobby topic per network.
- `discovery_topic_differs_from_discovery_key` — discovery ≠ public-room DHT key.
- `topic_kind_lobby_is_conversation` — the canonical lobby is **not** Discovery.
- `topic_kind_classifies_conversation_topics` — arbitrary room topics are Conversation.
- `topic_kind_direct_topic_is_conversation` — deterministic direct topics are Conversation.
- New GUI-level test (this task) — see §5.

Guards added in BORU-DISC-13 keep the separation airtight at the UI/persistence
layer: `OpenRoom`/`RoomOpened` refuse Discovery topics, `persist_remote_message` /
`persist_remote_file_share` skip the message DB for Discovery topics,
`SubscribeStoredConversations`/`BackgroundSubscribe` filter them, and
`BackfillAuthorizer::authorize` denies them first (documented in
`13-persistence-isolation.md`). Public rooms are unaffected by these guards —
they are Conversation topics by construction.

## 4. The public-room tracker is scoped to explicit rooms only

Before this refactor, `main.rs` started a **startup public-lobby tracker**
(`PublicRoomTracker::start(network)` deriving the canonical lobby identity) and
held it in `IcedChat.continuous_tracker`. BORU-DISC-12 removed the startup
lobby conversation; this task removes the last vestige of that implicit
lobby tracker:

- **Removed** `IcedChat.continuous_tracker: Option<PublicContinuousTracker>`
  (field, constructor parameter, and all 3 call sites: `main.rs` +
  `app.rs` test fixtures). It was dead code marked `#[expect(dead_code)]` and
  always `None` after BORU-DISC-12. Keeping it implied a lobby tracker could
  still exist; removing it makes the scoping explicit: **the only public-room
  trackers in the app are created per user-created room** in
  `ConfirmCreateNewRoom` (`app.rs:11333-11364`).
- The generic constructor `PublicRoomTracker::start` (canonical lobby identity)
  remains in `src/public_room_tracker.rs` and is still used by the integration
  tests (`tests/test_public_lobby_integration.rs` uses `PublicNetwork::Test`) —
  that is the DHT member-discovery layer, which is legitimate networking
  infrastructure and is exercised for user rooms through `new_with_metadata`.
- Renamed `main.rs` test `no_dht_flag_disables_member_discovery_and_tracker` →
  `no_dht_flag_disables_member_discovery_client`: the old test asserted a local
  `let continuous_tracker: Option<()> = None` placeholder was `None` (a
  tautology once the field was gone). The remaining assertion — `--no-dht`
  suppresses the shared member-discovery DHT client used by room trackers —
  still holds.

## 5. New regression test: public room ≠ discovery topic (GUI level)

`app.rs` test `vr_created_public_room_is_conversation_never_discovery_topic`
(bin tests) drives the real create flow (`CreateNewRoom` → name → advertise →
`ConfirmCreateNewRoom`) and asserts:

1. The created room has a conversation-store entry with its own name metadata
   ("Beach House") — an ordinary conversation.
2. `!is_discovery_topic(entry.topic)` — its topic is not the discovery topic.
3. `topic_kind(entry.topic) == TopicKind::Conversation` — it classifies as a
   conversation, never discovery.
4. The discovery topic never appears in the conversation store.
5. `OpenRoom(discovery_topic)` is refused (BORU-DISC-13 guard) — the discovery
   mesh cannot become a public chat.

Together with the existing lib tests (§3) this pins the T12 acceptance
criterion: **removing the old lobby did not remove public chats; public rooms
are explicit conversations with their own topic and metadata, and the
discovery topic stays out of the conversation model.**

## 6. Test evidence

| Suite | Command | Result |
|---|---|---|
| Public lobby integration | `rb test --features net --test test_public_lobby_integration` | 8/8 PASS |
| Private room DHT discovery | `rb test --features net --test test_private_room_dht_discovery` | 10/10 PASS |
| Private room invitation discovery | `rb test --features net --test test_private_room_invitation_discovery` | 15/15 PASS |
| Lib discovery_topic + backfill | `rb test --lib --features net -- discovery_topic backfill` | 39/39 PASS |
| Bin GUI create-room + no-dht | `rb test --bin boru --features gui -- vr_created_public_room vr_create_public_room no_dht_flag` | 4/4 PASS (incl. new test) |
| Bin compile gate | `rb check --bin boru --features gui` | PASS exit 0 (259 pre-existing warnings) |

No regressions caused by the refactor. Deliberate changes:
- `continuous_tracker` field removal (dead code; see §4).
- `no_dht_flag_disables_member_discovery_and_tracker` test renamed + trimmed to
  remove the tautological assertion (see §4).
- No changes to deterministic topic derivation, public-room identity, or the
  DHT member-discovery layer.

## 7. Remaining known limitations / out of scope

- Terminology cleanup (lobby → discovery wording in UI/docs) → BORU-DISC-15.
- Migration / stale lobby rows in storage → BORU-DISC-16+.
- The stale MCP lobby literal (`mcp_server.rs:2451`) stays untouched; it is a
  Conversation-kind diagnostic id, not the discovery topic (documented in
  `13-persistence-isolation.md` §3.12).
