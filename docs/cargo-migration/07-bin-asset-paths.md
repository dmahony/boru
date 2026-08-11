# BORU-CARGO-07 — Asset + path behaviour under the bin target (verification)

**Task:** t_9baf1e93 (BORU-CARGO-07, step 7 of the Boru Cargo target migration)
**Date:** 2026-08-12
**Commit audited:** `deb268d3` (origin/main @ BORU-CARGO-06), worktree `wt/t_9baf1e93` fast-forwarded to origin/main before verifying.
**Outcome:** **Verification-only — no code fixes required.** Every compile-time and runtime asset/path resolves identically under the `[[bin]] boru` target (`examples/iced_chat/main.rs`). No path fix was needed; the evidence below is committed for the regression gate (BORU-CARGO-08).

---

## 1. Compile-time asset includes (include_str!/include_bytes!/env!/concat!)

BORU-CARGO-03/05 kept the bin at `examples/iced_chat/main.rs`, so every `include_*` path is still resolved relative to its **source file** — the same path as under the legacy example layout. All referenced files exist (verified with `ls`/`find`); the `rb build --bin boru --features gui` success in §4 is the definitive proof.

### App module tree (`examples/iced_chat/`)

| File | Macro | Target | Resolves relative to source file |
|---|---|---|---|
| `fonts.rs:40-75` | `include_bytes!` | `fonts/Figtree-*.ttf`, `Raleway-ExtraBold.ttf`, `JetBrainsMono-*.ttf`, `InterTight-Bold.ttf`, `PublicSans-*.ttf` | `examples/iced_chat/fonts/` — all 12 referenced files present |
| `icon_system.rs:42-67` | `include_bytes!` | `../../assets/icons/lucide/*.svg` (27 icons) | repo `assets/icons/lucide/` — all present |
| `app.rs:925-951` | `include_bytes!` | `../../assets/icons/lucide/*.svg` (23 icons) | repo `assets/icons/lucide/` — all present |
| `file_type_icon.rs:690-696` | `include_str!`/`include_bytes!` | `../../assets/third_party/papirus/manifest.json`, `.../32/application-x-generic.svg` | repo `assets/third_party/papirus/` — present |
| `file_type_resolver.rs:102` | `include_str!` | `../../assets/third_party/papirus/manifest.json` | present |
| `terminal_view.rs:20` | `include_str!` | `motd.txt` | `examples/iced_chat/motd.txt` — present |
| Self-includes (test modules) | `include_str!` | `app.rs`, `app/*.rs`, `card_shell.rs`, `quick_actions.rs`, `video_file_card.rs`, `ui_components.rs`, `download_progress_view.rs`, `focusable_button.rs`, `status_card.rs`, `sharing_summary.rs`, `boru_dialog.rs`, `form_components.rs`, `log_viewer.rs`, `connection_details.rs` | all same-dir — present |
| `offscreen_status_card.rs` (test) | `include_bytes!` | `fonts/*.ttf` | present |
| `app.rs:29733-29742` (test) | `include_bytes!` | `fonts/*.ttf` | present |

### Library tree (`src/`)

| File | Macro | Target | Status |
|---|---|---|---|
| `src/lib.rs:1` | `include_str!` | `../README.md` | repo-root README — present |

### env!/concat! (all `CARGO_MANIFEST_DIR`-anchored, compile-time, stable under crate layout)

- `file_type_icon.rs` test module (lines 581, 1177, 1274, 1519, 1647-1782) — `env!("CARGO_MANIFEST_DIR")` in unit tests; anchors to the crate root, independent of bin-vs-example.
- `app.rs` test module (24392, 24594, 24685) — same pattern.
- `app.rs:533-534` — `option_env!("BORU_APP_VERSION")` / `option_env!("GIT_HASH")`, both set by `build.rs` (`CARGO_PKG_VERSION` / `git rev-parse`). Unchanged.

**No cwd-relative compile-time include exists in the app module tree.**

## 2. Runtime resources

### Fonts
Bundled at compile time (`include_bytes!` → `iced::font` load at startup) — zero runtime path dependence. Rendered screenshot (§5) shows crisp bundled-font text (Figtree/Public Sans/Inter Tight); no missing-glyph boxes, no font errors in the startup log.

### Icons (lucide)
Bundled at compile time — rendered in the screenshot (sidebar nav, status card, toolbar).

### Papirus file-type icons (runtime probe — the ONLY cwd/exe-sensitive runtime asset, pre-existing)
`file_type_icon.rs::papirus_asset_root()` probes: `BORU_PAPIRUS_ASSETS` env → `<exe_dir>/assets/third_party/papirus` → `<exe_dir>/../assets/third_party/papirus` → `<cwd>/assets/third_party/papirus` → `CARGO_MANIFEST_DIR/assets/third_party/papirus`. This probe was unchanged by the migration (BORU-CARGO-01 §5.4 documented it); the bin target only changes `<exe_dir>` from `target/debug/examples/` to `target/debug/`, which the release package layout (assets next to exe) and the dev-tree `CARGO_MANIFEST_DIR` candidate both cover. Failure is non-fatal (embedded generic icon + one-time WARN). **No change made** — the probe order is the documented runtime contract and the packaging scripts ship the bundle next to the exe.

### Splash window
`main.rs:647-652` looks for `splash.py` next to the exe, else a baked dev-machine absolute fallback (`/home/dan/iroh-gossip-chat/scripts/splash.py`). Pre-existing; absent → splash silently skipped; no cwd dependence. **No change made.**

## 3. Data-directory resolution (round-trip)

