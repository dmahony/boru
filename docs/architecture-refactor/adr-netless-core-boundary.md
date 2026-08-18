# ADR: Boundary of the net-less (zero-feature) boru-core build (BORU-ARCH-42-FOLLOWUP)

- Status: **Accepted / recorded**
- Date: 2026-08-19
- Task: `t_124a933a` — BORU-ARCH-42 follow-up: decide the boundary for the fully
  net-less (`--no-default-features`, zero features) `boru-core` build.
- Reads against: `docs/architecture-refactor/dependency-coupling-audit.md` §6
  (BORU-REPO-003), `docs/architecture-refactor/adr-workspace-boundaries.md`
  (BORU-REPO-002), and `docs/architecture-refactor/completion-report.md` §6 item 3.
- Decides: whether a zero-feature (no `net`, no `gui`) build of `boru-core` must
  compile, and if not, where the boundary sits and what the intended outcome is.

## Summary / Decision

**`boru-core` is a networking crate and `net` is its base feature. A fully
net-less (zero-feature) build is explicitly **not** a supported build shape —
the ~24 modules that are net-coupled are carved out of the zero-feature profile
and this boundary is documented here. The zero-feature build failing to compile
is the **intended**, recorded outcome; it is not a regression and not something
to fix until the deferred physical `boru-net` / `boru-app` crate split lands.**

This is scope option **(c)** from the task body: *"explicitly carve these modules
out of the zero-feature build and document the boundary."* No production code and
no module feature-gating is changed in this task.

## Why the zero-feature build fails (and why that is intended)

The crate is declared in `Cargo.toml` with `default = ["net", "metrics", "gui"]`
and `net` enables the net-only crates (`iroh`, `iroh-blobs`, `tokio`,
`serde_json`, …). Several modules are declared in `src/lib.rs` **without** a
feature gate and carry doc comments describing them as *"always available (no
feature gate)"* — e.g. `storage`, `store`, `streaming_server`,
`catalogue_protocol`, `file_access_protocol`, `protocol_signing`,
`diagnostics`, `discovery_secret`. Those modules' implementations reference the
net-only crates (`iroh::PublicKey` / `SecretKey` / `Signature`, `tokio`, 
`serde_json`) and/or net-gated sibling modules (`chat_core`, `catalogue_model`,
`friends`, `mailbox`, `discovery_service`, …). When `net` is off, those crates
and siblings are absent, so the net-less build cannot compile.

"Always available (no feature gate)" therefore means **available in every
supported profile** — and every supported profile includes `net`. It does not
mean "available in a net-less build." That ambiguity is the core of this
boundary question, and the resolution is to make the intent explicit (below)
rather than to make the net-less build compile.

## Verified build matrix (DEBSRV via `rb`, this branch `wt/t_124a933a` @ `a744cdc7`)

| Build shape | Command | Result |
|---|---|---|
| Minimal core | `rb check --no-default-features --features net --lib` | **exit 0**, 5 pre-existing warnings |
| Core + metrics | `rb check --no-default-features --features net,metrics --lib` | **exit 0**, 5 pre-existing warnings |
| Full application | `rb check --bin boru --features gui,video-playback,terminal` | **exit 0**, 318 pre-existing warnings |
| **Zero-feature (net-less)** | `rb check --no-default-features --lib` | **exit 101 — intended** — 125 errors |

### Intended outcome of the zero-feature build (the recorded, documented result)

`rb check --no-default-features --lib` produces `exit 101` with **125 errors**
(24 × `E0432` unresolved import of net-gated siblings; 101 × `E0433` unresolved
net crate `iroh`/`tokio`/`serde_json`/`iroh_blobs`) across **24 source modules**:

```
 20  src/streaming_server.rs          3  src/catalogue_rate_limits.rs
 13  src/storage/conversation.rs      3  src/store/mod.rs
 13  src/catalogue_client.rs          3  src/control_plane/dispatch.rs
 12  src/storage/mod.rs               2  src/video_poster.rs
  8  src/outbox_delivery.rs           2  src/protocol_version.rs
  8  src/catalogue_handler/serve.rs   2  src/protocol_signing.rs
  7  src/catalogue_handler/mod.rs     2  src/discovery_secret.rs
  6  src/storage/identity.rs          2  src/catalogue_protocol.rs
  5  src/storage/transfer.rs          2  src/catalogue_policy.rs
  5  src/file_access_protocol.rs      1  src/storage/schema.rs
  3  src/catalogue_wire.rs            1  src/peer_names.rs
                                      1  src/download.rs
                                      1  src/diagnostics/events.rs
```

