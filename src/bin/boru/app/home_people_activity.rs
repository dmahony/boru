//! People & Activity projection helpers for the Home panel.
//!
//! The panel is intentionally a read-only projection of FriendsStore,
//! presence, cached profile images, and the notification activity ring. This
//! module contains identity logic that must remain stable while Iced rebuilds
//! the surrounding card.

use super::*;

/// Resolve the width passed to the live People & Activity renderer.
pub(crate) fn allocated_width(
    inner_width: f32,
    columns: usize,
    stack_breakpoint: f32,
    gap: f32,
) -> f32 {
    if columns > 1 && inner_width >= stack_breakpoint {
        ((inner_width - gap) / 2.0).max(0.0)
    } else {
        inner_width
    }
}

/// Presence states represented by the panel's `online/total` count.
/// Away is reachable; transitional discovery states are not evidence of
/// availability.
pub(crate) fn counts_as_available(presence: PeerPresence) -> bool {
    matches!(presence, PeerPresence::Online | PeerPresence::Away)
}

/// Whether the avatar should carry the live-presence marker.
pub(crate) fn shows_presence_dot(presence: PeerPresence) -> bool {
    counts_as_available(presence)
}

/// Stable identity for a legacy activity event that predates explicit IDs.
/// Source content is retained in the ID so repeated renders never manufacture
/// a different event identity.
pub(crate) fn activity_event_id(event: &RecentActivityEvent) -> String {
    activity_event_id_with_occurrence(event, 0)
}

/// Stable identity for a source event, including its occurrence among
/// otherwise identical records. The occurrence is supplied by the projection
/// so duplicate activity entries remain distinct without adding a protocol
/// field or inventing an ID in the notification producer.
pub(crate) fn activity_event_id_with_occurrence(
    event: &RecentActivityEvent,
    occurrence: usize,
) -> String {
    let millis = event
        .timestamp
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!(
        "activity:{millis}:{}:{}:{occurrence}",
        event.kind as u8, event.description
    )
}

/// Project the notification ring into the bounded, newest-first feed shown
/// on Home. This pure seam keeps count and presentation filtering testable
/// without constructing the network-backed `IcedChat` state.
pub(crate) fn project_activity_rows<'a, I>(events: I) -> Vec<ActivityRow>
where
    I: IntoIterator<Item = &'a RecentActivityEvent>,
{
    let mut occurrences = std::collections::HashMap::<String, usize>::new();
    let mut rows: Vec<_> = events
        .into_iter()
        .map(|event| {
            let base = activity_event_id(event);
            let occurrence = occurrences.entry(base).or_default();
            let id = activity_event_id_with_occurrence(event, *occurrence);
            *occurrence += 1;
            ActivityRow {
                id,
                description: event.description.clone(),
                kind: event.kind,
                timestamp: event.timestamp,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));
    rows.truncate(15);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_identity_is_repeatable_and_content_bound() {
        let event = RecentActivityEvent::with_kind("A file arrived", ActivityKind::FileShared);
        let first = activity_event_id(&event);
        assert_eq!(first, activity_event_id(&event));
        assert!(first.contains("A file arrived"));
    }

    #[test]
    fn different_activity_details_do_not_share_identity() {
        let first = RecentActivityEvent::new("first");
        let second = RecentActivityEvent::new("second");
        assert_ne!(activity_event_id(&first), activity_event_id(&second));
    }

    #[test]
    fn projection_keeps_identical_events_distinct_and_caps_feed() {
        let mut events = Vec::new();
        for _ in 0..16 {
            events.push(RecentActivityEvent {
                description: "same activity".to_string(),
                timestamp: std::time::UNIX_EPOCH,
                kind: ActivityKind::Message,
            });
        }
        let rows = project_activity_rows(&events);
        assert_eq!(rows.len(), 15);
        assert_eq!(
            rows.iter()
                .map(|row| &row.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            15
        );
        assert!(rows.iter().all(|row| row.description == "same activity"));
    }

    #[test]
    fn allocated_width_matches_wide_and_stacked_layouts() {
        assert_eq!(allocated_width(1200.0, 2, 720.0, 24.0), 588.0);
        assert_eq!(allocated_width(700.0, 2, 720.0, 24.0), 700.0);
        assert_eq!(allocated_width(400.0, 1, 720.0, 24.0), 400.0);
    }

    #[test]
    fn only_online_and_away_count_as_available() {
        assert!(counts_as_available(PeerPresence::Online));
        assert!(counts_as_available(PeerPresence::Away));
        assert!(!counts_as_available(PeerPresence::Connecting));
        assert!(!counts_as_available(PeerPresence::RecentlySeen));
        assert!(!counts_as_available(PeerPresence::Offline));
        assert!(!counts_as_available(PeerPresence::Unknown));
    }

    #[test]
    fn presence_dot_matches_available_states() {
        assert!(shows_presence_dot(PeerPresence::Online));
        assert!(shows_presence_dot(PeerPresence::Away));
        assert!(!shows_presence_dot(PeerPresence::Offline));
        assert!(!shows_presence_dot(PeerPresence::Connecting));
    }
}
