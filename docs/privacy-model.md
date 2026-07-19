# File-sharing privacy model

## Metadata exposed to peers

A profile advertises signed, requester-filtered metadata through the catalogue protocol. A visible `RemoteSharedFile` contains a stable shared-file ID, display name, optional description, MIME type, size, content hash, version/update metadata, and collection IDs. The catalogue does not contain local filesystem paths, database row IDs, permission rows, upload secrets, blob tickets, or unrestricted addresses.

Filtering is per authenticated requester. Blocked peers are denied; friends or explicitly granted peers see only enabled, available entries permitted for that peer. A non-friend in the default contacts-only mode receives an empty view. A catalogue cache is a local projection and is not shared back to the owner or other peers.

## Local-path protection

Local `SharedFile` paths are never serialized as remote metadata. Conversion to `RemoteSharedFile` rejects absolute paths and paths containing parent-directory components. Remote peers therefore learn the safe display filename, not the source path or local directory layout. Local-path privacy does not prevent the local owner from seeing its own path in its own database/UI.

## Authorization privacy

Visibility does not disclose a download grant. Download authorization is re-evaluated at request time, and refusal responses use structured/generic errors where appropriate rather than exposing whether an inaccessible object exists. The signed descriptor identifies the authorized requester and expires after 60 seconds; it is not a permanent capability.

## Cache and offline behavior

Verified catalogue data may remain in the local SQLite cache for offline display. Cached metadata can be stale and must not be used as permission proof. A live `/boru-file-access/1` request is required before bytes are transferred, even when the catalogue was previously cached.

## Byte and storage privacy

Iroh/QUIC protects bytes in transit. Iroh-blobs and the receiver use content hashes for addressing and verification. Local SQLite storage is protected by filesystem permissions where configured, but the database is not encrypted at the file level; local file data may be plaintext. See [`message-storage-design.md`](message-storage-design.md).
