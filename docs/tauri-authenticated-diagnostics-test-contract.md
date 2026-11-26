# React/Tauri authenticated diagnostics test contract

## Decision

Use a **test-only Tauri IPC command set**, not a network MCP server. The React/Tauri application already has a single native `DesktopRuntime`; an extra MCP transport duplicates lifecycle and protocol wiring without adding value to desktop automation. This is separate from (and does not change) the legacy Iced-binary CLI MCP service or its agent-test contracts.

## Build and exposure boundary

The Tauri crate's `diagnostics-test` Cargo feature is opt-in and disabled by default. Test-only commands and their module are compiled and registered only under this feature. Never enable this feature for release, packaging, signing, or distribution builds. The ordinary React invoke API and its production command allowlist remain unchanged. The `test_diagnostics_ping` command is a gated skeleton to establish the seam; it is not yet a complete GUI automation API.

## Authentication and locality

Tauri IPC is the transport; there is no TCP listener, HTTP endpoint, or Unix socket. The privileged webview origin is the local Tauri application. Every test command must additionally require the test harness's `BORU_TEST_DIAGNOSTICS_TOKEN` and reject missing/empty/mismatched credentials. The harness must inject a fresh, high-entropy token only into the test process and pass it in each command request; do not persist it, log it, expose it to production React code, or accept it from a remote client. Token comparison should avoid content-dependent early exit (the skeleton uses a constant-time byte fold after length validation).

Enabling the feature is itself a test-build trust decision; authentication is defense in depth, not a claim that a feature-enabled binary is safe to distribute. Do not enable remote Tauri origins or add a listening socket as part of this contract.

## Allowed actions and mapping

Expose only explicit, typed operations needed by desktop tests. Prefer read-only probes first:

| Test action | Existing Tauri/runtime path |
| --- | --- |
| `ping` / readiness | `AppState.runtime` presence; gated `test_diagnostics_ping` skeleton |
| snapshot and event cursor | existing `get_app_snapshot` / `DesktopRuntime::get_app_snapshot_with_revision` |
| room/contact/network/settings inspection | existing read commands (`list_rooms`, `get_contacts`, `get_network_status`, `get_settings`) |
| send a fixture message or open a room | delegate to existing `send_message` / `commands::open_room::open_room`, with test-owned fixture data |
| await visible state | `DesktopRuntime` sequenced UI events / existing replay cursor contract, with bounded timeout |

The test interface must not expose arbitrary filesystem paths, shell execution, raw database access, key material, arbitrary runtime mutation, or a generic command dispatcher. Mutating actions should be added only as narrowly scoped wrappers around existing production commands, not by expanding those commands' semantics. Keep test credentials out of command logs and error details.

## Future contract requirements

Before adding more actions, define stable request/response DTOs, timeouts, cleanup behavior, and tests for missing/wrong tokens and feature-off absence. Test builds should prove production IPC remains exactly the same when the feature is off. Any operation that starts state must have a corresponding cleanup operation. The CLI MCP and its existing tests remain independently maintained.
