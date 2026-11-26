//! Pure decoding and validation of room invitation strings.
use crate::chat_core::protocol::{ManagedRoomInvitation, ManagedRoomInvitationV2, RoomInvitation};

/// Ticket format accepted by the room join path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomTicketKind {
    /// Recipient-bound managed invitation, version 2.
    ManagedV2,
    /// Owner-bound managed invitation, version 1.
    ManagedV1,
    /// Standard room invitation.
    Invitation,
}
/// Parsed ticket envelope, preserving all invitation metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodedInvitation {
    /// Managed invitation version 2.
    ManagedV2(ManagedRoomInvitationV2),
    /// Managed invitation version 1.
    ManagedV1(ManagedRoomInvitation),
    /// Standard room invitation.
    Room(RoomInvitation),
}

/// Decoded room ticket data, without performing discovery or joining.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedRoomTicket {
    /// The recognized ticket format.
    pub kind: RoomTicketKind,
    /// Gossip topic bytes.
    pub topic: [u8; 32],
    /// Parsed invitation envelope; its variant agrees with `kind`.
    pub invitation: DecodedInvitation,
}

impl DecodedInvitation {
    /// Return the managed-v2 envelope when this is a v2 invitation.
    pub fn as_managed_v2(&self) -> Option<&ManagedRoomInvitationV2> {
        if let Self::ManagedV2(invitation) = self {
            Some(invitation)
        } else {
            None
        }
    }

    /// Return the managed-v1 envelope when this is a v1 invitation.
    pub fn as_managed_v1(&self) -> Option<&ManagedRoomInvitation> {
        if let Self::ManagedV1(invitation) = self {
            Some(invitation)
        } else {
            None
        }
    }

