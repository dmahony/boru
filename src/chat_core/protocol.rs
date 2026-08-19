//! Pure protocol and wire types for the Boru chat — data only, no I/O, no UI.
//!
//! Contains the gossip message envelope ([`Message`]), the signed transport
//! envelope ([`SignedMessage`]), network events ([`NetEvent`]), room
//! invitations ([`Ticket`], [`RoomInvitation`], [`RoomInviteV2`]) and the
//! public-room advertisement helpers.  Everything in this module is a data
//! type or a pure function over data types, so it can be exercised with
//! unit tests without opening a network connection or touching storage.

use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use iroh::{EndpointAddr, PublicKey, SecretKey};
use n0_error::{bail_any, Result, StdResultExt};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use crate::discovery_secret::DiscoverySecret;
use crate::group_encryption::message::EncryptedGroupEnvelope;
use crate::proto::TopicId;
use crate::user_profile::UserProfile;

/// Default maximum age of a received message before it is rejected as stale.
pub const DEFAULT_MESSAGE_TTL: Duration = Duration::from_secs(3600);

/// BORU-DIR-08 (PDF Task 3.2): default advertisement TTL (seconds).
///
/// This is the expiry/refresh mechanism: a room advertisement whose
/// advertiser stops refreshing is considered stale by directory clients
/// `expires_after_secs` after it was received, and is evicted from the
/// directory cache.  The periodic refresh interval (60 s — see
/// `ADVERT_REFRESH_INTERVAL_SECS` in the GUI) is deliberately much shorter
/// than this TTL so temporary packet loss does not flicker rooms in and out
/// of the directory (PDF Task 3.2 step 5).
///
/// Advertisements that predate the `expires_after_secs` wire field decode
/// with this value.
pub const DEFAULT_ADVERT_TTL_SECS: u32 = 300;

/// An event received from the gossip network (decoded from the wire).
#[derive(Debug, Clone)]
pub enum NetEvent {
    /// A decoded message from a peer.
    Message {
        /// Public key of the sender.
        from: PublicKey,
        /// The decoded message payload.
        message: Message,
        /// Unix epoch seconds when the message was sent.
        sent_at: u64,
    },
    /// A peer has joined the gossip mesh (new neighbor connection).
    NeighborUp {
        /// Public key of the peer that joined.
        peer: PublicKey,
    },
    /// A peer has left the gossip mesh (connection dropped or app closed).
    NeighborDown {
        /// Public key of the peer that left.
        peer: PublicKey,
    },
    /// The gossip receiver stream closed.
    Closed,
    /// A fatal network error occurred.
    Error(String),
}

// ── Protocol types ───────────────────────────────────────────────────────────

