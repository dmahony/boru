//! Named-ring permission groups for file resources (iroh-rings borrow).
//!
//! A **ring** is a named set of peers (by [`FriendId`](crate::friends::FriendId)
//! string form) that share typed permissions — [`RingPermission`](crate::rings::RingPermission) — on file
//! resources (identified by content hash).  Rings are persisted in SQLite
//! via [`crate::storage::Storage`] and are enforced **request-time** in the
//! file-access handler *before* a [`SignedDownloadDescriptor`](crate::file_access_protocol::SignedDownloadDescriptor)
//! is ever issued.
//!
//! # Design notes (borrowed from iroh-rings, implemented locally)
//!
//! - A resource with **no ring association** is implicitly denied by the
//!   ring model: rings only ever grant what an owner explicitly associates.
//! - Ring grants are **additive** with the existing friend-relationship
//!   checks: a peer may be authorized either because they are a friend
//!   (existing behaviour) *or* because they belong to a ring that holds the
//!   requested permission on the resource.  Explicit per-peer `deny` grants
//!   still win over ring grants.
//! - The **open ring** (`is_open = true`) is a built-in ring that grants
//!   its associated permissions to *any authenticated peer* — no membership
//!   row is required.  By convention (and enforced at
//!   [`Storage::set_ring_permission`](crate::storage::Storage::set_ring_permission))
//!   the open ring is read-only: only `Read` may be granted on it.
//!
//! # Wire format
//!
//! This module deliberately does **not** change the `/boru-file-access/1`
//! wire format.  The existing [`FileAccessRequest`](crate::file_access_protocol::FileAccessRequest)
//! already carries the resource identity (`shared_file_id` /
//! `expected_content_hash`); the handler maps a download request to
//! [`RingPermission::Read`](crate::rings::RingPermission::Read) internally.

use serde::{Deserialize, Serialize};

/// Typed permission that a ring may hold on a file resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RingPermission {
    /// Read the file contents (authorizes download descriptor issuance).
    #[serde(rename = "read")]
    Read,
    /// Modify/replace the resource.
    #[serde(rename = "write")]
    Write,
    /// Delete the resource.
    #[serde(rename = "delete")]
    Delete,
}

impl RingPermission {
    /// All ring permissions, in stable order.
    pub const ALL: [RingPermission; 3] = [
        RingPermission::Read,
        RingPermission::Write,
        RingPermission::Delete,
    ];

    /// Stable snake_case string form, matching the SQLite `permission` column.
    pub fn as_str(self) -> &'static str {
        match self {
            RingPermission::Read => "read",
            RingPermission::Write => "write",
            RingPermission::Delete => "delete",
        }
    }

    /// Parse a stable string form back into a permission.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(RingPermission::Read),
            "write" => Some(RingPermission::Write),
            "delete" => Some(RingPermission::Delete),
            _ => None,
        }
    }
}

impl std::fmt::Display for RingPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A named ring owned by a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    /// Primary key row id.
    pub id: i64,
    /// Owning profile (hex-encoded public key).
    pub owner_user_id: String,
    /// Human-readable ring name (unique per owner).
    pub name: String,
    /// Whether this is the built-in open ring (grants to any authenticated peer).
    pub is_open: bool,
    /// Creation timestamp (ms since UNIX epoch).
    pub created_at_ms: u64,
    /// Last modification timestamp (ms since UNIX epoch).
    pub updated_at_ms: u64,
}

/// A member of a ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingMember {
    /// References `rings.id`.
    pub ring_id: i64,
    /// Peer id (string form of a [`FriendId`](crate::friends::FriendId)).
    pub member_user_id: String,
    /// When the member joined (ms since UNIX epoch).
    pub joined_at_ms: u64,
}

/// A typed permission associated with a resource inside a ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingResourcePermission {
    /// References `rings.id`.
    pub ring_id: i64,
    /// Resource identity — the content hash of the file.
    pub content_hash: String,
    /// The typed permission granted by this association.
    pub permission: RingPermission,
    /// When the association was created (ms since UNIX epoch).
    pub created_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_string_roundtrip() {
        for perm in RingPermission::ALL {
            assert_eq!(RingPermission::from_str(perm.as_str()), Some(perm));
            assert_eq!(perm.to_string(), perm.as_str());
        }
        assert_eq!(RingPermission::from_str("rename"), None);
        assert_eq!(RingPermission::from_str(""), None);
    }

    #[test]
    fn permission_serde_roundtrip() {
        for perm in RingPermission::ALL {
            let json = serde_json::to_string(&perm).expect("serialize");
            let back: RingPermission = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, perm);
        }
    }
}
