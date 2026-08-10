//! Wire-format encrypted message types for group encryption.
//!
//! Defines the local [`ForwardSecureGroupMessage`](crate::group_encryption::message::ForwardSecureGroupMessage) trait, the
//! [`EncryptedGroupContent`](crate::group_encryption::message::EncryptedGroupContent) enum, and the [`EncryptedGroupEnvelope`](crate::group_encryption::message::EncryptedGroupEnvelope) wrapper
//! that is serialized via postcard and included in the gossip-signed
//! [`crate::Message::EncryptedGroupMessage`](crate::chat_core::Message::EncryptedGroupMessage) variant.
//!
//! # ForwardSecureGroupMessage trait
//!
//! This is a local trait (not re-exporting p2panda's trait directly) so that
//! we can avoid depending on a concrete `AckedGroupMembership` implementation
//! (the [`membership`](crate::group_encryption::membership) module) until it exists.  Direct
//! messages are carried as opaque serialised payloads (`Vec<u8>`); they are
//! deserialised at the outer processing layer.

use iroh_blobs::Hash as IrohHash;
use p2panda_encryption::message_scheme::{dcgka::DirectMessage, ControlMessage, Generation};
use p2panda_encryption::traits::{
    ForwardSecureGroupMessage as P2pandaForwardSecureGroupMessage, ForwardSecureMessageContent,
};
use serde::{Deserialize, Serialize};

use super::membership::Membership;
use super::types::{OpId, PeerId};

// ── EncryptedGroupContent ─────────────────────────────────────────────────

/// The content of a forward-secure group message, discriminating between
/// control messages (group management) and application messages (encrypted
/// payload).
///
/// This mirrors [`p2panda_encryption::traits::ForwardSecureMessageContent`]
/// but parameterised with our local [`PeerId`] and [`OpId`] types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EncryptedGroupContent {
    /// Group-management control message (create, add, remove, ack, etc.).
    Control(ControlMessage<PeerId, OpId>),

    /// Encrypted application payload with the ratchet generation used.
    Application {
        /// AEAD ciphertext of the application message.
        ciphertext: Vec<u8>,
        /// Ratchet generation used to encrypt this message.
        generation: Generation,
    },
}

// ── EncryptedGroupEnvelope ────────────────────────────────────────────────

/// A self-contained encrypted group message envelope.
///
/// Contains all information needed by a recipient to process a group
/// operation: the unique [`OpId`], the sender's [`PeerId`], the message
/// [`EncryptedGroupContent`], and any opaque serialised direct-message
/// payloads for individual peers.
///
/// The envelope is serialised via postcard and wrapped in a
/// [`SignedMessage`](crate::chat_core::SignedMessage) for gossip broadcast, exactly like
/// every other [`Message`](crate::chat_core::Message) variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedGroupEnvelope {
    /// Unique identifier (hash of the message).
    pub id: OpId,
    /// Sender's peer identity.
    pub sender: PeerId,
    /// Message content (control or application).
    pub content: EncryptedGroupContent,
    /// Opaque serialised direct-message payloads for key delivery.
    ///
    /// Each element is a postcard-serialised
    /// [`p2panda DirectMessage`](p2panda_encryption::message_scheme::DirectMessage)
    /// destined for a specific group member.  The receiving layer
    /// deserialises and dispatches these to the appropriate recipient's
    /// DCGKA processing function.
    #[serde(default)]
    pub direct_messages: Vec<Vec<u8>>,
}

// ── ForwardSecureGroupMessage trait ───────────────────────────────────────

/// Local trait for forward-secure encrypted group messages.
///
/// Provides access to the unique identifier, sender, content, and direct
/// messages of an encrypted group envelope.  This is a simplified local
/// trait rather than directly using
/// [`p2panda_encryption::traits::ForwardSecureGroupMessage`] to avoid
/// depending on a concrete `AckedGroupMembership` implementation before
/// the [`membership`](super::membership) module is built.
pub trait ForwardSecureGroupMessage {
    /// Unique identifier (hash) of this message.
    fn id(&self) -> OpId;

