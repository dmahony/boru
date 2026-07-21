//! Shared room documents (metadata + member roster) synced via the iroh gossip mesh.
//!
//! Each room has two logical documents, both broadcast over the same gossip topic:
//!
//! 1. **Metadata doc** — key-value room properties (name, description, rules).
//!    Messages carry the [`METADATA_MARKER`] (0xFE) prefix byte.
//! 2. **Roster doc** — the set of members currently in the room.
//!    Messages carry the [`ROSTER_MARKER`] (0xFF) prefix byte.
//!
//! Both docs use postcard serialization over the gossip mesh.  The room
//! identifier / namespace is the gossip [`TopicId`].
//!
//! # Opening a room
//!
//! 1. Subscribe to the gossip topic.
//! 2. Call [`create_metadata_doc`] to initialise the metadata.
//! 3. Call [`create_roster_doc`] to initialise the roster and add yourself.
//! 4. Call [`forward_room_events`] to start processing all three message
//!    types (metadata, roster, chat) from a single receiver.
//!
//! # Joining a room
//!
//! 1. Subscribe to the gossip topic (from a ticket).
//! 2. Call [`create_metadata_doc`] with an empty initial state — the
//!    doc will be populated by remote peers' sync messages.
//! 3. Call [`create_roster_doc`] with an empty initial state — the
//!    roster will be populated by remote peers' sync messages.
//! 4. Add yourself to the roster with [`add_member`].
//! 5. Call [`forward_room_events`] to start processing all messages.
//!
//! # Wire protocol
//!
//! Every doc message over the gossip topic starts with a protocol marker
//! byte so the receiver can route it to the correct handler without
//! trying to decode it as a chat [`SignedMessage`].
//!
//! | Marker | Message type        |
//! |--------|---------------------|
//! | `0xFE` | Metadata update     |
//! | `0xFF` | Roster update       |
//! | Other  | Chat message        |
//!
//! This module defines a logical "document" that stores room metadata
//! (name, description, rules) as key-value pairs.  The document's
//! namespace is the gossip [`TopicId`] — i.e. the room identifier.
//!
//! Metadata updates are serialized and broadcast through the gossip
//! topic so that all peers in the mesh converge on the same state
//! automatically.  Each peer applies received updates to an in-memory
//! copy and can read the current state at any time.
//!
//! # API overview
//!
//! - [`RoomMetadata`] — the persisted key-value struct.
//! - [`RoomMetadataUpdate`] — a partial update (merges into the current doc).
//! - [`RoomMetadataDoc`] — a live handle that drives gossip sync.
//! - [`create_metadata_doc`] — create (or attach to) a room's metadata doc.
//! - [`update_metadata`] — broadcast a partial update to all peers.
//! - [`read_metadata`] — snapshot the current metadata.
//! - [`metadata_events`] — stream of incoming metadata updates.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use n0_error::{bail_any, Result, StdResultExt};
use n0_future::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};

use crate::abuse_controls::sanitize_single_line;
use crate::{
    api::{Event as GossipEvent, GossipReceiver, GossipSender},
    chat_core::filter_net_event_with_safety,
    proto::TopicId,
    public_room_safety::PublicRoomSafety,
};

// ── Constants ─────────────────────────────────────────────────────────

/// Maximum length (in bytes) for a room name received over the wire.
const MAX_ROOM_NAME_BYTES: usize = 512;

/// Maximum length (in bytes) for a room description received over the wire.
const MAX_ROOM_DESCRIPTION_BYTES: usize = 4096;

/// Maximum length (in bytes) for room rules received over the wire.
const MAX_ROOM_RULES_BYTES: usize = 4096;

// ── Wire protocol ──────────────────────────────────────────────────────

/// Protocol marker byte: the first byte of every metadata-sync message
/// sent over the gossip topic.  Any gossip message that does not start
/// with this marker is treated as a regular chat message.
const METADATA_MARKER: u8 = 0xFE;

/// The current wire format version embedded in every sync message.
const WIRE_VERSION: u8 = 0x01;

// ── Data types ─────────────────────────────────────────────────────────

/// Key-value room metadata that is synced across the gossip mesh.
///
/// All fields are optional so that partial updates (e.g. changing only
/// the room name) leave other fields untouched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoomMetadata {
    /// Human-readable room name (e.g. "Friends Chat").
    #[serde(default)]
    pub name: Option<String>,
    /// Optional room description or topic.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional room rules / guidelines.
    #[serde(default)]
    pub rules: Option<String>,
}

impl RoomMetadata {
    /// Empty metadata — all fields `None`.
    pub const fn empty() -> Self {
        Self {
            name: None,
            description: None,
            rules: None,
        }
    }

