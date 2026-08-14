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
//! Schema complete (BORU-LAYOUT-02): typed leaf structs + defaults + the
//! [`LayoutOverrides`] partial-override mirror. BORU-LAYOUT-03 wires the
//! `home.*` group into `app/home.rs` (section order/visibility, grid/list
//! mode, columns, max width, padding/gaps, card sizing); the remaining
//! groups (sidebar, chat, component, tables, responsive) are wired by later
//! tasks. TOML parsing/merge/watcher are later BORU-LAYOUT tasks.
//! `#![allow(dead_code)]` guards the still-unwired groups; drop it once
//! every group is consumed by a view.

#![allow(dead_code)] // unwired groups remain until later BORU-LAYOUT tasks consume them

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

impl HomeLayout {
    /// Sections that render on the home dashboard, in vertical order:
    /// [`HomeLayout::section_order`] with every [`HomeLayout::hidden_sections`]
    /// entry removed (BORU-LAYOUT-03: the view renders exactly this list).
    pub fn visible_sections(&self) -> Vec<HomeSection> {
        self.section_order
            .iter()
            .copied()
            .filter(|s| !self.hidden_sections.contains(s))
            .collect()
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
    /// Status card minimum content height (`STATUS_CARD_MIN_CONTENT_HEIGHT` = 110 px).
    pub status_card_min_content_height: f32,
    /// Status card content width at/above which the Full tier applies
    /// (`STATUS_CARD_MEDIUM_CONTENT` = 760 px).
    pub status_card_medium_content: f32,
    /// Status card content width at/above which the Medium tier applies
    /// (`STATUS_CARD_NARROW_CONTENT` = 560 px).
    pub status_card_narrow_content: f32,
    /// Status card content width below which the decorative mesh is hidden
    /// (`STATUS_CARD_MESH_HIDE_CONTENT` = 520 px).
    pub status_card_mesh_hide_content: f32,
    /// Status card text-column minimum width (`STATUS_CARD_TEXT_MIN_WIDTH` = 260 px).
    pub status_card_text_min_width: f32,
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
            status_card_min_content_height: crate::status_card::STATUS_CARD_MIN_CONTENT_HEIGHT,
            status_card_medium_content: crate::status_card::STATUS_CARD_MEDIUM_CONTENT,
            status_card_narrow_content: crate::status_card::STATUS_CARD_NARROW_CONTENT,
            status_card_mesh_hide_content: crate::status_card::STATUS_CARD_MESH_HIDE_CONTENT,
            status_card_text_min_width: crate::status_card::STATUS_CARD_TEXT_MIN_WIDTH,
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

// ── Partial overrides (PDF Task 2: "Support defaults and partial overrides") ──
//
// Every concrete group above has a matching `*Overrides` mirror here where
// each leaf is `Option<T>` — the same organisation as `theme_config.rs` for
// `BoruTheme`. A missing key (a `None` leaf, or a missing group) falls back
// to the corresponding [`LayoutConfig::default`] value at merge time
// (BORU-LAYOUT-03). This file defines the model only; the TOML file,
// merge and watcher come in BORU-LAYOUT-03/06.

/// Root partial-override file model. Every group optional; a missing group
/// means "no overrides" and the merge step falls back to
/// [`LayoutConfig::default`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutOverrides {
    /// Home dashboard overrides.
    pub home: Option<HomeOverrides>,
    /// Sidebar shell overrides.
    pub sidebar: Option<SidebarOverrides>,
    /// Chat screen overrides.
    pub chat: Option<ChatOverrides>,
    /// Component-placement overrides.
    pub component: Option<ComponentOverrides>,
    /// Data-table overrides.
    pub tables: Option<TablesOverrides>,
    /// Responsive-breakpoint overrides.
    pub responsive: Option<ResponsiveOverrides>,
    /// Per-screen overrides for future screens (stable screen-id keys).
    pub screens: BTreeMap<String, ScreenOverrides>,
}

// ── Flat override-group macro ─────────────────────────────────────────
//
// Mirrors `theme_config.rs::config_group!`: generates a struct whose leaves
// are all `Option<T>`, so a partial file deserializes to `None` leaves and
// the merge falls back to the layout defaults. Field names MUST match the
// concrete layout struct so BORU-LAYOUT-03 can merge without a mapping table.

macro_rules! layout_override_group {
    ($(#[$doc:meta])* $name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Default, PartialEq)]
        pub struct $name {
            $(pub $field: Option<$ty>,)*
        }
    };
}

// ── Home overrides ────────────────────────────────────────────────────

/// Home dashboard partial overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeOverrides {
    /// Override the section order.
    pub section_order: Option<Vec<HomeSection>>,
    /// Override which sections are hidden.
    pub hidden_sections: Option<Vec<HomeSection>>,
    /// Override grid/list presentation mode.
    pub mode: Option<HomeLayoutMode>,
    /// Override the main/rail grid split.
    pub grid: Option<HomeGridOverrides>,
    /// Override quick-action column counts / breakpoints.
    pub quick_actions: Option<QuickActionsOverrides>,
    /// Override max dashboard canvas width.
    pub max_content_width: Option<f32>,
    /// Override dashboard padding.
    pub padding: Option<HomePaddingOverrides>,
    /// Override section/card gaps.
    pub gaps: Option<HomeGapsOverrides>,
    /// Override card sizing constraints.
    pub card_sizing: Option<HomeCardSizingOverrides>,
}

layout_override_group! {
    /// Home grid split overrides.
    HomeGridOverrides {
        main_portion: u16,
        rail_portion: u16,
        column_gap: f32,
        stack_breakpoint: f32,
    }
}

layout_override_group! {
    /// Quick-action grid overrides.
    QuickActionsOverrides {
        columns_wide: usize,
        columns_mid: usize,
        columns_narrow: usize,
        four_col_breakpoint: f32,
        two_col_breakpoint: f32,
    }
}

layout_override_group! {
    /// Dashboard canvas padding overrides.
    HomePaddingOverrides {
        top: f32,
        bottom: f32,
        horizontal_large: f32,
        horizontal_default: f32,
    }
}

layout_override_group! {
    /// Home gap overrides.
    HomeGapsOverrides {
        card_gap: f32,
        hero_gap: f32,
        header_dashboard_gap: f32,
        footer_gap: f32,
        compact_header_stack_gap: f32,
    }
}

layout_override_group! {
    /// Home card-sizing overrides.
    HomeCardSizingOverrides {
        peers_body_min: f32,
        activity_row_height: f32,
        quick_action_icon_size: f32,
        status_card_min_content_height: f32,
        status_card_medium_content: f32,
        status_card_narrow_content: f32,
        status_card_mesh_hide_content: f32,
        status_card_text_min_width: f32,
        status_card_text_min_width_medium: f32,
        status_card_mesh_max_width: f32,
        status_card_padding_x: f32,
        status_icon_text_gap_full: f32,
        status_icon_text_gap_medium: f32,
        status_text_graph_gap_full: f32,
        status_text_graph_gap_medium: f32,
        status_divider_width: f32,
        status_divider_height: f32,
    }
}

// ── Sidebar overrides ─────────────────────────────────────────────────

/// Sidebar shell partial overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SidebarOverrides {
    /// Override the target sidebar width.
    pub width: Option<f32>,
    /// Override the minimum responsive width.
    pub width_min: Option<f32>,
    /// Override the maximum responsive width.
    pub width_max: Option<f32>,
    /// Override the horizontal inset.
    pub inset: Option<f32>,
    /// Override the section order.
    pub section_order: Option<Vec<SidebarSection>>,
    /// Override which sections are hidden.
    pub hidden_sections: Option<Vec<SidebarSection>>,
    /// Override padding regions.
    pub padding: Option<SidebarPaddingOverrides>,
    /// Override row heights.
    pub row_heights: Option<SidebarRowHeightsOverrides>,
}

