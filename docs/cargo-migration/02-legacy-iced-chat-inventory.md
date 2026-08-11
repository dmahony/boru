# BORU-CARGO-02 — Legacy `iced_chat` naming + `--example` usage inventory

**Task:** t_8987e193 (BORU-CARGO-02, step 2 of the Boru Cargo target migration)
**Date:** 2026-08-11
**Tree audited:** `origin/main` @ `119b633d` (worktree `wt/t_8987e193` fast-forwarded to origin/main before searching; BORU-CARGO-01's `01-cargo-audit.md` is the entry-point source of truth for target facts)
**Scope:** read-only inventory. **No source, docs, script, or Cargo.toml changes were made** — the only file added by this task is this inventory.
**Inputs:** repo-wide `git grep` (tracked files only; excludes `target/`, `.git/`, `captures/`, untracked binaries).

---

## 0. Raw grep counts (origin/main @ 119b633d)

| Pattern | Matches (all files) | Notes |
|---|---|---|
| `iced_chat` | 2,358 | the overwhelming majority are path references `examples/iced_chat/...` |
| `examples/iced_chat` (path) | 2,278 | source `mod`/`#[path]`/`include_str!`, docs, evidence logs |
| `--example` | 358 | build/test/launch invocations across docs, scripts, CI, justfile, patched/ crates |
| `[[example]]` | 103 | Cargo.toml declarations, mostly in patched/ vendored crates |
| `iced-chat` | 6 | `.cargo/config.toml` alias + comments; `patched/` zero |
| `iced chat` (lowercase) | 5 | log line + comments (main.rs, perf_tracker.rs, friend_ping.rs) |
| `Iced Chat` (title case) | 2 | comments (perf_tracker.rs + UI audit history) |
| Files touched | 230 | 142 `docs/`, 25 `patched/`, 12 `tests/`, 11 `scripts/`, 9 `examples/`, 4 `src/`, 2 `.github/`, root md + config + justfile + report.html |

Verification greps (re-run to confirm coverage):
```
git grep -c -I -e 'iced_chat|iced-chat|Iced Chat|iced chat|examples/iced_chat'   # 230 files
git grep -l -I -e '--example' | wc -l                                            # 128 files
```

---

## 1. Classification legend

| Code | Classification | Rename verdict meaning |
|---|---|---|
| (a) | Cargo target name | target name / `[[example]]` / `--example <name>` invocation |
| (b) | source/module name | file path, module path, symbol, test fixture path |
| (c) | documentation/script | markdown, shell script, CI config, justfile, alias |
| (d) | test/CI command | command line in tests, CI, or verification docs |
| (e) | packaging command | build/package/release pipeline command |
| (f) | runtime/persisted identifier | storage path, data-dir name, DB key, wire/protocol string, topic domain separator, URI scheme, env var contract |

Verdicts: **SAFE** = application scaffolding / developer-facing Boru naming that the migration may rename. **UNSAFE** = persisted/runtime/protocol identifier that must never be renamed without proof of wire/data compatibility (each carries a "retained because…" note).

---

## 2. Cargo target names — the `boru` example and the demo examples (a)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `Cargo.toml:326-329` | `[[example]] name = "boru"` `required-features = ["gui"]` `path = "examples/iced_chat/main.rs"` | (a) | **SAFE** (target name already `boru`; path changes with the directory move) | The example NAME is already migrated (`iced_chat`→`boru`, commit 2cd365e7). Only the `path` points at the legacy directory. `required-features=["gui"]` is load-bearing (forces `--features gui`); keep it on any replacement target. |
| `Cargo.toml:322-324, 331-333, 335-337, 339-341` | `[[example]] setup` / `video_backend_probe` / `doctor` / `catalogue_browser` | (a) | **SAFE to keep — DO NOT REMOVE** | Genuine demo examples (auto-discovered `dht_harness`, `test_addr` additionally). Any migration must preserve these targets. |
| `.cargo/config.toml:10` | `chat = "run --features examples --example chat --"` | (a/c) | **SAFE to keep** | Cargo alias for the demo `chat` example; unrelated to iced_chat (retained as a working dev alias). |
| `.cargo/config.toml:12` | `iced-chat = "run --features gui --example boru --"` | (a/c) | **SAFE to rename** | Dev-facing cargo alias; dead name (`cargo iced-chat` was the pre-rename alias). `boru` alias at line 13 is the current one. |
| `.cargo/config.toml:5-6` | comments `cargo iced-chat open/join <ticket>` | (c) | **SAFE to rename** | Stale doc comment. |

**No `iced_chat` target exists today.** Declaring `[[example]] name="boru" path="examples/iced_chat/main.rs"` claims the path, so Cargo auto-discovery does not create an `iced_chat` example. `cargo build --example iced_chat` → exit 101 (`no example target named 'iced_chat'`), verified in BORU-CARGO-01. All `--example` invocations in this repo that name a target use `boru` or one of the genuine demo examples.

---

## 3. The `examples/iced_chat/` source tree — the rename subject (b)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `examples/iced_chat/` (whole dir: main.rs + ~45 sibling modules incl. app.rs, app/ feature modules, fonts/, motd.txt) | — | (b) | **SAFE to rename as a unit** (subject of BORU-CARGO-03/05/06) | Entry `main.rs:465 fn main()`; iced root `app::IcedChat`. The dir is self-contained (no example-vs-bin assumptions; `main.rs:8-45` declares all sibling `mod`s). Renaming the dir breaks every reference in §4-§7 unless updated in lockstep. |
| `examples/iced_chat/main.rs:1-6` | doc comment `cargo run --features gui --example boru` | (c) | **SAFE to rename** | Dev-facing usage doc; already uses `--example boru`. |

---

## 4. Source references that pin the `examples/iced_chat` path (b — compile-time bindings)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `Cargo.toml:329` | `path = "examples/iced_chat/main.rs"` | (a) | **SAFE** (must move with the dir) | The only Cargo declaration of the path. |
| `tests/fs17_activity_log.rs:10` | `#[path = "../examples/iced_chat/activity_log_view_model.rs"]` | (b) | **SAFE** (must move with the dir) | Test compiles the view model via `#[path]`; breaks if dir renames without updating. |
| `tests/fs22_dashboard_coverage.rs:23,25,27,29,31` | `#[path = "../examples/iced_chat/{dashboard,downloaded,downloading,peers_downloading,recent_activity}_view_model.rs"]` | (b) | **SAFE** (must move with the dir) | Same `#[path]` pattern, 5 modules. |
| `tests/protocol_registration.rs:72` | `include_str!("../examples/iced_chat/main.rs")` | (b) | **SAFE** (must move with the dir) | Compile-time `include_str!` of the entry file (protocol-name regression test). |
| `examples/iced_chat/app.rs:24393, 24595, 24686` | `.join("examples/iced_chat/app.rs")` | (b) | **SAFE** (must move with the dir) | Test fixtures that assert on the source file's own path (`env!("CARGO_MANIFEST_DIR")`-relative). |
| `examples/iced_chat/app.rs:29685` | comment `rb test --example boru … -- offscreen_capture` | (c) | **SAFE** | In-file test command comment. |
| `src/backfill.rs:266` | doc `/// \`examples/iced_chat/app.rs\` (view_group_member_list)` | (c) | **SAFE** (doc path) | Doc comment; stale after rename. |
| `src/chat_core/friend_ping.rs:962` | comment `// Both chat-gui.rs and iced_chat/main.rs do:` | (c) | **SAFE** | Doc comment. |
| `src/ticket_share.rs:20` | doc `at startup (see \`examples/iced_chat/main.rs\`)` | (c) | **SAFE** | Doc comment. |
| `scripts/package-windows.sh:9` | comment `loader (\`papirus_asset_root()\` in examples/iced_chat/file_type_icon.rs)` | (c) | **SAFE** | Script comment; the runtime loader itself is path-independent (§10). |

---

## 5. Test and CI command references (d)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `.github/workflows/codeql.yml:30` | `cargo build --features gui --example boru` | (d) | **SAFE** (already `--example boru`) | CI must keep building the GUI. |
| `.github/workflows/release.yaml:71` | `cargo build --release --target ${{ matrix.target }} --features ${{ matrix.features }} --example boru` | (e) | **SAFE** (already `--example boru`) | Release packaging pipeline; `--example` required until a `[[bin]]` replaces it (BORU-CARGO-03+). |
| `justfile:12,16,20,26,34,38,58` | `cargo {build,check,run,clippy} --… --example boru` | (d) | **SAFE** | Dev recipes; already `--example boru`. |
| `scripts/install.sh:34,59` | `cargo build … --example boru` / `cargo run --example boru --features gui` | (e) | **SAFE** | Install/launch packaging. |
| `scripts/remote-test.sh:130-131` | `cargo build --example boru --features gui` | (d) | **SAFE** | Remote test harness. |
| `scripts/ui13_visual_regression.sh:30`, `ui14_states_evidence.sh:22`, `ui15_composer_evidence.sh:22`, `ui19_focus_evidence.sh:24`, `ui_baseline_screenshots.sh:31`, `ui_home01_baseline.sh:26` | `cargo build --features gui --example boru` / `rb build --example boru …` | (d/e) | **SAFE** | Evidence-capture scripts; already `--example boru`. |
| `scripts/flamegraph.sh:43` | `--example "$EXAMPLE"` | (d) | **SAFE** | Parameterized; `$EXAMPLE` is `boru` at call sites. |
| `examples/{catalogue_browser,dht_harness,doctor,video_backend_probe}.rs` | `cargo run --example <demo> …` doc comments | (c/d) | **SAFE to keep** | Genuine demo examples' own usage docs. |
| `src/perf.rs:13` | `BORU_PERF=1 cargo run --example boru` | (c) | **SAFE** | Dev doc. |
| `tests/repro_two_iced_instances.rs:6-7` | `cargo run --features gui --example boru open/join` | (c/d) | **SAFE** | Test doc; already `--example boru`. |
| `docs/cargo-migration/01-cargo-audit.md` (passim) | `--example boru` launch baseline; documents `--example iced_chat` as BROKEN | (c/d) | **SAFE** | Parent audit; already current. |
| ~100 other docs/evidence log files | `cargo {build,test,run} … --example boru` | (c/d) | **SAFE** | Historical evidence (build logs, READMEs); no code impact. |

---

## 6. Test / module-name `iced_chat` identifiers (b — developer-facing)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `examples/iced_chat/main.rs:1677` | `.expect("iced_chat boot called more than once")` | (b) | **SAFE** | Panic message string (internal). |
| `examples/iced_chat/main.rs:2243,2249,2257,2273,2283,2290,2298,2309,2321` | `Args::try_parse_from(&["iced_chat", …])` | (b) | **SAFE** | clap test harnesses; `"iced_chat"` is only the argv[0] label for usage rendering in tests, not a runtime invocation. |
| `examples/iced_chat/main.rs:2550,2571` | test `dir.path().join("iced_chat.log")` | (b) | **SAFE** | Test-local temp filename. Runtime log file is `<data_dir>/logs/boru.log` (init_logging) — `iced_chat.log` exists only in these unit tests. |
| `examples/iced_chat/main.rs:488` | `info!(data_dir = …, "starting iced chat")` | (c) | **SAFE** | Startup log line (present in BORU-CARGO-01 baseline evidence `startup-boru.log`); user-visible but not persisted as a contract. |
| `examples/iced_chat/main.rs:258` | comment `…grow iced_chat.log to tens of megabytes` | (c) | **SAFE** | Comment only. |
| `examples/iced_chat/perf_tracker.rs:1` | `//! Non-invasive performance instrumentation for the iced chat GUI.` | (c) | **SAFE** | Module doc. |
| `tests/test_iced_chat_flow.rs:1,145,157,207,274` | `iced_chat` in test name/docs (`test_iced_chat_exact_flow`, "like iced_chat …") | (b) | **SAFE** | Test name + doc comments describing the flow replica. |
| `tests/verify_gui_bootstrap.rs:36` | `// ── SimChat as used by test_iced_chat_flow.rs ──` | (c) | **SAFE** | Comment. |
| `tests/test_message_transfer.rs:1,132,140,148,159,232`, `test_no_bootstrap.rs:2,76`, `test_full_chat_list_flow.rs:1,138,258`, `test_conversation_integration.rs:545`, `test_two_peers_exchange.rs:132`, `test_onboarding_integration.rs:6,113`, `repro_two_iced_instances.rs:1,141,149` | comments "like iced_chat …", "as in the iced_chat frontend", "iced_chat OpenRoom flow", etc. | (c) | **SAFE** | Doc comments in test files describing which app flow the test mimics. |
| `scripts/boru-test-instance.sh:62` | `# Also kill any bare 'iced_chat' debug binary or other-named variants` | (c) | **SAFE** | Comment; the kill logic targets `boru`/`boru-x86_64-linux` processes, not `iced_chat`. |
| `src/chat_core/friend_ping.rs:962`, `src/ticket_share.rs:20`, `src/backfill.rs:266` | see §4 | (c) | **SAFE** | Doc comments. |

---

## 7. `examples/iced_chat` path references in docs, evidence, and root markdown (c)

These are documentation/evidence references to the source tree path. Renaming the directory makes them stale **but never breaks a build**. The migration (03/05/06) should update the *active* ones (ARCHITECTURE.md, DESIGN_SYSTEM.md, README-adjacent guides, `docs/app-module-map.md`, `BASELINE.md`, `docs/cargo-migration/*`) and may leave historical evidence as-is (they are point-in-time records).

| path | match text (representative) | class | verdict | note |
|---|---|---|---|---|
| `ARCHITECTURE.md:7,13,256,261` | "example GUI application (`examples/iced_chat`)" / ASCII tree "Frontend (iced_chat)" | (c) | **SAFE** | Active architecture doc — update on rename. |
| `BASELINE.md:19-22,29-38,46,78` | "`--example iced_chat`" (dead command note), path lists | (c) | **SAFE** | Baseline record; already documents the rename. |
| `DESIGN_SYSTEM.md:6,25,66,183,479-484,514-515,764,1147` | `examples/iced_chat/{design_tokens,fonts,ui_components,boru_dialog,form_components,status_card}.rs` + `--example boru` | (c) | **SAFE** | Active design-system doc. |
| `docs/app-module-map.md:5,64,87,101,254` | `examples/iced_chat/app.rs` decomposition map | (c) | **SAFE** | Active module map. |
| `docs/gui-architecture.md` (per BORU_BRANDING_AUDIT) | "GUI Architecture — iced_chat", "`iced_chat logs`" | (c) | **SAFE** | Active doc; also has a stale `--example iced_chat` reference worth fixing (see BORU_BRANDING_AUDIT §8). |
| `docs/ui-redesign/current-ui-map.md:44` + evidence READMEs/logs (~140 docs files incl. `docs/ui-redesign/evidence/*` logs, `report.html`) | `--example boru` in build/test logs; `examples/iced_chat/...` path mentions | (c) | **SAFE** | Historical evidence — point-in-time records; do NOT rewrite. |
| `docs/KLIPY-01-gif-system-audit.md:9,15,212`, `CONN-01-status-card-audit.md`, `CONN-12-width-sweep.md`, `DHT_AUDIT.md`, `STUDY.md`, `UI_POLISH_AUDIT_REPORT.md`, `UX_AUDIT.md`, `CATALOGUE_AUDIT.md`, `EPIC-*` reports, `docs/video-download-card/*`, `docs/video-inline-playback/*` | `examples/iced_chat/...` path mentions + `--example boru` commands | (c) | **SAFE** | Audit reports / evidence; historical. |
| `examples/iced_chat/fonts/THIRD_PARTY_NOTICES.md:122` | "bundled at compile time via `include_bytes!` in `examples/iced_chat/fonts.rs`" | (c) | **SAFE** | Licensing notice; path mention only. |
| `docs/BORU_BRANDING_AUDIT.md`, `docs/branding-rename-deliverables.md` | prior branding audit tables (incl. `boru-chat` protocol strings) | (c) | **SAFE** (doc) | Read-only historical audit; its UNSAFE items are re-derived in §9 below. |
| `docs/cargo-migration/01-cargo-audit.md` | §4 launch verification; entry-point facts | (c) | **SAFE** | Parent task output; consumed by this inventory. |

---

## 8. `patched/` vendored crates — OUT OF SCOPE (excluded from migration)

| path | match text | class | verdict | note |
|---|---|---|---|---|
| `patched/iced_aw/Cargo.toml` (25×), `.orig` (17×), `.vscode/launch.json` (25×), `.vscode/tasks.json`, `src/widget/menu/README.md` | `[[example]]` × N, `--example=<widget>` | (a/c) | **EXCLUDED** | Upstream iced_aw demo examples (`badge`, `card`, `menu`, …) belong to the vendored crate's own Cargo.toml — not boru-core targets. Do not touch. |
| `patched/iroh/Cargo.toml` (18×), `.orig` (14×), `README.md:132`, `examples/*.rs` | `[[example]]` + `cargo run --example echo|transfer|listen|search|…` | (a/c) | **EXCLUDED** | Upstream iroh demo examples; `[patch.crates-io]`-vendored. Load-bearing for the build but unrelated to the iced_chat rename. |
| `patched/irpc/Cargo.toml` (5×), `.orig` (4×), `examples/local.rs`, `.github/workflows/ci.yml` | `[[example]]` + `--examples` CI flags | (a/c) | **EXCLUDED** | Upstream irpc demos + upstream CI. |
| `patched/irpc-iroh/Cargo.toml` (5×), `examples/0rtt.rs`, `examples/span_propagation.rs` | `[[example]]` + `--example 0rtt|span_propagation` | (a/c) | **EXCLUDED** | Upstream demos. |
| `patched/iced_aw/src/widget/menu/README.md:376` | `cargo run --example menu …` | (c) | **EXCLUDED** | Upstream doc. |

Note: `patched/` crates are load-bearing for any build (`[patch.crates-io]` in Cargo.toml — see BORU-CARGO-01 §2.5). They are in scope only to confirm they contain **no** `iced_chat` identifiers of boru's; none do.

---

## 9. Runtime / persisted / protocol identifiers — UNSAFE (f)

These must **never** be renamed by the structural migration without a wire/data-compatibility proof. They are the reason steps 03/05/06 must treat "rename `iced_chat`" as a UI-tree rename only, never a protocol rename.

### 9.1 Legacy persisted data-directory name (`boru-chat`)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `src/data_dir.rs:26` | `const LEGACY_DIR_NAME: &str = "boru-chu…"` → `"boru-chat"` | (f) | **UNSAFE** | The legacy data dir on disk (`~/.local/share/boru-chat`, `$XDG_DATA_HOME/boru-chat`, `$PWD/.boru-chat`). `auto_migrate_data_dir()` copies it to `boru` once, never overwrites; `legacy_candidate_dirs()` probes it. Renaming breaks migration for every existing install. Retained because: on-disk persisted data root contract. |
| `src/data_dir.rs:215,234,304,349` | "legacy (`boru-chat`) data directory", migration docs/log lines | (f) | **UNSAFE** | Same persisted dir contract (doc/log mirrors of `LEGACY_DIR_NAME`). |
| `src/data_dir.rs:511,524,561,578,624,634,659,682,844,861` | test paths `.local/share/boru-chat`, `.boru-chat`, `/custom/xdg/boru-chat` | (f) | **UNSAFE** | Tests lock the legacy-dir discovery behaviour. Retained because: migration tests assert the legacy layout. |
| `tests/test_branding_rename.rs:355-388` | asserts `LEGACY_DIR_NAME == "boru-chat"`, legacy candidates contain `boru-chat` | (f) | **UNSAFE** | Retained because: regression tests for the migration contract. |
| `examples/iced_chat/main.rs:478` | comment `// Opportunistically migrate legacy boru-chat data directory to new boru path` | (f) | **UNSAFE** (comment) | Comment documents the persisted-dir migration; keep in sync with `LEGACY_DIR_NAME`. |
| `examples/iced_chat/log_viewer.rs:143` | test `Path::new("/tmp/boru-chat")` | (f) | **UNSAFE** (test) | Test-only path, but mirrors the legacy dir name; only safe to change together with `data_dir.rs` proof. |
| `src/gossip_debug.rs:10,167` | doc + `.join("boru-chat")` (debug log dir) | (f) | **UNSAFE** | Runtime debug-log path under the legacy data dir. Retained because: runtime artifact location. |

### 9.2 Wire / protocol domain separators & namespaces (topic derivations, DHT keys)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `src/directory.rs:31,36,46` | `DIRECTORY_DOMAIN_SEPARATOR = b"boru-chat/public-room-directory/v1"` | (f) | **UNSAFE** | Gossip topic domain for the public-room directory; `TopicId = BLAKE3(sep \|\| relay_url)`. All peers must derive the identical topic — rename breaks directory discovery mesh-wide. |
| `examples/iced_chat/app.rs:7900,7915` | `hasher.update(b"boru-chat/public-room-directory/v1")` | (f) | **UNSAFE** | GUI-side duplicate of the same separator (aligned with `src/directory.rs`; see directory-topic-alignment). Must move in lockstep with §9.2 row above or the GUI derives a different topic than the library. |
| `src/discovery_backend.rs:21` | `PUBLIC_LOBBY_KEY_DOMAIN = b"boru-chat/public-lobby/v1"` | (f) | **UNSAFE** | Public-lobby DHT key domain; wire contract. |
| `src/public_room.rs:39,41,44` | `DISCOVERY_KEY_DOMAIN_SEPARATOR = b"boru-chat discovery-key v1"`; `APPLICATION_NAMESPACE = "boru-chat"` | (f) | **UNSAFE** | Public-room DHT discovery namespace + ticket namespace; wire contract. |
| `src/topic_derivation.rs:16,72,93,196,204,275` | `PUBLIC_ROOM_DOMAIN_SEPARATOR = b"boru-chat public-room v1"`; `TRACKER_NAMESPACE_DOMAIN_SEPARATOR = b"boru-chat room discovery v1"` (+ doc comments, pre-computed topic hashes in tests) | (f) | **UNSAFE** | Room-topic and tracker-namespace derivation inputs; changing re-derives every room topic and tracker key. |
| `src/private_room_tracker.rs:78` | `PRIVATE_ROOM_DOMAIN_SEPARATOR = b"boru-chat private-room v1"` | (f) | **UNSAFE** | Private-room tracker namespace. |
| `src/discovery_secret.rs:52-54,78,83,88` | `SUBKEY_{NAMESPACE,ENCRYPTION,SIGNING}_DOMAIN = b"boru-chat private-room v2 {namespace,encryption,signing}"` | (f) | **UNSAFE** | HKDF subkey domains for private-room encryption keys. Renaming would make existing rooms undecryptable. |
| `src/short_code.rs:63,172` | `SHORTCODE_DOMAIN_SEPARATOR = b"boru-chat/short-code/v1"` | (f) | **UNSAFE** | Short-code (pairing/join code) topic derivation. |
| `src/spake2_pairing.rs:61` | `SPAKE2_CONTEXT = b"boru-chat short-code pairing v1"` | (f) | **UNSAFE** | SPAKE2 pairing context string; a rename breaks pairing between old and new clients. |
| `src/storage.rs:1879` | `b"boru-chat/dm/request/v1"` | (f) | **UNSAFE** | Storage key for DM-request records (persisted in SQLite/kv_store). Retained because: DB key contract. |
| `src/proto/state.rs:1` | `//! The protocol state of the \`boru-chat\` protocol.` | (f) | **UNSAFE** (doc) | Module doc naming the protocol; keep consistent with the wire strings. |
| `tests/test_branding_rename.rs:178,187,196,205,233` | asserts wire constants equal `boru-chat public-room v1`, `boru-chat room discovery v1`, `boru-chat discovery-key v1`, `boru-chat`, `boru-chat private-room v1` | (f) | **UNSAFE** | Regression tests that pin the protocol strings; renaming the constants fails these tests (which is the point). |
| `src/peer_invitation.rs:1,10,13,73,75,78,80,132,204,210` | `URI_PREFIX = "boru-chat://pair/"` + docs | (f) | **UNSAFE** | Peer-invitation URI scheme. Retained because: QR codes and copied invitation URIs are persisted/transmitted artifacts; old clients must still parse them. |
| `src/qr.rs:28,78`, `src/spake2_pairing.rs:8` | `boru-chat://pair/...` URI references | (f) | **UNSAFE** | QR/SPAKE2 docs mirroring the URI scheme. |
| `src/peer_invitation.rs:670-671` | test relay host `relay1.boru-chat.example.com:443` | (f) | **UNSAFE** (test) | Test fixture hostname in the invitation URI format; harmless but part of the scheme's test contract. |
| `src/public_room.rs:266`, `src/topic_derivation.rs:196,275` | doc `printf 'boru-chat … v1\0…' \| b3sum` example commands | (f) | **UNSAFE** (doc) | Show how to reproduce topic hashes from the separator bytes; must match the constants. |

### 9.3 Wire-visible node name & topic bytes in the GUI

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `examples/iced_chat/app.rs:11967` | `name: Some("boru-chat".to_string())` | (f) | **UNSAFE** | Gossip node display name broadcast to peers in presence/AboutMe messages. Renaming changes what every peer shows for this node; interop-visible. Retained because: wire-visible identity string (was kept during the branding rename — BORU_BRANDING_AUDIT §8 row 75/76). |
| `examples/iced_chat/app/discover.rs:1821` | `name: Some("boru-chat".to_string())` | (f) | **UNSAFE** | Same node-name string in the discover flow. |
| `tests/test_image_iced_gui_flow.rs:175,223` | `name: Some("boru-chat".to_string())` | (f) | **UNSAFE** | Tests mirror the node-name wire string. |
| `examples/iced_chat/app.rs:23457` | test temp dir `boru-iced-chat-join-request-{suffix}` | (f) | **UNSAFE** (test) | Test-created temp data dir in `/tmp`; safe to rename only together with the app's own naming (it is ephemeral, but the join-request flow's test contract references it). |
| `examples/iced_chat/app.rs:23671` | test temp dir `boru-iced-chat-prewarm-{suffix}` | (f) | **UNSAFE** (test) | Same reasoning; ephemeral test temp dir. |

