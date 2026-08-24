//! Correlated, body-free messaging adapter for the E2E test-control path.
//!
//! The adapter owns only correlation and observation state. A GUI/MCP caller
//! remains responsible for entering the marker through the normal composer and
//! for feeding delivery observations back into this store.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::e2e_control::{
    E2eError, E2eErrorCode, MessageDeliveryState, MessageMarker, MessageSnapshot, NodeAlias,
    RoomMarker, RunId, SchemaVersion,
};

const MARKER_PREFIX: &str = "E2E:";
const MAX_MARKER_LEN: usize = 128;

/// Generates opaque, ordered message markers for one harness run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageMarkerGenerator {
    run_id: RunId,
    next_sequence: u64,
}

impl MessageMarkerGenerator {
    /// Creates a generator whose first marker has sequence number one.
    pub fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            next_sequence: 1,
        }
    }

    /// Returns the next `E2E:<run_id>:<sequence>` marker.
    pub fn next_marker(&mut self) -> Result<(MessageMarker, u64), E2eError> {
        let sequence = self.next_sequence;
        let marker = format!("{MARKER_PREFIX}{}:{sequence}", self.run_id.as_ref());
        validate_marker(&marker)?;
        self.next_sequence = self.next_sequence.saturating_add(1);
        Ok((MessageMarker::from(marker), sequence))
    }

    /// Returns the run this generator belongs to.
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

/// A durable, body-free record used to correlate one message across reconnects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelatedMessage {
    /// Contract schema/version.
    pub schema: SchemaVersion,
    /// Harness run that created the marker.
    pub run_id: RunId,
    /// Room correlation marker.
    pub room_marker: RoomMarker,
    /// Opaque message correlation marker.
    pub message_marker: MessageMarker,
    /// Harness-assigned ordering sequence.
    pub sequence: u64,
    /// Safe sender alias.
    pub sender: NodeAlias,
    /// Safe receiver alias.
    pub receiver: NodeAlias,
    /// Latest monotonic delivery state.
    pub delivery_state: MessageDeliveryState,
    /// Number of repeated transport/presentation observations.
    pub duplicate_count: u32,
    /// Number of times the UI presented this marker.
    pub presentation_count: u32,
    /// States observed in order, including the initial queued state.
    pub state_history: Vec<MessageDeliveryState>,
}

impl CorrelatedMessage {
    fn snapshot(&self) -> MessageSnapshot {
        MessageSnapshot {
            schema: self.schema.clone(),
            room_marker: self.room_marker.clone(),
            message_marker: self.message_marker.clone(),
            present: true,
            delivery_state: Some(self.delivery_state),
            duplicate_count: self.duplicate_count,
            sequence: Some(self.sequence),
            sender: Some(self.sender.clone()),
            receiver: Some(self.receiver.clone()),
        }
    }
}

/// In-memory projection of correlated messaging state.
///
/// Serialize this value with `serde_json` (or the harness' durable store) to
/// preserve correlation over a process restart. It intentionally contains no
/// message body or synthetic payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageCorrelationStore {
    messages: HashMap<MessageMarker, CorrelatedMessage>,
}

/// Messaging-domain adapter used by the existing GUI/MCP action dispatcher.
///
/// It provides marker allocation plus body-free send/query/observation
/// operations while leaving composer input and network delivery to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagingDomainAdapter {
    marker_generator: MessageMarkerGenerator,
    store: MessageCorrelationStore,
}

impl MessagingDomainAdapter {
    /// Creates an adapter for one harness run.
    pub fn new(run_id: RunId) -> Self {
        Self {
            marker_generator: MessageMarkerGenerator::new(run_id),
            store: MessageCorrelationStore::default(),
        }
    }

    /// Allocates the next marker for a normal composer send.
    pub fn next_marker(&mut self) -> Result<(MessageMarker, u64), E2eError> {
        self.marker_generator.next_marker()
    }

    /// Registers a marker after the caller has submitted it through the
    /// normal composer path.
    pub fn send_marker(
        &mut self,
        room_marker: RoomMarker,
        message_marker: MessageMarker,
        sequence: u64,
        sender: NodeAlias,
        receiver: NodeAlias,
    ) -> Result<MessageSnapshot, E2eError> {
        self.store.register_send(
            self.marker_generator.run_id().clone(),
            room_marker,
            message_marker,
            sequence,
            sender,
            receiver,
        )
    }

