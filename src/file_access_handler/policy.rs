//! Request-time authorization / policy for file access.
//!
//! Owns [`FileAccessHandler::check_permission`](crate::file_access_handler::FileAccessHandler::check_permission)
//! — the live permission, availability, and integrity check against current
//! database state — and the structured per-request diagnostic emission.
//! Also carries the [`FileAccessErrorCode`] → [`FileAccessResponse`] mapping.

use std::time::{SystemTime, UNIX_EPOCH};

use iroh::PublicKey;
use tracing::{error, info, warn};

use crate::file_access_protocol::{
    sign_download_descriptor, BlobFormat, FileAccessErrorCode, FileAccessRequest,
    FileAccessResponse,
};
use crate::friends::{FriendId, FriendRelationship};
use crate::rings::RingPermission;

use super::limits::PrepareError;
use super::prepare::{prepare_imported_file, prepare_referenced_file};
use super::{FileAccessHandler, DOWNLOAD_DESCRIPTOR_TTL};

impl FileAccessHandler {
    /// Emit a structured diagnostic [`info!`] event for an access request.
    ///
    /// Fields follow a fixed schema so log aggregators can index them:
    /// - `peer` — requesting peer (short form)
    /// - `shared_file_id` — first 16 characters of the file id (privacy-safe prefix)
    /// - `result` — the overall outcome (`"Granted"`, `"PermissionDenied"`, etc.)
    /// - `error_category` — high-level grouping (`"none"`, `"permission"`, `"availability"`, etc.)
    /// - `version_ok` — `true`/`false` when the version check was reached, absent otherwise
    /// - `prep_ok` — `true`/`false` when preparation ran, absent otherwise
    /// - `descriptor_issued` — `true` only when a [`SignedDownloadDescriptor`] was created
    ///
    /// No descriptor secrets or local filesystem paths are ever included.
    fn access_diag(
        peer: &iroh::PublicKey,
        shared_file_id: &str,
        result: &'static str,
        error_category: &'static str,
        version_ok: Option<bool>,
        prep_ok: Option<bool>,
        descriptor_issued: bool,
    ) {
        let prefix = &shared_file_id[..shared_file_id.len().min(16)];
        info!(
            peer = %peer.fmt_short(),
            shared_file_id = %prefix,
            result,
            error_category,
            version_ok,
            prep_ok,
            descriptor_issued,
            "file-access: access request",
        );
    }

    /// Perform a request-time permission, availability, and integrity check.
    ///
    /// This is the core access-control function.  It checks **everything**
    /// against current database state — no cached catalogue data is trusted.
    ///
    /// Returns the appropriate [`FileAccessResponse`] variant:
    /// - `Granted(...)` — all checks pass, a short-lived download descriptor
    ///   is returned.
    /// - `UnsupportedVersion` — wire or inner-request version not supported.
    /// - `InvalidRequest` / `NotFound` — structural problems.
    /// - `PermissionDenied` — requester is blocked or lacks explicit grant.
    /// - `Disabled` — the file offer is disabled.
    /// - `Unavailable` — the file object is no longer available locally.
    /// - `Changed` — the content_hash has changed since the requester's
    ///   catalogue was fetched.
    pub(crate) async fn check_permission(
        &self,
        requester: &PublicKey,
        request: &FileAccessRequest,
    ) -> FileAccessResponse {
        let requester_id = FriendId::from_public_key(*requester);

        // ── 1. Structural validation ──────────────────────────────────
        if let Err((code, _msg)) = request.validate() {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "InvalidRequest",
                "invalid",
                None,
                None,
                false,
            );
            return FileAccessResponse::from(code);
        }

        // ── 2. Look up the shared file by metadata_id ────────────────
        // SQLite read — run on the blocking pool so the QUIC accept worker
        // is never stalled (BORU-AUDIT-18).
        let profile_user_id = self.profile_user_id.clone();
        let shared_file_id = request.shared_file_id.clone();
        let row = match self
            .storage
            .run_blocking("file_access.get_shared_file_by_metadata_id", move |s| {
                s.get_shared_file_by_metadata_id(&profile_user_id, &shared_file_id)
            })
            .await
        {
            Ok(Some(r)) => r,
            Ok(None) => {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "NotFound",
                    "not_found",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::NotFound;
            }
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "get_shared_file_by_metadata_id: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "NotFound",
                    "internal",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        // ── 3. Offer enabled check ──────────────────────────────────
        if !row.offered {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "Disabled",
                "disabled",
                None,
                None,
                false,
            );
            return FileAccessResponse::Disabled;
        }

