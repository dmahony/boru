//! Dev-only Layout Inspector (BORU-LAYOUT-08 / PDF Task 8).
//!
//! Extends the Developer UI Inspector ([`crate::inspector`]) with a
//! **Layout** section that edits every editable property of the structural
//! layout model ([`LayoutConfig`](crate::layout::LayoutConfig)) and saves
//! the result back to `boru-layout.toml`.
//!
//! ## Design rules (mirror the theme half of `inspector.rs`)
//!
//! - **Pure mapping.** [`LayoutField`] identifies one editable layout leaf;
//!   the pure `read_layout_*` / `apply_layout_*` functions map between the
//!   **live merged** [`LayoutConfig`](crate::layout::LayoutConfig) (display)
//!   and the editable [`LayoutOverrides`] (the `boru-layout.toml` override
//!   set). `app.rs` calls `apply_layout_*` and then recomputes the live
//!   layout through the same seam the `boru-layout.toml` watcher uses
//!   (`IcedChat::set_layout_overrides`), so every edit bumps
//!   `layout_revision` and redraws affected widgets immediately.
//! - **Only Some leaves are written.** Saving serializes the override set,
//!   so defaults stay code defaults and Git diffs of `boru-layout.toml`
//!   stay minimal (exactly like the theme save path).
//! - **Validation rules apply.** Duplicate section ids are rejected by the
//!   existing validation layer (BORU-LAYOUT-07) on load/reload; numeric
//!   values are clamped by the merge layer. The inspector never bypasses
//!   either.
//! - **Scope.** Home sections order/visibility, grid/list mode, column
//!   counts, max width, padding, gaps, card sizing; component thumbnail
//!   position, metadata alignment, button placement, card orientation;
//!   responsive breakpoints. Sidebar, chat and tables groups are not yet
//!   wired to views (later BORU-LAYOUT tasks), so they are deliberately
//!   not exposed here — editing a value the UI never reads would be
//!   misleading (guardrail: leave unverified values out of the live layout
//!   system).

use iced::widget::{button, pick_list, row, slider, text, text_input, Space};
use iced::{Alignment, Color, Element, Length};

use crate::inspector::{FieldKind, InspectorDraft, InspectorMsg};
use crate::layout::{HomeLayoutMode, HomeSection, LayoutConfig, LayoutOverrides};
use crate::layout_config::LAYOUT_CONFIG_FILE_NAME;

/// Top-level layout section of the inspector (BORU-LAYOUT-08).
///
/// Mirrors the layout groups of the typed [`LayoutConfig`]: Home
/// (dashboard arrangement), Component (per-component placement + media-card
/// sizing) and Responsive (breakpoints + per-tier tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutSectionId {
    /// Home dashboard: sections order/visibility, mode, grid, quick actions,
    /// max width, padding, gaps, card sizing.
    Home,
    /// Per-component placement + media-card sizing.
    Component,
    /// Responsive breakpoints + per-tier home columns/padding.
    Responsive,
}

impl LayoutSectionId {
    /// Human-readable section label.
    pub fn label(self) -> &'static str {
        match self {
            LayoutSectionId::Home => "Layout · Home",
            LayoutSectionId::Component => "Layout · Component",
            LayoutSectionId::Responsive => "Layout · Responsive",
        }
    }

    /// Reset this layout group back to defaults (clear its override group).
    /// Used by the per-section Reset action in the panel.
    pub fn reset(self, overrides: &mut LayoutOverrides) {
        match self {
            LayoutSectionId::Home => overrides.home = None,
            LayoutSectionId::Component => overrides.component = None,
            LayoutSectionId::Responsive => overrides.responsive = None,
        }
    }
}

/// Identifies one editable leaf of the structural layout model.
///
/// Every variant maps 1:1 to a [`LayoutConfig`] field (for display) and a
/// [`LayoutOverrides`] `Option` leaf (for editing). Variant names follow
/// the group + field naming convention of `layout.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutField {
    // ── Home: sections (PDF Task 3) ──
    /// Vertical section order (comma-separated section names).
    HomeSectionOrder,
    /// Sections hidden from the dashboard (comma-separated names).
    HomeHiddenSections,
    /// Grid vs list presentation mode.
    HomeMode,
    // ── Home: grid split ──
    HomeGridMainPortion,
    HomeGridRailPortion,
    HomeGridColumnGap,
    HomeGridStackBreakpoint,
    // ── Home: quick-action grid ──
    HomeQuickColumnsWide,
    HomeQuickColumnsMid,
    HomeQuickColumnsNarrow,
    HomeQuickFourColBreakpoint,
    HomeQuickTwoColBreakpoint,
    HomeQuickCardPaddingY,
    HomeQuickCardPaddingX,
    // ── Home: canvas ──
    HomeMaxContentWidth,
    // ── Home: padding ──
    HomePaddingTop,
    HomePaddingBottom,
    HomePaddingHorizontalLarge,
    HomePaddingHorizontalDefault,
    // ── Home: gaps ──
    HomeGapsCardGap,
    HomeGapsHeroGap,
    HomeGapsHeaderDashboardGap,
    HomeGapsFooterGap,
    HomeGapsCompactHeaderStackGap,
    // ── Home: card sizing ──
    HomeCardSizingPeersBodyMin,
    HomeCardSizingActivityRowHeight,
    HomeCardSizingQuickActionIconSize,
    HomeCardSizingStatusCardMinContentHeight,
    HomeCardSizingStatusCardMediumContent,
    HomeCardSizingStatusCardNarrowContent,
    HomeCardSizingStatusCardMeshHideContent,
    HomeCardSizingStatusCardTextMinWidth,
    HomeCardSizingStatusCardTextMinWidthMedium,
    HomeCardSizingStatusCardMeshMaxWidth,
    HomeCardSizingStatusCardPaddingX,
    HomeCardSizingStatusIconTextGapFull,
    HomeCardSizingStatusIconTextGapMedium,
    HomeCardSizingStatusTextGraphGapFull,
    HomeCardSizingStatusTextGraphGapMedium,
    HomeCardSizingStatusDividerWidth,
    HomeCardSizingStatusDividerHeight,
    // ── Component: global fallback placement (PDF Task 5) ──
    ComponentThumbnailPosition,
    ComponentMetadataAlignment,
    ComponentButtonPlacement,
    ComponentCardOrientation,
    // ── Component: video/file card placement ──
    ComponentVideoCardThumbnailPosition,
    ComponentVideoCardMetadataAlignment,
    ComponentVideoCardButtonPlacement,
    ComponentVideoCardCardOrientation,
    // ── Component: "Files I'm Sharing" row placement ──
    ComponentSharedByMeThumbnailPosition,
    ComponentSharedByMeMetadataAlignment,
    ComponentSharedByMeButtonPlacement,
    ComponentSharedByMeCardOrientation,
    // ── Component: video card sizing ──
    ComponentVideoNarrowBreakpoint,
    ComponentVideoMediumBreakpoint,
    ComponentVideoPlayOverlaySize,
    ComponentVideoHeaderFilenameMaxWidth,
    ComponentVideoControlsSliderWidth,
    // ── Responsive: viewport reference sizes (PDF Task 4) ──
    ResponsiveViewportRefWidth,
    ResponsiveViewportRefHeight,
    ResponsiveViewportMinWidth,
    ResponsiveViewportMinHeight,
    ResponsiveViewportLgWidth,
    ResponsiveViewportLgHeight,
    ResponsiveViewportXlWidth,
    ResponsiveViewportXlHeight,
    // ── Responsive: content-width breakpoints ──
    ResponsiveContentMaxWidth,
    ResponsiveHomeIllustrationFullContent,
    ResponsiveHomeIllustrationHideContent,
    ResponsiveHomeCompactHeaderContent,
    // ── Responsive: tier thresholds ──
    ResponsiveNarrowMaxWidth,
    ResponsiveUltraWideMinWidth,
    // ── Responsive: per-tier home columns ──
    ResponsiveHomeColumnsNarrow,
    ResponsiveHomeColumnsDesktop,
    ResponsiveHomeColumnsUltraWide,
    // ── Responsive: per-tier home horizontal padding ──
    ResponsiveHomePaddingXNarrow,
    ResponsiveHomePaddingXDesktop,
    ResponsiveHomePaddingXUltraWide,
}

