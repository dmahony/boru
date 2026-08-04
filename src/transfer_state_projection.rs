//! Live transfer-state projection for dashboard consumers.
//!
//! The projection is fed by authoritative transfer lifecycle callbacks. It is
//! deliberately independent of storage and widgets: callers publish events on
//! [`TransferStateStore`] and an Iced subscription can consume its broadcast
//! receiver without polling the database or blocking the UI thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;

use crate::chat_callbacks::{TransferId, TransferProgress};

/// Whether this node is receiving or serving bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferDirection {
    /// A remote peer is sending bytes to this node.
    Inbound,
    /// A remote peer is downloading bytes from this node.
    Outbound,
}

/// A lifecycle event from a real transfer engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferEvent {
    /// Unique event id supplied by the producer; used for at-least-once dedup.
    pub event_id: String,
    /// Stable logical transfer id.
    pub transfer_id: String,
    /// Stable content/file id, never a local filesystem path.
    pub item_id: String,
    /// Transfer direction.
    pub direction: TransferDirection,
    /// Authenticated peer identity, when the producer has it.
    pub peer_id: Option<String>,
    /// Monotonic sequence within the logical transfer.
    pub sequence: u64,
    /// Attempt number; retries may increase it.
    pub attempt: u32,
    /// Producer observation time in Unix milliseconds.
    pub occurred_at_ms: u64,
    /// Lifecycle kind.
    pub kind: EventName,
    /// Cumulative bytes observed.
    pub bytes: u64,
    /// Expected bytes, when known.
    pub total_bytes: Option<u64>,
    /// Bounded human-readable error summary for failed transfers.
    pub error: Option<String>,
}

/// Lifecycle categories accepted by the projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventName {
    /// Transfer admission or network work began.
    Started,
    /// Cumulative progress update.
    Progress,
    /// Verification is in progress.
    Verifying,
    /// Transfer completed successfully.
    Completed,
    /// Transfer failed.
    Failed,
    /// Transfer was cancelled.
    Cancelled,
}

/// Current dashboard state for one logical transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferRecord {
    /// Stable logical transfer id.
    pub transfer_id: String,
    /// Stable content/file id.
    pub item_id: String,
    /// Direction of the transfer.
    pub direction: TransferDirection,
    /// Authenticated peer identity, if known.
    pub peer_id: Option<String>,
    /// Latest monotonic byte count.
    pub bytes: u64,
    /// Latest known total.
    pub total_bytes: Option<u64>,
    /// Current state.
    pub state: TransferState,
    /// First observed timestamp.
    pub started_at_ms: u64,
    /// Last accepted event timestamp.
    pub updated_at_ms: u64,
    /// Latest bounded error summary.
    pub error: Option<String>,
    /// Latest attempt number.
    pub attempt: u32,
}

/// State visible to the dashboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferState {
    /// Waiting or actively transferring.
    Active,
    /// Integrity verification is running.
    Verifying,
    /// Finished successfully.
    Completed,
    /// Failed and no longer active.
    Failed,
    /// Cancelled by a user or lifecycle shutdown.
    Cancelled,
    /// Peer disconnected while the transfer was active.
    Disconnected,
}

impl TransferState {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Whether this state is a durable terminal outcome. Disconnected is
    /// intentionally not terminal — an interrupted transfer may resume when
    /// the peer returns.
    pub fn is_terminal(self) -> bool {
        self.terminal()
    }
}

/// A state change delivered to subscribers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionUpdate {
    /// Updated transfer record.
    pub transfer: TransferRecord,
    /// True for start, disconnect, and all terminal states; progress may be coalesced.
    pub immediate: bool,
}

/// Mutable, deduplicating event reducer.
#[derive(Debug)]
pub struct TransferProjection {
    records: HashMap<String, TransferRecord>,
    seen_event_ids: HashSet<String>,
    last_sequence: HashMap<String, u64>,
    progress_interval_ms: u64,
    last_progress_emit_ms: HashMap<String, u64>,
    /// Terminal records are retained for the dashboard's completed/history view.
    archive: VecDeque<TransferRecord>,
}