### 9.4 Legacy env-var contracts (persisted config interface — from BORU-CARGO-01 §5.2)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `src/data_dir.rs` (env chain) | `BORU_CHAT_DATA_DIR` (deprecated legacy override) | (f) | **UNSAFE** | Environment contract honoured for existing deployments/scripts. Priority: `--data-dir` flag > `BORU_DATA_DIR` (new) > `BORU_CHAT_DATA_DIR` (deprecated) > legacy auto-detect > defaults (verified `resolve_data_dir`, `data_dir.rs:60-96`). Retained because: existing launchers set it and it must keep working as the legacy fallback. |
| `examples/iced_chat/app.rs:7196` + `docs/configuration.md` | `BORU_CHAT_FILES_DIR` (legacy files-root override) | (f) | **UNSAFE** | Existing configs/users rely on it; renaming would silently move image-store roots. |
| `examples/iced_chat/main.rs:103,152`, `log_viewer.rs:89,144`, `mcp_server.rs:5171,5192` | `BORU_CHAT_DATA_DIR` env passthrough/deprecation warning | (f) | **UNSAFE** | Same legacy env contract; warning text may be reworded but the variable must keep working. |

### 9.5 Other persisted runtime identifiers (already migrated; keep)

| path:line | match text | class | verdict | note |
|---|---|---|---|---|
| `examples/iced_chat/main.rs` (init_logging) | `<data_dir>/logs/boru.log`; `instance.log`; `crash_reports/crash-*.txt` | (f) | **UNSAFE** (keep current names) | Current runtime file names — do not rename during migration (log tailers/ops tooling may reference them). |
| `src/data_dir.rs` (resolve chain) | `$XDG_DATA_HOME/boru`, `$HOME/.local/share/boru`, `$LOCALAPPDATA\boru`, `$PWD/.boru` | (f) | **UNSAFE** (keep current names) | Current data-dir root names; they are the NEW contract and must stay. |