impl LayoutField {
    /// Human-readable label shown beside the control.
    pub fn label(self) -> &'static str {
        use LayoutField::*;
        match self {
            HomeSectionOrder => "Section order",
            HomeHiddenSections => "Hidden sections",
            HomeMode => "Mode",
            HomeGridMainPortion => "Main column portion",
            HomeGridRailPortion => "Rail portion",
            HomeGridColumnGap => "Column gap",
            HomeGridStackBreakpoint => "Stack below width",
            HomeQuickColumnsWide => "Columns (wide)",
            HomeQuickColumnsMid => "Columns (mid)",
            HomeQuickColumnsNarrow => "Columns (narrow)",
            HomeQuickFourColBreakpoint => "4-col breakpoint",
            HomeQuickTwoColBreakpoint => "2-col breakpoint",
            HomeQuickCardPaddingY => "Quick-action card padding y",
            HomeQuickCardPaddingX => "Quick-action card padding x",
            HomeMaxContentWidth => "Max content width",
            HomePaddingTop => "Top padding",
            HomePaddingBottom => "Bottom padding",
            HomePaddingHorizontalLarge => "Horizontal padding (large)",
            HomePaddingHorizontalDefault => "Horizontal padding (default)",
            HomeGapsCardGap => "Card gap",
            HomeGapsHeroGap => "Hero gap",
            HomeGapsHeaderDashboardGap => "Header → dashboard gap",
            HomeGapsFooterGap => "Footer gap",
            HomeGapsCompactHeaderStackGap => "Compact header stack gap",
            HomeCardSizingPeersBodyMin => "Peers body min height",
            HomeCardSizingActivityRowHeight => "Activity row height",
            HomeCardSizingQuickActionIconSize => "Quick-action icon size",
            HomeCardSizingStatusCardMinContentHeight => "Status card min height",
            HomeCardSizingStatusCardMediumContent => "Status card medium ≥",
            HomeCardSizingStatusCardNarrowContent => "Status card narrow ≥",
            HomeCardSizingStatusCardMeshHideContent => "Status card mesh hide <",
            HomeCardSizingStatusCardTextMinWidth => "Status text min width",
            HomeCardSizingStatusCardTextMinWidthMedium => "Status text min width (medium)",
            HomeCardSizingStatusCardMeshMaxWidth => "Status mesh max width",
            HomeCardSizingStatusCardPaddingX => "Status card padding x",
            HomeCardSizingStatusIconTextGapFull => "Status icon→text gap (full)",
            HomeCardSizingStatusIconTextGapMedium => "Status icon→text gap (medium)",
            HomeCardSizingStatusTextGraphGapFull => "Status text→graph gap (full)",
            HomeCardSizingStatusTextGraphGapMedium => "Status text→graph gap (medium)",
            HomeCardSizingStatusDividerWidth => "Status divider width",
            HomeCardSizingStatusDividerHeight => "Status divider height",
            ComponentThumbnailPosition => "Thumbnail position",
            ComponentMetadataAlignment => "Metadata alignment",
            ComponentButtonPlacement => "Button placement",
            ComponentCardOrientation => "Card orientation",
            ComponentVideoCardThumbnailPosition => "Video card · thumbnail",
            ComponentVideoCardMetadataAlignment => "Video card · metadata",
            ComponentVideoCardButtonPlacement => "Video card · buttons",
            ComponentVideoCardCardOrientation => "Video card · orientation",
            ComponentSharedByMeThumbnailPosition => "Shared-by-me · thumbnail",
            ComponentSharedByMeMetadataAlignment => "Shared-by-me · metadata",
            ComponentSharedByMeButtonPlacement => "Shared-by-me · buttons",
            ComponentSharedByMeCardOrientation => "Shared-by-me · orientation",
            ComponentVideoNarrowBreakpoint => "Video narrow breakpoint",
            ComponentVideoMediumBreakpoint => "Video medium breakpoint",
            ComponentVideoPlayOverlaySize => "Video play overlay size",
            ComponentVideoHeaderFilenameMaxWidth => "Video filename max width",
            ComponentVideoControlsSliderWidth => "Video volume slider width",
            ResponsiveViewportRefWidth => "Reference width",
            ResponsiveViewportRefHeight => "Reference height",
            ResponsiveViewportMinWidth => "Minimum width",
            ResponsiveViewportMinHeight => "Minimum height",
            ResponsiveViewportLgWidth => "Large width",
            ResponsiveViewportLgHeight => "Large height",
            ResponsiveViewportXlWidth => "Ultra-wide width",
            ResponsiveViewportXlHeight => "Ultra-wide height",
            ResponsiveContentMaxWidth => "Content max width",
            ResponsiveHomeIllustrationFullContent => "Hero illustration full ≥",
            ResponsiveHomeIllustrationHideContent => "Hero illustration hide <",
            ResponsiveHomeCompactHeaderContent => "Compact header ≥",
            ResponsiveNarrowMaxWidth => "Narrow tier < width",
            ResponsiveUltraWideMinWidth => "Ultra-wide tier ≥ width",
            ResponsiveHomeColumnsNarrow => "Columns · narrow",
            ResponsiveHomeColumnsDesktop => "Columns · desktop",
            ResponsiveHomeColumnsUltraWide => "Columns · ultra-wide",
            ResponsiveHomePaddingXNarrow => "Padding x · narrow",
            ResponsiveHomePaddingXDesktop => "Padding x · desktop",
            ResponsiveHomePaddingXUltraWide => "Padding x · ultra-wide",
        }
    }

    /// The layout section the field is rendered under.
    pub fn section(self) -> LayoutSectionId {
        use LayoutField::*;
        match self {
            HomeSectionOrder
            | HomeHiddenSections
            | HomeMode
            | HomeGridMainPortion
            | HomeGridRailPortion
            | HomeGridColumnGap
            | HomeGridStackBreakpoint
            | HomeQuickColumnsWide
            | HomeQuickColumnsMid
            | HomeQuickColumnsNarrow
            | HomeQuickFourColBreakpoint
            | HomeQuickTwoColBreakpoint
            | HomeQuickCardPaddingY
            | HomeQuickCardPaddingX
            | HomeMaxContentWidth
            | HomePaddingTop
            | HomePaddingBottom
            | HomePaddingHorizontalLarge
            | HomePaddingHorizontalDefault
            | HomeGapsCardGap
            | HomeGapsHeroGap
            | HomeGapsHeaderDashboardGap
            | HomeGapsFooterGap
            | HomeGapsCompactHeaderStackGap
            | HomeCardSizingPeersBodyMin
            | HomeCardSizingActivityRowHeight
            | HomeCardSizingQuickActionIconSize
            | HomeCardSizingStatusCardMinContentHeight
            | HomeCardSizingStatusCardMediumContent
            | HomeCardSizingStatusCardNarrowContent
            | HomeCardSizingStatusCardMeshHideContent
            | HomeCardSizingStatusCardTextMinWidth
            | HomeCardSizingStatusCardTextMinWidthMedium
            | HomeCardSizingStatusCardMeshMaxWidth
            | HomeCardSizingStatusCardPaddingX
            | HomeCardSizingStatusIconTextGapFull
            | HomeCardSizingStatusIconTextGapMedium
            | HomeCardSizingStatusTextGraphGapFull
            | HomeCardSizingStatusTextGraphGapMedium
            | HomeCardSizingStatusDividerWidth
            | HomeCardSizingStatusDividerHeight => LayoutSectionId::Home,
            ComponentThumbnailPosition
            | ComponentMetadataAlignment
            | ComponentButtonPlacement
            | ComponentCardOrientation
            | ComponentVideoCardThumbnailPosition
            | ComponentVideoCardMetadataAlignment
            | ComponentVideoCardButtonPlacement
            | ComponentVideoCardCardOrientation
            | ComponentSharedByMeThumbnailPosition
            | ComponentSharedByMeMetadataAlignment
            | ComponentSharedByMeButtonPlacement
            | ComponentSharedByMeCardOrientation
            | ComponentVideoNarrowBreakpoint
            | ComponentVideoMediumBreakpoint
            | ComponentVideoPlayOverlaySize
            | ComponentVideoHeaderFilenameMaxWidth
            | ComponentVideoControlsSliderWidth => LayoutSectionId::Component,
            ResponsiveViewportRefWidth
            | ResponsiveViewportRefHeight
            | ResponsiveViewportMinWidth
            | ResponsiveViewportMinHeight
            | ResponsiveViewportLgWidth
            | ResponsiveViewportLgHeight
            | ResponsiveViewportXlWidth
            | ResponsiveViewportXlHeight
            | ResponsiveContentMaxWidth
            | ResponsiveHomeIllustrationFullContent
            | ResponsiveHomeIllustrationHideContent
            | ResponsiveHomeCompactHeaderContent
            | ResponsiveNarrowMaxWidth
            | ResponsiveUltraWideMinWidth
            | ResponsiveHomeColumnsNarrow
            | ResponsiveHomeColumnsDesktop
            | ResponsiveHomeColumnsUltraWide
            | ResponsiveHomePaddingXNarrow
            | ResponsiveHomePaddingXDesktop
            | ResponsiveHomePaddingXUltraWide => LayoutSectionId::Responsive,
        }
    }

    /// Value type driving which control is rendered.
    pub fn kind(self) -> FieldKind {
        use LayoutField::*;
        match self {
            HomeSectionOrder | HomeHiddenSections => FieldKind::Sections,
            HomeMode
            | ComponentThumbnailPosition
            | ComponentMetadataAlignment
            | ComponentButtonPlacement
            | ComponentCardOrientation
            | ComponentVideoCardThumbnailPosition
            | ComponentVideoCardMetadataAlignment
            | ComponentVideoCardButtonPlacement
            | ComponentVideoCardCardOrientation
            | ComponentSharedByMeThumbnailPosition
            | ComponentSharedByMeMetadataAlignment
            | ComponentSharedByMeButtonPlacement
            | ComponentSharedByMeCardOrientation => FieldKind::Choice,
            HomeGridMainPortion
            | HomeGridRailPortion
            | HomeQuickColumnsWide
            | HomeQuickColumnsMid
            | HomeQuickColumnsNarrow
            | ResponsiveHomeColumnsNarrow
            | ResponsiveHomeColumnsDesktop
            | ResponsiveHomeColumnsUltraWide => FieldKind::Int,
            _ => FieldKind::Float,
        }
    }

    /// Slider bounds for float/int fields (a sane band around the default).
    pub fn range(self) -> (f32, f32) {
        use LayoutField::*;
        match self {
            HomeMaxContentWidth => (600.0, 2400.0),
            HomeGridColumnGap
            | HomePaddingTop
            | HomePaddingBottom
            | HomePaddingHorizontalLarge
            | HomePaddingHorizontalDefault
            | HomeGapsCardGap
            | HomeGapsHeroGap
            | HomeGapsHeaderDashboardGap
            | HomeGapsFooterGap
            | HomeGapsCompactHeaderStackGap
            | HomeCardSizingStatusCardPaddingX
            | HomeCardSizingStatusIconTextGapFull
            | HomeCardSizingStatusIconTextGapMedium
            | HomeCardSizingStatusTextGraphGapFull
            | HomeCardSizingStatusTextGraphGapMedium
            | ResponsiveHomePaddingXNarrow
            | ResponsiveHomePaddingXDesktop
            | ResponsiveHomePaddingXUltraWide => (0.0, 128.0),
            HomeGridStackBreakpoint
            | HomeQuickFourColBreakpoint
            | HomeQuickTwoColBreakpoint
            | HomeQuickCardPaddingY
            | HomeQuickCardPaddingX
            | ComponentVideoNarrowBreakpoint
            | ComponentVideoMediumBreakpoint
            | ResponsiveHomeIllustrationFullContent
            | ResponsiveHomeIllustrationHideContent
            | ResponsiveHomeCompactHeaderContent => (200.0, 1600.0),
            HomeCardSizingPeersBodyMin => (0.0, 400.0),
            HomeCardSizingActivityRowHeight => (16.0, 96.0),
            HomeCardSizingQuickActionIconSize => (16.0, 96.0),
            HomeCardSizingStatusCardMinContentHeight => (0.0, 400.0),
            HomeCardSizingStatusCardMediumContent
            | HomeCardSizingStatusCardNarrowContent
            | HomeCardSizingStatusCardMeshHideContent => (200.0, 1600.0),
            HomeCardSizingStatusCardTextMinWidth | HomeCardSizingStatusCardTextMinWidthMedium => {
                (0.0, 600.0)
            }
            HomeCardSizingStatusCardMeshMaxWidth => (0.0, 400.0),
            HomeCardSizingStatusDividerWidth => (0.0, 200.0),
            HomeCardSizingStatusDividerHeight => (0.0, 64.0),
            ComponentVideoPlayOverlaySize => (16.0, 160.0),
            ComponentVideoHeaderFilenameMaxWidth => (100.0, 1200.0),
            ComponentVideoControlsSliderWidth => (20.0, 300.0),
            ResponsiveViewportRefWidth
            | ResponsiveViewportMinWidth
            | ResponsiveViewportLgWidth
            | ResponsiveViewportXlWidth => (200.0, 4000.0),
            ResponsiveViewportRefHeight
            | ResponsiveViewportMinHeight
            | ResponsiveViewportLgHeight
            | ResponsiveViewportXlHeight => (200.0, 2400.0),
            ResponsiveContentMaxWidth => (200.0, 2000.0),
            ResponsiveNarrowMaxWidth => (200.0, 1200.0),
            ResponsiveUltraWideMinWidth => (600.0, 4000.0),
            // Int fields: column counts / FillPortion splits.
            HomeGridMainPortion
            | HomeGridRailPortion
            | HomeQuickColumnsWide
            | HomeQuickColumnsMid
            | HomeQuickColumnsNarrow
            | ResponsiveHomeColumnsNarrow
            | ResponsiveHomeColumnsDesktop
            | ResponsiveHomeColumnsUltraWide => (1.0, 8.0),
            // Sections / Choice fields never use the slider range.
            HomeSectionOrder
            | HomeHiddenSections
            | HomeMode
            | ComponentThumbnailPosition
            | ComponentMetadataAlignment
            | ComponentButtonPlacement
            | ComponentCardOrientation
            | ComponentVideoCardThumbnailPosition
            | ComponentVideoCardMetadataAlignment
            | ComponentVideoCardButtonPlacement
            | ComponentVideoCardCardOrientation
            | ComponentSharedByMeThumbnailPosition
            | ComponentSharedByMeMetadataAlignment
            | ComponentSharedByMeButtonPlacement
            | ComponentSharedByMeCardOrientation => (0.0, 1.0),
        }
    }

    /// The selectable options for a Choice field. The value's serialised
    /// name (its `Debug` spelling, which matches the TOML spelling) is the
    /// option's string.
    pub fn choices(self) -> &'static [&'static str] {
        use LayoutField::*;
        match self {
            HomeMode => &["Row", "Column", "Grid", "List"],
            ComponentThumbnailPosition
            | ComponentVideoCardThumbnailPosition
            | ComponentSharedByMeThumbnailPosition => &["Left", "Right", "Top", "Bottom", "Hidden"],
            ComponentMetadataAlignment
            | ComponentVideoCardMetadataAlignment
            | ComponentSharedByMeMetadataAlignment => &["Start", "Center", "End"],
            ComponentButtonPlacement
            | ComponentVideoCardButtonPlacement
            | ComponentSharedByMeButtonPlacement => &["Below", "Overlay", "Side"],
            ComponentCardOrientation
            | ComponentVideoCardCardOrientation
            | ComponentSharedByMeCardOrientation => &["Horizontal", "Vertical"],
            _ => &[],
        }
    }
}

