//! Text sanitisation for safe display and storage.
//!
//! Sanitises untrusted text fields (user-supplied names, descriptions,
//! message bodies, file metadata) before they reach the UI, log output,
//! or the filesystem.  The goal is to prevent rendering corruption,
//! log injection, and Unicode-based phishing/obfuscation — not HTML/JS
//! injection (iced is a native GUI, not a web renderer).
//!
//! # Guarantees
//!
//! - ASCII control characters (`U+0000`–`U+001F`, `U+007F`) are replaced
//!   with the Unicode replacement character or stripped for single-line use.
//! - Unicode directionality/format characters that can be used for
//!   obfuscation or phishing are stripped:
//!   - `U+200B`–`U+200F` (zero-width space, ZWNJ, ZWJ, LRM, RLM)
//!   - `U+2028`–`U+202F` (line/paragraph separator, LRO, RLO, PDF, LRE, RLE)
//!   - `U+2060`–`U+2064` (word joiner, invisible operators)
//!   - `U+FEFF` (BOM / zero-width no-break space)
//! - Text is truncated to a caller-specified maximum length.
//! - Unicode normalisation (NFC) is applied for consistent rendering.
//!
//! # What this does NOT prevent
//!
//! - This is **not** an HTML/JS sanitizer (not needed for a native GUI).
//! - This is **not** a filesystem sanitizer (see [`safe_destination`] for that).
//! - This does **not** prevent SQL injection (SQLite uses parameterised
//!   queries internally).
//!
//! # When to use
//!
//! - **Display text**: message body, sender label, file name, room name,
//!   description, user bio, friend label, preview snippet — use
//!   [`sanitize_display_text`].
//! - **Single-line fields** (labels, names, nicknames): use
//!   [`sanitize_single_line`] to additionally strip newlines.
//! - **Log output**: already handled by [`mcp_server::sanitize_for_log`]
//!   in the iced example — this crate-level function is for the display path.

use unicode_normalization::UnicodeNormalization;

// ── Limits ────────────────────────────────────────────────────────────────

/// Default maximum length (in Unicode characters) for display text.
/// Messages longer than this are truncated.
pub const DEFAULT_MAX_DISPLAY_LENGTH: usize = 10_000;

/// Maximum length for single-line fields (labels, names, nicknames).
pub const DEFAULT_MAX_SINGLE_LINE_LENGTH: usize = 256;

// ── Control-character handling ──────────────────────────────────────────

/// ASCII control characters (0x00–0x1F, 0x7F) that should always be removed
/// from display text.  Newline (`0x0A`), carriage return (`0x0D`), and tab
/// (`0x09`) are excluded so multi-line fields survive.
fn is_stripped_control(c: char) -> bool {
    matches!(c,
        '\u{0000}'..='\u{0008}'  | // NUL .. BS
        '\u{000B}'..='\u{000C}'  | // VT .. FF (newline-adjacent controls)
        '\u{000E}'..='\u{001F}'  | // SO .. US
        '\u{007F}'                  // DEL
    )
}

/// A superset of [`is_stripped_control`] that also strips newline-family
/// characters — appropriate for single-line fields.
fn is_stripped_control_strict(c: char) -> bool {
    is_stripped_control(c)
        || matches!(
            c,
            '\u{0009}'
                ..='\u{000D}' | // HT, LF, VT, FF, CR
            '\u{0085}' // NEL (next line)
        )
}

// ── Unicode format / directionality characters to strip ───────────────────

/// Unicode format control characters that can be used for obfuscation,
/// phishing (e.g. swapping character order with RTL override), or that
/// produce invisible output.
fn is_stripped_unicode_format(c: char) -> bool {
    matches!(
        c,
        // Zero-width and invisible formatting
        '\u{200B}' // ZERO WIDTH SPACE
        | '\u{200C}' // ZERO WIDTH NON-JOINER
        | '\u{200D}' // ZERO WIDTH JOINER
        | '\u{200E}' // LEFT-TO-RIGHT MARK
        | '\u{200F}' // RIGHT-TO-LEFT MARK
        // Line/paragraph break characters (not actual line feeds)
        | '\u{2028}' // LINE SEPARATOR
        | '\u{2029}' // PARAGRAPH SEPARATOR
        // Bidirectional text overrides
        | '\u{202A}' // LEFT-TO-RIGHT EMBEDDING
        | '\u{202B}' // RIGHT-TO-LEFT EMBEDDING
        | '\u{202C}' // POP DIRECTIONAL FORMATTING
        | '\u{202D}' // LEFT-TO-RIGHT OVERRIDE
        | '\u{202E}' // RIGHT-TO-LEFT OVERRIDE
        // Deprecated / invisible formatting
        | '\u{2060}' // WORD JOINER
        | '\u{2061}' // FUNCTION APPLICATION
        | '\u{2062}' // INVISIBLE TIMES
        | '\u{2063}' // INVISIBLE SEPARATOR
        | '\u{2064}' // INVISIBLE PLUS
        // BOM / zero-width no-break space (also legal at start of string)
        | '\u{FEFF}' // ZERO WIDTH NO-BREAK SPACE / BOM
        // Tag characters (used in special-purpose planes)
        | '\u{E0001}' // LANGUAGE TAG
        | '\u{E0020}'..='\u{E007F}' // TAG SPACE .. CANCEL TAG
    )
}

