# Developer-mode gate for the live UI editor (BORU-UI-08)

The live UI editor — the `boru-ui.toml` dev theme override file, the file
watcher, and the live theme redraw wired in BORU-UI-04..07 — is a
**developer facility**. It must never silently ship as an active production
facility, so it sits behind a developer-mode gate.

## How to enable it

| Build | Enable with | Works? |
|---|---|---|
| Any build (including release) | `--features dev-ui` | ✅ always on |
| Debug build (`cargo build` / `cargo run`, no feature) | `--dev-ui` CLI flag | ✅ |
| Debug build (no feature) | `BORU_DEV_UI=1` (or `BORU_DEV_UI=true`, case-insensitive) | ✅ |
| Release build, no feature | anything — `--dev-ui`, `BORU_DEV_UI=1` | ❌ always off |

## Precedence (single decision point in `main.rs`)

`dev_ui_enabled(args.dev_ui)` (in `src/bin/boru/main.rs`) implements
the gate as:

1. `cfg!(feature = "dev-ui")` — the cargo feature wins in every build,
   including release. This is the *deliberate* opt-in.
2. Otherwise, only when `cfg!(debug_assertions)` (a debug build) **and** the
   operator passed `--dev-ui` **or** set `BORU_DEV_UI=1`/`true`.
3. Everything else — most importantly **release builds without the
   feature** — is off, regardless of flags or environment.

The pure predicate is `dev_ui_gate_on(feature_on, debug_build, cli_flag,
env_value)` so the precedence is unit-testable in every build
configuration.

## What "off" means

When the gate is off (`dev_ui == false` in `main.rs`):

- `boru-ui.toml` is **never read** — `theme_config::load_ui_theme_config`
  is not called; the app uses `BoruTheme` defaults only (an empty
  `UiThemeConfig` is passed to `set_ui_theme_config`, making the startup
  merge the identity).
- The **file watcher is never spawned** — `spawn_ui_theme_watcher` is not
  called, so no watcher thread exists. `IcedChat::ui_theme_rx` stays
  `None`, the subscription falls back to its closed-dummy receiver, and no
  `UiThemeReloaded` message can ever reach the update loop.
- Startup logs a single `debug!` line: "live UI editor disabled (dev-ui
  gate off); boru-ui.toml not loaded".
- The **inspector** (BORU-UI-09+) does not exist yet; when it lands it must
  respect the same gate.

When the gate is on, behaviour is exactly BORU-UI-04..07: the file is
loaded at startup, the watcher watches `<data_dir>` non-recursively for
`boru-ui.toml` changes, and valid reloads replace only theme state in the
app (last known-good theme is kept on parse errors).

## Design note: runtime gate vs compile-time gating

Compile-time feature gating (`#[cfg(feature = "dev-ui")]`) was considered
for the editor modules. It was deliberately **not** applied to the watcher
module: `notify` is already an unconditional dependency (shared-folder
monitoring), and the reload channel is threaded through
`IcedChat::subscription`'s 9-tuple stream-unfold state, so removing it
compile-time would force a high-risk refactor of the subscription machinery
for zero dependency savings. The runtime gate is a single decision point in
`main.rs`; when off, the load and watcher code paths are unreachable and
release builds never touch `boru-ui.toml`.

## Security

The editor subsystem performs **no network I/O**. It reads one local file
(`<data_dir>/boru-ui.toml`) and watches the local data directory with
`notify`. There is no HTTP/QUIC/TCP listener anywhere in
`theme_config.rs`, `theme_merge.rs`, or `theme_watcher.rs`, and this task
adds none — no unauthenticated remote editing port is or will be exposed by
the live UI editor.