    /// Return the standard envelope when this is a room invitation.
    pub fn as_room(&self) -> Option<&RoomInvitation> {
        if let Self::Room(invitation) = self {
            Some(invitation)
        } else {
            None
        }
    }
}
/// Errors encountered while decoding a room ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinTicketError {
    /// Input is empty.
    Empty,
    /// Invalid managed v2 envelope.
    InvalidManagedV2,
    /// Invalid managed v1 envelope.
    InvalidManaged,
    /// Invalid standard invitation.
    InvalidInvitation,
    /// Unsupported input.
    Unsupported,
}
/// Parse and validate a room ticket without network or stateful work.
pub fn decode_room_ticket(input: &str) -> Result<DecodedRoomTicket, JoinTicketError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(JoinTicketError::Empty);
    }
    if input.starts_with(ManagedRoomInvitationV2::PREFIX) {
        let invite =
            ManagedRoomInvitationV2::parse(input).map_err(|_| JoinTicketError::InvalidManagedV2)?;
        let topic = *invite.ticket.topic.as_bytes();
        return Ok(DecodedRoomTicket {
            kind: RoomTicketKind::ManagedV2,
            topic,
            invitation: DecodedInvitation::ManagedV2(invite),
        });
    }
    if input.starts_with(ManagedRoomInvitation::PREFIX) {
        let invite =
            ManagedRoomInvitation::parse(input).map_err(|_| JoinTicketError::InvalidManaged)?;
        let topic = *invite.ticket.topic.as_bytes();
        return Ok(DecodedRoomTicket {
            kind: RoomTicketKind::ManagedV1,
            topic,
            invitation: DecodedInvitation::ManagedV1(invite),
        });
    }
    let invitation = RoomInvitation::parse(input).map_err(|_| {
        if input.starts_with("boru") {
            JoinTicketError::InvalidInvitation
        } else {
            JoinTicketError::Unsupported
        }
    })?;
    let topic = *invitation.topic().as_bytes();
    Ok(DecodedRoomTicket {
        kind: RoomTicketKind::Invitation,
        topic,
        invitation: DecodedInvitation::Room(invitation),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_core::protocol::{RoomInviteV2, Ticket};
    use crate::discovery_secret::DiscoverySecret;
    use crate::TopicId;
    fn legacy_ticket(topic: TopicId) -> String {
        Ticket::new(topic, vec![]).to_string()
    }
    #[test]
    fn decodes_managed_v1_and_preserves_envelope() {
        let topic = TopicId::from_bytes([0x41; 32]);
        let owner = iroh::SecretKey::from_bytes(&[0x42; 32]);
        let invitation = ManagedRoomInvitation {
            version: 1,
            ticket: Ticket::new(topic, vec![]),
            bootstrap: crate::authorization::AuthorizationBootstrap::sign(&owner, topic).unwrap(),
        };
        let input = invitation.encode().unwrap();
        let decoded = decode_room_ticket(&input).unwrap();
        assert_eq!(decoded.kind, RoomTicketKind::ManagedV1);
        assert_eq!(decoded.topic, [0x41; 32]);
        assert_eq!(decoded.invitation.as_managed_v1(), Some(&invitation));
        assert!(decoded.invitation.as_managed_v2().is_none());
    }

    #[test]
    fn decodes_managed_v2_and_preserves_envelope() {
        let topic = TopicId::from_bytes([0x51; 32]);
        let owner = iroh::SecretKey::from_bytes(&[0x52; 32]);
        let recipient = iroh::SecretKey::from_bytes(&[0x53; 32]);
        let bootstrap = crate::authorization::AuthorizationBootstrap::sign(&owner, topic).unwrap();
        let admission = crate::authorization::AuthorizationEvent::sign(
            &owner,
            topic,
            1,
            recipient.public(),
            crate::authorization::AuthorizationAction::Admit {
                role: crate::authorization::Role::Member,
            },
        )
        .unwrap();
        let invitation = ManagedRoomInvitationV2 {
            version: 2,
            ticket: Ticket::new(topic, vec![]),
            bootstrap,
            recipient: recipient.public(),
            events: vec![admission],
        };
        let input = invitation.encode().unwrap();
        let decoded = decode_room_ticket(&input).unwrap();
        assert_eq!(decoded.kind, RoomTicketKind::ManagedV2);
        assert_eq!(decoded.topic, [0x51; 32]);
        assert_eq!(decoded.invitation.as_managed_v2(), Some(&invitation));
        assert!(decoded.invitation.as_managed_v1().is_none());
    }

    #[test]
    fn decodes_standard_invitation_and_topic() {
        let topic = TopicId::from_bytes([0x5a; 32]);
        let input = RoomInviteV2::new(topic, DiscoverySecret::from_bytes([7; 32])).encode();
        let decoded = decode_room_ticket(&input).unwrap();
        assert_eq!(decoded.kind, RoomTicketKind::Invitation);
        assert_eq!(decoded.topic, [0x5a; 32]);
        assert!(decoded.invitation.as_room().is_some());
    }
    #[test]
    fn decodes_legacy_ticket() {
        let input = legacy_ticket(TopicId::from_bytes([0x31; 32]));
        let decoded = decode_room_ticket(&input).unwrap();
        assert_eq!(decoded.kind, RoomTicketKind::Invitation);
        assert_eq!(decoded.topic, [0x31; 32]);
    }
    #[test]
    fn rejects_empty_whitespace_and_malformed_formats() {
        assert_eq!(decode_room_ticket(""), Err(JoinTicketError::Empty));
        assert_eq!(decode_room_ticket(" \n\t"), Err(JoinTicketError::Empty));
        assert_eq!(
            decode_room_ticket("boru-managed2:!!!"),
            Err(JoinTicketError::InvalidManagedV2)
        );
        assert_eq!(
            decode_room_ticket("boru-managed1:!!!"),
            Err(JoinTicketError::InvalidManaged)
        );
        assert_eq!(
            decode_room_ticket("boru1:!!!"),
            Err(JoinTicketError::InvalidInvitation)
        );
    }
    #[test]
    fn rejects_unsupported_input() {
        assert_eq!(
            decode_room_ticket("not-a-ticket"),
            Err(JoinTicketError::Unsupported)
        );
    }
}
