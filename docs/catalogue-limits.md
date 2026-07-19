# Catalogue Protocol: Request and Response Size Limits

These are enforced limits, not deployment recommendations. Both the handler
and client fail closed when a boundary is exceeded.

## Hard Limits (enforced at protocol boundaries)

All limits are defined in `src/catalogue_limits.rs` and enforced on both the
server (handler) and client side.

### Byte Size Limits

| Limit | Value | Enforced At |
|---|---|---|
| Max request payload | 256 KiB | Handler — rejects oversized requests before deserialization |
| Max response payload | 4 MiB | Handler (before write) + Client (after read) |
| Max file-details response | 256 KiB | Handler (before write) + Client (after read) |
| Max paginated page response | 1 MiB | Handler (before write) + Client (after read) |

### Count Limits

| Limit | Value | Enforced At |
|---|---|---|
| Max files per catalogue | 10,000 | Handler (`validate_catalogue_view` before signing) + Client (after deserialization) |
| Max collections per catalogue | 1,000 | Handler (`validate_catalogue_view` before signing) + Client (after deserialization) |
| Max entries per collection | 10,000 | Handler + Client membership validation |
| Max page size | 500 files | Handler clamps requests + Client clamps outgoing requests |
| Max invalid response attempts | 3 | Client aborts malformed pagination responses |

### Field-Level String Length Limits

| Limit | Value | Enforced By |
|---|---|---|
| Max display_name length | 512 bytes | `RemoteSharedFile::validate()` |
| Max description length | 1,024 bytes | `RemoteSharedFile::validate()`, `RemoteCollection::validate()` |
| Max mime_type length | 128 bytes | `RemoteSharedFile::validate()` |
| Max content_hash length | 128 bytes | `RemoteSharedFile::validate()` |
| Max shared_file_id length | 256 bytes | `RemoteSharedFile::validate()` |
| Max collection_id length | 256 bytes | `RemoteCollection::validate()` |
| Max collection name length | 512 bytes | `RemoteCollection::validate()` |

### Collection Membership Limits

| Limit | Value | Enforced By |
|---|---|---|
| Max collections per file | 256 | `RemoteSharedFile::validate()` |

### File Size Limit

| Limit | Value | Enforced At |
|---|---|---|
| Max individual file `size_bytes` | 10 TiB | Handler (`validate_catalogue_view` before signing) |

## Enforcement Points

### Server (handler) — `catalogue_handler.rs`

1. **Request byte limit**: checked immediately after reading the wire frame, before
   deserialization. Returns `CatalogErrorCode::InvalidRequest` on violation.
2. **Catalogue view validation**: `validate_catalogue_view()` checks file count,
   collection count, per-file `size_bytes`, and individual entry field validity.
   Called in both `build_catalogue_for_requester()` and the `GetCatalogue` handler
   path before signing.
3. **Response byte limit**: `write_catalogue_response()` serialises the response
   and checks the payload size against `MAX_CATALOGUE_RESPONSE_BYTES` before writing.
   Returns an `io::Error` on violation (propagated as a connection error).
4. **File-details response byte limit**: `write_file_details_response()` enforces
   the stricter `MAX_FILE_DETAILS_PAYLOAD_BYTES` limit.
5. **File entry validation**: in the `GetFileDetails` path, `file.validate()` is
   called before sending the response. Invalid entries return `InternalError`.
6. **Page limits**: paginated responses use the 500-item and 1 MiB caps,
   independently of the larger full-catalogue response cap.

### Client — `catalogue_client.rs`

1. **Response byte limit**: checked after reading the wire frame, before
   deserialization. Returns `ProtocolError` on violation.
2. **Catalogue file/collection count limits**: checked against `MAX_CATALOGUE_FILES`
   and `MAX_COLLECTIONS` after deserializing a `SignedCatalogue` response.
3. **Field-level validation**: each `RemoteSharedFile` and `RemoteCollection` entry
   is validated via its `.validate()` method. Invalid entries return `ProtocolError`.
4. **Pagination limits**: page responses are capped at 1 MiB, page size is
   capped at 500, total pages are bounded by the catalogue file limit, and
   invalid pagination responses are fail-closed after three attempts.

## Error Handling

- Oversized requests → `CatalogErrorCode::InvalidRequest`
- Field/count violations → `CatalogErrorCode::InvalidRequest` (server) or
  `RemoteCatalogueFetchError::ProtocolError` (client)
- Oversized responses → connection error (server) or `ProtocolError` (client)

File-access and download admission is bounded separately. Default file-access
limits are four concurrent preparations (1 GiB/file, 60-second preparation
timeout), eight active upload requests, two requests per peer, a 32-request
queue, four concurrent permission verifications, and a 60-second request
timeout. Default download limits are four active downloads, one per peer, two
concurrent hash verifications, and a 32-item queue. These controls prevent
catalogue or transfer requests from turning untrusted peer input into
unbounded memory, CPU, disk, or network work.
