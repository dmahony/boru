//! LayoutConfig — typed structural layout model for the Boru desktop UI.
//!
//! BORU-LAYOUT-01 / PDF Task 1 of the Live Layout (TOML) chain: separates
//! **structural layout** from **visual styling**. [`BoruTheme`](crate::theme::BoruTheme)
//! stays purely visual (colours, typography, radii, icon/avatar sizes, border
//! widths); this module owns *arrangement* — section order/visibility,
//! grid/list modes, column counts, max content widths, padding/gaps, card
//! sizing and per-component placement (thumbnail position, metadata alignment,
//! button placement, card orientation).
//!
//! ## Design rules
//!
//! - **Defaults = current appearance.** Every leaf's `Default` reproduces the
//!   baseline from `design_tokens.rs` / `theme.rs` / view code (audited in
//!   `docs/live-layout/layout-audit.md`), so the UI is unchanged when no
//!   `boru-layout.toml` is present. Later tasks layer TOML overrides on top of
//!   [`LayoutConfig::default`] exactly like `theme_config.rs` does for
//!   [`BoruTheme`](crate::theme::BoruTheme).
//! - **No theme tokens for layout.** Layout values are structural; colours,
//!   typography, radii, icon/avatar sizes and motion counts stay in `theme.rs`.
//!   Nothing in this module reads `BoruTheme`.
//! - **Copy/Clone leaf structs** mirror the `theme.rs` organisation so view
//!   code can pass groups by value; the root is Clone-only because the
//!   future-screens extension point is a map.
//! - **Extension point.** [`LayoutConfig::screens`] reserves per-screen layout
//!   groups for future screens keyed by a stable screen id (PDF Task 2:
//!   "typed structs for Home, Sidebar, Chat and future screens").
//!
//! ## Status
//!
//! Skeleton only — intentionally NOT wired into views yet (BORU-LAYOUT-02/03
//! define the full schema and integrate it). `#![allow(dead_code)]` guards the
//! unwired model until then; remove it once views consume `LayoutConfig`.

#![allow(dead_code)] // unwired skeleton; drop once BORU-LAYOUT-03+ wires views to LayoutConfig

use std::collections::BTreeMap;

// ── Root ─────────────────────────────────────────────────────────────

/// Root of the structural layout model. `Default` reproduces the current
/// arrangement exactly; a later `layout_merge` layer (BORU-LAYOUT-03) will
/// apply partial `boru-layout.toml` overrides onto it.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutConfig {
    /// Home dashboard (PDF Task 3): section order/visibility, grid/list mode,
    /// column counts, max content width, padding, gaps, card sizing.
    pub home: HomeLayout,
    /// Sidebar shell: width, section order/visibility, padding, row heights.
    pub sidebar: SidebarLayout,
    /// Chat screen: bubble/message widths, picker sizes, composer layout,
    /// detail panel width.
    pub chat: ChatLayout,
    /// Per-component placement (PDF Task 5): thumbnail position, metadata
    /// alignment, button placement, card orientation, media-card sizing.
    pub component: ComponentLayout,
    /// Data-table column widths (files dashboard, "Files I'm Sharing").
    pub tables: TablesLayout,
    /// Responsive breakpoints (PDF Task 4): viewport tiers and the
    /// content-width thresholds that switch column counts and stacking.
    pub responsive: ResponsiveLayout,
    /// Extension point for future screens. Keyed by a stable screen id
    /// (e.g. `"settings"`, `"files"`); empty today. Future tasks register a
    /// [`ScreenLayout`] per screen here and the view layer consults it.
    pub screens: BTreeMap<String, ScreenLayout>,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            home: HomeLayout::default(),
            sidebar: SidebarLayout::default(),
            chat: ChatLayout::default(),
            component: ComponentLayout::default(),
            tables: TablesLayout::default(),
            responsive: ResponsiveLayout::default(),
            screens: BTreeMap::new(),
        }
    }
}

// ── Home dashboard (PDF Task 3) ──────────────────────────────────────

