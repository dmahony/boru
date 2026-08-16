//! Emoji metadata and categories (BORU-TWEMOJI-05 catalog model).
//!
//! This module owns the *metadata* side of emoji: the Unicode value that goes
//! into messages, the display name, the picker category, and the presentation
//! asset key. The model is defined here (BORU-TWEMOJI-05); full population of
//! every vendored Twemoji entry is driven by the generated manifest in
//! BORU-TWEMOJI-06. Today the curated list the picker historically hard-coded
//! lives here, plus a representative multi-codepoint fixture proving the
//! model handles flags, skin-tone modifiers, variation selectors and ZWJ
//! sequences (used by resolver/renderer tests in BORU-TWEMOJI-07+).
//!
//! Guardrail: `unicode` is the only value that ever enters a chat message.
//! `asset` is presentation metadata only — never transmitted, never stored as
//! message content.

/// Picker categories (PDF Task 12: Smileys & People, Animals & Nature,
/// Food & Drink, Activities, Travel & Places, Objects, Symbols, Flags).
///
/// `Recent` is a placeholder pseudo-category for the recently-used section
/// (PDF Task 14). It is intentionally excluded from [`EmojiCategory::ALL`]
/// because it is not a content category of catalog entries — recent entries
/// are dynamic user history stored as plain Unicode strings in settings.
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
    /// Recently-used section placeholder (PDF Task 14); not a catalog-entry
    /// category, so it is absent from [`EmojiCategory::ALL`].
    Recent,
}

impl EmojiCategory {
    /// All content categories in picker display order (Task 12 navigation;
    /// `Recent` is handled separately by the picker, see [`EmojiCategory`]).
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
            EmojiCategory::Recent => "emoji.category.recent",
        }
    }
}

/// A single emoji catalog record.
///
/// `unicode` is what the picker inserts into the composer and what messages
/// carry on the wire. `asset` is a presentation-only Twemoji asset key
/// (e.g. `"1f600"`) used by [`crate::emoji::renderer`] to find the bundled
/// SVG; it is never part of a chat message.
///
/// The record is fully static: every field is `&'static` data with no runtime
/// parsing, so building an entry (or scanning the catalog) costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emoji {
    /// The Unicode string sent/stored in chat. May contain multiple code
    /// points: variation selectors (U+FE0F), skin-tone modifiers (U+1F3FB..
    /// U+1F3FF), regional-indicator flag pairs, ZWJ sequences (U+200D) and
    /// tag sequences. It is a `&str`, never a single `char` — one Rust `char`
    /// is NOT one visual emoji.
    pub unicode: &'static str,
    /// Display name (English; indexed for search in BORU-TWEMOJI-13).
    pub name: &'static str,
    /// Picker category.
    pub category: EmojiCategory,
    /// Search keywords (populated from the Twemoji manifest in
    /// BORU-TWEMOJI-13; the curated entries below keep `&[]` until then).
    pub keywords: &'static [&'static str],
    /// Twemoji asset key, presentation metadata only. Bare normalized
    /// identifier (lowercase hex, `-`-joined for sequences), matching the
    /// vendored `assets/emoji/twemoji/svg/<key>.svg` filenames. Never an
    /// SVG path, URL or message payload.
    pub asset: &'static str,
}

