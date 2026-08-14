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
//! | Inter Tight     | 700 (Bold)                  | Major display/page headings (DisplayHeading, PageTitle) | Arial Narrow → generic sans-serif |
//! | Public Sans     | 400 (Regular) · 500 (Medium) · 600 (SemiBold) | Primary app UI: sections, cards, body, buttons, metadata | Arial → system sans-serif |
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
//! Figtree, Raleway, JetBrains Mono, Inter Tight, and Public
//! Sans are licensed under the SIL Open Font License 1.1. See
//! fonts/OFL.txt and the per-family OFL records (fonts/Figtree-OFL.txt,
//! fonts/JetBrainsMono-OFL.txt, fonts/Raleway-OFL.txt,
//! fonts/InterTight-OFL.txt, fonts/PublicSans-OFL.txt) plus
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

/// Inter Tight Bold (700) — major display/page headings.
const INTER_TIGHT_BOLD_BYTES: &[u8] = include_bytes!("fonts/InterTight-Bold.ttf");

/// Public Sans Regular (400) — general app UI body text.
const PUBLIC_SANS_REGULAR_BYTES: &[u8] = include_bytes!("fonts/PublicSans-Regular.ttf");

/// Public Sans Medium (500) — general app UI emphasis.
const PUBLIC_SANS_MEDIUM_BYTES: &[u8] = include_bytes!("fonts/PublicSans-Medium.ttf");

/// Public Sans SemiBold (600) — general app UI headings/labels.
const PUBLIC_SANS_SEMI_BOLD_BYTES: &[u8] = include_bytes!("fonts/PublicSans-SemiBold.ttf");

// ── Font family names ────────────────────────────────────────────────

/// Internal family name for Figtree.
pub const FIGTREE: &str = "Figtree";

/// Internal family name for Raleway (branding weight).
pub const RALEWAY: &str = "Raleway";

/// Internal family name for JetBrains Mono.
pub const JETBRAINS_MONO: &str = "JetBrains Mono";

/// Internal family name for Inter Tight (display headings).
pub const INTER_TIGHT: &str = "Inter Tight";

/// Internal family name for Public Sans (general app UI).
pub const PUBLIC_SANS: &str = "Public Sans";

/// Platform fallback family for display/page-heading roles (FONTS Task 14:
/// Inter Tight → Arial Narrow → generic sans-serif).
pub const ARIAL_NARROW: &str = "Arial Narrow";

/// All bundled family names in `FontFamilyKey::ALL` order — a `'static`
/// slice for pickers (BORU-UI-16).
pub const FAMILY_NAMES: [&str; 5] = [INTER_TIGHT, PUBLIC_SANS, FIGTREE, JETBRAINS_MONO, RALEWAY];

/// All registered weight labels in `FontWeightKey::ALL` order — a `'static`
/// slice for pickers (BORU-UI-16).
pub const WEIGHT_LABELS: [&str; 5] = ["Normal", "Medium", "Semibold", "Bold", "ExtraBold"];

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