/// Messages that can be sent between peers in the chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// Announce or change your display name.
    AboutMe {
        /// The new display name.
        name: String,
        /// Optional BlobTicket for the profile image.
        #[serde(default)]
        profile_image_ticket: Option<String>,
    },
    /// A regular text message.
    Message {
        /// The message text.
        text: String,
    },
    /// A text message that points at an earlier message without copying its body.
    Reply {
        /// The reply body.
        text: String,
        /// Stable identifier of the message being answered.
        reply_to_message_id: MessageId,
    },
    /// A text message carrying stable peer-ID mention metadata.
    MessageWithMentions {
        /// The message text, including visible `@label` spellings.
        text: String,
        /// Structured mention ranges keyed by peer ID.
        mentions: Vec<crate::mentions::Mention>,
    },
    /// Ephemeral typing lease; never persisted or rendered as a chat entry.
    Typing {
        /// Whether the sender is currently composing.
        active: bool,
    },
    /// Announce a file available for download.
    FileShare {
        /// The file name (basename only, no path).  For a whole-directory
        /// share this holds the root folder name.
        name: String,
        /// BlobTicket serialized to string.
        ticket: String,
        /// Total file size in bytes, so the receiver can show a
        /// progress bar immediately without waiting for blob metadata.
        #[serde(default, deserialize_with = "deserialize_tolerant_u64")]
        size: u64,
        /// Optional video thumbnail — the blake3 hash of a separate
        /// blob containing the WebP poster bytes.  Receivers fetch the
        /// poster via the same iroh blob mechanism as the file itself,
        /// keeping gossip messages small.
        #[serde(default, deserialize_with = "deserialize_tolerant_opt_hash")]
        thumbnail_hash: Option<MessageHash>,
        /// Optional HashSeq collection root hash.  When present this share
        /// is a whole directory: the `ticket` is a `BlobFormat::HashSeq`
        /// BlobTicket whose root hash is this value, and `name` is the root
        /// folder name.  Absent for the legacy single-file form.
        #[serde(default, deserialize_with = "deserialize_tolerant_opt_hash")]
        collection_hash: Option<MessageHash>,
        /// Number of entries (files) in the collection.  Only meaningful
        /// when `collection_hash` is present; 0 for
        /// single-file shares.
        #[serde(default, deserialize_with = "deserialize_tolerant_u64")]
        collection_entries: u64,
    },
    /// Graceful goodbye — the sender is leaving the chat.
    /// This is a best-effort notification: the gossip protocol also
    /// detects disconnection via NeighborDown events.
    Leave,
    /// Periodic presence heartbeat.
    Presence,
    /// Presence heartbeat plus a ticket for opening a chat with this peer.
    ///
    /// This is additive to [`Message::Presence`] so older peers can still
    /// participate in the presence protocol without understanding tickets.
    PresenceWithTicket {
        /// Serialized chat-room ticket advertised by the sender.
        ticket: String,
    },
    /// Acknowledge that the sender read a message.
    ReadReceipt {
        /// Hash of the message being acknowledged.
        message_hash: MessageHash,
    },
    /// Replace the text of a previously sent message.
    Edit {
        /// Hash of the original message being edited.
        original_hash: MessageHash,
        /// Replacement message text.
        new_text: String,
    },
    /// Mark a previously sent message as deleted.
    Delete {
        /// Hash of the message being deleted.
        message_hash: MessageHash,
    },
    /// Add an emoji reaction to a previously sent message.
    Reaction {
        /// Hash of the message being reacted to.
        message_hash: MessageHash,
        /// Reaction emoji.
        emoji: String,
    },
    /// Authenticated, idempotent reaction add keyed by stable message id and actor.
    ReactionAdd {
        /// Stable id of the message being reacted to.
        message_id: MessageHash,
        /// Reaction emoji (usually one grapheme cluster).
        emoji: String,
    },
    /// Authenticated reaction removal. Removes are tombstones and therefore
    /// remain effective when delivered before the corresponding add.
    ReactionRemove {
        /// Stable id of the message being reacted to.
        message_id: MessageHash,
        /// Reaction emoji to remove for the authenticated actor.
        emoji: String,
    },
    /// Pin a message in the current conversation. The enclosing
    /// `SignedMessage` authenticates the operation and its sender.
    PinMessage {
        /// Stable conversation identifier, preventing cross-room replay.
        topic: TopicId,
        /// Hash of the message being pinned.
        message_hash: MessageHash,
    },
    /// Remove a message pin in the current conversation.
    UnpinMessage {
        /// Stable conversation identifier, preventing cross-room replay.
        topic: TopicId,
        /// Hash of the message whose pin is being removed.
        message_hash: MessageHash,
    },
    /// Announce an image available for download and inline display.
    ImageShare {
        /// The image file name (basename only, no path).
        name: String,
        /// Blob hash for the image content, for blob-store lookup and download.
        hash: MessageHash,
    },
    /// Broadcast a public room advertisement into the directory topic.
    RoomAdvertisement {
        /// The room advertisement payload.
        ad: RoomAdvertisement,
        /// Ed25519 signature over postcard-serialized RoomAdvertisement bytes
        /// by the room creator's node key, so receivers can verify authenticity.
        #[serde(default)]
        signature: Vec<u8>,
    },
    /// Invisible keepalive heartbeat — keeps connections warm and updates
    /// mesh health timestamps without producing any chat log entry or UI
    /// notification.
    ///
    /// Frontends broadcast this periodically (every 2–3 seconds) as a
    /// lightweight gossip message.  Peers receive it and update their
    /// `last_activity` timestamp for the sender, preventing the mesh
    /// health from decaying to "Degraded" or "Offline."
    ///
    /// This is intentionally separate from `Presence`, which is a
    /// *visible* status indicator.
    Heartbeat,
    /// Latency probe ping — asks the receiving peer to reply with a pong
    /// carrying the same `sent_at_ms` so the sender can measure round-trip
    /// time.  Never displayed in the chat log.
    LatencyPing {
        /// Unix epoch milliseconds when this ping was sent.
        sent_at_ms: u64,
    },
    /// Latency probe pong — echoes back the `sent_at_ms` from the
    /// corresponding [`LatencyPing`](crate::chat_core::Message::LatencyPing) so the original sender can compute
    /// round-trip latency.  Never displayed in the chat log.
    LatencyPong {
        /// Unix epoch milliseconds from the original ping.
        sent_at_ms: u64,
    },
    /// A diagnostic probe sent through the normal gossip path — not displayed
    /// as an ordinary chat message by default.
    DiagnosticProbe(crate::diagnostics::DiagnosticProbe),
    /// Opaque contact-layer control message (friend requests, chat invites).
    ContactControl {
        /// Serialised SignedContactMessage payload.
        payload: Vec<u8>,
    },
    /// Publish the sender's profile and metadata over gossip.
    ProfileUpdate(UserProfile),
    /// End-to-end encrypted group message using p2panda's forward-secure
    /// message encryption scheme.  The envelope is serialised via postcard
    /// and authenticated by the gossip-layer signature mechanism.
    EncryptedGroupMessage {
        /// The 32-byte group identifier.
        group_id: [u8; 32],
        /// Forward-secure encrypted group message envelope.
        envelope: EncryptedGroupEnvelope,
    },
    /// Announce an external catalogue GIF (provider-neutral payload).
    ///
    /// Carries a [`crate::gif_provider::SharedGif`] with the direct
    /// rendition URLs, so the receiver renders the media without calling
    /// the provider search endpoint again.  The payload deliberately
    /// contains no API key, search query, or tracking values.
    SharedGif {
        /// Provider-neutral GIF payload.
        #[serde(default)]
        gif: crate::gif_provider::SharedGif,
    },
    /// Broadcast a room withdrawal / tombstone into the directory topic
    /// (BORU-DIR-09, PDF Task 3.3).
    ///
    /// When a room is deleted, made unlisted, or intentionally removed from
    /// discovery, the advertiser broadcasts this so directory clients can
    /// remove the matching advertisement immediately instead of waiting for
    /// the advertisement TTL. TTL expiry remains the safety net if the
    /// withdrawal is missed.
    ///
    /// Carries only the room's stable gossip topic — the advertisement
    /// being withdrawn — plus an Ed25519 signature by the withdrawing
    /// node's key so receivers can authenticate it (the same authoritative
    /// identity rule as [`Message::RoomAdvertisement`]: a withdrawal only
    /// ever removes the matching advertisement published by the signer).
    RoomWithdrawal {
        /// The room's gossip [`TopicId`] — the advertisement being
        /// withdrawn.
        topic: TopicId,
        /// Ed25519 signature over the canonical withdrawal framing by the
        /// withdrawing node's key, so receivers can verify authenticity.
        /// Signed with [`sign_room_withdrawal`] and checked with
        /// [`verify_room_withdrawal`].
        #[serde(default)]
        signature: Vec<u8>,
    },
    /// Announce a file that is available for direct download from the sender.
    ///
    /// This deliberately carries no local filesystem path. The sender's
    /// authenticated identity and this opaque offer ID authorize downloads.
    FileOffer {
        /// Opaque identifier used to correlate this offer with later updates.
        offer_id: FileOfferId,
        /// Safe basename displayed to the recipient (never a filesystem path).
        name: String,
        /// File size in bytes.
        size: u64,
    },
    /// Upgrade a previously announced file offer once blob ingestion completes.
    ///
    /// Receivers correlate this update with the original attachment by
    /// `offer_id`; the filename is intentionally not repeated here.
    FileOfferReady {
        /// Opaque identifier of the previously announced file offer.
        offer_id: FileOfferId,
        /// BlobTicket serialized to string for the completed blob ingest.
        ticket: String,
        /// Optional video thumbnail blob hash.
        thumbnail_hash: Option<MessageHash>,
    },
    /// A text message carrying stable peer-ID mention metadata.
    ///
    /// This variant is intentionally appended to preserve postcard variant
    /// discriminants used by older peers. Older peers can still decode every
    /// legacy variant and simply reject this new message as unknown.
    MessageWithMentions {
        /// The message text, including visible `@label` spellings.
        text: String,
        /// Structured mention ranges keyed by peer ID.
        mentions: Vec<crate::mentions::Mention>,
    },
}

