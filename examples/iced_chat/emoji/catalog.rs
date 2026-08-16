//! Emoji metadata and categories (BORU-TWEMOJI-04 skeleton).
//!
//! This module owns the *metadata* side of emoji: the Unicode value that goes
//! into messages, the display name, the picker category, and the presentation
//! asset key. The full catalog model (generated from the vendored Twemoji set)
//! lands in BORU-TWEMOJI-05; today this provides the same curated list the
//! picker historically hard-coded, plus the category enum the picker will use
//! for navigation (BORU-TWEMOJI-12).
//!
//! Guardrail: `unicode` is the only value that ever enters a chat message.
//! `asset` is presentation metadata only — never transmitted, never stored as
//! message content.

/// Picker categories (PDF Task 12: Smileys & People, Animals & Nature,
/// Food & Drink, Activities, Travel & Places, Objects, Symbols, Flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmojiCategory {
    SmileysAndPeople,
    AnimalsAndNature,
    FoodAndDrink,
    Activities,
    TravelAndPlaces,
    Objects,
    Symbols,
    Flags,
}

impl EmojiCategory {
    /// All categories in picker display order.
    pub const ALL: [EmojiCategory; 8] = [
        EmojiCategory::SmileysAndPeople,
        EmojiCategory::AnimalsAndNature,
        EmojiCategory::FoodAndDrink,
        EmojiCategory::Activities,
        EmojiCategory::TravelAndPlaces,
        EmojiCategory::Objects,
        EmojiCategory::Symbols,
        EmojiCategory::Flags,
    ];

    /// Stable display-name key for localization (consumed via `i18n::t`).
    pub fn label_key(self) -> &'static str {
        match self {
            EmojiCategory::SmileysAndPeople => "emoji.category.smileys_people",
            EmojiCategory::AnimalsAndNature => "emoji.category.animals_nature",
            EmojiCategory::FoodAndDrink => "emoji.category.food_drink",
            EmojiCategory::Activities => "emoji.category.activities",
            EmojiCategory::TravelAndPlaces => "emoji.category.travel_places",
            EmojiCategory::Objects => "emoji.category.objects",
            EmojiCategory::Symbols => "emoji.category.symbols",
            EmojiCategory::Flags => "emoji.category.flags",
        }
    }
}

/// A single emoji catalog record.
///
/// `unicode` is what the picker inserts into the composer and what messages
/// carry on the wire. `asset` is a presentation-only Twemoji asset key
/// (e.g. `"1f600"`) used by [`crate::emoji::renderer`] to find the bundled
/// SVG; it is never part of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emoji {
    /// The Unicode string sent/stored in chat (may be multi-codepoint:
    /// variation selectors, ZWJ sequences, flags, skin tones).
    pub unicode: &'static str,
    /// Display name (English; localized search keywords arrive in
    /// BORU-TWEMOJI-05/13).
    pub name: &'static str,
    /// Picker category.
    pub category: EmojiCategory,
    /// Search keywords (empty in the skeleton; filled by BORU-TWEMOJI-05/13).
    pub keywords: &'static [&'static str],
    /// Twemoji asset key, presentation metadata only.
    pub asset: &'static str,
}