    /// Applies a delivery observation from the GUI/MCP test path.
    pub fn observe_delivery(
        &mut self,
        marker: &MessageMarker,
        state: MessageDeliveryState,
    ) -> Result<MessageSnapshot, E2eError> {
        self.store.observe_delivery(marker, state)
    }

    /// Records one UI presentation and reports whether it was the first.
    pub fn mark_presented(&mut self, marker: &MessageMarker) -> Result<bool, E2eError> {
        self.store.mark_presented(marker)
    }

    /// Reads a stable body-free message snapshot.
    pub fn query_message(&self, marker: &MessageMarker) -> MessageSnapshot {
        self.store.query(marker)
    }

    /// Exposes the durable correlation projection for persistence.
    pub fn store(&self) -> &MessageCorrelationStore {
        &self.store
    }
}

impl MessageCorrelationStore {
    /// Registers a newly sent marker at the queued state.
    pub fn register_send(
        &mut self,
        run_id: RunId,
        room_marker: RoomMarker,
        message_marker: MessageMarker,
        sequence: u64,
        sender: NodeAlias,
        receiver: NodeAlias,
    ) -> Result<MessageSnapshot, E2eError> {
        validate_marker(message_marker.as_ref())?;
        if self.messages.contains_key(&message_marker) {
            return Err(E2eError::new(
                E2eErrorCode::InvalidState,
                "message marker already registered",
            ));
        }
        let message = CorrelatedMessage {
            schema: SchemaVersion::default(),
            run_id,
            room_marker,
            message_marker: message_marker.clone(),
            sequence,
            sender,
            receiver,
            delivery_state: MessageDeliveryState::Queued,
            duplicate_count: 0,
            presentation_count: 0,
            state_history: vec![MessageDeliveryState::Queued],
        };
        let snapshot = message.snapshot();
        self.messages.insert(message_marker, message);
        Ok(snapshot)
    }

    /// Records a transport observation. Repeated observations increment the
    /// duplicate counter but never move state backwards.
    pub fn observe_delivery(
        &mut self,
        marker: &MessageMarker,
        state: MessageDeliveryState,
    ) -> Result<MessageSnapshot, E2eError> {
        let message = self
            .messages
            .get_mut(marker)
            .ok_or_else(|| E2eError::new(E2eErrorCode::NotFound, "message marker is unknown"))?;
        if message.delivery_state == state {
            message.duplicate_count = message.duplicate_count.saturating_add(1);
        } else if can_transition(message.delivery_state, state) {
            message.delivery_state = state;
            message.state_history.push(state);
        } else {
            return Err(E2eError::new(
                E2eErrorCode::InvalidState,
                "message delivery state regressed",
            ));
        }
        Ok(message.snapshot())
    }

    /// Marks presentation of a message and returns whether it was first seen.
    /// Callers can use `false` to assert at-most-once UI presentation.
    pub fn mark_presented(&mut self, marker: &MessageMarker) -> Result<bool, E2eError> {
        let message = self
            .messages
            .get_mut(marker)
            .ok_or_else(|| E2eError::new(E2eErrorCode::NotFound, "message marker is unknown"))?;
        if message.presentation_count == 0 {
            message.presentation_count = 1;
            Ok(true)
        } else {
            message.presentation_count = message.presentation_count.saturating_add(1);
            message.duplicate_count = message.duplicate_count.saturating_add(1);
            Ok(false)
        }
    }

    /// Returns a read-only snapshot; querying never changes duplicate counts.
    pub fn query(&self, marker: &MessageMarker) -> MessageSnapshot {
        self.messages
            .get(marker)
            .map(CorrelatedMessage::snapshot)
            .unwrap_or_else(|| MessageSnapshot {
                schema: SchemaVersion::default(),
                room_marker: RoomMarker::from(""),
                message_marker: marker.clone(),
                present: false,
                delivery_state: None,
                duplicate_count: 0,
                sequence: None,
                sender: None,
                receiver: None,
            })
    }