/// Opaque identifier for a direct file offer.
///
/// IDs are generated from the operating system's cryptographically secure
/// random source and contain no information about the offered file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileOfferId([u8; 32]);

impl FileOfferId {
    /// Generate a fresh offer identifier using the operating system CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS entropy source failed");
        Self(bytes)
    }

    /// Return the raw identifier bytes for protocol implementations.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Message {
    /// Construct a file offer after validating that `name` is a basename.
    ///
    /// Both slash styles are rejected so a message created on one platform
    /// cannot carry a path component when decoded on another platform.
    pub fn file_offer(
        offer_id: FileOfferId,
        name: String,
        size: u64,
    ) -> std::result::Result<Self, &'static str> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err("file offer name must be a safe basename");
        }
        Ok(Self::FileOffer {
            offer_id,
            name,
            size,
        })
    }
}

/// Deserialize a `u64`, defaulting to `0` when the wire buffer is exhausted.
///
/// Postcard's `SeqAccess` returns `Err(EOF)` — not `Ok(None)` — when a
/// struct-variant's declared field count exceeds the bytes on the wire, so
/// `#[serde(default)]` alone cannot backfill a legacy `Message::FileShare`
/// envelope that predates the `size` field.  Treating an end-of-buffer as
/// `size = 0` keeps old peers' file shares decodable.
fn deserialize_tolerant_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match u64::deserialize(deserializer) {
        Ok(value) => Ok(value),
        Err(_) => Ok(0),
    }
}

/// Deserialize an `Option<MessageHash>`, defaulting to `None` when the wire
/// buffer is exhausted (legacy `Message::FileShare` envelopes without the
/// trailing thumbnail field).
fn deserialize_tolerant_opt_hash<'de, D>(deserializer: D) -> Result<Option<MessageHash>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<MessageHash>::deserialize(deserializer) {
        Ok(value) => Ok(value),
        Err(_) => Ok(None),
    }
}

/// A room advertisement broadcast into the directory topic.
///
/// Peers can use this to discover public rooms without needing an
/// out-of-band invitation.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize)]
pub struct RoomAdvertisement {
    /// Human-readable room name.
    pub room_name: String,
    /// Short description (max 200 chars).
    pub description: String,
    /// The room's gossip topic.
    pub topic: TopicId,
    /// Serialized room ticket for joining.
    pub ticket: String,
    /// Approximate member count.
    pub member_count: u32,
    /// Unix ms timestamp of last activity.
    pub last_activity: u64,
    /// Advertisement TTL in seconds (BORU-DIR-08, PDF Task 3.2) — the
    /// expiry/refresh mechanism. Directory clients consider the
    /// advertisement stale `expires_after_secs` after receipt and evict it
    /// unless the advertiser refreshes first. The publisher refreshes well
    /// before expiry (refresh interval 60 s vs. TTL 300 s). Legacy
    /// advertisements that predate this field decode with
    /// [`DEFAULT_ADVERT_TTL_SECS`].
    pub expires_after_secs: u32,
}

/// Manual [`Deserialize`] for [`RoomAdvertisement`] so legacy advertisements
/// (no trailing `expires_after_secs` field) still decode (BORU-DIR-08).
///
/// The TTL field is appended at the END of the struct.  Postcard's sequence
/// access returns `Err(EOF)` — not `Ok(None)` — when the buffer is exhausted
/// before the declared field count, so `#[serde(default)]` alone cannot
/// backfill the missing field.  We read the six original fields, then treat
/// an end-of-buffer on the seventh as [`DEFAULT_ADVERT_TTL_SECS`] (same
/// pattern as `SignedMessage.compression` and the `FileShare` thumbnail
/// field).
impl<'de> Deserialize<'de> for RoomAdvertisement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RoomAdvertisementVisitor;

        impl<'de> serde::de::Visitor<'de> for RoomAdvertisementVisitor {
            type Value = RoomAdvertisement;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a room advertisement")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let room_name = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `room_name`"))?;
                let description = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `description`"))?;
                let topic = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `topic`"))?;
                let ticket = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `ticket`"))?;
                let member_count = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `member_count`"))?;
                let last_activity = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `last_activity`"))?;
                // Trailing TTL field (BORU-DIR-08). Legacy advertisements end
                // after `last_activity` — treat a missing/EOF field as the
                // protocol default so old advertisers keep working.
                let expires_after_secs = match seq.next_element() {
                    Ok(Some(v)) => v,
                    Ok(None) | Err(_) => DEFAULT_ADVERT_TTL_SECS,
                };
                Ok(RoomAdvertisement {
                    room_name,
                    description,
                    topic,
                    ticket,
                    member_count,
                    last_activity,
                    expires_after_secs,
                })
            }
        }

        deserializer.deserialize_struct(
            "RoomAdvertisement",
            &[
                "room_name",
                "description",
                "topic",
                "ticket",
                "member_count",
                "last_activity",
                "expires_after_secs",
            ],
            RoomAdvertisementVisitor,
        )
    }
}