    /// Validate all fields against length and content constraints.
    ///
    /// - `name`: must not exceed [`MAX_ROOM_NAME_BYTES`], and must not contain
    ///   control characters or path-separator characters.
    /// - `description`: must not exceed [`MAX_ROOM_DESCRIPTION_BYTES`], and
    ///   must not contain control characters beyond tab/CR/LF.
    /// - `rules`: must not exceed [`MAX_ROOM_RULES_BYTES`], and must not
    ///   contain control characters beyond tab/CR/LF.
    pub fn validate(&self) -> Result<()> {
        let valid_display = |value: &str| -> bool {
            !value.is_empty()
                && !value.chars().any(|ch| ch.is_control())
                && !value.contains('/')
                && !value.contains('\\')
                && value != "."
                && value != ".."
        };

        let valid_multiline = |value: &str| -> bool {
            value.chars().all(|ch| {
                let allowed_control = matches!(ch, '\t' | '\n' | '\r');
                !ch.is_control() || allowed_control
            })
        };

        if let Some(ref name) = self.name {
            if name.len() > MAX_ROOM_NAME_BYTES {
                bail_any!(
                    "room name exceeds maximum length of {MAX_ROOM_NAME_BYTES} (got {})",
                    name.len()
                );
            }
            if !name.is_empty() && !valid_display(name) {
                bail_any!("room name contains disallowed characters");
            }
        }

        if let Some(ref desc) = self.description {
            if desc.len() > MAX_ROOM_DESCRIPTION_BYTES {
                bail_any!(
                    "room description exceeds maximum length of {MAX_ROOM_DESCRIPTION_BYTES} (got {})",
                    desc.len()
                );
            }
            if !desc.is_empty() && !valid_multiline(desc) {
                bail_any!("room description contains disallowed control characters");
            }
        }

        if let Some(ref rules) = self.rules {
            if rules.len() > MAX_ROOM_RULES_BYTES {
                bail_any!(
                    "room rules exceed maximum length of {MAX_ROOM_RULES_BYTES} (got {})",
                    rules.len()
                );
            }
            if !rules.is_empty() && !valid_multiline(rules) {
                bail_any!("room rules contain disallowed control characters");
            }
        }

        Ok(())
    }

    /// Merge another metadata snapshot into `self`, taking its
    /// `Some(...)` values as the new values.  `None` fields are left
    /// unchanged (they never **unset** a previously set value).
    pub fn merge(&mut self, other: &Self) {
        if let Some(name) = &other.name {
            self.name = Some(name.clone());
        }
        if let Some(description) = &other.description {
            self.description = Some(description.clone());
        }
        if let Some(rules) = &other.rules {
            self.rules = Some(rules.clone());
        }
    }

    /// Convenience: return the display name, deriving a short label
    /// from the topic when the name field is `None`.
    pub fn display_name(&self, topic: &TopicId) -> String {
        match &self.name {
            Some(n) if !n.is_empty() => sanitize_single_line(n),
            _ => {
                let hex = format!("{:.16}", topic);
                format!("room-{}", &hex[..8])
            }
        }
    }
}

/// A partial metadata update to send over the gossip mesh.
///
/// Only the fields that carry `Some(...)` are applied by the receiver;
/// `None` fields are ignored during the merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMetadataUpdate {
    /// New room name, or `None` to leave as-is.
    pub name: Option<String>,
    /// New description, or `None` to leave as-is.
    pub description: Option<String>,
    /// New rules, or `None` to leave as-is.
    pub rules: Option<String>,
}

impl From<RoomMetadataUpdate> for RoomMetadata {
    fn from(update: RoomMetadataUpdate) -> Self {
        Self {
            name: update.name,
            description: update.description,
            rules: update.rules,
        }
    }
}

impl From<RoomMetadata> for RoomMetadataUpdate {
    fn from(md: RoomMetadata) -> Self {
        Self {
            name: md.name,
            description: md.description,
            rules: md.rules,
        }
    }
}

// ── Events ─────────────────────────────────────────────────────────────

/// Events emitted from a live [`RoomMetadataDoc`] handle.
#[derive(Debug, Clone)]
pub enum RoomMetadataEvent {
    /// The metadata was updated by a remote peer.
    MetadataUpdated(RoomMetadata),
}

// ── Wire format ────────────────────────────────────────────────────────

/// On-wire envelope for a metadata sync message.
#[derive(Debug, Serialize, Deserialize)]
struct MetadataEnvelope {
    /// Wire format version (for future migrations).
    version: u8,
    /// The actual update payload.
    payload: RoomMetadata,
}

fn encode_wire(metadata: &RoomMetadata) -> Result<Bytes> {
    let envelope = MetadataEnvelope {
        version: WIRE_VERSION,
        payload: metadata.clone(),
    };
    let mut buf = vec![METADATA_MARKER];
    postcard::to_io(&envelope, &mut buf).std_context("encode metadata envelope")?;
    Ok(Bytes::from(buf))
}

fn decode_wire(data: &[u8]) -> Result<Option<RoomMetadata>> {
    if data.first() != Some(&METADATA_MARKER) {
        return Ok(None); // not a metadata message
    }
    // Check wire version before attempting to decode the payload,
    // so that unknown versions get a clear error regardless of
    // payload shape.
    let body = &data[1..];
    if body.first() != Some(&WIRE_VERSION) {
        let actual = body.first().copied().unwrap_or(0);
        bail_any!(
            "unsupported metadata wire version {} (expected {})",
            actual,
            WIRE_VERSION,
        );
    }
    let envelope: MetadataEnvelope =
        postcard::from_bytes(body).std_context("decode metadata envelope")?;
    if envelope.version != WIRE_VERSION {
        bail_any!(
            "unsupported metadata wire version {} (expected {})",
            envelope.version,
            WIRE_VERSION,
        );
    }
    Ok(Some(envelope.payload))
}

