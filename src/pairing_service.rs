//! Pairing service — connects out-of-band [`PeerInvitation`] to the friend request
//! infrastructure.
//!
//! The main entry point is [`accept_peer_invitation`], which validates the
//! invitation, updates the friends store with the peer's addresses, creates a
//! pending friend request in the store, signs it, persists a [`PendingPairing`]
//! for restart recovery, and returns a structured [`PairingOutcome`] together
//! with the signed message bytes the caller should send over the whisper channel.
//!
//! On restart the caller should call [`resolve_pending_pairings`] to attempt
//! connection to peers whose pairing was accepted but never completed, turning
//! pending outcomes into `Connected` or keeping them for a future retry.
//!
//! # Usage
//!
//! ```rust,ignore
//! let (outcome, signed_msg) = accept_peer_invitation(invitation, context, &mut friends, &mut friend_requests)?;
//! match outcome {
//!     PairingOutcome::RequestSent => {
//!         whisper_handle.send_control(peer_pk, signed_msg.unwrap()).await?;
//!     }
//!     PairingOutcome::AlreadyFriends => { /* show "already friends" */ }
//!     // ...
//! }
//!
//! // After restart:
//! let resolved = resolve_pending_pairings(&endpoint, data_dir).await?;
//! ```

use iroh::{EndpointAddr, PublicKey, RelayUrl, SecretKey};
use n0_error::{bail_any, Result, StdResultExt};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chat_core::atomic_write::atomic_write_json;
use crate::contact::{ContactAction, SignedContactMessage};
use crate::conversations::{ConversationEntry, ConversationStore};
use crate::friend_request::{FriendRequest, FriendRequestError, FriendRequestStore};
use crate::friends::{DirectConversationState, FriendId, FriendRelationship, FriendsStore};
use crate::peer_invitation::PeerInvitation;
use crate::proto::TopicId;

// ── PairingContext ────────────────────────────────────────────────────────

/// Context required for the pairing flow.
///
/// Bundles the caller's identity, persistent stores, and network hints so
/// [`accept_peer_invitation`] can validate, persist, and sign a friend request
/// in one call.
#[derive(Debug, Clone)]
pub struct PairingContext {
    /// Our secret key — used for signing the friend request message.
    pub our_secret_key: SecretKey,
    /// Our display name — included in the friend request.
    pub our_display_name: String,
    /// Our relay/gossip bootstrap URLs (shared with the peer).
    pub our_relay_urls: Vec<String>,
    /// Our direct connection addresses (shared with the peer).
    pub our_direct_addresses: Vec<String>,
}

impl PairingContext {
    /// Create a new pairing context from the essential identity and network hints.
    pub fn new(
        our_secret_key: SecretKey,
        our_display_name: impl Into<String>,
        our_relay_urls: Vec<String>,
        our_direct_addresses: Vec<String>,
    ) -> Self {
        Self {
            our_secret_key,
            our_display_name: our_display_name.into(),
            our_relay_urls,
            our_direct_addresses,
        }
    }

    /// Our public key, derived from the secret key.
    pub fn our_public(&self) -> PublicKey {
        self.our_secret_key.public()
    }

    /// Our public key as a stable string.
    pub fn our_public_str(&self) -> String {
        self.our_public().to_string()
    }
}

// ── PairingOutcome ────────────────────────────────────────────────────────

/// Structured result of the pairing flow.
///
/// Each variant tells the UI what happened and what to show the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingOutcome {
    /// A new friend request was created and a signed message is ready to send
    /// over the whisper channel.  The caller MUST send the bytes returned by
    /// [`accept_peer_invitation`] to finalise the pairing.
    RequestSent {
        /// The peer's public key.
        peer: PublicKey,
        /// The created friend request record.
        request: FriendRequest,
    },
    /// The peer is already a friend — no action needed.
    AlreadyFriends {
        /// The peer's public key.
        peer: PublicKey,
    },
    /// We already have a pending outgoing friend request to this peer.
    ExistingOutgoingRequest {
        /// The peer's public key.
        peer: PublicKey,
        /// The existing pending request.
        request: FriendRequest,
    },
    /// The peer already sent us a friend request (incoming pending).
    ExistingIncomingRequest {
        /// The peer's public key.
        peer: PublicKey,
        /// The existing incoming request that we should process.
        request: FriendRequest,
    },
    /// The pairing completed and the direct conversation is available.
    Connected {
        /// The peer's public key.
        peer: PublicKey,
    },
    /// The invitation was accepted, but the connection is not yet established
    /// (e.g., the peer is offline).  The request will be delivered when the
    /// peer becomes reachable.
    PendingConnection {
        /// The peer's public key.
        peer: PublicKey,
        /// The created friend request record.
        request: FriendRequest,
    },
}

impl PairingOutcome {
    /// The peer public key this outcome refers to.
    pub fn peer(&self) -> PublicKey {
        match self {
            Self::RequestSent { peer, .. }
            | Self::AlreadyFriends { peer, .. }
            | Self::ExistingOutgoingRequest { peer, .. }
            | Self::ExistingIncomingRequest { peer, .. }
            | Self::Connected { peer, .. }
            | Self::PendingConnection { peer, .. } => *peer,
        }
    }

    /// Returns `true` if the outcome indicates the user can already (or will
    /// soon be able to) message the peer.
    pub fn can_message(&self) -> bool {
        matches!(self, Self::AlreadyFriends { .. } | Self::Connected { .. })
    }

    /// Returns a human-readable summary string suitable for UI display.
    pub fn summary(&self) -> &'static str {
        match self {
            Self::RequestSent { .. } => {
                "Your invitation was accepted and a friend request was sent."
            }
            Self::AlreadyFriends { .. } => "You are already friends with this peer.",
            Self::ExistingOutgoingRequest { .. } => {
                "You already sent a friend request to this peer."
            }
            Self::ExistingIncomingRequest { .. } => "This peer already sent you a friend request.",
            Self::Connected { .. } => "You are now connected.",
            Self::PendingConnection { .. } => {
                "Request saved and will complete when the peer becomes available."
            }
        }
    }

    /// Returns a short label for the peer, e.g. "Added" or "Already friends".
    pub fn label(&self) -> &'static str {
        match self {
            Self::RequestSent { .. } => "Friend request sent",
            Self::AlreadyFriends { .. } => "Already friends",
            Self::ExistingOutgoingRequest { .. } => "Request pending",
            Self::ExistingIncomingRequest { .. } => "Incoming request",
            Self::Connected { .. } => "Connected",
            Self::PendingConnection { .. } => "Connection pending",
        }
    }

    /// Returns `true` if a "Send first message" button should be shown.
    ///
    /// A request that was only sent is deliberately excluded: the recipient
    /// must accept it before the direct conversation becomes usable.
    pub fn show_send_message(&self) -> bool {
        matches!(self, Self::AlreadyFriends { .. } | Self::Connected { .. })
    }
}

