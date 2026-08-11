# BORU-CARGO-10 — Final cleanup + end-state verification + report

**Task:** t_dd439d4e (BORU-CARGO-10, step 10 — final step of the Boru Cargo target migration)
**Date:** 2026-08-12
**Tree audited:** origin/main @ `62c4d6f9` (BORU-CARGO-09) merged into worktree `wt/t_dd439d4e`; this task's commit sits on top.
**Build host:** debsrv (172.16.0.59) via `rb` wrapper (slot 3, sccache-warmed).
**Outcome:** **ALL acceptance criteria PASS.** Four stale active docs corrected; every remaining `iced_chat` / `--example` match classified (legitimate path reference, historical record, genuine demo example, or excluded vendored crate). Binary confirmed named `boru`; end-state commands verified green; report committed.

---

## 1. What changed in this step (BORU-CARGO-10)

Repo-wide re-search for `iced_chat` and `--example` (git grep, tracked files, excluding `target/`, `captures/`, `.git/`) and cleanup of stale "Boru is an Iced example" documentation.

### 1.1 Files edited (4 active docs — stale "example"/old-name wording removed)

| File | Change | Why |
|---|---|---|
| `docs/configuration.md:8` | `### iced_chat (GUI)` → `### boru (GUI)` | Active config doc; section header named the app by its legacy module name. |
| `docs/notification-system/01-audit-and-plan.md:145` | `The GUI (\`iced_chat\` example) targets:` → `The GUI (\`boru\` binary) targets:` | Active audit/plan doc; called the GUI a cargo example. |
| `docs/storage-unification-audit.md:92` | `### Startup sequence (iced_chat example)` → `### Startup sequence (boru GUI)` | Active audit doc; called the GUI a cargo example. |
| `docs/video-download-card/VIDCARD-01-architecture.md:14` | `Boru example entry` + `Cargo.toml:227-235 ([[example]] name = "boru"...)` → `Boru binary entry` + `Cargo.toml:344-347 ([[bin]] name = "boru"...; gui at :291, video-playback at :292, terminal at :293)` | Active architecture doc; referenced the removed `[[example]] boru` and stale Cargo.toml line numbers. |

No source code, Cargo.toml, script, CI, or runtime identifier was changed in this step. No persisted/protocol identifier was renamed (guardrail honoured).

### 1.2 Final grep counts (origin/main + this task's 4 doc edits)

| Pattern | Count (all files) | Notes |
|---|---|---|
| `iced_chat` (all) | 2,495 matches in 149 files | — |
| `iced_chat` *not* containing `examples/iced_chat` path | 146 files | classified below |
| `--example` (all) | 163 matches in 46 files | classified below |
| `--example iced_chat` in active (non-historical) docs | **0** | old launch command fully gone from live docs |
| `--example boru` in active (non-historical) docs | **0** | example-based GUI launch fully gone from live docs |

Full grep output: appendix A (this file §A.1–§A.3).

---

## 2. Classification of every remaining `iced_chat` match

Per the BORU-CARGO-02 inventory (`docs/cargo-migration/02-legacy-iced-chat-inventory.md`), the classification legend is (a) cargo target name, (b) source/module name, (c) documentation/script, (d) test/CI command, (e) packaging command, (f) runtime/persisted identifier.

### 2.1 Retained — legitimate path references to the source tree (the large majority)

The GUI source tree deliberately REMAINS at `examples/iced_chat/` (BORU-CARGO-03/05 decision; the `[[bin]] boru` points at `examples/iced_chat/main.rs`). Every reference below names a **real, current path**:

- `Cargo.toml` — `path = "examples/iced_chat/main.rs"` for `[[bin]] boru` (the only Cargo declaration).
- `tests/fs17_activity_log.rs`, `tests/fs22_dashboard_coverage.rs` — `#[path = "../examples/iced_chat/..."]` compile-time module bindings.
- `tests/protocol_registration.rs:72` — `include_str!("../examples/iced_chat/main.rs")` — this is why the path must stay; the test passes (see §4).
- `examples/iced_chat/app.rs` test fixtures — `env!("CARGO_MANIFEST_DIR")`-relative self-path assertions.
- `src/backfill.rs:266`, `src/chat_core/friend_ping.rs:962`, `src/ticket_share.rs:20` — doc/comment references to the real file path.
- `scripts/package-windows.sh:9` — comment referencing the real loader file.
- ~140 docs (ARCHITECTURE, DESIGN_SYSTEM, STUDY, discovery-refactor maps, fs-* handoffs, file-type-icons, UI audit reports, etc.) — all name the real source path.