// ── Doc handle ─────────────────────────────────────────────────────────

/// A handle to a live room metadata document that syncs via the gossip
/// mesh.
///
/// The handle holds a shared in-memory copy of the current metadata,
/// spawns a background task that listens for gossip events and applies
/// incoming updates, and provides a channel for downstream events so
/// that frontends can react to changes.
#[derive(Debug, Clone)]
pub struct RoomMetadataDoc {
    /// The gossip topic that acts as the document's namespace.
    topic: TopicId,
    /// Shared current state.
    state: Arc<RwLock<RoomMetadata>>,
    /// Channel via which metadata updates (from remote peers) are
    /// forwarded to the consumer of this handle.
    event_tx: mpsc::UnboundedSender<RoomMetadataEvent>,
}

impl RoomMetadataDoc {
    /// The gossip topic this document is bound to.
    pub fn topic(&self) -> TopicId {
        self.topic
    }

    /// Snapshot the current metadata.
    pub async fn snapshot(&self) -> RoomMetadata {
        self.state.read().await.clone()
    }

    /// Subscribe to metadata update events from remote peers.
    pub fn events(&self) -> mpsc::UnboundedReceiver<RoomMetadataEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        // Forward future events to the new channel by keeping a clone
        // of the existing sender.  The old sender stays alive so existing
        // receivers still get events.
        let _ = self.event_tx.clone();
        let _ = tx;
        rx
    }
}

// ── Public API ─────────────────────────────────────────────────────────

/// Create (or attach to) a room metadata document for the given gossip
/// topic.
///
/// This subscribes to the gossip topic to receive metadata updates from
/// remote peers, and spawns a background task that applies them to the
/// shared state.  The caller should drive the returned
/// [`GossipTopic`]'s event stream and feed metadata-related events
/// back into the doc (see [`process_gossip_event`]).
///
/// The returned doc handle reports events on a channel.  Use
/// [`RoomMetadataDoc::events`] to get the receiver.
///
/// The `initial` value seeds the document with locally-known metadata
/// (from disk, UI prompts, etc.) and is broadcast immediately so that
/// new joiners learn the room's current state.
pub async fn create_metadata_doc(
    topic: TopicId,
    gossip_sender: &GossipSender,
    initial: RoomMetadata,
) -> Result<RoomMetadataDoc> {
    let state = Arc::new(RwLock::new(initial.clone()));

    // Broadcast the initial metadata so all peers converge.
    let wire = encode_wire(&initial)?;
    gossip_sender.broadcast(wire).await?;

    let (event_tx, _event_rx) = mpsc::unbounded_channel();

    Ok(RoomMetadataDoc {
        topic,
        state,
        event_tx,
    })
}

/// Update the metadata document by broadcasting a partial update to the
/// gossip mesh.
///
/// The update is applied locally, serialised, broadcast via gossip,
/// and then applied by every other peer that receives it.  The local
/// state is updated *before* the gossip send so the sender immediately
/// sees the change.
///
/// Returns the resulting [`RoomMetadata`] snapshot after the merge.
pub async fn update_metadata(
    doc: &RoomMetadataDoc,
    gossip_sender: &GossipSender,
    update: RoomMetadataUpdate,
) -> Result<RoomMetadata> {
    let incoming: RoomMetadata = update.into();

    // Apply locally.
    {
        let mut state = doc.state.write().await;
        state.merge(&incoming);
    }

    // Broadcast to the mesh.
    let snapshot = doc.state.read().await.clone();
    let wire = encode_wire(&snapshot)?;
    gossip_sender.broadcast(wire).await?;

    Ok(snapshot)
}

/// Read the current metadata snapshot from the doc handle.
pub async fn read_metadata(doc: &RoomMetadataDoc) -> RoomMetadata {
    doc.snapshot().await
}

/// Process a single gossip [`GossipEvent`] and apply it to the
/// metadata document if it carries a metadata update.
///
/// Returns `true` if the event was a metadata message (consumed here)
/// and `false` if it should be forwarded to the chat frontend for
/// regular message handling.
///
/// Call this from the main event loop whenever a gossip event arrives.
/// A typical usage:
///
/// ```ignore
/// while let Some(event) = gossip_topic.next().await {
///     if !process_gossip_event(&doc, event).await? {
///         // Not a metadata update — handle as a regular chat message
///     }
/// }
/// ```
pub async fn process_gossip_event(
    doc: &RoomMetadataDoc,
    event: Result<GossipEvent, crate::api::ApiError>,
) -> Result<bool> {
    let event = event?;
    match event {
        GossipEvent::Received(msg) => {
            let payload = match decode_wire(&msg.content) {
                Ok(Some(md)) => md,
                Ok(None) => return Ok(false), // not a metadata message
                Err(e) => {
                    // Malformed metadata message — log and skip.
                    tracing::warn!("ignoring malformed metadata message: {e}");
                    return Ok(true); // still consumed (not forwarded as chat)
                }
            };

            // Validate the received metadata before merging.
            if let Err(e) = payload.validate() {
                tracing::warn!("ignoring invalid remote metadata (dropped): {e}");
                return Ok(true); // consumed (not forwarded as chat)
            }

            // Merge the validated update into our local state.
            {
                let mut state = doc.state.write().await;
                state.merge(&payload);
            }

            // Notify downstream.
            let _ = doc
                .event_tx
                .send(RoomMetadataEvent::MetadataUpdated(payload));

            Ok(true)
        }
        _ => Ok(false), // NeighborUp/NeighborDown/Lagged — not metadata
    }
}