// ── PendingPairing — recovery state across restart ────────────────────────

/// File name for the list of pairings that were accepted but not yet resolved
/// (connection not established).  Persisted across restarts for retry.
const PENDING_PAIRINGS_FILE: &str = "pending_pairings.json";

/// A pairing that was accepted but not yet fully resolved (connection not
/// established).  Persisted across restarts so the application can retry
/// the connection after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPairing {
    /// Our public key (stable string).
    pub our_key: String,
    /// The peer's public key (stable string).
    pub peer_key: String,
    /// The peer's display name from the invitation.
    pub peer_display_name: String,
    /// Unix timestamp (milliseconds) when the pairing was first created.
    pub created_at_unix_ms: u64,
    /// Number of resolution attempts so far.
    #[serde(default)]
    pub retry_count: u32,
}

impl PendingPairing {
    /// Create a new pending pairing with the current timestamp and zero retries.
    pub fn new(our_key: &str, peer_key: &str, peer_display_name: &str) -> Self {
        Self {
            our_key: our_key.to_string(),
            peer_key: peer_key.to_string(),
            peer_display_name: peer_display_name.to_string(),
            created_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            retry_count: 0,
        }
    }
}

fn pending_pairings_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PENDING_PAIRINGS_FILE)
}

/// Load all pending pairings from disk.
///
/// Returns an empty vector if the file does not exist.  Propagates I/O and
/// parse errors so callers can decide how to handle corruption.
pub fn load_pending_pairings(data_dir: &Path) -> Result<Vec<PendingPairing>> {
    let path = pending_pairings_path(data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)
        .with_std_context(|_| format!("failed to read {}", path.display()))?;
    let pairings: Vec<PendingPairing> = serde_json::from_str(&raw)
        .with_std_context(|_| format!("failed to parse {}", path.display()))?;
    Ok(pairings)
}

/// Persist a pending pairing to disk, merging with any existing entries.
///
/// If a pending pairing for the same peer already exists, it is silently
/// replaced so recovery state stays current (e.g. after a re-accept).
pub fn save_pending_pairing(data_dir: &Path, pairing: &PendingPairing) -> Result<()> {
    let path = pending_pairings_path(data_dir);
    let mut existing = load_pending_pairings(data_dir)?;
    // Replace any entry for the same peer rather than duplicating.
    if let Some(pos) = existing.iter().position(|p| p.peer_key == pairing.peer_key) {
        existing[pos] = pairing.clone();
    } else {
        existing.push(pairing.clone());
    }
    atomic_write_json(&path, &existing, "pending pairings")?;
    Ok(())
}

/// Remove a pending pairing by peer key (e.g. after successful resolution).
pub fn remove_pending_pairing(data_dir: &Path, peer_key: &str) -> Result<()> {
    let path = pending_pairings_path(data_dir);
    let mut existing = load_pending_pairings(data_dir)?;
    existing.retain(|p| p.peer_key != peer_key);
    atomic_write_json(&path, &existing, "pending pairings")?;
    Ok(())
}

/// Outcome from resolving a pending pairing.
#[derive(Debug, Clone)]
pub enum ResolvedPairing {
    /// Successfully connected to the peer.
    Connected {
        /// The peer's public key string.
        peer_key: String,
    },
    /// The peer is still unreachable; retry later.
    StillPending {
        /// The peer's public key string.
        peer_key: String,
        /// Updated retry count.
        retry_count: u32,
    },
}

impl ResolvedPairing {
    /// The peer public key string.
    pub fn peer_key(&self) -> &str {
        match self {
            Self::Connected { peer_key } | Self::StillPending { peer_key, .. } => peer_key,
        }
    }
}

/// Try to resolve all pending pairings by connecting to each peer over the
/// whisper ALPN.
///
/// This function:
///
/// 1. Loads all persisted [`PendingPairing`] entries from `data_dir`.
/// 2. For each, tries to connect to the peer using `endpoint.connect`.
/// 3. Removes successfully resolved entries from the pending list.
/// 4. Persists the remaining (still-unreachable) entries, incrementing their
///    retry count, ready for the next restart.
///
/// Call this on application startup (after the iroh endpoint is initialised)
/// to recover pairings that were accepted before a restart but never completed.
///
/// # Errors
///
/// Propagates I/O errors from loading or saving the pending pairings file.
/// Connection failures are **not** errors — they are reflected in the returned
/// [`ResolvedPairing`] variants so the caller can log or notify the UI.
pub async fn resolve_pending_pairings(
    endpoint: &iroh::Endpoint,
    data_dir: &Path,
) -> Result<Vec<ResolvedPairing>> {
    let pairings = load_pending_pairings(data_dir)?;
    if pairings.is_empty() {
        return Ok(Vec::new());
    }

    let mut results: Vec<ResolvedPairing> = Vec::with_capacity(pairings.len());
    let mut remaining: Vec<PendingPairing> = Vec::new();

    for mut pairing in pairings {
        let peer_pk = match pairing.peer_key.parse::<PublicKey>() {
            Ok(pk) => pk,
            Err(e) => {
                tracing::warn!(
                    "invalid peer key in pending pairing '{}': {e}",
                    pairing.peer_key
                );
                // Remove invalid entries by not adding them to `remaining`.
                continue;
            }
        };

        // Build an EndpointAddr from the peer key — endpoint.connect resolves
        // the actual transport addresses via the address-lookup backend.
        let addr = EndpointAddr::new(peer_pk);

        match endpoint.connect(addr, crate::whisper::WHISPER_ALPN).await {
            Ok(conn) => {
                // Connection established — close it (the whisper layer will
                // open its own managed connection when needed) and record
                // success.
                conn.close(0u32.into(), b"pairing resolution probe");
                tracing::info!("resolved pending pairing for peer {}", pairing.peer_key);
                results.push(ResolvedPairing::Connected {
                    peer_key: pairing.peer_key.clone(),
                });
                // Do NOT add to `remaining`.
            }
            Err(e) => {
                pairing.retry_count += 1;
                tracing::debug!(
                    "pending pairing for peer {} not yet reachable (attempt {}): {e}",
                    pairing.peer_key,
                    pairing.retry_count,
                );
                results.push(ResolvedPairing::StillPending {
                    peer_key: pairing.peer_key.clone(),
                    retry_count: pairing.retry_count,
                });
                remaining.push(pairing);
            }
        }
    }

    // Persist the (updated) remaining pairings for the next retry.
    atomic_write_json(
        &pending_pairings_path(data_dir),
        &remaining,
        "pending pairings",
    )?;

    Ok(results)
}

