# BORU-CARGO-01 — Cargo structure audit + launch baseline

**Task:** t_362e86b3 (BORU-CARGO-01, step 1 of the Boru Cargo target migration)
**Date:** 2026-08-11
**Commit audited:** `9e8ddbaa` — origin/main @ 0.200.1 (worktree `wt/t_362e86b3` fast-forwarded to origin/main before auditing)
**Scope:** read-only audit. No Cargo.toml, target, or source restructuring was performed.
**Prior related work:** `BASELINE.md` (repo root, BORU-CALL-0.1, t_f4c89b47) already established that the example name is `boru` and that `--example iced_chat` is dead; this report re-verifies that against current main and adds the launch dependency inventory + fresh run evidence.

---

## 1. TL;DR — what launches the Boru desktop app TODAY

The Boru desktop app is built and launched as a **Cargo example**, not a bin:

```
cargo run --features gui --example boru
```

- The **real entry point** is `fn main()` at `examples/iced_chat/main.rs:465`, inside a
  module tree of ~45 sibling modules under `examples/iced_chat/` (`app.rs` holds the
  `IcedChat` iced-application root).
- The target is declared explicitly in `Cargo.toml`:
  `[[example]] name = "boru"` `path = "examples/iced_chat/main.rs"` `required-features = ["gui"]`.
- There is **no `src/main.rs`** and **no `[[bin]] boru`**. The only `[[bin]]` is
  `sim` (`src/bin/sim.rs`, `required-features = ["simulator"]`).
- The PDF's old command `cargo run --example iced_chat` is **broken on current main**:
  `cargo build --example iced_chat` exits 101 with
  `error: no example target named 'iced_chat' in default-run packages`. It worked only
  before commit `2cd365e7` ("chore(branding): rename example binary from iced_chat to boru").

Verified empirically via `cargo metadata --no-deps --format-version 1` (target list) and
by running the candidate commands (see §4).

---

## 2. Current Cargo structure inventory

Package: `boru-core` v0.200.1, edition 2021, resolver 2, rust-version 1.91.

### Target kinds (from `cargo metadata --no-deps`)

| Kind | Name | Path | required-features |
|---|---|---|---|
| lib (rlib) | `boru_core` | `src/lib.rs` | — |
| bin | `sim` | `src/bin/sim.rs` | `simulator` |
| example | **`boru`** | **`examples/iced_chat/main.rs`** | **`gui`** |
| example | `setup` | `examples/setup.rs` | `examples` |
| example | `video_backend_probe` | `examples/video_backend_probe.rs` | `video-playback` |
| example | `doctor` | `examples/doctor.rs` | `net` |
| example | `catalogue_browser` | `examples/catalogue_browser.rs` | `net` |
| example | `dht_harness` | `examples/dht_harness.rs` | — (auto-discovered) |
| example | `test_addr` | `examples/test_addr.rs` | — (auto-discovered) |
| bench | `phase23` | `benches/phase23.rs` (harness=false) | — |
| bench | `compression_bench` | `benches/compression_bench.rs` (harness=false) | `net` |
| custom-build | build-script | `build.rs` | — |
| test | ~95 targets | `tests/*.rs` | various (`net`, `test-utils`, `gui`, `voice-calls`, `video-calls`) |

Auto-discovery facts:
- Declaring `[[example]] name="boru" path="examples/iced_chat/main.rs"` **claims that
  path**, so Cargo's example auto-discovery does **not** additionally create an example
  named `iced_chat`. `cargo metadata` lists **no** `iced_chat` target. (Auto-discovery
  still creates `dht_harness` and `test_addr` from unclaimed files.)
- `src/bin/sim.rs` is the only bin; `src/main.rs` does not exist.

### [features] (all)