/// Metadata for a file advertised by a peer in [`Message::ProfileUpdate`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedFileMeta {
    /// Stable identifier assigned by the publisher.
    pub id: String,
    /// Basename shown to peers (never a local path).
    pub filename: String,
    /// File size in bytes.
    pub size: u64,
    /// MIME type, if known.
    pub mime_type: String,
    /// Unix timestamp in milliseconds of the source file's modification time.
    pub modified_time: u64,
    /// Content hash used to identify the file.
    pub hash: MessageHash,
}

/// Content hash used by richer interaction messages to refer to a chat message.
pub type Hash = [u8; 32];

/// Descriptive alias for message reference hashes.
pub type MessageHash = Hash;

/// Stable address of a signed chat message.
pub type MessageId = [u8; 32];

/// Calculate the stable content hash for a protocol message.
pub fn message_hash(message: &Message) -> MessageHash {
    let bytes = postcard::to_stdvec(message).expect("postcard::to_stdvec is infallible");
    *blake3::hash(&bytes).as_bytes()
}

/// Derive the local address for a received signed envelope.
pub fn signed_message_id(signed_bytes: &[u8]) -> MessageId {
    *blake3::hash(signed_bytes).as_bytes()
}

impl Message {
    /// Construct a reply without embedding the parent body.
    pub fn reply(text: impl Into<String>, reply_to_message_id: MessageId) -> Self {
        Self::Reply {
            text: text.into(),
            reply_to_message_id,
        }
    }

    /// Return the referenced parent, if this message is a reply.
    pub fn reply_to_message_id(&self) -> Option<MessageId> {
        match self {
            Self::Reply {
                reply_to_message_id,
                ..
            } => Some(*reply_to_message_id),
            _ => None,
        }
    }
}

/// Canonical protocol tag for signed room advertisements (BORU-AUDIT-27).
pub const ROOM_ADVERTISEMENT_PROTOCOL: &str = "boru/room-advertisement";

/// Version of the signed room-advertisement payload layout (BORU-AUDIT-27).
pub const ROOM_ADVERTISEMENT_VERSION: u16 = 1;

/// Sign a [`RoomAdvertisement`] with the room creator's secret key.
///
/// Returns the Ed25519 signature bytes that [`verify_advertisement`] can check.
/// The signature covers the canonical framing
/// (`boru/room-advertisement` / 1 / all advertisement fields), so identity,
/// routing/topic and freshness fields are authenticated.
pub fn sign_advertisement(ad: &RoomAdvertisement, sk: &SecretKey) -> Vec<u8> {
    let canonical = crate::protocol_signing::canonical_signed_bytes(
        ROOM_ADVERTISEMENT_PROTOCOL,
        ROOM_ADVERTISEMENT_VERSION,
        ad,
    )
    .expect("postcard advertisement encoding cannot fail");
    sk.sign(&canonical).to_bytes().to_vec()
}

/// Verify an Ed25519 signature over a [`RoomAdvertisement`].
///
/// Returns `true` if the signature is valid for the given author's public key.
/// Pre-AUDIT-27 signatures over the bare advertisement bytes still verify
/// during the migration window.
pub fn verify_advertisement(ad: &RoomAdvertisement, signature: &[u8], author: PublicKey) -> bool {
    let Ok(canonical) = crate::protocol_signing::canonical_signed_bytes(
        ROOM_ADVERTISEMENT_PROTOCOL,
        ROOM_ADVERTISEMENT_VERSION,
        ad,
    ) else {
        return false;
    };
    let legacy = match postcard::to_stdvec(ad) {
        Ok(b) => b,
        Err(_) => return false,
    };
    crate::protocol_signing::verify_canonical_or_legacy(&author, signature, &canonical, &legacy)
}

/// Canonical protocol tag for signed room withdrawals (BORU-DIR-09,
/// PDF Task 3.3).
///
/// A signature over this tag can never verify as a signature over any
/// other Boru protocol object family (room advertisements, chat messages,
/// mailbox acks, ...).
pub const ROOM_WITHDRAWAL_PROTOCOL: &str = "boru/room-withdrawal";

/// Version of the signed room-withdrawal payload layout (BORU-DIR-09).
pub const ROOM_WITHDRAWAL_VERSION: u16 = 1;

/// Sign a room withdrawal with the withdrawing node's secret key.
///
/// Returns the Ed25519 signature bytes that [`verify_room_withdrawal`] can
/// check. The signature covers the canonical framing
/// (`boru/room-withdrawal` / 1 / `topic`), so the withdrawn room identity
/// is authenticated — a receiver can prove which node withdrew the room.
pub fn sign_room_withdrawal(topic: &TopicId, sk: &SecretKey) -> Vec<u8> {
    let canonical = crate::protocol_signing::canonical_signed_bytes(
        ROOM_WITHDRAWAL_PROTOCOL,
        ROOM_WITHDRAWAL_VERSION,
        topic,
    )
    .expect("postcard withdrawal encoding cannot fail");
    sk.sign(&canonical).to_bytes().to_vec()
}