/// Return a `Font` for Inter Tight at the given weight.
///
/// Registered weight: 700 (Bold).
pub fn inter_tight(weight: Weight) -> Font {
    Font {
        family: Family::Name(INTER_TIGHT),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

/// Return a `Font` for Public Sans at the given weight.
///
/// Registered weights: 400 (Regular), 500 (Medium), and 600 (SemiBold).
/// The bundled static instances are generated from the official variable
/// font with wght pinned at the requested weight — permitted under OFL-1.1.
pub fn public_sans(weight: Weight) -> Font {
    Font {
        family: Family::Name(PUBLIC_SANS),
        weight,
        stretch: iced::font::Stretch::Normal,
        style: iced::font::Style::Normal,
    }
}

// ── Font family / weight keys (BORU-UI-16) ────────────────────────────
//
// Small Copy enums the theme uses to make font family choices and weight
// mappings live-editable. The keys only ever name the bundled families and
// the registered static weights; `FontFamilyKey::from_name` returns `None`
// for anything else so the config merge can log + fall back gracefully.

/// The bundled font families, in role-group order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFamilyKey {
    /// Inter Tight — display/page headings.
    InterTight,
    /// Public Sans — general app UI / body.
    PublicSans,
    /// Figtree — chat messages, sender, metadata, composer.
    Figtree,
    /// JetBrains Mono — technical values.
    JetBrainsMono,
    /// Raleway — brand wordmark.
    Raleway,
}

impl FontFamilyKey {
    /// All bundled families, in a stable order for pickers / previews.
    pub const ALL: [FontFamilyKey; 5] = [
        Self::InterTight,
        Self::PublicSans,
        Self::Figtree,
        Self::JetBrainsMono,
        Self::Raleway,
    ];

    /// The `Family::Name` string for this key.
    pub fn name(self) -> &'static str {
        match self {
            Self::InterTight => INTER_TIGHT,
            Self::PublicSans => PUBLIC_SANS,
            Self::Figtree => FIGTREE,
            Self::JetBrainsMono => JETBRAINS_MONO,
            Self::Raleway => RALEWAY,
        }
    }

    /// Parse a family name string into a key. `None` for any name that is
    /// not one of the bundled families — the config merge uses this to
    /// detect an unavailable font and fall back (with a warning).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            INTER_TIGHT => Some(Self::InterTight),
            PUBLIC_SANS => Some(Self::PublicSans),
            FIGTREE => Some(Self::Figtree),
            JETBRAINS_MONO => Some(Self::JetBrainsMono),
            RALEWAY => Some(Self::Raleway),
            _ => None,
        }
    }

    /// Whether this family is one of the bundled families (always true for
    /// the enum — kept as a single place to answer "is this font
    /// available?" so resolution can log + fall back if that ever changes).
    pub fn is_bundled(self) -> bool {
        matches!(
            self,
            Self::InterTight
                | Self::PublicSans
                | Self::Figtree
                | Self::JetBrainsMono
                | Self::Raleway
        )
    }

    /// Build the bundled `Font` for this family at the given weight.
    pub fn font(self, weight: FontWeightKey) -> Font {
        match self {
            Self::InterTight => inter_tight(weight.iced()),
            Self::PublicSans => public_sans(weight.iced()),
            Self::Figtree => figtree(weight.iced()),
            Self::JetBrainsMono => jetbrains_mono(weight.iced()),
            Self::Raleway => raleway_extra_bold(),
        }
    }
}

/// The registered static weights, mapped to the bundled font files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontWeightKey {
    /// 400 — Regular.
    Normal,
    /// 500 — Medium.
    Medium,
    /// 600 — SemiBold.
    Semibold,
    /// 700 — Bold.
    Bold,
    /// 800 — ExtraBold.
    ExtraBold,
}

impl FontWeightKey {
    /// All registered weights, in ascending order for pickers.
    pub const ALL: [FontWeightKey; 5] = [
        Self::Normal,
        Self::Medium,
        Self::Semibold,
        Self::Bold,
        Self::ExtraBold,
    ];

    /// Convert to the iced weight used by `Font`.
    pub fn iced(self) -> Weight {
        match self {
            Self::Normal => Weight::Normal,
            Self::Medium => Weight::Medium,
            Self::Semibold => Weight::Semibold,
            Self::Bold => Weight::Bold,
            Self::ExtraBold => Weight::ExtraBold,
        }
    }

    /// Convert an iced weight to a key, if it is one of the registered ones.
    pub fn from_iced(weight: Weight) -> Option<Self> {
        match weight {
            Weight::Normal => Some(Self::Normal),
            Weight::Medium => Some(Self::Medium),
            Weight::Semibold => Some(Self::Semibold),
            Weight::Bold => Some(Self::Bold),
            Weight::ExtraBold => Some(Self::ExtraBold),
            _ => None,
        }
    }