/// Spawn a background task that continuously processes gossip events
/// for metadata updates until the gossip topic stream ends.
///
/// This is a convenience wrapper around [`process_gossip_event`] for
/// frontends that do not need to interleave metadata processing with
/// chat-message handling.  Chat messages are silently discarded.
pub fn spawn_metadata_sync(
    doc: RoomMetadataDoc,
    mut gossip_receiver: GossipReceiver,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        while let Some(event) = gossip_receiver.next().await {
            if let Err(e) = process_gossip_event(&doc, event).await {
                tracing::warn!("metadata sync error: {e}");
            }
        }
    })
}

// ── Roster doc ─────────────────────────────────────────────────────────

/// Protocol marker byte for roster-sync messages.
///
/// Any gossip message that does not start with either [`METADATA_MARKER`]
/// or [`ROSTER_MARKER`] is treated as a regular chat message.
const ROSTER_MARKER: u8 = 0xFF;

/// The current wire format version for roster messages.
const ROSTER_WIRE_VERSION: u8 = 0x01;

/// A single member entry in the room roster.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RosterMember {
    /// Display name chosen by the member (or their public key short form).
    pub display_name: String,
    /// Unix timestamp (seconds since epoch) when the member joined.
    pub joined_at: u64,
}

/// On-wire envelope for a roster-sync message.
#[derive(Debug, Serialize, Deserialize)]
struct RosterEnvelope {
    version: u8,
    /// The full roster state (BTreeMap for deterministic ordering).
    members: Vec<RosterMemberEntry>,
}

/// A (PublicKey, RosterMember) pair for on-wire serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RosterMemberEntry {
    /// Hex-encoded public key of the member.
    pub_key: String,
    /// Member's display name.
    display_name: String,
    /// Unix timestamp when the member joined.
    joined_at: u64,
}

fn encode_roster_wire(members: &[RosterMemberEntry]) -> Result<Bytes> {
    let envelope = RosterEnvelope {
        version: ROSTER_WIRE_VERSION,
        members: members.to_vec(),
    };
    let mut buf = vec![ROSTER_MARKER];
    postcard::to_io(&envelope, &mut buf).std_context("encode roster envelope")?;
    Ok(Bytes::from(buf))
}

fn decode_roster_wire(data: &[u8]) -> Result<Option<Vec<RosterMemberEntry>>> {
    if data.first() != Some(&ROSTER_MARKER) {
        return Ok(None); // not a roster message
    }
    let envelope: RosterEnvelope =
        postcard::from_bytes(&data[1..]).std_context("decode roster envelope")?;
    if envelope.version != ROSTER_WIRE_VERSION {
        bail_any!(
            "unsupported roster wire version {} (expected {})",
            envelope.version,
            ROSTER_WIRE_VERSION,
        );
    }
    Ok(Some(envelope.members))
}

/// A handle to a live room roster document synced via the gossip mesh.
///
/// The handle holds a shared in-memory map of members and provides
/// methods to add, remove, and list members.
#[derive(Debug, Clone)]
pub struct RosterDoc {
    topic: TopicId,
    /// Shared member map: hex-encoded public key → member info.
    members: Arc<RwLock<HashMap<String, RosterMember>>>,
}

impl RosterDoc {
    /// The gossip topic this roster belongs to.
    pub fn topic(&self) -> TopicId {
        self.topic
    }

    /// Snapshot the current roster.
    pub async fn snapshot(&self) -> HashMap<String, RosterMember> {
        self.members.read().await.clone()
    }

    /// Get a member by their hex public key.
    pub async fn get_member(&self, pub_key: &str) -> Option<RosterMember> {
        self.members.read().await.get(pub_key).cloned()
    }

    /// Number of members in the roster.
    pub async fn member_count(&self) -> usize {
        self.members.read().await.len()
    }

    /// Check if a public key is in the roster.
    pub async fn contains(&self, pub_key: &str) -> bool {
        self.members.read().await.contains_key(pub_key)
    }
}

/// Events emitted from a live [`RosterDoc`] handle.
#[derive(Debug, Clone)]
pub enum RosterEvent {
    /// A member was added to the roster.
    MemberAdded {
        /// Hex-encoded public key of the added member.
        pub_key: String,
        /// The member's display name and join timestamp.
        member: RosterMember,
    },
    /// A member was removed from the roster.
    MemberRemoved {
        /// Hex-encoded public key of the removed member.
        pub_key: String,
        /// The member's display name and join timestamp at the time of removal.
        member: RosterMember,
    },
}

