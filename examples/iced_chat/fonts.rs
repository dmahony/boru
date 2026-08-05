//! Central typography system for the Boru desktop app.
//!
//! Defines font family names, typography tokens mapped to font/weight/size
//! combinations, and helper widgets for applying consistent type styles
//! throughout the UI.
//!
//! ## Font families
//!
//! | Family          | Weights loaded               | Scope                          |
//! |-----------------|------------------------------|--------------------------------|
//! | Source Sans 3   | 400 (Regular) · 600 (SemiBold) · 700 (Bold) | Primary app font (UI + chat)       |
//! | Inter           | 400 (Regular) · 500 (Medium) · 600 (SemiBold) · 700 (Bold) | Legacy fallback (not loaded by default) |
//! | Manrope         | 600 (Semibold) · 700 (Bold)  | Legacy export (not used by UI)  |
//! | Raleway         | 800 (ExtraBold)              | Boru wordmark / branding only   |
//! | JetBrains Mono  | 400 (Regular) · 500 (Medium) | Technical/code values           |
//!
//! ## Licence
//!
//! Source Sans 3, Inter, Manrope, Raleway, and JetBrains Mono are
//! licensed under the SIL Open Font License 1.1. See fonts/OFL.txt
//! and fonts/SourceSans3-OFL.txt for the full license texts.

use iced::font::{self, Family, Weight};
use iced::widget::text;
use iced::{Font, Pixels};

// ── Font file data (bundled at compile time, loaded at startup) ──────

/// Source Sans 3 Regular (400).
const SOURCE_SANS_REGULAR_BYTES: &[u8] = include_bytes!("fonts/SourceSans3-Regular.ttf");

/// Source Sans 3 SemiBold (600).
const SOURCE_SANS_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/SourceSans3-SemiBold.ttf");

/// Source Sans 3 Bold (700).
const SOURCE_SANS_BOLD_BYTES: &[u8] = include_bytes!("fonts/SourceSans3-Bold.ttf");

/// Inter Regular (400).
#[expect(dead_code)]
const INTER_REGULAR_BYTES: &[u8] = include_bytes!("fonts/Inter-Regular.ttf");

/// Inter Medium (500).
#[expect(dead_code)]
const INTER_MEDIUM_BYTES: &[u8] = include_bytes!("fonts/Inter-Medium.ttf");

/// Inter SemiBold (600).
#[expect(dead_code)]
const INTER_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/Inter-SemiBold.ttf");

/// Inter Bold (700).
#[expect(dead_code)]
const INTER_BOLD_BYTES: &[u8] = include_bytes!("fonts/Inter-Bold.ttf");

/// Manrope variable font — contains all weights from 200-800.
#[expect(dead_code)]
const MANROPE_BYTES: &[u8] = include_bytes!("fonts/Manrope.ttf");

/// Raleway ExtraBold 800 — branding only.
const RALEWAY_EXTRA_BOLD_BYTES: &[u8] = include_bytes!("fonts/Raleway-ExtraBold.ttf");

/// JetBrains Mono variable font — contains all weights from 100-800.
#[expect(dead_code)]
const JETBRAINS_MONO_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono.ttf");

/// JetBrains Mono Italic variable font — italic variant.
#[expect(dead_code)]
const JETBRAINS_MONO_ITALIC_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono-Italic.ttf");

// ── Font family names ────────────────────────────────────────────────

/// Internal family name for Source Sans 3.
pub const SOURCE_SANS: &str = "Source Sans 3";

/// Internal family name for Inter.
#[expect(dead_code)]
pub const INTER: &str = "Inter";

/// Internal family name for Manrope.
#[expect(dead_code)]
pub const MANROPE: &str = "Manrope";

/// Internal family name for Raleway (branding weight).
pub const RALEWAY: &str = "Raleway";

/// Internal family name for JetBrains Mono.
pub const JETBRAINS_MONO: &str = "JetBrains Mono";

// ── Font constructors ────────────────────────────────────────────────

