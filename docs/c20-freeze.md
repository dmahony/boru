# C20 compatibility freeze and desktop milestone handoff

Status: desktop protocol boundary frozen for the implemented v1 surface. This is
not a release, mobile-runtime, or push-provider deployment claim.

## Commits and dispositions

- C19 predecessor: `5d675a21` (`docs/c19-verification.md`), merged before this
  change. Its unverified network, Windows, Android, and iOS rows remain
  unverified.
- C20 implementation: `180c0045`; this handoff's commit reference update is
  `96614779`. The implementation covers companion capability intersection,
  protocol-only Cargo profile, fixture compatibility test, handler negative-path
  test, and CI wiring.
- Release publishing and push-provider deployment are separate follow-up work.

Desktop status: READY for reproducible local protocol probes and desktop unit /
fixture checks.

Mobile status: IMPLEMENTED as a documented wire/storage contract only; Android
and iOS runtime behavior remains UNVERIFIED.

Production push status: NOT VERIFIED and intentionally not part of this
milestone.

## Source and runtime map

| Boundary | Source | Runtime owner | Status |
|---|---|---|---|
| ALPN/framing, JSON enums, limits | `src/companion_protocol.rs` | authenticated Iroh endpoint | desktop implemented |
| Pairing invitation and approval | `src/companion_protocol.rs` | desktop link manager + user approval | desktop unit-tested |
| Durable registrations, grants, epoch | `src/store/companion.rs` | SQLite | desktop unit-tested |
| Snapshot and bounded change references | `src/store/companion.rs` | SQLite-backed sync service | desktop unit-tested |
| Reconnect state machine | `src/companion_reconnection.rs` | client/application integration | contract published |
| Background push boundary | `src/background_push.rs`, `docs/background-push.md` | provider adapter | disabled/recording only |
| Mobile UI, secure storage, lifecycle | `docs/mobile-app-contract.md` | Android/iOS app | not runtime-verified |

## Compatibility rules

- ALPN is `/boru/companion/1`; the current wire version is integer `1`.
- A client sends `hello` before using the boundary. The server rejects a hello
  that does not advertise version `1` with `incompatible_version`.
- Capabilities are an explicit intersection: the response contains only names
  present in both the client's `hello.capabilities` and the server's explicit
  `PUBLIC_CAPABILITIES` list (`pairing`, `history-v1`, `snapshot-v1`). Unknown
  names are ignored; clients must feature-gate by the response, not by guessing.
- Unsupported future major versions are rejected. There is no silent downgrade
  or interpretation of an unknown version as v1.
- Frame length, page-record, and page-byte limits are enforced before allocation
  or response emission. The fixture file is checked against the Rust serde
  shapes by `tests/companion_protocol_compat.rs`.

## Fresh-install and upgrade policy

Companion access is opt-in. A fresh install must persist `enabled = false` and
must not register the companion ALPN handler until the user enables linking.
An upgrade must preserve an existing explicit choice; missing or unreadable
settings migrate to false. Passing `enabled = false` to
`CompanionProtocolHandler::new` returns only `disabled` and exposes no data.
Enabling the feature never approves a device automatically: pairing still
requires the comparison-code check and explicit desktop approval.

## Migration, backup, and rollback constraints

- SQLite schema creation and forward migrations are the source of truth. Apply
  migrations transactionally and keep migration markers; do not implement
  destructive down-migrations.
- Before upgrading, close Boru and copy the complete database plus its
  companion files as a backup. Restore only a backup made by the same or a
  compatible schema version, then allow forward migration to run.
- Arbitrary replacement of a database file is not claimed to preserve revocation
  safety. A restored file can contain old registrations, grants, epochs, or
  pending state; inspect and revoke as part of the restore procedure.
- `revoke_device(registration_id)` invalidates one durable registration. The
  sync-epoch reset path invalidates every grant and snapshot. The runtime policy
  also closes tracked sessions after revocation.
- Rollback is binary/application rollback with a separately retained backup,
  not a database down-migration. If a newer schema has already migrated the
  file, use the documented backup rather than attempting to delete columns or
  markers.

## Limits and probe commands

The current limits are a 64 KiB encoded frame, 10-second frame deadline, 100
records per history page, 256 KiB history page bytes, 500 change references per
resume page, and 15-minute snapshot-token lifetime.

From the repository root:

```text
cargo fmt --all -- --check
cargo test --features protocol-only --test companion_protocol_compat
cargo test --features net --lib companion_protocol
cargo check --features protocol-only --lib
cargo run
```

For the repository's heavy or relay-dependent checks, use the configured remote
runner (`rb`) rather than a long local Cargo build, for example:

```text
rb test --lib companion_protocol
rb check --features protocol-only --lib
```

The fixture test proves schema compatibility only. It does not claim a second
peer, LAN/WAN/relay transport, GUI lifecycle, native package, mobile runtime,
or push delivery. Those dispositions remain in `docs/c19-verification.md`.

## Executed C20 evidence

Executed on September 21, 2026 from the C20 worktree:

- `rb test --features protocol-only --test companion_protocol_compat` — PASS;
  4 tests passed, including the live handler rejection of version `99`.
- `rb test --features net --lib companion_protocol` — PASS; 7 tests passed.
- `rb check --bin boru --features gui,video-playback,terminal` — PASS; build
  completed with warnings only.
- `git diff --check` — PASS.

The CI workflow runs the protocol-only compatibility test on every invocation
of `.github/workflows/tests.yaml`. The desktop check above is evidence for this
worktree only; it is not a claim that mobile, packaged, relay, or production
push paths were exercised.
