//! Central typography system for the Boru desktop app.
//!
//! Defines font family names, typography tokens mapped to font/weight/size
//! combinations, and helper widgets for applying consistent type styles
//! throughout the UI.
//!
//! ## Font families
//!
//! | Family          | Weights loaded               | Scope                          | Fallback chain (FONTS-14)          |
//! |-----------------|------------------------------|--------------------------------|------------------------------------|
//! | Archivo SemiCondensed | 600 (SemiBold) · 700 (Bold) | Major display/page headings (DisplayHeading, PageTitle) | Arial Narrow → generic sans-serif |
//! | IBM Plex Sans   | 400 (Regular) · 500 (Medium) · 600 (SemiBold) | Primary app UI: sections, cards, body, buttons, metadata | Arial → system sans-serif |
//! | Figtree         | 400 · 500 · 600               | Chat messages, sender, metadata, composer | Arial → system sans-serif |
//! | JetBrains Mono  | 400 · 500                     | Technical/code values           | Consolas → monospace               |
//! | Raleway         | 800 (ExtraBold)               | BORU wordmark / branding only   | Raleway (bundled — unchanged)      |
//!
//! In iced terms, `TypeRole::fallback_family()` maps these chains to
//! `Family` values: display/page-heading roles → `Family::Name("Arial
//! Narrow")` (iced/fontdb resolves the terminal generic sans-serif when the
//! named family is absent); UI and chat roles → `Family::SansSerif` (the
//! system sans-serif — Arial on Windows); technical values →
//! `Family::Monospace`; the wordmark stays `Family::Name("Raleway")`.
//!
//! ## Licence
//!
//! Figtree, Raleway, JetBrains Mono, Archivo SemiCondensed, and IBM
//! Plex Sans are licensed under the SIL Open Font License 1.1. See
//! fonts/OFL.txt and the per-family OFL records (fonts/Figtree-OFL.txt,
//! fonts/JetBrainsMono-OFL.txt, fonts/Raleway-OFL.txt,
//! fonts/Archivo-OFL.txt, fonts/IBMPlexSans-OFL.txt) plus
//! fonts/THIRD_PARTY_NOTICES.md for exact sources and versions.

use iced::font::{self, Family, Weight};
use iced::widget::text;
use iced::Font;

// ── Font file data (bundled at compile time, loaded at startup) ──────

/// Figtree Regular (400) — chat message / composer text.
const FIGTREE_REGULAR_BYTES: &[u8] = include_bytes!("fonts/Figtree-Regular.ttf");

/// Figtree Medium (500) — chat text emphasis.
const FIGTREE_MEDIUM_BYTES: &[u8] = include_bytes!("fonts/Figtree-Medium.ttf");

/// Figtree SemiBold (600) — chat sender names.
const FIGTREE_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/Figtree-SemiBold.ttf");

/// Raleway ExtraBold 800 — branding only.
const RALEWAY_EXTRA_BOLD_BYTES: &[u8] = include_bytes!("fonts/Raleway-ExtraBold.ttf");

/// JetBrains Mono variable font (100–800) — legacy bundled asset, NOT loaded at startup.
#[expect(dead_code)]
const JETBRAINS_MONO_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono.ttf");

/// JetBrains Mono Italic variable font — legacy bundled asset, NOT loaded at startup.
#[expect(dead_code)]
const JETBRAINS_MONO_ITALIC_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono-Italic.ttf");

/// JetBrains Mono Regular (400) — technical values.
const JETBRAINS_MONO_REGULAR_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono-Regular.ttf");

/// JetBrains Mono Medium (500) — emphasised technical values.
const JETBRAINS_MONO_MEDIUM_BYTES: &[u8] = include_bytes!("fonts/JetBrainsMono-Medium.ttf");

/// Archivo SemiCondensed SemiBold (600) — display headings (width axis 87.5).
const ARCHIVO_SEMI_CONDENSED_SEMI_BOLD_BYTES: &[u8] =
    include_bytes!("fonts/ArchivoSemiCondensed-SemiBold.ttf");

/// Archivo SemiCondensed Bold (700) — major display headings (width axis 87.5).
const ARCHIVO_SEMI_CONDENSED_BOLD_BYTES: &[u8] =
    include_bytes!("fonts/ArchivoSemiCondensed-Bold.ttf");

/// IBM Plex Sans Regular (400) — general app UI (static instance, wdth 100).
const IBM_PLEX_SANS_REGULAR_BYTES: &[u8] = include_bytes!("fonts/IBMPlexSans-Regular.ttf");

