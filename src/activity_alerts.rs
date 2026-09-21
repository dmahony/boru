//! Bounded activity alerts for connected companion clients.
//!
//! Alerts are emitted only after an incoming message has been durably
//! committed.  This module deliberately has no transport or window-focus
//! dependency: a connected companion is an independent alert consumer.

use std::collections::{HashMap, HashSet};

/// The only event which is eligible for an activity alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityOrigin {
    /// A newly committed incoming message.
    IncomingCommit,
    /// A replayed historical row or initial sync row.
    HistorySync,
    /// A local send acknowledgement/echo.
    SendAcknowledgement,
}

/// A stable, authorization-filtered activity alert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityAlert {
    /// Message identity. Retries and reordered hints use this same identity.
    pub message_id: [u8; 32],
    /// Conversation the client may open.
    pub conversation_id: [u8; 32],
    /// Sender identity.
    pub sender_id: [u8; 32],
    /// Safe preview selected by the caller's privacy policy.
    pub preview: String,
    /// True when this is the explicit post-sync coalesced alert.
    pub coalesced: bool,
    /// Number of messages represented by a coalesced alert.
    pub count: u32,
}

/// Sink used by connected clients. Implementations should enqueue quickly.
pub trait ActivityAlertClient: Send + Sync {
    /// Record one already-authorized activity alert.
    fn record_activity(&self, alert: ActivityAlert);
}

/// A recording sink useful for integration tests and diagnostics.
#[derive(Debug, Clone, Default)]
pub struct RecordingActivityAlertClient {
    alerts: std::sync::Arc<std::sync::Mutex<Vec<ActivityAlert>>>,
}

impl RecordingActivityAlertClient {
    /// Return a snapshot of alerts recorded so far.
    pub fn alerts(&self) -> Vec<ActivityAlert> {
        self.alerts.lock().expect("alert recorder lock").clone()
    }
}

impl ActivityAlertClient for RecordingActivityAlertClient {
    fn record_activity(&self, alert: ActivityAlert) {
        self.alerts.lock().expect("alert recorder lock").push(alert);
    }
}

/// Dispatches committed activity while enforcing grants, mutes, and sync
/// semantics.  The client is never called for send acknowledgements.
#[derive(Debug)]
pub struct ActivityAlertDispatcher<C> {
    client: C,
    connected: bool,
    enabled: bool,
    muted: HashSet<[u8; 32]>,
    /// None means all authorized conversations; Some means an explicit grant.
    authorized: Option<HashSet<[u8; 32]>>,
    seen: HashSet<[u8; 32]>,
    syncing: bool,
    missed: HashMap<[u8; 32], (u32, ActivityAlert)>,
}

impl<C: ActivityAlertClient> ActivityAlertDispatcher<C> {
    /// Create a connected, enabled dispatcher for a client sink.
    pub fn new(client: C) -> Self {
        Self {
            client,
            connected: true,
            enabled: true,
            muted: HashSet::new(),
            authorized: None,
            seen: HashSet::new(),
            syncing: false,
            missed: HashMap::new(),
        }
    }

    /// Enable or disable delivery based on the companion connection state.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    /// Apply the user's activity-alert preference.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Set or clear a conversation mute at dispatch time.
    pub fn set_muted(&mut self, conversation: [u8; 32], muted: bool) {
        if muted {
            self.muted.insert(conversation);
        } else {
            self.muted.remove(&conversation);
        }
    }

    /// Replace the grant at dispatch time. An empty explicit grant authorizes
    /// no conversation and therefore fails closed after revocation.
    pub fn set_authorized_conversations(&mut self, conversations: Option<HashSet<[u8; 32]>>) {
        self.authorized = conversations;
    }

    /// Start a sync window whose historical rows must be coalesced.
    pub fn begin_initial_sync(&mut self) {
        self.syncing = true;
        self.missed.clear();
    }

    /// End sync and emit at most one explicit coalesced alert per conversation.
    /// Finish sync and emit explicit coalesced missed-activity alerts.
    pub fn finish_initial_sync(&mut self) {
        self.syncing = false;
        let missed = std::mem::take(&mut self.missed);
        if !self.can_emit() {
            return;
        }
        for (_, (count, mut alert)) in missed {
            alert.coalesced = true;
            alert.count = count;
            self.client.record_activity(alert);
        }
    }

