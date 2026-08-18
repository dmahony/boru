# Dependency coupling audit — GUI-only dependencies vs. default features (BORU-REPO-003)

- Status: **Applied**
- Date: 2026-08-19
- Task: BORU-ARCH-34 (PDF BORU-REPO-003, "Reduce default-feature coupling")
- Scope: which dependencies are pulled only because `gui` is part of `default` features,
  and how GUI-only dependencies are (or are not) owned by the application layer.
- Reads against: `docs/architecture-refactor/adr-workspace-boundaries.md`
  (BORU-REPO-002 / BORU-ARCH-33, which deferred the physical `boru-app` crate split)
  and the crate takes its features from `Cargo.toml` `[features]`.

## Summary

Boru continues to be a single Cargo package (`boru-core`) in which:

- `[lib]` — `src/` (boru-core domain library)
- `[[bin]] boru` — `src/bin/boru/` (the Iced desktop application, `required-features = ["gui"]`)
- `[[bin]] sim` — `src/bin/sim.rs` (`feature = "simulator"`)
- `default-run = "boru"`, `default = ["net", "metrics", "gui"]`

The net result of this audit is that **the core library is already Iced-independent**:
every GUI-heavy dependency is `optional` and only activated by the `gui` feature, which
is required by the application `[[bin]] boru`. The core builds cleanly without Iced
(verified, see below). One residual always-on dependency that was used *only* by the
GUI application layer — `socket2` — was moved behind `gui` in this task.

## 1. What `default` currently means

```
default = ["net", "metrics", "gui"]
```

`gui` is in `default` for a deliberate developer-ergonomics reason: so that
plain `cargo run` (with no `--features` flags, `default-run = "boru"`) builds and
launches the desktop app. This matches the acceptance criterion *"developer launch
remains one simple command"* and the parent ADR's rule that default developer commands
stay unchanged. Removing `gui` from `default` is not possible without a crate split —
the single `[[bin]] boru` target requires the `gui` feature, and a second package
(`boru-app`) is explicitly deferred to a later task by
`adr-workspace-boundaries.md`. The one-command `cargo run` launch is therefore preserved
through the application layer, not by forcing any GUI dependency into core code.

## 2. Audit: dependencies pulled only because `gui` is in `default`

The `gui` feature enables these optional dependencies:

| Dependency | Core (`src/`, non-bin) usage | Application (`src/bin/boru`) usage | Gated behind `gui`? |
|---|---|---|---|
| `iced` | none (only doc references) | whole app | ✔ (`gui`) |
| `iced_aw` | none | 5 files | ✔ (`gui`) |
| `iced_moving_picture` | none | 3 files | ✔ (`gui`) |
| `iced_video_player` | none | `video-playback` (implies `gui`) | ✔ |
| `iced_term` | none | `terminal` (implies `gui`) | ✔ |
| `rfd` (file dialogs) | none | 4 files | ✔ (`gui`) |
| `tracing-subscriber` | none | `main.rs` etc. | ✔ (`gui`) |
| `tracing-appender` | none | `main.rs` | ✔ (`gui`) |
| `mimalloc` | none | `main.rs` | ✔ (`gui`) |
| `rayon` | none | 1 file | ✔ (`gui`) |
| `profiling` | none | none (dev tooling) | ✔ (`gui`) |
| `rustc-hash` | none | `app.rs` | ✔ (`gui`) |
| `regex` | none | 2 files | ✔ (`gui`) |
| `open` | none | 4 files | ✔ (`gui`) |
| `url` | `klipy_provider` (gui-gated) | 2 files | ✔ (`gui`) |
| `reqwest` | `klipy_provider` (gui-gated) | 3 files | ✔ (`gui`) |
| `netstat2` | `local_service_scan` (gui-gated) | none | ✔ (`gui`) |
| `sysinfo` | `local_service_scan` (gui-gated) | none | ✔ (`gui`) |
| `clap`, `toml` | none | app/sim | ✔ (`gui` / `simulator`) |
| `unicode-segmentation` | none | emoji parser | ✔ (`gui`) |
| `socket2` | **none (was always-on)** | `mcp_server.rs` | **→ moved to `gui` (this task)** |

Core-lib modules that consume GUI-only crates are themselves gated behind `gui` in
`src/lib.rs`:
- `local_service_scan` (uses `netstat2`, `sysinfo`) — `#[cfg(feature = "gui")]`
- `klipy_provider` (uses `url`, `reqwest`) — `#[cfg(feature = "gui")]`
- `image_optimizer`'s display-thumbnail helper `compress_image` — `#[cfg(feature = "gui")]`

So **no GUI-only dependency is required to compile the core library**.

## 3. The one real fix: `socket2`

