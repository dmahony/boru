#![allow(clippy::large_enum_variant)]

//! Signed contact and direct-conversation control messages.
//!
//! Control messages are transported over the authenticated whisper channel,
//! but are signed as well so they can be safely queued, replayed, or forwarded
//! by a frontend without trusting transport metadata.

use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

use crate::{mailbox::MailboxPublicKey, proto::TopicId};

const SIGNATURE_LENGTH: usize = Signature::LENGTH;
const MAX_CONTROL_CLOCK_SKEW_SECS: u64 = 24 * 60 * 60;

/// Canonical protocol tag for signed contact messages (BORU-AUDIT-27).
const CONTACT_MESSAGE_PROTOCOL: &str = "boru/contact";
/// Version of the signed contact-message layout (BORU-AUDIT-27).
const CONTACT_MESSAGE_VERSION: u16 = 1;

/// A signed control-plane operation between two peers.
///
/// Friend-request actions (`FriendRequest`, `FriendRequestAccepted`,
/// `FriendRequestRejected`) are distinct from conversation actions
/// (`ConversationInvite`).  A `ConversationInvite` is also the authenticated
/// signal emitted by the explicit Chat action; its recipient may auto-accept
/// and open the deterministic direct room without a separate friend request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContactAction {
    /// Ask a peer to become friends (replaces `ContactRequest`).
    FriendRequest {
        /// Optional display name proposed by the requester.
        name: Option<String>,
    },
    /// Accept a pending friend request (replaces `ContactAccept`).
    FriendRequestAccepted,
    /// Reject a pending friend request (new).
    FriendRequestRejected,
    /// Propose or confirm the stable direct conversation topic.
    /// Only used between peers who are **already friends**.
    ConversationInvite {
        /// Stable one-to-one topic.
        topic: TopicId,
        /// Addresses the receiver may use for gossip bootstrap.
        addrs: Vec<iroh::EndpointAddr>,
    },
    /// Refresh addresses used to bootstrap the direct conversation.
    AddressUpdate {
        /// Current endpoint addresses owned by the sender.
        addrs: Vec<iroh::EndpointAddr>,
    },
    /// Advertise the recipient-hosted encrypted mailbox key for offline DMs.
    ///
    /// This is carried inside the existing signed contact channel rather than
    /// inferred from transport metadata.
    MailboxAdvertise {
        /// Public identity and X25519 encryption key used by the mailbox.
        mailbox: MailboxPublicKey,
    },
    /// Offer a recipient-bound secure tunnel to a local service.
    ///
    /// The capability is signed by the sender and names only an existing
    /// tunnel — never an arbitrary destination.  The recipient verifies the
    /// capability before presenting it when connecting.
    TunnelOffer {
        /// Serialised tunnel offer payload.
        offer: crate::tunnel::TunnelOffer,
    },
}

/// Wire envelope for authenticated contact control messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedContactMessage {
    /// Identity of the signer.
    pub from: PublicKey,
    /// Monotonic-enough wall-clock timestamp used for replay bounds.
    pub sent_at_unix_secs: u64,
    /// Encoded [`ContactAction`].
    pub data: Vec<u8>,
    /// Signature over `sent_at_unix_secs || data`.
    pub signature: ByteArray<SIGNATURE_LENGTH>,
}

impl SignedContactMessage {
    /// Sign a control action for transport over whisper.
    pub fn sign(secret_key: &SecretKey, action: &ContactAction) -> n0_error::Result<Vec<u8>> {
        let data = postcard::to_stdvec(action)
            .map_err(|e| n0_error::anyerr!("encode contact action: {e}"))?;
        let sent_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let signing_data = signing_bytes(sent_at_unix_secs, &data);
        let signature = secret_key.sign(&signing_data);
        postcard::to_stdvec(&Self {
            from: secret_key.public(),
            sent_at_unix_secs,
            data,
            signature: ByteArray::new(signature.to_bytes()),
        })
        .map_err(|e| n0_error::anyerr!("encode signed contact message: {e}"))
    }