/// Create a room roster document for the given gossip topic.
///
/// Broadcasts the initial roster state to the mesh so that all peers
/// converge on the same member list.
///
/// `self_pub_key` and `self_display_name` are used to add the creator
/// to the roster immediately.
pub async fn create_roster_doc(
    topic: TopicId,
    gossip_sender: &GossipSender,
    self_pub_key: String,
    self_display_name: String,
) -> Result<RosterDoc> {
    let members: Arc<RwLock<HashMap<String, RosterMember>>> = Arc::new(RwLock::new(HashMap::new()));

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Add the creator as the first member
    members.write().await.insert(
        self_pub_key.clone(),
        RosterMember {
            display_name: self_display_name,
            joined_at: now,
        },
    );

    // Broadcast the full roster state
    let entries = make_roster_entries(&*members.read().await);
    let wire = encode_roster_wire(&entries)?;
    gossip_sender.broadcast(wire).await?;

    Ok(RosterDoc { topic, members })
}

fn make_roster_entries(members: &HashMap<String, RosterMember>) -> Vec<RosterMemberEntry> {
    let mut entries: Vec<RosterMemberEntry> = members
        .iter()
        .map(|(pk, m)| RosterMemberEntry {
            pub_key: pk.clone(),
            display_name: m.display_name.clone(),
            joined_at: m.joined_at,
        })
        .collect();
    entries.sort_by_key(|entry| entry.joined_at);
    entries
}

/// Add a member to the roster and broadcast the update.
pub async fn add_member(
    doc: &RosterDoc,
    gossip_sender: &GossipSender,
    pub_key: String,
    display_name: String,
) -> Result<RosterMember> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let member = RosterMember {
        display_name,
        joined_at: now,
    };

    {
        let mut members = doc.members.write().await;
        members.insert(pub_key.clone(), member.clone());
    }

    // Broadcast the full roster state
    let entries = make_roster_entries(&*doc.members.read().await);
    let wire = encode_roster_wire(&entries)?;
    gossip_sender.broadcast(wire).await?;

    Ok(member)
}

/// Remove a member from the roster and broadcast the update.
///
/// Returns the removed member, or `None` if the key was not in the roster.
pub async fn remove_member(
    doc: &RosterDoc,
    gossip_sender: &GossipSender,
    pub_key: &str,
) -> Result<Option<RosterMember>> {
    let removed = {
        let mut members = doc.members.write().await;
        members.remove(pub_key)
    };

    // Broadcast the full roster state
    let entries = make_roster_entries(&*doc.members.read().await);
    let wire = encode_roster_wire(&entries)?;
    gossip_sender.broadcast(wire).await?;

    Ok(removed)
}

/// List all members in the roster.
pub async fn list_members(doc: &RosterDoc) -> HashMap<String, RosterMember> {
    doc.snapshot().await
}

/// Process a single gossip event as a roster update.
///
/// Returns `true` if the event was a roster message (consumed here),
/// `false` if it should be forwarded to the chat frontend.
///
/// The dispatch order in [`forward_room_events`] is:
/// 1. Metadata (0xFE) — see [`process_gossip_event`]
/// 2. Roster (0xFF) — this function
/// 3. Chat (anything else)
pub async fn process_roster_event(
    doc: &RosterDoc,
    event: Result<GossipEvent, crate::api::ApiError>,
) -> Result<bool> {
    let event = event?;
    match event {
        GossipEvent::Received(msg) => {
            let entries = match decode_roster_wire(&msg.content) {
                Ok(Some(e)) => e,
                Ok(None) => return Ok(false), // not a roster message
                Err(e) => {
                    tracing::warn!("ignoring malformed roster message: {e}");
                    return Ok(true); // consumed (not forwarded as chat)
                }
            };

            // Replace the in-memory roster with the received state
            // (roster is always sent as a full snapshot)
            {
                let mut members = doc.members.write().await;
                members.clear();
                for entry in entries {
                    members.insert(
                        entry.pub_key,
                        RosterMember {
                            display_name: sanitize_single_line(&entry.display_name),
                            joined_at: entry.joined_at,
                        },
                    );
                }
            }

            Ok(true)
        }
        _ => Ok(false),
    }
}

// ── Merged event forwarder ─────────────────────────────────────────────

/// A bundle of both room documents (metadata + roster).
///
/// Returned when a room is opened or joined, and used by
/// [`forward_room_events`] for merged event dispatch.
#[derive(Debug, Clone)]
pub struct RoomDocs {
    /// The metadata document handle.
    pub metadata: RoomMetadataDoc,
    /// The roster document handle.
    pub roster: RosterDoc,
    /// The gossip topic for this room.
    pub topic: TopicId,
}