(`src/lib.rs` also appears 29× as the *anchor* of the "configured out" notes —
the gated sibling modules the ungated files reference. The files above are the
actual error sites.) This measured 24-module inventory supersedes the
approximate list quoted in `dependency-coupling-audit.md` §6 (`rings`,
`wire_compression` do not surface errors in this build). This outcome is
**intended**: the net-less shape is carved out, not a compile regression.

## Why not options (a) or (b)

- **(a) Gate the ~24 modules behind `net`** — would force `#[cfg(feature = "net")]`
  onto modules whose owning docs deliberately declare them "always available (no
  feature gate)". Honest gating would require **both** editing ~24 doc comments
  *and* adding the gates — the task body itself notes "one concern at a time."
  It also purchases a promise (a working net-less core) that is not a real
  build target: a net-less `boru-core` has no identity, no transport, and no
  wire protocols, so there is nothing meaningful to compile.
- **(b) Decouple their protocol types from net crates** — replacing
  `iroh::PublicKey`/`SecretKey`/`Signature` and the async `tokio`/`iroh`
  stream types with crate-local abstractions is a **broad public-API change
  across unrelated domains** (storage, catalogue, file access, diagnostics,
  signing, streaming) — a PDF §14 Stop Condition. Those types are iroh types by
  design (identity is iroh identity). Not appropriate for a boundary decision.

Both are therefore rejected in favour of (c). The net-less boundary belongs to
the deferred crate split, not to this task.

## Boundary placement (what "carve out" means)

Until the deferred physical split (`adr-workspace-boundaries.md`: `boru-net` /
`boru-app`), the canonical supported shapes of `boru-core` are **exactly** those
that include `net`:

- `net` (minimal), `net,metrics` (core), and `net,metrics,gui` (= default, full app).

`boru-core` has **no** supported configuration without `net`. The net-coupled
modules are part of the `net` boundary: they ship with `net` and are absent from
the (unsupported) net-less profile by declaration, not by per-module
`#[cfg]`. This is recorded here so that `rb check --no-default-features`
failing is a known, documented, intended outcome rather than a surprise.

## CI implication (recorded, not changed in this task)

CI's `clippy_check` job runs a `linux-no-default` leg with
`cargo clippy --workspace --no-default-features --lib --bins --tests`
(`.github/workflows/ci.yaml`), which lints the intentionally-unsupported
zero-feature shape and is therefore **red** today. Per the boundary above, that
leg's arg set does not correspond to any supported build. The recommended fix
(kept out of this task to avoid broadening scope, and to keep the parent chain's
"CI preserved" invariant untouched) is to retarget that leg to the minimal
**supported** core shape — verified green above:

```
cargo clippy --workspace --no-default-features --features net --lib --bins --tests
```

(`--bins`/`--tests` are harmless here: the `boru` bin requires `gui` and `sim`
requires `simulator`, both absent, so only the library is compiled.) This change
belongs with the deferred `boru-net` / `boru-app` crate-boundary work and is
recorded as a follow-up recommendation, not executed here.

## Consequences

- Positive: the boundary is now explicit and documented; the zero-feature build
  is a known, intended non-goal rather than an unexplained CI failure; no
  production code, protocol bytes, storage bytes, or module gating changed; the
  supported `net`-based core builds (including metrics-off) remain green.
- Negative (recorded, deferred): the `linux-no-default` clippy leg stays red
  until it is retargeted to a supported shape with the crate-split work.

## Follow-up (recorded, not acted on here)

- **Retarget the CI `linux-no-default` clippy leg** to
  `--no-default-features --features net` (a supported shape) as part of the
  deferred `boru-net` / `boru-app` crate-boundary work.
- **Revisit at the crate split:** once `boru-net` exists as a crate, `boru-core`
  (or its reduced successor) can gain a true net-less footprint; the modules
  carved out here become `boru-net`'s responsibility, and this boundary is then
  repealed.

No protocol bytes, storage bytes, or user-visible behaviour changed by this task.