// ── Main entry point ──────────────────────────────────────────────────────

/// Result of `accept_peer_invitation`: the outcome and optionally the signed
/// message bytes to send over the whisper channel.
pub type PairingResult = (PairingOutcome, Option<Vec<u8>>);

/// Accept a peer invitation and initiate the pairing flow.
///
/// This function:
///
/// 1. Re-validates the invitation (against our public key).
/// 2. Checks whether the peer is already known (friend, blocked, pending).
/// 3. Stores the peer's address hints in the friends store.
/// 4. Creates (or reuses) a pending friend request record.
/// 5. Signs a [`ContactAction::FriendRequest`] for the caller to send.
/// 6. Persists a [`PendingPairing`] for restart recovery.
///
/// The caller MUST send the returned signed bytes over the whisper channel
/// (via `whisper_handle.send_control(peer_pk, bytes)`) when the outcome is
/// `RequestSent` or `PendingConnection` — otherwise the friend request will
/// never reach the peer.
///
/// # Idempotency
///
/// Calling this function multiple times with the same invitation is safe:
/// - If the peer is already a friend, `AlreadyFriends` is returned.
/// - If a pending outgoing request already exists, `ExistingOutgoingRequest`
///   is returned.
/// - If a pending incoming request exists, `ExistingIncomingRequest` is returned.
/// - If the friend request was already created (stale-pending from a prior call
///   that failed after step 4), the `DuplicatePending` is caught and mapped to
///   `ExistingOutgoingRequest`.
///
/// # Errors
///
/// Returns an error if:
/// - The invitation fails validation (expired, unsupported version, etc.)
/// - The invitation's peer_id matches our own public key (self-invitation)
/// - The peer is blocked
/// - A store operation fails (I/O error)
/// - Signing the friend request message fails
pub fn accept_peer_invitation(
    invitation: &PeerInvitation,
    context: &PairingContext,
    friends_store: &mut FriendsStore,
    friend_request_store: &mut FriendRequestStore,
) -> Result<PairingResult> {
    // ── 1. Validate the invitation ──────────────────────────────────────
    invitation
        .validate(Some(&context.our_public()))
        .map_err(|e| n0_error::anyerr!("invitation validation failed: {e}"))?;

    let peer_pk = invitation.peer_id;
    let peer_pk_str = peer_pk.to_string();
    let our_pk_str = context.our_public_str();
    let friend_id = FriendId::from_public_key(peer_pk);

    // ── 2. Check if peer is blocked ─────────────────────────────────────
    if let Some(record) = friends_store.get(&friend_id) {
        if record.relationship == FriendRelationship::Blocked {
            bail_any!("cannot pair with a blocked peer");
        }
    }

    // ── 3. Check existing relationship ──────────────────────────────────
    // Check if we're already friends
    if let Some(record) = friends_store.get(&friend_id) {
        if record.relationship == FriendRelationship::Friends {
            return Ok((PairingOutcome::AlreadyFriends { peer: peer_pk }, None));
        }
    }

    // ── 4. Check for existing outgoing friend requests ──────────────────
    let outgoing = friend_request_store.list_outgoing_by_status(
        &our_pk_str,
        crate::friend_request::FriendRequestStatus::Pending,
    );
    for req in outgoing {
        if req.recipient == peer_pk_str {
            return Ok((
                PairingOutcome::ExistingOutgoingRequest {
                    peer: peer_pk,
                    request: req.clone(),
                },
                None,
            ));
        }
    }

    // ── 5. Check for existing incoming friend requests ──────────────────
    let incoming = friend_request_store.list_incoming_by_status(
        &our_pk_str,
        crate::friend_request::FriendRequestStatus::Pending,
    );
    for req in incoming {
        if req.requester == peer_pk_str {
            return Ok((
                PairingOutcome::ExistingIncomingRequest {
                    peer: peer_pk,
                    request: req.clone(),
                },
                None,
            ));
        }
    }

    // ── 6. Update friends store with peer's info ────────────────────────
    let record = friends_store.ensure_friend(friend_id.clone());

    // Store the display name from the invitation
    record.last_announced_name = Some(invitation.display_name.clone());

    // Preserve both directly usable socket addresses and valid relay hints in
    // the existing address store. Invalid individual hints are ignored after
    // the invitation's structural validation; they must never become a
    // fabricated endpoint or cause pairing to fail unnecessarily.
    let relay_addrs = invitation
        .relay_urls
        .iter()
        .filter_map(|raw| raw.parse::<RelayUrl>().ok())
        .map(|relay| EndpointAddr::new(peer_pk).with_relay_url(relay));
    let direct_addrs = invitation
        .direct_addresses
        .iter()
        .filter_map(|raw| raw.parse::<SocketAddr>().ok())
        .map(|addr| EndpointAddr::new(peer_pk).with_ip_addr(addr))
        .chain(relay_addrs)
        .collect::<Vec<_>>();
    record.record_addrs(direct_addrs);

    // ── 7. Create the direct conversation topic ─────────────────────────
    let topic = crate::contact::direct_topic(&context.our_public(), &peer_pk);
    record.set_direct_conversation(topic, DirectConversationState::Pending);

    // ── 8. Create and persist the friend request ────────────────────────
    let request = match friend_request_store.send_request(
        &our_pk_str,
        &peer_pk_str,
        Some(format!("Invitation from {}", invitation.display_name)),
    ) {
        Ok(req) => req,
        // Idempotency: if a concurrent/sibling call created the request
        // between our step-4 check and now, return the existing request.
        Err(FriendRequestError::DuplicatePending { existing_id }) => {
            let existing = friend_request_store
                .get(&existing_id)
                .ok_or_else(|| {
                    n0_error::anyerr!("duplicate pending request {existing_id} not found in store")
                })?
                .clone();
            return Ok((
                PairingOutcome::ExistingOutgoingRequest {
                    peer: peer_pk,
                    request: existing,
                },
                None,
            ));
        }
        Err(e) => {
            return Err(n0_error::anyerr!("failed to create friend request: {e}"));
        }
    };

    // ── 9. Sign the friend request message ──────────────────────────────
    let action = ContactAction::FriendRequest {
        name: Some(context.our_display_name.clone()),
    };

    let signed_payload = SignedContactMessage::sign(&context.our_secret_key, &action)
        .map_err(|e| n0_error::anyerr!("failed to sign friend request: {e}"))?;

    // ── 10. Persist recovery state (best-effort) ────────────────────────
    let data_dir = friend_request_store.data_dir().to_path_buf();
    if !data_dir.as_os_str().is_empty() {
        let pending = PendingPairing::new(&our_pk_str, &peer_pk_str, &invitation.display_name);
        if let Err(e) = save_pending_pairing(&data_dir, &pending) {
            // The request is already created — log the persistence failure
            // but do not fail the overall operation.
            tracing::warn!("failed to persist pending pairing for restart recovery: {e}");
        }
    }
    // Also store the signed message bytes in the friend request store so
    // callers can retry delivery without re-signing.
    friend_request_store.store_pending_signed_message(&request.id, signed_payload.clone());

    // ── 11. Determine the outcome ──────────────────────────────────────
    // After creating the request, we return RequestSent.  The caller will
    // send the signed message via whisper.  For now, we optimistically
    // return RequestSent; the caller can retry sending on failure or call
    // resolve_pending_pairings on restart to attempt connection.
    let outcome = PairingOutcome::RequestSent {
        peer: peer_pk,
        request,
    };

    Ok((outcome, Some(signed_payload)))
}