    /// Peer identity of the original sender.
    fn sender(&self) -> PeerId;

    /// Message content (control or application).
    fn content(&self) -> EncryptedGroupContent;

    /// Opaque serialised direct-message payloads for key delivery.
    fn direct_messages(&self) -> &[Vec<u8>];
}

impl ForwardSecureGroupMessage for EncryptedGroupEnvelope {
    fn id(&self) -> OpId {
        self.id
    }

    fn sender(&self) -> PeerId {
        self.sender
    }

    fn content(&self) -> EncryptedGroupContent {
        self.content.clone()
    }

    fn direct_messages(&self) -> &[Vec<u8>] {
        &self.direct_messages
    }
}

// ── p2panda-encryption ForwardSecureGroupMessage impl ─────────────────────
//
// Bridges our local EncryptedGroupEnvelope into p2panda-encryption's
// ForwardSecureGroupMessage trait so that MessageGroup can use it directly.

impl P2pandaForwardSecureGroupMessage<PeerId, OpId, Membership> for EncryptedGroupEnvelope {
    fn id(&self) -> OpId {
        self.id
    }

    fn sender(&self) -> PeerId {
        self.sender
    }

    fn content(&self) -> ForwardSecureMessageContent<PeerId, OpId> {
        match &self.content {
            EncryptedGroupContent::Control(ctrl) => {
                ForwardSecureMessageContent::Control(ctrl.clone())
            }
            EncryptedGroupContent::Application {
                ciphertext,
                generation,
            } => ForwardSecureMessageContent::Application {
                ciphertext: ciphertext.clone(),
                generation: *generation,
            },
        }
    }

    fn direct_messages(&self) -> Vec<DirectMessage<PeerId, OpId, Membership>> {
        self.direct_messages
            .iter()
            .filter_map(|bytes| postcard::from_bytes(bytes).ok())
            .collect()
    }
}

// ── Helper constructors ───────────────────────────────────────────────────

impl EncryptedGroupEnvelope {
    /// Create a new envelope carrying a control message.
    pub fn new_control(
        sender: PeerId,
        control: ControlMessage<PeerId, OpId>,
        direct_messages: Vec<Vec<u8>>,
    ) -> Self {
        let content = EncryptedGroupContent::Control(control);
        let bytes = postcard::to_allocvec(&content).unwrap_or_default();
        let hash = *blake3::hash(&bytes).as_bytes();
        let id = OpId::from(IrohHash::from_bytes(hash));
        Self {
            id,
            sender,
            content,
            direct_messages,
        }
    }

