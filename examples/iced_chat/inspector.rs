//! Dev-only UI Inspector panel (BORU-UI-09 / PDF Task 9).
//!
//! A hidden developer panel that edits the **currently active BoruTheme in
//! memory** while the app runs. It is toggled with Ctrl+Shift+D and exposes:
//!
//! - sliders for continuous values (padding, radius, width, sizes);
//! - numeric text inputs where exact values are useful;
//! - toggles for optional visual features (`HomeTheme::show_activity_feed`);
//! - colour controls via a hex/RGBA text field (initial implementation).
//!
//! ## State boundary
//!
//! The panel NEVER mutates view-local state. Every edit becomes a normal Iced
//! message ([`InspectorMsg`]) handled in `app.rs`'s `update()`, which applies
//! the change to the stored [`UiThemeConfig`] overrides and recomputes the
//! live theme through the same seam the `boru-ui.toml` watcher uses
//! (`IcedChat::set_ui_theme_config`): default + overrides → merged
//! `BoruTheme`, theme revision bumped, normal state/update/view cycle
//! redraws affected widgets.
//!
//! ## Gating
//!
//! This module is declared `#[cfg(feature = "dev-ui")]` in `main.rs`, so
//! release builds do not compile any of it. The `dev-ui` cargo feature is the
//! deliberate opt-in that also turns the runtime dev gate on in every build
//! (BORU-UI-08), so the panel can only ever exist when the live editor is
//! enabled.
//!
//! ## Pure mapping (unit-tested)
//!
//! [`ThemeField`] identifies a theme leaf. [`apply_float`] / [`apply_bool`] /
//! [`apply_color`] are the pure message → theme-edit mapping: they mutate a
//! `UiThemeConfig` and return `Err` for a field that cannot hold the value.
//! The tests in this module exercise those mappings plus the merge round-trip.

use std::collections::{HashMap, HashSet};

use iced::widget::{
    button, container, pick_list, row, scrollable, slider, text, text_input, toggler, Space,
};
use iced::{Alignment, Color, Element, Length};

use crate::app::AppMessage;
use crate::theme::BoruTheme;
use crate::theme_config::{ColorValue, UiThemeConfig};

/// Panel width in px. Fixed so the inspector does not fight the app layout.
pub const INSPECTOR_PANEL_WIDTH: f32 = 320.0;

/// Which value type a theme leaf holds. Drives which control the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Continuous / exact float (px sizes, ratios, spacing).
    Float,
    /// Boolean optional visual feature (rendered as a toggle).
    Bool,
    /// Colour (rendered as a hex/RGBA text field + swatch).
    Color,
    /// Discrete choice from a fixed list (rendered as a pick_list).
    /// BORU-UI-16: font family and weight mappings.
    Choice,
    /// BORU-LAYOUT-08: whole number (column counts, FillPortion splits).
    /// Theme fields never use it; layout fields do.
    Int,
    /// BORU-LAYOUT-08: ordered list of home section names. Theme fields
    /// never use it; layout fields do.
    Sections,
}

/// Identifies one editable leaf of the typed theme.
///
/// Every variant maps 1:1 to a `BoruTheme` field (for display) and a
/// `UiThemeConfig` `Option` leaf (for editing). Variant names follow the
/// group + field naming convention of `theme.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeField {
    // ── Colours (ColorTokens) ──
    ColorCanvas,
    ColorSidebar,
    ColorSurface,
    ColorSurfaceElevated,
    ColorSurfaceSelected,
    ColorSurfaceHover,
    ColorSurfacePressed,
    ColorSurfaceSecondary,
    ColorInputBg,
    ColorBorderMuted,
    ColorBorderStrong,
    ColorTextPrimary,
    ColorTextSecondary,
    ColorTextMuted,
    ColorPrimary,
    ColorPrimaryHover,
    ColorPrimaryPressed,
    ColorPrimarySoft,
    ColorSuccess,
    ColorDanger,
    ColorWarning,
    ColorFocus,
    ColorDialogBackdrop,
    // ── Typography (TypographyTokens) ──
    TypeDisplayHeading,
    TypePageTitle,
    TypeSectionTitle,
    TypeCardTitle,
    TypeBody,
    TypeBodyEmphasised,
    TypeButtonLabel,
    TypeSupportingText,
    TypeMetadata,
    TypeChatMessage,
    TypeChatSender,
    TypeChatMetadata,
    TypeComposerText,
    TypeHomeSubtitle,
    TypeDialogTitle,
    TypeDialogSubtitle,
    // ── BORU-UI-16: font family choices per role group (Choice) ──
    TypeDisplayFamily,
    TypeUiFamily,
    TypeChatFamily,
    TypeTechnicalFamily,
    TypeBrandFamily,
    // ── BORU-UI-16: weight mapping per canonical role (Choice) ──
    TypeDisplayHeadingWeight,
    TypePageTitleWeight,
    TypeSectionTitleWeight,
    TypeCardTitleWeight,
    TypeBodyWeight,
    TypeBodyEmphasisedWeight,
    TypeButtonLabelWeight,
    TypeSupportingTextWeight,
    TypeMetadataWeight,
    TypeChatMessageWeight,
    TypeChatSenderWeight,
    TypeChatMetadataWeight,
    TypeComposerTextWeight,
    TypeTechnicalValueWeight,
    TypeBrandWordmarkWeight,
    // ── BORU-UI-16: line-height mapping per canonical role (Float) ──
    TypeDisplayHeadingLineHeight,
    TypePageTitleLineHeight,
    TypeSectionTitleLineHeight,
    TypeCardTitleLineHeight,
    TypeBodyLineHeight,
    TypeBodyEmphasisedLineHeight,
    TypeButtonLabelLineHeight,
    TypeSupportingTextLineHeight,
    TypeMetadataLineHeight,
    TypeChatMessageLineHeight,
    TypeChatSenderLineHeight,
    TypeChatMetadataLineHeight,
    TypeComposerTextLineHeight,
    TypeTechnicalValueLineHeight,
    TypeBrandWordmarkLineHeight,
    // ── Spacing (SpacingTokens) ──
    Space4,
    Space8,
    Space12,
    Space16,
    Space20,
    Space24,
    Space32,
    Space40,
    ControlHeight,
    // ── Radii (RadiusTokens) ──
    RadiusSm,
    RadiusMd,
    RadiusLg,
    RadiusXl,
    RadiusCard,
    RadiusPill,
    RadiusDialog,
    // ── Sidebar (SidebarTheme) ──
    SidebarWidth,
    SidebarWidthMin,
    SidebarWidthMax,
    SidebarInset,
    SidebarItemRadius,
    SidebarAvatarContainerRadius,
    SidebarNameSize,
    SidebarSectionLabelSize,
    // ── Home (HomeTheme) ──
    HomePeersBodyMin,
    HomeActivityRowHeight,
    HomeHeroGap,
    HomeQuickActionGap,
    HomeQuickActionIconSize,
    HomeQuickActionTitleSize,
    HomeQuickActionDescSize,
    HomeQuickActionDescLineHeight,
    HomeShowActivityFeed,
    // ── Chat (ChatTheme) ──
    ChatBubbleMaxWidth,
    ChatBubbleWidthRatio,
    ChatMessageMaxWidth,
    ChatImagePreviewMaxWidth,
    ChatImagePreviewMaxHeight,
    ChatGifThumbnailWidth,
    ChatGifThumbnailHeight,
    ChatEmojiPickerWidth,
    // ── Attachments (AttachmentTheme) ──
    AttachProgressBarGirth,
    AttachChipAvatarSize,
    AttachSearchWidthFull,
    AttachEmptyStateHeight,
    // ── Rooms (RoomTheme) ──
    RoomCatalogueRowHeight,
    RoomBannerWidth,
    RoomProgressGirth,
    // ── Tunnels (TunnelTheme) ──
    TunnelChipPaddingX,
    TunnelChipPaddingY,
    // ── Dialogs (DialogTheme) ──
    DialogPadding,
    DialogSpacing,
    DialogTitleSize,
    DialogBodySize,
    DialogControlPaddingX,
    // ── Calls (CallTheme) ──
    CallAvatarSize,
    CallPipW,
    CallPipH,
    CallControlsGap,
    // ── Controls (ControlTokens) ──
    ControlHeaderHeight,
    ControlSliderWidth,
    // ── Motion (MotionTokens) ──
    MotionSidebarFadeFrames,
}

/// Top-level component section of the inspector (BORU-UI-10 hierarchy).
///
/// Mirrors the component groups of the typed [`BoruTheme`]: the PDF's
/// Global / Sidebar / Home / Chat / Attachments / Rooms / Tunnels / Dialogs
/// plus the remaining typed-theme groups the inspector exposes (Calls,
/// Controls, Motion). `Global` groups the four global token families
/// (Colours, Typography, Spacing, Radii).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionId {
    /// Global token families: Colours, Typography, Spacing, Radii.
    Global,
    /// Sidebar / global shell.
    Sidebar,
    /// Home dashboard.
    Home,
    /// Chat message list + composer.
    Chat,
    /// File / shared / download / video attachments.
    Attachments,
    /// Public rooms / discover.
    Rooms,
    /// Tunnels.
    Tunnels,
    /// Dialogs.
    Dialogs,
    /// Call screens.
    Calls,
    /// Settings / generic controls.
    Controls,
    /// Presentation motion.
    Motion,
}

impl SectionId {
    /// Human-readable section label.
    pub fn label(self) -> &'static str {
        match self {
            SectionId::Global => "Global",
            SectionId::Sidebar => "Sidebar",
            SectionId::Home => "Home",
            SectionId::Chat => "Chat",
            SectionId::Attachments => "Attachments",
            SectionId::Rooms => "Rooms",
            SectionId::Tunnels => "Tunnels",
            SectionId::Dialogs => "Dialogs",
            SectionId::Calls => "Calls",
            SectionId::Controls => "Controls",
            SectionId::Motion => "Motion",
        }
    }
    /// The config groups this section owns. `Global` owns all four global
    /// token families; every other section owns exactly its typed group.
    /// Used by `reset_section` so Reset Section clears exactly this
    /// component group back to defaults.
    pub fn reset(self, config: &mut UiThemeConfig) {
        match self {
            SectionId::Global => {
                config.colors = None;
                config.typography = None;
                config.spacing = None;
                config.radii = None;
            }
            SectionId::Sidebar => config.sidebar = None,
            SectionId::Home => config.home = None,
            SectionId::Chat => config.chat = None,
            SectionId::Attachments => config.attachments = None,
            SectionId::Rooms => config.rooms = None,
            SectionId::Tunnels => config.tunnels = None,
            SectionId::Dialogs => config.dialogs = None,
            SectionId::Calls => config.calls = None,
            SectionId::Controls => config.controls = None,
            SectionId::Motion => config.motion = None,
        }
    }
}

/// Dev-only identity of a visible app component (BORU-UI-11).
///
/// The inspection mode lets the developer hover/click a supported component
/// to discover its component ID/name and jump the inspector to the section
/// that controls it. Each [`ComponentId`] maps 1:1 to the inspector
/// [`SectionId`] whose fields drive that component's appearance.
///
/// This is pure development metadata — it exists only under the `dev-ui`
/// cargo feature and never affects release behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentId {
    /// Left navigation sidebar / global shell.
    Sidebar,
    /// Home dashboard (chat list empty state).
    Home,
    /// Chat message list + composer.
    Chat,
    /// File sharing / download manager screens.
    Attachments,
    /// Public room directory / groups.
    Rooms,
    /// Tunnel management UI.
    Tunnels,
    /// Dialog overlays (room settings, create room, invite, etc.).
    Dialogs,
    /// Call screens (outgoing / active / incoming).
    Calls,
    /// Settings / generic controls.
    Controls,
}

impl ComponentId {
    /// Human-readable component name shown while inspecting.
    pub fn label(self) -> &'static str {
        match self {
            ComponentId::Sidebar => "Sidebar",
            ComponentId::Home => "Home",
            ComponentId::Chat => "Chat",
            ComponentId::Attachments => "Attachments",
            ComponentId::Rooms => "Rooms",
            ComponentId::Tunnels => "Tunnels",
            ComponentId::Dialogs => "Dialogs",
            ComponentId::Calls => "Calls",
            ComponentId::Controls => "Controls",
        }
    }

    /// The inspector section that controls this component's appearance.
    pub fn section(self) -> SectionId {
        match self {
            ComponentId::Sidebar => SectionId::Sidebar,
            ComponentId::Home => SectionId::Home,
            ComponentId::Chat => SectionId::Chat,
            ComponentId::Attachments => SectionId::Attachments,
            ComponentId::Rooms => SectionId::Rooms,
            ComponentId::Tunnels => SectionId::Tunnels,
            ComponentId::Dialogs => SectionId::Dialogs,
            ComponentId::Calls => SectionId::Calls,
            ComponentId::Controls => SectionId::Controls,
        }
    }
}

/// Scrollable ID of the inspector panel — used to jump to a section when a
/// component is selected in inspection mode (BORU-UI-11).
pub const INSPECTOR_SCROLL_ID: &str = "boru-inspector-scroll";

// ── Section scroll estimation (BORU-UI-11) ──────────────────────────────
//
// The inspector panel is a fixed-width scrollable column of sections. When a
// component is selected in inspection mode we scroll the panel so the
// component's section header lands near the top. Iced does not expose the
// rendered y-position of a child inside a scrollable, so we estimate it from
// the section structure: approximate per-row heights for the fixed panel
// width, summed in `SECTIONS` order. The estimate only needs to be close
// enough to bring the right section into view.

/// Approximate height of the panel chrome above the first section
/// (panel heading + reset row + top spacer).
const INSPECTOR_TOP_CHROME: f32 = 24.0 + 24.0 + 6.0;
/// Approximate height of a collapsible section header row.
const SECTION_HEADER_H: f32 = 26.0;
/// Approximate height of a sub-group header row.
const SUBGROUP_HEADER_H: f32 = 16.0;
/// Approximate height of a float field row (label + slider + input).
const FLOAT_ROW_H: f32 = 48.0;
/// Approximate height of a bool field row (single toggler line).
const BOOL_ROW_H: f32 = 26.0;
/// Approximate height of a colour field row (label + hex input).
const COLOR_ROW_H: f32 = 46.0;
/// Approximate height of a choice field row (label + pick_list).
const CHOICE_ROW_H: f32 = 46.0;
/// Approximate inter-row / inter-group spacing.
const ROW_GAP: f32 = 4.0;

/// Estimated y-offset (px) of a section header inside the inspector
/// scrollable, accounting for currently collapsed sections. Pure function so
/// it can be unit-tested; used to scroll the panel to a selected component's
/// section in inspection mode.
pub fn section_scroll_offset(section: SectionId, collapsed: &HashSet<SectionId>) -> f32 {
    let mut y = INSPECTOR_TOP_CHROME;
    for s in SECTIONS {
        if s.id == section {
            break;
        }
        y += section_estimated_height(s, collapsed.contains(&s.id));
    }
    y
}

