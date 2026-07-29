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
//! | Inter           | 400 (Regular) · 500 (Medium) · 600 (SemiBold) · 700 (Bold) | Primary app font (85-90% of UI) |
//! | Manrope        | 600 (Semibold) · 700 (Bold)  | Legacy export (not used by UI)  |
//! | Raleway         | 800 (ExtraBold)              | Boru wordmark / branding only   |
//! | JetBrains Mono  | 400 (Regular) · 500 (Medium) | Technical/code values           |
//!
//! ## Licence
//!
//! Inter, Manrope, Raleway, and JetBrains Mono are licensed under
//! the SIL Open Font License 1.1. See fonts/Inter-OFL.txt for the full
//! Inter license text and fonts/OFL.txt for the other families.

use iced::font::{self, Family, Weight};
use iced::widget::text;
use iced::{Font, Pixels};

// ── Font file data (bundled at compile time, loaded at startup) ──────

/// Inter Regular (400) — static font.
const INTER_REGULAR_BYTES: &[u8] = include_bytes!("fonts/Inter-Regular.ttf");

/// Inter Medium (500) — static font.
const INTER_MEDIUM_BYTES: &[u8] = include_bytes!("fonts/Inter-Medium.ttf");

/// Inter SemiBold (600) — static font.
const INTER_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/Inter-SemiBold.ttf");

/// Inter Bold (700) — static font.
const INTER_BOLD_BYTES: &[u8] = include_bytes!("fonts/Inter-Bold.ttf");

/// Manrope variable font — contains all weights from 200-800.
const MANROPE_BYTES: &[u8] = include_bytes!("fonts/Manrope.ttf");

/// Raleway ExtraBold 800 — branding only.
const RALEWAY_EXTRA_BOLD_BYTES: &[u8] = include_bytes!("fonts/Raleway-ExtraBold.ttf");

/// JetBrains Mono variable font — contains all weights from 100-800.
const JETBRAINS_MONO_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono.ttf");

/// JetBrains Mono Italic variable font — italic variant.
const JETBRAINS_MONO_ITALIC_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono-Italic.ttf");

// ── Font family names ────────────────────────────────────────────────

/// Internal family name for Inter.
pub const INTER: &str = "Inter";

/// Internal family name for Manrope.
#[expect(dead_code)]
pub const MANROPE: &str = "Manrope";

/// Internal family name for Raleway (branding weight).
pub const RALEWAY: &str = "Raleway";

/// Internal family name for JetBrains Mono.
pub const JETBRAINS_MONO: &str = "JetBrains Mono";

// ── Font constructors ────────────────────────────────────────────────

/// Return a `Font` for Inter at the given weight.
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

// ── Type scale tokens (preserving original sizes) ────────────────────
//
// These constants match the original TYPO_* scale from app.rs. They are
// defined here so the typography system owns all type decisions.

mod sizes {
    //! Type-size constants (pixels).  Preserves the original Boru scale.
    pub const XL: f32 = 24.0; // Primary heading
    pub const LG: f32 = 18.0; // Secondary heading
    pub const MD: f32 = 15.0; // Body / section headers / button labels
    pub const SM: f32 = 13.0; // Secondary body / previews
    pub const XS: f32 = 11.0; // Metadata / secondary labels
    pub const XXS: f32 = 10.0; // Fine print
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
    // Manrope display headings
    /// Display large — Inter Bold, 24 px.
    DisplayLarge,
    /// Display medium — Inter SemiBold, 18 px.
    DisplayMedium,

    // Inter headings and interface text
    /// Page title — Inter Bold, 24 px.
    PageTitle,
    /// Section heading — Inter SemiBold, 18 px.
    SectionHeading,
    /// Contact name — Inter SemiBold, 15 px.
    ContactName,

    /// Chat message body — Inter Regular, 13 px.
    ChatMessage,
    /// Conversation preview — Inter Regular, 13 px.
    ConversationPreview,
    /// Unread conversation preview — Inter SemiBold, 13 px.
    ConversationPreviewUnread,
    /// Button label — Inter Medium, 15 px.
    ButtonLabel,
    /// Navigation label — Inter Medium, 15 px.
    NavigationLabel,
    /// Form label — Inter Medium, 15 px.
    FormLabel,
    /// Supporting / secondary text — Inter Regular, 13 px.
    SupportingText,
    /// Timestamp — Inter Regular, 11 px.
    Timestamp,
    /// Delivery state — Inter Regular, 11 px.
    DeliveryState,
    /// System message — Inter Medium, 13 px.
    SystemMessage,

    // JetBrains Mono
    /// Technical value (peer IDs, keys, diagnostics) — JetBrains Mono Regular, 11 px.
    TechnicalValue,

    // Branding
    /// Boru wordmark — Raleway ExtraBold.
    BoruWordmark,
}

impl Typography {
    /// Return the font family name for this token.
    #[expect(dead_code)]
    pub fn family_name(self) -> &'static str {
        match self {
            Self::DisplayLarge | Self::DisplayMedium => INTER,
            Self::BoruWordmark => RALEWAY,
            Self::TechnicalValue => JETBRAINS_MONO,
            _ => INTER,
        }
    }

    /// Return the font weight for this token.
    #[expect(dead_code)]
    pub fn weight(self) -> Weight {
        match self {
            Self::DisplayLarge | Self::PageTitle => Weight::Bold,
            Self::DisplayMedium => Weight::Semibold,
            Self::SectionHeading | Self::ContactName | Self::ConversationPreviewUnread => {
                Weight::Semibold
            }
            Self::ButtonLabel | Self::NavigationLabel | Self::FormLabel | Self::SystemMessage => {
                Weight::Medium
            }
            Self::BoruWordmark => Weight::ExtraBold,
            Self::TechnicalValue => Weight::Normal,
            _ => Weight::Normal,
        }
    }

    /// Return the default pixel size for this token.
    #[expect(dead_code)]
    pub fn size_px(self) -> f32 {
        match self {
            Self::DisplayLarge | Self::PageTitle => XL,
            Self::DisplayMedium | Self::SectionHeading => LG,
            Self::ContactName | Self::ButtonLabel | Self::NavigationLabel | Self::FormLabel => MD,
            Self::ChatMessage
            | Self::ConversationPreview
            | Self::ConversationPreviewUnread
            | Self::SupportingText
            | Self::SystemMessage => SM,
            Self::Timestamp | Self::DeliveryState | Self::TechnicalValue => XS,
            Self::BoruWordmark => XL,
        }
    }

    /// Return an `iced::Font` for this token.
    #[expect(dead_code)]
    pub fn font(self) -> Font {
        match self {
            Self::DisplayLarge => inter(Weight::Bold),
            Self::DisplayMedium => inter(Weight::Semibold),
            Self::BoruWordmark => raleway_extra_bold(),
            Self::TechnicalValue => jetbrains_mono(Weight::Normal),
            _ => inter(self.weight()),
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
pub fn load_fonts() -> iced::Task<crate::app::AppMessage> {
    iced::Task::batch(vec![
        font::load(INTER_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(INTER_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(INTER_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(INTER_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(MANROPE_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(RALEWAY_EXTRA_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(JETBRAINS_MONO_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(JETBRAINS_MONO_ITALIC_BYTES).map(|_| crate::app::AppMessage::Noop),
    ])
}
