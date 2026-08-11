# BORU-CARGO-09 — Boru smoke test on DEBSRV (window, UI, data, net, conversation)

**Task:** t_c3e83dd2 (BORU-CARGO-09, step 9 of the Boru Cargo target migration)
**Date:** 2026-08-12
**Commit audited:** `bd3467e5` (origin/main @ BORU-CARGO-08). Worktree `wt/t_c3e83dd2` fast-forwarded to origin/main before testing.
**Build host:** debsrv (172.16.0.59, 8 cores, 31 GiB) via the `rb` wrapper (slot 2).
**Outcome:** **SMOKE PASS — no observable behavioural regression vs the BORU-CARGO-01 baseline.** Main window opens; home/chat UI renders with bundled fonts + lucide icons; existing local data/config is found across restarts (same identity, no re-migration); networking initialises to the same state as the baseline (same relay, same two LAN peers joined + direct-connected, lobby/directory topics subscribed); a conversation opens and the composer→send→SQLite→UI pipeline works; zero panics / zero missing-file errors / no target-specific warnings in any run.

---

## 1. Build & deploy

| Step | Command | Result |
|---|---|---|
| Build | `rb build --bin boru --features gui` (debug) | **PASS** — 40.85s, exit 0, 259 warnings (identical count to the BORU-CARGO-01 baseline; all pre-existing `#[expect(dead_code)]` lints) |
| Deploy | `cp ~/boru-build/work-target-2/debug/boru /home/dan/boru` on debsrv | 1,229,140,088 bytes, mtime 2026-08-12 06:16 (fresh, after HEAD) |

**Profile note:** debug build used, not release. Debug is sufficient for a behaviour smoke test (the GUI, bundled assets, and networking behave identically; the release profile was already proven by BORU-CARGO-08 gate item 9, `rb build --release` = 12m45s). No asset/path difference exists between profiles — all fonts/icons are `include_bytes!`-bundled at compile time.

## 2. Smoke runs (all on debsrv, xvfb headless)

All four runs used the same installed binary `/home/dan/boru`. Runs 2–4 re-used the **same data dir** (`/tmp/boru_data_cargo09a`) created by run 1.

| Run | Launch | MCP ready | Evidence |
|---|---|---|---|
| #1 fresh data | `scripts/start_boru_headless.sh debsrv cargo09a 19066 /tmp/boru_data_cargo09a` (xvfb-run + `--relay boru.chat:8443 --mcp --enable-gui-test-actions --mcp-bind 127.0.0.1:19066 --name cargo09a open`) | 22s | `run1-open-room.png`, `run1-home.png`, `run1-boru.log`, `run1-node-status.json`, `run1-gui-snapshot.json` |
| #2 existing data | custom headless launch, **same data dir**, no subcommand (home screen), port 19067, name cargo09b | 17s | `run2-home.png`, `run2-boru.log`, `run2-node-status.json`, `run2-secret-key.sha256` |
| #3 existing data | same data dir, no subcommand, port 19068, name cargo09c + MCP lobby join probe | 17s | `run3-lobby.png`, `run3-room-status.json` (lobby joined, 2 peers), `run3-boru.log` |
| #4 existing data | same data dir, `open` subcommand (conversation), port 19069, name cargo09d + MCP composer send | 16s | `run4-conversation.png` (message bubble), `run4-set-composer.json`, `run4-submit.json`, `run4-boru.log` |

Evidence files: `docs/cargo-migration/evidence/t09-smoke/` (committed).

## 3. Acceptance criteria vs baseline

### 3.1 Main window opens / UI renders with fonts + icons — PASS
- Run 1 captured a real rendered window under Xvfb (`run1-open-room.png`: conversation view with header, E2E lock icon, composer, sidebar) and the home/dashboard (`run1-home.png`: "Your Boru node is online and ready", connection-status card, Mesh Health card, quick-action cards, People & Activity + TUNNELS panels).
- Vision + pixel checks: bundled Figtree/Public Sans/Inter Tight fonts render crisply (no tofu/glyph boxes), lucide icons render (gear, home, plus, search, folder, share, bell, paperclip, send, lock, mesh). No rendering glitches.
- Home-screen determinism: `run2-home.png` vs `run3-lobby.png` (both no-subcommand, same data) — `compare_screenshot.py` PASS, 1.14% mismatch (tolerance 12, max 5%), deltas are only the activity-feed timestamps / "connected Ns" counter (`run2-vs-run3-metrics.json`, `run2-vs-run3-diff.png`). Same pattern as BORU-CARGO-07's run1-vs-run2 (0.94%).

