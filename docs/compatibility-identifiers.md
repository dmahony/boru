# Compatibility-sensitive identifiers

This inventory is the preservation list for the Boru branding rename. It is based on the repository audit in `file_repository_audit.md` and a source audit of the protocol, discovery, cryptographic, and persistence modules.

## Policy

A product/display rename may change crate/package names, Rust module names, UI text, documentation, and human-readable log labels. It must not change any identifier below in an existing installation or a peer-visible deployment unless a compatibility bridge and an explicit migration are shipped first.

`MUST KEEP` means the bytes, string, numeric value, enum ordering, or on-disk key are part of an existing contract. A source-level alias is fine; changing the value is not. `CAN RENAME with migration` is reserved for identifiers that are local and can be migrated without changing peer identity or old data. No protocol or cryptographic identifier found in this audit qualifies for an unconditional rename.

## 1. Protocol negotiation identifiers — MUST KEEP

These values are QUIC ALPNs. They are selected before application payloads are decoded; changing one prevents old and new peers from selecting the same handler. Keep the exact bytes, including the version suffix.

| Identifier | Location | Contract and migration |
|---|---|---|
| `/iroh-gossip/1` (`GOSSIP_ALPN`) | `src/net.rs:46-47` | Primary iroh gossip negotiation and the existing `boru_chat::ALPN` re-export. MUST KEEP for peer interoperability. A future protocol requires a new ALPN and a dual-registration/dual-stack rollout; do not replace v1 in place. |
| `/boru-file-catalog/1` (`CATALOGUE_ALPN`) | `src/protocol_version.rs:19-22`; registered by `examples/iced_chat/main.rs:708-716` | Catalogue retrieval negotiation. MUST KEEP. The Rust symbol may be aliased, but the ALPN value and frame version remain v1. Run old and new handlers concurrently for a future version. |
| `/boru-file-access/1` (`FILE_ACCESS_ALPN`) | `src/net.rs:49-56` and `src/file_access_handler.rs:593-595` | File-access/transfer authorization negotiation. MUST KEEP in both definitions (and they should remain byte-identical). Changing only one copy is a compatibility bug. |
| `/iroh-gossip-chat/whisper/1` (`WHISPER_ALPN`) | `src/whisper/mod.rs:39-42` | Direct-message/file-transfer QUIC negotiation. MUST KEEP; dual-register a new ALPN for a breaking v2. |
| `/iroh-gossip-chat/backfill/1` (`BACKFILL_ALPN`) | `src/backfill.rs:59-62` | History backfill request/response negotiation. MUST KEEP; preserve v1 while adding any new version. |
| `/iroh-gossip-chat/friend-ping/1` (`FRIEND_PING_ALPN`) | `src/chat_core/friend_ping.rs:30-36` | Reachability probe handler negotiation. MUST KEEP so mixed-version friends still report status. |
| `/iroh-chat-inbox/1` (`INBOX_ALPN`) | `src/inbox.rs:46-49` | Offline-message delivery and mailbox synchronization. MUST KEEP; changing it strands pending deliveries unless both ALPNs are served during migration. |

The `iroh-blobs::ALPN` value is an upstream dependency contract, not a Boru-owned string. Do not wrap it with a renamed value or stop registering it when blob transfer is enabled.

## 2. Wire names, versions, and numeric encodings — MUST KEEP

The following are exchanged after negotiation and therefore remain protocol identifiers even though they are Rust types:

- `src/protocol_version.rs`: `CATALOGUE_RETRIEVAL_V1 = 1` and `SUPPORTED_CATALOGUE_RETRIEVAL = &[1]`; frame layout is little-endian `u16 version`, little-endian `u32 payload length`, payload. Keep the layout and v1 acceptance.
- `src/file_access_protocol.rs`: `FILE_ACCESS_WIRE_VERSION = 1`, `SUPPORTED_FILE_ACCESS_VERSIONS = &[1]`, `FileAccessErrorCode` uses `#[repr(u8)]` with `UnsupportedVersion = 1` and the existing discriminant order. `FileAccessWireRequest`/`FileAccessWireResponse` are versioned wrappers. Keep field meanings and enum discriminants; add a version rather than changing v1.
- `src/catalogue_protocol.rs`: `CatalogErrorCode` serializes as the stable snake_case values `permission_denied`, `not_found`, `invalid_request`, `unsupported_version`, `rate_limited`, `busy`, `response_too_large`, and `internal_error`. Its postcard variant indexes 0 through 7 are explicitly decoded. Keep both strings and indexes. New error values must be appended or handled as an explicitly versioned protocol change.
- `src/backfill.rs`: the length-prefixed postcard request/response format and signed-message replay verification are wire contracts. Keep request/response field meanings and the existing server/client limits when changing branding.
- `src/proto/{mod,topic,hyparview,plumtree}.rs`: postcard/Serde enums (`Message`, `Event`, HyParView messages, PlumTree messages) are the iroh-gossip wire model. Variant order and field types are compatibility-sensitive; do not rename/reorder variants in a v1 deployment.
- `src/inbox.rs`, `src/whisper/mod.rs`, `src/mailbox.rs`: `MailboxEnvelope`, `MailboxAck`, `SignedInboxMessage`, and `WhisperWireMessage` fields are serialized transport data. Keep field names/ordering and signature-covered bytes. Existing pending envelopes must remain decodable.
- `src/transfer_telemetry.rs:149`: telemetry `schema_version = 1` and event names in `src/diagnostics.rs:472-494` (`download_queued`, `access_requested`, `access_granted`, `transfer_started`, `progress_checkpoint`, `pause`, `resume`, `verification`, `completion`, `failure`, `cancellation`) are stored diagnostic data. They may be display-renamed only through an explicit data migration or an old-name reader.

