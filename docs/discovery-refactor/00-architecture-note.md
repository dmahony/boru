# BORU-DISC-04: Phase-1 Architecture Note — Dependency Inventory for the Hidden Discovery Topic

Audit scope: committed worktree (`wt/t_31125ed0`) after `git fetch origin && git merge origin/main`
(origin/main @ b2f736d3, BORU-DISC-03). This is Task 4 of the *Boru: Replace the Auto-Joined
Lobby with a Hidden Discovery Topic* plan (PDF T4) and the Phase-1 synthesis note that the
implementation chain (BORU-DISC-05+) builds on. **Read-only audit — no code was changed.**

It consolidates the three predecessor audits and adds the dependency inventory (who assumes the
lobby is a user-visible conversation):

| Doc | Task | Result |
|---|---|---|
| `docs/discovery-refactor/01-lobby-constants-map.md` | BORU-DISC-01 | Canonical identity + every read/write path touching the lobby, classified DISCOVERY / UI/CHAT / MIXED |
| `docs/discovery-refactor/02-startup-flow.md` | BORU-DISC-02 | Startup joins the lobby twice; `OpenRoom` is the user-facing coupling; 6 insertion points |
| `docs/discovery-refactor/03-message-handling.md` | BORU-DISC-03 | Inbound pipeline is topic-tagged but not topic-KIND aware; separation point = forwarder-spawn boundary + NetEvent guard |
| **this doc** | **BORU-DISC-04** | **Dependency inventory: every test, fixture, example, CLI flag, doc, script, and CI file that assumes the lobby is user-visible, plus the recommended target architecture** |

---

## 1. Current lobby behavior summary

The canonical Mainnet lobby topic (`ebab66f6…4016`, derived by `public_lobby_topic(Mainnet)` /
`IcedChat::default_lobby_topic()`, `app.rs:7893`) is currently **dual-purpose**:

1. **Discovery infrastructure** — mDNS/DHT peer bootstrap and gossip neighbor lifecycle
   (startup raw subscription, DHT tracker, continuous tracker, dynamic joiner).
2. **A normal user-facing room** — auto-selected at startup, materialized as `ConversationLive`,
   inserted into the conversation map, replayed with chat history, rendered in the chat UI,
   counted for sidebar/unread state, and reachable by send/persistence/read-receipt paths.

User-visible lobby behaviors today (verified this audit):

- **Auto-selected at startup**: no CLI subcommand → `main.rs:582-588` selects the lobby as
  `initial_room`; `main.rs:1612` sets `return_to_chat_list_after_open = true`; the boot task
  `OpenRoom(lobby)` (`main.rs:1671-1689`) drives the UI to `Screen::Chat` and back to ChatList.
- **Live conversation**: `app.rs:12524-12541` keeps a `ConversationLive` for the lobby in
  `self.conversations` so its `GossipSender` survives room switches (the central coupling).
- **Chat UI**: system messages `Chat joined.` + `/help` hint (`app.rs:12292-12305`), history
  replay (`app.rs:12308-12465`), room-history upsert (`app.rs:12485-12488`), mesh status text
  `Connecting to lobby...` / `Connected to lobby — N peer(s) online` (`app.rs:11846`,
  `app.rs:12103-12107`).
- **Home screen**: Mesh Health card shows lobby state + connection duration
  (`app/home.rs:1178-1183`), fed from `sender_ready`/`mesh_connected_at`.
- **Sidebar/unread**: lobby events bump `chats_sidebar_revision` and `ConversationStore`
  ordering/preview (`app/chat.rs:4792-4806`); inactive lobby traffic increments
  `conversation.unread` (`app/chat.rs:4894-4912`); sidebar badge renders it
  (`app/sidebar.rs:547-551, 1073-1081`).
- **Discovery UI**: Discover-sidebar "discovered peers" list is fed by the raw lobby receiver
  (`main.rs:1179-1218`) and mDNS (`main.rs:1220-1277`). This part is legitimately
  discovery-oriented and should survive (repurposed for the internal topic).