`src/data_dir.rs` is untouched by the migration — priority chain unchanged: `--data-dir` > `BORU_DATA_DIR` > `BORU_CHAT_DATA_DIR` (deprecated) > legacy auto-detect > `$XDG_DATA_HOME/boru` > `$PWD/.boru` (pre-existing fallback, deliberately preserved).

**Round-trip evidence** (fresh binary, same `--data-dir /tmp/boru_cargo07` across two runs):

| Observation | Run 1 (fresh) | Run 2 (same dir) |
|---|---|---|
| public key | `a1ad579e76e92bacd55972cb9bfeff31a7c0a733d67d7132d4cf1c9e2e4bebc7` | **same** `a1ad579e…` |
| identity file | `/tmp/boru_cargo07/secret_key.txt` (created) | `/tmp/boru_cargo07/secret_key.txt` (read; mtime unchanged) |
| storage | `boru.db` migrations 0→19 | `boru.db` opened; **no re-migration** |
| UI screenshot | rendered home screen | rendered home screen (pixel-identical within 0.94% — only activity-feed timestamps differ) |

The app finds and reads existing local data/config identically to pre-migration behaviour.

## 4. Build verification

```
rb build --bin boru --features gui        # debsrv slot 2
Finished `dev` profile [unoptimized + debuginfo] target(s) in 56.90s
exit 0 — 259 warnings (identical count to BORU-CARGO-01 baseline; all pre-existing
unfulfilled #[expect(dead_code)] lints)
```
Compile-time assets (fonts, lucide icons, papirus manifest/generic icon, MOTD, README, source self-includes) all resolve under the bin target. CI/scripts were already migrated to the binary workflow by BORU-CARGO-06 (`codeql.yml`, `release.yaml` build with `--bin boru`, artifact paths `target/<t>/release/boru*`).

## 5. Headless launch + screenshot evidence

Launch: `xvfb-run`-style headless run of the freshly built bin on debsrv (172.16.0.59):
`/home/dan/boru --relay boru.chat:8443 --name smoke --data-dir /tmp/boru_cargo07`

- Startup log: `starting iced chat data_dir=/tmp/boru_cargo07`, identity created/loaded, `storage opened successfully`, lobby + directory topics subscribed, 2 LAN peers connected (47974d77…, 754d5785…), MCP-adjacent startup, `RoomOpened FIRED`, **no panic, no missing-file errors**.
- The only ERROR-level lines are network bootstrap noise (`Could not bootstrap the routing table`, `DHT put_mutable … NoClosestNodes`) — pre-existing headless-VM behaviour, unrelated to assets/paths.
- Screenshot (run1): full BORU home screen — sidebar (BORU wordmark, profile chip "smok · Online", CHATS/GROUPS/FRIENDS/DISCOVER/PUBLIC ROOMS/REQUESTS sections, toolbar icons), "Good morning / Your Boru node is online and ready", Download Manager button, dark "Boru is connected / Peer to peer / Secure • Decentralized • Private" status card, Mesh Health card ("Healthy — Connected: 2 direct • 0 relayed • 2 neighbors"), People & Activity + TUNNELS panels. Icons and fonts render.

Evidence files committed under `docs/cargo-migration/evidence/t07-bin-assets/`:
- `run1-screenshot.png` / `run2-screenshot.png` (1280x800, real rendered frames)
- `run1-boru.log` (full startup trace)
- `run1-stdout.log`, `run2-stdout.log`
- `run1-vs-run2-diff.png`, `run1-vs-run2-metrics.json` (comparison: PASS, 0.94% mismatch — timestamps only)

## 6. Packaging resources

- `scripts/package-windows.sh` (cross-build + package): already bin-based — `cargo build … --bin boru`, exe at `target/<target>/<profile>/boru.exe`, ships `assets/third_party/papirus` next to the exe. No stale `examples/` path. (BORU-CARGO-06).
- `scripts/package_windows.sh` (release staging, used by `release.yaml`): no `iced_chat`/`--example` references; stages exe + papirus + GStreamer runtime + notices.
- `.github/workflows/release.yaml`: builds `--bin boru`, artifact path `target/${{ matrix.target }}/release/${{ matrix.binary }}` (bin layout), packages papirus into dist. Correct.
- `.github/workflows/codeql.yml`: `cargo build --features gui --bin boru`. Correct.
- `.cargo/config.toml`: alias `boru = "run --features gui --"` → launches the bin via `default-run`. Correct.
- Tests referencing `../examples/iced_chat/…` via `#[path]` (`tests/fs17_activity_log.rs`, `tests/fs22_dashboard_coverage.rs`, `tests/protocol_registration.rs`) remain valid because the source directory was deliberately NOT moved (BORU-CARGO-03/05).

## 7. Fixes applied

**None.** No path broke under the bin target. All compile-time assets are source-file-relative (unchanged), the single runtime asset (Papirus) has an unchanged probe order with exe-relative release coverage, and data-dir resolution is untouched. Per PDF guardrails, no redesign/refactor was performed.

## 8. Acceptance criteria

- [x] `rb build --bin boru --features gui` succeeds (compile-time assets resolve).
- [x] Headless launch on debsrv: screenshot shows fonts/icons; startup log has no missing-file errors.
- [x] Data-dir round-trip works (same identity/storage read on second run, no re-migration).
- [x] UI assets and runtime resources load identically when launched via the binary workflow (same as `cargo run` — `cargo run` builds this exact `[[bin]] boru` target).
- [x] No path fix relies on cwd being the repo root (no fixes were needed).
- [x] Packaging resources (`package-windows.sh`, `package_windows.sh`, release.yaml, codeql.yml, cargo alias) use the bin layout.