/// Estimated rendered height of one inspector section.
fn section_estimated_height(section: &InspectorSection, is_collapsed: bool) -> f32 {
    let mut h = SECTION_HEADER_H;
    if is_collapsed {
        return h;
    }
    let multi = section.groups.len() > 1;
    for group in section.groups {
        if multi {
            h += SUBGROUP_HEADER_H + ROW_GAP;
        }
        for field in group.fields {
            h += match field.kind() {
                FieldKind::Float => FLOAT_ROW_H,
                FieldKind::Bool => BOOL_ROW_H,
                FieldKind::Color => COLOR_ROW_H,
                FieldKind::Choice => CHOICE_ROW_H,
                // Layout-only kinds are not used by the theme panel; the
                // layout panel below does not participate in section
                // scrolling, so the estimate is irrelevant.
                FieldKind::Int | FieldKind::Sections => FLOAT_ROW_H,
            } + ROW_GAP;
        }
        h += ROW_GAP;
    }
    h + ROW_GAP
}

impl ThemeField {
    /// Human-readable label shown beside the control.
    pub fn label(self) -> &'static str {
        use ThemeField::*;
        match self {
            ColorCanvas => "Canvas",
            ColorSidebar => "Sidebar",
            ColorSurface => "Surface",
            ColorSurfaceElevated => "Surface (elevated)",
            ColorSurfaceSelected => "Surface (selected)",
            ColorSurfaceHover => "Surface (hover)",
            ColorSurfacePressed => "Surface (pressed)",
            ColorSurfaceSecondary => "Surface (secondary)",
            ColorInputBg => "Input background",
            ColorBorderMuted => "Border (muted)",
            ColorBorderStrong => "Border (strong)",
            ColorTextPrimary => "Text (primary)",
            ColorTextSecondary => "Text (secondary)",
            ColorTextMuted => "Text (muted)",
            ColorPrimary => "Accent",
            ColorPrimaryHover => "Accent (hover)",
            ColorPrimaryPressed => "Accent (pressed)",
            ColorPrimarySoft => "Accent (soft)",
            ColorSuccess => "Success",
            ColorDanger => "Danger",
            ColorWarning => "Warning",
            ColorFocus => "Focus ring",
            ColorDialogBackdrop => "Dialog backdrop",
            TypeDisplayHeading => "Display heading",
            TypePageTitle => "Page title",
            TypeSectionTitle => "Section title",
            TypeCardTitle => "Card title",
            TypeBody => "Body",
            TypeBodyEmphasised => "Body (emphasised)",
            TypeButtonLabel => "Button label",
            TypeSupportingText => "Supporting text",
            TypeMetadata => "Metadata",
            TypeChatMessage => "Chat message",
            TypeChatSender => "Chat sender",
            TypeChatMetadata => "Chat metadata",
            TypeComposerText => "Composer text",
            TypeHomeSubtitle => "Home subtitle",
            TypeDialogTitle => "Dialog title",
            TypeDialogSubtitle => "Dialog subtitle",
            TypeDisplayFamily => "Display font family",
            TypeUiFamily => "UI font family",
            TypeChatFamily => "Chat font family",
            TypeTechnicalFamily => "Technical font family",
            TypeBrandFamily => "Brand font family",
            TypeDisplayHeadingWeight => "Display heading weight",
            TypePageTitleWeight => "Page title weight",
            TypeSectionTitleWeight => "Section title weight",
            TypeCardTitleWeight => "Card title weight",
            TypeBodyWeight => "Body weight",
            TypeBodyEmphasisedWeight => "Body emphasised weight",
            TypeButtonLabelWeight => "Button label weight",
            TypeSupportingTextWeight => "Supporting text weight",
            TypeMetadataWeight => "Metadata weight",
            TypeChatMessageWeight => "Chat message weight",
            TypeChatSenderWeight => "Chat sender weight",
            TypeChatMetadataWeight => "Chat metadata weight",
            TypeComposerTextWeight => "Composer weight",
            TypeTechnicalValueWeight => "Technical value weight",
            TypeBrandWordmarkWeight => "Wordmark weight",
            TypeDisplayHeadingLineHeight => "Display heading line height",
            TypePageTitleLineHeight => "Page title line height",
            TypeSectionTitleLineHeight => "Section title line height",
            TypeCardTitleLineHeight => "Card title line height",
            TypeBodyLineHeight => "Body line height",
            TypeBodyEmphasisedLineHeight => "Body emphasised line height",
            TypeButtonLabelLineHeight => "Button label line height",
            TypeSupportingTextLineHeight => "Supporting text line height",
            TypeMetadataLineHeight => "Metadata line height",
            TypeChatMessageLineHeight => "Chat message line height",
            TypeChatSenderLineHeight => "Chat sender line height",
            TypeChatMetadataLineHeight => "Chat metadata line height",
            TypeComposerTextLineHeight => "Composer line height",
            TypeTechnicalValueLineHeight => "Technical value line height",
            TypeBrandWordmarkLineHeight => "Wordmark line height",
            Space4 => "Space 4",
            Space8 => "Space 8",
            Space12 => "Space 12",
            Space16 => "Space 16",
            Space20 => "Space 20",
            Space24 => "Space 24",
            Space32 => "Space 32",
            Space40 => "Space 40",
            ControlHeight => "Control height",
            RadiusSm => "Radius sm",
            RadiusMd => "Radius md",
            RadiusLg => "Radius lg",
            RadiusXl => "Radius xl",
            RadiusCard => "Card radius",
            RadiusPill => "Pill radius",
            RadiusDialog => "Dialog radius",
            SidebarWidth => "Width",
            SidebarWidthMin => "Width (min)",
            SidebarWidthMax => "Width (max)",
            SidebarInset => "Inset",
            SidebarItemRadius => "Item radius",
            SidebarAvatarContainerRadius => "Avatar radius",
            SidebarNameSize => "Name size",
            SidebarSectionLabelSize => "Section label size",
            HomePeersBodyMin => "Peers body min",
            HomeActivityRowHeight => "Activity row height",
            HomeHeroGap => "Hero gap",
            HomeQuickActionGap => "Quick action gap",
            HomeQuickActionIconSize => "Quick action icon size",
            HomeQuickActionTitleSize => "Quick action title size",
            HomeQuickActionDescSize => "Quick action desc size",
            HomeQuickActionDescLineHeight => "Quick action desc line height",
            HomeShowActivityFeed => "Show Recent Activity feed",
            ChatBubbleMaxWidth => "Bubble max width",
            ChatBubbleWidthRatio => "Bubble width ratio",
            ChatMessageMaxWidth => "Message max width",
            ChatImagePreviewMaxWidth => "Image preview max width",
            ChatImagePreviewMaxHeight => "Image preview max height",
            ChatGifThumbnailWidth => "GIF thumbnail width",
            ChatGifThumbnailHeight => "GIF thumbnail height",
            ChatEmojiPickerWidth => "Emoji picker width",
            AttachProgressBarGirth => "Progress bar girth",
            AttachChipAvatarSize => "Chip avatar size",
            AttachSearchWidthFull => "Search width (full)",
            AttachEmptyStateHeight => "Empty state height",
            RoomCatalogueRowHeight => "Catalogue row height",
            RoomBannerWidth => "Banner width",
            RoomProgressGirth => "Progress girth",
            TunnelChipPaddingX => "Chip padding x",
            TunnelChipPaddingY => "Chip padding y",
            DialogPadding => "Padding",
            DialogSpacing => "Spacing",
            DialogTitleSize => "Title size",
            DialogBodySize => "Body size",
            DialogControlPaddingX => "Control padding x",
            CallAvatarSize => "Avatar size",
            CallPipW => "PiP width",
            CallPipH => "PiP height",
            CallControlsGap => "Controls gap",
            ControlHeaderHeight => "Header height",
            ControlSliderWidth => "Slider width",
            MotionSidebarFadeFrames => "Sidebar fade frames",
        }
    }

    /// Top-level component section the field is rendered under
    /// (BORU-UI-10 component hierarchy).
    pub fn section(self) -> SectionId {
        use ThemeField::*;
        match self {
            ColorCanvas
            | ColorSidebar
            | ColorSurface
            | ColorSurfaceElevated
            | ColorSurfaceSelected
            | ColorSurfaceHover
            | ColorSurfacePressed
            | ColorSurfaceSecondary
            | ColorInputBg
            | ColorBorderMuted
            | ColorBorderStrong
            | ColorTextPrimary
            | ColorTextSecondary
            | ColorTextMuted
            | ColorPrimary
            | ColorPrimaryHover
            | ColorPrimaryPressed
            | ColorPrimarySoft
            | ColorSuccess
            | ColorDanger
            | ColorWarning
            | ColorFocus
            | ColorDialogBackdrop => SectionId::Global,
            TypeDisplayHeading
            | TypePageTitle
            | TypeSectionTitle
            | TypeCardTitle
            | TypeBody
            | TypeBodyEmphasised
            | TypeButtonLabel
            | TypeSupportingText
            | TypeMetadata
            | TypeChatMessage
            | TypeChatSender
            | TypeChatMetadata
            | TypeComposerText
            | TypeHomeSubtitle
            | TypeDialogTitle
            | TypeDialogSubtitle
            | TypeDisplayFamily
            | TypeUiFamily
            | TypeChatFamily
            | TypeTechnicalFamily
            | TypeBrandFamily
            | TypeDisplayHeadingWeight
            | TypePageTitleWeight
            | TypeSectionTitleWeight
            | TypeCardTitleWeight
            | TypeBodyWeight
            | TypeBodyEmphasisedWeight
            | TypeButtonLabelWeight
            | TypeSupportingTextWeight
            | TypeMetadataWeight
            | TypeChatMessageWeight
            | TypeChatSenderWeight
            | TypeChatMetadataWeight
            | TypeComposerTextWeight
            | TypeTechnicalValueWeight
            | TypeBrandWordmarkWeight
            | TypeDisplayHeadingLineHeight
            | TypePageTitleLineHeight
            | TypeSectionTitleLineHeight
            | TypeCardTitleLineHeight
            | TypeBodyLineHeight
            | TypeBodyEmphasisedLineHeight
            | TypeButtonLabelLineHeight
            | TypeSupportingTextLineHeight
            | TypeMetadataLineHeight
            | TypeChatMessageLineHeight
            | TypeChatSenderLineHeight
            | TypeChatMetadataLineHeight
            | TypeComposerTextLineHeight
            | TypeTechnicalValueLineHeight
            | TypeBrandWordmarkLineHeight => SectionId::Global,
            Space4 | Space8 | Space12 | Space16 | Space20 | Space24 | Space32 | Space40
            | ControlHeight => SectionId::Global,
            RadiusSm | RadiusMd | RadiusLg | RadiusXl | RadiusCard | RadiusPill | RadiusDialog => {
                SectionId::Global
            }
            SidebarWidth | SidebarWidthMin | SidebarWidthMax | SidebarInset | SidebarItemRadius
            | SidebarAvatarContainerRadius | SidebarNameSize | SidebarSectionLabelSize => {
                SectionId::Sidebar
            }
            HomePeersBodyMin | HomeActivityRowHeight | HomeHeroGap | HomeQuickActionGap
            | HomeQuickActionIconSize | HomeQuickActionTitleSize | HomeQuickActionDescSize
            | HomeQuickActionDescLineHeight | HomeShowActivityFeed => SectionId::Home,
            ChatBubbleMaxWidth | ChatBubbleWidthRatio | ChatMessageMaxWidth
            | ChatImagePreviewMaxWidth | ChatImagePreviewMaxHeight | ChatGifThumbnailWidth
            | ChatGifThumbnailHeight | ChatEmojiPickerWidth => SectionId::Chat,
            AttachProgressBarGirth | AttachChipAvatarSize | AttachSearchWidthFull
            | AttachEmptyStateHeight => SectionId::Attachments,
            RoomCatalogueRowHeight | RoomBannerWidth | RoomProgressGirth => SectionId::Rooms,
            TunnelChipPaddingX | TunnelChipPaddingY => SectionId::Tunnels,
            DialogPadding | DialogSpacing | DialogTitleSize | DialogBodySize
            | DialogControlPaddingX => SectionId::Dialogs,
            CallAvatarSize | CallPipW | CallPipH | CallControlsGap => SectionId::Calls,
            ControlHeaderHeight | ControlSliderWidth => SectionId::Controls,
            MotionSidebarFadeFrames => SectionId::Motion,
        }
    }

    /// Value type driving which control is rendered.
    pub fn kind(self) -> FieldKind {
        use ThemeField::*;
        match self {
            HomeShowActivityFeed => FieldKind::Bool,
            // BORU-UI-16: family choices + weight mappings are discrete.
            TypeDisplayFamily
            | TypeUiFamily
            | TypeChatFamily
            | TypeTechnicalFamily
            | TypeBrandFamily
            | TypeDisplayHeadingWeight
            | TypePageTitleWeight
            | TypeSectionTitleWeight
            | TypeCardTitleWeight
            | TypeBodyWeight
            | TypeBodyEmphasisedWeight
            | TypeButtonLabelWeight
            | TypeSupportingTextWeight
            | TypeMetadataWeight
            | TypeChatMessageWeight
            | TypeChatSenderWeight
            | TypeChatMetadataWeight
            | TypeComposerTextWeight
            | TypeTechnicalValueWeight
            | TypeBrandWordmarkWeight => FieldKind::Choice,
            ColorCanvas
            | ColorSidebar
            | ColorSurface
            | ColorSurfaceElevated
            | ColorSurfaceSelected
            | ColorSurfaceHover
            | ColorSurfacePressed
            | ColorSurfaceSecondary
            | ColorInputBg
            | ColorBorderMuted
            | ColorBorderStrong
            | ColorTextPrimary
            | ColorTextSecondary
            | ColorTextMuted
            | ColorPrimary
            | ColorPrimaryHover
            | ColorPrimaryPressed
            | ColorPrimarySoft
            | ColorSuccess
            | ColorDanger
            | ColorWarning
            | ColorFocus
            | ColorDialogBackdrop => FieldKind::Color,
            _ => FieldKind::Float,
        }
    }

    /// Slider bounds for float fields (a sane band around the theme value).
    pub fn range(self) -> (f32, f32) {
        use ThemeField::*;
        match self {
            TypeDisplayHeading | TypePageTitle | TypeDialogTitle => (16.0, 64.0),
            TypeSectionTitle | TypeCardTitle | TypeHomeSubtitle => (12.0, 40.0),
            TypeBody | TypeBodyEmphasised | TypeComposerText | TypeChatMessage | TypeChatSender => {
                (8.0, 32.0)
            }
            TypeButtonLabel | TypeSupportingText | TypeMetadata | TypeChatMetadata => (8.0, 24.0),
            TypeDialogSubtitle => (10.0, 24.0),
            // BORU-UI-16: relative line-height multipliers.
            TypeDisplayHeadingLineHeight
            | TypePageTitleLineHeight
            | TypeSectionTitleLineHeight
            | TypeCardTitleLineHeight
            | TypeBodyLineHeight
            | TypeBodyEmphasisedLineHeight
            | TypeButtonLabelLineHeight
            | TypeSupportingTextLineHeight
            | TypeMetadataLineHeight
            | TypeChatMessageLineHeight
            | TypeChatSenderLineHeight
            | TypeChatMetadataLineHeight
            | TypeComposerTextLineHeight
            | TypeTechnicalValueLineHeight
            | TypeBrandWordmarkLineHeight => (0.5, 4.0),
            Space4 | Space8 | Space12 | Space16 | Space20 | Space24 | Space32 | Space40 => {
                (0.0, 64.0)
            }
            ControlHeight => (20.0, 64.0),
            RadiusSm | RadiusMd | RadiusLg | RadiusXl | RadiusCard | RadiusPill | RadiusDialog => {
                (0.0, 48.0)
            }
            SidebarWidth | SidebarWidthMin | SidebarWidthMax => (80.0, 2000.0),
            SidebarInset => (0.0, 64.0),
            SidebarItemRadius => (0.0, 32.0),
            SidebarAvatarContainerRadius => (0.0, 48.0),
            SidebarNameSize => (10.0, 28.0),
            SidebarSectionLabelSize => (8.0, 20.0),
            HomePeersBodyMin => (0.0, 400.0),
            HomeActivityRowHeight => (16.0, 64.0),
            HomeHeroGap | HomeQuickActionGap => (0.0, 80.0),
            HomeQuickActionIconSize => (16.0, 96.0),
            HomeQuickActionTitleSize | HomeQuickActionDescSize => (8.0, 48.0),
            HomeQuickActionDescLineHeight => (1.0, 3.0),
            ChatBubbleMaxWidth | ChatMessageMaxWidth => (100.0, 1200.0),
            ChatBubbleWidthRatio => (0.3, 1.0),
            ChatImagePreviewMaxWidth | ChatImagePreviewMaxHeight => (80.0, 800.0),
            ChatGifThumbnailWidth | ChatGifThumbnailHeight | ChatEmojiPickerWidth => (40.0, 600.0),
            AttachProgressBarGirth | AttachChipAvatarSize => (0.0, 48.0),
            AttachSearchWidthFull => (80.0, 800.0),
            AttachEmptyStateHeight => (0.0, 512.0),
            RoomCatalogueRowHeight => (16.0, 160.0),
            RoomBannerWidth => (80.0, 800.0),
            RoomProgressGirth => (0.0, 32.0),
            TunnelChipPaddingX | TunnelChipPaddingY => (0.0, 64.0),
            DialogPadding | DialogSpacing => (0.0, 128.0),
            DialogTitleSize => (12.0, 48.0),
            DialogBodySize => (8.0, 32.0),
            DialogControlPaddingX => (0.0, 64.0),
            CallAvatarSize => (32.0, 256.0),
            CallPipW | CallPipH => (0.0, 1024.0),
            CallControlsGap => (0.0, 160.0),
            ControlHeaderHeight => (24.0, 160.0),
            ControlSliderWidth => (40.0, 600.0),
            MotionSidebarFadeFrames => (0.0, 240.0),
            // Colour / toggle fields never use the slider range; the panel
            // renders a hex field / toggle instead.
            _ => (0.0, 1.0),
        }
    }
}