/// Stable identity of a home-dashboard section. Baseline order matches
/// `app/home.rs` `view_chat_list_content`: left column Hero → MeshHealth →
/// QuickActions, right rail PeopleActivity → Tunnels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HomeSection {
    /// Large connection status card (`status_card.rs`).
    Hero,
    /// Mesh Health card.
    MeshHealth,
    /// Quick-action card grid (`quick_actions.rs`).
    QuickActions,
    /// "People & Activity" card (online peers + recent activity).
    PeopleActivity,
    /// Tunnels card.
    Tunnels,
}

/// Home dashboard structural layout.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeLayout {
    /// Vertical section order (top→bottom; left column first, then the right
    /// rail in two-column mode). Baseline: Hero, MeshHealth, QuickActions,
    /// PeopleActivity, Tunnels.
    pub section_order: Vec<HomeSection>,
    /// Sections hidden entirely from the dashboard. Empty = all visible.
    pub hidden_sections: Vec<HomeSection>,
    /// Grid vs list presentation.
    pub mode: HomeLayoutMode,
    /// Dashboard grid column split and stacking rule.
    pub grid: HomeGrid,
    /// Quick-action card grid columns per width tier.
    pub quick_actions: QuickActionsLayout,
    /// Max dashboard canvas width (`DASHBOARD_MAX_WIDTH` = 1480 px).
    pub max_content_width: f32,
    /// Padding around the dashboard canvas.
    pub padding: HomePadding,
    /// Vertical/horizontal gaps between sections and cards.
    pub gaps: HomeGaps,
    /// Card sizing constraints (min heights, row heights, icon containers).
    pub card_sizing: HomeCardSizing,
}

impl Default for HomeLayout {
    fn default() -> Self {
        Self {
            section_order: vec![
                HomeSection::Hero,
                HomeSection::MeshHealth,
                HomeSection::QuickActions,
                HomeSection::PeopleActivity,
                HomeSection::Tunnels,
            ],
            hidden_sections: Vec::new(),
            mode: HomeLayoutMode::Grid,
            grid: HomeGrid::default(),
            quick_actions: QuickActionsLayout::default(),
            max_content_width: crate::design_tokens::DASHBOARD_MAX_WIDTH,
            padding: HomePadding::default(),
            gaps: HomeGaps::default(),
            card_sizing: HomeCardSizing::default(),
        }
    }
}

/// Grid/list presentation mode for the home dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomeLayoutMode {
    /// Two-column dashboard grid (baseline): main column + right rail.
    #[default]
    Grid,
    /// Single stacked column (what the app does below the stack breakpoint).
    List,
}

/// Dashboard grid: FillPortion split of the main column vs the right rail,
/// the column gap, and the content-width breakpoint below which the rail
/// stacks under the main column (`home.rs:1495-1541`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HomeGrid {
    /// Main column FillPortion (2).
    pub main_portion: u16,
    /// Right rail FillPortion (1).
    pub rail_portion: u16,
    /// Column gap between main and rail (`SPACE_24` = 24 px).
    pub column_gap: f32,
    /// Below this *content* width the rail stacks below the main column
    /// (`HOME_TWO_COL_CONTENT` = 720 px).
    pub stack_breakpoint: f32,
}

impl Default for HomeGrid {
    fn default() -> Self {
        Self {
            main_portion: 2,
            rail_portion: 1,
            column_gap: crate::design_tokens::SPACE_24,
            stack_breakpoint: crate::design_tokens::HOME_TWO_COL_CONTENT,
        }
    }
}

/// Quick-action card grid (`quick_actions.rs::grid_columns_for`): the column
/// counts per width tier and the two content-width breakpoints that switch
/// between them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuickActionsLayout {
    /// Columns at/above `four_col_breakpoint` (4).
    pub columns_wide: usize,
    /// Columns between `two_col_breakpoint` and `four_col_breakpoint` (2).
    pub columns_mid: usize,
    /// Columns below `two_col_breakpoint` (1).
    pub columns_narrow: usize,
    /// Content width at/above which the grid uses `columns_wide`
    /// (`HOME_QUICK_FOUR_COL_CONTENT` = 1000 px).
    pub four_col_breakpoint: f32,
    /// Content width at/above which the grid uses `columns_mid`
    /// (`HOME_QUICK_ONE_COL_CONTENT` = 520 px).
    pub two_col_breakpoint: f32,
}