// ── Read from the ACTIVE merged layout (display side) ────────────────

/// Read a float leaf from the live merged layout.
pub fn read_layout_float(layout: &LayoutConfig, field: LayoutField) -> f32 {
    use LayoutField::*;
    let h = &layout.home;
    match field {
        HomeGridColumnGap => h.grid.column_gap,
        HomeGridStackBreakpoint => h.grid.stack_breakpoint,
        HomeQuickFourColBreakpoint => h.quick_actions.four_col_breakpoint,
        HomeQuickTwoColBreakpoint => h.quick_actions.two_col_breakpoint,
        HomeQuickCardPaddingY => h.quick_actions.card_padding_y,
        HomeQuickCardPaddingX => h.quick_actions.card_padding_x,
        HomeMaxContentWidth => h.max_content_width,
        HomePaddingTop => h.padding.top,
        HomePaddingBottom => h.padding.bottom,
        HomePaddingHorizontalLarge => h.padding.horizontal_large,
        HomePaddingHorizontalDefault => h.padding.horizontal_default,
        HomeGapsCardGap => h.gaps.card_gap,
        HomeGapsHeroGap => h.gaps.hero_gap,
        HomeGapsHeaderDashboardGap => h.gaps.header_dashboard_gap,
        HomeGapsFooterGap => h.gaps.footer_gap,
        HomeGapsCompactHeaderStackGap => h.gaps.compact_header_stack_gap,
        HomeCardSizingPeersBodyMin => h.card_sizing.peers_body_min,
        HomeCardSizingActivityRowHeight => h.card_sizing.activity_row_height,
        HomeCardSizingQuickActionIconSize => h.card_sizing.quick_action_icon_size,
        HomeCardSizingStatusCardMinContentHeight => h.card_sizing.status_card_min_content_height,
        HomeCardSizingStatusCardMediumContent => h.card_sizing.status_card_medium_content,
        HomeCardSizingStatusCardNarrowContent => h.card_sizing.status_card_narrow_content,
        HomeCardSizingStatusCardMeshHideContent => h.card_sizing.status_card_mesh_hide_content,
        HomeCardSizingStatusCardTextMinWidth => h.card_sizing.status_card_text_min_width,
        HomeCardSizingStatusCardTextMinWidthMedium => {
            h.card_sizing.status_card_text_min_width_medium
        }
        HomeCardSizingStatusCardMeshMaxWidth => h.card_sizing.status_card_mesh_max_width,
        HomeCardSizingStatusCardPaddingX => h.card_sizing.status_card_padding_x,
        HomeCardSizingStatusIconTextGapFull => h.card_sizing.status_icon_text_gap_full,
        HomeCardSizingStatusIconTextGapMedium => h.card_sizing.status_icon_text_gap_medium,
        HomeCardSizingStatusTextGraphGapFull => h.card_sizing.status_text_graph_gap_full,
        HomeCardSizingStatusTextGraphGapMedium => h.card_sizing.status_text_graph_gap_medium,
        HomeCardSizingStatusDividerWidth => h.card_sizing.status_divider_width,
        HomeCardSizingStatusDividerHeight => h.card_sizing.status_divider_height,
        ComponentVideoNarrowBreakpoint => layout.component.video.narrow_breakpoint,
        ComponentVideoMediumBreakpoint => layout.component.video.medium_breakpoint,
        ComponentVideoPlayOverlaySize => layout.component.video.play_overlay_size,
        ComponentVideoHeaderFilenameMaxWidth => layout.component.video.header_filename_max_width,
        ComponentVideoControlsSliderWidth => layout.component.video.controls_slider_width,
        ResponsiveViewportRefWidth => layout.responsive.viewport_ref_width,
        ResponsiveViewportRefHeight => layout.responsive.viewport_ref_height,
        ResponsiveViewportMinWidth => layout.responsive.viewport_min_width,
        ResponsiveViewportMinHeight => layout.responsive.viewport_min_height,
        ResponsiveViewportLgWidth => layout.responsive.viewport_lg_width,
        ResponsiveViewportLgHeight => layout.responsive.viewport_lg_height,
        ResponsiveViewportXlWidth => layout.responsive.viewport_xl_width,
        ResponsiveViewportXlHeight => layout.responsive.viewport_xl_height,
        ResponsiveContentMaxWidth => layout.responsive.content_max_width,
        ResponsiveHomeIllustrationFullContent => layout.responsive.home_illustration_full_content,
        ResponsiveHomeIllustrationHideContent => layout.responsive.home_illustration_hide_content,
        ResponsiveHomeCompactHeaderContent => layout.responsive.home_compact_header_content,
        ResponsiveNarrowMaxWidth => layout.responsive.narrow_max_width,
        ResponsiveUltraWideMinWidth => layout.responsive.ultra_wide_min_width,
        ResponsiveHomePaddingXNarrow => layout.responsive.home_padding_x.narrow,
        ResponsiveHomePaddingXDesktop => layout.responsive.home_padding_x.desktop,
        ResponsiveHomePaddingXUltraWide => layout.responsive.home_padding_x.ultra_wide,
        // Int / Choice / Sections fields have no float read; the caller
        // checks `kind()` first.
        HomeSectionOrder
        | HomeHiddenSections
        | HomeMode
        | HomeGridMainPortion
        | HomeGridRailPortion
        | HomeQuickColumnsWide
        | HomeQuickColumnsMid
        | HomeQuickColumnsNarrow
        | ComponentThumbnailPosition
        | ComponentMetadataAlignment
        | ComponentButtonPlacement
        | ComponentCardOrientation
        | ComponentVideoCardThumbnailPosition
        | ComponentVideoCardMetadataAlignment
        | ComponentVideoCardButtonPlacement
        | ComponentVideoCardCardOrientation
        | ComponentSharedByMeThumbnailPosition
        | ComponentSharedByMeMetadataAlignment
        | ComponentSharedByMeButtonPlacement
        | ComponentSharedByMeCardOrientation
        | ResponsiveHomeColumnsNarrow
        | ResponsiveHomeColumnsDesktop
        | ResponsiveHomeColumnsUltraWide => 0.0,
    }
}

/// Read an integer leaf (column counts, FillPortion splits) from the live
/// merged layout as `i64` (the config slots are `usize`/`u16`).
pub fn read_layout_int(layout: &LayoutConfig, field: LayoutField) -> i64 {
    use LayoutField::*;
    let h = &layout.home;
    match field {
        HomeGridMainPortion => h.grid.main_portion as i64,
        HomeGridRailPortion => h.grid.rail_portion as i64,
        HomeQuickColumnsWide => h.quick_actions.columns_wide as i64,
        HomeQuickColumnsMid => h.quick_actions.columns_mid as i64,
        HomeQuickColumnsNarrow => h.quick_actions.columns_narrow as i64,
        ResponsiveHomeColumnsNarrow => layout.responsive.home_columns.narrow as i64,
        ResponsiveHomeColumnsDesktop => layout.responsive.home_columns.desktop as i64,
        ResponsiveHomeColumnsUltraWide => layout.responsive.home_columns.ultra_wide as i64,
        _ => 0,
    }
}