/// IBM Plex Sans Medium (500) — general app UI emphasis (static instance, wdth 100).
const IBM_PLEX_SANS_MEDIUM_BYTES: &[u8] = include_bytes!("fonts/IBMPlexSans-Medium.ttf");

/// IBM Plex Sans SemiBold (600) — general app UI headings/labels (static instance, wdth 100).
const IBM_PLEX_SANS_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/IBMPlexSans-SemiBold.ttf");

// ── Font family names ────────────────────────────────────────────────

/// Internal family name for Figtree.
pub const FIGTREE: &str = "Figtree";

/// Internal family name for Raleway (branding weight).
pub const RALEWAY: &str = "Raleway";

/// Internal family name for JetBrains Mono.
pub const JETBRAINS_MONO: &str = "JetBrains Mono";

/// Internal family name for Archivo SemiCondensed (display headings).
pub const ARCHIVO_SEMI_CONDENSED: &str = "Archivo SemiCondensed";

/// Internal family name for IBM Plex Sans (general app UI).
pub const IBM_PLEX_SANS: &str = "IBM Plex Sans";

/// Platform fallback family for display/page-heading roles (FONTS Task 14:
/// Archivo SemiCondensed → Arial Narrow → generic sans-serif).
pub const ARIAL_NARROW: &str = "Arial Narrow";

// ── Font constructors ────────────────────────────────────────────────

