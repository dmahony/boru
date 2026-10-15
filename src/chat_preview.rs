//! Shared formatting for bounded chat-message previews.

/// Keep at most 60 user-perceived characters, then append an ellipsis.
/// Grapheme boundaries keep combining sequences and joined emoji intact.
pub fn format_chat_preview(text: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;

    let mut graphemes = text.graphemes(true);
    let prefix: String = graphemes.by_ref().take(60).collect();
    if graphemes.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::format_chat_preview;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn preview_is_unicode_safe_and_grapheme_bounded() {
        let cases = [
            String::new(),
            "short\ttext\n".to_owned(),
            "a".repeat(60),
            format!("{}x", "a".repeat(61)),
            format!("{}é-tail", "a".repeat(59)),
            format!("{}界-tail", "a".repeat(59)),
            format!("{}😀-tail", "a".repeat(59)),
            format!("{}e\u{301}-tail", "a".repeat(59)),
            format!("{}👩‍👩‍👧‍👦-tail", "a".repeat(59)),
        ];
        for text in cases {
            let preview = format_chat_preview(&text);
            assert!(std::str::from_utf8(preview.as_bytes()).is_ok());
            assert_eq!(
                preview.ends_with('…'),
                text.graphemes(true).count() > 60,
                "{text:?}"
            );
            assert!(preview.graphemes(true).count() <= 61);
        }
        assert_eq!(
            format_chat_preview(&format!("{}é", "a".repeat(59))),
            format!("{}é", "a".repeat(59))
        );
        assert_eq!(
            format_chat_preview(&format!("{}éx", "a".repeat(59))),
            format!("{}é…", "a".repeat(59))
        );
    }
}