- **MCP diagnostics**: `boru_join_lobby_room` opens the lobby through the GUI action path
  (`mcp_server.rs:2439-2498`), using a **stale literal hash**
  (`b"iroh-gossip-chat/default-lobby/v1"`, `mcp_server.rs:2451`) that is NOT the canonical
  `public_lobby_topic` derivation (BORU-DISC-01 §6).

## 2. Discovery-only vs chat paths (condensed from 01–03)

| Path | Location | Kind |
|---|---|---|
| Canonical identity derivation | `src/public_room.rs:34-176`, `src/topic_derivation.rs:10-72`, `src/discovery_backend.rs:15-24` | DISCOVERY identity |
| Public-room DHT tracker (publish/discover) | `src/public_room_tracker.rs:135-330`, `src/public_room_continuous.rs` | DISCOVERY-only |
| Startup raw gossip subscription | `main.rs:1110-1113` | DISCOVERY bootstrap |
| Raw receiver drain (NeighborUp/Down only, chat dropped) | `main.rs:1179-1218` | DISCOVERY-only — **the model for DiscoveryService** |
| mDNS → join peers + Discover sidebar | `main.rs:1220-1277` | DISCOVERY + diagnostics |
| Dynamic joiner / continuous tracker fanout | `main.rs:1126-1166`, `src/dynamic_joiner.rs` | DISCOVERY |
| **GUI second subscription (`OpenRoom` slow path)** | `app.rs:11848-11903`, forwarder `app.rs:11983-11989` | **UI/CHAT — duplicate join** |
| RoomOpened → Screen::Chat, system entries, history replay, room-history upsert | `app.rs:12208-12488` | UI/CHAT |
| Lobby `ConversationLive` retention | `app.rs:12524-12541` | UI/CHAT (lifetime workaround) |
| Inbound NetEvent → touch/bump/preview/unread/render | `app/chat.rs:4788-4950`, `net_event.rs:167-567` | UI/CHAT (no kind guard) |
| Outbound send + persistence for lobby | `app/chat.rs:7226-7293`, `app.rs:9326-9370` | UI/CHAT (reachable) |
| Backfill policy: lobby readable by any authenticated peer | `src/backfill.rs:208-258`, `:1573-1590` | DISCOVERY/transport policy (needs decision) |
| Directory topic (RoomAdvertisement mesh) | `main.rs:1285-1362`, `src/directory.rs` | Separate — must NOT merge into discovery |
| Private-room tracker / direct-topic mesh | `src/private_room_tracker.rs`, `app.rs:11910-11929` | Separate — must stay conversation-owned |

**Net finding (BORU-DISC-03):** the pipeline is topic-tagged after
`spawn_conversation_forwarder` (`src/conversations.rs:674-726`) but no stage distinguishes
Discovery from Conversation. Lobby traffic reaches every chat stage today.

## 3. Dependency inventory — what assumes the lobby is user-visible

### 3.1 Tests (`tests/`)