**Verdict: KEEP — these are accurate references to the current code tree, not stale claims that Boru is an example.**

### 2.2 Retained — historical / point-in-time records (BORU-CARGO-02 §7: "may stay")

- `docs/cargo-migration/01-cargo-audit.md`, `02-legacy-iced-chat-inventory.md`, `07-bin-asset-paths.md`, `08-regression-results.md` — the migration's own record; they deliberately document the old command as BROKEN.
- `docs/ui-redesign/**` (baseline-*.log, evidence/*.log, current-ui-map.md, UI-* reports) — build/test evidence logs at their point in time.
- `BASELINE.md`, `docs/fs-00-baseline.md`, `docs/video-inline-playback/step1-baseline.md` — baseline snapshots documenting the pre-rename state (`--example iced_chat` as the dead command).
- `docs/BORU_BRANDING_AUDIT.md`, `docs/branding-rename-deliverables.md`, `CATALOGUE_AUDIT.md`, `DHT_AUDIT.md`, `UX_AUDIT.md`, `UI_POLISH_AUDIT_REPORT.md`, EPIC-* reports, CONN-*, KLIPY-*, PAPIRUS-*, etc. — audit reports recording what was found at audit time.
- `docs/performance/phase25-final-report.md`, `docs/chat-ui-regression-report.md`, etc. — task close-out reports.

**Verdict: KEEP — point-in-time records; rewriting them would falsify history (inventory §7 explicitly permits leaving them).**

### 2.3 Retained — test names / comments describing the flow the test replicates

- `tests/test_iced_chat_flow.rs` — file name + `test_iced_chat_exact_flow` + comments ("Exact replica of iced_chat message flow", "like iced_chat JoinFromTicket", "exact iced_chat SendPressed path").
- `tests/repro_two_iced_instances.rs`, `test_message_transfer.rs`, `test_no_bootstrap.rs`, `test_full_chat_list_flow.rs`, `test_conversation_integration.rs`, `test_two_peers_exchange.rs`, `verify_gui_bootstrap.rs`, `test_onboarding_integration.rs` — comments "like iced_chat", "as in the iced_chat frontend", "iced_chat OpenRoom flow", etc.
- `src/chat_core/friend_ping.rs:962` — "Both chat-gui.rs and iced_chat/main.rs do:".

**Verdict: KEEP — these describe which application flow the test mimics; the frontend they reference still lives at `examples/iced_chat/`. Renaming them is out of scope for a structural cleanup (inventory §6 classified them SAFE but they were deliberately not renamed; they carry no runtime/persisted meaning).**

### 2.4 Retained — the startup log line `starting iced chat`

`examples/iced_chat/main.rs:488` — `info!(data_dir = ..., "starting iced chat")`. Inventory §6 classified this SAFE-to-rename (cosmetic), but the CARGO-03–09 steps chose not to change it and the baseline evidence logs record the old text. Per the guardrail "preserve logging", and because it is a log line, not a doc/comment, it is **retained** and documented here as an intentional retention.

**Verdict: KEEP (intentional retention — cosmetic runtime log string; changing it would churn baseline evidence for zero behavioural gain).**

### 2.5 Retained — `--example` references that are genuine demo examples

- `[[example]] setup / video_backend_probe / doctor / dht_harness / test_addr / catalogue_browser` (Cargo.toml) — genuine demo harnesses, all `required-features`-gated.
- `.cargo/config.toml` alias `chat = "run --features examples --example chat --"` — demo TUI example alias.
- `docs/build-release.md` (`--example doctor`, `--example setup`), `docs/inline-video-*` (`--example video_backend_probe`), `examples/{catalogue_browser,dht_harness,doctor,video_backend_probe}.rs` doc comments — usage docs for the genuine demos.
- `docs/video-inline-playback/step1-baseline.md` — historical record that `--example iced_chat` fails.

**Verdict: KEEP — these name real example targets, not Boru.**

### 2.6 Excluded — `patched/` vendored crates

All `[[example]]` tables and `--example` invocations inside `patched/` (iroh, iroh-dns, irpc, iced_aw, etc.) belong to vendored upstream crates, not boru-core targets (inventory §8). Confirmed none contain boru's `iced_chat` identifiers.

**Verdict: EXCLUDED — upstream vendored code, out of scope.**

---

## 3. Old vs new Cargo target structure

| Aspect | Before migration (BORU-CARGO-01 baseline @ `119b633d`) | After migration (this task @ `62c4d6f9`+) |
|---|---|---|
| GUI target | `[[example]] name = "boru"` (path `examples/iced_chat/main.rs`), plus a stray auto-discovered `iced_chat` example | `[[bin]] name = "boru"` (same path), `autoexamples = false`, `default-run = "boru"` |
| Launch command | `cargo run --example boru --features gui` (old PDF command `cargo run --example iced_chat` already broken) | plain `cargo run` (default features include `gui`) |
| Demo examples | auto-discovered | explicitly declared (`setup`, `video_backend_probe`, `doctor`, `dht_harness`, `test_addr`, `catalogue_browser`) |
| `sim` | `[[bin]] sim` | unchanged (`cargo run --bin sim --features simulator`) |
| CI/scripts/justfile | `--example boru` invocations | `--bin boru` / plain `cargo run` (BORU-CARGO-06) |
| Cargo alias | `iced-chat = "run --features gui --example boru --"` | removed; `boru = "run --features gui --"` |
| Profile | — | release `strip = true, lto = "fat", codegen-units = 1` (unchanged, applies to the bin) |

Per-step records: `01-cargo-audit.md` (baseline), `02-legacy-iced-chat-inventory.md` (classification), `07-bin-asset-paths.md` (asset/path verification), `08-regression-results.md` (build+behaviour gate), `09-smoke-results.md` (behaviour smoke on debsrv), this report (final).

---

## 4. End-state verification results (all on debsrv via `rb`, slot 3)

| # | Command | Result | Detail |
|---|---|---|---|
| 1 | `cargo build` (`rb build`) | **PASS** (exit 0, 14.58s incremental) | 259 pre-existing warnings (identical count to CARGO-01 baseline; unfulfilled `#[expect(dead_code)]` lints) |
| 2 | `cargo build --release --bin boru` (`rb build --release --bin boru`) | **PASS** (exit 0, 7m55s) | 259 pre-existing warnings; release profile strip+lto applied |
| 3 | Executable name | **`boru` confirmed** | `~/boru-build/work-target-3/release/boru` (52,507,256 bytes stripped, ELF x86-64 pie) and `~/boru-build/work-target-3/debug/boru` (1,229,140,088 bytes) |
| 4 | `cargo metadata` target structure | **PASS** | `boru ['bin'] /home/dan/boru-build/work-3/examples/iced_chat/main.rs`; **no `iced_chat` target exists**; demo examples `['example']`; `sim ['bin']` |
| 5 | `cargo test --no-run` (`rb test --no-run`) | **PASS** (exit 0) | all default-feature test targets compile (lib + bin + ~100 integration/bench targets) |
| 6 | `cargo test --lib` (`rb test --lib`) | **PASS** | **2249 passed; 0 failed; 2 ignored** (355.39s) — identical to CARGO-08 |
| 7 | `cargo test --bin boru` (`rb test --bin boru`) | **5 pre-existing failures** (1217 passed / 5 failed) | see §4.1 — same 5 source-audit failures documented in CARGO-08, 0-line diff across the migration |
| 8 | Integration suite subset | **ALL PASS** | `protocol_registration` (2) — include_str!s main.rs (bin-path regression proof); `test_branding_rename` (28) — pins wire/persisted identifiers; `security` (58); `test_serde_format` (1); `test_hostile_input` (41); `test_storage_integration` (15) |
| 9 | Full integration gate | **72 PASS / 12 FAIL — all pre-existing** | CARGO-08 (`08-regression-results.md`) ran all 84 default-feature suites on debsrv; 11 hang on the debsrv IPv6 relay (`RelayMode::Default` + `online().await`), 1 flaky (`test_iced_chat_flow`), plus documented `test_onboarding_integration` assertion failures. All 0-line diff across the migration range. This task's tree differs from CARGO-08's only in 4 doc files → re-running all 84 adds nothing. |
| 10 | `cargo run` headless (established pattern) | **PASS** | literal `cargo run` under xvfb-run on debsrv launched the `boru` bin; then deployed to `/home/dan/boru` and `scripts/start_boru_headless.sh` run: **MCP ready after 18s**, window rendered (screenshot), 2 LAN peers direct-connected (47974d77, 754d5785), lobby+directory topics subscribed, `RoomOpened FIRED`, zero panics / zero missing-file errors. Evidence in `evidence/t10-final/`. |
| 11 | Old launch command | **DEAD as required** | `cargo run --example iced_chat` → exit 101: `error: no example target named 'iced_chat' in default-run packages` |

### 4.1 The 5 pre-existing bin-test failures (identical to CARGO-08, all pre-migration)

```
app::tests::conn_refresh_no_block_on_in_update_path   (block_on audit; broken by July block_on changes, pre-migration)
app::tests::home_screen_fonts05_approved_family_mapping
app::tests::home_screen_spacing_uses_the_shared_scale
app::tests::home_screen_uses_type_role_roles          (home.rs/design_tokens.rs audits; broken by BORU-HOME-02 / POLISH-05, pre-migration)
design_tokens::tests::contrast_ratios_pass_wcag_aa
```

Proof of pre-existing: CARGO-08 (`08-regression-results.md` §2) verified `home.rs`/`design_tokens.rs` are 0-line diff across `119b633d^..bc4ddfbd`; these tests fail identically at the pre-migration baseline. No migration step touched them.

### 4.2 Unrelated pre-existing warnings/failures (carried, not migration-caused)

- 259 build warnings: unfulfilled `#[expect(dead_code)]` lints — identical count since BORU-CARGO-01.
- `cargo fmt --check`: fails (1477 hunks) — rustfmt 1.9.0 config drift, byte-identical at baseline (CARGO-08 §1.11).
- `cargo clippy --all-targets`: fails on `tests/gen_stress_data.rs` only (E0063 missing `ConversationEntry`/`HistoryEntry` fields) — stale fixture, pre-migration (CARGO-08 §1.12).
- Headless network noise: mDNS/relay/DHT bootstrap WARNs (same as t01/t07/t09 baselines).

---

## 5. Acceptance criteria checklist

- [x] **Remaining legacy matches are either zero or intentionally documented.** Zero stale `--example iced_chat` / `--example boru` in active docs; every remaining `iced_chat` match classified in §2 (legitimate path / historical / test-name / log-string) or excluded (patched/).
- [x] **Plain `cargo run` launches Boru** — verified headless on debsrv (§4 #10).
- [x] **Normal binary target named `boru`** — `target/release/boru` and `target/debug/boru` exist; cargo metadata shows `boru ['bin']`; no `iced_chat` target.
- [x] **Debug + release builds succeed** — §4 #1/#2.
- [x] **`cargo test` passes (or pre-existing failures clearly identified)** — lib 2249/0; bin 1217/5 all pre-existing and documented; integration subset green; full gate 72/12 with documented pre-existing causes.
- [x] **Old application launch command (`cargo run --example iced_chat`) no longer required** — exits 101 with "no example target named `iced_chat`" (§4 #11).
- [x] **No accidental renaming of persisted/protocol identifiers** — zero source/protocol/storage changes in this step; `test_branding_rename` (28 assertions pinning wire constants) passes (§4 #8).

---

## 6. Files changed in BORU-CARGO-10

- `docs/configuration.md`
- `docs/notification-system/01-audit-and-plan.md`
- `docs/storage-unification-audit.md`
- `docs/video-download-card/VIDCARD-01-architecture.md`
- `docs/cargo-migration/10-final-report.md` (this report)
- `docs/cargo-migration/evidence/t10-final/run1-home.png` (headless launch screenshot)

---

## Appendix A — Final grep output

### A.1 `--example iced_chat` — all matches (historical records only; 0 active)

```
BASELINE.md:19
docs/cargo-migration/01-cargo-audit.md:7,26,27,131,310
docs/cargo-migration/02-legacy-iced-chat-inventory.md:57,101,131,134
docs/video-inline-playback/step1-baseline.md:33,36
```

### A.2 `--example boru` — all matches (historical records only; 0 active)

```
BASELINE.md:16,21,22,60
docs/cargo-migration/01-cargo-audit.md:16,132,134,135,136,137,138,139,146,265,311,314
docs/cargo-migration/02-legacy-iced-chat-inventory.md:54,66,79,91,92,93,94,95,96,99,100,101,102,132,135,136,254,262
docs/ui-redesign/baseline-launch.log:4
docs/ui-redesign/baseline-test-list.log:1843
docs/ui-redesign/evidence/t_8834836b/gate_build_test.log:1,1650
docs/ui-redesign/evidence/ui-18/verification.json:49,50
docs/video-inline-playback/step1-baseline.md:33,36
```

### A.3 `--example <demo>` — all matches (genuine demo examples + patched/ upstream)

```
Cargo.toml ([[example]] setup/video_backend_probe/doctor/dht_harness/test_addr/catalogue_browser declarations)
.cargo/config.toml (chat alias)
docs/build-release.md:91,94 (doctor, setup)
docs/inline-video-backend.md:10,85 (video_backend_probe)
docs/inline-video-rollout.md:30 (video_backend_probe)
docs/inline-video-test-matrix.md:39,70 (video_backend_probe)
examples/catalogue_browser.rs, dht_harness.rs, doctor.rs, video_backend_probe.rs (own usage docs)
patched/iced_aw, patched/iroh, patched/irpc, patched/irpc-iroh (upstream vendored examples — EXCLUDED)
```

### A.4 `iced_chat` non-path matches by category (files)

- **Historical/evidence (KEEP, §2.2)**: `docs/ui-redesign/**` (evidence logs, baseline logs, current-ui-map, UI-* reports), `docs/cargo-migration/01,02,07,08`, `BASELINE.md`, `docs/fs-00-baseline.md`, `docs/video-inline-playback/step1-baseline.md`, `docs/BORU_BRANDING_AUDIT.md`, `docs/branding-rename-deliverables.md`, `CATALOGUE_AUDIT.md`, `DHT_AUDIT.md`, `UX_AUDIT.md`, `UI_POLISH_AUDIT_REPORT.md`, `EPIC-*`, `CONN-*`, `KLIPY-*`, `PAPIRUS-*`, `STUDY.md`, `DESIGN_SYSTEM.md`, `docs/chat-ui-regression-report.md`, `docs/onboarding-and-pairing-design.md`, `docs/networking-audit.md`, `docs/performance/*`, `docs/storage-*`, `docs/testing.md`, `docs/video-download-card/*`, `docs/inline-video-*`, `docs/discovery-*`, `docs/empty-state-spec.md`, `docs/plans/*`, `docs/secure-tunnels-design.md`, `docs/app-module-map.md`, `docs/gui-architecture.md`, `docs/ui-*`, `docs/fs-*`, `docs/file-type-icons/*`, `docs/design/*`, `docs/error-state-taxonomy.md`, `docs/gif-search.md`, `docs/offline-direct-messaging.md`, `docs/reliability-implementation-status.md`, `docs/screenshots/dashboard-mockup.html`, `docs/telepathy-*`, `docs/chat-interface-design-tokens.md`, `docs/chat-ui-redesign-baseline.md`, `docs/compatibility-identifiers.md`, `AUTO_HIDE.md`, `CONDITIONAL_FEATURES.md`, `CONVERSATION_HISTORY_DIAGNOSIS.md`, `VOICE_ACCEPTANCE.md`, `report.html`
- **Test names/comments (KEEP, §2.3)**: `tests/test_iced_chat_flow.rs`, `tests/repro_two_iced_instances.rs`, `tests/test_message_transfer.rs`, `tests/test_no_bootstrap.rs`, `tests/test_full_chat_list_flow.rs`, `tests/test_conversation_integration.rs`, `tests/test_two_peers_exchange.rs`, `tests/test_onboarding_integration.rs`, `tests/verify_gui_bootstrap.rs`
- **Source comment (KEEP, §2.3)**: `src/chat_core/friend_ping.rs:962`
- **Path references (KEEP, §2.1)**: `Cargo.toml`, `src/backfill.rs`, `src/ticket_share.rs`, `scripts/package-windows.sh`, `tests/fs17_activity_log.rs`, `tests/fs22_dashboard_coverage.rs`, `tests/protocol_registration.rs`, and ~140 docs naming `examples/iced_chat/...`
- **Startup log string (KEEP, §2.4)**: `examples/iced_chat/main.rs:488`
