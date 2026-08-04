# Boru — File Sharing Dashboard Release Note (FS-20 … FS-25)

Feature area: **File Sharing dashboard** (route, five-tab UI, projections,
persistence, security hardening, visual QA, multi-peer E2E harness,
documentation).

This note covers the FS epic as delivered through FS-25. It is written for
release managers and operators; end users should read
[`docs/file-sharing-guide.md`](file-sharing-guide.md).

## What shipped

- **File Sharing screen** (`Screen::FileSharing`) with five tabs: Shared by
  Me, Downloading, Downloaded, Shared with Me, Activity Log. See
  [`docs/fs-02-file-sharing-dashboard-spec.md`](fs-02-file-sharing-dashboard-spec.md)
  for the design-system spec and
  [`docs/file-sharing-guide.md`](file-sharing-guide.md) for the user guide.
- **Live projections** — the dashboard renders from in-memory
  `TransferProjection` state (Downloading / Peers Downloading from Me tabs)
  and durable SQLite projections (Shared by Me, Downloaded, Shared with Me,
  Activity Log). See
  [`docs/fs-06-persistence-projections.md`](fs-06-persistence-projections.md).
- **Persistence** — schema v17: `transfer_activity` event log with
  `direction` (v16: versioned shared files + activity log; v17: direction
  column). Forward-only migrations, idempotent column adds, no data reset.
- **Security hardening (FS-20)** — request-time permission expiry
  enforcement, backend download gate (no UI-only authorization), safe
  destination path at the download write site. See
  [`docs/fs-20-security-review.md`](fs-20-security-review.md).
- **Test coverage (FS-22)** — 66 tests in `tests/fs22_dashboard_coverage.rs`
  covering projections, state transitions, persistence, security, and share
  lifecycle.
- **Multi-peer E2E harness (FS-23)** — `scripts/fs23_launch.sh`,
  `scripts/fs23_mcp.py`, `scripts/fs23_seed.py` for deterministic two-peer
  smoke runs under Xvfb + private D-Bus + portal (native picker path).
- **Visual QA (FS-24)** — token-level refinements (`destructive_soft()`
  theme token, `Typography::PageTitle`), screenshots under
  `docs/ui-redesign/evidence/fs-24/`.

## Compatibility

- Schema version: **17** (forward-only; databases from newer versions are
  refused — see `docs/troubleshooting.md`).
- No new runtime dependencies added in the FS range; the only Cargo.toml
  change is a `[[test]]` target for the FS-22 coverage suite. License
  unchanged: MIT/Apache-2.0.
- Native OS file picker remains the sole file-selection mechanism; the
  dashboard adds no in-app file browser.

## Release gates (all passed)

| Gate | Result |
|---|---|
| `cargo fmt --check` | PASS (see FS-25 handoff for command output) |
| `cargo clippy --all-features` | PASS |
| Full test suite (`cargo test --all-targets`) | PASS — see FS-25 handoff for counts |
| Multi-peer smoke test (clean profile) | PASS — see FS-25 handoff |
| Visual review | PASS — FS-24 screenshots approved |
| Security review | No unresolved release blocker (FS-20) |

## Rollback / recovery guidance

**There is no automatic downgrade path.** Schema migrations are forward-only.
If a release must be reverted:

1. **Stop** the application.
2. **Restore `boru.db` from a backup taken before the upgrade** (or from the
   previous version's data directory). The schema-version guard refuses to
   open a database newer than the binary, so simply deleting the newer
   database and starting the old binary re-initialises an empty store — use
   a backup to keep data:
   ```sh
   cp ~/.local/share/boru/boru.db.backup ~/.local/share/boru/boru.db
   rm -f ~/.local/share/boru/boru.db-wal ~/.local/share/boru/boru.db-shm
   ```
3. **Start the previous version.** The old binary reads the restored schema.

Recovery-specific notes for this feature area:

- Removing a shared offer deletes its grants in the same transaction but does
  **not** abort in-flight downloads; a rolled-back UI will still see them in
  the download state machine.
- The activity log is disposable: it is rebuilt from live lifecycle events as
  transfers run. `prune_transfer_activity` trims it (bounded at 1,000 rows).
- Downloads already completed and verified remain on disk under
  `<data-dir>/downloads`; the database rows are the only index, so keep the
  database consistent with the directory when restoring.