---

## 10. Runtime asset and launch contracts that must survive the target restructure (from BORU-CARGO-01 §5)

These are not `iced_chat` matches themselves but are listed because the target restructure (03/05/06) must preserve them (per the PDF guardrails):

- `[patch.crates-io]` `patched/` paths — load-bearing for every build.
- Papirus icon runtime probe order: `BORU_PAPIRUS_ASSETS` env → exe-relative `assets/third_party/papirus` → `../assets/…` → cwd → baked `CARGO_MANIFEST_DIR` (non-fatal fallback).
- Data-dir env chain and migration (§9.1, §9.4).
- `required-features=["gui"]` gating on the GUI target.
- `build.rs` `GIT_HASH` / `BORU_APP_VERSION` env + `rerun-if-changed` behaviour.

---

## 11. Verdict summary for downstream steps (03/05/06/10)

**SAFE to rename** (developer-facing scaffolding; update in lockstep):
- `examples/iced_chat/` directory → new location/name (e.g. `src/bin/` or `examples/boru/`), with §2/§4 reference updates.
- `.cargo/config.toml` alias `iced-chat` (and its comments) → drop or rename; keep `boru`.
- Clap test harness argv[0] labels (`"iced_chat"` in `main.rs` tests).
- Test-local `iced_chat.log` filenames in `main.rs` unit tests.
- Startup log line "starting iced chat" → "starting boru" (cosmetic; note the BORU-CARGO-01 baseline evidence log records the old text — historical).
- `iced_chat` mentions in test names / comments / docs (active docs: ARCHITECTURE, DESIGN_SYSTEM, app-module-map, gui-architecture, README-adjacent; historical evidence may stay).
- `scripts/boru-test-instance.sh:62` comment.