    /// Record one committed incoming message. Returns true when an alert was
    /// emitted or retained for the post-sync coalesced alert.
    pub fn dispatch_committed(
        &mut self,
        origin: ActivityOrigin,
        message_id: [u8; 32],
        conversation_id: [u8; 32],
        sender_id: [u8; 32],
        preview: impl Into<String>,
    ) -> bool {
        if origin != ActivityOrigin::IncomingCommit || !self.seen.insert(message_id) {
            return false;
        }
        if !self.can_emit_for(conversation_id) {
            return false;
        }
        let alert = ActivityAlert {
            message_id,
            conversation_id,
            sender_id,
            preview: preview.into(),
            coalesced: false,
            count: 1,
        };
        if self.syncing {
            let entry = self
                .missed
                .entry(conversation_id)
                .or_insert((0, alert.clone()));
            entry.0 = entry.0.saturating_add(1);
            entry.1 = alert;
            true
        } else {
            self.client.record_activity(alert);
            true
        }
    }

    fn can_emit(&self) -> bool {
        self.connected && self.enabled
    }

    fn can_emit_for(&self, conversation: [u8; 32]) -> bool {
        self.can_emit()
            && !self.muted.contains(&conversation)
            && self
                .authorized
                .as_ref()
                .map_or(true, |ids| ids.contains(&conversation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatcher() -> (
        ActivityAlertDispatcher<RecordingActivityAlertClient>,
        RecordingActivityAlertClient,
    ) {
        let client = RecordingActivityAlertClient::default();
        (ActivityAlertDispatcher::new(client.clone()), client)
    }

    #[test]
    fn duplicate_and_reordered_hints_emit_once() {
        let (mut d, client) = dispatcher();
        let topic = [1; 32];
        assert!(d.dispatch_committed(
            ActivityOrigin::IncomingCommit,
            [2; 32],
            topic,
            [3; 32],
            "one"
        ));
        assert!(!d.dispatch_committed(
            ActivityOrigin::IncomingCommit,
            [2; 32],
            topic,
            [3; 32],
            "one"
        ));
        assert!(d.dispatch_committed(
            ActivityOrigin::IncomingCommit,
            [4; 32],
            topic,
            [3; 32],
            "two"
        ));
        assert_eq!(client.alerts().len(), 2);
    }

    #[test]
    fn mutes_and_revoked_grants_are_applied_at_dispatch() {
        let (mut d, client) = dispatcher();
        let topic = [5; 32];
        d.set_muted(topic, true);
        assert!(!d.dispatch_committed(
            ActivityOrigin::IncomingCommit,
            [6; 32],
            topic,
            [7; 32],
            "muted"
        ));
        d.set_muted(topic, false);
        d.set_authorized_conversations(Some(HashSet::new()));
        assert!(!d.dispatch_committed(
            ActivityOrigin::IncomingCommit,
            [8; 32],
            topic,
            [7; 32],
            "revoked"
        ));
        assert!(client.alerts().is_empty());
    }

    #[test]
    fn sync_coalesces_history_without_a_flood() {
        let (mut d, client) = dispatcher();
        let topic = [9; 32];
        d.begin_initial_sync();
        for id in 10..15 {
            assert!(d.dispatch_committed(
                ActivityOrigin::IncomingCommit,
                [id; 32],
                topic,
                [7; 32],
                "history"
            ));
        }
        assert!(client.alerts().is_empty());
        d.finish_initial_sync();
        let alerts = client.alerts();
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].coalesced);
        assert_eq!(alerts[0].count, 5);
    }

    #[test]
    fn acknowledgements_never_alert() {
        let (mut d, client) = dispatcher();
        assert!(!d.dispatch_committed(
            ActivityOrigin::SendAcknowledgement,
            [1; 32],
            [2; 32],
            [3; 32],
            "ack"
        ));
        assert!(client.alerts().is_empty());
    }
}
