//! GUI-independent, read-only projection of authoritative application state.
//!
//! Production runtimes implement [`SnapshotSource`] by querying their backend
//! stores/services. This boundary deliberately contains no frontend types.
use crate::conversations::{ConversationEntry, ConversationKind};
use crate::snapshot::{
    ActivitySnapshot, AppSnapshot, DirectMessageSnapshot, IdentitySnapshot, NetworkHealthSnapshot,
    ProfileSnapshot, RoomSnapshot, TransferSummary, UnreadSnapshot, SNAPSHOT_SCHEMA_VERSION,
};
use crate::transfer_state_projection::{TransferDirection, TransferRecord, TransferState};
use crate::user_profile::UserProfile;

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

/// Hex conversation identifier for a persisted topic.
fn conversation_id(entry: &ConversationEntry) -> String {
    entry.topic.to_string()
}

/// Project persisted group conversations into room snapshots.
fn project_rooms<'a>(
    entries: impl IntoIterator<Item = &'a ConversationEntry>,
    members: &std::collections::HashMap<
        crate::TopicId,
        std::collections::HashSet<iroh::EndpointId>,
    >,
) -> Vec<RoomSnapshot> {
    let mut rooms: Vec<RoomSnapshot> = entries
        .into_iter()
        .filter(|entry| entry.kind == ConversationKind::Group && !entry.archived)
        .map(|entry| RoomSnapshot {
            id: conversation_id(entry),
            name: entry.name.clone(),
            topic: (!entry.description.is_empty()).then(|| entry.description.clone()),
            // Number of remote connected neighbors; the local endpoint is not counted.
            participant_count: members.get(&entry.topic).map(|peers| peers.len() as u32),
            last_activity_at_ms: (entry.last_seen_at_unix_ms > 0)
                .then_some(entry.last_seen_at_unix_ms),
        })
        .collect();
    rooms.sort_by(|a, b| {
        b.last_activity_at_ms
            .cmp(&a.last_activity_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    rooms
}

/// Project persisted direct conversations into DM snapshots.
fn project_direct_messages<'a>(
    entries: impl IntoIterator<Item = &'a ConversationEntry>,
) -> Vec<DirectMessageSnapshot> {
    let mut messages: Vec<DirectMessageSnapshot> = entries
        .into_iter()
        .filter(|entry| entry.kind == ConversationKind::Direct && !entry.archived)
        .map(|entry| DirectMessageSnapshot {
            id: conversation_id(entry),
            peer_id: entry.peer_id.clone(),
            peer_name: if entry.name.is_empty() {
                entry.peer_id.clone()
            } else {
                entry.name.clone()
            },
            last_message_preview: (!entry.last_message_preview.is_empty())
                .then(|| entry.last_message_preview.clone()),
            last_activity_at_ms: (entry.last_seen_at_unix_ms > 0)
                .then_some(entry.last_seen_at_unix_ms),
        })
        .collect();
    messages.sort_by(|a, b| {
        b.last_activity_at_ms
            .cmp(&a.last_activity_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    messages
}

/// Project durable unread counts, omitting conversations with nothing unread.
///
/// `store_count` supplies the authoritative per-conversation count that the room
/// pumps persist in the message store. It returns `None` when the store has no
/// row for the conversation, in which case the catalog count is used.
fn project_unread<'a>(
    entries: impl IntoIterator<Item = &'a ConversationEntry>,
    store_count: impl Fn(&ConversationEntry) -> Option<u32>,
) -> Vec<UnreadSnapshot> {
    let mut unread: Vec<UnreadSnapshot> = entries
        .into_iter()
        .filter(|entry| !entry.archived)
        .filter_map(|entry| {
            let count = store_count(entry).unwrap_or(entry.unread_count);
            (count > 0).then(|| UnreadSnapshot {
                conversation_id: conversation_id(entry),
                count,
            })
        })
        .collect();
    unread.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.conversation_id.cmp(&b.conversation_id))
    });
    unread
}

/// Machine-readable direction label used by the snapshot contract.
fn direction_label(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Inbound => "download",
        TransferDirection::Outbound => "upload",
    }
}

/// Machine-readable state label used by the snapshot contract.
fn state_label(state: TransferState) -> &'static str {
    match state {
        TransferState::Active => "active",
        TransferState::Verifying => "verifying",
        TransferState::Completed => "completed",
        TransferState::Failed => "failed",
        TransferState::Cancelled => "cancelled",
        TransferState::Disconnected => "disconnected",
    }
}