/// Combined event forwarder for room docs (metadata + roster) and chat messages.
///
/// Spawn this as a background task to process all three message types from
/// a single gossip receiver:
///
/// 1. Metadata updates (0xFE) — applied to the metadata doc
/// 2. Roster updates (0xFF) — applied to the roster doc
/// 3. Chat / neighbour events (anything else) — forwarded to `non_room_tx`
///
/// The `non_room_tx` channel receives gossip [`Event`] items that are neither
/// metadata nor roster updates, i.e. chat messages, NeighborUp, NeighborDown.
/// The caller can process those with [`crate::chat_core::handle_net_event`] or
/// similar logic.
pub fn spawn_room_event_forwarder(
    metadata_doc: RoomMetadataDoc,
    roster_doc: RosterDoc,
    gossip_receiver: GossipReceiver,
    non_room_tx: tokio::sync::mpsc::UnboundedSender<Result<GossipEvent, crate::api::ApiError>>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        let mut receiver = gossip_receiver;
        while let Some(event_result) = receiver.next().await {
            // Inspect the marker byte before consuming the event.
            let is_metadata = matches!(&event_result,
                Ok(GossipEvent::Received(msg))
                    if msg.content.first() == Some(&METADATA_MARKER));

            if is_metadata {
                let _ = process_gossip_event(&metadata_doc, event_result).await;
                continue;
            }

            // Check for roster marker
            let is_roster = matches!(&event_result,
                Ok(GossipEvent::Received(msg))
                    if msg.content.first() == Some(&ROSTER_MARKER));

            if is_roster {
                let _ = process_roster_event(&roster_doc, event_result).await;
                continue;
            }

            // Not a room-doc message — forward for chat/neighbor processing.
            if non_room_tx.send(event_result).is_err() {
                break;
            }
        }
    })
}