/// Verify an Ed25519 signature over a room withdrawal.
///
/// Returns `true` only when `author`'s public key verifies the signature
/// over the canonical withdrawal framing for `topic`. A missing or
/// malformed signature (wrong length) simply fails verification — never a
/// panic. The caller decides what the verified author may remove: the
/// withdrawal applies only to the advertisement **that author** published
/// for the room, so a spoofed withdrawal cannot remove unrelated rooms.
pub fn verify_room_withdrawal(topic: &TopicId, signature: &[u8], author: PublicKey) -> bool {
    let Ok(canonical) = crate::protocol_signing::canonical_signed_bytes(
        ROOM_WITHDRAWAL_PROTOCOL,
        ROOM_WITHDRAWAL_VERSION,
        topic,
    ) else {
        return false;
    };
    crate::protocol_signing::verify(&author, signature, &canonical)
}

const SIGNATURE_LENGTH: usize = iroh::Signature::LENGTH;
pub(crate) type Signature = ByteArray<SIGNATURE_LENGTH>;

/// Canonical protocol tag for signed chat messages (BORU-AUDIT-27).
///
/// The signature over [`SignedMessage`] covers
/// `canonical_signed_bytes("boru/chat-message", 1, (from, sent_at,
/// compression, data))`, so identity, freshness and interpretation fields
/// are all authenticated.  Pre-AUDIT-27 envelopes (signature over `data`
/// alone) still verify during the migration window via
/// [`crate::protocol_signing::verify_canonical_or_legacy`].
pub const SIGNED_MESSAGE_PROTOCOL: &str = "boru/chat-message";

/// Version of the signed chat-message payload layout (BORU-AUDIT-27).
pub const SIGNED_MESSAGE_VERSION: u16 = 1;

/// A signed message envelope with sender identity and signature.
#[derive(Debug, Serialize)]
pub struct SignedMessage {
    pub(crate) from: PublicKey,
    pub(crate) data: Bytes,
    pub(crate) signature: Signature,
    /// Unix epoch seconds when the message was sent.
    pub(crate) sent_at: u64,
    /// Compression applied to `data`: `0` = none (legacy), `1` = deflate
    /// with the shared dictionary.  Envelopes produced before this field
    /// existed omit the byte entirely and decode as `0`.
    pub(crate) compression: u8,
    /// Sender-generated immutable identity of this message.
    pub(crate) message_id: ByteArray<32>,
}

/// Manual [`Deserialize`] so legacy envelopes (no trailing `compression`
/// byte) still decode.
///
/// Postcard's sequence access returns `Err(EOF)` — not `Ok(None)` — when the
/// buffer is exhausted before the declared field count, so `#[serde(default)]`
/// alone cannot backfill the missing byte.  We read the four original fields,
/// then treat an end-of-buffer on the fifth as `compression = 0`.
impl<'de> Deserialize<'de> for SignedMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SignedMessageVisitor;

        impl<'de> serde::de::Visitor<'de> for SignedMessageVisitor {
            type Value = SignedMessage;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a signed message envelope")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let from = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `from`"))?;
                let data = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `data`"))?;
                let signature = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `signature`"))?;
                let sent_at = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("missing `sent_at`"))?;
                // `compression` is a single byte, so the only way its read can
                // fail is an empty buffer — i.e. a legacy 4-field envelope.
                let compression = match seq.next_element() {
                    Ok(Some(c)) => c,
                    Ok(None) | Err(_) => 0,
                };
                let message_id = match seq.next_element::<ByteArray<32>>() {
                    Ok(Some(id)) => id,
                    Ok(None) | Err(_) => ByteArray::new([0; 32]),
                };
                Ok(SignedMessage {
                    from,
                    data,
                    signature,
                    sent_at,
                    compression,
                    message_id,
                })
            }
        }

        deserializer.deserialize_struct(
            "SignedMessage",
            &["from", "data", "signature", "sent_at", "compression", "message_id"],
            SignedMessageVisitor,
        )
    }
}

impl SignedMessage {
    /// Return the stable address of this signed envelope.
    pub fn message_id(&self) -> MessageId {
        let embedded = *self.message_id.as_ref();
        if embedded != [0; 32] {
            embedded
        } else {
            let bytes = postcard::to_stdvec(self).expect("signed message encoding is infallible");
            signed_message_id(&bytes)
        }
    }

    /// Verify a signed message and decode the inner [`Message`].
    pub fn verify_and_decode(bytes: &[u8]) -> Result<(PublicKey, Message, u64)> {
        let signed_message: Self =
            postcard::from_bytes(bytes).std_context("decode signed message")?;
        let key: PublicKey = signed_message.from;
        // BORU-AUDIT-27: the canonical framing authenticates identity,
        // freshness (`sent_at`) and interpretation (`compression`) as well
        // as the payload.  Pre-AUDIT-27 envelopes signed only over `data`
        // still verify during the migration window.
        let canonical = crate::protocol_signing::canonical_signed_bytes(
            SIGNED_MESSAGE_PROTOCOL,
            SIGNED_MESSAGE_VERSION,
            &(
                signed_message.from,
                signed_message.sent_at,
                signed_message.compression,
                signed_message.message_id,
                &signed_message.data,
            ),
        )
        .std_context("canonical signed message bytes")?;
        let legacy_canonical = crate::protocol_signing::canonical_signed_bytes(
            SIGNED_MESSAGE_PROTOCOL,
            SIGNED_MESSAGE_VERSION,
            &(
                signed_message.from,
                signed_message.sent_at,
                signed_message.compression,
                &signed_message.data,
            ),
        )
        .std_context("legacy canonical signed message bytes")?;
        if !crate::protocol_signing::verify_canonical_or_legacy(
            &key,
            signed_message.signature.as_ref(),
            &canonical,
            &signed_message.data,
        ) && !crate::protocol_signing::verify(
            &key,
            signed_message.signature.as_ref(),
            &legacy_canonical,
        ) {
            bail_any!("verify signature");
        }
        // The canonical framing covers the `compression` byte, so tampering
        // with either the compressed payload or the `compression` byte is
        // caught by the signature itself.  We still inflate after verifying.
        let raw = match signed_message.compression {
            0 => signed_message.data.to_vec(),
            1 => crate::wire_compression::decompress(&signed_message.data)?,
            other => bail_any!("unsupported compression value {other}"),
        };
        let message: Message = postcard::from_bytes(&raw).std_context("decode message")?;
        Ok((signed_message.from, message, signed_message.sent_at))
    }

