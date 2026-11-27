//! Production room lifecycle facade shared by desktop frontends.
use anyhow::{anyhow, Result};
use crate::{
    chat_core::protocol::{ManagedRoomInvitation, Ticket},
    desktop_runtime::DesktopRuntime,
    proto::TopicId,
    room_ticket::decode_room_ticket,
};

/// Room invitation text produced by the protocol's managed-ticket encoder.
pub type RoomInvitationTicket = String;

/// A successfully subscribed room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinedRoom { /// Gossip topic.
    pub topic: TopicId,
}

/// Create, persist and mint a managed room invitation.
pub async fn create_room(runtime: &DesktopRuntime) -> Result<(TopicId, RoomInvitationTicket)> {
    let topic = TopicId::from_bytes(rand::random());
    let owner = runtime.secret_key();
    let bootstrap = crate::authorization::AuthorizationBootstrap::sign(&owner, topic)
        .map_err(|e| anyhow!("sign room authorization root: {e}"))?;
    let state = crate::authorization::AuthorizationState::new(topic, owner.public());
    runtime.storage().initialize_room_authorization_root(&topic, &bootstrap, &state)?;
    runtime.storage().add_room_membership(&topic)?;
    let ticket = ManagedRoomInvitation { version: 1, ticket: Ticket::new(topic, vec![]), bootstrap }.encode()?;
    Ok((topic, ticket))
}

/// Decode a ticket and subscribe before returning, seeding its endpoint peers.
pub async fn join_by_ticket(runtime: &DesktopRuntime, ticket: &str, peers: Vec<iroh::EndpointId>) -> Result<JoinedRoom> {
    let decoded = decode_room_ticket(ticket).map_err(|e| anyhow!("invalid room ticket: {e:?}"))?;
    let topic = TopicId::from_bytes(decoded.topic);
    runtime.join_room_with_peers(decoded.topic, peers).await?;
    runtime.storage().add_room_membership(&topic)?;
    Ok(JoinedRoom { topic })
}

/// Leave the room and remove its local membership record.
pub async fn leave_room(runtime: &DesktopRuntime, topic: TopicId) -> Result<()> {
    runtime.room_sender_session().leave(topic).await;
    runtime.storage().remove_room_membership(&topic)?;
    Ok(())
}

/// Query the current public-room directory snapshot.
pub fn directory_query(directory: &crate::room_directory::RoomDirectory) -> Vec<crate::room_directory::DirectoryEntry> {
    directory.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_core::protocol::ManagedRoomInvitation;

    #[test]
    fn managed_ticket_is_decoded_by_join_path_decoder() {
        let secret = iroh::SecretKey::generate();
        let topic = TopicId::from_bytes([0x42; 32]);
        let bootstrap = crate::authorization::AuthorizationBootstrap::sign(&secret, topic).unwrap();
        let ticket = ManagedRoomInvitation { version: 1, ticket: Ticket::new(topic, vec![]), bootstrap }.encode().unwrap();
        let decoded = decode_room_ticket(&ticket).unwrap();
        assert_eq!(decoded.topic, [0x42; 32]);
        assert_eq!(decoded.kind, crate::room_ticket::RoomTicketKind::ManagedV1);
    }
}