/// Project live transfer projection records into snapshot summaries.
fn project_transfers(records: &[TransferRecord]) -> Vec<TransferSummary> {
    records
        .iter()
        .map(|record| TransferSummary {
            id: record.transfer_id.clone(),
            direction: direction_label(record.direction).to_string(),
            // The projection store carries the content/file id, which is the
            // only stable human-recognisable label available here.
            file_name: record.item_id.clone(),
            state: state_label(record.state).to_string(),
            transferred_bytes: record.bytes,
            total_bytes: record.total_bytes,
        })
        .collect()
}

/// Project the stored local user profile into a profile snapshot.
fn project_profile(profile: &UserProfile) -> ProfileSnapshot {
    ProfileSnapshot {
        id: profile.user_id.to_string(),
        display_name: profile.display_name.clone(),
        description: (!profile.bio.is_empty()).then(|| profile.bio.clone()),
    }
}

/// Documented residual: no canonical all-app activity stream exists.
///
/// `crate::activity_alerts::RecordingActivityAlertClient` is a test-only
/// recorder, not a live source, so the runtime projects the contract default.
fn project_activity<'a>(
    entries: impl IntoIterator<Item = &'a ConversationEntry>,
) -> Vec<ActivitySnapshot> {
    entries
        .into_iter()
        .filter(|entry| {
            entry.kind == ConversationKind::Group
                && !entry.last_message_preview.is_empty()
                && entry.last_seen_at_unix_ms > 0
        })
        .map(|entry| ActivitySnapshot {
            id: format!("conversation:{}", conversation_id(entry)),
            kind: "message".to_string(),
            summary: entry.last_message_preview.clone(),
            occurred_at_ms: entry.last_seen_at_unix_ms,
            conversation_id: Some(conversation_id(entry)),
        })
        .collect()
}

/// Snapshot projection backed by the live GUI-independent runtime.
impl SnapshotSource for crate::desktop_runtime::DesktopRuntime {
    fn identity(&self) -> Option<IdentitySnapshot> {
        Some(IdentitySnapshot {
            id: self.endpoint().id().to_string(),
            display_name: None,
        })
    }

    fn active_profile(&self) -> Option<ProfileSnapshot> {
        self.profile_snapshot().as_ref().map(project_profile)
    }

    fn rooms(&self) -> Vec<RoomSnapshot> {
        let conversations = self.conversations();
        project_rooms(conversations.iter(), &self.room_members())
    }

    fn direct_messages(&self) -> Vec<DirectMessageSnapshot> {
        let conversations = self.conversations();
        project_direct_messages(conversations.iter())
    }

    fn unread(&self) -> Vec<UnreadSnapshot> {
        let conversations = self.conversations();
        // Room pumps persist unread counts in the message store, not in the
        // conversation catalog. Prefer that authoritative count so snapshots and
        // `mark_read` observe the same state; fall back to the catalog count
        // when the store has no row for the conversation.
        let store = self.message_store();
        project_unread(conversations.iter(), |entry| {
            store
                .get_unread_count(entry.topic.as_bytes())
                .ok()
                .flatten()
        })
    }

    fn activity(&self) -> Vec<ActivitySnapshot> {
        let conversations = self.conversations();
        project_activity(conversations.iter())
    }

    fn network_health(&self) -> Option<NetworkHealthSnapshot> {
        use iroh::Watcher;

        let relays = self.endpoint().home_relay_status().get();
        let connected = relays.iter().any(|status| status.is_connected());
        let relay_url = relays
            .iter()
            .find(|status| status.is_connected())
            .or_else(|| relays.first())
            .map(|status| status.url().to_string());
        Some(NetworkHealthSnapshot {
            // A live runtime endpoint is bound and available for use.
            ready: true,
            status: if connected {
                "connected".to_string()
            } else {
                "disconnected".to_string()
            },
            // The endpoint exposes relay reachability, not a peer census.
            connected_peer_count: None,
            relay_url,
        })
    }

    fn transfers(&self) -> Vec<TransferSummary> {
        project_transfers(&self.transfer_snapshot())
    }
}

impl crate::desktop_runtime::DesktopRuntime {
    /// Return the current read-only application snapshot.
    pub fn get_app_snapshot(&self) -> AppSnapshot {
        AppSnapshot::project(self)
    }