    /// Verify and decode a message while also returning its stable address.
    pub fn verify_and_decode_with_id(
        bytes: &[u8],
    ) -> Result<(PublicKey, Message, u64, MessageId)> {
        let signed_message: Self = postcard::from_bytes(bytes).std_context("decode signed message")?;
        let (from, message, sent_at) = Self::verify_and_decode(bytes)?;
        let embedded = *signed_message.message_id.as_ref();
        let id = if embedded == [0; 32] {
            signed_message_id(bytes)
        } else {
            embedded
        };
        Ok((from, message, sent_at, id))
    }

    /// Sign a [`Message`] and encode it into a `Bytes` payload ready for gossip broadcast.
    ///
    /// Uses the legacy uncompressed wire format (`compression = 0`), which
    /// every peer can decode.  Use [`Self::sign_and_encode_compressed`] to
    /// enable deflate compression with the shared dictionary.
    pub fn sign_and_encode(secret_key: &SecretKey, message: &Message) -> Result<Bytes> {
        Self::encode(secret_key, message, false)
    }

    /// Sign a [`Message`] and encode it with deflate compression
    /// (`compression = 1`) using the shared dictionary from
    /// [`crate::wire_compression`].
    ///
    /// The signature covers the *compressed* `data` bytes.  Receivers that
    /// do not know compression value `1` reject the message with a clear
    /// error rather than mis-decoding it.
    pub fn sign_and_encode_compressed(secret_key: &SecretKey, message: &Message) -> Result<Bytes> {
        Self::encode(secret_key, message, true)
    }

    fn encode(secret_key: &SecretKey, message: &Message, compress: bool) -> Result<Bytes> {
        let raw = postcard::to_stdvec(&message).std_context("encode message")?;
        let (data, compression): (Bytes, u8) = if compress {
            let compressed = crate::wire_compression::compress(&raw);
            if compressed.len() < raw.len() {
                // Deflate actually shrank the payload — store it compressed.
                (compressed.into(), 1u8)
            } else {
                // Deflate framing exceeds tiny payloads (unit variants like
                // Presence/Heartbeat/Leave are 1-6 bytes).  Fall back to raw
                // postcard so compression never makes the wire message
                // larger — every peer can still decode it (compression = 0).
                (raw.into(), 0u8)
            }
        } else {
            (raw.into(), 0u8)
        };
        let key: PublicKey = secret_key.public();
        let sent_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // BORU-AUDIT-27: sign the canonical framing — identity, freshness and
        // interpretation fields are all authenticated.  The signature covers
        // exactly what verify_and_decode recomputes.
        let mut message_id = [0u8; 32];
        getrandom::fill(&mut message_id).std_context("generate message id")?;
        let canonical = crate::protocol_signing::canonical_signed_bytes(
            SIGNED_MESSAGE_PROTOCOL,
            SIGNED_MESSAGE_VERSION,
            &(key, sent_at, compression, ByteArray::new(message_id), &data),
        )
        .std_context("canonical signed message bytes")?;
        let signature = secret_key.sign(&canonical);
        let signed_message = Self {
            from: key,
            data,
            signature: ByteArray::new(signature.to_bytes()),
            sent_at,
            compression,
            message_id: ByteArray::new(message_id),
        };
        let encoded = postcard::to_stdvec(&signed_message).std_context("encode signed message")?;
        Ok(encoded.into())
    }
}

/// A chat-room ticket that peers use to join a topic.
///
/// The optional [`DiscoverySecret`] enables DHT-based private-room discovery:
/// when present, the holder can publish and look up discovery records under
/// the room's encrypted namespace. Legacy tickets without a secret use their
/// endpoint-bearing bootstrap peers only; no replacement secret is generated.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// The gossip topic to join.
    pub topic: TopicId,
    /// Known peers to bootstrap from.
    pub peers: Vec<EndpointAddr>,
    /// Optional bearer capability for DHT-based private-room lookup.
    ///
    /// Holders can derive the room's private discovery namespace, so this
    /// value must be protected like the ticket itself. It is not message
    /// encryption and does not authenticate room membership. `#[serde(default)]`
    /// ensures legacy tickets (serialised without this field) deserialise to
    /// `None` instead of failing.
    #[serde(default)]
    pub discovery_secret: Option<DiscoverySecret>,
}

impl fmt::Debug for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ticket")
            .field("topic", &self.topic)
            .field("peers", &self.peers)
            .field(
                "discovery_secret",
                &self.discovery_secret.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl Ticket {
    /// Create a ticket from a topic, bootstrap peers, and an optional
    /// discovery secret.
    pub fn new(topic: TopicId, peers: Vec<EndpointAddr>) -> Self {
        Self {
            topic,
            peers,
            discovery_secret: None,
        }
    }

    /// Create a ticket with a discovery secret for DHT-based private-room
    /// discovery.
    pub fn with_discovery(
        topic: TopicId,
        peers: Vec<EndpointAddr>,
        secret: DiscoverySecret,
    ) -> Self {
        Self {
            topic,
            peers,
            discovery_secret: Some(secret),
        }
    }

    /// Decode a ticket from serialized bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).std_context("decode chat ticket")
    }

    /// Encode this ticket into bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("postcard::to_stdvec is infallible")
    }
}

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = data_encoding::BASE32_NOPAD.encode(&self.to_bytes()[..]);
        text.make_ascii_lowercase();
        write!(f, "{text}")
    }
}