| File | Lobby assumption | Impact when lobby becomes hidden |
|---|---|---|
| `tests/test_public_lobby_integration.rs` | **None user-visible.** 8 tests (not 19 as stated in the task brief — verified), all exercise `PublicRoomTracker` publish/discover/validation over `InMemoryDiscoveryBackend` (`canonical_lobby_key`, DHT records, leave/shutdown). Uses `PublicNetwork::Test`. | **Safe.** These test the discovery/DHT layer that survives. Test names/comments say "opens the public lobby" (`:79,85,118`) but only mean tracker discovery, not a chat. |
| `tests/test_branding_rename.rs` | Protects identity: `PUBLIC_ROOM_NAME = "public-lobby"` (`:211-215`), domain separators (`:175-205`), deterministic `public_room_topic(0,"public-lobby",1)` (`:268-310`). | **Safe but constraining.** Keeps the canonical identity; the hidden discovery topic must either reuse this derivation or add `BORU_DISCOVERY_TOPIC_V1` alongside without touching it. |
| `tests/test_deterministic_discovery_integration.rs` | `PublicationPolicy`/`ContinuousTrackerConfig` behavior; no GUI. | **Safe.** Discovery-policy layer only. |
| `tests/test_online_user_list.rs` | NeighborUp/Down tracking; no lobby topic constant. | **Safe.** |
| `tests/test_full_chat_list_flow.rs`, `tests/test_iced_chat_flow.rs` | Random `TopicId`; simulate create/join/send on **arbitrary** topics. | **Safe.** No dependency on the lobby; they prove the generic conversation flow that must remain unchanged. |
| `tests/test_private_room_dht_discovery.rs`, `tests/test_private_room_invitation_discovery.rs` | Private-room DHT; no lobby reference. | **Safe.** |
| `tests/security/authorization.rs:475`, `tests/security/mutation.rs:184` | `RoomAdvertisement { room_name: "Lobby" }` — a **fixture string**, not the canonical lobby. | **Safe** (cosmetic name only). |
| `tests/test_mcp_diagnostics_integration.rs`, `tests/verify_gui_bootstrap.rs` | No lobby-specific reference. | **Safe.** |
| `tests/generate_test_images.py`, `tests/download-fixtures/`, `tests/gen_stress_data.rs` | No lobby reference. | **Safe.** |

**Test conclusion:** no committed test pins the lobby as a visible conversation (sidebar entry,
default selection, unread, history, notifications). The UI-visible lobby behavior is instead
pinned by **evidence scripts** (§3.5) and **runtime smoke logs** (§3.4), which are not
CI-gated. This is good news: removing the visible lobby will not fail the suite, but the suite
also does not yet cover the target state (Phase-7 tests in the PDF will add that).

### 3.2 Examples / CLI flags