/// The curated common emoji list the picker shows by default.
///
/// This is the same list the picker historically hard-coded
/// (`app/chat.rs` `view_emoji_picker`), moved here so the picker sources its
/// data from the catalog. BORU-TWEMOJI-12 expanded it so every content
/// category has at least one entry (the category tabs are never empty);
/// every asset key below is vendored under
/// `assets/emoji/twemoji/svg/<key>.svg`. The full generated catalog replaces
/// this in BORU-TWEMOJI-05/06.
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
    // ── Animals & Nature (BORU-TWEMOJI-12) ────────────────────────
    Emoji {
        unicode: "🐶",
        name: "dog face",
        category: EmojiCategory::AnimalsAndNature,
        keywords: &[],
        asset: "1f436",
    },
    Emoji {
        unicode: "🐱",
        name: "cat face",
        category: EmojiCategory::AnimalsAndNature,
        keywords: &[],
        asset: "1f431",
    },
    Emoji {
        unicode: "🐼",
        name: "panda",
        category: EmojiCategory::AnimalsAndNature,
        keywords: &[],
        asset: "1f43c",
    },
    Emoji {
        unicode: "🐦",
        name: "bird",
        category: EmojiCategory::AnimalsAndNature,
        keywords: &[],
        asset: "1f426",
    },
    Emoji {
        unicode: "🌸",
        name: "cherry blossom",
        category: EmojiCategory::AnimalsAndNature,
        keywords: &[],
        asset: "1f338",
    },
    Emoji {
        unicode: "🌳",
        name: "deciduous tree",
        category: EmojiCategory::AnimalsAndNature,
        keywords: &[],
        asset: "1f333",
    },
    // ── Food & Drink (BORU-TWEMOJI-12) ────────────────────────────
    Emoji {
        unicode: "🍔",
        name: "hamburger",
        category: EmojiCategory::FoodAndDrink,
        keywords: &[],
        asset: "1f354",
    },
    Emoji {
        unicode: "🍦",
        name: "soft ice cream",
        category: EmojiCategory::FoodAndDrink,
        keywords: &[],
        asset: "1f366",
    },
    Emoji {
        unicode: "🍺",
        name: "beer mug",
        category: EmojiCategory::FoodAndDrink,
        keywords: &[],
        asset: "1f37a",
    },
    // ── Activities (BORU-TWEMOJI-12) ──────────────────────────────
    Emoji {
        unicode: "⚽",
        name: "soccer ball",
        category: EmojiCategory::Activities,
        keywords: &[],
        asset: "26bd",
    },
    Emoji {
        unicode: "🎮",
        name: "video game",
        category: EmojiCategory::Activities,
        keywords: &[],
        asset: "1f3ae",
    },
    Emoji {
        unicode: "🎬",
        name: "clapper board",
        category: EmojiCategory::Activities,
        keywords: &[],
        asset: "1f3ac",
    },
    // ── Travel & Places (BORU-TWEMOJI-12) ──────────────────────────
    Emoji {
        unicode: "✈️",
        name: "airplane",
        category: EmojiCategory::TravelAndPlaces,
        keywords: &[],
        asset: "2708",
    },
    Emoji {
        unicode: "🚀",
        name: "rocket",
        category: EmojiCategory::TravelAndPlaces,
        keywords: &[],
        asset: "1f680",
    },
    // ── Objects (BORU-TWEMOJI-12) ─────────────────────────────────
    Emoji {
        unicode: "💎",
        name: "gem stone",
        category: EmojiCategory::Objects,
        keywords: &[],
        asset: "1f48e",
    },
    Emoji {
        unicode: "📱",
        name: "mobile phone",
        category: EmojiCategory::Objects,
        keywords: &[],
        asset: "1f4f1",
    },
    Emoji {
        unicode: "🔑",
        name: "key",
        category: EmojiCategory::Objects,
        keywords: &[],
        asset: "1f511",
    },
    // ── Flags (BORU-TWEMOJI-12) ───────────────────────────────────
    Emoji {
        unicode: "🇬🇧",
        name: "flag: United Kingdom",
        category: EmojiCategory::Flags,
        keywords: &[],
        asset: "1f1ec-1f1e7",
    },
    Emoji {
        unicode: "🇫🇷",
        name: "flag: France",
        category: EmojiCategory::Flags,
        keywords: &[],
        asset: "1f1eb-1f1f7",
    },
    Emoji {
        unicode: "🇯🇵",
        name: "flag: Japan",
        category: EmojiCategory::Flags,
        keywords: &[],
        asset: "1f1ef-1f1f5",
    },
    Emoji {
        unicode: "🇩🇪",
        name: "flag: Germany",
        category: EmojiCategory::Flags,
        keywords: &[],
        asset: "1f1e9-1f1ea",
    },
];

/// All curated common emojis (picker default list).
pub fn common_emojis() -> &'static [Emoji] {
    COMMON_EMOJIS
}

/// Emojis shown for a content category, in catalog order (BORU-TWEMOJI-12).
///
/// This is the single filtering surface the category view (and later the
/// search view, BORU-TWEMOJI-13) use against the shared catalog, so a
/// category switch and a search both read the same entries. The pseudo
/// category [`EmojiCategory::Recent`] is intentionally not a content
/// category: it yields an empty iterator (recents are dynamic user history,
/// BORU-TWEMOJI-14).
pub fn emojis_for_category(category: EmojiCategory) -> impl Iterator<Item = &'static Emoji> {
    COMMON_EMOJIS.iter().filter(move |e| e.category == category)
}

/// Representative Twemoji artwork for each content category's tab
/// (BORU-TWEMOJI-12).
///
/// Each icon is a catalog entry whose asset is guaranteed vendored, so the
/// picker's category tabs render as Twemoji SVG through the shared renderer
/// (falling back to the Unicode text glyph if the bundle is missing).
/// `Recent` has no icon yet — it is not a content category — so the lookup
/// returns `None` and the picker can decide its own placeholder (the simple
/// extension point for BORU-TWEMOJI-14).
pub fn category_icon(category: EmojiCategory) -> Option<&'static Emoji> {
    CATEGORY_ICONS
        .iter()
        .find(|(c, _)| *c == category)
        .map(|(_, e)| e)
}