layout_override_group! {
    /// Sidebar padding-region overrides.
    SidebarPaddingOverrides {
        brand_top: f32,
        brand_bottom: f32,
        identity_top: f32,
        identity_bottom: f32,
        section_top: f32,
        utility_top: f32,
        utility_bottom: f32,
        row_x: f32,
        join_top: f32,
        join_bottom: f32,
    }
}

layout_override_group! {
    /// Sidebar / dashboard row-height overrides.
    SidebarRowHeightsOverrides {
        conversation_row: f32,
        peer_row: f32,
        peer_panel_max_height: f32,
        default_list_max_height: f32,
    }
}

// ── Chat overrides ────────────────────────────────────────────────────

/// Chat screen partial overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatOverrides {
    /// Override the bubble max width.
    pub bubble_max_width: Option<f32>,
    /// Override the bubble width ratio.
    pub bubble_width_ratio: Option<f32>,
    /// Override the message content max width.
    pub message_max_width: Option<f32>,
    /// Override the inline image preview max width.
    pub image_preview_max_width: Option<f32>,
    /// Override the inline image preview max height.
    pub image_preview_max_height: Option<f32>,
    /// Override the context-menu width.
    pub context_menu_width: Option<f32>,
    /// Override the details-panel width.
    pub details_panel_width: Option<f32>,
    /// Override the emoji picker geometry.
    pub emoji_picker: Option<PickerOverrides>,
    /// Override the GIF picker geometry.
    pub gif_picker: Option<GifPickerOverrides>,
    /// Override the screen-share viewer box.
    pub screen_share: Option<ScreenShareOverrides>,
    /// Override the composer bar.
    pub composer: Option<ComposerOverrides>,
    /// Override the member-list panel.
    pub member_list: Option<MemberListOverrides>,
}

