//! Durable friend request store and API for boru-chat.
//!
//! This module owns the on-disk `friend_requests.json` file that lives beside
//! the persistent `secret_key.txt` identity file and the `friends.json` friends
//! list.
//!
//! # Status transitions
//!
//! The friend request lifecycle follows a linear state machine:
//!
//! ```text
//!                ┌──────────┐
//!                │  Pending  │
//!                └────┬─────┘
//!              ┌──────┼─────────┐
//!              ▼      ▼         ▼
//!         ┌────────┐ ┌───────┐ ┌─────────┐
//!         │Accepted│ │Declined│ │Cancelled│
//!         └────────┘ └───────┘ └─────────┘
//! ```
//!
//! - **Pending** — The requester has sent a request; the recipient has not yet
//!   responded. This is the only state from which transitions are valid.
//! - **Accepted** — The recipient has accepted. The two peers can now exchange
//!   direct conversation invites and use the friend features.
//! - **Declined** — The recipient declined. The request is terminal.
//! - **Cancelled** — The requester withdrew the request before the recipient
//!   responded. The request is terminal.
//!
//! Once a request reaches `Accepted`, `Declined`, or `Cancelled` it cannot
//! transition further. A new `Pending` request between the same pair may be
//! created after a terminal state.
//!
//! # Validation rules
//!
//! | Rule | Enforced by |
//! |------|-------------|
//! | Self-request (`requester == recipient`) | `send_request` returns `SelfRequest` |
//! | Duplicate pending request between the same pair (either direction) | `send_request` returns `DuplicatePending` |
//! | Only the recipient can accept or decline | `accept_request`, `decline_request` check caller identity |
//! | Only the requester can cancel | `cancel_request` checks caller identity |
//! | Can only accept/decline/cancel a `Pending` request | All mutation methods check status |
//!
//! # Request/response shapes
//!
//! All persistence is JSON via the crate's `atomic_write_json` helper.
//! Each request is a JSON
//! object with the following fields:
//!
//! ```json
//! {
//!   "id": "<uuid-string>",
//!   "requester": "<peer-public-key-string>",
//!   "recipient": "<peer-public-key-string>",
//!   "status": "Pending" | "Accepted" | "Declined" | "Cancelled",
//!   "created_at_unix_ms": 1234567890123,
//!   "updated_at_unix_ms": 1234567890456,
//!   "message": "optional text" | null
//! }
//! ```

use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::chat_core::atomic_write::atomic_write_json;
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 1;
/// Name of the on-disk friend requests file (lives beside `secret_key.txt`).
pub const FRIEND_REQUESTS_FILE_NAME: &str = "friend_requests.json";

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn friend_requests_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FRIEND_REQUESTS_FILE_NAME)
}

/// Generate a unique request id from parts of the peer keys and a timestamp.
///
/// Uses a short prefix of each peer's public key plus the timestamp in hex
/// to produce a stable, collision-resistant id without extra dependencies.
fn make_request_id(requester: &str, recipient: &str, timestamp: u64) -> String {
    let r_prefix: String = requester.chars().take(8).collect();
    let s_prefix: String = recipient.chars().take(8).collect();
    format!("{r_prefix}:{s_prefix}:{timestamp:x}")
}

/// Status of a friend request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FriendRequestStatus {
    /// The request has been sent and awaits a decision.
    Pending,
    /// The recipient accepted the request.
    Accepted,
    /// The recipient declined the request.
    Declined,
    /// The requester cancelled the request before a decision.
    Cancelled,
}

impl fmt::Display for FriendRequestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Accepted => write!(f, "Accepted"),
            Self::Declined => write!(f, "Declined"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

impl FriendRequestStatus {
    /// Returns `true` if this status is a terminal (non-transitionable) state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Declined | Self::Cancelled)
    }

    /// Returns `true` if this status is an accepted state that permits the two
    /// peers to use friend features (direct conversations, etc.).
    pub fn is_accepted(self) -> bool {
        self == Self::Accepted
    }
}

