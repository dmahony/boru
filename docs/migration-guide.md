# Migration Guide: Legacy JSON → SQLite Storage

## Overview

Starting from Phase 22, Boru uses **SQLite (`boru.db`)** as its authoritative
persistence backend. Legacy JSON stores (`chat_history.json`, `outbox.json`,
`friends.json`, etc.) are **read-only** — their `save()` methods are no-ops
that log deprecation warnings.

This guide explains how existing installations migrate their data, what to
expect, and what to do if something goes wrong.

---

## What changed

| Aspect | Before | After |
|---|---|---|
| Authoritative store | Mixed JSON + SQLite | SQLite (`boru.db`) only |
| JSON `save()` methods | Active writes | No-ops (deprecated) |
| GUI outgoing queue | `outbox.json` | SQLite `outgoing_messages` table (V10) |
| `PersistenceCoordinator` | Managed periodic flushes | Removed |
| Schema version | V2 | V19 |
| `UserProfile` | JSON — writes disabled | JSON — still active (no SQLite equivalent) |
| `AppSettings` | JSON — active | JSON — still active (no SQLite equivalent) |

---

## Before you migrate

### Check your data directory

Locate your Boru data directory. The application resolves it in this order:

1. `--data-dir` CLI flag
2. `BORU_DATA_DIR` environment variable
3. `$XDG_DATA_HOME/boru` (typically `~/.local/share/boru/`)
4. `$PWD/.boru`

Inside the data directory, you should see:

```
<data_dir>/
├── message_store.db    # Legacy SQLite V1 (migration source)
├── chat_history.json   # Legacy JSON stores
├── outbox.json
├── friends.json
├── friend_requests.json
├── conversations.json
├── rooms.json
├── mailbox.json
├── settings.json
├── user_profile.json
├── secret_key.txt
└── files/              # Image store (unchanged)
```

### Backup your data

Before running the new application version:

```sh
# Backup the entire data directory
cp -a ~/.local/share/boru ~/.local/share/boru.backup-$(date +%Y%m%d)

# Or just the database files
cp ~/.local/share/boru/boru.db ~/.local/share/boru/boru.db.pre-migrate
cp ~/.local/share/boru/message_store.db ~/.local/share/boru/message_store.db.pre-migrate
```

---

## The migration process

Migration happens automatically on the first `Storage::open()` call. The
process has two phases:

### Phase 1: Legacy SQLite import

If a `message_store.db` file exists alongside `boru.db` (and `boru.db` has
no inbox/outbox data yet), `Storage::import_legacy_db()` copies:

- `inbox` rows → new `inbox` table
- `outbox` rows → new `outbox` table
- `contacts` rows → new `contacts` table
- `sync_cursor` rows → new `sync_cursor` table

The import is **idempotent** — `INSERT OR IGNORE` prevents duplicates. If the
migration is interrupted and restarted, already-copied rows are skipped.

### Phase 2: Schema migration

The `run_migrations()` function applies all unapplied schema versions in
sequence. Each migration runs in its own transaction and is recorded in the
`schema_version` table. If a migration crashes mid-way, the next `open()`
re-runs only the unapplied versions.

The application starts at V1 and migrates forward through V2, V3, ..., V19.
See [`message-storage-design.md`](message-storage-design.md) for the full
schema history.

### What happens to JSON files

Legacy JSON files (`chat_history.json`, `outbox.json`, etc.) remain on disk.
They are **not deleted** — existing code paths that read from them continue
to work. New writes silently go to SQLite; the JSON `save()` methods log a
deprecation warning and return without writing.

---

## Verifying migration

After the first run with the new version:

1. Check that `boru.db` exists in your data directory
2. Check that `boru.db` has the expected schema version:
   ```sh
   sqlite3 ~/.local/share/boru/boru.db "SELECT MAX(version) FROM schema_version;"
   # Expected: 10
   ```
3. Check that your conversations, messages, and friends are still visible
   in the GUI
4. Check the application logs for deprecation warnings from JSON `save()` calls

---

## What to do if migration fails

### Error: "Database has schema version X..."

This means `boru.db` was created by a **newer** version of the application
than the one you are running. The forward-only migration system refuses to
open a database from the future to prevent data loss.

**Solution:** Upgrade your application to the version that created the
database, or restore from a backup.

### Error: "Database integrity check failed: ..."

SQLite detected corruption in `boru.db`. The database is never silently
repaired — this would risk data loss.

**Solution:**

1. Restore from backup: `cp ~/.local/share/boru/boru.db.pre-migrate ~/.local/share/boru/boru.db`
2. If no backup exists, try SQLite's recovery tools:
   ```sh
   sqlite3 ~/.local/share/boru/boru.db ".recover" | sqlite3 ~/.local/share/boru/boru-recovered.db
   mv ~/.local/share/boru/boru.db ~/.local/share/boru/boru.db.corrupt
   mv ~/.local/share/boru/boru-recovered.db ~/.local/share/boru/boru.db
   ```
3. If recovery fails, delete `boru.db` and restart (the application will
   create a fresh database). You will lose all persisted state.

### Error: "cannot open database" / "database is locked"

Another process may have the database open. Boru uses WAL mode with
`busy_timeout=5000` — wait 5 seconds and retry. If the error persists,
check for stale processes:

```sh
lsof ~/.local/share/boru/boru.db
```

### Migration appears stuck on "applying migration"

Check the application log for errors. If a specific migration step fails:

1. The error message includes the migration version number
2. Restart the application — it will retry only the failed migration
3. If it keeps failing on the same step, report the error with the
   migration version and the error message

### Loss of data after migration

If data seems missing after migration:

1. Check that `boru.db` contains the data:
   ```sh
   sqlite3 ~/.local/share/boru/boru.db "SELECT COUNT(*) FROM inbox;"
   sqlite3 ~/.local/share/boru/boru.db "SELECT COUNT(*) FROM outbox;"
   ```
2. Compare with legacy stores (if they still exist):
   ```sh
   sqlite3 ~/.local/share/boru/message_store.db "SELECT COUNT(*) FROM inbox;"
   ```
3. If SQLite is empty but the legacy store has data, the import may not have
   run. Check logs for `import_legacy_db` messages.
4. If the data is in SQLite but not visible in the GUI, it may be a GUI
   integration issue rather than a storage problem.

---

## Rolling back

**There is no automatic downgrade path.** Schema migrations are forward-only.
If you need to revert to an older version:

1. Restore from backup (see "Backup your data" above)
2. Delete any `boru.db` created by the newer version:
   ```sh
   rm ~/.local/share/boru/boru.db
   rm -rf ~/.local/share/boru/boru.db-wal ~/.local/share/boru/boru.db-shm
   ```
3. Start the old application version

---

## Long-term considerations

- **JSON files can be deleted** — once you have confirmed the migration is
  complete and stable, legacy JSON files are safe to remove. The application
  will recreate empty JSON stores on next startup (they load as empty and
  never write to disk).
- **Back up regularly** — `boru.db` is a standard SQLite file. Use `VACUUM INTO`
  or `.backup` for consistent checkpoints.
- **UserProfile retains JSON** — display name and sharing settings remain in
  `user_profile.json`. This file is still actively written.
- **AppSettings retains JSON** — UI preferences remain in `settings.json`.
  This file is still actively written.