layout_override_group! {
    /// Fixed-size picker panel overrides.
    PickerOverrides {
        width: f32,
        scroll_height: f32,
    }
}

layout_override_group! {
    /// GIF picker overrides.
    GifPickerOverrides {
        width: f32,
        scroll_height: f32,
        thumbnail_width: f32,
        thumbnail_height: f32,
    }
}

layout_override_group! {
    /// Screen-share viewer box overrides.
    ScreenShareOverrides {
        width: f32,
        height: f32,
    }
}

/// Composer bar partial overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComposerOverrides {
    /// Override the button order (input stays between leading/trailing).
    pub button_order: Option<Vec<ComposerButton>>,
    /// Override row spacing.
    pub spacing: Option<f32>,
    /// Override bar padding.
    pub padding: Option<f32>,
}

layout_override_group! {
    /// Member-list panel overrides.
    MemberListOverrides {
        width: f32,
        max_height: f32,
        name_portion: u16,
        role_portion: u16,
    }
}

// ── Component overrides (PDF Task 5) ──────────────────────────────────

/// Component-placement partial overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComponentOverrides {
    /// Override thumbnail position.
    pub thumbnail_position: Option<ThumbnailPosition>,
    /// Override metadata alignment.
    pub metadata_alignment: Option<MetadataAlignment>,
    /// Override button placement.
    pub button_placement: Option<ButtonPlacement>,
    /// Override card orientation.
    pub card_orientation: Option<CardOrientation>,
    /// Override video card sizing.
    pub video: Option<VideoCardOverrides>,
}

layout_override_group! {
    /// Video attachment card sizing overrides.
    VideoCardOverrides {
        narrow_breakpoint: f32,
        medium_breakpoint: f32,
        play_overlay_size: f32,
        header_filename_max_width: f32,
        controls_slider_width: f32,
    }
}

// ── Tables overrides ──────────────────────────────────────────────────

/// Data-table partial overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TablesOverrides {
    /// File-dashboard table column overrides.
    pub file_table: Option<FileTableOverrides>,
    /// Sharing-table column overrides.
    pub shared_table: Option<SharedTableOverrides>,
}

