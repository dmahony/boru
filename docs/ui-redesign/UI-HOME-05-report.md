# UI-HOME-05 — Restore the Full Mesh Health Card

- Task: `t_faa541a0` (UI-HOME-05)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-05 card)
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: COMPLETE. The one-line "Mesh Activity" summary is replaced with the
  full "Mesh Health" dashboard card: mesh-glyph header, real status badge,
  live connection-count stat tiles, lobby + duration line, and a short
  recent-events list fed from the same bounded mesh event log the rest of the
  app uses. No statistics or events are invented; the existing "View details"
  destination is preserved.

## 1. What was delivered

The Mesh Activity card (a single status row inside a CardShell) is now the
Mesh Health card (a content-driven dashboard card):

| Area | Content | Live source |
|---|---|---|
| Header icon | mesh glyph (`ICON_MESH`) | static asset (presentation) |
| Title | "Mesh Health" | static label |
| Subtitle | "Current connection status" | static label |
| Status badge | Healthy / Degraded / Offline pill | `dep.mesh_health` (`MeshHealthSnapshot` ← `self.mesh_health`) |
| Header action | "View details" → `AppMessage::OpenConnectionDetails` | existing destination (unchanged) |
| Status row | status icon + label + detail (peer counts + duration) | `home_connection_variant(mesh_health, has_peer_connections, sender_ready)` + `dep.direct_peers` / `dep.relayed_peers` / `dep.neighbors_len` / `dep.connected_age_secs` |
| Stat tiles | Neighbors / Direct / Relayed | `dep.neighbors_len` / `dep.direct_peers` / `dep.relayed_peers` |
| Lobby line | "Lobby: connected" / "Lobby: connecting…" + duration | `dep.sender_ready` (`self.sender.is_some()`), `dep.connected_age_secs` |
| Recent events | up to 4 rows: small status icon + description + relative time | `self.mesh_event_log` (`VecDeque<MeshEvent>`, capacity 50) snapshotted into `ChatListDependency.mesh_events` |
| No-events state | "No recent mesh events" + activity icon | empty `mesh_events` slice |

## 2. Data-source mapping (every visible value → state field)

| Visible value | State field | Where |
|---|---|---|
| "Mesh Health" title | static | `view_chat_list_content` (app.rs) |
| Healthy/Degraded/Offline pill | `self.mesh_health` → `MeshHealthSnapshot` → `StatusBadgeKind` | app.rs mesh card build |
| Status icon/label ("Connected", "Starting up…", "Degraded — …", …) | `home_connection_variant(&mesh_health, has_peer_connections, relay_reachable)` | pure fn, tested |
| "N direct · N relayed · N neighbors" detail | `dep.direct_peers` / `dep.relayed_peers` / `dep.neighbors_len` | snapshot fields |
| "connected Xm Ys" | `dep.connected_age_secs` (from `self.mesh_connected_at`, watchdog-maintained) | snapshot field |
| Neighbors / Direct / Relayed tiles | `dep.neighbors_len` / `dep.direct_peers` / `dep.relayed_peers` | snapshot fields |
| "Lobby: connected" / "Lobby: connecting…" | `dep.sender_ready` (`self.sender.is_some()`) | snapshot field |
| Recent event rows | `self.mesh_event_log` (push_mesh_event at app.rs:8088; capacity 50) → `ChatListDependency.mesh_events` (newest-first, capped 4, `age_secs` captured at snapshot time) | `chat_list_dependency()` |
| Event row icon/tone | `mesh_event_tone(message)` + `mesh_event_visual(tone)` — content classification of real log lines | pure fns, tested |
| Relative time ("just now" / "5s ago" / "2m ago") | `presentation::relative_age_secs(age_secs, 10)` | new helper, tested |

Mesh events are pushed by real lifecycle code only: lobby connect
(app.rs:11127-11133), peer discovery (app.rs:18500-18503), conversation
subscription (app.rs:18560-18562), and watchdog transitions
(app.rs:18596-18597). `clear_transient_mesh_events()` (UI-28) keeps
"Starting up...", "Connecting to lobby...", "Connected to lobby..." and
"Subscribing to..." from lingering once the mesh is Good.

## 3. Component changes

- `card_shell.rs` — **extended** the shared dashboard-card foundation with an
  optional `.header_icon(element)` slot rendered before the title column. The
  shell stays data-agnostic; the caller owns the glyph element.
- `presentation.rs` — new `relative_age_secs(age_secs, just_now_seconds)`
  helper mirroring `relative_time_at`'s thresholds for monotonic-age event
  rows (mesh events record `Instant`, so ages are captured at snapshot time).
