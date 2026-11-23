//! Gossip-backed room facade and snapshot hydration helpers.
use std::collections::HashMap;

use boru_core::{api::GossipSender, desktop_runtime::DesktopRuntime, proto::TopicId, snapshot::{AppSnapshot, RoomSnapshot}};
use bytes::Bytes;

use crate::bridge::AppError;

/// Handle for sending messages to a joined conversation topic.
#[derive(Clone, Debug)]
pub struct RoomHandle {
    conversation_id: [u8; 32],
    sender: GossipSender,
}

impl RoomHandle {
    pub fn conversation_id(&self) -> [u8; 32] { self.conversation_id }
}

/// Owns joined gossip senders for desktop conversations.
pub struct RoomClient<'a> {
    runtime: &'a DesktopRuntime,
    senders: HashMap<[u8; 32], GossipSender>,
}

impl<'a> RoomClient<'a> {
    pub fn new(runtime: &'a DesktopRuntime) -> Self { Self { runtime, senders: HashMap::new() } }

    /// Join the conversation topic identified directly by its canonical 32-byte ID.
    pub async fn subscribe(&mut self, conversation_id: [u8; 32]) -> Result<RoomHandle, AppError> {
        if let Some(sender) = self.senders.get(&conversation_id) {
            return Ok(RoomHandle { conversation_id, sender: sender.clone() });
        }
        let topic = TopicId::from_bytes(conversation_id);
        let joined = self.runtime.gossip().subscribe(topic, Vec::new()).await
            .map_err(|_| AppError::Internal("room subscription failed".into()))?;
        let (sender, _receiver) = joined.split();
        self.senders.insert(conversation_id, sender.clone());
        Ok(RoomHandle { conversation_id, sender })
    }

    /// Broadcast an already-encoded protocol message to a conversation.
    pub async fn broadcast(&mut self, conversation_id: [u8; 32], encoded: Vec<u8>) -> Result<(), AppError> {
        let handle = self.subscribe(conversation_id).await?;
        handle.sender.broadcast(Bytes::from(encoded)).await
            .map_err(|_| AppError::Internal("room broadcast failed".into()))
    }

    pub fn room_summary(snapshot: &AppSnapshot, conversation_id: [u8; 32]) -> Option<RoomSnapshot> {
        let id = TopicId::from_bytes(conversation_id).to_string();
        snapshot.rooms.iter().find(|room| room.id == id).cloned()
    }

    pub fn list_room_summaries(snapshot: &AppSnapshot) -> Vec<RoomSnapshot> {
        snapshot.rooms.clone()
    }
}
