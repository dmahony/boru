# Security model

## Scope

This document describes the cryptographic and storage security properties of
the Boru chat application. It does not cover host-level security (OS hardening,
filesystem encryption, physical access control) beyond the application's own
enforcement.

---

## Transport-layer security

All peer-to-peer communication uses **iroh's QUIC transport with TLS 1.3**.
Every connection is mutually authenticated: each peer proves its Ed25519
identity key during the QUIC handshake. Message bodies, metadata, and protocol
framing are encrypted in transit. An attacker on the network path cannot read,
modify, or inject messages between two directly connected peers.

---

## Storage at rest

The SQLite database (`boru.db`) and all legacy JSON files are stored **without
file-level encryption**. The application relies on filesystem permissions:

- Data directory: `0o700` (owner-only)
- Database file: `0o600` (owner-only)

Specific data sensitivity:

| Data | Location | At-rest protection |
|---|---|---|
| Inbox ciphertext | `boru.db` / `inbox.ciphertext` | Opaque ciphertext blob — encrypted by sender, cannot be decrypted without recipient's key; byte payload is readable by filesystem user |
| Outbox data | `boru.db` / `outbox` rows | Same as inbox — ciphertext blobs |
| DM messages | `boru.db` / `dm_messages.plaintext` | **No encryption** — decrypted plaintext stored at rest |
| File objects | `boru.db` / `file_objects.data` | Raw bytes — plaintext for uploaded images; ciphertext for encrypted payloads |
| Secret key | `secret_key.txt` | Plaintext hex-encoded Ed25519 secret key — must be protected by filesystem permissions |
| Transport | Wire (QUIC) | TLS 1.3 — always encrypted in flight |

**Do not overstate:** The database is not encrypted at the file level. A
compromised local host, a misconfigured backup, or a process that leaks
the database path can expose all stored data. For full at-rest protection,
pair Boru with filesystem-level encryption (e.g. LUKS, eCryptfs).

---

## File-sharing security

### Catalogue trust

The owner signs each requester-specific `SignedFileCatalogue` with its
identity key. The signed payload includes the owner, revision, generation
time, collections, and every advertised file field. A client validates
metadata and the signature, then requires `owner_id` to match the
authenticated QUIC peer. Tampering with the revision, owner, collection,
file metadata, or signature is rejected.

A catalogue is an advertisement only. It is not a bearer capability, and
a cached entry does not authorize a download.

### Request-time authorization

Every download sends a fresh `FileAccessRequest` over `/boru-file-access/1`.
The owner re-checks the live relationship, per-file grants and denials, offer
status, file availability, expected content hash, expected size, and expected
version. A stale catalogue therefore cannot preserve access after a permission
or file change.

Permission grants may carry an optional `expires_at_ms`. Expired grants are
inert at request time: they neither authorize nor deny (FS-20 hardening). The
request-time loops in `FileAccessHandler::check_permission`, the catalogue
visibility paths, and the SQL helpers (`check_permission`,
`count_read_grants_for_file`, `has_active_permissions_for_file`) all treat an
expired grant as absent. Descriptor lifetime (default 60 s) is separate from
grant expiry; the descriptor is bound to its own issue/expiry window.

A successful response is an owner-signed `SignedDownloadDescriptor` bound
to the owner, requester, shared-file ID, content/blob hash, size, timestamps,
and random nonce. The default lifetime is 60 seconds. Descriptors are intended
for one use; the shared nonce store rejects replay while the descriptor is
valid. The requester checks signature, owner, requester, lifetime, hash, and
size before transfer.

### Transport and content integrity

Iroh/QUIC encrypts traffic in transit. The file-access protocol grants access
but carries no file bytes; iroh-blobs transfers the content-addressed bytes.
The receiver writes temporary output, checks exact size and BLAKE3 content
hash, and atomically installs only verified output. A descriptor signature
authenticates authorization metadata; it is not a separate signature over the
file bytes.

### Resource exhaustion controls

Catalogue payload/count limits, file-access deadlines and preparation limits,
upload queue/concurrency limits, download queue/concurrency limits,
hash-verification limits, and blob transfer timeouts bound work derived from
peer input. See [`catalogue-limits.md`](catalogue-limits.md) and
[`remote-file-sharing.md`](remote-file-sharing.md#resource-limits).

---

## Secure-tunnel security

### Transport

Secure-tunnel traffic is **encrypted in transit by Iroh/QUIC (TLS 1.3)**,
exactly like all other Boru peer-to-peer traffic. Tunnel bytes, handshake
frames, and capabilities travel over the same mutually authenticated QUIC
channel as chat and file traffic; an attacker on the network path cannot read
or modify forwarded bytes.

### Authorisation

Tunnel access is controlled by **recipient-bound, expiring, signed
capabilities** (`TunnelCapability`):

- The owner signs the capability with its Ed25519 identity key. The signature
  covers the capability version, tunnel ID, owner and allowed-peer endpoint
  IDs, creation/expiry timestamps, and a random nonce. Any tampering is
  rejected.
- The capability names exactly one allowed peer. The `/boru-tunnel/1` handler
  verifies that the authenticated requesting peer matches the named recipient
  before forwarding any stream.
- Capabilities expire; expired or not-yet-valid capabilities are rejected.
- Capability material is never written to logs or persisted to SQLite.

### Scope of access

Sharing a local service **deliberately grants the selected peer network-level
access to that specific service** while the tunnel is active. The grant is
bounded by the owner-chosen target (loopback-only, enforced at creation), the
tunnel's expiry, per-tunnel/global connection limits, and owner revocation.
The remote peer can never select an arbitrary destination — requests name only
an existing tunnel.

### Revocation and expiry

Owners can revoke a tunnel at any time. Plain revocation blocks new
connections; revocation with termination cancels in-flight streams. Expired
tunnels reject new connections.

### Resource exhaustion controls

Tunnel limits bound work derived from peer input: max shared tunnels (32),
max simultaneously received streams (32), default per-tunnel connection limit
(16), per-peer connection-attempt rate limit (8 per 60 seconds), handshake
size bound (64 KiB), and connection/handshake/idle timeouts.

See [`secure-tunnels.md`](secure-tunnels.md) for the full tunnel security and
threat model.

---

## What this model does not claim

- The database is not encrypted at the SQLite file level. Files imported into
  local storage may be readable by a local filesystem user with access to the
  data directory.
- Network encryption and signatures do not protect a compromised local host.
- DM message plaintext is stored unencrypted in the `dm_messages` table.
- Secret keys are stored in a plaintext hex file protected only by filesystem
  permissions.
- Tunnel encryption protects the traffic path, not the shared service itself:
  a tunnel deliberately grants the recipient network-level access to the
  shared local service, and the service must be treated as exposed to that
  peer for the tunnel's lifetime.
- Tunnel traffic is not anonymised: relay operators and network observers can
  see that two peers communicate and how much traffic flows (encrypted).

---

## Related documentation

- [`message-storage-design.md`](message-storage-design.md) — full storage
  schema, crash recovery, delivery state machine
- [`privacy-model.md`](privacy-model.md) — what metadata peers see, path
  protection, authorization privacy
- [`remote-file-sharing.md`](remote-file-sharing.md) — end-to-end file
  sharing protocol
- [`catalogue-limits.md`](catalogue-limits.md) — resource usage bounds
- [`secure-tunnels.md`](secure-tunnels.md) — secure-tunnel protocol, security
  model, threat model, sharing/connecting, expiration, revocation, and
  limitations
- [`secure-tunnels-design.md`](secure-tunnels-design.md) — tunnel integration
  design and future-use-cases boundary