/// Static icon lookup table for [`category_icon`]. Keys mirror the catalog
/// entries so the tab artwork and the grid artwork share one renderer path.
const CATEGORY_ICONS: &[(EmojiCategory, Emoji)] = &[
    (
        EmojiCategory::SmileysAndPeople,
        Emoji {
            unicode: "😀",
            name: "grinning face",
            category: EmojiCategory::SmileysAndPeople,
            keywords: &[],
            asset: "1f600",
        },
    ),
    (
        EmojiCategory::AnimalsAndNature,
        Emoji {
            unicode: "🦊",
            name: "fox",
            category: EmojiCategory::AnimalsAndNature,
            keywords: &[],
            asset: "1f98a",
        },
    ),
    (
        EmojiCategory::FoodAndDrink,
        Emoji {
            unicode: "🍕",
            name: "pizza",
            category: EmojiCategory::FoodAndDrink,
            keywords: &[],
            asset: "1f355",
        },
    ),
    (
        EmojiCategory::Activities,
        Emoji {
            unicode: "🎉",
            name: "party popper",
            category: EmojiCategory::Activities,
            keywords: &[],
            asset: "1f389",
        },
    ),
    (
        EmojiCategory::TravelAndPlaces,
        Emoji {
            unicode: "✈️",
            name: "airplane",
            category: EmojiCategory::TravelAndPlaces,
            keywords: &[],
            asset: "2708",
        },
    ),
    (
        EmojiCategory::Objects,
        Emoji {
            unicode: "💡",
            name: "light bulb",
            category: EmojiCategory::Objects,
            keywords: &[],
            asset: "1f4a1",
        },
    ),
    (
        EmojiCategory::Symbols,
        Emoji {
            unicode: "❤️",
            name: "red heart",
            category: EmojiCategory::Symbols,
            keywords: &[],
            asset: "2764",
        },
    ),
    (
        EmojiCategory::Flags,
        Emoji {
            unicode: "🇺🇸",
            name: "flag: United States",
            category: EmojiCategory::Flags,
            keywords: &[],
            asset: "1f1fa-1f1f8",
        },
    ),
];

/// Representative multi-codepoint emoji proving the model's expressiveness.
///
/// Not shown in the picker grid (BORU-TWEMOJI-10 keeps the picker list
/// glyph-safe until the grapheme-safe insertion fix lands in BORU-TWEMOJI-07);
/// these entries exist as fixtures for the catalog/resolver/renderer tests
/// and as the shape template the manifest-driven population (BORU-TWEMOJI-06)
/// will fill. Every asset key below matches a vendored
/// `assets/emoji/twemoji/svg/<key>.svg` file.
pub const REPRESENTATIVE_EMOJIS: &[Emoji] = &[
    Emoji {
        unicode: "🦊",
        name: "fox",
        category: EmojiCategory::AnimalsAndNature,
        keywords: &["animal", "fox"],
        asset: "1f98a",
    },
    Emoji {
        unicode: "❤️", // U+2764 U+FE0F — variation selector
        name: "red heart",
        category: EmojiCategory::Symbols,
        keywords: &["love", "heart"],
        asset: "2764",
    },
    Emoji {
        unicode: "🇺🇸", // U+1F1FA U+1F1F8 — regional-indicator flag pair
        name: "flag: United States",
        category: EmojiCategory::Flags,
        keywords: &["flag", "us", "america"],
        asset: "1f1fa-1f1f8",
    },
    Emoji {
        unicode: "👍🏽", // U+1F44D U+1F3FD — skin-tone modifier
        name: "thumbs up: medium skin tone",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &["thumb", "up", "skin tone"],
        asset: "1f44d-1f3fd",
    },
    Emoji {
        unicode: "👩‍💻", // U+1F469 ZWJ U+1F4BB — ZWJ sequence
        name: "woman technologist",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &["woman", "coder", "developer", "tech"],
        asset: "1f469-200d-1f4bb",
    },
    Emoji {
        unicode: "👨‍👩‍👧‍👦", // 4-codepoint ZWJ family sequence
        name: "family: man, woman, girl, boy",
        category: EmojiCategory::SmileysAndPeople,
        keywords: &["family", "man", "woman", "girl", "boy"],
        asset: "1f468-200d-1f469-200d-1f467-200d-1f466",
    },
];