**UNSAFE — must never be renamed without proof** (§9):
- Legacy data-dir name `boru-chat` + migration logic + its tests (`src/data_dir.rs`, `test_branding_rename.rs`, `log_viewer.rs:143`, `gossip_debug.rs`).
- All wire/protocol domain separators and namespaces containing `boru-chat` (directory, lobby, public-room, private-room, discovery-key, short-code, SPAKE2, HKDF subkeys, dm/request storage key, invitation URI scheme `boru-chat://pair/`).
- Wire-visible node display name `boru-chat` (app.rs, discover.rs, mirror tests).
- Legacy env vars `BORU_CHAT_DATA_DIR`, `BORU_CHAT_FILES_DIR`.
- Current runtime file names `boru.log` / `instance.log` / `crash_reports/` and current data-dir roots `boru` / `.boru`.

**KEEP (out of scope / not legacy):**
- The genuine demo examples (`catalogue_browser`, `dht_harness`, `doctor`, `setup`, `test_addr`, `video_backend_probe`) and every `--example <demo>` invocation.
- `patched/` vendored crates' own `[[example]]` tables and `--example` docs.
- All `--example boru` invocations (CI, scripts, justfile, docs) — they already use the current name; they change only if a `[[bin]] boru` replaces the example (BORU-CARGO-03 decision).

---

## 12. Verification (this task)

- [x] All six search patterns run repo-wide at origin/main @ 119b633d (counts in §0).
- [x] Every `iced_chat` / `iced-chat` / "Iced Chat" / "iced chat" match classified (a)-(f); tables cover all non-`patched/` reference sites; `patched/` classified EXCLUDED.
- [x] `--example` / `[[example]]` usage classified: genuine demos KEEP, `--example boru` SAFE, `patched/` EXCLUDED.
- [x] Persisted/runtime/protocol identifiers marked UNSAFE with "retained because…" notes (§9).
- [x] No files other than this inventory were added/modified (`git status` clean apart from this doc).