// ── Read from the ACTIVE theme (display side) ─────────────────────────

/// Read a float leaf from the active theme.
pub fn read_float(theme: &BoruTheme, field: ThemeField) -> f32 {
    use ThemeField::*;
    match field {
        TypeDisplayHeading => theme.typography.display_heading,
        TypePageTitle => theme.typography.page_title,
        TypeSectionTitle => theme.typography.section_title,
        TypeCardTitle => theme.typography.card_title,
        TypeBody => theme.typography.body,
        TypeBodyEmphasised => theme.typography.body_emphasised,
        TypeButtonLabel => theme.typography.button_label,
        TypeSupportingText => theme.typography.supporting_text,
        TypeMetadata => theme.typography.metadata,
        TypeChatMessage => theme.typography.chat_message,
        TypeChatSender => theme.typography.chat_sender,
        TypeChatMetadata => theme.typography.chat_metadata,
        TypeComposerText => theme.typography.composer_text,
        TypeHomeSubtitle => theme.typography.home_subtitle,
        TypeDialogTitle => theme.typography.dialog_title,
        TypeDialogSubtitle => theme.typography.dialog_subtitle,
        TypeDisplayHeadingLineHeight => theme.typography.display_heading_line_height,
        TypePageTitleLineHeight => theme.typography.page_title_line_height,
        TypeSectionTitleLineHeight => theme.typography.section_title_line_height,
        TypeCardTitleLineHeight => theme.typography.card_title_line_height,
        TypeBodyLineHeight => theme.typography.body_line_height,
        TypeBodyEmphasisedLineHeight => theme.typography.body_emphasised_line_height,
        TypeButtonLabelLineHeight => theme.typography.button_label_line_height,
        TypeSupportingTextLineHeight => theme.typography.supporting_text_line_height,
        TypeMetadataLineHeight => theme.typography.metadata_line_height,
        TypeChatMessageLineHeight => theme.typography.chat_message_line_height,
        TypeChatSenderLineHeight => theme.typography.chat_sender_line_height,
        TypeChatMetadataLineHeight => theme.typography.chat_metadata_line_height,
        TypeComposerTextLineHeight => theme.typography.composer_text_line_height,
        TypeTechnicalValueLineHeight => theme.typography.technical_value_line_height,
        TypeBrandWordmarkLineHeight => theme.typography.brand_wordmark_line_height,
        Space4 => theme.spacing.space_4,
        Space8 => theme.spacing.space_8,
        Space12 => theme.spacing.space_12,
        Space16 => theme.spacing.space_16,
        Space20 => theme.spacing.space_20,
        Space24 => theme.spacing.space_24,
        Space32 => theme.spacing.space_32,
        Space40 => theme.spacing.space_40,
        ControlHeight => theme.spacing.control_height,
        RadiusSm => theme.radii.sm,
        RadiusMd => theme.radii.md,
        RadiusLg => theme.radii.lg,
        RadiusXl => theme.radii.xl,
        RadiusCard => theme.radii.card,
        RadiusPill => theme.radii.pill,
        RadiusDialog => theme.radii.dialog,
        SidebarWidth => theme.sidebar.width,
        SidebarWidthMin => theme.sidebar.width_min,
        SidebarWidthMax => theme.sidebar.width_max,
        SidebarInset => theme.sidebar.inset,
        SidebarItemRadius => theme.sidebar.item_radius,
        SidebarAvatarContainerRadius => theme.sidebar.avatar_container_radius,
        SidebarNameSize => theme.sidebar.name_size,
        SidebarSectionLabelSize => theme.sidebar.section_label_size,
        HomePeersBodyMin => theme.home.peers_body_min,
        HomeActivityRowHeight => theme.home.activity_row_height,
        HomeHeroGap => theme.home.hero_gap,
        HomeQuickActionGap => theme.home.quick_action_gap,
        HomeQuickActionIconSize => theme.home.quick_action_icon_size,
        HomeQuickActionTitleSize => theme.home.quick_action_title_size,
        HomeQuickActionDescSize => theme.home.quick_action_desc_size,
        HomeQuickActionDescLineHeight => theme.home.quick_action_desc_line_height,
        ChatBubbleMaxWidth => theme.chat.bubble_max_width,
        ChatBubbleWidthRatio => theme.chat.bubble_width_ratio,
        ChatMessageMaxWidth => theme.chat.message_max_width,
        ChatImagePreviewMaxWidth => theme.chat.image_preview_max_width,
        ChatImagePreviewMaxHeight => theme.chat.image_preview_max_height,
        ChatGifThumbnailWidth => theme.chat.gif_thumbnail_width,
        ChatGifThumbnailHeight => theme.chat.gif_thumbnail_height,
        ChatEmojiPickerWidth => theme.chat.emoji_picker_width,
        AttachProgressBarGirth => theme.attachments.progress_bar_girth,
        AttachChipAvatarSize => theme.attachments.chip_avatar_size,
        AttachSearchWidthFull => theme.attachments.search_width_full,
        AttachEmptyStateHeight => theme.attachments.empty_state_height,
        RoomCatalogueRowHeight => theme.rooms.catalogue_row_height,
        RoomBannerWidth => theme.rooms.banner_width,
        RoomProgressGirth => theme.rooms.progress_girth,
        TunnelChipPaddingX => theme.tunnels.chip_padding_x,
        TunnelChipPaddingY => theme.tunnels.chip_padding_y,
        DialogPadding => theme.dialogs.padding,
        DialogSpacing => theme.dialogs.spacing,
        DialogTitleSize => theme.dialogs.title_size,
        DialogBodySize => theme.dialogs.body_size,
        DialogControlPaddingX => theme.dialogs.control_padding_x,
        CallAvatarSize => theme.calls.avatar_size,
        CallPipW => theme.calls.pip_w,
        CallPipH => theme.calls.pip_h,
        CallControlsGap => theme.calls.controls_gap,
        ControlHeaderHeight => theme.controls.header_height,
        ControlSliderWidth => theme.controls.slider_width,
        MotionSidebarFadeFrames => theme.motion.sidebar_fade_frames as f32,
        // Bool / Colour / Choice fields have no float read; the caller
        // checks `kind()` first.
        HomeShowActivityFeed | ColorCanvas | ColorSidebar | ColorSurface | ColorSurfaceElevated
        | ColorSurfaceSelected
        | ColorSurfaceHover | ColorSurfacePressed | ColorSurfaceSecondary | ColorInputBg
        | ColorBorderMuted | ColorBorderStrong | ColorTextPrimary | ColorTextSecondary
        | ColorTextMuted | ColorPrimary | ColorPrimaryHover | ColorPrimaryPressed | ColorPrimarySoft
        | ColorSuccess | ColorDanger | ColorWarning | ColorFocus | ColorDialogBackdrop
        | TypeDisplayFamily | TypeUiFamily | TypeChatFamily | TypeTechnicalFamily | TypeBrandFamily
        | TypeDisplayHeadingWeight | TypePageTitleWeight | TypeSectionTitleWeight
        | TypeCardTitleWeight | TypeBodyWeight | TypeBodyEmphasisedWeight | TypeButtonLabelWeight
        | TypeSupportingTextWeight | TypeMetadataWeight | TypeChatMessageWeight
        | TypeChatSenderWeight | TypeChatMetadataWeight | TypeComposerTextWeight
        | TypeTechnicalValueWeight | TypeBrandWordmarkWeight => 0.0,
    }
}

/// Read a colour leaf from the active theme.
pub fn read_color(theme: &BoruTheme, field: ThemeField) -> Color {
    use ThemeField::*;
    match field {
        ColorCanvas => theme.colors.canvas,
        ColorSidebar => theme.colors.sidebar,
        ColorSurface => theme.colors.surface,
        ColorSurfaceElevated => theme.colors.surface_elevated,
        ColorSurfaceSelected => theme.colors.surface_selected,
        ColorSurfaceHover => theme.colors.surface_hover,
        ColorSurfacePressed => theme.colors.surface_pressed,
        ColorSurfaceSecondary => theme.colors.surface_secondary,
        ColorInputBg => theme.colors.input_bg,
        ColorBorderMuted => theme.colors.border_muted,
        ColorBorderStrong => theme.colors.border_strong,
        ColorTextPrimary => theme.colors.text_primary,
        ColorTextSecondary => theme.colors.text_secondary,
        ColorTextMuted => theme.colors.text_muted,
        ColorPrimary => theme.colors.primary,
        ColorPrimaryHover => theme.colors.primary_hover,
        ColorPrimaryPressed => theme.colors.primary_pressed,
        ColorPrimarySoft => theme.colors.primary_soft,
        ColorSuccess => theme.colors.success,
        ColorDanger => theme.colors.danger,
        ColorWarning => theme.colors.warning,
        ColorFocus => theme.colors.focus,
        ColorDialogBackdrop => theme.colors.dialog_backdrop,
        _ => Color::TRANSPARENT,
    }
}

/// Read a boolean optional-visual-feature leaf from the active theme.
pub fn read_bool(theme: &BoruTheme, field: ThemeField) -> bool {
    match field {
        ThemeField::HomeShowActivityFeed => theme.home.show_activity_feed,
        _ => false,
    }
}

// ── Apply to the config (message → theme-edit mapping) ────────────────