/// Every catalog entry: the picker list plus the representative fixtures.
pub fn all_emoji() -> impl Iterator<Item = &'static Emoji> {
    COMMON_EMOJIS.iter().chain(REPRESENTATIVE_EMOJIS.iter())
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

    /// Acceptance: selecting an emoji yields its Unicode string, never an SVG
    /// path. The asset key must stay presentation metadata: bare identifier,
    /// no path separators, no extension, never equal to the inserted Unicode.
    #[test]
    fn insert_value_is_unicode_never_asset_or_path() {
        for e in all_emoji() {
            assert!(!e.unicode.is_empty(), "empty unicode for {:?}", e.name);
            assert!(!e.asset.is_empty(), "empty asset for {}", e.unicode);

            // The value the picker inserts / the message carries:
            let insert_value = e.unicode;
            assert_ne!(
                insert_value, e.asset,
                "{} must insert Unicode, not asset",
                e.unicode
            );

            // Asset keys are bare normalized identifiers, never paths.
            assert!(
                !e.asset.contains('/') && !e.asset.contains('\\'),
                "asset must not be a path: {}",
                e.asset
            );
            assert!(
                !e.asset.ends_with(".svg") && !e.asset.contains('.'),
                "asset must not carry a file extension: {}",
                e.asset
            );
            assert!(
                e.asset.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
                "asset must be lowercase hex + '-' separators: {}",
                e.asset
            );
        }
    }

    /// Acceptance: the catalog can represent a single code point.
    #[test]
    fn catalog_represents_single_codepoints() {
        let single: Vec<_> = REPRESENTATIVE_EMOJIS
            .iter()
            .filter(|e| e.unicode.chars().count() == 1)
            .collect();
        assert!(
            !single.is_empty(),
            "need at least one single-codepoint fixture"
        );
        assert_eq!(single[0].unicode.chars().count(), 1);
        assert_eq!(single[0].unicode, "🦊");
    }

    /// Acceptance: the catalog can represent flags (regional-indicator pair).
    #[test]
    fn catalog_represents_flags() {
        let us = REPRESENTATIVE_EMOJIS
            .iter()
            .find(|e| e.asset == "1f1fa-1f1f8")
            .expect("US flag fixture");
        let chars: Vec<char> = us.unicode.chars().collect();
        assert_eq!(chars.len(), 2, "flag is a regional-indicator pair");
        assert!(
            chars
                .iter()
                .all(|c| ('\u{1F1E6}'..='\u{1F1FF}').contains(c)),
            "flag chars are regional indicators"
        );
        assert_eq!(us.category, EmojiCategory::Flags);
    }

    /// Acceptance: the catalog can represent skin-tone variants
    /// (U+1F3FB..U+1F3FF modifier appended to a base emoji).
    #[test]
    fn catalog_represents_skin_tone_variants() {
        let thumb = REPRESENTATIVE_EMOJIS
            .iter()
            .find(|e| e.asset == "1f44d-1f3fd")
            .expect("skin-tone fixture");
        assert!(
            thumb
                .unicode
                .chars()
                .any(|c| ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)),
            "skin-tone modifier present in {}",
            thumb.unicode
        );
        assert!(thumb.unicode.chars().count() > 1);
    }

    /// Acceptance: the catalog can represent ZWJ sequences (U+200D joining
    /// multiple code points into one visual emoji).
    #[test]
    fn catalog_represents_zwj_sequences() {
        let zwj: Vec<_> = REPRESENTATIVE_EMOJIS
            .iter()
            .filter(|e| e.unicode.contains('\u{200D}'))
            .collect();
        assert_eq!(zwj.len(), 2, "two ZWJ fixtures");
        let family = REPRESENTATIVE_EMOJIS
            .iter()
            .find(|e| e.asset == "1f468-200d-1f469-200d-1f467-200d-1f466")
            .expect("family fixture");
        assert_eq!(family.unicode.chars().count(), 7, "4 emoji + 3 ZWJ");
        assert!(zwj.iter().all(|e| e.unicode.chars().count() > 1));
    }

    /// Acceptance: the catalog can represent variation-selector forms
    /// (U+FE0F) — already used by the picker list (❤️, ⚠️).
    #[test]
    fn catalog_represents_variation_selectors() {
        let with_vs16: Vec<_> = all_emoji()
            .filter(|e| e.unicode.contains('\u{FE0F}'))
            .collect();
        assert!(
            !with_vs16.is_empty(),
            "need at least one VS16 entry (red heart, warning)"
        );
        for e in with_vs16 {
            assert!(e.unicode.chars().count() > 1, "VS16 adds a code point");
        }
    }

    /// Task 12/14: Recent is a reserved pseudo-category with a stable label
    /// key, but is not a content category of catalog entries.
    #[test]
    fn recent_category_is_a_reserved_placeholder() {
        assert_eq!(EmojiCategory::Recent.label_key(), "emoji.category.recent");
        assert!(
            !EmojiCategory::ALL.contains(&EmojiCategory::Recent),
            "Recent must not be a catalog-entry category"
        );
        assert!(all_emoji().all(|e| e.category != EmojiCategory::Recent));
    }

    // ── Task 12: category navigation support ─────────────────────────

    /// Acceptance: every catalog entry belongs to one of the 8 content
    /// categories (and never the Recent pseudo-category).
    #[test]
    fn every_entry_belongs_to_a_content_category() {
        assert_eq!(EmojiCategory::ALL.len(), 8);
        let mut seen = std::collections::HashSet::new();
        for e in all_emoji() {
            assert!(
                EmojiCategory::ALL.contains(&e.category),
                "{:?} ({}) has non-content category {:?}",
                e.name,
                e.unicode,
                e.category
            );
            seen.insert(e.category);
        }
        assert_eq!(
            seen.len(),
            EmojiCategory::ALL.len(),
            "every content category must be represented by at least one entry"
        );
    }

    /// Acceptance: every category tab has at least one grid entry — no
    /// empty category when the user clicks its tab.
    #[test]
    fn every_content_category_has_picker_entries() {
        for category in EmojiCategory::ALL {
            let entries: Vec<_> = emojis_for_category(category).collect();
            assert!(
                !entries.is_empty(),
                "category {category:?} must have at least one picker entry"
            );
            assert!(
                entries.iter().all(|e| e.category == category),
                "category {category:?} returned a foreign entry"
            );
        }
    }

    /// Acceptance: category filtering is exact — switching categories
    /// yields disjoint entry sets, so the grid can never show stale items
    /// from the previously selected category.
    #[test]
    fn emojis_for_category_is_exact_and_disjoint() {
        let all: Vec<&Emoji> = common_emojis().iter().collect();
        let by_cat: Vec<Vec<&Emoji>> = EmojiCategory::ALL
            .iter()
            .map(|c| emojis_for_category(*c).collect())
            .collect();
        // No entry is lost and none is duplicated across categories.
        let flat: Vec<&Emoji> = by_cat.iter().flatten().copied().collect();
        assert_eq!(flat.len(), all.len(), "filtering must not drop entries");
        for (i, a) in flat.iter().enumerate() {
            for b in flat.iter().skip(i + 1) {
                assert_ne!(
                    a.unicode, b.unicode,
                    "entry {} appears in two categories",
                    a.unicode
                );
            }
        }
    }

    /// Acceptance: every content category has Twemoji artwork for its tab;
    /// the Recent pseudo-category has none (its tab is a later task,
    /// BORU-TWEMOJI-14).
    #[test]
    fn category_icons_cover_content_categories_only() {
        for category in EmojiCategory::ALL {
            let icon = category_icon(category)
                .unwrap_or_else(|| panic!("category {category:?} needs an icon"));
            assert!(
                !icon.asset.is_empty() && !icon.unicode.is_empty(),
                "icon for {category:?} must carry unicode + asset"
            );
        }
        assert!(category_icon(EmojiCategory::Recent).is_none());
    }

    /// Every category icon's asset key exists in the vendored set (guards
    /// against typos the same way the representative fixtures are guarded).
    #[test]
    fn category_icon_assets_match_vendored_files() {
        let vendored: std::collections::HashSet<String> = std::fs::read_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/emoji/twemoji/svg"
        ))
        .expect("vendored twemoji dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
        for (category, icon) in CATEGORY_ICONS {
            let file = format!("{}.svg", icon.asset);
            assert!(
                vendored.contains(&file),
                "category {category:?} icon asset {} has no vendored {}",
                icon.asset,
                file
            );
        }
    }

    /// Every representative fixture's asset key exists in the vendored set
    /// (guards against typos; manifest validation is BORU-TWEMOJI-06).
    #[test]
    fn representative_asset_keys_match_vendored_files() {
        let vendored: std::collections::HashSet<String> = std::fs::read_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/emoji/twemoji/svg"
        ))
        .expect("vendored twemoji dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
        for e in REPRESENTATIVE_EMOJIS {
            let file = format!("{}.svg", e.asset);
            assert!(
                vendored.contains(&file),
                "asset {} has no vendored {}",
                e.asset,
                file
            );
        }
    }
}
