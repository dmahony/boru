//! Shared presentation helpers used by the Iced views.
//!
//! Keep display rules here rather than reimplementing them in each sidebar,
//! dashboard, and profile view. These functions are deliberately data-only so
//! they remain easy to test and do not couple formatting to widget lifetimes.

use iced::Color;
use std::time::{SystemTime, UNIX_EPOCH};

/// Messages from the same sender stay in one visual group for this long.
/// Keeping this rule in the presentation layer means replayed history and live
/// delivery get identical grouping without changing the stored message data.
pub(crate) const MESSAGE_GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageKind {
    System,
    Local,
    Remote,
}

/// Whether two adjacent entries can share a sender/avatar treatment.
pub(crate) fn continues_message_group(
    previous_kind: MessageKind,
    current_kind: MessageKind,
    previous_sender: Option<&str>,
    current_sender: Option<&str>,
    previous_timestamp_ms: Option<i64>,
    current_timestamp_ms: Option<i64>,
) -> bool {
    if matches!(previous_kind, MessageKind::System)
        || matches!(current_kind, MessageKind::System)
        || previous_kind != current_kind
    {
        return false;
    }
    if previous_kind == MessageKind::Local {
        // Local entries belong to the current user, even when older data has
        // no sender key attached.
        if previous_sender != current_sender
            && previous_sender.is_some()
            && current_sender.is_some()
        {
            return false;
        }
    } else if previous_sender != current_sender {
        return false;
    }
    let (Some(previous), Some(current)) = (previous_timestamp_ms, current_timestamp_ms) else {
        return false;
    };
    current.abs_diff(previous) <= MESSAGE_GROUP_WINDOW_MS as u64
}

/// Return a stable day key for date-divider comparisons.
pub(crate) fn day_key(timestamp_ms: Option<i64>) -> Option<i64> {
    timestamp_ms.map(|timestamp| timestamp.div_euclid(86_400_000))
}

/// Format the label used by date dividers in the chat log.
pub(crate) fn date_divider_label(timestamp_ms: i64, today_day: i64) -> String {
    let day = timestamp_ms.div_euclid(86_400_000);
    match today_day.saturating_sub(day) {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        _ => chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
            .map(|date| date.format("%A, %B %-d, %Y").to_string())
            .unwrap_or_else(|| "Earlier".to_string()),
    }
}

/// Accessible delivery copy for the metadata row.  These labels intentionally
/// avoid exposing protocol-specific state names such as `Queued` or `Seen`.
pub(crate) fn delivery_label(state: &boru_core::chat_history::DeliveryState) -> &'static str {
    use boru_core::chat_history::DeliveryState;
    match state {
        DeliveryState::Queued => "Sending",
        DeliveryState::Sent => "Sent",
        DeliveryState::Delivered => "Delivered",
        DeliveryState::Seen => "Read",
        DeliveryState::Failed => "Failed",
    }
}

/// Generate up-to-two-letter initials from a display name.
///
/// Empty names and names without alphabetic characters return an empty string;
/// callers can choose their own accessible fallback (usually `?`).
pub(crate) fn initials(name: &str) -> String {
    let words: Vec<&str> = name.trim().split_whitespace().collect();
    match words.as_slice() {
        [] => String::new(),
        [word] => {
            let chars: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
            match chars.as_slice() {
                [] => String::new(),
                [first] => first.to_uppercase().to_string(),
                [first, second, ..] => format!("{first}{second}").to_uppercase(),
            }
        }
        [first_word, second_word, ..] => {
            let first = first_word.chars().find(|c| c.is_alphabetic());
            let second = second_word.chars().find(|c| c.is_alphabetic());
            match (first, second) {
                (Some(first), Some(second)) => format!("{first}{second}").to_uppercase(),
                (Some(first), None) => first.to_uppercase().to_string(),
                _ => String::new(),
            }
        }
    }
}

/// Deterministic avatar colour derived from a display name.
pub(crate) fn initials_color(name: &str, dark_mode: bool) -> Color {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let hue = (hasher.finish() % 360) as f32;
    let (saturation, lightness) = if dark_mode {
        (0.55, 0.55)
    } else {
        (0.45, 0.55)
    };
    // Iced exposes RGB constructors but not HSL. Convert the small HSL
    // palette locally so every avatar still gets a stable, theme-aware hue.
    let chroma: f32 = (1.0_f32 - (2.0_f32 * lightness - 1.0_f32).abs()) * saturation;
    let h = hue / 60.0;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    Color::from_rgb(r1 + m, g1 + m, b1 + m)
}

/// Format a Unix-millisecond timestamp relative to `now_ms`.
pub(crate) fn relative_time_at(unix_ms: u64, now_ms: u64, just_now_seconds: u64) -> String {
    let elapsed_secs = now_ms.saturating_sub(unix_ms) / 1000;
    if elapsed_secs < just_now_seconds {
        "just now".to_string()
    } else if elapsed_secs < 60 {
        format!("{elapsed_secs}s ago")
    } else if elapsed_secs < 3_600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3_600)
    } else {
        format!("{}d ago", elapsed_secs / 86_400)
    }
}

/// Format a Unix-millisecond timestamp as a short relative label.
pub(crate) fn relative_time(unix_ms: u64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    relative_time_at(unix_ms, now_ms, 10)
}