/// Apply a float edit to the stored `UiThemeConfig` overrides.
///
/// This is the pure mapping half of the inspector: it mutates only the
/// config (never any view state). `app.rs` calls it and then recomputes the
/// merged theme via `set_ui_theme_config`.
pub fn apply_float(config: &mut UiThemeConfig, field: ThemeField, value: f32) -> Result<(), String> {
    use ThemeField::*;
    if !matches!(field.kind(), FieldKind::Float) {
        return Err(format!("{} is not a float field", field.label()));
    }
    let set = |slot: &mut Option<f32>| *slot = Some(value);
    match field {
        TypeDisplayHeading => set(&mut cfg_typography(config).display_heading),
        TypePageTitle => set(&mut cfg_typography(config).page_title),
        TypeSectionTitle => set(&mut cfg_typography(config).section_title),
        TypeCardTitle => set(&mut cfg_typography(config).card_title),
        TypeBody => set(&mut cfg_typography(config).body),
        TypeBodyEmphasised => set(&mut cfg_typography(config).body_emphasised),
        TypeButtonLabel => set(&mut cfg_typography(config).button_label),
        TypeSupportingText => set(&mut cfg_typography(config).supporting_text),
        TypeMetadata => set(&mut cfg_typography(config).metadata),
        TypeChatMessage => set(&mut cfg_typography(config).chat_message),
        TypeChatSender => set(&mut cfg_typography(config).chat_sender),
        TypeChatMetadata => set(&mut cfg_typography(config).chat_metadata),
        TypeComposerText => set(&mut cfg_typography(config).composer_text),
        TypeHomeSubtitle => set(&mut cfg_typography(config).home_subtitle),
        TypeDialogTitle => set(&mut cfg_typography(config).dialog_title),
        TypeDialogSubtitle => set(&mut cfg_typography(config).dialog_subtitle),
        TypeDisplayHeadingLineHeight => {
            set(&mut cfg_typography(config).display_heading_line_height)
        }
        TypePageTitleLineHeight => set(&mut cfg_typography(config).page_title_line_height),
        TypeSectionTitleLineHeight => set(&mut cfg_typography(config).section_title_line_height),
        TypeCardTitleLineHeight => set(&mut cfg_typography(config).card_title_line_height),
        TypeBodyLineHeight => set(&mut cfg_typography(config).body_line_height),
        TypeBodyEmphasisedLineHeight => {
            set(&mut cfg_typography(config).body_emphasised_line_height)
        }
        TypeButtonLabelLineHeight => set(&mut cfg_typography(config).button_label_line_height),
        TypeSupportingTextLineHeight => {
            set(&mut cfg_typography(config).supporting_text_line_height)
        }
        TypeMetadataLineHeight => set(&mut cfg_typography(config).metadata_line_height),
        TypeChatMessageLineHeight => set(&mut cfg_typography(config).chat_message_line_height),
        TypeChatSenderLineHeight => set(&mut cfg_typography(config).chat_sender_line_height),
        TypeChatMetadataLineHeight => set(&mut cfg_typography(config).chat_metadata_line_height),
        TypeComposerTextLineHeight => set(&mut cfg_typography(config).composer_text_line_height),
        TypeTechnicalValueLineHeight => {
            set(&mut cfg_typography(config).technical_value_line_height)
        }
        TypeBrandWordmarkLineHeight => set(&mut cfg_typography(config).brand_wordmark_line_height),
        Space4 => set(&mut cfg_spacing(config).space_4),
        Space8 => set(&mut cfg_spacing(config).space_8),
        Space12 => set(&mut cfg_spacing(config).space_12),
        Space16 => set(&mut cfg_spacing(config).space_16),
        Space20 => set(&mut cfg_spacing(config).space_20),
        Space24 => set(&mut cfg_spacing(config).space_24),
        Space32 => set(&mut cfg_spacing(config).space_32),
        Space40 => set(&mut cfg_spacing(config).space_40),
        ControlHeight => set(&mut cfg_spacing(config).control_height),
        RadiusSm => set(&mut cfg_radii(config).sm),
        RadiusMd => set(&mut cfg_radii(config).md),
        RadiusLg => set(&mut cfg_radii(config).lg),
        RadiusXl => set(&mut cfg_radii(config).xl),
        RadiusCard => set(&mut cfg_radii(config).card),
        RadiusPill => set(&mut cfg_radii(config).pill),
        RadiusDialog => set(&mut cfg_radii(config).dialog),
        SidebarWidth => set(&mut cfg_sidebar(config).width),
        SidebarWidthMin => set(&mut cfg_sidebar(config).width_min),
        SidebarWidthMax => set(&mut cfg_sidebar(config).width_max),
        SidebarInset => set(&mut cfg_sidebar(config).inset),
        SidebarItemRadius => set(&mut cfg_sidebar(config).item_radius),
        SidebarAvatarContainerRadius => set(&mut cfg_sidebar(config).avatar_container_radius),
        SidebarNameSize => set(&mut cfg_sidebar(config).name_size),
        SidebarSectionLabelSize => set(&mut cfg_sidebar(config).section_label_size),
        HomePeersBodyMin => set(&mut cfg_home(config).peers_body_min),
        HomeActivityRowHeight => set(&mut cfg_home(config).activity_row_height),
        HomeHeroGap => set(&mut cfg_home(config).hero_gap),
        HomeQuickActionGap => set(&mut cfg_home(config).quick_action_gap),
        HomeQuickActionIconSize => set(&mut cfg_home(config).quick_action_icon_size),
        HomeQuickActionTitleSize => set(&mut cfg_home(config).quick_action_title_size),
        HomeQuickActionDescSize => set(&mut cfg_home(config).quick_action_desc_size),
        HomeQuickActionDescLineHeight => set(&mut cfg_home(config).quick_action_desc_line_height),
        ChatBubbleMaxWidth => set(&mut cfg_chat(config).bubble_max_width),
        ChatBubbleWidthRatio => set(&mut cfg_chat(config).bubble_width_ratio),
        ChatMessageMaxWidth => set(&mut cfg_chat(config).message_max_width),
        ChatImagePreviewMaxWidth => set(&mut cfg_chat(config).image_preview_max_width),
        ChatImagePreviewMaxHeight => set(&mut cfg_chat(config).image_preview_max_height),
        ChatGifThumbnailWidth => set(&mut cfg_chat(config).gif_thumbnail_width),
        ChatGifThumbnailHeight => set(&mut cfg_chat(config).gif_thumbnail_height),
        ChatEmojiPickerWidth => set(&mut cfg_chat(config).emoji_picker_width),
        AttachProgressBarGirth => set(&mut cfg_attachments(config).progress_bar_girth),
        AttachChipAvatarSize => set(&mut cfg_attachments(config).chip_avatar_size),
        AttachSearchWidthFull => set(&mut cfg_attachments(config).search_width_full),
        AttachEmptyStateHeight => set(&mut cfg_attachments(config).empty_state_height),
        RoomCatalogueRowHeight => set(&mut cfg_rooms(config).catalogue_row_height),
        RoomBannerWidth => set(&mut cfg_rooms(config).banner_width),
        RoomProgressGirth => set(&mut cfg_rooms(config).progress_girth),
        TunnelChipPaddingX => set(&mut cfg_tunnels(config).chip_padding_x),
        TunnelChipPaddingY => set(&mut cfg_tunnels(config).chip_padding_y),
        DialogPadding => set(&mut cfg_dialogs(config).padding),
        DialogSpacing => set(&mut cfg_dialogs(config).spacing),
        DialogTitleSize => set(&mut cfg_dialogs(config).title_size),
        DialogBodySize => set(&mut cfg_dialogs(config).body_size),
        DialogControlPaddingX => set(&mut cfg_dialogs(config).control_padding_x),
        CallAvatarSize => set(&mut cfg_calls(config).avatar_size),
        CallPipW => set(&mut cfg_calls(config).pip_w),
        CallPipH => set(&mut cfg_calls(config).pip_h),
        CallControlsGap => set(&mut cfg_calls(config).controls_gap),
        ControlHeaderHeight => set(&mut cfg_controls(config).header_height),
        ControlSliderWidth => set(&mut cfg_controls(config).slider_width),
        MotionSidebarFadeFrames => {
            let frames = value.round().clamp(0.0, 240.0) as u32;
            cfg_motion(config).sidebar_fade_frames = Some(frames);
        }
        // Non-float fields rejected above.
        HomeShowActivityFeed | ColorCanvas | ColorSidebar | ColorSurface | ColorSurfaceElevated
        | ColorSurfaceSelected
        | ColorSurfaceHover | ColorSurfacePressed | ColorSurfaceSecondary | ColorInputBg
        | ColorBorderMuted | ColorBorderStrong | ColorTextPrimary | ColorTextSecondary
        | ColorTextMuted | ColorPrimary | ColorPrimaryHover | ColorPrimaryPressed | ColorPrimarySoft
        | ColorSuccess | ColorDanger | ColorWarning | ColorFocus | ColorDialogBackdrop
        | TypeDisplayFamily | TypeUiFamily | TypeChatFamily | TypeTechnicalFamily | TypeBrandFamily
        | TypeDisplayHeadingWeight | TypePageTitleWeight | TypeSectionTitleWeight
        | TypeCardTitleWeight | TypeBodyWeight | TypeBodyEmphasisedWeight | TypeButtonLabelWeight
        | TypeSupportingTextWeight | TypeMetadataWeight | TypeChatMessageWeight
        | TypeChatSenderWeight | TypeChatMetadataWeight | TypeComposerTextWeight
        | TypeTechnicalValueWeight | TypeBrandWordmarkWeight => {}
    }
    Ok(())
}

/// Apply a boolean optional-visual-feature edit to the stored config.
pub fn apply_bool(config: &mut UiThemeConfig, field: ThemeField, value: bool) -> Result<(), String> {
    match field {
        ThemeField::HomeShowActivityFeed => {
            cfg_home(config).show_activity_feed = Some(value);
            Ok(())
        }
        _ => Err(format!("{} is not a toggle field", field.label())),
    }
}

/// Apply a colour edit (as a parsed `ColorValue`) to the stored config.
pub fn apply_color(
    config: &mut UiThemeConfig,
    field: ThemeField,
    value: ColorValue,
) -> Result<(), String> {
    use ThemeField::*;
    if !matches!(field.kind(), FieldKind::Color) {
        return Err(format!("{} is not a colour field", field.label()));
    }
    let set = |slot: &mut Option<ColorValue>| *slot = Some(value);
    match field {
        ColorCanvas => set(&mut cfg_colors(config).canvas),
        ColorSidebar => set(&mut cfg_colors(config).sidebar),
        ColorSurface => set(&mut cfg_colors(config).surface),
        ColorSurfaceElevated => set(&mut cfg_colors(config).surface_elevated),
        ColorSurfaceSelected => set(&mut cfg_colors(config).surface_selected),
        ColorSurfaceHover => set(&mut cfg_colors(config).surface_hover),
        ColorSurfacePressed => set(&mut cfg_colors(config).surface_pressed),
        ColorSurfaceSecondary => set(&mut cfg_colors(config).surface_secondary),
        ColorInputBg => set(&mut cfg_colors(config).input_bg),
        ColorBorderMuted => set(&mut cfg_colors(config).border_muted),
        ColorBorderStrong => set(&mut cfg_colors(config).border_strong),
        ColorTextPrimary => set(&mut cfg_colors(config).text_primary),
        ColorTextSecondary => set(&mut cfg_colors(config).text_secondary),
        ColorTextMuted => set(&mut cfg_colors(config).text_muted),
        ColorPrimary => set(&mut cfg_colors(config).primary),
        ColorPrimaryHover => set(&mut cfg_colors(config).primary_hover),
        ColorPrimaryPressed => set(&mut cfg_colors(config).primary_pressed),
        ColorPrimarySoft => set(&mut cfg_colors(config).primary_soft),
        ColorSuccess => set(&mut cfg_colors(config).success),
        ColorDanger => set(&mut cfg_colors(config).danger),
        ColorWarning => set(&mut cfg_colors(config).warning),
        ColorFocus => set(&mut cfg_colors(config).focus),
        ColorDialogBackdrop => set(&mut cfg_colors(config).dialog_backdrop),
        _ => {}
    }
    Ok(())
}

// ── Choice fields (BORU-UI-16: font family + weight mappings) ─────────

impl ThemeField {
    /// The selectable options for a Choice field (family names / weight
    /// labels). The value's serialised name is the option's string.
    pub fn choices(self) -> &'static [&'static str] {
        use ThemeField::*;
        match self {
            TypeDisplayFamily
            | TypeUiFamily
            | TypeChatFamily
            | TypeTechnicalFamily
            | TypeBrandFamily => &crate::fonts::FAMILY_NAMES,
            TypeDisplayHeadingWeight
            | TypePageTitleWeight
            | TypeSectionTitleWeight
            | TypeCardTitleWeight
            | TypeBodyWeight
            | TypeBodyEmphasisedWeight
            | TypeButtonLabelWeight
            | TypeSupportingTextWeight
            | TypeMetadataWeight
            | TypeChatMessageWeight
            | TypeChatSenderWeight
            | TypeChatMetadataWeight
            | TypeComposerTextWeight
            | TypeTechnicalValueWeight
            | TypeBrandWordmarkWeight => &crate::fonts::WEIGHT_LABELS,
            _ => &[],
        }
    }
}

/// Read the current value of a Choice field as the selected option string.
pub fn read_choice(theme: &BoruTheme, field: ThemeField) -> &'static str {
    use ThemeField::*;
    match field {
        TypeDisplayFamily => theme.typography.display_family.name(),
        TypeUiFamily => theme.typography.ui_family.name(),
        TypeChatFamily => theme.typography.chat_family.name(),
        TypeTechnicalFamily => theme.typography.technical_family.name(),
        TypeBrandFamily => theme.typography.brand_family.name(),
        TypeDisplayHeadingWeight => theme.typography.display_heading_weight.label(),
        TypePageTitleWeight => theme.typography.page_title_weight.label(),
        TypeSectionTitleWeight => theme.typography.section_title_weight.label(),
        TypeCardTitleWeight => theme.typography.card_title_weight.label(),
        TypeBodyWeight => theme.typography.body_weight.label(),
        TypeBodyEmphasisedWeight => theme.typography.body_emphasised_weight.label(),
        TypeButtonLabelWeight => theme.typography.button_label_weight.label(),
        TypeSupportingTextWeight => theme.typography.supporting_text_weight.label(),
        TypeMetadataWeight => theme.typography.metadata_weight.label(),
        TypeChatMessageWeight => theme.typography.chat_message_weight.label(),
        TypeChatSenderWeight => theme.typography.chat_sender_weight.label(),
        TypeChatMetadataWeight => theme.typography.chat_metadata_weight.label(),
        TypeComposerTextWeight => theme.typography.composer_text_weight.label(),
        TypeTechnicalValueWeight => theme.typography.technical_value_weight.label(),
        TypeBrandWordmarkWeight => theme.typography.brand_wordmark_weight.label(),
        _ => "",
    }
}

/// Apply a Choice edit to the stored config. `value` is the selected option
/// string (family name / weight label) — validated against the known keys
/// so the merge never sees an unknown family/weight.
pub fn apply_choice(
    config: &mut UiThemeConfig,
    field: ThemeField,
    value: &str,
) -> Result<(), String> {
    use ThemeField::*;
    if !matches!(field.kind(), FieldKind::Choice) {
        return Err(format!("{} is not a choice field", field.label()));
    }
    let set = |slot: &mut Option<String>| *slot = Some(value.to_string());
    match field {
        TypeDisplayFamily => {
            crate::fonts::FontFamilyKey::from_name(value).ok_or_else(|| {
                format!("{}: unknown font family {value:?}", field.label())
            })?;
            set(&mut cfg_typography(config).display_family);
        }
        TypeUiFamily => {
            crate::fonts::FontFamilyKey::from_name(value).ok_or_else(|| {
                format!("{}: unknown font family {value:?}", field.label())
            })?;
            set(&mut cfg_typography(config).ui_family);
        }
        TypeChatFamily => {
            crate::fonts::FontFamilyKey::from_name(value).ok_or_else(|| {
                format!("{}: unknown font family {value:?}", field.label())
            })?;
            set(&mut cfg_typography(config).chat_family);
        }
        TypeTechnicalFamily => {
            crate::fonts::FontFamilyKey::from_name(value).ok_or_else(|| {
                format!("{}: unknown font family {value:?}", field.label())
            })?;
            set(&mut cfg_typography(config).technical_family);
        }
        TypeBrandFamily => {
            crate::fonts::FontFamilyKey::from_name(value).ok_or_else(|| {
                format!("{}: unknown font family {value:?}", field.label())
            })?;
            set(&mut cfg_typography(config).brand_family);
        }
        TypeDisplayHeadingWeight => set_weight(&mut cfg_typography(config).display_heading_weight, value, field)?,
        TypePageTitleWeight => set_weight(&mut cfg_typography(config).page_title_weight, value, field)?,
        TypeSectionTitleWeight => set_weight(&mut cfg_typography(config).section_title_weight, value, field)?,
        TypeCardTitleWeight => set_weight(&mut cfg_typography(config).card_title_weight, value, field)?,
        TypeBodyWeight => set_weight(&mut cfg_typography(config).body_weight, value, field)?,
        TypeBodyEmphasisedWeight => set_weight(&mut cfg_typography(config).body_emphasised_weight, value, field)?,
        TypeButtonLabelWeight => set_weight(&mut cfg_typography(config).button_label_weight, value, field)?,
        TypeSupportingTextWeight => set_weight(&mut cfg_typography(config).supporting_text_weight, value, field)?,
        TypeMetadataWeight => set_weight(&mut cfg_typography(config).metadata_weight, value, field)?,
        TypeChatMessageWeight => set_weight(&mut cfg_typography(config).chat_message_weight, value, field)?,
        TypeChatSenderWeight => set_weight(&mut cfg_typography(config).chat_sender_weight, value, field)?,
        TypeChatMetadataWeight => set_weight(&mut cfg_typography(config).chat_metadata_weight, value, field)?,
        TypeComposerTextWeight => set_weight(&mut cfg_typography(config).composer_text_weight, value, field)?,
        TypeTechnicalValueWeight => set_weight(&mut cfg_typography(config).technical_value_weight, value, field)?,
        TypeBrandWordmarkWeight => set_weight(&mut cfg_typography(config).brand_wordmark_weight, value, field)?,
        _ => return Err(format!("{} is not a choice field", field.label())),
    }
    Ok(())
}