impl Default for QuickActionsLayout {
    fn default() -> Self {
        Self {
            columns_wide: 4,
            columns_mid: 2,
            columns_narrow: 1,
            four_col_breakpoint: crate::design_tokens::HOME_QUICK_FOUR_COL_CONTENT,
            two_col_breakpoint: crate::design_tokens::HOME_QUICK_ONE_COL_CONTENT,
        }
    }
}

/// Dashboard canvas padding (`home.rs:1565-1569`, `home.rs:1594`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HomePadding {
    /// Top padding (`SPACE_28` = 28 px).
    pub top: f32,
    /// Bottom padding (`SPACE_32` = 32 px).
    pub bottom: f32,
    /// Horizontal padding on large windows (`SPACE_32` = 32 px).
    pub horizontal_large: f32,
    /// Horizontal padding elsewhere (`SPACE_28` = 28 px).
    pub horizontal_default: f32,
}

impl Default for HomePadding {
    fn default() -> Self {
        Self {
            top: crate::design_tokens::SPACE_28,
            bottom: crate::design_tokens::SPACE_32,
            horizontal_large: crate::design_tokens::SPACE_32,
            horizontal_default: crate::design_tokens::SPACE_28,
        }
    }
}

/// Vertical/horizontal gaps between home sections and cards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HomeGaps {
    /// Vertical gap between cards in a column (`quick_action_gap` = 20 px).
    pub card_gap: f32,
    /// Gap between hero card and mesh card (`home.rs:810` hero_gap = 40 px).
    pub hero_gap: f32,
    /// Page header → dashboard gap (`SPACE_28 + SPACE_12` = 40 px,
    /// `home.rs:1576`).
    pub header_dashboard_gap: f32,
    /// Dashboard → footer gap (`SPACE_16` = 16 px, `home.rs:1578`).
    pub footer_gap: f32,
    /// Compact page-header inner stack gap (`SPACE_12` = 12 px,
    /// `home.rs:1466`).
    pub compact_header_stack_gap: f32,
}

impl Default for HomeGaps {
    fn default() -> Self {
        Self {
            card_gap: crate::design_tokens::SPACE_20,
            hero_gap: 40.0,
            header_dashboard_gap: crate::design_tokens::SPACE_28 + crate::design_tokens::SPACE_12,
            footer_gap: crate::design_tokens::SPACE_16,
            compact_header_stack_gap: crate::design_tokens::SPACE_12,
        }
    }
}

/// Card sizing constraints on the home dashboard (min heights, row heights,
/// icon-container diameters). Corner radii and typography stay in `theme.rs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HomeCardSizing {
    /// Online Peers body minimum height (128 px).
    pub peers_body_min: f32,
    /// Recent-activity row height (32 px).
    pub activity_row_height: f32,
    /// Quick-action icon container diameter (40 px).
    pub quick_action_icon_size: f32,
    /// Status card text-column minimum width, Medium tier (260 px).
    pub status_card_text_min_width_medium: f32,
    /// Status card decorative mesh max width (170 px).
    pub status_card_mesh_max_width: f32,
    /// Status card horizontal padding (`SPACE_24` = 24 px).
    pub status_card_padding_x: f32,
    /// Status card icon→text gap, Full tier (24 px).
    pub status_icon_text_gap_full: f32,
    /// Status card icon→text gap, Medium tier (20 px).
    pub status_icon_text_gap_medium: f32,
    /// Status card text→graph gap, Full tier (24 px).
    pub status_text_graph_gap_full: f32,
    /// Status card text→graph gap, Medium tier (24 px).
    pub status_text_graph_gap_medium: f32,
    /// Status card accent divider width (44 px).
    pub status_divider_width: f32,
    /// Status card accent divider height (3 px).
    pub status_divider_height: f32,
}

impl Default for HomeCardSizing {
    fn default() -> Self {
        Self {
            peers_body_min: 128.0,
            activity_row_height: 32.0,
            quick_action_icon_size: 40.0,
            status_card_text_min_width_medium: 260.0,
            status_card_mesh_max_width: 170.0,
            status_card_padding_x: crate::design_tokens::SPACE_24,
            status_icon_text_gap_full: 24.0,
            status_icon_text_gap_medium: 20.0,
            status_text_graph_gap_full: 24.0,
            status_text_graph_gap_medium: 24.0,
            status_divider_width: 44.0,
            status_divider_height: 3.0,
        }
    }
}