// ── First-conversation action ─────────────────────────────────────────────

/// Result of opening or retrieving the first direct conversation with a
/// paired peer.
///
/// Returned by [`open_or_create_first_conversation`].
#[derive(Debug, Clone)]
pub enum FirstConversationResult {
    /// A direct conversation is available and the UI should navigate to it.
    Ready {
        /// The gossip topic for the direct conversation.
        topic: TopicId,
    },
    /// Messaging is not yet available.  The UI should show the pending state.
    NotReady {
        /// The peer's public key.
        peer: PublicKey,
        /// Why the conversation is not available.
        reason: NotReadyReason,
    },
}

/// Why a first conversation cannot be opened yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotReadyReason {
    /// A friend request is still pending (outgoing or incoming).
    PendingFriendRequest,
    /// The peer's connection is not yet established.
    PendingConnection,
    /// The peer is not a friend and has no pending request.
    NotFriends,
}

impl NotReadyReason {
    /// Human-readable summary for UI display.
    pub fn summary(self) -> &'static str {
        match self {
            Self::PendingFriendRequest => {
                "Friend request is still pending — wait for the peer to accept."
            }
            Self::PendingConnection => {
                "Connection is not yet established — the peer may be offline."
            }
            Self::NotFriends => "You must be friends before you can send a message.",
        }
    }
}