/// Read the current value of a Choice field as the selected option string
/// (the variant's `Debug`/TOML spelling). Returns `&'static str` so the
/// pick_list in the panel can borrow it without leaking per-frame strings.
pub fn read_layout_choice(layout: &LayoutConfig, field: LayoutField) -> &'static str {
    use crate::layout::{
        ButtonPlacement::*, CardOrientation::*, HomeLayoutMode::*, MetadataAlignment::*,
        ThumbnailPosition::*,
    };
    use LayoutField::*;
    let c = &layout.component;
    match field {
        HomeMode => match layout.home.mode {
            Row => "Row",
            Column => "Column",
            Grid => "Grid",
            List => "List",
        },
        ComponentThumbnailPosition => match c.thumbnail_position {
            Left => "Left",
            Right => "Right",
            Top => "Top",
            Bottom => "Bottom",
            Hidden => "Hidden",
        },
        ComponentMetadataAlignment => match c.metadata_alignment {
            Start => "Start",
            Center => "Center",
            End => "End",
        },
        ComponentButtonPlacement => match c.button_placement {
            Below => "Below",
            Overlay => "Overlay",
            Side => "Side",
        },
        ComponentCardOrientation => match c.card_orientation {
            Horizontal => "Horizontal",
            Vertical => "Vertical",
        },
        ComponentVideoCardThumbnailPosition => match c.video_card.thumbnail_position {
            Left => "Left",
            Right => "Right",
            Top => "Top",
            Bottom => "Bottom",
            Hidden => "Hidden",
        },
        ComponentVideoCardMetadataAlignment => match c.video_card.metadata_alignment {
            Start => "Start",
            Center => "Center",
            End => "End",
        },
        ComponentVideoCardButtonPlacement => match c.video_card.button_placement {
            Below => "Below",
            Overlay => "Overlay",
            Side => "Side",
        },
        ComponentVideoCardCardOrientation => match c.video_card.card_orientation {
            Horizontal => "Horizontal",
            Vertical => "Vertical",
        },
        ComponentSharedByMeThumbnailPosition => match c.shared_by_me.thumbnail_position {
            Left => "Left",
            Right => "Right",
            Top => "Top",
            Bottom => "Bottom",
            Hidden => "Hidden",
        },
        ComponentSharedByMeMetadataAlignment => match c.shared_by_me.metadata_alignment {
            Start => "Start",
            Center => "Center",
            End => "End",
        },
        ComponentSharedByMeButtonPlacement => match c.shared_by_me.button_placement {
            Below => "Below",
            Overlay => "Overlay",
            Side => "Side",
        },
        ComponentSharedByMeCardOrientation => match c.shared_by_me.card_orientation {
            Horizontal => "Horizontal",
            Vertical => "Vertical",
        },
        _ => "",
    }
}

/// Read a section list as comma-separated names (the sections text input).
pub fn read_layout_sections(layout: &LayoutConfig, field: LayoutField) -> String {
    use LayoutField::*;
    let join = |ids: &[HomeSection]| {
        ids.iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match field {
        HomeSectionOrder => join(&layout.home.section_order),
        HomeHiddenSections => join(&layout.home.hidden_sections),
        _ => String::new(),
    }
}

// ── Apply to the overrides (message → layout-edit mapping) ───────────

/// Parse a unit enum variant from its TOML spelling (e.g. `"List"` →
/// `HomeLayoutMode::List`). The layout enums derive `Deserialize`, and a
/// quoted string is how TOML represents a unit variant. serde_json is
/// used for the string→enum step because toml's top-level string
/// deserializer does not drive `deserialize_enum` the way a nested table
/// value does.
fn parse_enum_variant<T>(value: &str) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str::<T>(&format!("\"{value}\"")).ok()
}

/// Parse a comma-separated list of home section names into typed ids.
/// Empty input (or a list of only separators) yields an empty vector —
/// useful for clearing `hidden_sections`. An unknown name is an error, so
/// a half-typed list simply does not apply yet.
fn parse_home_sections(value: &str) -> Result<Vec<HomeSection>, String> {
    let mut out = Vec::new();
    for part in value.split(',') {
        let name = part.trim();
        if name.is_empty() {
            continue;
        }
        let section = parse_enum_variant::<HomeSection>(name)
            .ok_or_else(|| format!("unknown home section {name:?}"))?;
        out.push(section);
    }
    Ok(out)
}

/// Apply a float edit to the stored layout overrides.
///
/// This is the pure mapping half of the layout inspector: it mutates only
/// the overrides (never any view state). `app.rs` calls it and then
/// recomputes the live layout via `set_layout_overrides`.
pub fn apply_layout_float(
    overrides: &mut LayoutOverrides,
    field: LayoutField,
    value: f32,
) -> Result<(), String> {
    use LayoutField::*;
    if !matches!(field.kind(), FieldKind::Float) {
        return Err(format!("{} is not a float field", field.label()));
    }
    let set = |slot: &mut Option<f32>| *slot = Some(value);
    match field {
        HomeGridColumnGap => set(&mut ov_home_grid(overrides).column_gap),
        HomeGridStackBreakpoint => set(&mut ov_home_grid(overrides).stack_breakpoint),
        HomeQuickFourColBreakpoint => set(&mut ov_home_quick(overrides).four_col_breakpoint),
        HomeQuickTwoColBreakpoint => set(&mut ov_home_quick(overrides).two_col_breakpoint),
        HomeQuickCardPaddingY => set(&mut ov_home_quick(overrides).card_padding_y),
        HomeQuickCardPaddingX => set(&mut ov_home_quick(overrides).card_padding_x),
        HomeMaxContentWidth => set(&mut ov_home(overrides).max_content_width),
        HomePaddingTop => set(&mut ov_home_padding(overrides).top),
        HomePaddingBottom => set(&mut ov_home_padding(overrides).bottom),
        HomePaddingHorizontalLarge => set(&mut ov_home_padding(overrides).horizontal_large),
        HomePaddingHorizontalDefault => set(&mut ov_home_padding(overrides).horizontal_default),
        HomeGapsCardGap => set(&mut ov_home_gaps(overrides).card_gap),
        HomeGapsHeroGap => set(&mut ov_home_gaps(overrides).hero_gap),
        HomeGapsHeaderDashboardGap => set(&mut ov_home_gaps(overrides).header_dashboard_gap),
        HomeGapsFooterGap => set(&mut ov_home_gaps(overrides).footer_gap),
        HomeGapsCompactHeaderStackGap => set(&mut ov_home_gaps(overrides).compact_header_stack_gap),
        HomeCardSizingPeersBodyMin => set(&mut ov_home_card(overrides).peers_body_min),
        HomeCardSizingActivityRowHeight => set(&mut ov_home_card(overrides).activity_row_height),
        HomeCardSizingQuickActionIconSize => {
            set(&mut ov_home_card(overrides).quick_action_icon_size)
        }
        HomeCardSizingStatusCardMinContentHeight => {
            set(&mut ov_home_card(overrides).status_card_min_content_height)
        }
        HomeCardSizingStatusCardMediumContent => {
            set(&mut ov_home_card(overrides).status_card_medium_content)
        }
        HomeCardSizingStatusCardNarrowContent => {
            set(&mut ov_home_card(overrides).status_card_narrow_content)
        }
        HomeCardSizingStatusCardMeshHideContent => {
            set(&mut ov_home_card(overrides).status_card_mesh_hide_content)
        }
        HomeCardSizingStatusCardTextMinWidth => {
            set(&mut ov_home_card(overrides).status_card_text_min_width)
        }
        HomeCardSizingStatusCardTextMinWidthMedium => {
            set(&mut ov_home_card(overrides).status_card_text_min_width_medium)
        }
        HomeCardSizingStatusCardMeshMaxWidth => {
            set(&mut ov_home_card(overrides).status_card_mesh_max_width)
        }
        HomeCardSizingStatusCardPaddingX => set(&mut ov_home_card(overrides).status_card_padding_x),
        HomeCardSizingStatusIconTextGapFull => {
            set(&mut ov_home_card(overrides).status_icon_text_gap_full)
        }
        HomeCardSizingStatusIconTextGapMedium => {
            set(&mut ov_home_card(overrides).status_icon_text_gap_medium)
        }
        HomeCardSizingStatusTextGraphGapFull => {
            set(&mut ov_home_card(overrides).status_text_graph_gap_full)
        }
        HomeCardSizingStatusTextGraphGapMedium => {
            set(&mut ov_home_card(overrides).status_text_graph_gap_medium)
        }
        HomeCardSizingStatusDividerWidth => set(&mut ov_home_card(overrides).status_divider_width),
        HomeCardSizingStatusDividerHeight => {
            set(&mut ov_home_card(overrides).status_divider_height)
        }
        ComponentVideoNarrowBreakpoint => set(&mut ov_video(overrides).narrow_breakpoint),
        ComponentVideoMediumBreakpoint => set(&mut ov_video(overrides).medium_breakpoint),
        ComponentVideoPlayOverlaySize => set(&mut ov_video(overrides).play_overlay_size),
        ComponentVideoHeaderFilenameMaxWidth => {
            set(&mut ov_video(overrides).header_filename_max_width)
        }
        ComponentVideoControlsSliderWidth => set(&mut ov_video(overrides).controls_slider_width),
        ResponsiveViewportRefWidth => set(&mut ov_responsive(overrides).viewport_ref_width),
        ResponsiveViewportRefHeight => set(&mut ov_responsive(overrides).viewport_ref_height),
        ResponsiveViewportMinWidth => set(&mut ov_responsive(overrides).viewport_min_width),
        ResponsiveViewportMinHeight => set(&mut ov_responsive(overrides).viewport_min_height),
        ResponsiveViewportLgWidth => set(&mut ov_responsive(overrides).viewport_lg_width),
        ResponsiveViewportLgHeight => set(&mut ov_responsive(overrides).viewport_lg_height),
        ResponsiveViewportXlWidth => set(&mut ov_responsive(overrides).viewport_xl_width),
        ResponsiveViewportXlHeight => set(&mut ov_responsive(overrides).viewport_xl_height),
        ResponsiveContentMaxWidth => set(&mut ov_responsive(overrides).content_max_width),
        ResponsiveHomeIllustrationFullContent => {
            set(&mut ov_responsive(overrides).home_illustration_full_content)
        }
        ResponsiveHomeIllustrationHideContent => {
            set(&mut ov_responsive(overrides).home_illustration_hide_content)
        }
        ResponsiveHomeCompactHeaderContent => {
            set(&mut ov_responsive(overrides).home_compact_header_content)
        }
        ResponsiveNarrowMaxWidth => set(&mut ov_responsive(overrides).narrow_max_width),
        ResponsiveUltraWideMinWidth => set(&mut ov_responsive(overrides).ultra_wide_min_width),
        ResponsiveHomePaddingXNarrow => set(&mut ov_resp_padding_x(overrides).narrow),
        ResponsiveHomePaddingXDesktop => set(&mut ov_resp_padding_x(overrides).desktop),
        ResponsiveHomePaddingXUltraWide => set(&mut ov_resp_padding_x(overrides).ultra_wide),
        // Non-float fields rejected above.
        _ => {}
    }
    Ok(())
}