// ── Sidebar (PDF Task 2) ─────────────────────────────────────────────

/// Stable identity of a sidebar section. Baseline order matches
/// `app/sidebar.rs::view_sidebar` (CHATS, GROUPS, FRIENDS, DISCOVER,
/// PUBLIC ROOMS, REQUESTS). The collapsed-state array index in the sidebar
/// today is: Chats 0, Groups 1, Friends 2, Discover 3, Requests 4,
/// PublicRooms 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SidebarSection {
    Chats,
    Groups,
    Friends,
    Discover,
    PublicRooms,
    Requests,
}

/// Sidebar shell structural layout.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarLayout {
    /// Target sidebar width at the reference viewport (`SIDEBAR_WIDTH` = 304).
    pub width: f32,
    /// Minimum responsive sidebar width (`SIDEBAR_WIDTH_MIN` = 288).
    pub width_min: f32,
    /// Maximum responsive sidebar width (`SIDEBAR_WIDTH_MAX` = 320).
    pub width_max: f32,
    /// Horizontal inset from sidebar edges to content (`SIDEBAR_INSET` = 24).
    pub inset: f32,
    /// Section order (baseline: Chats, Groups, Friends, Discover,
    /// PublicRooms, Requests).
    pub section_order: Vec<SidebarSection>,
    /// Sections hidden entirely from the sidebar. Empty = all visible.
    pub hidden_sections: Vec<SidebarSection>,
    /// Padding regions (baseline from `theme.rs::SidebarPadding`).
    pub padding: SidebarPadding,
    /// Row heights for sidebar lists.
    pub row_heights: SidebarRowHeights,
}

impl Default for SidebarLayout {
    fn default() -> Self {
        Self {
            width: crate::design_tokens::SIDEBAR_WIDTH,
            width_min: crate::design_tokens::SIDEBAR_WIDTH_MIN,
            width_max: crate::design_tokens::SIDEBAR_WIDTH_MAX,
            inset: crate::design_tokens::SIDEBAR_INSET,
            section_order: vec![
                SidebarSection::Chats,
                SidebarSection::Groups,
                SidebarSection::Friends,
                SidebarSection::Discover,
                SidebarSection::PublicRooms,
                SidebarSection::Requests,
            ],
            hidden_sections: Vec::new(),
            padding: SidebarPadding::default(),
            row_heights: SidebarRowHeights::default(),
        }
    }
}

/// Sidebar padding regions, decomposed from the `iced::Padding` literals in
/// `app/sidebar.rs` (values are `SPACE_*` tokens, see `theme.rs::SidebarPadding`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarPadding {
    /// Pinned brand row: top (`SPACE_16`).
    pub brand_top: f32,
    /// Pinned brand row: bottom (`SPACE_8`).
    pub brand_bottom: f32,
    /// Pinned identity row: top (`SPACE_4`).
    pub identity_top: f32,
    /// Pinned identity row: bottom (`SPACE_8`).
    pub identity_bottom: f32,
    /// Scrollable sections column: top (`SPACE_4`).
    pub section_top: f32,
    /// Bottom utility row: top (`SPACE_8`).
    pub utility_top: f32,
    /// Bottom utility row: bottom (`SPACE_12`).
    pub utility_bottom: f32,
    /// Horizontal row padding for sidebar rows (`SPACE_12`).
    pub row_x: f32,
    /// Join-by-ticket label block: top (`SPACE_8`).
    pub join_top: f32,
    /// Join-by-ticket label block: bottom (`SPACE_4`).
    pub join_bottom: f32,
}

impl Default for SidebarPadding {
    fn default() -> Self {
        Self {
            brand_top: crate::design_tokens::SPACE_16,
            brand_bottom: crate::design_tokens::SPACE_8,
            identity_top: crate::design_tokens::SPACE_4,
            identity_bottom: crate::design_tokens::SPACE_8,
            section_top: crate::design_tokens::SPACE_4,
            utility_top: crate::design_tokens::SPACE_8,
            utility_bottom: crate::design_tokens::SPACE_12,
            row_x: crate::design_tokens::SPACE_12,
            join_top: crate::design_tokens::SPACE_8,
            join_bottom: crate::design_tokens::SPACE_4,
        }
    }
}

