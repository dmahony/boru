//! Developer component gallery / UI playground — dev-ui only (PDF Task 14).
//!
//! Shows every primitive from `ui_components` in every applicable state,
//! plus representative states for chat messages, attachment cards and video
//! cards rendered through the EXACT production components (never duplicated
//! mocks). Accessible only via `Screen::Gallery` (Ctrl+Shift+G or the
//! "Component Gallery" button in the dev UI Inspector) in dev-ui builds.
//!
//! PDF Task 15 (BORU-UI-15): a responsive-preview control bar sits above the
//! gallery content. It can simulate a narrow panel / small-window column, a
//! typical desktop column, a maximized / wide-screen column, or an arbitrary
//! width via the custom slider. The whole gallery content below the bar is
//! re-laid out inside a fixed-width frame (container width constraint), so
//! every production component shown here can be inspected at every size.

use iced::widget::{container, rule::horizontal, slider, text, Column, Row, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::app::{
    DownloadAttachment, DownloadFailure, DownloadState, TransferKind, AppMessage,
};
use crate::boru_dialog::BoruDialog;
use crate::card_shell::{CardShell, CARD_ROW_HEIGHT};
use crate::design_tokens;
use crate::download_progress_view::view_download_progress;
use crate::fonts::TypeRole;
use crate::icon_system::{Icon, IconSize};
use crate::layout::{
    self, ButtonPlacement, ByTier, ByTierOverrides, CardOrientation, ComponentOverrides,
    ComponentPlacement, ComponentPlacementOverrides, HomeGapsOverrides, HomeGridOverrides,
    HomeLayoutMode, HomeOverrides, HomePaddingOverrides, HomeSection, LayoutConfig,
    LayoutOverrides, MetadataAlignment, ResponsiveOverrides, ThumbnailPosition,
};
use crate::ui_components::{
    self, badge, card_header, date_separator, divider, elevated_card, empty_state,
    ghost_icon_button, icon_tile, primary_button, primary_button_icon, secondary_button,
    section_header, status_dot, system_event_chip, text_input_field, Avatar, BadgeKind, Card,
    FileIdentityCell, InlineError, ListRow, LoadingSkeleton, MetricBlock, OverflowMenu,
    PeerChipStack, ProgressBar, ProgressKind, StatusDotKind, TabStrip, TableHeaderRow,
};

/// Minimum width the custom-width slider accepts.
pub const CUSTOM_WIDTH_MIN: f32 = 240.0;
/// Maximum width the custom-width slider accepts.
pub const CUSTOM_WIDTH_MAX: f32 = 1920.0;

/// Preset content widths the gallery can simulate (PDF Task 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GalleryWidthPreset {
    /// Narrow panel / small-window content column.
    Narrow,
    /// Typical desktop window content column.
    Desktop,
    /// Maximized / wide-screen content column.
    Maximized,
    /// Width driven by the custom slider.
    Custom,
}

impl GalleryWidthPreset {
    /// The simulated content-column width in pixels for this preset.
    pub const fn width(self) -> f32 {
        match self {
            GalleryWidthPreset::Narrow => 360.0,
            GalleryWidthPreset::Desktop => 960.0,
            GalleryWidthPreset::Maximized => 1440.0,
            GalleryWidthPreset::Custom => CUSTOM_WIDTH_MIN + (CUSTOM_WIDTH_MAX - CUSTOM_WIDTH_MIN) / 2.0,
        }
    }

    /// Short label shown on the preset button.
    pub const fn label(self) -> &'static str {
        match self {
            GalleryWidthPreset::Narrow => "Narrow",
            GalleryWidthPreset::Desktop => "Desktop",
            GalleryWidthPreset::Maximized => "Maximized",
            GalleryWidthPreset::Custom => "Custom",
        }
    }
}

/// Layout-config presets the gallery preview can apply (BORU-LAYOUT-09 /
/// PDF Task 9).
///
/// Each preset is a concrete [`LayoutConfig`] produced by merging preset
/// [`LayoutOverrides`] onto [`LayoutConfig::default()`] — the same pure
/// merge path `boru-layout.toml` uses (BORU-LAYOUT-06) — so the preview
/// shows exactly what each configuration would do to the real home screen
/// and to layout-aware components (download/video cards). The `Default`
/// preset is the empty override set, i.e. `LayoutConfig::default()` and
/// the current appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GalleryLayoutPreset {
    /// Default layout — `LayoutConfig::default()` (current appearance).
    Default,
    /// Narrow-friendly: single column at every width, list mode, tight
    /// gaps/padding, the Tunnels section hidden, compact video card.
    Narrow,
    /// Desktop reference: two-column grid, standard gaps, a wider main
    /// column, and a side-placement video card (thumbnail left,
    /// horizontal orientation).
    Desktop,
    /// Maximized / wide canvas: three-column grid at ultra-wide widths,
    /// generous gaps and responsive padding, larger max content width.
    Maximized,
}

impl GalleryLayoutPreset {
    /// All presets in display order.
    pub const ALL: [GalleryLayoutPreset; 4] = [
        GalleryLayoutPreset::Default,
        GalleryLayoutPreset::Narrow,
        GalleryLayoutPreset::Desktop,
        GalleryLayoutPreset::Maximized,
    ];