layout_override_group! {
    /// File-dashboard table column-width overrides.
    FileTableOverrides {
        size_col: f32,
        source_col: f32,
        ago_col: f32,
        peer_col: f32,
        started_col: f32,
        state_col: f32,
        direction_col: f32,
        event_col: f32,
        details_col: f32,
        download_started_col: f32,
        download_state_col: f32,
        activity_ago_col: f32,
    }
}

layout_override_group! {
    /// Sharing-table column-width overrides.
    SharedTableOverrides {
        shared_with: f32,
        size: f32,
        shared_on: f32,
        downloads: f32,
        actions: f32,
    }
}

// ── Responsive overrides (PDF Task 4) ─────────────────────────────────

layout_override_group! {
    /// Responsive breakpoint / viewport-tier overrides.
    ResponsiveOverrides {
        viewport_ref_width: f32,
        viewport_ref_height: f32,
        viewport_min_width: f32,
        viewport_min_height: f32,
        viewport_lg_width: f32,
        viewport_lg_height: f32,
        viewport_xl_width: f32,
        viewport_xl_height: f32,
        content_max_width: f32,
        home_illustration_full_content: f32,
        home_illustration_hide_content: f32,
        home_compact_header_content: f32,
    }
}

// ── Future-screen overrides (extension point) ─────────────────────────