/// Apply an integer edit (column counts, FillPortion splits) to the stored
/// layout overrides. The merge layer clamps absurd values (BORU-LAYOUT-07),
/// so a bad number can never produce a broken layout.
pub fn apply_layout_int(
    overrides: &mut LayoutOverrides,
    field: LayoutField,
    value: i64,
) -> Result<(), String> {
    use LayoutField::*;
    if !matches!(field.kind(), FieldKind::Int) {
        return Err(format!("{} is not an integer field", field.label()));
    }
    match field {
        HomeGridMainPortion => ov_home_grid(overrides).main_portion = Some(value as u16),
        HomeGridRailPortion => ov_home_grid(overrides).rail_portion = Some(value as u16),
        HomeQuickColumnsWide => ov_home_quick(overrides).columns_wide = Some(value as usize),
        HomeQuickColumnsMid => ov_home_quick(overrides).columns_mid = Some(value as usize),
        HomeQuickColumnsNarrow => ov_home_quick(overrides).columns_narrow = Some(value as usize),
        ResponsiveHomeColumnsNarrow => ov_resp_columns(overrides).narrow = Some(value as usize),
        ResponsiveHomeColumnsDesktop => ov_resp_columns(overrides).desktop = Some(value as usize),
        ResponsiveHomeColumnsUltraWide => {
            ov_resp_columns(overrides).ultra_wide = Some(value as usize)
        }
        _ => return Err(format!("{} is not an integer field", field.label())),
    }
    Ok(())
}

/// Apply a Choice edit (enum pick) to the stored layout overrides. `value`
/// is the selected option string (the TOML spelling); unknown names are
/// rejected so the merge never sees an invalid enum.
pub fn apply_layout_choice(
    overrides: &mut LayoutOverrides,
    field: LayoutField,
    value: &str,
) -> Result<(), String> {
    use LayoutField::*;
    if !matches!(field.kind(), FieldKind::Choice) {
        return Err(format!("{} is not a choice field", field.label()));
    }
    match field {
        HomeMode => {
            home_mode(overrides, value, field)?;
        }
        ComponentThumbnailPosition => {
            ov_component(overrides).thumbnail_position = Some(parse_choice(value, field)?);
        }
        ComponentMetadataAlignment => {
            ov_component(overrides).metadata_alignment = Some(parse_choice(value, field)?);
        }
        ComponentButtonPlacement => {
            ov_component(overrides).button_placement = Some(parse_choice(value, field)?);
        }
        ComponentCardOrientation => {
            ov_component(overrides).card_orientation = Some(parse_choice(value, field)?);
        }
        ComponentVideoCardThumbnailPosition => {
            ov_video_card(overrides).thumbnail_position = Some(parse_choice(value, field)?);
        }
        ComponentVideoCardMetadataAlignment => {
            ov_video_card(overrides).metadata_alignment = Some(parse_choice(value, field)?);
        }
        ComponentVideoCardButtonPlacement => {
            ov_video_card(overrides).button_placement = Some(parse_choice(value, field)?);
        }
        ComponentVideoCardCardOrientation => {
            ov_video_card(overrides).card_orientation = Some(parse_choice(value, field)?);
        }
        ComponentSharedByMeThumbnailPosition => {
            ov_shared_by_me(overrides).thumbnail_position = Some(parse_choice(value, field)?);
        }
        ComponentSharedByMeMetadataAlignment => {
            ov_shared_by_me(overrides).metadata_alignment = Some(parse_choice(value, field)?);
        }
        ComponentSharedByMeButtonPlacement => {
            ov_shared_by_me(overrides).button_placement = Some(parse_choice(value, field)?);
        }
        ComponentSharedByMeCardOrientation => {
            ov_shared_by_me(overrides).card_orientation = Some(parse_choice(value, field)?);
        }
        _ => return Err(format!("{} is not a choice field", field.label())),
    }
    Ok(())
}

fn home_mode(
    overrides: &mut LayoutOverrides,
    value: &str,
    field: LayoutField,
) -> Result<(), String> {
    let mode = parse_choice::<HomeLayoutMode>(value, field)?;
    overrides.home.get_or_insert_with(Default::default).mode = Some(mode);
    Ok(())
}

fn parse_choice<T>(value: &str, field: LayoutField) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    parse_enum_variant::<T>(value)
        .ok_or_else(|| format!("{}: unknown option {value:?}", field.label()))
}

// ── Override group get-or-create helpers ─────────────────────────────
//
// Module-level function items (NOT closures): a closure `|o| o.home.
// get_or_insert_with(...)` fails lifetime elision (the input reference's
// lifetime is not tied to the output), while a function item elides
// correctly.

fn ov_home(o: &mut LayoutOverrides) -> &mut crate::layout::HomeOverrides {
    o.home.get_or_insert_with(Default::default)
}
fn ov_home_grid(o: &mut LayoutOverrides) -> &mut crate::layout::HomeGridOverrides {
    ov_home(o).grid.get_or_insert_with(Default::default)
}
fn ov_home_quick(o: &mut LayoutOverrides) -> &mut crate::layout::QuickActionsOverrides {
    ov_home(o)
        .quick_actions
        .get_or_insert_with(Default::default)
}
fn ov_home_padding(o: &mut LayoutOverrides) -> &mut crate::layout::HomePaddingOverrides {
    ov_home(o).padding.get_or_insert_with(Default::default)
}
fn ov_home_gaps(o: &mut LayoutOverrides) -> &mut crate::layout::HomeGapsOverrides {
    ov_home(o).gaps.get_or_insert_with(Default::default)
}
fn ov_home_card(o: &mut LayoutOverrides) -> &mut crate::layout::HomeCardSizingOverrides {
    ov_home(o).card_sizing.get_or_insert_with(Default::default)
}
fn ov_component(o: &mut LayoutOverrides) -> &mut crate::layout::ComponentOverrides {
    o.component.get_or_insert_with(Default::default)
}
fn ov_video_card(o: &mut LayoutOverrides) -> &mut crate::layout::ComponentPlacementOverrides {
    ov_component(o)
        .video_card
        .get_or_insert_with(Default::default)
}
fn ov_shared_by_me(o: &mut LayoutOverrides) -> &mut crate::layout::ComponentPlacementOverrides {
    ov_component(o)
        .shared_by_me
        .get_or_insert_with(Default::default)
}
fn ov_video(o: &mut LayoutOverrides) -> &mut crate::layout::VideoCardOverrides {
    ov_component(o).video.get_or_insert_with(Default::default)
}
fn ov_responsive(o: &mut LayoutOverrides) -> &mut crate::layout::ResponsiveOverrides {
    o.responsive.get_or_insert_with(Default::default)
}
fn ov_resp_columns(o: &mut LayoutOverrides) -> &mut crate::layout::ByTierOverrides<usize> {
    ov_responsive(o)
        .home_columns
        .get_or_insert_with(Default::default)
}
fn ov_resp_padding_x(o: &mut LayoutOverrides) -> &mut crate::layout::ByTierOverrides<f32> {
    ov_responsive(o)
        .home_padding_x
        .get_or_insert_with(Default::default)
}

/// Apply a comma-separated home section list edit to the stored layout
/// overrides (section order / hidden sections). Unknown names are rejected;
/// an empty list clears the list (e.g. no hidden sections).
pub fn apply_layout_sections(
    overrides: &mut LayoutOverrides,
    field: LayoutField,
    value: &str,
) -> Result<(), String> {
    use LayoutField::*;
    if !matches!(field.kind(), FieldKind::Sections) {
        return Err(format!("{} is not a section list field", field.label()));
    }
    let sections = parse_home_sections(value)?;
    let home = overrides.home.get_or_insert_with(Default::default);
    match field {
        HomeSectionOrder => home.section_order = Some(sections),
        HomeHiddenSections => home.hidden_sections = Some(sections),
        _ => return Err(format!("{} is not a section list field", field.label())),
    }
    Ok(())
}

// ── Panel structure (BORU-LAYOUT-08) ─────────────────────────────────

/// One sub-group of layout fields inside a layout section.
pub struct LayoutFieldGroup {
    pub label: &'static str,
    pub fields: &'static [LayoutField],
}

/// One top-level layout section of the inspector.
pub struct LayoutInspectorSection {
    pub id: LayoutSectionId,
    pub label: &'static str,
    /// Ordered sub-groups. A section with a single sub-group renders its
    /// fields directly under the section header (no duplicate header).
    pub groups: &'static [LayoutFieldGroup],
}