    /// Return a snapshot with a conservative event cursor. Subscribe to events
    /// before calling this method, then replay strictly after the returned
    /// cursor. Events racing with projection may be reflected in both snapshot
    /// and replay; consumers should apply event IDs idempotently.
    pub fn get_app_snapshot_with_revision(&self) -> (AppSnapshot, crate::ui_events::EventCursor) {
        let cursor = self.event_cursor();
        (self.get_app_snapshot(), cursor)
    }
}

/// Explicit empty/unavailable projection for contract tests and callers without a runtime.
#[derive(Debug, Default)]
pub struct UnavailableSnapshotSource;

impl SnapshotSource for UnavailableSnapshotSource {
    // Identity has no GUI-independent runtime accessor in the current service.
    fn identity(&self) -> Option<IdentitySnapshot> {
        None
    }
    // RESIDUAL BRT: active-profile selection is not authoritative outside Iced.
    fn active_profile(&self) -> Option<ProfileSnapshot> {
        None
    }
    // RESIDUAL BRT: room metadata/classification currently lives outside the service.
    fn rooms(&self) -> Vec<RoomSnapshot> {
        Vec::new()
    }
    // RESIDUAL BRT: DM classification/name resolution lacks a backend read API.
    fn direct_messages(&self) -> Vec<DirectMessageSnapshot> {
        Vec::new()
    }
    // Existing service will provide durable unread data when wired as a source.
    fn unread(&self) -> Vec<UnreadSnapshot> {
        Vec::new()
    }
    // RESIDUAL BRT: no canonical all-app activity stream exists.
    fn activity(&self) -> Vec<ActivitySnapshot> {
        Vec::new()
    }
    // RESIDUAL BRT: endpoint/mesh health has no GUI-independent read API.
    fn network_health(&self) -> Option<NetworkHealthSnapshot> {
        None
    }
    // RESIDUAL BRT: transfer summaries need a backend transfer-store accessor.
    fn transfers(&self) -> Vec<TransferSummary> {
        Vec::new()
    }
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
        fn identity(&self) -> Option<IdentitySnapshot> {
            self.identity.clone()
        }
        fn active_profile(&self) -> Option<ProfileSnapshot> {
            self.profile.clone()
        }
        fn rooms(&self) -> Vec<RoomSnapshot> {
            self.rooms.clone()
        }
        fn direct_messages(&self) -> Vec<DirectMessageSnapshot> {
            self.dms.clone()
        }
        fn unread(&self) -> Vec<UnreadSnapshot> {
            self.unread.clone()
        }
        fn activity(&self) -> Vec<ActivitySnapshot> {
            self.activity.clone()
        }
        fn network_health(&self) -> Option<NetworkHealthSnapshot> {
            self.health.clone()
        }
        fn transfers(&self) -> Vec<TransferSummary> {
            self.transfers.clone()
        }
    }

    #[test]
    fn projection_populates_every_field_without_mutating_source() {
        let source = FixtureSnapshotSource {
            identity: Some(IdentitySnapshot {
                id: "peer-1".into(),
                display_name: Some("A".into()),
            }),
            profile: Some(ProfileSnapshot {
                id: "profile-1".into(),
                display_name: "A".into(),
                description: Some("bio".into()),
            }),
            rooms: vec![RoomSnapshot {
                id: "room-1".into(),
                name: "General".into(),
                topic: Some("topic".into()),
                participant_count: Some(2),
                last_activity_at_ms: Some(10),
            }],
            dms: vec![DirectMessageSnapshot {
                id: "dm-1".into(),
                peer_id: "peer-2".into(),
                peer_name: "B".into(),
                last_message_preview: Some("hello".into()),
                last_activity_at_ms: Some(11),
            }],
            unread: vec![UnreadSnapshot {
                conversation_id: "room-1".into(),
                count: 3,
            }],
            activity: vec![ActivitySnapshot {
                id: "event-1".into(),
                kind: "message".into(),
                summary: "new message".into(),
                occurred_at_ms: 12,
                conversation_id: Some("room-1".into()),
            }],
            health: Some(NetworkHealthSnapshot {
                ready: true,
                status: "connected".into(),
                connected_peer_count: Some(1),
                relay_url: None,
            }),
            transfers: vec![TransferSummary {
                id: "transfer-1".into(),
                direction: "download".into(),
                file_name: "file.bin".into(),
                state: "complete".into(),
                transferred_bytes: 5,
                total_bytes: Some(5),
            }],
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

    #[test]
    fn unavailable_source_projects_contract_defaults() {
        let snapshot = AppSnapshot::project(&UnavailableSnapshotSource);

        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.identity, None);
        assert_eq!(snapshot.active_profile, None);
        assert!(snapshot.rooms.is_empty());
        assert!(snapshot.direct_messages.is_empty());
        assert!(snapshot.unread.is_empty());
        assert!(snapshot.activity.is_empty());
        assert_eq!(snapshot.network_health, None);
        assert!(snapshot.transfers.is_empty());
    }

    #[test]
    fn projection_preserves_empty_and_disconnected_fixture_values() {
        let source = FixtureSnapshotSource {
            identity: None,
            profile: None,
            rooms: Vec::new(),
            dms: Vec::new(),
            unread: Vec::new(),
            activity: Vec::new(),
            health: Some(NetworkHealthSnapshot {
                ready: false,
                status: "disconnected".into(),
                connected_peer_count: Some(0),
                relay_url: None,
            }),
            transfers: Vec::new(),
        };

        let snapshot = AppSnapshot::project(&source);

        assert_eq!(snapshot.identity, None);
        assert_eq!(snapshot.active_profile, None);
        assert!(snapshot.rooms.is_empty());
        assert!(snapshot.direct_messages.is_empty());
        assert!(snapshot.unread.is_empty());
        assert!(snapshot.activity.is_empty());
        assert_eq!(snapshot.network_health.as_ref().unwrap().ready, false);
        assert_eq!(
            snapshot
                .network_health
                .as_ref()
                .unwrap()
                .connected_peer_count,
            Some(0)
        );
        assert!(snapshot.transfers.is_empty());
    }

    // ---- Runtime projection mapping (authoritative backend sources) ----

    use crate::proto::TopicId;

    fn group_entry(seed: u8, name: &str, seen_at: u64, unread: u32) -> ConversationEntry {
        let mut entry = ConversationEntry::new_group(TopicId::from_bytes([seed; 32]), name);
        entry.description = "shared topic".to_string();
        entry.last_seen_at_unix_ms = seen_at;
        entry.unread_count = unread;
        entry
    }

    fn direct_entry(seed: u8, peer: &str, name: &str, seen_at: u64) -> ConversationEntry {
        let mut entry = ConversationEntry::new(TopicId::from_bytes([seed; 32]), peer, name);
        entry.last_seen_at_unix_ms = seen_at;
        entry.last_message_preview = "hello there".to_string();
        entry
    }

    #[test]
    fn runtime_projection_populates_rooms_from_group_conversations() {
        let entries = vec![
            group_entry(1, "General", 10, 0),
            group_entry(2, "Standup", 30, 0),
            direct_entry(3, "peer-3", "B", 20),
            {
                let mut archived = group_entry(4, "Old", 5, 0);
                archived.archived = true;
                archived
            },
        ];

        let rooms = project_rooms(&entries, &std::collections::HashMap::new());

        assert_eq!(rooms.len(), 2, "only active group conversations are rooms");
        assert_eq!(rooms[0].name, "Standup", "newest activity first");
        assert_eq!(rooms[1].name, "General");
        assert_eq!(rooms[0].topic.as_deref(), Some("shared topic"));
        assert_eq!(rooms[0].id, TopicId::from_bytes([2u8; 32]).to_string());
        assert_eq!(rooms[0].last_activity_at_ms, Some(30));
        assert_eq!(rooms[0].participant_count, None);
    }

    #[test]
    fn runtime_projection_populates_live_room_participant_count() {
        let entry = group_entry(9, "Live", 10, 0);
        let mut members = std::collections::HashMap::new();
        members.insert(
            entry.topic,
            [iroh::EndpointId::from(iroh::SecretKey::generate().public())]
                .into_iter()
                .collect(),
        );

        let rooms = project_rooms(std::slice::from_ref(&entry), &members);
        assert_eq!(rooms[0].participant_count, Some(1));

        // An unjoined room has unknown membership, not a known zero count.
        let unjoined = project_rooms(std::slice::from_ref(&entry), &std::collections::HashMap::new());
        assert_eq!(unjoined[0].participant_count, None);
    }

    #[test]
    fn runtime_projection_populates_direct_messages_from_direct_conversations() {
        let mut unnamed = direct_entry(7, "peer-7", "", 40);
        unnamed.last_message_preview = String::new();
        let entries = vec![
            direct_entry(5, "peer-5", "Alice", 10),
            unnamed,
            group_entry(6, "General", 20, 0),
        ];

        let messages = project_direct_messages(&entries);

        assert_eq!(messages.len(), 2, "only direct conversations are DMs");
        assert_eq!(messages[0].peer_name, "peer-7", "empty name falls back");
        assert_eq!(messages[0].last_message_preview, None);
        assert_eq!(messages[1].peer_name, "Alice");
        assert_eq!(messages[1].peer_id, "peer-5");
        assert_eq!(
            messages[1].last_message_preview.as_deref(),
            Some("hello there")
        );
        assert_eq!(messages[1].last_activity_at_ms, Some(10));
    }

    #[test]
    fn runtime_projection_populates_unread_counts() {
        let mut archived = group_entry(9, "Old", 1, 4);
        archived.archived = true;
        let entries = vec![
            group_entry(8, "General", 10, 3),
            direct_entry(9, "peer-9", "B", 20),
            archived,
        ];

        let unread = project_unread(&entries, |_| None);

        assert_eq!(unread.len(), 1, "zero and archived counts are omitted");
        assert_eq!(unread[0].count, 3);
        assert_eq!(
            unread[0].conversation_id,
            TopicId::from_bytes([8u8; 32]).to_string()
        );
    }

    #[test]
    fn runtime_projection_prefers_store_unread_over_catalog_count() {
        let mut direct = direct_entry(12, "peer-12", "B", 20);
        direct.unread_count = 2;
        let entries = vec![group_entry(11, "Room", 10, 0), direct];
        let room_topic = TopicId::from_bytes([11u8; 32]);

        // The store is authoritative for the room; the direct chat has no store
        // row and keeps its catalog count.
        let unread = project_unread(&entries, |entry| (entry.topic == room_topic).then_some(4));

        assert_eq!(unread.len(), 2);
        assert_eq!(unread[0].count, 4, "store count overrides the catalog count");
        assert_eq!(unread[0].conversation_id, room_topic.to_string());
        assert_eq!(unread[1].count, 2, "missing store row falls back to the catalog");
    }

    fn transfer_record(
        id: &str,
        direction: TransferDirection,
        state: TransferState,
        bytes: u64,
    ) -> TransferRecord {
        TransferRecord {
            transfer_id: id.to_string(),
            item_id: format!("file-{id}"),
            direction,
            peer_id: Some("peer-1".to_string()),
            bytes,
            total_bytes: Some(bytes + 1),
            state,
            started_at_ms: 1,
            updated_at_ms: 2,
            error: None,
            attempt: 1,
        }
    }

    #[test]
    fn runtime_projection_populates_transfer_summaries() {
        let records = vec![
            transfer_record(
                "t1",
                TransferDirection::Inbound,
                TransferState::Completed,
                10,
            ),
            transfer_record("t2", TransferDirection::Outbound, TransferState::Active, 5),
        ];

        let transfers = project_transfers(&records);

        assert_eq!(transfers.len(), 2);
        assert_eq!(transfers[0].direction, "download");
        assert_eq!(transfers[0].state, "completed");
        assert_eq!(transfers[0].file_name, "file-t1");
        assert_eq!(transfers[0].transferred_bytes, 10);
        assert_eq!(transfers[0].total_bytes, Some(11));
        assert_eq!(transfers[1].direction, "upload");
        assert_eq!(transfers[1].state, "active");
    }

    #[test]
    fn runtime_projection_populates_active_profile() {
        let mut profile = UserProfile::new(iroh::SecretKey::from_bytes(&[7u8; 32]).public());
        profile.display_name = "Dan".to_string();
        profile.bio = "tester".to_string();

        let snapshot = project_profile(&profile);

        assert_eq!(snapshot.id, profile.user_id.to_string());
        assert_eq!(snapshot.display_name, "Dan");
        assert_eq!(snapshot.description.as_deref(), Some("tester"));

        profile.bio = String::new();
        assert_eq!(project_profile(&profile).description, None);
    }

    #[test]
    fn activity_projection_exposes_latest_room_summary_and_timestamp() {
        let mut room = group_entry(12, "General", 10, 0);
        room.last_message_preview = "latest room message".into();
        let activity = project_activity(&[room]);
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].summary, "latest room message");
        assert_eq!(activity[0].occurred_at_ms, 10);
        assert_eq!(
            activity[0].conversation_id.as_deref(),
            Some(TopicId::from_bytes([12; 32]).to_string().as_str())
        );
    }

    #[test]
    fn activity_projection_documents_empty_default() {
        assert!(project_activity(&[]).is_empty());
    }
}
