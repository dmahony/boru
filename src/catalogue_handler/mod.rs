//! Catalogue retrieval protocol handler — server side.
//!
//! Implements [`ProtocolHandler`](iroh::protocol::ProtocolHandler) for the `/boru-file-catalog/1` ALPN.
//! On each incoming connection:
//!
//! 1. Authenticate the requester via [`Connection::remote_id()`](iroh::endpoint::Connection::remote_id).
//! 2. Look up the requester in [`FriendsStore`](crate::friends::FriendsStore); blocked peers get
//!    `PermissionDenied`.
//! 3. Call [`Storage::catalogue_entries_for_peer()`](crate::storage::Storage::catalogue_entries_for_peer) to get the filtered,
//!    authorised view of files and collections.
//! 4. Build and sign a [`SignedFileCatalogue`](crate::catalogue_model::SignedFileCatalogue) with the local [`SecretKey`](iroh::SecretKey).
//! 5. Return it as [`CatalogResponse::SignedCatalogue`](crate::catalogue_protocol::CatalogResponse::SignedCatalogue).
//!
//! The handler never reuses a catalogue signed for one requester as
//! another's — every request builds a fresh signed envelope.
//!
//! # Layout
//!
//! This module is a facade over the catalogue retrieval engine:
//! - [`serve`] – the per-connection `serve_catalogue` request dispatch
//! - [`tests`] – unit + integration tests
//!
//! The public surface is [`CatalogueHandler`].

mod serve;

#[cfg(test)]
pub(crate) mod tests;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    PublicKey, SecretKey,
};
use n0_error::Result;
use tracing::{debug, error, warn};

use crate::catalogue_model::{CatalogueView, FileCatalogueCollection, SignedFileCatalogue};
use crate::catalogue_policy::{is_requester_blocked, validate_catalogue_view, ViewHashCache};
use crate::catalogue_protocol::CatalogErrorCode;
use crate::catalogue_rate_limits::{
    write_busy_response, CatalogueConcurrencyLimiter, CatalogueRateConfig,
    PeerCatalogueAbuseLimiter,
};
use crate::friends::{FriendId, FriendRelationship, FriendsStore};
use crate::storage::Storage;

/// Timeout for the entire catalogue protocol handler — a single request/response
/// cycle must complete within this window or the connection is dropped.
const CATALOGUE_HANDLER_TIMEOUT: Duration = Duration::from_secs(60);

// ── CatalogueHandler ───────────────────────────────────────────────────────
///
/// Creates signed, requester-filtered catalogue snapshots.
#[derive(Debug, Clone)]
pub struct CatalogueHandler {
    /// Shared storage backend.
    storage: Arc<Storage>,
    /// The secret key of the owning profile — used to sign catalogues.
    secret_key: SecretKey,
    /// The owning profile's user id (the PublicKey string form).
    profile_user_id: String,
    /// Friends store — relationship lookups for each requester.
    friends: FriendsStore,
    /// Content-hash cache for NotModified detection.
    ///
    /// Maps (profile_user_id, requester_id) → (revision, view_hash).
    /// The view_hash is a blake3 digest of the requester's catalogue view
    /// content (sorted file hashes + sorted collection ids).  When a
    /// `GetCatalogue` request arrives with `known_revision` matching the
    /// current revision and the same view hash, the handler returns
    /// `NotModified` instead of the full catalogue.
    view_hash_cache: ViewHashCache,
    /// Concurrency limiter — bounds simultaneous catalogue connections.
    concurrency_limiter: Arc<CatalogueConcurrencyLimiter>,
    /// Combined request, response-volume, and malformed-request limiter.
    abuse_limiter: Arc<PeerCatalogueAbuseLimiter>,
}

impl CatalogueHandler {
    /// Create a new [`CatalogueHandler`].
    ///
    /// * `storage` — shared storage for querying shared files and manifest state.
    /// * `secret_key` — the owner's identity key, used to sign every catalogue.
    /// * `profile_user_id` — the owner's PublicKey string (used as the profile id
    ///   in Storage queries).
    /// * `friends` — the owner's friends list for relationship checks.
    pub fn new(
        storage: Arc<Storage>,
        secret_key: SecretKey,
        profile_user_id: String,
        friends: FriendsStore,
    ) -> Self {
        Self::with_rate_config(
            storage,
            secret_key,
            profile_user_id,
            friends,
            CatalogueRateConfig::default(),
        )
    }

    /// Create a handler with explicit request-frequency, response-volume,
    /// and malformed-request budgets.
    pub fn with_rate_config(
        storage: Arc<Storage>,
        secret_key: SecretKey,
        profile_user_id: String,
        friends: FriendsStore,
        rate_config: CatalogueRateConfig,
    ) -> Self {
        Self {
            storage,
            secret_key,
            profile_user_id,
            friends,
            view_hash_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            concurrency_limiter: Arc::new(CatalogueConcurrencyLimiter::new(
                crate::catalogue_rate_limits::MAX_CONCURRENT_CATALOGUE_CONNECTIONS,
            )),
            abuse_limiter: Arc::new(PeerCatalogueAbuseLimiter::new(&rate_config)),
        }
    }