    /// Verify the signature and decode the requested action.
    pub fn verify(
        bytes: &[u8],
        expected_from: Option<PublicKey>,
    ) -> n0_error::Result<(PublicKey, ContactAction)> {
        let envelope: Self = postcard::from_bytes(bytes)
            .map_err(|e| n0_error::anyerr!("decode signed contact message: {e}"))?;
        if let Some(expected) = expected_from {
            if envelope.from != expected {
                return Err(n0_error::anyerr!(
                    "contact signer does not match transport peer"
                ));
            }
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if envelope.sent_at_unix_secs.abs_diff(now) > MAX_CONTROL_CLOCK_SKEW_SECS {
            return Err(n0_error::anyerr!(
                "contact message timestamp is outside replay window"
            ));
        }
        // BORU-AUDIT-27: canonical framing first, legacy `timestamp || data`
        // concatenation as a migration fallback.
        let canonical = signing_bytes(envelope.sent_at_unix_secs, &envelope.data);
        let legacy = legacy_signing_bytes(envelope.sent_at_unix_secs, &envelope.data);
        if !crate::protocol_signing::verify_canonical_or_legacy(
            &envelope.from,
            envelope.signature.as_ref(),
            &canonical,
            &legacy,
        ) {
            return Err(n0_error::anyerr!("verify contact signature"));
        }
        let action = postcard::from_bytes(&envelope.data)
            .map_err(|e| n0_error::anyerr!("decode contact action: {e}"))?;
        Ok((envelope.from, action))
    }
}

fn signing_bytes(timestamp: u64, data: &[u8]) -> Vec<u8> {
    crate::protocol_signing::canonical_signed_bytes(
        CONTACT_MESSAGE_PROTOCOL,
        CONTACT_MESSAGE_VERSION,
        &(timestamp, data),
    )
    .expect("postcard contact signing bytes cannot fail")
}

/// Legacy pre-AUDIT-27 signing bytes: `timestamp || data` raw concatenation.
fn legacy_signing_bytes(timestamp: u64, data: &[u8]) -> Vec<u8> {
    let mut bytes = timestamp.to_le_bytes().to_vec();
    bytes.extend_from_slice(data);
    bytes
}

/// Derive the stable one-to-one gossip topic shared by two public keys.
pub fn direct_topic(a: &PublicKey, b: &PublicKey) -> TopicId {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"iroh-gossip-chat/direct/v1");
    hasher.update(first.as_bytes());
    hasher.update(second.as_bytes());
    (*hasher.finalize().as_bytes()).into()
}

/// Validate that an address update belongs to the signed sender.
pub fn validate_addrs(sender: PublicKey, addrs: &[iroh::EndpointAddr]) -> bool {
    addrs.iter().all(|addr| addr.id == sender)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_contact_round_trip_and_identity_check() {
        let key = SecretKey::generate();
        let action = ContactAction::ConversationInvite {
            topic: direct_topic(&key.public(), &SecretKey::generate().public()),
            addrs: Vec::new(),
        };
        let encoded = SignedContactMessage::sign(&key, &action).unwrap();
        assert_eq!(
            SignedContactMessage::verify(&encoded, Some(key.public())).unwrap(),
            (key.public(), action)
        );
        assert!(
            SignedContactMessage::verify(&encoded, Some(SecretKey::generate().public())).is_err()
        );
    }

    #[test]
    fn direct_topic_is_order_independent() {
        let a = SecretKey::generate().public();
        let b = SecretKey::generate().public();
        assert_eq!(direct_topic(&a, &b), direct_topic(&b, &a));
    }

    // ── BORU-AUDIT-27: canonical contact-message framing ───────────────────

    /// The canonical bytes a contact message signs must be stable:
    /// `postcard(("boru/contact", 1, sent_at_unix_secs, data))`.
    #[test]
    fn contact_message_canonical_bytes_golden_vector() {
        let key = SecretKey::generate();
        let action = ContactAction::ConversationInvite {
            topic: direct_topic(&key.public(), &SecretKey::generate().public()),
            addrs: Vec::new(),
        };
        let data = postcard::to_stdvec(&action).unwrap();
        let canonical = signing_bytes(1_700_000_000, &data);
        assert_eq!(canonical[0] as usize, CONTACT_MESSAGE_PROTOCOL.len());
        assert_eq!(
            &canonical[1..1 + CONTACT_MESSAGE_PROTOCOL.len()],
            CONTACT_MESSAGE_PROTOCOL.as_bytes()
        );
        assert_eq!(canonical[1 + CONTACT_MESSAGE_PROTOCOL.len()], 0x01);
        let decoded: (String, u16, u64, Vec<u8>) =
            postcard::from_bytes(&canonical).expect("decode canonical contact bytes");
        assert_eq!(decoded.0, CONTACT_MESSAGE_PROTOCOL);
        assert_eq!(decoded.1, CONTACT_MESSAGE_VERSION);
        assert_eq!(decoded.2, 1_700_000_000);
        assert_eq!(decoded.3, data);
    }

    /// Cross-version: a pre-AUDIT-27 contact message signed over the raw
    /// `timestamp || data` concatenation still verifies during migration.
    #[test]
    fn contact_message_legacy_framing_still_verifies() {
        let key = SecretKey::generate();
        let action = ContactAction::ConversationInvite {
            topic: direct_topic(&key.public(), &SecretKey::generate().public()),
            addrs: Vec::new(),
        };
        let data = postcard::to_stdvec(&action).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Hand-build a legacy envelope (same wire shape, legacy signature).
        let legacy_sig = legacy_signing_bytes(now, &data);
        let legacy_sig = key.sign(&legacy_sig);
        let envelope = super::SignedContactMessage {
            from: key.public(),
            sent_at_unix_secs: now,
            data,
            signature: ByteArray::new(legacy_sig.to_bytes()),
        };
        let encoded = postcard::to_stdvec(&envelope).unwrap();
        let (from, decoded) = SignedContactMessage::verify(&encoded, Some(key.public())).unwrap();
        assert_eq!(from, key.public());
        assert_eq!(decoded, action);
    }
}