        // ── 4. Blocked check ─────────────────────────────────────────
        if let Some(record) = self.friends.get(&requester_id) {
            if record.relationship == FriendRelationship::Blocked {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "permission",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::PermissionDenied;
            }
        }

        // ── 4a. Ring-based authorization (iroh-rings borrow) ─────────
        // A ring is a named set of peers sharing typed Read/Write/Delete
        // permissions on file resources.  Ring grants are ADDITIVE to the
        // friend-relationship checks below: a peer authorized by a ring may
        // download even when they are not a friend and have no per-peer
        // grant.  Explicit per-peer `deny` grants (checked below) still win
        // over ring grants.
        //
        // The check is a LIVE SQLite query — membership changes revoke
        // access at request time; no cached catalogue state is trusted.
        // Resources with no ring association are implicitly denied by the
        // ring model (`check_ring_access` returns false).
        let ring_allows_read = match self
            .storage
            .run_blocking("file_access.check_ring_access", {
                let profile_user_id = self.profile_user_id.clone();
                let requester_id_str = requester_id.as_str().to_owned();
                let content_hash = row.content_hash.clone();
                move |s| {
                    s.check_ring_access(
                        &profile_user_id,
                        &requester_id_str,
                        &content_hash,
                        RingPermission::Read,
                    )
                }
            })
            .await
        {
            Ok(allowed) => allowed,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "check_ring_access: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "internal",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        // Authorization and catalogue-integrity checks must precede local
        // preparation.  A stale catalogue must produce the precise denial or
        // mismatch response even when the local object is unavailable.
        let permissions = match self
            .storage
            .run_blocking("file_access.list_permissions_for_grantee", {
                let requester_id_str = requester_id.as_str().to_owned();
                move |s| s.list_permissions_for_grantee(&requester_id_str)
            })
            .await
        {
            Ok(p) => p,
            Err(_) => return FileAccessResponse::from(FileAccessErrorCode::InternalError),
        };
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut explicitly_granted = false;
        for perm in &permissions {
            if perm.grantor_user_id == self.profile_user_id && perm.content_hash == row.content_hash
            {
                // Expired grants must not authorize (or deny) access — they are
                // treated as if absent, matching the SQL-level expiry filter in
                // `count_read_grants_for_file` / `check_permission`.
                if !perm.is_active_at(now_ms) {
                    continue;
                }
                match perm.permission.as_str() {
                    "deny" => return FileAccessResponse::PermissionDenied,
                    "read" => explicitly_granted = true,
                    _ => {}
                }
            }
        }
        let has_any_read_grants = match self
            .storage
            .run_blocking("file_access.count_read_grants_for_file", {
                let content_hash = row.content_hash.clone();
                let profile_user_id = self.profile_user_id.clone();
                move |s| s.count_read_grants_for_file(&content_hash, &profile_user_id)
            })
            .await
        {
            Ok(n) => n > 0,
            Err(_) => return FileAccessResponse::from(FileAccessErrorCode::InternalError),
        };
        // Ring grants bypass the friend/explicit-grant requirement (additive).
        // Explicit `deny` grants above still win over ring grants.
        if !ring_allows_read
            && ((has_any_read_grants && !explicitly_granted)
                || (!has_any_read_grants
                    && !self
                        .friends
                        .get(&requester_id)
                        .is_some_and(|r| r.relationship == FriendRelationship::Friends)))
        {
            return FileAccessResponse::PermissionDenied;
        }

        let expected_hex = hex::encode(request.expected_content_hash);
        if expected_hex != row.content_hash {
            return FileAccessResponse::Changed;
        }
        if request.expected_version != 0 && request.expected_version != row.version {
            return FileAccessResponse::VersionMismatch {
                current_version: row.version,
            };
        }

        // ── 5. Availability check ─────────────────────────────────────
        // Determine whether this is a referenced file (has source_path)
        // and call the appropriate preparation function, bounded by
        // the prepare limiter (concurrency, size, timeout).
        let file_obj = match self
            .storage
            .run_blocking("file_access.get_file_object", {
                let content_hash = row.content_hash.clone();
                move |s| s.get_file_object(&content_hash)
            })
            .await
        {
            Ok(Some(fo)) => fo,
            Ok(None) => {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "get_file_object: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "internal",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        // ── 5a. Apply preparation bounds ──────────────────────────
        let permit = match self.prepare_limiter.try_begin(file_obj.size) {
            Ok(p) => p,
            Err(PrepareError::Busy) => {
                warn!(
                    peer = %requester.fmt_short(),
                    "file preparation busy — max_concurrent_preparations reached (size={})",
                    file_obj.size,
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Busy",
                    "busy",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Busy;
            }
            Err(PrepareError::TooLarge {
                size_bytes,
                max_bytes,
            }) => {
                warn!(
                    peer = %requester.fmt_short(),
                    "file too large for preparation: {size_bytes} > {max_bytes}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
            Err(PrepareError::Timeout { .. }) => {
                // Should not happen from try_begin (timeout is applied
                // during the actual async call), but handle defensively.
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
        };

        // ── 5b. Run bounded preparation with timeout ──────────────
        let storage = self.storage.clone();
        let blob_store = self.blob_store.clone();
        let content_hash = row.content_hash.clone();
        let is_referenced = file_obj.source_path.is_some();
        // The descriptor's signed size must come from metadata verified at
        // request time.  Pass the DB-recorded size as the expected size so
        // the referenced path cross-checks the on-disk file against it and
        // the imported path cross-checks the content-addressed store
        // (BORU-AUDIT-07).  A mismatch fails closed instead of signing a
        // descriptor with stale or guessed metadata.
        let expected_size = file_obj.size;

        let bounded_prepare = async move {
            if is_referenced {
                prepare_referenced_file(
                    &storage,
                    &blob_store,
                    &content_hash,
                    None,                // verify_hash — deferred to descriptor verification
                    Some(expected_size), // verify_size — on-disk size must match the record
                )
                .await
            } else {
                prepare_imported_file(
                    &storage,
                    &blob_store,
                    &content_hash,
                    None,                // verify_hash — deferred to descriptor verification
                    Some(expected_size), // verify_size — blob-store size must match the record
                )
                .await
            }
        };

        // The permit is moved into the timeout future so it stays
        // alive (holding its semaphore slot) for the duration.
        let prepare_result = tokio::time::timeout(permit.timeout(), bounded_prepare).await;

        let prepare_result = match prepare_result {
            Ok(res) => res,
            Err(_elapsed) => {
                warn!(
                    peer = %requester.fmt_short(),
                    "file preparation timed out (size={})",
                    file_obj.size,
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
        };

        let prepared = match prepare_result {
            Ok(prepared) => prepared,
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("not found") || msg.contains("missing") {
                    Self::access_diag(
                        requester,
                        &request.shared_file_id,
                        "Unavailable",
                        "availability",
                        None,
                        Some(false),
                        false,
                    );
                    return FileAccessResponse::Unavailable;
                }
                error!(
                    peer = %requester.fmt_short(),
                    "file preparation: {msg}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "internal",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        // ── 6. Explicit denial check ──────────────────────────────────
        let permissions = match self
            .storage
            .run_blocking("file_access.list_permissions_for_grantee", {
                let requester_id_str = requester_id.as_str().to_owned();
                move |s| s.list_permissions_for_grantee(&requester_id_str)
            })
            .await
        {
            Ok(p) => p,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "list_permissions_for_grantee: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "internal",
                    None,
                    Some(true),
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        let mut explicitly_granted = false;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        for perm in &permissions {
            if perm.grantor_user_id == self.profile_user_id && perm.content_hash == row.content_hash
            {
                // Expired grants are inert: they neither deny nor authorize.
                if !perm.is_active_at(now_ms) {
                    continue;
                }
                match perm.permission.as_str() {
                    "deny" => {
                        Self::access_diag(
                            requester,
                            &request.shared_file_id,
                            "PermissionDenied",
                            "permission",
                            None,
                            Some(true),
                            false,
                        );
                        return FileAccessResponse::PermissionDenied;
                    }
                    "read" => explicitly_granted = true,
                    _ => {}
                }
            }
        }

        // ── 7. Visibility / permission mode check ─────────────────────
        let has_any_read_grants = match self
            .storage
            .run_blocking("file_access.count_read_grants_for_file", {
                let content_hash = row.content_hash.clone();
                let profile_user_id = self.profile_user_id.clone();
                move |s| s.count_read_grants_for_file(&content_hash, &profile_user_id)
            })
            .await
        {
            Ok(n) => n > 0,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "count_read_grants_for_file: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "internal",
                    None,
                    Some(true),
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        // Ring grants bypass the friend/explicit-grant requirement here too
        // (additive).  Explicit `deny` grants above still win over rings.
        if !ring_allows_read {
            if has_any_read_grants {
                // Selected-peers mode: requester must have an explicit read grant.
                if !explicitly_granted {
                    Self::access_diag(
                        requester,
                        &request.shared_file_id,
                        "PermissionDenied",
                        "permission",
                        None,
                        Some(true),
                        false,
                    );
                    return FileAccessResponse::PermissionDenied;
                }
            } else {
                // Contacts-only mode: requester must be a friend.
                let is_friend = self
                    .friends
                    .get(&requester_id)
                    .is_some_and(|r| r.relationship == FriendRelationship::Friends);
                if !is_friend {
                    Self::access_diag(
                        requester,
                        &request.shared_file_id,
                        "PermissionDenied",
                        "permission",
                        None,
                        Some(true),
                        false,
                    );
                    return FileAccessResponse::PermissionDenied;
                }
            }
        }

        // ── 8. Content hash integrity check ──────────────────────────
        // Convert the expected (raw [u8; 32]) to hex for comparison with
        // the stored hex string.  A mismatch means the file content
        // changed since the requester fetched their catalogue.
        let expected_hex = hex::encode(request.expected_content_hash);
        if expected_hex != row.content_hash {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "Changed",
                "integrity",
                None,
                Some(true),
                false,
            );
            return FileAccessResponse::Changed;
        }

        // ── 9. Version check ──────────────────────────────────────────
        // Compare against the database `version` column, which is
        // monotonically incremented on every metadata or content change
        // (description, display name, collection membership, hash).
        // The content-hash check above (step 8) already catches content
        // changes; this version check is a guard against mere metadata
        // changes that do not alter the content hash.
        //
        // Backward compatibility: expected_version == 0 (from older clients
        // that did not track versions) is treated as "accept any version".
        // Both code paths (the permissive check in stage 4 and the exact
        // check here in stage 9) now compare against `row.version`.
        if request.expected_version != row.version {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "VersionMismatch",
                "version",
                Some(true),
                Some(true),
                false,
            );
            return FileAccessResponse::VersionMismatch {
                current_version: row.version,
            };
        }

        // ── 10. All checks pass — issue download descriptor ──────────
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expires_at_ms = now_ms + DOWNLOAD_DESCRIPTOR_TTL.as_millis() as u64;

        // The size signed into the descriptor comes from the preparation
        // result, which verified the file's metadata at request time
        // (filesystem metadata for referenced files, the content-addressed
        // blob store for imported files).  Never fall back to a guessed 0 —
        // a signed capability must carry verified metadata only
        // (BORU-AUDIT-07).
        let size_bytes = prepared.size_bytes;

        // Convert the hex content_hash to raw 32 bytes for the descriptor.
        // This is the canonical blob hash that gets signed.  Fail closed: a
        // malformed or non-32-byte stored hash must never produce a silently
        // zero/truncated blob_hash in a signed descriptor (BORU-AUDIT-06).
        let raw_hash: [u8; 32] = match hex::decode(&row.content_hash) {
            Ok(bytes) => match <[u8; 32]>::try_from(bytes.as_slice()) {
                Ok(arr) => arr,
                Err(_) => {
                    error!(
                        peer = %requester.fmt_short(),
                        "stored content hash is not 32 bytes: refusing to sign descriptor"
                    );
                    return FileAccessResponse::from(FileAccessErrorCode::InternalError);
                }
            },
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "stored content hash is not valid hex: refusing to sign descriptor: {e}"
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        let descriptor = sign_download_descriptor(
            &self.secret_key,
            *requester,
            request.shared_file_id.clone(),
            raw_hash,
            size_bytes,
            BlobFormat::Raw,
            now_ms,
            expires_at_ms,
        );

        Self::access_diag(
            requester,
            &request.shared_file_id,
            "Granted",
            "none",
            Some(true),
            Some(true),
            true,
        );
        FileAccessResponse::Granted(Box::new(descriptor))
    }
}

impl From<FileAccessErrorCode> for FileAccessResponse {
    fn from(code: FileAccessErrorCode) -> Self {
        match code {
            FileAccessErrorCode::UnsupportedVersion => FileAccessResponse::UnsupportedVersion,
            FileAccessErrorCode::PermissionDenied => FileAccessResponse::PermissionDenied,
            FileAccessErrorCode::NotFound => FileAccessResponse::NotFound,
            FileAccessErrorCode::InvalidRequest => FileAccessResponse::NotFound,
            FileAccessErrorCode::RateLimited => FileAccessResponse::RateLimited,
            FileAccessErrorCode::Busy => FileAccessResponse::Busy,
            FileAccessErrorCode::ResponseTooLarge => FileAccessResponse::Unavailable,
            FileAccessErrorCode::InternalError => FileAccessResponse::Unavailable,
        }
    }
}