| Feature | Implies | Purpose |
|---|---|---|
| `default` | `net`, `metrics` | — |
| `net` | irpc, iroh, iroh-blobs, iroh-mdns-address-lookup, iroh-mainline-address-lookup, tokio, tokio-util, futures-concurrency, serde_json | core networking |
| `metrics` | iroh-metrics/metrics | telemetry (local, off by default via explicit feature selection) |
| `examples` | `net` | for `setup` example |
| **`gui`** | `net` + iced, iced_aw, iced_moving_picture, tokio, rfd, tracing-subscriber, tracing-appender, mimalloc, rayon, profiling, rustc-hash, regex, open, url, reqwest, netstat2, sysinfo | the desktop app |
| `video-playback` | `gui` + iced_video_player | inline GStreamer video playback |
| `terminal` | `gui` + iced_term | embedded terminal tab |
| `voice-calls` | cpal, rtrb, opus, rubato, nnnoiseless | native audio I/O |
| `video-calls` | `voice-calls` + nokhwa, openh264 | camera capture + H.264 |
| `screen-sharing` | `net` + openh264, zbus, libloading, windows-sys, windows, x11rb | remote control / screen cast |
| `experimental-vnc` | `gui` | dev-only VNC prototype |
| `simulator` | `test-utils` + tracing-subscriber, toml, clap, serde_json, rayon, comfy-table | sim bin |
| `test-utils` | rand/chacha, humantime-serde | tests |

Launch-relevant feature facts:
- `gui` is **required** by the `boru` example (`required-features=["gui"]`). Building
  without it errors: `target 'boru' requires the features: 'gui'`.
- `video-playback` and `terminal` are **separate** features that each imply `gui` and
  compile in additional UI (inline video player; terminal tab). The documented base
  launch command (`--features gui`) omits them; CI Linux release builds use
  `gui,video-playback`; Windows release uses `gui,terminal,voice-calls,video-calls,screen-sharing`.

### [patch.crates-io]

All **local path patches** under `patched/` (these are load-bearing for any build —
moving/renaming them or the `[patch]` section breaks the build):

- `iroh` → `patched/iroh`
- `iroh-dns` → `patched/iroh-dns` (Windows 11 DoH DNS fix)
- `iroh-relay` → `patched/iroh-relay`
- `irpc` → `patched/irpc`
- `mainline` → `patched/mainline` (Windows UDP read-timeout fix)
- `n0-mainline` → `patched/n0-mainline`
- `iced_tiny_skia` → `patched/iced_tiny_skia` (0.14.0 geometry-clip double-transform fix)

### [profiles]

- `release`: `debug=0`, `strip=true`, `lto="fat"`, `codegen-units=1`
- `profiling`: inherits release, `debug=true`, `strip=false`
- `bench`: `debug=true`

---

## 3. Real entry point

- **File:** `examples/iced_chat/main.rs`
- **Function:** `fn main() -> Result<()>` (line 465) — clap `Args::parse()` (line 466),
  then `ensure_graphical_session()` (line 467, Linux requires `DISPLAY` or
  `WAYLAND_DISPLAY`, else exits 1 — this is why `xvfb-run` is needed headless).
- **Application root:** `app::IcedChat` (iced `Application`), constructed later in
  `main()`; the `run()` starts the window/event loop.
- Module tree: `mod` declarations at `main.rs:8-45` (~45 modules; `terminal_view` is
  `#[cfg(feature = "terminal")]`-gated; `offscreen_status_card` is `#[cfg(test)]`).
- `main.rs` also installs a `#[global_allocator]` (mimalloc) and a panic hook that
  writes `crash_reports/crash-<ts>.txt` + appends to `instance.log` under the data dir.

---

## 4. Launch command verification (empirical)

| Candidate | Result | Verdict |
|---|---|---|
| `cargo build --example iced_chat` | exit 101: `error: no example target named 'iced_chat' in default-run packages` (help lists `boru, catalogue_browser, dht_harness, doctor, setup, test_addr, video_backend_probe`) | **BROKEN** — PDF's old command |
| `cargo run --features gui --example boru` | builds + launches (verified, see §8) | **KNOWN-GOOD launch command** |

Doc/CI references agreeing with `--example boru`:
- `README.md:43-49` — `cargo run --example boru --features gui -- --name <nickname>` (+ `--help`, `BORU_DATA_DIR=~/.boru` variant)
- `examples/iced_chat/main.rs:3-6` — doc comment: `cargo run --features gui --example boru` (+ `open`, `join <ticket>`)
- `.github/workflows/release.yaml:71` — `cargo build --release ... --features <matrix> --example boru`
- `.github/workflows/codeql.yml` — `cargo build --features gui --example boru`
- `scripts/install.sh:34,59`, `VALIDATION.md`, `BASELINE.md`, `DESIGN_SYSTEM.md`, many `scripts/ui_*_evidence.sh` — all reference `--example boru`

