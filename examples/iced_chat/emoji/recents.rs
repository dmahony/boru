//! Recently-used emoji list (BORU-TWEMOJI-14).
//!
//! Pure, testable helpers for the picker's Recent section. Recents are plain
//! Unicode strings — the same graphemes that go into chat messages — stored
//! in Boru's normal local settings (`AppSettings::recent_emojis` →
//! `settings.json`). No asset keys, SVG paths, image bytes or wire-protocol
//! fields ever appear here: the picker renders each stored string through
//! the shared resolver/fallback pipeline exactly like a catalog entry.
//!
//! Ordering rules (PDF Task 14):
//! - selecting an emoji moves it to the front of the list;
//! - duplicate selections never create duplicate entries;
//! - the list is capped at [`RECENT_LIMIT`] entries (24–32 per the plan);
//! - corrupt/unknown stored entries (empty or whitespace strings, or
//!   anything that does not resolve to a bundled asset) degrade gracefully:
//!   empty strings are skipped at load/render time, unknown graphemes fall
//!   back to their original Unicode text like every other picker item.

/// Maximum number of recent emoji entries (PDF Task 14: ~24–32).
pub const RECENT_LIMIT: usize = 32;

/// Normalize a stored recents list for display/use.
///
/// Skips empty/whitespace entries (corrupt storage), deduplicates while
/// preserving first-seen order, and caps the result at [`RECENT_LIMIT`].
/// Unknown-but-valid Unicode is intentionally kept — it renders through the
/// same resolver/fallback pipeline as every other picker item, so it can
/// never break the picker.
pub fn sanitize_recents(entries: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(entries.len().min(RECENT_LIMIT));
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() || !seen.insert(entry.to_string()) {
            continue;
        }
        out.push(entry.to_string());
        if out.len() >= RECENT_LIMIT {
            break;
        }
    }
    out
}

/// Record a selection in the recent list.
///
/// Returns a NEW list with `selected` at the front, any previous occurrence
/// of the same grapheme removed (deduplication), and the whole list capped at
/// [`RECENT_LIMIT`]. An empty/whitespace selection is a no-op (returns a
/// sanitized copy of the input).
pub fn record_recent(current: &[String], selected: &str) -> Vec<String> {
    let selected = selected.trim();
    if selected.is_empty() {
        return sanitize_recents(current);
    }
    let mut next = Vec::with_capacity(RECENT_LIMIT.min(current.len() + 1));
    next.push(selected.to_string());
    for entry in sanitize_recents(current) {
        if entry == selected {
            continue;
        }
        next.push(entry);
        if next.len() >= RECENT_LIMIT {
            break;
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_of(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Acceptance: a newly selected emoji goes to the front of the list.
    #[test]
    fn record_recent_moves_selected_to_front() {
        let current = vec_of(&["😀", "😂", "🤣"]);
        let next = record_recent(&current, "❤️");
        assert_eq!(next, vec_of(&["❤️", "😀", "😂", "🤣"]));
    }

    /// Acceptance: duplicate selections do not create duplicate entries —
    /// the old copy is removed and the grapheme moves to the front.
    #[test]
    fn record_recent_deduplicates() {
        let current = vec_of(&["😀", "😂", "❤️", "🤣"]);
        let next = record_recent(&current, "😂");
        assert_eq!(next, vec_of(&["😂", "😀", "❤️", "🤣"]));
        // The re-selected grapheme appears exactly once.
        assert_eq!(next.iter().filter(|s| *s == "😂").count(), 1);
    }

    /// Acceptance: the list is capped at RECENT_LIMIT (32) — the oldest
    /// entry falls off and the newest stays at the front.
    #[test]
    fn record_recent_caps_at_limit() {
        let mut current: Vec<String> = (0..RECENT_LIMIT).map(|i| format!("e{i}")).collect();
        let next = record_recent(&current, "new");
        assert_eq!(next.len(), RECENT_LIMIT);
        assert_eq!(next[0], "new");
        // The oldest 0..31 entries survive except... check ordering: new,
        // e0..e30 (31 entries) = 32 total; e31 falls off.
        assert_eq!(next[1], "e0");
        assert_eq!(next[RECENT_LIMIT - 1], "e30");
        assert!(!next.contains(&"e31".to_string()));

        // Repeated inserts beyond the limit never exceed it.
        for i in 0..(RECENT_LIMIT * 2) {
            current = record_recent(&current, &format!("x{i}"));
            assert!(current.len() <= RECENT_LIMIT);
        }
    }

    /// Acceptance: empty/whitespace selections are no-ops (the composer
    /// never inserts them, but the helper must stay robust).
    #[test]
    fn record_recent_ignores_empty_selection() {
        let current = vec_of(&["😀", "😂"]);
        assert_eq!(record_recent(&current, ""), current);
        assert_eq!(record_recent(&current, "   "), current);
    }

    /// Corrupt/unknown stored entries do not break the picker: empty and
    /// whitespace-only strings are skipped by sanitization.
    #[test]
    fn sanitize_recents_skips_empty_entries() {
        let stored = vec_of(&["", "   ", "😀", "\t", "😂"]);
        let clean = sanitize_recents(&stored);
        assert_eq!(clean, vec_of(&["😀", "😂"]));
    }

    /// Sanitization deduplicates stored entries (defensive against
    /// hand-edited settings.json) while preserving first-seen order.
    #[test]
    fn sanitize_recents_deduplicates() {
        let stored = vec_of(&["😀", "😂", "😀", "❤️"]);
        let clean = sanitize_recents(&stored);
        assert_eq!(clean, vec_of(&["😀", "😂", "❤️"]));
    }

    /// Sanitization caps at RECENT_LIMIT, so an over-long settings.json
    /// cannot blow up the picker grid.
    #[test]
    fn sanitize_recents_caps_at_limit() {
        let stored: Vec<String> = (0..(RECENT_LIMIT * 2)).map(|i| format!("e{i}")).collect();
        let clean = sanitize_recents(&stored);
        assert_eq!(clean.len(), RECENT_LIMIT);
    }

    /// Unknown-but-valid Unicode is preserved by sanitization — the picker
    /// renders it through the fallback pipeline (BORU-TWEMOJI-20: never
    /// suppress unsupported emoji).
    #[test]
    fn sanitize_recents_keeps_unknown_unicode() {
        let stored = vec_of(&["🫩", "😀"]);
        let clean = sanitize_recents(&stored);
        assert_eq!(clean, vec_of(&["🫩", "😀"]));
    }
}