/// Row heights used by sidebar and dashboard lists
/// (`card_shell.rs` / `theme.rs::ListTokens`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarRowHeights {
    /// Chat/conversation row height (`CARD_ROW_HEIGHT` = 48 px).
    pub conversation_row: f32,
    /// Friend/peer row height (`PEER_ROW_HEIGHT` = 60 px).
    pub peer_row: f32,
    /// Discovered-peers panel max height (`PEER_PANEL_MAX_HEIGHT` = 320 px).
    pub peer_panel_max_height: f32,
    /// Default list max height before scrolling (`DEFAULT_LIST_MAX_HEIGHT` = 180 px).
    pub default_list_max_height: f32,
}

impl Default for SidebarRowHeights {
    fn default() -> Self {
        Self {
            conversation_row: crate::card_shell::CARD_ROW_HEIGHT,
            peer_row: crate::card_shell::PEER_ROW_HEIGHT,
            peer_panel_max_height: crate::design_tokens::PEER_PANEL_MAX_HEIGHT,
            default_list_max_height: crate::card_shell::DEFAULT_LIST_MAX_HEIGHT,
        }
    }
}

// ── Chat (PDF Task 2) ────────────────────────────────────────────────

/// Chat screen structural layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatLayout {
    /// Hard maximum bubble width (`CHAT_BUBBLE_MAX_WIDTH` = 560 px).
    pub bubble_max_width: f32,
    /// Bubble width as a fraction of the timeline (`CHAT_BUBBLE_WIDTH_RATIO` = 0.68).
    pub bubble_width_ratio: f32,
    /// Message content max width (`MESSAGE_MAX_WIDTH` = 480 px).
    pub message_max_width: f32,
    /// Inline image preview max width (`IMAGE_PREVIEW_MAX_WIDTH` = 360 px).
    pub image_preview_max_width: f32,
    /// Inline image preview max height (`IMAGE_PREVIEW_MAX_HEIGHT` = 400 px).
    pub image_preview_max_height: f32,
    /// Right-click context menu width (180 px).
    pub context_menu_width: f32,
    /// Details panel width (`DETAILS_PANEL_WIDTH` = 280 px).
    pub details_panel_width: f32,
    /// Emoji picker geometry.
    pub emoji_picker: PickerLayout,
    /// GIF picker geometry.
    pub gif_picker: GifPickerLayout,
    /// Screen-share viewer box.
    pub screen_share: ScreenShareLayout,
    /// Composer bar layout (button placement/order).
    pub composer: ComposerLayout,
    /// Member-list panel geometry (chat.rs member popover).
    pub member_list: MemberListLayout,
}

impl Default for ChatLayout {
    fn default() -> Self {
        Self {
            bubble_max_width: crate::design_tokens::CHAT_BUBBLE_MAX_WIDTH,
            bubble_width_ratio: crate::design_tokens::CHAT_BUBBLE_WIDTH_RATIO,
            message_max_width: crate::design_tokens::MESSAGE_MAX_WIDTH,
            image_preview_max_width: crate::design_tokens::IMAGE_PREVIEW_MAX_WIDTH,
            image_preview_max_height: crate::design_tokens::IMAGE_PREVIEW_MAX_HEIGHT,
            context_menu_width: 180.0,
            details_panel_width: crate::design_tokens::DETAILS_PANEL_WIDTH,
            emoji_picker: PickerLayout::default(),
            gif_picker: GifPickerLayout::default(),
            screen_share: ScreenShareLayout::default(),
            composer: ComposerLayout::default(),
            member_list: MemberListLayout::default(),
        }
    }
}

/// A fixed-size picker panel (emoji picker, GIF search).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickerLayout {
    /// Panel width.
    pub width: f32,
    /// Scrollable list height.
    pub scroll_height: f32,
}

impl Default for PickerLayout {
    fn default() -> Self {
        Self {
            width: 280.0,
            scroll_height: 160.0,
        }
    }
}

