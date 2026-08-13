# Changelog

All notable changes to Boru are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The authoritative version source is `Cargo.toml`. For full change history,
see the [git log](https://github.com/dmahony/boru/commits/main/).

## Unreleased

### Added

- Owner/admin directory-visibility controls (BORU-DIR-06): a room-settings
  dialog lets the room owner switch a public room between
  Public-Discoverable and Public-Unlisted and edit the advertised metadata
  (name / description / tags). Switching to Discoverable immediately
  publishes a fresh signed advertisement and marks the room for periodic
  refresh; switching to Unlisted stops refreshing, stops any DHT tracker,
  and removes the room from the local directory immediately (remote
  directories drop it after the advertisement TTL — no withdrawal message
  yet, BORU-DIR-09). Metadata edits republish without changing room
  identity. Non-authorized users (rooms merely joined from the directory)
  cannot change directory visibility — the UI, the dialog, and the switch
  handler all reject them. See
  `docs/public-room-directory/visibility-switching.md`.
- Room directory visibility (BORU-DIR chain): room visibility model with
  `Private`, `PublicUnlisted`, and `PublicDiscoverable` states, persisted on
  room metadata (`ConversationEntry.visibility`). Only `PublicDiscoverable`
  rooms emit directory advertisements — the emit site refuses
  Private/PublicUnlisted with `AnnounceOutcome::NotDiscoverable`, and legacy
  public rooms are conservatively migrated to `PublicUnlisted` on startup so
  no existing room is unexpectedly exposed. See
  `docs/public-room-directory/room-visibility-state.md`.
- Create-room dialog (BORU-DIR-05): an explicit visibility picker
  (Private / Public-Unlisted / Public-Discoverable, defaulting to the
  conservative Public-Unlisted) plus optional description and tags fields
  with clear limits. Creator metadata is validated and normalized
  (`normalize_room_metadata`, reusing the BORU-DIR-02 bounds) before any
  broadcast; invalid/oversized input is rejected inline and never
  advertised. Unlisted rooms are created without any directory/DHT/broadcast
  side effects, and validated description/tags persist on the room entry for
  the later control-plane advertisement phase.
- File Sharing dashboard screen with five tabs — Shared by Me, Downloading,
  Downloaded, Shared with Me, and Activity Log — backed by live transfer
  projections and durable SQLite projections. Includes a per-tab search,
  per-tab sorting, the Open Downloads Folder action, share/revoke flows with
  inline destructive confirmation, and reference-aware cleanup. No messaging,
  networking, or command behaviour changed. See `docs/file-sharing-guide.md`.
- Multi-peer E2E test harness (`scripts/fs23_launch.sh`, `scripts/fs23_mcp.py`,
  `scripts/fs23_seed.py`) for deterministic two-peer smoke runs.
- FS-22 dashboard coverage suite (`tests/fs22_dashboard_coverage.rs`).

### Changed

- Storage schema now at v17: v16 adds `shared_files.version` and the
  `transfer_activity` event log; v17 adds the activity `direction` column.
  Migrations are forward-only and idempotent (see
  `docs/fs-06-persistence-projections.md`).
- Security hardening: expired permission grants are inert in all
  authorization loops, dashboard downloads route through a backend
  `validate_download_request` gate, and download writes use the
  `safe_destination_path` helper (see `docs/fs-20-security-review.md`).
- Redesigned the home and chat screens with a modern visual system: a
  cleaner sidebar, card-based home layout with quick actions and an
  activity rail, and a refreshed conversation view with grouped message
  bubbles and an elevated composer. The design uses centralized tokens
  (`design_tokens.rs`), shared components (`ui_components.rs`), and the
  Source Sans 3 / Raleway font pairing. No messaging, file-sharing,
  networking, or command behaviour changed.

## 0.102.0

Performance and transfer improvements from Phase 25, including improved file
indexing, hashing, outbox delivery, image optimization, persistence, and
release build profiles.

## 0.101.0

Latest release. See the git log for the full commit history.

### How changelog entries are added

Changelog entries are updated as part of the version-bump pull request.
Before merging a version bump, run:

```bash
git cliff --prepend CHANGELOG.md --tag v<new-version> --unreleased
```

This groups changes into:

- Added (features)
- Fixed (bug fixes)
- Performance
- Breaking changes

Internal changes (formatting, CI maintenance, dependency bumps) are skipped
unless they are user-facing.