**Why `--example` is currently required:** the GUI is a library *example* of the
`boru-core` package. There is no `[[bin]]` that points at the GUI, so Cargo has no
`bin`/`cargo run` (bare) target for it. The branding rename (`2cd365e7`) changed the
example's *name* (`iced_chat` → `boru`) and added the explicit `[[example]]` entry, but
kept the file under `examples/`, so the only Cargo invocation that produces/runs the app
is `--example boru` with the `gui` feature (enforced by `required-features`).

---

## 5. Launch dependencies

### 5.1 Required features

- Minimum: `--features gui` (implies `net`).
- Production/CI additionally: `video-playback` (Linux release), `terminal`,
  `voice-calls`, `video-calls`, `screen-sharing` (Windows release; macOS ships `gui` only).
- With bare `gui` only: inline video player is compiled out (Play opens files
  externally) and the terminal tab is absent — observed behavior, not an error.

### 5.2 Environment variables (read at startup)

| Var | Used by | Effect |
|---|---|---|
| `--data-dir` (flag) > `BORU_DATA_DIR` > `BORU_CHAT_DATA_DIR` (deprecated) > legacy auto-detect > `$XDG_DATA_HOME/boru` > `$HOME/.local/share/boru` > `$LOCALAPPDATA\boru` > `$PWD/.boru` | `src/data_dir.rs::resolve_data_dir` | persistent identity/friend/storage root; `auto_migrate_data_dir()` copies legacy `boru-chat` dirs once, never overwrites |
| `RUST_LOG` | `main.rs:init_logging` | EnvFilter override; default file filter `info,iroh::socket=error,iroh::net_report=error,noq_proto::connection=error,winit=error` |
| `DISPLAY` / `WAYLAND_DISPLAY` | `ensure_graphical_session` (Linux) | required, else exit 1 (use `xvfb-run` headless) |
| `BORU_PAPIRUS_ASSETS` | `file_type_icon.rs` | override for the Papirus icon bundle root |
| `BORU_CHAT_FILES_DIR` | `app.rs:7196` | override image-store files root (default `<data_dir>/files`) |
| `BORU_PERF` | `src/perf.rs` doc | enable perf instrumentation (also `--perf`) |
| `REDUCED_MOTION` | `app.rs:7117` | animation reduction |
| `RUST_BACKTRACE` | panic hook (crash report) | recorded, not required |
| `SHELL` / `COMSPEC` | `terminal_view.rs` | shell for the embedded terminal (feature `terminal`) |

### 5.3 Working-directory assumptions

- **No required cwd for the core launch.** Data dir, logs, keys all resolve from the
  data-dir priority chain (§5.2), independent of cwd.
- Cwd-dependent fallbacks exist but are non-essential:
  - `$PWD/.boru` / `$PWD/.boru-chat` are last-resort data dirs (`data_dir.rs`).
  - Papirus icon probe includes `<cwd>/assets/third_party/papirus` (ad-hoc layout).
  - `app.rs:8631` falls back to `current_dir().join(name)` when locating a downloaded
    file by name (backward-compat).
- Splash: looks for `splash.py` next to the binary, else a **baked absolute fallback**
  `/home/dan/iroh-gossip-chat/scripts/splash.py` (dev-machine path; harmless when absent
  — splash silently skipped).

### 5.4 Asset paths

**Compile-time (`include_str!`/`include_bytes!`) — all resolved relative to the source
file, baked into the binary, no cwd/exe dependency at runtime:**

- Fonts: `examples/iced_chat/fonts/*.ttf` (Figtree, Raleway, JetBrainsMono, InterTight,
  PublicSans) via `fonts.rs:40-75` (+ `offscreen_status_card.rs` test module)
- Icons: `assets/icons/lucide/*.svg` via `icon_system.rs:42-67`, `app.rs:925-930`
- Papirus manifest + fallback icon: `assets/third_party/papirus/manifest.json` and
  `.../32/application-x-generic.svg` via `file_type_icon.rs:690-696`,
  `file_type_resolver.rs:102`