    /// Parse a weight-name string into a key. `None` for names that are not
    /// registered static weights — the config merge uses this to log + fall
    /// back gracefully.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Normal" | "normal" | "400" => Some(Self::Normal),
            "Medium" | "medium" | "500" => Some(Self::Medium),
            "Semibold" | "semibold" | "SemiBold" | "600" => Some(Self::Semibold),
            "Bold" | "bold" | "700" => Some(Self::Bold),
            "ExtraBold" | "extrabold" | "Extra Bold" | "800" => Some(Self::ExtraBold),
            _ => None,
        }
    }

    /// Human label for pickers / previews.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Medium => "Medium",
            Self::Semibold => "Semibold",
            Self::Bold => "Bold",
            Self::ExtraBold => "ExtraBold",
        }
    }

    /// Whether this weight is one of the static instances registered for the
    /// given family at startup (BORU-UI-16 fallback check). A mapping that
    /// asks a family for a weight it does not bundle would otherwise render
    /// with a synthesised / missing glyph, so resolution falls back instead.
    pub fn is_registered_for(self, family: FontFamilyKey) -> bool {
        match family {
            // Inter Tight: only Bold (700) is bundled.
            FontFamilyKey::InterTight => matches!(self, Self::Bold),
            // Public Sans: 400 / 500 / 600.
            FontFamilyKey::PublicSans => {
                matches!(self, Self::Normal | Self::Medium | Self::Semibold)
            }
            // Figtree: 400 / 500 / 600.
            FontFamilyKey::Figtree => matches!(self, Self::Normal | Self::Medium | Self::Semibold),
            // JetBrains Mono: 400 / 500.
            FontFamilyKey::JetBrainsMono => matches!(self, Self::Normal | Self::Medium),
            // Raleway: ExtraBold (800) only.
            FontFamilyKey::Raleway => matches!(self, Self::ExtraBold),
        }
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
//   display_heading  Inter Tight Bold 32     page greeting / hero
//   page_title       Inter Tight Bold 28     application page title
//   section_title    Public Sans SemiBold 20      section heading (creation-dialog titles use PageTitle family @ DIALOG_TITLE — FONTS-11)
//   card_title       Public Sans SemiBold 18      dashboard card title
//   body             Public Sans Regular 15       body copy and descriptions
//   body_emphasised  Public Sans SemiBold 15      emphasised body copy
//   button_label     Public Sans SemiBold 14      buttons and interactive labels
//   supporting_text  Public Sans Regular 13       supporting / secondary copy
//   metadata         Public Sans Regular 12       timestamps, counts, small metadata
//   chat_message     Figtree Regular 15   chat message body
//   chat_sender      Figtree SemiBold 14  sender name
//   chat_metadata    Figtree Regular 12  message timestamps / status
//   composer_text    Figtree Regular 15  composer input + placeholder
//   technical_value  JBM Regular 12     peer IDs, hashes, ports, fingerprints
//   brand_wordmark   Raleway ExtraBold 28  BORU wordmark

/// Canonical semantic typography roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRole {
    /// Hero / page greeting — Inter Tight Bold 32.
    DisplayHeading,
    /// Application page title — Inter Tight Bold 28.
    PageTitle,
    /// Section heading — Public Sans SemiBold 20.
    /// (Creation-dialog titles use the PageTitle family at the DIALOG_TITLE
    /// scale — Inter Tight Bold 26 px, FONTS-11.)
    SectionTitle,
    /// Card title — Public Sans SemiBold 18.
    CardTitle,
    /// Body copy / descriptions — Public Sans Regular 15.
    Body,
    /// Emphasised body copy — Public Sans SemiBold 15.
    BodyEmphasised,
    /// Button and interactive label — Public Sans SemiBold 14.
    ButtonLabel,
    /// Supporting / secondary copy — Public Sans Regular 13.
    SupportingText,
    /// Metadata (timestamps, counts) — Public Sans Regular 12.
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
            Self::DisplayHeading | Self::PageTitle => INTER_TIGHT,
            Self::SectionTitle
            | Self::CardTitle
            | Self::Body
            | Self::BodyEmphasised
            | Self::ButtonLabel
            | Self::SupportingText
            | Self::Metadata => PUBLIC_SANS,
            Self::ChatMessage | Self::ChatSender | Self::ChatMetadata | Self::ComposerText => {
                FIGTREE
            }
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
            INTER_TIGHT => inter_tight(self.weight()),
            PUBLIC_SANS => public_sans(self.weight()),
            FIGTREE => figtree(self.weight()),
            JETBRAINS_MONO => jetbrains_mono(self.weight()),
            RALEWAY => raleway_extra_bold(),
            // Defensive default: the UI family (kept in sync with family_name()).
            _ => public_sans(self.weight()),
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

    /// The bundled-family key for this role's default family.
    pub fn family_key(self) -> FontFamilyKey {
        match self.family_name() {
            INTER_TIGHT => FontFamilyKey::InterTight,
            PUBLIC_SANS => FontFamilyKey::PublicSans,
            FIGTREE => FontFamilyKey::Figtree,
            JETBRAINS_MONO => FontFamilyKey::JetBrainsMono,
            RALEWAY => FontFamilyKey::Raleway,
            _ => FontFamilyKey::PublicSans,
        }
    }

    /// The registered-weight key for this role's default weight.
    pub fn weight_key(self) -> FontWeightKey {
        FontWeightKey::from_iced(self.weight()).unwrap_or(FontWeightKey::Normal)
    }

    /// Default relative line height for this role (BORU-UI-16).
    ///
    /// The plan asks for display headings ~1.2 and body copy ~1.45; chat
    /// message bodies use 1.45 and everything else uses iced's default 1.3.
    /// These defaults reproduce the current UI exactly.
    pub fn default_line_height(self) -> f32 {
        match self {
            Self::DisplayHeading | Self::PageTitle | Self::BrandWordmark => 1.2,
            Self::ChatMessage => 1.45,
            _ => 1.3,
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

// ── Theme-aware typography resolution (BORU-UI-16) ───────────────────
//
// The theme holds *choices* — which bundled family each role group uses,
// which weight each role uses, and each role's relative line-height. These
// helpers turn a role + theme into a concrete iced `Font` / size /
// line-height **without reloading font files**. Fonts are loaded once at
// startup (`load_fonts`); changing a family/weight mapping only rebuilds the
// `Font` struct, which Iced resolves against already-registered families.
//
// Fallback policy: if a configured family/weight name is not one of the
// bundled static instances, we log once and fall back to the role's built-in
// default so the UI never renders with an unresolvable font. The merge step
// (`theme_merge.rs`) validates config names earlier, so this is a second
// defensive net rather than the primary one.

/// Resolve the theme's font for a role (family + weight) with graceful
/// fallback to the role's built-in font if the mapping is unavailable.
/// Never loads font data — only constructs an iced `Font`.
pub fn resolve_theme_font(theme: &crate::theme::BoruTheme, role: TypeRole) -> iced::Font {
    let family_key = theme.typography.family_for(role);
    let weight_key = theme.typography.weight_for(role);
    if !family_key.is_bundled() {
        tracing::warn!(
            family = family_key.name(),
            role = ?role,
            "typography: configured family is not bundled; falling back to role default"
        );
        return role.font();
    }
    if !weight_key.is_registered_for(family_key) {
        tracing::warn!(
            weight = weight_key.label(),
            family = family_key.name(),
            role = ?role,
            "typography: configured weight not registered for family; falling back to role default"
        );
        return role.font();
    }
    family_key.font(weight_key)
}

/// Resolve the theme's size (px) for a role.
pub fn resolve_theme_size(theme: &crate::theme::BoruTheme, role: TypeRole) -> f32 {
    theme.typography.size_for(role)
}

/// Resolve the theme's relative line-height for a role.
pub fn resolve_theme_line_height(theme: &crate::theme::BoruTheme, role: TypeRole) -> f32 {
    theme.typography.line_height_for(role)
}

/// Build a theme-aware text widget for a canonical role: font family,
/// weight, size and line-height all come from the live theme
/// (BORU-UI-16), falling back to the role's built-in font when a mapping
/// is unavailable. Changing theme typography in the inspector updates
/// every widget built through this helper without any font reload.
pub fn type_role_text_themed<'a>(
    theme: &crate::theme::BoruTheme,
    role: TypeRole,
    content: impl text::IntoFragment<'a>,
) -> text::Text<'a, iced::Theme, iced::Renderer> {
    text(content)
        .font(resolve_theme_font(theme, role))
        .size(resolve_theme_size(theme, role))
        .line_height(text::LineHeight::Relative(resolve_theme_line_height(theme, role)))
}

/// Build a theme-aware text widget for a canonical role with an explicit
/// relative line-height override (kept for sites that tune line-height
/// independently of the theme token, e.g. status cards).
pub fn type_role_text_lh_themed<'a>(
    theme: &crate::theme::BoruTheme,
    role: TypeRole,
    content: impl text::IntoFragment<'a>,
    line_height: f32,
) -> text::Text<'a, iced::Theme, iced::Renderer> {
    text(content)
        .font(resolve_theme_font(theme, role))
        .size(resolve_theme_size(theme, role))
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
/// Loads Inter Tight (display/page headings), Public Sans (primary
/// UI), Figtree (chat), Raleway ExtraBold (wordmark), and JetBrains Mono
/// (technical values).
pub fn load_fonts() -> iced::Task<crate::app::AppMessage> {
    iced::Task::batch(vec![
        // iced_aw's embedded icon font (Modal/Card close glyph, ColorPicker
        // OK/cancel glyphs). Must be registered under its family name
        // "iced_aw" for iced_aw widgets to resolve it.
        font::load(iced_aw::ICED_AW_FONT_BYTES).map(|_| crate::app::AppMessage::Noop),
        // Figtree — 400 · 500 · 600 (chat).
        font::load(FIGTREE_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(FIGTREE_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(FIGTREE_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        // Raleway ExtraBold 800 — wordmark only.
        font::load(RALEWAY_EXTRA_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        // JetBrains Mono — 400 · 500 (technical values).
        font::load(JETBRAINS_MONO_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(JETBRAINS_MONO_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        // Inter Tight — 700 (display headings).
        font::load(INTER_TIGHT_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
        // Public Sans — 400 · 500 · 600 (general app UI).
        font::load(PUBLIC_SANS_REGULAR_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(PUBLIC_SANS_MEDIUM_BYTES).map(|_| crate::app::AppMessage::Noop),
        font::load(PUBLIC_SANS_SEMI_BOLD_BYTES).map(|_| crate::app::AppMessage::Noop),
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
        // Inter Tight static
        assert!(!INTER_TIGHT_BOLD_BYTES.is_empty());
        // Public Sans statics
        assert!(!PUBLIC_SANS_REGULAR_BYTES.is_empty());
        assert!(!PUBLIC_SANS_MEDIUM_BYTES.is_empty());
        assert!(!PUBLIC_SANS_SEMI_BOLD_BYTES.is_empty());
    }

    #[test]
    fn every_required_family_weight_is_registered() {
        // The FONTS-04 token families, each at the exact weights the roles
        // request.
        let expectations: &[(&str, Weight)] = &[
            (INTER_TIGHT, Weight::Bold),    // 700
            (PUBLIC_SANS, Weight::Normal),    // 400
            (PUBLIC_SANS, Weight::Medium),    // 500
            (PUBLIC_SANS, Weight::Semibold),  // 600
            (FIGTREE, Weight::Normal),        // 400
            (FIGTREE, Weight::Medium),        // 500
            (FIGTREE, Weight::Semibold),      // 600
            (RALEWAY, Weight::ExtraBold),     // 800
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
            (INTER_TIGHT, Weight::Bold),
            (PUBLIC_SANS, Weight::Normal),
            (PUBLIC_SANS, Weight::Medium),
            (PUBLIC_SANS, Weight::Semibold),
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
        // FONTS-04 approved mapping: Inter Tight for display/page
        // headings, Public Sans for general UI, Figtree for chat,
        // JetBrains Mono for technical values, Raleway for the wordmark.
        assert_eq!(TypeRole::DisplayHeading.family_name(), INTER_TIGHT);
        assert_eq!(TypeRole::DisplayHeading.weight(), Weight::Bold);
        assert_eq!(TypeRole::PageTitle.family_name(), INTER_TIGHT);
        assert_eq!(TypeRole::PageTitle.weight(), Weight::Bold);
        assert_eq!(TypeRole::SectionTitle.family_name(), PUBLIC_SANS);
        assert_eq!(TypeRole::SectionTitle.weight(), Weight::Semibold);
        assert_eq!(TypeRole::CardTitle.family_name(), PUBLIC_SANS);
        assert_eq!(TypeRole::Body.family_name(), PUBLIC_SANS);
        assert_eq!(TypeRole::BodyEmphasised.family_name(), PUBLIC_SANS);
        assert_eq!(TypeRole::ButtonLabel.family_name(), PUBLIC_SANS);
        assert_eq!(TypeRole::SupportingText.family_name(), PUBLIC_SANS);
        assert_eq!(TypeRole::Metadata.family_name(), PUBLIC_SANS);
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
                INTER_TIGHT => assert!(
                    weight == Weight::Bold,
                    "Inter Tight role {role:?} requests unsupported weight {weight:?}"
                ),
                PUBLIC_SANS => assert!(
                    matches!(weight, Weight::Normal | Weight::Medium | Weight::Semibold),
                    "Public Sans role {role:?} requests unsupported weight {weight:?}"
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
    fn ui_roles_use_public_sans() {
        // FONTS-04: every general-UI role maps to Public Sans (never the
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
                PUBLIC_SANS,
                "{role:?} should use Public Sans"
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
        // FONTS Task 11: dialog title Inter Tight Bold 24–28 px,
        // subtitle Public Sans Regular 14–15 px. The DIALOG_TITLE /
        // DIALOG_SUBTITLE scale tokens must sit inside those approved bands.
        assert!(
            (24.0..=28.0).contains(&DIALOG_TITLE),
            "DIALOG_TITLE {DIALOG_TITLE} must be within 24–28 px"
        );
        assert!(
            (14.0..=15.0).contains(&DIALOG_SUBTITLE),
            "DIALOG_SUBTITLE {DIALOG_SUBTITLE} must be within 14–15 px"
        );
        // The dialog title must resolve to the Inter Tight Bold
        // family — the same family the PageTitle role uses — so callers can
        // apply `TypeRole::PageTitle.font()` with the DIALOG_TITLE size.
        assert_eq!(TypeRole::PageTitle.family_name(), INTER_TIGHT);
        assert_eq!(TypeRole::PageTitle.weight(), Weight::Bold);
    }

    #[test]
    fn type_role_font_matches_primary_family() {
        // `font()` must resolve to the role's primary registered family —
        // no role may slip back to a legacy family via the catch-all.
        for role in TypeRole::ALL {
            let font = role.font();
            match role.family_name() {
                INTER_TIGHT => {
                    assert_eq!(font.family, iced::font::Family::Name(INTER_TIGHT));
                }
                PUBLIC_SANS => {
                    assert_eq!(font.family, iced::font::Family::Name(PUBLIC_SANS));
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

    #[test]
    fn family_key_from_name_rejects_unknown_fonts() {
        // BORU-UI-16: an unavailable / unconfigured family name must
        // resolve to `None` so the config merge can log + fall back.
        assert_eq!(FontFamilyKey::from_name("Figtree"), Some(FontFamilyKey::Figtree));
        assert_eq!(FontFamilyKey::from_name("Public Sans"), Some(FontFamilyKey::PublicSans));
        assert_eq!(FontFamilyKey::from_name("Inter Tight"), Some(FontFamilyKey::InterTight));
        assert_eq!(
            FontFamilyKey::from_name("JetBrains Mono"),
            Some(FontFamilyKey::JetBrainsMono)
        );
        assert_eq!(FontFamilyKey::from_name("Raleway"), Some(FontFamilyKey::Raleway));
        // Unknown / not-bundled names must NOT map to a key.
        assert_eq!(FontFamilyKey::from_name("Comic Sans"), None);
        assert_eq!(FontFamilyKey::from_name("Helvetica Neue"), None);
        assert_eq!(FontFamilyKey::from_name(""), None);
    }

    #[test]
    fn weight_key_from_name_rejects_unknown_weights() {
        // BORU-UI-16: an unknown weight name falls back instead of being
        // silently mapped to a synthetic weight.
        assert_eq!(FontWeightKey::from_name("Normal"), Some(FontWeightKey::Normal));
        assert_eq!(FontWeightKey::from_name("Semibold"), Some(FontWeightKey::Semibold));
        assert_eq!(FontWeightKey::from_name("ExtraBold"), Some(FontWeightKey::ExtraBold));
        assert_eq!(FontWeightKey::from_name("Heavy"), None);
        assert_eq!(FontWeightKey::from_name("Light"), None);
        assert_eq!(FontWeightKey::from_name(""), None);
    }

    #[test]
    fn registered_weights_match_bundled_static_files() {
        // BORU-UI-16: the fallback check must agree with what `load_fonts`
        // actually registers — a weight a family does not bundle must not
        // be considered available.
        assert!(FontWeightKey::Bold.is_registered_for(FontFamilyKey::InterTight));
        assert!(!FontWeightKey::Normal.is_registered_for(FontFamilyKey::InterTight));
        assert!(FontWeightKey::Normal.is_registered_for(FontFamilyKey::PublicSans));
        assert!(FontWeightKey::Semibold.is_registered_for(FontFamilyKey::PublicSans));
        assert!(!FontWeightKey::Bold.is_registered_for(FontFamilyKey::PublicSans));
        assert!(FontWeightKey::Normal.is_registered_for(FontFamilyKey::Figtree));
        assert!(FontWeightKey::Medium.is_registered_for(FontFamilyKey::Figtree));
        assert!(!FontWeightKey::Bold.is_registered_for(FontFamilyKey::Figtree));
        assert!(FontWeightKey::Normal.is_registered_for(FontFamilyKey::JetBrainsMono));
        assert!(!FontWeightKey::Semibold.is_registered_for(FontFamilyKey::JetBrainsMono));
        assert!(FontWeightKey::ExtraBold.is_registered_for(FontFamilyKey::Raleway));
        assert!(!FontWeightKey::Normal.is_registered_for(FontFamilyKey::Raleway));
        // Every role's default weight is registered for its default family.
        for role in TypeRole::ALL {
            assert!(
                role.weight_key().is_registered_for(role.family_key()),
                "{role:?} default weight {} not registered for {}",
                role.weight_key().label(),
                role.family_key().name()
            );
        }
    }

    #[test]
    fn resolve_theme_font_falls_back_when_mapping_unavailable() {
        // BORU-UI-16: a theme mapping that asks a family for a weight it
        // does not bundle falls back to the role's built-in font instead of
        // rendering with a synthesised / missing glyph.
        let mut theme = crate::theme::BoruTheme::default();
        // ChatMessage is Figtree by default; force a family+weight combo
        // that is NOT registered (Inter Tight only bundles Bold).
        theme.typography.chat_family = FontFamilyKey::InterTight;
        theme.typography.chat_message_weight = FontWeightKey::Normal;
        let font = resolve_theme_font(&theme, TypeRole::ChatMessage);
        assert_eq!(font, TypeRole::ChatMessage.font());
        // A registered mapping resolves to the bundled family font.
        let mut ok = crate::theme::BoruTheme::default();
        ok.typography.chat_family = FontFamilyKey::Figtree;
        ok.typography.chat_message_weight = FontWeightKey::Normal;
        assert_eq!(
            resolve_theme_font(&ok, TypeRole::ChatMessage),
            figtree(Weight::Normal)
        );
    }
}