| File | Lobby assumption | Impact |
|---|---|---|
| `examples/dht_harness.rs` | Isolated discovery tool on Development/Test networks; explicitly avoids the production Mainnet public-lobby (`:50-61,105`). | **Safe**; keep as a discovery harness, retarget docs to the internal topic later. |
| `examples/setup.rs`, `examples/doctor.rs`, `examples/catalogue_browser.rs`, `examples/test_addr.rs`, `examples/video_backend_probe.rs` | No lobby reference. | **Safe.** |
| `examples/iced_chat/main.rs:582-588` | No subcommand ⇒ auto-select lobby as initial room. | **BREAKS**: must change to not open a visible lobby (home/ChatList state). |
| `examples/iced_chat/main.rs:1612` | `return_to_chat_list_after_open = initial_topic.is_some() && command.is_none()` — after lobby open, go back to ChatList. | **BREAKS**: the "open lobby then return home" dance disappears. |
| `examples/iced_chat/main.rs:1671-1689` | Boot task `OpenRoom(lobby_topic)`. | **BREAKS**: automatic `OpenRoom` for the discovery topic must be removed (insertion point D). |
| `examples/iced_chat/main.rs:1110-1113` | Startup raw lobby subscription (DISCOVERY). | **KEEP** — becomes the internal discovery subscription (or moves into DiscoveryService). |
| `examples/iced_chat/main.rs:1279-1282` | Splash/log text `subscribed to lobby topic` / `Lobby joined — discovering peers`. | **UPDATE** terminology to discovery/rendezvous. |
| `examples/iced_chat/main.rs:1102,1155,1161,1282` | Log strings `public-lobby DHT discovery disabled`, `public-lobby continuous DHT tracker started/failed`. | **UPDATE** terminology. |
| `examples/iced_chat/main.rs:2588-2626` (unit tests) | `default_lobby_topic_matches_canonical_mainnet_lobby`, `tracker_identity_matches_default_lobby_topic`. | **KEEP/ADAPT**: identity tests are desirable; ensure the new discovery identity is tested separately. |
| `examples/iced_chat/mcp_server.rs:22,494,503,2439-2498,5739,5782` | `boru_join_lobby_room` opens the lobby via GUI action path with the **stale literal hash** (`:2451`). | **BREAKS/NEEDS DECISION**: MCP tool cannot target the internal discovery topic as a chat room; the stale hash must be resolved (BORU-DISC-01 §6). |
| `examples/iced_chat/app.rs:7886-7895` | `default_lobby_topic()` helper used by all lobby checks. | **KEEP** for identity; move ad-hoc checks to a `topic_kind()` classifier (BORU-DISC-03 §3). |
| `examples/iced_chat/app.rs:11846,12103-12107` | Mesh text `Connecting to lobby...`, `Connected to lobby — N peer(s) online`. | **UPDATE** to discovery terminology; home screen relies on them (`home.rs:1182-1183`). |
| `examples/iced_chat/app.rs:12163-12186` | Post-subscribe: join pending discovered peers for lobby. | **RELOCATE** to discovery service (BORU-DISC-03 checklist 4). |
| `examples/iced_chat/app.rs:12524-12541` | Lobby `ConversationLive` retention in `self.conversations`. | **REMOVE** — discovery must own its sender lifetime (insertion point F). |
| `examples/iced_chat/app.rs:14061-14071` | MCP diagnostic `OpenRoom` treats canonical lobby as known room without history. | **REMOVE/RELOCATE** to discovery diagnostics. |
| `examples/iced_chat/app.rs:16107-16121` | `should_announce_new_peer` suppresses announcements for non-friend lobby participants. | **SIMPLIFY** once lobby traffic never reaches `push_system` (guard makes this moot). |
| `examples/iced_chat/app.rs:1631-1632,26020-26021,19287` (unit tests) | `is_transient_mesh_event`/`mesh_event_tone` treat `Connecting to lobby...`/`Connected to lobby...` as transient/success. | **UPDATE** strings with discovery terminology. |
| `examples/iced_chat/app/chat.rs:4882` | Comment: pending-events queue exists "on dense public topics like the shared lobby". | **KEEP** queue for conversations; discovery traffic must not enter it. |
| `examples/iced_chat/app/home.rs:1178-1183` | Mesh Health card shows "lobby state + connection duration" from `sender_ready`/`mesh_connected_at`. | **UPDATE** — display discovery connectivity rather than a lobby conversation. |
| `examples/iced_chat/app/sidebar.rs:586-588` | "Chats" section doc-comment: "public room pinned at top". | **VERIFY** — no pinning code found; the lobby appears as a chat only if the conversation store has it (BORU-DISC-02 §7). Clean the comment. |

CLI flags audited (`main.rs:132-189`): `--secret-key`, `--relay`, `--no-relay`, `--no-dht`,
`--publish-direct-addresses`, `--data-dir`, `--name`, `--bind-port`, `--perf`, `--mcp`,
`--enable-gui-test-actions`, `--mcp-bind`, subcommands `open` / `join` / `logs`.
**Only the no-subcommand default and `--enable-gui-test-actions`'s lobby MCP action assume a
user-visible lobby.** `--no-dht` disables DHT member discovery but leaves the lobby gossip
subscription active — that distinction survives the refactor (the internal discovery topic is
joined regardless; DHT is one of its peer sources).

### 3.3 Docs

