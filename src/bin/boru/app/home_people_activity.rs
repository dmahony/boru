//! People & Activity projection helpers for the Home panel.
//!
//! The panel is intentionally a read-only projection of FriendsStore,
//! presence, cached profile images, and the notification activity ring. This
//! module contains identity logic that must remain stable while Iced rebuilds
//! the surrounding card.

use super::*;

/// Stable identity for a legacy activity event that predates explicit IDs.
/// Source content is retained in the ID so repeated renders never manufacture
/// a different event identity.
pub(crate) fn activity_event_id(event: &RecentActivityEvent) -> String {
    let millis = event
        .timestamp
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!(
        "activity:{millis}:{}:{}",
        event.kind as u8, event.description
    )
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
}