/// Return a `Font` for Figtree at the given weight.
pub fn figtree(weight: Weight) -> Font {
    Font {
        family: Family::Name(FIGTREE),
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

/// Return a `Font` for Archivo SemiCondensed at the given weight.
///
/// Registered weights: 600 (SemiBold) and 700 (Bold). The family's width
/// axis is pinned at 87.5 (SemiCondensed) in the bundled static instances.
pub fn archivo_semi_condensed(weight: Weight) -> Font {
    Font {
        family: Family::Name(ARCHIVO_SEMI_CONDENSED),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

/// Return a `Font` for IBM Plex Sans at the given weight.
///
/// Registered weights: 400 (Regular), 500 (Medium), and 600 (SemiBold).
/// The bundled static instances are normal-width (wdth 100) statics
/// generated from the official variable font.
pub fn ibm_plex_sans(weight: Weight) -> Font {
    Font {
        family: Family::Name(IBM_PLEX_SANS),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

// ── Type scale tokens ─────────────────────────────────────────────────
//
// The canonical sizes live in `TypeRole::size_px()` (FONTS Task 16
// baseline — all roles are within the approved ranges):
//   display_heading  32 · page_title 28 · section_title 20 · card_title 18
//   body 15 · body_emphasised 15 · button_label 14 · supporting_text 13
//   metadata 12 · chat_message 15 · chat_sender 14 · chat_metadata 12
//   composer_text 15 · technical_value 12 · brand_wordmark 28
//
// The `sizes` module below keeps only the constants still referenced by
// app code (`HOME_SUBTITLE`, the dialog-scale tokens `DIALOG_TITLE` /
// `DIALOG_SUBTITLE` added in FONTS-11, and the legacy `TYPO_*` aliases).
// The former named scale constants (PAGE_TITLE, CONVERSATION_TITLE,
// SIDEBAR_IDENTITY, CHAT_MESSAGE, BODY, SECONDARY) were removed in
// FONTS-16 — no token or call site referenced them; `TypeRole::size_px()`
// is the single source for the role sizes.

mod sizes {
    //! Type-size constants (pixels).  Boru Modern spec scale.
    //!
    //! `TypeRole::size_px()` owns the canonical sizes (FONTS Task 16
    //! baseline). This module keeps the constants app.rs still references
    //! directly: `HOME_SUBTITLE` (home subtitle) and the legacy aliases
    //! re-exported as `TYPO_*`.

    /// Home subtitle (UI-HOME-02) — 16 px (approved mockup range 15–17 px).
    pub const HOME_SUBTITLE: f32 = 16.0;
    /// Creation-dialog title (FONTS Task 11) — 26 px (approved band 24–28 px).
    pub const DIALOG_TITLE: f32 = 26.0;
    /// Creation-dialog subtitle (FONTS Task 11) — 14 px (approved band 14–15 px).
    pub const DIALOG_SUBTITLE: f32 = 14.0;

    // ── Legacy size aliases (kept for gradual migration in app.rs) ────
    /// @deprecated use `TypeRole::PageTitle` (28 px) instead.
    pub const XL: f32 = 28.0;
    /// @deprecated use `TypeRole::CardTitle` (18 px) instead.
    pub const LG: f32 = 18.0;
    /// @deprecated use `TypeRole::ChatMessage` (15 px) instead.
    pub const MD: f32 = 15.0;
    /// @deprecated use `TypeRole::ButtonLabel` (14 px) instead.
    pub const SM: f32 = 14.0;
    /// @deprecated use `TypeRole::Metadata` (12 px) instead.
    pub const XS: f32 = 12.0;
    /// @deprecated use `TypeRole::Metadata` (12 px) instead.
    pub const XXS: f32 = 12.0;
}
pub use sizes::*;

// ── Canonical semantic roles (UI-HOME-11) ─────────────────────────────
//
// `TypeRole` is the central typography system approved by the Boru plan:
// every role names the content kind and knows its family, weight and default
// size. Screens (UI-HOME-12/13/14) migrate onto these roles. The legacy
// `Typography` token set was removed in FONTS-04 — one central system.
//
// Sizes follow the Boru plan's approved mapping (UI-HOME-12/13) and the
// FONTS Task 16 baseline (all values are within the approved ranges):
//   display_heading  Archivo SemiCondensed Bold 32   page greeting / hero
//   page_title       Archivo SemiCondensed Bold 28   application page title
//   section_title    IBM Plex Sans SemiBold 20       section heading (creation-dialog titles use PageTitle family @ DIALOG_TITLE — FONTS-11)
//   card_title       IBM Plex Sans SemiBold 18       dashboard card title
//   body             IBM Plex Sans Regular 15        body copy and descriptions
//   body_emphasised  IBM Plex Sans SemiBold 15       emphasised body copy
//   button_label     IBM Plex Sans SemiBold 14       buttons and interactive labels
//   supporting_text  IBM Plex Sans Regular 13        supporting / secondary copy
//   metadata         IBM Plex Sans Regular 12        timestamps, counts, small metadata
//   chat_message     Figtree Regular 15   chat message body
//   chat_sender      Figtree SemiBold 14  sender name
//   chat_metadata    Figtree Regular 12  message timestamps / status
//   composer_text    Figtree Regular 15  composer input + placeholder
//   technical_value  JBM Regular 12     peer IDs, hashes, ports, fingerprints
//   brand_wordmark   Raleway ExtraBold 28  BORU wordmark

/// Canonical semantic typography roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRole {
    /// Hero / page greeting — Archivo SemiCondensed Bold 32.
    DisplayHeading,
    /// Application page title — Archivo SemiCondensed Bold 28.
    PageTitle,
    /// Section heading — IBM Plex Sans SemiBold 20.
    /// (Creation-dialog titles use the PageTitle family at the DIALOG_TITLE
    /// scale — Archivo SemiCondensed Bold 26 px, FONTS-11.)
    SectionTitle,
    /// Card title — IBM Plex Sans SemiBold 18.
    CardTitle,
    /// Body copy / descriptions — IBM Plex Sans Regular 15.
    Body,
    /// Emphasised body copy — IBM Plex Sans SemiBold 15.
    BodyEmphasised,
    /// Button and interactive label — IBM Plex Sans SemiBold 14.
    ButtonLabel,
    /// Supporting / secondary copy — IBM Plex Sans Regular 13.
    SupportingText,
    /// Metadata (timestamps, counts) — IBM Plex Sans Regular 12.
    Metadata,
    /// Chat message body — Figtree Regular 15.
    ChatMessage,
    /// Chat sender name — Figtree SemiBold 14.
    ChatSender,
    /// Chat message metadata (timestamp/status) — Figtree Regular 12.
    ChatMetadata,
    /// Composer input and placeholder — Figtree Regular 15.
    ComposerText,
    /// Technical identifier (peer ID, hash, port, fingerprint) — JetBrains Mono Regular 12.
    TechnicalValue,
    /// BORU wordmark — Raleway ExtraBold 28.
    BrandWordmark,
}

impl TypeRole {
    /// Primary font family name for this role.
    pub fn family_name(self) -> &'static str {
        match self {
            Self::DisplayHeading | Self::PageTitle => ARCHIVO_SEMI_CONDENSED,
            Self::SectionTitle
            | Self::CardTitle
            | Self::Body
            | Self::BodyEmphasised
            | Self::ButtonLabel
            | Self::SupportingText
            | Self::Metadata => IBM_PLEX_SANS,
            Self::ChatMessage | Self::ChatSender | Self::ChatMetadata | Self::ComposerText => FIGTREE,
            Self::TechnicalValue => JETBRAINS_MONO,
            Self::BrandWordmark => RALEWAY,
        }
    }

    /// Font weight for this role (all weights are registered statically —
    /// no synthetic bolding is required).
    pub fn weight(self) -> Weight {
        match self {
            Self::DisplayHeading | Self::PageTitle => Weight::Bold,
            Self::SectionTitle
            | Self::CardTitle
            | Self::BodyEmphasised
            | Self::ButtonLabel
            | Self::ChatSender => Weight::Semibold,
            Self::BrandWordmark => Weight::ExtraBold,
            Self::Body
            | Self::SupportingText
            | Self::Metadata
            | Self::ChatMessage
            | Self::ChatMetadata
            | Self::ComposerText
            | Self::TechnicalValue => Weight::Normal,
        }
    }

    /// Default pixel size for this role.
    pub fn size_px(self) -> f32 {
        match self {
            Self::DisplayHeading => 32.0,
            Self::PageTitle | Self::BrandWordmark => 28.0,
            Self::SectionTitle => 20.0,
            Self::CardTitle => 18.0,
            Self::Body | Self::BodyEmphasised | Self::ChatMessage | Self::ComposerText => 15.0,
            Self::ButtonLabel => 14.0,
            Self::ChatSender => 14.0,
            Self::SupportingText => 13.0,
            Self::Metadata | Self::ChatMetadata | Self::TechnicalValue => 12.0,
        }
    }

    /// Return an `iced::Font` for this role.
    pub fn font(self) -> Font {
        match self.family_name() {
            ARCHIVO_SEMI_CONDENSED => archivo_semi_condensed(self.weight()),
            IBM_PLEX_SANS => ibm_plex_sans(self.weight()),
            FIGTREE => figtree(self.weight()),
            JETBRAINS_MONO => jetbrains_mono(self.weight()),
            RALEWAY => raleway_extra_bold(),
            // Defensive default: the UI family (kept in sync with family_name()).
            _ => ibm_plex_sans(self.weight()),
        }
    }

    /// Fallback font family for this role, used when the primary family is
    /// not registered on the platform. Returns the FONTS Task 14 platform
    /// chain as an iced `Family`:
    ///   display/page headings → `Family::Name("Arial Narrow")` (iced/fontdb
    ///     resolves the terminal generic sans-serif when Arial Narrow is
    ///     absent — e.g. Linux/macOS)
    ///   UI roles              → `Family::SansSerif` (system sans-serif —
    ///     Arial on Windows)
    ///   chat roles            → `Family::SansSerif` (system sans-serif —
    ///     Arial on Windows)
    ///   technical values      → `Family::Monospace` (platform monospace —
    ///     Consolas on Windows)
    ///   brand wordmark        → Raleway (bundled — unchanged)
    pub fn fallback_family(self) -> Family {
        match self {
            Self::DisplayHeading | Self::PageTitle => Family::Name(ARIAL_NARROW),
            Self::TechnicalValue => Family::Monospace,
            Self::BrandWordmark => Family::Name(RALEWAY),
            // UI and chat roles both degrade to the system sans-serif.
            _ => Family::SansSerif,
        }
    }

    /// Fallback weight — the closest weight the fallback family actually
    /// registers (keeps emphasis without synthetic bolding).
    pub fn fallback_weight(self) -> Weight {
        match self.weight() {
            Weight::ExtraBold | Weight::Bold => Weight::Bold,
            Weight::Semibold => Weight::Semibold,
            Weight::Medium => Weight::Medium,
            _ => Weight::Normal,
        }
    }

    /// Return the fallback `Font` for this role.
    pub fn fallback_font(self) -> Font {
        Font {
            family: self.fallback_family(),
            weight: self.fallback_weight(),
            stretch: iced::font::Stretch::Normal,
            style: iced::font::Style::Normal,
        }
    }

    /// Short human label for previews / docs.
    pub fn label(self) -> &'static str {
        match self {
            Self::DisplayHeading => "display_heading",
            Self::PageTitle => "page_title",
            Self::SectionTitle => "section_title",
            Self::CardTitle => "card_title",
            Self::Body => "body",
            Self::BodyEmphasised => "body_emphasised",
            Self::ButtonLabel => "button_label",
            Self::SupportingText => "supporting_text",
            Self::Metadata => "metadata",
            Self::ChatMessage => "chat_message",
            Self::ChatSender => "chat_sender",
            Self::ChatMetadata => "chat_metadata",
            Self::ComposerText => "composer_text",
            Self::TechnicalValue => "technical_value",
            Self::BrandWordmark => "brand_wordmark",
        }
    }

    /// All roles, in display order for previews.
    pub const ALL: [TypeRole; 15] = [
        Self::DisplayHeading,
        Self::PageTitle,
        Self::SectionTitle,
        Self::CardTitle,
        Self::Body,
        Self::BodyEmphasised,
        Self::ButtonLabel,
        Self::SupportingText,
        Self::Metadata,
        Self::ChatMessage,
        Self::ChatSender,
        Self::ChatMetadata,
        Self::ComposerText,
        Self::TechnicalValue,
        Self::BrandWordmark,
    ];
}

/// Build an `Element`-ready text widget for a canonical role.
pub fn type_role_text<'a>(
    role: TypeRole,
    content: impl text::IntoFragment<'a>,
) -> text::Text<'a, iced::Theme, iced::Renderer> {
    text(content).font(role.font()).size(role.size_px())
}

/// Build a text widget for a canonical role with an explicit relative
/// line-height (plan UI-HOME-12: display headings ~1.2, body copy ~1.45).
pub fn type_role_text_lh<'a>(
    role: TypeRole,
    content: impl text::IntoFragment<'a>,
    line_height: f32,
) -> text::Text<'a, iced::Theme, iced::Renderer> {
    text(content)
        .font(role.font())
        .size(role.size_px())
        .line_height(text::LineHeight::Relative(line_height))
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
/// Loads Archivo SemiCondensed (display/page headings), IBM Plex Sans
/// (primary UI), Figtree (chat), Raleway ExtraBold (wordmark), and
/// JetBrains Mono (technical values).
pub fn load_fonts() -> iced::Task<crate::app::AppMessage> {
    iced::Task::batch(vec![
        // Figtree — 400 · 500 · 600 (chat).
        font::load(FIGTREE_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(FIGTREE_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(FIGTREE_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        // Raleway ExtraBold 800 — wordmark only.
        font::load(RALEWAY_EXTRA_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        // JetBrains Mono — 400 · 500 (technical values).
        font::load(JETBRAINS_MONO_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(JETBRAINS_MONO_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        // Archivo SemiCondensed — 600 · 700 (display headings).
        font::load(ARCHIVO_SEMI_CONDENSED_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(ARCHIVO_SEMI_CONDENSED_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        // IBM Plex Sans — 400 · 500 · 600 (general app UI).
        font::load(IBM_PLEX_SANS_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(IBM_PLEX_SANS_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(IBM_PLEX_SANS_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
    ])
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_bytes_are_non_empty() {
        // Raleway
        assert!(!RALEWAY_EXTRA_BOLD_BYTES.is_empty());
        // JetBrains Mono
        assert!(!JETBRAINS_MONO_BYTES.is_empty());
        assert!(!JETBRAINS_MONO_ITALIC_BYTES.is_empty());
        // Figtree
        assert!(!FIGTREE_REGULAR_BYTES.is_empty());
        assert!(!FIGTREE_MEDIUM_BYTES.is_empty());
        assert!(!FIGTREE_SEMI_BOLD_BYTES.is_empty());
        // JetBrains Mono statics
        assert!(!JETBRAINS_MONO_REGULAR_BYTES.is_empty());
        assert!(!JETBRAINS_MONO_MEDIUM_BYTES.is_empty());
        // Archivo SemiCondensed statics
        assert!(!ARCHIVO_SEMI_CONDENSED_SEMI_BOLD_BYTES.is_empty());
        assert!(!ARCHIVO_SEMI_CONDENSED_BOLD_BYTES.is_empty());
        // IBM Plex Sans statics
        assert!(!IBM_PLEX_SANS_REGULAR_BYTES.is_empty());
        assert!(!IBM_PLEX_SANS_MEDIUM_BYTES.is_empty());
        assert!(!IBM_PLEX_SANS_SEMI_BOLD_BYTES.is_empty());
    }

    #[test]
    fn every_required_family_weight_is_registered() {
        // The FONTS-04 token families, each at the exact weights the roles
        // request.
        let expectations: &[(&str, Weight)] = &[
            (ARCHIVO_SEMI_CONDENSED, Weight::Semibold), // 600
            (ARCHIVO_SEMI_CONDENSED, Weight::Bold),     // 700
            (IBM_PLEX_SANS, Weight::Normal),     // 400
            (IBM_PLEX_SANS, Weight::Medium),     // 500
            (IBM_PLEX_SANS, Weight::Semibold),   // 600
            (FIGTREE, Weight::Normal),     // 400
            (FIGTREE, Weight::Medium),     // 500
            (FIGTREE, Weight::Semibold),   // 600
            (RALEWAY, Weight::ExtraBold),  // 800
            (JETBRAINS_MONO, Weight::Normal), // 400
            (JETBRAINS_MONO, Weight::Medium), // 500
        ];
        let loaded: &[(&str, Weight)] = &[
            (FIGTREE, Weight::Normal),
            (FIGTREE, Weight::Medium),
            (FIGTREE, Weight::Semibold),
            (RALEWAY, Weight::ExtraBold),
            (JETBRAINS_MONO, Weight::Normal),
            (JETBRAINS_MONO, Weight::Medium),
            (ARCHIVO_SEMI_CONDENSED, Weight::Semibold),
            (ARCHIVO_SEMI_CONDENSED, Weight::Bold),
            (IBM_PLEX_SANS, Weight::Normal),
            (IBM_PLEX_SANS, Weight::Medium),
            (IBM_PLEX_SANS, Weight::Semibold),
        ];
        for (family, weight) in expectations {
            assert!(
                loaded.contains(&(*family, *weight)),
                "required weight not registered: {family} {weight:?}"
            );
        }
    }

    #[test]
    fn type_role_uses_approved_families() {
        // FONTS-04 approved mapping: Archivo SemiCondensed for display/page
        // headings, IBM Plex Sans for general UI, Figtree for chat,
        // JetBrains Mono for technical values, Raleway for the wordmark.
        assert_eq!(TypeRole::DisplayHeading.family_name(), ARCHIVO_SEMI_CONDENSED);
        assert_eq!(TypeRole::DisplayHeading.weight(), Weight::Bold);
        assert_eq!(TypeRole::PageTitle.family_name(), ARCHIVO_SEMI_CONDENSED);
        assert_eq!(TypeRole::PageTitle.weight(), Weight::Bold);
        assert_eq!(TypeRole::SectionTitle.family_name(), IBM_PLEX_SANS);
        assert_eq!(TypeRole::SectionTitle.weight(), Weight::Semibold);
        assert_eq!(TypeRole::CardTitle.family_name(), IBM_PLEX_SANS);
        assert_eq!(TypeRole::Body.family_name(), IBM_PLEX_SANS);
        assert_eq!(TypeRole::BodyEmphasised.family_name(), IBM_PLEX_SANS);
        assert_eq!(TypeRole::ButtonLabel.family_name(), IBM_PLEX_SANS);
        assert_eq!(TypeRole::SupportingText.family_name(), IBM_PLEX_SANS);
        assert_eq!(TypeRole::Metadata.family_name(), IBM_PLEX_SANS);
        assert_eq!(TypeRole::ChatMessage.family_name(), FIGTREE);
        assert_eq!(TypeRole::ChatSender.family_name(), FIGTREE);
        assert_eq!(TypeRole::ChatMetadata.family_name(), FIGTREE);
        assert_eq!(TypeRole::ComposerText.family_name(), FIGTREE);
        assert_eq!(TypeRole::TechnicalValue.family_name(), JETBRAINS_MONO);
        assert_eq!(TypeRole::BrandWordmark.family_name(), RALEWAY);
        assert_eq!(TypeRole::BrandWordmark.weight(), Weight::ExtraBold);
    }

    #[test]
    fn type_role_weights_are_real_not_synthetic() {
        // Every weight a role requests must be a registered static weight.
        for role in TypeRole::ALL {
            let family = role.family_name();
            let weight = role.weight();
            match family {
                ARCHIVO_SEMI_CONDENSED => assert!(
                    weight == Weight::Semibold || weight == Weight::Bold,
                    "Archivo role {role:?} requests unsupported weight {weight:?}"
                ),
                IBM_PLEX_SANS => assert!(
                    matches!(weight, Weight::Normal | Weight::Medium | Weight::Semibold),
                    "IBM Plex Sans role {role:?} requests unsupported weight {weight:?}"
                ),
                FIGTREE => assert!(
                    matches!(weight, Weight::Normal | Weight::Medium | Weight::Semibold),
                    "Figtree role {role:?} requests unsupported weight {weight:?}"
                ),
                JETBRAINS_MONO => assert!(
                    matches!(weight, Weight::Normal | Weight::Medium),
                    "JBM role {role:?} requests unsupported weight {weight:?}"
                ),
                RALEWAY => assert_eq!(weight, Weight::ExtraBold),
                other => panic!("unexpected family {other}"),
            }
        }
    }

    #[test]
    fn type_role_fallbacks_are_platform_appropriate() {
        // FONTS Task 14: display/page headings fall back to Arial Narrow
        // (iced/fontdb resolves the terminal generic sans-serif when the
        // named family is absent), UI and chat roles to the system
        // sans-serif (Arial on Windows), technical values to the platform
        // monospace (Consolas on Windows), and the wordmark stays Raleway.
        // The legacy families were removed in FONTS-12, so every fallback
        // must be one of the approved platform families below.
        assert_eq!(
            TypeRole::TechnicalValue.fallback_family(),
            iced::font::Family::Monospace
        );
        assert_eq!(
            TypeRole::DisplayHeading.fallback_family(),
            iced::font::Family::Name(ARIAL_NARROW)
        );
        assert_eq!(
            TypeRole::PageTitle.fallback_family(),
            iced::font::Family::Name(ARIAL_NARROW)
        );
        assert_eq!(
            TypeRole::ChatMessage.fallback_family(),
            iced::font::Family::SansSerif
        );
        assert_eq!(
            TypeRole::BrandWordmark.fallback_family(),
            iced::font::Family::Name(RALEWAY)
        );
        for role in TypeRole::ALL {
            match role {
                TypeRole::DisplayHeading | TypeRole::PageTitle => {
                    assert_eq!(
                        role.fallback_family(),
                        iced::font::Family::Name(ARIAL_NARROW)
                    );
                }
                TypeRole::SectionTitle
                | TypeRole::CardTitle
                | TypeRole::Body
                | TypeRole::BodyEmphasised
                | TypeRole::ButtonLabel
                | TypeRole::SupportingText
                | TypeRole::Metadata => {
                    assert_eq!(role.fallback_family(), iced::font::Family::SansSerif);
                }
                TypeRole::ChatMessage
                | TypeRole::ChatSender
                | TypeRole::ChatMetadata
                | TypeRole::ComposerText => {
                    assert_eq!(role.fallback_family(), iced::font::Family::SansSerif);
                }
                TypeRole::TechnicalValue => {
                    assert_eq!(role.fallback_family(), iced::font::Family::Monospace);
                }
                TypeRole::BrandWordmark => {
                    assert_eq!(role.fallback_family(), iced::font::Family::Name(RALEWAY));
                }
            }
            // Every fallback must be one of the approved FONTS-14 platform
            // families — no legacy family may reappear.
            let approved = [
                iced::font::Family::Name(ARIAL_NARROW),
                iced::font::Family::SansSerif,
                iced::font::Family::Monospace,
                iced::font::Family::Name(RALEWAY),
            ];
            assert!(
                approved.contains(&role.fallback_family()),
                "fallback family for {role:?} must be an approved FONTS-14 platform family"
            );
            let fw = role.fallback_weight();
            assert!(
                matches!(
                    fw,
                    Weight::Normal | Weight::Medium | Weight::Semibold | Weight::Bold
                ),
                "fallback weight {fw:?} not registered for the role's fallback family"
            );
        }
    }

    #[test]
    fn type_role_text_lh_builds_text_widget() {
        // The helper must produce a usable text widget with the role's
        // font/size and the requested relative line-height (private fields,
        // so the check is an API-level smoke test; the role mapping itself
        // is covered by the TypeRole tests above).
        let widget = type_role_text_lh(TypeRole::DisplayHeading, "Good evening", 1.2);
        let _ = widget;
    }

    #[test]
    fn type_role_has_all_plan_roles() {
        let labels: Vec<&str> = TypeRole::ALL.iter().map(|r| r.label()).collect();
        for expected in [
            "display_heading",
            "page_title",
            "section_title",
            "card_title",
            "body",
            "body_emphasised",
            "button_label",
            "supporting_text",
            "metadata",
            "chat_message",
            "chat_sender",
            "chat_metadata",
            "composer_text",
            "technical_value",
            "brand_wordmark",
        ] {
            assert!(
                labels.contains(&expected),
                "missing semantic role {expected}"
            );
        }
    }

    #[test]
    fn ui_roles_use_ibm_plex_sans() {
        // FONTS-04: every general-UI role maps to IBM Plex Sans (never the
        // legacy default font, removed in FONTS-12).
        let roles: &[TypeRole] = &[
            TypeRole::SectionTitle,
            TypeRole::CardTitle,
            TypeRole::Body,
            TypeRole::BodyEmphasised,
            TypeRole::ButtonLabel,
            TypeRole::SupportingText,
            TypeRole::Metadata,
        ];
        for role in roles {
            assert_eq!(
                role.family_name(),
                IBM_PLEX_SANS,
                "{role:?} should use IBM Plex Sans"
            );
        }
    }

    #[test]
    fn type_role_wordmark_uses_raleway() {
        assert_eq!(TypeRole::BrandWordmark.family_name(), RALEWAY);
        assert_eq!(TypeRole::BrandWordmark.weight(), Weight::ExtraBold);
        assert_eq!(TypeRole::BrandWordmark.size_px(), 28.0);
    }

    #[test]
    fn type_role_technical_uses_jetbrains_mono() {
        assert_eq!(TypeRole::TechnicalValue.family_name(), JETBRAINS_MONO);
        assert_eq!(TypeRole::TechnicalValue.weight(), Weight::Normal);
    }

    #[test]
    fn type_role_sizes_match_task16_baseline() {
        // FONTS Task 16 baseline (all within the approved ranges):
        //   DisplayHeading 32, PageTitle 28, CardTitle 17–18, Body 14–15,
        //   Button 14, Metadata 12–13, Chat body 15–16, Chat sender 14–15,
        //   Technical ID 12–14. DialogTitle 24–28 has no dedicated role —
        //   creation dialogs use the PageTitle family at the DIALOG_TITLE
        //   scale token (26 px, FONTS-11).
        assert_eq!(TypeRole::DisplayHeading.size_px(), 32.0);
        assert_eq!(TypeRole::PageTitle.size_px(), 28.0);
        assert_eq!(TypeRole::SectionTitle.size_px(), 20.0);
        assert_eq!(TypeRole::CardTitle.size_px(), 18.0);
        assert_eq!(TypeRole::Body.size_px(), 15.0);
        assert_eq!(TypeRole::BodyEmphasised.size_px(), 15.0);
        assert_eq!(TypeRole::ButtonLabel.size_px(), 14.0);
        assert_eq!(TypeRole::SupportingText.size_px(), 13.0);
        assert_eq!(TypeRole::Metadata.size_px(), 12.0);
        assert_eq!(TypeRole::ChatMessage.size_px(), 15.0);
        assert_eq!(TypeRole::ChatSender.size_px(), 14.0);
        assert_eq!(TypeRole::ChatMetadata.size_px(), 12.0);
        assert_eq!(TypeRole::ComposerText.size_px(), 15.0);
        assert_eq!(TypeRole::TechnicalValue.size_px(), 12.0);
        assert_eq!(TypeRole::BrandWordmark.size_px(), 28.0);
    }

    #[test]
    fn dialog_size_tokens_fit_task11_bands() {
        // FONTS Task 11: dialog title Archivo SemiCondensed Bold 24–28 px,
        // subtitle IBM Plex Sans Regular 14–15 px. The DIALOG_TITLE /
        // DIALOG_SUBTITLE scale tokens must sit inside those approved bands.
        assert!(
            (24.0..=28.0).contains(&DIALOG_TITLE),
            "DIALOG_TITLE {DIALOG_TITLE} must be within 24–28 px"
        );
        assert!(
            (14.0..=15.0).contains(&DIALOG_SUBTITLE),
            "DIALOG_SUBTITLE {DIALOG_SUBTITLE} must be within 14–15 px"
        );
        // The dialog title must resolve to the Archivo SemiCondensed Bold
        // family — the same family the PageTitle role uses — so callers can
        // apply `TypeRole::PageTitle.font()` with the DIALOG_TITLE size.
        assert_eq!(TypeRole::PageTitle.family_name(), ARCHIVO_SEMI_CONDENSED);
        assert_eq!(TypeRole::PageTitle.weight(), Weight::Bold);
    }

    #[test]
    fn type_role_font_matches_primary_family() {
        // `font()` must resolve to the role's primary registered family —
        // no role may slip back to a legacy family via the catch-all.
        for role in TypeRole::ALL {
            let font = role.font();
            match role.family_name() {
                ARCHIVO_SEMI_CONDENSED => {
                    assert_eq!(font.family, iced::font::Family::Name(ARCHIVO_SEMI_CONDENSED));
                }
                IBM_PLEX_SANS => {
                    assert_eq!(font.family, iced::font::Family::Name(IBM_PLEX_SANS));
                }
                FIGTREE => assert_eq!(font.family, iced::font::Family::Name(FIGTREE)),
                JETBRAINS_MONO => {
                    assert_eq!(font.family, iced::font::Family::Name(JETBRAINS_MONO));
                }
                RALEWAY => assert_eq!(font.family, iced::font::Family::Name(RALEWAY)),
                other => panic!("unexpected family {other}"),
            }
        }
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