/// GIF picker geometry (theme.rs::ChatTheme gif_* fields).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GifPickerLayout {
    /// Panel width (320 px).
    pub width: f32,
    /// Scrollable list height (300 px).
    pub scroll_height: f32,
    /// Thumbnail width (150 px).
    pub thumbnail_width: f32,
    /// Thumbnail height (100 px).
    pub thumbnail_height: f32,
}

impl Default for GifPickerLayout {
    fn default() -> Self {
        Self {
            width: 320.0,
            scroll_height: 300.0,
            thumbnail_width: 150.0,
            thumbnail_height: 100.0,
        }
    }
}

/// Screen-share viewer box (theme.rs::ChatTheme screen_share_*).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareLayout {
    /// Viewer width (640 px).
    pub width: f32,
    /// Viewer height (360 px).
    pub height: f32,
}

impl Default for ScreenShareLayout {
    fn default() -> Self {
        Self {
            width: 640.0,
            height: 360.0,
        }
    }
}

/// Composer bar layout (PDF Task 5 "button placement"): the order of buttons
/// along the composer row and the row spacing/padding
/// (`app/chat.rs:3982` — attach | folder | input | gif | emoji | send).
#[derive(Debug, Clone, PartialEq)]
pub struct ComposerLayout {
    /// Button order, left→right. The text input is always between the leading
    /// buttons and the trailing buttons; only button placement is listed.
    /// Baseline: Attach, Folder, Gif, Emoji, Send (input sits after Folder).
    pub button_order: Vec<ComposerButton>,
    /// Row spacing (`SPACE_6` = 6 px).
    pub spacing: f32,
    /// Composer bar padding (`SPACE_4` = 4 px).
    pub padding: f32,
}

impl Default for ComposerLayout {
    fn default() -> Self {
        Self {
            button_order: vec![
                ComposerButton::Attach,
                ComposerButton::Folder,
                ComposerButton::Gif,
                ComposerButton::Emoji,
                ComposerButton::Send,
            ],
            spacing: crate::design_tokens::SPACE_6,
            padding: crate::design_tokens::SPACE_4,
        }
    }
}

/// A composer button slot. The text input is implicit and fixed between the
/// leading and trailing button groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComposerButton {
    /// File attach button.
    Attach,
    /// Folder/choose-file button.
    Folder,
    /// GIF picker button.
    Gif,
    /// Emoji picker button.
    Emoji,
    /// Send button.
    Send,
}

/// Member-list popover geometry (`app/chat.rs:1826-1832`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemberListLayout {
    /// Panel width (300 px).
    pub width: f32,
    /// Panel max height (500 px).
    pub max_height: f32,
    /// Row layout: name FillPortion(3), role FillPortion(1), status dot.
    pub name_portion: u16,
    pub role_portion: u16,
}

impl Default for MemberListLayout {
    fn default() -> Self {
        Self {
            width: 300.0,
            max_height: 500.0,
            name_portion: 3,
            role_portion: 1,
        }
    }
}

// ── Component placement (PDF Task 5) ─────────────────────────────────

/// Per-component arrangement: thumbnail position, metadata alignment, button
/// placement and card orientation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComponentLayout {
    /// Thumbnail position inside media cards (baseline: Left).
    pub thumbnail_position: ThumbnailPosition,
    /// Horizontal alignment of metadata rows inside cards (baseline: Start).
    pub metadata_alignment: MetadataAlignment,
    /// Placement of action buttons relative to card content (baseline: Below).
    pub button_placement: ButtonPlacement,
    /// Overall card orientation (baseline: Horizontal).
    pub card_orientation: CardOrientation,
    /// Video attachment card sizing (`video_file_card.rs`).
    pub video: VideoCardLayout,
}

impl Default for ComponentLayout {
    fn default() -> Self {
        Self {
            thumbnail_position: ThumbnailPosition::Left,
            metadata_alignment: MetadataAlignment::Start,
            button_placement: ButtonPlacement::Below,
            card_orientation: CardOrientation::Horizontal,
            video: VideoCardLayout::default(),
        }
    }
}