`socket2` was declared non-optional (`socket2 = "0.5"`), yet the only usage in the whole
repository is the desktop MCP server socket binding in `src/bin/boru/mcp_server.rs`
(`socket2::Domain / Socket / SockAddr`). It was therefore being compiled into every
build (including core-only) even though it is purely an application-layer dependency.

Change (this task):
- `socket2 = "0.5"` → `socket2 = { version = "0.5", optional = true }`
- `gui = [ ... , "dep:socket2" ]` (the application feature now owns it)

Behaviour is unchanged: `gui` builds (and therefore `cargo run`) still get `socket2`;
core and `net`-only builds no longer compile it. `Cargo.lock` is unchanged.

## 4. Verified build matrix (debsrv via `rb`)

| Build | Command | Result |
|---|---|---|
| Full application | `rb check --bin boru --features gui,video-playback,terminal` | ✔ exit 0 (318 pre-existing warnings) |
| Core-only (net, no GUI) | `rb check --no-default-features --features net,metrics` | ✔ exit 0 (5 pre-existing warnings) |
| Core-only (net, no GUI) — pre-change baseline | same | ✔ exit 0 |

Both build shapes pass before and after the `socket2` change, confirming the core
library builds without Iced and that moving `socket2` behind `gui` did not regress the
`net`-only core build.

### Zero-feature build (`--no-default-features`, no `net`)

`:warning:` A fully feature-less build of `boru-core` (`cargo check --no-default-features`,
no `net`, no `gui`) does **not** compile: ~125 errors across ~24 modules (e.g.
`storage`, `store`, `catalogue_*`, `control_plane`, `protocol_signing`,
`file_access_protocol`, `diagnostics`, `rings`, `wire_compression`, `streaming_server`,
`discovery`/`discovery_secret`) that reference `net`-only crates (`iroh`, `tokio`,
`serde_json`) or `net`-gated sibling modules.

This is **out of scope for BORU-REPO-003** and deliberately not fixed here, for three
reasons:
1. The acceptance criterion is *"core/domain code can be built without Iced"*, which is
   satisfied by the `net,metrics` core-only build above; boru-core is a networking crate
   and `net` is its base feature.
2. Several of the affected modules carry doc comments stating they are "always available
   (no feature gate)" (e.g. `catalogue_protocol`, `file_access_protocol`, `diagnostics`);
   making a net-less core compile would require either gating ~24 modules (contradicting
   that documented design) or decoupling their protocol types from `net` (a broad,
   cross-domain refactor — a PDF §14 stop condition).
3. It is unrelated to GUI/default-feature coupling; it is a core-boundary concern for
   the deferred `boru-app` / `boru-core`-vs-`boru-net` split work and the CI
   `--no-default-features` job, and is recorded as a follow-up.

## 5. Acceptance criteria

- **Core/domain code can be built without Iced.** ✔ Verified: core-only
  (`--no-default-features --features net,metrics`) builds with exit 0; all GUI deps are
  optional and gated behind `gui`.
- **GUI dependencies are owned by the application layer.** ✔ All heavy GUI deps are
  optional and only enabled by the `gui` feature that the `[[bin]] boru` application
  requires; the one always-on GUI-only dep (`socket2`) was moved behind `gui` in this
  task.
- **Developer launch remains one simple command.** ✔ `default-run = "boru"` +
  `gui` in `default` keeps plain `cargo run` working unchanged.

## 6. Follow-up (recorded, not acted on here)

- **Full `--no-default-features` (net-less) core build** — **RESOLVED by
  `t_124a933a`** → boundary decided `docs/architecture-refactor/adr-netless-core-boundary.md`:
  the zero-feature build is intentionally **not** supported (scope option (c) —
  carve out + document). `net` is `boru-core`'s base feature; the ~24 net-coupled
  modules are part of the `net` boundary; `rb check --no-default-features` failing
  (125 errors, exit 101) is the documented, intended outcome. Retargeting the CI
  `--no-default-features` clippy leg (CI `.github/workflows/ci.yaml` already runs
  `cargo clippy --workspace --no-default-features --lib --bins --tests`) to
  `--features net` is recorded there as a follow-up for the deferred `boru-net` /
  `boru-app` crate-boundary work.
- **Physical `boru-app` crate split** (per `adr-workspace-boundaries.md`) — once `gui`
  can be dropped from `default` via a dedicated application package, plain `cargo run`
  keeps working through that package while the core library's default build is GUI-free.

## 7. DoD

- `cargo fmt --check` clean on changed regions; `git diff --check` clean.
- Full + core-only builds verified on debsrv (exit 0).
- No protocol bytes, storage bytes, or user-visible behaviour changed.
- Cargo.lock unchanged.