| File | Lobby assumption | Impact |
|---|---|---|
| `docs/discovery-architecture.md:167-197` | GUI startup flow: same lobby topic for gossip subscription, tracker identity, and **initial selected room**. | **UPDATE** — the initial-selected-room part is the coupling to remove. |
| `docs/protocol-layers.md:285-287` | **STALE**: claims `ContinuousTracker` "is never spawned in `main.rs`". **Verified false** — `main.rs:1141` spawns `ContinuousTracker::start_with_joiner`. | **FIX** the factual error regardless of refactor. |
| `docs/compatibility-identifiers.md:45-48,80` | Records lobby identity inputs as MUST-KEEP. | **KEEP** — supports the guardrail to not change deterministic derivation. |
| `docs/configuration.md:17` | `--no-dht`: "Disable private-room DHT discovery (public lobby unaffected)". | **UPDATE** wording when discovery is split. |
| `docs/gui-architecture.md:80` | "Open room, join lobby, send messages..." as GUI test actions. | **UPDATE** — remove "join lobby" as a user action. |
| `docs/mcp-agent-instructions.md:96,137-160,168-176,290` | Lobby as diagnostic room: `boru_join_lobby_room`, room_id="lobby", `boru_send_probe` on lobby topic, "subscribed to lobby topic" log check. | **UPDATE** — MCP tool set changes; discovery diagnostics become separate. |
| `docs/plans/public-rooms-hybrid-registry.md:82,108-119` | "Start ContinuousTracker for user-created rooms, not just public-lobby"; replace `neighbors.len()` (lobby count) with per-room counts. | **ALIGNS** with the target architecture (public rooms stay explicit); note the lobby-specific bits become discovery-owned. |
| `docs/BORU_BRANDING_AUDIT.md:148,284-290` | Records `b"boru-chat/public-lobby/v1"` (DHT key, preserve) and the **old** `b"iroh-gossip-chat/default-lobby/v1"` literal at `app.rs:3364`/`mcp_server.rs:1932` (stale line refs — current literal only at `mcp_server.rs:2451`; `app.rs` no longer contains it). | **UPDATE** stale line refs; decision on the legacy literal (BORU-DISC-01 §6). |
| `docs/branding-rename-deliverables.md:128,387-389` | Same legacy-literal records with stale line refs. | **UPDATE** refs. |
| `docs/testing.md:21` | Test inventory line: "test_public_lobby_integration.rs # Public lobby discovery tests". | **KEEP** (accurate — discovery tests). |
| `docs/ui-redesign/HOME_BASELINE.md:29,99,105`, `UI-HOME-05-report.md:8-114`, `UI-HOME-10-report.md:27-86`, `UI-HOME-16/17/18` reports, `current-ui-map.md:104,194` | Home screen shows a "Lobby: connected/connecting…" line; startup auto-selects the lobby; "lobby race" after subscription; mesh events `Connecting to lobby...` / `Connected to lobby — 1 peer online`. | **UPDATE** reports/terminology; the home card must reflect discovery state, not a lobby conversation. |
| `docs/ui-redesign/evidence/ui-18/README.md:87,160-161`, `ui-18-worker-report.md:153-154` | Evidence scripts' "lobby race" description. | Historical evidence; re-capture after refactor. |
| `docs/cargo-migration/01-cargo-audit.md:248,278`, `02-legacy-iced-chat-inventory.md:179,246`, `07-bin-asset-paths.md:86`, `09-smoke-results.md:7-107`, `10-final-report.md:128`, evidence `*.log` | Operational/smoke evidence: "lobby + directory topics subscribed", `boru_join_lobby_room` timing quirk. | Historical; useful regression anchors but must be re-run after refactor. |
| `docs/discovery-refactor/01-03` | Predecessor audits. | **KEEP** — this doc supersedes none; it consolidates. |
| `docs/secure-tunnels-design.md:76` | Startup sequence: "subscribe to lobby/directory rooms and start discovery/background trackers". | **UPDATE** wording. |

### 3.4 Scripts