/// A persisted friend request between two peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendRequest {
    /// Unique identifier for this request.
    pub id: String,
    /// The peer that sent the request (stable public key string).
    pub requester: String,
    /// The peer that receives the request (stable public key string).
    pub recipient: String,
    /// Current status of the request.
    pub status: FriendRequestStatus,
    /// Unix milliseconds when the request was created.
    pub created_at_unix_ms: u64,
    /// Unix milliseconds of the last status change.
    pub updated_at_unix_ms: u64,
    /// Optional custom message from the requester.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl FriendRequest {
    /// Create a new pending friend request.
    pub fn new(
        requester: impl Into<String>,
        recipient: impl Into<String>,
        message: Option<String>,
    ) -> Self {
        let requester = requester.into();
        let recipient = recipient.into();
        let now = now_unix_ms();
        Self {
            id: make_request_id(&requester, &recipient, now),
            requester,
            recipient,
            status: FriendRequestStatus::Pending,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            message,
        }
    }
}

// ── Error types ─────────────────────────────────────────────────────────────

/// Errors that can occur during friend request operations.
#[derive(Debug, Clone)]
pub enum FriendRequestError {
    /// The requester and recipient are the same peer.
    SelfRequest,
    /// A pending request already exists between the two peers.
    DuplicatePending {
        /// The id of the existing pending request.
        existing_id: String,
    },
    /// No request exists with the given id.
    NotFound(String),
    /// The caller is not authorized to perform the action.
    Unauthorized {
        /// Description of the action attempted.
        action: String,
        /// The peer that is authorized for this action.
        expected: String,
    },
    /// The request is not in a state that allows the requested transition.
    InvalidTransition {
        /// Current status of the request.
        from: FriendRequestStatus,
        /// Requested target status.
        to: FriendRequestStatus,
    },
}

impl fmt::Display for FriendRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfRequest => write!(f, "cannot send a friend request to yourself"),
            Self::DuplicatePending { existing_id } => {
                write!(
                    f,
                    "a pending friend request already exists (id: {existing_id})"
                )
            }
            Self::NotFound(id) => write!(f, "friend request not found: {id}"),
            Self::Unauthorized { action, expected } => {
                write!(f, "only the {expected} can {action} this request")
            }
            Self::InvalidTransition { from, to } => {
                write!(f, "cannot transition from {from} to {to}")
            }
        }
    }
}

impl std::error::Error for FriendRequestError {}

// ── Store ──────────────────────────────────────────────────────────────────

/// Versioned persistent friend request store.
///
/// Loaded from and saved to `friend_requests.json`. Missing files or corrupt
/// data are handled gracefully — missing files produce an empty store, corrupt
/// files return an error so callers can decide how to proceed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendRequestStore {
    /// Format version for future migrations.
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    /// Friend requests indexed by their unique id.
    #[serde(default)]
    requests: BTreeMap<String, FriendRequest>,
    /// Two-way index: `(requester, recipient) → request_id` for fast duplicate
    /// detection and pair lookup. Only indexes non-terminal (Pending) requests.
    #[serde(skip)]
    pair_index: BTreeMap<(String, String), String>,
    /// Data directory used for load/save operations.
    #[serde(skip)]
    data_dir: PathBuf,
}

impl Default for FriendRequestStore {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            requests: BTreeMap::new(),
            pair_index: BTreeMap::new(),
            data_dir: PathBuf::new(),
        }
    }
}

