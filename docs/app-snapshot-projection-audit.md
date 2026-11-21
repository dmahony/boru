# Iced app snapshot projection audit

## Scope and finding

In this checkout there is **no `AppSnapshot` type or `get_app_snapshot` API** (`rg AppSnapshot` returns no matches). Therefore there are no existing AppSnapshot construction or consumption call sites to preserve. The existing, similarly named APIs are diagnostic projections, and should not be conflated with the proposed app-level snapshot.

## Existing construction/consumption paths

- `src/bin/boru/main.rs`: composition root constructs `IcedChat::new(...)` (around line 2086) after creating the runtime services and channels. This is the current live application construction path; it is heavyweight and unsuitable as a headless snapshot constructor because it requires network/runtime handles and service objects.
- `src/bin/boru/app.rs`: `IcedChat` is the frontend state owner (struct around line 2531); its public `view(&self) -> iced::Element<'_, AppMessage>` (around line 16973) produces the Iced view. Many module tests call `app.view()` as a smoke/render behavior check. Traversing that returned widget tree is not a stable state API: it is renderer/widget implementation, may omit non-rendered backend state, and can be affected by screen/layout/lazy-widget choices.
- `src/bin/boru/app.rs`: `publish_gui_snapshot`/`build_dashboard_snapshot` (around lines 9714/9768) construct a `boru_core::diagnostics::IcedStateSnapshot`-style GUI diagnostic publication from frontend state and publish it through `gui_state_tx`. This is currently the live GUI-to-MCP snapshot path; it is a curated diagnostic subset, not a complete authoritative backend projection.
- `src/bin/boru/mcp_server.rs`: `handle_get_gui_snapshot` and `boru_get_iced_state` read the latest optional watch-channel value (`gui_state_rx`) and serialize selected fields. Absence is represented as no live snapshot; tests cover no-live-snapshot/dashboard-absent, dashboard surfacing, and security/no-secret guarantees. GUI action/wait helpers consume the published snapshot. These consumers are coupled to the watch channel and need explicit migration if the new projection is intended to replace them.
- `src/diagnostics/snapshots.rs`: `IcedStateSnapshot` and diagnostic snapshot types are serializable public diagnostics structures; tests in `src/diagnostics/tests.rs` and `tests/test_security.rs` construct them directly. Their public JSON/security contract must remain separate or be consciously versioned; do not silently expand it with secrets or raw private state.
- There are no headless callers of an `AppSnapshot`, because the symbol/API does not exist in this source tree. Many unit tests construct `IcedChat` fixtures and call `view()`, but these test construction paths are distinct from the diagnostics snapshot struct literals.

## Constraints and invariants for a backend projection

1. Snapshot reads must borrow immutable state only. No `&mut self`, dispatch, event polling, lock acquisition that can block indefinitely, service startup, network operation, persistence write, cache population, or lazy initialization. If service access requires a mutex, use nonblocking/read-only access or make the projected data available from an immutable snapshot already owned by the backend.
2. The projection is a value object independent of Iced types (`Element`, widget `Tree`, renderer, `Theme`, layout bounds) and does not require an initialized window, event loop, or GUI runtime.
3. Explicitly distinguish known-empty data from unavailable/uninitialized services. Prefer an `Option`/availability status per independently optional service over fabricated defaults that imply a successful empty query.
4. Output must be deterministic where order is observable (sort map/set-derived collections), bounded, and contain only fields required by consumers. Never expose secret keys, raw credentials, or private message/file contents by default. Preserve diagnostic serialization guarantees separately.
5. Snapshot consistency should be point-in-time for each service; if cross-service atomicity cannot be guaranteed, document it as a best-effort projection rather than claiming a globally atomic snapshot.
6. Keep rendering-derived state (window dimensions, text-input widget state, hover/focus, ephemeral overlay layout) out of a headless backend projection. If MCP needs these UI facts, keep them in the existing GUI diagnostic channel or model them as a separate optional frontend projection.

## Minimal API recommendation

For a GUI-independent owner that already owns the domain/service state, use a read-only receiver and return a plain owned value:

```rust
pub fn get_app_snapshot(&self) -> AppSnapshot
```

`AppSnapshot` should live in the GUI-independent runtime/backend crate/module and use only domain/value types (no Iced dependency). Use `Result<AppSnapshot, SnapshotError>` only if snapshot generation can genuinely fail; otherwise encode per-service unavailability in the snapshot and keep retrieval infallible. Do not put `get_app_snapshot` on `IcedChat` if the acceptance requirement is headless use: that type's constructor requires the live Iroh/router/runtime and many event channels. The natural owner is the proposed GUI-independent `DesktopRuntime` (or the backend state aggregate that will own it): call it after construction without starting a window. The immutable `&self` signature makes mutation structurally unavailable; this is preferable to a `&mut self` method or a method that accepts an Iced view.

A concrete availability shape (illustrative; fields should be limited to the agreed projection) is:

```rust
pub struct AppSnapshot {
    pub services: ServiceAvailability,
    pub conversations: Vec<ConversationSnapshot>,
    // other explicitly agreed domain projections
}

pub struct ServiceAvailability {
    pub network: Availability,
    pub storage: Availability,
}

pub enum Availability { Ready, Uninitialized, Unavailable }
```

Avoid returning borrowed references or service handles: owned data is easier to consume headlessly, serialize/test, and cannot outlive locks or expose mutation capabilities.

## Empty and uninitialized behavior

- A valid, initialized service with no records returns an empty collection and `Ready`; this is a successful empty state, not an error.
- A service not constructed/started returns `Uninitialized` (or `None` for that service's projection), not a panic and not a misleading `Ready` + empty collection.
- An initialized but failed/unavailable service reports `Unavailable` and omits/marks only its own projection; independent services should still be projected.
- A wholly uninitialized runtime should return a valid snapshot value with all relevant services marked `Uninitialized` and empty/absent domain projections. No Iced window, network connection, database mutation, or implicit startup is triggered by calling the getter.

## Regression risks / migration checks

- Widget traversal couples tests and external automation to Iced's current tree; backend projection will intentionally stop reporting layout-only or widget-local state unless kept separately.
- Replacing the GUI watch-channel snapshot wholesale can regress `boru_get_gui_snapshot`, wait conditions, dashboard visibility, and GUI test-action status; map each existing serialized field and preserve no-secret tests and absent-snapshot semantics.
- Treating uninitialized as empty can make headless tests falsely pass and MCP consumers report zero peers/rooms as authoritative.
- Calling an async service query from a synchronous getter risks blocking or runtime deadlock; prefer already-maintained in-memory projections, or define an async API deliberately (`async fn get_app_snapshot(&self) -> Result<...>`), not an implicit runtime bridge.
- HashMap iteration order, stale caches, or independently sampled services can create nondeterministic/regressing test output.
- Do not remove `IcedStateSnapshot` public serialization/constructibility until its external/public API compatibility and security consumers are explicitly migrated.

No functional code was changed for this audit.