## 3. Cryptographic domain separators and signing inputs — MUST NOT CHANGE

These literals are domain separation values or inputs to deterministic keys/signatures. Changing them silently creates different keys, namespaces, signatures, or trust domains. They are security-sensitive and must remain byte-for-byte identical:

| Value | Location | Purpose |
|---|---|---|
| `boru-chat public-room v1` | `src/topic_derivation.rs:10-16` | BLAKE3 gossip topic derivation. |
| `boru-chat discovery-key v1` | `src/public_room.rs:34-39` | BLAKE3 public-room DHT discovery-key derivation. |
| `boru-chat room discovery v1` | `src/topic_derivation.rs:61-72` | SHA-256 distributed-topic-tracker namespace derivation. |
| `boru-chat/public-lobby/v1` | `src/discovery_backend.rs:20-24` | Canonical public-lobby key derivation. |
| `boru-chat private-room v1` | `src/private_room_tracker.rs:68-92` | BLAKE3 private-room namespace derivation. |
| `iroh-gossip-chat/mailbox/v1` | `src/mailbox.rs:206-210` | Mailbox encryption-key derivation from the shared secret. |
| `boru-chat private-room v2 namespace` | `src/discovery_secret.rs:68-72` | Reserved/implemented V2 namespace subkey derivation. |
| `boru-chat private-room v2 encryption` | `src/discovery_secret.rs:75-77` | Reserved/implemented V2 encryption subkey derivation. |
| `boru-chat private-room v2 signing` | `src/discovery_secret.rs:79-82` | Reserved/implemented V2 signing subkey derivation. |

Also preserve the exact signed-byte constructions in `src/mailbox.rs`, `src/inbox.rs`, `src/file_access_protocol.rs`, and the `blake3` hash definition for message/file IDs. A branding rename must never be used as a new signature prefix or as a replacement for a legacy prefix. If a V2 cryptographic scheme is introduced, label it as V2 and retain V1 verification/decryption for existing records during migration.

## 4. Room identity and network namespaces — MUST NOT CHANGE

- `src/public_room.rs`: `APPLICATION_NAMESPACE = "boru-chat"`, `PUBLIC_ROOM_NAME = "public-lobby"`, `PROTOCOL_VERSION = 1`, and `PublicNetwork` byte assignments (`Mainnet=0x00`, `Development=0x01`, `Test=0x02`) are identity inputs. Changing any one changes the public lobby topic and discovery key, causing peers to split into different rooms. Preserve the known-answer vectors in the module tests.
- `src/topic_derivation.rs`: preserve the room-name length encoding (`u16` little-endian), room bytes, network byte, and version ordering in addition to the separator.
- `src/room.rs`: `RoomStore.topic` is persisted in `room.json`; it is the room identity and must be read unchanged. `RoomStore.discovery_secret` is persisted room key material and must not be regenerated during a rename. Legacy room files without the field require the existing migration behavior.
- `src/private_room_tracker.rs` and `src/discovery_secret.rs`: the random `DiscoverySecret` serialized in `room.json` is identity/key material. Preserve it and the V1 namespace derivation. A migration may add V2 derived keys, but must retain the V1 path for existing rooms.
- `src/discovery_backend.rs:NamespaceId`: the 32-byte namespace bytes are the DHT lookup key. Never substitute a display name, crate name, or new hash prefix for existing namespaces.

## 5. Persistent files, database, and stored schema — MUST KEEP or migrate