/// Ordered layout sections rendered after the theme sections (BORU-LAYOUT-08).
///
/// The hierarchy mirrors the layout model groups that are actually wired
/// into views: Home (PDF Task 3), Component (PDF Task 5) and Responsive
/// (PDF Task 4). Sub-group labels use the actual field names from
/// `layout.rs` — only values that exist in the typed model are exposed,
/// nothing derived.
pub const LAYOUT_SECTIONS: &[LayoutInspectorSection] = &[
    LayoutInspectorSection {
        id: LayoutSectionId::Home,
        label: "Layout · Home",
        groups: &[
            LayoutFieldGroup {
                label: "Sections",
                fields: &[
                    LayoutField::HomeSectionOrder,
                    LayoutField::HomeHiddenSections,
                ],
            },
            LayoutFieldGroup {
                label: "Mode",
                fields: &[LayoutField::HomeMode],
            },
            LayoutFieldGroup {
                label: "Grid",
                fields: &[
                    LayoutField::HomeGridMainPortion,
                    LayoutField::HomeGridRailPortion,
                    LayoutField::HomeGridColumnGap,
                    LayoutField::HomeGridStackBreakpoint,
                ],
            },
            LayoutFieldGroup {
                label: "Quick actions",
                fields: &[
                    LayoutField::HomeQuickColumnsWide,
                    LayoutField::HomeQuickColumnsMid,
                    LayoutField::HomeQuickColumnsNarrow,
                    LayoutField::HomeQuickFourColBreakpoint,
                    LayoutField::HomeQuickTwoColBreakpoint,
                    LayoutField::HomeQuickCardPaddingY,
                    LayoutField::HomeQuickCardPaddingX,
                ],
            },
            LayoutFieldGroup {
                label: "Canvas",
                fields: &[LayoutField::HomeMaxContentWidth],
            },
            LayoutFieldGroup {
                label: "Padding",
                fields: &[
                    LayoutField::HomePaddingTop,
                    LayoutField::HomePaddingBottom,
                    LayoutField::HomePaddingHorizontalLarge,
                    LayoutField::HomePaddingHorizontalDefault,
                ],
            },
            LayoutFieldGroup {
                label: "Gaps",
                fields: &[
                    LayoutField::HomeGapsCardGap,
                    LayoutField::HomeGapsHeroGap,
                    LayoutField::HomeGapsHeaderDashboardGap,
                    LayoutField::HomeGapsFooterGap,
                    LayoutField::HomeGapsCompactHeaderStackGap,
                ],
            },
            LayoutFieldGroup {
                label: "Card sizing",
                fields: &[
                    LayoutField::HomeCardSizingPeersBodyMin,
                    LayoutField::HomeCardSizingActivityRowHeight,
                    LayoutField::HomeCardSizingQuickActionIconSize,
                    LayoutField::HomeCardSizingStatusCardMinContentHeight,
                    LayoutField::HomeCardSizingStatusCardMediumContent,
                    LayoutField::HomeCardSizingStatusCardNarrowContent,
                    LayoutField::HomeCardSizingStatusCardMeshHideContent,
                    LayoutField::HomeCardSizingStatusCardTextMinWidth,
                    LayoutField::HomeCardSizingStatusCardTextMinWidthMedium,
                    LayoutField::HomeCardSizingStatusCardMeshMaxWidth,
                    LayoutField::HomeCardSizingStatusCardPaddingX,
                    LayoutField::HomeCardSizingStatusIconTextGapFull,
                    LayoutField::HomeCardSizingStatusIconTextGapMedium,
                    LayoutField::HomeCardSizingStatusTextGraphGapFull,
                    LayoutField::HomeCardSizingStatusTextGraphGapMedium,
                    LayoutField::HomeCardSizingStatusDividerWidth,
                    LayoutField::HomeCardSizingStatusDividerHeight,
                ],
            },
        ],
    },
    LayoutInspectorSection {
        id: LayoutSectionId::Component,
        label: "Layout · Component",
        groups: &[
            LayoutFieldGroup {
                label: "Global placement",
                fields: &[
                    LayoutField::ComponentThumbnailPosition,
                    LayoutField::ComponentMetadataAlignment,
                    LayoutField::ComponentButtonPlacement,
                    LayoutField::ComponentCardOrientation,
                ],
            },
            LayoutFieldGroup {
                label: "Video card placement",
                fields: &[
                    LayoutField::ComponentVideoCardThumbnailPosition,
                    LayoutField::ComponentVideoCardMetadataAlignment,
                    LayoutField::ComponentVideoCardButtonPlacement,
                    LayoutField::ComponentVideoCardCardOrientation,
                ],
            },
            LayoutFieldGroup {
                label: "Shared-by-me placement",
                fields: &[
                    LayoutField::ComponentSharedByMeThumbnailPosition,
                    LayoutField::ComponentSharedByMeMetadataAlignment,
                    LayoutField::ComponentSharedByMeButtonPlacement,
                    LayoutField::ComponentSharedByMeCardOrientation,
                ],
            },
            LayoutFieldGroup {
                label: "Video card sizing",
                fields: &[
                    LayoutField::ComponentVideoNarrowBreakpoint,
                    LayoutField::ComponentVideoMediumBreakpoint,
                    LayoutField::ComponentVideoPlayOverlaySize,
                    LayoutField::ComponentVideoHeaderFilenameMaxWidth,
                    LayoutField::ComponentVideoControlsSliderWidth,
                ],
            },
        ],
    },
    LayoutInspectorSection {
        id: LayoutSectionId::Responsive,
        label: "Layout · Responsive",
        groups: &[
            LayoutFieldGroup {
                label: "Viewport",
                fields: &[
                    LayoutField::ResponsiveViewportRefWidth,
                    LayoutField::ResponsiveViewportRefHeight,
                    LayoutField::ResponsiveViewportMinWidth,
                    LayoutField::ResponsiveViewportMinHeight,
                    LayoutField::ResponsiveViewportLgWidth,
                    LayoutField::ResponsiveViewportLgHeight,
                    LayoutField::ResponsiveViewportXlWidth,
                    LayoutField::ResponsiveViewportXlHeight,
                ],
            },
            LayoutFieldGroup {
                label: "Content breakpoints",
                fields: &[
                    LayoutField::ResponsiveContentMaxWidth,
                    LayoutField::ResponsiveHomeIllustrationFullContent,
                    LayoutField::ResponsiveHomeIllustrationHideContent,
                    LayoutField::ResponsiveHomeCompactHeaderContent,
                ],
            },
            LayoutFieldGroup {
                label: "Tier thresholds",
                fields: &[
                    LayoutField::ResponsiveNarrowMaxWidth,
                    LayoutField::ResponsiveUltraWideMinWidth,
                ],
            },
            LayoutFieldGroup {
                label: "Home columns",
                fields: &[
                    LayoutField::ResponsiveHomeColumnsNarrow,
                    LayoutField::ResponsiveHomeColumnsDesktop,
                    LayoutField::ResponsiveHomeColumnsUltraWide,
                ],
            },
            LayoutFieldGroup {
                label: "Home horizontal padding",
                fields: &[
                    LayoutField::ResponsiveHomePaddingXNarrow,
                    LayoutField::ResponsiveHomePaddingXDesktop,
                    LayoutField::ResponsiveHomePaddingXUltraWide,
                ],
            },
        ],
    },
];

/// The number of layout fields exposed in the panel (used by the
/// "every field maps to a real config leaf" regression test).
///
/// A plain literal because `Iterator` methods are not const-stable; the
/// test below (`every_exposed_layout_field_maps_to_a_real_config_leaf`)
/// asserts the literal matches the actual `LAYOUT_SECTIONS` contents.
#[cfg(test)]
pub const LAYOUT_FIELD_COUNT: usize = 78;

// ── Save / reload status (BORU-LAYOUT-08) ────────────────────────────

/// Result of the last Save Layout action, shown as the panel's layout
/// save-status line. View-local display state only — never part of the
/// layout model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutSaveStatus {
    /// No save has been attempted yet this session.
    None,
    /// The last Save Layout action wrote `boru-layout.toml` successfully.
    Saved,
    /// The last Save Layout action failed; the message is shown in the panel.
    Failed(String),
}

impl Default for LayoutSaveStatus {
    fn default() -> Self {
        Self::None
    }
}

/// Result of the last "Reload Layout From Disk" action, shown as the
/// panel's layout reload-status line. View-local display state only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutReloadStatus {
    /// No reload has been attempted yet this session.
    None,
    /// The last "Reload Layout From Disk" action reloaded `boru-layout.toml`.
    Reloaded,
    /// The last "Reload Layout From Disk" action failed; the message (path +
    /// parser detail) is shown in the panel.
    Failed(String),
}

impl Default for LayoutReloadStatus {
    fn default() -> Self {
        Self::None
    }
}

// ── View ─────────────────────────────────────────────────────────────

/// Render one layout field row (dispatches by [`FieldKind`]).
pub fn layout_field_row(
    layout: &LayoutConfig,
    draft: &InspectorDraft,
    field: LayoutField,
    dark_mode: bool,
) -> Element<'static, crate::app::AppMessage> {
    match field.kind() {
        FieldKind::Float => layout_float_row(layout, draft, field, dark_mode),
        FieldKind::Int => layout_int_row(layout, draft, field, dark_mode),
        FieldKind::Choice => layout_choice_row(layout, draft, field, dark_mode),
        FieldKind::Sections => layout_sections_row(layout, draft, field, dark_mode),
        // Bool / Color are theme-only kinds; layout fields never use them.
        _ => Space::new().height(Length::Fixed(0.0)).into(),
    }
}

fn layout_float_row(
    layout: &LayoutConfig,
    draft: &InspectorDraft,
    field: LayoutField,
    dark_mode: bool,
) -> Element<'static, crate::app::AppMessage> {
    let current = read_layout_float(layout, field);
    let (min, max) = field.range();
    let text_value = draft
        .layout_float_text
        .get(&field)
        .cloned()
        .unwrap_or_else(|| format!("{current:.1}"));

    let label = text(field.label()).size(11.0).color(muted_text(dark_mode));
    let value = text(format!("{current:.1}"))
        .size(11.0)
        .color(value_text(dark_mode));

    let slider = slider(min..=max, current.clamp(min, max), move |v| {
        crate::app::AppMessage::Inspector(InspectorMsg::SetLayoutFloat { field, value: v })
    })
    .width(Length::Fill);

    let input = text_input("value", &text_value)
        .width(Length::Fixed(64.0))
        .padding([2, 6])
        .size(11.0)
        .on_input(move |s| {
            crate::app::AppMessage::Inspector(InspectorMsg::LayoutFloatTextChanged {
                field,
                text: s,
            })
        });

    iced::widget::Column::new()
        .push(row![label, Space::new().width(Length::Fill), value].align_y(Alignment::Center))
        .push(
            row![slider, Space::new().width(Length::Fixed(6.0)), input].align_y(Alignment::Center),
        )
        .spacing(2.0)
        .into()
}

fn layout_int_row(
    layout: &LayoutConfig,
    draft: &InspectorDraft,
    field: LayoutField,
    dark_mode: bool,
) -> Element<'static, crate::app::AppMessage> {
    let current = read_layout_int(layout, field);
    let (min, max) = field.range();
    let text_value = draft
        .layout_int_text
        .get(&field)
        .cloned()
        .unwrap_or_else(|| current.to_string());

    let label = text(field.label()).size(11.0).color(muted_text(dark_mode));
    let value = text(current.to_string())
        .size(11.0)
        .color(value_text(dark_mode));

    let slider = slider(min..=max, (current as f32).clamp(min, max), move |v| {
        crate::app::AppMessage::Inspector(InspectorMsg::SetLayoutInt {
            field,
            value: v.round() as i64,
        })
    })
    .width(Length::Fill);

    let input = text_input("value", &text_value)
        .width(Length::Fixed(64.0))
        .padding([2, 6])
        .size(11.0)
        .on_input(move |s| {
            crate::app::AppMessage::Inspector(InspectorMsg::LayoutIntTextChanged { field, text: s })
        });

    iced::widget::Column::new()
        .push(row![label, Space::new().width(Length::Fill), value].align_y(Alignment::Center))
        .push(
            row![slider, Space::new().width(Length::Fixed(6.0)), input].align_y(Alignment::Center),
        )
        .spacing(2.0)
        .into()
}

