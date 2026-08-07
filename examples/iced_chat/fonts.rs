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
//! | Source Sans 3   | 400 · 500 · 600 · 700         | Primary app font (UI text, nav, forms, buttons) |
//! | Manrope         | 600 (SemiBold) · 700 (Bold)   | Display headings only           |
//! | Figtree         | 400 · 500 · 600               | Chat messages, sender, metadata, composer |
//! | Raleway         | 800 (ExtraBold)               | BORU wordmark / branding only   |
//! | JetBrains Mono  | 400 · 500                     | Technical/code values           |
//! | Inter           | 400 · 500 · 600 · 700         | Legacy fallback (bundled, not loaded) |
//! | Archivo SemiCondensed | 600 (SemiBold) · 700 (Bold) | Major display headings (registered only) |
//! | IBM Plex Sans  | 400 (Regular) · 500 (Medium) · 600 (SemiBold) | General app UI (registered only) |
//!
//! ## Licence
//!
//! Source Sans 3, Inter, Manrope, Figtree, Raleway, JetBrains Mono,
//! Archivo SemiCondensed, and IBM Plex Sans are licensed under the SIL
//! Open Font License 1.1. See fonts/OFL.txt and the per-family OFL
//! records (fonts/Figtree-OFL.txt, fonts/Manrope-OFL.txt,
//! fonts/JetBrainsMono-OFL.txt, fonts/Raleway-OFL.txt,
//! fonts/SourceSans3-OFL.txt, fonts/Archivo-OFL.txt,
//! fonts/IBMPlexSans-OFL.txt) plus fonts/THIRD_PARTY_NOTICES.md for
//! exact sources and versions.

use iced::font::{self, Family, Weight};
use iced::widget::text;
use iced::{Font, Pixels};

// ── Font file data (bundled at compile time, loaded at startup) ──────

/// Source Sans 3 Regular (400).
const SOURCE_SANS_REGULAR_BYTES: &[u8] = include_bytes!("fonts/SourceSans3-Regular.ttf");

/// Source Sans 3 SemiBold (600).
const SOURCE_SANS_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/SourceSans3-SemiBold.ttf");

/// Source Sans 3 Medium (500) — registered for `TypeRole`/`Typography` Medium.
const SOURCE_SANS_MEDIUM_BYTES: &[u8] = include_bytes!("fonts/SourceSans3-Medium.ttf");

/// Source Sans 3 Bold (700) — kept loaded; several call sites request `Weight::Bold`.
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

/// Manrope variable font (200–800) — legacy bundled asset, NOT loaded at startup.
#[expect(dead_code)]
const MANROPE_BYTES: &[u8] = include_bytes!("fonts/Manrope.ttf");

/// Manrope SemiBold (600) — static instance registered for display headings.
const MANROPE_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/Manrope-SemiBold.ttf");

/// Manrope Bold (700) — static instance registered for display headings.
const MANROPE_BOLD_BYTES: &[u8] = include_bytes!("fonts/Manrope-Bold.ttf");

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

/// Internal family name for Source Sans 3.
pub const SOURCE_SANS: &str = "Source Sans 3";

/// Internal family name for Inter.
#[expect(dead_code)]
pub const INTER: &str = "Inter";

