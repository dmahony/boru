//! [`FileCatalogueCollection`] — a logical grouping of shared files.

use n0_error::Result;
use serde::{Deserialize, Serialize};

use super::{
    valid_description_text, valid_display_text, valid_identifier, MAX_COLLECTION_NAME_LENGTH,
    MAX_DESCRIPTION_LENGTH,
};

// ── FileCatalogueCollection ──────────────────────────────────────────────

/// A named group of shared files within a catalogue.
///
/// Used to organise files into logical collections (e.g. "photos",
/// "documents", "projects").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileCatalogueCollection {
    /// Unique identifier for this collection.
    pub collection_id: String,
    /// Human-readable display name.
    pub name: String,
    /// Optional description. Tab, CR, and LF are allowed for ordinary
    /// formatting; all other control, line/paragraph-separator, and Unicode
    /// format characters are rejected by [`Self::validate`].
    #[serde(default)]
    pub description: Option<String>,
}

impl FileCatalogueCollection {
    /// Validate the collection fields against length and content constraints.
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
