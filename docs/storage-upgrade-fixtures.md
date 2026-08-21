# Cross-version storage upgrade fixtures

`tests/fixtures/storage-upgrade/` contains compact, deterministic SQLite inputs for
three schema families:

- `v1`: reduced from the historical `v0.103.0` `src/storage.rs` migration v1.
- `v13`: reduced from historical `v0.108.0`, immediately before v14's additive
  group-invite columns.
- `v23`: the current repository's pre-v24 shape, so v24-v26 are exercised as a
  pending/interrupted-upgrade sequence.

Each fixture has `fixture.sql` and `manifest.json`. The SQL contains only
synthetic IDs and message/file metadata; manifests contain no secret keys,
private keys, host paths, or real profile data. The integration harness creates
a temporary profile, loads the SQL, opens it with current `Storage`, verifies
schema version, semantic counts/hash values and SQLite integrity, closes and
reopens it, then checks the manifest remains unchanged.

Run the targeted matrix with:

```sh
rb test --test test_storage_upgrade_fixtures
```

The future-schema test first takes a database backup, injects
`CURRENT_SCHEMA_VERSION + 1`, asserts the explicit non-destructive rejection,
then restores and successfully reopens the backup. A failed open does not
rewrite the future database. A fixture that stops at v23 provides the supported
interrupted-migration/restart coverage: current open applies v24-v26 and a
second open is idempotent.