/// Validate a weight label and store it in the config slot.
fn set_weight(
    slot: &mut Option<String>,
    value: &str,
    field: ThemeField,
) -> Result<(), String> {
    crate::fonts::FontWeightKey::from_name(value)
        .ok_or_else(|| format!("{}: unknown weight {value:?}", field.label()))?;
    *slot = Some(value.to_string());
    Ok(())
}

// ── Config group get-or-create helpers ────────────────────────────────

fn cfg_colors(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::ColorConfig {
    cfg.colors.get_or_insert_with(Default::default)
}
fn cfg_typography(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::TypographyConfig {
    cfg.typography.get_or_insert_with(Default::default)
}
fn cfg_spacing(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::SpacingConfig {
    cfg.spacing.get_or_insert_with(Default::default)
}
fn cfg_radii(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::RadiusConfig {
    cfg.radii.get_or_insert_with(Default::default)
}
fn cfg_sidebar(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::SidebarConfig {
    cfg.sidebar.get_or_insert_with(Default::default)
}
fn cfg_home(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::HomeConfig {
    cfg.home.get_or_insert_with(Default::default)
}
fn cfg_chat(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::ChatConfig {
    cfg.chat.get_or_insert_with(Default::default)
}
fn cfg_attachments(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::AttachmentConfig {
    cfg.attachments.get_or_insert_with(Default::default)
}
fn cfg_rooms(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::RoomConfig {
    cfg.rooms.get_or_insert_with(Default::default)
}
fn cfg_tunnels(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::TunnelConfig {
    cfg.tunnels.get_or_insert_with(Default::default)
}
fn cfg_dialogs(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::DialogConfig {
    cfg.dialogs.get_or_insert_with(Default::default)
}
fn cfg_calls(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::CallConfig {
    cfg.calls.get_or_insert_with(Default::default)
}
fn cfg_controls(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::ControlConfig {
    cfg.controls.get_or_insert_with(Default::default)
}
fn cfg_motion(cfg: &mut UiThemeConfig) -> &mut crate::theme_config::MotionConfig {
    cfg.motion.get_or_insert_with(Default::default)
}

// ── Colour helpers ────────────────────────────────────────────────────

/// Parse `#RRGGBB` / `#RRGGBBAA` (leading `#` optional) into a `ColorValue`.
pub fn parse_hex_rgba(s: &str) -> Option<ColorValue> {
    let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if !(hex.len() == 6 || hex.len() == 8) {
        return None;
    }
    let mut channels = [0u8; 4];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(pair).ok()?;
        channels[i] = u8::from_str_radix(pair, 16).ok()?;
    }
    let (r, g, b) = (channels[0], channels[1], channels[2]);
    let a = if hex.len() == 8 { channels[3] } else { 255 };
    Some(ColorValue {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    })
}

/// Format a colour as `#RRGGBB` or `#RRGGBBAA` (alpha included only when < 1).
pub fn color_to_hex(c: Color) -> String {
    let r = (c.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (c.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (c.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = (c.a.clamp(0.0, 1.0) * 255.0).round() as u8;
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

// ── Messages ──────────────────────────────────────────────────────────

/// Inspector panel messages — all normal Iced messages handled in `update()`.
#[derive(Debug, Clone)]
pub enum InspectorMsg {
    /// Toggle panel visibility (Ctrl+Shift+D).
    ToggleVisible,
    /// Toggle a component section expanded/collapsed (BORU-UI-10).
    ToggleSection(SectionId),
    /// Reset one component section back to Boru defaults (BORU-UI-10).
    ResetSection(SectionId),
    /// Reset the complete active theme back to Boru defaults (BORU-UI-10).
    ResetAll,
    /// Slider changed a continuous float value.
    SetFloat { field: ThemeField, value: f32 },
    /// BORU-UI-16: pick_list changed a discrete choice (font family /
    /// weight mapping). `value` is the selected option string.
    SetChoice { field: ThemeField, value: String },
    /// Toggle changed an optional visual feature.
    SetBool { field: ThemeField, value: bool },
    /// Numeric text input changed; apply when it parses.
    FloatTextChanged { field: ThemeField, text: String },
    /// Hex/RGBA text input changed; apply when it parses.
    ColorTextChanged { field: ThemeField, text: String },
    /// BORU-UI-11: toggle inspection mode ('Inspect UI' switch in the panel).
    /// When enabled, hovering/clicking supported components reports their
    /// component ID and selecting one jumps the inspector to its section.
    SetInspectUi(bool),
    /// BORU-UI-11: the cursor entered/exited a supported component region.
    /// `Some(id)` on enter, `None` on exit (mouse_area on_enter/on_exit).
    InspectHover(Option<ComponentId>),
    /// BORU-UI-11: a supported component was clicked while inspection mode is
    /// enabled. Jumps the inspector to the component's section.
    InspectSelect(ComponentId),
    /// BORU-UI-12: save the current editable theme overrides to
    /// `boru-ui.toml`. The write is atomic (temp + rename) so the watcher
    /// never sees a partial file; success/failure is shown in the panel's
    /// save-status line.
    SaveTheme,
    /// BORU-UI-13: discard unsaved inspector changes and reload
    /// `boru-ui.toml` from disk. If the file is missing/invalid the
    /// current theme is kept and the error is reported per BORU-UI-18
    /// (path + parser detail in the panel status line).
    ReloadFromDisk,
    // ── Layout (BORU-LAYOUT-08 / PDF Task 8) ───────────────────
    /// Toggle a layout section expanded/collapsed.
    ToggleLayoutSection(crate::layout_inspector::LayoutSectionId),
    /// Reset one layout section back to the layout defaults (BORU-LAYOUT-08).
    ResetLayoutSection(crate::layout_inspector::LayoutSectionId),
    /// Reset the complete active layout back to defaults.
    ResetLayoutAll,
    /// Slider changed a continuous float layout value.
    SetLayoutFloat {
        field: crate::layout_inspector::LayoutField,
        value: f32,
    },
    /// Slider changed a whole-number layout value (columns, portions).
    SetLayoutInt {
        field: crate::layout_inspector::LayoutField,
        value: i64,
    },
    /// Pick list changed a discrete layout choice. `value` is the option
    /// string (the TOML spelling of the enum variant).
    SetLayoutChoice {
        field: crate::layout_inspector::LayoutField,
        value: String,
    },
    /// Numeric text input for a layout float changed; apply when it parses.
    LayoutFloatTextChanged {
        field: crate::layout_inspector::LayoutField,
        text: String,
    },
    /// Numeric text input for a layout int changed; apply when it parses.
    LayoutIntTextChanged {
        field: crate::layout_inspector::LayoutField,
        text: String,
    },
    /// Section-list text input changed; apply when every name parses.
    LayoutSectionsTextChanged {
        field: crate::layout_inspector::LayoutField,
        text: String,
    },
    /// Save the current editable layout overrides to `boru-layout.toml`
    /// (atomic write, same format the dev watcher reloads).
    SaveLayout,
    /// Discard unsaved layout changes and reload `boru-layout.toml` from
    /// disk. A missing/invalid file keeps the current layout and reports
    /// the error in the panel status line.
    ReloadLayoutFromDisk,
}

/// Result of the last Save Theme action (BORU-UI-12), shown as the panel's
/// save-status line. View-local display state only — never part of the theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSaveStatus {
    /// No save has been attempted yet this session.
    None,
    /// The last Save Theme action wrote `boru-ui.toml` successfully.
    Saved,
    /// The last Save Theme action failed; the message is shown in the panel.
    Failed(String),
}

impl Default for ThemeSaveStatus {
    fn default() -> Self {
        Self::None
    }
}

/// Result of the last "Reload From Disk" action (BORU-UI-13), shown as the
/// panel's reload-status line. View-local display state only — never part
/// of the theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeReloadStatus {
    /// No reload has been attempted yet this session.
    None,
    /// The last "Reload From Disk" action reloaded `boru-ui.toml`.
    Reloaded,
    /// The last "Reload From Disk" action failed; the message (path +
    /// parser detail, per BORU-UI-18) is shown in the panel.
    Failed(String),
}

impl Default for ThemeReloadStatus {
    fn default() -> Self {
        Self::None
    }
}

/// Draft text for the inspector's text inputs. Kept so a half-typed value
/// (e.g. `"2."`) is not clobbered by the rendered current value each frame;
/// the value is only applied to the theme once it parses.
#[derive(Debug, Clone, Default)]
pub struct InspectorDraft {
    /// Per-field numeric text (float fields).
    pub float_text: HashMap<ThemeField, String>,
    /// Per-field hex/RGBA text (colour fields).
    pub color_text: HashMap<ThemeField, String>,
    /// Component sections the user collapsed (BORU-UI-10). View-local
    /// state only — never part of the theme.
    pub collapsed_sections: HashSet<SectionId>,
    /// BORU-UI-12: result of the last Save Theme action (view-local status
    /// line only — never part of the theme).
    pub save_status: ThemeSaveStatus,
    /// BORU-UI-13: result of the last "Reload From Disk" action
    /// (view-local status line only — never part of the theme).
    pub reload_status: ThemeReloadStatus,
    /// BORU-UI-18: field-level merge adjustments from the last theme
    /// recompute (values that had to be clamped or replaced by defaults,
    /// e.g. an out-of-range colour channel or an unknown font name). Set
    /// by the app after every merge; rendered as a compact warnings list
    /// in the panel. View-local display state only — never part of the
    /// theme.
    pub merge_warnings: Vec<String>,
    // ── BORU-LAYOUT-08 layout panel state (view-local only) ────
    /// Per-field numeric text for layout float fields.
    pub layout_float_text: HashMap<crate::layout_inspector::LayoutField, String>,
    /// Per-field numeric text for layout int fields.
    pub layout_int_text: HashMap<crate::layout_inspector::LayoutField, String>,
    /// Per-field text for layout section lists.
    pub layout_sections_text: HashMap<crate::layout_inspector::LayoutField, String>,
    /// Layout sections the user collapsed. View-local state only — never
    /// part of the layout.
    pub collapsed_layout_sections: HashSet<crate::layout_inspector::LayoutSectionId>,
    /// Result of the last Save Layout action (view-local status line only
    /// — never part of the layout).
    pub layout_save_status: crate::layout_inspector::LayoutSaveStatus,
    /// Result of the last Reload Layout From Disk action (view-local
    /// status line only — never part of the layout).
    pub layout_reload_status: crate::layout_inspector::LayoutReloadStatus,
    /// BORU-LAYOUT-08: layout field-level merge adjustments from the last
    /// layout recompute (values clamped or replaced by defaults). Set by
    /// the app after every merge; rendered as a compact warnings list.
    pub layout_merge_warnings: Vec<String>,
}

// ── View ──────────────────────────────────────────────────────────────

/// One sub-group of fields inside a component section (e.g. "Width" under
/// Sidebar). The label is rendered as a smaller header above its rows.
pub struct FieldGroup {
    pub label: &'static str,
    pub fields: &'static [ThemeField],
}

/// One top-level component section of the inspector (BORU-UI-10).
pub struct InspectorSection {
    pub id: SectionId,
    pub label: &'static str,
    /// Ordered sub-groups. A section with a single sub-group renders its
    /// fields directly under the section header (no duplicate header).
    pub groups: &'static [FieldGroup],
}

/// Ordered component sections rendered top-to-bottom (BORU-UI-10).
///
/// The hierarchy mirrors the typed theme's component groups: the PDF's
/// Global / Sidebar / Home / Chat / Attachments / Rooms / Tunnels / Dialogs,
/// plus the remaining typed groups the inspector exposes (Calls, Controls,
/// Motion). Sub-group labels use the actual field names from `theme.rs` —
/// only values that exist in the typed theme are exposed, nothing derived.
pub const SECTIONS: &[InspectorSection] = &[
    InspectorSection {
        id: SectionId::Global,
        label: "Global",
        groups: &[
            FieldGroup {
                label: "Colours",
                fields: &[
                    ThemeField::ColorCanvas,
                    ThemeField::ColorSidebar,
                    ThemeField::ColorSurface,
                    ThemeField::ColorSurfaceElevated,
                    ThemeField::ColorSurfaceSelected,
                    ThemeField::ColorSurfaceHover,
                    ThemeField::ColorSurfacePressed,
                    ThemeField::ColorSurfaceSecondary,
                    ThemeField::ColorInputBg,
                    ThemeField::ColorBorderMuted,
                    ThemeField::ColorBorderStrong,
                    ThemeField::ColorTextPrimary,
                    ThemeField::ColorTextSecondary,
                    ThemeField::ColorTextMuted,
                    ThemeField::ColorPrimary,
                    ThemeField::ColorPrimaryHover,
                    ThemeField::ColorPrimaryPressed,
                    ThemeField::ColorPrimarySoft,
                    ThemeField::ColorSuccess,
                    ThemeField::ColorDanger,
                    ThemeField::ColorWarning,
                    ThemeField::ColorFocus,
                    ThemeField::ColorDialogBackdrop,
                ],
            },
            FieldGroup {
                label: "Typography",
                fields: &[
                    ThemeField::TypeDisplayHeading,
                    ThemeField::TypePageTitle,
                    ThemeField::TypeSectionTitle,
                    ThemeField::TypeCardTitle,
                    ThemeField::TypeBody,
                    ThemeField::TypeBodyEmphasised,
                    ThemeField::TypeButtonLabel,
                    ThemeField::TypeSupportingText,
                    ThemeField::TypeMetadata,
                    ThemeField::TypeChatMessage,
                    ThemeField::TypeChatSender,
                    ThemeField::TypeChatMetadata,
                    ThemeField::TypeComposerText,
                    ThemeField::TypeHomeSubtitle,
                    ThemeField::TypeDialogTitle,
                    ThemeField::TypeDialogSubtitle,
                ],
            },
            FieldGroup {
                label: "Typography — font families",
                fields: &[
                    ThemeField::TypeDisplayFamily,
                    ThemeField::TypeUiFamily,
                    ThemeField::TypeChatFamily,
                    ThemeField::TypeTechnicalFamily,
                    ThemeField::TypeBrandFamily,
                ],
            },
            FieldGroup {
                label: "Typography — weights",
                fields: &[
                    ThemeField::TypeDisplayHeadingWeight,
                    ThemeField::TypePageTitleWeight,
                    ThemeField::TypeSectionTitleWeight,
                    ThemeField::TypeCardTitleWeight,
                    ThemeField::TypeBodyWeight,
                    ThemeField::TypeBodyEmphasisedWeight,
                    ThemeField::TypeButtonLabelWeight,
                    ThemeField::TypeSupportingTextWeight,
                    ThemeField::TypeMetadataWeight,
                    ThemeField::TypeChatMessageWeight,
                    ThemeField::TypeChatSenderWeight,
                    ThemeField::TypeChatMetadataWeight,
                    ThemeField::TypeComposerTextWeight,
                    ThemeField::TypeTechnicalValueWeight,
                    ThemeField::TypeBrandWordmarkWeight,
                ],
            },
            FieldGroup {
                label: "Typography — line heights",
                fields: &[
                    ThemeField::TypeDisplayHeadingLineHeight,
                    ThemeField::TypePageTitleLineHeight,
                    ThemeField::TypeSectionTitleLineHeight,
                    ThemeField::TypeCardTitleLineHeight,
                    ThemeField::TypeBodyLineHeight,
                    ThemeField::TypeBodyEmphasisedLineHeight,
                    ThemeField::TypeButtonLabelLineHeight,
                    ThemeField::TypeSupportingTextLineHeight,
                    ThemeField::TypeMetadataLineHeight,
                    ThemeField::TypeChatMessageLineHeight,
                    ThemeField::TypeChatSenderLineHeight,
                    ThemeField::TypeChatMetadataLineHeight,
                    ThemeField::TypeComposerTextLineHeight,
                    ThemeField::TypeTechnicalValueLineHeight,
                    ThemeField::TypeBrandWordmarkLineHeight,
                ],
            },
            FieldGroup {
                label: "Spacing",
                fields: &[
                    ThemeField::Space4,
                    ThemeField::Space8,
                    ThemeField::Space12,
                    ThemeField::Space16,
                    ThemeField::Space20,
                    ThemeField::Space24,
                    ThemeField::Space32,
                    ThemeField::Space40,
                    ThemeField::ControlHeight,
                ],
            },
            FieldGroup {
                label: "Radii",
                fields: &[
                    ThemeField::RadiusSm,
                    ThemeField::RadiusMd,
                    ThemeField::RadiusLg,
                    ThemeField::RadiusXl,
                    ThemeField::RadiusCard,
                    ThemeField::RadiusPill,
                    ThemeField::RadiusDialog,
                ],
            },
        ],
    },
    InspectorSection {
        id: SectionId::Sidebar,
        label: "Sidebar",
        groups: &[
            FieldGroup {
                label: "Width",
                fields: &[
                    ThemeField::SidebarWidth,
                    ThemeField::SidebarWidthMin,
                    ThemeField::SidebarWidthMax,
                ],
            },
            FieldGroup {
                label: "Item",
                fields: &[
                    ThemeField::SidebarInset,
                    ThemeField::SidebarItemRadius,
                    ThemeField::SidebarAvatarContainerRadius,
                ],
            },
            FieldGroup {
                label: "Typography",
                fields: &[
                    ThemeField::SidebarNameSize,
                    ThemeField::SidebarSectionLabelSize,
                ],
            },
        ],
    },
    InspectorSection {
        id: SectionId::Home,
        label: "Home",
        groups: &[
            FieldGroup {
                label: "Quick Action Card",
                fields: &[
                    ThemeField::HomeQuickActionGap,
                    ThemeField::HomeQuickActionIconSize,
                    ThemeField::HomeQuickActionTitleSize,
                    ThemeField::HomeQuickActionDescSize,
                    ThemeField::HomeQuickActionDescLineHeight,
                ],
            },
            FieldGroup {
                label: "Status & Activity",
                fields: &[
                    ThemeField::HomePeersBodyMin,
                    ThemeField::HomeActivityRowHeight,
                    ThemeField::HomeHeroGap,
                    ThemeField::HomeShowActivityFeed,
                ],
            },
        ],
    },
    InspectorSection {
        id: SectionId::Chat,
        label: "Chat",
        groups: &[
            FieldGroup {
                label: "Message Bubble",
                fields: &[
                    ThemeField::ChatBubbleMaxWidth,
                    ThemeField::ChatBubbleWidthRatio,
                    ThemeField::ChatMessageMaxWidth,
                ],
            },
            FieldGroup {
                label: "Media & Pickers",
                fields: &[
                    ThemeField::ChatImagePreviewMaxWidth,
                    ThemeField::ChatImagePreviewMaxHeight,
                    ThemeField::ChatGifThumbnailWidth,
                    ThemeField::ChatGifThumbnailHeight,
                    ThemeField::ChatEmojiPickerWidth,
                ],
            },
        ],
    },
    InspectorSection {
        id: SectionId::Attachments,
        label: "Attachments",
        groups: &[
            FieldGroup {
                label: "File Card",
                fields: &[
                    ThemeField::AttachSearchWidthFull,
                    ThemeField::AttachEmptyStateHeight,
                ],
            },
            FieldGroup {
                label: "Progress & Chips",
                fields: &[
                    ThemeField::AttachProgressBarGirth,
                    ThemeField::AttachChipAvatarSize,
                ],
            },
        ],
    },
    InspectorSection {
        id: SectionId::Rooms,
        label: "Rooms",
        groups: &[FieldGroup {
            label: "Rooms",
            fields: &[
                ThemeField::RoomCatalogueRowHeight,
                ThemeField::RoomBannerWidth,
                ThemeField::RoomProgressGirth,
            ],
        }],
    },
    InspectorSection {
        id: SectionId::Tunnels,
        label: "Tunnels",
        groups: &[FieldGroup {
            label: "Tunnels",
            fields: &[
                ThemeField::TunnelChipPaddingX,
                ThemeField::TunnelChipPaddingY,
            ],
        }],
    },
    InspectorSection {
        id: SectionId::Dialogs,
        label: "Dialogs",
        groups: &[FieldGroup {
            label: "Dialogs",
            fields: &[
                ThemeField::DialogPadding,
                ThemeField::DialogSpacing,
                ThemeField::DialogTitleSize,
                ThemeField::DialogBodySize,
                ThemeField::DialogControlPaddingX,
            ],
        }],
    },
    InspectorSection {
        id: SectionId::Calls,
        label: "Calls",
        groups: &[FieldGroup {
            label: "Calls",
            fields: &[
                ThemeField::CallAvatarSize,
                ThemeField::CallPipW,
                ThemeField::CallPipH,
                ThemeField::CallControlsGap,
            ],
        }],
    },
    InspectorSection {
        id: SectionId::Controls,
        label: "Controls",
        groups: &[FieldGroup {
            label: "Controls",
            fields: &[
                ThemeField::ControlHeaderHeight,
                ThemeField::ControlSliderWidth,
            ],
        }],
    },
    InspectorSection {
        id: SectionId::Motion,
        label: "Motion",
        groups: &[FieldGroup {
            label: "Motion",
            fields: &[ThemeField::MotionSidebarFadeFrames],
        }],
    },
];

/// Build the inspector panel element.
///
/// `theme` is the CURRENT active theme (display values), `layout` the
/// CURRENT active structural layout (BORU-LAYOUT-08), `draft` holds
/// in-progress text input state, `dark_mode` selects panel styling. The
/// returned element emits [`InspectorMsg`] wrapped in
/// [`AppMessage::Inspector`].
///
/// BORU-UI-11 adds the 'Inspect UI' toggle: `inspect_enabled` is the current
/// inspection-mode state, `inspect_hover` the component under the cursor
/// (None when the cursor left every supported region), and `inspect_selected`
/// the last component the developer clicked. The panel renders the toggle and
/// a status line so the developer always sees the active component ID/name.
pub fn view_inspector(
    theme: &BoruTheme,
    layout: &crate::layout::LayoutConfig,
    draft: &InspectorDraft,
    dark_mode: bool,
    designer_enabled: bool,
    inspect_enabled: bool,
    inspect_hover: Option<ComponentId>,
    inspect_selected: Option<ComponentId>,
) -> Element<'static, AppMessage> {
    let mut col = iced::widget::Column::new()
        .push(panel_heading(dark_mode))
        .push(designer_mode_row(designer_enabled, dark_mode))
        .push(inspect_ui_row(inspect_enabled, inspect_hover, inspect_selected, dark_mode))
        .push(reset_actions_row(dark_mode))
        .push(save_theme_row(dark_mode, &draft.save_status))
        .push(reload_theme_row(dark_mode, &draft.reload_status))
        .push(merge_warnings_row(dark_mode, &draft.merge_warnings))
        .push(gallery_row(dark_mode))
        .push(Space::new().height(Length::Fixed(6.0)))
        .spacing(2.0);

    for section in SECTIONS {
        let collapsed = draft.collapsed_sections.contains(&section.id);
        let highlighted = inspect_selected.map(|c| c.section()) == Some(section.id);
        col = col
            .push(section_header(section, collapsed, highlighted, dark_mode))
            .push(Space::new().height(Length::Fixed(2.0)));
        if collapsed {
            continue;
        }
        let multi = section.groups.len() > 1;
        for group in section.groups {
            if multi {
                col = col
                    .push(subgroup_header(group.label, dark_mode))
                    .push(Space::new().height(Length::Fixed(2.0)));
            }
            for field in group.fields {
                col = col.push(field_row(theme, draft, *field, dark_mode));
            }
            col = col.push(Space::new().height(Length::Fixed(4.0)));
        }
        col = col.push(Space::new().height(Length::Fixed(8.0)));
    }

    // ── Layout (BORU-LAYOUT-08 / PDF Task 8) ───────────────────
    // A second block of the panel: layout controls with their own
    // save/reload/status rows and collapsible sections. The layout rows
    // live in `layout_inspector.rs` (the pure read/apply mapping) and are
    // rendered here so the whole panel stays in one scrollable column.
    col = col
        .push(crate::layout_inspector::layout_panel_heading(dark_mode))
        .push(crate::layout_inspector::save_layout_row(
            dark_mode,
            &draft.layout_save_status,
        ))
        .push(crate::layout_inspector::reload_layout_row(
            dark_mode,
            &draft.layout_reload_status,
        ))
        .push(crate::layout_inspector::layout_merge_warnings_row(
            dark_mode,
            &draft.layout_merge_warnings,
        ))
        .push(Space::new().height(Length::Fixed(6.0)));

    for section in crate::layout_inspector::LAYOUT_SECTIONS {
        let collapsed = draft.collapsed_layout_sections.contains(&section.id);
        col = col
            .push(crate::layout_inspector::layout_section_header(
                section,
                collapsed,
                dark_mode,
            ))
            .push(Space::new().height(Length::Fixed(2.0)));
        if collapsed {
            continue;
        }
        let multi = section.groups.len() > 1;
        for group in section.groups {
            if multi {
                col = col
                    .push(subgroup_header(group.label, dark_mode))
                    .push(Space::new().height(Length::Fixed(2.0)));
            }
            for field in group.fields {
                col = col.push(crate::layout_inspector::layout_field_row(
                    layout,
                    draft,
                    *field,
                    dark_mode,
                ));
            }
            col = col.push(Space::new().height(Length::Fixed(4.0)));
        }
        col = col.push(Space::new().height(Length::Fixed(8.0)));
    }

    let panel = container(
        scrollable(col)
            .id(INSPECTOR_SCROLL_ID)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fixed(INSPECTOR_PANEL_WIDTH))
    .height(Length::Fill)
    .padding(10)
    .style(move |t| panel_style(t, dark_mode));

    panel.into()
}

/// Row with the Visual Designer toggle (BORU-DESIGN-03).
fn designer_mode_row(enabled: bool, dark_mode: bool) -> Element<'static, AppMessage> {
    let toggle = toggler(enabled)
        .label("Visual Designer")
        .on_toggle(|value| {
            AppMessage::Designer(if value {
                crate::designer::DesignerMessage::Enter
            } else {
                crate::designer::DesignerMessage::Exit
            })
        });
    let status = text(if enabled {
        "Active — designer interactions enabled"
    } else {
        "Off — normal application behaviour"
    })
    .size(9.0)
    .color(if enabled {
        Color::from_rgb(0.2, 0.65, 0.4)
    } else if dark_mode {
        Color::from_rgb(0.55, 0.55, 0.6)
    } else {
        Color::from_rgb(0.45, 0.45, 0.45)
    });
    iced::widget::Column::new()
        .push(row![toggle, Space::new().width(Length::Fill)].align_y(Alignment::Center))
        .push(status)
        .spacing(2.0)
        .into()
}

/// Row with the 'Inspect UI' toggle + status line (BORU-UI-11).
///
/// The toggle flips inspection mode: when enabled, hovering a supported
/// component shows its component ID/name and clicking it jumps the inspector
/// to the relevant section. The status line always shows the current hover /
/// selected component so the developer sees the active component ID.
fn inspect_ui_row(
    enabled: bool,
    hover: Option<ComponentId>,
    selected: Option<ComponentId>,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let tg = toggler(enabled)
        .label("Inspect UI")
        .on_toggle(move |v| AppMessage::Inspector(InspectorMsg::SetInspectUi(v)));

    let status = if enabled {
        match (hover, selected) {
            (Some(h), _) => format!("{} — click to jump", h.label()),
            (None, Some(s)) => format!("Selected: {}", s.label()),
            (None, None) => "Hover a component to identify it".to_string(),
        }
    } else {
        "Off — clicks pass through normally".to_string()
    };

    let status_text = text(status)
        .size(9.0)
        .color(if dark_mode {
            Color::from_rgb(0.65, 0.75, 0.7)
        } else {
            Color::from_rgb(0.2, 0.45, 0.3)
        });

    iced::widget::Column::new()
        .push(
            row![tg, Space::new().width(Length::Fill)]
                .align_y(Alignment::Center),
        )
        .push(status_text)
        .spacing(2.0)
        .into()
}

/// Row with the Reset All action (BORU-UI-10).
fn reset_actions_row(dark_mode: bool) -> Element<'static, AppMessage> {
    let reset_all = button(text("Reset All").size(11.0).color(if dark_mode {
        Color::from_rgb(0.85, 0.85, 0.85)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    }))
    .on_press(AppMessage::Inspector(InspectorMsg::ResetAll))
    .padding([3, 8]);
    let hint = text("resets every section to Boru defaults")
        .size(9.0)
        .color(if dark_mode {
            Color::from_rgb(0.55, 0.55, 0.6)
        } else {
            Color::from_rgb(0.45, 0.45, 0.45)
        });
    row![
        reset_all,
        Space::new().width(Length::Fixed(6.0)),
        hint,
        Space::new().width(Length::Fill)
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Row with the Component Gallery launch button (BORU-UI-14 / PDF Task 14).
///
/// The button navigates to the developer-only component gallery screen
/// (Ctrl+Shift+G); the hint keeps the shortcut discoverable inside the panel.
fn gallery_row(dark_mode: bool) -> Element<'static, AppMessage> {
    let open = button(text("Component Gallery").size(11.0).color(if dark_mode {
        Color::from_rgb(0.85, 0.85, 0.85)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    }))
    .on_press(AppMessage::ToggleGallery)
    .padding([3, 8]);
    let hint = text("opens the component playground (Ctrl+Shift+G)")
        .size(9.0)
        .color(if dark_mode {
            Color::from_rgb(0.55, 0.55, 0.6)
        } else {
            Color::from_rgb(0.45, 0.45, 0.45)
        });
    row![
        open,
        Space::new().width(Length::Fixed(6.0)),
        hint,
        Space::new().width(Length::Fill)
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Row with the Save Theme action + status line (BORU-UI-12).
///
/// The button serializes the current editable theme overrides to
/// `boru-ui.toml` (atomic temp + rename) and the status line shows the
/// result of the last save inside the panel.
fn save_theme_row(dark_mode: bool, status: &ThemeSaveStatus) -> Element<'static, AppMessage> {
    let save = button(text("Save Theme").size(11.0).color(if dark_mode {
        Color::from_rgb(0.85, 0.85, 0.85)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    }))
    .on_press(AppMessage::Inspector(InspectorMsg::SaveTheme))
    .padding([3, 8]);

    let (msg, color) = match status {
        ThemeSaveStatus::None => (
            "saves current overrides to boru-ui.toml".to_string(),
            if dark_mode {
                Color::from_rgb(0.55, 0.55, 0.6)
            } else {
                Color::from_rgb(0.45, 0.45, 0.45)
            },
        ),
        ThemeSaveStatus::Saved => (
            "✓ saved".to_string(),
            if dark_mode {
                Color::from_rgb(0.6, 0.85, 0.65)
            } else {
                Color::from_rgb(0.1, 0.55, 0.25)
            },
        ),
        ThemeSaveStatus::Failed(e) => {
            // Keep the panel compact: the full error is in the logs, the
            // status line shows a prefix.
            let preview: String = e.chars().take(120).collect();
            (
                format!("✗ {preview}"),
                if dark_mode {
                    Color::from_rgb(0.95, 0.6, 0.6)
                } else {
                    Color::from_rgb(0.75, 0.2, 0.2)
                },
            )
        }
    };
    let status_text = text(msg).size(9.0).color(color);

    row![
        save,
        Space::new().width(Length::Fixed(6.0)),
        status_text,
        Space::new().width(Length::Fill)
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Row with the Reload From Disk action + status line (BORU-UI-13).
///
/// The button discards unsaved inspector changes and reloads
/// `boru-ui.toml` from disk. The status line shows the result of the last
/// reload inside the panel; a failed reload keeps the current theme and
/// reports the error (path + parser detail, per BORU-UI-18).
fn reload_theme_row(dark_mode: bool, status: &ThemeReloadStatus) -> Element<'static, AppMessage> {
    let reload = button(text("Reload From Disk").size(11.0).color(if dark_mode {
        Color::from_rgb(0.85, 0.85, 0.85)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    }))
    .on_press(AppMessage::Inspector(InspectorMsg::ReloadFromDisk))
    .padding([3, 8]);

    let (msg, color) = match status {
        ThemeReloadStatus::None => (
            "discards unsaved changes; reloads boru-ui.toml".to_string(),
            if dark_mode {
                Color::from_rgb(0.55, 0.55, 0.6)
            } else {
                Color::from_rgb(0.45, 0.45, 0.45)
            },
        ),
        ThemeReloadStatus::Reloaded => (
            "✓ reloaded from disk".to_string(),
            if dark_mode {
                Color::from_rgb(0.6, 0.85, 0.65)
            } else {
                Color::from_rgb(0.1, 0.55, 0.25)
            },
        ),
        ThemeReloadStatus::Failed(e) => {
            // Keep the panel compact: the full error is in the logs, the
            // status line shows a prefix.
            let preview: String = e.chars().take(120).collect();
            (
                format!("✗ {preview}"),
                if dark_mode {
                    Color::from_rgb(0.95, 0.6, 0.6)
                } else {
                    Color::from_rgb(0.75, 0.2, 0.2)
                },
            )
        }
    };
    let status_text = text(msg).size(9.0).color(color);

    row![
        reload,
        Space::new().width(Length::Fixed(6.0)),
        status_text,
        Space::new().width(Length::Fill)
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Compact list of the last merge's field-level adjustments (BORU-UI-18).
///
/// Shown only when the last theme merge had to clamp a value or fall back
/// to a default (e.g. an out-of-range colour channel, an absurd width, an
/// unknown font name). Each entry is the merge's warning string, which
/// already names the field (`colors.primary: …`). View-local display state
/// only — never part of the theme.
fn merge_warnings_row(dark_mode: bool, warnings: &[String]) -> Element<'static, AppMessage> {
    if warnings.is_empty() {
        return Space::new().height(Length::Fixed(0.0)).into();
    }
    let heading = text(format!("⚠ {} value(s) adjusted on load", warnings.len()))
        .size(9.0)
        .color(if dark_mode {
            Color::from_rgb(0.95, 0.75, 0.4)
        } else {
            Color::from_rgb(0.7, 0.45, 0.0)
        });
    let mut col = iced::widget::Column::new().push(heading).spacing(1.0);
    for w in warnings.iter().take(4) {
        let preview: String = w.chars().take(90).collect();
        col = col.push(
            text(format!("· {preview}"))
                .size(8.0)
                .color(if dark_mode {
                    Color::from_rgb(0.8, 0.65, 0.45)
                } else {
                    Color::from_rgb(0.45, 0.3, 0.1)
                }),
        );
    }
    if warnings.len() > 4 {
        col = col.push(
            text(format!("· … {} more", warnings.len() - 4))
                .size(8.0)
                .color(if dark_mode {
                    Color::from_rgb(0.6, 0.5, 0.4)
                } else {
                    Color::from_rgb(0.4, 0.35, 0.3)
                }),
        );
    }
    col.padding([2, 6]).into()
}

/// Collapsible component section header with a per-section Reset action.
/// `highlighted` (BORU-UI-11) marks the section selected via inspection mode.
fn section_header(
    section: &InspectorSection,
    collapsed: bool,
    highlighted: bool,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let chevron = if collapsed { "▸" } else { "▾" };
    let toggle = button(
        row![
            text(chevron).size(10.0).color(if dark_mode {
                Color::from_rgb(0.6, 0.8, 0.7)
            } else {
                Color::from_rgb(0.1, 0.5, 0.3)
            }),
            Space::new().width(Length::Fixed(4.0)),
            text(section.label.to_uppercase())
                .size(11.0)
                .color(if highlighted {
                    if dark_mode {
                        Color::from_rgb(1.0, 0.85, 0.4)
                    } else {
                        Color::from_rgb(0.75, 0.5, 0.0)
                    }
                } else if dark_mode {
                    Color::from_rgb(0.7, 0.85, 0.75)
                } else {
                    Color::from_rgb(0.1, 0.45, 0.28)
                }),
        ]
        .align_y(Alignment::Center),
    )
    .on_press(AppMessage::Inspector(InspectorMsg::ToggleSection(section.id)))
    .padding([3, 6])
    .style(button::text);

    let reset = button(text("Reset").size(9.0).color(if dark_mode {
        Color::from_rgb(0.75, 0.75, 0.78)
    } else {
        Color::from_rgb(0.4, 0.42, 0.4)
    }))
    .on_press(AppMessage::Inspector(InspectorMsg::ResetSection(section.id)))
    .padding([2, 6])
    .style(button::text);

    row![toggle, Space::new().width(Length::Fill), reset]
        .align_y(Alignment::Center)
        .into()
}

fn subgroup_header(label: &str, dark_mode: bool) -> Element<'static, AppMessage> {
    text(label.to_uppercase())
        .size(9.0)
        .color(if dark_mode {
            Color::from_rgb(0.5, 0.6, 0.55)
        } else {
            Color::from_rgb(0.2, 0.4, 0.28)
        })
        .into()
}

fn panel_heading(dark_mode: bool) -> Element<'static, AppMessage> {
    let title = text("UI Inspector (dev)")
        .size(14.0)
        .color(if dark_mode {
            Color::from_rgb(0.9, 0.9, 0.9)
        } else {
            Color::from_rgb(0.1, 0.1, 0.1)
        });
    let hint = text("Ctrl+Shift+D")
        .size(10.0)
        .color(if dark_mode {
            Color::from_rgb(0.6, 0.6, 0.6)
        } else {
            Color::from_rgb(0.4, 0.4, 0.4)
        });
    let close = button(
        text("×")
            .size(14.0)
            .color(if dark_mode {
                Color::from_rgb(0.8, 0.8, 0.8)
            } else {
                Color::from_rgb(0.2, 0.2, 0.2)
            }),
    )
    .on_press(AppMessage::Inspector(InspectorMsg::ToggleVisible))
    .padding([0, 6]);
    row![
        title,
        Space::new().width(Length::Fill),
        hint,
        Space::new().width(Length::Fixed(6.0)),
        close
    ]
    .align_y(Alignment::Center)
    .into()
}

fn panel_style(t: &iced::Theme, dark_mode: bool) -> container::Style {
    let bg = if dark_mode {
        Color::from_rgb(0.12, 0.12, 0.20)
    } else {
        Color::from_rgb(0.98, 0.99, 0.98)
    };
    let border = if dark_mode {
        Color::from_rgb(0.28, 0.28, 0.38)
    } else {
        Color::from_rgb(0.82, 0.88, 0.84)
    };
    let _ = t;
    container::Style {
        background: Some(iced::Background::Color(bg)),
        border: iced::Border {
            color: border,
            width: 1.0,
            radius: iced::border::Radius::default(),
        },
        ..Default::default()
    }
}

fn field_row(
    theme: &BoruTheme,
    draft: &InspectorDraft,
    field: ThemeField,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    match field.kind() {
        FieldKind::Float => float_row(theme, draft, field, dark_mode),
        FieldKind::Bool => bool_row(theme, draft, field, dark_mode),
        FieldKind::Color => color_row(theme, draft, field, dark_mode),
        FieldKind::Choice => choice_row(theme, draft, field, dark_mode),
        // Theme fields never use the layout-only kinds; the layout panel
        // (layout_inspector) renders those rows itself.
        FieldKind::Int | FieldKind::Sections => {
            Space::new().height(Length::Fixed(0.0)).into()
        }
    }
}

fn float_row(
    theme: &BoruTheme,
    draft: &InspectorDraft,
    field: ThemeField,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let current = read_float(theme, field);
    let (min, max) = field.range();
    let text_value = draft
        .float_text
        .get(&field)
        .cloned()
        .unwrap_or_else(|| format!("{current:.1}"));

    let label = text(field.label()).size(11.0).color(muted_text(dark_mode));
    let value = text(format!("{current:.1}")).size(11.0).color(value_text(dark_mode));

    let slider = slider(min..=max, current.clamp(min, max), move |v| {
        AppMessage::Inspector(InspectorMsg::SetFloat { field, value: v })
    })
    .width(Length::Fill);

    let input = text_input("value", &text_value)
        .width(Length::Fixed(64.0))
        .padding([2, 6])
        .size(11.0)
        .on_input(move |s| {
            AppMessage::Inspector(InspectorMsg::FloatTextChanged { field, text: s })
        });

    iced::widget::Column::new()
        .push(
            row![label, Space::new().width(Length::Fill), value]
                .align_y(Alignment::Center),
        )
        .push(row![slider, Space::new().width(Length::Fixed(6.0)), input].align_y(Alignment::Center))
        .spacing(2.0)
        .into()
}

fn bool_row(
    theme: &BoruTheme,
    draft: &InspectorDraft,
    field: ThemeField,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let _ = draft;
    let current = read_bool(theme, field);
    let label = field.label();
    let tg = toggler(current).label(label).on_toggle(move |v| {
        AppMessage::Inspector(InspectorMsg::SetBool { field, value: v })
    });
    let _ = dark_mode;
    container(tg).width(Length::Fill).into()
}

fn color_row(
    theme: &BoruTheme,
    draft: &InspectorDraft,
    field: ThemeField,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let current = read_color(theme, field);
    let text_value = draft
        .color_text
        .get(&field)
        .cloned()
        .unwrap_or_else(|| color_to_hex(current));

    let label = text(field.label()).size(11.0).color(muted_text(dark_mode));
    let swatch = container(Space::new().width(Length::Fixed(18.0)).height(Length::Fixed(14.0)))
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(current)),
            border: iced::Border {
                color: Color::from_rgb(0.5, 0.5, 0.5),
                width: 1.0,
                radius: iced::border::Radius::from(3.0),
            },
            ..Default::default()
        });

    let input = text_input("#RRGGBB[AA]", &text_value)
        .width(Length::Fill)
        .padding([2, 6])
        .size(11.0)
        .on_input(move |s| {
            AppMessage::Inspector(InspectorMsg::ColorTextChanged { field, text: s })
        });

    iced::widget::Column::new()
        .push(
            row![label, Space::new().width(Length::Fill), swatch]
                .align_y(Alignment::Center),
        )
        .push(input)
        .spacing(2.0)
        .into()
}

/// Row for a discrete Choice field (font family / weight mapping,
/// BORU-UI-16). Renders a labelled pick_list with the current selection.
fn choice_row(
    theme: &BoruTheme,
    _draft: &InspectorDraft,
    field: ThemeField,
    dark_mode: bool,
) -> Element<'static, AppMessage> {
    let options = field.choices().to_vec();
    let selected = read_choice(theme, field);

    let label = text(field.label()).size(11.0).color(muted_text(dark_mode));

    let list = pick_list(options, Some(selected), move |choice: &str| {
        AppMessage::Inspector(InspectorMsg::SetChoice {
            field,
            value: choice.to_string(),
        })
    })
    .width(Length::Fill)
    .padding([2, 6])
    .text_size(11.0);

    iced::widget::Column::new()
        .push(
            row![label, Space::new().width(Length::Fill)]
                .align_y(Alignment::Center),
        )
        .push(list)
        .spacing(2.0)
        .into()
}

fn muted_text(dark_mode: bool) -> Color {
    if dark_mode {
        Color::from_rgb(0.65, 0.65, 0.70)
    } else {
        Color::from_rgb(0.35, 0.38, 0.36)
    }
}

fn value_text(dark_mode: bool) -> Color {
    if dark_mode {
        Color::from_rgb(0.80, 0.85, 0.80)
    } else {
        Color::from_rgb(0.10, 0.45, 0.28)
    }
}

// ── Tests: message → theme-edit mapping ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_merge::merge_ui_theme;

    fn default_config() -> UiThemeConfig {
        UiThemeConfig::default()
    }

    fn merged(config: &UiThemeConfig) -> BoruTheme {
        merge_ui_theme(&BoruTheme::default(), config).0
    }

    #[test]
    fn apply_float_sets_exact_config_leaf() {
        let mut cfg = default_config();
        apply_float(&mut cfg, ThemeField::SidebarWidth, 270.0).unwrap();
        let theme = merged(&cfg);
        assert_eq!(theme.sidebar.width, 270.0);
        // Unrelated fields stay at defaults.
        assert_eq!(theme.sidebar.width_min, BoruTheme::default().sidebar.width_min);
        assert_eq!(theme.typography.body, BoruTheme::default().typography.body);
    }

    #[test]
    fn apply_float_rejects_non_float_field() {
        let mut cfg = default_config();
        let err = apply_float(&mut cfg, ThemeField::ColorPrimary, 1.0).unwrap_err();
        assert!(err.contains("not a float"), "{err}");
    }

    #[test]
    fn apply_bool_sets_optional_visual_feature() {
        let mut cfg = default_config();
        apply_bool(&mut cfg, ThemeField::HomeShowActivityFeed, false).unwrap();
        let theme = merged(&cfg);
        assert!(!theme.home.show_activity_feed);
        let err = apply_bool(&mut cfg, ThemeField::SidebarWidth, false).unwrap_err();
        assert!(err.contains("not a toggle"), "{err}");
    }

    #[test]
    fn apply_color_sets_exact_config_leaf() {
        let mut cfg = default_config();
        let cv = parse_hex_rgba("#187F50").unwrap();
        apply_color(&mut cfg, ThemeField::ColorPrimary, cv).unwrap();
        let theme = merged(&cfg);
        assert_eq!(theme.colors.primary, cv.to_iced());
        let err = apply_color(&mut cfg, ThemeField::SidebarWidth, cv).unwrap_err();
        assert!(err.contains("not a colour"), "{err}");
    }

    #[test]
    fn float_text_draft_applies_on_valid_parse() {
        // Simulate what update() does for FloatTextChanged.
        let mut cfg = default_config();
        let text = "288.5".to_string();
        let value: f32 = text.trim().parse().unwrap();
        apply_float(&mut cfg, ThemeField::SidebarWidth, value).unwrap();
        assert_eq!(merged(&cfg).sidebar.width, 288.5);
    }

    #[test]
    fn color_text_draft_applies_on_valid_parse() {
        let mut cfg = default_config();
        let text = "#F7F9F8";
        let cv = parse_hex_rgba(text).unwrap();
        apply_color(&mut cfg, ThemeField::ColorCanvas, cv).unwrap();
        assert_eq!(merged(&cfg).colors.canvas, cv.to_iced());
    }

    #[test]
    fn parse_hex_rgba_accepts_6_and_8_digit_and_rejects_garbage() {
        let six = parse_hex_rgba("#F7F9F8").unwrap();
        assert_eq!((six.r, six.g, six.b, six.a), (247.0 / 255.0, 249.0 / 255.0, 248.0 / 255.0, 1.0));
        let eight = parse_hex_rgba("187F5080").unwrap();
        assert!((eight.a - 128.0 / 255.0).abs() < 1e-6);
        assert!(parse_hex_rgba("not-a-color").is_none());
        assert!(parse_hex_rgba("#FFF").is_none());
        assert!(parse_hex_rgba("").is_none());
    }

    #[test]
    fn color_to_hex_round_trips() {
        let c = Color::from_rgb(0x18 as f32 / 255.0, 0x7F as f32 / 255.0, 0x50 as f32 / 255.0);
        assert_eq!(color_to_hex(c), "#187F50");
        let c8 = Color::from_rgba(0.1, 0.2, 0.3, 0.5);
        assert_eq!(color_to_hex(c8), "#1A334D80");
    }

    #[test]
    fn every_exposed_field_maps_to_a_real_config_leaf() {
        // Every field in the panel's section list must apply without error
        // (regression guard: a ThemeField added to the panel but missing
        // from apply_* would silently no-op and fail this test).
        let mut cfg = default_config();
        for section in SECTIONS {
            for group in section.groups {
                for field in group.fields {
                    match field.kind() {
                        FieldKind::Float => {
                            let (min, max) = field.range();
                            let v = (min + max) / 2.0;
                            apply_float(&mut cfg, *field, v)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Bool => {
                            apply_bool(&mut cfg, *field, true)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Color => {
                            apply_color(&mut cfg, *field, parse_hex_rgba("#123456").unwrap())
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Choice => {
                            let first = field.choices().first().copied().unwrap_or("");
                            apply_choice(&mut cfg, *field, first)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        // Layout-only kinds never occur on theme fields.
                        FieldKind::Int | FieldKind::Sections => {
                            panic!("{field:?} must not be a layout-only kind")
                        }
                    }
                }
            }
        }
        // The merged theme reflects the edits and stays finite everywhere.
        let theme = merged(&cfg);
        assert_eq!(theme.home.show_activity_feed, true);
        assert!(theme.sidebar.width.is_finite());
    }

    #[test]
    fn reads_match_writes_for_a_sample_field() {
        let mut cfg = default_config();
        apply_float(&mut cfg, ThemeField::ChatBubbleMaxWidth, 620.0).unwrap();
        let theme = merged(&cfg);
        assert_eq!(read_float(&theme, ThemeField::ChatBubbleMaxWidth), 620.0);
    }

    #[test]
    fn reset_section_clears_only_that_section() {
        let mut cfg = default_config();
        apply_float(&mut cfg, ThemeField::SidebarWidth, 270.0).unwrap();
        apply_float(&mut cfg, ThemeField::ChatBubbleMaxWidth, 620.0).unwrap();
        apply_bool(&mut cfg, ThemeField::HomeShowActivityFeed, false).unwrap();

        SectionId::Sidebar.reset(&mut cfg);

        // Sidebar group cleared back to defaults.
        assert!(cfg.sidebar.is_none());
        let theme = merged(&cfg);
        assert_eq!(theme.sidebar.width, BoruTheme::default().sidebar.width);
        // Other sections keep their edits.
        assert_eq!(theme.chat.bubble_max_width, 620.0);
        assert!(!theme.home.show_activity_feed);
    }

    #[test]
    fn reset_global_clears_all_four_global_groups() {
        let mut cfg = default_config();
        apply_color(&mut cfg, ThemeField::ColorPrimary, parse_hex_rgba("#123456").unwrap())
            .unwrap();
        apply_float(&mut cfg, ThemeField::TypeBody, 18.0).unwrap();
        apply_float(&mut cfg, ThemeField::Space8, 9.0).unwrap();
        apply_float(&mut cfg, ThemeField::RadiusCard, 20.0).unwrap();

        SectionId::Global.reset(&mut cfg);

        assert!(cfg.colors.is_none());
        assert!(cfg.typography.is_none());
        assert!(cfg.spacing.is_none());
        assert!(cfg.radii.is_none());
        let theme = merged(&cfg);
        assert_eq!(theme, BoruTheme::default());
    }

    #[test]
    fn reset_all_clears_every_config_group() {
        let mut cfg = default_config();
        apply_float(&mut cfg, ThemeField::SidebarWidth, 270.0).unwrap();
        apply_float(&mut cfg, ThemeField::ChatBubbleMaxWidth, 620.0).unwrap();
        apply_bool(&mut cfg, ThemeField::HomeShowActivityFeed, false).unwrap();
        apply_color(&mut cfg, ThemeField::ColorPrimary, parse_hex_rgba("#123456").unwrap())
            .unwrap();

        SectionId::Motion.reset(&mut cfg); // unrelated group first
        for section in SECTIONS {
            section.id.reset(&mut cfg);
        }

        assert_eq!(cfg, UiThemeConfig::default());
        assert_eq!(merged(&cfg), BoruTheme::default());
    }

    #[test]
    fn every_section_reset_restores_defaults_for_its_fields() {
        // Regression guard: Reset Section must return every field under that
        // section back to the Boru default (no field left overridden).
        for section in SECTIONS {
            let mut cfg = default_config();
            for group in section.groups {
                for field in group.fields {
                    match field.kind() {
                        FieldKind::Float => {
                            let (min, max) = field.range();
                            apply_float(&mut cfg, *field, min + 1.0)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Bool => {
                            apply_bool(&mut cfg, *field, false)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Color => {
                            apply_color(&mut cfg, *field, parse_hex_rgba("#123456").unwrap())
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Choice => {
                            let first = field.choices().first().copied().unwrap_or("");
                            apply_choice(&mut cfg, *field, first)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        // Layout-only kinds never occur on theme fields.
                        FieldKind::Int | FieldKind::Sections => {
                            panic!("{field:?} must not be a layout-only kind")
                        }
                    }
                }
            }
            let edited = merged(&cfg);
            assert_ne!(
                edited, BoruTheme::default(),
                "{} must actually edit the theme (test fixture broken)",
                section.label
            );

            section.id.reset(&mut cfg);
            let reset = merged(&cfg);
            assert_eq!(
                reset,
                BoruTheme::default(),
                "{} reset must restore defaults",
                section.label
            );
        }
    }

    #[test]
    fn section_id_reset_is_total_for_known_sections() {
        // Every config group the merge reads AND the inspector exposes is
        // owned by exactly one section; resetting all sections yields an
        // empty config. (The typed-theme groups the inspector deliberately
        // does not expose — icons, avatars, lists, borders, responsive —
        // are not owned by any section; Reset All handles them via
        // `UiThemeConfig::default()`.)
        let mut cfg = default_config();
        cfg.colors = Some(Default::default());
        cfg.typography = Some(Default::default());
        cfg.spacing = Some(Default::default());
        cfg.radii = Some(Default::default());
        cfg.sidebar = Some(Default::default());
        cfg.home = Some(Default::default());
        cfg.chat = Some(Default::default());
        cfg.attachments = Some(Default::default());
        cfg.rooms = Some(Default::default());
        cfg.tunnels = Some(Default::default());
        cfg.dialogs = Some(Default::default());
        cfg.calls = Some(Default::default());
        cfg.controls = Some(Default::default());
        cfg.motion = Some(Default::default());

        for section in SECTIONS {
            section.id.reset(&mut cfg);
        }
        assert_eq!(cfg, UiThemeConfig::default());
    }

    #[test]
    fn section_membership_matches_field_section_mapping() {
        // Every field rendered under a section must claim that section via
        // ThemeField::section(), so the hierarchy and the pure mapping never
        // drift apart.
        for section in SECTIONS {
            for group in section.groups {
                for field in group.fields {
                    assert_eq!(
                        field.section(),
                        section.id,
                        "{field:?} is listed under {} but maps to {:?}",
                        section.label,
                        field.section()
                    );
                }
            }
        }
    }

    #[test]
    fn component_id_registry_maps_to_existing_section() {
        // BORU-UI-11: every supported component maps to an inspector section
        // that actually exists in the panel, so selecting a component always
        // jumps to a real section.
        let all_sections: HashSet<SectionId> = SECTIONS.iter().map(|s| s.id).collect();
        let components = [
            ComponentId::Sidebar,
            ComponentId::Home,
            ComponentId::Chat,
            ComponentId::Attachments,
            ComponentId::Rooms,
            ComponentId::Tunnels,
            ComponentId::Dialogs,
            ComponentId::Calls,
            ComponentId::Controls,
        ];
        for component in components {
            assert!(
                all_sections.contains(&component.section()),
                "{component:?} maps to {:?} which is not in SECTIONS",
                component.section()
            );
            assert!(!component.label().is_empty(), "{component:?} has empty label");
        }
    }

    #[test]
    fn component_id_section_mapping_is_consistent_with_sections() {
        // BORU-UI-11: the section a component maps to must own at least one
        // field, otherwise "jump to section" would point at an empty group.
        for component in [
            ComponentId::Sidebar,
            ComponentId::Home,
            ComponentId::Chat,
            ComponentId::Attachments,
            ComponentId::Rooms,
            ComponentId::Tunnels,
            ComponentId::Dialogs,
            ComponentId::Calls,
            ComponentId::Controls,
        ] {
            let section = component.section();
            let fields: usize = SECTIONS
                .iter()
                .find(|s| s.id == section)
                .map(|s| s.groups.iter().map(|g| g.fields.len()).sum())
                .unwrap_or(0);
            assert!(fields > 0, "{component:?} section {section:?} has no fields");
        }
    }

    #[test]
    fn section_scroll_offset_is_monotonic_across_sections() {
        // BORU-UI-11: offsets must increase in SECTIONS order so scrolling to
        // a section header brings the right section into view.
        let collapsed = HashSet::new();
        let mut previous = section_scroll_offset(SECTIONS[0].id, &collapsed);
        for section in &SECTIONS[1..] {
            let offset = section_scroll_offset(section.id, &collapsed);
            assert!(
                offset > previous,
                "{} offset {offset} not > previous {previous}",
                section.label
            );
            previous = offset;
        }
    }

    #[test]
    fn section_scroll_offset_handles_collapsed_sections() {
        // BORU-UI-11: collapsing an earlier section shortens the content, so
        // a later section's offset shrinks accordingly.
        let empty = HashSet::new();
        let mut collapsed = HashSet::new();
        collapsed.insert(SectionId::Global);
        let global_collapsed = section_scroll_offset(SectionId::Chat, &collapsed);
        let global_expanded = section_scroll_offset(SectionId::Chat, &empty);
        assert!(
            global_collapsed < global_expanded,
            "collapsing Global should move Chat up ({global_collapsed} vs {global_expanded})"
        );
    }

    #[test]
    fn section_scroll_offset_lands_last_section_inside_panel() {
        // BORU-UI-11: the estimate for the final section should stay within
        // a plausible panel height (a few thousand px), guarding against the
        // estimator running away.
        let collapsed = HashSet::new();
        let last = section_scroll_offset(SECTIONS[SECTIONS.len() - 1].id, &collapsed);
        assert!(
            last > INSPECTOR_TOP_CHROME && last < 10_000.0,
            "last section offset {last} out of expected band"
        );
    }
}