fn layout_choice_row(
    layout: &LayoutConfig,
    _draft: &InspectorDraft,
    field: LayoutField,
    dark_mode: bool,
) -> Element<'static, crate::app::AppMessage> {
    let options = field.choices().to_vec();
    let selected = read_layout_choice(layout, field);

    let label = text(field.label()).size(11.0).color(muted_text(dark_mode));

    let list = pick_list(options, Some(selected), move |choice: &str| {
        crate::app::AppMessage::Inspector(InspectorMsg::SetLayoutChoice {
            field,
            value: choice.to_string(),
        })
    })
    .width(Length::Fill)
    .padding([2, 6])
    .text_size(11.0);

    iced::widget::Column::new()
        .push(row![label, Space::new().width(Length::Fill)].align_y(Alignment::Center))
        .push(list)
        .spacing(2.0)
        .into()
}

fn layout_sections_row(
    layout: &LayoutConfig,
    draft: &InspectorDraft,
    field: LayoutField,
    dark_mode: bool,
) -> Element<'static, crate::app::AppMessage> {
    let current = read_layout_sections(layout, field);
    let text_value = draft
        .layout_sections_text
        .get(&field)
        .cloned()
        .unwrap_or(current);

    let label = text(field.label()).size(11.0).color(muted_text(dark_mode));
    let hint = text("comma-separated")
        .size(8.0)
        .color(muted_text(dark_mode));

    let input = text_input("e.g. Hero, MeshHealth, QuickActions", &text_value)
        .width(Length::Fill)
        .padding([2, 6])
        .size(11.0)
        .on_input(move |s| {
            crate::app::AppMessage::Inspector(InspectorMsg::LayoutSectionsTextChanged {
                field,
                text: s,
            })
        });

    iced::widget::Column::new()
        .push(row![label, Space::new().width(Length::Fill), hint].align_y(Alignment::Center))
        .push(input)
        .spacing(2.0)
        .into()
}

/// Collapsible layout section header with a per-section Reset action.
pub fn layout_section_header(
    section: &LayoutInspectorSection,
    collapsed: bool,
    dark_mode: bool,
) -> Element<'static, crate::app::AppMessage> {
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
                .color(if dark_mode {
                    Color::from_rgb(0.7, 0.85, 0.75)
                } else {
                    Color::from_rgb(0.1, 0.45, 0.28)
                }),
        ]
        .align_y(Alignment::Center),
    )
    .on_press(crate::app::AppMessage::Inspector(
        InspectorMsg::ToggleLayoutSection(section.id),
    ))
    .padding([3, 6])
    .style(button::text);

    let reset = button(text("Reset").size(9.0).color(if dark_mode {
        Color::from_rgb(0.75, 0.75, 0.78)
    } else {
        Color::from_rgb(0.4, 0.42, 0.4)
    }))
    .on_press(crate::app::AppMessage::Inspector(
        InspectorMsg::ResetLayoutSection(section.id),
    ))
    .padding([2, 6])
    .style(button::text);

    row![toggle, Space::new().width(Length::Fill), reset]
        .align_y(Alignment::Center)
        .into()
}

/// Heading row for the Layout half of the inspector panel: title + a
/// Reset All action that clears every layout override group.
pub fn layout_panel_heading(dark_mode: bool) -> Element<'static, crate::app::AppMessage> {
    let title = text("LAYOUT (boru-layout.toml)")
        .size(12.0)
        .color(if dark_mode {
            Color::from_rgb(0.75, 0.9, 0.8)
        } else {
            Color::from_rgb(0.08, 0.4, 0.25)
        });
    let reset = button(text("Reset All").size(9.0).color(if dark_mode {
        Color::from_rgb(0.75, 0.75, 0.78)
    } else {
        Color::from_rgb(0.4, 0.42, 0.4)
    }))
    .on_press(crate::app::AppMessage::Inspector(InspectorMsg::ResetLayoutAll))
    .padding([2, 6])
    .style(button::text);

    row![title, Space::new().width(Length::Fill), reset]
        .align_y(Alignment::Center)
        .into()
}