// ── Public API ────────────────────────────────────────────────────────────

/// Sanitise text for safe display in the UI.
///
/// Replaces or strips characters that could cause rendering corruption,
/// log injection, or Unicode-based phishing.  Applies NFC normalisation,
/// then filters character-by-character.
///
/// * `text` — the raw, untrusted input.
/// * `max_chars` — maximum number of Unicode characters to keep;
///   pass [`DEFAULT_MAX_DISPLAY_LENGTH`] for general display,
///   a smaller value for constrained fields like labels.
///
/// Returns sanitised text.  The output will never be longer than
/// `max_chars` characters (may be shorter after stripping).
///
/// # Examples
///
/// ```
/// use boru_core::abuse_controls::sanitize_display_text;
///
/// let clean = sanitize_display_text("hello\u{0000}world\nline2", 100);
/// assert_eq!(clean, "hello\u{FFFD}world\nline2");
/// ```
pub fn sanitize_display_text(text: &str, max_chars: usize) -> String {
    // 1. Unicode NFC normalisation for consistent rendering.
    let normalized: String = text.nfc().collect();

    // 2. Iterate and process each character.
    let mut result = String::with_capacity(normalized.len().min(max_chars));
    for c in normalized.chars() {
        if result.chars().count() >= max_chars {
            break;
        }
        if is_stripped_unicode_format(c) {
            // Strip invisible format characters entirely.
            continue;
        }
        if is_stripped_control(c) {
            // Replace dangerous control characters with the Unicode replacement
            // character so the user sees something is wrong, rather than
            // silently removing them.
            result.push('\u{FFFD}');
        } else {
            result.push(c);
        }
    }
    result
}

/// Sanitise text for a single-line display field (labels, names, nicknames).
///
/// Like [`sanitize_display_text`], but also strips newlines (`LF`, `CR`),
/// tabs, and other line-break characters, and applies a tighter length cap.
///
/// # Examples
///
/// ```
/// use boru_core::abuse_controls::sanitize_single_line;
///
/// let clean = sanitize_single_line("hello\nworld\u{0000}test");
/// assert_eq!(clean, "hello world test");
/// ```
pub fn sanitize_single_line(text: &str) -> String {
    sanitize_single_line_with_max(text, DEFAULT_MAX_SINGLE_LINE_LENGTH)
}

