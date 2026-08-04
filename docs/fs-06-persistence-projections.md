# FS-06 / FS-25 — File Sharing dashboard: projection, subscription, persistence

This is the authoritative architecture note for the File Sharing dashboard's
data flow. It describes how the dashboard stays current (projection +
subscription), what is persisted and how (SQLite migrations, retention), and
the security boundaries around each layer.

The dashboard reads durable projections from SQLite and live in-memory state;
it does **not** own file bytes, authorization, descriptors, or transfer
execution.

## 1. Data-flow overview

```text
┌─ Sources of truth ──────────────────────────────────────────────────────┐
│                                                                         │
│  src/* protocol handlers          examples/iced_chat runtime            │
│  (catalogue, file-access,         (NetEvent loop, provider/serving      │
│   blob transfer, download         callbacks, download manager)          │
│   manager)                                                              │
│      │                                     │                            │
│      │ TransferLifecycleEvent              │ TransferEvent stream       │
│      │ (telemetry, diagnostics)            │ (transfer_state_projection)│
│      ▼                                     ▼                            │
│  SQLite (boru.db)                    in-memory TransferProjection       │
│  ┌─────────────────────┐             (per-transfer state machine)       │
│  │ shared_files        │                      │                         │
│  │ file_objects        │                      │ ProjectionUpdate        │
│  │ permissions         │                      ▼                         │
│  │ downloads           │              examples/iced_chat/app.rs         │
│  │ transfer_activity   │              inbound_active, downloaded_...    │
│  │ remote catalogue    │              shared_by_me_rows, activity_rows  │
│  └─────────────────────┘                      │                         │
│         ▲                                     │                         │
│         │ list_* projections                  │ subscribe() watch       │
│         │                                     ▼                         │
│  ┌──────────────────────────────────────────────────────────────────────┐│
│  │  dashboard_view_model.rs — presentation projections (no I/O)        ││
│  │  DashboardTab / SharedItem / TransferRecord / Progress / rows       ││
│  │  ← fed by refresh_* tasks + live TransferProjection updates        ││
│  └──────────────────────────────────────────────────────────────────────┘│
│         │                                                               │
│         ▼                                                               │
│  File Sharing screen (shared_by_me_table.rs, downloading_...rs,        │
│  downloaded_...rs, peers_downloading_..., sharing_summary.rs,           │
│  recent activity card)                                                  │
└─────────────────────────────────────────────────────────────────────────┘
```

Layering rules:

1. **Storage** (`src/storage.rs`) is the only owner of durable rows. The GUI
   calls `list_*` / `get_*` projections; it never writes SQL directly.
2. **`transfer_state_projection.rs`** folds the live `TransferEvent` stream
   into per-transfer `TransferRecord`s (state, bytes, total, peer) and emits
   `ProjectionUpdate`s. It is pure in-memory — nothing is persisted here.
3. **`dashboard_view_model.rs`** is a pure presentation layer: it converts
   authoritative domain records into stable UI-facing values and owns no
   storage, networking, or widget state.
4. **The iced app** subscribes to the projection (`Diagnostics::subscribe`
   watch channel and the transfer-event channel) and re-renders the active
   tab's rows from the in-memory buffers.

## 2. Subscription flow (live updates)

- The GUI runtime loop consumes network/provider events (`NetEvent`,
  `ProviderMessage`, download-manager callbacks) and translates them into
  `TransferEvent`s fed to `TransferProjection::apply`.
- `TransferProjection` maintains one `TransferRecord` per `transfer_id`
  (state, bytes, total_bytes, peer_id, progress interval) and produces
  `ProjectionUpdate`s for state transitions, progress checkpoints, and
  terminal states.
- The dashboard's Downloading tab renders directly from
  `app.inbound_active` (a `HashMap<String, TransferRecord>`); peers-
  downloading-from-me renders from the outbound serve side of the same
  projection.
- Disconnects are folded via `disconnect_peer(peer_id, …)`, which transitions
  affected rows to `Disconnected` (non-terminal, so a reconnect can resume).

## 3. Persistence flow (durable projections)

The dashboard persists only what the design requires:

- **`shared_files`** — local offers (offered flag, version, source availability)
  and, for remote peers, the complete validated catalogue snapshot
  (`replace_remote_catalogue`). Local metadata upserts increment the version;
  catalogue adapters expose it as the descriptor version.
- **`file_objects`** — content-addressed records (BLAKE3 hash, size, MIME,
  filename); `source_path` is stored locally only and is never a wire field.
- **`permissions`** — per-recipient read grants/denies with optional
  `expires_at_ms`.
- **`downloads`** — durable rows for the download state machine
  (`queued` → `resolving_peer` → `requesting_permission` → `downloading` →
  `verifying` → `complete`/`failed`/`cancelled`), with `remote_peer` as the
  source peer key.
- **`transfer_activity`** — the allow-listed lifecycle event log (see §5).

### Schema and migration

`CURRENT_SCHEMA_VERSION = 17` (`src/storage.rs`). The forward-only migration
loop is versioned; a database with a higher version than the binary supports
is refused (see `docs/troubleshooting.md`).

- **v16** adds `shared_files.version` (default `1`; local upserts increment
  it, catalogue adapters expose it as descriptor version) and the
  `transfer_activity` table, keyed by lifecycle `event_id`, uniquely
  constrained by `(transfer_id, sequence)`. Replayed events are ignored
  (`INSERT OR IGNORE`).
- **v17** adds `transfer_activity.direction` (`'inbound'` default, or
  `'outbound'`), so the Activity Log can distinguish served uploads from
  received downloads.

Migration v16 is additive and runs in the existing per-version migration loop.
Column additions use an existence check (`add_column_if_missing`) so a
partially-applied legacy migration can be safely resumed. Existing
`group_invites` v14/v15 column migrations use the same idempotent helper. No
data reset or file-byte copy is performed.

## 4. Retention and cleanup

- `list_transfer_activity` is bounded to **1,000 rows** and returns newest
  activity first.
- Callers should periodically call `prune_transfer_activity(cutoff_ms)` with
  their chosen retention cutoff; rows older than the timestamp are deleted.
- Deleting a shared offer removes its authorization grants in the same SQLite
  transaction. It intentionally does **not** delete `downloads`: queued or
  active transfers remain authoritative in the download state machine and can
  finish or transition according to its existing revocation semantics.
- File objects are removed only by existing reference-aware cleanup once no
  attachments, shares, permissions, collections, or downloads reference them.

## 5. Security boundaries

| Layer | Boundary |
|---|---|
| Wire / protocol | Remote catalogue entries are signature- and metadata-validated before render (`RemoteSharedFile::validate` rejects separators, control chars, over-long names, bad MIME). Descriptors are recipient-bound, expiring, signed; download permission is re-checked at request time by the backend (`FileAccessHandler::check_permission`). |
| Activity log | `record_transfer_activity` stores only an allow-listed payload (`sanitize_activity_payload`). Paths, tokens, descriptors, hashes, and arbitrary payload keys are discarded at write time. |
| Download write site | Completed downloads are verified (exact size + streaming BLAKE3) then renamed into place via `safe_destination_path` (strip separators, reject traversal, dedupe collisions, hash fallback stem). |
| UI rendering | `dashboard_view_model.rs` renders display labels only. Local paths are never rendered in remote rows; peer identity for outbound rows comes from the authenticated QUIC connection (`endpoint_id`), never a display string. |
| Expiry | Expired permission grants are inert in every in-memory authorization loop (storage, catalogue handler, file-access handler) and at request time. |

See `docs/fs-20-security-review.md` for the full hardening pass, findings, and
residual limitations.

## 6. Cross-platform desktop integration

- **File selection**: native OS picker only (`rfd::AsyncFileDialog`). No
  in-app file browser. On Linux the picker is backed by `xdg-desktop-portal`
  (GTK); the FS-23 harness shows the required portal + D-Bus activation
  environment for headless/Xvfb runs.
- **Downloads folder**: `Open Downloads Folder` delegates to the OS opener
  (`open::that`) on the `<data-dir>/downloads` directory, which is created at
  startup. Works on Linux (xdg-open), macOS (open), and Windows.
- **File reveal**: "Open containing folder" reveals the file in the OS file
  manager (`open -R` on macOS, `explorer /select,` on Windows, parent-folder
  open on Linux).