| Identifier | Location | Verdict and migration |
|---|---|---|
| `boru.db` (`DB_FILE_NAME`) | `src/storage.rs:55-58` | MUST KEEP for existing installations. If the product adopts another database filename, open/read the legacy file first and migrate/copy atomically; do not start an empty database. |
| SQLite table `schema_version` and versions 1 through 9 | `src/storage.rs:45-49, 546-609` | MUST KEEP as the migration history. Never reset or rename it; a newer schema is deliberately rejected. New migrations append version numbers. |
| SQLite tables `inbox`, `outbox`, `contacts`, `sync_cursor` | `src/storage.rs:612-656` | MUST KEEP: message delivery state, peer identity/address state, and cursors are durable. Renaming requires transactional copy plus compatibility rollback. |
| SQLite tables `file_objects`, `message_attachments`, `shared_files`, `file_collections`, `file_collection_items`, `shared_file_permissions`, `downloads`, `profile_manifest_state` | `src/storage.rs` V2+ migrations; also enumerated in `file_repository_audit.md` | MUST KEEP. Existing content hashes, foreign keys, offers, permissions, download retry state, and manifest revisions depend on these names and columns. Any schema rename requires a numbered migration and a backup/rollback path. |
| `secret_key.txt` | referenced by all durable stores and `README.md:44` | MUST KEEP. This is the node identity root; replacing it changes the endpoint/public key and friend identity. |
| `room.json`, `friends.json`, `profile.json`, `conversations.json`, `friend_requests.json`, `chat_history.json`, `outbox.json`, `mailbox.json` | `src/room.rs`, `friends.rs`, `user_profile.rs`, `conversations.rs`, `friend_request.rs`, `chat_history.rs`, `outbox.rs`, `mailbox.rs` | MUST KEEP for existing data. If a new filename is desired, read the legacy name, validate/migrate it, and write both or atomically adopt the new name only after successful conversion. Preserve each file's schema version and unknown-field behavior. |
| `<data_dir>/files/<user-hash>/<content-hash>.<extension>` | `src/image_store.rs:17,50-73` | MUST KEEP. The user hash, BLAKE3 content hash, and allowed extension form stable local image identifiers; changing them breaks profile/image references. Provide a lookup migration or retain legacy resolution. |
| `file_objects.content_hash`, `SharedFileMeta.id`, catalogue `shared_file_id`, message IDs, and transfer IDs | `src/storage.rs`, `src/catalogue_model.rs`, `src/chat_core`, `src/inbox.rs` | MUST KEEP as stored/reference identifiers. Human display labels can change, but IDs and content hashes cannot be regenerated for a branding release. |

The `file_repository_audit.md` report identifies the file-object and sharing tables above as the authoritative data model. It does not identify a separate catalogue table: catalogue data is the `ProfileUpdate`/`SharedFileMeta` wire model plus `profile_manifest_state`. Do not invent a migration that renames those concepts without preserving both representations.

## 6. Configuration and runtime environment keys

`BORU_CHAT_DATA_DIR` is referenced by the image-store documentation and is a runtime data-location key. Treat it as `MUST KEEP` for compatibility. If a branded replacement is introduced, accept the legacy key with defined precedence (new key first, legacy fallback), and document the choice. Likewise preserve the semantics of platform data-directory fallback; moving the directory without migration makes all persistent identifiers appear lost.

No OS upgrade-detection marker, App ID, bundle ID, or platform package identifier was found in the Rust repository audit. The Gradle wrapper's `org.gradle.appname` is a Gradle tooling label, not a peer/data identifier; changing it is `CAN RENAME` if external packaging metadata does not depend on it. Before shipping native installers, separately audit Android/iOS/desktop manifests and signing configuration; those platform identifiers are persistent and would require an OS-specific migration/update path.

## 7. Safe rename candidates and required migration shape

Usually safe without a data migration: UI/product title, README prose, Rust documentation, log messages, crate aliases that preserve the old public API, and internal variable/module names that do not appear in serialized data or protocol selection.

For any proposed persistent rename:

1. Keep the old reader and identifier constants.
2. Add a numbered, transactional migration (SQLite) or atomic copy/rename with round-trip validation (JSON/files).
3. Preserve old protocol ALPNs and cryptographic derivations; dual-register/dual-decode only for an intentionally versioned protocol.
4. Add mixed-version tests, known-answer derivation tests, old-database/old-JSON fixtures, and rollback coverage.
5. Do not delete the legacy data until a successful backup and verified read-back are complete.

## Audit conclusion

All protocol negotiation values, wire versions/enums, cryptographic domain separators, public/private room derivation inputs, DHT namespace derivations, identity roots, and durable database/file identifiers are preservation-critical. The branding rename should be implemented as a display/source-level rename with compatibility aliases; changing any `MUST KEEP` value without a dual-stack or data migration would break peer connectivity, existing rooms/friends, signatures, or stored user data.
