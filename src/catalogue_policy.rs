//! Catalogue view validation and requester authorization.
//!
//! Pure policy functions over [`CatalogueView`] and [`FriendsStore`] — no
//! network, no storage, no UI.  Extracted from `catalogue_handler` so the
//! admission rules are independently unit-testable (BORU-AUDIT-23).

use std::sync::Arc;

use crate::catalogue_limits::{
    MAX_CATALOGUE_FILES, MAX_COLLECTIONS, MAX_ENTRIES_PER_COLLECTION, MAX_FILE_SIZE_BYTES,
};
use crate::catalogue_model::CatalogueView;
use crate::friends::{FriendId, FriendRelationship, FriendsStore};

/// View hash cache type: maps (profile_user_id, requester_id) → (revision, view_hash).
pub(crate) type ViewHashCache =
    Arc<std::sync::Mutex<std::collections::HashMap<(String, FriendId), (u64, u64)>>>;

/// Validate a [`CatalogueView`] against size and count limits, and validate
/// every file and collection entry.
///
/// Returns `Some(error_message)` on the first violation, `None` when valid.
pub(crate) fn validate_catalogue_view(view: &CatalogueView) -> Option<String> {
    if view.files.len() > MAX_CATALOGUE_FILES {
        return Some(format!(
            "catalogue has {} files, exceeds maximum of {MAX_CATALOGUE_FILES}",
            view.files.len()
        ));
    }
    if view.collections.len() > MAX_COLLECTIONS {
        return Some(format!(
            "catalogue has {} collections, exceeds maximum of {MAX_COLLECTIONS}",
            view.collections.len()
        ));
    }
    for file in &view.files {
        if file.size_bytes > MAX_FILE_SIZE_BYTES {
            return Some(format!(
                "file size_bytes {} exceeds maximum of {MAX_FILE_SIZE_BYTES}",
                file.size_bytes
            ));
        }
        if let Err(e) = file.validate() {
            return Some(format!("invalid file in catalogue: {e}"));
        }
    }
    let mut entries_per_collection = std::collections::HashMap::<&str, usize>::new();
    for file in &view.files {
        for collection_id in &file.collection_ids {
            let count = entries_per_collection
                .entry(collection_id.as_str())
                .and_modify(|count| *count += 1)
                .or_insert(1);
            if *count > MAX_ENTRIES_PER_COLLECTION {
                return Some(format!(
                    "collection {collection_id} has more than {MAX_ENTRIES_PER_COLLECTION} entries"
                ));
            }
        }
    }
    for col in &view.collections {
        if let Err(e) = col.validate() {
            return Some(format!("invalid collection in catalogue: {e}"));
        }
    }
    None
}

/// Authorization: is this requester blocked from viewing the catalogue?
///
/// Blocked peers always receive `PermissionDenied` before any storage query
/// runs.  Accepts the relationship store and the resolved requester id so the
/// decision is pure and testable without a network connection.
pub(crate) fn is_requester_blocked(friends: &FriendsStore, requester_id: &FriendId) -> bool {
    friends
        .get(requester_id)
        .is_some_and(|r| r.relationship == FriendRelationship::Blocked)
}
