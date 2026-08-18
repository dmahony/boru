//! Remote-safe representation of shared file entries for wire transfer.
//!
//! [`RemoteSharedFile`](crate::catalogue_model::RemoteSharedFile) is the wire-friendly counterpart of
//! [`crate::user_profile::SharedFile`] — it strips local-only fields
//! (paths, database row IDs, blob tickets, permissions) so that file
//! metadata can be safely transmitted to remote peers.
//!
//! The module also provides [`SignedFileCatalogue`](crate::catalogue_model::SignedFileCatalogue) (a signed collection
//! of [`RemoteSharedFile`](crate::catalogue_model::RemoteSharedFile) entries) and `FileCatalogueCollection`
//! (logical groupings of shared files).
//!
//! # Layout
//!
//! This module is a facade over the catalogue data model:
//! - [`file`]       – [`RemoteSharedFile`] + its `TryFrom<SharedFile>` mapping
//! - [`collection`] – [`FileCatalogueCollection`]
//! - [`signed`]     – [`SignedFileCatalogue`] (signing + verification)
//! - [`view`]       – [`RemoteCollection`] + [`CatalogueView`]
//! - [`cursor`]     – [`SignedCatalogueCursor`]
//! - [`tests`]      – unit tests
//!
//! The shared size/format constants and the pure validation helpers live
//! here so every submodule can reference them without duplication.

mod collection;
mod cursor;
mod file;
mod signed;
mod view;

#[cfg(test)]
pub(crate) mod tests;

pub use collection::FileCatalogueCollection;
pub use cursor::SignedCatalogueCursor;
pub use file::{LocalPathError, RemoteSharedFile};
pub use signed::SignedFileCatalogue;
pub use view::{CatalogueView, RemoteCollection};

// ── Constants & shared validation helpers ────────────────────────────────────

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) use crate::user_profile::SharedFile;

/// Maximum length of a `shared_file_id`.
pub const MAX_SHARED_FILE_ID_LENGTH: usize = 256;

/// Maximum length of a `display_name`.
pub const MAX_DISPLAY_NAME_LENGTH: usize = 512;

/// Maximum UTF-8 byte length of a `description`.
pub const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// Maximum length of a `mime_type` string.
pub const MAX_MIME_TYPE_LENGTH: usize = 128;

/// Maximum length of a `content_hash` string.
pub const MAX_CONTENT_HASH_LENGTH: usize = 128;

/// Maximum number of collection IDs per file.
pub const MAX_COLLECTION_IDS: usize = 256;

/// Maximum length of a single collection ID string.
pub const MAX_COLLECTION_ID_LENGTH: usize = 256;

/// Maximum collection display-name length, in bytes.
pub const MAX_COLLECTION_NAME_LENGTH: usize = 512;

/// Remote timestamps may be modestly ahead because peers' clocks differ.
pub const MAX_TIMESTAMP_FUTURE_SKEW_MS: u64 = 24 * 60 * 60 * 1000;

pub(crate) const SIGNATURE_LEN: usize = iroh::Signature::LENGTH;

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn valid_display_text(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(|ch| ch.is_control())
}

/// Return whether a description contains only display-safe text.
///
/// Descriptions intentionally permit ordinary multiline formatting: tab,
/// carriage return, and line feed are allowed. All other Unicode control
/// characters, Unicode line/paragraph separators, and Unicode format
/// characters (including bidi overrides and zero-width characters) are
/// rejected so signed metadata cannot hide content or alter its visual order.
pub(crate) fn valid_description_text(value: &str) -> bool {
    value.chars().all(|ch| {
        let allowed_control = matches!(ch, '\t' | '\n' | '\r');
        let unicode_format = matches!(
            ch,
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        );
        (!ch.is_control() || allowed_control)
            && !matches!(ch, '\u{2028}' | '\u{2029}')
            && !unicode_format
    })
}

pub(crate) fn valid_mime_type(value: &str) -> bool {
    let Some((major, minor)) = value.split_once('/') else {
        return false;
    };
    let valid_token = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'-' | b'^' | b'_' | b'.' | b'+'
                    )
            })
    };
    value.is_ascii() && valid_token(major) && valid_token(minor)
}

pub(crate) fn timestamp_is_reasonable(timestamp_ms: u64) -> bool {
    timestamp_ms <= now_ms().saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS)
}