| File | Lobby assumption | Impact |
|---|---|---|
| `scripts/ui08_home_hero_screenshots.sh:6-18,130+` | "the lobby opens and the app auto-returns to the chat list"; two instances "on the same lobby topic (mDNS connects them)". | **BREAKS** — home-mode determinism relies on the auto-open + `return_to_chat_list_after_open` behavior. Rework to launch without a visible lobby. |
| `scripts/ui18_responsive_evidence.sh:46-50,118-122` | `home_mode` omits `open` so `return_to_chat_list_after_open=true` lands deterministically on ChatList; comment "the app may be on the lobby chat" race. | **BREAKS** — must launch in a state that never shows a lobby. |
| `scripts/ui_home04_hero_evidence.sh:10` | "two instances on the same lobby topic: mDNS connects them". | **UPDATE** wording; still valid as discovery connectivity. |
| `scripts/ui_home05_mesh_health_evidence.sh:9,119-120` | Mesh events `Connected to lobby — 1 peer online`; waits for `Connecting to lobby...` to land before clearing. | **UPDATE** event strings with discovery terminology. |
| `scripts/fs23_launch.sh:89` | Comment: receiver's "lobby subscription registers before its endpoint binds". | **UPDATE** wording only. |
| `scripts/seed_boru_data.py`, `seed_chat_history.py`, `seed_two_instances.py`, `boru-test-instance.sh`, `mcp_call.sh`, `fs23_mcp.py`, others | No lobby reference found. | **Safe.** |

### 3.5 CI (`.github/workflows/`)

`ci.yaml`, `tests.yaml`, `simulation.yaml`, `release.yaml`, `beta.yaml`, `cleanup.yaml`,
`codeql.yml`, `commit.yaml`, `docs.yaml`, `flaky.yaml`, `apply-version.yml`, `version-check.yml`:
**zero lobby references** (verified by repo-wide search). CI neither asserts nor depends on the
visible lobby. **Safe** — but Phase-7 topic-isolation tests should be added to `tests.yaml`.

### 3.6 Fixtures and migration code

- Fixtures: no lobby assumptions (verified `tests/download-fixtures`, `generate_test_images.py`,
  `gen_stress_data.rs`).
- Storage/migrations: no lobby references in `src/storage.rs` schema or migrations; the lobby is
  **not** persisted as a conversation by the constructor (BORU-DISC-02 §4). SQLite `message_store.db`
  rows are keyed by topic bytes only — legacy lobby rows may exist for users who chatted in the
  lobby and would need the Phase-5 stale-lobby cleanup (PDF task 16).
- Branding/migration docs (§3.3) carry the stale legacy literal records.

## 4. Breakage map — everything that must change when the lobby stops being user-visible

**Will break at runtime if not handled (GUI):**
1. `main.rs:582-588` default `initial_room` selection + `main.rs:1612` + `main.rs:1671-1689`
   boot `OpenRoom` — remove the auto-open; land on Home/ChatList.
2. `app.rs:12524-12541` `ConversationLive` retention — replace with discovery-owned sender.
3. `app.rs:11846`, `app.rs:12103-12107` mesh strings, `app/home.rs:1178-1183` home card.
4. `app/chat.rs:4788-4950` NetEvent handler — add the defensive topic-kind guard.
5. `app.rs:12163-12186` join-pending-peers, `app.rs:14061-14071` MCP known-room,
   `app.rs:16107-16121` announce suppression.
6. `mcp_server.rs:2439-2498` `boru_join_lobby_room` + stale hash `:2451`.
7. Sidebar doc-comment (`sidebar.rs:586-588`) and any lingering "public room pinned" assumption.

**Will break if not handled (tests/scripts/docs):** §3.3 docs (protocol-layers stale claim,
configuration wording, gui-architecture, mcp-agent-instructions, HOME reports), §3.4 evidence
scripts (ui08/ui18 rely on auto-open-and-return), §3.1 main.rs unit tests asserting
`default_lobby_topic` identity (keep, adapt).

**Must NOT break:** direct-topic derivation, group topics, public-room explicit create/join,
private-room tracker, directory mesh, backfill policies for user rooms, `test_branding_rename`
identity vectors.

## 5. Recommended target architecture

**Adopt the PDF target:** on startup every node joins one internal discovery topic
(`BORU_DISCOVERY_TOPIC_V1`, Phase-2 task 5) treated purely as **networking infrastructure**:

- **Not** a conversation: no `ConversationLive`, no `ConversationStore` entry, no `Screen::Chat`,
  no sidebar row, no unread count, no persisted history, no notifications, no system entries,
  no read receipts, no outbound send path.