- `app.rs` — new `MeshEventRow` snapshot + `mesh_events` on
  `ChatListDependency`; new `MeshEventTone` classifier + `mesh_event_visual`;
  full card body (status row, stat tiles, lobby line, divider, recent events,
  no-events state); regression guard updated ("Mesh Activity" → "Mesh
  Health").
- `src/diagnostics.rs` — **test-only** `GuiTestCommand::ClearMeshEventLog`
  variant (validate + expected_state + round-trip test) so evidence harnesses
  can capture the intentional no-events state without fabricating events.
- `app.rs` GuiTestActionReceived — handles `ClearMeshEventLog` (clears the
  live log, marks the action complete).
- `component_gallery.rs` / `DESIGN_SYSTEM.md` — "Mesh Activity" → "Mesh
  Health" naming.

## 4. Tests

- `cargo build --example boru --features gui` — OK (exit 0).
- `cargo test --example boru --features gui` — **869 passed / 0 failed**
  (prior: 864; +5 net new).
- New tests:
  - `card_shell_stores_header_icon`, `card_shell_build_with_header_icon_does_not_panic`
  - `relative_age_secs_matches_wall_clock_thresholds`
  - `mesh_event_tone_classifies_real_log_lines_truthfully`
  - `mesh_event_visual_covers_every_tone`
  - `test_gui_test_command_clear_mesh_event_log_validates_and_round_trips`

## 5. Evidence

- `docs/ui-redesign/evidence/t_faa541a0/t_faa541a0_populated_1600x900.png` —
  two seeded instances connected over localhost QUIC; card shows Healthy /
  Connected, "1 direct · 0 relayed · 1 neighbors", Neighbors/Direct/Relayed
  tiles, "Lobby: connected", and four real events ("Discovered 1 direct, 0
  relayed peer", "Connected to lobby — 1 peer online", "Connecting to
  lobby...", "Starting up...").
- `docs/ui-redesign/evidence/t_faa541a0/t_faa541a0_populated_1280x800.png` —
  same two-instance state at the reference viewport (Connecting detail,
  real events, "View details").
- `docs/ui-redesign/evidence/t_faa541a0/t_faa541a0_noevents_1280x800.png` —
  fresh instance, mesh event log cleared via test-only
  `boru_send_gui_action` `clear_mesh_event_log`; card keeps its summary and
  shows "No recent mesh events".
- `docs/ui-redesign/evidence/t_faa541a0/t_faa541a0_details_1280x800.png` —
  "View details" clicked (OCR-located); the connection-details dialog opens
  with Relay URL, "Room: Chat List open « Mesh: Healthy", transport state,
  Copy details / Close.

Evidence script: `scripts/ui_home05_mesh_health_evidence.sh
[populated|noevents|details|all]` (Xvfb + xdotool + ImageMagick + tesseract;
two instances use deterministic seeds + direct QUIC addresses from
`seed_two_instances.py`).

## 6. Height / sizing notes

The card is content-driven (no fixed height, no hidden overflow): header +
status row + stat tiles + lobby line + divider + events header + up to four
32 px event rows lands in the plan's ~270–330 px populated band at the
reference viewport, and shrinks naturally for the no-events state. The events
slice is capped at 4 in the dependency selector so a long log cannot grow the
card without bound.

## 7. Changed files

- `examples/iced_chat/card_shell.rs` (header_icon extension + tests)
- `examples/iced_chat/presentation.rs` (relative_age_secs + test)
- `examples/iced_chat/app.rs` (MeshEventRow, dependency, classifier, card
  body, ClearMeshEventLog handler, guard update, tests)
- `src/diagnostics.rs` (test-only ClearMeshEventLog variant + test)
- `examples/iced_chat/component_gallery.rs` (demo title)
- `DESIGN_SYSTEM.md` (naming)
- `scripts/ui_home05_mesh_health_evidence.sh` (new)
- `docs/ui-redesign/evidence/t_faa541a0/` (4 png)

## 8. Remaining risks

- The no-events capture depends on the test-only `ClearMeshEventLog` command
  being available (`--enable-gui-test-actions` only); production behaviour is
  unchanged.
- `MeshEventTone` classification is content-based (substring match). Unknown
  future log lines fall back to `Neutral` (muted activity icon) rather than
  being misrepresented; the classifier is unit-tested against the current
  real log vocabulary.
- The two-instance evidence uses `--no-dht --no-relay` + seeded direct QUIC
  addresses, so it is deterministic on localhost; real-world discovery may
  produce more varied event text, which the neutral fallback handles.
- Height band is approximate (content-driven); a final visual pass
  (UI-HOME-18) owns exact pixel QA.

No business/network/state logic touched. This card gates UI-HOME-09.