- MOTD: `examples/iced_chat/motd.txt` via `terminal_view.rs:20`
- Source-view self-includes (dev/test): `card_shell.rs`, `quick_actions.rs`,
  `video_file_card.rs`, `ui_components.rs` etc.

**Runtime (exe/cwd-relative) — the ONE cwd/exe-sensitive runtime asset:**

- Papirus icon bundle (`file_type_icon.rs::papirus_asset_root`) probes in priority order:
  1. `BORU_PAPIRUS_ASSETS` env
  2. `<exe_dir>/assets/third_party/papirus` (release package layout)
  3. `<exe_dir>/../assets/third_party/papirus`
  4. `<cwd>/assets/third_party/papirus`
  5. `CARGO_MANIFEST_DIR/assets/third_party/papirus` (baked build-machine path)
  - Failure is non-fatal: renders an embedded generic icon + one-time WARN diagnostic.
  - `scripts/package-windows.sh` ships the papirus tree next to the exe for this reason.

### 5.5 build.rs behaviour (`build.rs`)

- Runs `git rev-parse --short HEAD` → sets `cargo:rustc-env=GIT_HASH` (silent no-op if
  git fails).
- Sets `cargo:rustc-env=BORU_APP_VERSION` = `CARGO_PKG_VERSION` (authoritative version;
  version bumps go through `scripts/version.py`).
- `cargo:rerun-if-changed=.git/HEAD` and `.git/refs/heads/` (so a new commit rebuilds;
  a `touch build.rs` is required to force rebuild when the ref-file watch misses).
- `app.rs::version_tag()` renders `v<version> (<git-hash>)` when GIT_HASH is present,
  else `v<version>`.