/// Like [`sanitize_single_line`] but with an explicit character limit.
pub fn sanitize_single_line_with_max(text: &str, max_chars: usize) -> String {
    let normalized: String = text.nfc().collect();
    let mut result = String::with_capacity(normalized.len().min(max_chars));
    for c in normalized.chars() {
        if result.chars().count() >= max_chars {
            break;
        }
        if is_stripped_unicode_format(c) {
            continue;
        }
        if is_stripped_control_strict(c) {
            // Replace controls with space for single-line, so line breaks
            // become word separators rather than disappearing entirely.
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    // Collapse consecutive spaces from replaced controls.
    let mut collapsed = String::with_capacity(result.len());
    let mut prev_space = false;
    for c in result.chars() {
        if c == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        collapsed.push(c);
    }
    collapsed
}

/// Convenience: call [`sanitize_display_text`] with the default max length.
pub fn sanitize_display(text: &str) -> String {
    sanitize_display_text(text, DEFAULT_MAX_DISPLAY_LENGTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic control character handling ──────────────────────────────

    #[test]
    fn test_replaces_null_byte() {
        let result = sanitize_display_text("hello\u{0000}world", 100);
        assert_eq!(result, "hello\u{FFFD}world");
    }

    #[test]
    fn test_strips_bell_and_backspace() {
        let result = sanitize_display_text("\u{0007}bell\u{0008}\u{0008}", 100);
        // Both U+0007 (bell) and U+0008 (backspace) are stripped controls,
        // so they become replacement characters.
        assert_eq!(result, "\u{FFFD}bell\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn test_preserves_newlines() {
        let result = sanitize_display_text("line1\nline2\r\nline3", 100);
        assert_eq!(result, "line1\nline2\r\nline3");
    }

    #[test]
    fn test_preserves_tabs() {
        let result = sanitize_display_text("col1\tcol2", 100);
        assert_eq!(result, "col1\tcol2");
    }

    #[test]
    fn test_strips_escape() {
        let result = sanitize_display_text("esc\u{001B}aped", 100);
        assert_eq!(result, "esc\u{FFFD}aped");
    }

    // ── Unicode format characters ────────────────────────────────────

    #[test]
    fn test_strips_zero_width_space() {
        let result = sanitize_display_text("hello\u{200B}world", 100);
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn test_strips_bidi_overrides() {
        let result = sanitize_display_text("a\u{202E}!b\u{202C}c", 100);
        assert_eq!(result, "a!bc");
    }

    #[test]
    fn test_strips_lrm_rlm() {
        let result = sanitize_display_text("a\u{200E}b\u{200F}c", 100);
        assert_eq!(result, "abc");
    }

    #[test]
    fn test_strips_bom() {
        let result = sanitize_display_text("\u{FEFF}hello", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_strips_invisible_operators() {
        let result = sanitize_display_text("a\u{2062}b", 100);
        assert_eq!(result, "ab");
    }

    // ── Truncation ────────────────────────────────────────────────────

    #[test]
    fn test_truncates_long_text() {
        let long = "a".repeat(200);
        let result = sanitize_display_text(&long, 10);
        assert_eq!(result.len(), 10);
        assert_eq!(result, "aaaaaaaaaa");
    }

    #[test]
    fn test_truncates_with_unicode() {
        let long = "é".repeat(20); // é (U+00E9) is a single char
        let result = sanitize_display_text(&long, 5);
        assert_eq!(result.chars().count(), 5);
    }

    #[test]
    fn test_short_text_not_truncated() {
        let result = sanitize_display_text("hello", 100);
        assert_eq!(result, "hello");
    }

    // ── Normalisation ─────────────────────────────────────────────────

    #[test]
    fn test_nfc_normalisation() {
        // "é" as combining sequence (e + combining accent) → NFC single char
        let composed: String = "e\u{0301}".to_string(); // NFD form
        let result = sanitize_display_text(&composed, 100);

        // The character count should be 1 after NFC, and the visual result
        // should be a single "é".
        assert_eq!(result.chars().count(), 1);
        assert_eq!(result, "é");
    }

    // ── Single-line sanitisation ──────────────────────────────────────

    #[test]
    fn test_single_line_strips_newlines() {
        let result = sanitize_single_line("hello\nworld\r\ntest");
        assert_eq!(result, "hello world test");
    }

    #[test]
    fn test_single_line_strips_tabs() {
        let result = sanitize_single_line("a\tb");
        assert_eq!(result, "a b");
    }

    #[test]
    fn test_single_line_collapses_spaces() {
        let result = sanitize_single_line("a\n\n\nb");
        assert_eq!(result, "a b");
    }

    #[test]
    fn test_single_line_truncates() {
        let result = sanitize_single_line_with_max("hello world", 5);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_single_line_empty() {
        assert_eq!(sanitize_single_line(""), "");
    }

    // ── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn test_empty_string() {
        assert_eq!(sanitize_display_text("", 100), "");
    }

    #[test]
    fn test_all_controls() {
        // All ASCII control chars except newline/tab
        let input: String = (0u8..=31u8)
            .chain(127u8..=127u8)
            .map(|b| b as char)
            .collect();
        let result = sanitize_display_text(&input, 1000);
        // Every char should either be preserved (LF/CR/TAB) or replaced
        for c in result.chars() {
            assert!(c == '\n' || c == '\r' || c == '\t' || c == '\u{FFFD}');
        }
        // Count the replacement chars: 0x00-08 (9) + 0x0B-0C (2) + 0x0E-0F (2) + 0x10-1F (16) + 0x7F (1) = 30
        let replacement_count = result.chars().filter(|&c| c == '\u{FFFD}').count();
        assert_eq!(replacement_count, 30);
    }

    #[test]
    fn test_max_chars_with_stripping() {
        // Input longer than max but with format chars that get stripped
        let input = "a\u{200B}b\u{200B}c\u{200B}d\u{200B}e\u{200B}f";
        let result = sanitize_display_text(input, 3);
        assert_eq!(result.chars().count(), 3);
        assert_eq!(result, "abc");
    }

    #[test]
    fn test_unicode_format_in_single_line() {
        let result = sanitize_single_line("a\u{202E}b\u{202C}c");
        assert_eq!(result, "abc");
    }

    #[test]
    fn test_preserves_normal_unicode() {
        let result = sanitize_display_text("日本語も大丈夫", 100);
        assert_eq!(result, "日本語も大丈夫");
    }

    #[test]
    fn test_preserves_emojis() {
        let result = sanitize_display_text("Hello 😀 🌍", 100);
        assert_eq!(result, "Hello 😀 🌍");
    }

    #[test]
    fn test_sanitize_display_convenience() {
        let result = sanitize_display("test\u{0000}ok");
        assert_eq!(result.chars().count(), 7);
        assert_eq!(result, "test\u{FFFD}ok");
    }
}
