//! Canonical file-admission policy: size limits and extension allowlists.
//!
//! This module is the **single authoritative implementation** of the
//! "may this file be shared / announced?" gate. Every intake boundary that
//! applies a size ceiling or an extension allowlist MUST call [`admission`](crate::file_policy::admission)
//! instead of re-implementing the rule inline.
//!
//! # Why this module exists (BORU-AUDIT-20)
//!
//! The legacy profile module (`crate::user_profile::UserProfile`) used to
//! carry its own size/extension check (`is_file_announce_allowed`) while the
//! shared-folder indexer (`crate::file_indexer`) re-implemented the same
//! rule inline with subtly different semantics (case-insensitive matching
//! there, case-sensitive in the profile method; no leading-dot trimming in
//! the indexer). Security fixes had to be applied to two places and could
//! drift. All callers now go through [`admission`](crate::file_policy::admission), which is
//! case-insensitive and trims leading dots — the union of both behaviours.
//!
//! # Policy changes
//!
//! Change size-limit or extension-allowlist behaviour HERE, then update the
//! conformance matrix in `tests/test_policy_conformance.rs`.

/// Outcome of applying the size + extension admission policy to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileAdmission {
    /// The file exceeds the configured `max_file_size`.
    pub over_limit: bool,
    /// The file's extension is not in the configured allowlist.
    ///
    /// Only meaningful when the allowlist is non-empty — an empty
    /// allowlist means "all extensions allowed".
    pub extension_blocked: bool,
}

impl FileAdmission {
    /// Whether the file passes both gates and may be shared/announced.
    pub fn is_allowed(self) -> bool {
        !self.over_limit && !self.extension_blocked
    }
}

/// Evaluate the size + extension admission policy for a file.
///
/// * `size` — the file size in bytes.
/// * `extension` — the file extension **without** a leading dot (may be
///   empty for extension-less files). Comparison is case-insensitive after
///   trimming leading dots and surrounding whitespace.
/// * `max_file_size` — the size ceiling in bytes. Files strictly larger
///   than this are flagged `over_limit`.
/// * `allowed_extensions` — the extension allowlist. An **empty** list
///   means "all extensions allowed" (the extension gate is disabled); a
///   non-empty list admits only listed extensions.
///
/// This is the canonical implementation. Callers MUST NOT re-derive
/// `over_limit` / `extension_blocked` inline.
pub fn admission(
    size: u64,
    extension: &str,
    max_file_size: u64,
    allowed_extensions: &[String],
) -> FileAdmission {
    let over_limit = size > max_file_size;

    let extension_blocked = if allowed_extensions.is_empty() {
        false
    } else {
        let ext = extension.trim().trim_start_matches('.').to_lowercase();
        !allowed_extensions
            .iter()
            .any(|allowed| allowed.trim().eq_ignore_ascii_case(&ext))
    };

    FileAdmission {
        over_limit,
        extension_blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn accepts_when_enabled_and_in_limits() {
        let a = admission(512, "txt", 1024, &list(&["txt", "jpg"]));
        assert_eq!(
            a,
            FileAdmission {
                over_limit: false,
                extension_blocked: false,
            }
        );
        assert!(a.is_allowed());
        // Leading dot is trimmed (legacy accepted ".txt").
        assert!(admission(512, ".txt", 1024, &list(&["txt"])).is_allowed());
        // Extension comparison is case-insensitive.
        assert!(admission(512, "JPG", 1024, &list(&["jpg"])).is_allowed());
        assert!(admission(512, "jpg", 1024, &list(&["JPG"])).is_allowed());
    }

    #[test]
    fn rejects_over_max_size() {
        let a = admission(200, "txt", 100, &list(&["txt"]));
        assert!(a.over_limit);
        assert!(!a.is_allowed());
    }

    #[test]
    fn rejects_blocked_extension() {
        let a = admission(100, "jpg", 1024, &list(&["pdf"]));
        assert!(a.extension_blocked);
        assert!(!a.is_allowed());
    }

    #[test]
    fn empty_allowlist_allows_all() {
        let a = admission(100, "exe", 1024, &[]);
        assert!(!a.extension_blocked);
        assert!(a.is_allowed());
        let a = admission(100, "zip", 1024, &[]);
        assert!(a.is_allowed());
    }

    #[test]
    fn empty_extension_blocked_when_allowlist_nonempty() {
        // An extension-less file is blocked when an allowlist is configured
        // (its extension "" is not in the list) — matches the legacy rule.
        let a = admission(100, "", 1024, &list(&["txt"]));
        assert!(a.extension_blocked);
        assert!(!a.is_allowed());
    }

    #[test]
    fn empty_extension_allowed_when_allowlist_empty() {
        let a = admission(100, "", 1024, &[]);
        assert!(a.is_allowed());
    }

    #[test]
    fn size_boundary_is_strict_inequality() {
        // size == max is allowed; size > max is not.
        assert!(admission(100, "txt", 100, &list(&["txt"])).is_allowed());
        assert!(admission(101, "txt", 100, &list(&["txt"])).over_limit);
    }

    #[test]
    fn both_flags_can_be_set() {
        let a = admission(10_000, "jpg", 100, &list(&["txt"]));
        assert!(a.over_limit && a.extension_blocked);
        assert!(!a.is_allowed());
    }
}
