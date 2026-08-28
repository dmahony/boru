#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]
#![allow(unused_variables, unused_assignments)]

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
///
/// UI-14 grouping rule: two adjacent user messages share a visual group when
/// (a) both are the same kind (Local or Remote), (b) they have the same
/// sender, and (c) the timestamps are within this window.  Grouping is purely
/// presentational — stored timestamps, sender fields, and message order are
/// never modified.  A group's first bubble carries the sender avatar; the
/// delivery/read indicator is shown on the group's last bubble.
pub(crate) const MESSAGE_GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Effective maximum width for a message bubble.
///
/// Spec (plan §4): 560 px or 68 % of the timeline width, whichever is smaller.
/// A non-positive timeline width (pre-layout frame) falls back to the 560 px
/// cap so bubbles never collapse to zero on the first frame.
pub(crate) fn chat_bubble_max_width(timeline_width: f32) -> f32 {
    chat_bubble_max_width_with(
        timeline_width,
        crate::design_tokens::CHAT_BUBBLE_MAX_WIDTH,
        crate::design_tokens::CHAT_BUBBLE_WIDTH_RATIO,
    )
}

/// Effective bubble width using the live structural chat layout.
pub(crate) fn chat_bubble_max_width_with(
    timeline_width: f32,
    max_width: f32,
    width_ratio: f32,
) -> f32 {
    if timeline_width <= 0.0 {
        return max_width;
    }
    max_width.min(timeline_width * width_ratio)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageKind {
    System,
    Local,
    Remote,
}

/// Whether two adjacent plain system chips should be visually grouped.
///
/// Both entries must be plain system notices (no download attachment — those
/// render as attachment cards, not chips). Grouping only tightens vertical
/// spacing; it never reorders or filters entries.
pub(crate) fn continues_system_group(previous_is_chip: bool, current_is_chip: bool) -> bool {
    previous_is_chip && current_is_chip
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
    let words: Vec<&str> = name.split_whitespace().collect();
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

/// Format a `SystemTime` timestamp as a short relative label.
///
/// Convenience wrapper for [`relative_time`] that accepts the `SystemTime`
/// values stored on real event streams (e.g. the landing-page activity feed)
/// instead of requiring callers to convert to Unix milliseconds first.
///
/// Timestamps before the Unix epoch are clamped to "just now": they cannot
/// represent a real past event (1970 predates the app), so formatting them
/// as a huge age would only expose a broken/placeholder clock.
pub(crate) fn relative_time_from_system(ts: SystemTime) -> String {
    let unix_ms = match ts.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => {
            return "just now".to_string();
        }
    };
    relative_time(unix_ms)
}

/// Truncate display text to at most `max_chars` characters, appending a
/// Unicode ellipsis (`…`) when the text is longer.
///
/// Operates on Unicode scalar values so multi-byte text is never split in the
/// middle of a code point. Used by list rows (e.g. Recent Activity titles)
/// where an unbounded description would break the shared row rhythm.
pub(crate) fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Truncate an activity description for compact card rendering.
///
/// Short descriptions (≤ `max_chars`) are kept intact. Longer ones are
/// truncated with an ellipsis. When the text contains a filename-like
/// pattern (a dot followed by 2–5 alphanumeric characters at a word
/// boundary), the ellipsis is placed before the extension so the caller
/// can still see what kind of file is involved (e.g.
/// `"Alice finished downloading very-long-file-name-report-final.pdf from you"`
/// becomes `"Alice finished downl…report-final.pdf from you"`).
///
/// The default `max_chars` of 75 keeps rows at roughly two lines at the
/// card's typical width with the `Body` font role (15 px Public Sans).
pub(crate) fn truncate_activity_description(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();

    // Find the LAST filename-like extension pattern in the text: a dot
    // followed by 2–5 alphanumeric characters followed by a word boundary
    // (end of string, space, quote, or punctuation).  This catches
    // descriptions like "… downloaded report.pdf from you" where the
    // extension is mid-sentence, not at the very end.
    let mut ext_at: Option<usize> = None;
    let mut ext_end: usize = total;

    // Walk backward to find `.ext` at word boundaries.
    let mut i = total;
    while i >= 3 {
        i -= 1;
        // Look for a dot at position i, followed by 2-5 alphanumeric chars,
        // followed by a word boundary.
        if chars[i] != '.' {
            continue;
        }
        let suffix_start = i + 1;
        let mut suffix_end = suffix_start;
        while suffix_end < total && chars[suffix_end].is_alphanumeric() {
            suffix_end += 1;
        }
        let suffix_len = suffix_end - suffix_start;
        if !(2..=5).contains(&suffix_len) {
            continue;
        }
        // Check word boundary after the extension: end of string, space,
        // quote, or other non-alphanumeric.
        let boundary_ok = suffix_end == total
            || chars[suffix_end] == ' '
            || chars[suffix_end] == '"'
            || chars[suffix_end] == '\''
            || chars[suffix_end] == '.'
            || chars[suffix_end] == ','
            || chars[suffix_end] == ')'
            || chars[suffix_end] == ']';
        if boundary_ok {
            ext_at = Some(i);
            ext_end = suffix_end;
            break; // last (rightmost) extension wins
        }
    }

    if let Some(ext_at) = ext_at {
        // Keep everything from the extension onward, fill the front
        // with enough chars to reach max_chars.
        let back_len = total - ext_at;
        let front_chars = max_chars.saturating_sub(back_len).saturating_sub(1); // 1 for ellipsis
        if front_chars > 0 {
            let front: String = chars[..front_chars].iter().collect();
            let back: String = chars[ext_at..].iter().collect();
            format!("{front}…{back}")
        } else {
            // Extension + suffix alone exceeds max_chars — fall back.
            truncate_with_ellipsis(text, max_chars)
        }
    } else {
        truncate_with_ellipsis(text, max_chars)
    }
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

/// Format an elapsed age (whole seconds) as a short relative label.
///
/// Mirrors [`relative_time_at`]'s thresholds for consistency between wall-clock
/// timestamps and monotonic-age event rows (mesh health events record an
/// `Instant`, so the age is captured at snapshot time rather than a unix ms).
pub(crate) fn relative_age_secs(age_secs: u64, just_now_seconds: u64) -> String {
    if age_secs < just_now_seconds {
        "just now".to_string()
    } else if age_secs < 60 {
        format!("{age_secs}s ago")
    } else if age_secs < 3_600 {
        format!("{}m ago", age_secs / 60)
    } else if age_secs < 86_400 {
        format!("{}h ago", age_secs / 3_600)
    } else {
        format!("{}d ago", age_secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn chat_bubble_max_width_caps_at_560_or_68_percent() {
        // Wide timeline: 560 px cap wins.
        assert_eq!(chat_bubble_max_width(1200.0), 560.0);
        // Medium timeline: 68 % is smaller than 560 px.
        assert!((chat_bubble_max_width(688.0) - 467.84).abs() < 0.01);
        // Narrow timeline: 68 % shrinks further.
        assert!((chat_bubble_max_width(400.0) - 272.0).abs() < 0.01);
        // Non-positive width (pre-layout frame) falls back to the cap.
        assert_eq!(chat_bubble_max_width(0.0), 560.0);
        assert_eq!(chat_bubble_max_width(-10.0), 560.0);
    }

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
    fn system_group_requires_consecutive_plain_chips() {
        assert!(continues_system_group(true, true));
        assert!(!continues_system_group(false, true));
        assert!(!continues_system_group(true, false));
        assert!(!continues_system_group(false, false));
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
    fn relative_time_from_system_delegates_to_unix_ms_formatter() {
        let two_min_ago = SystemTime::now() - Duration::from_secs(120);
        assert_eq!(relative_time_from_system(two_min_ago), "2m ago");
        let just_now = SystemTime::now();
        assert_eq!(relative_time_from_system(just_now), "just now");
    }

    #[test]
    fn relative_time_from_system_clamps_pre_epoch_timestamps() {
        // A timestamp before the Unix epoch must not panic or produce a
        // negative age; it falls back to the epoch and formats as "just now".
        let pre_epoch = UNIX_EPOCH - Duration::from_secs(5);
        assert_eq!(relative_time_from_system(pre_epoch), "just now");
    }

    #[test]
    fn truncate_with_ellipsis_keeps_short_text_untouched() {
        assert_eq!(truncate_with_ellipsis("Alice joined", 48), "Alice joined");
        assert_eq!(truncate_with_ellipsis("", 48), "");
    }

    #[test]
    fn truncate_with_ellipsis_cuts_long_text_at_char_boundary() {
        assert_eq!(
            truncate_with_ellipsis("A very long activity message that must be cut", 12),
            "A very long…"
        );
    }

    #[test]
    fn truncate_with_ellipsis_never_splits_multibyte_chars() {
        // "héllo wörld" contains é and ö (2-byte UTF-8); a byte-level slice
        // at an odd index would panic, the char-level cut must not.
        let long = "héllo wörld — this message has multibyte characters";
        let cut = truncate_with_ellipsis(long, 10);
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= 10);
        // Round-trip through lossless UTF-8 to prove the string is valid.
        assert_eq!(cut, String::from_utf8_lossy(cut.as_bytes()));
    }

    #[test]
    fn truncate_with_ellipsis_reserves_room_for_ellipsis_char() {
        // max_chars includes the ellipsis: 6 chars of text + '…' = 7 total.
        let out = truncate_with_ellipsis("abcdefghij", 7);
        assert_eq!(out, "abcdef…");
        assert_eq!(out.chars().count(), 7);
    }

    #[test]
    fn truncate_activity_preserves_short_text() {
        assert_eq!(
            truncate_activity_description("Alice came online", 75),
            "Alice came online"
        );
        assert_eq!(truncate_activity_description("", 75), "");
    }

    #[test]
    fn truncate_activity_caps_long_text_with_ellipsis() {
        let long = "A".repeat(100);
        let out = truncate_activity_description(&long, 20);
        assert!(out.chars().count() <= 20);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_activity_preserves_file_extension() {
        let desc = "Alice started downloading very-long-file-name-report-final.pdf from you";
        let out = truncate_activity_description(desc, 50);
        assert!(out.chars().count() <= 50);
        assert!(out.contains('…'), "must contain ellipsis");
        assert!(
            out.contains(".pdf from you"),
            "must preserve extension mid-sentence: got '{out}'"
        );
    }

    #[test]
    fn truncate_activity_preserves_file_extension_at_end() {
        // Extension at the end of string, over the limit: preserved.
        let desc =
            "A very long message about downloading the final version of the quarterly report.pdf";
        assert!(desc.chars().count() > 50);
        let out = truncate_activity_description(desc, 50);
        assert!(out.chars().count() <= 50);
        assert!(out.contains('…'), "must contain ellipsis");
        assert!(
            out.ends_with(".pdf"),
            "extension at end preserved: got '{out}'"
        );
    }

    #[test]
    fn truncate_activity_falls_back_when_extension_too_long() {
        // Extension alone exceeds max_chars — falls back to simple truncation.
        let desc = "file.verylongextension";
        let out = truncate_activity_description(desc, 8);
        assert!(out.chars().count() <= 8);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_activity_no_false_positive_on_dots() {
        // Dots in the middle of text, not at end, should not trigger extension logic.
        let desc = "User shared a message... waiting for response from the peer";
        let out = truncate_activity_description(desc, 30);
        assert!(out.chars().count() <= 30);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_activity_handles_unicode_and_mid_sentence_extension() {
        // Extension detection works with Unicode characters before and after the dot.
        let desc = "héllo wörld — downloaded archive.zip from peer";
        let out = truncate_activity_description(desc, 40);
        assert!(out.chars().count() <= 40);
        assert!(
            out.contains(".zip"),
            "should preserve .zip extension: got '{out}'"
        );
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
    fn relative_age_secs_matches_wall_clock_thresholds() {
        assert_eq!(relative_age_secs(4, 10), "just now");
        assert_eq!(relative_age_secs(10, 10), "10s ago");
        assert_eq!(relative_age_secs(59, 10), "59s ago");
        assert_eq!(relative_age_secs(60, 10), "1m ago");
        assert_eq!(relative_age_secs(3599, 10), "59m ago");
        assert_eq!(relative_age_secs(3600, 10), "1h ago");
        assert_eq!(relative_age_secs(86_399, 10), "23h ago");
        assert_eq!(relative_age_secs(86_400, 10), "1d ago");
        assert_eq!(relative_age_secs(172_799, 10), "1d ago");
        // Consistency with the wall-clock formatter at the same elapsed time.
        let now = 200_000_000u64;
        for elapsed_ms in [5_000u64, 30_000, 120_000, 3_700_000, 50_000_000] {
            assert_eq!(
                relative_age_secs(elapsed_ms / 1000, 10),
                relative_time_at(now.saturating_sub(elapsed_ms), now, 10),
                "age and wall-clock labels must agree at {elapsed_ms}ms"
            );
        }
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