### 3.2 Existing local data/config found — PASS
Run 2 (same data dir as run 1):
- **Identity:** `> our public key: b04a2ef7dc5d33a49aba0d2bf34968319b9608fa754ca69f3ea8f79fca94b0c6` — **identical** in all four runs (run1-node-status, run2-node-status, run3-node-status, run4-node-status). `secret_key.txt` mtime unchanged (06:16, created in run 1; read in runs 2–4, not rewritten).
- **Storage:** `storage opened successfully db_path=/tmp/boru_data_cargo09a/boru.db` on every run; **zero "running database migrations" lines in the run-2+ blocks** (no re-migration).
- Data dir contents (blobs/, boru.db, downloads/, logs/, message_store.db, secret_key.txt, seen_peers.json) preserved across runs.
- Outgoing-message persistence exercised in run 4: `SQLite insert_outgoing_message OK for event_id=1` → the message row is written to SQLite (`outgoing_messages`), same storage path as pre-migration.

### 3.3 Networking initialises to the same state as baseline — PASS
Startup sequence in every run is byte-for-byte the same shape as `docs/cargo-migration/evidence/t01-baseline/startup-boru.log`:

| Baseline (t01) | This smoke test |
|---|---|
| `> relay: boru.chat:8443` | same (`--relay boru.chat:8443`) |
| `relay.online() timed out after 15s, proceeding anyway` (WARN, pre-existing endpoint-online mitigation) | same WARN in runs 1–4 |
| `endpoint address ready` → `> endpoint: e5c0faa8…` | same, `b04a2ef7…` |
| `Mainline DHT started address=0.0.0.0:6881` | same |
| `public-lobby continuous DHT tracker started room=ebab66f6` | same |
| `subscribed to lobby topic` / `subscribing to directory topic … d68fa4ec…` | same |
| `join_peers succeeded peer=47974d77…` + `peer=754d5785…` (the two LAN test VMs 172.16.0.54/55) | same two peers, every run |
| `direct connect succeeded` for both peers | same |
| `RoomOpened FIRED … neighbor_count=0` | same (newly opened room has no peers yet — identical to baseline) |

Lobby membership (run 3, MCP `boru_get_room_status` on `ebab66f6…`): `joined:true, subscribed:true, peer_count:2`, both peers `connected:true, topic_member:true`. Same 2-peer state as baseline.

### 3.4 Conversation opens — PASS
- CLI `open` subcommand opens a chat room: `run1-open-room.png` (Room e057e7ed, "Chat joined", E2E-encrypted header, composer) — same behaviour as the t01 baseline screenshot.
- MCP-driven send pipeline (run 4): `boru_gui_set_composer` (39 chars) → `boru_gui_submit_composer` → `run4-conversation.png` shows the outgoing message bubble **"BORU-CARGO-09 smoke message from debsrv"** with sender label, timestamp, and Sending state; log confirms `SQLite insert_outgoing_message OK for event_id=1`. GUI snapshot: `active_screen:"Chat"`, `active_room` set.
- Note: the MCP `boru_join_lobby_room` GUI action timed out in run 3 (join at the *protocol* layer succeeded — see 3.3; the GUI-level navigate timed out after 45s). This is a pre-existing MCP-tool quirk (the join handler's subscribe+join can exceed the client's socket timeout), unrelated to the cargo migration; the public conversation is verifiably open at the protocol layer (`run3-room-status.json`).

