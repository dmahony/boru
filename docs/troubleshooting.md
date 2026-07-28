# Troubleshooting Guide

## Storage and Migration

### "Database has schema version X, but this application only supports up to Y"

You have a `boru.db` created by a newer version of the application. The
forward-only migration system prevents data loss by refusing to open a
database from the future.

**Causes:**
- You downgraded the application after running a newer version
- You copied a `boru.db` from another installation that was newer
- You restored a backup from a newer version

**Solutions:**
1. **Upgrade** the application to match the database version
2. **Restore from backup** — replace `boru.db` with a copy from before
   the newer version was run
3. **Start fresh** — delete `boru.db` (and its WAL/SHM files) and restart.
   The application creates a new empty database.

> ⚠️ **Do not** modify the `schema_version` table manually. The forward-only
> guard exists to prevent silent data loss. Forcing a version number change
> can corrupt the database.

### "Database integrity check failed"

SQLite's `PRAGMA integrity_check` detected corruption in `boru.db`.

**Causes:**
- Filesystem corruption
- Partial write during a crash (rare with WAL mode, possible with disk errors)
- Copying the database file while the application is running
- Disk or hardware failure

**Solutions:**

1. **Restore from backup** (recommended):
   ```sh
   cp ~/.local/share/boru/boru.db.backup ~/.local/share/boru/boru.db
   ```

2. **Recover via SQLite** (may lose some data):
   ```sh
   # Stop the application first
   sqlite3 ~/.local/share/boru/boru.db ".recover" | \
     sqlite3 ~/.local/share/boru/boru-recovered.db
   mv ~/.local/share/boru/boru.db ~/.local/share/boru/boru.db.corrupt
   mv ~/.local/share/boru/boru-recovered.db ~/.local/share/boru/boru.db
   ```

3. **Start fresh** — delete the corrupted database. The application creates
   a new empty one on next launch. All chat history, files, and settings
   stored in SQLite are lost.

### "database is locked" errors

SQLite returns `SQLITE_BUSY` when another connection holds a write lock.

**Causes:**
- Another Boru process is running with the same data directory
- A previous process crashed without releasing the WAL lock
- You are running `sqlite3` CLI on the database while the app is running

**Solutions:**
- Check for stale processes:
  ```sh
  lsof ~/.local/share/boru/boru.db
  ps aux | grep boru
  ```
- Kill any stale Boru processes and restart
- If WAL files are stale, clean them:
  ```sh
  rm -f ~/.local/share/boru/boru.db-wal ~/.local/share/boru/boru.db-shm
  ```
  (This is safe if no Boru process is running — WAL and SHM are temporary
  journal files that SQLite regenerates automatically.)

---

## Delivery Issues

### Messages not being delivered

**Causes:**
- Peer is offline (message is queued)
- Network connectivity issue (relay unreachable, DHT down)
- Peer has blocked you or removed you as a friend
- Schema mismatch between versions

**Check:**
1. Is the peer showing as online in the UI? (green dot in the FRIENDS list)
2. Are you connected to a relay? Check the Settings → Network section.
3. Are both instances using compatible application versions?
4. Check the logs for delivery-related errors:
   ```sh
   grep -i "delivery\|outbox\|retry\|ack" ~/.local/share/boru/logs/*.log
   ```

**Solutions:**
- Wait for automatic retry (exponential backoff: 5s → 10s → 20s → ... → 180s max)
- If the peer reconnects, pending messages are retried automatically
- Restart both instances to clear transient network state
- Check that both instances use the same relay URL

### Messages stuck in "Sending" or "Queued"

**Causes:**
- A delivery worker crashed without releasing its outbox lease
- The peer is unreachable but no retry has been scheduled yet

**Solutions:**
- Restart the application. Crash recovery on `Storage::open()` resets
  `Sending` rows back to `Pending` and clears stale worker leases.
- If a restart doesn't help, check the database directly:
  ```sh
  sqlite3 ~/.local/share/boru/boru.db "SELECT msg_id, status, attempts, \
    last_error_code FROM outbox WHERE status != 2;"
  ```
- Messages in `Sent` (1) status are awaiting an ACK. If the peer received
  the message but the ACK was lost, the message may be delivered twice
  (at-least-once transport — ACK-based dedup at the recipient prevents
  duplicate storage).

