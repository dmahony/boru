# Privacy model

## Scope

This document describes what data Boru exposes to remote peers and what
protections apply. It covers catalogue visibility, metadata exposure,
local-path protection, and the privacy boundaries of the storage system.

---

## Metadata exposed to peers

A profile advertises signed, requester-filtered metadata through the catalogue
protocol. A visible `RemoteSharedFile` contains a stable shared-file ID,
display name, optional description, MIME type, size, content hash,
version/update metadata, and collection IDs. The catalogue does **not** contain:

- Local filesystem paths
- Database row IDs
- Permission rows
- Upload secrets or blob tickets
- Unrestricted addresses

Filtering is per authenticated requester. Blocked peers are denied; friends or
explicitly granted peers see only enabled, available entries permitted for that
peer. A non-friend in the default contacts-only mode receives an empty view.
A catalogue cache is a local projection and is not shared back to the owner or
other peers.

---

## Local-path protection

Local `SharedFile` paths are never serialized as remote metadata. Conversion to
`RemoteSharedFile` rejects absolute paths and paths containing parent-directory
components. Remote peers therefore learn the safe display filename, not the
source path or local directory layout. Local-path privacy does not prevent the
local owner from seeing its own path in its own database/UI.

---

## Authorization privacy

Visibility does not disclose a download grant. Download authorization is
re-evaluated at request time, and refusal responses use structured/generic
errors where appropriate rather than exposing whether an inaccessible object
exists. The signed descriptor identifies the authorized requester and expires
after 60 seconds; it is not a permanent capability.

---

## Cache and offline behavior

Verified catalogue data may remain in the local SQLite cache for offline
display. Cached metadata can be stale and must not be used as permission proof.
A live `/boru-file-access/1` request is required before bytes are transferred,
even when the catalogue was previously cached.

---

## Storage privacy

### At rest

- **SQLite database (`boru.db`)** — not encrypted at the file level.
  Transport-layer encryption (QUIC/TLS 1.3) protects messages in flight, but
  at rest the storage depends on filesystem permissions (`0o600`).
- **DM message plaintext** — decrypted message content is stored in the
  `dm_messages.plaintext` column. This is plaintext at rest.
- **Inbox ciphertext** — encrypted by the sender, but the encrypted byte
  payload is readable by anyone with filesystem access to the database.
- **File objects** — user-uploaded images may be stored as raw bytes in the
  `file_objects.data` column or referenced by iroh-blobs hash.

### In transit

Iroh/QUIC protects bytes in transit using TLS 1.3. Iroh-blobs and the receiver
use content hashes for addressing and verification.

---

## Data not encrypted at rest

The following data is stored as plaintext in the SQLite database or on disk:

| Data | Location | Notes |
|---|---|---|
| DM message content | `dm_messages.plaintext` | Decrypted message text |
| File display names | `shared_files.display_filename` | Safe display names shared with peers |
| Shared file metadata | Various tables | Description, version, timestamps |
| Peer identities | `contacts`, `friends` tables | Public keys, endpoint addresses |
| Conversation metadata | `conversation_meta`, `dm_conversations` | Conversation IDs, activity timestamps |
| User settings | `settings.json` | Theme, UI preferences |
| Node secret key | `secret_key.txt` | Plaintext hex — must be protected by `0o600` permissions |

See [`security-model.md`](security-model.md) for the cryptographic security
properties and [`message-storage-design.md`](message-storage-design.md) for
the full storage architecture.