### 3.5 No new startup panics / missing-file errors / target-specific warnings — PASS
For every run's `$DATA_DIR/logs/boru.log`: `grep -iE "panic|missing file|no such file|cannot find|fatal|XNotSupported"` → **zero matches** (runs 1–4).
The only ERROR/WARN lines are pre-existing network bootstrap noise, identical in kind to the t01 baseline and BORU-CARGO-07 evidence:
- `swarm-discovery.actor: iroh_mdns_address_lookup: mdns subscriber is blocked` (WARN — also in baseline t01)
- `relay.online() timed out after 15s` (WARN — baseline t01)
- `mainline::rpc: Could not bootstrap the routing table` + `DHT put_mutable operation failed: Query(NoClosestNodes)` (ERROR/WARN — pre-existing headless-VM DHT behaviour, documented in BORU-CARGO-07 §5)
No target-specific warnings: build warning count is 259, identical to the BORU-CARGO-01 baseline.

## 4. Pre-existing observations (not migration regressions, documented per guardrails)
1. **Message stays "Sending" in a brand-new room** (run 4). The `open` room has `neighbor_count=0` (matches baseline), so the gossip mesh has no peers to deliver to; the footer reads "Not connected" exactly as the t01 baseline screenshot. Not a regression.
2. **MCP `boru_join_lobby_room` GUI action can time out** (run 3) while the protocol-level lobby join succeeds. Pre-existing MCP tool behaviour; not migration-caused.
3. **No conversation-store rows for the `open` room** (`dm_conversations`/`groups`/`chat_messages` empty in the data dir's SQLite). Consistent with the known pre-existing persistence gap (rooms are only persisted as conversation entries on activity; no message → no entry). BORU-CARGO-07's round-trip evidence shows the same behaviour; the migration changed nothing here.

## 5. Commands run (summary)
```bash
rb build --bin boru --features gui                      # debsrv slot 2 — 40.85s, exit 0
ssh debsrv 'cp ~/boru-build/work-target-2/debug/boru /home/dan/boru'
./scripts/start_boru_headless.sh debsrv cargo09a 19066 /tmp/boru_data_cargo09a   # run 1 — MCP ready 22s
# run 2/3/4: headless Xvfb launches on debsrv (same data dir), scrot screenshots,
# MCP JSON-RPC via scripts/ui_mcp.py (boru_ping / boru_get_node_status /
# boru_get_gui_snapshot / boru_get_room_status / boru_gui_set_composer /
# boru_gui_submit_composer / boru_join_lobby_room)
python3 scripts/compare_screenshot.py evidence/t09-smoke/run3-lobby.png evidence/t09-smoke/run2-home.png --diff ... --metrics ...   # PASS 1.14%
```

## 6. Evidence manifest (`docs/cargo-migration/evidence/t09-smoke/`, committed)
- Screenshots: `run1-open-room.png` (conversation via `open`), `run1-home.png` (home/dashboard), `run2-home.png` (home, existing data), `run3-lobby.png` (home, repeatability), `run4-conversation.png` (conversation with sent message), `run2-vs-run3-diff.png`
- Logs: `run1-boru.log`, `run2-boru.log`, `run3-boru.log`, `run4-boru.log`, `run{1,2,3,4}-stdout.log` (GUI-test-actions banner)
- MCP state: `run{1,2,3,4}-node-status.json`, `run{1,2,3,4}-gui-snapshot.json`, `run3-room-status.json` (lobby joined + 2 peers), `run3-lobby-join.json` (GUI join timed out), `run4-room-status.json`, `run4-set-composer.json`, `run4-submit.json`
- Comparison: `run2-vs-run3-metrics.json`
- Identity: `run2-secret-key.sha256`

## 7. Acceptance criteria checklist
- [x] No observable behavioural regression vs the BORU-CARGO-01 baseline (startup log sequence, relay, peers, topics identical; UI matches the end-of-migration BORU-CARGO-07 home).
- [x] Main window opens (Xvfb-rendered frames captured for every run).
- [x] Home/chat UI renders with fonts + icons (bundled fonts/lucide verified in screenshots; no missing glyphs).
- [x] Existing local data/config found (same identity across 4 runs, no re-migration, storage opened cleanly).
- [x] Networking initialises (same relay, same 2 LAN peers joined + direct-connected, lobby + directory topics).
- [x] A conversation opens (CLI `open` + MCP composer send → message bubble renders; lobby protocol-joined with 2 topic members).
- [x] No new startup panics / missing-file errors / target-specific warnings (grep-clean logs, 259 pre-existing build warnings).
