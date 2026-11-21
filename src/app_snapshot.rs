//! GUI-independent, read-only projection of authoritative application state.
//!
//! Production runtimes implement [`SnapshotSource`] by querying their backend
//! stores/services. This boundary deliberately contains no frontend types.
use crate::snapshot::{
    ActivitySnapshot, AppSnapshot, DirectMessageSnapshot, IdentitySnapshot, NetworkHealthSnapshot,
    ProfileSnapshot, RoomSnapshot, TransferSummary, UnreadSnapshot, SNAPSHOT_SCHEMA_VERSION,
};

/// Read-only snapshot inputs exposed by a backend runtime.
pub trait SnapshotSource {
    /// Return public local identity, or `None` when unavailable.
    fn identity(&self) -> Option<IdentitySnapshot>;
    /// Return selected profile, or `None` when no authoritative selection exists.
    fn active_profile(&self) -> Option<ProfileSnapshot>;
    /// Return known room summaries; empty means no known rooms.
    fn rooms(&self) -> Vec<RoomSnapshot>;
    /// Return known direct-message summaries.
    fn direct_messages(&self) -> Vec<DirectMessageSnapshot>;
    /// Return durable unread counts by conversation.
    fn unread(&self) -> Vec<UnreadSnapshot>;
    /// Return recent canonical user-visible activity.
    fn activity(&self) -> Vec<ActivitySnapshot>;
    /// Return network health when a runtime health source is available.
    fn network_health(&self) -> Option<NetworkHealthSnapshot>;
    /// Return active/recent transfer summaries.
    fn transfers(&self) -> Vec<TransferSummary>;
}

impl AppSnapshot {
    /// Build a point-in-time DTO without mutating or retaining the source.
    pub fn project(source: &impl SnapshotSource) -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            identity: source.identity(),
            active_profile: source.active_profile(),
            rooms: source.rooms(),
            direct_messages: source.direct_messages(),
            unread: source.unread(),
            activity: source.activity(),
            network_health: source.network_health(),
            transfers: source.transfers(),
        }
    }
}

/// Explicit empty/unavailable projection for the current production service.
/// Callers should implement the trait as backend read APIs become available.
#[derive(Debug, Default)]
pub struct UnavailableSnapshotSource;

impl SnapshotSource for UnavailableSnapshotSource {
    // Identity has no GUI-independent runtime accessor in the current service.
    fn identity(&self) -> Option<IdentitySnapshot> { None }
    // RESIDUAL BRT: active-profile selection is not authoritative outside Iced.
    fn active_profile(&self) -> Option<ProfileSnapshot> { None }
    // RESIDUAL BRT: room metadata/classification currently lives outside the service.
    fn rooms(&self) -> Vec<RoomSnapshot> { Vec::new() }
    // RESIDUAL BRT: DM classification/name resolution lacks a backend read API.
    fn direct_messages(&self) -> Vec<DirectMessageSnapshot> { Vec::new() }
    // Existing service will provide durable unread data when wired as a source.
    fn unread(&self) -> Vec<UnreadSnapshot> { Vec::new() }
    // RESIDUAL BRT: no canonical all-app activity stream exists.
    fn activity(&self) -> Vec<ActivitySnapshot> { Vec::new() }
    // RESIDUAL BRT: endpoint/mesh health has no GUI-independent read API.
    fn network_health(&self) -> Option<NetworkHealthSnapshot> { None }
    // RESIDUAL BRT: transfer summaries need a backend transfer-store accessor.
    fn transfers(&self) -> Vec<TransferSummary> { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Eq, Debug)]
    struct FixtureSnapshotSource {
        identity: Option<IdentitySnapshot>,
        profile: Option<ProfileSnapshot>,
        rooms: Vec<RoomSnapshot>,
        dms: Vec<DirectMessageSnapshot>,
        unread: Vec<UnreadSnapshot>,
        activity: Vec<ActivitySnapshot>,
        health: Option<NetworkHealthSnapshot>,
        transfers: Vec<TransferSummary>,
    }

    impl SnapshotSource for FixtureSnapshotSource {
        fn identity(&self) -> Option<IdentitySnapshot> { self.identity.clone() }
        fn active_profile(&self) -> Option<ProfileSnapshot> { self.profile.clone() }
        fn rooms(&self) -> Vec<RoomSnapshot> { self.rooms.clone() }
        fn direct_messages(&self) -> Vec<DirectMessageSnapshot> { self.dms.clone() }
        fn unread(&self) -> Vec<UnreadSnapshot> { self.unread.clone() }
        fn activity(&self) -> Vec<ActivitySnapshot> { self.activity.clone() }
        fn network_health(&self) -> Option<NetworkHealthSnapshot> { self.health.clone() }
        fn transfers(&self) -> Vec<TransferSummary> { self.transfers.clone() }
    }

    #[test]
    fn projection_populates_every_field_without_mutating_source() {
        let source = FixtureSnapshotSource {
            identity: Some(IdentitySnapshot { id: "peer-1".into(), display_name: Some("A".into()) }),
            profile: Some(ProfileSnapshot { id: "profile-1".into(), display_name: "A".into(), description: Some("bio".into()) }),
            rooms: vec![RoomSnapshot { id: "room-1".into(), name: "General".into(), topic: Some("topic".into()), participant_count: Some(2), last_activity_at_ms: Some(10) }],
            dms: vec![DirectMessageSnapshot { id: "dm-1".into(), peer_id: "peer-2".into(), peer_name: "B".into(), last_message_preview: Some("hello".into()), last_activity_at_ms: Some(11) }],
            unread: vec![UnreadSnapshot { conversation_id: "room-1".into(), count: 3 }],
            activity: vec![ActivitySnapshot { id: "event-1".into(), kind: "message".into(), summary: "new message".into(), occurred_at_ms: 12, conversation_id: Some("room-1".into()) }],
            health: Some(NetworkHealthSnapshot { ready: true, status: "connected".into(), connected_peer_count: Some(1), relay_url: None }),
            transfers: vec![TransferSummary { id: "transfer-1".into(), direction: "download".into(), file_name: "file.bin".into(), state: "complete".into(), transferred_bytes: 5, total_bytes: Some(5) }],
        };
        let before = source.clone();
        let first = AppSnapshot::project(&source);
        let second = AppSnapshot::project(&source);
        assert_eq!(first.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert!(first.identity.is_some() && first.active_profile.is_some());
        assert!(!first.rooms.is_empty() && !first.direct_messages.is_empty());
        assert!(!first.unread.is_empty() && !first.activity.is_empty());
        assert!(first.network_health.is_some() && !first.transfers.is_empty());
        assert_eq!(first, second);
        assert_eq!(source, before);
    }
}
