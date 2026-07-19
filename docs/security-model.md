# File-sharing security model

Remote file sharing separates signed metadata, authorization, transport, and byte integrity.

## Catalogue trust

The owner signs each requester-specific `SignedFileCatalogue` with its identity key. The signed payload includes the owner, revision, generation time, collections, and every advertised file field. A client validates metadata and the signature, then requires `owner_id` to match the authenticated QUIC peer. Tampering with the revision, owner, collection, file metadata, or signature is rejected.

A catalogue is an advertisement only. It is not a bearer capability, and a cached entry does not authorize a download.

## Request-time authorization

Every download sends a fresh `FileAccessRequest` over `/boru-file-access/1`. The owner re-checks the live relationship, per-file grants and denials, offer status, file availability, expected content hash, expected size, and expected version. A stale catalogue therefore cannot preserve access after a permission or file change.

A successful response is an owner-signed `SignedDownloadDescriptor` bound to the owner, requester, shared-file ID, content/blob hash, size, timestamps, and random nonce. The default lifetime is 60 seconds. Descriptors are intended for one use; the shared nonce store rejects replay while the descriptor is valid. The requester checks signature, owner, requester, lifetime, hash, and size before transfer.

## Transport and content integrity

Iroh/QUIC encrypts traffic in transit. The file-access protocol grants access but carries no file bytes; iroh-blobs transfers the content-addressed bytes. The receiver writes temporary output, checks exact size and BLAKE3 content hash, and atomically installs only verified output. A descriptor signature authenticates authorization metadata; it is not a separate signature over the file bytes.

## Resource exhaustion controls

Catalogue payload/count limits, file-access deadlines and preparation limits, upload queue/concurrency limits, download queue/concurrency limits, hash-verification limits, and blob transfer timeouts bound work derived from peer input. See [`catalogue-limits.md`](catalogue-limits.md) and [`remote-file-sharing.md`](remote-file-sharing.md#resource-limits).

## What this model does not claim

The database is not encrypted at the SQLite file level. Files imported into local storage may be readable by a local filesystem user with access to the data directory. Network encryption and signatures do not protect a compromised local host.