/// Format an optional last-seen timestamp, returning an empty label when absent.
#[expect(dead_code)]
pub(crate) fn format_last_seen(last_seen_ms: Option<u64>) -> String {
    let Some(unix_ms) = last_seen_ms else {
        return String::new();
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    relative_time_at(unix_ms, now_ms, 6)
}

/// Consistent singular/plural wording for count-based labels.
#[expect(dead_code)]
pub(crate) fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_groups_require_kind_sender_and_time_window() {
        assert!(continues_message_group(
            MessageKind::Remote,
            MessageKind::Remote,
            Some("alice"),
            Some("alice"),
            Some(1_000),
            Some(301_000),
        ));
        assert!(!continues_message_group(
            MessageKind::Remote,
            MessageKind::Remote,
            Some("alice"),
            Some("bob"),
            Some(1_000),
            Some(2_000),
        ));
        assert!(!continues_message_group(
            MessageKind::Remote,
            MessageKind::Remote,
            Some("alice"),
            Some("alice"),
            Some(1_000),
            Some(301_001),
        ));
        assert!(!continues_message_group(
            MessageKind::System,
            MessageKind::System,
            None,
            None,
            Some(1_000),
            Some(2_000),
        ));
    }

    #[test]
    fn date_dividers_handle_day_boundaries_and_relative_labels() {
        assert_eq!(day_key(Some(-1)), Some(-1));
        assert_eq!(date_divider_label(2 * 86_400_000, 2), "Today");
        assert_eq!(date_divider_label(86_400_000, 2), "Yesterday");
        assert!(date_divider_label(0, 2).contains("1970"));
    }

    #[test]
    fn delivery_labels_preserve_user_facing_truth() {
        use boru_core::chat_history::DeliveryState;
        assert_eq!(delivery_label(&DeliveryState::Queued), "Sending");
        assert_eq!(delivery_label(&DeliveryState::Sent), "Sent");
        assert_eq!(delivery_label(&DeliveryState::Delivered), "Delivered");
        assert_eq!(delivery_label(&DeliveryState::Seen), "Read");
        assert_eq!(delivery_label(&DeliveryState::Failed), "Failed");
    }

    #[test]
    fn initials_cover_empty_single_and_multiple_words() {
        assert_eq!(initials(""), "");
        assert_eq!(initials("alice"), "AL");
        assert_eq!(initials("Alice Example"), "AE");
        assert_eq!(initials("123"), "");
    }

    #[test]
    fn relative_time_is_deterministic_at_boundaries() {
        let now = 200_000_000;
        assert_eq!(relative_time_at(now, now, 10), "just now");
        assert_eq!(relative_time_at(now - 10_000, now, 10), "10s ago");
        assert_eq!(relative_time_at(now - 60_000, now, 10), "1m ago");
        assert_eq!(relative_time_at(now - 3_600_000, now, 10), "1h ago");
    }

    #[test]
    fn count_label_uses_correct_grammar() {
        assert_eq!(count_label(1, "friend", "friends"), "1 friend");
        assert_eq!(count_label(2, "friend", "friends"), "2 friends");
    }

    #[test]
    fn initials_trim_and_ignore_non_letters() {
        assert_eq!(initials("  alice   example  "), "AE");
        assert_eq!(initials("123 alice"), "");
        assert_eq!(initials("!!!"), "");
    }

    #[test]
    fn initials_support_unicode_letters() {
        assert_eq!(initials("Élodie Noël"), "ÉN");
    }

    #[test]
    fn relative_time_clamps_future_timestamps() {
        assert_eq!(relative_time_at(101_000, 100_000, 10), "just now");
    }

    #[test]
    fn relative_time_uses_singular_units_without_special_cases() {
        let now = 200_000_000;
        assert_eq!(relative_time_at(now - 60_000, now, 10), "1m ago");
        assert_eq!(relative_time_at(now - 3_600_000, now, 10), "1h ago");
        assert_eq!(relative_time_at(now - 86_400_000, now, 10), "1d ago");
    }

    #[test]
    fn relative_time_handles_each_plural_boundary() {
        let now = 200_000_000;
        assert_eq!(relative_time_at(now - 59_000, now, 10), "59s ago");
        assert_eq!(relative_time_at(now - 119_000, now, 10), "1m ago");
        assert_eq!(relative_time_at(now - 7_199_000, now, 10), "1h ago");
        assert_eq!(relative_time_at(now - 172_799_000, now, 10), "1d ago");
    }

    #[test]
    fn relative_time_is_monotonic_for_older_values() {
        let now = 200_000_000;
        let recent = relative_time_at(now - 30_000, now, 10);
        let old = relative_time_at(now - 3_600_000, now, 10);
        assert_eq!(recent, "30s ago");
        assert_eq!(old, "1h ago");
    }

    #[test]
    fn initials_color_is_stable_for_same_name_and_theme() {
        assert_eq!(
            initials_color("Alice", false),
            initials_color("Alice", false)
        );
        assert_ne!(initials_color("Alice", false), initials_color("Bob", false));
    }

    #[test]
    fn initials_color_changes_theme_palette() {
        assert_ne!(
            initials_color("Alice", false),
            initials_color("Alice", true)
        );
    }

    #[test]
    fn count_label_handles_zero_and_large_counts() {
        assert_eq!(count_label(0, "message", "messages"), "0 messages");
        assert_eq!(count_label(100, "message", "messages"), "100 messages");
    }

    #[test]
    fn optional_last_seen_is_empty_when_missing() {
        assert_eq!(format_last_seen(None), "");
    }
}