- **Caveat observed (remote worktree builds):** the worktree's `.git` is a *file*
  (`gitdir: /home/dan/iroh-gossip-chat/.git/worktrees/<id>` — an absolute local path).
  `rb` rsyncs that file verbatim, so on debsrv the remote checkout is not a valid git
  repo; `git rev-parse` fails and **GIT_HASH is not embedded** (verified: no `9e8ddbaa`
  in the built binary's strings, while `0.200.1` IS present). Canonical-repo builds on
  debsrv embed the hash normally. The baseline binary therefore reports
  `v0.200.1` without a hash — expected for this environment, not a regression.

### 5.6 CLI args (clap, `main.rs:132-189`)

- `--secret-key <hex>` — override identity (default: load/generate
  `<data_dir>/secret_key.txt`, 0600)
- `-r, --relay <RelayUrl>` — relay override (default `https://boru.chat:8443`)
- `--no-relay` — disable relay (mutually exclusive with `--relay`)
- `--no-dht` — disable public/private room DHT discovery (mDNS/relay/tickets stay active)
- `--publish-direct-addresses` — publish public IPs to DHT (privacy warning; requires DHT)
- `--data-dir <dir>` — persistent data root (§5.2)
- `-n, --name <nickname>` — display name (default: short public key)
- `--bind-port <u16>` — local bind port (default 0 = ephemeral)
- `--perf` — enable perf instrumentation + exit report
- `--mcp` — enable MCP diagnostic server
- `--enable-gui-test-actions` — GUI test actions over MCP (requires `--mcp`; emits a
  stderr warning)
- `--mcp-bind <addr>` — MCP bind (default `127.0.0.1:8765`)
- Subcommands: `open [topic]` | `join <ticket>` | `logs`; **no subcommand** = open the
  public lobby automatically (auto-subscription startup path).

---

## 6. Baseline evidence (captured before any migration change)

Directory: `docs/cargo-migration/evidence/t01-baseline/` (committed with this task)

| File | Description |
|---|---|
| `boru-screenshot.png` | 1280x800 rendered GUI under Xvfb (64,868 bytes; 1,425 unique colours; light theme with dark status card — genuine rendered frame, verified not blank) |
| `startup-boru.log` | Application persistent log (`<data-dir>/logs/boru.log`), full startup trace |
| `boru-help.txt` | `boru --help` output (all options/subcommands, see §5.6) |
| `startup-stdout.log` | stdout/stderr of the launch (empty except the GUI-test-actions warning — the app logs via the file appender) |

Build + launch facts:

- **Command:** `rb build --example boru --features gui` (debsrv slot 2, sccache-warmed;
  finished 56.49s; exit 0; 259 pre-existing warnings — unfulfilled `#[expect(dead_code)]`
  lints etc.)
- **Binary:** `debsrv:~/boru-build/work-target-2/debug/examples/boru`, 1,257,545,344 bytes
  (debug), sha256 `2c82cc9835e5809b2abf010dc5345c03af8ac4ed75ad3a8c479bee897161168a`,
  mtime 2026-08-11 23:53 +1000 (fresh; commit 9e8ddbaa)
- **Launch command (headless):**
  `xvfb-run --auto-servernum --server-args="-screen 0 1280x800x24" ./boru --relay boru.chat:8443 --mcp --enable-gui-test-actions --mcp-bind 127.0.0.1:18765 --name audit-t01 --data-dir /tmp/boru-baseline-t01 open`
- **Startup confirmation (from `startup-boru.log`):**
  - `starting iced chat data_dir=/tmp/boru-baseline-t01`
  - identity: `secret_key.txt` created (0600), public key logged
  - `> relay: boru.chat:8443` (default VPS relay)
  - SQLite migrations 0→19, `storage opened successfully`, `boru.db` created
  - lobby topic subscribed; directory topic subscribed (`d68fa4ec…`)
  - **Mainline DHT listening 0.0.0.0:42713**
  - gossip mesh: `direct connect succeeded` to both LAN test VMs
    (`172.16.0.54:38026`, `172.16.0.55:33227` — peers 47974d77… / 754d5785…)
  - **MCP diagnostic server listening on 127.0.0.1:18765** (reached ~20s after launch;
    the GUI event loop is up by then)
  - `download-manager: startup recovery complete`, `RoomOpened FIRED`
  - **no panic, no `crash_reports/`** — clean startup
- Screenshot taken ~26s after launch, after MCP was listening, so the window had rendered.

---

## 7. Migration guardrails derived from this audit (for BORU-CARGO-03+)

- Keep the entry file reachable under whatever target kind replaces the example:
  `main.rs` (and its `mod` tree) is self-contained inside `examples/iced_chat/` and
  does not assume example-vs-bin; but its splash fallback bakes a dev-machine path
  (`/home/dan/iroh-gossip-chat/scripts/splash.py`) — fine as a fallback, don't rely on it.
- `required-features=["gui"]` on the example is the mechanism that forces `--features gui`;
  any future `[[bin]] boru` must carry the same gating or document the feature requirement.
- Keep `[patch.crates-io]` paths (`patched/`) intact — they are load-bearing.
- Runtime assets must keep the exe-relative + env-var probe order (Papirus bundle),
  because a bare binary (esp. Windows cross-build) has no `CARGO_MANIFEST_DIR` at runtime.
- Data-dir resolution chain and env var names (`BORU_DATA_DIR`, `BORU_CHAT_DATA_DIR`,
  `BORU_CHAT_FILES_DIR`, `BORU_PAPIRUS_ASSETS`) are persisted-layout contracts — do not
  rename without a compat migration.
- `build.rs` GIT_HASH behaviour: only emitted when the build runs inside a real git
  checkout; preserve the `rerun-if-changed` lines so version-tag freshness keeps working.

## 8. Verification checklist (this task)

- [x] `cargo metadata --no-deps` target inventory captured (§2)
- [x] Candidate launch commands tried empirically: `--example iced_chat` fails (101),
      `--features gui --example boru` is the known-good command (§4)
- [x] Asset `include_*` classified compile-time vs runtime (§5.4)
- [x] build.rs behaviour documented incl. remote-worktree GIT_HASH caveat (§5.5)
- [x] Baseline built (`rb build --example boru --features gui`) and launched headless on
      debsrv with xvfb-run; startup log + screenshot captured (§6)
- [x] Networking confirmed (DHT listening, relay configured, LAN peers connected, MCP up);
      no startup panics
- [x] Report written at `docs/cargo-migration/01-cargo-audit.md`