    /// Short label shown on the preset button.
    pub const fn label(self) -> &'static str {
        match self {
            GalleryLayoutPreset::Default => "Default",
            GalleryLayoutPreset::Narrow => "Narrow",
            GalleryLayoutPreset::Desktop => "Desktop",
            GalleryLayoutPreset::Maximized => "Maximized",
        }
    }

    /// The concrete [`LayoutConfig`] this preset previews, derived from
    /// preset overrides merged onto [`LayoutConfig::default()`]. Building
    /// it is pure struct construction (no I/O); the gallery re-derives it
    /// every frame so a preset change applies immediately.
    pub fn layout_config(self) -> LayoutConfig {
        let (merged, _warnings) =
            crate::layout_merge::merge_layout_config(&LayoutConfig::default(), &self.overrides());
        merged
    }

    /// The partial override set that defines this preset. `Default` is the
    /// empty override set (the merge becomes the identity).
    fn overrides(self) -> LayoutOverrides {
        match self {
            GalleryLayoutPreset::Default => LayoutOverrides::default(),

            GalleryLayoutPreset::Narrow => LayoutOverrides {
                home: Some(HomeOverrides {
                    mode: Some(HomeLayoutMode::List),
                    hidden_sections: Some(vec![HomeSection::Tunnels]),
                    max_content_width: Some(640.0),
                    grid: Some(HomeGridOverrides {
                        main_portion: Some(1),
                        rail_portion: Some(1),
                        ..Default::default()
                    }),
                    padding: Some(HomePaddingOverrides {
                        top: Some(16.0),
                        bottom: Some(16.0),
                        horizontal_large: Some(12.0),
                        horizontal_default: Some(12.0),
                    }),
                    gaps: Some(HomeGapsOverrides {
                        card_gap: Some(12.0),
                        hero_gap: Some(24.0),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                component: Some(ComponentOverrides {
                    video_card: Some(ComponentPlacementOverrides {
                        thumbnail_position: Some(ThumbnailPosition::Top),
                        metadata_alignment: Some(MetadataAlignment::Start),
                        button_placement: Some(ButtonPlacement::Below),
                        card_orientation: Some(CardOrientation::Vertical),
                    }),
                    ..Default::default()
                }),
                responsive: Some(ResponsiveOverrides {
                    home_columns: Some(ByTierOverrides {
                        narrow: Some(1),
                        desktop: Some(1),
                        ultra_wide: Some(1),
                    }),
                    home_padding_x: Some(ByTierOverrides {
                        narrow: Some(12.0),
                        desktop: Some(12.0),
                        ultra_wide: Some(16.0),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },

            GalleryLayoutPreset::Desktop => LayoutOverrides {
                home: Some(HomeOverrides {
                    max_content_width: Some(1280.0),
                    grid: Some(HomeGridOverrides {
                        main_portion: Some(3),
                        rail_portion: Some(2),
                        ..Default::default()
                    }),
                    gaps: Some(HomeGapsOverrides {
                        card_gap: Some(16.0),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                component: Some(ComponentOverrides {
                    video_card: Some(ComponentPlacementOverrides {
                        thumbnail_position: Some(ThumbnailPosition::Left),
                        metadata_alignment: Some(MetadataAlignment::Start),
                        button_placement: Some(ButtonPlacement::Side),
                        card_orientation: Some(CardOrientation::Horizontal),
                    }),
                    ..Default::default()
                }),
                responsive: Some(ResponsiveOverrides {
                    home_columns: Some(ByTierOverrides {
                        narrow: Some(1),
                        desktop: Some(2),
                        ultra_wide: Some(2),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },

            GalleryLayoutPreset::Maximized => LayoutOverrides {
                home: Some(HomeOverrides {
                    max_content_width: Some(1920.0),
                    padding: Some(HomePaddingOverrides {
                        horizontal_large: Some(40.0),
                        horizontal_default: Some(32.0),
                        ..Default::default()
                    }),
                    gaps: Some(HomeGapsOverrides {
                        card_gap: Some(28.0),
                        hero_gap: Some(48.0),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                responsive: Some(ResponsiveOverrides {
                    home_columns: Some(ByTierOverrides {
                        narrow: Some(1),
                        desktop: Some(2),
                        ultra_wide: Some(3),
                    }),
                    home_padding_x: Some(ByTierOverrides {
                        narrow: Some(20.0),
                        desktop: Some(32.0),
                        ultra_wide: Some(40.0),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        }
    }
}

/// Interactive state behind the gallery's responsive preview controls
/// (preset selection + custom-width slider). Owned by `IcedChat`, dev-ui
/// only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GalleryState {
    /// The active preset (falls back to `Custom` while the slider is used).
    pub preset: GalleryWidthPreset,
    /// The slider value; retained so switching back to Custom keeps the
    /// last dragged width.
    pub custom_width: f32,
    /// BORU-LAYOUT-09: the layout-config preset the preview applies.
    pub layout_preset: GalleryLayoutPreset,
}

impl Default for GalleryState {
    fn default() -> Self {
        Self {
            preset: GalleryWidthPreset::Desktop,
            custom_width: GalleryWidthPreset::Custom.width(),
            layout_preset: GalleryLayoutPreset::Default,
        }
    }
}

impl GalleryState {
    /// The simulated content width for the current preset, clamped into the
    /// slider range so out-of-range values can never reach layout.
    pub fn width(&self) -> f32 {
        match self.preset {
            GalleryWidthPreset::Custom => {
                self.custom_width.clamp(CUSTOM_WIDTH_MIN, CUSTOM_WIDTH_MAX)
            }
            preset => preset.width(),
        }
    }
}

/// Effective simulated preview width: the selected width bounded by the
/// available window so wide presets never overflow horizontally, and never
/// below the slider minimum. A small gutter keeps the frame's border
/// visible inside the scrollable even at maximized widths.
fn effective_preview_width(state: &GalleryState, window_width: f32) -> f32 {
    let available = if window_width > 0.0 { window_width } else { 1280.0 };
    let bounded = available - 96.0;
    if bounded < CUSTOM_WIDTH_MIN {
        state.width().min(available)
    } else {
        state.width().min(bounded)
    }
}

/// Build the complete component gallery view with responsive preview
/// controls (PDF Task 15). `window_width` bounds the simulated width so
/// wide presets degrade gracefully on small windows. `btheme` is the live
/// merged theme so typography samples reflect inspector edits (BORU-UI-16).
///
/// BORU-LAYOUT-09 / PDF Task 9: the preview also applies a layout-config
/// preset (see [`GalleryLayoutPreset`]) — the concrete [`LayoutConfig`]
/// for the selected preset is threaded through every layout-aware
/// component (download/video cards) and the Layout Preview mini-home.
pub fn view_gallery(
    state: &GalleryState,
    window_width: f32,
    btheme: &crate::theme::BoruTheme,
) -> Element<'static, AppMessage> {
    let preview_width = effective_preview_width(state, window_width);
    let layout = state.layout_preset.layout_config();

    // Full-width control bar, then the whole gallery re-laid out inside a
    // fixed-width frame so every production component responds to the
    // simulated content width (container width constraint).
    let page = Column::new()
        .push(responsive_preview_controls(state, preview_width))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
        .push(
            container(gallery_sections(btheme, &layout, preview_width))
                .padding(design_tokens::SPACE_24)
                .width(Length::Fixed(preview_width))
                .style(design_tokens::card_style),
        )
        .spacing(0)
        .align_x(Alignment::Center)
        .width(Length::Fill);

    crate::ui_components::gutter_scrollable(
        container(page)
            .padding(design_tokens::SPACE_24)
            .width(Length::Fill),
    )
    .into()
}

/// BORU-UI-15: the responsive-preview control bar — preset buttons for
/// narrow / desktop / maximized widths plus the custom-width slider. Always
/// full width (outside the simulated frame) so the controls stay usable
/// while the preview below is width-constrained. The readout shows the
/// effective simulated width, including any window clamping.
fn responsive_preview_controls(
    state: &GalleryState,
    preview_width: f32,
) -> Element<'static, AppMessage> {
    let label = text("Preview width:")
        .font(TypeRole::Metadata.font())
        .size(TypeRole::Metadata.size_px())
        .color(design_tokens::text_muted(&Theme::Light));

    let preset_button = |preset: GalleryWidthPreset| {
        if state.preset == preset {
            primary_button(preset.label(), None, false)
        } else {
            secondary_button(
                preset.label(),
                Some(AppMessage::GalleryPreset(preset)),
                false,
            )
        }
    };

    let custom_label = text("Custom:")
        .font(TypeRole::Metadata.font())
        .size(TypeRole::Metadata.size_px())
        .color(design_tokens::text_muted(&Theme::Light));

    let width_slider = slider(
        CUSTOM_WIDTH_MIN..=CUSTOM_WIDTH_MAX,
        state.custom_width.clamp(CUSTOM_WIDTH_MIN, CUSTOM_WIDTH_MAX),
        move |v| AppMessage::GalleryCustomWidth(v),
    )
    .width(Length::Fixed(220.0));

    let readout = text(format!("{} px", preview_width as u32))
        .font(TypeRole::Metadata.font())
        .size(TypeRole::Metadata.size_px())
        .color(design_tokens::text_primary(&Theme::Light));

    let width_row = Row::new()
        .push(label)
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
        .push(preset_button(GalleryWidthPreset::Narrow))
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
        .push(preset_button(GalleryWidthPreset::Desktop))
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
        .push(preset_button(GalleryWidthPreset::Maximized))
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_16)))
        .push(custom_label)
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
        .push(width_slider)
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
        .push(readout)
        .spacing(0)
        .align_y(Alignment::Center)
        .wrap();

    // BORU-LAYOUT-09: layout-config preset row. Each preset is a concrete
    // LayoutConfig (same merge path as boru-layout.toml); the readout
    // shows the resolved mode/columns/gap at the current preview width.
    let layout_label = text("Layout:")
        .font(TypeRole::Metadata.font())
        .size(TypeRole::Metadata.size_px())
        .color(design_tokens::text_muted(&Theme::Light));

    let layout_preset_button = |preset: GalleryLayoutPreset| {
        if state.layout_preset == preset {
            primary_button(preset.label(), None, false)
        } else {
            secondary_button(
                preset.label(),
                Some(AppMessage::GalleryLayoutPreset(preset)),
                false,
            )
        }
    };

    let layout = state.layout_preset.layout_config();
    let columns = layout.responsive.home_columns_for_width(preview_width);
    let mode = match layout.home.mode {
        crate::layout::HomeLayoutMode::Row => "row",
        crate::layout::HomeLayoutMode::Column => "column",
        crate::layout::HomeLayoutMode::Grid => "grid",
        crate::layout::HomeLayoutMode::List => "list",
    };
    let layout_readout = text(format!(
        "{mode} · {columns} col · gap {}px · pad {}px",
        layout.home.gaps.card_gap as u32,
        layout.responsive.home_padding_x_for_width(preview_width) as u32,
    ))
    .font(TypeRole::Metadata.font())
    .size(TypeRole::Metadata.size_px())
    .color(design_tokens::text_primary(&Theme::Light));

    let layout_row = Row::new()
        .push(layout_label)
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_8)))
        .push(layout_preset_button(GalleryLayoutPreset::Default))
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
        .push(layout_preset_button(GalleryLayoutPreset::Narrow))
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
        .push(layout_preset_button(GalleryLayoutPreset::Desktop))
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_4)))
        .push(layout_preset_button(GalleryLayoutPreset::Maximized))
        .push(Space::new().width(Length::Fixed(design_tokens::SPACE_16)))
        .push(layout_readout)
        .spacing(0)
        .align_y(Alignment::Center)
        .wrap();

    Column::new()
        .push(width_row)
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(layout_row)
        .spacing(0)
        .align_x(Alignment::Start)
        .into()
}

/// The static gallery sections (buttons, cards, bubbles, attachments, …).
/// `btheme` is the live merged theme so the typography section reflects
/// inspector edits (BORU-UI-16).
///
/// BORU-LAYOUT-09 / PDF Task 9: `layout` is the concrete [`LayoutConfig`]
/// of the selected gallery layout preset. It drives the Layout Preview
/// mini-home section and the component placement of the download/video
/// cards (the layout-aware production components), so every reusable
/// component can be inspected under different layout configurations.
fn gallery_sections(
    btheme: &crate::theme::BoruTheme,
    layout: &LayoutConfig,
    preview_width: f32,
) -> Element<'static, AppMessage> {
    let video_placement = layout.component.video_card;
    Column::new()
        .push(gallery_heading("Component Gallery"))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
        .push(gallery_section("Layout Preview (BORU-LAYOUT-09)"))
        .push(layout_preview_gallery(layout, preview_width))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Buttons"))
        .push(button_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Cards"))
        .push(card_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Card Shell (Figure 3 rail)"))
        .push(card_shell_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("List Rows"))
        .push(list_row_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Avatars"))
        .push(avatar_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Status Dots & Badges"))
        .push(status_and_badge_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Text Input"))
        .push(text_input_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Empty State"))
        .push(empty_state_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Section Header & Card Header"))
        .push(header_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Dividers"))
        .push(divider_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Timeline (Figure 4)"))
        .push(timeline_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Elevated Card (Dialog)"))
        .push(dialog_example())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("BoruDialog (Reusable Modal)"))
        .push(boru_dialog_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Tab Strip"))
        .push(tab_strip_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Progress Bar"))
        .push(progress_bar_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("File Identity Cell"))
        .push(file_identity_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Peer Chip Stack"))
        .push(peer_chip_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Metric Block"))
        .push(metric_block_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Loading Skeleton"))
        .push(loading_skeleton_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Inline Error & Retry"))
        .push(inline_error_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Table Header Row"))
        .push(table_header_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Overflow Menu"))
        .push(overflow_menu_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Form Primitives (UI-RESTYLE-03)"))
        .push(form_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Names: Short & Long (PDF Task 14)"))
        .push(name_variants_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Message Bubbles (PDF Task 14)"))
        .push(message_bubble_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Attachment States (PDF Task 14)"))
        .push(attachment_states_gallery(video_placement, preview_width))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Video Cards & Aspect Ratios (PDF Task 14)"))
        .push(video_card_gallery(video_placement, preview_width))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Available Widths (PDF Task 14)"))
        .push(width_variants_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("State Variants (PDF Task 14)"))
        .push(state_variants_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Typography (UI-HOME-11 / BORU-UI-16)"))
        .push(typography_gallery(btheme))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_32)))
        .spacing(0)
        .into()
}

fn gallery_heading(label: &str) -> Element<'static, AppMessage> {
    let owned = label.to_string();
    text(owned)
        .font(TypeRole::PageTitle.font())
        .size(TypeRole::PageTitle.size_px())
        .color(design_tokens::text_primary(&Theme::Light))
        .into()
}

fn gallery_section(label: &str) -> Element<'static, AppMessage> {
    let label_str = label.to_string();
    let label_el: Element<'_, AppMessage> = text(label_str)
        .font(TypeRole::SectionTitle.font())
        .size(TypeRole::SectionTitle.size_px())
        .color(design_tokens::primary(&Theme::Light))
        .into();
    Column::new()
        .push(label_el)
        .push(divider::<AppMessage>())
        .spacing(design_tokens::SPACE_8)
        .into()
}

fn state_label(label: &str) -> Element<'static, AppMessage> {
    let owned = label.to_string();
    text(owned)
        .size(TypeRole::Metadata.size_px())
        .color(design_tokens::text_muted(&Theme::Light))
        .into()
}

// ── Layout preview (BORU-LAYOUT-09 / PDF Task 9) ──────────────────────

/// Split the visible home sections into the main column and the right rail
/// the way `app/home.rs` does: the rail holds the trailing sections
/// (PeopleActivity, Tunnels by default) and everything before them goes to
/// the main column. Only used in grid mode; list mode stacks everything in
/// one column.
fn split_home_sections(
    visible: &[HomeSection],
) -> (Vec<HomeSection>, Vec<HomeSection>) {
    if visible.len() > 2 {
        let split_at = visible.len() - 2;
        (visible[..split_at].to_vec(), visible[split_at..].to_vec())
    } else {
        (visible.to_vec(), Vec::new())
    }
}

/// A representative demo card for one home-dashboard section, built from
/// the production `CardShell` component (the Figure 3 rail).
fn home_section_demo(section: HomeSection) -> Element<'static, AppMessage> {
    let theme = Theme::Light;
    let (title, rows): (&str, Vec<(&str, &str)>) = match section {
        HomeSection::Hero => (
            "Hero",
            vec![("Connection", "Direct QUIC"), ("Neighbors", "5")],
        ),
        HomeSection::MeshHealth => (
            "Mesh Health",
            vec![("Peers online", "3"), ("Relay", "Healthy")],
        ),
        HomeSection::QuickActions => (
            "Quick Actions",
            vec![("New chat", "Create"), ("Share file", "Send")],
        ),
        HomeSection::PeopleActivity => (
            "People & Activity",
            vec![("Alice", "online"), ("Bob", "idle")],
        ),
        HomeSection::Tunnels => (
            "Tunnels",
            vec![("Direct", "Active"), ("Relay", "Standby")],
        ),
    };
    let rows: Vec<Element<'static, AppMessage>> = rows
        .into_iter()
        .map(|(label, meta)| card_shell_row(label, meta))
        .collect();
    CardShell::new(title, rows).count(2).build(&theme)
}

/// BORU-LAYOUT-09 / PDF Task 9: preview every reusable component under the
/// selected layout configuration. This section renders a miniature home
/// dashboard — one demo card per home section — arranged by the live
/// `LayoutConfig`: section order/visibility (`home.section_order` /
/// `home.hidden_sections`), grid vs list mode (`home.mode`), the per-tier
/// column count resolved from the simulated preview width
/// (`responsive.home_columns`), the main/rail split
/// (`home.grid.main_portion`/`rail_portion`), section gaps (`home.gaps`)
/// and the responsive canvas padding (`responsive.home_padding_x`). All
/// values come from the same [`LayoutConfig`] the real home screen uses.
fn layout_preview_gallery(
    layout: &LayoutConfig,
    preview_width: f32,
) -> Element<'static, AppMessage> {
    let home = &layout.home;
    let visible = home.visible_sections();
    let columns = layout.responsive.home_columns_for_width(preview_width);
    let grid = home.mode == HomeLayoutMode::Grid && columns >= 2;
    let card_gap = home.gaps.card_gap;
    let canvas_pad_x = layout.responsive.home_padding_x_for_width(preview_width);

    let body: Element<'static, AppMessage> = if grid {
        let (main, rail) = split_home_sections(&visible);
        let main_col = Column::new()
            .spacing(card_gap)
            .extend(main.iter().copied().map(home_section_demo))
            .width(Length::FillPortion(home.grid.main_portion));
        let rail_col = Column::new()
            .spacing(card_gap)
            .extend(rail.iter().copied().map(home_section_demo))
            .width(Length::FillPortion(home.grid.rail_portion));
        Row::new()
            .push(main_col)
            .push(Space::new().width(Length::Fixed(home.grid.column_gap)))
            .push(rail_col)
            .spacing(0)
            .align_y(Alignment::Start)
            .into()
    } else {
        Column::new()
            .spacing(card_gap)
            .extend(visible.iter().copied().map(home_section_demo))
            .into()
    };

    let summary = format!(
        "{} · {} col · gap {}px · pad {}px · {} sections",
        if grid { "grid" } else { "list" },
        columns,
        card_gap as u32,
        canvas_pad_x as u32,
        visible.len(),
    );

    Column::new()
        .push(state_label(&summary))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(
            container(body)
                .padding(iced::Padding {
                    top: home.padding.top,
                    right: canvas_pad_x,
                    bottom: home.padding.bottom,
                    left: canvas_pad_x,
                })
                .width(Length::Fill)
                .max_width(home.max_content_width)
                .style(design_tokens::card_style),
        )
        .spacing(0)
        .width(Length::Fill)
        .into()
}

// ── Button gallery ────────────────────────────────────────────────────

fn button_gallery() -> Element<'static, AppMessage> {
    let row = Row::new()
        .push(button_pair(&crate::i18n::t("gallery.primary"),
            primary_button(crate::i18n::t("gallery.primary"), None, false),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(&crate::i18n::t("gallery.primary_disabled"),
            primary_button(crate::i18n::t("gallery.disabled"), None, true),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(&crate::i18n::t("gallery.secondary"),
            secondary_button(crate::i18n::t("gallery.secondary"), None, false),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(&crate::i18n::t("gallery.secondary_disabled"),
            secondary_button(crate::i18n::t("gallery.disabled"), None, true),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(&crate::i18n::t("gallery.primary_icon"),
            primary_button_icon(Icon::Plus, crate::i18n::t("gallery.add"), None, false),
        ))
        .spacing(0)
        .align_y(Alignment::Center)
        .wrap();

    Column::new()
        .push(row)
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_12)),
        )
        .push(
            Row::new()
                .push(button_pair(&crate::i18n::t("gallery.ghost_icon"),
                    ghost_icon_button(
                        Icon::Settings,
                        IconSize::Md,
                        Some("Settings"),
                        None,
                        false,
                        false,
                        false,
                    ),
                ))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(button_pair(&crate::i18n::t("gallery.ghost_destructive"),
                    ghost_icon_button(
                        Icon::Delete,
                        IconSize::Md,
                        Some("Delete"),
                        None,
                        false,
                        true,
                        false,
                    ),
                ))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(button_pair(&crate::i18n::t("gallery.ghost_disabled"),
                    ghost_icon_button(Icon::Chat, IconSize::Md, None, None, true, false, false),
                ))
                .spacing(0)
                .align_y(Alignment::Center),
        )
        .spacing(0)
        .into()
}

fn button_pair(label: &str, btn: Element<'static, AppMessage>) -> Element<'static, AppMessage> {
    Column::new()
        .push(state_label(label))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(btn)
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

// ── Card gallery ──────────────────────────────────────────────────────

fn card_gallery() -> Element<'static, AppMessage> {
    let card_content: Vec<Element<'static, AppMessage>> = vec![
        text(crate::i18n::t("gallery.this_is_a_card_with_some_content"))
            .size(TypeRole::Body.size_px())
            .into(),
        primary_button(crate::i18n::t("gallery.action"), None, false),
    ];

    let clickable_content: Vec<Element<'static, AppMessage>> =
        vec![text(crate::i18n::t("gallery.click_me_i_m_interactive"))
            .size(TypeRole::Body.size_px())
            .into()];

    let card_noop: AppMessage = AppMessage::Noop;

    Row::new()
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.standard_card")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(Card::new(card_content).build(&Theme::Light))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.clickable_card")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(
                    Card::new(clickable_content)
                        .on_press(card_noop)
                        .build(&Theme::Light),
                )
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.icon_tile")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(icon_tile::<AppMessage>(Icon::Chat, IconSize::Lg, None))
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── Card shell gallery (Figure 3 rail) ────────────────────────────────

/// A single demo row at the shared 48 px rail row height.
fn card_shell_row(label: &str, meta: &str) -> Element<'static, AppMessage> {
    container(
        Row::new()
            .push(
                text(label.to_string())
                    .font(TypeRole::Body.font())
                    .size(TypeRole::Body.size_px())
                    .color(design_tokens::text_primary(&Theme::Light)),
            )
            .push(Space::new().width(Length::Fill))
            .push(
                text(meta.to_string())
                    .font(TypeRole::Metadata.font())
                    .size(TypeRole::Metadata.size_px())
                    .color(design_tokens::text_muted(&Theme::Light)),
            )
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center),
    )
    .height(Length::Fixed(CARD_ROW_HEIGHT))
    .width(Length::Fill)
    .align_y(Alignment::Center)
    .into()
}

fn card_shell_gallery() -> Element<'static, AppMessage> {
    let noop: AppMessage = AppMessage::Noop;

    // Empty state — title + count badge + caller-provided empty message.
    let empty_shell = CardShell::new("Online Peers", vec![])
        .count(0)
        .empty_message("No peers are online right now.")
        .build(&Theme::Light);

    // Populated state — 8 rows at 48 px exceed the bounded 140 px body, so a
    // vertical scrollbar appears instead of the card growing without bound.
    let rows: Vec<Element<'static, AppMessage>> = (1..=8)
        .map(|i| {
            card_shell_row(
                &format!("Peer {i}"),
                if i % 2 == 0 { "online" } else { "idle" },
            )
        })
        .collect();
    let populated_shell = CardShell::new("Online Peers", rows)
        .count(5)
        .on_view_all(noop)
        .max_height(140.0)
        .row_spacing(design_tokens::SPACE_4)
        .build(&Theme::Light);

    // Full foundation — every semantic area: sentence-case title, subtitle,
    // count badge, status pill, header action, content-driven body and footer.
    let body_block: Element<'static, AppMessage> = container(
        Column::new()
            .push(
                text(crate::i18n::t("gallery.connected_3_direct_2_relayed_5_neighbors"))
                    .font(TypeRole::Body.font())
                    .size(TypeRole::Body.size_px())
                    .color(design_tokens::text_primary(&Theme::Light)),
            )
            .push(
                Space::new()
                    .width(Length::Shrink)
                    .height(Length::Fixed(design_tokens::SPACE_4)),
            )
            .push(
                text(crate::i18n::t("gallery.quic_encrypted_connected_12m"))
                    .font(TypeRole::Metadata.font())
                    .size(TypeRole::Metadata.size_px())
                    .color(design_tokens::text_muted(&Theme::Light)),
            )
            .spacing(0)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .into();
    let footer_line: Element<'static, AppMessage> = text(crate::i18n::t("gallery.mesh_healthy_3_peers"))
        .font(TypeRole::Metadata.font())
        .size(TypeRole::Metadata.size_px())
        .color(design_tokens::text_muted(&Theme::Light))
        .into();
    let full_shell = CardShell::new("Mesh Health", vec![])
        .title_case(false)
        .subtitle(crate::i18n::t("gallery.current_connection_status"))
        .count(3)
        .count_total(12)
        .status_badge("Healthy", crate::card_shell::StatusBadgeKind::Success)
        .header_action("View details", AppMessage::OpenConnectionDetails)
        .body(body_block)
        .footer(footer_line)
        .build(&Theme::Light);

    Row::new()
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.empty_state")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(empty_shell)
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.8_rows_max_height_scrollbar_count_badge_view_all"),
                ))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(populated_shell)
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.full_foundation_title_subtitle_status_pill_actio"),
                ))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(full_shell)
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── List row gallery ──────────────────────────────────────────────────

fn list_row_gallery() -> Element<'static, AppMessage> {
    let noop: AppMessage = AppMessage::Noop;
    let theme = Theme::Light;

    let avatar_alice: Element<'static, AppMessage> = Avatar::<AppMessage>::new("Alice")
        .size(design_tokens::AVATAR_SM)
        .build();
    let avatar_bob: Element<'static, AppMessage> = Avatar::<AppMessage>::new("Bob")
        .size(design_tokens::AVATAR_SM)
        .build();
    let avatar_carol: Element<'static, AppMessage> = Avatar::<AppMessage>::new("Carol")
        .size(design_tokens::AVATAR_SM)
        .build();

    let dot_online: Element<'static, AppMessage> = status_dot(StatusDotKind::Online, 10.0);
    let badge_count: Element<'static, AppMessage> = badge("3", BadgeKind::Count);

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.default_rows")))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(
            ListRow::<AppMessage>::new("Alice — with avatar & timestamp")
                .leading(avatar_alice)
                .secondary(crate::i18n::t("gallery.last_seen_2m_ago"))
                .trailing(dot_online)
                .build(&theme),
        )
        .push(
            ListRow::<AppMessage>::new("Bob — with unread badge")
                .leading(avatar_bob)
                .secondary(crate::i18n::t("gallery.hey_are_you_free"))
                .trailing(badge_count)
                .build(&theme),
        )
        .push(
            ListRow::<AppMessage>::new("Carol — selected state")
                .leading(avatar_carol)
                .secondary(crate::i18n::t("gallery.selected_row_example"))
                .selected(true)
                .build(&theme),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(state_label(&crate::i18n::t("gallery.clickable_row")))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(
            ListRow::<AppMessage>::new("Clickable row — full-width hit target")
                .secondary(crate::i18n::t("gallery.press_anywhere_on_this_row"))
                .on_press(noop)
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Avatar gallery ────────────────────────────────────────────────────

fn avatar_gallery() -> Element<'static, AppMessage> {
    Row::new()
        .push(avatar_example("Alice", "Initial"))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_example("Bob Smith", "Two initials"))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_example("", "Empty (fallback ?)"))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_with_extra("Carol", "With online dot", true, None))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_with_extra(
            "Dave",
            "With unread badge",
            false,
            Some(5),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_example("Eve", "Size: SM (36px)"))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_example("Frank", "Size: PROFILE (72px)"))
        .spacing(0)
        .align_y(Alignment::End)
        .into()
}

fn avatar_example(name: &str, label: &str) -> Element<'static, AppMessage> {
    let mut avatar = Avatar::<AppMessage>::new(name);
    if name == "Eve" {
        avatar = avatar.size(design_tokens::AVATAR_SM);
    }
    if name == "Frank" {
        avatar = avatar.profile_size();
    }

    Column::new()
        .push(avatar.build())
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(state_label(label))
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

fn avatar_with_extra(
    name: &str,
    label: &str,
    online: bool,
    unread: Option<u32>,
) -> Element<'static, AppMessage> {
    let mut avatar = Avatar::<AppMessage>::new(name);
    if online {
        avatar = avatar.online_dot(true);
    }
    if let Some(count) = unread {
        avatar = avatar.unread_badge(count);
    }

    Column::new()
        .push(avatar.build())
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(state_label(label))
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

// ── Status dots & badges ──────────────────────────────────────────────

fn status_and_badge_gallery() -> Element<'static, AppMessage> {
    Row::new()
        .push(badge_sample(
            "Online",
            status_dot::<AppMessage>(StatusDotKind::Online, 12.0),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Offline",
            status_dot::<AppMessage>(StatusDotKind::Offline, 12.0),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Warning",
            status_dot::<AppMessage>(StatusDotKind::Warning, 12.0),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_24))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Default",
            badge::<AppMessage>("Default", BadgeKind::Default),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Accent",
            badge::<AppMessage>("Accent", BadgeKind::Accent),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Count",
            badge::<AppMessage>("42", BadgeKind::Count),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Danger",
            badge::<AppMessage>("Error", BadgeKind::Danger),
        ))
        .spacing(0)
        .align_y(Alignment::Center)
        .into()
}

fn badge_sample(label: &str, sample: Element<'static, AppMessage>) -> Element<'static, AppMessage> {
    Column::new()
        .push(sample)
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(state_label(label))
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

// ── Text input gallery ────────────────────────────────────────────────

fn text_input_gallery() -> Element<'static, AppMessage> {
    Row::new()
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.default")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(text_input_field(
                    "Placeholder text…",
                    "",
                    |_| AppMessage::Noop,
                    false,
                ))
                .width(Length::Fixed(240.0)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.with_value")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(text_input_field(
                    "Placeholder…",
                    "Current value",
                    |_| AppMessage::Noop,
                    false,
                ))
                .width(Length::Fixed(240.0)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.error_state")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(text_input_field(
                    "Placeholder…",
                    "bad input",
                    |_| AppMessage::Noop,
                    true,
                ))
                .width(Length::Fixed(240.0)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── Empty state gallery ───────────────────────────────────────────────

fn empty_state_gallery() -> Element<'static, AppMessage> {
    container(
        Column::new()
            .push(state_label(&crate::i18n::t("gallery.empty_state_with_action")))
            .push(
                Space::new()
                    .width(Length::Shrink)
                    .height(Length::Fixed(8.0)),
            )
            .push(empty_state(
                Icon::Chat,
                "No conversations yet",
                "Start a new conversation to see it here.",
                Some("New Chat"),
                Some(AppMessage::Noop),
            ))
            .spacing(0),
    )
    .height(Length::Fixed(240.0))
    .into()
}

// ── Section header & card header ──────────────────────────────────────

fn header_gallery() -> Element<'static, AppMessage> {
    let action_btn = ghost_icon_button(
        Icon::Plus,
        IconSize::Sm,
        Some("Add"),
        Some(AppMessage::Noop),
        false,
        false,
        false,
    );

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.section_header")))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(section_header("CONVERSATIONS", Some(action_btn)))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        )
        .push(state_label(&crate::i18n::t("gallery.card_header")))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(card_header(
            Some(Icon::Chat),
            "Active Chats",
            Some("12"),
            BadgeKind::Accent,
            Some(ghost_icon_button(
                Icon::More,
                IconSize::Sm,
                None,
                Some(AppMessage::Noop),
                false,
                false,
                false,
            )),
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(card_header(
            Some(Icon::AlertTriangle),
            "Error Monitoring",
            Some("3"),
            BadgeKind::Danger,
            None,
        ))
        .spacing(0)
        .into()
}

// ── Divider gallery ───────────────────────────────────────────────────

fn divider_gallery() -> Element<'static, AppMessage> {
    Column::new()
        .push(text(crate::i18n::t("gallery.above_divider")).size(TypeRole::Body.size_px()))
        .push(divider::<AppMessage>())
        .push(text(crate::i18n::t("gallery.below_divider")).size(TypeRole::Body.size_px()))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(state_label(&crate::i18n::t("gallery.divider_is_a_thin_horizontal_line_between_items_"),
        ))
        .spacing(design_tokens::SPACE_8)
        .into()
}

// ── Timeline (Figure 4) gallery ──────────────────────────────────────

fn timeline_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;
    let spacing = design_tokens::SPACE_24;

    // Sample timeline: date separators open each day; system-event chips use
    // the same muted/centred treatment as the real chat log. The caller
    // supplies label + accent — no classification logic lives in the chip.
    let timeline = Column::new()
        .push(date_separator("Today", &theme))
        .push(system_event_chip(
            "MEMBER",
            design_tokens::online(&theme),
            "Alice joined the chat.",
            &theme,
        ))
        .push(system_event_chip(
            "NAME",
            design_tokens::primary(&theme),
            "Alice renamed the room to Kitchen",
            &theme,
        ))
        .push(system_event_chip(
            "HELP",
            design_tokens::text_muted(&theme),
            "Usage: /help — show available commands",
            &theme,
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        )
        .push(date_separator("Yesterday", &theme))
        .push(system_event_chip(
            "NOTICE",
            design_tokens::color_warning(&theme),
            "Message delivery failed after 3 attempts.",
            &theme,
        ))
        .push(system_event_chip(
            "INFO",
            design_tokens::text_muted(&theme),
            "Invite sent to Bob.",
            &theme,
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        )
        .push(date_separator("Sunday, August 2, 2026", &theme))
        .push(system_event_chip(
            "MEMBER",
            design_tokens::online(&theme),
            "Chat joined.",
            &theme,
        ))
        .spacing(design_tokens::SPACE_4);

    Row::new()
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.date_separators_centered_muted_12_px_chips_muted"),
                ))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(timeline)
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(spacing))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.chip_inputs_come_from_the_caller")))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(
                    container(
                        Column::new()
                            .push(state_label(&crate::i18n::t("gallery.system_event_chip_label_accent_body_theme")))
                            .push(
                                Space::new()
                                    .width(Length::Shrink)
                                    .height(Length::Fixed(design_tokens::SPACE_4)),
                            )
                            .push(system_event_chip(
                                "MEMBER",
                                design_tokens::online(&theme),
                                "accent = online green",
                                &theme,
                            ))
                            .push(system_event_chip(
                                "NAME",
                                design_tokens::primary(&theme),
                                "accent = primary green",
                                &theme,
                            ))
                            .push(system_event_chip(
                                "HELP",
                                design_tokens::text_muted(&theme),
                                "accent = muted text",
                                &theme,
                            ))
                            .push(system_event_chip(
                                "NOTICE",
                                design_tokens::color_warning(&theme),
                                "accent = warning amber",
                                &theme,
                            ))
                            .spacing(design_tokens::SPACE_4),
                    )
                    .padding(design_tokens::SPACE_12)
                    .style(design_tokens::card_style),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── Elevated card / dialog example ────────────────────────────────────

fn dialog_example() -> Element<'static, AppMessage> {
    let content = Column::new()
        .push(
            text(crate::i18n::t("gallery.elevated_card_dialog"))
                .font(TypeRole::SectionTitle.font())
                .size(TypeRole::SectionTitle.size_px()),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(
            text(crate::i18n::t("gallery.this_is_an_elevated_card_with_a_higher_drop_shad"))
                .size(TypeRole::Body.size_px()),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_12)),
        )
        .push(
            Row::new()
                .push(secondary_button(crate::i18n::t("gallery.cancel"), None, false))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_8))
                        .height(Length::Shrink),
                )
                .push(primary_button(crate::i18n::t("gallery.confirm"), None, false))
                .spacing(0),
        )
        .spacing(0);

    container(elevated_card::<AppMessage>(vec![content.into()]))
        .width(Length::Fixed(400.0))
        .into()
}

// ── BoruDialog (reusable modal) gallery ─────────────────────────────

fn boru_dialog_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    let state_label = text(crate::i18n::t("gallery.the_reusable_borudialog_shell_header_title_subti"))
        .size(TypeRole::Metadata.size_px())
        .style(move |_| iced::widget::text::Style {
            color: Some(design_tokens::text_secondary(&Theme::Light)),
        });

    // Bound the full-screen modal overlay inside a fixed-height frame so the
    // gallery can demonstrate the backdrop + centred panel without taking over
    // the whole window.
    let modal = BoruDialog::new(crate::i18n::t("gallery.create_group_chat"))
        .subtitle(crate::i18n::t("gallery.start_a_private_group_conversation"))
        .push_body(text_input_field("Group name…", "", |_| AppMessage::Noop, false))
        .push_body(text_input_field(
            "Description (optional)…",
            "",
            |_| AppMessage::Noop,
            false,
        ))
        .push_body(
            text(crate::i18n::t("gallery.long_form_content_scrolls_internally_inside_the_"))
                .size(TypeRole::Metadata.size_px())
                .into(),
        )
        .scroll_body(120.0)
        .secondary(crate::i18n::t("gallery.cancel"), AppMessage::Noop)
        .primary(crate::i18n::t("gallery.create"), AppMessage::Noop)
        .on_close(AppMessage::Noop)
        .width(560.0)
        .build(&theme);

    let framed: Element<'static, AppMessage> = container(modal)
        .width(Length::Fill)
        .height(Length::Fixed(340.0))
        .into();

    Column::new()
        .push(state_label)
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_12)))
        .push(framed)
        .spacing(0)
        .into()
}