impl FromStr for Ticket {
    type Err = n0_error::AnyError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .std_context("decode chat ticket base32")?;
        Self::from_bytes(&bytes)
    }
}

/// The invitation formats accepted by the room join path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomInvitation {
    /// Stable, endpoint-free invitation carrying the shared discovery secret.
    Stable(RoomInviteV2),
    /// Legacy postcard ticket. Bootstrap peers are preserved. A missing
    /// discovery secret explicitly disables DHT discovery for this room.
    Legacy(Ticket),
}

impl RoomInvitation {
    /// Detect and decode an invitation. A malformed `boru1:` string never
    /// falls through into the legacy decoder.
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.starts_with(RoomInviteV2::PREFIX) {
            return Ok(Self::Stable(RoomInviteV2::parse(trimmed)?));
        }
        Ok(Self::Legacy(trimmed.parse()?))
    }

    /// Return the room topic represented by this invitation.
    pub fn topic(&self) -> TopicId {
        match self {
            Self::Stable(invite) => invite.topic,
            Self::Legacy(ticket) => ticket.topic,
        }
    }

    /// Return the endpoint-bearing bootstrap peers, if any.
    pub fn bootstrap_peers(&self) -> &[EndpointAddr] {
        match self {
            Self::Stable(_) => &[],
            Self::Legacy(ticket) => &ticket.peers,
        }
    }

    /// Return the shared DHT discovery capability, if present.
    pub fn discovery_secret(&self) -> Option<&DiscoverySecret> {
        match self {
            Self::Stable(invite) => Some(&invite.discovery_secret),
            Self::Legacy(ticket) => ticket.discovery_secret.as_ref(),
        }
    }
}

// ── RoomInviteV2 — stable, versioned, compact invitation ────────────────────

/// A stable versioned room invitation with no endpoint/relay/creator identity.
///
/// Unlike the legacy [`Ticket`], this format carries only the room identity
/// (topic) and a bearer discovery secret — no transport address information.
/// It produces shorter, copy-paste-safe strings prefixed with `boru1:`.
///
/// # Format
///
/// `boru1:` + base32-nopad-lowercase of `[version: u8, topic: [u8; 32], discovery_secret: [u8; 32]]`
///
/// Total payload: 1 + 32 + 32 = 65 bytes.  Encoded string: ~105 chars + `boru1:` prefix.
///
/// # Safety
///
/// The discovery secret is redacted in the [`Debug`] implementation so it
/// never appears in logs or terminal output.
#[derive(Clone, PartialEq, Eq)]
pub struct RoomInviteV2 {
    /// The gossip topic to join (32 bytes).
    pub topic: TopicId,
    /// Bearer capability for DHT-based private-room discovery.
    /// Redacted in Debug output.
    pub discovery_secret: DiscoverySecret,
}

impl fmt::Debug for RoomInviteV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomInviteV2")
            .field("topic", &self.topic)
            .field("discovery_secret", &"[redacted]")
            .finish()
    }
}

impl RoomInviteV2 {
    /// Current wire version for serialisation.
    /// Increment this when the payload layout changes (e.g. adding a field).
    const VERSION: u8 = 1;

    /// The human-readable prefix that identifies this invitation format.
    const PREFIX: &'static str = "boru1:";

    /// Create a new invitation from a topic and discovery secret.
    pub fn new(topic: TopicId, discovery_secret: DiscoverySecret) -> Self {
        Self {
            topic,
            discovery_secret,
        }
    }

    /// Serialise this invitation into a compact string with the `boru1:` prefix.
    ///
    /// Payload layout: `[version: u8, topic: [u8; 32], discovery_secret: [u8; 32]]`
    pub fn encode(&self) -> String {
        let mut payload = Vec::with_capacity(65);
        payload.push(Self::VERSION);
        payload.extend_from_slice(self.topic.as_ref());
        payload.extend_from_slice(self.discovery_secret.as_bytes());
        let encoded = data_encoding::BASE32_NOPAD.encode(&payload);
        let mut lower = encoded.to_ascii_lowercase();
        lower.insert_str(0, Self::PREFIX);
        lower
    }