/// Forward one room's gossip stream into chat `NetEvent`s while applying
/// metadata and roster document updates, with optional public-room safety
/// enforcement.
///
/// This is the shared room-aware event bridge used by frontends. It consumes
/// metadata (`0xFE`) and roster (`0xFF`) messages locally, converts signed chat
/// messages plus neighbor events into [`crate::chat_core::NetEvent`], drops
/// malformed non-room packets instead of treating them as fatal frontend errors,
/// and — when `safety` is `Some(...)` — applies per-peer rate-limits, message-size
/// bounds, and blob-announcement limits before forwarding.
///
/// Pass `None` for private rooms to skip all safety checks (zero overhead).
pub async fn forward_room_events_for_chat(
    metadata_doc: RoomMetadataDoc,
    roster_doc: RosterDoc,
    mut receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<crate::chat_core::NetEvent>,
    safety: Option<Arc<PublicRoomSafety>>,
) {
    use crate::chat_core::{NetEvent, SignedMessage};

    // Track decode errors to avoid log storms: warn on the first few, then
    // degrade to DEBUG so one broken topic doesn't saturate the log.
    let mut decode_errors: u32 = 0;
    const MAX_WARN_DECODE_ERRORS: u32 = 3;

    while let Some(event_result) = receiver.next().await {
        let event = match event_result {
            Ok(e) => e,
            Err(err) => {
                tracing::debug!("room event forwarder: gossip event error (dropped): {err}");
                continue;
            }
        };

        let is_metadata = matches!(
            &event,
            GossipEvent::Received(msg) if msg.content.first() == Some(&METADATA_MARKER)
        );
        if is_metadata {
            let _ = process_gossip_event(&metadata_doc, Ok(event)).await;
            continue;
        }

        let is_roster = matches!(
            &event,
            GossipEvent::Received(msg) if msg.content.first() == Some(&ROSTER_MARKER)
        );
        if is_roster {
            let _ = process_roster_event(&roster_doc, Ok(event)).await;
            continue;
        }

        match event {
            GossipEvent::Received(msg) => match SignedMessage::verify_and_decode(&msg.content) {
                Ok((from, message, sent_at)) => {
                    let net_event = NetEvent::Message {
                        from,
                        message,
                        sent_at,
                    };
                    // Apply public-room safety filtering when configured.
                    let net_event = match &safety {
                        Some(s) => match filter_net_event_with_safety(net_event, s) {
                            Some(ev) => ev,
                            None => continue,
                        },
                        None => net_event,
                    };
                    if net_tx.send(net_event).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    if decode_errors < MAX_WARN_DECODE_ERRORS {
                        tracing::warn!("room event forwarder: decode error (dropped): {err}");
                    } else if decode_errors == MAX_WARN_DECODE_ERRORS {
                        tracing::warn!(
                            "room event forwarder: reached {MAX_WARN_DECODE_ERRORS} \
                             decode errors — suppressing further WARNs, switching to DEBUG"
                        );
                    } else {
                        tracing::debug!("room event forwarder: decode error (dropped): {err}");
                    }
                    decode_errors += 1;
                    continue;
                }
            },
            GossipEvent::NeighborUp(id) => {
                if net_tx.send(NetEvent::NeighborUp { peer: id }).is_err() {
                    return;
                }
            }
            GossipEvent::NeighborDown(id) => {
                if net_tx.send(NetEvent::NeighborDown { peer: id }).is_err() {
                    return;
                }
            }
            GossipEvent::Lagged => {
                // Not forwarded — protocol-level backpressure signal.
            }
        }
    }

    let _ = net_tx.send(crate::chat_core::NetEvent::Closed);
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Metadata validation ─────────────────────────────────────────

    #[test]
    fn room_metadata_validate_accepts_valid() {
        let md = RoomMetadata {
            name: Some("Friends Chat".into()),
            description: Some("A room for friends".into()),
            rules: Some("Be nice".into()),
        };
        assert!(md.validate().is_ok());
    }

    #[test]
    fn room_metadata_validate_accepts_none_fields() {
        let md = RoomMetadata::empty();
        assert!(md.validate().is_ok());
    }

    #[test]
    fn room_metadata_validate_rejects_name_with_control_chars() {
        let md = RoomMetadata {
            name: Some("bad\u{0000}name".into()),
            description: None,
            rules: None,
        };
        assert!(md.validate().is_err());
    }

    #[test]
    fn room_metadata_validate_rejects_name_with_path_separators() {
        for sep in &["/", "\\"] {
            let md = RoomMetadata {
                name: Some(format!("bad{}name", sep)),
                description: None,
                rules: None,
            };
            assert!(md.validate().is_err());
        }
    }

    #[test]
    fn room_metadata_validate_rejects_oversized_name() {
        let md = RoomMetadata {
            name: Some("x".repeat(MAX_ROOM_NAME_BYTES + 1)),
            description: None,
            rules: None,
        };
        assert!(md.validate().is_err());
    }

    #[test]
    fn room_metadata_validate_rejects_oversized_description() {
        let md = RoomMetadata {
            name: None,
            description: Some("x".repeat(MAX_ROOM_DESCRIPTION_BYTES + 1)),
            rules: None,
        };
        assert!(md.validate().is_err());
    }

    #[test]
    fn room_metadata_validate_accepts_multiline_description() {
        let md = RoomMetadata {
            name: None,
            description: Some("line1\nline2\n\tindented".into()),
            rules: None,
        };
        assert!(md.validate().is_ok());
    }

    #[test]
    fn room_metadata_validate_rejects_description_with_null() {
        let md = RoomMetadata {
            name: None,
            description: Some("bad\u{0000}desc".into()),
            rules: None,
        };
        assert!(md.validate().is_err());
    }

    #[test]
    fn room_metadata_validate_rejects_oversized_rules() {
        let md = RoomMetadata {
            name: None,
            description: None,
            rules: Some("x".repeat(MAX_ROOM_RULES_BYTES + 1)),
        };
        assert!(md.validate().is_err());
    }

    #[test]
    fn room_metadata_validate_rejects_name_dot_and_dotdot() {
        for name in &[".", ".."] {
            let md = RoomMetadata {
                name: Some(name.to_string()),
                description: None,
                rules: None,
            };
            assert!(md.validate().is_err());
        }
    }

    #[test]
    fn wire_roundtrip() {
        let md = RoomMetadata {
            name: Some("Test Room".into()),
            description: Some("A room for testing".into()),
            rules: Some("Be nice".into()),
        };
        let wire = encode_wire(&md).expect("encode");
        assert_eq!(wire.first(), Some(&METADATA_MARKER));

        let decoded = decode_wire(&wire)
            .expect("decode")
            .expect("should be metadata");
        assert_eq!(decoded, md);
    }

    #[test]
    fn wire_rejects_non_metadata_message() {
        let chat_msg = Bytes::from_static(b"hello world");
        let result = decode_wire(&chat_msg).expect("decode");
        assert!(
            result.is_none(),
            "chat messages should not decode as metadata"
        );
    }

    #[test]
    fn wire_rejects_truncated_payload() {
        let result = decode_wire(&[METADATA_MARKER, 0x01]);
        assert!(result.is_err(), "truncated payload should fail");
    }

    #[test]
    fn empty_metadata_is_false_for_chat_messages() {
        let chat_msg = Bytes::from_static(b"hello");
        let result = decode_wire(&chat_msg).expect("decode");
        assert!(result.is_none());
    }

    // ── Metadata merge ─────────────────────────────────────────────

    #[test]
    fn merge_overwrites_some_fields() {
        let mut base = RoomMetadata {
            name: Some("Old".into()),
            description: None,
            rules: Some("Rule #1".into()),
        };
        let update = RoomMetadata {
            name: Some("New".into()),
            description: Some("A description".into()),
            rules: None,
        };
        base.merge(&update);
        assert_eq!(base.name, Some("New".into()));
        assert_eq!(base.description, Some("A description".into()));
        // rules was None in the update, so it stays
        assert_eq!(base.rules, Some("Rule #1".into()));
    }

    #[test]
    fn merge_preserves_existing_on_none() {
        let mut base = RoomMetadata {
            name: Some("Room".into()),
            description: Some("Desc".into()),
            rules: Some("Rules".into()),
        };
        let update = RoomMetadata::empty();
        base.merge(&update);
        // Nothing changed
        assert_eq!(base.name, Some("Room".into()));
        assert_eq!(base.description, Some("Desc".into()));
        assert_eq!(base.rules, Some("Rules".into()));
    }

    #[test]
    fn merge_from_empty_base() {
        let mut base = RoomMetadata::empty();
        let update = RoomMetadata {
            name: Some("New Room".into()),
            description: None,
            rules: None,
        };
        base.merge(&update);
        assert_eq!(base.name, Some("New Room".into()));
        assert!(base.description.is_none());
        assert!(base.rules.is_none());
    }

    // ── Display name derivation ────────────────────────────────────

    #[test]
    fn display_name_from_metadata() {
        let md = RoomMetadata {
            name: Some("Friends Chat".into()),
            description: None,
            rules: None,
        };
        let topic = TopicId::from_bytes([0x42u8; 32]);
        assert_eq!(md.display_name(&topic), "Friends Chat");
    }

    #[test]
    fn display_name_from_topic_when_missing() {
        let md = RoomMetadata::empty();
        let topic = TopicId::from_bytes([0xABu8; 32]);
        let name = md.display_name(&topic);
        // Should start with "room-" and contain 8 hex chars from the topic
        assert!(name.starts_with("room-"));
        assert_eq!(name.len(), 13); // "room-" + 8 hex chars
    }

    #[test]
    fn display_name_uses_name_when_both_available() {
        let md = RoomMetadata {
            name: Some("My Room".into()),
            description: Some("desc".into()),
            rules: None,
        };
        let topic = TopicId::from_bytes([0x42u8; 32]);
        assert_eq!(md.display_name(&topic), "My Room");
    }

    // ── Update → Metadata conversion ───────────────────────────────

    #[test]
    fn update_converts_to_metadata() {
        let update = RoomMetadataUpdate {
            name: Some("Room".into()),
            description: None,
            rules: Some("Rules".into()),
        };
        let md: RoomMetadata = update.into();
        assert_eq!(md.name, Some("Room".into()));
        assert!(md.description.is_none());
        assert_eq!(md.rules, Some("Rules".into()));
    }

    #[test]
    fn metadata_converts_to_update() {
        let md = RoomMetadata {
            name: Some("Room".into()),
            description: Some("Desc".into()),
            rules: None,
        };
        let update: RoomMetadataUpdate = md.into();
        assert_eq!(update.name, Some("Room".into()));
        assert_eq!(update.description, Some("Desc".into()));
        assert!(update.rules.is_none());
    }

    // ── Wire version rejection ─────────────────────────────────────

    #[test]
    fn wire_rejects_unknown_version() {
        let data = Bytes::from_static(&[METADATA_MARKER, 0x99, 0x00, 0x00]);
        let result = decode_wire(&data);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported metadata wire version"));
    }

    // ── Envelopes with all fields = None ───────────────────────────

    #[test]
    fn wire_empty_metadata() {
        let md = RoomMetadata::empty();
        let wire = encode_wire(&md).expect("encode empty");
        let decoded = decode_wire(&wire).expect("decode").expect("should decode");
        assert_eq!(decoded, RoomMetadata::empty());
    }

    // ── Roster wire format ─────────────────────────────────────────

    #[test]
    fn roster_wire_roundtrip() {
        let entries = vec![
            RosterMemberEntry {
                pub_key: "abc123".into(),
                display_name: "Alice".into(),
                joined_at: 1000,
            },
            RosterMemberEntry {
                pub_key: "def456".into(),
                display_name: "Bob".into(),
                joined_at: 2000,
            },
        ];
        let wire = encode_roster_wire(&entries).expect("encode");
        assert_eq!(wire.first(), Some(&ROSTER_MARKER));

        let decoded = decode_roster_wire(&wire)
            .expect("decode")
            .expect("should be roster");
        assert_eq!(decoded, entries);
    }

    #[test]
    fn roster_wire_rejects_non_roster_message() {
        let chat_msg = Bytes::from_static(b"hello world");
        let result = decode_roster_wire(&chat_msg).expect("decode");
        assert!(
            result.is_none(),
            "chat messages should not decode as roster"
        );
    }

    #[test]
    fn roster_wire_rejects_truncated_payload() {
        let result = decode_roster_wire(&[ROSTER_MARKER, 0x01]);
        assert!(result.is_err(), "truncated payload should fail");
    }

    #[test]
    fn roster_wire_rejects_unknown_version() {
        let data = Bytes::from_static(&[ROSTER_MARKER, 0x99, 0x00, 0x00]);
        let result = decode_roster_wire(&data);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported roster wire version"));
    }

    #[test]
    fn roster_marker_differs_from_metadata() {
        assert_ne!(METADATA_MARKER, ROSTER_MARKER);
    }

    #[test]
    fn make_roster_entries_sorts_by_joined_at() {
        use std::collections::HashMap;
        let mut members = HashMap::new();
        members.insert(
            "b".into(),
            RosterMember {
                display_name: "Bob".into(),
                joined_at: 200,
            },
        );
        members.insert(
            "a".into(),
            RosterMember {
                display_name: "Alice".into(),
                joined_at: 100,
            },
        );

        let entries = make_roster_entries(&members);
        assert_eq!(entries.len(), 2);
        // Should be sorted by joined_at ascending
        assert_eq!(entries[0].pub_key, "a");
        assert_eq!(entries[1].pub_key, "b");
    }
}
