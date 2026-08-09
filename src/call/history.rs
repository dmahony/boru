//! Local call-history metadata formatting.
//!
//! Call history deliberately contains only a text event.  Media, call IDs,
//! peer addresses, and negotiation payloads are never persisted here.

use std::time::Duration;

use super::CallKind;

/// Terminal reason used to decide which local history marker to create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallHistoryOutcome {
    /// A call became active and later ended.
    Completed,
    /// An incoming call ended before acceptance.
    Missed,
    /// A call was explicitly declined.
    Declined,
    /// Negotiation failed without being accepted or explicitly declined.
    Failed,
}

/// Format an active-call duration as `Xm Ys` (or `Ys` for sub-minute calls).
pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

/// Return the local conversation text for a terminal call event.
///
/// `duration` must be `Some` only when the call emitted `CallEvent::Active`.
/// A failed negotiation is intentionally omitted, while missed and declined
/// calls are retained as metadata without pretending media was established.
pub fn event_text(
    kind: CallKind,
    outcome: CallHistoryOutcome,
    duration: Option<Duration>,
) -> Option<String> {
    let label = kind.label();
    match (outcome, duration) {
        (CallHistoryOutcome::Completed, Some(duration)) => {
            Some(format!("{label} call • {}", format_duration(duration)))
        }
        (CallHistoryOutcome::Missed, None) => Some(format!("Missed {label_lower} call", label_lower = label.to_lowercase())),
        (CallHistoryOutcome::Declined, None) => {
            Some(format!("Declined {label_lower} call", label_lower = label.to_lowercase()))
        }
        (CallHistoryOutcome::Failed, _) | (_, Some(_)) => None,
        (CallHistoryOutcome::Completed, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{event_text, format_duration, CallHistoryOutcome};
    use crate::call::CallKind;
    use std::time::Duration;

    #[test]
    fn active_voice_call_records_duration() {
        assert_eq!(
            event_text(
                CallKind::Voice,
                CallHistoryOutcome::Completed,
                Some(Duration::from_secs(272)),
            ),
            Some("Voice call • 4m 32s".to_string())
        );
    }

    #[test]
    fn non_active_failed_call_is_not_recorded() {
        assert_eq!(
            event_text(CallKind::Video, CallHistoryOutcome::Failed, None),
            None
        );
    }

    #[test]
    fn missed_and_declined_variants_are_recorded() {
        assert_eq!(
            event_text(CallKind::Voice, CallHistoryOutcome::Missed, None),
            Some("Missed voice call".to_string())
        );
        assert_eq!(
            event_text(CallKind::Video, CallHistoryOutcome::Declined, None),
            Some("Declined video call".to_string())
        );
    }

    #[test]
    fn duration_formatting_handles_subminute_and_exact_minute() {
        assert_eq!(format_duration(Duration::from_secs(9)), "9s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m 0s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "61m 1s");
    }
}