impl TransferProjection {
    /// Construct a projection with a 250ms progress cadence.
    pub fn new(_now_ms: u64) -> Self {
        Self::with_progress_interval(250)
    }

    /// Construct a projection with a caller-selected progress cadence.
    pub fn with_progress_interval(progress_interval_ms: u64) -> Self {
        Self {
            records: HashMap::new(),
            seen_event_ids: HashSet::new(),
            last_sequence: HashMap::new(),
            progress_interval_ms,
            last_progress_emit_ms: HashMap::new(),
            archive: VecDeque::new(),
        }
    }

    /// Apply one event. Duplicate, stale, and post-terminal events are ignored.
    pub fn apply(&mut self, event: TransferEvent) -> Option<ProjectionUpdate> {
        if !self.seen_event_ids.insert(event.event_id.clone()) {
            return None;
        }
        if self
            .last_sequence
            .get(&event.transfer_id)
            .is_some_and(|last| event.sequence <= *last)
        {
            return None;
        }
        self.last_sequence
            .insert(event.transfer_id.clone(), event.sequence);

        let record = self
            .records
            .entry(event.transfer_id.clone())
            .or_insert_with(|| TransferRecord {
                transfer_id: event.transfer_id.clone(),
                item_id: event.item_id.clone(),
                direction: event.direction,
                peer_id: event.peer_id.clone(),
                bytes: 0,
                total_bytes: None,
                state: TransferState::Active,
                started_at_ms: event.occurred_at_ms,
                updated_at_ms: event.occurred_at_ms,
                error: None,
                attempt: event.attempt,
            });
        if record.state.terminal() {
            return None;
        }
        record.bytes = record.bytes.max(event.bytes);
        record.total_bytes = match (record.total_bytes, event.total_bytes) {
            (Some(old), Some(new)) => Some(old.max(new)),
            (old, new) => old.or(new),
        };
        if record.peer_id.is_none() {
            record.peer_id = event.peer_id.clone();
        }
        record.updated_at_ms = record.updated_at_ms.max(event.occurred_at_ms);
        record.attempt = record.attempt.max(event.attempt);
        record.item_id = event.item_id;
        let terminal = match event.kind {
            EventName::Started | EventName::Progress => TransferState::Active,
            EventName::Verifying => TransferState::Verifying,
            EventName::Completed => TransferState::Completed,
            EventName::Failed => {
                record.error = event.error.map(|value| value.chars().take(256).collect());
                TransferState::Failed
            }
            EventName::Cancelled => TransferState::Cancelled,
        };
        record.state = terminal;
        let immediate = !matches!(event.kind, EventName::Progress)
            || terminal.terminal()
            || self
                .last_progress_emit_ms
                .get(&event.transfer_id)
                .is_none_or(|last| {
                    event.occurred_at_ms.saturating_sub(*last) >= self.progress_interval_ms
                });
        if matches!(event.kind, EventName::Started) {
            self.last_progress_emit_ms
                .insert(event.transfer_id.clone(), event.occurred_at_ms);
        }
        if matches!(event.kind, EventName::Progress) && immediate {
            self.last_progress_emit_ms
                .insert(event.transfer_id.clone(), event.occurred_at_ms);
        }
        let update = ProjectionUpdate {
            transfer: record.clone(),
            immediate,
        };
        if terminal.terminal() {
            self.archive.push_back(record.clone());
        }
        immediate.then_some(update)
    }

    /// Mark active transfers for an authenticated peer disconnected.
    pub fn disconnect_peer(&mut self, peer_id: &str, occurred_at_ms: u64) -> Vec<ProjectionUpdate> {
        self.records
            .values_mut()
            .filter(|record| record.peer_id.as_deref() == Some(peer_id) && !record.state.terminal())
            .map(|record| {
                record.state = TransferState::Disconnected;
                record.updated_at_ms = record.updated_at_ms.max(occurred_at_ms);
                ProjectionUpdate {
                    transfer: record.clone(),
                    immediate: true,
                }
            })
            .collect()
    }

