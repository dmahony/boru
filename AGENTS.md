# AGENTS.md — Boru (iroh-gossip-chat)

Guidance for code agents working in this repository.

## What this is

**Boru** is a peer-to-peer chat application built on [Iroh](https://www.iroh.computer/)
(QUIC + gossip), with no central server. Features: group chat, encrypted direct
messaging (inbox/offline delivery), content-addressed file sharing, secure TCP
tunnels, peer discovery (mDNS / Mainline DHT / tickets / relay), SQLite
persistence with forward-only migrations, and a cross-platform **Iced** GUI.

The package is `boru-core`. The binary target is `boru` (the single
authoritative startup path — `default-run = "boru"`). `sim` is a separate
simulator binary gated behind the `simulator` feature.

## Working here

- **Rust edition 2021**, MSRV `1.91`. Workspace-local lints in `Cargo.toml`
  (`[lints.rust]`, `[lints.clippy]`).
- **Repo discipline**: the canonical checkout is `/home/dan/iroh-gossip-chat`.
  Do work on a branch/worktree, not directly on `main`.  Commit early and often;
  push/merge when a unit is mechanical and verified.
- **Don't run drive-by refactors.** Touch only what the task needs. Match
  surrounding style exactly (naming, module layout, error handling).
- **SQLite is the single source of truth** for app data. Prefer SQLite over
  JSON/dual-store persistence for any new state.

## Build / check / verify

The repo has a `justfile` — prefer these recipes:

```sh
just check-gui           # cargo check --features gui --bin boru (fastest)
just build-gui           # cargo build --features gui --bin boru (debug)
just build-gui-release   # release build
just lint-gui            # cargo clippy --features gui --bin boru
just test-gui            # cargo test --features gui
```

Raw equivalents if you don't have `just`:

```sh
cargo check --features gui --bin boru
cargo build --features gui --bin boru
cargo clippy --features gui --bin boru
cargo test --features gui
```

- Debug builds are preferred for VM iteration (~25s) over release (~8min).
- **Never** run `clippy-driver`/`rustc` directly on a source file without the
  edition flag — the harness defaults to Rust 2015. Use `cargo clippy` or
  `./scripts/lint.sh`.

## Architecture touchpoints

- **Protocol types / `ChatCallbacks` trait** — the backbone. Instantiating a
  new packet type means threading it through parsing, the trait, and both
  frontends (TUI + iced).
- **Frontend synchronization** — a change in one frontend usually needs the
  same change in the other. Don't fix one and forget the sibling.
- **Image handling / history persistence** — see `docs/` for design notes.
- **Feature flags** — conditional compilation is documented in
  `CONDITIONAL_FEATURES.md` and `docs/build-release.md`. The Windows cross-build
  uses a different feature set (`gui,terminal,voice-calls,video-calls`) — don't
  assume Linux features carry over.

## After any state mutation

Trace **every consumer**: sidebar, persistence layer, render, handlers. A
boolean flag set on the start path that is not reset on the cleanup path leaves
the UI stuck. Cleanup must reset everything the start path sets.

## Verification standard

- Silent failures are unacceptable. Any visible UI element (files, uploads,
  downloads, presence) must actually render.
- For P2P changes, verify normal chat messages in **both directions**, not just
  diagnostic probes. Distinguish transport failures, sender-lifecycle failures,
  and persistence failures.

## More context

- `README.md` — overview, running, licensing.
- `docs/ARCHITECTURE.md`, `docs/app-module-map.md` — module & architecture.
- `docs/BUILD_NOTES.md`, `docs/build-release.md` — build specifics.
- `PLAN.md` — current implementation plan.
- Keep generated/agent workflow artifacts out of the tracked repo; store them
  locally (e.g. `/home/dan/...` or a scratch dir).