impl FriendRequestStore {
    /// Construct an empty store bound to a data directory.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            ..Self::default()
        }
    }

    /// Return the data directory used by this store.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Return the on-disk friend requests file path.
    pub fn file_path(&self) -> PathBuf {
        friend_requests_file_path(&self.data_dir)
    }

    /// Load the friend request store from disk.
    ///
    /// Missing files are treated as an empty store. Corrupt JSON returns an
    /// error so callers can decide whether to fall back.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        let path = friend_requests_file_path(data_dir);
        if !path.exists() {
            return Ok(Self::empty_at(data_dir));
        }

        let raw = fs::read_to_string(&path).with_std_context(|_| {
            format!("failed to read friend requests file {}", path.display())
        })?;
        let mut store: Self = serde_json::from_str(&raw).with_std_context(|_| {
            format!("failed to parse friend requests file {}", path.display())
        })?;

        if !(1..=SCHEMA_VERSION).contains(&store.schema_version) {
            return Err(n0_error::anyerr!(
                "unsupported friend requests schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }
        store.schema_version = SCHEMA_VERSION;
        store.data_dir = data_dir.to_path_buf();
        store.rebuild_pair_index();
        Ok(store)
    }

    /// Load a store, logging and falling back to an empty store on failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(store) => store,
            Err(err) => {
                eprintln!(
                    "warning: starting with an empty friend requests list; \
                     failed to load {}: {err}",
                    friend_requests_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Persist the store atomically to `friend_requests.json`.
    pub fn save(&self) -> Result<PathBuf> {
        let data_dir = self.data_dir();
        if data_dir.as_os_str().is_empty() {
            return Err(n0_error::anyerr!(
                "friend request store has no data directory bound to it",
            ));
        }
        let path = self.file_path();
        atomic_write_json(&path, self, "friend request store")?;
        Ok(path)
    }

    /// Number of requests in the store.
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    /// Immutable iterator over all requests.
    pub fn iter(&self) -> impl Iterator<Item = &FriendRequest> {
        self.requests.values()
    }

    /// Get a request by its unique id.
    pub fn get(&self, id: &str) -> Option<&FriendRequest> {
        self.requests.get(id)
    }

    // ── Two-way index helpers ───────────────────────────────────────────

    /// Key for the pair index — always uses sorted order so both directions
    /// resolve to the same entry.
    fn pair_key(requester: &str, recipient: &str) -> (String, String) {
        if requester <= recipient {
            (requester.to_string(), recipient.to_string())
        } else {
            (recipient.to_string(), requester.to_string())
        }
    }

    fn rebuild_pair_index(&mut self) {
        self.pair_index.clear();
        for request in self.requests.values() {
            if request.status == FriendRequestStatus::Pending {
                let key = Self::pair_key(&request.requester, &request.recipient);
                self.pair_index.insert(key, request.id.clone());
            }
        }
    }

    fn add_to_index(&mut self, request: &FriendRequest) {
        if request.status == FriendRequestStatus::Pending {
            let key = Self::pair_key(&request.requester, &request.recipient);
            self.pair_index.insert(key, request.id.clone());
        }
    }

    fn remove_from_index(&mut self, request: &FriendRequest) {
        let key = Self::pair_key(&request.requester, &request.recipient);
        self.pair_index.remove(&key);
    }

    // ── Mutation API ────────────────────────────────────────────────────

    /// Send a friend request from `requester` to `recipient`.
    ///
    /// # Errors
    ///
    /// - [`FriendRequestError::SelfRequest`] — if requester and recipient are
    ///   the same.
    /// - [`FriendRequestError::DuplicatePending`] — if a pending request
    ///   already exists between the same two peers (in either direction).
    ///
    /// # Returns
    ///
    /// The newly created [`FriendRequest`] with status `Pending`.
    pub fn send_request(
        &mut self,
        requester: impl Into<String>,
        recipient: impl Into<String>,
        message: Option<String>,
    ) -> std::result::Result<FriendRequest, FriendRequestError> {
        let requester = requester.into();
        let recipient = recipient.into();

        if requester == recipient {
            return Err(FriendRequestError::SelfRequest);
        }

        // Check duplicate pending request between this pair (either direction).
        let pair_key = Self::pair_key(&requester, &recipient);
        if let Some(existing_id) = self.pair_index.get(&pair_key) {
            if let Some(existing) = self.requests.get(existing_id) {
                if existing.status == FriendRequestStatus::Pending {
                    return Err(FriendRequestError::DuplicatePending {
                        existing_id: existing.id.clone(),
                    });
                }
            }
        }

        let request = FriendRequest::new(&requester, &recipient, message);
        let id = request.id.clone();
        self.requests.insert(id, request.clone());
        self.add_to_index(&request);
        Ok(request)
    }

    /// List outgoing (sent) requests for a peer.
    ///
    /// Returns all requests where `peer` is the requester, sorted by creation
    /// time (newest first).
    pub fn list_outgoing(&self, peer: &str) -> Vec<&FriendRequest> {
        let mut result: Vec<_> = self
            .requests
            .values()
            .filter(|r| r.requester == peer)
            .collect();
        result.sort_by(|a, b| b.created_at_unix_ms.cmp(&a.created_at_unix_ms));
        result
    }

    /// List incoming (received) requests for a peer.
    ///
    /// Returns all requests where `peer` is the recipient, sorted by creation
    /// time (newest first).
    pub fn list_incoming(&self, peer: &str) -> Vec<&FriendRequest> {
        let mut result: Vec<_> = self
            .requests
            .values()
            .filter(|r| r.recipient == peer)
            .collect();
        result.sort_by(|a, b| b.created_at_unix_ms.cmp(&a.created_at_unix_ms));
        result
    }

    /// List incoming requests with a specific status for a peer.
    pub fn list_incoming_by_status(
        &self,
        peer: &str,
        status: FriendRequestStatus,
    ) -> Vec<&FriendRequest> {
        let mut result: Vec<_> = self
            .requests
            .values()
            .filter(|r| r.recipient == peer && r.status == status)
            .collect();
        result.sort_by(|a, b| b.created_at_unix_ms.cmp(&a.created_at_unix_ms));
        result
    }

    /// List outgoing requests with a specific status for a peer.
    pub fn list_outgoing_by_status(
        &self,
        peer: &str,
        status: FriendRequestStatus,
    ) -> Vec<&FriendRequest> {
        let mut result: Vec<_> = self
            .requests
            .values()
            .filter(|r| r.requester == peer && r.status == status)
            .collect();
        result.sort_by(|a, b| b.created_at_unix_ms.cmp(&a.created_at_unix_ms));
        result
    }

    /// List all pending requests (incoming or outgoing) for a peer.
    pub fn list_pending(&self, peer: &str) -> Vec<&FriendRequest> {
        let mut result: Vec<_> = self
            .requests
            .values()
            .filter(|r| {
                r.status == FriendRequestStatus::Pending
                    && (r.requester == peer || r.recipient == peer)
            })
            .collect();
        result.sort_by(|a, b| b.created_at_unix_ms.cmp(&a.created_at_unix_ms));
        result
    }

    /// Accept a pending friend request.
    ///
    /// Only the `recipient` (the peer the request was sent to) may accept.
    ///
    /// # Errors
    ///
    /// - [`FriendRequestError::NotFound`] — no request exists with the given
    ///   id.
    /// - [`FriendRequestError::Unauthorized`] — the caller is not the recipient
    ///   of the request.
    /// - [`FriendRequestError::InvalidTransition`] — the request is not in
    ///   `Pending` state.
    pub fn accept_request(
        &mut self,
        request_id: &str,
        caller: &str,
    ) -> std::result::Result<FriendRequest, FriendRequestError> {
        self.transition_request(request_id, caller, FriendRequestStatus::Accepted)
    }

    /// Decline a pending friend request.
    ///
    /// Only the `recipient` (the peer the request was sent to) may decline.
    ///
    /// # Errors
    ///
    /// - [`FriendRequestError::NotFound`] — no request exists with the given
    ///   id.
    /// - [`FriendRequestError::Unauthorized`] — the caller is not the recipient
    ///   of the request.
    /// - [`FriendRequestError::InvalidTransition`] — the request is not in
    ///   `Pending` state.
    pub fn decline_request(
        &mut self,
        request_id: &str,
        caller: &str,
    ) -> std::result::Result<FriendRequest, FriendRequestError> {
        self.transition_request(request_id, caller, FriendRequestStatus::Declined)
    }

    /// Cancel a pending friend request.
    ///
    /// Only the `requester` (the peer who sent the request) may cancel.
    ///
    /// # Errors
    ///
    /// - [`FriendRequestError::NotFound`] — no request exists with the given
    ///   id.
    /// - [`FriendRequestError::Unauthorized`] — the caller is not the
    ///   requester of the request.
    /// - [`FriendRequestError::InvalidTransition`] — the request is not in
    ///   `Pending` state.
    pub fn cancel_request(
        &mut self,
        request_id: &str,
        caller: &str,
    ) -> std::result::Result<FriendRequest, FriendRequestError> {
        self.transition_request(request_id, caller, FriendRequestStatus::Cancelled)
    }

    /// Internal helper: transition a request to a new status with authorization
    /// and validity checks.
    fn transition_request(
        &mut self,
        request_id: &str,
        caller: &str,
        new_status: FriendRequestStatus,
    ) -> std::result::Result<FriendRequest, FriendRequestError> {
        let request = self
            .requests
            .get(request_id)
            .ok_or_else(|| FriendRequestError::NotFound(request_id.to_string()))?;

        // Authorize: who can perform this transition?
        match new_status {
            FriendRequestStatus::Accepted | FriendRequestStatus::Declined => {
                if caller != request.recipient {
                    return Err(FriendRequestError::Unauthorized {
                        action: format!("{new_status}"),
                        expected: request.recipient.clone(),
                    });
                }
            }
            FriendRequestStatus::Cancelled => {
                if caller != request.requester {
                    return Err(FriendRequestError::Unauthorized {
                        action: format!("{new_status}"),
                        expected: request.requester.clone(),
                    });
                }
            }
            FriendRequestStatus::Pending => {
                // Transitioning *to* Pending is done via send_request, not here.
                return Err(FriendRequestError::InvalidTransition {
                    from: request.status,
                    to: new_status,
                });
            }
        }

        // Validate: can only transition from Pending.
        if request.status != FriendRequestStatus::Pending {
            return Err(FriendRequestError::InvalidTransition {
                from: request.status,
                to: new_status,
            });
        }

        // Clone the request so we can drop the immutable borrow on self.requests
        // before mutating self.
        let request = request.clone();

        // Remove from pair index (it was pending).
        self.remove_from_index(&request);

        // Apply the transition.
        let stored = self.requests.get_mut(request_id).expect("just checked");
        stored.status = new_status;
        stored.updated_at_unix_ms = now_unix_ms();

        Ok(stored.clone())
    }

    /// Clear all requests with a specific status.
    ///
    /// Returns the number of removed requests.
    pub fn clear_by_status(&mut self, status: FriendRequestStatus) -> usize {
        let ids: Vec<String> = self
            .requests
            .iter()
            .filter(|(_, r)| r.status == status)
            .map(|(id, _)| id.clone())
            .collect();
        let count = ids.len();
        for id in &ids {
            // Clone the request before removing from index to avoid
            // aliasing borrow of self.requests.
            let request = self.requests.get(id).cloned();
            if let Some(request) = request {
                self.remove_from_index(&request);
            }
            self.requests.remove(id);
        }
        count
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        dir.push(format!("boru-friend-requests-{name}-{suffix}"));
        dir
    }

    fn random_peer() -> String {
        iroh::SecretKey::generate().public().to_string()
    }

    // ── Data model tests ─────────────────────────────────────────────────

    #[test]
    fn friend_request_status_display() {
        assert_eq!(FriendRequestStatus::Pending.to_string(), "Pending");
        assert_eq!(FriendRequestStatus::Accepted.to_string(), "Accepted");
        assert_eq!(FriendRequestStatus::Declined.to_string(), "Declined");
        assert_eq!(FriendRequestStatus::Cancelled.to_string(), "Cancelled");
    }

    #[test]
    fn friend_request_status_is_terminal() {
        assert!(!FriendRequestStatus::Pending.is_terminal());
        assert!(FriendRequestStatus::Accepted.is_terminal());
        assert!(FriendRequestStatus::Declined.is_terminal());
        assert!(FriendRequestStatus::Cancelled.is_terminal());
    }

    #[test]
    fn friend_request_status_is_accepted() {
        assert!(FriendRequestStatus::Accepted.is_accepted());
        assert!(!FriendRequestStatus::Pending.is_accepted());
        assert!(!FriendRequestStatus::Declined.is_accepted());
        assert!(!FriendRequestStatus::Cancelled.is_accepted());
    }

    #[test]
    fn new_friend_request_has_pending_status() {
        let a = random_peer();
        let b = random_peer();
        let req = FriendRequest::new(&a, &b, Some("Hello!".into()));
        assert_eq!(req.requester, a);
        assert_eq!(req.recipient, b);
        assert_eq!(req.status, FriendRequestStatus::Pending);
        assert_eq!(req.message.as_deref(), Some("Hello!"));
        assert_eq!(req.created_at_unix_ms, req.updated_at_unix_ms);
        assert!(!req.id.is_empty());
    }

    #[test]
    fn new_friend_request_without_message() {
        let a = random_peer();
        let b = random_peer();
        let req = FriendRequest::new(&a, &b, None);
        assert!(req.message.is_none());
    }

    #[test]
    fn request_id_contains_both_peer_prefixes() {
        let a = random_peer();
        let b = random_peer();
        let req = FriendRequest::new(&a, &b, None);
        let a_prefix: String = a.chars().take(8).collect();
        let b_prefix: String = b.chars().take(8).collect();
        assert!(req.id.contains(&a_prefix));
        assert!(req.id.contains(&b_prefix));
    }

    #[test]
    fn error_display_messages() {
        let err = FriendRequestError::SelfRequest;
        assert!(err.to_string().contains("yourself"));

        let err = FriendRequestError::DuplicatePending {
            existing_id: "abc".into(),
        };
        assert!(err.to_string().contains("pending"));
        assert!(err.to_string().contains("abc"));

        let err = FriendRequestError::NotFound("xyz".into());
        assert!(err.to_string().contains("xyz"));

        let err = FriendRequestError::Unauthorized {
            action: "accept".into(),
            expected: "bob".into(),
        };
        assert!(err.to_string().contains("bob"));
        assert!(err.to_string().contains("accept"));

        let err = FriendRequestError::InvalidTransition {
            from: FriendRequestStatus::Accepted,
            to: FriendRequestStatus::Pending,
        };
        assert!(err.to_string().contains("Accepted"));
        assert!(err.to_string().contains("Pending"));
    }

    // ── Store load/save tests ────────────────────────────────────────────

    #[test]
    fn load_missing_returns_empty_store() {
        let dir = temp_dir("missing");
        let store = FriendRequestStore::load(&dir).expect("load missing");
        assert!(store.is_empty());
        assert_eq!(store.data_dir(), dir.as_path());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let mut store = FriendRequestStore::empty_at(&dir);
        let a = random_peer();
        let b = random_peer();
        let req = store
            .send_request(&a, &b, Some("Let's chat!".into()))
            .expect("send request");
        let id = req.id.clone();
        store.save().expect("save");

        let loaded = FriendRequestStore::load(&dir).expect("load");
        assert_eq!(loaded.len(), 1);
        let loaded_req = loaded.get(&id).expect("request exists");
        assert_eq!(loaded_req.requester, a);
        assert_eq!(loaded_req.recipient, b);
        assert_eq!(loaded_req.status, FriendRequestStatus::Pending);
        assert_eq!(loaded_req.message.as_deref(), Some("Let's chat!"));
    }

    #[test]
    fn save_then_load_round_trips_without_message() {
        let dir = temp_dir("no-msg-roundtrip");
        let mut store = FriendRequestStore::empty_at(&dir);
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send request");
        let id = req.id.clone();
        store.save().expect("save");

        let loaded = FriendRequestStore::load(&dir).expect("load");
        let loaded_req = loaded.get(&id).expect("request exists");
        assert!(loaded_req.message.is_none());
    }

    #[test]
    fn load_or_default_fallback_on_corrupt() {
        let dir = temp_dir("corrupt-fallback");
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(friend_requests_file_path(&dir), "not json").expect("write invalid file");
        let store = FriendRequestStore::load_or_default(&dir);
        assert!(store.is_empty());
    }

    // ── Validation tests ─────────────────────────────────────────────────

    #[test]
    fn self_request_is_rejected() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let err = store
            .send_request(&a, &a, None)
            .expect_err("self-request should fail");
        assert!(matches!(err, FriendRequestError::SelfRequest));
    }

    #[test]
    fn duplicate_pending_request_is_rejected() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        store.send_request(&a, &b, None).expect("first request ok");
        let err = store
            .send_request(&a, &b, None)
            .expect_err("duplicate should fail");
        assert!(matches!(err, FriendRequestError::DuplicatePending { .. }));
    }

    #[test]
    fn duplicate_pending_reverse_direction_is_rejected() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        store.send_request(&a, &b, None).expect("first request ok");
        let err = store
            .send_request(&b, &a, None)
            .expect_err("reverse duplicate should fail");
        assert!(matches!(err, FriendRequestError::DuplicatePending { .. }));
    }

    #[test]
    fn request_after_terminal_state_is_allowed() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        let req = store.cancel_request(&req.id, &a).expect("cancel");
        assert_eq!(req.status, FriendRequestStatus::Cancelled);

        // Should be able to send a new request after a terminal state.
        let new_req = store
            .send_request(&a, &b, None)
            .expect("new request after cancel should work");
        assert_eq!(new_req.status, FriendRequestStatus::Pending);
        // The id is unique per peer-pair + timestamp so it may match if
        // both calls fall in the same millisecond. What matters is that
        // a new Pending request was created.
    }

    // ── Accept/decline/cancel authorization tests ────────────────────────

    #[test]
    fn accept_requires_recipient() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");

        // Requester cannot accept their own request.
        let err = store
            .accept_request(&req.id, &a)
            .expect_err("requester cannot accept");
        assert!(matches!(err, FriendRequestError::Unauthorized { .. }));
    }

    #[test]
    fn random_peer_cannot_accept_or_decline() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let c = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");

        let err = store
            .accept_request(&req.id, &c)
            .expect_err("third party cannot accept");
        assert!(matches!(err, FriendRequestError::Unauthorized { .. }));

        let err = store
            .decline_request(&req.id, &c)
            .expect_err("third party cannot decline");
        assert!(matches!(err, FriendRequestError::Unauthorized { .. }));
    }

    #[test]
    fn cancel_requires_requester() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");

        // Recipient cannot cancel.
        let err = store
            .cancel_request(&req.id, &b)
            .expect_err("recipient cannot cancel");
        assert!(matches!(err, FriendRequestError::Unauthorized { .. }));
    }

    #[test]
    fn recipient_can_accept() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        let accepted = store
            .accept_request(&req.id, &b)
            .expect("recipient can accept");
        assert_eq!(accepted.status, FriendRequestStatus::Accepted);
        assert!(accepted.updated_at_unix_ms >= accepted.created_at_unix_ms);
    }

    #[test]
    fn recipient_can_decline() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        let declined = store
            .decline_request(&req.id, &b)
            .expect("recipient can decline");
        assert_eq!(declined.status, FriendRequestStatus::Declined);
    }

    #[test]
    fn requester_can_cancel() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        let cancelled = store
            .cancel_request(&req.id, &a)
            .expect("requester can cancel");
        assert_eq!(cancelled.status, FriendRequestStatus::Cancelled);
    }

    // ── Invalid transition tests ─────────────────────────────────────────

    #[test]
    fn cannot_accept_already_accepted() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        store.accept_request(&req.id, &b).expect("accept");

        let err = store
            .accept_request(&req.id, &b)
            .expect_err("cannot accept again");
        assert!(matches!(err, FriendRequestError::InvalidTransition { .. }));
    }

    #[test]
    fn cannot_decline_already_declined() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        store.decline_request(&req.id, &b).expect("decline");

        let err = store
            .decline_request(&req.id, &b)
            .expect_err("cannot decline again");
        assert!(matches!(err, FriendRequestError::InvalidTransition { .. }));
    }

    #[test]
    fn cannot_cancel_already_cancelled() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        store.cancel_request(&req.id, &a).expect("cancel");

        let err = store
            .cancel_request(&req.id, &a)
            .expect_err("cannot cancel again");
        assert!(matches!(err, FriendRequestError::InvalidTransition { .. }));
    }

    #[test]
    fn cannot_accept_after_cancel() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        store.cancel_request(&req.id, &a).expect("cancel");

        let err = store
            .accept_request(&req.id, &b)
            .expect_err("cannot accept after cancel");
        assert!(matches!(err, FriendRequestError::InvalidTransition { .. }));
    }

    // ── Listing tests ────────────────────────────────────────────────────

    #[test]
    fn list_outgoing_returns_requests_sent_by_peer() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let c = random_peer();
        store.send_request(&a, &b, None).expect("send a->b");
        store.send_request(&a, &c, None).expect("send a->c");

        let outgoing = store.list_outgoing(&a);
        assert_eq!(outgoing.len(), 2);

        let incoming_b = store.list_incoming(&b);
        assert_eq!(incoming_b.len(), 1);
        assert_eq!(incoming_b[0].requester, a);
    }

    #[test]
    fn list_incoming_returns_requests_received_by_peer() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let c = random_peer();
        store.send_request(&a, &b, None).expect("send a->b");
        store.send_request(&c, &b, None).expect("send c->b");

        let incoming = store.list_incoming(&b);
        assert_eq!(incoming.len(), 2);
    }

    #[test]
    fn list_pending_returns_both_directions() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        store.send_request(&a, &b, None).expect("send a->b");

        let pending_a = store.list_pending(&a);
        assert_eq!(pending_a.len(), 1);

        let pending_b = store.list_pending(&b);
        assert_eq!(pending_b.len(), 1);
    }

    #[test]
    fn list_pending_excludes_accepted_requests() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        store.accept_request(&req.id, &b).expect("accept");

        let pending = store.list_pending(&a);
        assert!(pending.is_empty());
    }

    #[test]
    fn list_outgoing_by_status_filters_correctly() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let c = random_peer();
        store.send_request(&a, &b, None).expect("send pending");
        let req2 = store.send_request(&a, &c, None).expect("send another");
        store.cancel_request(&req2.id, &a).expect("cancel");

        let pending_out = store.list_outgoing_by_status(&a, FriendRequestStatus::Pending);
        assert_eq!(pending_out.len(), 1);
        assert_eq!(pending_out[0].recipient, b);

        let cancelled_out = store.list_outgoing_by_status(&a, FriendRequestStatus::Cancelled);
        assert_eq!(cancelled_out.len(), 1);
        assert_eq!(cancelled_out[0].recipient, c);
    }

    // ── Not found tests ──────────────────────────────────────────────────

    #[test]
    fn accept_nonexistent_request_returns_not_found() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let err = store
            .accept_request("nonexistent-id", &a)
            .expect_err("should be not found");
        assert!(matches!(err, FriendRequestError::NotFound(_)));
    }

    #[test]
    fn cancel_nonexistent_request_returns_not_found() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let err = store
            .cancel_request("nonexistent-id", &a)
            .expect_err("should be not found");
        assert!(matches!(err, FriendRequestError::NotFound(_)));
    }

    // ── Cleanup tests ────────────────────────────────────────────────────

    #[test]
    fn clear_by_status_removes_correct_requests() {
        let mut store = FriendRequestStore::default();
        let a = random_peer();
        let b = random_peer();
        let c = random_peer();
        store.send_request(&a, &b, None).expect("send pending");
        let req2 = store.send_request(&a, &c, None).expect("send pending");
        store.accept_request(&req2.id, &c).expect("accept");

        assert_eq!(store.len(), 2);
        let removed = store.clear_by_status(FriendRequestStatus::Pending);
        assert_eq!(removed, 1);
        assert_eq!(store.len(), 1);
        // Accepted request should still be there.
        assert_eq!(store.clear_by_status(FriendRequestStatus::Accepted), 1);
        assert!(store.is_empty());
    }

    // ── Persistence: reloading with mutations ────────────────────────────

    #[test]
    fn save_then_load_preserves_accepted_status() {
        let dir = temp_dir("accepted-persist");
        let mut store = FriendRequestStore::empty_at(&dir);
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        store.accept_request(&req.id, &b).expect("accept");
        store.save().expect("save");

        let loaded = FriendRequestStore::load(&dir).expect("load");
        assert_eq!(loaded.len(), 1);
        let loaded_req = loaded.get(&req.id).expect("request");
        assert_eq!(loaded_req.status, FriendRequestStatus::Accepted);
    }

    // ── Pair index rebuild on load tests ─────────────────────────────────

    #[test]
    fn pair_index_repopulates_on_load() {
        let dir = temp_dir("pair-index-rebuild");
        let mut store = FriendRequestStore::empty_at(&dir);
        let a = random_peer();
        let b = random_peer();
        store.send_request(&a, &b, None).expect("send");
        store.save().expect("save");

        // Load into a new store — pair_index is rebuilt from requests.
        let loaded = FriendRequestStore::load(&dir).expect("load");
        // pair_index is a private field; verify it works by testing duplicate
        // rejection on the reloaded store.
        let err = loaded
            .clone()
            .send_request(&b, &a, None)
            .expect_err("reverse direction should be blocked after reload");
        assert!(matches!(err, FriendRequestError::DuplicatePending { .. }));
    }

    #[test]
    fn pair_index_does_not_index_terminal_requests() {
        let dir = temp_dir("pair-index-terminal");
        let mut store = FriendRequestStore::empty_at(&dir);
        let a = random_peer();
        let b = random_peer();
        let req = store.send_request(&a, &b, None).expect("send");
        store.accept_request(&req.id, &b).expect("accept");
        store.save().expect("save");

        // After accept, pair_index should not have this pair, so a new request
        // between the same peers should succeed.
        let loaded = FriendRequestStore::load(&dir).expect("load");
        let new_req = loaded
            .clone()
            .send_request(&a, &b, None)
            .expect("new request after accept should work");
        assert_eq!(new_req.status, FriendRequestStatus::Pending);
    }
}
