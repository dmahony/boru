//! Shared presentation helpers used by the Iced views.
//!
//! Keep display rules here rather than reimplementing them in each sidebar,
//! dashboard, and profile view. These functions are deliberately data-only so
//! they remain easy to test and do not couple formatting to widget lifetimes.

use iced::Color;
use std::time::{SystemTime, UNIX_EPOCH};

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
pub(crate) fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

#[cfg(test)]
mod tests {
    use super::*;

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