    /// Returns the body-free durable record for internal assertions.
    pub fn get(&self, marker: &MessageMarker) -> Option<&CorrelatedMessage> {
        self.messages.get(marker)
    }
}

fn validate_marker(marker: &str) -> Result<(), E2eError> {
    let parts: Vec<_> = marker.split(':').collect();
    if marker.len() > MAX_MARKER_LEN
        || parts.len() != 3
        || parts[0] != "E2E"
        || parts[1].is_empty()
        || parts[2].parse::<u64>().is_err()
        || !marker.is_ascii()
        || marker.chars().any(char::is_whitespace)
    {
        return Err(E2eError::new(
            E2eErrorCode::InvalidState,
            "invalid E2E message marker",
        ));
    }
    Ok(())
}

fn can_transition(current: MessageDeliveryState, next: MessageDeliveryState) -> bool {
    if current == MessageDeliveryState::Failed {
        return false;
    }
    if next == MessageDeliveryState::Failed {
        return true;
    }
    state_rank(next) >= state_rank(current)
}

fn state_rank(state: MessageDeliveryState) -> u8 {
    match state {
        MessageDeliveryState::Queued => 0,
        MessageDeliveryState::Sent => 1,
        MessageDeliveryState::Delivered => 2,
        MessageDeliveryState::Seen => 3,
        MessageDeliveryState::Failed => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_opaque_ordered_and_restart_serializable() {
        let mut generator = MessageMarkerGenerator::new("run-7".into());
        let (first, first_sequence) = generator.next_marker().unwrap();
        let (second, second_sequence) = generator.next_marker().unwrap();
        assert_eq!(first.as_ref(), "E2E:run-7:1");
        assert_eq!(second.as_ref(), "E2E:run-7:2");
        assert_eq!((first_sequence, second_sequence), (1, 2));
        let restored: MessageMarkerGenerator =
            serde_json::from_str(&serde_json::to_string(&generator).unwrap()).unwrap();
        assert_eq!(restored.next_sequence, 3);
    }

    #[test]
    fn duplicate_queries_do_not_mutate_and_presentation_is_at_most_once() {
        let marker = MessageMarker::from("E2E:r:1");
        let mut store = MessageCorrelationStore::default();
        store
            .register_send(
                "r".into(),
                "room".into(),
                marker.clone(),
                1,
                "a".into(),
                "b".into(),
            )
            .unwrap();
        let before = store.query(&marker);
        assert_eq!(store.query(&marker), before);
        assert!(store.mark_presented(&marker).unwrap());
        assert!(!store.mark_presented(&marker).unwrap());
        assert_eq!(store.query(&marker).duplicate_count, 1);
    }

    #[test]
    fn delivery_progression_is_monotonic_and_survives_round_trip() {
        let marker = MessageMarker::from("E2E:r:1");
        let mut store = MessageCorrelationStore::default();
        store
            .register_send(
                "r".into(),
                "room".into(),
                marker.clone(),
                1,
                "a".into(),
                "b".into(),
            )
            .unwrap();
        store
            .observe_delivery(&marker, MessageDeliveryState::Sent)
            .unwrap();
        store
            .observe_delivery(&marker, MessageDeliveryState::Delivered)
            .unwrap();
        assert!(store
            .observe_delivery(&marker, MessageDeliveryState::Queued)
            .is_err());
        let restored: MessageCorrelationStore =
            serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();
        assert_eq!(
            restored.query(&marker).delivery_state,
            Some(MessageDeliveryState::Delivered)
        );
        assert_eq!(
            restored.get(&marker).unwrap().state_history,
            vec![
                MessageDeliveryState::Queued,
                MessageDeliveryState::Sent,
                MessageDeliveryState::Delivered
            ]
        );
    }

    #[test]
    fn snapshots_never_contain_message_bodies() {
        let marker = MessageMarker::from("E2E:r:1");
        let mut store = MessageCorrelationStore::default();
        store
            .register_send("r".into(), "room".into(), marker, 1, "a".into(), "b".into())
            .unwrap();
        let json = serde_json::to_string(&store.query(&MessageMarker::from("E2E:r:1"))).unwrap();
        assert!(!json.contains("body"));
        assert!(!json.contains("payload"));
    }
}
