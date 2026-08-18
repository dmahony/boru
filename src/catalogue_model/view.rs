//! [`RemoteCollection`] and [`CatalogueView`] — the wire-safe collection
//! view a remote peer sees.

use n0_error::Result;
use serde::{Deserialize, Serialize};

use super::{
    valid_description_text, valid_display_text, valid_identifier, RemoteSharedFile,
    MAX_COLLECTION_NAME_LENGTH, MAX_DESCRIPTION_LENGTH,
};

// ── RemoteCollection ─────────────────────────────────────────────────────

/// A collection visible to a remote peer in a catalogue.
///
/// This is the wire-safe version of a collection, distinct from
/// [`FileCatalogueCollection`] which lacks the `sort_order` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteCollection {
    /// Unique identifier for this collection.
    pub collection_id: String,
    /// Human-readable display name.
    pub name: String,
    /// Optional description. Tab, CR, and LF are allowed for ordinary
    /// formatting; all other control, line/paragraph-separator, and Unicode
    /// format characters are rejected by [`Self::validate`].
    #[serde(default)]
    pub description: Option<String>,
    /// Display order among collections (lower = first).
    pub sort_order: u32,
}

impl RemoteCollection {
    /// Validate the remote collection fields.
    pub fn validate(&self) -> Result<()> {
        if self.collection_id.is_empty() {
            return Err(n0_error::anyerr!("collection_id must not be empty"));
        }
        if !valid_identifier(&self.collection_id) {
            return Err(n0_error::anyerr!(
                "collection_id contains unsafe characters"
            ));
        }
        if self.name.is_empty() {
            return Err(n0_error::anyerr!("name must not be empty"));
        }
        if self.name.len() > MAX_COLLECTION_NAME_LENGTH || !valid_display_text(&self.name) {
            return Err(n0_error::anyerr!(
                "name is too long or contains control characters"
            ));
        }
        if let Some(ref desc) = self.description {
            if desc.len() > MAX_DESCRIPTION_LENGTH || !valid_description_text(desc) {
                return Err(n0_error::anyerr!(
                    "description is too long or contains disallowed control/format characters"
                ));
            }
        }
        Ok(())
    }
}

// ── CatalogueView ────────────────────────────────────────────────────────

/// A filtered view of a catalogue for a specific requester, used for
/// validation and content-hash computation before signing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogueView {
    /// Collections visible to the requester.
    pub collections: Vec<RemoteCollection>,
    /// Files visible to the requester.
    pub files: Vec<RemoteSharedFile>,
}