/// Thumbnail position relative to the card's text content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThumbnailPosition {
    /// Thumbnail to the left of the text (baseline media cards).
    #[default]
    Left,
    /// Thumbnail to the right of the text.
    Right,
    /// Thumbnail above the text.
    Top,
    /// Thumbnail below the text.
    Bottom,
    /// No thumbnail rendered.
    Hidden,
}

/// Horizontal alignment of metadata rows inside a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataAlignment {
    /// Aligned to the start (left in LTR; baseline).
    #[default]
    Start,
    /// Centred.
    Center,
    /// Aligned to the end (right in LTR).
    End,
}

/// Placement of action buttons relative to card content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonPlacement {
    /// Buttons below the content (baseline).
    #[default]
    Below,
    /// Buttons overlaid on the media/content surface.
    Overlay,
    /// Buttons on a side rail.
    Side,
}

/// Overall card orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardOrientation {
    /// Content flows horizontally — media left, text right (baseline
    /// video/download cards).
    #[default]
    Horizontal,
    /// Content flows vertically — media on top, text below.
    Vertical,
}

/// Video attachment card sizing (`video_file_card.rs` CardBand breakpoints
/// and theme.rs::VideoTokens).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoCardLayout {
    /// Below this timeline width the card uses the 100%-width layout (560 px).
    pub narrow_breakpoint: f32,
    /// Below this timeline width the media caps are scaled down (780 px).
    pub medium_breakpoint: f32,
    /// Play overlay button diameter (64 px).
    pub play_overlay_size: f32,
    /// Hard width cap for the header filename (420 px).
    pub header_filename_max_width: f32,
    /// Inline volume slider width (90 px).
    pub controls_slider_width: f32,
}

impl Default for VideoCardLayout {
    fn default() -> Self {
        Self {
            narrow_breakpoint: 560.0,
            medium_breakpoint: 780.0,
            play_overlay_size: 64.0,
            header_filename_max_width: 420.0,
            controls_slider_width: 90.0,
        }
    }
}

// ── Data tables ──────────────────────────────────────────────────────

/// Data-table column widths (fixed `Length::Fixed` literals in the file
/// dashboard and sharing tables).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TablesLayout {
    /// File-dashboard table column widths (`app/files.rs`, theme.rs::FileTableColumns).
    pub file_table: FileTableColumns,
    /// "Files I'm Sharing" table column widths (`shared_by_me_table.rs`).
    pub shared_table: SharedTableColumns,
}

impl Default for TablesLayout {
    fn default() -> Self {
        Self {
            file_table: FileTableColumns::default(),
            shared_table: SharedTableColumns::default(),
        }
    }
}

/// Column widths for the file-dashboard tables (`app/files.rs` fixed widths:
/// 72 / 120 / 140 / 100 / 110 / 90 / 80 / 240 …).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileTableColumns {
    pub size_col: f32,
    pub source_col: f32,
    pub ago_col: f32,
    pub peer_col: f32,
    pub started_col: f32,
    pub state_col: f32,
    pub direction_col: f32,
    pub event_col: f32,
    pub details_col: f32,
    /// Download-manager transfer row: Started column (100 px, files.rs:2616).
    pub download_started_col: f32,
    /// Download-manager / uploads row: State column (100 px, files.rs:2622/2754).
    pub download_state_col: f32,
    /// Activity-log row: Ago column (110 px, files.rs:3572).
    pub activity_ago_col: f32,
}

impl Default for FileTableColumns {
    fn default() -> Self {
        Self {
            size_col: 72.0,
            source_col: 120.0,
            ago_col: 120.0,
            peer_col: 140.0,
            started_col: 120.0,
            state_col: 110.0,
            direction_col: 90.0,
            event_col: 110.0,
            details_col: 80.0,
            download_started_col: 100.0,
            download_state_col: 100.0,
            activity_ago_col: 110.0,
        }
    }
}

/// Column widths for the "Files I'm Sharing" card (`COL_*` in
/// `shared_by_me_table.rs`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharedTableColumns {
    pub shared_with: f32,
    pub size: f32,
    pub shared_on: f32,
    pub downloads: f32,
    pub actions: f32,
}

