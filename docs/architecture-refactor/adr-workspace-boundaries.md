# ADR: Workspace boundaries after module cleanup (BORU-REPO-002)

- Status: **Accepted**
- Date: 2026-08-19
- Task: BORU-ARCH-33 (PDF BORU-REPO-002, "Decide workspace boundaries after module cleanup")
- Decides: whether to split the single `boru-core` package into a multi-crate workspace
  (`boru-core`, `boru-net`, `boru-storage`, `boru-app`).
- Reads against: `docs/architecture-refactor/architecture-boundaries.md` (BORU-ARCH-002)
  and `docs/architecture-refactor/baseline.md` (BORU-ARCH-001).

## Context

After Phase 4 module cleanup the repository is a single Cargo package named `boru-core`:

- `[lib]` — `src/` (boru-core), already decomposed into focused module directories
  (`net/`, `store/`, `storage/`, `chat_core/`, `backfill/`, `diagnostics/`,
  `catalogue_*`, `discovery_*`, `tunnel/`, `screen_share/`, …) plus a facade `lib.rs`.
- `[[bin]] boru` — `src/bin/boru/` (the Iced desktop app tree, ~100 view-layer files),
  which consumes the library by crate name (`use boru_core::…`).
- `[[bin]] sim` — `src/bin/sim.rs` (simulator, `feature = "simulator"`).

BORU-REPO-001 already moved the app out of `examples/` into a normal application
binary location; the `[[bin]] boru` ↔ `[lib] boru-core` separation is therefore an
existing logical boundary, currently realized *inside one package*.

## Decision

**Do not create a multi-crate workspace at this point. Keep the single `boru-core`
package (library `src/` + `[[bin]] boru` + `[[bin]] sim`) and record the intended
future crate boundary instead.** This task is **ADR-only**: no crate is split and no
domain types are moved or duplicated. Default developer commands (`cargo run`,
`rb build --bin boru --features gui,video-playback,terminal`) are unchanged.

The proposed four-crate layout is the long-term target, but each of its boundaries is
blocked today by a PDF §14 stop condition:

1. **`boru-net` / `boru-storage` / `boru-core` split — rejected now.** The module
   directories within `src/` (net, store, storage, chat_core, …) are tightly
   coupled: they share domain types (`Message`, `TopicId`, identity/hash types,
   `GossipSender`, store handles) across what would become crate edges. Hoisting those
   types into shared crates, or duplicating them per crate, violates the *"same state
   begins to exist in both the old and new module"* and *"extraction requires broad
   public API changes across unrelated domains"* stop conditions. The intra-crate
   module boundaries already deliver the API-isolation benefit this split would, at a
   fraction of the churn.

2. **`boru-app` split — the real first boundary, deferred.** The one split with a
   genuine build/API benefit (core builds without Iced; the app owns GUI-only
   dependencies) is promoting `src/bin/boru` to its own `boru-app` crate. That is
   blocked on two preconditions that are themselves open work:
   - the app tree is still a view-layer split (per `architecture-boundaries.md` §1)
     with no clean module interface to expose — a crate boundary now would merely
     relocate the monolith, not isolate an API; and
   - `gui` is part of `default` features, so `boru-core` cannot yet build without
     Iced. Moving GUI-only dependencies off the core/default path is the explicit
     scope of **BORU-REPO-003**, the immediately following task.
   Splitting the app before either lands would be a big-bang migration with no API
   benefit, violating "move one boundary at a time" and "start with the minimum split
   that provides a real build/API benefit".

## Consequences

- Positive: no duplicated domain types; no broad public API churn; default developer
  commands stay simple; the existing lib↔bin boundary is preserved; the decision is
  fully reversible (revisit in a later task).
- Negative: core still links Iced through `default` features until BORU-REPO-003;
  physical crate boundaries remain implicit in the single package for now.

## Follow-up (recorded, not acted on here)

- **BORU-REPO-003** (`Reduce default-feature coupling`) — its direct scope. It moves
  GUI-only dependencies to the application layer and enables a core-only
  `--no-default-features` build of `boru-core` without Iced.
- **First real boundary (future task, after BORU-REPO-003 + app domain
  decomposition):** promote `src/bin/boru` to a `boru-app` workspace crate that
  depends on `boru-core` and owns the Iced/GUI dependency set. One crate, one
  dependency edge, verified with `rb check`/`rb build` before merging.

No protocol bytes, storage bytes, or user-visible behaviour change in this task.