    /// Compute a deterministic hash of a [`CatalogueView`] for
    /// content-aware NotModified detection.
    ///
    /// The hash is computed over the sorted file content hashes and
    /// sorted collection IDs, so it is stable across serialization
    /// format changes.
    fn compute_view_hash(view: &CatalogueView) -> u64 {
        let mut hasher = blake3::Hasher::new();

        // Sort file hashes for deterministic ordering.
        let mut file_hashes: Vec<&str> =
            view.files.iter().map(|f| f.content_hash.as_str()).collect();
        file_hashes.sort();
        for h in &file_hashes {
            hasher.update(h.as_bytes());
        }

        // Sort collection IDs for deterministic ordering.
        let mut col_ids: Vec<&str> = view
            .collections
            .iter()
            .map(|c| c.collection_id.as_str())
            .collect();
        col_ids.sort();
        for id in &col_ids {
            hasher.update(id.as_bytes());
        }

        let hash = hasher.finalize();
        u64::from_le_bytes(
            hash.as_bytes()[..8]
                .try_into()
                .expect("blake3 output >= 8 bytes"),
        )
    }

    /// Check whether a requester-specific catalogue view is unchanged
    /// since `known_revision`, using the view hash cache.
    ///
    /// Returns `true` when the current view content matches what was
    /// cached for (profile_user_id, requester_id) at `known_revision`.
    fn is_view_unchanged(
        &self,
        requester_id: &FriendId,
        known_revision: u64,
        current_hash: u64,
    ) -> bool {
        let key = (self.profile_user_id.clone(), requester_id.clone());
        let cache = self.view_hash_cache.lock().expect("view_hash_cache lock");
        match cache.get(&key) {
            Some(&(rev, hash)) => rev == known_revision && hash == current_hash,
            None => false,
        }
    }

    /// Update the view hash cache for a requester.
    fn cache_view_hash(&self, requester_id: &FriendId, revision: u64, view_hash: u64) {
        let key = (self.profile_user_id.clone(), requester_id.clone());
        let mut cache = self.view_hash_cache.lock().expect("view_hash_cache lock");
        cache.insert(key, (revision, view_hash));
    }