/// Open or retrieve the direct conversation for a paired peer.
///
/// This is the library-level implementation of the "Send first message"
/// flow from the pairing result screen.  It:
///
/// 1. Checks whether messaging is permitted (peer must be a friend).
/// 2. Derives the deterministic direct topic from the two public keys via
///    [`crate::contact::direct_topic`].
/// 3. Checks whether a conversation entry already exists (idempotency).
/// 4. Creates or updates the direct conversation record in the friends store
///    (setting it to [`DirectConversationState::Active`]).
/// 5. Upserts a [`ConversationEntry`] in the conversation store.
/// 6. Persists both stores to disk.
///
/// # Idempotency
///
/// Calling this function multiple times with the same peer is safe:
/// - If the conversation already exists, its topic is returned without
///   any modification.
/// - If the peer is not yet a friend, [`FirstConversationResult::NotReady`]
///   is returned — no conversation record is created.
/// - The direct conversation state in the friend record is always set to
///   [`DirectConversationState::Active`] when a conversation is created.
///
/// # Errors
///
/// Returns an error if the peer is blocked.
/// Store save failures are logged but **not** propagated — the in-memory
/// state is still correct and will be persisted on the next save cycle.
pub fn open_or_create_first_conversation(
    our_public: &PublicKey,
    peer: PublicKey,
    friends_store: &mut FriendsStore,
    conversation_store: &mut ConversationStore,
) -> Result<FirstConversationResult> {
    let fid = FriendId::from_public_key(peer);
    let topic = crate::contact::direct_topic(our_public, &peer);

    // ── 1. Check peer is not blocked ───────────────────────────────────
    if let Some(record) = friends_store.get(&fid) {
        if record.relationship == FriendRelationship::Blocked {
            bail_any!("cannot open a conversation with a blocked peer");
        }
    }

    // ── 2. Check if the peer is a friend ────────────────────────────────
    let is_friend = friends_store
        .get(&fid)
        .is_some_and(|r| r.relationship.can_message());

    if !is_friend {
        // Determine the reason so the UI can show the right state.
        let reason = if friends_store.get(&fid).is_some() {
            NotReadyReason::PendingFriendRequest
        } else {
            NotReadyReason::NotFriends
        };
        return Ok(FirstConversationResult::NotReady { peer, reason });
    }

    // ── 3. Check for existing conversation (idempotency) ────────────────
    if conversation_store.find(&topic).is_some() {
        // Already exists — just return the topic.
        return Ok(FirstConversationResult::Ready { topic });
    }

    // ── 4. Ensure direct conversation in friends store ──────────────────
    let record = friends_store.ensure_friend(fid);
    record.set_direct_conversation(topic, DirectConversationState::Active);

    // ── 5. Create conversation entry ───────────────────────────────────
    let name = peer.fmt_short().to_string();
    conversation_store.upsert(ConversationEntry::new(topic, peer.to_string(), name));

    // ── 6. Persist both stores (best-effort) ───────────────────────────
    if let Err(e) = conversation_store.save() {
        tracing::warn!("failed to save conversation store: {e}");
    }
    if let Err(e) = friends_store.save() {
        tracing::warn!("failed to save friends store: {e}");
    }

    Ok(FirstConversationResult::Ready { topic })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        dir.push(format!("boru-pairing-{name}-{suffix}"));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn make_invitation(display_name: &str, secret_key: &SecretKey) -> PeerInvitation {
        PeerInvitation {
            version: 1,
            peer_id: secret_key.public(),
            display_name: display_name.to_string(),
            avatar_hash: None,
            relay_urls: vec![],
            direct_addresses: vec![],
            friend_request_token: None,
            expires_at: Some(i64::MAX), // far future
        }
    }

    fn make_context(secret_key: &SecretKey, display_name: &str) -> PairingContext {
        PairingContext::new(secret_key.clone(), display_name, vec![], vec![])
    }

    // ── Existing tests (preserved) ─────────────────────────────────────────

    #[test]
    fn test_successful_pairing() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("success");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let (outcome, signed_msg) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("pairing should succeed");

        assert!(matches!(outcome, PairingOutcome::RequestSent { .. }));
        assert!(signed_msg.is_some(), "must return signed message bytes");
        assert_eq!(outcome.peer(), their_sk.public());

        // Verify the friend record was created
        let fid = FriendId::from_public_key(their_sk.public());
        let record = friends.get(&fid).expect("friend record should exist");
        assert_eq!(
            record.last_announced_name.as_deref(),
            Some("Alice"),
            "should store display name from invitation"
        );
        assert!(
            record.direct_conversation.is_some(),
            "direct conversation should be set"
        );

        // Verify the friend request was created
        let our_pk_str = our_sk.public().to_string();
        let outgoing = friend_requests.list_outgoing_by_status(
            &our_pk_str,
            crate::friend_request::FriendRequestStatus::Pending,
        );
        assert_eq!(outgoing.len(), 1, "should have one outgoing request");
        assert_eq!(outgoing[0].recipient, their_sk.public().to_string());

        // Verify the signed message is valid
        let (from, action) = SignedContactMessage::verify(&signed_msg.unwrap(), None)
            .expect("signed message should be valid");
        assert_eq!(from, our_sk.public());
        assert!(
            matches!(action, ContactAction::FriendRequest { .. }),
            "should be a FriendRequest action"
        );

        // Verify a pending pairing was persisted for restart recovery
        let pairings = load_pending_pairings(&dir).expect("load pending pairings");
        assert_eq!(pairings.len(), 1, "should persist one pending pairing");
        assert_eq!(
            pairings[0].peer_key,
            their_sk.public().to_string(),
            "pending pairing should reference the invited peer"
        );
        assert_eq!(pairings[0].retry_count, 0);
    }

    #[test]
    fn test_already_friends() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("already-friends");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        // Set up: they're already friends
        let fid = FriendId::from_public_key(their_sk.public());
        friends.set_relationship(fid, FriendRelationship::Friends);
        friends.save().ok();

        let (outcome, signed_msg) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("pairing should succeed");

        assert!(matches!(outcome, PairingOutcome::AlreadyFriends { .. }));
        assert!(
            signed_msg.is_none(),
            "no message needed when already friends"
        );
    }

    #[test]
    fn test_existing_outgoing_request() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("existing-outgoing");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        // Pre-create an outgoing request
        let our_pk_str = our_sk.public().to_string();
        let their_pk_str = their_sk.public().to_string();
        friend_requests
            .send_request(&our_pk_str, &their_pk_str, None)
            .expect("pre-create outgoing request");

        let (outcome, signed_msg) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("pairing should succeed");

        assert!(
            matches!(outcome, PairingOutcome::ExistingOutgoingRequest { .. }),
            "should detect existing outgoing request"
        );
        assert!(signed_msg.is_none());
    }

    #[test]
    fn test_existing_incoming_request() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("existing-incoming");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        // Pre-create an incoming request
        let our_pk_str = our_sk.public().to_string();
        let their_pk_str = their_sk.public().to_string();
        friend_requests
            .send_request(&their_pk_str, &our_pk_str, None)
            .expect("pre-create incoming request");

        let (outcome, signed_msg) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("pairing should succeed");

        assert!(
            matches!(outcome, PairingOutcome::ExistingIncomingRequest { .. }),
            "should detect existing incoming request"
        );
        assert!(signed_msg.is_none());
    }

    #[test]
    fn test_rejects_self_invitation() {
        let our_sk = SecretKey::generate();
        let invitation = make_invitation("Self", &our_sk);
        let context = make_context(&our_sk, "Self");
        let dir = temp_dir("self-invite");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err("self-invitation should be rejected");
        assert!(
            err.to_string().contains("cannot pair with yourself")
                || err.to_string().contains("self"),
            "error should mention self-invitation: {err}"
        );
    }

    #[test]
    fn test_rejects_expired_invitation() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let mut invitation = make_invitation("Expired", &their_sk);
        invitation.expires_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64
                - 3600,
        ); // 1 hour ago
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("expired");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err("expired invitation should be rejected");
        assert!(err.to_string().contains("expired"), "error: {err}");
    }

    #[test]
    fn test_rejects_blocked_peer() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Blocked", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("blocked");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        // Set up: peer is blocked
        let fid = FriendId::from_public_key(their_sk.public());
        friends.set_relationship(fid, FriendRelationship::Blocked);
        friends.save().ok();

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err("blocked peer should be rejected");
        assert!(err.to_string().contains("blocked"), "error: {err}");
    }

    #[test]
    fn test_pairing_outcome_summaries() {
        use crate::friend_request::FriendRequestStatus;
        let peer = SecretKey::generate().public();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let dummy_request = FriendRequest {
            id: "test-id".to_string(),
            requester: "requester".to_string(),
            recipient: "recipient".to_string(),
            status: FriendRequestStatus::Pending,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            message: None,
        };

        let outcomes = vec![
            PairingOutcome::RequestSent {
                peer,
                request: dummy_request.clone(),
            },
            PairingOutcome::AlreadyFriends { peer },
            PairingOutcome::ExistingOutgoingRequest {
                peer,
                request: dummy_request.clone(),
            },
            PairingOutcome::ExistingIncomingRequest {
                peer,
                request: dummy_request.clone(),
            },
            PairingOutcome::Connected { peer },
            PairingOutcome::PendingConnection {
                peer,
                request: dummy_request.clone(),
            },
        ];

        for outcome in outcomes {
            let summary = outcome.summary();
            assert!(
                !summary.is_empty(),
                "summary should not be empty for {outcome:?}"
            );
            let label = outcome.label();
            assert!(
                !label.is_empty(),
                "label should not be empty for {outcome:?}"
            );
        }

        let request_sent = PairingOutcome::RequestSent {
            peer,
            request: dummy_request.clone(),
        };
        assert!(
            !request_sent.show_send_message(),
            "a pending request must not open a conversation before acceptance"
        );
        assert!(PairingOutcome::AlreadyFriends { peer }.show_send_message());
    }

    #[test]
    fn test_rejects_unknown_version() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let mut invitation = make_invitation("BadVer", &their_sk);
        invitation.version = 99;
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("bad-version");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err("unknown version should be rejected");
        assert!(err.to_string().contains("version"), "error: {err}");
    }

    #[test]
    fn test_stores_relay_and_direct_addresses() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();

        let invitation = PeerInvitation {
            version: 1,
            peer_id: their_sk.public(),
            display_name: "Alice".to_string(),
            avatar_hash: None,
            relay_urls: vec!["https://relay.example.com".to_string()],
            direct_addresses: vec!["192.168.1.42:9876".to_string()],
            friend_request_token: None,
            expires_at: Some(i64::MAX),
        };

        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("addresses");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect("pairing should succeed");

        let fid = FriendId::from_public_key(their_sk.public());
        let record = friends.get(&fid).expect("friend record should exist");
        assert!(
            record.known_addrs.len() >= 2,
            "should have stored both direct and relay address hints"
        );
    }

    // ── New tests: idempotency ────────────────────────────────────────────

    #[test]
    fn test_duplicate_accept_returns_existing_outgoing() {
        // Calling accept_peer_invitation twice with the same invitation
        // must not create a duplicate request or duplicate friend record.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("duplicate-accept");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        // First call — should succeed
        let (outcome1, msg1) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("first accept should succeed");
        assert!(matches!(outcome1, PairingOutcome::RequestSent { .. }));
        assert!(msg1.is_some(), "first call must return signed message");

        // Second call — should detect the existing outgoing request
        let (outcome2, msg2) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("second accept should succeed (idempotent)");
        assert!(
            matches!(outcome2, PairingOutcome::ExistingOutgoingRequest { .. }),
            "second call should detect existing outgoing request, got {outcome2:?}"
        );
        assert!(msg2.is_none(), "second call must not return signed message");

        // Still only one friend request in the store
        let our_pk_str = our_sk.public().to_string();
        let count = friend_requests
            .list_outgoing_by_status(
                &our_pk_str,
                crate::friend_request::FriendRequestStatus::Pending,
            )
            .len();
        assert_eq!(count, 1, "must not duplicate friend requests");

        // Still only one friend record
        assert_eq!(friends.len(), 1, "must not duplicate friend records");
    }

    #[test]
    fn test_idempotent_after_friends_store_partial_update() {
        // Simulate a scenario where the first call created the friend request
        // but failed to sign (so no pending pairing was persisted). The second
        // call should detect the existing request and return ExistingOutgoingRequest.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("partial-update");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        // Manually create the friend request to simulate a partial success
        let our_pk_str = our_sk.public().to_string();
        let their_pk_str = their_sk.public().to_string();
        friend_requests
            .send_request(
                &our_pk_str,
                &their_pk_str,
                Some("Invitation from Alice".into()),
            )
            .expect("pre-create friend request");

        // Now the pairing call should detect the existing request
        let (outcome, signed_msg) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("pairing should succeed idempotently");
        assert!(
            matches!(outcome, PairingOutcome::ExistingOutgoingRequest { .. }),
            "should detect pre-existing outgoing request"
        );
        assert!(signed_msg.is_none(), "no new message needed");
    }

    // ── New tests: incompatible invitations ────────────────────────────────

    #[test]
    fn test_rejects_empty_display_name() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let mut invitation = make_invitation("", &their_sk);
        invitation.display_name = String::new();
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("empty-name");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err("empty display name should be rejected");
        assert!(
            err.to_string().contains("display name must not be empty")
                || err.to_string().contains("empty"),
            "error should mention empty display name: {err}"
        );
    }

    #[test]
    fn test_rejects_display_name_too_long() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let mut invitation = make_invitation("x", &their_sk);
        invitation.display_name = "x".repeat(65); // > 64 chars
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("long-name");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err("display name >64 chars should be rejected");
        assert!(
            err.to_string().contains("too long"),
            "error should mention name too long: {err}"
        );
    }

    #[test]
    fn test_rejects_too_many_relay_urls() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let mut invitation = make_invitation("Alice", &their_sk);
        invitation.relay_urls = (0..11)
            .map(|i| format!("https://relay{i}.example.com"))
            .collect();
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("too-many-relays");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err(">10 relay URLs should be rejected");
        assert!(
            err.to_string().contains("too many relay"),
            "error should mention too many relays: {err}"
        );
    }

    #[test]
    fn test_rejects_too_many_direct_addresses() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let mut invitation = make_invitation("Alice", &their_sk);
        invitation.direct_addresses = (0..11).map(|i| format!("192.168.1.{i}:9876")).collect();
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("too-many-addrs");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let err = accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect_err(">10 direct addresses should be rejected");
        assert!(
            err.to_string().contains("too many direct"),
            "error should mention too many direct addresses: {err}"
        );
    }

    // ── New tests: restart persistence ──────────────────────────────────────

    #[test]
    fn test_restart_persistence() {
        // Simulate a full restart: create a pairing, save stores to disk,
        // reload fresh stores, and verify all state is intact.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("restart-persistence");

        // ── First session ────────────────────────────────────────────────
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect("pairing should succeed");

        // Save both stores to disk
        friends.save().expect("save friends");
        friend_requests.save().expect("save friend requests");

        // ── After restart: reload from disk ─────────────────────────────
        let reloaded_friends = FriendsStore::load(&dir).expect("reload friends");
        let reloaded_requests = FriendRequestStore::load(&dir).expect("reload friend requests");

        // Verify friend record survived
        let fid = FriendId::from_public_key(their_sk.public());
        let record = reloaded_friends
            .get(&fid)
            .expect("friend record must survive restart");
        assert_eq!(
            record.last_announced_name.as_deref(),
            Some("Alice"),
            "display name must survive restart"
        );
        assert!(
            record.direct_conversation.is_some(),
            "direct conversation must survive restart"
        );

        // Verify friend request survived
        let our_pk_str = our_sk.public().to_string();
        let outgoing = reloaded_requests.list_outgoing_by_status(
            &our_pk_str,
            crate::friend_request::FriendRequestStatus::Pending,
        );
        assert_eq!(outgoing.len(), 1, "friend request must survive restart");
        assert_eq!(
            outgoing[0].recipient,
            their_sk.public().to_string(),
            "recipient must be correct after restart"
        );

        // Verify pending pairing file survived
        let pairings = load_pending_pairings(&dir).expect("load pending pairings after restart");
        assert_eq!(pairings.len(), 1, "pending pairing must survive restart");
        assert_eq!(pairings[0].peer_key, their_sk.public().to_string());
    }

    #[test]
    fn test_pending_signed_message_persistence() {
        // Verify the friend_request_store's pending_signed_messages map
        // persists the signed message bytes across save/load cycles.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("pending-signed-persist");

        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        let (outcome, signed_msg) =
            accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
                .expect("pairing should succeed");

        let request_id = match &outcome {
            PairingOutcome::RequestSent { request, .. } => &request.id,
            _ => panic!("expected RequestSent"),
        };

        // Verify the signed message was stored before save
        assert!(
            friend_requests.has_pending_signed_message(request_id),
            "signed message should be stored in friend request store"
        );
        let stored = friend_requests
            .take_pending_signed_message(request_id)
            .expect("should retrieve stored signed message");
        assert_eq!(
            stored,
            signed_msg.expect("signed message should exist"),
            "stored bytes should match returned bytes"
        );

        // Store again for persistence test
        friend_requests.store_pending_signed_message(request_id.clone(), stored.clone());

        // Save and reload
        friend_requests.save().expect("save friend requests");
        let mut reloaded = FriendRequestStore::load(&dir).expect("reload");

        assert!(
            reloaded.has_pending_signed_message(request_id),
            "pending signed message must survive save/load"
        );
        let retrieved = reloaded
            .take_pending_signed_message(request_id)
            .expect("should retrieve after reload");
        assert_eq!(
            retrieved, stored,
            "signed message bytes must survive persistence"
        );
    }

    #[test]
    fn test_pending_pairing_save_and_load() {
        let dir = temp_dir("pending-saveload");

        let pairing = PendingPairing::new("our_key_abc", "peer_key_xyz", "TestPeer");
        save_pending_pairing(&dir, &pairing).expect("save should succeed");

        let loaded = load_pending_pairings(&dir).expect("load should succeed");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].our_key, "our_key_abc");
        assert_eq!(loaded[0].peer_key, "peer_key_xyz");
        assert_eq!(loaded[0].peer_display_name, "TestPeer");
        assert_eq!(loaded[0].retry_count, 0);
        assert!(loaded[0].created_at_unix_ms > 0);
    }

    #[test]
    fn test_pending_pairing_replace_on_same_peer() {
        // Saving a pending pairing for the same peer should replace the old entry.
        let dir = temp_dir("pending-replace");

        let first = PendingPairing::new("our_key", "peer_key", "Alice");
        let mut second = PendingPairing::new("our_key", "peer_key", "Bob");
        second.retry_count = 2;

        save_pending_pairing(&dir, &first).expect("save first");
        save_pending_pairing(&dir, &second).expect("save second (replace)");

        let loaded = load_pending_pairings(&dir).expect("load");
        assert_eq!(loaded.len(), 1, "should still be exactly one entry");
        assert_eq!(loaded[0].peer_display_name, "Bob");
        assert_eq!(loaded[0].retry_count, 2);
    }

    #[test]
    fn test_pending_pairing_multiple_peers() {
        let dir = temp_dir("pending-multi");

        save_pending_pairing(&dir, &PendingPairing::new("our", "peer_a", "Alice")).expect("save A");
        save_pending_pairing(&dir, &PendingPairing::new("our", "peer_b", "Bob")).expect("save B");

        let loaded = load_pending_pairings(&dir).expect("load");
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn test_remove_pending_pairing() {
        let dir = temp_dir("pending-remove");

        save_pending_pairing(&dir, &PendingPairing::new("our", "peer_a", "Alice")).expect("save A");
        save_pending_pairing(&dir, &PendingPairing::new("our", "peer_b", "Bob")).expect("save B");

        remove_pending_pairing(&dir, "peer_a").expect("remove A");

        let loaded = load_pending_pairings(&dir).expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].peer_key, "peer_b");
    }

    #[test]
    fn test_load_pending_pairings_missing_file() {
        let dir = temp_dir("pending-missing");
        let loaded = load_pending_pairings(&dir).expect("load missing should return empty vec");
        assert!(loaded.is_empty());
    }

    #[test]
    fn test_remove_pending_pairing_nonexistent() {
        // Removing a peer that doesn't exist should be a no-op (not an error).
        let dir = temp_dir("pending-remove-nonexistent");
        save_pending_pairing(&dir, &PendingPairing::new("our", "the_only_peer", "Solo"))
            .expect("save");

        remove_pending_pairing(&dir, "nonexistent_peer")
            .expect("remove nonexistent should succeed");

        let loaded = load_pending_pairings(&dir).expect("load");
        assert_eq!(loaded.len(), 1, "the only peer should still be there");
    }

    // ── New tests: pairing creates pending pairing on disk ─────────────────

    #[test]
    fn test_accept_creates_pending_pairing_file() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("creates-pending");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect("pairing should succeed");

        // Verify the pending pairings file exists with the right content
        let pairings = load_pending_pairings(&dir).expect("load pending pairings");
        assert_eq!(pairings.len(), 1, "should have one pending pairing");
        assert_eq!(pairings[0].peer_key, their_sk.public().to_string());
        assert_eq!(pairings[0].peer_display_name, "Alice");
    }

    #[test]
    fn test_duplicate_accept_does_not_duplicate_pending_pairing() {
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let invitation = make_invitation("Alice", &their_sk);
        let context = make_context(&our_sk, "Bob");
        let dir = temp_dir("no-dup-pending");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut friend_requests = FriendRequestStore::empty_at(&dir);

        // First accept — creates pending pairing
        accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect("first accept");

        // Second accept — returns ExistingOutgoingRequest, should not add a
        // duplicate pending pairing (the function returns before the persist step)
        accept_peer_invitation(&invitation, &context, &mut friends, &mut friend_requests)
            .expect("second accept");

        let pairings = load_pending_pairings(&dir).expect("load pending pairings");
        assert_eq!(
            pairings.len(),
            1,
            "must not duplicate pending pairing entries"
        );
    }

    // ── New tests: PairingOutcome completeness ─────────────────────────────

    #[test]
    fn test_pairing_outcome_connected_usage() {
        // Connected should allow messaging and show appropriate labels
        let peer = SecretKey::generate().public();
        let outcome = PairingOutcome::Connected { peer };

        assert!(outcome.can_message(), "Connected should allow messaging");
        assert!(
            outcome.show_send_message(),
            "Connected should show send button"
        );
        assert_eq!(outcome.summary(), "You are now connected.");
        assert_eq!(outcome.label(), "Connected");
        assert_eq!(outcome.peer(), peer);
    }

    #[test]
    fn test_pairing_outcome_pending_connection_usage() {
        // PendingConnection should have a request but not allow messaging
        let peer = SecretKey::generate().public();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let request = FriendRequest {
            id: "pc-test-id".to_string(),
            requester: "requester".to_string(),
            recipient: peer.to_string(),
            status: crate::friend_request::FriendRequestStatus::Pending,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            message: Some("Waiting for peer".into()),
        };

        let outcome = PairingOutcome::PendingConnection {
            peer,
            request: request.clone(),
        };

        assert!(
            !outcome.can_message(),
            "PendingConnection should not allow messaging"
        );
        assert!(
            !outcome.show_send_message(),
            "PendingConnection should not show send button"
        );
        assert_eq!(
            outcome.summary(),
            "Request saved and will complete when the peer becomes available."
        );
        assert_eq!(outcome.label(), "Connection pending");
        assert_eq!(outcome.peer(), peer);
    }

    // ── Tests: first-conversation action ────────────────────────────────────

    #[test]
    fn test_first_conversation_success() {
        // Given: a peer who is a friend
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let dir = temp_dir("first-conv-success");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut conversations = ConversationStore::empty_at(&dir);

        let fid = FriendId::from_public_key(their_sk.public());
        friends.set_relationship(fid.clone(), FriendRelationship::Friends);
        friends.save().ok();

        // When: opening the first conversation
        let result = open_or_create_first_conversation(
            &our_sk.public(),
            their_sk.public(),
            &mut friends,
            &mut conversations,
        )
        .expect("should succeed");

        // Then: returns Ready with a valid topic
        let topic = match result {
            FirstConversationResult::Ready { topic } => topic,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_ne!(
            topic,
            TopicId::from_bytes([0u8; 32]),
            "topic should not be zeroed"
        );

        // Verify the conversation record exists in both stores
        assert!(
            conversations.find(&topic).is_some(),
            "conversation should exist in store"
        );

        let record = friends.get(&fid).expect("friend record should exist");
        assert!(
            record.direct_conversation.is_some(),
            "direct conversation should be set in friend record"
        );
        let dc = record.direct_conversation.as_ref().unwrap();
        assert_eq!(
            dc.state,
            DirectConversationState::Active,
            "conversation should be active"
        );
        assert_eq!(dc.topic, topic, "topic should match");
    }

    #[test]
    fn test_first_conversation_idempotent() {
        // Calling twice should return the same topic without duplicating.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let dir = temp_dir("first-conv-idempotent");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut conversations = ConversationStore::empty_at(&dir);

        let fid = FriendId::from_public_key(their_sk.public());
        friends.set_relationship(fid.clone(), FriendRelationship::Friends);
        friends.save().ok();

        // First call
        let result1 = open_or_create_first_conversation(
            &our_sk.public(),
            their_sk.public(),
            &mut friends,
            &mut conversations,
        )
        .expect("first call should succeed");

        let topic1 = match result1 {
            FirstConversationResult::Ready { topic } => topic,
            other => panic!("expected Ready, got {other:?}"),
        };

        assert_eq!(
            conversations.len(),
            1,
            "should have exactly one conversation"
        );

        // Second call
        let result2 = open_or_create_first_conversation(
            &our_sk.public(),
            their_sk.public(),
            &mut friends,
            &mut conversations,
        )
        .expect("second call should succeed");

        let topic2 = match result2 {
            FirstConversationResult::Ready { topic } => topic,
            other => panic!("expected Ready, got {other:?}"),
        };

        assert_eq!(topic1, topic2, "both calls should return the same topic");
        assert_eq!(
            conversations.len(),
            1,
            "idempotent: should still have exactly one conversation"
        );

        // Verify the conversation record was not modified
        let record = friends.get(&fid).expect("friend record should exist");
        let dc = record.direct_conversation.as_ref().unwrap();
        assert_eq!(dc.topic, topic1, "topic should match first call");
    }

    #[test]
    fn test_first_conversation_not_friends() {
        // Peer with no relationship should return NotReady::NotFriends.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let dir = temp_dir("first-conv-not-friends");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut conversations = ConversationStore::empty_at(&dir);

        let result = open_or_create_first_conversation(
            &our_sk.public(),
            their_sk.public(),
            &mut friends,
            &mut conversations,
        )
        .expect("should succeed (not an error to be not ready)");

        match result {
            FirstConversationResult::NotReady { peer, reason } => {
                assert_eq!(peer, their_sk.public());
                assert_eq!(reason, NotReadyReason::NotFriends, "should be NotFriends");
            }
            other => panic!("expected NotReady, got {other:?}"),
        }

        // No conversation should have been created
        assert!(
            conversations.is_empty(),
            "no conversation should be created"
        );
    }

    #[test]
    fn test_first_conversation_pending_request() {
        // Peer with a friend record but not Friends should return NotReady.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let dir = temp_dir("first-conv-pending");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut conversations = ConversationStore::empty_at(&dir);

        // Create a friend record with relationship set to NotFriend
        // (but the record exists because we ensure_friend)
        let fid = FriendId::from_public_key(their_sk.public());
        friends.ensure_friend(fid.clone());

        let result = open_or_create_first_conversation(
            &our_sk.public(),
            their_sk.public(),
            &mut friends,
            &mut conversations,
        )
        .expect("should succeed (not an error to be not ready)");

        match result {
            FirstConversationResult::NotReady { peer, reason } => {
                assert_eq!(peer, their_sk.public());
                assert_eq!(
                    reason,
                    NotReadyReason::PendingFriendRequest,
                    "should be PendingFriendRequest when record exists"
                );
            }
            other => panic!("expected NotReady, got {other:?}"),
        }

        assert!(
            conversations.is_empty(),
            "no conversation should be created"
        );
    }

    #[test]
    fn test_first_conversation_blocked_peer() {
        // Blocked peer should return an error.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let dir = temp_dir("first-conv-blocked");
        let mut friends = FriendsStore::empty_at(&dir);
        let mut conversations = ConversationStore::empty_at(&dir);

        let fid = FriendId::from_public_key(their_sk.public());
        friends.set_relationship(fid, FriendRelationship::Blocked);
        friends.save().ok();

        let err = open_or_create_first_conversation(
            &our_sk.public(),
            their_sk.public(),
            &mut friends,
            &mut conversations,
        )
        .expect_err("blocked peer should return an error");

        assert!(
            err.to_string().contains("blocked"),
            "error should mention blocked: {err}"
        );

        assert!(
            conversations.is_empty(),
            "no conversation should be created"
        );
    }

    #[test]
    fn test_first_conversation_persists_across_restart() {
        // Create a conversation, reload stores from disk, verify it survived.
        let our_sk = SecretKey::generate();
        let their_sk = SecretKey::generate();
        let dir = temp_dir("first-conv-restart");

        // ── First session ────────────────────────────────────────────────
        let mut friends = FriendsStore::empty_at(&dir);
        let mut conversations = ConversationStore::empty_at(&dir);
        let fid = FriendId::from_public_key(their_sk.public());
        friends.set_relationship(fid.clone(), FriendRelationship::Friends);
        friends.save().ok();

        let result = open_or_create_first_conversation(
            &our_sk.public(),
            their_sk.public(),
            &mut friends,
            &mut conversations,
        )
        .expect("should succeed");

        let topic = match result {
            FirstConversationResult::Ready { topic } => topic,
            other => panic!("expected Ready, got {other:?}"),
        };

        // Stores were saved by the function
        assert!(
            conversations.file_path().exists(),
            "conversations file should exist"
        );

        // ── After restart: reload from disk ────────────────────────────
        let reloaded_friends = FriendsStore::load(&dir).expect("reload friends");
        let reloaded_convos = ConversationStore::load(&dir).expect("reload conversations");

        // Friend record preserved
        let record = reloaded_friends
            .get(&fid)
            .expect("friend record should survive restart");
        let dc = record
            .direct_conversation
            .as_ref()
            .expect("direct conversation should survive restart");
        assert_eq!(dc.topic, topic);
        assert_eq!(dc.state, DirectConversationState::Active);

        // Conversation entry preserved
        let entry = reloaded_convos
            .find(&topic)
            .expect("conversation entry should survive restart");
        assert_eq!(entry.peer_id, their_sk.public().to_string());
    }
}