### Duplicate messages appearing

**Short term:** The at-least-once transport can deliver a message twice if
the ACK is lost. The recipient's deduplication (`INSERT … ON CONFLICT DO
NOTHING`) prevents duplicate storage, but the sender may retransmit if the
ACK doesn't arrive. A duplicate will appear briefly in the sender's UI.

**Long term:** If duplicates persist in the chat log, the deduplication
logic in the inbox handler may not be matching message IDs correctly.
Check the logs for `incoming_message_result` entries.

---

## Crash Recovery

### After a crash, are messages lost?

**No.** Boru is designed to survive crashes without losing messages:

1. **WAL journal mode** — SQLite writes are crash-safe
2. **Outbox recovery** — `Sent` and `Sending` rows are reset to `Pending`
   on the next `Storage::open()`
3. **Stale lease cleanup** — worker leases that expired during the crash
   are cleared so other workers can claim those rows
4. **Exactly-once inbox** — received messages are stored with
   `ON CONFLICT DO NOTHING` — crashes during insert don't lose data
5. **Tombstones persisted** — deleted messages remain deleted across
   restarts

Every `Storage::open()` runs `recover_crash_state()`, which performs four
recovery passes:

```
1. Sent → Pending    (crash-left outbound rows)
2. Sending → Pending (crash-left in-flight rows)
3. Stale timestamps  (future next_attempt_at → now)
4. Stale leases      (expired worker locks → cleared)
```

### After a crash, settings are reset

Settings (`theme`, UI preferences) are stored in `settings.json`, which
uses atomic writes (write to temp file + rename). This is crash-safe for
the settings file itself. If settings are reset, the `settings.json` file
may have been corrupted or deleted.

**Solution:** The application loads default settings if the file is missing.
Settings are not yet stored in SQLite, so there is no migration path.

---

## Diagnostics

### Enabling diagnostic logging

```sh
# Run with RUST_LOG for detailed output
RUST_LOG=boru_core=debug cargo run --example iced_chat --features gui -- --name <nickname>

# Capture all logs including storage and delivery
RUST_LOG=boru_core=trace cargo run --example iced_chat --features gui -- --name <nickname>
```

### Checking database state

```sh
# Schema version
sqlite3 ~/.local/share/boru/boru.db "SELECT MAX(version) FROM schema_version;"

# Inbox message count
sqlite3 ~/.local/share/boru/boru.db "SELECT COUNT(*) FROM inbox;"

# Outbox state breakdown
sqlite3 ~/.local/share/boru/boru.db \
  "SELECT status, COUNT(*) FROM outbox GROUP BY status;"

# Tombstone count
sqlite3 ~/.local/share/boru/boru.db "SELECT COUNT(*) FROM message_tombstones;"

# DM message count
sqlite3 ~/.local/share/boru/boru.db "SELECT COUNT(*) FROM dm_messages;"

# Stuck outbox rows (non-acked, non-expired)
sqlite3 ~/.local/share/boru/boru.db \
  "SELECT msg_id, status, attempts, last_error_code, next_attempt_at_ms \
   FROM outbox WHERE status NOT IN (2, 3);"
```

### Checking for BORU CRASH events

The application logs a `BORU CRASH` line when it detects an inconsistency
that could indicate a crash-related issue:

```sh
grep "BORU CRASH" ~/.local/share/boru/logs/*.log
```

---

## Known Limitations

| Issue | Status | Workaround |
|---|---|---|
| No SQLite file-level encryption | Won't fix (by design) | Use LUKS/eCryptfs/dm-crypt at the filesystem level |
| DM plaintext stored unencrypted in `dm_messages` table | Won't fix (by design) | Filesystem-level encryption |
| Secret key in plaintext file | Won't fix (by design) | Filesystem permissions (`0o600`) |
| Tombstones accumulate indefinitely | Known issue | Future TTL-based pruning (tracked) |
| GUI offline DM fallback not wired to SQLite retry worker | Known issue | Whisper DMs work when peer is online; offline fallback is best-effort |
| JSON `save()` deprecation warnings appear in logs | By design | Expected until legacy stores are fully removed |