impl Default for SharedTableColumns {
    fn default() -> Self {
        Self {
            shared_with: 144.0,
            size: 64.0,
            shared_on: 122.0,
            downloads: 80.0,
            actions: 36.0,
        }
    }
}

// ── Responsive (PDF Task 4) ──────────────────────────────────────────

/// Responsive breakpoints: viewport tiers (widths used by `design_tokens`
/// `is_compact`/`is_medium`/`is_large`/`sidebar_width_for`) and the home
/// content-width thresholds. Later tasks add per-tier column-count tables
/// here (PDF Task 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResponsiveLayout {
    /// Reference viewport width (`VIEWPORT_REF_WIDTH` = 1280).
    pub viewport_ref_width: f32,
    /// Reference viewport height (`VIEWPORT_REF_HEIGHT` = 800).
    pub viewport_ref_height: f32,
    /// Minimum supported viewport width (`VIEWPORT_MIN_WIDTH` = 1024).
    pub viewport_min_width: f32,
    /// Minimum supported viewport height (`VIEWPORT_MIN_HEIGHT` = 720).
    pub viewport_min_height: f32,
    /// Large viewport width (`VIEWPORT_LG_WIDTH` = 1440).
    pub viewport_lg_width: f32,
    /// Large viewport height (`VIEWPORT_LG_HEIGHT` = 900).
    pub viewport_lg_height: f32,
    /// Ultra-wide viewport width (`VIEWPORT_XL_WIDTH` = 1920).
    pub viewport_xl_width: f32,
    /// Ultra-wide viewport height (`VIEWPORT_XL_HEIGHT` = 1080).
    pub viewport_xl_height: f32,
    /// Generic content max width (`CONTENT_MAX_WIDTH` = 720).
    pub content_max_width: f32,
    /// Hero illustration full-size breakpoint (720 px).
    pub home_illustration_full_content: f32,
    /// Hero illustration hide breakpoint (520 px).
    pub home_illustration_hide_content: f32,
    /// Compact card-header breakpoint (560 px).
    pub home_compact_header_content: f32,
}

impl Default for ResponsiveLayout {
    fn default() -> Self {
        Self {
            viewport_ref_width: crate::design_tokens::VIEWPORT_REF_WIDTH,
            viewport_ref_height: crate::design_tokens::VIEWPORT_REF_HEIGHT,
            viewport_min_width: crate::design_tokens::VIEWPORT_MIN_WIDTH,
            viewport_min_height: crate::design_tokens::VIEWPORT_MIN_HEIGHT,
            viewport_lg_width: crate::design_tokens::VIEWPORT_LG_WIDTH,
            viewport_lg_height: crate::design_tokens::VIEWPORT_LG_HEIGHT,
            viewport_xl_width: crate::design_tokens::VIEWPORT_XL_WIDTH,
            viewport_xl_height: crate::design_tokens::VIEWPORT_XL_HEIGHT,
            content_max_width: crate::design_tokens::CONTENT_MAX_WIDTH,
            home_illustration_full_content: crate::design_tokens::HOME_ILLUSTRATION_FULL_CONTENT,
            home_illustration_hide_content: crate::design_tokens::HOME_ILLUSTRATION_HIDE_CONTENT,
            home_compact_header_content: crate::design_tokens::HOME_COMPACT_HEADER_CONTENT,
        }
    }
}

// ── Future screens (extension point) ─────────────────────────────────

/// Per-screen structural layout registered under [`LayoutConfig::screens`].
/// The shape here is the common skeleton every future screen can fill in;
/// individual screens may add screen-specific sections in later tasks.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenLayout {
    /// Ordered section ids for this screen (opaque strings; a future task
    /// assigns typed section enums per screen).
    pub section_order: Vec<String>,
    /// Section ids hidden for this screen.
    pub hidden_sections: Vec<String>,
    /// Max content width for the screen's canvas.
    pub max_content_width: f32,
    /// Column count for the screen's primary grid.
    pub columns: usize,
}

impl Default for ScreenLayout {
    fn default() -> Self {
        Self {
            section_order: Vec::new(),
            hidden_sections: Vec::new(),
            max_content_width: crate::design_tokens::CONTENT_MAX_WIDTH,
            columns: 1,
        }
    }
}
