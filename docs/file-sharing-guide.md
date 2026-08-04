# File Sharing — User Guide

This guide describes the Boru File Sharing screen as implemented by the FS
epic (FS-03 route … FS-24 visual QA). It covers what each dashboard tab means,
how the native OS file picker works, where downloads land, and what sharing
and revocation semantics you can rely on.

## Opening the screen

Select **Files** in the sidebar. The File Sharing screen opens with the
**Shared by Me** tab active. The sidebar, network connections, gossip
subscriptions, and conversation forwarders keep running — only the main panel
swaps.

## The five tabs

| Tab | What it shows |
|-----|---------------|
| **Shared by Me** | Files this node has registered for sharing (its catalogue entries). Each row shows the file type icon, display name, MIME type, size, updated timestamp, recipient permission chips, download counts, and an action menu (Stop sharing, View details). |
| **Downloading** | Inbound transfers currently in progress, with live progress bars, byte counts, source peer, and state (active / verifying / failed / cancelled / disconnected). |
| **Downloaded** | Completed downloads, sorted by recency, with display name, size, and source peer label. Rows offer Open file, Open containing folder, and Remove from list. |
| **Shared with Me** | Files peers have made available to this node — the combined, validated remote catalogue. Rows offer Download and View peer catalogue. |
| **Activity Log** | A chronological stream of file-sharing lifecycle events (requested, authorized, started, downloaded, uploaded, failed, cancelled, denied). Read-only. |

The search box in the header filters across the dashboard; the active tab's
rows, the recent activity card, and the summary metrics all respond to the
same query. Sorting controls are per-tab.

## Native OS file picker

Boru deliberately uses the **native OS file picker** (`rfd::AsyncFileDialog`)
to select files for sharing — there is no in-app file browser. On Linux the
picker is provided by `xdg-desktop-portal` (GTK); on macOS and Windows it is
the platform file dialog.

- The picker returns a path; Boru never asks for a file by typing a path
  manually in the GUI.
- Selected files are content-hashed (BLAKE3) and stored in the local file
  library; the catalogue advertises only display-safe metadata (name, size,
  MIME, hash, version) — never local paths.
- When the portal is unavailable (headless or minimal desktop), the picker
  may fail to appear; see [Troubleshooting](#troubleshooting).

## Download folder action

The **Open Downloads Folder** button in the header opens the Boru downloads
directory in the OS file manager. The directory is `<data-dir>/downloads`
(created automatically at startup). Completed downloads are verified (exact
size + streaming BLAKE3) before they are renamed into place, so files listed
as Downloaded have already passed integrity checks.

## Sharing semantics

- Registering a file creates a `shared_files` row (offered) plus a
  content-addressed `file_objects` record. Peers see the offer through the
  signed, requester-filtered catalogue.
- Permission model: a peer can download when it is a friend, or when an
  explicit read grant covers it, and when no deny grant applies. Grants are
  per-recipient and can carry an expiry (`expires_at_ms`).
- **Expired grants are inert**: an expired read grant no longer authorizes a
  download at request time; an expired deny lapses and does not block a
  currently-valid grant.
- Download initiation always re-checks the backend (verified catalogue
  precondition + file metadata + permission) — the UI cannot start a transfer
  the backend would refuse.
- Every download completes only after size and BLAKE3 verification; the file
  is written through a safe destination path (no traversal, no raw
  remote-controlled names, collision-safe).

## Revocation semantics

| Action | What happens |
|--------|--------------|
| **Stop sharing** (Shared by Me row menu) | Two-step destructive action: the first press shows an inline confirmation banner; the second press deletes the shared offer. Removing the offer also removes its authorization grants in the same SQLite transaction. Existing downloads are *not* aborted — queued/active transfers remain authoritative in the download state machine and finish or transition per its own revocation semantics. |
| **Revoke access** (recipient chip / details) | Removes the read grant for one recipient. The recipient immediately loses catalogue visibility for that file; future download attempts are refused by the backend permission check. |
| **Cancel download** (Downloading tab) | Marks the in-flight transfer cancelled; the temporary file is cleaned up. |

Removing a share never deletes the underlying file object while other
attachments, shares, permissions, collections, or downloads still reference it
(reference-aware cleanup).

## Privacy properties

- The dashboard renders display labels only. Local paths are never sent on
  the wire and never rendered in remote rows.
- Peer identity for "who is downloading" comes from the authenticated QUIC
  connection, not a display string.
- The durable activity log stores an allow-listed payload only; paths,
  tokens, descriptors, hashes, and arbitrary payload keys are discarded at
  write time.

## Troubleshooting

### The file picker never appears (Linux)

The native picker is provided by `xdg-desktop-portal`. Check:

```sh
ps aux | grep -E "xdg-desktop-portal"
```

and that a portal backend (e.g. `xdg-desktop-portal-gtk`) is running. In a
bare Xvfb/session environment the portal must be started explicitly and the
D-Bus activation environment must carry `DISPLAY` — this is exactly what the
FS-23 test harness (`scripts/fs23_launch.sh`) does.

### "Could not open downloads folder"

The `open` (OS open-command) call failed, or the downloads directory could
not be created. Check permissions on `<data-dir>` and that a file manager is
installed.

### A download stays in "Downloading" without progress

- The source peer may have gone offline — the row transitions to
  *disconnected* once the transfer/chunk timeout elapses.
- The owner may have revoked access mid-transfer — the backend refuses the
  permission re-check and the row fails with a permission error.
- The descriptor may have expired (issued/expiry TTL) — the client re-requests
  a fresh descriptor.

### A peer cannot see a file I shared

- Confirm the file row is in **Shared by Me** and marked offered/available.
- Confirm the peer is a friend (or covered by an explicit read grant) and is
  not blocked.
- Remember the catalogue is requester-filtered and signed per requester — a
  peer only sees files it is allowed to see, and only after a fresh catalogue
  fetch.

### "The owner may have revoked access or blocked your account" on download

The request-time permission check refused the transfer. Either the owner
revoked access, blocked you, or the grant expired. Ask the owner to re-grant
or re-share.

### Activity Log is empty

The log records transfer lifecycle events only — it is not a general event
feed. If no transfers have been attempted since upgrade, the log is empty by
design. `list_transfer_activity` is bounded to the newest 1,000 rows.

### Files vanish from Shared by Me after restart

Shared offers persist in SQLite; a row disappears only if it was removed or
if its source file is no longer available (the row is filtered when
`source_available` is false — the file was moved/deleted on disk, or the
referenced path is missing).
