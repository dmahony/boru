# File-access descriptor signing (BORU-AUDIT-05)

Status: implemented 2026-08-09 (BORU-AUDIT-05)

## Protocol

A `SignedDownloadDescriptor` is a short-lived capability that authorises one
requester to download one file from one owner.  It is issued by the owner over
the `/boru-file-access/1` QUIC ALPN and consumed by the requester before the
blob transfer starts.

The descriptor is authenticated by an Ed25519 signature (`owner_id`) over a
canonical byte encoding.  The bytes are produced by
`DescriptorSignedPayloadV2::canonical_bytes()` in `src/file_access_protocol.rs`,
which is the single function used by both `sign_download_descriptor` and
`verify_download_descriptor`.

## Signed-field invariant

The signature authenticates exactly the fields of `DescriptorSignedPayloadV2`,
in declaration order, serialized with the project's deterministic serializer
(postcard):

```
protocol:      "boru/file-descriptor"   // domain separation
version:       2                        // signed payload version
owner_id:      PublicKey                // file owner (signer)
requester:     PublicKey                // authorised requester
shared_file_id: String                  // stable catalogue identifier
blob_hash:     [u8; 32]                 // canonical BLAKE3 content hash
size_bytes:    u64                      // expected file size
blob_ticket:   Vec<u8>                  // opaque iroh blob ticket
nonce:         [u8; 32]                 // replay protection
issued_at_ms:  u64                      // issue time (ms since UNIX epoch)
expires_at_ms: u64                      // expiry time (ms since UNIX epoch)
```

Rules that MUST hold for this struct:

1. Field order IS the wire order.  postcard serializes struct fields in
   declaration order and length-prefixes every variable-length field, so two
   different field values can never produce the same canonical bytes.
2. Adding, removing, or reordering a field changes the canonical bytes and
   therefore MUST bump `DESCRIPTOR_SIGNED_PAYLOAD_VERSION`.  Old descriptors
   are then rejected as `DescriptorVerification::UnsupportedVersion` — never
   guessed.
3. The hex display string `SignedDownloadDescriptor::content_hash` is NOT part
   of the signed payload.  It is a derived, human-readable representation of
   `blob_hash`; the strongly typed raw hash is the single authenticated
   representation.  (BORU-AUDIT-06 removes the duplicate string entirely.)
4. Both sign and verify must go through `canonical_bytes()`.  There is no other
   code path that constructs descriptor signature bytes.

## Domain separation and versioning

- `protocol = "boru/file-descriptor"` prevents a descriptor signature from
  being replayed or confused with any other signed Boru object.
- `version = 2` is embedded inside the signed structure, so it is
  authenticated.  The wire descriptor also carries `signed_version`, which is
  checked by `verify_download_descriptor` before any field layout is trusted.

## Migration

Descriptors are ephemeral (60-second TTL, issued per request).  They are never
persisted, so there is no on-disk migration and no V1 decode path: a V1-style
descriptor (raw `extend_from_slice` concatenation, no domain/version) fails
verification immediately and is rejected.  Both peers ship the same protocol
version in a coordinated deploy.

## Tests

- `descriptor_canonical_bytes_golden_vector` pins the exact canonical bytes
  for fixed fields (golden vector).
- `descriptor_field_mutation_invalidates_signature` mutates every signed field
  and asserts the descriptor is no longer `Valid`.
- `descriptor_json_reorder_does_not_affect_canonical_bytes` proves JSON/map key
  order and display serialization cannot change the signed bytes.
- `descriptor_unknown_version_rejected` proves unknown versions fail closed.
- `descriptor_sign_verify_round_trip` proves the shared sign/verify path.