/// The curated common emoji list the picker shows by default.
///
/// This is the same 40 entries the picker historically hard-coded
/// (`app/chat.rs` `view_emoji_picker`), moved here so the picker sources its
/// data from the catalog. The full generated catalog replaces this in
/// BORU-TWEMOJI-05.
pub const COMMON_EMOJIS: &[Emoji] = &[
    Emoji {
        unicode: "😀",
        name: "grinning face",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f600",
    },
    Emoji {
        unicode: "😂",
        name: "face with tears of joy",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f602",
    },
    Emoji {
        unicode: "🤣",
        name: "rolling on the floor laughing",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f923",
    },
    Emoji {
        unicode: "😊",
        name: "smiling face with smiling eyes",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f60a",
    },
    Emoji {
        unicode: "😍",
        name: "smiling face with heart-eyes",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f60d",
    },
    Emoji {
        unicode: "🥰",
        name: "smiling face with hearts",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f970",
    },
    Emoji {
        unicode: "😘",
        name: "face blowing a kiss",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f618",
    },
    Emoji {
        unicode: "😜",
        name: "winking face with tongue",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f61c",
    },
    Emoji {
        unicode: "🤔",
        name: "thinking face",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f914",
    },
    Emoji {
        unicode: "🙄",
        name: "face with rolling eyes",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f644",
    },
    Emoji {
        unicode: "😢",
        name: "crying face",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f622",
    },
    Emoji {
        unicode: "😭",
        name: "loudly crying face",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f62d",
    },
    Emoji {
        unicode: "😤",
        name: "face with steam from nose",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f624",
    },
    Emoji {
        unicode: "😡",
        name: "pouting face",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f621",
    },
    Emoji {
        unicode: "🥺",
        name: "pleading face",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f97a",
    },
    Emoji {
        unicode: "😎",
        name: "smiling face with sunglasses",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f60e",
    },
    Emoji {
        unicode: "🤩",
        name: "star-struck",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f929",
    },
    Emoji {
        unicode: "👍",
        name: "thumbs up",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f44d",
    },
    Emoji {
        unicode: "👎",
        name: "thumbs down",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f44e",
    },
    Emoji {
        unicode: "👏",
        name: "clapping hands",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f44f",
    },
    Emoji {
        unicode: "🙌",
        name: "raising hands",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f64c",
    },
    Emoji {
        unicode: "💪",
        name: "flexed biceps",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f4aa",
    },
    Emoji {
        unicode: "🤝",
        name: "handshake",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &[],
        asset: "1f91d",
    },
    Emoji {
        unicode: "❤️",
        name: "red heart",
        category: EmojiCategory::Symbols,
        keywords: &[],
        asset: "2764",
    },
    Emoji {
        unicode: "🔥",
        name: "fire",
        category: EmojiCategory::TravelAndPlaces,
        keywords: &[],
        asset: "1f525",
    },
    Emoji {
        unicode: "⭐",
        name: "star",
        category: EmojiCategory::TravelAndPlaces,
        keywords: &[],
        asset: "2b50",
    },
    Emoji {
        unicode: "🎉",
        name: "party popper",
        category: EmojiCategory::Activities,
        keywords: &[],
        asset: "1f389",
    },
    Emoji {
        unicode: "✨",
        name: "sparkles",
        category: EmojiCategory::Activities,
        keywords: &[],
        asset: "2728",
    },
    Emoji {
        unicode: "💯",
        name: "hundred points",
        category: EmojiCategory::Symbols,
        keywords: &[],
        asset: "1f4af",
    },
    Emoji {
        unicode: "✅",
        name: "check mark button",
        category: EmojiCategory::Symbols,
        keywords: &[],
        asset: "2705",
    },
    Emoji {
        unicode: "❌",
        name: "cross mark",
        category: EmojiCategory::Symbols,
        keywords: &[],
        asset: "274c",
    },
    Emoji {
        unicode: "⚠️",
        name: "warning",
        category: EmojiCategory::Symbols,
        keywords: &[],
        asset: "26a0",
    },
    Emoji {
        unicode: "💡",
        name: "light bulb",
        category: EmojiCategory::Objects,
        keywords: &[],
        asset: "1f4a1",
    },
    Emoji {
        unicode: "📌",
        name: "pushpin",
        category: EmojiCategory::Objects,
        keywords: &[],
        asset: "1f4cc",
    },
    Emoji {
        unicode: "🎵",
        name: "musical note",
        category: EmojiCategory::Activities,
        keywords: &[],
        asset: "1f3b5",
    },
    Emoji {
        unicode: "🌈",
        name: "rainbow",
        category: EmojiCategory::TravelAndPlaces,
        keywords: &[],
        asset: "1f308",
    },
    Emoji {
        unicode: "🍕",
        name: "pizza",
        category: EmojiCategory::FoodAndDrink,
        keywords: &[],
        asset: "1f355",
    },
    Emoji {
        unicode: "☕",
        name: "hot beverage",
        category: EmojiCategory::FoodAndDrink,
        keywords: &[],
        asset: "2615",
    },
    Emoji {
        unicode: "🕐",
        name: "one o'clock",
        category: EmojiCategory::TravelAndPlaces,
        keywords: &[],
        asset: "1f550",
    },
    Emoji {
        unicode: "💤",
        name: "zzz",
        category: EmojiCategory::Symbols,
        keywords: &[],
        asset: "1f4a4",
    },
];

/// All curated common emojis (picker default list).
pub fn common_emojis() -> &'static [Emoji] {
    COMMON_EMOJIS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_emojis_are_unique() {
        let list = common_emojis();
        assert!(!list.is_empty());
        for (i, a) in list.iter().enumerate() {
            for b in list.iter().skip(i + 1) {
                assert_ne!(a.unicode, b.unicode, "duplicate unicode entry");
                assert_ne!(a.asset, b.asset, "duplicate asset entry");
            }
        }
    }

    #[test]
    fn every_common_emoji_has_an_asset_key() {
        for e in common_emojis() {
            assert!(!e.asset.is_empty(), "missing asset for {}", e.unicode);
        }
    }

    #[test]
    fn categories_are_stable() {
        assert_eq!(EmojiCategory::ALL.len(), 8);
        assert_eq!(
            EmojiCategory::SmileysAndPeople.label_key(),
            "emoji.category.smileys_people"
        );
    }
}