/// Row with the Save Layout action + status line (BORU-LAYOUT-08).
///
/// The button serializes the current editable layout overrides to
/// `boru-layout.toml` (atomic temp + rename) and the status line shows the
/// result of the last save inside the panel.
pub fn save_layout_row(
    dark_mode: bool,
    status: &LayoutSaveStatus,
) -> Element<'static, crate::app::AppMessage> {
    let save = button(text("Save Layout").size(11.0).color(if dark_mode {
        Color::from_rgb(0.85, 0.85, 0.85)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    }))
    .on_press(crate::app::AppMessage::Inspector(InspectorMsg::SaveLayout))
    .padding([3, 8]);

    let (msg, color) = match status {
        LayoutSaveStatus::None => (
            format!("saves current overrides to {LAYOUT_CONFIG_FILE_NAME}"),
            if dark_mode {
                Color::from_rgb(0.55, 0.55, 0.6)
            } else {
                Color::from_rgb(0.45, 0.45, 0.45)
            },
        ),
        LayoutSaveStatus::Saved => (
            "✓ saved".to_string(),
            if dark_mode {
                Color::from_rgb(0.6, 0.85, 0.65)
            } else {
                Color::from_rgb(0.1, 0.55, 0.25)
            },
        ),
        LayoutSaveStatus::Failed(e) => {
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

/// Row with the Reload Layout From Disk action + status line (BORU-LAYOUT-08).
///
/// The button discards unsaved inspector layout changes and reloads
/// `boru-layout.toml` from disk. A failed reload keeps the current layout
/// and reports the error (path + parser detail) in the panel.
pub fn reload_layout_row(
    dark_mode: bool,
    status: &LayoutReloadStatus,
) -> Element<'static, crate::app::AppMessage> {
    let reload = button(
        text("Reload Layout From Disk")
            .size(11.0)
            .color(if dark_mode {
                Color::from_rgb(0.85, 0.85, 0.85)
            } else {
                Color::from_rgb(0.15, 0.15, 0.15)
            }),
    )
    .on_press(crate::app::AppMessage::Inspector(
        InspectorMsg::ReloadLayoutFromDisk,
    ))
    .padding([3, 8]);

    let (msg, color) = match status {
        LayoutReloadStatus::None => (
            "discards unsaved changes; reloads boru-layout.toml".to_string(),
            if dark_mode {
                Color::from_rgb(0.55, 0.55, 0.6)
            } else {
                Color::from_rgb(0.45, 0.45, 0.45)
            },
        ),
        LayoutReloadStatus::Reloaded => (
            "✓ reloaded from disk".to_string(),
            if dark_mode {
                Color::from_rgb(0.6, 0.85, 0.65)
            } else {
                Color::from_rgb(0.1, 0.55, 0.25)
            },
        ),
        LayoutReloadStatus::Failed(e) => {
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

/// Compact list of the last layout merge's field-level adjustments
/// (BORU-LAYOUT-08, mirroring the theme merge-warnings row).
pub fn layout_merge_warnings_row(
    dark_mode: bool,
    warnings: &[String],
) -> Element<'static, crate::app::AppMessage> {
    if warnings.is_empty() {
        return Space::new().height(Length::Fixed(0.0)).into();
    }
    let heading = text(format!(
        "⚠ {} layout value(s) adjusted on load",
        warnings.len()
    ))
    .size(9.0)
    .color(if dark_mode {
        Color::from_rgb(0.95, 0.75, 0.4)
    } else {
        Color::from_rgb(0.7, 0.45, 0.0)
    });
    let mut col = iced::widget::Column::new().push(heading).spacing(1.0);
    for w in warnings.iter().take(4) {
        let preview: String = w.chars().take(90).collect();
        col = col.push(text(format!("· {preview}")).size(8.0).color(if dark_mode {
            Color::from_rgb(0.8, 0.65, 0.45)
        } else {
            Color::from_rgb(0.45, 0.3, 0.1)
        }));
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

// ── Tests: message → layout-edit mapping ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{
        ButtonPlacement, CardOrientation, MetadataAlignment, ThumbnailPosition,
    };
    use crate::layout_merge::merge_layout_config;

    fn merged(overrides: &LayoutOverrides) -> LayoutConfig {
        merge_layout_config(&LayoutConfig::default(), overrides).0
    }

    #[test]
    fn parse_enum_variant_round_trips_layout_enums() {
        // The parse helper must accept every enum's TOML spelling (this is
        // what the pick_list choices are built from).
        assert_eq!(
            parse_enum_variant::<HomeLayoutMode>("List"),
            Some(HomeLayoutMode::List)
        );
        assert_eq!(
            parse_enum_variant::<HomeSection>("MeshHealth"),
            Some(HomeSection::MeshHealth)
        );
        assert_eq!(
            parse_enum_variant::<ThumbnailPosition>("Bottom"),
            Some(ThumbnailPosition::Bottom)
        );
        assert_eq!(
            parse_enum_variant::<MetadataAlignment>("Center"),
            Some(MetadataAlignment::Center)
        );
        assert_eq!(
            parse_enum_variant::<ButtonPlacement>("Overlay"),
            Some(ButtonPlacement::Overlay)
        );
        assert_eq!(
            parse_enum_variant::<CardOrientation>("Vertical"),
            Some(CardOrientation::Vertical)
        );
        assert!(parse_enum_variant::<HomeSection>("Bogus").is_none());
    }

    #[test]
    fn apply_layout_float_sets_exact_config_leaf() {
        let mut o = LayoutOverrides::default();
        apply_layout_float(&mut o, LayoutField::HomeMaxContentWidth, 1200.0).unwrap();
        let layout = merged(&o);
        assert_eq!(layout.home.max_content_width, 1200.0);
        // Unrelated leaves stay at defaults.
        assert_eq!(
            layout.home.padding.top,
            LayoutConfig::default().home.padding.top
        );
        assert_eq!(
            layout.responsive.narrow_max_width,
            LayoutConfig::default().responsive.narrow_max_width
        );
    }

    #[test]
    fn apply_layout_float_rejects_non_float_field() {
        let mut o = LayoutOverrides::default();
        let err = apply_layout_float(&mut o, LayoutField::HomeMode, 1.0).unwrap_err();
        assert!(err.contains("not a float"), "{err}");
        let err = apply_layout_float(&mut o, LayoutField::HomeGridMainPortion, 1.0).unwrap_err();
        assert!(err.contains("not a float"), "{err}");
    }

    #[test]
    fn apply_layout_int_sets_columns_and_portions() {
        let mut o = LayoutOverrides::default();
        apply_layout_int(&mut o, LayoutField::HomeGridMainPortion, 3).unwrap();
        apply_layout_int(&mut o, LayoutField::HomeQuickColumnsWide, 5).unwrap();
        apply_layout_int(&mut o, LayoutField::ResponsiveHomeColumnsUltraWide, 4).unwrap();
        let layout = merged(&o);
        assert_eq!(layout.home.grid.main_portion, 3);
        assert_eq!(layout.home.quick_actions.columns_wide, 5);
        assert_eq!(layout.responsive.home_columns.ultra_wide, 4);
        // The other tiers keep their defaults.
        assert_eq!(layout.responsive.home_columns.desktop, 2);
        assert_eq!(layout.home.grid.rail_portion, 1);
    }

    #[test]
    fn apply_layout_int_rejects_non_int_field() {
        let mut o = LayoutOverrides::default();
        let err = apply_layout_int(&mut o, LayoutField::HomeMaxContentWidth, 3).unwrap_err();
        assert!(err.contains("not an integer"), "{err}");
    }

    #[test]
    fn apply_layout_choice_sets_enum_leaves() {
        let mut o = LayoutOverrides::default();
        apply_layout_choice(&mut o, LayoutField::HomeMode, "List").unwrap();
        apply_layout_choice(
            &mut o,
            LayoutField::ComponentVideoCardThumbnailPosition,
            "Bottom",
        )
        .unwrap();
        apply_layout_choice(
            &mut o,
            LayoutField::ComponentSharedByMeButtonPlacement,
            "Overlay",
        )
        .unwrap();
        let layout = merged(&o);
        assert_eq!(layout.home.mode, HomeLayoutMode::List);
        assert_eq!(
            layout.component.video_card.thumbnail_position,
            ThumbnailPosition::Bottom
        );
        assert_eq!(
            layout.component.shared_by_me.button_placement,
            ButtonPlacement::Overlay
        );
        // The global fallback leaves stay at defaults.
        assert_eq!(layout.component.thumbnail_position, ThumbnailPosition::Left);
    }

    #[test]
    fn apply_layout_choice_rejects_unknown_and_wrong_kind() {
        let mut o = LayoutOverrides::default();
        let err = apply_layout_choice(&mut o, LayoutField::HomeMode, "Bogus").unwrap_err();
        assert!(err.contains("unknown option"), "{err}");
        let err =
            apply_layout_choice(&mut o, LayoutField::HomeMaxContentWidth, "List").unwrap_err();
        assert!(err.contains("not a choice"), "{err}");
    }

    #[test]
    fn apply_layout_sections_parses_order_and_hidden() {
        let mut o = LayoutOverrides::default();
        apply_layout_sections(
            &mut o,
            LayoutField::HomeSectionOrder,
            "Tunnels, Hero, PeopleActivity",
        )
        .unwrap();
        apply_layout_sections(&mut o, LayoutField::HomeHiddenSections, "QuickActions").unwrap();
        let layout = merged(&o);
        assert_eq!(
            layout.home.section_order,
            vec![
                HomeSection::Tunnels,
                HomeSection::Hero,
                HomeSection::PeopleActivity,
            ]
        );
        assert_eq!(layout.home.hidden_sections, vec![HomeSection::QuickActions]);
        // The visible order drops the hidden section and keeps the rest.
        assert_eq!(
            layout.home.visible_sections(),
            vec![
                HomeSection::Tunnels,
                HomeSection::Hero,
                HomeSection::PeopleActivity
            ]
        );
    }

    #[test]
    fn apply_layout_sections_empty_clears_hidden() {
        let mut o = LayoutOverrides::default();
        o.home.get_or_insert_with(Default::default).hidden_sections = Some(vec![HomeSection::Hero]);
        apply_layout_sections(&mut o, LayoutField::HomeHiddenSections, "").unwrap();
        let layout = merged(&o);
        assert!(layout.home.hidden_sections.is_empty());
    }

    #[test]
    fn apply_layout_sections_rejects_unknown_name() {
        let mut o = LayoutOverrides::default();
        let err = apply_layout_sections(&mut o, LayoutField::HomeSectionOrder, "Hero, Bogus")
            .unwrap_err();
        assert!(err.contains("unknown home section"), "{err}");
        // Nothing was applied — the previous value is retained.
        assert!(o.home.is_none());
    }

    #[test]
    fn apply_layout_sections_rejects_wrong_kind() {
        let mut o = LayoutOverrides::default();
        let err = apply_layout_sections(&mut o, LayoutField::HomeMode, "List").unwrap_err();
        assert!(err.contains("not a section list"), "{err}");
    }

    #[test]
    fn reads_match_writes_for_each_kind() {
        let mut o = LayoutOverrides::default();
        apply_layout_float(&mut o, LayoutField::HomeMaxContentWidth, 1330.0).unwrap();
        apply_layout_int(&mut o, LayoutField::HomeGridRailPortion, 2).unwrap();
        apply_layout_choice(&mut o, LayoutField::ComponentCardOrientation, "Vertical").unwrap();
        apply_layout_sections(&mut o, LayoutField::HomeSectionOrder, "MeshHealth, Tunnels")
            .unwrap();
        let layout = merged(&o);
        assert_eq!(
            read_layout_float(&layout, LayoutField::HomeMaxContentWidth),
            1330.0
        );
        assert_eq!(
            read_layout_int(&layout, LayoutField::HomeGridRailPortion),
            2
        );
        assert_eq!(
            read_layout_choice(&layout, LayoutField::ComponentCardOrientation),
            "Vertical"
        );
        assert_eq!(
            read_layout_sections(&layout, LayoutField::HomeSectionOrder),
            "MeshHealth, Tunnels"
        );
    }

    #[test]
    fn every_exposed_layout_field_maps_to_a_real_config_leaf() {
        // Regression guard: a LayoutField added to the panel but missing
        // from apply_* would silently no-op and fail this test. Every field
        // must apply without error AND change the merged layout away from
        // the default (the value chosen is deliberately different from the
        // baseline for each kind).
        assert_eq!(LAYOUT_FIELD_COUNT, 78, "panel field count drift");
        for section in LAYOUT_SECTIONS {
            for group in section.groups {
                for field in group.fields {
                    let mut o = LayoutOverrides::default();
                    match field.kind() {
                        FieldKind::Float => {
                            let (min, max) = field.range();
                            let v = (min + max) / 2.0;
                            apply_layout_float(&mut o, *field, v)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Int => {
                            let (min, _max) = field.range();
                            let v = (min + 2.0).round() as i64;
                            apply_layout_int(&mut o, *field, v)
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        FieldKind::Choice => {
                            // Pick the FIRST choice that actually changes
                            // the merged layout away from defaults. Some
                            // fields' first choice equals their default
                            // (e.g. HomeMode::Grid), so a blind "first"
                            // would make the assert_ne below fail.
                            let mut applied = false;
                            for choice in field.choices() {
                                let mut candidate = LayoutOverrides::default();
                                if apply_layout_choice(&mut candidate, *field, choice).is_err() {
                                    continue;
                                }
                                if merged(&candidate) != LayoutConfig::default() {
                                    o = candidate;
                                    applied = true;
                                    break;
                                }
                            }
                            assert!(
                                applied,
                                "{field:?}: no choice differs from the default layout"
                            );
                        }
                        FieldKind::Sections => {
                            apply_layout_sections(&mut o, *field, "Tunnels, Hero")
                                .unwrap_or_else(|e| panic!("{field:?}: {e}"));
                        }
                        _ => panic!("{field:?} has an unexpected kind"),
                    }
                    let layout = merged(&o);
                    assert_ne!(
                        layout,
                        LayoutConfig::default(),
                        "{field:?} must actually edit the layout (test fixture broken)"
                    );
                }
            }
        }
    }

    #[test]
    fn reset_layout_section_clears_only_that_group() {
        let mut o = LayoutOverrides::default();
        apply_layout_float(&mut o, LayoutField::HomeMaxContentWidth, 1200.0).unwrap();
        apply_layout_choice(&mut o, LayoutField::ComponentCardOrientation, "Vertical").unwrap();
        apply_layout_float(&mut o, LayoutField::ResponsiveNarrowMaxWidth, 400.0).unwrap();

        LayoutSectionId::Home.reset(&mut o);

        assert!(o.home.is_none(), "home group cleared");
        // Other groups keep their edits.
        let layout = merged(&o);
        assert_eq!(layout.component.card_orientation, CardOrientation::Vertical);
        assert_eq!(layout.responsive.narrow_max_width, 400.0);
        assert_eq!(
            layout.home.max_content_width,
            LayoutConfig::default().home.max_content_width
        );
    }

    #[test]
    fn reset_all_layout_clears_every_group() {
        let mut o = LayoutOverrides::default();
        apply_layout_float(&mut o, LayoutField::HomeMaxContentWidth, 1200.0).unwrap();
        apply_layout_choice(&mut o, LayoutField::ComponentCardOrientation, "Vertical").unwrap();
        apply_layout_float(&mut o, LayoutField::ResponsiveNarrowMaxWidth, 400.0).unwrap();

        for section in LAYOUT_SECTIONS {
            section.id.reset(&mut o);
        }
        assert_eq!(o, LayoutOverrides::default());
        assert_eq!(merged(&o), LayoutConfig::default());
    }

    #[test]
    fn layout_section_membership_matches_field_section_mapping() {
        // Every field rendered under a layout section must claim that
        // section via LayoutField::section(), so the hierarchy and the
        // pure mapping never drift apart.
        for section in LAYOUT_SECTIONS {
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
    fn every_choice_field_has_options_and_reads_one_of_them() {
        // A Choice field rendered as a pick_list must offer every possible
        // value AND the merged layout must always produce one of them.
        let layout = LayoutConfig::default();
        for section in LAYOUT_SECTIONS {
            for group in section.groups {
                for field in group.fields {
                    if field.kind() != FieldKind::Choice {
                        continue;
                    }
                    let options = field.choices();
                    assert!(!options.is_empty(), "{field:?} has no options");
                    let current = read_layout_choice(&layout, *field);
                    assert!(
                        options.contains(&current),
                        "{field:?} current value {current:?} not in options"
                    );
                }
            }
        }
    }
}