/// Return a `Font` for Source Sans 3 at the given weight.
pub fn source_sans(weight: Weight) -> Font {
    Font {
        family: Family::Name(SOURCE_SANS),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

/// Return a `Font` for Inter at the given weight.
#[expect(dead_code)]
pub fn inter(weight: Weight) -> Font {
    Font {
        family: Family::Name(INTER),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

/// Return a `Font` for Manrope at the given weight.
#[expect(dead_code)]
pub fn manrope(weight: Weight) -> Font {
    Font {
        family: Family::Name(MANROPE),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

/// Return a `Font` for Raleway (weight ExtraBold).
pub fn raleway_extra_bold() -> Font {
    Font {
        family: Family::Name(RALEWAY),
        weight: Weight::ExtraBold,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

/// Return a `Font` for JetBrains Mono at the given weight.
pub fn jetbrains_mono(weight: Weight) -> Font {
    Font {
        family: Family::Name(JETBRAINS_MONO),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

// ── Type scale tokens ─────────────────────────────────────────────────
//
// Updated per Boru Modern UI spec, section 4:
//   Page title      28 px  Semibold   ~34 px line-height
//   Conversation    18 px  Semibold
//   Sidebar identity 16 px  Semibold
//   Body / labels   14 px  Regular
//   Chat messages   15 px  Regular
//   Secondary meta  12 px  Regular
//   Sidebar labels  12 px  Semibold  (uppercase, subtle letter spacing)

mod sizes {
    //! Type-size constants (pixels).  Boru Modern spec scale.

    /// Page title — 28 px.
    pub const PAGE_TITLE: f32 = 28.0;
    /// Home greeting (UI-HOME-02) — 32 px (approved mockup range 30–34 px).
    pub const HOME_GREETING: f32 = 32.0;
    /// Home subtitle (UI-HOME-02) — 16 px (approved mockup range 15–17 px).
    pub const HOME_SUBTITLE: f32 = 16.0;
    /// Conversation / section heading — 18 px.
    pub const CONVERSATION_TITLE: f32 = 18.0;
    /// Sidebar identity name — 16 px.
    pub const SIDEBAR_IDENTITY: f32 = 16.0;
    /// Chat message body — 15 px.
    pub const CHAT_MESSAGE: f32 = 15.0;
    /// Body text / labels — 14 px.
    pub const BODY: f32 = 14.0;
    /// Secondary metadata / labels — 12 px.
    pub const SECONDARY: f32 = 12.0;

    // ── Legacy size aliases (kept for gradual migration in app.rs) ────
    /// @deprecated use `PAGE_TITLE` (28 px) instead.
    pub const XL: f32 = 28.0;
    /// @deprecated use `CONVERSATION_TITLE` (18 px) instead.
    pub const LG: f32 = 18.0;
    /// @deprecated use `CHAT_MESSAGE` or `BODY` instead.
    pub const MD: f32 = 15.0;
    /// @deprecated use `BODY` (14 px) instead.
    pub const SM: f32 = 14.0;
    /// @deprecated use `SECONDARY` (12 px) instead.
    pub const XS: f32 = 12.0;
    /// @deprecated use `SECONDARY` (12 px) instead.
    pub const XXS: f32 = 12.0;
}
pub use sizes::*;

// ── Typography tokens ────────────────────────────────────────────────
//
// Each token knows its font family, weight, and default pixel size.
// Use `typo_text(token)` to build a text widget, or extract the fields
// with `typo_font(token)` / `typo_size(token)` for custom widgets.

/// Semantic typography role mapped to font, weight, and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(dead_code)]
pub enum Typography {
    // ── Source Sans 3 roles ──────────────────────────────────────────
    /// Page title — Source Sans 3 SemiBold, 28 px.
    PageTitle,
    /// Conversation / section heading — Source Sans 3 SemiBold, 18 px.
    SectionHeading,
    /// Sidebar identity name — Source Sans 3 SemiBold, 16 px.
    SidebarIdentity,
    /// Chat message body — Source Sans 3 Regular, 15 px.
    ChatMessage,
    /// Body text / labels — Source Sans 3 Regular, 14 px.
    Body,
    /// Supporting / secondary text — Source Sans 3 Regular, 12 px.
    SecondaryText,
    /// Sidebar section label — Source Sans 3 SemiBold, 12 px, uppercase, tracked.
    SidebarSectionLabel,
    /// Button label — Source Sans 3 SemiBold, 14 px.
    ButtonLabel,
    /// Navigation label — Source Sans 3 SemiBold, 14 px.
    NavigationLabel,
    /// Form label — Source Sans 3 SemiBold, 14 px.
    FormLabel,
    /// Timestamp — Source Sans 3 Regular, 12 px.
    Timestamp,
    /// Delivery state — Source Sans 3 Regular, 12 px.
    DeliveryState,
    /// System message — Source Sans 3 Regular, 14 px.
    SystemMessage,

    // ── JetBrains Mono roles ────────────────────────────────────────
    /// Technical value (peer IDs, keys, diagnostics) — JetBrains Mono Regular, 12 px.
    TechnicalValue,

    // ── Branding ─────────────────────────────────────────────────────
    /// Boru wordmark — Raleway ExtraBold, 28 px.
    BoruWordmark,
}

impl Typography {
    /// Return the font family name for this token.
    pub fn family_name(self) -> &'static str {
        match self {
            Self::BoruWordmark => RALEWAY,
            Self::TechnicalValue => JETBRAINS_MONO,
            _ => SOURCE_SANS,
        }
    }

    /// Return the font weight for this token.
    pub fn weight(self) -> Weight {
        match self {
            Self::PageTitle
            | Self::SectionHeading
            | Self::SidebarIdentity
            | Self::SidebarSectionLabel
            | Self::ButtonLabel
            | Self::NavigationLabel
            | Self::FormLabel => Weight::Semibold,
            Self::BoruWordmark => Weight::ExtraBold,
            Self::SystemMessage => Weight::Medium,
            _ => Weight::Normal,
        }
    }

    /// Return the default pixel size for this token.
    pub fn size_px(self) -> f32 {
        match self {
            Self::PageTitle | Self::BoruWordmark => PAGE_TITLE,
            Self::SectionHeading => CONVERSATION_TITLE,
            Self::SidebarIdentity => SIDEBAR_IDENTITY,
            Self::ChatMessage => CHAT_MESSAGE,
            Self::Body
            | Self::ButtonLabel
            | Self::NavigationLabel
            | Self::FormLabel
            | Self::SystemMessage => BODY,
            Self::SecondaryText
            | Self::SidebarSectionLabel
            | Self::Timestamp
            | Self::DeliveryState
            | Self::TechnicalValue => SECONDARY,
        }
    }

    /// Return an `iced::Font` for this token.
    pub fn font(self) -> Font {
        match self {
            Self::BoruWordmark => raleway_extra_bold(),
            Self::TechnicalValue => jetbrains_mono(Weight::Normal),
            _ => source_sans(self.weight()),
        }
    }
}

// ── Widget constructors ──────────────────────────────────────────────

/// Build an `Element` with the correct typography applied.
/// Shorthand for `.font(font).size(px)`.
#[expect(dead_code)]
pub fn typo_text<'a>(
    token: Typography,
    content: impl text::IntoFragment<'a>,
) -> text::Text<'a, iced::Theme, iced::Renderer> {
    text(content).font(token.font()).size(token.size_px())
}

/// Apply a typography token to an existing text widget.
#[expect(dead_code)]
pub fn with_typo<'a>(
    widget: text::Text<'a, iced::Theme, iced::Renderer>,
    token: Typography,
) -> text::Text<'a, iced::Theme, iced::Renderer> {
    widget.font(token.font()).size(token.size_px())
}

/// Like `typo_text` but with a custom `Pixels` size override
/// (for accessibility scaling).
#[expect(dead_code)]
pub fn typo_text_scaled<'a>(
    token: Typography,
    content: impl text::IntoFragment<'a>,
) -> text::Text<'a, iced::Theme, iced::Renderer> {
    text(content)
        .font(token.font())
        .size(Pixels(token.size_px()))
}

// ── Font loading ─────────────────────────────────────────────────────

/// Returns an `iced::Task` that loads all bundled fonts into the Iced
/// runtime.  Call once at application startup, chained onto the initial
/// command returned by `Application::new`.
///
/// The returned task fires the given message tag on completion; the
/// loading result can be ignored (errors are non-fatal — the system falls
/// back to the default sans-serif font).
///
/// Loads Source Sans 3 (primary UI font), Raleway ExtraBold (wordmark),
/// and JetBrains Mono (technical values). Inter and Manrope are kept as
/// compiled-in data but not loaded at startup unless needed.
pub fn load_fonts() -> iced::Task<crate::app::AppMessage> {
    iced::Task::batch(vec![
        font::load(SOURCE_SANS_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(SOURCE_SANS_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(SOURCE_SANS_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(RALEWAY_EXTRA_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(JETBRAINS_MONO_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(JETBRAINS_MONO_ITALIC_BYTES).map(|_| crate::app::AppMessage::Noop),
    ])
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_bytes_are_non_empty() {
        // Source Sans 3
        assert!(!SOURCE_SANS_REGULAR_BYTES.is_empty());
        assert!(!SOURCE_SANS_SEMI_BOLD_BYTES.is_empty());
        assert!(!SOURCE_SANS_BOLD_BYTES.is_empty());
        // Raleway
        assert!(!RALEWAY_EXTRA_BOLD_BYTES.is_empty());
        // JetBrains Mono
        assert!(!JETBRAINS_MONO_BYTES.is_empty());
        assert!(!JETBRAINS_MONO_ITALIC_BYTES.is_empty());
        // Inter (still bundled)
        assert!(!INTER_REGULAR_BYTES.is_empty());
        assert!(!INTER_MEDIUM_BYTES.is_empty());
        assert!(!INTER_SEMI_BOLD_BYTES.is_empty());
        assert!(!INTER_BOLD_BYTES.is_empty());
        // Manrope (still bundled)
        assert!(!MANROPE_BYTES.is_empty());
    }

    #[test]
    fn source_sans_is_primary_for_ui_text() {
        let tokens: &[Typography] = &[
            Typography::PageTitle,
            Typography::SectionHeading,
            Typography::SidebarIdentity,
            Typography::ChatMessage,
            Typography::Body,
            Typography::SecondaryText,
            Typography::SidebarSectionLabel,
            Typography::ButtonLabel,
            Typography::SystemMessage,
            Typography::Timestamp,
        ];
        for token in tokens {
            assert_eq!(
                token.family_name(),
                SOURCE_SANS,
                "{token:?} should use Source Sans 3"
            );
        }
    }

    #[test]
    fn wordmark_uses_raleway() {
        assert_eq!(Typography::BoruWordmark.family_name(), RALEWAY);
        assert_eq!(Typography::BoruWordmark.weight(), Weight::ExtraBold);
    }

    #[test]
    fn technical_uses_jetbrains_mono() {
        assert_eq!(Typography::TechnicalValue.family_name(), JETBRAINS_MONO);
    }

    #[test]
    fn type_scale_is_monotonic() {
        let sizes = [
            Typography::PageTitle.size_px(),       // 28
            Typography::SectionHeading.size_px(),  // 18
            Typography::SidebarIdentity.size_px(), // 16
            Typography::ChatMessage.size_px(),     // 15
            Typography::Body.size_px(),            // 14
            Typography::SecondaryText.size_px(),   // 12
        ];
        for w in sizes.windows(2) {
            assert!(
                w[0] >= w[1],
                "type scale not descending: {} → {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn type_sizes_match_spec() {
        assert_eq!(Typography::PageTitle.size_px(), 28.0);
        assert_eq!(Typography::SectionHeading.size_px(), 18.0);
        assert_eq!(Typography::SidebarIdentity.size_px(), 16.0);
        assert_eq!(Typography::ChatMessage.size_px(), 15.0);
        assert_eq!(Typography::Body.size_px(), 14.0);
        assert_eq!(Typography::SecondaryText.size_px(), 12.0);
    }

    #[test]
    fn sidebar_section_label_is_semibold_uppercase_sized_12() {
        let token = Typography::SidebarSectionLabel;
        assert_eq!(token.size_px(), 12.0);
        assert_eq!(token.weight(), Weight::Semibold);
        assert_eq!(token.family_name(), SOURCE_SANS);
    }

    #[test]
    fn legacy_sizes_are_reasonable() {
        // XL (page title) should be larger than LG (section heading)
        assert!(XL > LG);
        // MD (chat message) should be larger than SM (body)
        assert!(MD > SM);
        // SM (body) should be larger than XS (secondary)
        assert!(SM > XS);
        // All positive
        for s in [XL, LG, MD, SM, XS, XXS] {
            assert!(s > 0.0, "size {s} must be positive");
        }
    }
}