    /// Create a new envelope carrying an encrypted application message.
    pub fn new_application(
        sender: PeerId,
        ciphertext: Vec<u8>,
        generation: Generation,
        direct_messages: Vec<Vec<u8>>,
    ) -> Self {
        let content = EncryptedGroupContent::Application {
            ciphertext,
            generation,
        };
        let bytes = postcard::to_allocvec(&content).unwrap_or_default();
        let hash = *blake3::hash(&bytes).as_bytes();
        let id = OpId::from(IrohHash::from_bytes(hash));
        Self {
            id,
            sender,
            content,
            direct_messages,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_encryption::types::PeerId;

    // Helpers to disambiguate local trait from p2panda-encryption's trait.
    fn msg_id(msg: &EncryptedGroupEnvelope) -> OpId {
        ForwardSecureGroupMessage::id(msg)
    }
    fn msg_sender(msg: &EncryptedGroupEnvelope) -> PeerId {
        ForwardSecureGroupMessage::sender(msg)
    }
    fn msg_content(msg: &EncryptedGroupEnvelope) -> EncryptedGroupContent {
        ForwardSecureGroupMessage::content(msg)
    }
    fn msg_direct_messages(msg: &EncryptedGroupEnvelope) -> &[Vec<u8>] {
        ForwardSecureGroupMessage::direct_messages(msg)
    }

    /// Helper: generate a PeerId for testing.
    fn make_peer() -> PeerId {
        let sk = iroh::SecretKey::generate();
        PeerId::from(sk.public())
    }

    #[test]
    fn test_envelope_roundtrip_postcard() {
        let sender = make_peer();
        let envelope = EncryptedGroupEnvelope::new_control(
            sender,
            ControlMessage::Create {
                initial_members: vec![make_peer()],
            },
            vec![b"direct-msg-payload".to_vec()],
        );

        // Serialise and deserialise.
        let bytes = postcard::to_allocvec(&envelope).expect("serialize");
        let deserialized: EncryptedGroupEnvelope =
            postcard::from_bytes(&bytes).expect("deserialize");

        assert_eq!(envelope, deserialized, "round-trip equality");
    }

    #[test]
    fn test_trait_returns_correct_sender() {
        let sender = make_peer();
        let envelope =
            EncryptedGroupEnvelope::new_application(sender, vec![1, 2, 3, 4], 42, vec![]);

        let expected_sender = sender;
        assert_eq!(
            msg_sender(&envelope),
            expected_sender,
            "sender should match"
        );
    }

    #[test]
    fn test_trait_returns_application_content() {
        let sender = make_peer();
        let ciphertext = b"secret-data".to_vec();
        let generation: Generation = 7;
        let envelope =
            EncryptedGroupEnvelope::new_application(sender, ciphertext.clone(), generation, vec![]);

        let content = msg_content(&envelope);
        match content {
            EncryptedGroupContent::Application {
                ciphertext: ct,
                generation: gen,
            } => {
                assert_eq!(ct, b"secret-data", "ciphertext match");
                assert_eq!(gen, 7, "generation match");
            }
            other => panic!("expected Application content, got {other:?}"),
        }
    }

    #[test]
    fn test_trait_returns_control_content() {
        let sender = make_peer();
        let initial_members = vec![make_peer(), make_peer()];
        let envelope = EncryptedGroupEnvelope::new_control(
            sender,
            ControlMessage::Create {
                initial_members: initial_members.clone(),
            },
            vec![],
        );

        let content = msg_content(&envelope);
        match content {
            EncryptedGroupContent::Control(ControlMessage::Create {
                initial_members: members,
            }) => {
                assert_eq!(members.len(), 2, "two initial members");
                assert_eq!(members, initial_members);
            }
            other => panic!("expected Control::Create content, got {other:?}"),
        }
    }

    #[test]
    fn test_trait_returns_id() {
        let sender = make_peer();
        let envelope = EncryptedGroupEnvelope::new_control(sender, ControlMessage::Update, vec![]);

        let id = msg_id(&envelope);
        // The ID is the hash of the serialised content — verify it's non-zero.
        assert!(
            id.0.as_bytes().iter().any(|&b| b != 0),
            "id should not be all zeros"
        );

        // A different message should produce a different hash.
        let envelope2 = EncryptedGroupEnvelope::new_control(
            sender,
            ControlMessage::Remove {
                removed: make_peer(),
            },
            vec![],
        );
        assert_ne!(
            msg_id(&envelope),
            msg_id(&envelope2),
            "different messages => different IDs"
        );
    }

    #[test]
    fn test_trait_returns_direct_messages() {
        let sender = make_peer();
        let dms: Vec<Vec<u8>> = vec![b"msg-for-alice".to_vec(), b"msg-for-bob".to_vec()];

        let envelope =
            EncryptedGroupEnvelope::new_control(sender, ControlMessage::Update, dms.clone());

        assert_eq!(
            msg_direct_messages(&envelope),
            &dms[..],
            "direct messages should match"
        );
    }

    #[test]
    fn test_application_envelope_roundtrip() {
        let sender = make_peer();
        let envelope = EncryptedGroupEnvelope::new_application(
            sender,
            vec![0u8; 32],
            99,
            vec![b"key-delivery".to_vec()],
        );

        let bytes = postcard::to_allocvec(&envelope).expect("serialize");
        let deserialized: EncryptedGroupEnvelope =
            postcard::from_bytes(&bytes).expect("deserialize");

        assert_eq!(envelope, deserialized);
        assert_eq!(msg_content(&deserialized), msg_content(&envelope));
    }
}