// ── Tab strip gallery ─────────────────────────────────────────────────

fn tab_strip_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    let tabs: Vec<(String, AppMessage)> = vec![
        (crate::i18n::t("gallery.tab_shared_by_me"), AppMessage::Noop),
        (crate::i18n::t("gallery.tab_downloading"), AppMessage::Noop),
        (crate::i18n::t("gallery.tab_downloaded"), AppMessage::Noop),
        (crate::i18n::t("gallery.tab_shared_with_me"), AppMessage::Noop),
        (crate::i18n::t("gallery.tab_activity_log"), AppMessage::Noop),
    ];

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.active_tab_shared_by_me_second_tab_is_clickable_"),
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(TabStrip::<AppMessage>::new(tabs).active(0).build(&theme))
        .spacing(0)
        .into()
}

// ── Progress bar gallery ──────────────────────────────────────────────

fn progress_bar_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Row::new()
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.0")))
                .push(ProgressBar::<AppMessage>::new(0.0).build(&theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.45")))
                .push(ProgressBar::<AppMessage>::new(0.45).build(&theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.100_complete")))
                .push(
                    ProgressBar::<AppMessage>::new(1.0)
                        .kind(ProgressKind::Complete)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.paused")))
                .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
                .push(state_label(&crate::i18n::t("gallery.indeterminate")))
                .push(
                    ProgressBar::<AppMessage>::new(0.0)
                        .indeterminate(true)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .into()
}

// ── File identity cell gallery ────────────────────────────────────────

fn file_identity_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    // PAPIRUS-13: file identity cells must lead with the central Papirus
    // file-type component (same icon the chat cards and dashboard rows use),
    // never a Lucide Icon — the icon answers "what type of file is this?",
    // and status is conveyed separately by the caller.
    // PAPIRUS-15: the gallery cells already print the MIME type in the
    // metadata line, so the icons are decorative (hidden from assistive
    // technology; no redundant type tooltip).
    let pdf_icon = crate::download_progress_view::decorative_file_type_icon_element(
        "QuarterlyReport.pdf",
        Some("application/pdf"),
        None,
        crate::file_type_icon::FileTypeIconSize::List,
        &theme,
    );
    let image_icon = crate::download_progress_view::decorative_file_type_icon_element(
        "vacation-photo-2024.jpg",
        Some("image/jpeg"),
        None,
        crate::file_type_icon::FileTypeIconSize::List,
        &theme,
    );
    let zip_icon = crate::download_progress_view::decorative_file_type_icon_element(
        "VeryLongFileNameThatMightGetClippedByTheContainerOrTruncatedWithEllipsis.zip",
        Some("application/zip"),
        None,
        crate::file_type_icon::FileTypeIconSize::List,
        &theme,
    );

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.pdf_document")))
        .push(
            FileIdentityCell::<AppMessage>::new(
                pdf_icon,
                "QuarterlyReport.pdf",
                "application/pdf · 2.4 MB · shared 3h ago",
            )
            .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.image_file")))
        .push(
            FileIdentityCell::<AppMessage>::new(
                image_icon,
                "vacation-photo-2024.jpg",
                "image/jpeg · 5.1 MB · downloaded yesterday",
            )
            .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.long_name_truncated")))
        .push(
            FileIdentityCell::<AppMessage>::new(
                zip_icon,
                "VeryLongFileNameThatMightGetClippedByTheContainerOrTruncatedWithEllipsis.zip",
                "application/zip · 128 MB",
            )
            .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Peer chip stack gallery ───────────────────────────────────────────

fn peer_chip_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    let few: Vec<&str> = vec!["Alice", "Bob"];
    let many: Vec<&str> = vec!["Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace"];

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.2_peers_no_overflow")))
        .push(PeerChipStack::<AppMessage>::new(few.clone()).build(&theme))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.7_peers_max_3_visible_overflow")))
        .push(
            PeerChipStack::<AppMessage>::new(many)
                .max_visible(3)
                .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.long_names_truncated_to_12_chars")))
        .push(
            PeerChipStack::<AppMessage>::new(vec!["AlexanderTheGreat", "ChristopherColumbus"])
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Metric block gallery ──────────────────────────────────────────────

fn metric_block_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Row::new()
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.files_shared")))
                .push(MetricBlock::<AppMessage>::new("42", "files shared").build(&theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.data_transferred_accented")))
                .push(
                    MetricBlock::<AppMessage>::new("2.4 GB", "transferred")
                        .accent(design_tokens::primary)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.active_peers_success")))
                .push(
                    MetricBlock::<AppMessage>::new("12", "active peers")
                        .accent(design_tokens::color_success)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .into()
}

// ── Loading skeleton gallery ──────────────────────────────────────────

fn loading_skeleton_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.5_row_skeleton_default_56_px_row_height")))
        .push(LoadingSkeleton::<AppMessage>::new(5).build(&theme))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.3_row_skeleton_compact_48_px")))
        .push(
            LoadingSkeleton::<AppMessage>::new(3)
                .row_height(design_tokens::TABLE_ROW_HEIGHT_COMPACT)
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Inline error gallery ──────────────────────────────────────────────

fn inline_error_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.error_message_only")))
        .push(InlineError::new("Transfer failed: hash mismatch.").build(&theme))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.error_with_retry_action")))
        .push(
            InlineError::new("Network error: peer disconnected.")
                .on_retry(AppMessage::Noop)
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Table header row gallery ──────────────────────────────────────────

fn table_header_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.file_table_header_4_columns")))
        .push(
            TableHeaderRow::<AppMessage>::new(vec![
                ("Name", None),
                ("Kind", Some(100.0)),
                ("Size", Some(80.0)),
                ("Shared", Some(100.0)),
            ])
            .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Overflow menu gallery ─────────────────────────────────────────────

fn overflow_menu_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Row::new()
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.normal")))
                .push(OverflowMenu::build(AppMessage::Noop, false, &theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(&crate::i18n::t("gallery.disabled")))
                .push(OverflowMenu::build(AppMessage::Noop, true, &theme))
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .into()
}

// ── Form primitives gallery (UI-RESTYLE-03) ───────────────────────────

fn form_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    // Labels / helper / error text
    let labels = Column::new()
        .push(state_label(&crate::i18n::t("gallery.field_label")))
        .push(crate::form_components::form_label(&crate::i18n::t("gallery.room_name")))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.helper_text")))
        .push(crate::form_components::helper_text(&crate::i18n::t("gallery.alphanumeric_3_40_chars")))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.error_text")))
        .push(crate::form_components::error_text(&crate::i18n::t("gallery.group_name_is_required")))
        .width(Length::FillPortion(1));

    // Labelled text input — default / with value / error
    let text_inputs = Column::new()
        .push(state_label(&crate::i18n::t("gallery.labelled_text_input_default")))
        .push(
            crate::form_components::TextInput::new(&crate::i18n::t("gallery.room_name"),
                "Room name…",
                "",
                |_| AppMessage::Noop,
            )
            .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.with_value_helper")))
        .push(
            crate::form_components::TextInput::new(&crate::i18n::t("gallery.description"),
                "Optional description…",
                "Weekly sync",
                |_| AppMessage::Noop,
            )
            .helper(&crate::i18n::t("gallery.shown_in_the_room_directory"))
            .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.error_state")))
        .push(
            crate::form_components::TextInput::new(&crate::i18n::t("gallery.group_name"),
                "Group name…",
                "",
                |_| AppMessage::Noop,
            )
            .error("Group name is required.")
            .build(),
        )
        .width(Length::FillPortion(1));

    let toggles = Column::new()
        .push(state_label(&crate::i18n::t("gallery.checkbox")))
        .push(crate::form_components::checkbox_field(&crate::i18n::t("gallery.enable_dht_discovery"),
            true,
            |_| AppMessage::Noop,
            None,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.toggle_switch")))
        .push(crate::form_components::toggle_field(
            "Advertise in Directory",
            true,
            |_| AppMessage::Noop,
            None,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.chips_selected_peers")))
        .push(
            Row::new()
                .push(crate::form_components::remove_chip("Alice", Some(AppMessage::Noop)))
                .push(crate::form_components::remove_chip("Bob", None))
                .spacing(design_tokens::SPACE_4),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.selection_summary")))
        .push(crate::form_components::selection_summary(3, "participant"))
        .width(Length::FillPortion(1));

    let selectable_rows = Column::new()
        .push(state_label(&crate::i18n::t("gallery.selectable_peer_list_bordered_panel")))
        .push(
            crate::form_components::peer_list(
                vec![
                    crate::form_components::SelectablePeerRow::new("Alice")
                        .secondary(crate::i18n::t("gallery.abc123"))
                        .selected(true)
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                    crate::form_components::SelectablePeerRow::new("Bob")
                        .secondary(crate::i18n::t("gallery.def456"))
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                    crate::form_components::SelectablePeerRow::new("Carol")
                        .secondary(crate::i18n::t("gallery.7890ab"))
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                ],
                160.0,
                Some(crate::i18n::t("gallery.no_peers_available")),
            ),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.selectablepeerlist_search_chips_summary")))
        .push(
            crate::form_components::SelectablePeerList::new(
                vec![
                    crate::form_components::SelectablePeerRow::new("Alice")
                        .secondary(crate::i18n::t("gallery.abc123"))
                        .selected(true)
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                    crate::form_components::SelectablePeerRow::new("Bob")
                        .secondary(crate::i18n::t("gallery.def456"))
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                ],
                120.0,
                Some(crate::i18n::t("gallery.no_peers_available")),
            )
            .search("Search participants…", "", |_| AppMessage::Noop)
            .chips(vec![crate::form_components::remove_chip(
                "Alice",
                Some(AppMessage::Noop),
            )])
            .summary(1, "participant")
            .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.dialog_footer")))
        .push(
            crate::form_components::DialogFooter::new()
                .cancel("Cancel", AppMessage::Noop)
                .confirm("Create", AppMessage::Noop)
                .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label(&crate::i18n::t("gallery.destructive_button")))
        .push(crate::form_components::destructive_button(
            "Remove",
            Some(AppMessage::Noop),
            false,
        ))
        .width(Length::FillPortion(1));

    // Form section wrapping a couple of fields
    let section = crate::form_components::FormSection::new(&crate::i18n::t("gallery.room_details"))
        .helper(&crate::i18n::t("gallery.these_settings_control_who_can_find_and_join_the"))
        .push(
            crate::form_components::TextInput::new(&crate::i18n::t("gallery.room_name"),
                "Room name…",
                "Design Sync",
                |_| AppMessage::Noop,
            )
            .build(),
        )
        .push(crate::form_components::checkbox_field(&crate::i18n::t("gallery.advertise_in_directory"),
            true,
            |_| AppMessage::Noop,
            None,
        ))
        .build();

    Column::new()
        .push(state_label(&crate::i18n::t("gallery.form_section")))
        .push(section)
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
        .push(
            Row::new()
                .push(labels)
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(text_inputs)
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(toggles)
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(selectable_rows)
                .align_y(Alignment::Start),
        )
        .spacing(0)
        .into()
}

// ── Typography preview (UI-HOME-11) ───────────────────────────────────

/// Sample copy rendered for every canonical semantic role, plus a
/// registered-weight demo for each bundled family. `btheme` is the live
/// merged theme so samples reflect inspector typography edits (BORU-UI-16).
fn typography_gallery(btheme: &crate::theme::BoruTheme) -> Element<'static, AppMessage> {
    let sample = |role: TypeRole| -> Element<'static, AppMessage> {
        let sample_text: &'static str = match role {
            TypeRole::DisplayHeading => "Good evening, Ada — welcome back",
            TypeRole::PageTitle => "Boru Settings",
            TypeRole::SectionTitle => "Connection Overview",
            TypeRole::CardTitle => "Mesh Health",
            TypeRole::Body => "Body copy and descriptions use Public Sans at fifteen pixels.",
            TypeRole::BodyEmphasised => {
                "Emphasised body copy stands out without synthetic bolding."
            }
            TypeRole::ButtonLabel => "Create Room",
            TypeRole::SupportingText => "Supporting text adds context at thirteen pixels.",
            TypeRole::Metadata => "12 min ago · 3 peers",
            TypeRole::ChatMessage => "Figtree carries the conversation.",
            TypeRole::ChatSender => "Ada Lovelace",
            TypeRole::ChatMetadata => "14:32 · Delivered",
            TypeRole::ComposerText => "Type a message…",
            TypeRole::TechnicalValue => "12D3KooW…c7f8 · 127.0.0.1:8765",
            TypeRole::BrandWordmark => "BORU",
        };
        let family_weight = match role {
            TypeRole::DisplayHeading => "Inter Tight Bold 700",
            TypeRole::PageTitle => "Inter Tight Bold 700",
            TypeRole::SectionTitle => "Public Sans SemiBold 600",
            TypeRole::CardTitle => "Public Sans SemiBold 600",
            TypeRole::Body => "Public Sans Regular 400",
            TypeRole::BodyEmphasised => "Public Sans SemiBold 600",
            TypeRole::ButtonLabel => "Public Sans SemiBold 600",
            TypeRole::SupportingText => "Public Sans Regular 400",
            TypeRole::Metadata => "Public Sans Regular 400",
            TypeRole::ChatMessage => "Figtree Regular 400",
            TypeRole::ChatSender => "Figtree SemiBold 600",
            TypeRole::ChatMetadata => "Figtree Regular 400",
            TypeRole::ComposerText => "Figtree Regular 400",
            TypeRole::TechnicalValue => "JetBrains Mono Regular 400",
            TypeRole::BrandWordmark => "Raleway ExtraBold 800",
        };
        let caption = format!(
            "{}  ·  {}  ·  {}px  ·  {}",
            role.label(),
            btheme.typography.family_for(role).name(),
            btheme.typography.size_for(role) as u32,
            family_weight
        );
        Column::new()
            .push(
                crate::fonts::type_role_text_themed(btheme, role, sample_text)
                    .color(design_tokens::text_primary(&Theme::Light)),
            )
            .push(
                text(caption)
                    .size(TypeRole::Metadata.size_px())
                    .color(design_tokens::text_muted(&Theme::Light)),
            )
            .spacing(design_tokens::SPACE_2)
            .into()
    };

    let role_rows = TypeRole::ALL.iter().fold(
        Column::new().spacing(design_tokens::SPACE_12),
        |col, role| col.push(sample(*role)),
    );

    // Real-weight demo: every registered static weight per family, so the
    // gallery visually proves there is no synthetic bolding.
    let weight_sample = |family: &'static str,
                         weight: iced::font::Weight,
                         label: &'static str|
     -> Element<'static, AppMessage> {
        let font = iced::Font {
            family: iced::font::Family::Name(family),
            weight,
            stretch: iced::font::Stretch::Normal,
            style: iced::font::Style::Normal,
        };
        text(label)
            .font(font)
            .size(16.0)
            .color(design_tokens::text_primary(&Theme::Light))
            .into()
    };
    let family_row = |name: &'static str| -> Element<'static, AppMessage> {
        text(name)
            .size(TypeRole::Metadata.size_px())
            .color(design_tokens::text_muted(&Theme::Light))
            .into()
    };
    let weights_demo = Column::new()
        .spacing(design_tokens::SPACE_8)
        .push(family_row("Figtree"))
        .push(
            Row::new()
                .spacing(design_tokens::SPACE_12)
                .push(weight_sample("Figtree", iced::font::Weight::Normal, "400"))
                .push(weight_sample("Figtree", iced::font::Weight::Medium, "500"))
                .push(weight_sample("Figtree", iced::font::Weight::Semibold, "600")),
        )
        .push(family_row("Raleway"))
        .push(
            Row::new()
                .spacing(design_tokens::SPACE_12)
                .push(weight_sample("Raleway", iced::font::Weight::ExtraBold, "800")),
        )
        .push(family_row("JetBrains Mono"))
        .push(
            Row::new()
                .spacing(design_tokens::SPACE_12)
                .push(weight_sample("JetBrains Mono", iced::font::Weight::Normal, "400"))
                .push(weight_sample("JetBrains Mono", iced::font::Weight::Medium, "500")),
        )
        .push(family_row("Archivo SemiCondensed (FONTS-04)"))
        .push(
            Row::new()
                .spacing(design_tokens::SPACE_12)
                .push(weight_sample(
                    "Archivo SemiCondensed",
                    iced::font::Weight::Semibold,
                    "600",
                ))
                .push(weight_sample(
                    "Archivo SemiCondensed",
                    iced::font::Weight::Bold,
                    "700",
                )),
        )
        .push(family_row("IBM Plex Sans (FONTS-04)"))
        .push(
            Row::new()
                .spacing(design_tokens::SPACE_12)
                .push(weight_sample("IBM Plex Sans", iced::font::Weight::Normal, "400"))
                .push(weight_sample("IBM Plex Sans", iced::font::Weight::Medium, "500"))
                .push(weight_sample("IBM Plex Sans", iced::font::Weight::Semibold, "600")),
        );

    let fallback_note = text(format!(
        "Fallbacks (FONTS-14): display/page → Arial Narrow → sans-serif; UI/chat → system sans-serif; {} → monospace; brand → Raleway.",
        "technical_value"
    ))
    .size(TypeRole::Metadata.size_px())
    .color(design_tokens::text_muted(&Theme::Light));

    let fallback_sample = |role: TypeRole, label: &'static str| -> Element<'static, AppMessage> {
        text(label)
            .font(role.fallback_font())
            .size(role.size_px())
            .color(design_tokens::text_muted(&Theme::Light))
            .into()
    };
    let fallback_demo = Row::new()
        .spacing(design_tokens::SPACE_16)
        .push(fallback_sample(
            TypeRole::DisplayHeading,
            "Fallback display_heading → Arial Narrow → sans-serif",
        ))
        .push(fallback_sample(
            TypeRole::TechnicalValue,
            "Fallback technical_value → monospace",
        ));

    Column::new()
        .spacing(design_tokens::SPACE_12)
        .push(role_rows)
        .push(horizontal(1))
        .push(state_label(&crate::i18n::t("gallery.registered_weights_no_synthetic_bolding")))
        .push(weights_demo)
        .push(horizontal(1))
        .push(state_label(&crate::i18n::t("gallery.fallback_demo")))
        .push(fallback_demo)
        .push(fallback_note)
        .into()
}

// ── Names: short & long (PDF Task 14) ────────────────────────────────

/// Short and long usernames and room names rendered through the production
/// `Avatar`, `ListRow`, `CardShell` and `PeerChipStack` components.
fn name_variants_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    // Short vs long usernames in the production avatar + list row.
    let short_avatar: Element<'static, AppMessage> = Avatar::<AppMessage>::new("Ada")
        .size(design_tokens::AVATAR_SM)
        .build();
    let long_avatar: Element<'static, AppMessage> = Avatar::<AppMessage>::new(
        "Alexandrina von Hohenzollern-Sigmaringen",
    )
    .size(design_tokens::AVATAR_SM)
    .build();

    // Short vs long room names in the production CardShell header.
    let short_room = CardShell::new("Lobby", vec![])
        .count(3)
        .empty_message("No one is here yet.")
        .build(&theme);
    let long_room = CardShell::new(
        "The Very Long Room Name That Keeps Going And Going For Testing Purposes",
        vec![],
    )
    .count(12)
    .empty_message("A very long room name still fits.")
    .build(&theme);

    Column::new()
        .push(state_label("Short username (Avatar + ListRow)"))
        .push(
            ListRow::<AppMessage>::new("Ada")
                .leading(short_avatar)
                .secondary("Online")
                .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Long username (Avatar + ListRow)"))
        .push(
            ListRow::<AppMessage>::new("Alexandrina von Hohenzollern-Sigmaringen")
                .leading(long_avatar)
                .secondary("Last seen 2m ago")
                .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Short room name (CardShell)"))
        .push(short_room)
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Long room name (CardShell)"))
        .push(long_room)
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Long peer names (PeerChipStack truncates to 12 chars)"))
        .push(
            PeerChipStack::<AppMessage>::new(vec![
                "Alexandrina von Hohenzollern-Sigmaringen",
                "Maximiliana-Theresia van der Berg",
            ])
            .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Message bubbles (PDF Task 14) ─────────────────────────────────────

/// A representative message bubble composed from the exact production style
/// functions `view_chat_log` uses (`design_tokens::bubble_bg`,
/// `bubble_border`, `TypeRole` fonts, `Avatar`, `delivery_label`). This is
/// not a mock — every style token and font role is the production one.
fn message_bubble(
    label: &str,
    body: &str,
    is_local: bool,
    failed: bool,
    delivery: Option<&boru_core::chat_history::DeliveryState>,
) -> Element<'static, AppMessage> {
    let theme = Theme::Light;
    let label_color = if is_local {
        design_tokens::text_local_label(&theme)
    } else {
        design_tokens::text_remote_label(&theme)
    };
    let body_color = if is_local {
        design_tokens::text_local_body(&theme)
    } else {
        design_tokens::text_remote_body(&theme)
    };

    let label_el = text(label.to_string())
        .font(TypeRole::ChatSender.font())
        .size(TypeRole::ChatSender.size_px())
        .color(label_color);
    let body_el = text(body.to_string())
        .font(TypeRole::ChatMessage.font())
        .size(TypeRole::ChatMessage.size_px())
        .line_height(iced::widget::text::LineHeight::Relative(1.45))
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
        .color(body_color);
    let ts = if let Some(state) = delivery {
        format!(
            "14:32 · {}",
            crate::presentation::delivery_label(state)
        )
    } else {
        "14:32".to_string()
    };
    let ts_el = text(ts)
        .font(TypeRole::ChatMetadata.font())
        .size(TypeRole::ChatMetadata.size_px())
        .color(design_tokens::text_muted(&theme));

    let bubble = container(body_el)
        .padding([design_tokens::SPACE_10, design_tokens::SPACE_16])
        .style(move |t| {
            let mut s = iced::widget::container::Style {
                border: design_tokens::bubble_border(t, is_local, false, failed)
                    .unwrap_or_default(),
                ..Default::default()
            };
            if let Some(bg) = design_tokens::bubble_bg(t, is_local, false) {
                s.background = Some(bg);
            }
            s
        });

    let avatar: Element<'static, AppMessage> = Avatar::<AppMessage>::new(label)
        .size(design_tokens::AVATAR_MSG)
        .build();

    let col = Column::new()
        .push(label_el)
        .push(bubble)
        .push(ts_el)
        .spacing(design_tokens::SPACE_2)
        .max_width(crate::presentation::chat_bubble_max_width(640.0))
        .align_x(if is_local {
            iced::Alignment::End
        } else {
            iced::Alignment::Start
        });

    let row = if is_local {
        Row::new().push(col).push(avatar).spacing(design_tokens::SPACE_8)
    } else {
        Row::new().push(avatar).push(col).spacing(design_tokens::SPACE_8)
    };
    row.width(Length::Fill).into()
}

fn message_bubble_gallery() -> Element<'static, AppMessage> {
    use boru_core::chat_history::DeliveryState;

    Column::new()
        .push(state_label("Incoming — short name"))
        .push(message_bubble(
            "Ada",
            "Hey, are you free for a quick call?",
            false,
            false,
            None,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Incoming — long name (wraps)"))
        .push(message_bubble(
            "Alexandrina von Hohenzollern-Sigmaringen",
            "The architecture review notes are ready — I dropped them in the shared folder.",
            false,
            false,
            None,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Outgoing — sent"))
        .push(message_bubble(
            "You",
            "Sounds good, send them over.",
            true,
            false,
            Some(&DeliveryState::Sent),
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Outgoing — delivered"))
        .push(message_bubble(
            "You",
            "Actually, let's do it after lunch.",
            true,
            false,
            Some(&DeliveryState::Delivered),
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Outgoing — read (Seen)"))
        .push(message_bubble(
            "You",
            "👍",
            true,
            false,
            Some(&DeliveryState::Seen),
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Outgoing — failed (danger border)"))
        .push(message_bubble(
            "You",
            "Did you get the file?",
            true,
            true,
            Some(&DeliveryState::Failed),
        ))
        .spacing(0)
        .into()
}

// ── Attachment states (PDF Task 14) ───────────────────────────────────

/// Build a production `DownloadAttachment` fixture for a given state.
fn attachment_fixture(
    kind: TransferKind,
    name: &str,
    state: DownloadState,
) -> DownloadAttachment {
    let mut att = DownloadAttachment::new(kind, name, "", "Ada", None);
    att.state = state;
    att
}

/// Render one production download card inside a labelled frame.
///
/// BORU-LAYOUT-09: `placement` comes from the selected gallery layout
/// preset's `component.video_card` (the same leaf the real chat log uses),
/// so the card preview responds to layout configuration; `timeline_width`
/// is the simulated preview width so the card also responds to the width
/// preset (Compact vs Full control layouts).
fn attachment_card(
    label: &str,
    attachment: &DownloadAttachment,
    placement: ComponentPlacement,
    timeline_width: f32,
) -> Element<'static, AppMessage> {
    let card = view_download_progress(
        0,
        attachment,
        false,
        false,
        Some(1_752_000_000_000),
        timeline_width,
        placement,
    );
    Column::new()
        .push(state_label(label))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(card)
        .width(Length::Fill)
        .spacing(0)
        .into()
}

fn attachment_states_gallery(
    placement: ComponentPlacement,
    timeline_width: f32,
) -> Element<'static, AppMessage> {
    let pending = attachment_fixture(
        TransferKind::File,
        "QuarterlyReport.pdf",
        DownloadState::Ready { total: Some(2_415_888) },
    );
    let downloading = attachment_fixture(
        TransferKind::File,
        "QuarterlyReport.pdf",
        DownloadState::Active {
            bytes: 1_200_000,
            total: Some(2_415_888),
        },
    );
    let downloaded = attachment_fixture(
        TransferKind::File,
        "QuarterlyReport.pdf",
        DownloadState::Completed {
            saved_name: "QuarterlyReport.pdf".to_string(),
            saved_path: Some(std::path::PathBuf::from("/home/ada/Downloads")),
            total_size: Some(2_415_888),
        },
    );
    let error = attachment_fixture(
        TransferKind::File,
        "QuarterlyReport.pdf",
        DownloadState::Failed {
            failure: DownloadFailure::VerificationFailed {
                attempts: 3,
                max_attempts: 3,
                detail: Some("hash mismatch".to_string()),
            },
        },
    );
    let image_pending = attachment_fixture(
        TransferKind::Image,
        "vacation-photo-2024.jpg",
        DownloadState::Ready { total: Some(5_120_000) },
    );
    let image_downloading = attachment_fixture(
        TransferKind::Image,
        "vacation-photo-2024.jpg",
        DownloadState::Active {
            bytes: 3_300_000,
            total: Some(5_120_000),
        },
    );
    let image_downloaded = attachment_fixture(
        TransferKind::Image,
        "vacation-photo-2024.jpg",
        DownloadState::Completed {
            saved_name: "vacation-photo-2024.jpg".to_string(),
            saved_path: Some(std::path::PathBuf::from("/home/ada/Downloads")),
            total_size: Some(5_120_000),
        },
    );

    Column::new()
        .push(attachment_card("File — pending (Ready)", &pending, placement, timeline_width))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(attachment_card(
            "File — downloading (Active)",
            &downloading,
            placement,
            timeline_width,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(attachment_card(
            "File — downloaded (Completed)",
            &downloaded,
            placement,
            timeline_width,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(attachment_card("File — error (Failed)", &error, placement, timeline_width))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(attachment_card(
            "Image — pending",
            &image_pending,
            placement,
            timeline_width,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(attachment_card(
            "Image — downloading",
            &image_downloading,
            placement,
            timeline_width,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(attachment_card(
            "Image — downloaded",
            &image_downloaded,
            placement,
            timeline_width,
        ))
        .spacing(0)
        .into()
}

// ── Video cards & aspect ratios (PDF Task 14) ─────────────────────────

/// Build a video attachment with explicit poster dimensions (aspect ratio).
fn video_fixture(
    name: &str,
    poster: (u32, u32),
    state: DownloadState,
) -> DownloadAttachment {
    let mut att = attachment_fixture(TransferKind::Video, name, state);
    att.poster_dimensions = Some(poster);
    att
}

fn video_card_gallery(
    placement: ComponentPlacement,
    timeline_width: f32,
) -> Element<'static, AppMessage> {
    let ready_16_9 = video_fixture(
        "presentation-recording.mp4",
        (1920, 1080),
        DownloadState::Completed {
            saved_name: "presentation-recording.mp4".to_string(),
            saved_path: Some(std::path::PathBuf::from("/home/ada/Downloads")),
            total_size: Some(48_000_000),
        },
    );
    let ready_square = video_fixture(
        "square-demo.mp4",
        (1080, 1080),
        DownloadState::Completed {
            saved_name: "square-demo.mp4".to_string(),
            saved_path: Some(std::path::PathBuf::from("/home/ada/Downloads")),
            total_size: Some(12_000_000),
        },
    );
    let ready_vertical = video_fixture(
        "vertical-short.mp4",
        (1080, 1920),
        DownloadState::Completed {
            saved_name: "vertical-short.mp4".to_string(),
            saved_path: Some(std::path::PathBuf::from("/home/ada/Downloads")),
            total_size: Some(8_000_000),
        },
    );
    let downloading = video_fixture(
        "presentation-recording.mp4",
        (1920, 1080),
        DownloadState::Active {
            bytes: 22_000_000,
            total: Some(48_000_000),
        },
    );
    let error = video_fixture(
        "presentation-recording.mp4",
        (1920, 1080),
        DownloadState::Failed {
            failure: DownloadFailure::PeerOffline {
                detail: Some("peer unreachable".to_string()),
            },
        },
    );

    Column::new()
        .push(state_label("16:9 — ready to play"))
        .push(attachment_card("16:9 (1920×1080)", &ready_16_9, placement, timeline_width))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Square — ready to play"))
        .push(attachment_card(
            "Square (1080×1080)",
            &ready_square,
            placement,
            timeline_width,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Vertical — ready to play"))
        .push(attachment_card(
            "Vertical (1080×1920)",
            &ready_vertical,
            placement,
            timeline_width,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Downloading (progress)"))
        .push(attachment_card(
            "Downloading",
            &downloading,
            placement,
            timeline_width,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Error (failed)"))
        .push(attachment_card("Error", &error, placement, timeline_width))
        .spacing(0)
        .into()
}

// ── Available widths (PDF Task 14) ────────────────────────────────────

/// Render one component at a fixed width to demonstrate narrow/normal/wide.
fn width_frame(
    label: &str,
    width: f32,
    inner: Element<'static, AppMessage>,
) -> Element<'static, AppMessage> {
    Column::new()
        .push(state_label(label))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(
            container(inner)
                .width(Length::Fixed(width))
                .style(design_tokens::card_style),
        )
        .width(Length::Fill)
        .spacing(0)
        .into()
}

fn width_variants_gallery() -> Element<'static, AppMessage> {
    use boru_core::chat_history::DeliveryState;

    // iced Elements are not Clone — rebuild per frame with the same inputs.
    let bubble_at = |width: f32| {
        message_bubble(
            "Ada",
            "This message demonstrates how the bubble behaves when the available column width changes.",
            false,
            false,
            Some(&DeliveryState::Delivered),
        )
    };
    let shell_at = || {
        CardShell::new(
            "Online Peers",
            vec![
                card_shell_row("Alice", "online"),
                card_shell_row("Bob", "idle"),
                card_shell_row("Carol", "online"),
            ],
        )
        .count(3)
        .build(&Theme::Light)
    };

    Column::new()
        .push(width_frame(
            "Narrow — 320 px (message bubble)",
            320.0,
            bubble_at(320.0),
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(width_frame(
            "Normal — 640 px (message bubble)",
            640.0,
            bubble_at(640.0),
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(width_frame(
            "Wide — 1024 px (message bubble)",
            1024.0,
            bubble_at(1024.0),
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(width_frame("Narrow — 320 px (card shell)", 320.0, shell_at()))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(width_frame("Normal — 640 px (card shell)", 640.0, shell_at()))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(width_frame("Wide — 1024 px (card shell)", 1024.0, shell_at()))
        .spacing(0)
        .into()
}

// ── State variants (PDF Task 14) ──────────────────────────────────────

fn state_variants_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    let dot_online: Element<'static, AppMessage> = status_dot(StatusDotKind::Online, 10.0);
    let unread_badge: Element<'static, AppMessage> = badge("4", BadgeKind::Count);
    let danger_badge: Element<'static, AppMessage> = badge("Error", BadgeKind::Danger);

    Column::new()
        .push(state_label("Selected (ListRow)"))
        .push(
            ListRow::<AppMessage>::new("Selected conversation")
                .secondary("Selected row example")
                .selected(true)
                .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Unread (ListRow with count badge)"))
        .push(
            ListRow::<AppMessage>::new("Unread conversation")
                .leading(dot_online)
                .secondary("New message preview…")
                .trailing(unread_badge)
                .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Disabled (buttons)"))
        .push(
            Row::new()
                .push(primary_button("Disabled primary", None, true))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_12))
                        .height(Length::Shrink),
                )
                .push(secondary_button("Disabled secondary", None, true))
                .spacing(0),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Error (InlineError with retry)"))
        .push(
            InlineError::new("Transfer failed: hash mismatch.")
                .on_retry(AppMessage::Noop)
                .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Error (danger badge)"))
        .push(danger_badge)
        .spacing(0)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PDF Task 14 smoke test: every gallery section must build its element
    /// tree without panicking. Building the tree is pure construction (no
    /// renderer, no layout) so this is a cheap guard against regressions in
    /// the fixture builders and production-component wiring.
    ///
    /// BORU-LAYOUT-09: the gallery also builds under every layout-config
    /// preset, since each preset changes the concrete `LayoutConfig` the
    /// sections are threaded with.
    #[test]
    fn all_gallery_sections_build() {
        let btheme = crate::theme::BoruTheme::default();
        for layout_preset in GalleryLayoutPreset::ALL {
            let state = GalleryState {
                layout_preset,
                ..GalleryState::default()
            };
            let _ = view_gallery(&state, 1280.0, &btheme);
        }
        let layout = GalleryLayoutPreset::Desktop.layout_config();
        let _ = gallery_sections(&btheme, &layout, 960.0);
        let _ = name_variants_gallery();
        let _ = message_bubble_gallery();
        let _ = attachment_states_gallery(layout.component.video_card, 960.0);
        let _ = video_card_gallery(layout.component.video_card, 960.0);
        let _ = width_variants_gallery();
        let _ = state_variants_gallery();
    }

    /// BORU-LAYOUT-09 / PDF Task 9: every layout preset resolves to a
    /// concrete `LayoutConfig` — through the same merge path
    /// `boru-layout.toml` uses — with the expected structural leaves.
    /// `Default` reproduces the current appearance exactly.
    #[test]
    fn gallery_layout_presets_map_to_expected_configs() {
        assert_eq!(
            GalleryLayoutPreset::Default.layout_config(),
            LayoutConfig::default(),
            "Default preset is the current appearance"
        );

        // Narrow: list mode, single column at every width, tight gaps and
        // the Tunnels section hidden.
        let narrow = GalleryLayoutPreset::Narrow.layout_config();
        assert_eq!(narrow.home.mode, HomeLayoutMode::List);
        assert_eq!(
            narrow.responsive.home_columns,
            ByTier {
                narrow: 1,
                desktop: 1,
                ultra_wide: 1,
            }
        );
        assert_eq!(narrow.home.gaps.card_gap, 12.0);
        assert!(narrow.home.hidden_sections.contains(&HomeSection::Tunnels));
        assert_eq!(narrow.responsive.home_columns_for_width(1440.0), 1);

        // Desktop: two-column grid at desktop+ widths and a side-placement
        // video card (thumbnail left, horizontal, side buttons).
        let desktop = GalleryLayoutPreset::Desktop.layout_config();
        assert_eq!(desktop.home.mode, HomeLayoutMode::Grid);
        assert_eq!(desktop.responsive.home_columns_for_width(960.0), 2);
        assert_eq!(
            desktop.component.video_card.thumbnail_position,
            ThumbnailPosition::Left
        );
        assert_eq!(
            desktop.component.video_card.card_orientation,
            CardOrientation::Horizontal
        );
        assert_eq!(
            desktop.component.video_card.button_placement,
            ButtonPlacement::Side
        );

        // Maximized: wider responsive padding at ultra-wide widths and a
        // three-column ultra-wide tier.
        let maximized = GalleryLayoutPreset::Maximized.layout_config();
        assert_eq!(maximized.responsive.home_padding_x_for_width(1440.0), 40.0);
        assert_eq!(maximized.responsive.home_columns_for_width(1440.0), 3);
    }

    /// BORU-LAYOUT-09: the Layout Preview mini-home builds without
    /// panicking for every layout preset at every width preset (narrow,
    /// desktop, maximized) — the acceptance criteria's "every reusable
    /// component previews under different layout configurations".
    #[test]
    fn layout_preview_builds_for_every_preset_and_width() {
        for layout_preset in GalleryLayoutPreset::ALL {
            let layout = layout_preset.layout_config();
            for width in [360.0, 960.0, 1440.0] {
                let _ = layout_preview_gallery(&layout, width);
            }
        }
    }

    /// BORU-LAYOUT-09: the main/rail split keeps the trailing sections
    /// (PeopleActivity, Tunnels) in the rail and everything before them in
    /// the main column, mirroring `app/home.rs`.
    #[test]
    fn home_section_split_matches_home_rail() {
        let order = vec![
            HomeSection::Hero,
            HomeSection::MeshHealth,
            HomeSection::QuickActions,
            HomeSection::PeopleActivity,
            HomeSection::Tunnels,
        ];
        let (main, rail) = split_home_sections(&order);
        assert_eq!(
            main,
            vec![
                HomeSection::Hero,
                HomeSection::MeshHealth,
                HomeSection::QuickActions,
            ]
        );
        assert_eq!(rail, vec![HomeSection::PeopleActivity, HomeSection::Tunnels]);

        // One or two sections: everything in the main column, empty rail.
        let (main, rail) = split_home_sections(&[
            HomeSection::Hero,
            HomeSection::MeshHealth,
        ]);
        assert_eq!(main, vec![HomeSection::Hero, HomeSection::MeshHealth]);
        assert!(rail.is_empty());
    }

    /// BORU-UI-15: the responsive preview presets map to the three required
    /// simulated widths — narrow panel, typical desktop, maximized/wide.
    #[test]
    fn gallery_presets_map_to_required_widths() {
        let mut state = GalleryState::default();
        state.preset = GalleryWidthPreset::Narrow;
        assert_eq!(state.width(), 360.0);
        state.preset = GalleryWidthPreset::Desktop;
        assert_eq!(state.width(), 960.0);
        state.preset = GalleryWidthPreset::Maximized;
        assert_eq!(state.width(), 1440.0);
    }

    /// BORU-UI-15: the custom slider drives the width and out-of-range
    /// values are clamped to the slider range so layout never sees a
    /// nonsense width.
    #[test]
    fn gallery_custom_width_clamps_to_slider_range() {
        let state = GalleryState {
            preset: GalleryWidthPreset::Custom,
            custom_width: 5000.0,
            ..GalleryState::default()
        };
        assert_eq!(state.width(), CUSTOM_WIDTH_MAX);

        let state = GalleryState {
            preset: GalleryWidthPreset::Custom,
            custom_width: 1.0,
            ..GalleryState::default()
        };
        assert_eq!(state.width(), CUSTOM_WIDTH_MIN);

        let state = GalleryState {
            preset: GalleryWidthPreset::Custom,
            custom_width: 777.0,
            ..GalleryState::default()
        };
        assert_eq!(state.width(), 777.0);
    }

    /// BORU-UI-15: wide presets are bounded by the available window so the
    /// simulated frame never overflows horizontally, while narrow widths
    /// pass through untouched.
    #[test]
    fn gallery_effective_width_bounds_to_window() {
        // Desktop (960) on a 1200 px window: fits, minus the frame gutter.
        let state = GalleryState::default();
        assert_eq!(effective_preview_width(&state, 1200.0), 960.0);

        // Maximized (1440) on a 1200 px window: clamped below the window.
        let state = GalleryState {
            preset: GalleryWidthPreset::Maximized,
            custom_width: 0.0,
            ..GalleryState::default()
        };
        assert!(effective_preview_width(&state, 1200.0) < 1200.0);
        assert_eq!(effective_preview_width(&state, 1200.0), 1104.0);

        // A zero/unknown window width falls back to a sane default.
        let state = GalleryState::default();
        assert_eq!(effective_preview_width(&state, 0.0), 960.0);
    }

    /// The attachment fixtures map to the production `DownloadState` variants
    /// the PDF requires: pending (Ready), downloading (Active), downloaded
    /// (Completed), error (Failed).
    #[test]
    fn attachment_fixture_states_cover_required_set() {
        let pending = attachment_fixture(
            TransferKind::File,
            "a.pdf",
            DownloadState::Ready { total: Some(10) },
        );
        assert!(matches!(pending.state, DownloadState::Ready { .. }));

        let downloading = attachment_fixture(
            TransferKind::File,
            "a.pdf",
            DownloadState::Active {
                bytes: 5,
                total: Some(10),
            },
        );
        assert!(matches!(downloading.state, DownloadState::Active { .. }));

        let downloaded = attachment_fixture(
            TransferKind::File,
            "a.pdf",
            DownloadState::Completed {
                saved_name: "a.pdf".into(),
                saved_path: None,
                total_size: Some(10),
            },
        );
        assert!(matches!(downloaded.state, DownloadState::Completed { .. }));

        let error = attachment_fixture(
            TransferKind::File,
            "a.pdf",
            DownloadState::Failed {
                failure: DownloadFailure::VerificationFailed {
                    attempts: 3,
                    max_attempts: 3,
                    detail: None,
                },
            },
        );
        assert!(matches!(error.state, DownloadState::Failed { .. }));
    }

    /// Video fixtures carry poster dimensions for the three required aspect
    /// ratios: 16:9, square and vertical.
    #[test]
    fn video_fixtures_cover_aspect_ratios() {
        let widescreen = video_fixture("w.mp4", (1920, 1080), DownloadState::Ready { total: None });
        assert_eq!(widescreen.poster_dimensions, Some((1920, 1080)));

        let square = video_fixture("s.mp4", (1080, 1080), DownloadState::Ready { total: None });
        assert_eq!(square.poster_dimensions, Some((1080, 1080)));

        let vertical = video_fixture("v.mp4", (1080, 1920), DownloadState::Ready { total: None });
        assert_eq!(vertical.poster_dimensions, Some((1080, 1920)));
    }
}