    /// Build and sign a [`SignedFileCatalogue`] for `requester`.
    ///
    /// Returns `None` (via `PermissionDenied` error) when the requester is
    /// blocked, or an empty catalogue when the requester has no authorised
    /// entries.
    fn build_catalogue_for_requester(
        &self,
        requester: &PublicKey,
    ) -> std::result::Result<SignedFileCatalogue, CatalogErrorCode> {
        // ── Blocked check ──────────────────────────────────────────────
        let requester_id = FriendId::from_public_key(*requester);
        if is_requester_blocked(&self.friends, &requester_id) {
            return Err(CatalogErrorCode::PermissionDenied);
        }

        // ── Get manifest revision ──────────────────────────────────────
        let manifest = self
            .storage
            .get_manifest_state(&self.profile_user_id)
            .ok()
            .flatten();
        let revision = manifest.map(|m| m.revision).unwrap_or(0);

        // ── Query authorised entries ────────────────────────────────────
        let view = match self.storage.catalogue_entries_for_peer(
            &self.profile_user_id,
            requester,
            &self.friends,
        ) {
            Ok(v) => v,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "catalogue_entries_for_peer: {e:#}"
                );
                return Err(CatalogErrorCode::InternalError);
            }
        };

        // ── Validate view against limits before signing ─────────────────
        if let Some(msg) = validate_catalogue_view(&view) {
            error!(
                peer = %requester.fmt_short(),
                "build_catalogue_for_requester: validation failed: {msg}"
            );
            return Err(CatalogErrorCode::InvalidRequest);
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // ── Build and sign ─────────────────────────────────────────────
        let collections: Vec<FileCatalogueCollection> = view
            .collections
            .iter()
            .map(|c| FileCatalogueCollection {
                collection_id: c.collection_id.clone(),
                name: c.name.clone(),
                description: c.description.clone(),
            })
            .collect();
        let catalogue =
            SignedFileCatalogue::sign(&self.secret_key, revision, now_ms, collections, view.files);

        Ok(catalogue)
    }

    /// Look up a single file by [`shared_file_id`] and return its
    /// metadata for `requester` — applying the same visibility rules
    /// as the full catalogue.
    ///
    /// Returns:
    /// - `Ok(Some(file))` when the file exists and the requester can see it.
    /// - `Ok(None)` when the file exists but is not visible to the requester
    ///   (not found / hidden / disabled).
    /// - `Err(PermissionDenied)` when the requester is blocked.
    /// - `Err(InternalError)` on storage errors.
    fn get_file_details_for_requester(
        &self,
        requester: &PublicKey,
        shared_file_id: &str,
    ) -> std::result::Result<Option<crate::catalogue_model::RemoteSharedFile>, CatalogErrorCode>
    {
        // ── Blocked check ──────────────────────────────────────────────
        let requester_id = FriendId::from_public_key(*requester);
        if is_requester_blocked(&self.friends, &requester_id) {
            return Err(CatalogErrorCode::PermissionDenied);
        }

        // ── Look up the shared file by metadata_id ─────────────────────
        let row = match self
            .storage
            .get_shared_file_by_metadata_id(&self.profile_user_id, shared_file_id)
        {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None), // not found
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "get_shared_file_by_metadata_id: {e:#}"
                );
                return Err(CatalogErrorCode::InternalError);
            }
        };

        // ── Offered check ──────────────────────────────────────────────
        if !row.offered {
            return Ok(None);
        }

        // ── Availability check ─────────────────────────────────────────
        match self.storage.file_object_exists(&row.content_hash) {
            Ok(false) => return Ok(None),
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "file_object_exists: {e:#}"
                );
                return Err(CatalogErrorCode::InternalError);
            }
            Ok(true) => {}
        }

        // ── Denial check ──────────────────────────────────────────────
        let permissions = match self
            .storage
            .list_permissions_for_grantee(requester_id.as_str())
        {
            Ok(p) => p,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "list_permissions_for_grantee: {e:#}"
                );
                return Err(CatalogErrorCode::InternalError);
            }
        };

        let mut denied = false;
        let mut explicitly_granted = false;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
                    "deny" => denied = true,
                    "read" => explicitly_granted = true,
                    _ => {}
                }
            }
        }

        if denied {
            return Ok(None);
        }

        // ── Permission mode check ─────────────────────────────────────
        let has_any_read_grants = match self
            .storage
            .count_read_grants_for_file(&row.content_hash, &self.profile_user_id)
        {
            Ok(n) => n > 0,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "count_read_grants_for_file: {e:#}"
                );
                return Err(CatalogErrorCode::InternalError);
            }
        };

        if has_any_read_grants {
            // Selected-peers mode: requester must have an explicit grant.
            if !explicitly_granted {
                return Ok(None);
            }
        } else {
            // Contacts-only default: requester must be a friend.
            let is_friend = self
                .friends
                .get(&requester_id)
                .is_some_and(|r| r.relationship == FriendRelationship::Friends);
            if !is_friend {
                return Ok(None);
            }
        }

        // ── Build RemoteSharedFile ─────────────────────────────────────
        // A shared-file row without its content record is corrupt or stale.
        // Refuse to sign guessed metadata (size 0, empty MIME) into a
        // file-details response (BORU-AUDIT-07).  Log the record identifier
        // and error class only — never secret capability data or file
        // contents.
        let fo = match self.storage.get_file_object(&row.content_hash) {
            Ok(Some(fo)) => fo,
            Ok(None) => {
                error!(
                    shared_file_id = %shared_file_id,
                    content_hash = %row.content_hash,
                    "file details: shared file has no file object — refusing to sign guessed metadata"
                );
                return Err(CatalogErrorCode::InternalError);
            }
            Err(e) => {
                error!(
                    shared_file_id = %shared_file_id,
                    content_hash = %row.content_hash,
                    "file details: get_file_object failed: {e:#}"
                );
                return Err(CatalogErrorCode::InternalError);
            }
        };

        Ok(Some(crate::catalogue_model::RemoteSharedFile {
            shared_file_id: row.metadata_id.clone(),
            display_name: row.display_filename.clone(),
            description: row.description.clone(),
            mime_type: fo.mime_type,
            size_bytes: fo.size,
            content_hash: row.content_hash.clone(),
            version_number: row.version as u32,
            updated_at_ms: row.updated_at_ms,
            collection_ids: Vec::new(),
            children: vec![],
        }))
    }
}

impl ProtocolHandler for CatalogueHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        debug!(
            peer = %remote_id.fmt_short(),
            "catalogue: incoming connection"
        );

        // ── Concurrency limit check ──────────────────────────────────
        // Hold the permit for the lifetime of this call so the slot stays occupied.
        let _permit = match self.concurrency_limiter.try_acquire() {
            Some(permit) => permit,
            None => {
                // Server is at capacity — send Busy and close.
                if let Ok((mut send, _recv)) = connection.accept_bi().await {
                    if let Err(e) = write_busy_response(&mut send).await {
                        warn!(
                            peer = %remote_id.fmt_short(),
                            "catalogue: failed to write busy response: {e:#}"
                        );
                    }
                    let _ = send.finish();
                }
                return Ok(());
            }
        };

        match tokio::time::timeout(
            CATALOGUE_HANDLER_TIMEOUT,
            serve::serve_catalogue(&connection, self),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(
                    peer = %remote_id.fmt_short(),
                    "catalogue: serve error: {e:#}"
                );
            }
            Err(_elapsed) => {
                warn!(
                    peer = %remote_id.fmt_short(),
                    "catalogue: handler timeout after {CATALOGUE_HANDLER_TIMEOUT:?}"
                );
            }
        }

        // Keep the connection alive until the client finishes reading the response.
        // Dropping the connection immediately after writing can reset the stream
        // before the peer has consumed the frame.
        let _ = connection.closed().await;
        Ok(())
    }
}