    /// Return the current record for a transfer.
    pub fn get(&self, transfer_id: &str) -> Option<&TransferRecord> {
        self.records.get(transfer_id)
    }

    /// Return terminal records retained for history.
    pub fn archive(&self) -> impl Iterator<Item = &TransferRecord> {
        self.archive.iter()
    }

    /// Return a snapshot suitable for an initial UI render.
    pub fn snapshot(&self) -> Vec<TransferRecord> {
        self.records.values().cloned().collect()
    }
}

/// Thread-safe reducer plus a non-blocking broadcast channel for UI subscribers.
#[derive(Clone, Debug)]
pub struct TransferStateStore {
    projection: Arc<Mutex<TransferProjection>>,
    updates: broadcast::Sender<ProjectionUpdate>,
}

impl TransferStateStore {
    /// Create a store using the default 250ms progress cadence.
    pub fn new(channel_capacity: usize) -> Self {
        let (updates, _) = broadcast::channel(channel_capacity.max(1));
        Self {
            projection: Arc::new(Mutex::new(TransferProjection::new(0))),
            updates,
        }
    }

    /// Subscribe without holding a lock or polling.
    pub fn subscribe(&self) -> broadcast::Receiver<ProjectionUpdate> {
        self.updates.subscribe()
    }

    /// Publish an event and notify subscribers when the reducer emits an update.
    pub fn publish(&self, event: TransferEvent) {
        let update = self
            .projection
            .lock()
            .expect("transfer projection lock")
            .apply(event);
        if let Some(update) = update {
            let _ = self.updates.send(update);
        }
    }

    /// Read a lock-free-to-call snapshot for initial UI state.
    pub fn snapshot(&self) -> Vec<TransferRecord> {
        self.projection
            .lock()
            .expect("transfer projection lock")
            .snapshot()
    }

    /// Mark active transfers for an authenticated peer disconnected and
    /// broadcast every resulting update to subscribers.
    pub fn disconnect_peer(&self, peer_id: &str, occurred_at_ms: u64) {
        let updates = self
            .projection
            .lock()
            .expect("transfer projection lock")
            .disconnect_peer(peer_id, occurred_at_ms);
        for update in updates {
            let _ = self.updates.send(update);
        }
    }

    /// Convert an existing authenticated inbound callback into a projection event.
    pub fn publish_progress(
        &self,
        event_id: impl Into<String>,
        id: TransferId,
        item_id: impl Into<String>,
        peer_id: Option<String>,
        sequence: u64,
        occurred_at_ms: u64,
        progress: TransferProgress,
    ) {
        let (kind, bytes, total_bytes, error) = match progress {
            TransferProgress::Started { total, .. } => (EventName::Started, 0, total, None),
            TransferProgress::Progress { bytes, total, .. } => {
                (EventName::Progress, bytes, total, None)
            }
            TransferProgress::Completed { .. } => (EventName::Completed, 0, None, None),
            TransferProgress::Failed { error, .. } => (EventName::Failed, 0, None, Some(error)),
            TransferProgress::Cancelled { .. } => (EventName::Cancelled, 0, None, None),
        };
        self.publish(TransferEvent {
            event_id: event_id.into(),
            transfer_id: id.into_u64().to_string(),
            item_id: item_id.into(),
            direction: TransferDirection::Inbound,
            peer_id,
            sequence,
            attempt: 1,
            occurred_at_ms,
            kind,
            bytes,
            total_bytes,
            error,
        });
    }
}

/// Keep the core channel independent of Iced while making the subscription seam explicit.
pub type TransferUpdateReceiver = broadcast::Receiver<ProjectionUpdate>;