- **Owned by a DiscoveryService** (Phase-2 task 7) that subsumes the raw startup drain
  (`main.rs:1179-1218`), the DHT tracker fanout, the dynamic joiner, and the mDNS join path —
  the sender lifetime currently hacked into `self.conversations` moves here.
- **Public chats remain explicit user features** (Phase-3 task 12): user-created/user-joined
  public rooms keep their own topics, tickets, `ConversationStore` entries, and UI. They are
  *conversations*, never the discovery topic.
- **Separation mechanism = the Phase-4 routing guard** (PDF §"Recommended routing guard";
  BORU-DISC-03 §3): a pure `topic_kind(topic) -> TopicKind::{Discovery, Conversation}` classifier
  applied at the **forwarder-spawn boundary** (`src/conversations.rs:674-726` call sites:
  `app.rs:11578, 11983, 12821`, `app/discover.rs:1836`) as the primary gate, plus a **defensive
  guard** at `AppMessage::NetEvent` (`app/chat.rs:4788`) before any conversation side effect.
  Discovery topics route to the DiscoveryService; conversation topics use the existing forwarder.
- **Guardrails preserved:** deterministic direct/public derivations unchanged; discovery state
  never merged with conversation state; no hidden "chat" object; private DMs and normal chat
  payloads never routed through the discovery topic (hard rule); small testable steps; current
  group/direct messaging behavior preserved.

### 5.1 Concrete decision needed before BORU-DISC-05 (carried from 01/03)

1. **`BORU_DISCOVERY_TOPIC_V1` identity**: separate derivation vs reusing the canonical lobby
   topic. The PDF names a versioned constant; keep `default_lobby_topic()` untouched either way
   (guardrail), and classify the old lobby topic as Discovery during a transition if mixed-version
   compatibility is required (Phase-5).
2. **MCP stale lobby hash** (`mcp_server.rs:2451`): do not treat as canonical; add an explicit
   compatibility decision — either drop `boru_join_lobby_room` or retarget it to a
   discovery-diagnostics tool.
3. **Backfill policy for the discovery topic** (`src/backfill.rs:208-258`): today the canonical
   lobby is "open to any authenticated peer" (`:1573-1590`). If the discovery topic is
   infrastructure-only, it should be excluded from chat backfill or handled by the
   DiscoveryService, not the conversation backfill path.
4. **Home screen copy**: replace "Lobby: connected…" wording with discovery/connectivity wording
   (Mesh Health card already exists; only labels/strings change).

## 6. Migration checklist for the implementation chain (BORU-DISC-05+)

1. Add `TopicKind` + `topic_kind()` classifier (BORU-DISC-05/10); keep all existing derivations.
2. Define `BORU_DISCOVERY_TOPIC_V1` + discovery message types (Hello/Presence/PeerAdvertisement)
   with protocol version and node identity (BORU-DISC-05/06).
3. Add `DiscoveryService`; move startup join, receiver drain, tracker fanout, mDNS joins, and the
   sender-lifetime workaround out of conversation code (BORU-DISC-07/08).
4. Route discovery topics away from `spawn_conversation_forwarder`; add the defensive guard at
   `app/chat.rs:4788` (BORU-DISC-10).
5. Remove the lobby UI/chat insertion points (startup `OpenRoom`, `ConversationLive`, history
   replay, unread, mesh strings, MCP tool) per §4 (BORU-DISC-11+).
6. Update docs (§3.3) and evidence scripts (§3.4); fix the stale `protocol-layers.md` claim.
7. Add Phase-7 topic-isolation tests + two-node restart tests; run the full suite.

## 7. Verification

- [x] `docs/discovery-refactor/00-architecture-note.md` exists (this file).
- [x] Cross-references `01-lobby-constants-map.md`, `02-startup-flow.md`, `03-message-handling.md`.
- [x] Dependency inventory covers tests, fixtures, examples, CLI flags, docs, scripts, CI.
- [x] Read-only: no source files changed; only this doc added.