    /// Parse an invitation string, accepting only the `boru1:` prefix.
    ///
    /// Returns an error for wrong prefix, version mismatch, or incorrect payload
    /// length.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if !s.starts_with(Self::PREFIX) {
            bail_any!(
                "invalid invitation: expected prefix '{}', got '{}'",
                Self::PREFIX,
                &s[..s.len().min(10)]
            );
        }
        let encoded = &s[Self::PREFIX.len()..];
        if encoded.len() < 104 {
            // 65 bytes → 104 base32-nopad chars
            bail_any!(
                "invalid invitation: payload too short ({} chars, need ≥104)",
                encoded.len()
            );
        }
        let bytes = data_encoding::BASE32_NOPAD
            .decode(encoded.to_ascii_uppercase().as_bytes())
            .std_context("decode invitation base32")?;
        if bytes.len() != 65 {
            bail_any!(
                "invalid invitation: expected 65 payload bytes, got {}",
                bytes.len()
            );
        }
        let version = bytes[0];
        if version != Self::VERSION {
            bail_any!(
                "unsupported invitation version {} (expected {})",
                version,
                Self::VERSION
            );
        }
        let topic_bytes: [u8; 32] = bytes[1..33]
            .try_into()
            .map_err(|_| n0_error::anyerr!("invitation topic is not 32 bytes"))?;
        let secret_bytes: [u8; 32] = bytes[33..65]
            .try_into()
            .map_err(|_| n0_error::anyerr!("invitation secret is not 32 bytes"))?;
        Ok(Self {
            topic: TopicId::from_bytes(topic_bytes),
            discovery_secret: DiscoverySecret::from_bytes(secret_bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_offer_id_is_random_and_serializable() {
        let first = FileOfferId::generate();
        let second = FileOfferId::generate();
        assert_ne!(first, second);

        let encoded = postcard::to_stdvec(&first).expect("serialize offer ID");
        let decoded: FileOfferId = postcard::from_bytes(&encoded).expect("deserialize offer ID");
        assert_eq!(first, decoded);
        assert_eq!(first.as_bytes().len(), 32);
    }

    #[test]
    fn file_offer_round_trips_without_a_path() {
        let offer_id = FileOfferId::generate();
        let message =
            Message::file_offer(offer_id, "report.pdf".to_owned(), 42).expect("safe basename");
        let encoded = postcard::to_stdvec(&message).expect("serialize file offer");
        let decoded: Message = postcard::from_bytes(&encoded).expect("deserialize file offer");
        assert!(matches!(
            decoded,
            Message::FileOffer {
                offer_id: decoded_id,
                name,
                size: 42,
            } if decoded_id == offer_id && name == "report.pdf"
        ));
    }

    #[test]
    fn mentions_are_append_only_and_legacy_messages_still_decode() {
        let legacy = Message::Message {
            text: "hello".into(),
        };
        let encoded = postcard::to_stdvec(&legacy).expect("serialize legacy message");
        assert!(matches!(
            postcard::from_bytes::<Message>(&encoded).expect("decode legacy message"),
            Message::Message { text } if text == "hello"
        ));

        let with_mentions = Message::MessageWithMentions {
            text: "@alice hello".into(),
            mentions: vec![crate::mentions::Mention::new([7; 32], "alice", 0, 6)],
        };
        let encoded = postcard::to_stdvec(&with_mentions).expect("serialize mention message");
        assert!(matches!(
            postcard::from_bytes::<Message>(&encoded).expect("decode mention message"),
            Message::MessageWithMentions { mentions, .. } if mentions.len() == 1
        ));
    }

    #[test]
    fn file_offer_rejects_path_components() {
        let offer_id = FileOfferId::generate();
        for name in ["", ".", "..", "subdir/report.pdf", r"subdir\report.pdf"] {
            assert!(Message::file_offer(offer_id, name.to_owned(), 1).is_err());
        }
        assert!(Message::file_offer(offer_id, "report.pdf".to_owned(), 1).is_ok());
    }

    #[test]
    fn file_offer_ready_round_trips() {
        let offer_id = FileOfferId::generate();
        let message = Message::FileOfferReady {
            offer_id,
            ticket: "blob:iroh:ticket".to_owned(),
            thumbnail_hash: Some([0xabu8; 32]),
        };
        let encoded = postcard::to_stdvec(&message).expect("serialize ready offer");
        let decoded: Message = postcard::from_bytes(&encoded).expect("deserialize ready offer");
        assert!(matches!(
            decoded,
            Message::FileOfferReady {
                offer_id: decoded_id,
                ticket,
                thumbnail_hash: Some(hash),
            } if decoded_id == offer_id && ticket == "blob:iroh:ticket" && hash == [0xabu8; 32]
        ));
    }

    /// Required Test 19: poster metadata is a post-announcement upgrade.
    ///
    /// The sender emits the offer and the first downloadable ticket before
    /// any poster result exists.  Poster generation can therefore complete
    /// later and publish an idempotent `FileOfferReady` upgrade keyed by the
    /// same offer ID without delaying `FileOffer`.
    #[test]
    fn required_test_19_video_poster_does_not_delay_file_offer() {
        let offer_id = FileOfferId::generate();
        let initial =
            Message::file_offer(offer_id, "clip.mp4".to_owned(), 123).expect("safe video basename");
        let ready = Message::FileOfferReady {
            offer_id,
            ticket: "ticket-for-video".to_owned(),
            thumbnail_hash: None,
        };
        let poster_upgrade = Message::FileOfferReady {
            offer_id,
            ticket: "ticket-for-video".to_owned(),
            thumbnail_hash: Some([0xabu8; 32]),
        };

        let events = [initial, ready, poster_upgrade];
        assert!(matches!(events[0], Message::FileOffer { offer_id: id, .. } if id == offer_id));
        assert!(matches!(
            events[1],
            Message::FileOfferReady {
                offer_id: id,
                thumbnail_hash: None,
                ..
            } if id == offer_id
        ));
        assert!(matches!(
            events[2],
            Message::FileOfferReady {
                offer_id: id,
                thumbnail_hash: Some(_),
                ..
            } if id == offer_id
        ));
    }

    #[test]
    fn file_offer_ready_correlates_distinct_same_named_offers_by_id() {
        use std::collections::HashMap;

        let first_id = FileOfferId::generate();
        let second_id = FileOfferId::generate();
        assert_ne!(first_id, second_id);

        let mut attachments =
            HashMap::from([(first_id, "same-name.txt"), (second_id, "same-name.txt")]);
        let ready = Message::FileOfferReady {
            offer_id: second_id,
            ticket: "ticket-for-second".to_owned(),
            thumbnail_hash: None,
        };

        if let Message::FileOfferReady {
            offer_id, ticket, ..
        } = ready
        {
            assert_eq!(attachments.remove(&offer_id), Some("same-name.txt"));
            assert!(attachments.contains_key(&first_id));
            assert_eq!(ticket, "ticket-for-second");
        } else {
            panic!("expected FileOfferReady");
        }
    }
}