/// Keep this import in the public API documentation for consumers using async subscriptions.
pub const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, sequence: u64, name: EventName, bytes: u64, at_ms: u64) -> TransferEvent {
        TransferEvent {
            event_id: id.into(),
            transfer_id: "transfer-1".into(),
            item_id: "item-1".into(),
            direction: TransferDirection::Inbound,
            peer_id: Some("peer-a".into()),
            sequence,
            attempt: 1,
            occurred_at_ms: at_ms,
            kind: name,
            bytes,
            total_bytes: Some(100),
            error: None,
        }
    }

    #[test]
    fn lifecycle_projection_tracks_progress_and_final_state() {
        let mut projection = TransferProjection::with_progress_interval(0);
        assert!(projection
            .apply(event("start", 0, EventName::Started, 0, 10))
            .is_some());
        assert!(projection
            .apply(event("progress", 1, EventName::Progress, 40, 20))
            .is_some());
        let update = projection
            .apply(event("done", 2, EventName::Completed, 100, 30))
            .unwrap();
        assert!(update.immediate);
        let transfer = projection.get("transfer-1").unwrap();
        assert_eq!(transfer.bytes, 100);
        assert_eq!(transfer.state, TransferState::Completed);
    }

    #[test]
    fn duplicate_and_out_of_order_events_do_not_regress_state() {
        let mut projection = TransferProjection::new(0);
        projection.apply(event("start", 0, EventName::Started, 0, 10));
        projection.apply(event("progress", 2, EventName::Progress, 80, 30));
        assert!(projection
            .apply(event("progress-old", 1, EventName::Progress, 20, 20))
            .is_none());
        assert!(projection
            .apply(event("progress", 2, EventName::Progress, 80, 30))
            .is_none());
        assert_eq!(projection.get("transfer-1").unwrap().bytes, 80);
    }

    #[test]
    fn progress_is_coalesced_but_terminal_state_is_immediate() {
        let mut projection = TransferProjection::with_progress_interval(100);
        projection.apply(event("start", 0, EventName::Started, 0, 0));
        assert!(projection
            .apply(event("p1", 1, EventName::Progress, 10, 10))
            .is_none());
        assert!(projection
            .apply(event("p2", 2, EventName::Progress, 20, 50))
            .is_none());
        assert!(projection
            .apply(event("p3", 3, EventName::Progress, 30, 110))
            .is_some());
        assert!(
            projection
                .apply(event("done", 4, EventName::Completed, 100, 111))
                .unwrap()
                .immediate
        );
    }

    #[test]
    fn disconnect_marks_active_transfer_disconnected_without_losing_peer_identity() {
        let mut projection = TransferProjection::new(0);
        projection.apply(event("start", 0, EventName::Started, 4, 10));
        let update = projection.disconnect_peer("peer-a", 20);
        assert_eq!(update.len(), 1);
        let transfer = projection.get("transfer-1").unwrap();
        assert_eq!(transfer.state, TransferState::Disconnected);
        assert_eq!(transfer.peer_id.as_deref(), Some("peer-a"));
    }

    #[test]
    fn outbound_events_retain_authenticated_downloading_peer() {
        let mut projection = TransferProjection::new(0);
        let mut outbound = event("start", 0, EventName::Started, 0, 10);
        outbound.direction = TransferDirection::Outbound;
        outbound.peer_id = Some("authenticated-peer".into());
        projection.apply(outbound);
        let record = projection.get("transfer-1").unwrap();
        assert_eq!(record.direction, TransferDirection::Outbound);
        assert_eq!(record.peer_id.as_deref(), Some("authenticated-peer"));
    }

    #[tokio::test]
    async fn store_broadcasts_reduced_updates_without_polling() {
        let store = TransferStateStore::new(8);
        let mut receiver = store.subscribe();
        store.publish(event("start", 0, EventName::Started, 0, 10));
        let update = receiver.recv().await.unwrap();
        assert_eq!(update.transfer.transfer_id, "transfer-1");
        assert!(store
            .snapshot()
            .iter()
            .any(|item| item.transfer_id == "transfer-1"));
    }

    #[test]
    fn cancelled_transfer_reaches_terminal_state_and_rejects_followups() {
        let mut projection = TransferProjection::new(0);
        projection.apply(event("start", 0, EventName::Started, 0, 10));
        projection.apply(event("cancel", 1, EventName::Cancelled, 0, 20));
        let record = projection.get("transfer-1").unwrap();
        assert_eq!(record.state, TransferState::Cancelled);
        assert!(record.state.is_terminal());
        // Post-terminal progress must never resurrect the record.
        assert!(projection
            .apply(event("late", 2, EventName::Progress, 50, 30))
            .is_none());
        assert_eq!(projection.get("transfer-1").unwrap().bytes, 0);
        assert_eq!(
            projection.get("transfer-1").unwrap().state,
            TransferState::Cancelled
        );
    }

    #[test]
    fn failed_transfer_records_bounded_error_and_ignores_later_events() {
        let mut projection = TransferProjection::new(0);
        projection.apply(event("start", 0, EventName::Started, 0, 10));
        let mut fail = event("fail", 1, EventName::Failed, 0, 20);
        fail.error = Some("e".repeat(1000));
        projection.apply(fail);
        let record = projection.get("transfer-1").unwrap();
        assert_eq!(record.state, TransferState::Failed);
        // Error summaries are bounded so a hostile producer cannot bloat rows.
        assert_eq!(record.error.as_deref().unwrap().len(), 256);
        assert!(projection
            .apply(event("late", 2, EventName::Progress, 90, 30))
            .is_none());
        assert_eq!(projection.get("transfer-1").unwrap().bytes, 0);
    }

    #[test]
    fn retry_bumps_attempt_without_regressing_state() {
        let mut projection = TransferProjection::new(0);
        projection.apply(event("start", 0, EventName::Started, 0, 10));
        let mut retry = event("retry-progress", 1, EventName::Progress, 5, 20);
        retry.attempt = 2;
        projection.apply(retry);
        let record = projection.get("transfer-1").unwrap();
        assert_eq!(record.attempt, 2);
        assert_eq!(record.state, TransferState::Active);
        // A lower attempt on a later sequence must not lower the counter.
        let mut stale = event("stale-attempt", 2, EventName::Progress, 10, 30);
        stale.attempt = 1;
        projection.apply(stale);
        assert_eq!(projection.get("transfer-1").unwrap().attempt, 2);
        assert_eq!(projection.get("transfer-1").unwrap().bytes, 10);
    }

    #[test]
    fn disconnected_transfer_restarts_on_newer_event() {
        let mut projection = TransferProjection::with_progress_interval(0);
        projection.apply(event("start", 0, EventName::Started, 4, 10));
        projection.disconnect_peer("peer-a", 20);
        assert_eq!(
            projection.get("transfer-1").unwrap().state,
            TransferState::Disconnected
        );
        // The peer returns: a newer progress event resumes the same record
        // and keeps the authenticated peer identity.
        let resume = projection
            .apply(event("resume", 1, EventName::Progress, 40, 30))
            .unwrap();
        assert!(resume.immediate);
        let record = projection.get("transfer-1").unwrap();
        assert_eq!(record.state, TransferState::Active);
        assert_eq!(record.peer_id.as_deref(), Some("peer-a"));
        assert_eq!(record.bytes, 40);
    }

    #[test]
    fn terminal_records_are_retained_in_archive() {
        let mut projection = TransferProjection::new(0);
        projection.apply(event("start", 0, EventName::Started, 0, 10));
        projection.apply(event("done", 1, EventName::Completed, 100, 20));
        let archived: Vec<_> = projection.archive().collect();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].state, TransferState::Completed);
        assert_eq!(archived[0].bytes, 100);
    }

    #[test]
    fn post_terminal_events_never_regress_an_archived_transfer() {
        let mut projection = TransferProjection::new(0);
        projection.apply(event("start", 0, EventName::Started, 0, 10));
        projection.apply(event("done", 1, EventName::Completed, 100, 20));
        // A replayed terminal event with a different event id but stale seq.
        assert!(projection
            .apply(event("done-copy", 1, EventName::Completed, 100, 20))
            .is_none());
        // Newer event ids after a terminal outcome are also ignored.
        assert!(projection
            .apply(event("newer", 2, EventName::Progress, 100, 30))
            .is_none());
        assert_eq!(
            projection.get("transfer-1").unwrap().state,
            TransferState::Completed
        );
        assert_eq!(projection.archive().count(), 1);
    }
}