/// Per-screen partial overrides registered under [`LayoutOverrides::screens`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScreenOverrides {
    /// Override the ordered section ids.
    pub section_order: Option<Vec<String>>,
    /// Override which section ids are hidden.
    pub hidden_sections: Option<Vec<String>>,
    /// Override the canvas max width.
    pub max_content_width: Option<f32>,
    /// Override the primary grid column count.
    pub columns: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_shell;
    use crate::design_tokens;
    use crate::status_card;
    use crate::theme::BoruTheme;

    // ── Default = current appearance ──────────────────────────────────

    #[test]
    fn home_visible_sections_filters_hidden_and_keeps_order() {
        let h = HomeLayout::default();
        assert_eq!(
            h.visible_sections(),
            vec![
                HomeSection::Hero,
                HomeSection::MeshHealth,
                HomeSection::QuickActions,
                HomeSection::PeopleActivity,
                HomeSection::Tunnels,
            ]
        );
        // Hidden sections are skipped; the remaining order is preserved.
        let hidden = HomeLayout {
            section_order: vec![
                HomeSection::Tunnels,
                HomeSection::Hero,
                HomeSection::PeopleActivity,
                HomeSection::MeshHealth,
            ],
            hidden_sections: vec![HomeSection::Hero],
            ..Default::default()
        };
        assert_eq!(
            hidden.visible_sections(),
            vec![
                HomeSection::Tunnels,
                HomeSection::PeopleActivity,
                HomeSection::MeshHealth,
            ]
        );
    }

    #[test]
    fn home_defaults_reproduce_current_appearance() {
        let h = HomeLayout::default();

        // Section order: left column then right rail.
        assert_eq!(
            h.section_order,
            vec![
                HomeSection::Hero,
                HomeSection::MeshHealth,
                HomeSection::QuickActions,
                HomeSection::PeopleActivity,
                HomeSection::Tunnels,
            ]
        );
        assert!(
            h.hidden_sections.is_empty(),
            "all sections visible by default"
        );
        assert_eq!(h.mode, HomeLayoutMode::Grid);
        assert_eq!(h.max_content_width, design_tokens::DASHBOARD_MAX_WIDTH);

        // Grid split + stack breakpoint.
        assert_eq!(h.grid.main_portion, 2);
        assert_eq!(h.grid.rail_portion, 1);
        assert_eq!(h.grid.column_gap, design_tokens::SPACE_24);
        assert_eq!(h.grid.stack_breakpoint, design_tokens::HOME_TWO_COL_CONTENT);

        // Quick-action column counts + breakpoints.
        assert_eq!(h.quick_actions.columns_wide, 4);
        assert_eq!(h.quick_actions.columns_mid, 2);
        assert_eq!(h.quick_actions.columns_narrow, 1);
        assert_eq!(
            h.quick_actions.four_col_breakpoint,
            design_tokens::HOME_QUICK_FOUR_COL_CONTENT
        );
        assert_eq!(
            h.quick_actions.two_col_breakpoint,
            design_tokens::HOME_QUICK_ONE_COL_CONTENT
        );

        // Padding + gaps.
        assert_eq!(h.padding.top, design_tokens::SPACE_28);
        assert_eq!(h.padding.bottom, design_tokens::SPACE_32);
        assert_eq!(h.padding.horizontal_large, design_tokens::SPACE_32);
        assert_eq!(h.padding.horizontal_default, design_tokens::SPACE_28);
        assert_eq!(h.gaps.card_gap, design_tokens::SPACE_20);
        assert_eq!(h.gaps.hero_gap, BoruTheme::default().home.hero_gap);
        assert_eq!(
            h.gaps.header_dashboard_gap,
            design_tokens::SPACE_28 + design_tokens::SPACE_12
        );
        assert_eq!(h.gaps.footer_gap, design_tokens::SPACE_16);
        assert_eq!(h.gaps.compact_header_stack_gap, design_tokens::SPACE_12);

        // Card sizing constraints.
        assert_eq!(h.card_sizing.peers_body_min, 128.0);
        assert_eq!(h.card_sizing.activity_row_height, 32.0);
        assert_eq!(h.card_sizing.quick_action_icon_size, 40.0);
        assert_eq!(
            h.card_sizing.status_card_min_content_height,
            status_card::STATUS_CARD_MIN_CONTENT_HEIGHT
        );
        assert_eq!(
            h.card_sizing.status_card_medium_content,
            status_card::STATUS_CARD_MEDIUM_CONTENT
        );
        assert_eq!(
            h.card_sizing.status_card_narrow_content,
            status_card::STATUS_CARD_NARROW_CONTENT
        );
        assert_eq!(
            h.card_sizing.status_card_mesh_hide_content,
            status_card::STATUS_CARD_MESH_HIDE_CONTENT
        );
        assert_eq!(
            h.card_sizing.status_card_text_min_width,
            status_card::STATUS_CARD_TEXT_MIN_WIDTH
        );
        assert_eq!(h.card_sizing.status_card_text_min_width_medium, 260.0);
        assert_eq!(
            h.card_sizing.status_card_mesh_max_width,
            status_card::STATUS_CARD_MESH_MAX_WIDTH
        );
        assert_eq!(
            h.card_sizing.status_card_padding_x,
            status_card::STATUS_CARD_PADDING_X
        );
        assert_eq!(h.card_sizing.status_icon_text_gap_full, 24.0);
        assert_eq!(h.card_sizing.status_icon_text_gap_medium, 20.0);
        assert_eq!(h.card_sizing.status_text_graph_gap_full, 24.0);
        assert_eq!(h.card_sizing.status_text_graph_gap_medium, 24.0);
        assert_eq!(h.card_sizing.status_divider_width, 44.0);
        assert_eq!(h.card_sizing.status_divider_height, 3.0);
    }

    #[test]
    fn sidebar_defaults_reproduce_current_appearance() {
        let s = SidebarLayout::default();
        assert_eq!(s.width, design_tokens::SIDEBAR_WIDTH);
        assert_eq!(s.width_min, design_tokens::SIDEBAR_WIDTH_MIN);
        assert_eq!(s.width_max, design_tokens::SIDEBAR_WIDTH_MAX);
        assert_eq!(s.inset, design_tokens::SIDEBAR_INSET);
        assert_eq!(
            s.section_order,
            vec![
                SidebarSection::Chats,
                SidebarSection::Groups,
                SidebarSection::Friends,
                SidebarSection::Discover,
                SidebarSection::PublicRooms,
                SidebarSection::Requests,
            ]
        );
        assert!(
            s.hidden_sections.is_empty(),
            "all sections visible by default"
        );

        let theme = BoruTheme::default();
        assert_eq!(s.padding.brand_top, theme.sidebar.padding.brand_top);
        assert_eq!(s.padding.brand_bottom, theme.sidebar.padding.brand_bottom);
        assert_eq!(s.padding.identity_top, theme.sidebar.padding.identity_top);
        assert_eq!(
            s.padding.identity_bottom,
            theme.sidebar.padding.identity_bottom
        );
        assert_eq!(s.padding.section_top, theme.sidebar.padding.section_top);
        assert_eq!(s.padding.utility_top, theme.sidebar.padding.utility_top);
        assert_eq!(
            s.padding.utility_bottom,
            theme.sidebar.padding.utility_bottom
        );
        assert_eq!(s.padding.row_x, theme.sidebar.padding.row_x);
        assert_eq!(s.padding.join_top, theme.sidebar.padding.join_top);
        assert_eq!(s.padding.join_bottom, theme.sidebar.padding.join_bottom);

        assert_eq!(s.row_heights.conversation_row, card_shell::CARD_ROW_HEIGHT);
        assert_eq!(s.row_heights.peer_row, card_shell::PEER_ROW_HEIGHT);
        assert_eq!(
            s.row_heights.peer_panel_max_height,
            design_tokens::PEER_PANEL_MAX_HEIGHT
        );
        assert_eq!(
            s.row_heights.default_list_max_height,
            card_shell::DEFAULT_LIST_MAX_HEIGHT
        );
    }

    #[test]
    fn chat_defaults_reproduce_current_appearance() {
        let c = ChatLayout::default();
        assert_eq!(c.bubble_max_width, design_tokens::CHAT_BUBBLE_MAX_WIDTH);
        assert_eq!(c.bubble_width_ratio, design_tokens::CHAT_BUBBLE_WIDTH_RATIO);
        assert_eq!(c.message_max_width, design_tokens::MESSAGE_MAX_WIDTH);
        assert_eq!(
            c.image_preview_max_width,
            design_tokens::IMAGE_PREVIEW_MAX_WIDTH
        );
        assert_eq!(
            c.image_preview_max_height,
            design_tokens::IMAGE_PREVIEW_MAX_HEIGHT
        );

        let theme = BoruTheme::default();
        assert_eq!(c.context_menu_width, theme.chat.context_menu_width);
        assert_eq!(c.details_panel_width, design_tokens::DETAILS_PANEL_WIDTH);
        assert_eq!(c.emoji_picker.width, theme.chat.emoji_picker_width);
        assert_eq!(
            c.emoji_picker.scroll_height,
            theme.chat.emoji_picker_scroll_height
        );
        assert_eq!(c.gif_picker.width, theme.chat.gif_picker_width);
        assert_eq!(
            c.gif_picker.scroll_height,
            theme.chat.gif_picker_scroll_height
        );
        assert_eq!(c.gif_picker.thumbnail_width, theme.chat.gif_thumbnail_width);
        assert_eq!(
            c.gif_picker.thumbnail_height,
            theme.chat.gif_thumbnail_height
        );
        assert_eq!(c.screen_share.width, theme.chat.screen_share_w);
        assert_eq!(c.screen_share.height, theme.chat.screen_share_h);

        assert_eq!(
            c.composer.button_order,
            vec![
                ComposerButton::Attach,
                ComposerButton::Folder,
                ComposerButton::Gif,
                ComposerButton::Emoji,
                ComposerButton::Send,
            ]
        );
        assert_eq!(c.composer.spacing, design_tokens::SPACE_6);
        assert_eq!(c.composer.padding, design_tokens::SPACE_4);

        assert_eq!(c.member_list.width, 300.0);
        assert_eq!(c.member_list.max_height, 500.0);
        assert_eq!(c.member_list.name_portion, 3);
        assert_eq!(c.member_list.role_portion, 1);
    }

    #[test]
    fn component_defaults_reproduce_current_appearance() {
        let c = ComponentLayout::default();
        assert_eq!(c.thumbnail_position, ThumbnailPosition::Left);
        assert_eq!(c.metadata_alignment, MetadataAlignment::Start);
        assert_eq!(c.button_placement, ButtonPlacement::Below);
        assert_eq!(c.card_orientation, CardOrientation::Horizontal);

        let theme = BoruTheme::default();
        assert_eq!(
            c.video.narrow_breakpoint,
            theme.attachments.video.narrow_breakpoint
        );
        assert_eq!(
            c.video.medium_breakpoint,
            theme.attachments.video.medium_breakpoint
        );
        assert_eq!(
            c.video.play_overlay_size,
            theme.attachments.video.play_overlay_size
        );
        assert_eq!(
            c.video.header_filename_max_width,
            theme.attachments.video.header_filename_max_width
        );
        assert_eq!(
            c.video.controls_slider_width,
            theme.attachments.video.controls_slider_width
        );
    }

    #[test]
    fn tables_defaults_reproduce_current_appearance() {
        let t = TablesLayout::default();
        let theme = BoruTheme::default();
        let ft = theme.attachments.file_table;
        assert_eq!(t.file_table.size_col, ft.size_col);
        assert_eq!(t.file_table.source_col, ft.source_col);
        assert_eq!(t.file_table.ago_col, ft.ago_col);
        assert_eq!(t.file_table.peer_col, ft.peer_col);
        assert_eq!(t.file_table.started_col, ft.started_col);
        assert_eq!(t.file_table.state_col, ft.state_col);
        assert_eq!(t.file_table.direction_col, ft.direction_col);
        assert_eq!(t.file_table.event_col, ft.event_col);
        assert_eq!(t.file_table.details_col, ft.details_col);
        assert_eq!(t.file_table.download_started_col, ft.download_started_col);
        assert_eq!(t.file_table.download_state_col, ft.download_state_col);
        assert_eq!(t.file_table.activity_ago_col, ft.activity_ago_col);

        let st = theme.attachments.shared_table;
        assert_eq!(t.shared_table.shared_with, st.shared_with);
        assert_eq!(t.shared_table.size, st.size);
        assert_eq!(t.shared_table.shared_on, st.shared_on);
        assert_eq!(t.shared_table.downloads, st.downloads);
        assert_eq!(t.shared_table.actions, st.actions);
    }

    #[test]
    fn responsive_defaults_reproduce_current_appearance() {
        let r = ResponsiveLayout::default();
        assert_eq!(r.viewport_ref_width, design_tokens::VIEWPORT_REF_WIDTH);
        assert_eq!(r.viewport_ref_height, design_tokens::VIEWPORT_REF_HEIGHT);
        assert_eq!(r.viewport_min_width, design_tokens::VIEWPORT_MIN_WIDTH);
        assert_eq!(r.viewport_min_height, design_tokens::VIEWPORT_MIN_HEIGHT);
        assert_eq!(r.viewport_lg_width, design_tokens::VIEWPORT_LG_WIDTH);
        assert_eq!(r.viewport_lg_height, design_tokens::VIEWPORT_LG_HEIGHT);
        assert_eq!(r.viewport_xl_width, design_tokens::VIEWPORT_XL_WIDTH);
        assert_eq!(r.viewport_xl_height, design_tokens::VIEWPORT_XL_HEIGHT);
        assert_eq!(r.content_max_width, design_tokens::CONTENT_MAX_WIDTH);
        assert_eq!(
            r.home_illustration_full_content,
            design_tokens::HOME_ILLUSTRATION_FULL_CONTENT
        );
        assert_eq!(
            r.home_illustration_hide_content,
            design_tokens::HOME_ILLUSTRATION_HIDE_CONTENT
        );
        assert_eq!(
            r.home_compact_header_content,
            design_tokens::HOME_COMPACT_HEADER_CONTENT
        );
    }

    #[test]
    fn screens_extension_point_is_empty_by_default() {
        let l = LayoutConfig::default();
        assert!(
            l.screens.is_empty(),
            "no future screens registered by default"
        );
        // A future screen starts from a sensible skeleton.
        let s = ScreenLayout::default();
        assert!(s.section_order.is_empty());
        assert!(s.hidden_sections.is_empty());
        assert_eq!(s.max_content_width, design_tokens::CONTENT_MAX_WIDTH);
        assert_eq!(s.columns, 1);
    }

    // ── Partial overrides: default = no changes ───────────────────────

    #[test]
    fn overrides_default_to_no_changes() {
        let o = LayoutOverrides::default();
        assert!(o.home.is_none());
        assert!(o.sidebar.is_none());
        assert!(o.chat.is_none());
        assert!(o.component.is_none());
        assert!(o.tables.is_none());
        assert!(o.responsive.is_none());
        assert!(o.screens.is_empty(), "no per-screen overrides by default");
    }

    #[test]
    fn overrides_missing_leaf_falls_back_to_default() {
        // A partial override with one leaf set leaves every other leaf
        // `None` — the merge layer (BORU-LAYOUT-03) treats `None` as
        // "keep the default", so a missing key falls back to defaults.
        let o = HomeOverrides {
            max_content_width: Some(1200.0),
            ..Default::default()
        };
        assert_eq!(o.max_content_width, Some(1200.0));
        assert!(o.section_order.is_none());
        assert!(o.grid.is_none());
        assert!(o.gaps.is_none());
        assert!(o.card_sizing.is_none());

        // Root with only the home group supplied.
        let root = LayoutOverrides {
            home: Some(o),
            ..Default::default()
        };
        assert!(root.sidebar.is_none());
        assert!(root.chat.is_none());
        assert_eq!(root.home.as_ref().unwrap().max_content_width, Some(1200.0));
    }

    #[test]
    fn overrides_enums_and_vectors_are_typed() {
        // The override shape must carry the same typed enum/vector values
        // as the concrete model (compile-time check + fallback semantics).
        let home = HomeOverrides {
            section_order: Some(vec![HomeSection::Tunnels, HomeSection::Hero]),
            hidden_sections: Some(vec![HomeSection::QuickActions]),
            mode: Some(HomeLayoutMode::List),
            ..Default::default()
        };
        assert_eq!(home.mode, Some(HomeLayoutMode::List));
        assert_eq!(
            home.section_order,
            Some(vec![HomeSection::Tunnels, HomeSection::Hero])
        );

        let comp = ComponentOverrides {
            thumbnail_position: Some(ThumbnailPosition::Top),
            metadata_alignment: Some(MetadataAlignment::Center),
            button_placement: Some(ButtonPlacement::Overlay),
            card_orientation: Some(CardOrientation::Vertical),
            ..Default::default()
        };
        assert_eq!(comp.thumbnail_position, Some(ThumbnailPosition::Top));
        assert_eq!(comp.card_orientation, Some(CardOrientation::Vertical));

        let composer = ComposerOverrides {
            button_order: Some(vec![ComposerButton::Send, ComposerButton::Gif]),
            ..Default::default()
        };
        assert_eq!(
            composer.button_order,
            Some(vec![ComposerButton::Send, ComposerButton::Gif])
        );

        let chat = ChatOverrides {
            composer: Some(composer),
            ..Default::default()
        };
        assert_eq!(
            chat.composer.as_ref().unwrap().button_order,
            Some(vec![ComposerButton::Send, ComposerButton::Gif])
        );
        assert!(
            chat.emoji_picker.is_none(),
            "missing nested group falls back"
        );
    }

    #[test]
    fn overrides_screens_map_supports_per_screen_keys() {
        let mut screens = BTreeMap::new();
        screens.insert(
            "settings".to_string(),
            ScreenOverrides {
                columns: Some(2),
                ..Default::default()
            },
        );
        let root = LayoutOverrides {
            screens,
            ..Default::default()
        };
        let s = root
            .screens
            .get("settings")
            .expect("settings screen present");
        assert_eq!(s.columns, Some(2));
        assert!(s.section_order.is_none());
        assert!(
            root.screens.get("files").is_none(),
            "missing screen key falls back"
        );
    }
}