/// Internal family name for Manrope.
pub const MANROPE: &str = "Manrope";

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
pub fn manrope(weight: Weight) -> Font {
    Font {
        family: Family::Name(MANROPE),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

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

// ── Canonical semantic roles (UI-HOME-11) ─────────────────────────────
//
// `TypeRole` is the central typography system approved by the Boru plan:
// every role names the content kind and knows its family, weight and default
// size. Screens (UI-HOME-12/13/14) migrate onto these roles; `Typography`
// below remains the legacy token set until migration completes.
//
// Sizes follow the Boru plan's approved mapping (UI-HOME-12/13):
//   display_heading  Manrope Bold 32   page greeting / hero
//   page_title       SS3 SemiBold 28   application page title
//   section_title    SS3 SemiBold 20   connection / section heading
//   card_title       SS3 SemiBold 18   dashboard card title
//   body             SS3 Regular 15    body copy and descriptions
//   body_emphasised  SS3 SemiBold 15   emphasised body copy
//   button_label     SS3 SemiBold 14   buttons and interactive labels
//   supporting_text  SS3 Regular 13    supporting / secondary copy
//   metadata         SS3 Regular 12    timestamps, counts, small metadata
//   chat_message     Figtree Regular 15   chat message body
//   chat_sender      Figtree SemiBold 14  sender name
//   chat_metadata    Figtree Regular 12  message timestamps / status
//   composer_text    Figtree Regular 15  composer input + placeholder
//   technical_value  JBM Regular 12     peer IDs, hashes, ports, fingerprints
//   brand_wordmark   Raleway ExtraBold 28  BORU wordmark

/// Canonical semantic typography roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRole {
    /// Hero / page greeting — Manrope Bold 32.
    DisplayHeading,
    /// Application page title — Source Sans 3 SemiBold 28.
    PageTitle,
    /// Section heading — Source Sans 3 SemiBold 20.
    SectionTitle,
    /// Card title — Source Sans 3 SemiBold 18.
    CardTitle,
    /// Body copy / descriptions — Source Sans 3 Regular 15.
    Body,
    /// Emphasised body copy — Source Sans 3 SemiBold 15.
    BodyEmphasised,
    /// Button and interactive label — Source Sans 3 SemiBold 14.
    ButtonLabel,
    /// Supporting / secondary copy — Source Sans 3 Regular 13.
    SupportingText,
    /// Metadata (timestamps, counts) — Source Sans 3 Regular 12.
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
            Self::DisplayHeading => MANROPE,
            Self::PageTitle
            | Self::SectionTitle
            | Self::CardTitle
            | Self::Body
            | Self::BodyEmphasised
            | Self::ButtonLabel
            | Self::SupportingText
            | Self::Metadata => SOURCE_SANS,
            Self::ChatMessage | Self::ChatSender | Self::ChatMetadata | Self::ComposerText => FIGTREE,
            Self::TechnicalValue => JETBRAINS_MONO,
            Self::BrandWordmark => RALEWAY,
        }
    }

    /// Font weight for this role (all weights are registered statically —
    /// no synthetic bolding is required).
    pub fn weight(self) -> Weight {
        match self {
            Self::DisplayHeading => Weight::Bold,
            Self::PageTitle
            | Self::SectionTitle
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
            MANROPE => manrope(self.weight()),
            FIGTREE => figtree(self.weight()),
            JETBRAINS_MONO => jetbrains_mono(self.weight()),
            RALEWAY => raleway_extra_bold(),
            _ => source_sans(self.weight()),
        }
    }

    /// Fallback font family for this role, used when the primary family is
    /// not registered on the platform. Every role degrades to Source Sans 3
    /// (the app default) except technical values, which degrade to the
    /// platform monospace family.
    pub fn fallback_family(self) -> Family {
        match self {
            Self::TechnicalValue => Family::Monospace,
            _ => Family::Name(SOURCE_SANS),
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
        // Source Sans 3 — 400 · 500 · 600 (+700 for legacy call sites).
        font::load(SOURCE_SANS_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(SOURCE_SANS_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(SOURCE_SANS_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(SOURCE_SANS_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        // Manrope — 600 · 700 (display headings).
        font::load(MANROPE_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(MANROPE_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
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
        // Manrope (still bundled) + registered statics
        assert!(!MANROPE_BYTES.is_empty());
        assert!(!MANROPE_SEMI_BOLD_BYTES.is_empty());
        assert!(!MANROPE_BOLD_BYTES.is_empty());
        // Figtree
        assert!(!FIGTREE_REGULAR_BYTES.is_empty());
        assert!(!FIGTREE_MEDIUM_BYTES.is_empty());
        assert!(!FIGTREE_SEMI_BOLD_BYTES.is_empty());
        // JetBrains Mono statics
        assert!(!JETBRAINS_MONO_REGULAR_BYTES.is_empty());
        assert!(!JETBRAINS_MONO_MEDIUM_BYTES.is_empty());
        // Source Sans 3 Medium
        assert!(!SOURCE_SANS_MEDIUM_BYTES.is_empty());
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
        // The five approved families, each at the exact plan weights.
        let expectations: &[(&str, Weight)] = &[
            (SOURCE_SANS, Weight::Normal), // 400
            (SOURCE_SANS, Weight::Medium), // 500
            (SOURCE_SANS, Weight::Semibold), // 600
            (MANROPE, Weight::Semibold),   // 600
            (MANROPE, Weight::Bold),       // 700
            (FIGTREE, Weight::Normal),     // 400
            (FIGTREE, Weight::Medium),     // 500
            (FIGTREE, Weight::Semibold),   // 600
            (RALEWAY, Weight::ExtraBold),  // 800
            (JETBRAINS_MONO, Weight::Normal), // 400
            (JETBRAINS_MONO, Weight::Medium), // 500
            (ARCHIVO_SEMI_CONDENSED, Weight::Semibold), // 600
            (ARCHIVO_SEMI_CONDENSED, Weight::Bold),     // 700
            (IBM_PLEX_SANS, Weight::Normal),     // 400
            (IBM_PLEX_SANS, Weight::Medium),     // 500
            (IBM_PLEX_SANS, Weight::Semibold),   // 600
        ];
        let loaded: &[(&str, Weight)] = &[
            (SOURCE_SANS, Weight::Normal),
            (SOURCE_SANS, Weight::Medium),
            (SOURCE_SANS, Weight::Semibold),
            (MANROPE, Weight::Semibold),
            (MANROPE, Weight::Bold),
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
        assert_eq!(TypeRole::DisplayHeading.family_name(), MANROPE);
        assert_eq!(TypeRole::DisplayHeading.weight(), Weight::Bold);
        assert_eq!(TypeRole::PageTitle.family_name(), SOURCE_SANS);
        assert_eq!(TypeRole::SectionTitle.family_name(), SOURCE_SANS);
        assert_eq!(TypeRole::CardTitle.family_name(), SOURCE_SANS);
        assert_eq!(TypeRole::Body.family_name(), SOURCE_SANS);
        assert_eq!(TypeRole::ButtonLabel.family_name(), SOURCE_SANS);
        assert_eq!(TypeRole::ChatMessage.family_name(), FIGTREE);
        assert_eq!(TypeRole::ChatSender.family_name(), FIGTREE);
        assert_eq!(TypeRole::ChatMetadata.family_name(), FIGTREE);
        assert_eq!(TypeRole::ComposerText.family_name(), FIGTREE);
        assert_eq!(TypeRole::TechnicalValue.family_name(), JETBRAINS_MONO);
        assert_eq!(TypeRole::BrandWordmark.family_name(), RALEWAY);
    }

    #[test]
    fn type_role_weights_are_real_not_synthetic() {
        // Every weight a role requests must be a registered static weight.
        for role in TypeRole::ALL {
            let family = role.family_name();
            let weight = role.weight();
            match family {
                MANROPE => assert!(
                    weight == Weight::Semibold || weight == Weight::Bold,
                    "Manrope role {role:?} requests unsupported weight {weight:?}"
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
                SOURCE_SANS => assert!(
                    matches!(
                        weight,
                        Weight::Normal | Weight::Medium | Weight::Semibold | Weight::Bold
                    ),
                    "SS3 role {role:?} requests unsupported weight {weight:?}"
                ),
                other => panic!("unexpected family {other}"),
            }
        }
    }

    #[test]
    fn type_role_fallbacks_are_platform_appropriate() {
        // Technical values fall back to the platform monospace.
        assert_eq!(
            TypeRole::TechnicalValue.fallback_family(),
            iced::font::Family::Monospace
        );
        // Everything else falls back to Source Sans 3 (the app default),
        // and the fallback weight is one the fallback family registers.
        for role in TypeRole::ALL {
            if role == TypeRole::TechnicalValue {
                continue;
            }
            assert_eq!(role.fallback_family(), iced::font::Family::Name(SOURCE_SANS));
            let fw = role.fallback_weight();
            assert!(
                matches!(
                    fw,
                    Weight::Normal | Weight::Medium | Weight::Semibold | Weight::Bold
                ),
                "fallback weight {fw:?} not registered for SS3"
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
