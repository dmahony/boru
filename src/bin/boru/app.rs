//! The iced Application shell for the boru gossip-chat frontend.
//!
//! # Architecture: `app.rs` as application coordinator
//!
//! `app.rs` is deliberately thin. It owns only what is genuinely global
//! shell / navigation / lifecycle state (window sizing, dark mode, screen
//! routing, net-event plumbing, persistence handles) and delegates every
//! domain concern to a sibling module in [`app::`](self):
//!
//! | Module            | Owns                                                     |
//! |-------------------|----------------------------------------------------------|
//! | `sidebar`         | ChatList, profile header, section row rendering          |
//! | `settings`        | Settings screen, developer UI (BORU-APP-003)             |
//! | `contacts`        | Contact / friend book                                    |
//! | `calls`           | Audio/video call surface                                 |
//! | `chat`            | Chat log, composer, image/video playback                 |
//! | `discover`        | Discovered peers + public room discovery                 |
//! | `files`           | File transfers, dashboard cards                          |
//! | `rooms`           | Room lifecycle + room-category navigation                |
//! | `home`            | Home/landing cards, mesh status, connection events       |
//! | `groups`          | Group chat create/invite/view                            |
//! | `dialogs`         | Reusable dialog components                               |
//! | `tunnels`         | Tunnel (friend IP) management                            |
//! | `help_overlay`    | Help overlay (BORU-APP-002 reference domain)             |
//! | `notifications`   | Notification service, toasts, activity feed (BORU-APP-004) |
//!
//! Each module follows the domain pattern in `app/domain_pattern.md`:
//! domain state + `DomainMessage` + `update()` + `view()` helpers that are
//! invoked from the coordinator's `AppMessage` dispatcher. Modules re-export
//! their surface through `pub(crate) use <module>::*` so the coordinator and
//! tests can call them without path noise; nothing below lives only in
//! `app.rs` unless it is shared shell plumbing or navigation glue.
//!
//! # Responsibilities that stay in the coordinator
//!
//! - [`IcedChat::new`] wiring: networking handles, stores, channels.
//! - The [`Application`] trait impl: `update`/`view`/`theme`/`subscription`
//!   routing, `AppMessage` dispatch.
//! - Screen-level state (`Screen`, active room/topic, layout cache).
//! - Cross-domain plumbing (net event routing, persistence handles).
//!
//! # History
//!
//! Previously a ~36k-line monolith, `app.rs` was progressively decomposed
//! (BORU-APP-001…010) by extracting each domain into `app/<domain>.rs` while
//! keeping the coordinator's public API stable. This file now documents the
//! final application composition: the coordinator plus the domain module map
//! above.

mod sidebar;
pub(crate) use sidebar::*;

mod settings;
pub(crate) use settings::*;

mod contacts;
pub(crate) use contacts::*;

mod calls;
pub(crate) use calls::*;

mod chat;
pub(crate) use chat::*;

#[cfg(feature = "screen-sharing")]
mod screen_share_surface;
#[cfg(feature = "screen-sharing")]
pub(crate) use screen_share_surface::*;

#[cfg(feature = "screen-sharing")]
mod screen_share_ui;
#[cfg(feature = "screen-sharing")]
pub(crate) use screen_share_ui::*;

mod discover;
pub(crate) use discover::*;

mod files;
pub(crate) use files::*;

mod rooms;
pub(crate) use rooms::*;

mod home;
pub(crate) use home::*;

mod groups;
pub(crate) use groups::*;

mod dialogs;
pub(crate) use dialogs::*;

mod tunnels;
pub(crate) use tunnels::*;

// BORU-APP-002: help-overlay domain — the reference implementation of the
// DomainState + DomainMessage + update() + view() pattern (see
// `app/domain_pattern.md`). Unlike the view-layer modules above, this domain
// owns its state and exposes only its domain surface, not `impl IcedChat`.
mod help_overlay;
pub(crate) use help_overlay::*;

// BORU-APP-004: notifications & activity domain — notification service +
// window focus tracker + in-app toast + landing-page Recent Activity feed.
mod notifications;
pub(crate) use notifications::*;

mod state;
pub(crate) use state::*;

use boru_core::abuse_controls::{
    sanitize_display_text, sanitize_single_line, DEFAULT_MAX_DISPLAY_LENGTH,
};
use boru_core::catalogue_client::fetch_paginated_remote_catalogue;
use boru_core::catalogue_model::RemoteSharedFile;
use boru_core::media_classification::{classify_attachment, MediaKind};
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::card_shell::{CardShell, StatusBadgeKind, CARD_ROW_HEIGHT};
#[cfg(feature = "dev-ui")]
use crate::designer::{DesignerHistory, DesignerMessage, DesignerState};
use crate::link_preview;
#[cfg(feature = "terminal")]
use crate::terminal_view::TerminalTab;
use boru_core::api::{GossipSender, GossipTopic};
use boru_core::authorization::{AuthorizationEvent, AuthorizationState, Permission};
use boru_core::backfill::{BackfillHandle, BACKFILL_TRIGGER_THRESHOLD};
use boru_core::call::history::{event_text as call_history_text, CallHistoryOutcome};
use boru_core::call::manager::{CallEvent, CallHandle};
#[cfg(feature = "video-calls")]
use boru_core::call::video::layout::contain_fit_rect;
#[cfg(feature = "video-calls")]
use boru_core::call::video::VideoFrame;
use boru_core::call::{CallId, CallKind};
pub(crate) use boru_core::chat_callbacks::TransferKind;
use boru_core::chat_callbacks::{ChatCallbacks, TransferId, TransferProgress};
use boru_core::chat_core::protocol::FileOfferId;
use boru_core::chat_core::{
    collect_bootstrap_peers, download_blob_to_file, download_blob_with_safety, download_candidates,
    friend_ping::{FriendEvent, FriendPingManager, FriendStatus},
    handle_net_event_with_safety_for_topic, merge_bootstrap_peer_addrs, message_hash,
    seed_memory_lookup, MeshHealth, MessageHash, RoomInviteV2,
};
use boru_core::pinned_messages::{PinAction, PinState};
use boru_core::chat_history::{ChatHistoryStore, DeliveryState, HistoryEntry};
use boru_core::contact::{direct_topic, ContactAction, SignedContactMessage};
use boru_core::control_plane::advertisement::{
    normalize_room_metadata, AdvertisementBounds, RoomVisibility,
};
use boru_core::control_plane::connectivity::{
    ConnectivityEvent, PeerConnectivityState, PeerConnectivityStore,
};
use boru_core::conversations::{
    spawn_conversation_forwarder, ConversationEntry, ConversationKind, ConversationNetEvent,
    ConversationStore, GroupTopicHistory,
};
use boru_core::discovery_backend::MainlineDhtBackend;
use boru_core::discovery_secret::DiscoverySecret;
use boru_core::download_limits::DownloadLimitsConfig;
use boru_core::download_manager::DownloadManager;
use boru_core::file_indexer::FileIndexer;
use boru_core::file_offer::FileOfferRegistry;
use boru_core::friend_request::{
    FriendRequest, FriendRequestError, FriendRequestStatus, FriendRequestStore,
};
use boru_core::friends::{DirectConversationState, FriendId, FriendRelationship, FriendsStore};
use boru_core::gif_provider::{
    GifContentRating, GifMediaFormat, GifProviderError, GifSearchPage, GifSearchRequest,
    GifSearchResult, GifTrendingRequest,
};
use boru_core::group_id::GroupId;
use boru_core::image_optimizer::{
    compress_image, optimize_chat_image_to_webp, CHAT_IMAGE_MAX_BYTES,
};
use boru_core::image_store::ImageStore;
use boru_core::inbox::{send_ack, send_deliver, send_sync_request, InboxEvent};
use boru_core::mailbox::{
    seal_for, IncomingAcceptance, MailboxAck, MailboxIdentity, MailboxPublicKey, MailboxStore,
};
use boru_core::net::Gossip;
use boru_core::private_room_tracker::{PrivateContinuousTracker, PrivateRoomTracker};
use boru_core::proto::TopicId;
use boru_core::public_room::{public_discovery_key, PublicNetwork, PublicRoomIdentity};
use boru_core::public_room_continuous::{
    ContinuousTracker as PublicContinuousTracker, ContinuousTrackerConfig,
};
use boru_core::public_room_safety::PublicRoomSafety;
use boru_core::public_room_tracker::PublicRoomTracker;
use boru_core::room::RoomStore;
use boru_core::room_cleanup::{clear_room_history, delete_room_history, RoomHistoryClearReport};
use boru_core::room_docs::{self, RoomMetadata};
use boru_core::room_history::RoomHistoryStore;
#[cfg(feature = "screen-sharing")]
use boru_core::screen_share::{
    composite_cursor_rgba, run_host_session, AudioOutput, Capability, CaptureSource,
    CaptureSourceId, CapturedFrame, ControlMessage, CursorSprite, HostCommand, InboundAudio,
    InboundMedia, InputEventKind, OpenH264Decoder, OpusAudioDecoder, PathKind, PixelFormat,
    QualityPreset, RedactedText, ScreenShareMessage, ScreenShareProtocol, ScreenShareSessionId,
    ScreenShareSessionMetrics, ScreenShareStatsSnapshot, SessionEvent, SourcePoint, ViewerPipeline,
    AUDIO_SAMPLES_PER_FRAME, DEFAULT_QUEUE_CAPACITY, MAX_CLIPBOARD_TEXT, MOD_ALT, MOD_CTRL,
    MOD_META, MOD_SHIFT, SCREEN_SHARE_PROTOCOL_VERSION,
};
use boru_core::storage::{SharedFileRow, Storage};
use boru_core::store::MessageStore;
use boru_core::streaming_server::StreamingServer;
use boru_core::transfer_state_projection::{
    EventName, ProjectionUpdate, TransferDirection, TransferEvent, TransferRecord, TransferState,
    TransferStateStore, TransferUpdateReceiver,
};
use boru_core::tunnel::service::TunnelStatus;
use boru_core::user_profile::{SharedFile, UserProfile, UserProfileStore};
use boru_core::video_playback::{
    validate_attachment_filename, verify_local_attachment, verify_local_attachment_unmanaged,
    PlaybackCoordinator, VideoInstanceKey, VideoJitterBuffer,
};
use boru_core::video_poster;
#[cfg(feature = "video-playback")]
use boru_core::video_runtime::VideoRuntimeCapability;
use boru_core::whisper::{WhisperEvent, WhisperHandle};
use iroh::{
    address_lookup::memory::MemoryLookup, EndpointAddr, PublicKey, RelayMode, SecretKey, Watcher,
};
use iroh_blobs::{store::fs::FsStore, ticket::BlobTicket};
use n0_future::task;
use n0_future::Stream;
use n0_future::StreamExt;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

use crate::connection_details::{
    self, ConnectionDetailsDialogAction, ConnectionDetailsDialogState, ConnectionDetailsViewModel,
};
use crate::perf_tracker::PerfTracker;
use crate::ui_components::{
    chat_status_footer, connection_footer, ghost_icon_button, secondary_button, section_fade,
    sidebar_empty_state, text_input_field, Avatar, SidebarSectionHeader,
};
use crate::{fmt_relay_mode, Message, NetEvent, SignedMessage, Ticket};
use boru_core::chat_core::{
    verify_advertisement, verify_room_withdrawal, RoomAdvertisement, RoomInvitation, TypingEmitter,
    TypingState, DIAGNOSTICS,
};
use boru_core::diagnostics::DiagnosticEventKind;
use boru_core::diagnostics::FailureLayer;
use boru_core::diagnostics::GuiActionError;
use boru_core::diagnostics::GuiActionErrorCode;
use boru_core::diagnostics::GuiActionHistory;
use boru_core::diagnostics::GuiActionId;
use boru_core::diagnostics::GuiActionRequest;
use boru_core::diagnostics::GuiActionState;
use boru_core::diagnostics::GuiTestCommand;
use boru_core::diagnostics::IcedMessageJournal;
use boru_core::diagnostics::IcedStateSnapshot;
use boru_core::diagnostics::DEFAULT_ACTION_STATE_TIMEOUT_MS;
use boru_core::directory::{DirectoryStore, LegacyAdmitOutcome};
use iced::Color;
#[cfg(feature = "video-playback")]
use iced_video_player::Video;

#[cfg(feature = "video-playback")]
#[derive(Debug, Clone)]
struct InlineVideoSession {
    key: VideoInstanceKey,
    video: Option<Arc<Video>>,
    error: Option<String>,
    /// Deadline-driven playout scheduler (telepathy `AudioJitterBuffer` pattern).
    /// Gates frame presentation on source-timed deadlines instead of a fixed
    /// timer, tracks the talkspurt anchor / keepalive floor, and counts loss.
    jitter: VideoJitterBuffer,
    /// Last position retained when lifecycle management pauses this player.
    resume_position: Duration,
    /// Keeps a paused decoder warm briefly while the user scrolls nearby.
    last_near_viewport: Instant,
    /// Local HTTP streaming server backing this player when the video is
    /// being streamed from a still-growing download. `None` for normal
    /// file-backed playback. Dropped (server stopped) when the session is
    /// dropped, i.e. when playback ends or the card is removed.
    streaming_server: Option<Arc<StreamingServer>>,
    /// Visibility and idle deadline for the on-media controls.
    controls_visible: bool,
    controls_last_interaction: Instant,
    /// Keyboard focus is currently inside the on-media controls. While
    /// true, auto-hide is suppressed (PDF task 18 / AC9: never remove
    /// keyboard-focused controls).
    controls_focused: bool,
}

#[cfg(feature = "video-playback")]
#[derive(Debug, Clone)]
enum InlineVideoEvent {
    Loaded {
        key: VideoInstanceKey,
        video: Arc<Video>,
    },
    Failed {
        key: VideoInstanceKey,
        error: String,
    },
    Ended {
        key: VideoInstanceKey,
    },
    Error {
        key: VideoInstanceKey,
        error: String,
    },
}


/// Decode a home-screen background image handle from an on-disk path.
/// Returns `None` when the path is missing, empty, or unreadable so a stale
/// `settings.json` entry (e.g. the file was moved) degrades gracefully.
fn load_home_background_handle(path: Option<&str>) -> Option<iced::widget::image::Handle> {
    let path = path?;
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(iced::widget::image::Handle::from_bytes(bytes))
}

fn invitation_endpoint_addr(
    endpoint_addr: EndpointAddr,
    share_direct_addresses: bool,
) -> EndpointAddr {
    if share_direct_addresses {
        return endpoint_addr;
    }

    let mut addr = EndpointAddr::new(endpoint_addr.id);
    for relay_url in endpoint_addr.relay_urls() {
        addr = addr.with_relay_url(relay_url.clone());
    }
    addr
}

/// Scrollable ID for the chat log — used to auto-scroll to bottom.
const CHAT_LOG: &str = "chat_log";
/// Stable widget ID used to focus the chat composer from the `/` shortcut.
const COMPOSER_INPUT: &str = "chat_composer";
/// Stable widget ID used to focus the room-name field in the create-room dialog.
const CREATE_ROOM_NAME_INPUT: &str = "create-room-name-input";
/// Stable widget ID used to focus the group-name field in the create-group dialog.
const CREATE_GROUP_NAME_INPUT: &str = "create-group-name-input";
/// Stable widget ID used to focus the first value field in the connection-details dialog.
const CONNECTION_DETAILS_FIRST_VALUE_INPUT: &str = "connection-details-first-value";
/// Stable widget ID used to restore focus to the settings-page details trigger.
const CONNECTION_DETAILS_TRIGGER_INPUT: &str = "connection-details-trigger";



// ── Typography scale (re-exported from typography system) ────────────
pub(crate) use crate::fonts::{
    LG as TYPO_LG, MD as TYPO_MD, SM as TYPO_SM, XL as TYPO_XL, XS as TYPO_XS, XXS as TYPO_XXS,
};

/// Brand wordmark font weight (800 = ExtraBold).
#[expect(dead_code)]
pub(crate) const BRAND_LOGO_WEIGHT: u16 = 800;

// ── BoruLogo component ────────────────────────────────────────────

/// Size preset for the `BoruLogo` component.
#[derive(Debug, Clone, Copy)]
pub(crate) enum LogoSize {
    /// Compact header — ~16 px.
    Small,
    /// Authentication / onboarding screens — ~28 px.
    #[expect(dead_code)]
    Medium,
    /// Splash screen — ~44 px.
    Large,
}

impl LogoSize {
    /// Return the font size in logical pixels.
    fn pt(&self) -> f32 {
        match self {
            LogoSize::Small => 16.0,
            LogoSize::Medium => 28.0,
            LogoSize::Large => 44.0,
        }
    }
}

/// Reusable "BORU" wordmark rendered in Raleway ExtraBold 800.
///
/// Uses uppercase letters and the brand weight.  Colour and size are
/// configurable through parameters.
///
/// ```ignore
/// // Splash screen
/// let logo = BoruLogo::new(LogoSize::Large)
///     .color(text_color);
/// // Compact header
/// let logo = BoruLogo::new(LogoSize::Small)
///     .color(accent_primary(&theme));
/// ```
pub(crate) fn boru_logo<'a>(size: LogoSize) -> BoruLogo<'a> {
    BoruLogo {
        size,
        color: None,
        _phantom: std::marker::PhantomData,
    }
}

/// Builder for the "BORU" brand wordmark.
#[derive(Debug, Clone)]
pub(crate) struct BoruLogo<'a> {
    size: LogoSize,
    color: Option<iced::Color>,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> BoruLogo<'a> {
    /// Set the text colour.  If not called, inherits the ambient text
    /// colour from the theme.
    pub fn color(mut self, color: iced::Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Convert the logo into an `iced::Element`, consuming the builder.
    pub fn into_element(self) -> iced::Element<'a, AppMessage> {
        self.into()
    }
}

impl<'a> From<BoruLogo<'a>> for iced::Element<'a, AppMessage> {
    fn from(logo: BoruLogo<'a>) -> iced::Element<'a, AppMessage> {
        use iced::widget::text;

        let font_size = logo.size.pt();
        let font = crate::fonts::raleway_extra_bold();
        let mut t = text("BORU").font(font).size(font_size);
        if let Some(c) = logo.color {
            t = t.style(move |t| text::Style { color: Some(c) });
        }
        t.into()
    }
}

// ── Memory budget limits ─────────────────────────────────────────
/// Maximum total decoded image bytes across all `ChatEntry.image_bytes`
/// (not including `image_handle` which shares the same Arc'd data via Iced).
/// When exceeded, the oldest evictable entries have their `image_bytes`
/// dropped (they can be re-loaded from `ImageStore` via `image_identifier`).
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Maximum number of `ChatEntry` entries kept in memory for the active room.
/// Older entries that have been persisted to `ChatHistoryStore` are dropped
/// first.  This bounds the in-memory overhead of long-running sessions.
const MAX_ENTRIES: usize = 2000;

/// Maximum number of cached profile-image handles for remote peers.
/// Beyond this, the least-recently-used entry is evicted when a new one
/// arrives.
const MAX_PROFILE_IMAGE_HANDLES: usize = 500;

/// Maximum number of hidden-room events replayed in one Iced update.
///
/// Replaying an entire direct-room backlog synchronously can starve the Iced
/// event loop and make the window appear frozen.  Remaining events are
/// scheduled through `ReplayPendingEvents` so input, rendering, and network
/// events continue to be serviced between batches.
const MAX_PENDING_REPLAY_PER_UPDATE: usize = 32;
/// Bound for the FS-05 outbound transfer history kept in the dashboard panel.
const MAX_OUTBOUND_HISTORY: usize = 50;
/// Bound for the FS-14 inbound transfer history kept in the dashboard panel.
const MAX_INBOUND_HISTORY: usize = 50;

/// Maximum number of events to queue for an inactive conversation.
/// When exceeded, the oldest event is dropped to prevent unbounded
/// memory growth and Iced event-loop starvation during replay.
const MAX_PENDING_EVENTS: usize = 256;

/// Version string: the current application version from Cargo.toml,
/// with the current git hash appended when available.
pub fn version_tag() -> String {
    let version = option_env!("BORU_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    match option_env!("GIT_HASH") {
        Some(h) => format!("v{} ({})", version, h),
        None => format!("v{}", version),
    }
}

/// Operating system name for diagnostics.
fn os_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        std::env::consts::OS
    }
}

/// Profile name (debug or release).
fn profile_name() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Build the pre-filled GitHub issue URL for bug reports.
///
/// Includes version, OS, profile, and the last ~100 lines of
/// the app log (if available).  The user fills in the description
/// and steps in the browser.
fn report_bug_url(data_dir: &std::path::Path) -> String {
    let version = version_tag();
    let os = os_name();
    let profile = profile_name();

    // Capture the tail of the log file (last 100 lines, ~8 KB max).
    let log_tail = {
        let log_path = crate::log_viewer::log_file_path(data_dir);
        match std::fs::read_to_string(&log_path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let start = lines.len().saturating_sub(100);
                lines[start..].join("\n")
            }
            Err(_) => "(log file not available)".to_string(),
        }
    };

    let body = format!(
        "### Environment\n\
         - Version: {version}\n\
         - OS: {os}\n\
         - Profile: {profile}\n\
         \n\
         ### What happened\n\
         <!-- describe the bug here -->\n\
         \n\
         ### Steps to reproduce\n\
         1. \n\
         2. \n\
         3. \n\
         \n\
         ### Recent logs\n\
         ```\n\
         {log_tail}\n\
         ```\n"
    );

    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("title", &format!("[bug] "))
        .append_pair("body", &body)
        .append_pair("labels", "bug")
        .finish();

    format!("https://github.com/dmahony/iroh-gossip-chat/issues/new?{encoded}")
}

/// Build the wire representation used for all GUI file shares.
///
/// A content hash by itself is insufficient: the receiver also needs the
/// sender's endpoint address and the blob format to construct a downloader
/// request. Keeping this in one helper prevents the gossip and whisper paths
/// from drifting into incompatible ticket formats.
fn blob_ticket_string(
    addr: EndpointAddr,
    hash: iroh_blobs::Hash,
    format: iroh_blobs::BlobFormat,
) -> String {
    BlobTicket::new(addr, hash, format).to_string()
}

// ── Spacing units (4px base) ─────────────────────────────────────────
pub(crate) const SPACE_2: f32 = 2.0;
pub(crate) use crate::design_tokens::{AVATAR_CHAT_HEADER, AVATAR_MD, AVATAR_MSG, AVATAR_SM};
pub(crate) use crate::design_tokens::{
    DETAILS_PANEL_WIDTH, RADIUS_SM, SIDEBAR_INSET, SIDEBAR_WIDTH, SPACE_12, SPACE_16, SPACE_18,
    SPACE_20, SPACE_24, SPACE_28, SPACE_32, SPACE_4, SPACE_8,
};
pub(crate) use crate::icon_system::{Icon, IconSize};
pub(crate) const SPACE_6: f32 = 6.0;
pub(crate) const SPACE_10: f32 = 10.0;

// ── Attachment presentation bounds ─────────────────────────────────────
// Keep previews useful without allowing a single attachment to take over the
// conversation canvas. ContentFit::Contain preserves the source aspect ratio.
const ATTACHMENT_RADIUS: f32 = 10.0;

const PROFILE_IMAGE_FILE: &str = "profile-image";
const PROFILE_IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;

fn supported_profile_image(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    )
}

#[expect(dead_code)]
fn save_profile_image(data_dir: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let temporary = data_dir.join(format!("{PROFILE_IMAGE_FILE}.tmp"));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, data_dir.join(PROFILE_IMAGE_FILE))
}

/// The subdirectory under data_dir where cached friend profile images are stored.
const FRIEND_PROFILE_IMAGES_DIR: &str = "profile_images";

/// Return the path to the on-disk cache for a peer's profile image.
fn friend_profile_image_path(data_dir: &std::path::Path, peer: &PublicKey) -> PathBuf {
    data_dir
        .join(FRIEND_PROFILE_IMAGES_DIR)
        .join(peer.fmt_short().to_string())
}

/// Save a friend's profile image bytes to the on-disk cache.
fn save_friend_profile_image(data_dir: &std::path::Path, peer: &PublicKey, bytes: &[u8]) {
    let dir = data_dir.join(FRIEND_PROFILE_IMAGES_DIR);
    if let Err(e) = fs::create_dir_all(&dir) {
        tracing::warn!("create profile_images dir: {e}");
        return;
    }
    let target = friend_profile_image_path(data_dir, peer);
    let tmp = target.with_extension("tmp");
    if let Err(e) = fs::write(&tmp, bytes) {
        tracing::warn!("write friend profile image {peer}: {e}");
        let _ = fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = fs::rename(&tmp, &target) {
        tracing::warn!("rename friend profile image {peer}: {e}");
        let _ = fs::remove_file(&tmp);
    }
}

/// Persisted seen-peers file name (PUBLIC-03). Stores the set of peer
/// public keys observed at least once, so a peer that reconnects or is
/// seen again after an app restart is not re-announced as "new".
const SEEN_PEERS_FILE: &str = "seen_peers.json";

/// Load the persisted set of peer public keys observed at least once.
///
/// The set is seeded with every existing friend: a contact already in the
/// friends store has demonstrably been seen before (they exchanged a
/// request/acceptance), so they must never be re-announced as a "new user"
/// after an upgrade or restart.
fn load_seen_peers(data_dir: &std::path::Path, friends: &FriendsStore) -> HashSet<PublicKey> {
    let mut seen: HashSet<PublicKey> = friends
        .iter()
        .filter_map(|(id, _)| id.parse_public_key().ok())
        .collect();
    let path = data_dir.join(SEEN_PEERS_FILE);
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(list) = serde_json::from_slice::<Vec<String>>(&bytes) {
            for key in list {
                if let Ok(pk) = key.parse::<PublicKey>() {
                    seen.insert(pk);
                }
            }
        }
    }
    seen
}

/// Persist the seen-peers set to `<data_dir>/seen_peers.json` (atomic).
fn save_seen_peers(data_dir: &std::path::Path, seen: &HashSet<PublicKey>) {
    let keys: Vec<String> = seen.iter().map(|pk| pk.to_string()).collect();
    if let Err(err) = boru_core::chat_core::atomic_write::atomic_write_json(
        &data_dir.join(SEEN_PEERS_FILE),
        &keys,
        "seen peers",
    ) {
        tracing::warn!("failed to save seen peers: {err}");
    }
}

/// Load all cached friend profile images from disk into the in-memory handle map.
fn load_cached_friend_profile_images(
    data_dir: &std::path::Path,
) -> HashMap<PublicKey, Option<iced::widget::image::Handle>> {
    let dir = data_dir.join(FRIEND_PROFILE_IMAGES_DIR);
    let mut handles = HashMap::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return handles,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Skip temp files left behind by interrupted writes.
        if path.extension().is_some_and(|e| e == "tmp") {
            let _ = fs::remove_file(&path);
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Parse the filename as a hex-encoded 32-byte public key.
        let Ok(bytes) = hex::decode(name) else {
            continue;
        };
        if bytes.len() != 32 {
            continue;
        }
        let arr: [u8; 32] = match bytes.try_into() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let Ok(peer) = PublicKey::from_bytes(&arr) else {
            continue;
        };
        match fs::read(&path) {
            Ok(data) if !data.is_empty() && data.len() <= PROFILE_IMAGE_MAX_BYTES => {
                let handle = iced::widget::image::Handle::from_bytes(data);
                handles.insert(peer, Some(handle));
            }
            Ok(_) => {
                // Empty or oversized — clean up.
                let _ = fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }
    handles
}

/// Load an image from the per-user store without exposing filesystem paths.
///
/// Returns `None` for missing, unreadable, empty, or oversized files so callers
/// can degrade gracefully without leaking storage details to the UI.
#[expect(dead_code)]
fn load_stored_chat_image(
    image_store: &ImageStore,
    user: &str,
    identifier: &str,
) -> Option<Vec<u8>> {
    let path = image_store.resolve_absolute_path(user, identifier).ok()?;
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > CHAT_IMAGE_MAX_BYTES {
        return None;
    }
    Some(bytes)
}

// ── Theme-aware chat colors ──────────────────────────────────────────
/// Return the muted secondary color for labels, previews, and counts.
pub(crate) fn text_muted(theme: &iced::Theme) -> Color {
    crate::design_tokens::text_muted(theme)
}

/// Color for system message text (label and body).
pub(crate) fn text_system(theme: &iced::Theme) -> Color {
    crate::design_tokens::text_secondary(theme)
}

/// Secondary text color for labels, hints, and secondary info.
pub(crate) fn text_secondary(theme: &iced::Theme) -> Color {
    crate::design_tokens::text_secondary(theme)
}

/// Color for local (self) message label.
fn text_local_label(theme: &iced::Theme) -> Color {
    crate::design_tokens::text_local_label(theme)
}

/// Color for local message body text.
fn text_local_body(theme: &iced::Theme) -> Color {
    crate::design_tokens::text_local_body(theme)
}

/// Color for remote message label (nickname).
fn text_remote_label(theme: &iced::Theme) -> Color {
    crate::design_tokens::text_remote_label(theme)
}

/// Color for remote message body text.
fn text_remote_body(theme: &iced::Theme) -> Color {
    crate::design_tokens::text_remote_body(theme)
}

/// Background tint for message bubbles. System messages get no bubble.
fn bubble_bg(theme: &iced::Theme, kind: ChatKind) -> Option<iced::Background> {
    crate::design_tokens::bubble_bg(theme, kind == ChatKind::Local, kind == ChatKind::System)
}

// ── Systematic palette (dark/light) ─────────────────────────────────────
/// Main window background — dark: #1a1a2e, light: #f0f0f5.
fn bg_primary(theme: &iced::Theme) -> Color {
    crate::design_tokens::app_background(theme)
}

/// Surface/card background (slightly lighter than primary).
pub(crate) fn bg_surface(theme: &iced::Theme) -> Color {
    crate::design_tokens::surface(theme)
}

/// Secondary surface for grouped controls and quiet sections.
pub(crate) fn bg_surface_secondary(theme: &iced::Theme) -> Color {
    crate::design_tokens::surface_secondary(theme)
}

/// Input field background.
#[expect(dead_code)]
fn bg_input(theme: &iced::Theme) -> Color {
    crate::design_tokens::bg_input(theme)
}

/// Hover-state background for rows and interactive surfaces.
fn bg_hover(theme: &iced::Theme) -> Color {
    crate::design_tokens::surface_hover(theme)
}

/// Restrained tinted surface for selected navigation rows.
fn bg_selected(theme: &iced::Theme) -> Color {
    crate::design_tokens::selected_surface(theme)
}

/// Subtle border for surfaces and cards.
pub(crate) fn border_muted(theme: &iced::Theme) -> Color {
    crate::design_tokens::border(theme)
}

/// Optional user-configured accent color (RGB bytes). Consulted by
/// [`accent_primary`] so a custom color picked in Settings applies app-wide.
/// `None` restores the theme default.
static ACCENT_OVERRIDE: StdMutex<Option<[u8; 3]>> = StdMutex::new(None);

/// Set (or clear, with `None`) the user's custom accent color.
pub(crate) fn set_accent_override(rgb: Option<[u8; 3]>) {
    *ACCENT_OVERRIDE.lock().unwrap() = rgb;
}

/// Primary accent (blue), or the user's custom accent color when set.
pub(crate) fn accent_primary(theme: &iced::Theme) -> Color {
    if let Some([r, g, b]) = *ACCENT_OVERRIDE.lock().unwrap() {
        return iced::Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    }
    crate::design_tokens::primary(theme)
}

/// Subtle primary-accent tint for small emphasis surfaces.
///
/// Keep this derived from [`accent_primary`] so user-selected accents also
/// reach icon tiles and similar Home highlights without affecting semantic
/// success/error/warning surfaces.
pub(crate) fn accent_soft(theme: &iced::Theme) -> Color {
    let accent = accent_primary(theme);
    Color::from_rgba(accent.r, accent.g, accent.b, 0.12)
}

/// Success / online indicator (green).
pub(crate) fn accent_green(theme: &iced::Theme) -> Color {
    crate::design_tokens::online(theme)
}

/// Error / destructive colour.
pub(crate) fn color_error(theme: &iced::Theme) -> Color {
    crate::design_tokens::destructive(theme)
}

/// Warning / amber colour for reconnecting states.
pub(crate) fn color_warning(theme: &iced::Theme) -> Color {
    crate::design_tokens::color_warning(theme)
}

// ── Lucide SVG icons (embedded as byte data at compile time) ───────
// Source: https://github.com/lucide-icons/lucide (MIT licence)
pub(crate) const ICON_CHAT: &[u8] = include_bytes!("../../../assets/icons/lucide/message-circle.svg");
pub(crate) const ICON_FRIEND: &[u8] = include_bytes!("../../../assets/icons/lucide/user-plus.svg");
pub(crate) const ICON_FILES: &[u8] = include_bytes!("../../../assets/icons/lucide/files.svg");
pub(crate) const ICON_RETRY: &[u8] = include_bytes!("../../../assets/icons/lucide/refresh-cw.svg");
pub(crate) const ICON_SETTINGS: &[u8] = include_bytes!("../../../assets/icons/lucide/settings.svg");
pub(crate) const ICON_CLOSE: &[u8] = include_bytes!("../../../assets/icons/lucide/x.svg");
pub(crate) const ICON_PLUS: &[u8] = include_bytes!("../../../assets/icons/lucide/plus.svg");
pub(crate) const ICON_SEARCH: &[u8] = include_bytes!("../../../assets/icons/lucide/search.svg");
pub(crate) const ICON_MORE: &[u8] = include_bytes!("../../../assets/icons/lucide/ellipsis.svg");
pub(crate) const ICON_ACTIVITY: &[u8] = include_bytes!("../../../assets/icons/lucide/activity.svg");
pub(crate) const ICON_NOTIFICATION: &[u8] = include_bytes!("../../../assets/icons/lucide/bell.svg");
pub(crate) const ICON_ONLINE: &[u8] = include_bytes!("../../../assets/icons/lucide/circle-filled.svg");
pub(crate) const ICON_OFFLINE: &[u8] = include_bytes!("../../../assets/icons/lucide/circle.svg");
pub(crate) const ICON_CHECK: &[u8] = include_bytes!("../../../assets/icons/lucide/check.svg");
pub(crate) const ICON_PLAY: &[u8] = include_bytes!("../../../assets/icons/lucide/play.svg");
pub(crate) const ICON_FOLDER: &[u8] = include_bytes!("../../../assets/icons/lucide/folder.svg");
pub(crate) const ICON_MESH: &[u8] = include_bytes!("../../../assets/icons/lucide/share-2.svg");
pub(crate) const ICON_PAPERCLIP: &[u8] = include_bytes!("../../../assets/icons/lucide/paperclip.svg");
pub(crate) const ICON_SEND: &[u8] = include_bytes!("../../../assets/icons/lucide/send.svg");
pub(crate) const ICON_EMOJI: &str = "😊";
#[expect(dead_code)]
pub(crate) const ICON_UNREAD: &[u8] =
    include_bytes!("../../../assets/icons/lucide/message-circle-fill.svg");
pub(crate) const ICON_SWEEP: &[u8] = include_bytes!("../../../assets/icons/lucide/trash-2.svg");
pub(crate) const ICON_LOCK: &[u8] = include_bytes!("../../../assets/icons/lucide/lock.svg");
pub(crate) const ICON_COPY: &[u8] = include_bytes!("../../../assets/icons/lucide/copy.svg");
pub(crate) const ICON_USER_PLUS: &[u8] = include_bytes!("../../../assets/icons/lucide/user-plus.svg");

// ── SVG icon helper ──────────────────────────────────────────────────
/// Create an SVG icon widget from embedded Lucide icon bytes.
/// Use `.style(|t, _| svg::Style { color: Some(accent_primary(t)) })` to colour.
pub(crate) fn icon_svg<'a>(
    svg_bytes: &'static [u8],
    size: f32,
) -> iced::widget::svg::Svg<'a, iced::Theme> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(svg_bytes))
        .width(iced::Length::Fixed(size))
        .height(iced::Length::Fixed(size))
        .style(|t, _s| iced::widget::svg::Style {
            color: Some(crate::design_tokens::text(t)),
        })
}

// ── Container style helpers ──────────────────────────────────────────────
/// Container style for the primary window background.
fn container_primary(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_primary(theme))),
        ..Default::default()
    }
}

/// Container style for a surface/card background.
fn container_surface(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_surface(theme))),
        ..Default::default()
    }
}

/// Container style for the conversation header — adds a subtle bottom border
/// to visually separate the header from the message log.
fn container_header(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_surface(theme))),
        border: iced::Border {
            color: crate::design_tokens::border(theme),
            width: 0.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Container style for hover-state background.
fn container_hover(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_hover(theme))),
        ..Default::default()
    }
}

/// Info row helper: label on the left, value on the right.
fn info_row(
    label: String,
    value: String,
    theme: &iced::Theme,
) -> iced::widget::Row<'static, AppMessage> {
    use iced::Length;
    iced::widget::row![
        crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, label)
            .color(text_secondary(theme)),
        crate::fonts::type_role_text(crate::fonts::TypeRole::Body, value)
            .color(crate::design_tokens::text(theme)),
    ]
    .spacing(SPACE_8)
    .width(Length::Fill)
}

/// A thin horizontal divider with muted color.
fn divider(_theme: &iced::Theme) -> iced::widget::Container<'static, AppMessage> {
    use iced::{widget::container, Length};
    container(iced::widget::Space::new().height(0.0).width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(move |t| container::Style {
            background: Some(iced::Background::Color(crate::design_tokens::border(t))),
            ..Default::default()
        })
}

/// Closures passed to `text().style()` need this static-compatible form.
pub fn text_muted_style(theme: &iced::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(text_muted(theme)),
    }
}

#[expect(dead_code)]
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct FriendsSidebarCacheKey {
    revision: u64,
    search_input: String,
    dark_mode: bool,
}

/// Container style for a card — surface background, muted border, rounded.
pub(crate) fn container_card(theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(bg_surface(theme))),
        border: iced::Border {
            color: border_muted(theme),
            width: 1.0,
            radius: SPACE_8.into(),
        },
        ..Default::default()
    }
}

// ── Button style helpers ─────────────────────────────────────────────
/// Ghost button — no background, text-colour accent on hover, otherwise
/// inherits the surrounding text colour.
const BUTTON_GHOST: fn(&iced::Theme, iced::widget::button::Status) -> iced::widget::button::Style =
    |theme, status| {
        iced::widget::button::Style {
            text_color: match status {
                iced::widget::button::Status::Hovered => accent_primary(theme),
                iced::widget::button::Status::Pressed => {
                    // Slightly dimmer accent on press
                    let mut c = accent_primary(theme);
                    c.r *= 0.85;
                    c.g *= 0.85;
                    c.b *= 0.85;
                    c
                }
                _ => {
                    crate::theme::BoruTheme::for_theme(theme)
                        .colors
                        .glyph_disabled
                }
            },
            ..Default::default()
        }
    };

/// Ghost button with hover background tint — like `BUTTON_GHOST` but with
/// a subtle `bg_hover` background on hover for better visual feedback.
pub(crate) const BUTTON_GHOST_BG: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| iced::widget::button::Style {
    background: match status {
        iced::widget::button::Status::Hovered => Some(iced::Background::Color(bg_hover(theme))),
        iced::widget::button::Status::Pressed => {
            let mut c = bg_hover(theme);
            c.r *= 0.85;
            c.g *= 0.85;
            c.b *= 0.85;
            Some(iced::Background::Color(c))
        }
        _ => None,
    },
    text_color: match status {
        iced::widget::button::Status::Hovered => accent_primary(theme),
        iced::widget::button::Status::Pressed => {
            let mut c = accent_primary(theme);
            c.r *= 0.85;
            c.g *= 0.85;
            c.b *= 0.85;
            c
        }
        iced::widget::button::Status::Disabled => text_muted(theme),
        _ => {
            crate::theme::BoruTheme::for_theme(theme)
                .colors
                .glyph_disabled
        }
    },
    border: iced::Border {
        radius: SPACE_4.into(),
        ..Default::default()
    },
    ..Default::default()
};

/// Primary filled button — accent background, white text, rounded.
pub(crate) const BUTTON_PRIMARY: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| {
    let (bg_r, bg_g, bg_b) = {
        let c = accent_primary(theme);
        (c.r, c.g, c.b)
    };
    let bg = match status {
        iced::widget::button::Status::Hovered => crate::design_tokens::primary_hover(theme),
        iced::widget::button::Status::Pressed => crate::design_tokens::primary_pressed(theme),
        _ => Color::from_rgb(bg_r, bg_g, bg_b),
    };
    iced::widget::button::Style {
        background: Some(iced::Background::Color(bg)),
        text_color: Color::WHITE,
        border: iced::Border {
            radius: SPACE_6.into(),
            ..Default::default()
        },
        ..Default::default()
    }
};

/// Green primary button — for positive actions (Send, Accept).
pub(crate) const BUTTON_PRIMARY_GREEN: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| {
    let base = accent_green(theme);
    let bg = match status {
        iced::widget::button::Status::Hovered => Color::from_rgb(
            (base.r * 1.15).min(1.0),
            (base.g * 1.15).min(1.0),
            (base.b * 1.15).min(1.0),
        ),
        iced::widget::button::Status::Pressed => {
            Color::from_rgb(base.r * 0.85, base.g * 0.85, base.b * 0.85)
        }
        _ => base,
    };
    iced::widget::button::Style {
        background: Some(iced::Background::Color(bg)),
        text_color: Color::WHITE,
        border: iced::Border {
            radius: RADIUS_SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
};

/// Danger/destructive button — error background, white text, rounded.
pub(crate) const BUTTON_DANGER: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| {
    let base = color_error(theme);
    let bg = match status {
        iced::widget::button::Status::Hovered => {
            Color::from_rgb((base.r * 1.2).min(1.0), base.g * 1.2, base.b * 1.2)
        }
        iced::widget::button::Status::Pressed => {
            Color::from_rgb(base.r * 0.85, base.g * 0.85, base.b * 0.85)
        }
        _ => base,
    };
    iced::widget::button::Style {
        background: Some(iced::Background::Color(bg)),
        text_color: Color::WHITE,
        border: iced::Border {
            radius: SPACE_6.into(),
            ..Default::default()
        },
        ..Default::default()
    }
};

/// Outline button — border_muted border, accent text on hover, transparent bg.
pub(crate) const BUTTON_OUTLINE: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| {
    let border_color = match status {
        iced::widget::button::Status::Hovered => accent_primary(theme),
        iced::widget::button::Status::Pressed => {
            let mut c = accent_primary(theme);
            c.r *= 0.85;
            c.g *= 0.85;
            c.b *= 0.85;
            c
        }
        _ => border_muted(theme),
    };
    iced::widget::button::Style {
        background: match status {
            iced::widget::button::Status::Hovered => Some(iced::Background::Color(
                Color::from_rgba(0.3, 0.3, 0.3, 0.08),
            )),
            _ => None,
        },
        text_color: match status {
            iced::widget::button::Status::Hovered => accent_primary(theme),
            iced::widget::button::Status::Pressed => {
                let mut c = accent_primary(theme);
                c.r *= 0.85;
                c.g *= 0.85;
                c.b *= 0.85;
                c
            }
            _ => {
                crate::theme::BoruTheme::for_theme(theme)
                    .colors
                    .glyph_disabled
            }
        },
        border: iced::Border {
            color: border_color,
            width: 1.0,
            radius: SPACE_6.into(),
        },
        ..Default::default()
    }
};

/// Muted text button — no background, muted colour, error on hover (for destructive actions).
pub(crate) const BUTTON_MUTED: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| iced::widget::button::Style {
    text_color: match status {
        iced::widget::button::Status::Hovered => color_error(theme),
        _ => Color::from_rgb(0.45, 0.45, 0.45),
    },
    border: iced::Border {
        radius: SPACE_4.into(),
        ..Default::default()
    },
    ..Default::default()
};

/// Icon-only button for sidebar — minimal padding, text-colour accent on hover.
pub(crate) const BUTTON_ICON: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = crate::design_tokens::icon_button;

/// Transparent full-size backdrop button — invisible but clickable.
#[expect(dead_code)]
pub(crate) const BUTTON_BACKDROP: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |_theme, _status| iced::widget::button::Style {
    background: None,
    border: iced::Border::default(),
    text_color: iced::Color::TRANSPARENT,
    ..Default::default()
};

/// Transparent-wide button — no background, no border, inherits parent text color.
/// Used for clickable rows that should look like plain containers.
#[expect(dead_code)]
pub(crate) const BUTTON_TRANSPARENT: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| iced::widget::button::Style {
    background: None,
    border: iced::Border::default(),
    text_color: if matches!(status, iced::widget::button::Status::Disabled) {
        text_muted(theme)
    } else {
        crate::design_tokens::text(theme)
    },
    ..Default::default()
};

/// Card-style button — surface background, subtle border, hover lift.
/// Matches `container_card` visually but adds interactive hover/press feedback.
pub(crate) const BUTTON_CARD: fn(
    &iced::Theme,
    iced::widget::button::Status,
) -> iced::widget::button::Style = |theme, status| iced::widget::button::Style {
    background: Some(iced::Background::Color(match status {
        iced::widget::button::Status::Hovered => bg_hover(theme),
        iced::widget::button::Status::Pressed => {
            let mut c = bg_hover(theme);
            c.r *= 0.92;
            c.g *= 0.92;
            c.b *= 0.92;
            c
        }
        _ => bg_surface(theme),
    })),
    text_color: match status {
        iced::widget::button::Status::Hovered => accent_primary(theme),
        iced::widget::button::Status::Pressed => {
            let mut c = accent_primary(theme);
            c.r *= 0.85;
            c.g *= 0.85;
            c.b *= 0.85;
            c
        }
        _ => text_muted(theme),
    },
    border: iced::Border {
        color: match status {
            iced::widget::button::Status::Hovered => accent_primary(theme),
            iced::widget::button::Status::Pressed => {
                let mut c = accent_primary(theme);
                c.r *= 0.85;
                c.g *= 0.85;
                c.b *= 0.85;
                c
            }
            _ => border_muted(theme),
        },
        width: 1.0,
        radius: SPACE_8.into(),
    },
    ..Default::default()
};

// ── Chat entry types ──────────────────────────────────────────────────

/// Current time as Unix epoch milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// A peer is considered Away when no presence/activity has refreshed its
/// last-seen timestamp for this long (5 minutes).
///
/// The 5-minute rule (SIDEBAR-04): active peers broadcast an invisible
/// Heartbeat every ~2s and PresenceWithTicket every ~5s while connected,
/// so a healthy peer's last-seen is refreshed well within this window and
/// stays Online. Away is reserved for peers that have genuinely been
/// silent for over 5 minutes.
const AWAY_THRESHOLD_MS: u64 = 5 * 60 * 1000;

/// Presence state shown in contact displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PeerPresence {
    Online,
    Away,
    Offline,
    /// The peer is known but we have not yet established a live connection
    /// (e.g. the room subscription is still in flight).
    Connecting,
    /// Seen recently on discovery but not (yet) connected for direct
    /// messaging. Derived from the BORU-CP-05 state machine's
    /// `Discovered` state (PDF 2.3 "Recently seen").
    RecentlySeen,
    /// We cannot determine the peer's state (no identity, no presence data).
    Unknown,
}

impl PeerPresence {
    fn label(&self) -> &'static str {
        match self {
            PeerPresence::Online => "Online",
            PeerPresence::Away => "Away",
            PeerPresence::Offline => "Offline",
            PeerPresence::Connecting => "Connecting…",
            PeerPresence::RecentlySeen => "Recently seen",
            PeerPresence::Unknown => "Unknown",
        }
    }

    fn color(&self, theme: &iced::Theme) -> Color {
        match self {
            PeerPresence::Online => accent_green(theme),
            PeerPresence::Away | PeerPresence::Connecting | PeerPresence::RecentlySeen => {
                color_warning(theme)
            }
            PeerPresence::Offline | PeerPresence::Unknown => text_muted(theme),
        }
    }

    fn icon(&self) -> &'static [u8] {
        match self {
            PeerPresence::Online | PeerPresence::Away => ICON_ONLINE,
            PeerPresence::Connecting => ICON_RETRY,
            PeerPresence::RecentlySeen => ICON_OFFLINE,
            PeerPresence::Offline | PeerPresence::Unknown => ICON_OFFLINE,
        }
    }
}

/// Map the BORU-CP-05 backend connectivity state machine onto the four
/// PDF 2.3 presence labels (Online / Recently seen / Connecting /
/// Offline). Pure so it can be unit-tested in isolation.
///
/// - `Reachable` / `DirectTopicReady` → Online (`is_online()`)
/// - `Discovered` → Recently seen (seen on discovery, not ready for direct)
/// - `Connecting` → Connecting
/// - `Degraded` / `OfflineStale` / `Unknown` → Offline (never 'online')
fn peer_presence_from_connectivity(state: PeerConnectivityState) -> PeerPresence {
    match state {
        PeerConnectivityState::Reachable | PeerConnectivityState::DirectTopicReady => {
            PeerPresence::Online
        }
        PeerConnectivityState::Discovered => PeerPresence::RecentlySeen,
        PeerConnectivityState::Connecting => PeerPresence::Connecting,
        PeerConnectivityState::Degraded
        | PeerConnectivityState::OfflineStale
        | PeerConnectivityState::Unknown => PeerPresence::Offline,
    }
}










#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatKind {
    System,
    Local,
    Remote,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum InlinePlaybackErrorKind {
    UnsupportedCodec,
    CorruptFile,
    MissingFile,
    PermissionDenied,
    Initialization,
    Unknown,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct InlinePlaybackError {
    pub(crate) kind: InlinePlaybackErrorKind,
    /// Short backend detail retained for diagnostics; never shown as a path.
    pub(crate) detail: String,
}

impl InlinePlaybackError {
    pub(crate) fn from_backend(error: impl Into<String>) -> Self {
        let raw_detail = error.into();
        let lower = raw_detail.to_ascii_lowercase();
        let kind = if lower.contains("codec")
            || lower.contains("decoder")
            || lower.contains("not-negotiated")
            || lower.contains("cannot decode")
        {
            InlinePlaybackErrorKind::UnsupportedCodec
        } else if lower.contains("not found")
            || lower.contains("missing")
            || lower.contains("no such file")
        {
            InlinePlaybackErrorKind::MissingFile
        } else if lower.contains("permission") || lower.contains("access denied") {
            InlinePlaybackErrorKind::PermissionDenied
        } else if lower.contains("corrupt")
            || lower.contains("invalid")
            || lower.contains("parse")
            || lower.contains("malformed")
        {
            InlinePlaybackErrorKind::CorruptFile
        } else if lower.contains("init")
            || lower.contains("construct")
            || lower.contains("open")
            || lower.contains("resource")
        {
            InlinePlaybackErrorKind::Initialization
        } else {
            InlinePlaybackErrorKind::Unknown
        };
        // Backend messages can contain local paths or peer-controlled names.
        // Keep a bounded diagnostic without carrying those values into logs.
        let detail = raw_detail
            .split_whitespace()
            .map(|token| {
                if token.contains('/') || token.contains('\\') {
                    "<path>"
                } else {
                    token
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            kind,
            detail: detail.chars().take(240).collect(),
        }
    }

    pub(crate) fn title(&self) -> &'static str {
        match self.kind {
            InlinePlaybackErrorKind::UnsupportedCodec => "Video format not supported",
            InlinePlaybackErrorKind::CorruptFile => "Video file appears damaged",
            InlinePlaybackErrorKind::MissingFile => "Video file is missing",
            InlinePlaybackErrorKind::PermissionDenied => "Video cannot be opened",
            InlinePlaybackErrorKind::Initialization => "Video player could not start",
            InlinePlaybackErrorKind::Unknown => "Video playback failed",
        }
    }

    pub(crate) fn message(&self) -> &'static str {
        match self.kind {
            InlinePlaybackErrorKind::UnsupportedCodec => {
                "This packaged player cannot decode this video's format."
            }
            InlinePlaybackErrorKind::CorruptFile => "The attachment is not a valid playable video.",
            InlinePlaybackErrorKind::MissingFile => {
                "The saved attachment is no longer on this device."
            }
            InlinePlaybackErrorKind::PermissionDenied => {
                "The saved attachment could not be accessed."
            }
            InlinePlaybackErrorKind::Initialization => {
                "Try again without downloading the attachment again."
            }
            InlinePlaybackErrorKind::Unknown => {
                "The attachment is still available for saving or opening externally."
            }
        }
    }

    pub(crate) fn retry_available(&self) -> bool {
        matches!(
            self.kind,
            InlinePlaybackErrorKind::Initialization | InlinePlaybackErrorKind::Unknown
        )
    }
}

#[derive(Clone, Debug)]
pub struct ChatEntry {
    kind: ChatKind,
    label: String,
    body: String,
    /// Protocol message content hash, for edit/delete/reaction matching.
    message_hash: Option<MessageHash>,
    /// Whether this entry has been edited after initial delivery.
    edited: bool,
    /// Emoji reactions attached to this entry.
    reactions: Vec<String>,
    /// Cached formatted label text, e.g. \"[Alice]\" or \"[Alice ✓]\"
    /// Avoids format!() allocation on every render frame.
    label_text: Option<String>,
    /// Cached joined reaction emoji string, e.g. \"👍  ❤️\"
    /// Avoids reactions.join() allocation on every render frame.
    reactions_text: Option<String>,
    /// Cached formatted message timestamp, e.g. \"12:34\" or \"Mon 12:34\" or \"Jan 5\".
    /// Computed once from the UTC timestamp when the entry is created or its
    /// timestamp changes — avoids calling `format_message_time` on every frame.
    formatted_time: Option<String>,
    /// Cached iced image handle, decoded once at construction time.
    /// Cloning is cheap (Arc<..>) — avoids re-decoding JPEG bytes on every frame.
    image_handle: Option<iced::widget::image::Handle>,
    /// Cached iced handle for this entry's sender avatar (profile picture).
    /// Populated once in `entries_push` from `friend_image_handles` so the
    /// view function never does a per-frame HashMap lookup.
    avatar_handle: Option<iced::widget::image::Handle>,
    /// Compressed image bytes for inline rendering, if this is an image message.
    /// Kept for session-history/replay persistence; the `image_handle` is used
    /// during rendering to avoid re-decoding on every frame.
    image_bytes: Option<Vec<u8>>,
    /// Storage identifier returned by the [`ImageStore`] for this image.
    /// Relative path within the store's files root — never an absolute filesystem path.
    /// Set when the image is persisted via `ImageStore::save_image()`.
    image_identifier: Option<String>,
    /// Non-fatal rendering / persistence error to show inline with the image.
    image_error: Option<String>,
    /// Original image pixel width, extracted from image bytes at construction time.
    /// Used to compute the display size (scale-to-fit) during rendering.
    image_width: Option<u32>,
    /// Original image pixel height, extracted from image bytes at construction time.
    /// Used to compute the display size (scale-to-fit) during rendering.
    image_height: Option<u32>,
    /// Animated GIF frames (raw RGBA handles + per-frame delays) managed by
    /// the iced-moving-picture `Gif` widget. None for static images and
    /// single-frame GIFs (those render as static images).
    gif_frames: Option<std::sync::Arc<iced_moving_picture::widget::gif::Frames>>,
    /// Unix epoch milliseconds when this message was sent (protocol sent_at
    /// for remote messages, local creation time for system/local messages).
    timestamp: Option<i64>,
    /// Stable event id for delivery state tracking (0 = unassigned).
    event_id: u64,
    /// Current delivery state of this message (only meaningful for Local kind).
    delivery_state: DeliveryState,
    /// PublicKey of the sender (None for local/system messages).
    sender_key: Option<PublicKey>,
    /// Optional download attachment rendered alongside this entry.
    download: Option<DownloadAttachment>,
    /// Generation counter bumped on every mutation to this entry's visible
    /// content.  Used by the view-layer widget cache to detect stale cached
    /// elements: when the current entry's gen differs from the cached gen,
    /// the entry's widget tree is rebuilt.
    widget_gen: u64,
    /// Cached link preview data for URLs in this message body.
    /// Set asynchronously by the link preview fetcher.
    link_preview: Option<link_preview::LinkPreviewData>,
    /// True while a link preview fetch is in flight for this entry.
    link_preview_loading: bool,
    /// Whether a link preview fetch for this entry failed.
    link_preview_error: bool,
    /// Cached URL segments parsed from the message body.
    /// Computed once when the entry is created or body changes.
    /// Avoids calling `link_preview::parse_url_segments` on every render frame.
    parsed_segments: Option<Vec<link_preview::TextSegment>>,
}

/// Maximum size of an external catalogue GIF media file we will download
/// (15 MiB — playback renditions are usually a few MB, this headroom covers
/// larger MP4 renditions while still bounding memory).
const GIF_MEDIA_MAX_BYTES: usize = 15 * 1024 * 1024;

/// Fetch external catalogue GIF media bytes over HTTP.
///
/// Used by the SharedGif receive path: the payload carries direct media
/// URLs, so the receiver fetches the rendition directly and never calls the
/// provider search endpoint again.  Bounded by [`GIF_MEDIA_MAX_BYTES`] and
/// an 8-second timeout so a missing or expired URL fails fast and the UI
/// can render a clear fallback.
///
/// # Privacy
/// The request uses a neutral browser-like `User-Agent` (no Boru branding,
/// no peer/room identifiers) so the media CDN cannot attribute the fetch to
/// a specific Boru identity.
async fn fetch_gif_media_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| format!("gif media client: {e}"))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("fetch: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("read body: {e}"))?
    {
        if body.len() + chunk.len() > GIF_MEDIA_MAX_BYTES {
            return Err(format!("media exceeds {GIF_MEDIA_MAX_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    if body.is_empty() {
        return Err("empty media body".to_string());
    }
    Ok(body)
}


/// Decode an animated GIF into iced-moving-picture `Frames` (raw RGBA
/// handles + per-frame delays). Returns None if the image is not a GIF or has
/// only one frame — single-frame GIFs render as static images.
fn decode_gif_frames(
    image_bytes: &[u8],
) -> Option<std::sync::Arc<iced_moving_picture::widget::gif::Frames>> {
    use image::codecs::gif::GifDecoder;
    use image::AnimationDecoder;
    use std::io::Cursor;

    // First pass: count frames. A single-frame GIF must stay on the static
    // image path — the Gif widget would otherwise keep requesting redraws
    // forever (its update loop re-schedules at the frame delay even when the
    // frame never changes).
    let decoder = GifDecoder::new(Cursor::new(image_bytes)).ok()?;
    let frame_count = decoder.into_frames().count();
    if frame_count <= 1 {
        return None;
    }

    // Second pass: decode once into raw RGBA handles. The widget advances
    // frames itself via per-frame delay + request_redraw_at, so there is no
    // PNG encode→decode cycle and no global tick.
    let frames = iced_moving_picture::widget::gif::Frames::from_bytes(image_bytes.to_vec()).ok()?;
    Some(std::sync::Arc::new(frames))
}

impl ChatEntry {
    fn system(text: impl Into<String>) -> Self {
        let mut s = Self {
            kind: ChatKind::System,
            label: "System".into(),
            body: sanitize_display_text(&text.into(), DEFAULT_MAX_DISPLAY_LENGTH),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            image_handle: None,
            avatar_handle: None,
            image_bytes: None,
            image_identifier: None,
            image_error: None,
            image_width: None,
            image_height: None,
            gif_frames: None,
            timestamp: Some(now_ms()),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: None,
            download: None,
            widget_gen: 0,
            label_text: None,
            reactions_text: None,
            formatted_time: None,
            link_preview: None,
            link_preview_loading: false,
            link_preview_error: false,
            parsed_segments: None,
        };
        s.update_cache();
        s
    }
    fn local(label: impl Into<String>, text: impl Into<String>) -> Self {
        let label = sanitize_single_line(&label.into());
        let text = sanitize_display_text(&text.into(), DEFAULT_MAX_DISPLAY_LENGTH);
        Self {
            kind: ChatKind::Local,
            label,
            body: text,
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            image_handle: None,
            avatar_handle: None,
            image_bytes: None,
            image_identifier: None,
            image_error: None,
            image_width: None,
            image_height: None,
            gif_frames: None,
            timestamp: Some(now_ms()),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: None,
            download: None,
            widget_gen: 0,
            label_text: None,
            reactions_text: None,
            formatted_time: None,
            link_preview: None,
            link_preview_loading: false,
            link_preview_error: false,
            parsed_segments: None,
        }
    }
    fn remote(
        label: impl Into<String>,
        text: impl Into<String>,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
        sender: Option<PublicKey>,
    ) -> Self {
        Self {
            kind: ChatKind::Remote,
            label: sanitize_single_line(&label.into()),
            body: sanitize_display_text(&text.into(), DEFAULT_MAX_DISPLAY_LENGTH),
            message_hash: hash,
            edited: false,
            reactions: Vec::new(),
            image_handle: None,
            avatar_handle: None,
            image_bytes: None,
            image_identifier: None,
            image_error: None,
            image_width: None,
            image_height: None,
            gif_frames: None,
            timestamp: sent_at_secs.map(|s| s as i64 * 1000),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: sender,
            download: None,
            widget_gen: 0,
            label_text: None,
            reactions_text: None,
            formatted_time: None,
            link_preview: None,
            link_preview_loading: false,
            link_preview_error: false,
            parsed_segments: None,
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn image(
        kind: ChatKind,
        label: impl Into<String>,
        body: impl Into<String>,
        image_bytes: Vec<u8>,
        hash: Option<MessageHash>,
        sent_at_secs: Option<u64>,
        sender: Option<PublicKey>,
        image_identifier: Option<String>,
        image_error: Option<String>,
    ) -> Self {
        // Extract original image dimensions for scale-to-fit rendering.
        let (img_w, img_h) = {
            use std::io::Cursor;
            image::ImageReader::new(Cursor::new(&image_bytes))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok())
                .map(|(w, h)| (Some(w), Some(h)))
                .unwrap_or((None, None))
        };
        Self {
            kind,
            label: sanitize_single_line(&label.into()),
            body: sanitize_display_text(&body.into(), DEFAULT_MAX_DISPLAY_LENGTH),
            message_hash: hash,
            edited: false,
            reactions: Vec::new(),
            image_handle: Some(iced::widget::image::Handle::from_bytes(image_bytes.clone())),
            avatar_handle: None,
            image_bytes: Some(image_bytes.clone()), // Keep for session history/replay
            image_identifier,
            image_error,
            image_width: img_w,
            image_height: img_h,
            gif_frames: decode_gif_frames(&image_bytes),
            timestamp: sent_at_secs.map(|s| s as i64 * 1000),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: sender,
            download: None,
            widget_gen: 0,
            label_text: None,
            reactions_text: None,
            formatted_time: None,
            link_preview: None,
            link_preview_loading: false,
            link_preview_error: false,
            parsed_segments: None,
        }
    }

    fn system_download(
        text: impl Into<String>,
        kind: TransferKind,
        name: impl Into<String>,
        ticket: impl Into<String>,
        source_peer: impl Into<String>,
        thumbnail: Option<Vec<u8>>,
    ) -> Self {
        Self {
            kind: ChatKind::System,
            label: "System".into(),
            body: sanitize_display_text(&text.into(), DEFAULT_MAX_DISPLAY_LENGTH),
            message_hash: None,
            edited: false,
            reactions: Vec::new(),
            image_handle: None,
            avatar_handle: None,
            image_bytes: None,
            image_identifier: None,
            image_error: None,
            image_width: None,
            image_height: None,
            gif_frames: None,
            timestamp: Some(now_ms()),
            event_id: 0,
            delivery_state: DeliveryState::default(),
            sender_key: None,
            download: Some(DownloadAttachment::new(
                kind,
                name,
                ticket,
                source_peer,
                thumbnail,
            )),
            widget_gen: 0,
            label_text: None,
            reactions_text: None,
            formatted_time: None,
            link_preview: None,
            link_preview_loading: false,
            link_preview_error: false,
            parsed_segments: None,
        }
    }

    /// Returns true when conservative attachment classification identifies a video.
    fn is_video_file(name: &str) -> bool {
        classify_attachment(None, name) == MediaKind::Video
    }

    #[expect(dead_code)]
    fn estimated_height(&self) -> f32 {
        LayoutCache::compute_height(self, None, TYPO_SM, 1024.0)
    }

    /// Override the timestamp with a specific Unix epoch millisecond value.
    #[expect(dead_code)]
    fn with_timestamp(mut self, ms: Option<i64>) -> Self {
        self.timestamp = ms;
        self
    }

    /// Mark this entry's visible content as changed, invalidating any cached
    /// widget tree in the renderer.  Call this after every mutation to fields
    /// that affect the on-screen rendering (body, label, delivery_state,
    /// reactions, image_handle, image_error, etc.).
    fn bump_gen(&mut self) {
        self.widget_gen += 1;
        self.update_cache();
    }

    /// Recompute cached display strings used by the renderer.
    /// Call whenever label, delivery_state, or reactions change.
    fn update_cache(&mut self) {
        self.label_text = if matches!(self.kind, ChatKind::Local) && self.event_id > 0 {
            let icon = self.delivery_state.display_icon();
            // The Seen (eye) icon is intentionally omitted from the label
            // per UX spec — it adds visual noise without actionable value.
            if icon == "\u{1F441}" {
                Some(self.label.clone())
            } else {
                Some(format!("{} {}", self.label, icon))
            }
        } else {
            Some(self.label.clone())
        };
        self.reactions_text = if self.reactions.is_empty() {
            None
        } else {
            Some(self.reactions.join("  "))
        };
        self.parsed_segments = Some(link_preview::parse_url_segments(&self.body));
        // Cache the formatted timestamp once so the bubble metadata row never
        // re-formats on every frame.  `entries_push` calls `update_cache` for
        // every entry (live and replayed), so the timestamp always renders.
        self.formatted_time = self.timestamp.map(format_message_time);
    }
}

// ── Screen navigation ─────────────────────────────────────────────────

/// The active view in the main panel.
///
/// The sidebar (chat list, friends, requests) is always visible regardless
/// of the active screen — only the right-hand main panel changes.
///
/// `Hash` is required so [`IcedChat::prewarm_cache`] can key pre-built
/// screen trees by screen (all payload types — `TopicId`, `PublicKey` — are
/// Hash).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Screen {
    /// No chat selected — empty state shown in the main panel.
    ChatList,
    /// File sharing dashboard — five-tab screen (see docs/file-sharing-guide.md).
    FileSharing,
    /// Download Manager — all active transfers in both directions
    /// (inbound downloads + outbound uploads) with pause/resume/cancel/stop.
    DownloadManager,
    /// An individual chat room with a given topic.
    Chat { topic: TopicId },
    /// Outgoing call ringing view.
    OutgoingCall,
    /// Active audio-only call view.
    ActiveCall,
    /// The friend request management screen.
    FriendRequests,
    /// Application settings screen.
    Settings,
    /// Peer profile overlay — shows shared files with Download buttons.
    PeerProfile(PublicKey),
    /// Remote file catalogue browsing — shows a peer's shared file catalogue.
    PeerCatalogue(PublicKey),
    /// Redesigned friend profile view with context menu and action buttons.
    FriendProfile(PublicKey),
    /// Public room directory — browse advertised rooms from the current relay.
    Discover,
    /// Group list — shows known groups and a create-group button.
    Groups,
    /// Embedded terminal tab (feature `terminal`).
    #[cfg(feature = "terminal")]
    Terminal,
    /// Developer component gallery — dev-ui only, excluded from release navigation.
    #[cfg(feature = "dev-ui")]
    Gallery,
}


// ── State-safety snapshots ─────────────────────────────────────────────

/// Atomic snapshot of the room a join was initiated for, captured when the
/// async subscription task is spawned. Mirrors telepathy's
/// `CallSlotSnapshot { state, direct_peer, generation }`: the `generation`
/// token is bumped on every room-join initiation, so a stale completion
/// (e.g. `RoomOpened` delivered for a room the user has since left or
/// switched away from) can be detected in debug builds before it clobbers
/// the newer room's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomSnapshot {
    pub topic: TopicId,
    pub generation: u64,
}

// ── Per-conversation runtime state ─────────────────────────────────────

/// Runtime state for a single live conversation (active or background).
///
/// Each conversation has its own gossip subscription, forwarder task,
/// message entries, name cache, and composer state. Background
/// conversations keep their forwarder alive when the user switches to
/// another conversation — only the UI display swaps.
#[derive(Debug)]
pub struct ConversationLive {
    // ── Subscription ──
    /// Gossip message sender for this conversation.
    pub sender: Option<GossipSender>,
    /// Optional forward-handle; kept alive so background subscriptions
    /// continue to receive events.
    pub forward_handle: Option<n0_future::task::JoinHandle<()>>,
    /// Pending forwarder handle awaiting transfer.
    pub forward_handle_slot: Arc<StdMutex<Option<n0_future::task::JoinHandle<()>>>>,
    /// The gossip topic for this conversation.
    #[expect(dead_code)]
    pub topic: TopicId,
    /// Ticket string for sharing this conversation.
    pub ticket_str: String,

    // ── Chat state ──
    /// Chat messages for this conversation.
    pub entries: Vec<ChatEntry>,
    /// Composer input text.
    pub composer_text: String,
    /// Whether to auto-scroll to the latest message.
    pub follow_latest: bool,
    /// Name cache: peer PublicKey → display name.
    pub names: HashMap<PublicKey, String>,
    /// Maps content hash to stable event id for self-sent messages.
    pub self_sent_events: HashMap<MessageHash, u64>,
    /// Maintained indexes into this conversation's entries.
    pub event_id_to_index: HashMap<u64, usize>,
    pub message_hash_to_index: HashMap<MessageHash, usize>,
    /// Number of entries already saved to ChatHistoryStore.
    pub history_saved_count: usize,
    /// Cached layout for the chat log.
    #[expect(dead_code)]
    pub layout_cache: std::cell::RefCell<LayoutCache>,
    /// Y scroll offset.
    pub scroll_offset: f32,
    /// Viewport height.
    pub viewport_height: f32,
    /// Per-entry generation tracker for widget cache invalidation.
    /// Resized to match `entries` length on every mutation; a mismatch between
    /// `widget_gen[i]` and `self.entries[i].widget_gen` means the cached widget
    /// tree for entry `i` is stale.
    #[expect(dead_code)]
    pub entry_widget_gen: Vec<u64>,

    // ── Downloads ──
    /// Pending file download info: (filename, ticket_string).
    pub pending_file: Option<(String, String)>,
    /// Pending image downloads queue.
    pub pending_image: VecDeque<(String, MessageHash, PublicKey)>,
    /// Index of the chat entry with the active download.
    pub download_entry_index: Option<usize>,
    /// Transfer ID for the active download.
    pub active_download_transfer_id: Option<TransferId>,
    /// TransferId → entry index cache for O(1) progress update lookups.
    pub transfer_id_to_index: HashMap<TransferId, usize>,

    // ── Network peers ──
    /// Set of gossip neighbors for this conversation.
    pub neighbors: HashSet<PublicKey>,
    /// Whether this conversation has a usable gossip sender. True when
    /// subscribed AND the subscription has yielded a valid sender handle.
    /// Cleared when the conversation is left or re-subscription is needed.
    pub sender_ready: bool,
    /// Events received while this conversation is not selected.
    pub pending_events: VecDeque<NetEvent>,
    /// Number of unread events received while hidden.
    pub unread: u64,
    /// Whether persisted history has been loaded from ChatHistoryStore
    /// and replayed into this conversation's entries.
    pub history_loaded: bool,
}

impl ConversationLive {
    /// Create a new live conversation for the given topic.
    fn new(topic: TopicId) -> Self {
        Self {
            sender: None,
            sender_ready: false,
            forward_handle: None,
            forward_handle_slot: Arc::new(StdMutex::new(None)),
            topic,
            ticket_str: String::new(),
            entries: Vec::new(),
            composer_text: String::new(),
            follow_latest: true,
            names: HashMap::new(),
            self_sent_events: HashMap::new(),
            event_id_to_index: HashMap::new(),
            message_hash_to_index: HashMap::new(),
            history_saved_count: 0,
            layout_cache: std::cell::RefCell::new(LayoutCache::new(TYPO_SM)),
            scroll_offset: 0.0,
            viewport_height: 0.0,
            entry_widget_gen: Vec::new(),
            pending_file: None,
            pending_image: VecDeque::new(),
            download_entry_index: None,
            active_download_transfer_id: None,
            transfer_id_to_index: HashMap::new(),
            neighbors: HashSet::new(),
            pending_events: VecDeque::new(),
            unread: 0,
            history_loaded: false,
        }
    }

    /// Convenience: read the gossip sender, or `None` if not yet subscribed.
    #[expect(dead_code)]
    fn sender(&self) -> Option<&GossipSender> {
        self.sender.as_ref()
    }
}

// ── Application state ─────────────────────────────────────────────────

/// Kind of entry for right-click context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextMenuKind {
    Text,
    Image,
}

/// User-facing message for a provider error (no key, no sensitive data).
fn gif_provider_error_message(error: &GifProviderError) -> String {
    match error {
        GifProviderError::NotConfigured => {
            "GIF search is not configured — set the KLIPY_API_KEY environment variable".to_string()
        }
        GifProviderError::InvalidApiKey => {
            "GIF provider rejected the API key — check KLIPY_API_KEY".to_string()
        }
        GifProviderError::RateLimited { retry_after } => match retry_after {
            Some(secs) => format!("GIF provider rate limited — retry in {secs}s"),
            None => "GIF provider rate limited — try again shortly".to_string(),
        },
        GifProviderError::Timeout => "GIF search timed out — try again".to_string(),
        GifProviderError::Network { details } => {
            format!(
                "GIF search network error: {}",
                sanitize_gif_error_details(details)
            )
        }
        GifProviderError::InvalidResponse { details } => {
            format!(
                "GIF provider returned an invalid response: {}",
                sanitize_gif_error_details(details)
            )
        }
        GifProviderError::MediaUnavailable { details } => {
            format!(
                "GIF media unavailable: {}",
                sanitize_gif_error_details(details)
            )
        }
        GifProviderError::Cancelled => "GIF search cancelled".to_string(),
        GifProviderError::Other { details } => {
            format!("GIF search failed: {}", sanitize_gif_error_details(details))
        }
    }
}

/// Strip URL-looking substrings from provider error details as a defensive
/// measure.  The KLIPY provider already redacts its request URL (and the
/// app-level message should never echo a search query or API key), but a
/// future provider might embed a URL in its `details`; never surface that
/// to the user or logs.
fn sanitize_gif_error_details(details: &str) -> String {
    let mut out = String::with_capacity(details.len());
    let mut rest = details;
    while let Some(start) = rest.find("http://").or_else(|| rest.find("https://")) {
        out.push_str(&rest[..start]);
        // Skip the URL token (up to whitespace).
        let after = &rest[start..];
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        out.push_str("[redacted URL]");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

// ── Pre-warm (PERF-4R-B) ─────────────────────────────────────────────

/// Tracks the last user input so screen pre-warming only runs while the
/// app is genuinely idle (no keyboard/mouse activity for 2+ seconds).
struct IdleTimer {
    last_input: std::time::Instant,
}

impl IdleTimer {
    /// Seconds of inactivity after which the app is considered idle.
    const IDLE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(2);

    fn new() -> Self {
        // Start counting from construction so the pre-warm cycle can begin
        // shortly after launch once the initial burst of startup events ends.
        Self {
            last_input: std::time::Instant::now(),
        }
    }

    /// Record user activity (any keyboard/mouse event).
    fn note_activity(&mut self) {
        self.last_input = std::time::Instant::now();
    }

    /// True when no user input has arrived for the idle threshold.
    fn is_idle(&self) -> bool {
        self.last_input.elapsed() >= Self::IDLE_THRESHOLD
    }
}

/// Responsive layout band. Pre-warmed screen trees are built for a specific
/// band and are invalidated when the window crosses a breakpoint, because
/// window width is not part of the per-screen dependency snapshots (except
/// FileSharing's own `FileSharingResponsiveMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponsiveMode {
    Compact,
    Medium,
    Reference,
    Large,
}

impl ResponsiveMode {
    fn of(width: f32) -> Self {
        use crate::design_tokens::{VIEWPORT_LG_WIDTH, VIEWPORT_MIN_WIDTH, VIEWPORT_REF_WIDTH};
        if width <= VIEWPORT_MIN_WIDTH {
            ResponsiveMode::Compact
        } else if width < VIEWPORT_REF_WIDTH {
            ResponsiveMode::Medium
        } else if width < VIEWPORT_LG_WIDTH {
            ResponsiveMode::Reference
        } else {
            ResponsiveMode::Large
        }
    }
}

/// Screens pre-warmed into the app-state cache during idle, in order of
/// likelihood of use (heaviest / most-used first). PeerProfile,
/// PeerCatalogue, FriendProfile and Chat are excluded — they carry per-peer
/// or per-room payloads, so a pre-warmed tree would almost never match the
/// requested payload and would only waste idle cycles.
const PREWARM_ORDER: &[Screen] = &[
    Screen::FileSharing,
    Screen::Settings,
    Screen::Discover,
    Screen::Groups,
    Screen::FriendRequests,
];

/// FxHash a dependency snapshot so the pre-warm cache key tracks exactly what
/// the tree was built from. Uses the same hasher as `iced_widget::lazy` so the
/// two caches agree on what counts as "unchanged".
fn fxhash_of<T: std::hash::Hash>(value: &T) -> u64 {
    use std::hash::Hasher;
    let mut hasher = rustc_hash::FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

/// A tiny delegating widget that hands out a pre-built [`iced::Element`] tree
/// each frame without rebuilding it. `view()` serves pre-warmed screens by
/// wrapping the cached element (shared via `Rc`) in this widget; the widget
/// forwards `layout`/`draw`/`update`/`operate`/`mouse_interaction`/`children`
/// to the inner element, following the exact pattern `iced_widget::lazy` uses
/// for its cached element (`Rc<RefCell<Element>>`).
///
/// `tag()` forwards to the inner content's tag: stable across frames for the
/// same screen (so iced keeps the cached subtree's widget state — scroll
/// offsets, text cursors) and distinct between screens (so switching screens
/// rebuilds instead of reusing the wrong tree).
struct Prebuilt(std::rc::Rc<std::cell::RefCell<iced::Element<'static, AppMessage>>>);

/// Tree state for [`Prebuilt`]: remembers which cached element the child
/// tree was built from so `diff` can reconcile it when the pre-warm cache
/// serves a freshly rebuilt element.
struct PrebuiltState(std::rc::Rc<std::cell::RefCell<iced::Element<'static, AppMessage>>>);

impl iced::advanced::Widget<AppMessage, iced::Theme, iced::Renderer> for Prebuilt {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        // Stable tag for the wrapper itself — deliberately NOT forwarded
        // from the inner content. Forwarding made iced treat a Prebuilt
        // wrapping e.g. a Container as the same widget as a plain live
        // Container at that tree position; when the position was occupied
        // by a live view (state `State::None`) and then served a
        // pre-warmed element, `diff` downcast a stateless tree → panic
        // ("Downcast on stateless state" on screen switch to Settings).
        struct PrebuiltTag;
        iced::advanced::widget::tree::Tag::of::<PrebuiltTag>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(PrebuiltState(self.0.clone()))
    }

    fn children(&self) -> Vec<iced::advanced::widget::tree::Tree> {
        vec![iced::advanced::widget::tree::Tree::new(
            self.0.borrow().as_widget(),
        )]
    }

    fn diff(&self, tree: &mut iced::advanced::widget::tree::Tree) {
        let state = tree.state.downcast_mut::<PrebuiltState>();
        if !std::rc::Rc::ptr_eq(&state.0, &self.0) {
            // The cached element was replaced (invalidate_prewarm +
            // pre_warm_next_screen rebuilt it, possibly with a different
            // structure). Reconcile the child tree against the new content
            // so per-widget state (e.g. Text paragraphs) matches the new
            // layout; otherwise iced downcasts stale state and panics.
            state.0 = self.0.clone();
            tree.diff_children(std::slice::from_ref(&self.0.borrow().as_widget()));
        }
        // Same Rc as last frame: the cached element is unchanged, so leave
        // the child tree untouched to preserve widget state inside it.
    }

    fn size(&self) -> iced::Size<iced::Length> {
        self.0.borrow().as_widget().size()
    }

    fn size_hint(&self) -> iced::Size<iced::Length> {
        self.0.borrow().as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::tree::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        self.0
            .borrow_mut()
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::tree::Tree,
        layout: iced::advanced::Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.0.borrow_mut().as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::tree::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn iced::advanced::Clipboard,
        shell: &mut iced::advanced::Shell<'_, AppMessage>,
        viewport: &iced::Rectangle,
    ) {
        self.0.borrow_mut().as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::tree::Tree,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.0.borrow().as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::tree::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.0.borrow().as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    // No overlay forwarding: the pre-warmable content functions contain no
    // overlay-producing widgets (verified: no tooltip/modal/popup in the five
    // static content fns), and the app's overlays (create-group dialog,
    // lightbox, help, connection details) compose at the top-level `view()`
    // layer, never inside the cached trees. Forwarding an overlay would
    // require the self-referential borrow `iced_widget::lazy` solves with
    // ouroboros; returning `None` is correct for the cached content.
    fn overlay<'b>(
        &'b mut self,
        _tree: &'b mut iced::advanced::widget::tree::Tree,
        _layout: iced::advanced::Layout<'b>,
        _renderer: &iced::Renderer,
        _viewport: &iced::Rectangle,
        _translation: iced::Vector,
    ) -> Option<iced::advanced::overlay::Element<'b, AppMessage, iced::Theme, iced::Renderer>> {
        None
    }
}

// ── Application state ────────────────────────────────────────────

pub struct IcedChat {
    // ── Navigation ──
    pub screen: Screen,
    /// Embedded terminal tab (feature `terminal`). Spawned eagerly with the
    /// platform shell so the tab is ready the first time it is opened.
    /// `None` when the PTY/shell could not be spawned (e.g. a missing shell
    /// on Windows) — the rest of the app must still run normally.
    #[cfg(feature = "terminal")]
    pub terminal: Option<TerminalTab>,
    /// Track which image is currently shown in the full-screen lightbox overlay.
    lightbox_image: Option<usize>,
    /// Guard counter for the stale-`Scrolled` race when the lightbox closes.
    /// The windowed scrollable is re-created (its tree path changes because
    /// the overlay is a `stack![base, overlay]` wrapper), and the fresh
    /// widget's initial `Scrolled(0, vp)` event would clobber the `f32::MAX`
    /// bottom sentinel before the snap task lands.  While > 0, non-bottom
    /// `Scrolled` events keep the sentinel armed and re-queue the snap;
    /// a bottom event confirms the snap landed and disarms the guard.
    lightbox_close_snap_guard: u8,
    /// Pending topic we're connecting to (used during the async handoff
    /// from clicking a room to actually subscribing).
    pending_topic: Option<TopicId>,
    /// True while the async gossip subscription for a room is in flight.
    /// Shows a spinner in the chat view instead of an empty panel.
    pub room_loading: bool,
    /// Monotonic ownership token for the active room. Bumped on every room
    /// join initiation (OpenRoom slow path, JoinFromTicket, private-room
    /// creation, group creation). Async subscription completions carry the
    /// generation they were spawned under; handlers assert the current
    /// generation still matches before applying the result, so a stale
    /// completion for a room the user has since left or switched away from
    /// is detected in debug builds instead of clobbering the newer room.
    room_generation: u64,
    /// Monotonic ownership token for the active conversation. Bumped on
    /// every active-conversation switch (`switch_to_conversation` and the
    /// `RoomOpened` apply path). Async completions that mutate the active
    /// conversation's display (e.g. `ImageDownloaded`) carry the generation
    /// they were started under; the handler asserts it still matches before
    /// applying, so a completion that resolves after a room switch cannot
    /// silently mutate the wrong conversation's entries.
    conversation_generation: u64,
    /// Screen to return to when closing the settings page.
    settings_return_to: Option<Screen>,
    /// Screen to return to when closing the friend requests page.
    friend_requests_return_to: Option<Screen>,
    /// Screen to return to when closing a peer profile / remote catalogue
    /// (both share the `ClosePeerProfile` handler).
    peer_profile_return_to: Option<Screen>,
    /// Screen to return to when closing the friend profile page.
    friend_profile_return_to: Option<Screen>,
    /// Screen to return to when closing the Discover (public rooms) page.
    discover_return_to: Option<Screen>,
    /// BORU-DIR-15 (PDF Task 5.3): Discover browse-surface search query.
    /// Local-only — never broadcast onto the discovery network.
    discover_search_query: String,
    /// BORU-DIR-15: filter toggles (Compatible / Not Joined / Recently
    /// Seen). All applied against the local cache snapshot only.
    discover_filter_compatible: bool,
    discover_filter_not_joined: bool,
    discover_filter_recently_seen: bool,
    /// BORU-DIR-15: selected tag/category filters (OR semantics).
    discover_selected_tags: Vec<String>,
    /// BORU-DIR-15: sort order for the browse surface.
    discover_sort: DiscoverSort,
    /// Screen to return to when closing the Groups page.
    groups_return_to: Option<Screen>,
    /// Screen to return to when closing the Download Manager page.
    download_manager_return_to: Option<Screen>,

    // ── Pre-warm (PERF-4R-B) ──
    /// Pre-built screen trees keyed by screen; the `u64` is the FxHash of the
    /// screen's dependency snapshot the tree was built from. The element is
    /// stored behind `Rc<RefCell<..>>` (same shape as `iced_widget::lazy`'s
    /// cached element) so the delegating [`Prebuilt`] widget can forward the
    /// `&mut self` widget methods into the cached tree each frame.
    prewarm_cache: std::collections::HashMap<
        Screen,
        (
            u64,
            std::rc::Rc<std::cell::RefCell<iced::Element<'static, AppMessage>>>,
        ),
    >,
    /// Whether a pre-warm build is in progress (guards re-entrancy).
    prewarming: bool,
    /// Tracks the last user input so pre-warming only runs while idle.
    idle_timer: IdleTimer,
    /// Responsive mode the pre-warm cache was built for; pre-warmed trees
    /// are invalidated when the window crosses a breakpoint.
    prewarm_window_mode: Option<ResponsiveMode>,
    /// BORU-UI-19: theme edits (inspector slider storms, boru-ui.toml
    /// reloads) set this flag instead of clearing `prewarm_cache` on every
    /// event. The actual invalidation is coalesced into the next idle tick
    /// (see `pre_warm_next_screen`), so a slider drag that emits dozens of
    /// messages per second does not churn the prewarm cache. Correctness is
    /// unaffected: `serve_prewarmed` already hash-checks `theme_revision`,
    /// so a stale pre-warmed tree is never served after a theme change.
    prewarm_invalidate_pending: bool,

    // ── Multi-conversation state ──
    /// Per-conversation runtime state. Each direct chat or group room
    /// keeps its own subscription, entries, and composer.
    conversations: HashMap<TopicId, ConversationLive>,
    /// Reconciled pin state for the active and recently visited rooms.
    pinned_state: PinState,

    // ── ChatList state ──
    room_history: RoomHistoryStore,
    room_history_dirty: bool,
    /// Text input for the "Join via ticket" field in the chat list.
    join_ticket_input: String,
    /// Optional error message shown in the chat list.
    chat_list_error: String,
    /// Currently selected chat list topic, used by cached sidebar rows to
    /// update selection styling without rebuilding row contents.
    sidebar_selected_topic: Rc<Cell<Option<TopicId>>>,
    /// Track sidebar section collapsed state: [chats, groups, friends, discover, requests, public_rooms]
    sidebar_section_collapsed: [bool; 6],
    /// Per-section appearance-animation frame (SIDEBAR-01). A value below
    /// `SIDEBAR_FADE_FRAMES` means the section just gained its first item and
    /// is still animating in; `SplashTick` advances it towards
    /// `SIDEBAR_FADE_FRAMES` (== no animation / idle).
    sidebar_fade_frame: [u32; 6],

    // ── Chat state (active room — display cache) ──
    /// Active conversation topic (display cache).
    topic: TopicId,
    /// Active conversation display name.
    ticket_str: String,
    /// Active conversation entries (display cache).
    entries: Vec<ChatEntry>,
    /// Active conversation composer text.
    composer_text: String,
    /// True while the last submitted composer send task is still in flight
    /// (drives the transient "sending" state on the send button).
    composer_sending: bool,
    /// True while a window file is dragged over the app (drives the subtle
    /// drag-over focus treatment on the composer).
    composer_drag_over: bool,
    /// True while an input-method (IME) composition is active on the composer;
    /// sending is suppressed while composing so Enter commits text instead.
    composer_ime_active: bool,
    /// Ephemeral remote typing leases for the active conversation.
    typing_peers: TypingState,
    /// Local typing emission throttle.
    typing_emitter: TypingEmitter,
    /// Privacy preference: when false, no typing events are sent.
    typing_privacy_enabled: bool,
    /// BORU-APP-002: the help-overlay domain. Owns the chat help overlay's
    /// visibility flag and its overlay view. Previously the bare
    /// `help_visible: bool` field; now the domain owns the state (see
    /// `app/help_overlay.rs` and `app/domain_pattern.md`).
    pub help_overlay: HelpOverlay,
    pending_file: Option<(String, String)>,
    /// Pending image download: (filename, blob_hash, sender_pk).
    pending_image: VecDeque<(String, MessageHash, PublicKey)>,
    /// Pending external catalogue GIF: (payload, sender_pk, message_hash).
    /// Fetched over HTTP from the provider media URL (not the iroh blob
    /// store), then rendered inline — or shown as a clear fallback when the
    /// media cannot be loaded.
    pending_gif: VecDeque<(boru_core::gif_provider::SharedGif, PublicKey, MessageHash)>,
    /// Pending video thumbnail blob fetch: (entry_index, thumbnail_hash, ticket).
    /// The sender publishes a small poster blob and includes its hash in the
    /// FileShare message; receivers fetch it off the UI thread so the card can
    /// show a poster before the full video download finishes.
    pending_thumbnail_fetch: VecDeque<(usize, MessageHash, String)>,
    /// Image selected by the user and currently being processed.
    pending_image_upload: Option<String>,
    /// Animation frame for the inline image-processing spinner.
    image_upload_spinner_frame: usize,
    /// File selected by the user and being uploaded/shared.
    pending_file_upload: Option<(String, u64)>,
    /// Animation frame for the file-upload spinner.
    file_upload_spinner_frame: usize,
    /// Index of the chat entry that owns the current download attachment.
    download_entry_index: Option<usize>,
    /// Transfer ID for the active download, used to keep updates attached to
    /// the correct row even if the view is recreated.
    active_download_transfer_id: Option<TransferId>,
    #[cfg(feature = "video-playback")]
    inline_video: Option<InlineVideoSession>,
    #[cfg(feature = "video-playback")]
    playback_coordinator: PlaybackCoordinator,
    #[cfg(feature = "video-playback")]
    inline_video_seek: Option<f32>,
    #[cfg(feature = "video-playback")]
    inline_video_expanded: bool,
    #[cfg(feature = "video-playback")]
    /// Position retained after an off-screen player is evicted. This is
    /// lightweight UI state; it never owns or removes the attachment file.
    inline_video_resume: Option<(VideoInstanceKey, Duration)>,
    #[cfg(feature = "video-playback")]
    video_runtime: VideoRuntimeCapability,
    /// Live external-player HTTP stream (non-`video-playback` builds, e.g.
    /// Windows). Keeps the [`StreamingServer`] alive while the OS default
    /// player (VLC/browser) consumes the URL; replaced on the next stream and
    /// dropped on room leave. Feature-gated builds park the server in
    /// `InlineVideoSession.streaming_server` instead.
    external_stream_server: Arc<StdMutex<Option<StreamingServer>>>,
    /// TransferId → entry index cache for O(1) progress update lookups.
    /// Populated lazily in handle_download_progress; cleared on room switch.
    transfer_id_to_index: HashMap<TransferId, usize>,
    names: HashMap<PublicKey, String>,
    /// Active conversation gossip sender.
    pub sender: Option<GossipSender>,
    /// Whether the active conversation's sender is usable. True when
    /// subscribed AND the subscription has yielded a valid sender handle.
    /// Cleared when leaving the room or during re-subscription.
    pub sender_ready: bool,
    /// JoinHandle for the active conversation's event forwarder.
    forward_handle: Option<task::JoinHandle<()>>,
    /// Pending forwarder handle slot for async transitions.
    forward_handle_slot: Arc<StdMutex<Option<task::JoinHandle<()>>>>,

    // ── Shared network state ──
    secret_key: SecretKey,
    gossip: Gossip,
    /// Keeps the protocol router alive for the lifetime of the GUI. Dropping
    /// the router stops accepting incoming gossip connections.
    _router: iroh::protocol::Router,
    /// Sender-side direct file offers. Filesystem paths remain process-local;
    /// only the opaque offer ID is announced over gossip.
    pub(crate) file_offer_registry: Arc<StdMutex<boru_core::file_offer::FileOfferRegistry>>,
    blob_store: FsStore,
    endpoint: iroh::Endpoint,
    memory_lookup: MemoryLookup,
    local_label: String,
    local_public: PublicKey,
    relay_mode: RelayMode,
    runtime_handle: tokio::runtime::Handle,
    pub net_rx: Arc<Mutex<Receiver<ConversationNetEvent>>>,
    net_tx: Sender<ConversationNetEvent>,
    /// Secure-tunnel service handle shared with the tunnel protocol handler.
    /// The GUI uses this to create local-service tunnels via the friend
    /// profile "Share local service" dialog.
    tunnel_service: Arc<boru_core::tunnel::service::TunnelService>,
    backfill_handle: boru_core::backfill::BackfillHandle,
    friends: FriendsStore,
    friends_dirty: bool,
    friend_mgr: FriendPingManager,
    pub friend_events_rx: Arc<Mutex<Receiver<FriendEvent>>>,
    /// Set of peer PublicKeys currently connected as gossip neighbors.
    neighbors: HashSet<PublicKey>,
    /// Peers we've already announced as newly-seen this session (via a
    /// presence heartbeat or a neighbor-up).  Prevents duplicate "joined"
    /// / "is online" system messages per peer; cleared on room leave so
    /// re-entering a room announces its peers fresh again.
    known_peers: HashSet<PublicKey>,
    /// Number of gossip neighbors for each subscribed room.
    room_neighbor_counts: HashMap<TopicId, u32>,
    /// Number of peers reachable via a direct (hole-punched) connection.
    direct_peers: usize,
    /// Number of peers connected through a relay server.
    relayed_peers: usize,
    /// Counter for periodic connection refresh (decremented per ConnMonitorTick).
    conn_refresh_counter: u32,
    /// Current mesh health summary from the quiescence watchdog.
    mesh_health: MeshHealth,
    /// Previous mesh health state, used to detect transitions.
    last_mesh_health: Option<MeshHealth>,
    /// When the mesh last reached `MeshHealth::Good` (sender present with
    /// gossip neighbors). Cleared on any non-Good transition. Drives the
    /// connection-time indicator on the home mesh card.
    mesh_connected_at: Option<Instant>,
    /// True until stored conversations have been subscribed after the mesh
    /// first establishes peer connectivity.
    conversation_subscription_pending: bool,
    /// Scrolling log of mesh connection progress events.
    mesh_event_log: std::collections::VecDeque<MeshEvent>,
    /// Counter for periodic presence broadcast (decremented per ConnMonitorTick,
    /// broadcasts Message::Presence when it hits 0, resets to 5).
    presence_counter: u32,
    /// Counter for periodic invisible keepalive heartbeat (decremented per
    /// ConnMonitorTick, broadcasts Message::Heartbeat when it hits 0, resets to 2).
    heartbeat_counter: u32,
    /// Counter for periodic latency ping (decremented per ConnMonitorTick,
    /// broadcasts Message::LatencyPing when it hits 0, resets to 15 ~ every 15s).
    latency_ping_counter: u32,
    /// Measured round-trip latency per peer, populated by latency ping/pong probes.
    peer_latencies: HashMap<PublicKey, Duration>,
    /// Guards against overlapping async connection refresh tasks.
    conn_refresh_in_flight: bool,
    /// Set by on_neighbor_up/on_neighbor_down when a connection count refresh
    /// is needed outside the normal ~60s cycle.
    needs_conn_refresh: bool,
    /// Extra gossip neighbors whose addressing info is embedded in the
    /// regenerated room ticket as additional bootstrap nodes. Populated
    /// asynchronously from [`Self::pending_ticket_peers`] via
    /// `endpoint.remote_info()` inside ConnMonitorTick.
    ticket_extra_peers: Vec<EndpointAddr>,
    /// Neighbor PublicKeys whose addressing info has not yet been resolved.
    /// Resolution is deferred because `endpoint.remote_info()` is async while
    /// `on_neighbor_up` is sync; ConnMonitorTick resolves them and moves the
    /// results into [`Self::ticket_extra_peers`].
    pending_ticket_peers: Vec<PublicKey>,
    /// Set when the mesh membership changed (neighbor up/down or a pending
    /// peer resolved) so the next presence broadcast regenerates the room
    /// ticket with the latest extra bootstrap peers.
    ticket_needs_regeneration: bool,
    /// Guards against overlapping async ticket-peer resolution tasks.
    ticket_resolve_in_flight: bool,
    /// Topics with a BackgroundSubscribe task currently in flight (single
    /// flight: prevents duplicate subscriptions when a stored conversation,
    /// a friend-direct topic, and an invite-accept all dispatch the same
    /// topic before any sender is installed).
    background_subscriptions_in_flight: std::collections::BTreeSet<TopicId>,
    /// Maps protocol message hashes to event_ids for delivery state resolution.
    self_sent_events: HashMap<MessageHash, u64>,
    /// Maintained indexes into the active conversation's entries.
    event_id_to_index: HashMap<u64, usize>,
    message_hash_to_index: HashMap<MessageHash, usize>,

    /// Maps offline mail envelope message_ids to ChatEntry indices for
    /// updating delivery status when an AckReceived or MailboxReplayed event
    /// arrives for a queued offline DM.
    pending_offline_ids: HashMap<String, usize>,

    /// Whether to auto-scroll to the latest message.
    follow_latest: bool,
    /// Estimated total content height of the chat log (set in view_chat_log).
    /// Cell interior mutability allows &self reads in view().
    total_content_height: std::cell::Cell<f32>,

    /// Settings / developer-UI domain (BORU-APP-003). Owns the Settings
    /// screen's UI state (toggles, accent picker, profile image, and the
    /// dev-ui inspector/gallery/designer state); see `app/settings.rs`.
    settings_state: settings::SettingsState,
    /// BORU-APP-004: notifications & activity domain (notification service,
    /// window focus tracker, in-app toast, Recent Activity feed + tick).
    notifications_state: notifications::NotificationsState,
    /// Whether dark mode is enabled.  Kept as a fast-access mirror of the
    /// persisted `AppSettings.dark_mode` (lags one write behind during
    /// update; always read from here).
    pub dark_mode: bool,
    /// Dev theme overrides loaded from `<data_dir>/boru-ui.toml` at startup
    /// (BORU-UI-04). Empty config when the file is missing or malformed (the
    /// error is logged and the last known-good theme is kept). BORU-UI-05
    /// merges these overrides on top of `BoruTheme::default()`.
    pub(crate) ui_theme_config: crate::theme_config::UiThemeConfig,
    /// BORU-UI-07: the live merged theme (default + `boru-ui.toml`
    /// overrides) currently active in app state. Replaced in-place when a
    /// valid reload arrives; view/style code reads it via
    /// [`IcedChat::boru_theme`] so the normal Iced state/update/view cycle
    /// redraws affected widgets without recreating any networking state.
    pub(crate) active_theme: crate::theme::BoruTheme,
    /// BORU-LAYOUT-03: the live structural layout currently active in app
    /// state. Starts as `LayoutConfig::default()` (which reproduces today's
    /// appearance exactly); a later BORU-LAYOUT task replaces it from
    /// `boru-layout.toml` via [`IcedChat::set_layout_config`]. View code
    /// reads it via [`IcedChat::boru_layout`] each frame.
    pub(crate) active_layout: crate::layout::LayoutConfig,
    /// BORU-LAYOUT-08: the editable layout override set (what the inspector
    /// edits and Save Layout serializes). Mirrors `ui_theme_config`: the
    /// watcher replaces it on reload; the inspector mutates it; the merged
    /// result is `active_layout`. Only layout state — never networking,
    /// gossip, rooms, tunnels, media playback, chat history, the selected
    /// conversation, scroll position or composer input.
    pub(crate) layout_overrides: crate::layout::LayoutOverrides,
    /// BORU-LAYOUT-03: monotonic counter bumped every time the active
    /// layout is replaced. Threaded into lazy dependency snapshots (like
    /// [`IcedChat::theme_revision`]) so cached view sections rebuild with
    /// the new layout.
    pub(crate) layout_revision: u64,
    /// BORU-UI-07: monotonic counter bumped every time the active theme is
    /// replaced (reload Ok or dark-mode toggle). Threaded into lazy/prewarm
    /// dependency snapshots so cached view sections rebuild with the new
    /// theme — without it iced's `lazy`/`Prebuilt` caches would keep
    /// serving the old theme's widget trees.
    pub(crate) theme_revision: u64,
    /// BORU-UI-06: receiver for debounced `boru-ui.toml` reload messages.
    /// Set by main.rs when the watcher thread starts; `None` in headless
    /// launches and tests (the subscription falls back to a closed dummy).
    pub(crate) ui_theme_rx:
        Option<Arc<Mutex<tokio::sync::mpsc::Receiver<crate::theme_watcher::UiThemeReloadMsg>>>>,
    /// BORU-UI-06: generation tracker for reload staleness — results older
    /// than the last accepted generation are dropped in update().
    ui_theme_reload_tracker: crate::theme_watcher::ReloadTracker,
    /// BORU-LAYOUT-06: receiver for debounced `boru-layout.toml` reload
    /// messages. Set by main.rs when the layout watcher thread starts; `None`
    /// in headless launches and tests (the subscription falls back to a
    /// closed dummy receiver).
    pub(crate) layout_rx:
        Option<Arc<Mutex<tokio::sync::mpsc::Receiver<crate::layout_watcher::LayoutReloadMsg>>>>,
    /// BORU-LAYOUT-06: generation tracker for layout reload staleness —
    /// results older than the last accepted generation are dropped in
    /// update().
    layout_reload_tracker: crate::theme_watcher::ReloadTracker,
    /// Read handle to the BORU-CP-05 backend connectivity state machine.
    /// Set from `main.rs` after construction (the discovery service handle
    /// itself is deliberately not stored on the UI). When `None` (e.g. in
    /// unit tests) the UI falls back to the legacy timestamp-based
    /// presence model.
    pub(crate) connectivity_store: Option<Arc<StdMutex<PeerConnectivityStore>>>,
    /// Read-only live Network Status map projection supplied by discovery.
    pub(crate) network_map_source: Option<Arc<dyn Fn(Instant) -> boru_core::network_map::NetworkMapState + Send + Sync>>,
    /// Read handle to the BORU-CP-12 negotiated-capability view (PDF Task
    /// 4.3): answers "does this peer support feature X, and at which
    /// version?" before the UI offers or initiates an optional feature
    /// (voice/video calls, screen share, file transfer, tunnels). Set from
    /// `main.rs` after construction. `None` in unit tests / when discovery
    /// is unavailable — feature actions then keep the legacy un-gated
    /// behaviour so offline/test paths are unchanged.
    pub capability_gate: Option<Arc<dyn boru_core::discovery_service::CapabilityGate>>,
    /// Read handle to the BORU-DIR-10 bounded room-directory cache (PDF
    /// Phase 4 Task 4.1). Set from `main.rs` after construction (the
    /// discovery service handle itself stays off the UI). The app feeds
    /// the real local room database facts (joined room ids from the
    /// conversation store + persisted hide preferences) into the cache
    /// via `sync_directory_local_states` so every discovered room
    /// carries a derived `local_join_state` (BORU-DIR-12, PDF Task 4.3).
    /// `None` in unit tests / when discovery is unavailable.
    pub room_directory: Option<Arc<StdMutex<boru_core::room_directory::RoomDirectory>>>,
    /// Whether the "clear history" confirmation is shown.
    history_confirm_clear: bool,
    /// Whether an async clear-history request is in flight.
    history_clear_pending: bool,
    /// Feedback shown after the last clear-history attempt.
    history_clear_feedback: Option<String>,
    /// Whether the feedback message is an error message.
    history_clear_feedback_is_error: bool,
    /// Topic awaiting delete confirmation (None = no confirm pending).
    room_delete_confirm_topic: Option<TopicId>,

    data_dir: PathBuf,

    /// Per-user image storage, backed by `<data_dir>/files/` (or `BORU_CHAT_FILES_DIR`).
    image_store: ImageStore,
    /// Persistent chat message history (loaded on startup, saved on each message).
    chat_history: Arc<std::sync::Mutex<ChatHistoryStore>>,
    /// Persistent download storage — opened once and shared with the
    /// download manager for startup recovery and ongoing tick processing.
    #[allow(dead_code)]
    storage: Option<Storage>,
    /// Restored authoritative authorization state for managed rooms.
    /// An absent topic is a legacy/unmanaged room; once present, checks fail closed.
    room_authorization: HashMap<TopicId, AuthorizationState>,
    /// Download state-machine manager with bounded startup burst.
    /// Wrapped for safe access — the async recovery call uses
    /// `runtime_handle.block_on` at init time.
    #[allow(dead_code)]
    download_manager: Option<Arc<std::sync::Mutex<DownloadManager>>>,
    /// Whether chat history has unsaved changes.
    /// Number of entries that have already been saved to chat_history
    /// for the current room. Used to avoid re-saving the same entries
    /// on every room-navigation event.
    history_saved_count: usize,
    /// Current Y scroll offset of the chat log, in pixels.
    scroll_offset: f32,
    /// Current viewport height of the chat log, in pixels.
    viewport_height: f32,
    /// Set when an entry was appended while following the latest message (or
    /// a conversation was opened in follow-latest mode).  The next update
    /// snaps the top-anchored chat scrollable back to the bottom so the
    /// newest message stays visible; cleared once the snap task is emitted.
    scroll_to_bottom_pending: bool,
    /// Per-peer presence tracking: PublicKey -> last-seen unix milliseconds.
    /// A peer stays in the map while connected; removal happens on
    /// NeighborDown only. Presence is derived from the timestamp:
    /// Online when fresh, Away when stale (> AWAY_THRESHOLD_MS), Offline
    /// when absent from the map.
    peer_presence_map: HashMap<PublicKey, u64>,
    /// Peers currently classified as Away (last-seen older than
    /// AWAY_THRESHOLD_MS), recomputed by ConnMonitorTick so the UI can
    /// re-render when peers transition between Online and Away.
    presence_away_peers: HashSet<PublicKey>,
    /// Peers that have been observed at least once (persisted across
    /// restarts in `<data_dir>/seen_peers.json`). The first-ever sighting
    /// of a peer (online transition) pushes a "New user" Recent Activity
    /// entry; the set prevents duplicate entries for reconnects/restarts
    /// (PUBLIC-03).
    seen_peers: HashSet<PublicKey>,
    /// Revision counter for the friends sidebar cache.
    friends_sidebar_revision: u64,
    /// Revision counter for the chats sidebar cache.
    chats_sidebar_revision: u64,
    /// Revision counter for the discovered-peers sidebar cache.
    discovered_sidebar_revision: u64,
    /// Revision counter for the incoming friend-requests sidebar cache.
    /// Receiving a request does not necessarily change the friends list.
    requests_sidebar_revision: u64,

    // ── Cached sidebar counts (recalculated on revision change) ──
    cached_chat_count: usize,
    cached_group_count: usize,
    cached_friend_count: usize,
    cached_discover_count: usize,
    cached_public_room_count: usize,
    cached_request_count: usize,

    // ── Cached sidebar dependencies (rebuilt only when revision changes) ──
    /// Cached chats section dependency — rebuilt when `chats_sidebar_revision` changes.
    cached_chats_revision: Cell<u64>,
    cached_chats_dep: std::cell::RefCell<Option<SidebarChatsDependency>>,
    /// Cached discovered-peers section dependency.
    cached_discovered_revision: Cell<u64>,
    cached_discovered_dep: std::cell::RefCell<Option<SidebarDiscoveredPeersDependency>>,
    /// Cached friends-rows section dependency.
    cached_friends_rows_revision: Cell<u64>,
    cached_friends_rows_dep: std::cell::RefCell<Option<SidebarFriendsRowsDependency>>,
    /// Cached requests section dependency.
    cached_requests_revision: Cell<u64>,
    cached_requests_dep: std::cell::RefCell<Option<SidebarRequestsDependency>>,

    /// Bootstrap peer addresses from the initial join ticket (if any).
    /// Used only for the first room subscription; cleared after use.
    initial_bootstrap_peers: Vec<EndpointAddr>,
    /// Whether the initial room open should leave the UI on the chat list.
    return_to_chat_list_after_open: bool,
    /// Handle for sending whisper/private messages.
    whisper_handle: WhisperHandle,
    /// Handle for enforcing call authorization alongside friend state.
    call_handle: CallHandle,
    /// Receiver for call actor events, consumed by the Iced subscription.
    pub call_events_rx: Arc<Mutex<Receiver<CallEvent>>>,
    call_return_screen: Option<Screen>,
    /// Calls & screen-share domain state (BORU-APP-008).
    pub(crate) calls_state: CallsState,
    /// Receiver for incoming inbox events.
    pub inbox_events_rx: Arc<Mutex<Receiver<InboxEvent>>>,
    /// Receiver for incoming whisper events.
    pub whisper_events_rx: Arc<Mutex<Receiver<WhisperEvent>>>,
    /// Home-screen background image (persisted path + decoded handle).
    /// The path comes from `AppSettings::home_background_image`; the handle is
    /// decoded once at startup and whenever the user picks a new image.
    home_background_path: Option<String>,
    home_background_handle: Option<iced::widget::image::Handle>,
    /// Opacity (0.0–1.0) applied to home-screen menu/action card backgrounds
    /// when a home background image is set. Persisted in AppSettings.
    home_menu_item_opacity: f32,
    /// Local mailbox public key derived from the node identity key.
    /// Advertised to friends via whisper control so they can encrypt
    /// offline messages to us.
    local_mailbox_key: Option<MailboxPublicKey>,
    /// Cached profile image handles for remote peers, keyed by PublicKey.
    /// `None` means the peer announced a ticket but the blob hasn't been downloaded yet.
    friend_image_handles: HashMap<PublicKey, Option<iced::widget::image::Handle>>,
    /// Last-seen profile image ticket string per peer.
    /// Used to avoid re-invalidating and re-downloading when the same ticket
    /// is re-announced in a periodic AboutMe broadcast (see ConnMonitorTick).
    friend_image_tickets: HashMap<PublicKey, String>,
    /// Queue of profile image tickets that arrived via AboutMe, awaiting async download.
    /// Each entry is (peer_public_key, blob_ticket_string).
    /// Downloaded entries are removed one-at-a-time each update tick to allow
    /// multiple concurrent peer image downloads without overwriting each other.
    pending_profile_image_tickets: std::collections::VecDeque<(PublicKey, String)>,
    /// Per-peer profile version counter, bumped whenever a new profile image
    /// ticket arrives for that peer.  Used in the sidebar lazy dependency keys
    /// so the friends/discovered-peers list re-renders only when a peer's
    /// profile actually changes, not on every ConnMonitorTick.
    friend_profile_versions: HashMap<PublicKey, u64>,
    /// Tracks last retry attempt time per peer for failed profile image downloads.
    /// Used by retry_stale_profile_images to implement a cooldown between retries.
    last_failed_profile_retry: HashMap<PublicKey, std::time::Instant>,
    /// Performance metrics for the last render — used by regression tests.
    perf: std::cell::RefCell<PerfMetrics>,
    /// Whether this is the user's first run (no room history, no friends, no chats).
    first_run: bool,
    /// Incrementally maintained layout cache for the chat log.
    /// Avoids O(n) full-height scan on every render.
    layout_cache: std::cell::RefCell<LayoutCache>,
    /// Friend request store — tracks pending/accepted/declined/cancelled requests.
    friend_request_store: FriendRequestStore,
    /// Outgoing request state per peer — tracks UI-level request lifecycle.
    /// None = no request sent to this peer.
    /// Some(OutgoingRequestState) = current state of the request.
    outgoing_request_states: HashMap<PublicKey, OutgoingRequestState>,
    /// Structured list of join-request items exposed to the main-menu ViewModel.
    /// Rebuilt after every state change; deduplicated by request ID.
    join_request_list: Vec<JoinRequestItem>,

    /// Search/input text for the peer public key in the friend requests screen.
    friend_request_search_input: String,
    /// Error message shown in the friend requests screen.
    friend_request_error: String,
    /// Public-room safety enforcement (rate limits, size limits, download queue bounding).
    /// `None` (default) means private-room behavior — all safety checks are skipped.
    pub public_room_safety: Option<Arc<PublicRoomSafety>>,
    /// Durable conversation records — persisted to `conversations.json`.
    /// Tracks metadata (peer, name, kind, archived) for all conversations.
    conversation_store: ConversationStore,
    /// Peers currently discovered via DHT, used as the source list for the
    /// "Discovered Peers" sidebar section.
    discovered_peers: Vec<PublicKey>,
    /// PublicKey -> online indicator cache (populated from neighbors set).
    /// Separate from peer_presence_map to avoid conflating friend vs
    /// discovered-peer online status.
    discovered_online_cache: HashSet<PublicKey>,
    /// Receiver handle for discovered peers from the DHT discovery loop.
    /// Read by the subscription stream to produce NewDiscoveredPeers events.
    pub discovered_peers_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<DiscoveredPeersUpdate>>>,
    /// Receiver for backend reconnection signals (BORU-CP-07): the backend
    /// re-established endpoint connectivity to a friend via a reconnect
    /// attempt. The app ensures the deterministic direct topic is
    /// joined/subscribed (friend-scoped, deduplicated by BackgroundSubscribe).
    pub reconnect_ready_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<PublicKey>>>,
    /// Backend reconnection handle (BORU-CP-07): used to report REAL
    /// direct-topic readiness (BackgroundSubscribed success), which clears
    /// the backend's retry/backoff state. Never used for discovery metadata.
    reconnect_handle: Option<boru_core::control_plane::reconnect::ReconnectHandle>,
    /// Debounce buffer for NeighborUp/NeighborDown events.
    /// Maps `PublicKey -> is_online`. Flushed on every ConnMonitorTick (~1s).
    pending_neighbor_status: HashMap<PublicKey, bool>,
    /// When non-empty, the app will attempt a history backfill for each
    /// topic as soon as a gossip neighbor becomes available.  Set during
    /// RoomOpened when local history is below the trigger threshold but
    /// no neighbors are connected yet.  Topics are removed when a
    /// backfill succeeds (history count reaches the threshold).
    pending_backfill_topics: Vec<TopicId>,

    // ── Invite menu state ──
    /// Whether the "Copied!" feedback is shown for the friend ID.
    friend_id_copied: bool,
    /// Whether the invite menu popover is currently visible.
    show_invite_menu: bool,
    /// The peer public key input text in the invite whisper field.
    invite_whisper_input: String,
    /// Shared DHT client for creating private-room discovery records.
    dht: Option<distributed_topic_tracker::Dht>,
    /// Disable private-room DHT discovery from the command line.
    private_dht_disabled: bool,
    /// Rooms/directory domain state (BORU-APP-006): create-room dialog,
    /// room-settings dialog, directory advertising state, per-room DHT
    /// trackers. See `app/rooms.rs`.
    pub(crate) rooms_state: RoomsState,
    /// Whether the group creation dialog is currently shown.
    show_create_group_dialog: bool,
    /// Whether the group-creation submit is in flight (async gossip
    /// subscription + metadata/roster docs). While true the Create Group
    /// button shows a loading state and the dialog cannot be dismissed.
    create_group_submitting: bool,
    /// Inline error shown inside the create-group dialog (name field area).
    create_group_error: Option<String>,
    /// Group name text input in the group creation dialog.
    create_group_name: String,
    /// Group description text input in the group creation dialog.
    create_group_description: String,
    /// Set of friend public keys selected as group members.
    create_group_selected_members: HashSet<PublicKey>,
    /// Participant search/filter text in the group creation dialog.
    create_group_search: String,
    /// Tunnels domain state (BORU-APP-009): create-tunnel dialog, incoming
    /// tunnel requests, share-local-service dialog, received/shared tunnel
    /// display metadata. Owned by `app/tunnels.rs`.
    pub(crate) tunnels_state: TunnelsState,
    /// Reusable advanced connection-details dialog state.
    connection_details_dialog: Option<ConnectionDetailsDialogState>,
    /// Accessible status announcement shown after copying from the dialog.
    connection_details_announcement: Option<String>,
    /// Focus target to restore when the connection-details dialog closes.
    connection_details_focus_target: Option<&'static str>,
    // ── Friend Profile screen state ──
    /// Whether the three-dot context menu in the friend profile is open.
    friend_profile_menu_open: bool,
    /// Text input for inline rename of a friend's display name.
    friend_profile_rename_input: String,
    /// Whether we're currently in rename-input mode.
    friend_profile_renaming: bool,
    /// Whether the "Remove Friend" confirmation dialog is shown.
    friend_remove_confirm: bool,
    /// Whether the "Block Friend" confirmation dialog is shown.
    friend_block_confirm: bool,
    /// Right-click context menu state: (entry_index, x, y) when visible.
    context_menu: Option<(usize, f32, f32, ContextMenuKind)>,
    /// Entry index whose video-card header overflow menu is open, if any.
    video_card_menu_open: Option<usize>,
    /// Cached profile data received from peers via ProfileUpdate gossip.
    /// DomainState for the file transfer & download UI domain (BORU-APP-005).
    /// Owns the transfer/download UI state, dashboard tabs, peer catalogue,
    /// and short-code/redeem dialog state. See `app/files.rs`.
    pub(crate) files_state: FilesState,
    profile_cache: HashMap<PublicKey, PeerProfileData>,
    /// Persistent profile store (display name, bio, sharing controls).
    profile_store: UserProfileStore,

    // ── GUI test actions (MCP-driven) ──
    /// Iced message journal for diagnostics (shared with the MCP server).
    pub iced_diagnostics: IcedMessageJournal,
    /// Receiver for GUI test actions from MCP.
    pub gui_action_rx: Option<Arc<Mutex<tokio::sync::mpsc::Receiver<GuiActionRequest>>>>,
    /// GUI action history with expected-state tracking.
    pub gui_action_history: GuiActionHistory,
    /// OpenRoom action currently waiting for the asynchronous room handoff to
    /// select its requested topic.  Completion is recorded only after the
    /// normal OpenRoom/RoomOpened path has updated both topic and screen.
    pending_open_room_action: Option<(GuiActionId, TopicId)>,
    /// OpenConversation action waiting for its derived direct room to open.
    pending_open_conversation_action: Option<(GuiActionId, PublicKey)>,
    /// SetComposerText action waiting for the normal InputChanged path.
    pending_set_composer_action: Option<(GuiActionId, String)>,
    /// SubmitComposer action waiting for the normal SendPressed path.
    pending_submit_composer_action: Option<GuiActionId>,
    /// GoToChatList action currently being handled by the normal update path.
    pending_chat_list_action: Option<GuiActionId>,
    /// OpenFriends action waiting for the normal friend-screen navigation path.
    pending_open_friends_action: Option<GuiActionId>,
    /// OpenSettings action waiting for the normal settings navigation path.
    pending_open_settings_action: Option<GuiActionId>,
    /// OpenFileSharing action waiting for the normal file-sharing navigation path.
    pending_open_file_sharing_action: Option<GuiActionId>,
    /// OpenDashboardTab action waiting for the dashboard tab to become active.
    pending_dashboard_tab_action: Option<GuiActionId>,
    /// TestShareFile action waiting for the shared-file registration to finish.
    pending_share_file_action: Option<GuiActionId>,
    /// CloseDialog action waiting for the normal dialog-cancel message path.
    pending_close_dialog_action: Option<GuiActionId>,
    /// ToggleHelp action waiting for the normal help-overlay toggle path.
    pending_toggle_help_action: Option<GuiActionId>,
    /// SelectPeer action waiting for the normal peer-profile navigation path.
    pending_select_peer_action: Option<(GuiActionId, PublicKey)>,
    /// CreateNewRoom action waiting for the create-room dialog to open.
    pending_create_room_action: Option<GuiActionId>,
    /// ConfirmCreateNewRoom action waiting for the room to be created.
    pending_confirm_create_room_action: Option<GuiActionId>,
    /// DownloadFile action waiting for the async download to complete.
    pending_download_action: Option<GuiActionId>,
    /// Sender for GUI state snapshots — publishes an [`IcedStateSnapshot`] after
    /// each `update()` so the MCP server can watch for condition changes.
    pub gui_state_tx: tokio::sync::watch::Sender<IcedStateSnapshot>,
    /// When false, `publish_gui_state()` is a no-op.  Set true by default;
    /// disabled in headless/test scenarios where nobody reads the watch channel.
    gui_state_enabled: bool,
    /// Last snapshot that was successfully sent, for dirty-state comparison.
    /// `None` means no snapshot has been published yet — always send.
    last_snapshot: Option<Box<IcedStateSnapshot>>,
    /// Monotonic clock timestamp of the last successfully published snapshot.
    /// Used for rate limiting.
    last_snapshot_at: std::time::Instant,
    /// Minimum interval (ms) between snapshot publishes.  `0` disables throttling.
    /// Production value: 125 (≈ 8 updates/sec).  Tests set `0`.
    pub gui_snapshot_throttle_ms: u64,
    /// When true, a state change was detected but the last publish was throttled.
    /// The next call to `publish_gui_state()` that is not throttled will flush it.
    gui_snapshot_pending: bool,
    /// Current window width, updated by resize events.
    /// Used for responsive layout decisions (breakpoint at 640px).
    window_width: f32,
    /// Current window height, updated by resize events for height-aware layout.
    window_height: f32,

    /// Animation frame counter for the room-loading and connecting spinners.
    splash_spinner_frame: usize,
    /// Animation frame counter for the connecting-to-peer spinner shown in
    /// the chat view when the gossip sender isn't ready yet.
    connecting_spinner_frame: usize,
    /// Animation frame counter for the main screen's "reconnecting to relay"
    /// status text (animated dots or spinner).
    main_screen_reconnect_frame: usize,
    /// OS reduced-motion preference — when true, skip all spinner/loading
    /// animations and show static indicators instead.
    reduced_motion: bool,
    /// Cached link previews (title, description, image) keyed by URL.
    link_preview_cache: std::sync::Arc<std::sync::Mutex<link_preview::LinkPreviewCache>>,

    /// Whether the chat options popover is open.
    show_chat_options: bool,
    /// Whether the in-conversation search panel is open.
    show_chat_search: bool,
    /// Live query for the in-conversation search panel.
    chat_search_query: String,
    /// Whether the group member list overlay is shown.
    show_member_list: bool,
    /// Whether the emoji picker panel is currently visible.
    show_emoji_picker: bool,
    /// Active emoji picker category (BORU-TWEMOJI-12). The grid shows
    /// exactly this category's catalog entries when no search is active.
    emoji_category: crate::emoji::EmojiCategory,
    /// Live emoji picker search query (BORU-TWEMOJI-13). Empty restores the
    /// category view; non-empty shows shared-catalog search results.
    emoji_search_query: String,
    /// Recently-used emoji as plain Unicode strings (BORU-TWEMOJI-14).
    /// Runtime copy of `AppSettings::recent_emojis`; updated on every
    /// selection and persisted through Boru's normal settings system.
    recent_emojis: Vec<String>,
    /// Whether the GIF picker panel is currently visible.
    show_gif_picker: bool,
    /// Search text for the GIF picker.
    gif_search_text: String,
    /// Provider-neutral GIF search results (from `GifProvider`).
    gif_results: Vec<GifSearchResult>,
    /// Preview thumbnail bytes keyed by `provider_id` (small WebP/GIF
    /// renditions only — never full-size originals).
    gif_preview_cache: HashMap<String, Vec<u8>>,
    /// Whether a GIF search/trending request is currently in flight.
    gif_loading: bool,
    /// Whether the current results are trending (shown before any search).
    gif_showing_trending: bool,
    /// Whether a search has been submitted at least once (distinguishes the
    /// empty state from the no-results state).
    gif_has_searched: bool,
    /// User-facing error from the last GIF request, if any.
    gif_error: Option<String>,
    /// Opaque pagination cursor for the next page of results.
    gif_next_cursor: Option<String>,
    /// Whether the next results message should append to (paginate) rather
    /// than replace the current `gif_results`.
    gif_appending: bool,
    /// Compact error shown under the grid when a load-more (pagination)
    /// request fails.  Keeps already-loaded results visible instead of
    /// replacing them with the full-screen error state.
    gif_append_error: Option<String>,
    /// Monotonic request id; responses carrying an older id are stale and
    /// must be ignored (prevents out-of-order completions overwriting newer
    /// results).
    gif_request_seq: u64,
    /// Debounce timer id; only the latest scheduled debounce fires.
    gif_debounce_seq: u64,
    /// Spinner frame for the GIF loading state.
    gif_spinner_frame: usize,
    /// Whether external GIF search is disabled because KLIPY is not
    /// configured (no KLIPY_API_KEY).  Drives the picker's
    /// provider-not-configured state instead of hitting the network.
    gif_not_configured: bool,
    /// Whether the invite member dialog is shown.
    show_invite_member_dialog: bool,
    /// Selected friends to invite in the invite member dialog.
    invite_member_selected: HashSet<PublicKey>,
    /// Whether the right-side details panel is open.
    details_panel_open: bool,

    // ── Receive from ticket (SENDME-02 wormhole sharing) ──
    /// Whether the "Receive from ticket" dialog is shown.
    show_receive_ticket_dialog: bool,
    /// The pasted BlobTicket string in the receive dialog.
    receive_ticket_input: String,
    /// Pre-flight result for the pasted ticket (size + format info).
    receive_ticket_preflight: Option<ReceiveTicketPreflight>,
    /// User-facing error from the pre-flight step, if any.
    receive_ticket_error: Option<String>,
    /// True while the pre-flight async task is in flight.
    receive_ticket_preflight_busy: bool,
    /// True while the confirmed download task is in flight.
    receive_ticket_downloading: bool,

    // ── Short-code file shares (FS-26) ──

    // ── Room advertisement (public directory) ──
    // Advertising state (advertised set, dedupe fingerprints, periodic
    // refresh counter, startup sweep, auto-subscribed set, per-room DHT
    // trackers) lives in `self.rooms_state` (BORU-APP-006). The directory
    // network handles below stay on the shell — they are shared discovery
    // infrastructure read by the net layer, discover.rs, MCP, and tests.
    /// Stable gossip topic used to discover public rooms on this relay.
    directory_topic: TopicId,
    /// Gossip sender for the directory topic (subscribed lazily when the
    /// first room is enabled for advertising).
    directory_sender: Option<GossipSender>,
    /// Received room advertisements from the directory gossip topic.
    pub(crate) directory_store: Arc<StdMutex<DirectoryStore>>,
    /// Channel for receiving room advertisements from the background directory
    /// subscription task.
    directory_room_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<DirectoryRoomEvent>>>,
}

/// Cached profile data received from a peer via ProfileUpdate gossip.
#[derive(Debug, Clone)]
pub struct PeerProfileData {
    /// Display name announced by the peer.
    pub display_name: String,
    /// Bio text.
    #[expect(dead_code)]
    pub bio: String,
    /// When this profile data was last received (SystemTime). Used for eviction.
    pub last_updated: SystemTime,
}

/// Tracks the UI-level lifecycle of an outgoing friend request.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum OutgoingRequestState {
    /// Request has been sent; waiting for a response.
    Pending,
    /// The recipient accepted our request.
    Accepted,
    /// The recipient declined our request.
    Declined,
    /// The request failed to send (network error, etc.).
    Failed(String),
}

// ── Screen dependencies (screen-level lazy() cache keys) ─────────────────
// Each struct holds ONLY the state slice its screen renders, as
// Hash-compatible fields (no f32, no image bytes). `iced::widget::lazy`
// compares a freshly computed dependency with the previous frame's value via
// PartialEq and, when equal, reuses the already-built subtree. Screens that
// render live per-layout state (Chat's responsive message log) are NOT
// wrapped and are documented inline at the Screen::Chat arm.

/// Reserved cache key for the Chat screen. `view_chat_panel` is NOT wrapped
/// in `lazy` because its `widget::responsive` message log captures `&self`
/// and mutates the incremental `layout_cache` on every layout pass — a
/// `'static` tree (required by `lazy`) is impossible without changing the
/// log's behavior. Kept so the cache-key shape is documented for when the
/// log is refactored to owned rendering.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
#[expect(dead_code)]
pub(crate) struct ChatDependency {
    pub(crate) dark_mode: bool,
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
    pub(crate) topic: TopicId,
    pub(crate) entries_len: usize,
    pub(crate) composer_text: String,
}

/// Dependency for the Peer Profile screen.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct PeerProfileDependency {
    pub(crate) dark_mode: bool,
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
    pub(crate) peer: PublicKey,
    pub(crate) display_name: String,
}

/// Hash-compatible snapshot of one catalogue file row for the Peer Catalogue
/// screen. The live [`RemoteSharedFile`] is not Hash, so the builder copies the
/// renderable fields (all Hash-compatible) plus the download-state discriminant
/// and the pending flag. The static renderer reconstructs a `RemoteSharedFile`
/// from this snapshot when an action message needs it.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct CatalogueRowSnapshot {
    pub(crate) shared_file_id: String,
    pub(crate) display_name: String,
    pub(crate) description: Option<String>,
    pub(crate) mime_type: String,
    pub(crate) size_bytes: u64,
    pub(crate) content_hash: String,
    pub(crate) version_number: u32,
    pub(crate) updated_at_ms: u64,
    pub(crate) collection_ids: Vec<String>,
    pub(crate) dl: CatalogueDownloadSnapshot,
    pub(crate) is_pending: bool,
}

impl CatalogueRowSnapshot {
    fn to_file(&self) -> RemoteSharedFile {
        RemoteSharedFile {
            shared_file_id: self.shared_file_id.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            mime_type: self.mime_type.clone(),
            size_bytes: self.size_bytes,
            content_hash: self.content_hash.clone(),
            version_number: self.version_number,
            updated_at_ms: self.updated_at_ms,
            collection_ids: self.collection_ids.clone(),
            children: vec![],
        }
    }
}

/// Hash-compatible projection of [`CatalogueDownloadState`] (which embeds a
/// `PathBuf` and an error string and therefore cannot derive Hash).
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) enum CatalogueDownloadSnapshot {
    None,
    Pending,
    Downloading {
        bytes: u64,
        total: Option<u64>,
        speed: u64,
    },
    Completed,
    Failed,
    Cancelled,
}

impl From<&CatalogueDownloadState> for CatalogueDownloadSnapshot {
    fn from(state: &CatalogueDownloadState) -> Self {
        match state {
            CatalogueDownloadState::Pending => CatalogueDownloadSnapshot::Pending,
            CatalogueDownloadState::Downloading {
                bytes,
                total,
                speed,
            } => CatalogueDownloadSnapshot::Downloading {
                bytes: *bytes,
                total: *total,
                speed: *speed,
            },
            CatalogueDownloadState::Completed { .. } => CatalogueDownloadSnapshot::Completed,
            CatalogueDownloadState::Failed(_) => CatalogueDownloadSnapshot::Failed,
            CatalogueDownloadState::Cancelled => CatalogueDownloadSnapshot::Cancelled,
        }
    }
}

/// Dependency for the Peer Catalogue screen. Holds the full renderable file
/// rows (Hash snapshots) plus the windowing state (scroll offset and viewport
/// height as `u32` bits) so the static content renderer can rebuild the
/// virtualised list exactly like the original did.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct PeerCatalogueDependency {
    pub(crate) dark_mode: bool,
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
    pub(crate) peer: PublicKey,
    pub(crate) display_name: String,
    pub(crate) catalogue_loading: bool,
    pub(crate) rows: Vec<CatalogueRowSnapshot>,
    pub(crate) catalogue_scroll_offset_bits: u32,
    pub(crate) catalogue_viewport_height_bits: u32,
}

/// Hash-compatible snapshot of one received tunnel (shared service) row shown
/// in the Friend Profile screen. The live `ReceivedTunnelState` is not Hash,
/// so the builder pre-renders every display field into this row.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct FriendProfileServiceRow {
    pub(crate) id: boru_core::tunnel::TunnelId,
    pub(crate) service_name: String,
    pub(crate) sharer_label: String,
    pub(crate) is_http: bool,
    pub(crate) expired: bool,
    pub(crate) connection_failed: bool,
    pub(crate) connected: bool,
    /// Route label for connected tunnels ("Direct" / "Relay" / custom).
    pub(crate) route_label: Option<String>,
    /// Rendered local loopback address when connected.
    pub(crate) local_addr: Option<String>,
    /// Rendered expiry label (e.g. "Expires in 2h").
    pub(crate) expiry: String,
}

/// Dependency for the Friend Profile screen. Holds the Hash-compatible state
/// slice the base content renders: identity, presence, rename input, catalogue
/// presence, recent message previews, and received shared-service rows. The
/// transient overlays (menu, confirm dialogs, share dialog, toast) are NOT part
/// of this snapshot — they contain live text inputs / combo boxes and render
/// every frame, layered on top of the cached base.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct FriendProfileDependency {
    pub(crate) dark_mode: bool,
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
    pub(crate) peer: PublicKey,
    pub(crate) display_name: String,
    pub(crate) presence: PeerPresence,
    pub(crate) has_addrs: bool,
    pub(crate) friend_profile_rename_input: String,
    pub(crate) friend_profile_renaming: bool,
    pub(crate) has_catalogue: bool,
    pub(crate) recent_messages: Vec<String>,
    pub(crate) shared_services: Vec<FriendProfileServiceRow>,
}

/// One renderable row of the Discover Rooms browse surface — a Hash-friendly
/// snapshot of a [`DirectoryEntry`](boru_core::room_directory::DirectoryEntry)
/// (BORU-DIR-10..12). The bounded cache stores richer per-entry state
/// (`Instant`s, publisher identity, auth verdict) that is deliberately not
/// part of the render dependency: iced's `lazy` / prewarm hashing only needs
/// the stable, user-visible metadata and the local-relationship verdict.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct DiscoverRoomRow {
    /// Room gossip topic bytes (the advertised room id).
    pub(crate) room_id: [u8; 32],
    /// Human-readable room name.
    pub(crate) room_name: String,
    /// Short description from the advertisement.
    pub(crate) short_description: String,
    /// Optional searchable category tags from the advertisement (empty =
    /// no tags; the card hides the tag row entirely).
    pub(crate) tags: Vec<String>,
    /// Advertised room chat protocol version.
    pub(crate) room_protocol_version: u8,
    /// Owner/creator peer id bytes (descriptive metadata only).
    pub(crate) owner_peer_id: [u8; 32],
    /// Optional approximate member count (untrusted hint, PDF Phase 7).
    pub(crate) member_count: Option<u32>,
    /// Room chat-protocol compatibility verdict.
    pub(crate) compatibility: boru_core::room_directory::RoomCompatibility,
    /// Optional-feature compatibility verdict (PDF Task 6.2 step 2).
    /// Informational only — never blocks the Join action.
    pub(crate) feature_compat: boru_core::room_directory::RoomFeatureCompatibility,
    /// Local relationship state (NotJoined/Joined/Blocked/...).
    pub(crate) local_join_state: boru_core::room_directory::LocalJoinState,
    /// The action the browse surface should offer (Join/Open/Incompatible).
    pub(crate) offered_action: boru_core::room_directory::RoomAction,
    /// Whether the stored metadata is contested by conflicting ads.
    pub(crate) conflict: bool,
}

/// Dependency for the Discover screen. Holds the full renderable room
/// browse rows plus the theme flag, so the static content renderer can
/// rebuild the whole screen from this snapshot.
///
/// BORU-DIR-15 (PDF Task 5.3): also carries the local search query,
/// filter toggles, selected tags, available tag chips, sort order, and
/// the pre-filter total, so the renderer can draw the search box /
/// filter chips / sort selector AND the lazy cache invalidates whenever
/// any of them changes.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct DiscoverDependency {
    pub(crate) dark_mode: bool,
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
    /// Structural layout state used by the screen's narrow/desktop variants.
    pub(crate) layout_revision: u64,
    pub(crate) responsive_mode: crate::layout::ViewportTier,
    pub(crate) max_content_width_bits: u32,
    /// Filtered + sorted rows (already limited to what passes the query,
    /// filters, selected tags, and sort order).
    pub(crate) rooms: Vec<DiscoverRoomRow>,
    /// Local search query (never broadcast onto the discovery network).
    pub(crate) search_query: String,
    /// Filter toggles (Compatible / Not Joined / Recently Seen).
    pub(crate) filter_compatible: bool,
    pub(crate) filter_not_joined: bool,
    pub(crate) filter_recently_seen: bool,
    /// Selected tag/category filters (OR semantics).
    pub(crate) selected_tags: Vec<String>,
    /// All tags present in the cache — the available tag chips.
    pub(crate) available_tags: Vec<String>,
    /// Current sort order.
    pub(crate) sort: DiscoverSort,
    /// Rooms in the cache BEFORE search/filtering (for "N of M" copy).
    pub(crate) total_count: usize,
}

/// Dependency for the Groups screen.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct GroupsDependency {
    pub(crate) dark_mode: bool,
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
    pub(crate) groups: Vec<(TopicId, String)>,
}

// ── Home-rail card dependencies (fine-grained selectors) ─────────────────
// Each struct holds ONLY the state slice its card renders. `iced::widget::lazy`
// compares a freshly computed dependency with the previous frame's value via
// PartialEq and, when equal, reuses the already-built subtree — the iced
// equivalent of React.memo + a shallow-equality selector. Because each card's
// dependency excludes the other cards' data, a change in one slice can never
// rebuild a different card. The Online Peers dependency deliberately excludes
// `activity_tick`, so the once-per-second ActivityTick (which refreshes
// relative timestamps in the Recent Activity card and expiry labels in the
// Tunnels card) does not touch the peers card at all.

/// A structured join-request item exposed by the main-menu ViewModel.
///
/// Each item carries the persistent request ID, the target peer's public key,
/// the direct-conversation chat topic, and the current request state.  Items
/// are deduplicated by request ID.
#[expect(dead_code)]
#[derive(Debug, Clone)]
pub struct JoinRequestItem {
    /// Persistent request ID from the friend request store.
    pub request_id: String,
    /// Target peer's public key string.
    pub target_user: String,
    /// Direct-conversation chat topic (chat identifier).
    pub chat_id: TopicId,
    /// Current state of the request.
    pub state: OutgoingRequestState,
}

impl JoinRequestItem {
    /// Create a new join-request item from known values.
    pub fn new(
        request_id: String,
        target_user: String,
        chat_id: TopicId,
        state: OutgoingRequestState,
    ) -> Self {
        Self {
            request_id,
            target_user,
            chat_id,
            state,
        }
    }
}

/// Keyboard shortcut actions triggered by global keybindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Escape — close help, close settings, or clear input.
    Escape,
    /// Ctrl+N — create a new chat.
    NewChat,
    /// Ctrl+Backspace — go back to the chat list.
    BackToChatList,
    /// Slash (/) — quick-command: focus composer with '/'.
    QuickCommand,
    /// Tab — move keyboard focus to the next text input.
    FocusNext,
    /// Shift+Tab — move keyboard focus to the previous text input.
    FocusPrevious,
    // ── Dashboard tab navigation (Ctrl+1..5 or Left/Right arrow) ────
    /// Ctrl+1 or Left arrow — select previous dashboard tab.
    DashboardTabPrevious,
    /// Ctrl+2 or Right arrow — select next dashboard tab.
    DashboardTabNext,
    /// Ctrl+Z — undo the last completed designer transaction.
    #[cfg(feature = "dev-ui")]
    DesignerUndo,
    /// Ctrl+Shift+Z or Ctrl+Y — redo a designer transaction.
    #[cfg(feature = "dev-ui")]
    DesignerRedo,
    /// Ctrl+S — save the current developer layout configuration.
    #[cfg(feature = "dev-ui")]
    DesignerSave,
    /// Arrow-key semantic nudge for the selected layout property.
    #[cfg(feature = "dev-ui")]
    DesignerNudgeUp,
    #[cfg(feature = "dev-ui")]
    DesignerNudgeDown,
    #[cfg(feature = "dev-ui")]
    DesignerNudgeLeft,
    #[cfg(feature = "dev-ui")]
    DesignerNudgeRight,
    /// Delete hides a supported optional designer section; it never removes
    /// production widgets or features.
    #[cfg(feature = "dev-ui")]
    DesignerDelete,
}

#[derive(Debug, Clone)]
pub enum OfflineDeliveryStatus {
    Queued,
    Delivered,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveredPeersUpdate {
    pub added: Vec<PublicKey>,
    pub removed: Vec<PublicKey>,
}

/// A room advertisement received from the directory gossip topic.
///
/// Paired with the author's [`PublicKey`] so the receiver can verify the
/// signature before storing in the [`DirectoryStore`].
/// Messages received from the dedicated directory gossip subscription in
/// main.rs and drained by the app on `ConnMonitorTick`.
///
/// - [`DirectoryRoomEvent::Advertisement`] carries a verified room
///   advertisement (upsert into [`DirectoryStore`]).
/// - [`DirectoryRoomEvent::Withdrawal`] carries a **verified** room
///   withdrawal (BORU-DIR-09, PDF Task 3.3): remove the matching
///   advertisement (`topic`, `author`) immediately. The signature was
///   verified by the directory receiver loop before the event was sent;
///   TTL expiry remains the safety net if a withdrawal is missed.
pub enum DirectoryRoomEvent {
    Advertisement(RoomAdvertisement, PublicKey),
    Withdrawal(TopicId, PublicKey),
}


/// Result of the "Receive from ticket" pre-flight check.
///
/// Carries the ticket string plus what the provider told us about the blob
/// (format + total size) so the confirm step can start the download through
/// the existing download machinery into a safe destination.
#[derive(Debug, Clone)]
pub(crate) struct ReceiveTicketPreflight {
    /// The BlobTicket string the user pasted (kept for the download step).
    pub(crate) ticket: String,
    /// Content hash (hex) of the blob.
    pub(crate) content_hash: String,
    /// Node id of the provider (short form for display).
    pub(crate) node_short: String,
    /// Total payload size in bytes.
    pub(crate) total_size: u64,
    /// `true` when the ticket addresses a HashSeq collection (folder).
    pub(crate) is_collection: bool,
    /// Number of children (1 for a raw blob, N for a collection).
    pub(crate) child_count: u64,
}

/// State for an active short-code share (FS-26). Kept while the sender-side
/// "share via code" dialog is open so the periodic tick can re-broadcast the
/// signed announcement on the code-derived rendezvous topic.
#[derive(Debug, Clone)]
pub(crate) struct ShortCodeActiveShare {
    /// The 7-character code being shared.
    pub(crate) code: String,
    /// Serialized blob ticket the code resolves to.
    pub(crate) ticket: String,
    /// Display file name.
    pub(crate) name: String,
    /// Expected total size in bytes.
    pub(crate) size: u64,
}

/// Result of redeeming a short code over the rendezvous gossip topic.
#[derive(Debug, Clone)]
pub(crate) struct ShortCodeRedemption {
    /// The code that was redeemed.
    pub(crate) code: String,
    /// Display file name from the announcement.
    pub(crate) name: String,
    /// Serialized blob ticket from the announcement.
    pub(crate) ticket: String,
    /// Expected total size in bytes.
    pub(crate) size: u64,
    /// Short form of the announcing peer's node id.
    pub(crate) node_short: String,
}

#[derive(Debug, Clone)]
pub enum AppMessage {
    /// Route developer-only designer actions through the normal Iced update
    /// pipeline. The variant is absent from production builds.
    #[cfg(feature = "dev-ui")]
    Designer(DesignerMessage),
    // ── Navigation ──
    /// Open the chat list screen (go back from a chat).
    GoToChatList,
    /// A global keyboard shortcut was activated.
    Shortcut(Shortcut),
    /// Open a specific room.
    OpenRoom(TopicId),
    /// A new room was created and we're now connected to it.
    RoomOpened {
        topic: TopicId,
        ticket: String,
        sender: GossipSender,
        /// Optional continuous DHT tracker for background publish/discovery
        /// in private rooms with DHT discovery enabled.
        room_tracker: Option<SharedTracker>,
        /// Number of gossip neighbors already known at subscription time.
        /// Used to set sender_ready immediately when NeighborUp was emitted
        /// before the forwarder started (common on startup).
        neighbor_count: usize,
        /// Peer IDs of gossip neighbors already known at subscription time.
        /// Used to emit diagnostic events retroactively when NeighborUp was
        /// missed (emitted before the forwarder started).
        neighbor_ids: Vec<PublicKey>,
        /// Room generation captured when this join was initiated. The
        /// handler asserts the current generation still matches before
        /// applying, so a stale completion for a room the user has since
        /// left or switched away from is caught in debug builds.
        generation: u64,
    },
    /// Finished creating a new room (random topic).
    CreateNewRoom,
    /// Confirm create-new-room with current dialog settings.
    ConfirmCreateNewRoom,
    /// Cancel the create-room dialog.
    CancelCreateRoom,
    /// Toggle whether DHT discovery is enabled when creating a new room.
    CreateNewRoomDhtToggled(bool),
    /// Update the room name text input in the create-room dialog.
    CreateNewRoomNameChanged(String),
    /// Change the visibility selected for the new room (BORU-DIR-05).
    CreateNewRoomVisibilityChanged(RoomVisibility),
    /// Update the optional description input in the create-room dialog.
    CreateNewRoomDescriptionChanged(String),
    /// Update the optional tags input in the create-room dialog.
    CreateNewRoomTagsChanged(String),
    /// Open the room-settings dialog for an existing room (BORU-DIR-06).
    /// Owner/admin-only; lets the owner switch directory visibility and edit
    /// advertised metadata (name / description / tags) after creation.
    OpenRoomSettings(TopicId),
    /// Update the room name in the room-settings dialog.
    RoomSettingsNameChanged(String),
    /// Update the description in the room-settings dialog.
    RoomSettingsDescriptionChanged(String),
    /// Update the tags in the room-settings dialog.
    RoomSettingsTagsChanged(String),
    /// Change the visibility selected in the room-settings dialog.
    RoomSettingsVisibilityChanged(RoomVisibility),
    /// Apply the room-settings dialog: persist metadata + visibility and
    /// republish / unlist the advertisement (BORU-DIR-06).
    ConfirmRoomSettings,
    /// Cancel the room-settings dialog.
    CancelRoomSettings,
    /// Owner/admin switch of an existing room's directory visibility
    /// (BORU-DIR-06, PDF Task 2.3): PublicDiscoverable <-> PublicUnlisted.
    /// When switching to discoverable the handler immediately publishes a
    /// fresh advertisement; when switching to unlisted it stops refreshing
    /// (TTL expiry applies — no withdrawal message yet, BORU-DIR-09).
    SetRoomDirectoryVisibility {
        /// The room to switch.
        topic: TopicId,
        /// The visibility to switch to.
        visibility: RoomVisibility,
    },
    /// Join a room from a ticket string.
    JoinFromTicket,
    /// The room switch / join failed.
    RoomJoinFailed {
        error: String,
        /// Room generation at join-initiation time; the handler asserts the
        /// current generation still matches before showing the error, so a
        /// stale failure for a superseded join does not yank the UI out of a
        /// newer room.
        generation: u64,
    },

    /// Open the file picker to select a file containing a friend's public key.
    ImportFriendFromFile,
    /// A file was selected for importing a friend's public key.
    ImportFriendFromFilePicked(String),
    // ── Group creation ──
    /// Show the group creation dialog.
    ShowCreateGroupDialog,
    /// Hide/cancel the group creation dialog.
    HideCreateGroupDialog,
    /// Update the group name input.
    CreateGroupNameChanged(String),
    /// Update the group description input.
    CreateGroupDescriptionChanged(String),
    /// Toggle a friend in the member selection.
    CreateGroupMemberToggled(PublicKey),
    /// Update the participant search/filter text.
    CreateGroupSearchChanged(String),
    /// Confirm and execute group creation.
    ConfirmCreateGroup,
    /// A group was created and is ready to join.
    GroupCreated {
        sender: Box<GossipSender>,
        topic: TopicId,
        ticket: String,
        entry: Box<ConversationEntry>,
        group_id: GroupId,
        name: String,
        description: String,
        members: Vec<PublicKey>,
        /// Room generation at group-creation initiation; the handler asserts
        /// the current generation still matches before navigating, so a stale
        /// creation cannot yank the UI out of a newer room.
        generation: u64,
    },
    // ── Tunnel creation ──
    /// Show the tunnel creation (friend-picker) dialog.
    ShowCreateTunnelDialog,
    /// The tunnel port input changed in the create-tunnel dialog.
    CreateTunnelPortChanged(String),
    /// Initiate a tunnel to a specific peer.
    CreateTunnel(PublicKey),
    /// Close the tunnel creation dialog without action.
    CancelCreateTunnel,
    /// An incoming tunnel request arrived from a peer.
    TunnelRequestReceived {
        peer: PublicKey,
        tunnel_id: String,
    },
    /// Accept an incoming tunnel request.
    AcceptTunnelRequest(String),
    /// Decline an incoming tunnel request.
    DeclineTunnelRequest(String),
    /// Close an active tunnel by its identifier.
    CloseTunnel(boru_core::tunnel::TunnelId),
    // ── ChatList ──
    JoinTicketInputChanged(String),
    NewChatCreated,
    RoomSelected(TopicId),
    /// Open a group chat from the groups sidebar list.
    OpenGroupChat(TopicId),

    // ── Chat ──
    /// Start a call through the actor without doing network work in `update`.
    StartVoiceCall(PublicKey),
    StartVideoCall(PublicKey),
    #[cfg(feature = "screen-sharing")]
    /// Start sharing with the given direct-chat peer (invitation + stream).
    StartScreenShare(PublicKey),
    #[cfg(feature = "screen-sharing")]
    /// Stop the local screen-share session immediately.
    StopScreenShare,
    #[cfg(feature = "screen-sharing")]
    /// Explicitly accept a pending screen-share invitation.
    AcceptScreenShare,
    #[cfg(feature = "screen-sharing")]
    /// Explicitly decline a pending screen-share invitation.
    DeclineScreenShare,
    #[cfg(feature = "screen-sharing")]
    /// Toggle the native viewer between inline and fullscreen presentation.
    ToggleScreenShareFullscreen,
    #[cfg(feature = "screen-sharing")]
    /// Toggle optional receiver session metadata below the viewer surface.
    ToggleScreenShareDetails,
    #[cfg(feature = "screen-sharing")]
    /// Forward one session event from the screen-share protocol subscription.
    ScreenShareEventReceived(SessionEvent),
    #[cfg(feature = "screen-sharing")]
    /// Forward the latest decoded frame from the viewer decode worker.
    ScreenShareFrameReceived(Option<CapturedFrame>),
    #[cfg(feature = "screen-sharing")]
    /// Periodic viewer pipeline stats for the developer diagnostics overlay
    /// (PDF Phase 12), published ~1 Hz by the decode worker.
    ScreenShareStatsReceived(Option<ScreenShareStatsSnapshot>),
    #[cfg(feature = "screen-sharing")]
    /// A screen-share control send finished (Accept/Reject/EndSession/Input).
    ScreenShareCommandFinished(Result<(), String>),
    #[cfg(feature = "screen-sharing")]
    /// Viewer requests explicit control (pointer + keyboard) from the host.
    ScreenShareRequestControl,
    #[cfg(feature = "screen-sharing")]
    /// Viewer requests the SEPARATE clipboard capability (PDF Task 9.3 /
    /// BORU-SS-25) — clipboard sync is never implied by remote control.
    ScreenShareRequestClipboard,
    #[cfg(feature = "screen-sharing")]
    /// Viewer pushes its local text clipboard to the host.
    ScreenShareSendClipboard,
    #[cfg(feature = "screen-sharing")]
    /// Host pushes its local text clipboard to the viewer.
    ScreenShareHostSendClipboard,
    #[cfg(feature = "screen-sharing")]
    /// Result of reading the local clipboard for screen-share sync.
    ScreenShareClipboardRead(Option<String>),
    #[cfg(feature = "screen-sharing")]
    /// Host grants the given control capabilities to the viewer.
    ScreenShareGrantControl(Vec<Capability>),
    #[cfg(feature = "screen-sharing")]
    /// Host declines the pending control request (no wire message).
    ScreenShareDenyControl,
    #[cfg(feature = "screen-sharing")]
    /// Host toggles system-audio sharing (BORU-SS-37). Audio is a SEPARATE
    /// optional capability — never enabled automatically with the share; the
    /// host must opt in (mirroring clipboard). Sends
    /// `HostCommand::SetAudioEnabled` into the host driver.
    ScreenShareToggleAudio,
    #[cfg(feature = "screen-sharing")]
    /// Host revokes control while keeping view-only sharing active.
    ScreenShareRevokeControl,
    #[cfg(feature = "screen-sharing")]
    /// Viewer asks the host to lower the stream quality manually
    /// (sends a versioned `QualityUpdate`).
    ScreenShareLowerQuality,
    #[cfg(feature = "screen-sharing")]
    /// Viewer asks the host to restore full quality (clears the manual ceiling).
    ScreenShareFullQuality,
    #[cfg(feature = "screen-sharing")]
    /// Sharer picks a capture source from the enumerated monitor list
    /// (PDF Phase 13: source selection before capture). Sends
    /// `HostCommand::SwitchSource` to the host driver so the choice applies
    /// whether the viewer has already accepted (in-session switch with a
    /// `SourceChanged` message) or is still deciding (pre-acceptance switch).
    ScreenShareSelectSource(CaptureSourceId),
    #[cfg(feature = "screen-sharing")]
    /// Sharer overrides the quality preset (BORU-SS-39). `None` restores the
    /// path-derived auto preset. Sends `HostCommand::SetQualityPreset` to the
    /// host driver.
    ScreenShareSetPreset(Option<QualityPreset>),
    #[cfg(feature = "screen-sharing")]
    /// Dismiss the terminal `Stopped` / `Error` notice and return to `Idle`.
    ScreenShareDismissNotice,
    #[cfg(feature = "screen-sharing")]
    /// Viewer pointer motion over the image (normalized 0..1, image-relative).
    ScreenSharePointerMove {
        x: f32,
        y: f32,
    },
    #[cfg(feature = "screen-sharing")]
    /// Viewer pointer button state change over the image.
    ScreenSharePointerButton {
        x: f32,
        y: f32,
        button: u32,
        pressed: bool,
    },
    #[cfg(feature = "screen-sharing")]
    /// Viewer key press/release while control is active (code = X11 keysym).
    ScreenShareKeyEvent {
        code: u32,
        pressed: bool,
    },
    #[cfg(feature = "screen-sharing")]
    /// Viewer wheel tick while control is active (normalized x/y + pixel
    /// deltas; the update maps the dominant axis to an X11 wheel button).
    ScreenShareWheel {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
    #[cfg(feature = "screen-sharing")]
    /// Set the scalable surface presentation mode and pan center.
    ScreenShareSetView {
        mode: ScreenShareViewMode,
        pan: Option<(f32, f32)>,
    },
    #[cfg(feature = "screen-sharing")]
    /// Begin a pan drag on the surface (viewer-only mode).
    ScreenSharePanStart {
        pos: iced::Point,
    },
    #[cfg(feature = "screen-sharing")]
    /// Continue a pan drag (viewer-only mode). `scale` is the surface scale
    /// at the time the drag began, so viewport deltas map to source pixels.
    ScreenSharePanMove {
        pos: iced::Point,
        scale: f32,
    },
    #[cfg(feature = "screen-sharing")]
    /// End a pan drag on the surface.
    ScreenSharePanEnd,
    #[cfg(feature = "screen-sharing")]
    /// Toggle the viewer's remote-cursor overlay (CUR-1 / BORU-SS-33).
    ToggleScreenShareCursor,
    /// Forward one event from the call actor subscription.
    CallEventReceived(CallEvent),
    /// Update notification suppression state from the native window focus event.
    WindowFocusChanged(bool),
    AcceptIncomingCall(CallId),
    RejectIncomingCall(CallId),
    HangUp(CallId),
    ToggleCallMute,
    /// Toggle local call playback without changing microphone capture.
    ToggleCallDeafen,
    ToggleCallCamera,
    SelectMicrophone(String),
    SelectSpeaker(String),
    SelectCamera(String),
    CallUiTick,
    CallStarted(Result<CallId, String>),
    CallCommandFinished(Result<(), String>),
    InputChanged(String),
    SendPressed,
    AttachPressed,
    /// The send task for the last submitted composer text finished (clears the
    /// transient "sending" button state).
    ComposerSendFinished,
    /// A window file was dragged over the app (true) or left (false).
    ComposerDragOver(bool),
    /// A window file was dropped onto the app; route through the normal
    /// attachment/send pipeline.
    ComposerFileDropped(PathBuf),
    /// Input-method (IME) composition state changed (true = composing).
    ComposerImeActive(bool),
    ToggleHelp,
    /// Toggle the chat options popover (room info, delete, settings).
    ToggleChatOptions,
    /// Toggle the in-conversation search panel.
    ToggleChatSearch,
    /// Live query text for the in-conversation search panel.
    ChatSearchQueryChanged(String),
    /// Clear conversation history from screen and database.
    ClearConversation,
    /// Toggle the right-side details panel.
    ToggleDetailsPanel,
    /// A reduced FS-05 projection update for an outbound transfer.
    TransferProjectionUpdate(ProjectionUpdate),
    /// The transfer broadcast receiver lagged or was restarted; rebuild the panel
    /// maps from the projection snapshot.
    TransferSnapshotResync,
    /// Cancel one inbound transfer shown in the Downloading tab (transfer id).
    DownloadingCancel(String),
    /// Pause one inbound transfer from the Download Manager (transfer id).
    DownloadingPause(String),
    /// Resume a paused inbound transfer from the Download Manager (transfer id).
    DownloadingResume(String),
    /// Stop an outbound upload from the Download Manager (transfer id).
    DownloadingStop(String),
    /// Open the Download Manager screen (all active downloads + uploads).
    OpenDownloadManager,
    /// Close the Download Manager screen and return to the previous screen.
    CloseDownloadManager,
    /// Toggle the "Files I'm Sharing" row action menu (content hash).
    SharedByMeMenuToggle(String),
    /// Open the details/access panel for a shared-by-me item (content hash).
    SharedByMeDetails(String),
    /// Close the "Files I'm Sharing" details/access panel.
    SharedByMeCloseDetails,
    /// Reveal a shared item's source file in the OS file manager (content hash).
    SharedByMeReveal(String),
    /// Show the inline stop-sharing confirmation for an item (content hash).
    SharedByMeConfirmStopSharing(String),
    /// Cancel the inline stop-sharing confirmation.
    SharedByMeCancelStopSharing,
    /// Revoke one recipient's access grant (content hash, grantee id).
    SharedByMeRevokeAccess(String, String),
    /// The shared-by-me projection finished loading after opening the dashboard.
    SharedByMeLoaded(Result<Vec<crate::shared_by_me_table::SharedByMeRow>, String>),
    /// UI-30: a uniform thumbnail finished generating for a Shared by Me row.
    /// `None` means generation failed or the file is not media — the row falls
    /// back to its type icon.
    SharedByMeThumbnailReady {
        content_hash: String,
        handle: Option<iced::widget::image::Handle>,
    },
    /// Durable transfer activity loaded for the Recent Download Activity card.
    DashboardRecentActivityLoaded(Vec<crate::recent_activity_view_model::RecentActivityRow>),
    /// The FS-13 Sharing Summary projection finished loading. `None` keeps
    /// the card in its unknown state (em dashes) — never a premature zero.
    DashboardSharingSummaryLoaded(Option<crate::sharing_summary::SharingSummary>),
    /// Reload the durable completed-download history for the Downloaded tab.
    DashboardDownloadedRefresh,
    /// The completed-download history finished loading.
    DashboardDownloadedLoaded(
        Result<Vec<crate::dashboard_view_model::CompletedDownloadItem>, String>,
    ),
    /// Open a completed download with the native OS handler (download row id).
    DownloadedOpen(i64),
    /// Reveal a completed download in the OS file manager (download row id).
    DownloadedReveal(i64),
    /// Remove a completed download's history record only — never the file.
    DownloadedRemoveHistory(i64),
    /// Toggle the group member list overlay (group conversations only).
    ToggleMemberList,
    OpenSettings,
    CloseSettings,
    /// Open the friend requests management screen.
    OpenFriendRequests,
    /// Open the file sharing dashboard screen.
    OpenFileSharing,
    /// Search input changed in the file sharing dashboard.
    DashboardSearchChanged(String),
    /// Clear the dashboard search query in one action (the header × button,
    /// or Escape while the field has text).
    DashboardSearchCleared,
    /// A sort key was clicked on the Shared by Me table (FS-18).
    DashboardSharedByMeSortClicked(crate::dashboard_filters::SharedByMeSortKey),
    /// A sort key was clicked on the Downloaded tab (FS-18).
    DashboardDownloadedSortClicked(crate::dashboard_filters::DownloadedSortKey),
    /// A sort key was clicked on the Activity Log tab (FS-18).
    DashboardActivitySortClicked(crate::dashboard_filters::ActivitySortKey),
    /// Tab selected in the file sharing dashboard.
    DashboardTabSelected(crate::dashboard_view_model::DashboardTab),
    /// The durable Activity Log projection finished loading (FS-17).
    ActivityLogLoaded(Vec<crate::activity_log_view_model::ActivityLogRow>),
    /// Reload the durable Activity Log projection.
    ActivityLogRefresh,
    /// A filter chip was selected in the Activity Log tab.
    ActivityLogFilterSelected(crate::activity_log_view_model::ActivityLogFilter),
    /// The Activity Log pagination moved to a page (zero-based).
    ActivityLogPageSelected(usize),
    /// Toggle the raw-error details affordance for an event id.
    ActivityLogDetailsToggled(String),
    /// The user asked to clear the local activity history (opens confirm).
    ActivityLogClearRequested,
    /// The user cancelled the clear-history confirmation.
    ActivityLogClearCancelled,
    /// The user confirmed the clear-history action.
    ActivityLogClearConfirmed,
    /// User dismissed the dashboard connectivity notice (offline / stale).
    DashboardConnectivityDismissed,
    /// Reload the active-downloads projection for the Downloading tab.
    DashboardDownloadingRefresh,
    /// A peer catalogue fetch failed. Carries the error message.
    CatalogueFetchFailed(String),
    /// User dismissed a catalogue-fetch error on the Shared with Me tab.
    CatalogueErrorDismissed,
    /// Toggle a sidebar section's collapsed state by index (0=chats, 1=friends, 2=discover, 3=requests, 4=public_rooms).
    ToggleSidebarSectionCollapsed(usize),
    CloseFriendRequests,
    FriendRequestSearchChanged(String),
    FriendRequestSend(String),
    FriendRequestAccept(String),
    FriendRequestDecline(String),
    FriendRequestCancel(String),
    FriendRequestSentResult(Result<FriendRequest, String>),
    FriendRequestActionResult(Result<FriendRequest, String>),
    NetEvent(ConversationNetEvent),
    /// Continue draining a hidden conversation's pending-event queue without
    /// blocking the Iced update loop.
    ReplayPendingEvents(TopicId),
    FriendEvent(FriendEvent),
    /// An event from the whisper (DM) protocol.
    WhisperEvent(WhisperEvent),
    /// An event from the inbox (offline-message) protocol.
    InboxEvent(InboxEvent),
    /// Results of the GUI's legacy mailbox retry pass.
    OutboxRetryResult(Vec<(TopicId, u64, bool)>),
    /// User tapped a failed outgoing message to retry it.
    RetryOutgoingMessage(u64),
    MessageSent(String, u64, MessageHash),
    FileSent(String),
    DownloadDone(String, PathBuf),
    /// File downloaded from a peer's shared profile — carries the saved path
    /// for the "Open" button.
    DownloadDonePeerFile(String, PathBuf),
    /// Result of probing a verified local video for an asynchronous poster.
    PosterGenerated {
        name: String,
        poster: Result<(Vec<u8>, Option<(u32, u32)>), String>,
    },
    /// Result of the async intrinsic-metadata probe (width/height/duration)
    /// for a verified local video. Success carries real measurements only;
    /// the card falls back to a bounded generic frame when dimensions are
    /// unavailable and the problem is logged through diagnostics (VIDCARD-09).
    VideoMetadataProbed {
        name: String,
        metadata: Result<boru_core::video_playback::MediaMetadata, String>,
    },
    DownloadFailed(String),
    /// Open a downloaded file with the platform default application.
    OpenDownloadedFile(String),
    /// Start verified inline playback for a completed video attachment.
    PlayInlineVideo(usize),
    /// Start progressive inline playback for a video that is still being
    /// downloaded: starts a local HTTP streaming server over the growing
    /// blob-store file and opens the inline player at its URL.
    StreamInlineVideo(usize),
    #[cfg(feature = "video-playback")]
    /// The streaming HTTP server is ready; open the inline player at the URL.
    StreamingServerReady {
        entry_index: usize,
        url: String,
        server: Arc<StreamingServer>,
    },
    #[cfg(feature = "video-playback")]
    /// The streaming HTTP server failed to start.
    StreamingServerFailed {
        entry_index: usize,
        error: String,
    },
    #[cfg(feature = "video-playback")]
    InlineVideoTick,
    InlineVideoShowControls,
    #[cfg(feature = "video-playback")]
    /// Keyboard focus entered or left the inline video controls
    /// (`true` = inside controls, `false` = left them).
    InlineVideoControlsFocused(bool),
    #[cfg(feature = "video-playback")]
    InlineVideoSeekChanged(f32),
    #[cfg(feature = "video-playback")]
    InlineVideoSeekReleased,
    #[cfg(feature = "video-playback")]
    /// Seek by a relative number of seconds from a keyboard-focused player control.
    InlineVideoSeekRelative(f32),
    #[cfg(feature = "video-playback")]
    InlineVideoToggleMute,
    #[cfg(feature = "video-playback")]
    /// Adjust volume by a relative amount from a keyboard-focused player control.
    InlineVideoAdjustVolume(f32),
    #[cfg(feature = "video-playback")]
    InlineVideoSetVolume(f32),
    #[cfg(feature = "video-playback")]
    InlineVideoToggleExpanded,
    #[cfg(feature = "video-playback")]
    /// Close the active inline player and restore its poster.
    CloseInlineVideo,
    /// A video stream URL is ready for external playback (no video-playback feature).
    StreamUrl(String),
    #[cfg(feature = "video-playback")]
    InlineVideoEvent(InlineVideoEvent),
    OpenDownloadsFolder,
    ErrorMsg(String),
    ExecuteFileSend(String),
    /// Open the native folder picker and send the selected directory as a
    /// HashSeq collection (SENDME-01).
    AttachFolderPressed,
    /// Send a whole directory as a HashSeq collection (SENDME-01).  Encoded
    /// payload is `name|abs_dir_path|abs_dir_path` (same shape as files).
    ExecuteFolderSend(String),
    ExecuteDownload,
    /// Start downloading the attachment belonging to a specific chat entry.
    /// Keeping the entry index in the message allows multiple file rows to
    /// download concurrently without a single global "pending file" slot.
    ExecuteDownloadAt(usize),
    /// Pause an active download at the given entry index.
    PauseDownloadAt(usize),
    /// Resume a paused download at the given entry index.
    ResumeDownloadAt(usize),
    /// Cancel or remove a download at the given entry index.
    CancelDownloadAt(usize),
    /// Re-share a completed download to the current room.
    ReshareFile(usize),
    /// Open the sender-side "share via short code" dialog for a download card.
    MintShortCode(usize),
    /// Result of the mint + subscribe step for a short code share. Carries
    /// the rendezvous-topic sender so the dialog can keep broadcasting.
    ShortCodeMinted(std::result::Result<(String, GossipSender), String>),
    /// Close the sender-side short-code dialog and stop broadcasting.
    CloseShortCodeDialog,
    /// Copy the short code shown in the sender dialog to the clipboard.
    CopyShortCode(String),
    /// Open the receiver-side "redeem a short code" dialog.
    OpenRedeemCodeDialog,
    /// Close the receiver-side redeem dialog.
    CloseRedeemCodeDialog,
    /// The typed code in the redeem dialog changed.
    RedeemCodeInputChanged(String),
    /// Start resolving the typed short code over the rendezvous gossip topic.
    RedeemShortCode,
    /// A short-code announcement was received and verified; create the
    /// download card exactly like a pasted ticket.
    ShortCodeRedeemed(std::result::Result<ShortCodeRedemption, String>),
    /// Set the overwrite-conflict policy on a download card (FS-26).
    SetOverwritePolicy(usize, boru_core::safe_destination::OverwritePolicy),
    /// A download initiated from a peer profile was created successfully.
    DownloadInitiated {
        /// Content hash of the file.
        content_hash: String,
        /// The remote peer.
        peer: PublicKey,
        /// The database id of the new download.
        download_id: i64,
    },
    /// A download initiated from a peer profile failed.
    DownloadInitiationFailed {
        /// Content hash of the file.
        content_hash: String,
        /// The remote peer.
        peer: PublicKey,
        /// Human-readable error message.
        error: String,
    },
    /// Open a peer's profile panel showing shared files with Download buttons.
    OpenPeerProfile(PublicKey),
    /// Open the redesigned friend profile screen with context menu.
    OpenFriendProfile(PublicKey),
    /// Close the friend profile screen and return to the previous screen.
    CloseFriendProfile,
    /// Toggle the three-dot context menu in the friend profile.
    ToggleFriendProfileMenu,
    /// Open the "Share local service" dialog for the friend whose profile is open.
    OpenShareLocalService,
    /// Open the experimental localhost-only VNC tunnel flow.
    OpenShareVncTunnel,
    /// Service name changed in the share dialog.
    ShareLocalServiceNameChanged(String),
    /// Local port changed in the share dialog.
    ShareLocalServicePortChanged(String),
    /// Expiry duration selected in the share dialog.
    ShareLocalServiceExpiryChanged(boru_core::tunnel::service::TunnelDuration),
    /// Confirm the share dialog and create the tunnel.
    ConfirmShareLocalService,
    /// Cancel the share dialog.
    CancelShareLocalService,
    /// A local-service scan finished with discovered suggestions.
    ShareLocalServiceScanDone(Vec<boru_core::local_service_scan::LocalServiceSuggestion>),
    /// The user picked a suggested local service (port) in the share dialog.
    SelectShareLocalServiceSuggestion(u16),
    /// A local service tunnel was created successfully.
    TunnelShared {
        /// Display name of the shared service.
        name: String,
        /// Display name of the friend the tunnel is shared with.
        friend: String,
        /// Absolute expiry timestamp (unix ms).
        expires_at_ms: u64,
    },
    /// Creating a local service tunnel failed.
    TunnelShareFailed {
        message: String,
    },
    /// The signed tunnel offer was dispatched to the friend's whisper inbox.
    TunnelOfferSent,
    /// Dispatching the tunnel offer to the friend failed (e.g. offline).
    TunnelOfferSendFailed {
        message: String,
    },
    /// Whether the shared service is explicitly identified as HTTP toggled.
    /// Controls whether the receiving side shows `http://` before the
    /// loopback address — never inferred from the port or service name.
    ShareLocalServiceHttpToggled(bool),
    /// Connect a received tunnel offer: bind a loopback listener that routes
    /// through the tunnel to the sharer's service.
    ConnectReceivedTunnel(boru_core::tunnel::TunnelId),
    /// A received tunnel's local listener is now accepting connections.
    ReceivedTunnelConnected {
        /// Tunnel being connected.
        tunnel_id: boru_core::tunnel::TunnelId,
        /// Loopback address the listener is bound to.
        local_addr: std::net::SocketAddr,
        /// Cancellation token driving the background listener task.
        cancellation: tokio_util::sync::CancellationToken,
        /// Shared live connection info updated by the listener transport.
        live_info: std::sync::Arc<boru_core::tunnel::service::TunnelLiveInfo>,
        /// Port the sharer requested through the TunnelOffer, when one was
        /// chosen. `Some(p)` with `p != local_addr.port()` means the listener
        /// fell back to an ephemeral port because the requested port was
        /// unavailable.
        requested_port: Option<u16>,
    },
    /// Connecting to a received tunnel failed.
    ReceivedTunnelConnectFailed {
        /// Tunnel being connected.
        tunnel_id: boru_core::tunnel::TunnelId,
        /// Human-readable failure reason.
        message: String,
    },
    /// Disconnect a connected received tunnel (cancels its listener task).
    DisconnectReceivedTunnel(boru_core::tunnel::TunnelId),
    /// Stop sharing a locally-shared tunnel (revokes it in the backend).
    StopSharingTunnel(boru_core::tunnel::TunnelId),
    /// Open the connected received tunnel's loopback address in a browser.
    OpenReceivedTunnel(boru_core::tunnel::TunnelId),
    /// Copy the connected received tunnel's loopback address to the clipboard.
    CopyReceivedTunnelAddress(boru_core::tunnel::TunnelId),
    /// Text input changed for inline rename of a friend's display name.
    FriendRenameInputChanged(String),
    /// Confirm the inline rename of a friend's display name.
    FriendRenameConfirm,
    /// Copy a peer's public key ID to the clipboard with toast feedback.
    CopyPeerId(PublicKey),
    /// Open the reusable connection-details dialog.
    OpenConnectionDetails,
    /// Close the reusable connection-details dialog.
    CloseConnectionDetails,
    // ── Invite Member ──
    /// Show the invite member dialog for the current group.
    ShowInviteMemberDialog,
    /// Hide the invite member dialog.
    HideInviteMemberDialog,
    /// Toggle a friend in the invite member selection.
    InviteMemberToggled(PublicKey),
    /// Confirm and send the group invite to selected members.
    ConfirmInviteMember,
    /// Accept a pending group invite and join the room.
    AcceptGroupInvite(Vec<u8>),
    // ── End Invite Member ──
    /// Copy the support-safe connection-details summary to the clipboard.
    CopyConnectionDetails,
    /// Copy one redacted connection-details value to the clipboard.
    CopyConnectionDetailsValue {
        label: String,
        value: String,
    },
    /// Dismiss the toast notification.
    DismissToast,
    /// Show the "Remove Friend" confirmation dialog.
    ShowRemoveFriendConfirm,
    /// Cancel the friend removal.
    CancelRemoveFriend,
    /// Confirm friend removal.
    ConfirmRemoveFriend,
    /// Show the "Block Friend" confirmation dialog.
    ShowBlockFriendConfirm,
    /// Show inline rename input for the friend's display name.
    ShowRenameFriendInput,
    /// Cancel the block action.
    CancelBlockFriend,
    /// Confirm blocking a friend.
    ConfirmBlockFriend,
    /// Close the peer profile panel and return to the previous screen.
    ClosePeerProfile,
    ExecuteImageSend(String),
    ImageDownloaded {
        sender: PublicKey,
        name: String,
        /// Display label that may include compression info like "photo.webp (45% smaller)".
        /// Passed directly into the chat entry's body text.
        display_name: String,
        image_bytes: Vec<u8>,
        message_hash: MessageHash,
        /// ImageStore identifier pre-saved by the async download task.
        /// None if the save failed (error is set on the chat entry instead).
        image_identifier: Option<String>,
        /// Conversation generation captured when the download was started.
        /// The handler asserts the current generation still matches before
        /// applying, so a download that resolves after a room switch cannot
        /// silently mutate the wrong conversation's entries.
        generation: u64,
    },
    /// External catalogue GIF media fetched over HTTP.
    ///
    /// `bytes` is `Ok` when the media URL was fetched and decoded, or `Err`
    /// with a human-readable message when the media is missing, expired, or
    /// unreachable — the handler then renders a clear fallback entry.
    GifMediaFetched {
        sender: PublicKey,
        gif: boru_core::gif_provider::SharedGif,
        message_hash: MessageHash,
        bytes: Result<Vec<u8>, String>,
        /// Conversation generation captured when the fetch was started.
        generation: u64,
    },
    FriendAdded {
        fid: String,
        label: String,
        was_new: bool,
    },
    FriendRemoved {
        label: String,
    },
    /// Remove a friend from the friends list (UI request).
    RemoveFriend(PublicKey),
    FriendListResult(Vec<(String, String)>),
    /// Delete a room from history (home screen delete or /leave).
    DeleteRoom(TopicId),
    /// Periodic tick for the splash screen spinner animation.
    SplashTick,
    /// Periodic redraw for relative labels in the home activity feed.
    ActivityTick,
    /// Periodic tick for connection type refresh.
    ConnMonitorTick,
    /// Periodic tick for mesh quiescence watchdog.
    MeshWatchdogTick,
    /// Periodic tick for the GUI's legacy mailbox retry pass.
    OutboxRetryTick,

    /// Toggle dark mode on/off.
    ToggleDark(bool),
    /// A debounced `boru-ui.toml` reload finished on the watcher thread
    /// (BORU-UI-06). `generation` increases per reload; results older than
    /// the last accepted generation are dropped in update(). The parsed
    /// config is delivered here so BORU-UI-07 can apply it to the live
    /// theme; this task only delivers + tracks staleness.
    UiThemeReloaded {
        generation: u64,
        result: Result<crate::theme_config::UiThemeConfig, crate::theme_config::ThemeReloadError>,
    },
    /// A debounced `boru-layout.toml` reload finished on the watcher thread
    /// (BORU-LAYOUT-06). `generation` increases per reload; results older
    /// than the last accepted generation are dropped in update(). The
    /// parsed overrides are delivered here so the merge layer can apply
    /// them to the live layout (keeping the last known-good layout on
    /// parse errors).
    LayoutReloaded {
        generation: u64,
        result: Result<crate::layout::LayoutOverrides, crate::layout_config::LayoutReloadError>,
    },
    /// BORU-UI-09: a dev UI Inspector panel message (Ctrl+Shift+D toggles
    /// the panel; sliders/inputs/toggles/colour fields emit edits). Compiled
    /// only with the `dev-ui` cargo feature.
    #[cfg(feature = "dev-ui")]
    Inspector(crate::inspector::InspectorMsg),
    /// Open/close the iced_aw ColorPicker overlay in Settings (accent color).
    ToggleAccentColorPicker,
    /// The user confirmed a new accent color in the ColorPicker (RGB bytes).
    AccentColorSelected([u8; 3]),
    /// The user cancelled the ColorPicker overlay.
    AccentColorCancelled,
    /// Update the local display name (nickname).
    SetNickname(String),

    /// Window was resized — carries the new logical width and height.
    /// Both dimensions feed the canonical responsive layout model.
    WindowResized {
        width: f32,
        height: f32,
    },

    /// Internal no-op for async task completions that should not change UI state.
    Noop,

    /// Toggle the developer component gallery screen (dev-ui builds only).
    #[cfg(feature = "dev-ui")]
    ToggleGallery,
    /// BORU-UI-15: select a gallery responsive-preview preset (dev-ui only).
    #[cfg(feature = "dev-ui")]
    GalleryPreset(crate::component_gallery::GalleryWidthPreset),
    /// BORU-UI-15: the gallery's custom-width slider moved (dev-ui only).
    #[cfg(feature = "dev-ui")]
    GalleryCustomWidth(f32),
    /// BORU-LAYOUT-09: select a gallery layout-config preset (dev-ui only).
    #[cfg(feature = "dev-ui")]
    GalleryLayoutPreset(crate::component_gallery::GalleryLayoutPreset),

    // ── Shared file catalogue management ──
    /// Open the file picker to select a file for sharing.
    AddSharedFile,
    /// Open the native folder picker (FS-10). Folder registration is not a
    /// separate subsystem: the secure catalogue is file-based, so a picked
    /// folder is surfaced as an explicit limitation, never flattened or faked.
    AddSharedFolder,
    /// A folder was selected via the native picker — carries the folder path.
    SharedFolderPicked(String),
    /// Toggle the compact "+ Share Files or Folder" menu on the Shared by Me card.
    SharedByMeToggleShareMenu,
    /// A file was selected via the picker — contains the file path.
    SharedFilePicked(String),
    /// Result of adding a shared file (success message or error).
    SharedFileAdded(String),
    /// Registering a shared file failed; carries a safe (path-free) message.
    SharedFileAddFailed(String),
    /// Remove a shared file by its content hash.
    RemoveSharedFile(String),
    /// Confirmation that a shared file was removed.
    SharedFileRemoved(String),

    /// Save the profile (display name + bio) to disk.
    SaveProfile,
    /// Profile was saved to disk; broadcast ProfileUpdate.
    ProfileSaved,
    /// Copy text to the system clipboard.
    CopyToClipboard(String),
    /// Copy a chat message body to the clipboard with visual feedback.
    CopyMessage(usize),
    /// Right-click on a text message to open a context menu.
    RightClickText(usize),
    /// Right-click on an image to open a context menu.
    RightClickImage(usize),
    /// Copy text from context menu.
    ContextCopyText(usize),
    /// Copy image data from context menu.
    ContextCopyImage(usize),
    /// Pin a message from its context menu.
    PinMessage(usize),
    /// Remove a pin from a message from its context menu.
    UnpinMessage(usize),
    /// Reveal a pinned message in the conversation timeline.
    RevealPinnedMessage(MessageHash),
    /// Dismiss the context menu overlay.
    CloseContextMenu,
    /// Toggle the video-card header overflow menu for a chat entry.
    ToggleVideoCardMenu(usize),
    /// Toggle the emoji picker panel visibility.
    ToggleEmojiPicker,
    /// Insert an emoji into the composer.
    ///
    /// Carries the full Unicode grapheme string (possibly multiple code
    /// points: variation selectors, skin-tone modifiers, ZWJ sequences),
    /// never an asset key or SVG path. BORU-TWEMOJI-10.
    InsertEmoji(String),
    /// Switch the emoji picker to a different content category
    /// (BORU-TWEMOJI-12). The picker grid is rebuilt from the filtered
    /// catalog, so no stale items survive the switch.
    SelectEmojiCategory(crate::emoji::EmojiCategory),
    /// Live emoji picker search query change (BORU-TWEMOJI-13). Every
    /// keystroke updates the query; the picker view filters the shared
    /// catalog immediately. An empty query restores the category view.
    EmojiSearchChanged(String),
    /// Toggle the GIF picker panel.
    ToggleGifPicker,
    /// Search text changed in the GIF picker.
    GifSearchChanged(String),
    /// Send a GIF selected from the picker as an image attachment.
    SendGif(GifSearchResult),
    /// Trigger a GIF search through the configured external provider.
    GifSearchSubmit,
    /// Retry the last failed GIF request (trending when the query is empty,
    /// otherwise the current search).  Unlike `GifSearchSubmit`, this works
    /// even when the failure happened before any query was entered (e.g. a
    /// trending request failed on picker open).
    GifRetry,
    /// Debounce timer fired with the debounce seq; only the latest fires.
    GifSearchDebounced(u64),
    /// GIF search results arrived (provider-neutral page + request seq).
    GifSearchResults {
        seq: u64,
        page: GifSearchPage,
    },
    /// Trending GIFs arrived (provider-neutral page + request seq).
    GifTrendingResults {
        seq: u64,
        page: GifSearchPage,
    },
    /// A GIF request failed (request seq + user-facing message).
    GifSearchFailed {
        seq: u64,
        message: String,
    },
    /// A GIF preview thumbnail finished downloading (provider_id, bytes).
    GifPreviewLoaded(String, Vec<u8>),
    /// Load the next page of GIF results (pagination).
    GifLoadMore,
    /// Copy the user's own friend ID (public key) to the clipboard with visual feedback.
    CopyFriendId,
    /// Clear the "Copied!" visual feedback after copy.
    FriendIdCopiedClear,
    /// Copy the share ticket (BlobTicket string) of a sent/completed file to
    /// the clipboard, like sendme's `sendme receive <ticket>` output.
    CopyShareTicket(usize),
    /// Open the "Receive from ticket" dialog (paste a BlobTicket to download
    /// a file shared outside the friend graph).
    OpenReceiveTicketDialog,
    /// Close the "Receive from ticket" dialog.
    CloseReceiveTicketDialog,
    /// The pasted ticket text changed.
    ReceiveTicketInputChanged(String),
    /// Run the pre-flight check (parse ticket, connect, read size/name).
    ReceiveTicketPreflight,
    /// Pre-flight finished: Ok carries the display name + total size;
    /// Err carries a user-facing error message.
    ReceiveTicketPreflightDone(Result<ReceiveTicketPreflight, String>),
    /// User confirmed the pre-flighted ticket — start the download through
    /// the existing download machinery into a safe destination.
    ConfirmReceiveTicket,
    /// Open a direct chat with an online friend.
    OpenFriendChat(PublicKey),
    /// Toggle notification sounds on/off.
    ToggleSound(bool),
    /// Set global message notification policy.
    SetNotificationPolicy(crate::notification::service::NotificationPolicy),
    /// Override or reset this conversation's notification policy.
    SetConversationNotificationPolicy(
        TopicId,
        Option<crate::notification::service::NotificationPolicy>,
    ),
    /// Toggle the optional BORU-CP-06 UI presence indicator on/off.
    /// Presentation-only: never affects discovery or reconnection.
    TogglePresenceIndicator(bool),
    /// Toggle ephemeral typing indicators.
    ToggleTypingIndicators(bool),
    /// Toggle whether room invitations include direct endpoint addresses.
    ToggleInviteAddressSharing(bool),
    /// Set the chat message body text size in pixels.
    SetChatTextSize(f32),
    /// Open the native picker for a local profile image.
    PickProfileImage,
    /// Result of reading the selected profile image.
    ProfileImagePicked(Result<Vec<u8>, String>),
    /// Open the native picker for a home-screen background image.
    PickHomeBackgroundImage,
    /// Result of picking a home-screen background image: the on-disk path.
    /// The bytes are read and decoded into the UI handle by the handler.
    HomeBackgroundImagePicked(Result<String, String>),
    /// The background image file was read and settings persisted; carries the
    /// path and raw bytes so the handler can build the UI handle.
    HomeBackgroundImageReady {
        path: String,
        image_bytes: Vec<u8>,
    },
    /// Remove the currently configured home-screen background image.
    RemoveHomeBackgroundImage,
    /// Set the opacity (0.0–1.0) of home-screen menu item backgrounds
    /// over the home background image (HOME-01).
    SetHomeMenuItemOpacity(f32),
    /// The profile image was uploaded to the local blob store; carries the
    /// BlobTicket string peers use to download it.
    ProfileImageUploaded(String),
    /// Remove the currently configured profile image.
    RemoveProfileImage,
    /// The background remove-profile-image task completed successfully.
    ProfileImageRemoved,
    /// Profile image was saved to the per-user image store and the identifier
    /// was persisted. Carries the identifier and raw bytes for the UI handle
    /// and blob store upload.
    ProfileImagePersisted {
        identifier: String,
        image_bytes: Vec<u8>,
    },
    /// Push a system message to the active room chat log.
    SystemMsg(String),
    /// A remote peer's profile image blob was downloaded and decoded.
    ProfileImageDownloaded(PublicKey, Vec<u8>),
    /// A remote peer's profile image download failed — clear cached ticket so
    /// the next periodic AboutMe broadcast can retry.
    ProfileImageDownloadFailed(PublicKey),
    /// Result of a background image hydration task: the entry at `index` now
    /// has a decoded image handle ready for rendering, or an error message.
    /// Processing image data off the UI thread prevents scroll jank when
    /// re-hydrating stored images from disk.
    ImageHydrated {
        index: usize,
        handle: Option<iced::widget::image::Handle>,
        error: Option<String>,
    },
    /// User requested to clear chat history — show confirmation.
    ClearHistoryRequested,
    /// User confirmed the clear history action.
    ConfirmClearHistory,
    /// Clear-history request completed successfully for a room.
    ClearHistoryFinished {
        topic: TopicId,
        room_history: RoomHistoryStore,
        report: RoomHistoryClearReport,
    },
    /// Clear-history request failed.
    ClearHistoryFailed {
        topic: TopicId,
        error: String,
    },
    /// User requested to delete a room — show confirmation.
    DeleteRoomRequested(TopicId),
    /// User confirmed deletion of a room.
    ConfirmDeleteRoom(TopicId),
    /// Results of a mailbox sync triggered on whisper reconnect.
    MailboxReplayed {
        /// Peer whose envelopes were replayed.
        peer: PublicKey,
        /// Accepted entries: (message_id, plaintext).
        texts: Vec<(String, String)>,
    },
    /// An offline DM was persisted and its current delivery status is known.
    OfflineDMStatus {
        /// Stable envelope identifier (blake3 hash of the envelope bytes).
        message_id: String,
        /// Human-readable peer label for the chat log.
        label: String,
        /// Current transport status.
        status: OfflineDeliveryStatus,
    },
    /// Scroll offset / viewport changed in the chat log.
    /// Used by windowed rendering to determine which entries to build widgets for.
    Scrolled(f32, f32),
    /// Async result from connection-type refresh (direct vs relay counts).
    ConnCountsResult {
        direct: usize,
        relayed: usize,
    },
    /// Async result from resolving pending ticket peers into concrete
    /// [`EndpointAddr`]s via `endpoint.remote_info()` inside ConnMonitorTick.
    /// `unresolved` are peers whose addressing info was not available yet;
    /// they are re-queued for a later resolution attempt.
    TicketPeersResolved {
        resolved: Vec<EndpointAddr>,
        unresolved: Vec<PublicKey>,
    },
    /// Async result from the /connections debug command.
    ConnectionsResult(Vec<String>),
    /// Send a friend request to a peer.
    SendFriendRequest(PublicKey),
    /// The friend request was sent successfully.
    FriendRequestSent {
        peer: PublicKey,
        request_id: String,
    },
    /// The friend request failed to send.
    FriendRequestFailed {
        peer: PublicKey,
        error: String,
    },
    /// The friend request was accepted by the recipient (incoming or outgoing).
    FriendRequestReceived {
        peer: PublicKey,
        request_id: String,
        status: FriendRequestStatus,
    },
    /// Retry a failed friend request.
    FriendRequestRetry(PublicKey),
    /// Accept an incoming friend request.
    IncomingFriendRequestAccept {
        request_id: String,
        peer: PublicKey,
    },
    /// Decline an incoming friend request.
    IncomingFriendRequestDecline {
        request_id: String,
        peer: PublicKey,
    },
    /// An incoming friend request was processed (accepted or declined).
    IncomingFriendRequestProcessed {
        request_id: String,
        peer: PublicKey,
        status: FriendRequestStatus,
    },
    /// A file/image download progress event from a background task.
    DownloadProgress(TransferProgress),
    /// Open a conversation with a peer (derive topic, create record, select).
    OpenConversation(PublicKey),
    /// Select a conversation for display (UI-only switch).
    SelectConversation(TopicId),
    /// Close / archive a conversation (remove from local list, keep friend).
    CloseConversation(TopicId),
    /// Send a text message to the specified conversation.
    SendMessage {
        conversation_topic: TopicId,
        content: String,
    },
    /// An update to the peers currently advertised by local discovery.
    NewDiscoveredPeers(DiscoveredPeersUpdate),
    /// BORU-CP-07: the backend re-established endpoint connectivity to a
    /// friend (reconnect attempt succeeded). The app ensures the
    /// deterministic direct topic is joined/subscribed.
    ReconnectPeerReady(PublicKey),

    // ── Remote catalogue browsing ──
    /// Initiate fetching a remote peer's shared file catalogue.
    BrowsePeerCatalogue(PublicKey),
    /// The remote catalogue was received successfully.
    PeerCatalogueReceived {
        /// The peer whose catalogue was fetched.
        peer: PublicKey,
        /// The files in their catalogue.
        files: Vec<RemoteSharedFile>,
    },
    /// The remote catalogue fetch failed.
    PeerCatalogueFailed(String),
    /// Scroll position changed in the windowed catalogue view.
    CatalogueScrolled(f32, f32),
    /// Request a file download from a peer's catalogue.
    RequestFileDownload {
        /// The peer hosting the file.
        peer: PublicKey,
        /// The file to download.
        file: RemoteSharedFile,
    },

    // ── Invite menu ──
    /// Toggle the invite menu popover in the current room view.
    ToggleInviteMenu,
    /// The peer key input in the invite whisper field changed.
    InviteWhisperInputChanged(String),
    /// Send a room invite via whisper to the entered peer key.
    InviteSendWhisper,
    /// Open the full-screen image lightbox for the given entry index.
    OpenImageLightbox(usize),
    /// Close the full-screen image lightbox.
    CloseImageLightbox,
    /// Image processing failed after the user selected it.
    ImageUploadFailed(String),
    /// File upload/sharing failed after the user selected it.
    FileUploadFailed(String),
    /// The direct file offer was broadcast before background blob ingest.
    FileOfferAnnounced {
        offer_id: boru_core::chat_core::protocol::FileOfferId,
    },
    /// Background blob ingest completed for an already announced offer.
    FileOfferCached {
        offer_id: boru_core::chat_core::protocol::FileOfferId,
        ticket: String,
        content_hash: String,
        thumbnail: Option<Vec<u8>>,
    },
    /// Background blob ingest failed; the direct offer may remain usable.
    FileOfferCacheFailed {
        offer_id: boru_core::chat_core::protocol::FileOfferId,
        error: String,
    },
    FileDownloaded {
        name: String,
        ticket: String,
        thumbnail: Option<Vec<u8>>,
        /// Absolute path to the local file (set only for the uploader).
        local_path: Option<String>,
    },
    /// A thumbnail blob hash was resolved; update the download card with the bytes.
    ThumbnailFetched {
        entry_index: usize,
        thumbnail_bytes: Vec<u8>,
    },

    // ── GUI test actions (MCP-driven) ──
    /// An action received from the MCP GUI test actions channel.
    GuiTestActionReceived(GuiActionRequest),
    /// Internal timer completion for an action that has not reached its
    /// expected state. The handler only expires still-active actions.
    GuiActionTimeout(GuiActionId),
    /// A wait condition has been satisfied.
    GuiTestWaitSatisfied(String),
    /// A wait condition timed out.
    GuiTestWaitTimedOut {
        idempotency_key: String,
        condition: String,
        expected: String,
        elapsed_ms: u64,
    },
    /// Retry connecting to the relay (manual retry from the dashboard).
    RetryConnection,
    /// Subscribe to a conversation topic in the background (no UI switch).
    BackgroundSubscribe(TopicId, Vec<PublicKey>),
    /// Background subscription completed. The forwarder must remain owned by
    /// the conversation; dropping it here leaves the gossip receiver
    /// unpolled, which makes background rooms appear asymmetrically connected.
    BackgroundSubscribed(
        TopicId,
        Option<GossipSender>,
        Option<Arc<StdMutex<Option<n0_future::task::JoinHandle<()>>>>>,
    ),
    /// Background subscription failed for the topic that was requested.
    /// Keeping the topic in the completion is essential: a fabricated topic
    /// would leave the real single-flight slot stuck forever and suppress all
    /// future attempts for that conversation.
    BackgroundSubscribeFailed(TopicId, String),

    // ── Bug reporting ──
    /// Open the pre-filled GitHub bug report in the system browser.
    ReportBug,
    /// Save a redacted support bundle through the native file picker.
    SaveSupportBundle,

    // ── Link preview ──
    /// Open a detected URL in the system default browser.
    OpenUrl(String),
    /// A link preview was fetched for the chat entry at the given index.
    LinkPreviewLoaded(usize, link_preview::LinkPreviewResult),
    /// Subscribe to all stored conversations at startup so messages can be
    /// received even before the user opens each chat.
    SubscribeStoredConversations,

    // ── Room advertisement ──
    /// Toggle whether a room appears in the public directory.
    ToggleAdvertiseRoom(TopicId),
    /// Subscribe to the directory gossip topic.
    SubscribeDirectoryTopic,
    /// The directory topic subscription completed.
    DirectorySubscribed(Option<GossipSender>),
    /// Open the public room directory (Discover screen).
    OpenDirectory,
    /// Close the Discover (public rooms) screen and return to the previous
    /// screen (the one the user came from, e.g. the File Sharing dashboard).
    CloseDiscover,
    /// Manually trigger an immediate global-registry DHT lookup and merge the
    /// discovered rooms into the local directory (PUBLIC ROOMS). The user can
    /// press the refresh button instead of waiting for the periodic ~120s tick.
    RefreshRoomRegistry,
    /// BORU-DIR-15 (PDF Task 5.3): the local Discover search box changed.
    /// The query is stored locally and NEVER broadcast onto the discovery
    /// network — filtering runs entirely against the local cache.
    DiscoverSearchChanged(String),
    /// BORU-DIR-15: toggle one of the simple Discover filters
    /// (Compatible / Not Joined / Recently Seen). Local-only.
    DiscoverFilterToggled(DiscoverFilter),
    /// BORU-DIR-15: toggle a tag/category filter chip. Local-only.
    DiscoverTagToggled(String),
    /// BORU-DIR-15: change the Discover sort order. Local-only.
    DiscoverSortChanged(DiscoverSort),
    /// BORU-DIR-15: reset the Discover search query and all filters.
    DiscoverClearFilters,
    /// Open the full Groups screen.
    OpenGroups,
    /// Close the Groups screen and return to the previous screen.
    CloseGroups,
    /// Join a room from the directory.
    DirectoryRoomJoin(RoomAdvertisement),
    /// BORU-DIR-16 (PDF Task 6.1): the user pressed Join on a Discover
    /// room card. Carries the advertised room's stable topic id bytes
    /// (`room_id` — the room's gossip [`TopicId`]). The handler validates
    /// protocol compatibility from the directory cache, then dispatches
    /// the existing public-room join path (`OpenRoom`), which creates the
    /// local conversation record exactly once and subscribes through
    /// normal room-topic logic.
    DirectoryRoomJoinById([u8; 32]),
    /// BORU-DIR-20 (PDF Task 7.2): the user pressed Hide on a Discover
    /// room card. Persists the hide preference locally through
    /// [`Storage::set_room_hidden`] and re-derives the directory cache so
    /// the room stops being offered. This is a LOCAL moderation choice —
    /// nothing is ever broadcast, and the preference is never sent to the
    /// directory topic or any peer.
    DirectoryRoomHideById([u8; 32]),
    /// BORU-DIR-20 (PDF Task 7.2): restore a previously hidden room from
    /// the Settings → Hidden rooms surface. Removes the persisted hide
    /// preference and re-derives the cache so the room is offered again
    /// (the explicit reset path the PDF requires). Never broadcast.
    DirectoryRoomUnhideById([u8; 32]),
    /// BORU-DIR-20 (PDF Task 7.2): restore ALL hidden rooms from the
    /// Settings → Hidden rooms surface. Clears the persisted preference
    /// set. Never broadcast.
    DirectoryRoomUnhideAll,
    /// Delete a locally-created room advertisement from the directory.
    DeleteDirectoryRoom(TopicId),
    /// A room advertisement was received from the directory gossip topic.
    DirectoryRoomUpdate(RoomAdvertisement, PublicKey),
    /// A **verified** room withdrawal was received from the directory
    /// gossip topic (BORU-DIR-09, PDF Task 3.3): remove the matching
    /// advertisement (`topic`, `author`) immediately. The signature was
    /// already verified by the directory receiver loop before this message
    /// was sent; TTL expiry remains the safety net if a withdrawal is
    /// missed.
    DirectoryRoomWithdrawal(TopicId, PublicKey),

    // ── Terminal tab ──
    /// Event from the embedded terminal backend (feature `terminal`).
    #[cfg(feature = "terminal")]
    TerminalEvent(iced_term::Event),
    /// Open the embedded terminal tab (feature `terminal`).
    #[cfg(feature = "terminal")]
    OpenTerminal,

    // ── Pre-warm (PERF-4R-B) ──
    /// Fired every 500 ms; the app pre-warms the next screen's widget tree
    /// only when the user has been idle for 2+ seconds.
    IdleTick,
    /// Fired on any keyboard/mouse event; resets the idle timer so
    /// pre-warming pauses while the user is active.
    UserActivity,
}

/// Map semantic GUI navigation commands to the same application messages used
/// by the visible navigation controls.
fn gui_navigation_message(command: &GuiTestCommand) -> Option<AppMessage> {
    match command {
        GuiTestCommand::GoToChatList => Some(AppMessage::GoToChatList),
        GuiTestCommand::OpenFriends => Some(AppMessage::OpenFriendRequests),
        GuiTestCommand::OpenSettings => Some(AppMessage::OpenSettings),
        GuiTestCommand::OpenFileSharing => Some(AppMessage::OpenFileSharing),
        _ => None,
    }
}

/// Map the semantic dashboard-tab test command to the concrete dashboard tab
/// the File Sharing screen renders.
fn dashboard_tab_from_name(
    name: boru_core::diagnostics::DashboardTabName,
) -> crate::dashboard_view_model::DashboardTab {
    use crate::dashboard_view_model::DashboardTab;
    match name {
        boru_core::diagnostics::DashboardTabName::FilesSharing => DashboardTab::SharedByMe,
        boru_core::diagnostics::DashboardTabName::Downloading => DashboardTab::Downloading,
        boru_core::diagnostics::DashboardTabName::Downloaded => DashboardTab::Downloaded,
        boru_core::diagnostics::DashboardTabName::SharedWithMe => DashboardTab::SharedWithMe,
        boru_core::diagnostics::DashboardTabName::Activity => DashboardTab::ActivityLog,
    }
}


/// Map the semantic dark-mode test command to the same application message
/// emitted by the visible settings toggle.
fn gui_dark_mode_message(command: &GuiTestCommand) -> Option<AppMessage> {
    match command {
        GuiTestCommand::ToggleDarkMode { enabled } => Some(AppMessage::ToggleDark(*enabled)),
        _ => None,
    }
}

/// Map the semantic help-overlay test command to the same application message
/// emitted by the visible help button in the chat header and help overlay.
fn gui_help_message(command: &GuiTestCommand) -> Option<AppMessage> {
    match command {
        GuiTestCommand::ToggleHelp => Some(AppMessage::ToggleHelp),
        _ => None,
    }
}

/// Run a GUI action task alongside its one-shot timeout. The timeout message
/// is harmless after completion because the handler checks the action state.
fn with_gui_action_timeout(
    action_id: GuiActionId,
    action: iced::Task<AppMessage>,
) -> iced::Task<AppMessage> {
    let timeout = iced::Task::perform(
        async move {
            tokio::time::sleep(Duration::from_millis(
                DEFAULT_ACTION_STATE_TIMEOUT_MS as u64,
            ))
            .await;
            action_id
        },
        AppMessage::GuiActionTimeout,
    );
    iced::Task::batch(vec![action, timeout])
}

// ── Performance metrics ──────────────────────────────────────────────

/// Tracks rendering performance of the chat log for regression detection.
///
/// Public fields are written by `view_chat_log` during `view()` and can be
/// inspected by performance regression tests without a display server.
#[derive(Debug, Clone, Default)]
pub struct PerfMetrics {
    /// Wall-clock time (ns) the last call to `view_chat_log` spent building
    /// the Iced widget tree.  Does **not** include GPU compositing time.
    pub last_render_time_ns: u64,
    /// Number of chat entries that were in scope (visible window) during
    /// the last render.
    pub window_size: usize,
    /// Total entries in the chat log at the time of the last render.
    pub total_entries: usize,
    /// Summed bytes of all `image_bytes` fields across all entries.
    pub total_image_bytes: usize,
    /// Number of entries that carry decoded image data.
    pub image_entry_count: usize,
}

impl PerfMetrics {
    #[expect(dead_code)]
    fn snapshot(&self) -> PerfSnapshot {
        PerfSnapshot {
            render_time_ns: self.last_render_time_ns,
            window_size: self.window_size,
            total_entries: self.total_entries,
            total_image_bytes: self.total_image_bytes,
            image_entry_count: self.image_entry_count,
        }
    }
}

/// Immutable snapshot at a point in time — used as test assertion target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerfSnapshot {
    pub render_time_ns: u64,
    pub window_size: usize,
    pub total_entries: usize,
    pub total_image_bytes: usize,
    pub image_entry_count: usize,
}

// ── Incremental layout cache ────────────────────────────────────────────
///
/// Maintains per-entry estimated heights and cumulative offsets so that
/// `view_chat_log` can compute the visible window in O(log n) time without
/// scanning the full entry list on every render.
///
/// Invariants:
/// - `heights.len() == cum.len()`  (may be empty)
/// - `cum[i]` = sum of `heights[0..i]`  (prefix sum, cum[0] = 0)
/// - `total_height` = sum of all heights (cached separately because cum
///   stores only prefix sums of length total, with the final total omitted).
/// - When `dirty_from` is `None`, the cache fully matches `entries`.
/// - When `dirty_from` is `Some(i)`, entries index `i..` need recomputation.


pub struct LayoutCache {
    heights: Vec<f32>,
    /// Prefix-sum: cum[i] = sum(heights[0..i]), same length as heights.
    cum: Vec<f32>,
    /// Sum of all heights; updated on append / rebuild.
    total_height: f32,
    /// First index whose height is stale, or `None` if fully valid.
    dirty_from: Option<usize>,
    /// The text-size value with which heights were last computed.
    cached_text_size: f32,
    /// Timeline width used for responsive video media-frame estimates.
    cached_timeline_width: f32,
    /// Summed image bytes across all entries (maintained incrementally).
    total_image_bytes: usize,
    /// Count of entries that carry image data.
    image_entry_count: usize,
}

impl std::fmt::Debug for LayoutCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutCache")
            .field("heights_len", &self.heights.len())
            .field("dirty_from", &self.dirty_from)
            .field("total_height", &self.total_height)
            .finish()
    }
}

impl LayoutCache {
    const DATE_SEP_H: f32 = 32.0;
    const SYSTEM_H: f32 = 24.0;
    const MSG_BASE_H: f32 = 76.0;
    /// Height of the image card header row (icon + label) above the preview.
    const IMAGE_HEADER_H: f32 = 26.0;
    const REACTION_EXTRA: f32 = 22.0;

    fn new(text_size: f32) -> Self {
        Self {
            heights: Vec::new(),
            cum: Vec::new(),
            total_height: 0.0,
            dirty_from: None,
            cached_text_size: text_size,
            cached_timeline_width: 0.0,
            total_image_bytes: 0,
            image_entry_count: 0,
        }
    }

    /// Compute the estimated pixel height for a single entry.
    fn compute_height(
        entry: &ChatEntry,
        prev_day: Option<i64>,
        _text_size: f32,
        timeline_width: f32,
    ) -> f32 {
        let mut h = 0.0;
        let day = entry.timestamp.map(|ts| ts / 86400000);
        if let Some(d) = day {
            if prev_day != Some(d) {
                h += Self::DATE_SEP_H;
            }
        }
        match entry.kind {
            ChatKind::System => {
                h += Self::SYSTEM_H;
                if let Some(download) = &entry.download {
                    h += download.estimated_height(timeline_width);
                }
            }
            _ => {
                h += Self::MSG_BASE_H;
                if entry.image_handle.is_some()
                    || entry.image_identifier.is_some()
                    || entry.image_error.is_some()
                {
                    // The entry renders an image card header plus a framed
                    // preview whose box is computed from the real pixel
                    // dimensions (or the max box fallback while decoding).
                    // Use the same box the view renders so total_height
                    // matches the real content height — a flat constant
                    // (old IMAGE_EXTRA=304) drifted from the actual box and
                    // made the scrollbar jump as images entered the window.
                    let (_, display_h) = chat_image_display_size(entry);
                    h += Self::IMAGE_HEADER_H + display_h + 2.0; // +2 = 1px border each side
                }
                if !entry.reactions.is_empty() {
                    h += Self::REACTION_EXTRA;
                }
            }
        }
        h
    }

    /// Append one entry to the cache (O(1)).
    fn append(&mut self, entry: &ChatEntry, prev_day: Option<i64>, text_size: f32) {
        let h = Self::compute_height(entry, prev_day, text_size, self.cached_timeline_width);
        self.heights.push(h);
        self.cum.push(self.total_height);
        self.total_height += h;
        if let Some(ref img) = entry.image_bytes {
            self.total_image_bytes += img.len();
            self.image_entry_count += 1;
        }
        self.dirty_from = None; // append at end doesn't break tail validity
    }

    /// Remove entry at `idx` and mark subsequent entries dirty.
    #[expect(dead_code)]
    fn remove(&mut self, idx: usize, entry: &ChatEntry) {
        if idx >= self.heights.len() {
            return;
        }
        self.heights.remove(idx);
        self.cum.pop(); // remove the final prefix sentinel
                        // Keep the cache internally consistent while the suffix is rebuilt. In
                        // particular, removing the last entry makes `dirty_from == len`, so a
                        // later incremental build must not index past `cum`.
        self.total_height = self.heights.iter().sum();
        if let Some(ref img) = entry.image_bytes {
            self.total_image_bytes = self.total_image_bytes.saturating_sub(img.len());
            self.image_entry_count = self.image_entry_count.saturating_sub(1);
        }
        self.dirty_from = Some(idx.min(self.heights.len()));
    }

    /// Clear the entire cache (O(1)).
    fn clear(&mut self) {
        self.heights.clear();
        self.cum.clear();
        self.total_height = 0.0;
        self.dirty_from = None;
        self.total_image_bytes = 0;
        self.image_entry_count = 0;
    }

    /// Mark the entire cache as needing rebuild (text-size change, etc.).
    fn invalidate_all(&mut self) {
        self.dirty_from = Some(0);
    }

    /// Mark entries from `idx` onward as stale.
    fn invalidate_from(&mut self, idx: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(idx, |current| current.min(idx)));
    }

    /// Rebuild the cache from a given index onward.
    fn build(&mut self, entries: &[ChatEntry], text_size: f32, from: usize, timeline_width: f32) {
        let total = entries.len();
        let from = from.min(total);

        // Nothing to rebuild when from == total (entry removed past end).
        if from >= total {
            self.dirty_from = None;
            return;
        }

        // Shrink vectors if entries shrunk, or grow as needed
        if self.heights.len() > total {
            self.heights.truncate(total);
            self.cum.truncate(total);
        }

        let mut prev_day: Option<i64> = if from > 0 {
            entries[from.saturating_sub(1)]
                .timestamp
                .map(|ts| ts / 86400000)
        } else {
            None
        };

        // Recompute image metrics from scratch (rare — only on invalidations)
        if from == 0 {
            self.total_image_bytes = 0;
            self.image_entry_count = 0;
            for e in entries {
                if let Some(ref img) = e.image_bytes {
                    self.total_image_bytes += img.len();
                    self.image_entry_count += 1;
                }
            }
        }

        let mut running = if from > 0 && from < self.cum.len() {
            self.cum[from] // prefix sum up to `from` is valid
        } else if from > 0 {
            // from == cum.len() means the last entry was removed;
            // recompute from the last known prefix sum.
            self.cum.last().copied().unwrap_or(0.0)
        } else {
            0.0
        };

        for (offset, e) in entries[from..total].iter().enumerate() {
            let i = from + offset;
            let day = e.timestamp.map(|ts| ts / 86400000);
            let h = Self::compute_height(e, prev_day, text_size, timeline_width);

            if i < self.heights.len() {
                self.heights[i] = h;
                self.cum[i] = running;
            } else {
                self.heights.push(h);
                self.cum.push(running);
            }
            running += h;
            if day.is_some() {
                prev_day = day;
            }
        }

        self.total_height = running;
        self.dirty_from = None;
        self.cached_text_size = text_size;
        self.cached_timeline_width = timeline_width;
    }

    /// Ensure the cache is fully valid. Rebuilds from the dirty point if needed.
    fn ensure(&mut self, entries: &[ChatEntry], text_size: f32, timeline_width: f32) {
        let needs_full = self.dirty_from == Some(0)
            || self.cached_text_size != text_size
            || self.cached_timeline_width != timeline_width
            || self.heights.len() != entries.len();

        if needs_full {
            self.build(entries, text_size, 0, timeline_width);
        } else if let Some(from) = self.dirty_from {
            self.build(entries, text_size, from, timeline_width);
        }
    }

    /// Compute the visible-window parameters using binary search on cum.
    /// Returns (first_idx, last_idx, top_spacer_height, bottom_spacer_height).
    fn window(&self, scroll_offset: f32, viewport_height: f32) -> (usize, usize, f32, f32) {
        const OVERSCAN: f32 = 800.0;

        let total = self.heights.len();
        if total == 0 || self.total_height <= 0.0 {
            return (0, 0, 0.0, 0.0);
        }

        let so = if scroll_offset >= f32::MAX / 2.0 {
            (self.total_height - viewport_height.max(200.0)).max(0.0)
        } else {
            scroll_offset
        };
        let view_top = so.max(0.0);
        let view_bot = view_top + viewport_height.max(200.0);

        let range_top = (view_top - OVERSCAN).max(0.0);
        let range_bot = (view_bot + OVERSCAN).min(self.total_height);

        let first_idx = self
            .cum
            .partition_point(|&c| c < range_top)
            .min(total.saturating_sub(1));
        let last_idx = self
            .cum
            .partition_point(|&c| c <= range_bot)
            .saturating_sub(1)
            .min(total.saturating_sub(1))
            .max(first_idx);

        let top_space_h = self.cum[first_idx];
        let bottom_start = self.cum[last_idx] + self.heights[last_idx];
        let bottom_h = (self.total_height - bottom_start).max(0.0);

        (first_idx, last_idx, top_space_h, bottom_h)
    }
}

/// A small card-like container with a muted title, a thin rule, and
/// content children — used in settings and friend-request screens.
fn section_card<'a>(
    title: &'a str,
    children: Vec<iced::Element<'a, AppMessage>>,
) -> iced::Element<'a, AppMessage> {
    use iced::widget::{container, rule, Column, Space};
    use iced::Length;
    let body = Column::new()
        .push(
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, title.to_string())
                .style(text_muted_style),
        )
        .push(rule::horizontal(1).style(iced::widget::rule::weak))
        .push(Space::new().height(Length::Fixed(SPACE_8)));
    let body = children
        .into_iter()
        .fold(body, |col, child| col.push(child));
    container(body)
        .padding([SPACE_12, SPACE_16])
        .width(Length::Fill)
        .style(container_card)
        .into()
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SidebarIdentityCacheKey {
    local_label: String,
    presence: PeerPresence,
    dark_mode: bool,
    has_profile_image: bool,
}


/// Renders the local-user profile block in the sidebar: avatar (profile image
/// or generated initials circle), display name, online/away/offline status, and a settings gear button.
///
/// SIDEBAR-03 / BORU-HOME-09: the top-left profile header avatar renders at
/// PROFILE_HEADER_AVATAR_SIZE: use AVATAR_PROFILE (72 px) — the largest
/// sidebar avatar, distinguishes the local-user identity block from
/// list-row avatars (AVATAR_CHAT_LIST = 56 px).
const PROFILE_HEADER_AVATAR_SIZE: f32 = crate::design_tokens::AVATAR_PROFILE;






/// UI-30: generate a uniform thumbnail handle for one local shared file.
///
/// Runs off the UI thread (spawned by `IcedChat::kick_shared_by_me_thumbnails`).
/// Pictures are downsampled with `boru_core::image_optimizer::thumbnail_image`;
/// videos produce a bounded poster frame via `boru_core::video_poster::generate`
/// (ffmpeg-backed, cached under the video-posters cache dir). Returns `None`
/// when the file is missing, unreadable, or not decodable so the table row can
/// fall back to its type icon.
async fn generate_shared_by_me_thumbnail(
    storage: &Storage,
    content_hash: &str,
    is_video: bool,
    cache_dir: &std::path::Path,
) -> Option<iced::widget::image::Handle> {
    // SQLite read — run on the blocking pool so the Task::perform worker
    // never blocks on disk I/O (BORU-AUDIT-18).
    let object = storage
        .run_blocking("app.thumbnail.get_file_object", {
            let content_hash = content_hash.to_owned();
            move |s| {
                s.get_file_object(&content_hash)
                    .map_err(|e| anyhow::anyhow!("{e:#}"))
            }
        })
        .await
        .ok()
        .flatten()?;
    if is_video {
        let path = std::path::PathBuf::from(object.source_path.as_deref()?);
        let cache_dir = cache_dir.to_path_buf();
        // Prefer the content-hash poster cache key: it skips the second
        // full-file read inside `generate` (the cache key is blake3 of the
        // content, which is exactly this hash).
        let content_hash = content_hash.to_string();
        let poster = tokio::task::spawn_blocking(move || {
            let hash = content_hash.parse::<iroh_blobs::Hash>().ok();
            match hash {
                Some(hash) => {
                    boru_core::video_poster::generate_with_content_hash(&path, &cache_dir, &hash)
                        .ok()
                }
                None => boru_core::video_poster::generate(&path, &cache_dir).ok(),
            }
        })
        .await
        .ok()?;
        let poster = poster?;
        return Some(iced::widget::image::Handle::from_bytes(poster.bytes));
    }
    let raw = if let Some(path) = object.source_path.as_deref() {
        let path = std::path::PathBuf::from(path);
        tokio::task::spawn_blocking(move || std::fs::read(&path).ok())
            .await
            .ok()?
    } else {
        Some(object.data?)
    };
    let Some(raw) = raw else {
        return None;
    };
    let thumb = tokio::task::spawn_blocking(move || {
        boru_core::image_optimizer::thumbnail_image(
            &raw,
            boru_core::image_optimizer::THUMBNAIL_MAX_EDGE,
        )
        .ok()
    })
    .await
    .ok()?;
    let thumb = thumb?;
    Some(iced::widget::image::Handle::from_bytes(thumb))
}





impl IcedChat {
    /// Detect OS reduced-motion preference.
    ///
    /// Checks the REDUCED_MOTION env var first (for CI/testing), then queries
    /// GNOME gsettings on Linux. Returns true when reduced motion is requested.
    fn detect_reduced_motion() -> bool {
        // Honour explicit override first.
        if let Ok(val) = std::env::var("REDUCED_MOTION") {
            return val == "1" || val.eq_ignore_ascii_case("true");
        }
        // GNOME-based desktops.
        if let Ok(output) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                return s.trim() == "false";
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        secret_key: SecretKey,
        gossip: Gossip,
        router: iroh::protocol::Router,
        file_offer_registry: Arc<StdMutex<FileOfferRegistry>>,
        blob_store: FsStore,
        endpoint: iroh::Endpoint,
        memory_lookup: MemoryLookup,
        local_label: String,
        local_public: PublicKey,
        relay_mode: RelayMode,
        data_dir: std::path::PathBuf,
        runtime_handle: tokio::runtime::Handle,
        net_rx: Arc<Mutex<Receiver<ConversationNetEvent>>>,
        net_tx: Sender<ConversationNetEvent>,
        room_history: RoomHistoryStore,
        friends: FriendsStore,
        friend_mgr: FriendPingManager,
        friend_events_rx: Arc<Mutex<Receiver<FriendEvent>>>,
        whisper_events_rx: Arc<Mutex<Receiver<WhisperEvent>>>,
        inbox_events_rx: Arc<Mutex<Receiver<InboxEvent>>>,
        whisper_handle: WhisperHandle,
        call_handle: CallHandle,
        call_events_rx: Arc<Mutex<Receiver<CallEvent>>>,
        initial_room: Option<(TopicId, Vec<EndpointAddr>)>,
        chat_history: Arc<std::sync::Mutex<ChatHistoryStore>>,
        backfill_handle: BackfillHandle,
        return_to_chat_list_after_open: bool,
        discovered_peers_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<DiscoveredPeersUpdate>>>,
        reconnect_ready_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<PublicKey>>>,
        reconnect_handle: Option<boru_core::control_plane::reconnect::ReconnectHandle>,
        directory_room_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<DirectoryRoomEvent>>>,
        dht: Option<distributed_topic_tracker::Dht>,
        private_dht_disabled: bool,
        iced_diagnostics: IcedMessageJournal,
        gui_action_rx: Option<Arc<Mutex<tokio::sync::mpsc::Receiver<GuiActionRequest>>>>,
        gui_state_tx: tokio::sync::watch::Sender<IcedStateSnapshot>,
        gui_action_history: GuiActionHistory,
        storage: Option<Storage>,
        tunnel_service: Arc<boru_core::tunnel::service::TunnelService>,
        transfer_store: Arc<TransferStateStore>,
        outbound_item_labels: Arc<StdMutex<HashMap<String, String>>>,
        inbound_item_labels: Arc<StdMutex<HashMap<String, String>>>,
    ) -> Self {
        let (initial_topic, initial_bootstrap) =
            initial_room.unwrap_or_else(|| (TopicId::from_bytes([0u8; 32]), vec![]));
        // Seed the presence map from persisted friends who were online at last
        // save, so they show the correct status immediately instead of starting
        // as offline. Timestamps default to "now".
        let peer_presence_map: HashMap<PublicKey, u64> = friends
            .iter()
            .filter(|(_, record)| record.status.online)
            .filter_map(|(id, _)| id.parse_public_key().ok())
            .map(|pk| (pk, now_ms().max(0) as u64))
            .collect();
        // PUBLIC-03: seed the persisted seen-peers set with every existing
        // friend, so a known contact is never announced as a "new user"
        // after upgrade or restart. Only genuinely unseen peers (mDNS
        // discovers / gossip neighbors / new friends) produce the entry.
        let seen_peers = load_seen_peers(&data_dir, &friends);
        // Initialise per-user image storage. The files root defaults to
        // `<data_dir>/files` but can be overridden via
        // `BORU_CHAT_FILES_DIR` for testing or alternate layouts.
        let image_store = match std::env::var("BORU_CHAT_FILES_DIR") {
            Ok(path) => ImageStore::from_files_dir(path),
            Err(_) => ImageStore::at(&data_dir),
        };
        // Load saved profile image from the per-user image store (or legacy
        // direct file) and regenerate the blob ticket so AboutMe broadcasts
        // include the ticket for peers to download.  Without this, a restart
        // loses the ticket (blob store is in-memory) and peers see the
        // fallback emoji instead of the avatar.
        let profile_image_id_file = data_dir.join(".profile-image-id");
        let (profile_image_handle, profile_image_ticket, profile_image_identifier) =
            if let Ok(identifier) = std::fs::read_to_string(&profile_image_id_file) {
                let identifier = identifier.trim().to_string();
                match image_store.resolve_absolute_path(&local_public.to_string(), &identifier) {
                    Ok(abs_path) => match fs::read(&abs_path) {
                        Ok(bytes)
                            if !bytes.is_empty() && bytes.len() <= PROFILE_IMAGE_MAX_BYTES =>
                        {
                            let handle =
                                Some(iced::widget::image::Handle::from_bytes(bytes.clone()));
                            let ticket = {
                                let bs = blob_store.clone();
                                let ep = endpoint.clone();
                                runtime_handle.block_on(async {
                                    bs.blobs().add_bytes(bytes).await.ok().map(|tag| {
                                        blob_ticket_string(
                                            ep.watch_addr().get(),
                                            tag.hash,
                                            tag.format,
                                        )
                                    })
                                })
                            };
                            (handle, ticket, Some(identifier))
                        }
                        _ => (None, None, None),
                    },
                    Err(_) => (None, None, None),
                }
            } else if let Ok(bytes) = fs::read(data_dir.join(PROFILE_IMAGE_FILE)) {
                if !bytes.is_empty() && bytes.len() <= PROFILE_IMAGE_MAX_BYTES {
                    let handle = Some(iced::widget::image::Handle::from_bytes(bytes.clone()));
                    let ticket = {
                        let bs = blob_store.clone();
                        let ep = endpoint.clone();
                        runtime_handle.block_on(async {
                            bs.blobs().add_bytes(bytes).await.ok().map(|tag| {
                                blob_ticket_string(ep.watch_addr().get(), tag.hash, tag.format)
                            })
                        })
                    };
                    (handle, ticket, None)
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            };
        // Create the download manager from the passed-in storage.
        // If storage is None (e.g. first run or permissions issue), both remain
        // None and downloads work as before via ad-hoc Storage::open calls.
        let download_manager = storage.as_ref().map(|stg| {
            let config = DownloadLimitsConfig::from_env().unwrap_or_default();
            Arc::new(std::sync::Mutex::new(DownloadManager::with_limits(
                stg.clone(),
                config,
            )))
        });
        // Run startup recovery synchronously at init (before the GUI
        // frame loop starts).  The scheduler limits the burst to
        // max_startup_downloads; remaining items wait in the pending queue.
        if let Some(dm) = &download_manager {
            match runtime_handle.block_on(dm.lock().unwrap().recover_from_restart()) {
                Ok(()) => tracing::info!(
                    "download-manager: startup recovery complete (bounded burst started)"
                ),
                Err(e) => tracing::warn!("download-manager: startup recovery failed: {e}"),
            }
        }
        let first_run = room_history.is_empty() && friends.is_empty();
        let app_settings = AppSettings::load(&data_dir);
        // ICEDAW-01: restore any persisted custom accent color so
        // `accent_primary` applies it app-wide from the first frame.
        set_accent_override(app_settings.accent_color);
        // Restore the persisted display name.  The CLI --name flag always
        // takes priority; when no --name was given the constructor receives
        // the public-key short form (which looks like "90af827f0d").  If a
        // persisted name exists we prefer it over that auto-generated default.
        let local_label = if local_label != local_public.fmt_short().to_string() {
            local_label // user explicitly passed --name, use it
        } else if let Some(ref saved) = app_settings.display_name {
            saved.clone() // use persisted name
        } else {
            local_label // no --name and no persisted, use default
        };
        // Derive a stable mailbox encryption key from the node identity key.
        // This allows friends to encrypt offline messages to us.
        let local_mailbox_key = Some(MailboxIdentity::from_secret(&secret_key).public_key());
        // Load shared files from storage for the settings GUI.
        let shared_files = storage
            .as_ref()
            .and_then(|stg| stg.list_shared_files(&local_public.to_string(), true).ok())
            .unwrap_or_default();
        let mut room_authorization = HashMap::new();
        if let Some(stg) = storage.as_ref() {
            if let Ok(Some((state, _events))) = stg.load_room_authorization(&initial_topic) {
                room_authorization.insert(initial_topic, state);
            }
        }
        // Hydrate friend profile images from stored tickets on startup.
        // Friend images are downloaded as blobs and cached locally, but
        // the in-memory handles are lost on restart.  Re-queue downloads
        // so images appear even when the friend is currently offline.
        let pending_friend_tickets: std::collections::VecDeque<(PublicKey, String)> = friends
            .iter()
            .filter_map(|(fid, record)| {
                let ticket = record.last_announced_profile_image_ticket.as_ref()?;
                if ticket.is_empty() {
                    return None;
                }
                let pk = fid.parse_public_key().ok()?;
                Some((pk, ticket.clone()))
            })
            .collect::<Vec<_>>()
            .into();

        let directory_store = Arc::new(StdMutex::new(DirectoryStore::new()));
        if let Some(storage) = storage.as_ref() {
            if let Err(err) = storage.with_conn(|conn| {
                directory_store
                    .lock()
                    .map_err(|_| anyhow::anyhow!("directory store mutex poisoned"))?
                    .load_from_db(conn)?;
                Ok(())
            }) {
                warn!("failed to load directory advertisements: {err}");
            }
        }

        let cached_friend_count = friends
            .iter()
            .filter(|(_, r)| r.relationship.can_message())
            .count();

        let mut conversation_store = {
            let mut store = if let Some(ref st) = storage {
                ConversationStore::load_from_sqlite(st, &data_dir)
            } else {
                ConversationStore::load_or_default(&data_dir)
            };
            if store.is_empty() {
                let json_store = ConversationStore::load_or_default(&data_dir);
                if !json_store.is_empty() {
                    if let Some(ref st) = storage {
                        let _ = json_store.save_to_sqlite(st);
                    }
                    json_store
                } else {
                    store
                }
            } else {
                store
            }
        };
        // BORU-DIR-04 (PDF 2.1): conservative visibility migration. Rooms
        // that this node advertised into the directory under the legacy
        // model (local-authored rows in the persisted directory store) are
        // migrated to PublicUnlisted — shareable but not browsable — so no
        // existing public room is unexpectedly exposed by the new directory.
        // Entries that already carry an explicit visibility are untouched.
        {
            let legacy_public_topics: std::collections::HashSet<TopicId> = {
                let store = directory_store.lock().unwrap();
                store
                    .list_active()
                    .into_iter()
                    .filter(|(_, author)| *author == local_public)
                    .map(|(ad, _)| ad.topic)
                    .collect()
            };
            let migrated = conversation_store.migrate_legacy_public_rooms(&legacy_public_topics);
            if migrated > 0 {
                info!(
                    migrated,
                    "BORU-DIR-04: migrated legacy public rooms to PublicUnlisted"
                );
                if let Some(ref st) = storage {
                    let _ = conversation_store.save_to_sqlite(st);
                }
            }
        }
        // BORU-DISC-18: never surface a stale saved lobby conversation in the
        // room list. Remove it from the in-memory store and persist the
        // removal so an install upgraded from an old version that auto-joined
        // the lobby starts clean. Only the exact canonical lobby topic is
        // matched; unrelated public rooms are untouched. (main.rs already runs
        // the persisted-state migration; this guard covers the load path
        // itself and is idempotent.)
        let lobby_pruned =
            boru_core::lobby_migration::prune_conversation_store(&mut conversation_store);
        if lobby_pruned > 0 {
            tracing::info!(
                removed = lobby_pruned,
                "lobby migration: pruned stale saved lobby from room list"
            );
            if let Some(ref st) = storage {
                let _ = conversation_store.save_to_sqlite(st);
            }
        }

        // The outbound/inbound panel seeding that used to live here now runs
        // inside FilesState::new (the FS-05 projection snapshot seeding),
        // keeping the file-transfer state construction with the state it
        // owns (BORU-APP-005).
        let boru_downloads_dir = {
            let dl = data_dir.join("downloads");
            let _ = std::fs::create_dir_all(&dl);
            dl
        };
        let file_indexer = FileIndexer::new(boru_core::file_indexer::default_shared_folder_path());
        let files_state = FilesState::new(
            transfer_store,
            outbound_item_labels,
            inbound_item_labels,
            shared_files,
            boru_downloads_dir,
            file_indexer,
        );
        Self {
            screen: Screen::ChatList,
            #[cfg(feature = "terminal")]
            terminal: TerminalTab::new().ok(),
            splash_spinner_frame: 0,
            connecting_spinner_frame: 0,
            main_screen_reconnect_frame: 0,
            reduced_motion: Self::detect_reduced_motion(),
            lightbox_image: None,
            lightbox_close_snap_guard: 0,
            pending_topic: None,
            room_loading: false,
            room_generation: 0,
            conversation_generation: 0,
            room_history,
            room_history_dirty: false,
            join_ticket_input: String::new(),
            chat_list_error: String::new(),
            conversations: HashMap::new(),
            pinned_state: PinState::default(),
            entries: Vec::new(),
            composer_text: String::new(),
            composer_sending: false,
            composer_drag_over: false,
            composer_ime_active: false,
            typing_peers: TypingState::default(),
            typing_emitter: TypingEmitter::default(),
            typing_privacy_enabled: app_settings.typing_indicators_enabled,
            help_overlay: HelpOverlay::new(),
            show_chat_options: false,
            show_chat_search: false,
            chat_search_query: String::new(),
            show_member_list: false,
            show_emoji_picker: false,
            emoji_category: crate::emoji::EmojiCategory::SmileysAndPeople,
            emoji_search_query: String::new(),
            // BORU-TWEMOJI-14: restore the persisted recently-used list,
            // sanitized so corrupt/unknown stored entries cannot break the
            // picker (empty/whitespace entries are dropped).
            recent_emojis: crate::emoji::recents::sanitize_recents(&app_settings.recent_emojis),
            show_gif_picker: false,
            gif_search_text: String::new(),
            gif_results: Vec::new(),
            gif_preview_cache: HashMap::new(),
            gif_loading: false,
            gif_showing_trending: false,
            gif_has_searched: false,
            gif_error: None,
            gif_next_cursor: None,
            gif_appending: false,
            gif_append_error: None,
            gif_request_seq: 0,
            gif_debounce_seq: 0,
            gif_spinner_frame: 0,
            gif_not_configured: false,
            show_invite_member_dialog: false,
            invite_member_selected: HashSet::new(),
            details_panel_open: false,
            pending_file: None,
            pending_offline_ids: HashMap::new(),
            pending_image: VecDeque::new(),
            pending_gif: VecDeque::new(),
            pending_thumbnail_fetch: VecDeque::new(),
            pending_image_upload: None,
            image_upload_spinner_frame: 0,
            pending_file_upload: None,
            file_upload_spinner_frame: 0,
            download_entry_index: None,
            active_download_transfer_id: None,
            show_receive_ticket_dialog: false,
            receive_ticket_input: String::new(),
            receive_ticket_preflight: None,
            receive_ticket_error: None,
            receive_ticket_preflight_busy: false,
            receive_ticket_downloading: false,
            #[cfg(feature = "video-playback")]
            inline_video: None,
            #[cfg(feature = "video-playback")]
            playback_coordinator: PlaybackCoordinator::new(),
            #[cfg(feature = "video-playback")]
            inline_video_seek: None,
            #[cfg(feature = "video-playback")]
            inline_video_expanded: false,
            #[cfg(feature = "video-playback")]
            inline_video_resume: None,
            #[cfg(feature = "video-playback")]
            video_runtime: {
                let capability = VideoRuntimeCapability::detect();
                if capability.available {
                    tracing::info!(detail = %capability.detail, "inline video runtime detected");
                } else {
                    tracing::warn!(detail = %capability.detail, "inline video runtime unavailable; retaining download and external-open actions");
                }
                capability
            },
            external_stream_server: Arc::new(StdMutex::new(None)),
            transfer_id_to_index: HashMap::new(),
            names: HashMap::new(),
            topic: initial_topic,
            ticket_str: String::new(),
            secret_key,
            gossip,
            _router: router,
            file_offer_registry,
            sender: None,
            sender_ready: false,
            blob_store,
            endpoint,
            memory_lookup,
            local_label,
            local_public,
            relay_mode: relay_mode.clone(),
            runtime_handle,
            net_rx,
            net_tx,
            backfill_handle,
            forward_handle: None,
            forward_handle_slot: Arc::new(StdMutex::new(None)),
            friends,
            friends_dirty: false,
            friend_mgr,
            friend_events_rx,
            neighbors: HashSet::new(),
            known_peers: HashSet::new(),
            room_neighbor_counts: HashMap::new(),
            direct_peers: 0,
            relayed_peers: 0,
            conn_refresh_counter: 0,
            mesh_health: MeshHealth::Good,
            last_mesh_health: None,
            mesh_connected_at: None,
            conversation_subscription_pending: true,
            mesh_event_log: {
                let mut log = std::collections::VecDeque::new();
                log.push_back(MeshEvent {
                    message: "Starting up...".to_string(),
                    recorded_at: Instant::now(),
                });
                log
            },
            presence_counter: 5,
            heartbeat_counter: 2,
            latency_ping_counter: 15,
            peer_latencies: HashMap::new(),
            conn_refresh_in_flight: false,
            needs_conn_refresh: false,
            ticket_extra_peers: Vec::new(),
            pending_ticket_peers: Vec::new(),
            ticket_needs_regeneration: false,
            ticket_resolve_in_flight: false,
            background_subscriptions_in_flight: std::collections::BTreeSet::new(),
            self_sent_events: HashMap::new(),
            event_id_to_index: HashMap::new(),
            message_hash_to_index: HashMap::new(),

            follow_latest: true,
            total_content_height: std::cell::Cell::new(0.0),
            scroll_offset: f32::MAX,
            viewport_height: 0.0,
            scroll_to_bottom_pending: false,
            settings_return_to: None,
            friend_requests_return_to: None,
            peer_profile_return_to: None,
            friend_profile_return_to: None,
            discover_return_to: None,
            discover_search_query: String::new(),
            discover_filter_compatible: false,
            discover_filter_not_joined: false,
            discover_filter_recently_seen: false,
            discover_selected_tags: Vec::new(),
            discover_sort: DiscoverSort::RecentlySeen,
            groups_return_to: None,
            download_manager_return_to: None,
            prewarm_cache: std::collections::HashMap::new(),
            prewarming: false,
            idle_timer: IdleTimer::new(),
            prewarm_window_mode: None,
            prewarm_invalidate_pending: false,
            dark_mode: app_settings.dark_mode,
            connectivity_store: None,
            network_map_source: None,
            capability_gate: None,
            room_directory: None,
            room_delete_confirm_topic: None,
            data_dir: data_dir.clone(),
            image_store,
            chat_history,
            storage,
            room_authorization,
            download_manager,
            history_saved_count: 0,
            history_confirm_clear: false,
            history_clear_pending: false,
            history_clear_feedback: None,
            history_clear_feedback_is_error: false,
            peer_presence_map,
            presence_away_peers: HashSet::new(),
            seen_peers,
            friends_sidebar_revision: 1,
            chats_sidebar_revision: 0,
            discovered_sidebar_revision: 0,
            requests_sidebar_revision: 0,
            cached_chat_count: 0, // Recalculated on first ConnMonitorTick
            cached_group_count: 0,
            cached_friend_count, // Pre-computed from loaded friends
            cached_discover_count: 0,
            cached_public_room_count: 0,
            cached_request_count: 0,
            cached_chats_revision: Cell::new(0),
            cached_chats_dep: std::cell::RefCell::new(None),
            cached_discovered_revision: Cell::new(0),
            cached_discovered_dep: std::cell::RefCell::new(None),
            cached_friends_rows_revision: Cell::new(0),
            cached_friends_rows_dep: std::cell::RefCell::new(None),
            cached_requests_revision: Cell::new(0),
            cached_requests_dep: std::cell::RefCell::new(None),
            sidebar_selected_topic: Rc::new(Cell::new(None)),
            sidebar_section_collapsed: [false; 6],
            sidebar_fade_frame: [crate::ui_components::SIDEBAR_FADE_FRAMES; 6],
            initial_bootstrap_peers: initial_bootstrap,
            return_to_chat_list_after_open,
            whisper_handle,
            call_handle,
            call_events_rx,
            call_return_screen: None,
            calls_state: CallsState::new(),
            inbox_events_rx,
            whisper_events_rx,
            home_background_path: app_settings.home_background_image.clone(),
            home_background_handle: load_home_background_handle(
                app_settings.home_background_image.as_deref(),
            ),
            home_menu_item_opacity: app_settings.home_menu_item_opacity,
            local_mailbox_key,
            friend_image_handles: load_cached_friend_profile_images(&data_dir),
            friend_image_tickets: HashMap::new(),
            pending_profile_image_tickets: pending_friend_tickets,
            friend_profile_versions: HashMap::new(),
            last_failed_profile_retry: HashMap::new(),
            perf: std::cell::RefCell::new(PerfMetrics::default()),
            first_run,
            layout_cache: std::cell::RefCell::new(LayoutCache::new(app_settings.chat_text_size)),
            friend_request_store: FriendRequestStore::load_or_default(&data_dir),
            outgoing_request_states: HashMap::new(),
            join_request_list: Vec::new(),
            friend_request_search_input: String::new(),
            friend_request_error: String::new(),
            public_room_safety: None,
            conversation_store,
            discovered_peers: Vec::new(),
            discovered_online_cache: HashSet::new(),
            discovered_peers_rx,
            reconnect_ready_rx,
            reconnect_handle,
            pending_neighbor_status: HashMap::new(),
            pending_backfill_topics: Vec::new(),
            friend_id_copied: false,
            show_invite_menu: false,
            invite_whisper_input: String::new(),
            show_create_group_dialog: false,
            create_group_submitting: false,
            create_group_error: None,
            create_group_name: String::new(),
            create_group_description: String::new(),
            create_group_selected_members: HashSet::new(),
            create_group_search: String::new(),
            tunnels_state: TunnelsState::new(),
            dht,
            private_dht_disabled,
            rooms_state: RoomsState::new(),
            connection_details_dialog: None,
            connection_details_announcement: None,
            connection_details_focus_target: None,

            friend_profile_menu_open: false,
            friend_profile_rename_input: String::new(),
            friend_profile_renaming: false,
            friend_remove_confirm: false,
            friend_block_confirm: false,
            context_menu: None,
            video_card_menu_open: None,
            files_state,

            profile_cache: HashMap::new(),
            profile_store: UserProfileStore::empty_at(&data_dir, local_public),
            settings_state: settings::SettingsState::new(
                &app_settings,
                profile_image_handle,
                profile_image_ticket,
                profile_image_identifier,
            ),
            iced_diagnostics,
            gui_action_rx,
            gui_action_history,
            pending_open_room_action: None,
            pending_open_conversation_action: None,
            pending_set_composer_action: None,
            pending_submit_composer_action: None,
            pending_chat_list_action: None,
            pending_open_friends_action: None,
            pending_open_settings_action: None,
            pending_open_file_sharing_action: None,
            pending_dashboard_tab_action: None,
            pending_share_file_action: None,
            pending_close_dialog_action: None,
            pending_toggle_help_action: None,
            pending_select_peer_action: None,
            pending_create_room_action: None,
            pending_confirm_create_room_action: None,
            pending_download_action: None,
            gui_state_tx,
            gui_state_enabled: true,
            last_snapshot: None,
            last_snapshot_at: std::time::Instant::now(),
            gui_snapshot_throttle_ms: 0, // no throttle by default; main.rs sets 125ms
            gui_snapshot_pending: false,
            // BORU-UI-04: dev theme overrides; main.rs sets the loaded value.
            ui_theme_config: crate::theme_config::UiThemeConfig::default(),
            // BORU-UI-07: the live merged theme. Starts as the default theme
            // for the current mode; main.rs replaces it via
            // `set_ui_theme_config` (which recomputes the merge) once the
            // startup config is loaded. Kept as Copy so `boru_theme()` is a
            // single-field read per frame.
            active_theme: crate::theme::BoruTheme::for_theme(&Self::theme_from_dark(
                app_settings.dark_mode,
            )),
            // BORU-LAYOUT-03: the live layout starts at the defaults, which
            // reproduce the current appearance exactly; a later BORU-LAYOUT
            // task replaces it via `set_layout_config` once the TOML file +
            // merge layer land.
            active_layout: crate::layout::LayoutConfig::default(),
            // BORU-LAYOUT-08: the editable override set starts empty
            // (defaults); main.rs replaces it via `set_layout_overrides`
            // when `boru-layout.toml` is loaded.
            layout_overrides: crate::layout::LayoutOverrides::default(),
            // BORU-LAYOUT-03: revision bumped on every applied layout change.
            layout_revision: 0,
            // BORU-UI-07: revision bumped on every applied theme change.
            theme_revision: 0,
            // BORU-UI-06: watcher receiver + staleness tracker. main.rs
            // starts the watcher thread and sets ui_theme_rx; until then
            // (and in tests/headless) the subscription uses a closed dummy.
            ui_theme_rx: None,
            ui_theme_reload_tracker: crate::theme_watcher::ReloadTracker::new(),
            // BORU-LAYOUT-06: watcher receiver + staleness tracker. main.rs
            // sets `layout_rx` when the dev layout watcher starts; `None` in
            // headless launches and tests (the subscription falls back to a
            // closed dummy receiver, so no reload can ever reach the loop).
            layout_rx: None,
            layout_reload_tracker: crate::theme_watcher::ReloadTracker::new(),
            // ── Notification system ──
            notifications_state: NotificationsState::new(),
            window_width: 1200.0,
            window_height: 800.0,
            link_preview_cache: Arc::new(StdMutex::new(link_preview::LinkPreviewCache::new())),

            // ── Room advertisement ──
            // Advertising state lives in `rooms_state` (BORU-APP-006).
            // Derive directory topic from the relay URL — all peers on the
            // same relay share the same directory topic.
            directory_topic: Self::derive_directory_topic_from_relay(
                fmt_relay_mode(&relay_mode).as_str(),
            ),
            directory_sender: None,
            directory_store,
            directory_room_rx,
            tunnel_service,
        }
    }

    /// Return a snapshot of the last render's performance metrics.
    /// Used by performance regression tests.
    #[expect(dead_code)]
    pub fn perf_metrics(&self) -> PerfSnapshot {
        self.perf.borrow().snapshot()
    }

    fn room_ticket(&self, topic: TopicId, extra_peers: &[EndpointAddr]) -> Ticket {
        let mut peers = vec![invitation_endpoint_addr(
            self.endpoint.watch_addr().get(),
            self.settings_state.share_direct_addresses,
        )];
        // Append extra bootstrap peers (e.g. peers that joined the mesh
        // after this ticket was first created). The same direct-address
        // policy is applied so tickets never leak direct IPs unless the
        // user opted in to sharing them.
        for extra in extra_peers {
            if extra.id != self.local_public {
                peers.push(invitation_endpoint_addr(
                    extra.clone(),
                    self.settings_state.share_direct_addresses,
                ));
            }
        }
        Ticket {
            topic,
            peers,
            discovery_secret: None,
        }
    }

    /// Canonical public-room topic helper (the explicit "public-lobby" room).
    ///
    /// The GUI uses the canonical versioned public-room identity
    /// ([`boru_core::public_room::public_lobby_topic`] with
    /// [`PublicNetwork::Mainnet`]) so the gossip mesh topic and the DHT
    /// [`PublicRoomTracker`] namespace resolve to the exact same topic.  See
    /// `docs/discovery-architecture.md`.
    pub fn default_lobby_topic() -> TopicId {
        boru_core::public_room::public_lobby_topic(boru_core::public_room::PublicNetwork::Mainnet)
    }

    /// Stable personal room advertised by this identity.
    fn personal_room_topic(&self) -> TopicId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"boru-chat/public-room-directory/v1");
        hasher.update(self.local_public.as_bytes());
        TopicId::from_bytes(*hasher.finalize().as_bytes())
    }

    /// Derive a stable directory topic from the relay URL.
    ///
    /// All peers on the same relay server derive the same topic ID,
    /// enabling discovery of advertised public rooms without an
    /// out-of-band rendezvous point.
    pub fn derive_directory_topic_from_relay(relay_url: &str) -> TopicId {
        // Must match the domain separator used by boru_core::directory::directory_topic()
        // so that IcedChat, the main.rs directory receiver, and the MCP broadcast
        // all share the same gossip mesh.
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"boru-chat/public-room-directory/v1");
        hasher.update(relay_url.as_bytes());
        TopicId::from_bytes(*hasher.finalize().as_bytes())
    }

    fn personal_room_ticket(&self) -> String {
        self.room_ticket(self.personal_room_topic(), &self.ticket_extra_peers)
            .to_string()
    }

    /// Refresh the displayed room ticket when iroh learns a new relay or
    /// direct address asynchronously. The current endpoint address is read
    /// by `room_ticket`, so this also detects changes without stale state.
    fn refresh_local_peer_addr(&mut self) -> bool {
        if self.ticket_str.is_empty() {
            return false;
        }

        let current_ticket = self
            .room_ticket(self.topic, &self.ticket_extra_peers)
            .to_string();
        if current_ticket == self.ticket_str {
            return false;
        }

        self.ticket_str = current_ticket;
        true
    }

    /// Persist current dark_mode, sound_enabled, chat_text_size, and display_name to disk.
    fn save_settings(&self) {
        let settings = AppSettings {
            dark_mode: self.dark_mode,
            sound_enabled: self.settings_state.sound_enabled,
            share_direct_addresses: self.settings_state.share_direct_addresses,
            chat_text_size: self.settings_state.chat_text_size,
            display_name: Some(self.local_label.clone()),
            home_background_image: self.home_background_path.clone(),
            home_menu_item_opacity: self.home_menu_item_opacity,
            accent_color: self.settings_state.accent_color,
            show_presence_indicator: self.settings_state.show_presence_indicator,
            typing_indicators_enabled: self.settings_state.typing_indicators_enabled,
            recent_emojis: self.recent_emojis.clone(),
            notification_policy: self.notifications_state.notification_service.message_policy,
            conversation_notification_policies: self
                .notifications_state
                .notification_service
                .conversation_policies_snapshot(),
        };
        settings.save(&self.data_dir);
    }

    /// Build an `AppSettings` snapshot that preserves the home-screen
    /// background path and persist it off the UI thread.
    fn persist_home_background(
        data_dir: &std::path::Path,
        dark_mode: bool,
        sound_enabled: bool,
        share_direct_addresses: bool,
        chat_text_size: f32,
        display_name: String,
        home_background_image: Option<String>,
        home_menu_item_opacity: f32,
        accent_color: Option<[u8; 3]>,
        show_presence_indicator: bool,
        recent_emojis: Vec<String>,
    ) -> iced::Task<AppMessage> {
        let settings = AppSettings {
            dark_mode,
            sound_enabled,
            share_direct_addresses,
            chat_text_size,
            display_name: Some(display_name),
            home_background_image,
            home_menu_item_opacity,
            accent_color,
            show_presence_indicator,
            typing_indicators_enabled: true,
            recent_emojis,
            notification_policy: crate::notification::service::NotificationPolicy::All,
            conversation_notification_policies: Vec::new(),
        };
        let data_dir = data_dir.to_path_buf();
        iced::Task::perform(
            tokio::task::spawn_blocking(move || settings.save(&data_dir)),
            |_| AppMessage::Noop,
        )
    }

    /// Persist the current public-room directory snapshot to SQLite.
    fn save_directory_store(&self) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let result = storage.with_conn(|conn| {
            let store = self
                .directory_store
                .lock()
                .map_err(|_| anyhow::anyhow!("directory store mutex poisoned"))?;
            store.save_to_db(conn)?;
            Ok(())
        });
        if let Err(err) = result {
            warn!("failed to save directory advertisements: {err}");
        }
    }

    // ── Background persistence helpers (SQLite is the single source of truth) ──

    /// Persist the friends store to SQLite in a background thread to avoid
    /// blocking the GUI event loop.
    fn send_save_friends(&self) {
        let store = self.friends.clone();
        let storage = self.storage.clone();
        std::thread::spawn(move || {
            if let Some(ref storage) = storage {
                if let Err(err) = store.save_to_sqlite(storage) {
                    tracing::warn!("failed to save friends store to SQLite: {err}");
                }
            }
        });
    }

    /// Persist the conversation store to SQLite in a background thread.
    fn send_save_conversations(&self) {
        let store = self.conversation_store.clone();
        let storage = self.storage.clone();
        std::thread::spawn(move || {
            if let Some(ref storage) = storage {
                if let Err(err) = store.save_to_sqlite(storage) {
                    tracing::warn!("failed to save conversation store to SQLite: {err}");
                }
            }
        });
    }

    /// Chat history is persisted at message creation and room lifecycle
    /// boundaries. This hook is retained for callers from the old JSON
    /// implementation, but deliberately does not write `chat_history.json`.
    fn send_save_chat_history(&self) {
        // SQLite is the only write target for chat messages.
    }

    /// Save AppSettings directly (not a legacy JSON store — still active).
    #[expect(dead_code)]
    fn send_save_settings(&self) {
        let settings = AppSettings {
            dark_mode: self.dark_mode,
            sound_enabled: self.settings_state.sound_enabled,
            share_direct_addresses: self.settings_state.share_direct_addresses,
            chat_text_size: self.settings_state.chat_text_size,
            display_name: Some(self.local_label.clone()),
            home_background_image: self.home_background_path.clone(),
            home_menu_item_opacity: self.home_menu_item_opacity,
            accent_color: self.settings_state.accent_color,
            show_presence_indicator: self.settings_state.show_presence_indicator,
            typing_indicators_enabled: self.settings_state.typing_indicators_enabled,
            recent_emojis: self.recent_emojis.clone(),
            notification_policy: self.notifications_state.notification_service.message_policy,
            conversation_notification_policies: self
                .notifications_state
                .notification_service
                .conversation_policies_snapshot(),
        };
        settings.save(&self.data_dir);
    }

    /// Persist the friend request store in a background thread.
    fn send_save_friend_requests(&self) {
        let store = self.friend_request_store.clone();
        std::thread::spawn(move || {
            if let Err(err) = store.save() {
                tracing::warn!("failed to save friend request store: {err}");
            }
        });
    }

    /// Keep the virtualized chat log anchored to the latest entry when the
    /// user is already following the conversation.  The custom windowed
    /// renderer uses `f32::MAX` as its bottom sentinel; retaining the old
    /// finite offset after an image changes the content height leaves the
    /// Iced scrollable's viewport stranded above newly appended messages.
    fn keep_latest_visible(&mut self) {
        if self.follow_latest {
            self.scroll_offset = f32::MAX;
            // The timeline is top-anchored; growth alone would strand the
            // viewport above the newly appended entry.  Snap to the bottom on
            // the next update instead.
            self.scroll_to_bottom_pending = true;
        }
    }

    /// Emit the queued `snap_to_end` task for the chat log, consuming the
    /// pending flag exactly once.  Used both by the shared update tail and by
    /// the `OpenRoom` fast path, which returns early and would otherwise skip
    /// the tail: deferring the snap to a later update lets a stale
    /// `Scrolled` event (carrying the previous conversation's offset) cancel
    /// the flag and strand the viewport away from the bottom.
    fn with_pending_snap(&mut self, task: iced::Task<AppMessage>) -> iced::Task<AppMessage> {
        if self.scroll_to_bottom_pending {
            self.scroll_to_bottom_pending = false;
            iced::Task::batch([task, iced::widget::operation::snap_to_end(CHAT_LOG)])
        } else {
            task
        }
    }

    fn entry_storage_user(&self, entry: &ChatEntry) -> Option<String> {
        match entry.kind {
            ChatKind::System => None,
            ChatKind::Local => Some(self.local_public.to_string()),
            ChatKind::Remote => entry.sender_key.map(|pk| pk.to_string()),
        }
    }

    fn image_chat_kind(sender: PublicKey, local_public: PublicKey) -> ChatKind {
        if sender == local_public {
            ChatKind::Local
        } else {
            ChatKind::Remote
        }
    }

    /// Check if a chat entry needs background image hydration.
    /// Returns `Some((user, identifier))` if the entry has a stored image
    /// identifier but no decoded handle yet — the caller should spawn a
    /// background task to load the image off the UI thread.
    /// Returns `None` if no hydration is needed.
    fn needs_image_hydration(&self, entry: &ChatEntry) -> Option<(String, String)> {
        if entry.image_handle.is_some() {
            return None;
        }
        let identifier = entry.image_identifier.as_deref()?;
        let user = self.entry_storage_user(entry)?;
        Some((user.clone(), identifier.to_string()))
    }

    /// Start a background task to hydrate a stored image from disk.
    /// The task loads the image bytes, creates an iced Handle, and sends
    /// the result back as `AppMessage::ImageHydrated`.
    #[expect(dead_code)]
    fn start_image_hydration(
        image_store: ImageStore,
        user: String,
        identifier: String,
        index: usize,
    ) -> iced::Task<AppMessage> {
        iced::Task::perform(
            async move {
                match load_stored_chat_image(&image_store, &user, &identifier) {
                    Some(bytes) => {
                        let handle = Some(iced::widget::image::Handle::from_bytes(bytes));
                        (handle, None)
                    }
                    None => (None, Some("Image preview unavailable".to_string())),
                }
            },
            move |(handle, error): (Option<iced::widget::image::Handle>, Option<String>)| {
                AppMessage::ImageHydrated {
                    index,
                    handle,
                    error,
                }
            },
        )
    }

    fn image_handle_for_entry(&self, entry: &ChatEntry) -> Option<iced::widget::image::Handle> {
        // Only return the cached handle — never fall through to disk I/O.
        // The disk-loading path (`hydrate_entry_image`) is called during
        // `entries_push` and populates `image_handle` once.  Every frame
        // should use the already-decoded handle; re-decoding on each
        // render would cause severe scroll stutter.
        entry.image_handle.clone()
    }

    fn start_next_pending_image_download(&mut self) -> iced::Task<AppMessage> {
        let Some((name, hash, sender_pk)) = self.pending_image.pop_front() else {
            return iced::Task::none();
        };
        // Capture the conversation ownership token when the download starts.
        // If the user switches rooms while the download is in flight, the
        // completion generation will not match and the stale entry is caught
        // in debug builds instead of landing in the wrong conversation.
        let generation = self.conversation_generation;
        let blob_store = self.blob_store.clone();
        let endpoint = self.endpoint.clone();
        let neighbors = self.neighbors.clone();
        let safety = self.public_room_safety.clone();
        let image_store = self.image_store.clone();
        iced::Task::perform(
            async move {
                use boru_core::chat_callbacks::TransferKind;
                let blob_hash: iroh_blobs::Hash = hash.into();
                let candidates = download_candidates(sender_pk, &neighbors);
                match download_blob_with_safety(
                    &blob_store,
                    &endpoint,
                    blob_hash,
                    candidates,
                    name.clone(),
                    TransferKind::Image,
                    |_| {},
                    safety.as_deref(),
                    sender_pk,
                )
                .await
                {
                    Ok(buf) => {
                        // Preserve GIF animation: skip JPEG re-encoding so
                        // decode_gif_frames on the receiver can extract
                        // individual frames.  All other image types get a
                        // lightweight display thumbnail for the preview card.
                        let is_gif = name.to_lowercase().ends_with(".gif");
                        let thumb = if is_gif {
                            buf.clone()
                        } else {
                            compress_image(&buf)
                        };
                        // Save to the per-user image store in the background task,
                        // avoiding blake3 hashing and file I/O on the UI thread.
                        let user = sender_pk.to_string();
                        let image_identifier = image_store.save_image(&user, &name, &thumb).ok();
                        Ok((name, thumb, image_identifier))
                    }
                    Err(e) => Err(format!("Download: {e}")),
                }
            },
            move |r: Result<(String, Vec<u8>, Option<String>), String>| match r {
                Ok((name, data, id)) => AppMessage::ImageDownloaded {
                    sender: sender_pk,
                    name: name.clone(),
                    display_name: name,
                    image_bytes: data,
                    message_hash: hash,
                    image_identifier: id,
                    generation,
                },
                Err(e) => AppMessage::ErrorMsg(e),
            },
        )
    }

    /// Start fetching the next queued video thumbnail blob (the sender's
    /// bounded poster). Runs off the UI thread via `Task::perform`; a
    /// failure only leaves the placeholder in place — it never fails the
    /// download card or the video itself.
    fn start_next_pending_thumbnail_fetch(&mut self) -> iced::Task<AppMessage> {
        let Some((entry_index, thumbnail_hash, ticket_str)) =
            self.pending_thumbnail_fetch.pop_front()
        else {
            return iced::Task::none();
        };
        let blob_store = self.blob_store.clone();
        let endpoint = self.endpoint.clone();
        let neighbors = self.neighbors.clone();
        let safety = self.public_room_safety.clone();
        iced::Task::perform(
            async move {
                use boru_core::chat_callbacks::TransferKind;
                let ticket: iroh_blobs::ticket::BlobTicket = ticket_str
                    .parse()
                    .map_err(|e| format!("Invalid thumbnail ticket: {e}"))?;
                let (addr, _hash, _format) = ticket.into_parts();
                let node_id = addr.id;
                let candidates = download_candidates(node_id, &neighbors);
                let blob_hash: iroh_blobs::Hash = thumbnail_hash.into();
                let bytes = download_blob_with_safety(
                    &blob_store,
                    &endpoint,
                    blob_hash,
                    candidates,
                    "video-thumbnail".to_string(),
                    TransferKind::Video,
                    |_| {},
                    safety.as_deref(),
                    node_id,
                )
                .await
                .map_err(|e| format!("thumbnail fetch failed: {e}"))?;
                // The sender's poster is bounded (≤ MAX_POSTER_BYTES). Reject
                // anything larger so a misbehaving sender cannot force a huge
                // download through the poster path.
                if bytes.is_empty() || bytes.len() > boru_core::video_poster::MAX_POSTER_BYTES {
                    return Err("thumbnail blob outside poster size bounds".to_string());
                }
                Ok((entry_index, bytes))
            },
            move |result| match result {
                Ok((entry_index, thumbnail_bytes)) => AppMessage::ThumbnailFetched {
                    entry_index,
                    thumbnail_bytes,
                },
                Err(error) => {
                    tracing::warn!(%error, "video thumbnail fetch failed; keeping placeholder");
                    AppMessage::Noop
                }
            },
        )
    }

    /// Start fetching the next pending external catalogue GIF media over
    /// HTTP.  Runs off the UI thread via `Task::perform`; a failure (missing
    /// or expired URL, network error, oversized body) produces an `Err` that
    /// the handler renders as a clear fallback card.
    fn start_next_pending_gif_fetch(&mut self) -> iced::Task<AppMessage> {
        let Some((gif, sender_pk, message_hash)) = self.pending_gif.pop_front() else {
            return iced::Task::none();
        };
        let generation = self.conversation_generation;
        // Choose the rendition to fetch by format: MP4 playback renditions
        // are played through the inline video player, GIF/WebP renditions
        // through the static-image path. For builds without the video
        // player, skip MP4 and fall back to a renderable image rendition.
        let url = if gif.format == GifMediaFormat::Mp4 && cfg!(feature = "video-playback") {
            gif.first_renderable_url()
        } else {
            gif.first_image_renderable_url()
        }
        .map(|s| s.to_string());
        iced::Task::perform(
            async move {
                let url = match url {
                    Some(u) => u,
                    None => return Err("GIF media URL is missing".to_string()),
                };
                fetch_gif_media_bytes(&url).await
            },
            move |bytes| AppMessage::GifMediaFetched {
                sender: sender_pk,
                gif,
                message_hash,
                bytes,
                generation,
            },
        )
    }

    /// Start the next pending image download (if any) and the next pending
    /// video thumbnail blob fetch (if any) in one batched task.
    fn drain_pending_transfers(&mut self) -> iced::Task<AppMessage> {
        let mut tasks: Vec<iced::Task<AppMessage>> = Vec::new();
        if !self.pending_image.is_empty() {
            tasks.push(self.start_next_pending_image_download());
        }
        if !self.pending_thumbnail_fetch.is_empty() {
            tasks.push(self.start_next_pending_thumbnail_fetch());
        }
        if !self.pending_gif.is_empty() {
            tasks.push(self.start_next_pending_gif_fetch());
        }
        if tasks.is_empty() {
            iced::Task::none()
        } else {
            iced::Task::batch(tasks)
        }
    }

    fn current_download_entry_index(&self, transfer_id: Option<TransferId>) -> Option<usize> {
        if let Some(id) = transfer_id {
            self.transfer_id_to_index
                .get(&id)
                .copied()
                .or(self.download_entry_index)
        } else {
            self.download_entry_index
        }
    }

    #[expect(dead_code)]
    fn current_download_entry_mut(&mut self) -> Option<&mut ChatEntry> {
        let idx = self.current_download_entry_index(self.active_download_transfer_id)?;
        self.entries.get_mut(idx)
    }

    fn handle_download_progress(&mut self, progress: TransferProgress) {
        use boru_core::chat_callbacks::TransferKind;

        let mut invalidate_from = None;
        let mut clear_active_transfer = false;

        match progress {
            TransferProgress::Started {
                id,
                kind,
                name,
                total,
                ..
            } if matches!(kind, TransferKind::File | TransferKind::Video) => {
                self.active_download_transfer_id = Some(id);
                // VID-01: bind the transfer to the card the user actually
                // initiated (`download_entry_index`) when it matches, not a
                // same-named card earlier in the entries list (which can be
                // the uploader's own Active upload card). The name-only scan
                // remains as a fallback for whisper/background downloads that
                // never set the shared index.
                if let Some(idx) =
                    started_target_index(&self.entries, kind, &name, self.download_entry_index)
                        .or_else(|| self.current_download_entry_index(None))
                {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            download.transfer_id = Some(id);
                            // Same terminal-state guard as the Progress
                            // branch: a late or restart-recovered Started
                            // event must not revert a card that DownloadDone
                            // already resolved to Completed{Some(path)} —
                            // the queued Completed event would then downgrade
                            // it to the "Verifying" placeholder
                            // (saved_path: None) and strand it there.
                            if !download.state.is_terminal() {
                                download.state = DownloadState::Active { bytes: 0, total };
                            }
                            self.transfer_id_to_index.insert(id, idx);
                            invalidate_from = Some(idx);
                        }
                    }
                } else if let Some(content_hash) = self.catalogue_name_to_hash(&name) {
                    self.files_state.catalogue_downloads.insert(
                        content_hash,
                        CatalogueDownloadState::Downloading {
                            bytes: 0,
                            total,
                            speed: 0,
                        },
                    );
                }
            }
            TransferProgress::Progress {
                id,
                kind,
                bytes,
                total,
                name,
                ..
            } if matches!(kind, TransferKind::File | TransferKind::Video) => {
                if let Some(idx) = self.current_download_entry_index(Some(id)) {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            if download.transfer_id.is_none() {
                                download.transfer_id = Some(id);
                            }
                            // Compute transfer speed if we have a previous timestamp.
                            let now = std::time::Instant::now();
                            let speed = if let Some(last_at) = self.files_state.last_download_progress_at {
                                let elapsed = now.duration_since(last_at).as_secs_f64().max(0.001);
                                let delta = bytes.saturating_sub(self.files_state.last_download_progress_bytes);
                                (delta as f64 / elapsed) as u64
                            } else {
                                0
                            };
                            self.files_state.last_download_progress_at = Some(now);
                            self.files_state.last_download_progress_bytes = bytes;
                            // Do not overwrite a terminal state (Completed /
                            // Failed / Cancelled) set by DownloadDone — a late
                            // progress event in the queue could otherwise flip
                            // the card back to Active after it has already
                            // shown the green Completed badge.
                            if !download.state.is_terminal() {
                                download.state = DownloadState::Active { bytes, total };
                            }
                            download.speed_bytes_per_sec = Some(speed);
                            self.transfer_id_to_index.insert(id, idx);
                            invalidate_from = Some(idx);
                        }
                    }
                } else if let Some(content_hash) = self.catalogue_name_to_hash(&name) {
                    let now = std::time::Instant::now();
                    let speed = if let Some(last_at) = self.files_state.last_download_progress_at {
                        let elapsed = now.duration_since(last_at).as_secs_f64().max(0.001);
                        let delta = bytes.saturating_sub(self.files_state.last_download_progress_bytes);
                        (delta as f64 / elapsed) as u64
                    } else {
                        0
                    };
                    self.files_state.last_download_progress_at = Some(now);
                    self.files_state.last_download_progress_bytes = bytes;
                    self.files_state.catalogue_downloads.insert(
                        content_hash,
                        CatalogueDownloadState::Downloading {
                            bytes,
                            total,
                            speed,
                        },
                    );
                }
            }
            TransferProgress::Completed { id, kind, name }
                if matches!(kind, TransferKind::File | TransferKind::Video) =>
            {
                tracing::info!(transfer_id=?id, %name, ?kind, "handle_download_progress: Completed");
                if let Some(idx) = self.current_download_entry_index(Some(id)) {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            if download.transfer_id.is_none() {
                                download.transfer_id = Some(id);
                            }
                            // Preserve total_size from Active state for use in Completed.
                            let total_size = match &download.state {
                                DownloadState::Active { total, .. } => *total,
                                _ => None,
                            };
                            // Only transition to Completed if the download is
                            // still Active — if DownloadDone has already resolved
                            // the state (or any other terminal state was reached),
                            // this late progress event must not overwrite it.
                            if matches!(download.state, DownloadState::Active { .. }) {
                                download.state = DownloadState::Completed {
                                    saved_name: name,
                                    saved_path: None,
                                    total_size,
                                };
                                tracing::info!(entry_index=idx, "Completed: transitioned from Active to Completed (saved_path=None)");
                            } else {
                                tracing::info!(
                                    idx,
                                    current_state=?download.state,
                                    "Completed: ignoring late event — download already in terminal state"
                                );
                            }
                            self.transfer_id_to_index.insert(id, idx);
                            invalidate_from = Some(idx);
                        }
                    }
                } else if let Some(content_hash) = self.catalogue_name_to_hash(&name) {
                    // Path will be populated later by DownloadDonePeerFile
                    self.files_state.catalogue_downloads.insert(
                        content_hash,
                        CatalogueDownloadState::Completed {
                            path: PathBuf::new(),
                        },
                    );
                }
                clear_active_transfer = true;
            }
            TransferProgress::Failed {
                id, error, name, ..
            } => {
                if let Some(idx) = self.current_download_entry_index(Some(id)) {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            if download.transfer_id.is_none() {
                                download.transfer_id = Some(id);
                            }
                            download.state = DownloadState::Failed {
                                failure: DownloadFailure::from_error(error.clone()),
                            };
                            self.transfer_id_to_index.insert(id, idx);
                            invalidate_from = Some(idx);
                        }
                    }
                } else if let Some(content_hash) = self.catalogue_name_to_hash(&name) {
                    self.files_state.catalogue_downloads
                        .insert(content_hash, CatalogueDownloadState::Failed(error));
                }
                clear_active_transfer = true;
            }
            TransferProgress::Cancelled { id, kind, .. }
                if matches!(kind, TransferKind::File | TransferKind::Video) =>
            {
                if let Some(idx) = self.current_download_entry_index(Some(id)) {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            if download.transfer_id.is_none() {
                                download.transfer_id = Some(id);
                            }
                            download.state = DownloadState::Cancelled;
                            self.transfer_id_to_index.insert(id, idx);
                            invalidate_from = Some(idx);
                        }
                    }
                }
                clear_active_transfer = true;
            }
            _ => {}
        }

        if clear_active_transfer {
            self.active_download_transfer_id = None;
        }

        if let Some(idx) = invalidate_from {
            self.layout_cache.borrow_mut().invalidate_from(idx);
        }
    }

    fn open_downloaded_file(&self, name: &str) -> Result<(), String> {
        // Check the explicit saved path first (peer file downloads go here).
        // If not found, fall back to current_dir for backward compat with
        // whisper-based downloads.
        let path = self
            .entries
            .iter()
            .find_map(|entry| {
                entry.download.as_ref().and_then(|d| match &d.state {
                    DownloadState::Completed {
                        saved_path: Some(p),
                        ..
                    } => {
                        if d.name == name {
                            let p_clone = p.clone();
                            if p_clone.exists() {
                                return Some(p_clone);
                            }
                        }
                        None
                    }
                    DownloadState::Shared { ref path, .. } => {
                        if d.name == name && path.exists() {
                            return Some(path.clone());
                        }
                        None
                    }
                    _ => None,
                })
            })
            .or_else(|| {
                // Fallback: check boru_downloads_dir and current_dir
                let dl = self.files_state.boru_downloads_dir.join(name);
                if dl.exists() {
                    Some(dl)
                } else {
                    let cwd = std::env::current_dir().unwrap_or_default().join(name);
                    if cwd.exists() {
                        Some(cwd)
                    } else {
                        None
                    }
                }
            });

        let path = match path {
            Some(p) => p,
            None => return Err(format!("File not found: {name}")),
        };

        #[cfg(target_os = "windows")]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "start", "", &path.to_string_lossy()])
                .status()
                .map_err(|e| format!("Open file: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("Open file exited with {status}"))
            }
        }

        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("open")
                .arg(&path)
                .status()
                .map_err(|e| format!("Open file: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("Open file exited with {status}"))
            }
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let status = std::process::Command::new("xdg-open")
                .arg(&path)
                .status()
                .map_err(|e| format!("Open file: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("Open file exited with {status}"))
            }
        }
    }

    /// Reconcile completed attachment rows with the filesystem. A deleted
    /// local file must not leave an apparently playable video card behind.
    fn refresh_missing_downloads(&mut self) {
        let mut first_changed = None;
        for (idx, entry) in self.entries.iter_mut().enumerate() {
            let Some(download) = entry.download.as_mut() else {
                continue;
            };
            let missing = match &download.state {
                DownloadState::Completed {
                    saved_path: Some(path),
                    ..
                } => !path.exists(),
                _ => false,
            };
            if missing {
                download.state = DownloadState::Failed {
                    failure: DownloadFailure::FileRemoved,
                };
                first_changed.get_or_insert(idx);
            }
        }
        if let Some(idx) = first_changed {
            self.layout_cache.borrow_mut().invalidate_from(idx);
        }
    }

    /// Convert a persisted `HistoryEntry` to a `ChatEntry` for in-memory replay.
    ///
    /// Returns `None` for entries whose kind or sender cannot be resolved.
    fn history_entry_to_chat_entry(
        &self,
        hist: &HistoryEntry,
        _topic: &TopicId,
        local_hex: &str,
    ) -> Option<ChatEntry> {
        let kind = match hist.kind.as_str() {
            "system" => ChatKind::System,
            "text" | "image" => {
                if hist.sender.is_empty() || hist.sender == local_hex {
                    ChatKind::Local
                } else {
                    ChatKind::Remote
                }
            }
            _ => return None,
        };

        let label = match kind {
            ChatKind::System => "System".to_string(),
            ChatKind::Local => "You".to_string(),
            ChatKind::Remote => {
                // Resolve the peer's display name through the full priority
                // chain (friend label → announced name → names cache → short key).
                // Without this, restarted apps show truncated hex keys in
                // history messages until a new live message refreshes the label.
                if let Ok(pk) = PublicKey::from_str(&hist.sender) {
                    self.resolve_name(&pk)
                } else {
                    // Fallback: truncated public key as label
                    hist.sender[..hist.sender.len().min(16)].to_string()
                }
            }
        };

        let sender_key = match kind {
            ChatKind::Remote => PublicKey::from_str(&hist.sender).ok(),
            ChatKind::Local => PublicKey::from_str(local_hex).ok(),
            ChatKind::System => None,
        };

        let is_image = hist.kind == "image";

        if is_image {
            let handle = hist
                .image_bytes
                .as_ref()
                .map(|bytes| iced::widget::image::Handle::from_bytes(bytes.clone()));
            Some(ChatEntry {
                kind,
                label: sanitize_single_line(&label),
                body: sanitize_display_text(&hist.text_preview, DEFAULT_MAX_DISPLAY_LENGTH),
                message_hash: None,
                edited: false,
                reactions: Vec::new(),
                label_text: None,
                reactions_text: None,
                formatted_time: None,
                image_handle: handle,
                avatar_handle: None,
                image_bytes: hist.image_bytes.clone(),
                image_identifier: hist.image_identifier.clone(),
                image_error: None,
                image_width: None,
                image_height: None,
                gif_frames: None,
                timestamp: Some(hist.timestamp as i64),
                event_id: hist.event_id,
                delivery_state: hist.delivery_state.clone(),
                sender_key,
                download: None,
                widget_gen: 0,
                link_preview: None,
                link_preview_loading: false,
                link_preview_error: false,
                parsed_segments: None,
            })
        } else {
            Some(ChatEntry {
                kind,
                label: sanitize_single_line(&label),
                body: sanitize_display_text(&hist.text_preview, DEFAULT_MAX_DISPLAY_LENGTH),
                message_hash: None,
                edited: false,
                reactions: Vec::new(),
                label_text: None,
                reactions_text: None,
                formatted_time: None,
                image_handle: None,
                avatar_handle: None,
                image_bytes: None,
                image_identifier: None,
                image_error: None,
                image_width: None,
                image_height: None,
                gif_frames: None,
                timestamp: Some(hist.timestamp as i64),
                event_id: hist.event_id,
                delivery_state: hist.delivery_state.clone(),
                sender_key,
                download: None,
                widget_gen: 0,
                link_preview: None,
                link_preview_loading: false,
                link_preview_error: false,
                parsed_segments: None,
            })
        }
    }

    /// Convert a SQLite `ChatMessageRow` to a `ChatEntry` for in-memory replay.
    #[expect(dead_code)]
    fn chat_message_row_to_chat_entry(
        row: &boru_core::store::ChatMessageRow,
        local_hex: &str,
    ) -> Option<ChatEntry> {
        use std::str::FromStr;
        if row.kind == "file" {
            let signed = row.signed_bytes.as_deref()?;
            let (_, message, _) = SignedMessage::verify_and_decode(signed).ok()?;
            let Message::FileShare {
                name,
                ticket,
                size,
                thumbnail_hash,
                collection_hash: _,
                collection_entries: _,
            } = message
            else {
                return None;
            };
            let sender = PublicKey::from_bytes(&row.sender).ok()?;
            let transfer_kind = if classify_attachment(None, &name) == MediaKind::Video {
                TransferKind::Video
            } else {
                TransferKind::File
            };
            let mut entry = ChatEntry::system_download(
                format!("File received: {name}"),
                transfer_kind,
                name,
                ticket,
                sender.fmt_short().to_string(),
                None,
            );
            if let Some(download) = entry.download.as_mut() {
                download.state = DownloadState::Ready {
                    total: (size > 0).then_some(size),
                };
                download.thumbnail_hash = thumbnail_hash;
            }
            entry.message_hash = Some(row.msg_hash);
            entry.timestamp = Some(row.timestamp_ms);
            entry.event_id = row.id as u64;
            entry.sender_key = Some(sender);
            return Some(entry);
        }
        let kind = match row.kind.as_str() {
            "system" => ChatKind::System,
            "text" | "image" => {
                let sender_hex = hex::encode(row.sender);
                if sender_hex == local_hex || sender_hex.is_empty() {
                    ChatKind::Local
                } else {
                    ChatKind::Remote
                }
            }
            _ => return None,
        };
        let label = match kind {
            ChatKind::System => "System".to_string(),
            ChatKind::Local => "You".to_string(),
            ChatKind::Remote => {
                let sender_hex = hex::encode(row.sender);
                sender_hex[..sender_hex.len().min(16)].to_string()
            }
        };
        let sender_key = match kind {
            ChatKind::Remote => PublicKey::from_str(&hex::encode(row.sender)).ok(),
            ChatKind::Local => PublicKey::from_str(local_hex).ok(),
            ChatKind::System => None,
        };
        let delivery_state = match row.delivery_state.as_str() {
            "queued" => DeliveryState::Queued,
            "sent" => DeliveryState::Sent,
            "delivered" => DeliveryState::Delivered,
            "seen" => DeliveryState::Seen,
            "failed" => DeliveryState::Failed,
            _ => DeliveryState::Queued,
        };
        let _is_image = row.kind == "image";
        Some(ChatEntry {
            kind,
            label: sanitize_single_line(&label),
            body: sanitize_display_text(&row.body, DEFAULT_MAX_DISPLAY_LENGTH),
            message_hash: Some(row.msg_hash),
            edited: false,
            reactions: Vec::new(),
            label_text: None,
            reactions_text: None,
            formatted_time: None,
            image_handle: None,
            avatar_handle: None,
            image_bytes: None,
            image_identifier: row.image_identifier.clone(),
            image_error: None,
            image_width: None,
            image_height: None,
            gif_frames: None,
            timestamp: Some(row.timestamp_ms),
            event_id: row.id as u64,
            delivery_state,
            sender_key,
            download: None,
            widget_gen: 0,
            link_preview: None,
            link_preview_loading: false,
            link_preview_error: false,
            parsed_segments: None,
        })
    }

    fn push_system(&mut self, text: impl Into<String>) {
        let entry = ChatEntry::system(text);
        self.entries_push(entry);
    }

    /// Push a message to the mesh event log shown on the home screen (capacity 50).
    fn push_mesh_event(&mut self, text: impl Into<String>) {
        if self.mesh_event_log.len() >= 50 {
            self.mesh_event_log.pop_front();
        }
        self.mesh_event_log.push_back(MeshEvent {
            message: text.into(),
            recorded_at: Instant::now(),
        });
    }

    /// UI-28: keep the home mesh card truthful. When the mesh is healthy,
    /// remember when it connected (for the connection-time indicator) and
    /// purge transient startup/connecting messages from the event log so
    /// they never linger after a successful connection. Any non-Good
    /// transition clears the connected timestamp.
    fn update_mesh_connected_state(&mut self, new_health: &MeshHealth) {
        if matches!(new_health, MeshHealth::Good) {
            if self.mesh_connected_at.is_none() {
                self.mesh_connected_at = Some(Instant::now());
            }
            self.clear_transient_mesh_events();
        } else {
            self.mesh_connected_at = None;
        }
    }

    /// UI-28: remove transient startup/connection-progress messages from the
    /// mesh event log. Called when the mesh reaches `MeshHealth::Good` so
    /// "Starting up...", "Connecting to room..." and similar status lines
    /// never linger after connect. Real lifecycle events (degraded/offline/
    /// recovered transitions, errors) are preserved.
    fn clear_transient_mesh_events(&mut self) {
        self.mesh_event_log
            .retain(|event| !is_transient_mesh_event(&event.message));
    }
    #[expect(dead_code)]
    fn push_local(&mut self, text: impl Into<String>) {
        let entry = ChatEntry::local(&self.local_label, text);
        self.entries_push(entry);
    }

    /// Record that `peer` has been observed for the first time ever
    /// (PUBLIC-03). Returns true when the peer was genuinely new — the
    /// Recent Activity feed gets a "New user … came online" entry and the
    /// seen set is persisted — and false when the peer was already known
    /// (reconnect, restart, or our own node).
    ///
    /// The persisted seen set is seeded with existing friends at startup,
    /// so known contacts never re-announce after an upgrade or restart;
    /// only genuinely unseen peers (mDNS discoveries, new gossip
    /// neighbors, new friends) produce the entry.
    fn note_peer_first_seen(&mut self, peer: PublicKey) -> bool {
        if peer == self.local_public {
            return false;
        }
        if !self.seen_peers.insert(peer) {
            return false;
        }
        let name = self.resolve_name(&peer);
        self.notifications_state.push_activity(format!("New user {name} came online"), ActivityKind::Online);
        // Persist in a background thread so the atomic write (fsync +
        // rename) never blocks the iced event loop. This fires only when a
        // genuinely new peer appears, so the cost is negligible.
        let data_dir = self.data_dir.clone();
        let seen = self.seen_peers.clone();
        std::thread::spawn(move || save_seen_peers(&data_dir, &seen));
        true
    }

    /// Rebuild both entry indexes after bulk mutations (room switch, load, eviction).
    fn rebuild_entry_indexes(&mut self) {
        self.event_id_to_index.clear();
        self.message_hash_to_index.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.event_id != 0 {
                self.event_id_to_index.insert(entry.event_id, index);
            }
            if let Some(hash) = entry.message_hash {
                self.message_hash_to_index.insert(hash, index);
            }
        }
        debug_assert!(self.entry_indexes_consistent());
    }

    fn entry_indexes_consistent(&self) -> bool {
        self.event_id_to_index.iter().all(|(id, &index)| {
            self.entries
                .get(index)
                .is_some_and(|entry| entry.event_id == *id)
        }) && self.message_hash_to_index.iter().all(|(hash, &index)| {
            self.entries
                .get(index)
                .and_then(|entry| entry.message_hash.as_ref())
                == Some(hash)
        })
    }

    fn index_entry(&mut self, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        if entry.event_id != 0 {
            self.event_id_to_index.insert(entry.event_id, index);
        }
        if let Some(hash) = entry.message_hash {
            self.message_hash_to_index.insert(hash, index);
        }
        debug_assert!(self.entry_indexes_consistent());
    }

    /// Push an entry and update the incremental layout cache atomically.
    /// Must be the *only* way entries are added to `self.entries`.
    /// Returns the index of the pushed entry.
    fn entries_push(&mut self, mut entry: ChatEntry) -> usize {
        if let Some(hash) = entry.message_hash.as_ref() {
            if self.has_message(hash) {
                return 0;
            }
        }
        // Check if the entry's image needs background hydration.
        // For entries with a stored image identifier but no decoded handle,
        // the caller should spawn a background task via `start_image_hydration`.
        // Currently all callers set image_handle before pushing (ChatEntry::image),
        // so this is a no-op until history replay is enabled.
        if let Some((_user, _id)) = self.needs_image_hydration(&entry) {
            // Placeholder for future history-replay hydration.
            // When replaying, use:
            //   let task = Self::start_image_hydration(
            //       self.image_store.clone(), user, id, self.entries.len()
            //   );
            // and chain it with the parent's returned Task.
        }
        // Cache the sender's avatar handle on the entry so `view_chat_log`
        // can render it without a per-frame HashMap lookup.
        if entry.avatar_handle.is_none() {
            match entry.kind {
                ChatKind::Remote => {
                    if let Some(pk) = entry.sender_key {
                        if let Some(Some(handle)) = self.friend_image_handles.get(&pk) {
                            entry.avatar_handle = Some(handle.clone());
                        }
                    }
                }
                ChatKind::Local => {
                    if let Some(ref handle) = self.settings_state.profile_image_handle {
                        entry.avatar_handle = Some(handle.clone());
                    }
                }
                ChatKind::System => {}
            }
        }
        entry.update_cache();
        let prev_day = self
            .entries
            .last()
            .and_then(|e| e.timestamp.map(|ts| ts / 86400000));
        self.layout_cache
            .borrow_mut()
            .append(&entry, prev_day, self.settings_state.chat_text_size);
        self.entries.push(entry);
        let index = self.entries.len() - 1;
        self.index_entry(index);
        self.keep_latest_visible();
        self.enforce_image_budget();
        self.enforce_entry_cap();
        index
    }

    /// Evict `image_bytes` from the oldest entries that have an
    /// `image_identifier` (can be re-loaded from `ImageStore` on demand)
    /// until total image bytes are within `MAX_IMAGE_BYTES`.
    /// Keeps the `image_handle` so the image still renders in the UI —
    /// only the raw bytes backing potential re-hydration are dropped.
    fn enforce_image_budget(&mut self) {
        let mut total = self.layout_cache.borrow().total_image_bytes;
        if total <= MAX_IMAGE_BYTES {
            return;
        }
        // Evict oldest-first: iterate in insertion order and drop image_bytes
        // from any entry that has an image_identifier (reloadable from ImageStore).
        for entry in &mut self.entries {
            if total <= MAX_IMAGE_BYTES {
                break;
            }
            if entry.image_bytes.is_some() && entry.image_identifier.is_some() {
                if let Some(ref img) = entry.image_bytes {
                    let len = img.len();
                    entry.image_bytes = None;
                    total = total.saturating_sub(len);
                    self.layout_cache.borrow_mut().total_image_bytes = self
                        .layout_cache
                        .borrow()
                        .total_image_bytes
                        .saturating_sub(len);
                }
            }
        }
        // If still over budget, drop image_bytes from entries without
        // an image_identifier too (these images cannot be reloaded, but
        // the handle still renders the current frame).
        if total > MAX_IMAGE_BYTES {
            for entry in &mut self.entries {
                if total <= MAX_IMAGE_BYTES {
                    break;
                }
                if let Some(ref img) = entry.image_bytes.take() {
                    let len = img.len();
                    total = total.saturating_sub(len);
                    self.layout_cache.borrow_mut().total_image_bytes = self
                        .layout_cache
                        .borrow()
                        .total_image_bytes
                        .saturating_sub(len);
                }
            }
        }
    }

    /// Drop the oldest persisted entries so `self.entries` never exceeds
    /// `MAX_ENTRIES`.  Older entries that have already been saved to
    /// `ChatHistoryStore` are removed first.  This bounds the in-memory
    /// overhead of long-running sessions without losing data.
    fn enforce_entry_cap(&mut self) {
        if self.entries.len() <= MAX_ENTRIES {
            return;
        }
        // Save all entries to history before dropping.
        self.save_room_to_history();
        let drain_count = self.entries.len() - MAX_ENTRIES;
        self.entries.drain(..drain_count);
        self.history_saved_count = self.history_saved_count.saturating_sub(drain_count);
        self.rebuild_entry_indexes();
        self.layout_cache.borrow_mut().invalidate_all();
    }

    /// Cap the profile image handle cache at `MAX_PROFILE_IMAGE_HANDLES`.
    /// When the limit is exceeded, entries are evicted in insertion order
    /// (oldest first) since `friend_image_handles` has no LRU ordering.
    fn enforce_profile_image_cap(&mut self) {
        if self.friend_image_handles.len() <= MAX_PROFILE_IMAGE_HANDLES {
            return;
        }
        let excess = self.friend_image_handles.len() - MAX_PROFILE_IMAGE_HANDLES;
        let keys: Vec<PublicKey> = self
            .friend_image_handles
            .keys()
            .take(excess)
            .cloned()
            .collect();
        for k in &keys {
            self.friend_image_handles.remove(k);
            self.friend_image_tickets.remove(k);
            // Also evict the on-disk cache so stale images don't survive.
            let path = friend_profile_image_path(&self.data_dir, k);
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(path.with_extension("tmp"));
        }
    }

    /// Shared send helper: sign, persist to history and outbox.
    /// Used by both the normal composer path and SendMessage (background).
    /// Returns the key data needed by the caller to push to entries and broadcast.
    fn persist_outgoing_message(
        &mut self,
        topic: TopicId,
        text: &str,
    ) -> Result<(u64, MessageHash, bytes::Bytes), String> {
        self.persist_outgoing_message_with_target(topic, text, None)
    }

    /// Persist and encode a normal or thread-targeted outgoing message.
    fn persist_outgoing_message_with_target(
        &mut self,
        topic: TopicId,
        text: &str,
        thread_target: Option<boru_core::threads::ThreadTarget>,
    ) -> Result<(u64, MessageHash, bytes::Bytes), String> {
        let msg = match thread_target {
            Some(target) => crate::Message::ThreadMessage {
                text: text.to_string(),
                target,
            },
            None => crate::Message::Message {
                text: text.to_string(),
            },
        };
        let msg_hash = message_hash(&msg);
        let local_hex = hex::encode(self.local_public.as_bytes());
        let encoded =
            SignedMessage::sign_and_encode(&self.secret_key, &msg).map_err(|e| e.to_string())?;
        let event_id = {
            let mut store = self.chat_history.lock().unwrap();
            let entry =
                HistoryEntry::new(topic, local_hex, encoded.to_vec(), "text", text.to_string());
            let id = store.push_with_id(entry);
            drop(store);
            id
        };
        // The message store is the single source of truth for conversation
        // history. Keep the separate outgoing table for delivery retries and
        // event-id compatibility, but never rely on it for replay.
        let message_store_path = self.data_dir.join("message_store.db");
        let message_hash = *blake3::hash(&encoded).as_bytes();
        if let Err(error) = MessageStore::open(&message_store_path).and_then(|store| {
            store
                .insert_chat_message(
                    &message_hash,
                    topic.as_bytes(),
                    self.local_public.as_bytes(),
                    now_ms() as u64,
                    "text",
                    text,
                    Some(&encoded),
                    None,
                    self.local_public.as_bytes(),
                )
                .map(|_| ())
        }) {
            warn!(%error, "failed to persist outgoing message history in SQLite");
        }
        if let Some(target) = thread_target {
            if let Err(error) = MessageStore::open(&message_store_path)
                .and_then(|store| store.set_thread_target(&message_hash, &target))
            {
                warn!(%error, "failed to project outgoing thread target");
            }
        }
        if let (Some(storage), Some(target)) = (&self.storage, thread_target) {
            if let Err(error) = storage.insert_thread_message(
                &msg_hash,
                topic.as_bytes(),
                self.local_public.as_bytes(),
                now_ms() as u64,
                &encoded,
                Some(target),
            ) {
                warn!(%error, "failed to persist outgoing thread relation");
            }
        }
        if let Some(storage) = &self.storage {
            let hash = boru_core::chat_history::blake3_hex(&encoded);
            match storage.insert_outgoing_message(event_id, &topic, &hash, &encoded) {
                Ok(()) => {
                    info!(
                        "SQLite insert_outgoing_message OK for event_id={}",
                        event_id
                    );
                }
                Err(e) => {
                    error!(
                        "SQLite insert_outgoing_message failed for event_id={}: {e}",
                        event_id
                    );
                }
            }
        } else {
            warn!("SQLite storage is None — outgoing messages not persisted to DB");
        }
        info!(
            topic = %topic,
            message_hash = ?msg_hash,
            local_peer = %self.local_public.fmt_short(),
            persistence_result = "queued",
            "message delivery telemetry"
        );
        Ok((event_id, msg_hash, encoded))
    }

    fn log_variant(message: &AppMessage) -> &'static str {
        match message {
            #[cfg(feature = "dev-ui")]
            AppMessage::Designer(_) => "Designer",
            AppMessage::GoToChatList => "GoToChatList",
            AppMessage::OpenRoom(_) => "OpenRoom",
            AppMessage::RoomOpened { .. } => "RoomOpened",
            AppMessage::CreateNewRoom => "CreateNewRoom",
            AppMessage::ConfirmCreateNewRoom => "ConfirmCreateNewRoom",
            AppMessage::CancelCreateRoom => "CancelCreateRoom",
            AppMessage::CreateNewRoomDhtToggled(..) => "CreateNewRoomDhtToggled",
            AppMessage::CreateNewRoomNameChanged(..) => "CreateNewRoomNameChanged",
            AppMessage::CreateNewRoomVisibilityChanged(..) => "CreateNewRoomVisibilityChanged",
            AppMessage::CreateNewRoomDescriptionChanged(..) => "CreateNewRoomDescriptionChanged",
            AppMessage::CreateNewRoomTagsChanged(..) => "CreateNewRoomTagsChanged",
            AppMessage::OpenRoomSettings(_) => "OpenRoomSettings",
            AppMessage::RoomSettingsNameChanged(..) => "RoomSettingsNameChanged",
            AppMessage::RoomSettingsDescriptionChanged(..) => "RoomSettingsDescriptionChanged",
            AppMessage::RoomSettingsTagsChanged(..) => "RoomSettingsTagsChanged",
            AppMessage::RoomSettingsVisibilityChanged(..) => "RoomSettingsVisibilityChanged",
            AppMessage::ConfirmRoomSettings => "ConfirmRoomSettings",
            AppMessage::CancelRoomSettings => "CancelRoomSettings",
            AppMessage::SetRoomDirectoryVisibility { .. } => "SetRoomDirectoryVisibility",
            AppMessage::JoinFromTicket => "JoinFromTicket",
            AppMessage::RoomJoinFailed { .. } => "RoomJoinFailed",
            AppMessage::JoinTicketInputChanged(_) => "JoinTicketInputChanged",
            AppMessage::NewChatCreated => "NewChatCreated",
            AppMessage::RoomSelected(_) => "RoomSelected",
            AppMessage::OpenGroupChat(_) => "OpenGroupChat",
            AppMessage::StartVoiceCall(_) => "StartVoiceCall",
            AppMessage::StartVideoCall(_) => "StartVideoCall",
            #[cfg(feature = "screen-sharing")]
            AppMessage::StartScreenShare(_) => "StartScreenShare",
            #[cfg(feature = "screen-sharing")]
            AppMessage::StopScreenShare => "StopScreenShare",
            #[cfg(feature = "screen-sharing")]
            AppMessage::AcceptScreenShare => "AcceptScreenShare",
            #[cfg(feature = "screen-sharing")]
            AppMessage::DeclineScreenShare => "DeclineScreenShare",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ToggleScreenShareFullscreen => "ToggleScreenShareFullscreen",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ToggleScreenShareDetails => "ToggleScreenShareDetails",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ToggleScreenShareCursor => "ToggleScreenShareCursor",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareEventReceived(_) => "ScreenShareEventReceived",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareFrameReceived(_) => "ScreenShareFrameReceived",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareStatsReceived(_) => "ScreenShareStatsReceived",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareCommandFinished(_) => "ScreenShareCommandFinished",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareRequestControl => "ScreenShareRequestControl",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareRequestClipboard => "ScreenShareRequestClipboard",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSendClipboard => "ScreenShareSendClipboard",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareHostSendClipboard => "ScreenShareHostSendClipboard",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareClipboardRead(_) => "ScreenShareClipboardRead",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareGrantControl(_) => "ScreenShareGrantControl",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareDenyControl => "ScreenShareDenyControl",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareToggleAudio => "ScreenShareToggleAudio",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareRevokeControl => "ScreenShareRevokeControl",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareLowerQuality => "ScreenShareLowerQuality",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareFullQuality => "ScreenShareFullQuality",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSelectSource(_) => "ScreenShareSelectSource",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareDismissNotice => "ScreenShareDismissNotice",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePointerMove { .. } => "ScreenSharePointerMove",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePointerButton { .. } => "ScreenSharePointerButton",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareKeyEvent { .. } => "ScreenShareKeyEvent",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareWheel { .. } => "ScreenShareWheel",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSetView { .. } => "ScreenShareSetView",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePanStart { .. } => "ScreenSharePanStart",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePanMove { .. } => "ScreenSharePanMove",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenSharePanEnd => "ScreenSharePanEnd",
            #[cfg(feature = "screen-sharing")]
            AppMessage::ScreenShareSetPreset(_) => "ScreenShareSetPreset",
            AppMessage::CallEventReceived(_) => "CallEventReceived",
            AppMessage::WindowFocusChanged(_) => "WindowFocusChanged",
            AppMessage::AcceptIncomingCall(_) => "AcceptIncomingCall",
            AppMessage::RejectIncomingCall(_) => "RejectIncomingCall",
            AppMessage::HangUp(_) => "HangUp",
            AppMessage::ToggleCallMute => "ToggleCallMute",
            AppMessage::ToggleCallDeafen => "ToggleCallDeafen",
            AppMessage::ToggleCallCamera => "ToggleCallCamera",
            AppMessage::SelectMicrophone(_) => "SelectMicrophone",
            AppMessage::SelectSpeaker(_) => "SelectSpeaker",
            AppMessage::SelectCamera(_) => "SelectCamera",
            AppMessage::CallUiTick => "CallUiTick",
            AppMessage::CallStarted(_) => "CallStarted",
            AppMessage::CallCommandFinished(_) => "CallCommandFinished",
            AppMessage::InputChanged(_) => "InputChanged",
            AppMessage::SendPressed => "SendPressed",
            AppMessage::AttachPressed => "AttachPressed",
            AppMessage::ComposerSendFinished => "ComposerSendFinished",
            AppMessage::ComposerDragOver(_) => "ComposerDragOver",
            AppMessage::ComposerFileDropped(_) => "ComposerFileDropped",
            AppMessage::ComposerImeActive(_) => "ComposerImeActive",
            AppMessage::ToggleHelp => "ToggleHelp",
            AppMessage::ToggleChatOptions => "ToggleChatOptions",
            AppMessage::ToggleChatSearch => "ToggleChatSearch",
            AppMessage::ChatSearchQueryChanged(_) => "ChatSearchQueryChanged",
            AppMessage::ClearConversation => "ClearConversation",
            AppMessage::OpenSettings => "OpenSettings",
            AppMessage::SharedByMeMenuToggle(_) => "SharedByMeMenuToggle",
            AppMessage::SharedByMeDetails(_) => "SharedByMeDetails",
            AppMessage::SharedByMeCloseDetails => "SharedByMeCloseDetails",
            AppMessage::SharedByMeReveal(_) => "SharedByMeReveal",
            AppMessage::SharedByMeConfirmStopSharing(_) => "SharedByMeConfirmStopSharing",
            AppMessage::SharedByMeCancelStopSharing => "SharedByMeCancelStopSharing",
            AppMessage::SharedByMeRevokeAccess(..) => "SharedByMeRevokeAccess",
            AppMessage::SharedByMeLoaded(_) => "SharedByMeLoaded",
            AppMessage::SharedByMeThumbnailReady { .. } => "SharedByMeThumbnailReady",
            AppMessage::DashboardRecentActivityLoaded(_) => "DashboardRecentActivityLoaded",
            AppMessage::DashboardSharingSummaryLoaded(_) => "DashboardSharingSummaryLoaded",
            AppMessage::DashboardDownloadedRefresh => "DashboardDownloadedRefresh",
            AppMessage::DashboardDownloadedLoaded(_) => "DashboardDownloadedLoaded",
            AppMessage::DownloadedOpen(_) => "DownloadedOpen",
            AppMessage::DownloadedReveal(_) => "DownloadedReveal",
            AppMessage::DownloadedRemoveHistory(_) => "DownloadedRemoveHistory",
            AppMessage::DownloadingCancel(_) => "DownloadingCancel",
            AppMessage::DownloadingPause(_) => "DownloadingPause",
            AppMessage::DownloadingResume(_) => "DownloadingResume",
            AppMessage::DownloadingStop(_) => "DownloadingStop",
            AppMessage::OpenDownloadManager => "OpenDownloadManager",
            AppMessage::CloseDownloadManager => "CloseDownloadManager",
            AppMessage::TransferProjectionUpdate(_) => "TransferProjectionUpdate",
            AppMessage::TransferSnapshotResync => "TransferSnapshotResync",
            AppMessage::CloseSettings => "CloseSettings",
            AppMessage::OpenFileSharing => "OpenFileSharing",
            AppMessage::DashboardSearchChanged(_) => "DashboardSearchChanged",
            AppMessage::DashboardSearchCleared => "DashboardSearchCleared",
            AppMessage::DashboardSharedByMeSortClicked(_) => "DashboardSharedByMeSortClicked",
            AppMessage::DashboardDownloadedSortClicked(_) => "DashboardDownloadedSortClicked",
            AppMessage::DashboardActivitySortClicked(_) => "DashboardActivitySortClicked",
            AppMessage::DashboardTabSelected(_) => "DashboardTabSelected",
            AppMessage::ActivityLogLoaded(_) => "ActivityLogLoaded",
            AppMessage::ActivityLogRefresh => "ActivityLogRefresh",
            AppMessage::ActivityLogFilterSelected(_) => "ActivityLogFilterSelected",
            AppMessage::ActivityLogPageSelected(_) => "ActivityLogPageSelected",
            AppMessage::ActivityLogDetailsToggled(_) => "ActivityLogDetailsToggled",
            AppMessage::ActivityLogClearRequested => "ActivityLogClearRequested",
            AppMessage::ActivityLogClearCancelled => "ActivityLogClearCancelled",
            AppMessage::ActivityLogClearConfirmed => "ActivityLogClearConfirmed",
            AppMessage::DashboardConnectivityDismissed => "DashboardConnectivityDismissed",
            AppMessage::DashboardDownloadingRefresh => "DashboardDownloadingRefresh",
            AppMessage::CatalogueFetchFailed(_) => "CatalogueFetchFailed",
            AppMessage::CatalogueErrorDismissed => "CatalogueErrorDismissed",
            AppMessage::NetEvent(_) => "NetEvent",
            AppMessage::ReplayPendingEvents(_) => "ReplayPendingEvents",
            AppMessage::FriendEvent(_) => "FriendEvent",
            AppMessage::WhisperEvent(_) => "WhisperEvent",
            AppMessage::InboxEvent(_) => "InboxEvent",
            AppMessage::OutboxRetryResult(_) => "OutboxRetryResult",
            AppMessage::RetryOutgoingMessage(_) => "RetryOutgoingMessage",
            AppMessage::MessageSent(..) => "MessageSent",
            AppMessage::FileSent(_) => "FileSent",
            AppMessage::DownloadDone(..) => "DownloadDone",
            AppMessage::DownloadDonePeerFile(..) => "DownloadDonePeerFile",
            AppMessage::PosterGenerated { .. } => "PosterGenerated",
            AppMessage::VideoMetadataProbed { .. } => "VideoMetadataProbed",
            AppMessage::DownloadFailed(_) => "DownloadFailed",
            AppMessage::OpenDownloadedFile(_) => "OpenDownloadedFile",
            AppMessage::PlayInlineVideo(_) => "PlayInlineVideo",
            AppMessage::StreamInlineVideo(_) => "StreamInlineVideo",
            #[cfg(feature = "video-playback")]
            AppMessage::StreamingServerReady { .. } => "StreamingServerReady",
            #[cfg(feature = "video-playback")]
            AppMessage::StreamingServerFailed { .. } => "StreamingServerFailed",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoTick => "InlineVideoTick",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoShowControls => "InlineVideoShowControls",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoControlsFocused(_) => "InlineVideoControlsFocused",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSeekChanged(_) => "InlineVideoSeekChanged",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSeekReleased => "InlineVideoSeekReleased",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSeekRelative(_) => "InlineVideoSeekRelative",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoToggleMute => "InlineVideoToggleMute",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoAdjustVolume(_) => "InlineVideoAdjustVolume",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSetVolume(_) => "InlineVideoSetVolume",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoToggleExpanded => "InlineVideoToggleExpanded",
            #[cfg(feature = "video-playback")]
            AppMessage::CloseInlineVideo => "CloseInlineVideo",
            AppMessage::StreamUrl(_) => "StreamUrl",
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoEvent(_) => "InlineVideoEvent",
            AppMessage::OpenDownloadsFolder => "OpenDownloadsFolder",
            AppMessage::ReportBug => "ReportBug",
            AppMessage::SaveSupportBundle => "SaveSupportBundle",
            AppMessage::OpenUrl(_) => "OpenUrl",
            AppMessage::LinkPreviewLoaded(..) => "LinkPreviewLoaded",
            AppMessage::ErrorMsg(_) => "ErrorMsg",
            AppMessage::ExecuteFileSend(_) => "ExecuteFileSend",
            AppMessage::AttachFolderPressed => "AttachFolderPressed",
            AppMessage::ExecuteFolderSend(_) => "ExecuteFolderSend",
            AppMessage::ExecuteDownload => "ExecuteDownload",
            AppMessage::ExecuteDownloadAt(_) => "ExecuteDownloadAt",
            AppMessage::PauseDownloadAt(_) => "PauseDownloadAt",
            AppMessage::ResumeDownloadAt(_) => "ResumeDownloadAt",
            AppMessage::CancelDownloadAt(_) => "CancelDownloadAt",
            AppMessage::ReshareFile(_) => "ReshareFile",
            AppMessage::MintShortCode(_) => "MintShortCode",
            AppMessage::ShortCodeMinted(_) => "ShortCodeMinted",
            AppMessage::CloseShortCodeDialog => "CloseShortCodeDialog",
            AppMessage::CopyShortCode(_) => "CopyShortCode",
            AppMessage::OpenRedeemCodeDialog => "OpenRedeemCodeDialog",
            AppMessage::CloseRedeemCodeDialog => "CloseRedeemCodeDialog",
            AppMessage::RedeemCodeInputChanged(_) => "RedeemCodeInputChanged",
            AppMessage::RedeemShortCode => "RedeemShortCode",
            AppMessage::ShortCodeRedeemed(_) => "ShortCodeRedeemed",
            AppMessage::SetOverwritePolicy(..) => "SetOverwritePolicy",
            AppMessage::DownloadInitiated { .. } => "DownloadInitiated",
            AppMessage::DownloadInitiationFailed { .. } => "DownloadInitiationFailed",
            AppMessage::OpenPeerProfile(..) => "OpenPeerProfile",
            AppMessage::ClosePeerProfile => "ClosePeerProfile",
            AppMessage::OpenFriendProfile(..) => "OpenFriendProfile",
            AppMessage::CloseFriendProfile => "CloseFriendProfile",
            AppMessage::ToggleFriendProfileMenu => "ToggleFriendProfileMenu",
            AppMessage::OpenShareLocalService => "OpenShareLocalService",
            AppMessage::OpenShareVncTunnel => "OpenShareVncTunnel",
            AppMessage::ShareLocalServiceNameChanged(_) => "ShareLocalServiceNameChanged",
            AppMessage::ShareLocalServicePortChanged(_) => "ShareLocalServicePortChanged",
            AppMessage::ShareLocalServiceExpiryChanged(_) => "ShareLocalServiceExpiryChanged",
            AppMessage::ConfirmShareLocalService => "ConfirmShareLocalService",
            AppMessage::CancelShareLocalService => "CancelShareLocalService",
            AppMessage::ShareLocalServiceScanDone(_) => "ShareLocalServiceScanDone",
            AppMessage::SelectShareLocalServiceSuggestion(_) => "SelectShareLocalServiceSuggestion",
            AppMessage::TunnelShared { .. } => "TunnelShared",
            AppMessage::TunnelShareFailed { .. } => "TunnelShareFailed",
            AppMessage::ShareLocalServiceHttpToggled(_) => "ShareLocalServiceHttpToggled",
            AppMessage::ConnectReceivedTunnel(_) => "ConnectReceivedTunnel",
            AppMessage::ReceivedTunnelConnected { .. } => "ReceivedTunnelConnected",
            AppMessage::ReceivedTunnelConnectFailed { .. } => "ReceivedTunnelConnectFailed",
            AppMessage::DisconnectReceivedTunnel(_) => "DisconnectReceivedTunnel",
            AppMessage::StopSharingTunnel(_) => "StopSharingTunnel",
            AppMessage::OpenReceivedTunnel(_) => "OpenReceivedTunnel",
            AppMessage::CopyReceivedTunnelAddress(_) => "CopyReceivedTunnelAddress",
            AppMessage::TunnelOfferSent => "TunnelOfferSent",
            AppMessage::TunnelOfferSendFailed { .. } => "TunnelOfferSendFailed",
            AppMessage::FriendRenameInputChanged(_) => "FriendRenameInputChanged",
            AppMessage::FriendRenameConfirm => "FriendRenameConfirm",
            AppMessage::CopyPeerId(_) => "CopyPeerId",
            AppMessage::OpenConnectionDetails => "OpenConnectionDetails",
            AppMessage::CloseConnectionDetails => "CloseConnectionDetails",
            AppMessage::ShowInviteMemberDialog => "ShowInviteMemberDialog",
            AppMessage::HideInviteMemberDialog => "HideInviteMemberDialog",
            AppMessage::InviteMemberToggled(_) => "InviteMemberToggled",
            AppMessage::ConfirmInviteMember => "ConfirmInviteMember",
            AppMessage::AcceptGroupInvite(_) => "AcceptGroupInvite",
            AppMessage::CopyConnectionDetails => "CopyConnectionDetails",
            AppMessage::CopyConnectionDetailsValue { .. } => "CopyConnectionDetailsValue",
            AppMessage::DismissToast => "DismissToast",
            AppMessage::ShowRemoveFriendConfirm => "ShowRemoveFriendConfirm",
            AppMessage::CancelRemoveFriend => "CancelRemoveFriend",
            AppMessage::ConfirmRemoveFriend => "ConfirmRemoveFriend",
            AppMessage::ShowBlockFriendConfirm => "ShowBlockFriendConfirm",
            AppMessage::ShowRenameFriendInput => "ShowRenameFriendInput",
            AppMessage::CancelBlockFriend => "CancelBlockFriend",
            AppMessage::ConfirmBlockFriend => "ConfirmBlockFriend",
            AppMessage::OpenImageLightbox(..) => "OpenImageLightbox",
            AppMessage::CloseImageLightbox => "CloseImageLightbox",
            AppMessage::RetryConnection => "RetryConnection",
            AppMessage::BackgroundSubscribe(..) => "BackgroundSubscribe",
            AppMessage::BackgroundSubscribed(..) => "BackgroundSubscribed",
            AppMessage::BackgroundSubscribeFailed(..) => "BackgroundSubscribeFailed",
            AppMessage::ImageUploadFailed(_) => "ImageUploadFailed",
            AppMessage::FileUploadFailed(_) => "FileUploadFailed",
            AppMessage::FileOfferAnnounced { .. } => "FileOfferAnnounced",
            AppMessage::FileOfferCached { .. } => "FileOfferCached",
            AppMessage::FileOfferCacheFailed { .. } => "FileOfferCacheFailed",
            AppMessage::FileDownloaded { .. } => "FileDownloaded",
            AppMessage::ThumbnailFetched { .. } => "ThumbnailFetched",
            AppMessage::ExecuteImageSend(_) => "ExecuteImageSend",
            AppMessage::ImageDownloaded { .. } => "ImageDownloaded",
            AppMessage::GifMediaFetched { .. } => "GifMediaFetched",
            AppMessage::FriendAdded { .. } => "FriendAdded",
            AppMessage::RemoveFriend(_) => "RemoveFriend",
            AppMessage::FriendRemoved { .. } => "FriendRemoved",
            AppMessage::FriendListResult(_) => "FriendListResult",
            AppMessage::DeleteRoom(_) => "DeleteRoom",
            AppMessage::SplashTick => "SplashTick",
            AppMessage::ActivityTick => "ActivityTick",
            AppMessage::ConnMonitorTick => "ConnMonitorTick",
            AppMessage::MeshWatchdogTick => "MeshWatchdogTick",
            AppMessage::OutboxRetryTick => "OutboxRetryTick",
            AppMessage::IdleTick => "IdleTick",
            AppMessage::UserActivity => "UserActivity",

            AppMessage::ToggleDark(_) => "ToggleDark",
            AppMessage::UiThemeReloaded { .. } => "UiThemeReloaded",
            AppMessage::LayoutReloaded { .. } => "LayoutReloaded",
            #[cfg(feature = "dev-ui")]
            AppMessage::Inspector(_) => "Inspector",
            AppMessage::ToggleAccentColorPicker => "ToggleAccentColorPicker",
            AppMessage::AccentColorSelected(_) => "AccentColorSelected",
            AppMessage::AccentColorCancelled => "AccentColorCancelled",
            AppMessage::SetNickname(_) => "SetNickname",

            AppMessage::WindowResized { .. } => "WindowResized",

            AppMessage::Noop => "Noop",
            #[cfg(feature = "dev-ui")]
            AppMessage::ToggleGallery => "ToggleGallery",
            #[cfg(feature = "dev-ui")]
            AppMessage::GalleryPreset(_) => "GalleryPreset",
            #[cfg(feature = "dev-ui")]
            AppMessage::GalleryCustomWidth(_) => "GalleryCustomWidth",
            #[cfg(feature = "dev-ui")]
            AppMessage::GalleryLayoutPreset(_) => "GalleryLayoutPreset",
            AppMessage::AddSharedFile => "AddSharedFile",
            AppMessage::AddSharedFolder => "AddSharedFolder",
            AppMessage::SharedFolderPicked(_) => "SharedFolderPicked",
            AppMessage::SharedByMeToggleShareMenu => "SharedByMeToggleShareMenu",
            AppMessage::SharedFilePicked(_) => "SharedFilePicked",
            AppMessage::SharedFileAddFailed(_) => "SharedFileAddFailed",
            AppMessage::SharedFileAdded(_) => "SharedFileAdded",
            AppMessage::RemoveSharedFile(_) => "RemoveSharedFile",
            AppMessage::SharedFileRemoved(_) => "SharedFileRemoved",
            AppMessage::CopyToClipboard(_) => "CopyToClipboard",
            AppMessage::CopyMessage(_) => "CopyMessage",
            AppMessage::RightClickText(_) => "RightClickText",
            AppMessage::RightClickImage(_) => "RightClickImage",
            AppMessage::ContextCopyText(_) => "ContextCopyText",
            AppMessage::ContextCopyImage(_) => "ContextCopyImage",
            AppMessage::PinMessage(_) => "PinMessage",
            AppMessage::UnpinMessage(_) => "UnpinMessage",
            AppMessage::RevealPinnedMessage(_) => "RevealPinnedMessage",
            AppMessage::CloseContextMenu => "CloseContextMenu",
            AppMessage::ToggleVideoCardMenu(_) => "ToggleVideoCardMenu",
            AppMessage::ToggleEmojiPicker => "ToggleEmojiPicker",
            AppMessage::InsertEmoji(_) => "InsertEmoji",
            AppMessage::SelectEmojiCategory(_) => "SelectEmojiCategory",
            AppMessage::EmojiSearchChanged(_) => "EmojiSearchChanged",
            AppMessage::ToggleGifPicker => "ToggleGifPicker",
            AppMessage::GifSearchChanged(_) => "GifSearchChanged",
            AppMessage::SendGif(_) => "SendGif",
            AppMessage::GifSearchSubmit => "GifSearchSubmit",
            AppMessage::GifRetry => "GifRetry",
            AppMessage::GifSearchDebounced(_) => "GifSearchDebounced",
            AppMessage::GifSearchResults { .. } => "GifSearchResults",
            AppMessage::GifTrendingResults { .. } => "GifTrendingResults",
            AppMessage::GifSearchFailed { .. } => "GifSearchFailed",
            AppMessage::GifPreviewLoaded(..) => "GifPreviewLoaded",
            AppMessage::GifLoadMore => "GifLoadMore",
            AppMessage::CopyFriendId => "CopyFriendId",
            AppMessage::FriendIdCopiedClear => "FriendIdCopiedClear",
            AppMessage::CopyShareTicket(_) => "CopyShareTicket",
            AppMessage::OpenReceiveTicketDialog => "OpenReceiveTicketDialog",
            AppMessage::CloseReceiveTicketDialog => "CloseReceiveTicketDialog",
            AppMessage::ReceiveTicketInputChanged(_) => "ReceiveTicketInputChanged",
            AppMessage::ReceiveTicketPreflight => "ReceiveTicketPreflight",
            AppMessage::ReceiveTicketPreflightDone(_) => "ReceiveTicketPreflightDone",
            AppMessage::ConfirmReceiveTicket => "ConfirmReceiveTicket",
            AppMessage::OpenFriendChat(_) => "OpenFriendChat",
            AppMessage::ToggleSound(_) => "ToggleSound",
            AppMessage::SetNotificationPolicy(_) => "SetNotificationPolicy",
            AppMessage::SetConversationNotificationPolicy(_, _) => "SetConversationNotificationPolicy",
            AppMessage::TogglePresenceIndicator(_) => "TogglePresenceIndicator",
            AppMessage::ToggleTypingIndicators(_) => "ToggleTypingIndicators",
            AppMessage::ToggleInviteAddressSharing(_) => "ToggleInviteAddressSharing",
            AppMessage::SetChatTextSize(_) => "SetChatTextSize",
            AppMessage::PickProfileImage => "PickProfileImage",
            AppMessage::ProfileImagePicked(_) => "ProfileImagePicked",
            AppMessage::PickHomeBackgroundImage => "PickHomeBackgroundImage",
            AppMessage::HomeBackgroundImagePicked(_) => "HomeBackgroundImagePicked",
            AppMessage::HomeBackgroundImageReady { .. } => "HomeBackgroundImageReady",
            AppMessage::RemoveHomeBackgroundImage => "RemoveHomeBackgroundImage",
            AppMessage::SetHomeMenuItemOpacity(_) => "SetHomeMenuItemOpacity",
            AppMessage::ProfileImageUploaded(_) => "ProfileImageUploaded",
            AppMessage::RemoveProfileImage => "RemoveProfileImage",
            AppMessage::ProfileImageDownloaded(..) => "ProfileImageDownloaded",
            AppMessage::ProfileImageDownloadFailed(..) => "ProfileImageDownloadFailed",
            AppMessage::ImageHydrated { .. } => "ImageHydrated",
            AppMessage::ClearHistoryRequested => "ClearHistoryRequested",
            AppMessage::ConfirmClearHistory => "ConfirmClearHistory",
            AppMessage::ClearHistoryFinished { .. } => "ClearHistoryFinished",
            AppMessage::ClearHistoryFailed { .. } => "ClearHistoryFailed",
            AppMessage::DeleteRoomRequested(_) => "DeleteRoomRequested",
            AppMessage::ConfirmDeleteRoom(_) => "ConfirmDeleteRoom",
            AppMessage::MailboxReplayed { .. } => "MailboxReplayed",
            AppMessage::Scrolled(..) => "Scrolled",
            AppMessage::ConnCountsResult { .. } => "ConnCountsResult",
            AppMessage::TicketPeersResolved { .. } => "TicketPeersResolved",
            AppMessage::ConnectionsResult(_) => "ConnectionsResult",
            AppMessage::SendFriendRequest(_) => "SendFriendRequest",
            AppMessage::FriendRequestSent { .. } => "FriendRequestSent",
            AppMessage::FriendRequestFailed { .. } => "FriendRequestFailed",
            AppMessage::FriendRequestReceived { .. } => "FriendRequestReceived",
            AppMessage::FriendRequestRetry(_) => "FriendRequestRetry",
            AppMessage::NewDiscoveredPeers(_) => "NewDiscoveredPeers",
            AppMessage::ReconnectPeerReady(_) => "ReconnectPeerReady",
            AppMessage::BrowsePeerCatalogue(_) => "BrowsePeerCatalogue",
            AppMessage::PeerCatalogueReceived { .. } => "PeerCatalogueReceived",
            AppMessage::PeerCatalogueFailed(_) => "PeerCatalogueFailed",
            AppMessage::CatalogueScrolled(..) => "CatalogueScrolled",
            AppMessage::RequestFileDownload { .. } => "RequestFileDownload",
            AppMessage::IncomingFriendRequestAccept { .. } => "IncomingFriendRequestAccept",
            AppMessage::IncomingFriendRequestDecline { .. } => "IncomingFriendRequestDecline",
            AppMessage::IncomingFriendRequestProcessed { .. } => "IncomingFriendRequestProcessed",
            AppMessage::OpenFriendRequests => "OpenFriendRequests",
            AppMessage::CloseFriendRequests => "CloseFriendRequests",
            AppMessage::ToggleSidebarSectionCollapsed(_) => "ToggleSidebarSectionCollapsed",
            AppMessage::FriendRequestSearchChanged(_) => "FriendRequestSearchChanged",
            AppMessage::FriendRequestSend(_) => "FriendRequestSend",
            AppMessage::FriendRequestAccept(_) => "FriendRequestAccept",
            AppMessage::FriendRequestDecline(_) => "FriendRequestDecline",
            AppMessage::FriendRequestCancel(_) => "FriendRequestCancel",
            AppMessage::FriendRequestSentResult(_) => "FriendRequestSentResult",
            AppMessage::FriendRequestActionResult(_) => "FriendRequestActionResult",
            AppMessage::Shortcut(s) => match s {
                Shortcut::Escape => "Shortcut(Escape)",
                Shortcut::NewChat => "Shortcut(NewChat)",
                Shortcut::BackToChatList => "Shortcut(BackToChatList)",
                Shortcut::QuickCommand => "Shortcut(QuickCommand)",
                Shortcut::FocusNext => "Shortcut(FocusNext)",
                Shortcut::FocusPrevious => "Shortcut(FocusPrevious)",
                Shortcut::DashboardTabPrevious => "Shortcut(DashboardTabPrevious)",
                Shortcut::DashboardTabNext => "Shortcut(DashboardTabNext)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerUndo => "Shortcut(DesignerUndo)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerRedo => "Shortcut(DesignerRedo)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerSave => "Shortcut(DesignerSave)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerNudgeUp => "Shortcut(DesignerNudgeUp)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerNudgeDown => "Shortcut(DesignerNudgeDown)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerNudgeLeft => "Shortcut(DesignerNudgeLeft)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerNudgeRight => "Shortcut(DesignerNudgeRight)",
                #[cfg(feature = "dev-ui")]
                Shortcut::DesignerDelete => "Shortcut(DesignerDelete)",
            },
            AppMessage::DownloadProgress(_) => "DownloadProgress",
            AppMessage::ShowCreateGroupDialog => "ShowCreateGroupDialog",
            AppMessage::HideCreateGroupDialog => "HideCreateGroupDialog",
            AppMessage::CreateGroupNameChanged(_) => "CreateGroupNameChanged",
            AppMessage::CreateGroupDescriptionChanged(_) => "CreateGroupDescriptionChanged",
            AppMessage::CreateGroupMemberToggled(_) => "CreateGroupMemberToggled",
            AppMessage::CreateGroupSearchChanged(_) => "CreateGroupSearchChanged",
            AppMessage::ConfirmCreateGroup => "ConfirmCreateGroup",
            AppMessage::GroupCreated { .. } => "GroupCreated",
            AppMessage::ShowCreateTunnelDialog => "ShowCreateTunnelDialog",
            AppMessage::CreateTunnelPortChanged(_) => "CreateTunnelPortChanged",
            AppMessage::CreateTunnel(_) => "CreateTunnel",
            AppMessage::CancelCreateTunnel => "CancelCreateTunnel",
            AppMessage::TunnelRequestReceived { .. } => "TunnelRequestReceived",
            AppMessage::AcceptTunnelRequest(_) => "AcceptTunnelRequest",
            AppMessage::DeclineTunnelRequest(_) => "DeclineTunnelRequest",
            AppMessage::CloseTunnel(_) => "CloseTunnel",
            AppMessage::OpenConversation(_) => "OpenConversation",
            AppMessage::SelectConversation(_) => "SelectConversation",
            AppMessage::CloseConversation(_) => "CloseConversation",
            AppMessage::SendMessage { .. } => "SendMessage",
            AppMessage::ToggleInviteMenu => "ToggleInviteMenu",
            AppMessage::InviteWhisperInputChanged(_) => "InviteWhisperInputChanged",
            AppMessage::InviteSendWhisper => "InviteSendWhisper",
            AppMessage::ProfileImageRemoved => "ProfileImageRemoved",
            AppMessage::ProfileImagePersisted { .. } => "ProfileImagePersisted",
            AppMessage::SystemMsg(_) => "SystemMsg",
            AppMessage::OfflineDMStatus { .. } => "OfflineDMStatus",
            AppMessage::SaveProfile => "SaveProfile",
            AppMessage::ProfileSaved => "ProfileSaved",
            AppMessage::GuiTestActionReceived(_) => "GuiTestActionReceived",
            AppMessage::GuiActionTimeout(_) => "GuiActionTimeout",
            AppMessage::GuiTestWaitSatisfied(_) => "GuiTestWaitSatisfied",
            AppMessage::GuiTestWaitTimedOut { .. } => "GuiTestWaitTimedOut",
            AppMessage::ImportFriendFromFile => "ImportFriendFromFile",
            AppMessage::ImportFriendFromFilePicked(_) => "ImportFriendFromFilePicked",
            AppMessage::SubscribeStoredConversations => "SubscribeStoredConversations",
            AppMessage::ToggleAdvertiseRoom(..) => "ToggleAdvertiseRoom",
            AppMessage::SubscribeDirectoryTopic => "SubscribeDirectoryTopic",
            AppMessage::DirectorySubscribed(..) => "DirectorySubscribed",
            AppMessage::OpenDirectory => "OpenDirectory",
            AppMessage::CloseDiscover => "CloseDiscover",
            AppMessage::RefreshRoomRegistry => "RefreshRoomRegistry",
            AppMessage::DiscoverSearchChanged(_) => "DiscoverSearchChanged",
            AppMessage::DiscoverFilterToggled(_) => "DiscoverFilterToggled",
            AppMessage::DiscoverTagToggled(_) => "DiscoverTagToggled",
            AppMessage::DiscoverSortChanged(_) => "DiscoverSortChanged",
            AppMessage::DiscoverClearFilters => "DiscoverClearFilters",
            AppMessage::OpenGroups => "OpenGroups",
            AppMessage::CloseGroups => "CloseGroups",
            AppMessage::DirectoryRoomJoin(..) => "DirectoryRoomJoin",
            AppMessage::DirectoryRoomJoinById(_) => "DirectoryRoomJoinById",
            AppMessage::DirectoryRoomHideById(_) => "DirectoryRoomHideById",
            AppMessage::DirectoryRoomUnhideById(_) => "DirectoryRoomUnhideById",
            AppMessage::DirectoryRoomUnhideAll => "DirectoryRoomUnhideAll",
            AppMessage::DeleteDirectoryRoom(_) => "DeleteDirectoryRoom",
            AppMessage::DirectoryRoomUpdate(..) => "DirectoryRoomUpdate",
            AppMessage::DirectoryRoomWithdrawal(..) => "DirectoryRoomWithdrawal",
            AppMessage::ToggleDetailsPanel => "ToggleDetailsPanel",
            AppMessage::ToggleMemberList => "ToggleMemberList",
            #[cfg(not(feature = "video-playback"))]
            AppMessage::InlineVideoShowControls => "InlineVideoShowControls",
            #[cfg(feature = "terminal")]
            AppMessage::TerminalEvent(_) => "TerminalEvent",
            #[cfg(feature = "terminal")]
            AppMessage::OpenTerminal => "OpenTerminal",
        }
    }
}

// ── Room switching helpers ───────────────────────────────────────────

impl IcedChat {
    #[cfg(feature = "video-playback")]
    const INLINE_VIDEO_RELEASE_AFTER: Duration = Duration::from_secs(10);

    /// The chat renderer already computes an overscanned virtualized window.
    /// Reuse that range as the lifecycle viewport: a player is considered
    /// nearby while its card is in the rendered window (800 px overscan), so
    /// scrolling does not make playback thrash at the viewport edge.
    #[cfg(feature = "video-playback")]
    fn inline_video_near_viewport(&self, key: &VideoInstanceKey) -> bool {
        let Some(entry_index) = self.entries.iter().position(|entry| {
            entry.event_id == key.message_id
                && entry
                    .download
                    .as_ref()
                    .is_some_and(|download| download.name == key.attachment_id)
        }) else {
            return false;
        };
        let layout = &mut *self.layout_cache.borrow_mut();
        layout.ensure(&self.entries, self.settings_state.chat_text_size, 1024.0);
        let (first, last, _, _) = layout.window(self.scroll_offset, self.viewport_height);
        (first..=last).contains(&entry_index)
    }

    /// Pause immediately when the active card leaves the overscanned window,
    /// then release the decoder after a 10-second grace period. This keeps
    /// rapid scrolling responsive without constructing players for visible
    /// cards, while retaining enough state for an intentional resume.
    #[cfg(feature = "video-playback")]
    fn reconcile_inline_video_viewport(&mut self) {
        let Some(key) = self
            .inline_video
            .as_ref()
            .map(|session| session.key.clone())
        else {
            return;
        };
        if !self.entries.iter().any(|entry| {
            entry.event_id == key.message_id
                && entry
                    .download
                    .as_ref()
                    .is_some_and(|download| download.name == key.attachment_id)
        }) {
            self.stop_inline_video();
            return;
        }
        if self.inline_video_near_viewport(&key) {
            if let Some(session) = self.inline_video.as_mut() {
                session.last_near_viewport = Instant::now();
            }
            return;
        }

        let should_release = if let Some(session) = self.inline_video.as_mut() {
            if let Some(video) = session.video.as_mut().and_then(Arc::get_mut) {
                session.resume_position = video.position();
                video.set_paused(true);
                // Pausing ends the current talkspurt: raise the keepalive
                // floor so stale frames from before the pause are dropped
                // when playback resumes.
                let framerate = video.framerate();
                if framerate.is_finite() && framerate > 0.0 {
                    let floor = (session.resume_position.as_secs_f64() * framerate).floor() as u32;
                    session.jitter.reset_after_keepalive(floor);
                }
            }
            session.last_near_viewport.elapsed() >= Self::INLINE_VIDEO_RELEASE_AFTER
        } else {
            false
        };
        if should_release {
            if let Some(session) = self.inline_video.take() {
                self.inline_video_resume = Some((session.key.clone(), session.resume_position));
                self.playback_coordinator.clear(Some(&session.key));
                self.inline_video_seek = None;
                self.inline_video_expanded = false;
                self.layout_cache.borrow_mut().clear();
            }
        }
    }

    #[cfg(feature = "video-playback")]
    fn stop_inline_video(&mut self) {
        if let Some(session) = self.inline_video.as_mut() {
            if let Some(video) = session.video.as_mut() {
                if let Some(video) = Arc::get_mut(video) {
                    video.set_paused(true);
                }
            }
            if session.jitter.total_losses() > 0 {
                tracing::info!(
                    message_id = session.key.message_id,
                    losses = session.jitter.total_losses(),
                    "inline video playout finished with lost frames"
                );
            }
        }
        self.inline_video = None;
        self.inline_video_resume = None;
        self.inline_video_expanded = false;
        self.playback_coordinator.clear(None);
        self.layout_cache.borrow_mut().clear();
    }

    /// Map a filename extension to a MIME content type for HTTP streaming.
    fn content_type_for_filename(name: &str) -> String {
        let ext = std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        match ext.to_lowercase().as_str() {
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "mkv" => "video/x-matroska",
            "avi" => "video/x-msvideo",
            "mov" => "video/quicktime",
            "ogv" | "ogg" => "video/ogg",
            _ => "application/octet-stream",
        }
        .to_string()
    }

    /// Start progressive external playback for an undownloaded video when the
    /// `video-playback` feature is disabled (Windows builds). The blob
    /// download runs in the background while a local HTTP streaming server
    /// serves the growing store file, and the URL is handed to the OS default
    /// player (VLC/browser). Returns a task that starts both; `None` when the
    /// video cannot be streamed (unknown size / missing identity).
    fn stream_for_external_play(
        &self,
        _entry_index: usize,
        download: &DownloadAttachment,
    ) -> Option<iced::Task<AppMessage>> {
        let total_size = match &download.state {
            DownloadState::Ready { total } => total.unwrap_or(0),
            DownloadState::Active { total, .. } => total.unwrap_or(0),
            DownloadState::Paused { total, .. } => total.unwrap_or(0),
            DownloadState::Completed { total_size, .. } => total_size.unwrap_or(0),
            _ => 0,
        };
        if total_size == 0 {
            return None;
        }
        // If already downloaded, just open it.
        if let DownloadState::Completed {
            saved_path: Some(_),
            ..
        } = &download.state
        {
            return Some(iced::Task::done(AppMessage::OpenDownloadedFile(
                download.name.clone(),
            )));
        }
        let Some(content_hash) = download.expected_content_hash.clone() else {
            return None;
        };

        // Download task: identical to the classic download-then-open path, so
        // the card still reaches Completed (and the file can be opened later)
        // even while the stream is being consumed.
        let name = download.name.clone();
        let expected_hash = content_hash.clone();
        let data_dir = self.data_dir.clone();
        let blob_store = self.blob_store.clone();
        let endpoint = self.endpoint.clone();
        let neighbors = self.neighbors.clone();
        let progress_queue = self.files_state.download_progress_queue.clone();
        let kind = download.kind;
        let ticket = download.ticket.clone();

        // Stream-task inputs are captured before `name`/`data_dir` move into
        // the download task below.
        let store_data_path = data_dir
            .join("blobs")
            .join("data")
            .join(format!("{content_hash}.data"));
        let content_type = Self::content_type_for_filename(&name);
        let server_slot = self.external_stream_server.clone();
        if let Ok(mut guard) = server_slot.lock() {
            guard.take(); // stop any previous external stream
        }

        let download_task = iced::Task::perform(
            async move {
                let dl_dir = data_dir.join("downloads");
                let _ = tokio::fs::create_dir_all(&dl_dir).await;
                // BORU-AUDIT-21: reserve the destination atomically
                // (O_EXCL) instead of checking a path and reopening it later.
                let mut destination =
                    match boru_core::safe_destination::reserve_download_destination(
                        &dl_dir,
                        &name,
                        "download",
                        boru_core::safe_destination::OverwritePolicy::KeepBoth,
                    )
                    .map_err(|e| format!("Unsafe download name: {e}"))?
                    {
                        boru_core::safe_destination::Reservation::Use(dest) => dest,
                        boru_core::safe_destination::Reservation::Skip => {
                            return Err("Download skipped: destination name already exists".into());
                        }
                    };

                let parsed: iroh_blobs::ticket::BlobTicket =
                    ticket.parse().map_err(|e| format!("Invalid ticket: {e}"))?;
                let (addr, hash, _format) = parsed.into_parts();
                let candidates = download_candidates(addr.id, &neighbors);

                download_blob_to_file(
                    &blob_store,
                    &endpoint,
                    hash,
                    candidates,
                    name.clone(),
                    kind,
                    &mut destination,
                    Some(expected_hash.as_str()),
                    move |ev| {
                        if let Ok(mut q) = progress_queue.lock() {
                            q.push_back(ev);
                        }
                    },
                    Some(total_size),
                )
                .await
                .map_err(|e| format!("Download failed: {e}"))?;
                let save_path = destination
                    .publish()
                    .map_err(|e| format!("Publish failed: {e}"))?;

                Ok::<_, String>((name, save_path))
            },
            |result| match result {
                Ok((name, save_path)) => AppMessage::DownloadDone(name, save_path),
                Err(e) => AppMessage::ErrorMsg(e),
            },
        );

        // Stream task: serve the growing FsStore data file over HTTP and hand
        // the URL to the OS default player. The server handle is parked in
        // `external_stream_server` so it outlives this task (the OS player
        // connects after the URL is shown); the next stream or a room leave
        // drops it, which stops the server and closes its file handles.
        let stream_task = iced::Task::perform(
            async move {
                let server = StreamingServer::start(store_data_path, total_size, content_type)
                    .await
                    .map_err(|e| e.to_string())?;
                let url = server.url();
                if let Ok(mut guard) = server_slot.lock() {
                    *guard = Some(server);
                }
                Ok::<_, String>(url)
            },
            |result| match result {
                Ok(url) => AppMessage::StreamUrl(url),
                Err(e) => AppMessage::ErrorMsg(format!("Could not start video stream: {e}")),
            },
        );

        Some(iced::Task::batch([download_task, stream_task]))
    }

    /// User-facing hint shown when an external-player stream is ready. The URL
    /// is intentionally included so the user can paste it into any player even
    /// if the default open fails.
    fn external_stream_hint(url: &str) -> String {
        format!(
            "Stream ready: {url}\nOpening in your default video player — or paste this URL into VLC or a browser."
        )
    }

    #[cfg(feature = "video-playback")]
    pub(crate) fn has_inline_video(&self) -> bool {
        // A preparation session has no decoder to advance or repaint.  Avoid
        // subscribing to the 250 ms UI tick until the backend has actually
        // produced a player; viewport cleanup still runs from the existing
        // connection/scroll events.
        self.inline_video
            .as_ref()
            .is_some_and(|session| session.video.is_some())
    }

    fn leave_current_room(&mut self) {
        // A room switch changes only the selected view. Keep the sender and
        // forwarder alive in the per-conversation map so incoming events are
        // not lost while another conversation is selected.
        let topic = self.topic;
        #[cfg(feature = "video-playback")]
        self.stop_inline_video();
        // Stop any external-player HTTP stream (non-video-playback builds):
        // the server holds a file handle on the store data path.
        if let Ok(mut guard) = self.external_stream_server.lock() {
            guard.take();
        }
        let mut conversation = self
            .conversations
            .remove(&topic)
            .unwrap_or_else(|| ConversationLive::new(topic));
        tracing::info!(topic=%topic, has_sender=self.sender.is_some(), "leave_current_room");
        conversation.sender = self.sender.take();
        conversation.sender_ready = self.sender_ready;
        self.sender_ready = false;
        conversation.forward_handle = self.forward_handle.take();
        conversation.forward_handle_slot = self.forward_handle_slot.clone();
        conversation.ticket_str = std::mem::take(&mut self.ticket_str);
        conversation.entries = std::mem::take(&mut self.entries);
        conversation.composer_text = std::mem::take(&mut self.composer_text);
        conversation.names = std::mem::take(&mut self.names);
        conversation.self_sent_events = std::mem::take(&mut self.self_sent_events);
        conversation.event_id_to_index = std::mem::take(&mut self.event_id_to_index);
        conversation.message_hash_to_index = std::mem::take(&mut self.message_hash_to_index);
        conversation.neighbors = std::mem::take(&mut self.neighbors);
        conversation.history_saved_count = self.history_saved_count;
        conversation.pending_file = self.pending_file.take();
        conversation.pending_image = std::mem::take(&mut self.pending_image);
        conversation.download_entry_index = self.download_entry_index.take();
        conversation.active_download_transfer_id = self.active_download_transfer_id.take();
        conversation.transfer_id_to_index = std::mem::take(&mut self.transfer_id_to_index);
        conversation.follow_latest = self.follow_latest;
        conversation.scroll_offset = self.scroll_offset;
        conversation.viewport_height = self.viewport_height;
        self.conversations.insert(topic, conversation);
        self.entries.clear();
        self.event_id_to_index.clear();
        self.message_hash_to_index.clear();
        self.layout_cache.borrow_mut().invalidate_all();
        self.names.clear();
        self.pending_file = None;
        self.pending_image.clear();
        self.transfer_id_to_index.clear();
        self.download_entry_index = None;
        self.active_download_transfer_id = None;
        // neighbors preserved across room switches so discovered-peers and
        // friend-online caches don't appear empty after switching rooms.
        self.history_saved_count = 0;
        // Clear the new-peer announcement set so re-entering this room (or
        // entering a different one) announces its peers fresh again.
        self.known_peers.clear();
    }

    fn clear_current_room_history_runtime(
        &mut self,
        topic: TopicId,
        report: &RoomHistoryClearReport,
    ) {
        if self.topic == topic {
            self.entries.clear();
            self.event_id_to_index.clear();
            self.message_hash_to_index.clear();
            self.layout_cache.borrow_mut().invalidate_all();
            self.pending_file = None;
            self.pending_image.clear();
            self.download_entry_index = None;
            self.active_download_transfer_id = None;
            self.transfer_id_to_index.clear();
            self.history_saved_count = 0;
            if report.room_history_updated {
                self.room_history_dirty = false;
            }
        }

        if let Some(conversation) = self.conversations.get_mut(&topic) {
            conversation.entries.clear();
            conversation.event_id_to_index.clear();
            conversation.message_hash_to_index.clear();
            conversation.self_sent_events.clear();
            conversation.pending_file = None;
            conversation.pending_image.clear();
            conversation.download_entry_index = None;
            conversation.active_download_transfer_id = None;
            conversation.transfer_id_to_index.clear();
            conversation.history_saved_count = 0;
        }

        self.room_history.update_preview(&topic, "");
    }

    /// Switch the display to a conversation whose runtime state is already in
    /// `self.conversations`.
    ///
    /// 1. Saves the current room's unsaved entries to history.
    /// 2. Restores the target conversation's sender, entries, composer text,
    ///    scroll position, and all other display fields from the HashMap.
    ///
    /// Returns `true` if the switch succeeded, `false` if the conversation was
    /// not found (caller should fall through to a fresh subscription).
    /// Drop transient typing state whenever conversation ownership changes.
    fn reset_typing_state(&mut self) {
        self.typing_peers = TypingState::default();
        self.typing_emitter.reset();
    }

    fn switch_to_conversation(&mut self, topic: TopicId) -> bool {
        tracing::info!(topic=%topic, "switch_to_conversation called");
        if let Some(mut conversation) = self.conversations.remove(&topic) {
            // Save the current active conversation's runtime state into
            // self.conversations before overwriting it with the target.
            // Without this, switching away and back triggers a full slow-path
            // subscribe every time (30s subscribe_and_join timeout).

            // Capture preview before leave_current_room clears entries.
            let preview = self
                .entries
                .last()
                .map(|e| {
                    let t = e.body.clone();
                    if t.len() > 60 {
                        format!("{}…", &t[..60])
                    } else {
                        t
                    }
                })
                .unwrap_or_default();

            self.save_room_to_history();
            self.leave_current_room();

            if !preview.is_empty() {
                self.room_history.update_preview(&self.topic, &preview);
            }
            self.room_history_dirty = true;

            // Restore the target conversation state.
            // sender may be None if the room subscription has not yet
            // completed — the conversation is still viewable (read-only)
            // until RoomOpened delivers the sender.
            self.topic = topic;
            self.screen = Screen::Chat { topic };
            self.sender = conversation.sender.take();
            self.sender_ready = conversation.sender_ready;
            conversation.sender_ready = false;
            self.forward_handle = conversation.forward_handle.take();
            self.forward_handle_slot = conversation.forward_handle_slot;
            self.ticket_str = std::mem::take(&mut conversation.ticket_str);
            self.entries = std::mem::take(&mut conversation.entries);
            self.composer_text = std::mem::take(&mut conversation.composer_text);
            self.names = std::mem::take(&mut conversation.names);
            self.self_sent_events = std::mem::take(&mut conversation.self_sent_events);
            self.event_id_to_index = std::mem::take(&mut conversation.event_id_to_index);
            self.message_hash_to_index = std::mem::take(&mut conversation.message_hash_to_index);
            self.neighbors = conversation.neighbors;
            self.history_saved_count = conversation.history_saved_count;
            self.pending_file = conversation.pending_file.take();
            self.pending_image = std::mem::take(&mut conversation.pending_image);
            self.download_entry_index = conversation.download_entry_index.take();
            self.active_download_transfer_id = conversation.active_download_transfer_id.take();
            self.transfer_id_to_index = std::mem::take(&mut conversation.transfer_id_to_index);
            // Returning to a conversation always shows the latest messages
            // (standard messenger behaviour): force follow-latest so the
            // timeline snaps to the bottom regardless of where the user was
            // reading when they last left.  The per-conversation reading
            // position is intentionally not preserved across switches.
            self.follow_latest = true;
            self.scroll_offset = f32::MAX;
            self.viewport_height = conversation.viewport_height;
            // Recompute the snap intent from the forced follow-latest state.
            // `scroll_to_bottom_pending` is a transient global, not
            // per-conversation state: a snap armed by the PREVIOUS
            // conversation (e.g. switching away from a follow-latest room)
            // must be cleared when the restored conversation is scrolled up,
            // otherwise the queued snap fires at the end of this update and
            // steals the reading position of the newly opened room.  Here the
            // open always follows latest, so the snap is always re-armed.
            self.scroll_to_bottom_pending = true;

            self.layout_cache.borrow_mut().invalidate_all();

            // The active conversation changed — bump the ownership token so
            // in-flight async completions (e.g. image downloads) started for
            // the previously active conversation are detected as stale.
            self.conversation_generation = self.conversation_generation.wrapping_add(1);

            // Keep the pending queue in the runtime map so it can be drained
            // incrementally by `ReplayPendingEvents`.  Draining the entire
            // queue here blocks the Iced update loop for large direct-room
            // backlogs.
            conversation.unread = 0;
            if !conversation.pending_events.is_empty() {
                let pending = std::mem::take(&mut conversation.pending_events);
                self.conversations
                    .entry(topic)
                    .or_insert_with(|| ConversationLive::new(topic))
                    .pending_events
                    .extend(pending);
            }

            return true;
        }
        tracing::info!(topic=%topic, "switch_to_conversation: no conversation found, returning false");
        false
    }

    /// Replay a bounded batch of events for the active conversation.
    ///
    /// This deliberately returns a follow-up Iced message instead of looping
    /// until the queue is empty.  A direct-room subscription can accumulate a
    /// large backlog while its conversation is hidden; replaying that backlog
    /// synchronously starves input, rendering, and the GUI action channel.
    fn replay_pending_events_batch(&mut self, topic: TopicId) -> iced::Task<AppMessage> {
        let pending: Vec<NetEvent> = self
            .conversations
            .get_mut(&topic)
            .map(|conversation| {
                conversation.unread = 0;
                let batch_len =
                    MAX_PENDING_REPLAY_PER_UPDATE.min(conversation.pending_events.len());
                conversation.pending_events.drain(..batch_len).collect()
            })
            .unwrap_or_default();

        let replayed_count = pending.len();
        let mut tasks = Vec::new();
        for event in pending {
            if let Some(task) = self.process_net_event_sync(&topic, &event) {
                tasks.push(task);
            }
        }
        self.layout_cache.borrow_mut().invalidate_all();

        let remaining = self
            .conversations
            .get(&topic)
            .is_some_and(|conversation| !conversation.pending_events.is_empty());
        if remaining {
            tracing::debug!(topic=%topic, "scheduled next pending-event replay batch");
            tasks.push(iced::Task::done(AppMessage::ReplayPendingEvents(topic)));
        } else if replayed_count > 0 {
            tracing::info!(topic=%topic, "completed pending-event replay");
        }

        if tasks.is_empty() {
            iced::Task::none()
        } else {
            iced::Task::batch(tasks)
        }
    }

    /// Record local-only call metadata in the deterministic direct chat.
    ///
    /// Only the formatted text is stored. No call ID, peer address, media,
    /// or signalling payload is written to the message store.

    /// Persist any newly observed room entries in SQLite.
    ///
    /// `chat_history.json` is intentionally not updated here. It remains a
    /// read-only migration source for old data directories.
    fn save_room_to_history(&mut self) {
        let topic = self.topic;
        let current_count = self.entries.len();
        if self.history_saved_count >= current_count {
            return;
        }
        let store_path = self.data_dir.join("message_store.db");
        let Ok(store) = MessageStore::open(store_path) else {
            warn!("failed to open SQLite message store while saving history");
            return;
        };
        for entry in &self.entries[self.history_saved_count..] {
            let kind = match entry.kind {
                ChatKind::System => "system",
                _ if entry.image_bytes.is_some() || entry.image_identifier.is_some() => "image",
                _ => "text",
            };
            let sender = match entry.kind {
                ChatKind::System => [0u8; 32],
                ChatKind::Local => *entry.sender_key.unwrap_or(self.local_public).as_bytes(),
                ChatKind::Remote => entry
                    .sender_key
                    .map(|pk| *pk.as_bytes())
                    .unwrap_or([0u8; 32]),
            };
            let hash = entry
                .message_hash
                .unwrap_or_else(|| *blake3::hash(entry.body.as_bytes()).as_bytes());
            if let Err(error) = store.insert_chat_message(
                &hash,
                topic.as_bytes(),
                &sender,
                entry.timestamp.unwrap_or_else(now_ms) as u64,
                kind,
                &entry.body,
                None,
                entry.image_identifier.as_deref(),
                self.local_public.as_bytes(),
            ) {
                warn!(%error, "failed to persist room history in SQLite");
            }
        }
        self.history_saved_count = current_count;
    }
}

// ── Deterministic private topic ────────────────────────────────────

/// Compute the chat footer's route/peer status labels (plan UI-16).
///
/// Mirrors the connection-type derivation used by the chat header tooltip
/// and the details panel: a direct peer on the gossip mesh is "Direct
/// (Mesh)", a connected non-neighbour routes over "Relay", and a peer with
/// no connection is "Not connected". Group chats report the gossip mesh
/// directly with the number of connected neighbours. Returns
/// `(route_label, connected, peer_label)`.
fn chat_footer_status(
    is_group: bool,
    neighbors: &HashSet<PublicKey>,
    peer: Option<PublicKey>,
    presence: PeerPresence,
) -> (String, bool, Option<String>) {
    if is_group {
        if neighbors.is_empty() {
            ("Not connected".to_string(), false, None)
        } else {
            (
                "Mesh".to_string(),
                true,
                Some(format!(
                    "{} peer{}",
                    neighbors.len(),
                    if neighbors.len() == 1 { "" } else { "s" }
                )),
            )
        }
    } else if peer.is_some_and(|pk| neighbors.contains(&pk)) {
        (
            "Direct (Mesh)".to_string(),
            true,
            Some("1 peer".to_string()),
        )
    } else if presence != PeerPresence::Offline && presence != PeerPresence::Unknown {
        ("Relay".to_string(), true, Some("1 peer".to_string()))
    } else {
        ("Not connected".to_string(), false, None)
    }
}






/// Create a deterministic topic id from two peer public keys.
///
/// Both peers derive the same topic by sorting their public keys
/// before hashing, so either side can initiate a private chat.
fn private_topic(a: &PublicKey, b: &PublicKey) -> TopicId {
    direct_topic(a, b)
}

#[expect(dead_code)]
fn online_friends_from_store(friends: &FriendsStore) -> HashMap<PublicKey, String> {
    friends
        .iter()
        .filter(|(_, record)| record.status.online)
        .filter_map(|(id, record)| {
            id.parse_public_key()
                .ok()
                .map(|pk| (pk, record.display_label(id, &pk)))
        })
        .collect()
}

// ── GUI test action validation ───────────────────────────────────────

impl IcedChat {
    /// Return the normal application message used to close the foremost
    /// blocking dialog. GUI test actions use the same messages as visible
    /// Cancel buttons rather than mutating dialog state directly.
    fn close_dialog_message(
        image_lightbox: bool,
        show_room_settings_dialog: bool,
        show_create_room_dialog: bool,
        connection_details_dialog: bool,
        history_confirm_clear: bool,
        room_delete_confirm_topic: Option<TopicId>,
    ) -> Result<AppMessage, GuiActionError> {
        if image_lightbox {
            return Ok(AppMessage::CloseImageLightbox);
        }
        if connection_details_dialog {
            return Ok(AppMessage::CloseConnectionDetails);
        }
        if show_room_settings_dialog {
            return Ok(AppMessage::CancelRoomSettings);
        }
        if show_create_room_dialog {
            return Ok(AppMessage::CancelCreateRoom);
        }
        if history_confirm_clear {
            return Ok(AppMessage::ClearHistoryRequested);
        }
        if let Some(topic) = room_delete_confirm_topic {
            return Ok(AppMessage::DeleteRoomRequested(topic));
        }
        Err(GuiActionError::new(
            GuiActionErrorCode::NoDialog,
            "No application dialog is currently open",
        ))
    }

    fn close_current_dialog(&self) -> Result<AppMessage, GuiActionError> {
        Self::close_dialog_message(
            self.lightbox_image.is_some(),
            self.rooms_state.show_room_settings_dialog,
            self.rooms_state.show_create_room_dialog,
            self.connection_details_dialog.is_some(),
            self.history_confirm_clear,
            self.room_delete_confirm_topic,
        )
    }

    fn current_connection_details_dialog(&self) -> ConnectionDetailsDialogState {
        let room_state = match &self.screen {
            Screen::Chat { topic } => format!("Chat room active ({topic})"),
            Screen::OutgoingCall => "Outgoing call".to_string(),
            Screen::ActiveCall => "Active call".to_string(),
            Screen::FriendProfile(_) => "Friend profile open".to_string(),
            Screen::PeerProfile(_) => "Peer profile open".to_string(),
            Screen::PeerCatalogue(_) => "Peer catalogue open".to_string(),
            Screen::FriendRequests => "Friend requests open".to_string(),
            Screen::FileSharing => "File sharing open".to_string(),
            Screen::DownloadManager => "Download manager open".to_string(),
            Screen::Settings => "Settings open".to_string(),
            Screen::ChatList => "Chat list open".to_string(),
            Screen::Discover => "Discover".to_string(),
            Screen::Groups => "Groups".to_string(),
            #[cfg(feature = "terminal")]
            Screen::Terminal => "Terminal open".to_string(),
            #[cfg(feature = "dev-ui")]
            Screen::Gallery => "Component gallery".to_string(),
        };

        let mesh_state = match &self.mesh_health {
            MeshHealth::Good => "Healthy".to_string(),
            MeshHealth::Degraded(reason) => format!("Degraded — {reason}"),
            MeshHealth::Offline(reason) => format!("Offline — {reason}"),
        };

        let relay_mode = match &self.relay_mode {
            RelayMode::Disabled => None,
            _ => Some(fmt_relay_mode(&self.relay_mode)),
        };

        let last_error = match &self.mesh_health {
            MeshHealth::Good => None,
            MeshHealth::Degraded(reason) | MeshHealth::Offline(reason) => Some(reason.clone()),
        };

        ConnectionDetailsDialogState::ready(ConnectionDetailsViewModel::new(
            self.local_public.to_string(),
            relay_mode,
            format!("Room: {room_state} · Mesh: {mesh_state}"),
            if self.discovered_peers.is_empty() {
                "No discovered peers yet".to_string()
            } else {
                format!("{} discovered peers", self.discovered_peers.len())
            },
            format!(
                "{} direct · {} relayed · {} neighbors",
                self.direct_peers,
                self.relayed_peers,
                self.neighbors.len(),
            ),
            self.neighbors.len(),
            last_error,
            if self.peer_latencies.is_empty() {
                None
            } else {
                let count = self.peer_latencies.len();
                let avg_ms: u128 = self
                    .peer_latencies
                    .values()
                    .map(|d| d.as_millis())
                    .sum::<u128>()
                    / count as u128;
                Some(format!(
                    "{avg_ms} ms ({count} peer{}",
                    if count == 1 { ")" } else { "s)" }
                ))
            },
        ))
    }

    fn complete_close_dialog_action(&mut self) {
        if let Some(action_id) = self.pending_close_dialog_action.take() {
            let _ = self
                .gui_action_history
                .set_state(&action_id, GuiActionState::AppMessageHandled);
            let _ = self
                .gui_action_history
                .set_state(&action_id, GuiActionState::Completed);
        }
    }

    fn close_connection_details_dialog(&mut self) -> iced::Task<AppMessage> {
        self.connection_details_dialog = None;
        self.connection_details_announcement = None;
        self.complete_close_dialog_action();
        if let Some(target) = self.connection_details_focus_target.take() {
            iced::widget::operation::focus(target)
        } else {
            iced::Task::none()
        }
    }

    /// Validate a semantic GUI test command against the current UI state.
    pub fn validate_gui_test_command(
        &self,
        command: &GuiTestCommand,
    ) -> Result<(), GuiActionError> {
        let blocking_dialog = || {
            self.rooms_state.show_create_room_dialog
                || self.connection_details_dialog.is_some()
                || self.history_confirm_clear
                || self.room_delete_confirm_topic.is_some()
        };
        let error = |code: GuiActionErrorCode, message: String| GuiActionError::new(code, message);
        let active_room = || matches!(self.screen, Screen::Chat { topic } if topic == self.topic);

        match command {
            GuiTestCommand::OpenRoom { room_id } => {
                let topic = room_id.parse::<TopicId>().map_err(|_| {
                    error(
                        GuiActionErrorCode::UnknownRoom,
                        format!("Room `{room_id}` is not known"),
                    )
                })?;
                if self.room_history.find(&topic).is_none() {
                    return Err(error(
                        GuiActionErrorCode::UnknownRoom,
                        format!("Room `{room_id}` is not known"),
                    ));
                }
                if blocking_dialog() {
                    return Err(error(
                        GuiActionErrorCode::BlockingDialogOpen,
                        "A blocking dialog is open".to_string(),
                    ));
                }
                Ok(())
            }
            GuiTestCommand::OpenConversation { conversation_id } => {
                if !self
                    .conversation_store
                    .iter()
                    .any(|entry| entry.peer_id == *conversation_id)
                {
                    return Err(error(
                        GuiActionErrorCode::UnknownConversation,
                        format!("Conversation `{conversation_id}` is not known"),
                    ));
                }
                Ok(())
            }
            GuiTestCommand::SetComposerText { text } => {
                if !active_room() {
                    return Err(error(
                        GuiActionErrorCode::NoActiveConversation,
                        "No active room".to_string(),
                    ));
                }
                if text.chars().count() > 4096 {
                    return Err(error(
                        GuiActionErrorCode::ComposerTooLong,
                        "Composer text exceeds 4096 characters".to_string(),
                    ));
                }
                Ok(())
            }
            GuiTestCommand::ClearComposer | GuiTestCommand::FocusComposer => {
                if !active_room() {
                    return Err(error(
                        GuiActionErrorCode::NoActiveConversation,
                        "No active room".to_string(),
                    ));
                }
                Ok(())
            }
            GuiTestCommand::SubmitComposer => {
                if !active_room() {
                    return Err(error(
                        GuiActionErrorCode::NoActiveConversation,
                        "No active room".to_string(),
                    ));
                }
                if self.composer_text.trim().is_empty() {
                    return Err(error(
                        GuiActionErrorCode::ComposerEmpty,
                        "Composer is empty".to_string(),
                    ));
                }
                // NOTE: a missing/unready sender must NOT reject the submit.
                // The real Enter key never validates sender state: SendPressed
                // clears the composer, persists the message, and
                // `broadcast_or_queue` drops it into the outgoing queue for
                // the periodic retry when the subscription/mesh becomes ready.
                // Rejecting here (as it did before) silently dropped sends
                // made right after a slow-path room open (sender arrives
                // asynchronously via RoomOpened ~seconds later) — the
                // composer stayed populated and the peer never received the
                // message. Mirror the real flow: accept and let the queue
                // absorb the not-ready window.
                if blocking_dialog() {
                    return Err(error(
                        GuiActionErrorCode::BlockingDialogOpen,
                        "A blocking dialog is open".to_string(),
                    ));
                }
                Ok(())
            }
            GuiTestCommand::SelectPeer { peer_id } => {
                let peer = peer_id.parse::<PublicKey>().map_err(|_| {
                    error(
                        GuiActionErrorCode::UnknownPeer,
                        format!("Peer `{peer_id}` is not known"),
                    )
                })?;
                let known = self.neighbors.contains(&peer)
                    || self.discovered_peers.contains(&peer)
                    || self.profile_cache.contains_key(&peer)
                    || self.names.contains_key(&peer)
                    || self.friends.get(&FriendId::from_public_key(peer)).is_some();
                if !known {
                    return Err(error(
                        GuiActionErrorCode::UnknownPeer,
                        format!("Peer `{peer_id}` is not known"),
                    ));
                }
                Ok(())
            }
            GuiTestCommand::SetPeerPresence { peer_id, .. } => {
                let peer = peer_id.parse::<PublicKey>().map_err(|_| {
                    error(
                        GuiActionErrorCode::UnknownPeer,
                        format!("Peer `{peer_id}` is not known"),
                    )
                })?;
                if self.friends.get(&FriendId::from_public_key(peer)).is_none() {
                    return Err(error(
                        GuiActionErrorCode::UnknownPeer,
                        format!("Peer `{peer_id}` is not a friend"),
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

// ── Update ────────────────────────────────────────────────────────────

impl IcedChat {
    fn publish_gui_state(&mut self) {
        // Step 1: Quick exit if diagnostics are disabled.
        if !self.gui_state_enabled {
            return;
        }

        // Step 2: No consumers listening — skip building the snapshot.
        if self.gui_state_tx.receiver_count() == 0 {
            return;
        }

        // Step 3: Build the snapshot from current state.
        let (active_screen, active_room) = match &self.screen {
            Screen::Chat { topic } => ("Chat", Some(topic.to_string())),
            Screen::OutgoingCall => ("OutgoingCall", None),
            Screen::ActiveCall => ("ActiveCall", None),
            Screen::ChatList => ("ChatList", None),
            Screen::FriendRequests => ("FriendRequests", None),
            Screen::FileSharing => ("FileSharing", None),
            Screen::DownloadManager => ("DownloadManager", None),
            Screen::Settings => ("Settings", None),
            Screen::PeerProfile(_) => ("PeerProfile", None),
            Screen::PeerCatalogue(_) => ("PeerCatalogue", None),
            Screen::FriendProfile(_) => ("FriendProfile", None),
            Screen::Discover => ("Discover", None),
            Screen::Groups => ("Groups", None),
            #[cfg(feature = "terminal")]
            Screen::Terminal => ("Terminal", None),
            #[cfg(feature = "dev-ui")]
            Screen::Gallery => ("Gallery", None),
        };
        let snapshot = IcedStateSnapshot {
            node_id: self.local_public.to_string(),
            version: version_tag(),
            active_screen: active_screen.to_string(),
            active_room,
            conversation_count: self.conversations.len(),
            neighbor_count: self.neighbors.len(),
            direct_peer_count: self.direct_peers,
            relayed_peer_count: self.relayed_peers,
            mesh_health: format!("{:?}", self.mesh_health),
            online_friend_count: 0,
            friend_count: self.friends.iter().count(),
            total_entry_count: self.entries.len(),
            dark_mode: self.dark_mode,
            composer_text: self.composer_text.clone(),
            dialog_open: self.rooms_state.show_create_room_dialog
                || self.connection_details_dialog.is_some()
                || self.history_confirm_clear
                || self.room_delete_confirm_topic.is_some()
                || self.help_overlay.visible(),
            unread_count: 0,
            dashboard: self.build_dashboard_snapshot(),
            timestamp: chrono::Utc::now(),
        };

        // Step 4: Dirty check — skip if no meaningful state change.
        if let Some(ref last) = self.last_snapshot {
            if last.node_id == snapshot.node_id
                && last.version == snapshot.version
                && last.active_screen == snapshot.active_screen
                && last.active_room == snapshot.active_room
                && last.conversation_count == snapshot.conversation_count
                && last.neighbor_count == snapshot.neighbor_count
                && last.direct_peer_count == snapshot.direct_peer_count
                && last.relayed_peer_count == snapshot.relayed_peer_count
                && last.mesh_health == snapshot.mesh_health
                && last.online_friend_count == snapshot.online_friend_count
                && last.friend_count == snapshot.friend_count
                && last.total_entry_count == snapshot.total_entry_count
                && last.dark_mode == snapshot.dark_mode
                && last.composer_text == snapshot.composer_text
                && last.dialog_open == snapshot.dialog_open
                && last.unread_count == snapshot.unread_count
                && last.dashboard == snapshot.dashboard
            {
                return; // No meaningful change — skip publish.
            }
        }

        // Step 5: Rate-limit — throttle to 4-10 updates/sec.
        let now = std::time::Instant::now();
        if self.gui_snapshot_throttle_ms > 0 {
            let elapsed_ms = now.duration_since(self.last_snapshot_at).as_millis() as u64;
            if elapsed_ms < self.gui_snapshot_throttle_ms {
                // Within throttle window — mark pending and skip.
                self.gui_snapshot_pending = true;
                return;
            }
        }

        // Step 6: Flush any accumulated pending flag and send.
        self.gui_snapshot_pending = false;
        self.last_snapshot_at = now;
        let boxed = Box::new(snapshot.clone());
        let _ = self.gui_state_tx.send(snapshot);
        self.last_snapshot = Some(boxed);
    }

    /// Build a display-safe [`DashboardSnapshot`] from current in-memory
    /// dashboard state. Returns `None` when the File Sharing screen is not
    /// the active screen (the snapshot then omits the dashboard section).
    ///
    /// All data comes from already-loaded in-memory structures (the same
    /// buffers the dashboard view renders) — no SQLite reads, no filesystem
    /// access, no local paths.  File names are display labels only.
    fn build_dashboard_snapshot(&self) -> Option<boru_core::diagnostics::DashboardSnapshot> {
        use boru_core::diagnostics::{
            ActivitySummary, DashboardSnapshot, DownloadSummary, FileSummary, TransferSummary,
        };

        if !matches!(self.screen, Screen::FileSharing) {
            return None;
        }

        // Active tab name — mirrors the DashboardTabName serialization used
        // by the MCP navigate destinations.
        let active_tab = match self.files_state.dashboard_active_tab {
            crate::dashboard_view_model::DashboardTab::SharedByMe => "files_sharing",
            crate::dashboard_view_model::DashboardTab::Downloading => "downloading",
            crate::dashboard_view_model::DashboardTab::Downloaded => "downloaded",
            crate::dashboard_view_model::DashboardTab::SharedWithMe => "shared_with_me",
            crate::dashboard_view_model::DashboardTab::ActivityLog => "activity",
        }
        .to_string();

        // Shared by Me tab: files this node registered for sharing.
        let shared_by_me_files: Vec<FileSummary> = self.files_state
            .shared_by_me_rows
            .iter()
            .map(|row| FileSummary {
                name: row.display_name.clone(),
                size_bytes: row.size_bytes,
            })
            .collect();

        // Downloading tab: in-progress inbound transfers with live progress.
        let item_labels = self.files_state
            .inbound_item_labels
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let mut downloading: Vec<TransferSummary> = self.files_state
            .inbound_active
            .values()
            .map(|record| {
                let name = item_labels
                    .get(&record.item_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        let prefix: String = record.item_id.chars().take(12).collect();
                        format!("file {prefix}…")
                    });
                TransferSummary {
                    name,
                    peer_id: record.peer_id.clone(),
                    bytes: record.bytes,
                    total_bytes: record.total_bytes,
                    state: transfer_state_name(record.state),
                }
            })
            .collect();
        downloading.sort_by(|a, b| b.bytes.cmp(&a.bytes));

        // Downloaded tab: completed downloads with source peer labels.
        let downloaded: Vec<DownloadSummary> = self.files_state
            .downloaded_history
            .iter()
            .map(|item| DownloadSummary {
                name: item.display_name.clone(),
                size_bytes: item.size_bytes,
                source_peer: item.source_peer.clone(),
            })
            .collect();

        // Shared with Me tab: validated remote catalogue files.
        let shared_with_me_files: Vec<FileSummary> = self.files_state
            .peer_catalogue_view
            .as_ref()
            .map(|(_peer, files)| {
                files
                    .iter()
                    .map(|file| FileSummary {
                        name: file.display_name.clone(),
                        size_bytes: Some(file.size_bytes),
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Activity tab: recent lifecycle events.
        let activity: Vec<ActivitySummary> = self.files_state
            .activity_log_rows
            .iter()
            .take(50)
            .map(|row| ActivitySummary {
                label: row.file_label.clone(),
                action: row.action.clone(),
                occurred_at_ms: row.occurred_at_ms,
            })
            .collect();

        Some(DashboardSnapshot {
            active_tab,
            shared_by_me_files,
            downloading,
            downloaded,
            shared_with_me_files,
            activity,
        })
    }

    pub fn update(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        let gui_action_timeout_id = match &message {
            AppMessage::GuiTestActionReceived(action) => Some(action.action_id.clone()),
            _ => None,
        };
        let _timer = PerfTracker::timer("update_msg", Self::log_variant(&message));
        debug!(message = Self::log_variant(&message), "app update");
        let task = match message {
            #[cfg(feature = "dev-ui")]
            AppMessage::Designer(designer_message) => {
                let selection = match &designer_message {
                    DesignerMessage::Select(component) => Some(*component),
                    DesignerMessage::StartDrag { component, .. } => Some(Some(*component)),
                    _ => None,
                };
                if matches!(designer_message, DesignerMessage::StartDrag { .. }) {
                    self.settings_state.designer_history.begin(&self.active_layout);
                }
                if let DesignerMessage::UpdateDrag(point) = designer_message {
                    // The whole-card overlay reports pointer movement. Route
                    // it to whichever gesture is active (a resize drag vs a
                    // home reorder drag); with no gesture active this is a
                    // plain hover and both handlers no-op.
                    if self.settings_state.designer.resize_operation.is_some() {
                        self.update_resize(point);
                    } else {
                        self.update_home_drag(point);
                    }
                    return iced::Task::none();
                }
                if let DesignerMessage::ReorderHome { index, delta } = designer_message {
                    self.reorder_home_from_tree(index, delta);
                    return iced::Task::none();
                }
                if let DesignerMessage::UpdateResize(point) = designer_message {
                    self.update_resize(point);
                    return iced::Task::none();
                }
                if let DesignerMessage::AdjustGridColumns(delta) = designer_message {
                    self.adjust_selected_grid_columns(delta);
                    return iced::Task::none();
                }
                if let DesignerMessage::SetCustomWidth(value) = &designer_message {
                    self.settings_state.designer
                        .update(DesignerMessage::SetCustomWidth(value.clone()));
                    return iced::Task::none();
                }
                if matches!(designer_message, DesignerMessage::CommitDrag) {
                    // The whole-card overlay reports the release. Commit the
                    // active gesture: a resize drag commits the resize
                    // transaction; otherwise commit the home reorder drag.
                    if self.settings_state.designer.resize_operation.is_some() {
                        self.settings_state.designer_history.commit(&self.active_layout);
                        self.settings_state.designer.update(DesignerMessage::CommitResize);
                    } else {
                        self.commit_home_drag();
                        self.settings_state.designer_history.commit(&self.active_layout);
                        self.settings_state.designer.update(DesignerMessage::CommitDrag);
                    }
                    return iced::Task::none();
                }
                if matches!(designer_message, DesignerMessage::CancelDrag) {
                    self.settings_state.designer_history.cancel();
                }
                if let DesignerMessage::StartResize { component, .. } = designer_message {
                    self.settings_state.designer_history.begin(&self.active_layout);
                    self.settings_state.designer.update(DesignerMessage::StartResize {
                        component,
                        origin: iced::Point::ORIGIN,
                    });
                    self.settings_state.designer.selected_component = Some(component);
                    let inspector_component = component.inspector_component();
                    let section = inspector_component.section();
                    self.settings_state.inspect_selected = Some(inspector_component);
                    self.settings_state.inspect_hover = Some(inspector_component);
                    self.settings_state.inspector_draft.collapsed_sections.remove(&section);
                    return iced::Task::none();
                }
                if matches!(designer_message, DesignerMessage::CommitResize) {
                    self.settings_state.designer_history.commit(&self.active_layout);
                    self.settings_state.designer.update(DesignerMessage::CommitResize);
                    return iced::Task::none();
                }
                if matches!(designer_message, DesignerMessage::CancelResize) {
                    // A cancelled gesture must not leave its pre-gesture
                    // snapshot pending.  Otherwise a later unrelated resize
                    // could commit a stale history transaction.
                    self.settings_state.designer_history.cancel();
                    self.settings_state.designer.update(DesignerMessage::CancelResize);
                    return iced::Task::none();
                }
                self.settings_state.designer.update(designer_message);
                if let Some(Some(component)) = selection {
                    let inspector_component = component.inspector_component();
                    let section = inspector_component.section();
                    self.settings_state.inspect_selected = Some(inspector_component);
                    self.settings_state.inspect_hover = Some(inspector_component);
                    self.settings_state.inspector_draft.collapsed_sections.remove(&section);
                    let offset = crate::inspector::section_scroll_offset(
                        section,
                        &self.settings_state.inspector_draft.collapsed_sections,
                    );
                    iced::widget::operation::scroll_to(
                        crate::inspector::INSPECTOR_SCROLL_ID,
                        iced::widget::operation::AbsoluteOffset { x: 0.0, y: offset },
                    )
                } else if selection.is_some() {
                    self.settings_state.inspect_selected = None;
                    self.settings_state.inspect_hover = None;
                    iced::Task::none()
                } else {
                    iced::Task::none()
                }
            }
            // ── Navigation ────────────────────────────────────────────
            AppMessage::GoToChatList => {
                // Dismiss any open video-card overflow menu before leaving.
                self.video_card_menu_open = None;
                // Save current room to history.
                self.save_room_to_history();
                // Update room list preview.
                let name = self
                    .names
                    .get(&self.local_public)
                    .cloned()
                    .unwrap_or_default();
                let preview = self
                    .entries
                    .last()
                    .map(|e| {
                        let t = e.body.clone();
                        if t.len() > 60 {
                            format!("{}…", &t[..60])
                        } else {
                            t
                        }
                    })
                    .unwrap_or_default();
                self.room_history.upsert(self.topic, &name, true);
                if !preview.is_empty() {
                    self.room_history.update_preview(&self.topic, &preview);
                }
                self.room_history_dirty = true;
                self.persist_room_history();

                // Going back to the chat list only changes the UI screen.
                // Keep the room subscription alive so returning is instant
                // and the local peer stays online in the room.
                self.screen = Screen::ChatList;
                if let Some(action_id) = self.pending_chat_list_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                iced::Task::none()
            }

            AppMessage::CreateNewRoom
            | AppMessage::CancelCreateRoom
            | AppMessage::OpenRoomSettings(_)
            | AppMessage::RoomSettingsNameChanged(_)
            | AppMessage::RoomSettingsDescriptionChanged(_)
            | AppMessage::RoomSettingsTagsChanged(_)
            | AppMessage::RoomSettingsVisibilityChanged(_)
            | AppMessage::CancelRoomSettings
            | AppMessage::ConfirmRoomSettings
            | AppMessage::SetRoomDirectoryVisibility { .. } => self.update_rooms(message),
            // ── Groups (state layer) ───────────────────────────────
            AppMessage::ShowCreateGroupDialog
            | AppMessage::HideCreateGroupDialog
            | AppMessage::CreateGroupNameChanged(_)
            | AppMessage::CreateGroupDescriptionChanged(_)
            | AppMessage::CreateGroupMemberToggled(_)
            | AppMessage::CreateGroupSearchChanged(_)
            | AppMessage::ConfirmCreateGroup => self.update_groups(message),
            // ── Tunnels (state layer) ──────────────────────────────
            AppMessage::ShowCreateTunnelDialog
            | AppMessage::CancelCreateTunnel
            | AppMessage::CreateTunnelPortChanged(_)
            | AppMessage::CreateTunnel(_)
            | AppMessage::TunnelRequestReceived { .. }
            | AppMessage::AcceptTunnelRequest(_)
            | AppMessage::DeclineTunnelRequest(_)
            | AppMessage::CloseTunnel(_) => self.update_tunnels(message),

            AppMessage::ImportFriendFromFile | AppMessage::ImportFriendFromFilePicked(_) => {
                self.update_contacts(message)
            }

            AppMessage::CreateNewRoomDhtToggled(_)
            | AppMessage::CreateNewRoomNameChanged(_)
            | AppMessage::CreateNewRoomVisibilityChanged(_)
            | AppMessage::CreateNewRoomDescriptionChanged(_)
            | AppMessage::CreateNewRoomTagsChanged(_)
            | AppMessage::ConfirmCreateNewRoom => self.update_rooms(message),

            AppMessage::OpenRoom(topic) => {
                let _timer = PerfTracker::timer("open_room", format!("topic={topic}"));

                // ── BORU-DISC-13 guard ─────────────────────────────────
                // The internal discovery topic is networking infrastructure,
                // never a conversation. Refuse to open it as a chat room from
                // any caller (UI, MCP OpenRoom action, ticket join, test
                // harness): materializing a ConversationLive here would
                // persist history, seed unread state, and render the
                // discovery mesh as a user chat.
                if boru_core::discovery_topic::topic_kind(topic)
                    == boru_core::discovery_topic::TopicKind::Discovery
                {
                    tracing::warn!(
                        topic = %topic,
                        "refusing to open discovery topic as a conversation room"
                    );
                    return iced::Task::none();
                }

                // A GUI test action is complete only after the normal room
                // opening path has selected the requested topic and rendered
                // the chat screen.  This covers both the cached fast path and
                // the asynchronous subscription path.
                let complete_open_room_action = |this: &mut Self| {
                    if let Some((action_id, expected_topic)) = this.pending_open_room_action.take()
                    {
                        if expected_topic == this.topic
                            && matches!(this.screen, Screen::Chat { topic } if topic == expected_topic)
                        {
                            let _ = this
                                .gui_action_history
                                .set_state(&action_id, GuiActionState::Completed);
                        } else {
                            this.pending_open_room_action = Some((action_id, expected_topic));
                        }
                    }
                };

                // Opening/returning to a conversation always lands at the
                // latest message (standard messenger behaviour).  Force
                // follow-latest and arm the snap so EVERY open path lands at
                // the bottom of the timeline: the already-active re-select,
                // the cached fast path, and the fresh slow path (whose
                // history replay only arms a snap while follow-latest — a
                // previous room left scrolled-up would otherwise leak
                // `follow_latest=false` into the new room's replay).
                self.follow_latest = true;
                self.scroll_to_bottom_pending = true;

                // If the topic is already active and subscribed, just reveal the chat screen
                // without tearing down the subscription.
                // An MCP OpenRoom request is already satisfied when the
                // requested room is selected, even if the test harness (or a
                // just-restored GUI state) has not attached its sender yet.
                // Do not leave the action queued while needlessly starting a
                // second subscription for the already-selected room.
                let pending_selected_room_action = self
                    .pending_open_room_action
                    .as_ref()
                    .is_some_and(|(_, expected_topic)| *expected_topic == topic);
                if topic == self.topic && (self.sender.is_some() || pending_selected_room_action) {
                    self.screen = Screen::Chat { topic };
                    complete_open_room_action(self);
                    // Re-selecting the visible chat (e.g. returning from the
                    // chat list) also lands at the latest message.
                    let replay = self.replay_pending_events_batch(topic);
                    return self.with_pending_snap(replay);
                }

                // Fast path: re-select an already-subscribed conversation from
                // the HashMap. Preserves sender, forwarder, entries, scroll,
                // draft text, and all other per-conversation state.
                if self.switch_to_conversation(topic) {
                    complete_open_room_action(self);
                    let task = if !self.pending_image.is_empty()
                        || !self.pending_thumbnail_fetch.is_empty()
                        || !self.pending_gif.is_empty()
                    {
                        iced::Task::batch([
                            self.replay_pending_events_batch(topic),
                            self.drain_pending_transfers(),
                        ])
                    } else {
                        self.replay_pending_events_batch(topic)
                    };
                    // `switch_to_conversation` armed the snap for THIS update.
                    // The fast path returns early, so the shared update tail
                    // never runs — emit the snap here instead.  Deferring it
                    // to a later update lets a stale Scrolled event (carrying
                    // the previous conversation's offset) cancel the flag and
                    // strand the viewport away from the bottom.
                    return self.with_pending_snap(task);
                }

                // Slow path: first-time subscription to this topic.
                // Guard against duplicate OpenRoom calls for the same topic:
                // whisper empty connection DMs, control messages, and gossip
                // RoomOpened can all fire in quick succession, each triggering
                // a separate subscription and resetting the chat view.
                if self.pending_topic == Some(topic) {
                    info!("OpenRoom: already opening topic={topic}, skipping duplicate");
                    return iced::Task::none();
                }
                self.pending_topic = Some(topic);
                // Bump the room generation so a stale RoomOpened from an
                // earlier join cannot clobber a newer room the user opened
                // while this subscription was in flight.
                self.room_generation = self.room_generation.wrapping_add(1);
                let room_snapshot = RoomSnapshot {
                    topic,
                    generation: self.room_generation,
                };

                // Save the current room first
                self.save_room_to_history();
                // Update room list preview for previous room
                let name = self
                    .names
                    .get(&self.local_public)
                    .cloned()
                    .unwrap_or_default();
                let preview = self
                    .entries
                    .last()
                    .map(|e| {
                        let t = e.body.clone();
                        if t.len() > 60 {
                            format!("{}…", &t[..60])
                        } else {
                            t
                        }
                    })
                    .unwrap_or_default();
                self.room_history.upsert(self.topic, &name, true);
                if !preview.is_empty() {
                    self.room_history.update_preview(&self.topic, &preview);
                }
                self.room_history_dirty = true;
                self.leave_current_room();

                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let personal_topic = self.personal_room_topic();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let endpoint = self.endpoint.clone();
                let runtime_handle = self.runtime_handle.clone();
                let memory_lookup = self.memory_lookup.clone();
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.files_state.download_progress_queue.clone();
                let profile_image_ticket = self.settings_state.profile_image_ticket.clone();
                let private_dht_disabled = self.private_dht_disabled;
                let dht = self.dht.clone();
                // Preserve a persisted private-room discovery secret when reopening
                // a room from the chat list.
                let saved_discovery_secret = RoomStore::load_or_none(&data_dir)
                    .filter(|room| room.topic == topic)
                    .and_then(|room| room.discovery_secret);
                // or from the saved RoomStore for this topic.
                let initial_addrs: Vec<EndpointAddr> =
                    self.initial_bootstrap_peers.drain(..).collect();
                let saved_addrs = RoomStore::load_or_none(&data_dir)
                    .filter(|room| room.topic == topic)
                    .map(|room| room.peers)
                    .unwrap_or_default();
                let (mut bootstrap_peers, initial_addrs) =
                    collect_bootstrap_peers([&initial_addrs, &saved_addrs]);
                let initial_addrs_for_save = initial_addrs.clone();
                let direct_conversation = self.friends.iter().any(|(_, record)| {
                    // Include mDNS / DHT-discovered LAN peers as bootstrap addresses
                    // so the room subscription can connect to them directly instead
                    // of waiting for a peer-to-peer discovery exchange on the new
                    // topic.  Discovered peers are ID-only (no transport info
                    // needed — the endpoint's address lookup chain handles
                    // resolution), so we wrap them in a bare EndpointAddr.
                    // Filter out our own identity — discovered_peers may
                    // transiently include it from a discovery source that
                    // doesn't self-filter (e.g. discovery bootstrap exchange).
                    let discovered_bootstrap_addrs: Vec<EndpointAddr> = self
                        .discovered_peers
                        .iter()
                        .filter(|&&pk| pk != self.local_public)
                        .map(|&pk| EndpointAddr::new(pk))
                        .collect();
                    // Merge discovered peers into the bootstrap list so they are
                    // also passed to gossip.subscribe() for the new room topic.
                    for addr in &discovered_bootstrap_addrs {
                        if !bootstrap_peers.contains(&addr.id) {
                            bootstrap_peers.push(addr.id);
                        }
                    }
                    // Persist bootstrap peers for reconnection.
                    let peers_file = data_dir.join("peers.json");
                    if let Err(error) = std::fs::write(
                        &peers_file,
                        serde_json::to_string(&bootstrap_peers).unwrap_or_default(),
                    ) {
                        warn!(?error, "failed to persist bootstrap peers");
                    }
                    record
                        .direct_conversation
                        .as_ref()
                        .is_some_and(|conversation| conversation.topic == topic)
                });
                let share_direct_addresses = self.settings_state.share_direct_addresses;
                // Show a loading spinner while the gossip subscription is in flight.
                self.room_loading = true;
                self.push_mesh_event("Connecting to room...");

                iced::Task::perform(
                    async move {
                        info!("OpenRoom task: ENTERED async block");
                        info!("OpenRoom task: starting subscribe topic={topic}");
                        // Seed the endpoint address lookup with bootstrap peer
                        // addresses so the endpoint can resolve them by their
                        // transport info (relay URL, direct addresses) from the
                        // ticket or RoomStore — not just by public key.
                        seed_memory_lookup(&memory_lookup, &initial_addrs);
                        info!("OpenRoom task: memory_lookup seeded");
                        // Wait for at least one gossip neighbor if we have bootstrap
                        // peers — matching the TUI behavior.  Without bootstrap
                        // peers (room creator) use subscribe() so we don't hang.
                        // Stale bootstrap peers are protected by a 30s timeout
                        // to avoid blocking the UI indefinitely.
                        // Run gossip subscription on the dedicated Tokio runtime.
                        // Calling it directly from an Iced task can leave the room
                        // marked subscribed while the gossip handshake never starts.
                        info!("OpenRoom task: about to spawn subscription");
                        let sub: GossipTopic = runtime_handle
                            .spawn(async move {
                                if direct_conversation || bootstrap_peers.is_empty() {
                                    gossip
                                        .subscribe(topic, bootstrap_peers)
                                        .await
                                        .map_err(|e| e.to_string())
                                } else {
                                    let peers = bootstrap_peers.clone();
                                    match tokio::time::timeout(Duration::from_secs(30), async {
                                        gossip.subscribe_and_join(topic, peers).await
                                    })
                                    .await
                                    {
                                        Ok(Ok(sub)) => Ok(sub),
                                        Ok(Err(e)) => Err(e.to_string()),
                                        Err(_elapsed) => {
                                            // subscribe_and_join timed out — fall
                                            // back to basic subscribe so the room
                                            // is at least open.  Peers can join the
                                            // mesh later via NeighborUp events.
                                            info!(
                                                topic = %topic,
                                                "subscribe_and_join timed out; falling back to subscribe"
                                            );
                                            gossip
                                                .subscribe(topic, bootstrap_peers)
                                                .await
                                                .map_err(|e| e.to_string())
                                        }
                                    }
                                }
                            })
                            .await
                            .map_err(|e| format!("room subscription task failed: {e}"))??;
                        let (sender, receiver) = sub.split();
                        let neighbor_ids: Vec<PublicKey> = receiver.neighbors().collect();
                        let neighbor_count = neighbor_ids.len();
                        let local_peer_addr = invitation_endpoint_addr(
                            endpoint.watch_addr().get(),
                            share_direct_addresses,
                        );

                        let room_tracker = if !private_dht_disabled {
                            // Clone the secret for the long-lived tracker; the
                            // original is persisted to the RoomStore below.
                            if let (Some(secret), Some(dht)) =
                                (saved_discovery_secret.clone(), dht.clone())
                            {
                                let backend = MainlineDhtBackend::new(dht);
                                let tracker = PrivateRoomTracker::new(
                                    Box::new(backend),
                                    topic,
                                    secret,
                                    endpoint.id(),
                                    endpoint.secret_key().clone(),
                                );
                                let (new_peers_tx, new_peers_rx) =
                                    tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
                                let join_cancel = tokio_util::sync::CancellationToken::new();
                                let _join_task =
                                    boru_core::public_room_continuous::spawn_join_fanout(
                                        new_peers_rx,
                                        sender.clone(),
                                        join_cancel.clone(),
                                    );
                                Some(SharedTracker::new(
                                    PrivateContinuousTracker::start(
                                        tracker,
                                        ContinuousTrackerConfig::default(),
                                        new_peers_tx,
                                    ),
                                    join_cancel,
                                ))
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        // Clone the secret into the invitation ticket; the
                        // original is persisted to the RoomStore below.
                        let room_secret = saved_discovery_secret.clone();
                        let ticket_str = Ticket {
                            topic,
                            peers: vec![local_peer_addr.clone()],
                            discovery_secret: room_secret,
                        }
                        .to_string();
                        let _personal_ticket = Ticket {
                            topic: personal_topic,
                            peers: vec![local_peer_addr.clone()],
                            discovery_secret: None,
                        }
                        .to_string();

                        let metadata_doc = room_docs::create_metadata_doc(
                            topic,
                            &sender,
                            RoomMetadata {
                                name: Some("boru-chat".to_string()),
                                description: None,
                                rules: None,
                            },
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        let roster_doc = room_docs::create_roster_doc(
                            topic,
                            &sender,
                            sk.public().to_string(),
                            label.clone(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                        let forward_handle = spawn_conversation_forwarder(
                            topic,
                            metadata_doc,
                            roster_doc,
                            receiver,
                            net_tx,
                            None,
                        );
                        *forward_handle_slot.lock().unwrap() = Some(forward_handle);

                        // Broadcast our presence (AboutMe + periodic Presence/Heartbeat
                        // handled by ConnMonitorTick).
                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe {
                                name: label,
                                profile_image_ticket,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;
                        let presence =
                            SignedMessage::sign_and_encode(&sk, &crate::Message::Presence)
                                .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(presence).await;
                        info!("OpenRoom task: broadcasts complete");

                        let saved_peers = if initial_addrs_for_save.is_empty() {
                            vec![local_peer_addr]
                        } else {
                            initial_addrs_for_save.clone()
                        };
                        let mut room = RoomStore::with_peers(&data_dir, topic, saved_peers);
                        room.discovery_secret = saved_discovery_secret;

                        Ok::<
                            (
                                GossipSender,
                                TopicId,
                                String,
                                Option<SharedTracker>,
                                usize,
                                Vec<PublicKey>,
                            ),
                            String,
                        >((
                            sender,
                            topic,
                            ticket_str,
                            room_tracker,
                            neighbor_count,
                            neighbor_ids,
                        ))
                    },
                    move |result| match result {
                        Ok((
                            sender,
                            topic,
                            ticket_str,
                            room_tracker,
                            neighbor_count,
                            neighbor_ids,
                        )) => AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                            sender,
                            room_tracker,
                            neighbor_count,
                            neighbor_ids,
                            generation: room_snapshot.generation,
                        },
                        Err(e) => AppMessage::RoomJoinFailed {
                            error: e,
                            generation: room_snapshot.generation,
                        },
                    },
                )
            }

            AppMessage::RoomOpened {
                topic,
                ticket,
                sender,
                room_tracker,
                neighbor_count,
                neighbor_ids,
                generation,
            } => {
                // ── BORU-DISC-13 guard (defense in depth) ─────────────
                // Even if an OpenRoom task for the discovery topic somehow
                // completed (e.g. a stale in-flight subscription spawned
                // before the OpenRoom guard shipped), never materialize the
                // discovery mesh as a conversation: no ConversationLive, no
                // Screen::Chat, no history replay, no room-history upsert,
                // no backfill deferral, no unread state.
                if boru_core::discovery_topic::topic_kind(topic)
                    == boru_core::discovery_topic::TopicKind::Discovery
                {
                    tracing::warn!(
                        topic = %topic,
                        "dropping RoomOpened for discovery topic"
                    );
                    return iced::Task::none();
                }
                // Dismiss any open video-card overflow menu when the room
                // changes (entry indices are not stable across rooms).
                self.video_card_menu_open = None;
                // If this completion settles an in-flight create-room submit,
                // close the dialog and clear the loading state.
                if self.rooms_state.create_room_submitting {
                    self.rooms_state.create_room_submitting = false;
                    self.rooms_state.show_create_room_dialog = false;
                    self.rooms_state.create_room_error = None;
                }
                info!("RoomOpened FIRED topic={topic} neighbor_count={neighbor_count}");
                // State-safety: this completion was spawned under
                // `generation`. If the user has since initiated a newer
                // join (which bumps `room_generation`), this is a stale
                // completion for a superseded room — drop it.
                if self.room_generation != generation {
                    warn!(
                        "stale RoomOpened for {topic}: completion generation {generation} \
                         != current room generation {current}",
                        current = self.room_generation,
                    );
                    return iced::Task::none();
                }
                // A new active conversation is being applied — bump the
                // conversation ownership token so in-flight image downloads
                // started for the previous conversation are detected.
                self.conversation_generation = self.conversation_generation.wrapping_add(1);
                self.reset_typing_state();
                self.pending_topic = None;
                self.room_loading = false;
                self.sender = Some(sender.clone());
                if neighbor_count > 0 {
                    self.push_mesh_event(format!(
                        "Connected to room — {neighbor_count} peer{} online",
                        if neighbor_count == 1 { "" } else { "s" },
                    ));
                } else {
                    self.push_mesh_event("Connected to room — waiting for peers...");
                }
                // Subscription creation is not proof that the room has a
                // usable route.  However, the gossip protocol may have already
                // discovered neighbors before the forwarder was spawned (common
                // on startup).  If so, mark the sender as ready immediately so
                // that the first message send does not stall.
                self.sender_ready = neighbor_count > 0;
                self.room_neighbor_counts
                    .insert(topic, neighbor_count as u32);

                // Retroactively emit diagnostic events for neighbors that were
                // already connected before the forwarder started. When NeighborUp
                // fires during gossip bootstrap (before the forwarder is spawned),
                // the app misses ConnectionEstablished/PeerDiscovered/etc. Emit
                // them now so the diagnostics layer transitions from Connecting
                // to Connected without requiring a manual chat click.
                for &peer in &neighbor_ids {
                    if peer != self.local_public {
                        // Seed the active conversation's neighbor set from
                        // neighbors known at subscription time, so the first
                        // send after a slow-path open does not stall (same
                        // gate as the NeighborUp sync above).
                        self.neighbors.insert(peer);
                        DIAGNOSTICS.record_with_peer(
                            Some(topic),
                            Some(peer.to_string()),
                            DiagnosticEventKind::PeerDiscovered,
                        );
                        DIAGNOSTICS.record_with_peer(
                            Some(topic),
                            Some(peer.to_string()),
                            DiagnosticEventKind::ConnectionEstablished {
                                remote_address: None,
                                transport: None,
                                used_relay: None,
                            },
                        );
                        DIAGNOSTICS.record_with_peer(
                            Some(topic),
                            Some(peer.to_string()),
                            DiagnosticEventKind::RoomSubscriptionJoined,
                        );
                        DIAGNOSTICS.record_with_peer(
                            Some(topic),
                            Some(peer.to_string()),
                            DiagnosticEventKind::PeerAddedToTopic,
                        );
                        DIAGNOSTICS.record_with_peer(
                            Some(topic),
                            Some(peer.to_string()),
                            DiagnosticEventKind::PeerJoinedRoom,
                        );
                    }
                }

                self.forward_handle = self.forward_handle_slot.lock().unwrap().take();

                // Store continuous tracker if one was provided (private room with DHT).
                if let Some(tracker) = room_tracker {
                    self.rooms_state.room_trackers.insert(topic, tracker);
                }

                // Auto-advertise a discoverable room opened right after
                // creation.  This path is reached from RoomOpened for both
                // private and public rooms; the ConfirmCreateNewRoom public
                // branch has already inserted the room into advertised_rooms,
                // so this re-insert is idempotent.  It also lazily subscribes
                // the directory topic so periodic advertisements flow.
                if self.rooms_state.create_room_visibility == RoomVisibility::PublicDiscoverable {
                    self.rooms_state.advertised_rooms.insert(topic);
                    info!(%topic, "auto-advertising new room in directory");
                    self.rooms_state.create_room_visibility = RoomVisibility::Private;
                    if self.directory_sender.is_none() {
                        return iced::Task::done(AppMessage::SubscribeDirectoryTopic);
                    }
                }

                // Unarchive or create the conversation entry so the room
                // appears in the CHATS sidebar after joining.  Public rooms
                // created via "Advertise in Directory" start archived and
                // need to be surfaced here.
                if let Some(entry) = self.conversation_store.find_mut(&topic) {
                    if entry.archived {
                        entry.archived = false;
                        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                    }
                } else {
                    // BORU-DIR-16 (PDF Task 6.1 step 5): a successful join
                    // from the directory creates the local conversation
                    // record exactly once. The control-plane directory
                    // never materializes entries on discovery (PDF Core
                    // rule: "Do not persist discovered rooms as
                    // conversations until the user actually joins"), so
                    // there is nothing to unarchive here — create the
                    // record now that the join succeeded, using the
                    // advertised metadata. If the room is not a directory
                    // room (e.g. legacy ticket join with no prior entry),
                    // this is a no-op.
                    self.ensure_directory_joined_record(topic);
                }

                // Record RoomJoined diagnostic event so diagnostic evidence
                // and MCP room-membership checks reflect the active subscription.
                DIAGNOSTICS.record(Some(topic), DiagnosticEventKind::RoomJoined);

                // BORU-DISC-20: log explicit room joins independently for
                // direct vs group topics and bump the matching counter, so
                // debugging can prove which conversation topics were joined
                // (separate from the discovery-topic join in main.rs).
                match self.conversation_store.find(&topic).map(|e| &e.kind) {
                    Some(ConversationKind::Direct) => {
                        boru_core::diagnostics::DIAGNOSTIC_COUNTERS.record_direct_topic_joined();
                        info!(topic = %topic, "room opened: direct conversation topic joined");
                    }
                    _ => {
                        boru_core::diagnostics::DIAGNOSTIC_COUNTERS.record_group_topic_joined();
                        info!(topic = %topic, "room opened: group conversation topic joined");
                    }
                }

                self.screen = Screen::Chat { topic };
                self.topic = topic;
                self.ticket_str = ticket.clone();
                self.entries.clear();
                self.event_id_to_index.clear();
                self.message_hash_to_index.clear();
                self.layout_cache.borrow_mut().clear();
                self.names.clear();
                self.composer_text.clear();
                self.first_run = false; // First action taken — onboarding complete

                // Auto-subscribe to all stored conversations so messages
                // can be received even before the user opens each chat.
                // Dispatch BackgroundSubscribe so the sender is properly
                // stored in self.conversations via BackgroundSubscribed.
                // Only skip topics with a LIVE sender (already actively
                // subscribed) or a subscription already in flight — a
                // conversation merely being loaded must never suppress
                // subscription (persistent state ≠ live network state).
                let mut bg_topics: std::collections::BTreeSet<TopicId> = self
                    .conversation_store
                    .active_iter()
                    .into_iter()
                    .map(|e| e.topic)
                    .filter(|t| {
                        *t != topic
                            && !self
                                .conversations
                                .get(t)
                                .is_some_and(|c| c.sender.is_some())
                            && !self.background_subscriptions_in_flight.contains(t)
                    })
                    .collect();
                // Also auto-subscribe to deterministic direct-chat topics
                // for every known peer.  Direct-conversation state is not
                // guaranteed to be symmetric or persisted on both sides
                // when a whisper ConversationInvite fails, but the topic is
                // deterministic from the two peer IDs.  Restricting this to
                // `DirectConversationState::Active` left one side unable to
                // receive messages after a restart.
                {
                    let local_pk = self.local_public;
                    for (fid, _) in self.friends.iter() {
                        if let Some(peer_pk) = fid.parse_public_key().ok() {
                            let direct_topic = direct_topic(&local_pk, &peer_pk);
                            if direct_topic != topic
                                && !self.conversations.contains_key(&direct_topic)
                            {
                                bg_topics.insert(direct_topic);
                            }
                        }
                    }
                }
                let mut bg_tasks: Vec<iced::Task<AppMessage>> = Vec::new();
                if !bg_topics.is_empty() {
                    let bootstrap_peers: Vec<PublicKey> = self.discovered_peers.clone();
                    info!(
                        count = bg_topics.len(),
                        bootstrap = bootstrap_peers.len(),
                        "auto-subscribing to stored conversations"
                    );
                    bg_tasks = bg_topics
                        .into_iter()
                        .map(|bg_topic| {
                            iced::Task::done(AppMessage::BackgroundSubscribe(
                                bg_topic,
                                bootstrap_peers.clone(),
                            ))
                        })
                        .collect();
                }
                self.push_system("Chat joined.");
                self.push_system("Type a message and press Enter to send.  /help for commands.");

                // If the ticket contains a discovery secret, also display a
                // stable boru1: invitation (no endpoint info, compact format).
                if let Ok(t) = ticket.parse::<Ticket>() {
                    if let Some(secret) = t.discovery_secret {
                        let invite = RoomInviteV2::new(t.topic, secret);
                        self.push_system(format!(
                            "Invite to join this room (boru1): {}",
                            invite.encode()
                        ));
                    }
                }

                // Load persisted history and replay it into the UI. SQLite's
                // `messages` table is authoritative; the JSON store is only
                // imported here for data directories created before the
                // SQLite history migration.
                {
                    let local_hex = self.local_public.to_string();
                    let legacy_entries: Vec<HistoryEntry> = self
                        .chat_history
                        .lock()
                        .unwrap()
                        .for_topic(&topic)
                        .into_iter()
                        .cloned()
                        .collect();
                    let store_path = self.data_dir.join("message_store.db");
                    let mut sqlite_rows = MessageStore::open(&store_path)
                        .and_then(|store| {
                            let mut rows =
                                store.get_messages_for_topic(topic.as_bytes(), 1_000_000, 0)?;
                            if rows.is_empty() {
                                // One-time, idempotent import of the legacy JSON
                                // mirror. INSERT OR IGNORE makes retries safe.
                                for entry in &legacy_entries {
                                    let hash_vec = hex::decode(&entry.hash).unwrap_or_default();
                                    let hash = if hash_vec.len() == 32 {
                                        let mut value = [0u8; 32];
                                        value.copy_from_slice(&hash_vec);
                                        value
                                    } else {
                                        *blake3::hash(&entry.signed_bytes).as_bytes()
                                    };
                                    let sender = PublicKey::from_str(&entry.sender)
                                        .map(|key| *key.as_bytes())
                                        .unwrap_or([0u8; 32]);
                                    store.insert_chat_message(
                                        &hash,
                                        entry.topic.as_bytes(),
                                        &sender,
                                        entry.timestamp,
                                        &entry.kind,
                                        &entry.text_preview,
                                        Some(&entry.signed_bytes),
                                        entry.image_identifier.as_deref(),
                                        self.local_public.as_bytes(),
                                    )?;
                                }
                                rows =
                                    store.get_messages_for_topic(topic.as_bytes(), 1_000_000, 0)?;
                            }
                            Ok(rows)
                        })
                        .unwrap_or_default();
                    for row in &sqlite_rows {
                        if let Some(chat_entry) =
                            Self::chat_message_row_to_chat_entry(row, &local_hex)
                        {
                            self.entries_push(chat_entry);
                        }
                    }
                    // Legacy rows with event_id == 0 are valid migration input,
                    // but must not hide or replace successfully imported SQLite
                    // history. They are used only if SQLite has no rows.
                    if sqlite_rows.is_empty() {
                        for hist_entry in &legacy_entries {
                            if let Some(chat_entry) =
                                self.history_entry_to_chat_entry(hist_entry, &topic, &local_hex)
                            {
                                self.entries_push(chat_entry);
                            }
                        }
                    }
                    self.history_saved_count = self.entries.len();

                    // Overlay the durable event-id delivery state for locally
                    // composed messages. The message-store row id is not the
                    // outgoing_messages event id.
                    if let Some(storage) = &self.storage {
                        if let Ok(outgoing) = storage.list_outgoing_for_topic(&topic) {
                            for row in outgoing {
                                if let Some(index) = self.entries.iter().position(|entry| {
                                    entry.message_hash
                                        == hex::decode(&row.hash)
                                            .ok()
                                            .and_then(|bytes| bytes.try_into().ok())
                                }) {
                                    if let Some(entry) = self.entries.get_mut(index) {
                                        entry.event_id = row.event_id;
                                        entry.delivery_state = match row.delivery_state.as_str() {
                                            "sent" => DeliveryState::Sent,
                                            "delivered" => DeliveryState::Delivered,
                                            "seen" => DeliveryState::Seen,
                                            "failed" => DeliveryState::Failed,
                                            _ => DeliveryState::Queued,
                                        };
                                        entry.bump_gen();
                                    }
                                }
                            }
                            self.rebuild_entry_indexes();
                        }
                    }
                }

                // Overlay outgoing delivery states from SQLite onto the
                // ChatEntries so the GUI shows delivery indicators from the
                // durable outgoing_messages table, not chat_history.json.
                // This is the Phase 10 replacement for reading outbox.json.
                if let Some(storage) = &self.storage {
                    if let Ok(rows) = storage.list_outgoing_for_topic(&topic) {
                        for row in &rows {
                            if let Some(&index) = self.event_id_to_index.get(&row.event_id) {
                                if let Some(entry) = self.entries.get_mut(index) {
                                    let state = match row.delivery_state.as_str() {
                                        "queued" => DeliveryState::Queued,
                                        "sent" => DeliveryState::Sent,
                                        "delivered" => DeliveryState::Delivered,
                                        "seen" => DeliveryState::Seen,
                                        "failed" => DeliveryState::Failed,
                                        _ => DeliveryState::Queued,
                                    };
                                    if entry.delivery_state != state {
                                        entry.delivery_state = state;
                                        entry.bump_gen();
                                    }
                                }
                            }
                        }
                    }
                }

                // Replay queued or previously-sent messages after a reconnect.
                // The signed bytes are reused verbatim, so retries cannot create
                // a second logical message or invalidate message-hash dedup.
                let replay = if let Some(storage) = &self.storage {
                    match storage.list_pending_outgoing_for_topic(&topic) {
                        Ok(rows) => rows
                            .into_iter()
                            .map(|row| (row.event_id, row.signed_bytes))
                            .collect(),
                        Err(_) => Vec::new(),
                    }
                } else {
                    Vec::new()
                };
                if !replay.is_empty() {
                    let sender = sender.clone();
                    let ids = replay.iter().map(|(id, _)| *id).collect::<Vec<_>>();
                    for id in &ids {
                        let _ = self
                            .chat_history
                            .lock()
                            .unwrap()
                            .update_delivery_state(*id, DeliveryState::Sent);
                    }
                    // After delivering, try the next pending
                    task::spawn(async move {
                        for (_, bytes) in replay {
                            let _ = sender.broadcast(bytes.into()).await;
                        }
                    });
                }

                // Pins are references and must be reloaded independently of
                // message backfill: a pin may arrive before its message.
                self.reload_pins_for_topic(topic);

                // ── Backfill: defer request until a gossip neighbor connects ──
                // If we have very few history entries for this topic, we likely
                // missed messages sent while offline.  Request them from a neighbor
                // via the backfill QUIC protocol once the gossip mesh has formed.
                // We defer because RoomOpened fires before NeighborUp events have
                // been processed through the iced event loop — checking
                // self.neighbors here would race and usually find it empty.
                if self.entries.len() < BACKFILL_TRIGGER_THRESHOLD {
                    if !self.pending_backfill_topics.contains(&topic) {
                        self.pending_backfill_topics.push(topic);
                    }
                    debug!(
                        topic = %topic,
                        entries = self.entries.len(),
                        "backfill: deferred — waiting for gossip neighbor"
                    );
                }

                // Update room history
                self.room_history.upsert(topic, &self.local_label, true);
                self.room_history_dirty = true;
                self.persist_room_history();

                if self
                    .pending_open_room_action
                    .as_ref()
                    .is_some_and(|(_, expected)| *expected == topic)
                    && matches!(self.screen, Screen::Chat { topic: selected } if selected == topic)
                {
                    if let Some((action_id, _)) = self.pending_open_room_action.take() {
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageHandled);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Completed);
                    }
                }

                if let Some((action_id, expected_peer)) =
                    self.pending_open_conversation_action.take()
                {
                    let expected_topic = direct_topic(&self.local_public, &expected_peer);
                    if expected_topic == topic
                        && matches!(self.screen, Screen::Chat { topic: selected } if selected == topic)
                    {
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageHandled);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Completed);
                    } else {
                        self.pending_open_conversation_action = Some((action_id, expected_peer));
                    }
                }

                if self.return_to_chat_list_after_open {
                    self.return_to_chat_list_after_open = false;
                    let task = iced::Task::done(AppMessage::GoToChatList);
                    let replay_task = self.replay_pending_events_batch(topic);
                    if bg_tasks.is_empty() {
                        return iced::Task::batch([task, replay_task]);
                    }
                    let mut all: Vec<iced::Task<AppMessage>> = bg_tasks;
                    all.push(task);
                    all.push(replay_task);
                    return iced::Task::batch(all);
                }

                let replay_task = self.replay_pending_events_batch(topic);
                if bg_tasks.is_empty() {
                    replay_task
                } else {
                    let mut all: Vec<iced::Task<AppMessage>> = bg_tasks;
                    all.push(replay_task);
                    iced::Task::batch(all)
                }
            }

            AppMessage::RoomJoinFailed { error, generation } => {
                // If the failure came from an in-flight create-room or
                // create-group submit, keep that dialog open and surface the
                // error inline so the user can retry or cancel.
                if self.create_group_submitting {
                    self.create_group_submitting = false;
                    self.create_group_error = Some(format!("Group creation failed: {error}"));
                    return iced::Task::none();
                }
                if self.rooms_state.create_room_submitting {
                    self.rooms_state.create_room_submitting = false;
                    self.rooms_state.create_room_error = Some(format!("Room creation failed: {error}"));
                    return iced::Task::none();
                }
                // State-safety: a stale join failure for a superseded room
                // must not yank the UI out of a newer room the user opened
                // while the failed join was in flight. Detect in debug builds.
                debug_assert_eq!(
                    self.room_generation, generation,
                    "stale RoomJoinFailed: completion generation {generation} \
                     != current room generation {}",
                    self.room_generation,
                );
                self.pending_topic = None;
                self.room_loading = false;
                if let Some((action_id, expected_topic)) = self.pending_open_room_action.take() {
                    let _ = self.gui_action_history.set_error(
                        &action_id,
                        GuiActionError::new(
                            GuiActionErrorCode::InternalError,
                            format!("Failed to open room {expected_topic}: {error}"),
                        ),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Failed);
                }
                self.chat_list_error = format!("Failed to join room: {error}");
                self.screen = Screen::ChatList;
                iced::Task::none()
            }

            AppMessage::JoinFromTicket => {
                // Validate before leaving the current room or starting an
                // asynchronous task.  Previously an empty/malformed field
                // was parsed inside the task, so clicking the button gave no
                // immediate feedback and looked like a no-op.
                let ticket_input = self.join_ticket_input.trim();
                if ticket_input.is_empty() {
                    self.chat_list_error = "Paste a ticket before joining a room.".to_string();
                    self.screen = Screen::ChatList;
                    return iced::Task::none();
                }
                let ticket = match RoomInvitation::parse(ticket_input) {
                    Ok(RoomInvitation::Stable(invite)) => Ticket {
                        topic: invite.topic,
                        peers: Vec::new(),
                        discovery_secret: Some(invite.discovery_secret),
                    },
                    Ok(RoomInvitation::Legacy(ticket)) => ticket,
                    Err(e) => {
                        self.chat_list_error = format!("Invalid ticket: {e}");
                        self.screen = Screen::ChatList;
                        return iced::Task::none();
                    }
                };

                // Show progress while subscribe_and_join waits for the
                // bootstrap peer.  Any connection error is converted to
                // RoomJoinFailed below and rendered in this same location.
                self.chat_list_error = "Joining room…".to_string();
                self.save_room_to_history();
                self.persist_room_history();
                self.leave_current_room();
                // Bump the room generation so a stale join completion for a
                // superseded ticket cannot clobber a newer room the user
                // opened while the join was in flight.
                self.room_generation = self.room_generation.wrapping_add(1);
                let room_snapshot = RoomSnapshot {
                    topic: ticket.topic,
                    generation: self.room_generation,
                };
                let gossip = self.gossip.clone();
                let runtime_handle = self.runtime_handle.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let personal_topic = self.personal_room_topic();
                let endpoint = self.endpoint.clone();
                let memory_lookup = self.memory_lookup.clone();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.files_state.download_progress_queue.clone();
                let profile_image_ticket = self.settings_state.profile_image_ticket.clone();
                let private_dht_disabled = self.private_dht_disabled;
                let dht = self.dht.clone();
                let share_direct_addresses = self.settings_state.share_direct_addresses;
                // Show a loading spinner while the gossip subscription is in flight.
                self.room_loading = true;

                iced::Task::perform(
                    async move {
                        let topic = ticket.topic;
                        let secret = ticket.discovery_secret;
                        let mut room_tracker: Option<SharedTracker> = None;
                        let mut pending_dht_fanout = None;
                        let saved_addrs = RoomStore::load_or_none(&data_dir)
                            .filter(|room| room.topic == topic)
                            .map(|room| room.peers)
                            .unwrap_or_default();

                        // ── DHT discovery for private-room tickets ──────
                        // If the ticket includes a discovery secret, attempt
                        // to find additional peers via the DHT before
                        // subscribing.  Non-fatal errors are silently
                        // downgraded to a fallback (ticket peers only).
                        let ticket_addrs = ticket.peers.clone();
                        let mut merged_peers: Vec<EndpointAddr> = {
                            let (mut ids, addrs) =
                                collect_bootstrap_peers([&ticket.peers, &saved_addrs]);
                            // include room addrs in peer list
                            ids.extend(addrs.iter().map(|a| a.id));
                            // deduplicate back — collect_bootstrap_peers returns
                            // deduped IDs but we need EndpointAddrs, rebuild
                            let mut seen = HashSet::new();
                            let mut result = Vec::new();
                            for a in ticket.peers.iter().chain(saved_addrs.iter()) {
                                if seen.insert(a.id) {
                                    result.push(a.clone());
                                }
                            }
                            result
                        };
                        // Seed MemoryLookup from ticket addresses only (DHT
                        // returns IDs, not addrs).
                        seed_memory_lookup(&memory_lookup, &ticket_addrs);

                        if !private_dht_disabled {
                            if let Some(secret) = secret {
                                let dht = dht.unwrap_or_else(|| {
                                    distributed_topic_tracker::Dht::new(
                                        &distributed_topic_tracker::DhtConfig::default(),
                                    )
                                });
                                let backend = MainlineDhtBackend::new(dht.clone());
                                let tracker = PrivateRoomTracker::new(
                                    Box::new(backend),
                                    topic,
                                    secret,
                                    endpoint.id(),
                                    sk.clone(),
                                );
                                match tracker.discover_once().await {
                                    Ok(discovered_ids) => {
                                        let existing: HashSet<iroh::EndpointId> =
                                            merged_peers.iter().map(|a| a.id).collect();
                                        for id in discovered_ids {
                                            if !existing.contains(&id) && id != endpoint.id() {
                                                merged_peers.push(EndpointAddr::new(id));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            error = %e,
                                            "DHT discovery failed, falling back to ticket peers"
                                        );
                                    }
                                }
                                let (new_peers_tx, new_peers_rx) =
                                    tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
                                let join_cancel = tokio_util::sync::CancellationToken::new();
                                pending_dht_fanout = Some((new_peers_rx, join_cancel.clone()));
                                room_tracker = Some(SharedTracker::new(
                                    PrivateContinuousTracker::start(
                                        tracker,
                                        ContinuousTrackerConfig::default(),
                                        new_peers_tx,
                                    ),
                                    join_cancel,
                                ));
                            }
                        } else {
                            debug!("private room DHT disabled by --no-dht; using ticket peers");
                        }

                        let peers: Vec<iroh::EndpointId> =
                            merged_peers.iter().map(|a| a.id).collect();

                        // Use subscribe_and_join so we wait for at least one gossip
                        // neighbor to connect before proceeding — matching the TUI
                        // behavior.  If no bootstrap peers are given (unlikely here
                        // since JoinFromTicket always has ticket.peers) fall back to
                        // subscribe() to avoid hanging forever.
                        // Run gossip subscription on the dedicated Tokio
                        // runtime. Iced tasks are polled by the GUI executor;
                        // doing the handshake there can leave a ticket join
                        // marked as ready without allowing gossip to progress.
                        let sub = runtime_handle
                            .spawn(async move {
                                tokio::time::timeout(Duration::from_secs(30), async {
                                    if peers.is_empty() {
                                        gossip.subscribe(topic, peers).await
                                    } else {
                                        gossip.subscribe_and_join(topic, peers).await
                                    }
                                })
                                .await
                                .map_err(|_| {
                                    "timed out waiting for a peer to join the room".to_string()
                                })
                                .and_then(|result| result.map_err(|e| e.to_string()))
                            })
                            .await
                            .map_err(|e| format!("room subscription task failed: {e}"))??;
                        let (sender, receiver) = sub.split();
                        let neighbor_ids: Vec<PublicKey> = receiver.neighbors().collect();
                        let neighbor_count = neighbor_ids.len();
                        if let Some((new_peers_rx, join_cancel)) = pending_dht_fanout {
                            let _join_task = boru_core::public_room_continuous::spawn_join_fanout(
                                new_peers_rx,
                                sender.clone(),
                                join_cancel,
                            );
                        }
                        let local_peer_addr = invitation_endpoint_addr(
                            endpoint.watch_addr().get(),
                            share_direct_addresses,
                        );
                        let new_ticket = Ticket {
                            topic,
                            peers: vec![local_peer_addr.clone()],
                            discovery_secret: None,
                        };
                        let ticket_str = new_ticket.to_string();
                        let personal_ticket = Ticket {
                            topic: personal_topic,
                            peers: vec![local_peer_addr.clone()],
                            discovery_secret: None,
                        }
                        .to_string();

                        let metadata_doc =
                            room_docs::create_metadata_doc(topic, &sender, RoomMetadata::empty())
                                .await
                                .map_err(|e| e.to_string())?;
                        let roster_doc = room_docs::create_roster_doc(
                            topic,
                            &sender,
                            sk.public().to_string(),
                            label.clone(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                        let forward_handle = spawn_conversation_forwarder(
                            topic,
                            metadata_doc,
                            roster_doc,
                            receiver,
                            net_tx,
                            None,
                        );
                        *forward_handle_slot.lock().unwrap() = Some(forward_handle);

                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe {
                                name: label,
                                profile_image_ticket,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;
                        let presence = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::PresenceWithTicket {
                                ticket: personal_ticket,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(presence).await;

                        let _room = RoomStore::with_peers(&data_dir, topic, merged_peers);

                        Ok::<
                            (
                                GossipSender,
                                TopicId,
                                String,
                                Option<SharedTracker>,
                                usize,
                                Vec<PublicKey>,
                            ),
                            String,
                        >((
                            sender,
                            topic,
                            ticket_str,
                            room_tracker,
                            neighbor_count,
                            neighbor_ids,
                        ))
                    },
                    move |result| match result {
                        Ok((
                            sender,
                            topic,
                            ticket_str,
                            room_tracker,
                            neighbor_count,
                            neighbor_ids,
                        )) => AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                            sender,
                            room_tracker,
                            neighbor_count,
                            neighbor_ids,
                            generation: room_snapshot.generation,
                        },
                        Err(e) => AppMessage::RoomJoinFailed {
                            error: e,
                            generation: room_snapshot.generation,
                        },
                    },
                )
            }

            AppMessage::NewChatCreated => {
                // Navigate to the newly created room — handled via OpenRoom
                iced::Task::done(AppMessage::CreateNewRoom)
            }

            // ── Friend requests (state layer) ────────────────
            AppMessage::SendFriendRequest(_)
            | AppMessage::FriendRequestSent { .. }
            | AppMessage::FriendRequestFailed { .. }
            | AppMessage::FriendRequestReceived { .. }
            | AppMessage::FriendRequestRetry(_)
            | AppMessage::IncomingFriendRequestAccept { .. }
            | AppMessage::IncomingFriendRequestDecline { .. }
            | AppMessage::IncomingFriendRequestProcessed { .. } => self.update_contacts(message),

            AppMessage::OpenFriendChat(_) => self.update_contacts(message),

            AppMessage::RoomSelected(topic) => iced::Task::done(AppMessage::OpenRoom(topic)),
            AppMessage::OpenGroupChat(topic) => iced::Task::done(AppMessage::OpenRoom(topic)),

            // ── Group Creation ───────────────────────────────────────
            // ── Group Created (state layer) ──────────────────
            AppMessage::GroupCreated { .. } => self.update_groups(message),

            // ── ChatList ─────────────────────────────────────────────
            AppMessage::JoinTicketInputChanged(_) => self.update_home(message),

            // ── Chat ─────────────────────────────────────────────────
            // ── Calls (state layer) ──────────────────────────────
            AppMessage::StartVoiceCall(_)
            | AppMessage::StartVideoCall(_)
            | AppMessage::CallStarted(_)
            | AppMessage::CallEventReceived(_)
            | AppMessage::AcceptIncomingCall(_)
            | AppMessage::RejectIncomingCall(_)
            | AppMessage::HangUp(_)
            | AppMessage::ToggleCallMute
            | AppMessage::ToggleCallDeafen
            | AppMessage::ToggleCallCamera
            | AppMessage::SelectCamera(_)
            | AppMessage::SelectMicrophone(_)
            | AppMessage::SelectSpeaker(_)
            | AppMessage::CallUiTick
            | AppMessage::CallCommandFinished(_) => self.update_calls(message),
            #[cfg(feature = "screen-sharing")]
            AppMessage::StartScreenShare(_)
            | AppMessage::StopScreenShare
            | AppMessage::AcceptScreenShare
            | AppMessage::DeclineScreenShare
            | AppMessage::ToggleScreenShareFullscreen
            | AppMessage::ToggleScreenShareDetails
            | AppMessage::ScreenShareEventReceived(_)
            | AppMessage::ScreenShareFrameReceived(_)
            | AppMessage::ScreenShareStatsReceived(_)
            | AppMessage::ScreenShareCommandFinished(_)
            | AppMessage::ScreenShareRequestControl
            | AppMessage::ScreenShareRequestClipboard
            | AppMessage::ScreenShareSendClipboard
            | AppMessage::ScreenShareHostSendClipboard
            | AppMessage::ScreenShareClipboardRead(_)
            | AppMessage::ScreenShareGrantControl(_)
            | AppMessage::ScreenShareDenyControl
            | AppMessage::ScreenShareToggleAudio
            | AppMessage::ScreenShareRevokeControl
            | AppMessage::ScreenShareLowerQuality
            | AppMessage::ScreenShareFullQuality
            | AppMessage::ScreenShareSelectSource(_)
            | AppMessage::ScreenShareSetPreset(_)
            | AppMessage::ScreenShareDismissNotice
            | AppMessage::ScreenSharePointerMove { .. }
            | AppMessage::ScreenSharePointerButton { .. }
            | AppMessage::ScreenShareWheel { .. }
            | AppMessage::ScreenShareKeyEvent { .. }
            | AppMessage::ScreenShareSetView { .. }
            | AppMessage::ScreenSharePanStart { .. }
            | AppMessage::ScreenSharePanMove { .. }
            | AppMessage::ScreenSharePanEnd
            | AppMessage::ToggleScreenShareCursor => self.update_screen_share(message),
            AppMessage::WindowFocusChanged(focused) => {
                self.notifications_state
                    .update(NotificationsMessage::WindowFocusChanged(focused));
                if !focused {
                    self.typing_emitter.reset();
                }
                iced::Task::none()
            }
            // ── Chat (state layer) ─────────────────────────────
            AppMessage::InputChanged(_)
            | AppMessage::SendPressed
            | AppMessage::AttachPressed
            | AppMessage::AttachFolderPressed
            | AppMessage::ComposerSendFinished
            | AppMessage::ComposerDragOver(_)
            | AppMessage::ComposerFileDropped(_)
            | AppMessage::ComposerImeActive(_)
            | AppMessage::ToggleChatOptions
            | AppMessage::ToggleChatSearch
            | AppMessage::ChatSearchQueryChanged(_)
            | AppMessage::ClearConversation
            | AppMessage::ToggleDetailsPanel
            | AppMessage::ToggleMemberList
            | AppMessage::ToggleInviteMenu
            | AppMessage::InviteWhisperInputChanged(_)
            | AppMessage::InviteSendWhisper
            | AppMessage::PinMessage(_)
            | AppMessage::UnpinMessage(_)
            | AppMessage::RevealPinnedMessage(_) => self.update_chat(message),
            // ── Help overlay domain (BORU-APP-002) ──────────────
            // The shell routes ToggleHelp to the domain's update() and applies
            // the returned event. The overlay is presentation-only; the only
            // side effect is completing a pending GUI-test action.
            AppMessage::ToggleHelp => {
                if let Some(HelpEvent::VisibilityChanged { visible }) =
                    self.help_overlay.update(HelpMessage::Toggle)
                {
                    let _ = visible;
                    if let Some(action_id) = self.pending_toggle_help_action.take() {
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageHandled);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Completed);
                    }
                }
                iced::Task::none()
            }
            // ── Global keyboard shortcuts ───────────────────────────
            #[cfg(feature = "dev-ui")]
            AppMessage::Shortcut(Shortcut::DesignerSave) => {
                if self.settings_state.designer.enabled {
                    return iced::Task::done(AppMessage::Inspector(
                        crate::inspector::InspectorMsg::SaveLayout,
                    ));
                }
                iced::Task::none()
            }
            #[cfg(feature = "dev-ui")]
            AppMessage::Shortcut(
                shortcut @ (Shortcut::DesignerNudgeUp
                | Shortcut::DesignerNudgeDown
                | Shortcut::DesignerNudgeLeft
                | Shortcut::DesignerNudgeRight),
            ) => {
                if self.settings_state.designer.enabled {
                    let direction = match shortcut {
                        Shortcut::DesignerNudgeUp | Shortcut::DesignerNudgeRight => 1.0,
                        Shortcut::DesignerNudgeDown | Shortcut::DesignerNudgeLeft => -1.0,
                        _ => unreachable!(),
                    };
                    let step = if self.settings_state.designer.fine_adjust { 1.0 } else { 8.0 };
                    if self.settings_state.designer
                        .selected_component
                        .is_some_and(|component| component.home_section().is_some())
                    {
                        let value = (self.active_layout.home.max_content_width + direction * step)
                            .clamp(600.0, 2400.0);
                        return iced::Task::done(AppMessage::Inspector(
                            crate::inspector::InspectorMsg::SetLayoutFloat {
                                field: crate::layout_inspector::LayoutField::HomeMaxContentWidth,
                                value,
                            },
                        ));
                    }
                }
                iced::Task::none()
            }
            #[cfg(feature = "dev-ui")]
            AppMessage::Shortcut(Shortcut::DesignerDelete) => {
                if self.settings_state.designer.enabled {
                    if let Some(component) = self.settings_state.designer.selected_component {
                        // Hero is a production-required feature. The other
                        // home sections are explicitly optional and use the
                        // existing visibility override semantics.
                        if let Some(section) = component.home_section() {
                            if section == crate::layout::HomeSection::Hero {
                                return iced::Task::none();
                            }
                            let mut overrides = self.layout_overrides.clone();
                            let home = overrides.home.get_or_insert_with(Default::default);
                            let hidden = home.hidden_sections.get_or_insert_with(Vec::new);
                            if !hidden.contains(&section) {
                                hidden.push(section);
                                self.set_layout_overrides(overrides);
                                self.settings_state.designer.update(DesignerMessage::MarkDirty);
                            }
                        }
                    }
                }
                iced::Task::none()
            }
            AppMessage::Shortcut(Shortcut::Escape) => {
                #[cfg(feature = "dev-ui")]
                if self.settings_state.designer.enabled
                    && (self.settings_state.designer.drag_operation.is_some()
                        || self.settings_state.designer.resize_operation.is_some()
                        || self.settings_state.designer.selected_component.is_some())
                {
                    self.settings_state.designer.update(DesignerMessage::CancelDrag);
                    self.settings_state.designer.update(DesignerMessage::CancelResize);
                    self.settings_state.designer.update(DesignerMessage::Select(None));
                    self.settings_state.inspect_selected = None;
                    self.settings_state.inspect_hover = None;
                    return iced::Task::none();
                }
                // Close any open overlay/dialog, outermost first.
                //
                // Safety: a dialog that is mid-submit must NOT be dismissed
                // by Escape — the submit button shows a loading state and the
                // user would otherwise lose track of the in-flight operation.
                // The cancel handlers below also guard on the submitting
                // flags, so backdrop clicks and Cancel buttons are equally
                // safe.
                #[cfg(feature = "video-playback")]
                if self.inline_video_expanded {
                    self.inline_video_expanded = false;
                    self.layout_cache.borrow_mut().clear();
                    return iced::Task::none();
                }
                #[cfg(feature = "screen-sharing")]
                if self.calls_state.screen_share_fullscreen {
                    self.calls_state.screen_share_fullscreen = false;
                    return iced::Task::none();
                }
                if self.lightbox_image.is_some() {
                    return iced::Task::done(AppMessage::CloseImageLightbox);
                }
                // Emoji / GIF pickers close on Escape (same as any overlay).
                if self.show_emoji_picker || self.show_gif_picker {
                    self.show_emoji_picker = false;
                    self.show_gif_picker = false;
                    return iced::Task::none();
                }
                if self.rooms_state.show_create_room_dialog {
                    // Safe close: never dismiss a mid-submit dialog (the
                    // cancel handlers apply the same guard for backdrop
                    // clicks and the Cancel button).
                    if !self.rooms_state.create_room_submitting {
                        self.rooms_state.show_create_room_dialog = false;
                        self.rooms_state.create_room_error = None;
                        self.complete_close_dialog_action();
                    }
                    return iced::Task::none();
                }
                if self.tunnels_state.show_create_tunnel_dialog {
                    self.tunnels_state.show_create_tunnel_dialog = false;
                    return iced::Task::none();
                }
                if self.tunnels_state.share_local_service_open {
                    if !self.tunnels_state.share_service_submitting {
                        self.tunnels_state.share_local_service_open = false;
                        self.tunnels_state.share_service_error = None;
                    }
                    return iced::Task::none();
                }
                if self.connection_details_dialog.is_some() {
                    return iced::Task::done(AppMessage::CloseConnectionDetails);
                }
                if self.show_invite_menu {
                    self.show_invite_menu = false;
                    self.invite_whisper_input.clear();
                } else if self.show_create_group_dialog {
                    if !self.create_group_submitting {
                        self.show_create_group_dialog = false;
                        self.create_group_error = None;
                    }
                } else if self.help_overlay.visible() {
                    // BORU-APP-002: route the Escape-close through the
                    // help-overlay domain (no-op when already hidden).
                    self.help_overlay.update(HelpMessage::Close);
                } else if matches!(self.screen, Screen::DownloadManager) {
                    self.screen = self
                        .download_manager_return_to
                        .take()
                        .unwrap_or(Screen::ChatList);
                } else if matches!(self.screen, Screen::Settings) {
                    self.screen = self.settings_return_to.take().unwrap_or(Screen::ChatList);
                } else if matches!(self.screen, Screen::FriendRequests) {
                    self.screen = self
                        .friend_requests_return_to
                        .take()
                        .unwrap_or(Screen::ChatList);
                } else if matches!(
                    self.screen,
                    Screen::PeerProfile(_) | Screen::PeerCatalogue(_)
                ) {
                    self.screen = self
                        .peer_profile_return_to
                        .take()
                        .unwrap_or(Screen::ChatList);
                } else if matches!(self.screen, Screen::FriendProfile(_)) {
                    self.screen = self
                        .friend_profile_return_to
                        .take()
                        .unwrap_or(Screen::ChatList);
                } else if matches!(self.screen, Screen::Discover) {
                    self.screen = self.discover_return_to.take().unwrap_or(Screen::ChatList);
                } else if matches!(self.screen, Screen::Groups) {
                    self.screen = self.groups_return_to.take().unwrap_or(Screen::ChatList);
                } else if matches!(self.screen, Screen::FileSharing)
                    && !self.files_state.dashboard_search_input.is_empty()
                {
                    // FS-18: Escape clears the dashboard search query in one
                    // action (keyboard-accessible equivalent of the × button).
                    self.files_state.dashboard_search_input.clear();
                    self.files_state.shared_by_me_ui.clear();
                    self.refresh_shared_by_me_filter();
                } else if !self.composer_text.is_empty() {
                    self.composer_text.clear();
                }
                iced::Task::none()
            }
            AppMessage::Shortcut(Shortcut::NewChat) => iced::Task::done(AppMessage::CreateNewRoom),
            #[cfg(feature = "dev-ui")]
            AppMessage::Shortcut(Shortcut::DesignerUndo) => {
                if self.settings_state.designer.enabled {
                    if let Some(layout) = self.settings_state.designer_history.undo(&self.active_layout) {
                        self.set_layout_config(layout);
                        self.settings_state.designer.update(DesignerMessage::MarkDirty);
                    }
                }
                iced::Task::none()
            }
            #[cfg(feature = "dev-ui")]
            AppMessage::Shortcut(Shortcut::DesignerRedo) => {
                if self.settings_state.designer.enabled {
                    if let Some(layout) = self.settings_state.designer_history.redo(&self.active_layout) {
                        self.set_layout_config(layout);
                        self.settings_state.designer.update(DesignerMessage::MarkDirty);
                    }
                }
                iced::Task::none()
            }
            AppMessage::Shortcut(Shortcut::BackToChatList) => {
                if matches!(self.screen, Screen::Chat { .. }) {
                    iced::Task::done(AppMessage::GoToChatList)
                } else {
                    iced::Task::none()
                }
            }
            AppMessage::Shortcut(Shortcut::QuickCommand) => {
                if matches!(self.screen, Screen::Chat { .. }) {
                    self.composer_text = "/".to_string();
                    iced::widget::operation::focus(COMPOSER_INPUT)
                } else {
                    iced::Task::none()
                }
            }
            AppMessage::Shortcut(Shortcut::FocusNext) => iced::widget::operation::focus_next(),
            AppMessage::Shortcut(Shortcut::FocusPrevious) => {
                iced::widget::operation::focus_previous()
            }
            // ── Dashboard tab navigation (Ctrl+Left / Ctrl+Right) ──────
            // When on the File Sharing screen, cycle through tabs. These
            // shortcuts are harmless elsewhere: they simply no-op.
            AppMessage::Shortcut(Shortcut::DashboardTabPrevious) => {
                if matches!(self.screen, Screen::FileSharing) {
                    let tabs = crate::dashboard_view_model::DashboardTab::ALL;
                    let idx = tabs
                        .iter()
                        .position(|t| *t == self.files_state.dashboard_active_tab)
                        .unwrap_or(0);
                    let prev_idx = if idx == 0 { tabs.len() - 1 } else { idx - 1 };
                    self.files_state.dashboard_active_tab = tabs[prev_idx];
                }
                iced::Task::none()
            }
            AppMessage::Shortcut(Shortcut::DashboardTabNext) => {
                if matches!(self.screen, Screen::FileSharing) {
                    let tabs = crate::dashboard_view_model::DashboardTab::ALL;
                    let idx = tabs
                        .iter()
                        .position(|t| *t == self.files_state.dashboard_active_tab)
                        .unwrap_or(0);
                    let next_idx = if idx + 1 >= tabs.len() { 0 } else { idx + 1 };
                    self.files_state.dashboard_active_tab = tabs[next_idx];
                }
                iced::Task::none()
            }

            // ── Settings / terminal navigation (state layer) ──
            AppMessage::OpenSettings | AppMessage::CloseSettings => self.update_settings(message),

            #[cfg(feature = "terminal")]
            AppMessage::OpenTerminal | AppMessage::TerminalEvent(_) => {
                self.update_settings(message)
            }

            // ── Friend Requests ───────────────────────────────────────
            // ── Friend Requests (state layer) ───────────────────────
            AppMessage::OpenFriendRequests
            | AppMessage::CloseFriendRequests
            | AppMessage::FriendRequestSearchChanged(_)
            | AppMessage::FriendRequestSend(_)
            | AppMessage::FriendRequestAccept(_)
            | AppMessage::FriendRequestDecline(_)
            | AppMessage::FriendRequestCancel(_)
            | AppMessage::FriendRequestSentResult(_)
            | AppMessage::FriendRequestActionResult(_)
            | AppMessage::FriendEvent(_) => self.update_contacts(message),

            AppMessage::OpenFileSharing => {
                // Navigation only — the shared shell, networking services, and
                // conversation subscriptions stay alive; only the main panel
                // swaps to the File Sharing screen.
                self.screen = Screen::FileSharing;
                // FILES-03: the files icon always opens the MAIN File Sharing
                // screen. Reset any sub-tab selection, dashboard search query,
                // and row-popover state from a previous visit so no stale
                // sub-screen state is left behind (matches the Escape-key
                // reset behaviour for the dashboard).
                self.files_state.dashboard_active_tab = crate::dashboard_view_model::DashboardTab::SharedByMe;
                self.files_state.dashboard_search_input.clear();
                self.files_state.shared_by_me_ui.clear();
                self.refresh_shared_by_me_filter();
                // Start loading the FS-13 Sharing Summary and the FS-09 Shared by
                // Me projection immediately so the cards never render a premature
                // empty state while storage loads.
                let mut tasks = Vec::new();
                tasks.push(self.refresh_sharing_summary());
                tasks.push(self.refresh_shared_by_me());
                if let Some(action_id) = self.pending_open_file_sharing_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                if tasks.is_empty() {
                    iced::Task::none()
                } else {
                    iced::Task::batch(tasks)
                }
            }

            AppMessage::ToggleSidebarSectionCollapsed(index) => {
                if index < self.sidebar_section_collapsed.len()
                    && self.sidebar_section_count(index) > 0
                {
                    // SIDEBAR-01: empty sections stay collapsed — only
                    // populated sections respond to the manual toggle.
                    self.sidebar_section_collapsed[index] = !self.sidebar_section_collapsed[index];
                    if !self.sidebar_section_collapsed[index] {
                        // Manual expand shows existing items; don't replay the
                        // appearance animation (it only plays when the first
                        // item arrives).
                        self.sidebar_fade_frame[index] = crate::ui_components::SIDEBAR_FADE_FRAMES;
                    }
                }
                iced::Task::none()
            }

            AppMessage::ReplayPendingEvents(topic) => self.replay_pending_events_batch(topic),

            // ── Conversation events (state layer) ─────────────
            AppMessage::NetEvent(_)
            | AppMessage::WhisperEvent(_)
            | AppMessage::OfflineDMStatus { .. }
            | AppMessage::InboxEvent(_)
            | AppMessage::OutboxRetryResult(_)
            | AppMessage::MessageSent(..)
            | AppMessage::RetryOutgoingMessage(_) => self.update_chat(message),

            // ── File transfers (state layer) ─────────────────────────
            AppMessage::ExecuteFileSend(_)
            | AppMessage::ExecuteFolderSend(_)
            | AppMessage::ExecuteImageSend(_)
            | AppMessage::ExecuteDownload
            | AppMessage::ExecuteDownloadAt(_)
            | AppMessage::PauseDownloadAt(_)
            | AppMessage::ResumeDownloadAt(_)
            | AppMessage::CancelDownloadAt(_)
            | AppMessage::DownloadInitiated { .. }
            | AppMessage::DownloadInitiationFailed { .. }
            | AppMessage::FileSent(_)
            | AppMessage::DownloadDone(..)
            | AppMessage::DownloadDonePeerFile(..)
            | AppMessage::PosterGenerated { .. }
            | AppMessage::VideoMetadataProbed { .. }
            | AppMessage::DownloadFailed(_)
            | AppMessage::DownloadProgress(_) => self.update_files(message),
            AppMessage::SaveProfile => {
                // Profile persistence is disabled.
                iced::Task::none()
            }
            // ── Inline video playback (state layer) ───────────
            AppMessage::PlayInlineVideo(_)
            | AppMessage::StreamInlineVideo(_)
            | AppMessage::StreamUrl(_)
            | AppMessage::InlineVideoShowControls => self.update_chat(message),

            #[cfg(feature = "video-playback")]
            AppMessage::StreamingServerReady { .. }
            | AppMessage::StreamingServerFailed { .. }
            | AppMessage::CloseInlineVideo
            | AppMessage::InlineVideoTick
            | AppMessage::InlineVideoControlsFocused(_)
            | AppMessage::InlineVideoSeekChanged(_)
            | AppMessage::InlineVideoSeekReleased
            | AppMessage::InlineVideoSeekRelative(_)
            | AppMessage::InlineVideoToggleMute
            | AppMessage::InlineVideoAdjustVolume(_)
            | AppMessage::InlineVideoSetVolume(_)
            | AppMessage::InlineVideoToggleExpanded
            | AppMessage::InlineVideoEvent(_) => self.update_chat(message),
            // ── File/media state (short codes, downloads, images) ──
            AppMessage::OpenDownloadedFile(_)
            | AppMessage::ReshareFile(_)
            | AppMessage::MintShortCode(_)
            | AppMessage::ShortCodeMinted(_)
            | AppMessage::CloseShortCodeDialog
            | AppMessage::CopyShortCode(_)
            | AppMessage::OpenRedeemCodeDialog
            | AppMessage::CloseRedeemCodeDialog
            | AppMessage::RedeemCodeInputChanged(_)
            | AppMessage::RedeemShortCode
            | AppMessage::ShortCodeRedeemed(_)
            | AppMessage::SetOverwritePolicy(..)
            | AppMessage::ImageDownloaded { .. }
            | AppMessage::GifMediaFetched { .. }
            | AppMessage::ProfileImageDownloaded(..)
            | AppMessage::ProfileImageDownloadFailed(_)
            | AppMessage::ImageHydrated { .. }
            | AppMessage::ImageUploadFailed(_)
            | AppMessage::FileUploadFailed(_)
            | AppMessage::FileOfferAnnounced { .. }
            | AppMessage::FileOfferCached { .. }
            | AppMessage::FileOfferCacheFailed { .. }
            | AppMessage::FileDownloaded { .. }
            | AppMessage::ThumbnailFetched { .. } => self.update_files(message),
            AppMessage::ErrorMsg(msg) => {
                self.push_system(msg);
                self.drain_pending_transfers()
            }

            AppMessage::SystemMsg(msg) => {
                self.push_system(msg);
                iced::Task::none()
            }

            AppMessage::OpenPeerProfile(_) => self.update_contacts(message),
            AppMessage::ClosePeerProfile => self.update_contacts(message),

            // ── Remote catalogue browsing ──

            // ── Friend Profile Navigation ──
            // ── Discover / directory (state layer) ─────────────────
            AppMessage::BrowsePeerCatalogue(_)
            | AppMessage::PeerCatalogueReceived { .. }
            | AppMessage::PeerCatalogueFailed(_)
            | AppMessage::CatalogueScrolled(..)
            | AppMessage::ToggleAdvertiseRoom(_)
            | AppMessage::SubscribeDirectoryTopic
            | AppMessage::DirectorySubscribed(_)
            | AppMessage::OpenDirectory
            | AppMessage::CloseDiscover
            | AppMessage::RefreshRoomRegistry
            | AppMessage::DiscoverSearchChanged(_)
            | AppMessage::DiscoverFilterToggled(_)
            | AppMessage::DiscoverTagToggled(_)
            | AppMessage::DiscoverSortChanged(_)
            | AppMessage::DiscoverClearFilters
            | AppMessage::DirectoryRoomJoin(_)
            | AppMessage::DirectoryRoomJoinById(_)
            | AppMessage::DirectoryRoomHideById(_)
            | AppMessage::DirectoryRoomUnhideById(_)
            | AppMessage::DirectoryRoomUnhideAll
            | AppMessage::DeleteDirectoryRoom(_)
            | AppMessage::DirectoryRoomUpdate(..) => self.update_discover(message),
            // BORU-DIR-09 (PDF Task 3.3): a verified withdrawal removes the
            // matching advertisement immediately. In the live app the
            // withdrawal arrives through the directory channel and is
            // drained on ConnMonitorTick; this arm covers programmatic /
            // test delivery of the same semantic event (rooms domain).
            AppMessage::DirectoryRoomWithdrawal(..) => self.update_rooms(message),
            AppMessage::OpenFriendProfile(_) => self.update_contacts(message),
            AppMessage::CloseFriendProfile => self.update_contacts(message),
            AppMessage::ToggleFriendProfileMenu => self.update_contacts(message),
            // ── Tunnels share-local-service (state layer) ──────────
            AppMessage::OpenShareLocalService
            | AppMessage::OpenShareVncTunnel
            | AppMessage::ShareLocalServiceNameChanged(_)
            | AppMessage::ShareLocalServicePortChanged(_)
            | AppMessage::ShareLocalServiceExpiryChanged(_)
            | AppMessage::ShareLocalServiceHttpToggled(_)
            | AppMessage::CancelShareLocalService
            | AppMessage::ShareLocalServiceScanDone(_)
            | AppMessage::SelectShareLocalServiceSuggestion(_)
            | AppMessage::ConfirmShareLocalService
            | AppMessage::TunnelShared { .. }
            | AppMessage::TunnelShareFailed { .. }
            | AppMessage::TunnelOfferSent
            | AppMessage::TunnelOfferSendFailed { .. }
            | AppMessage::ConnectReceivedTunnel(_)
            | AppMessage::ReceivedTunnelConnected { .. }
            | AppMessage::ReceivedTunnelConnectFailed { .. }
            | AppMessage::DisconnectReceivedTunnel(_)
            | AppMessage::StopSharingTunnel(_)
            | AppMessage::OpenReceivedTunnel(_)
            | AppMessage::CopyReceivedTunnelAddress(_) => self.update_tunnels(message),
            AppMessage::FriendRenameInputChanged(_) => self.update_contacts(message),
            AppMessage::FriendRenameConfirm => self.update_contacts(message),
            AppMessage::CopyPeerId(_) => self.update_contacts(message),
            AppMessage::OpenConnectionDetails => {
                self.rooms_state.show_create_room_dialog = false;
                self.friend_profile_menu_open = false;
                self.friend_profile_renaming = false;
                self.friend_remove_confirm = false;
                self.friend_block_confirm = false;
                self.history_confirm_clear = false;
                self.room_delete_confirm_topic = None;
                self.connection_details_announcement = None;
                self.connection_details_focus_target = Some(CONNECTION_DETAILS_TRIGGER_INPUT);
                self.connection_details_dialog = Some(self.current_connection_details_dialog());
                return iced::widget::operation::focus(CONNECTION_DETAILS_FIRST_VALUE_INPUT);
            }
            AppMessage::RetryConnection => {
                // The endpoint and gossip layer handle reconnection automatically.
                // This just triggers a re-render so the user sees the current state.
                info!("Manual retry requested from dashboard");
                iced::Task::none()
            }
            AppMessage::SubscribeStoredConversations
            | AppMessage::BackgroundSubscribe(..)
            | AppMessage::BackgroundSubscribed(..)
            | AppMessage::BackgroundSubscribeFailed(..) => self.update_discover(message),
            AppMessage::OpenGroups | AppMessage::CloseGroups => self.update_groups(message),
            AppMessage::CloseConnectionDetails => self.close_connection_details_dialog(),
            // ── Invite Member / Accept Group Invite (state layer) ──
            AppMessage::ShowInviteMemberDialog
            | AppMessage::HideInviteMemberDialog
            | AppMessage::InviteMemberToggled(_)
            | AppMessage::ConfirmInviteMember
            | AppMessage::AcceptGroupInvite(_) => self.update_groups(message),
            AppMessage::CopyConnectionDetails => {
                if self.connection_details_dialog.is_some() {
                    if let Some(summary) =
                        self.current_connection_details_dialog().support_summary()
                    {
                        self.connection_details_announcement =
                            Some("Support summary copied to clipboard".to_string());
                        return iced::clipboard::write(summary);
                    }
                }
                iced::Task::none()
            }
            AppMessage::CopyConnectionDetailsValue { label, value } => {
                self.connection_details_announcement = Some(format!("Copied {label} to clipboard"));
                return iced::clipboard::write(value);
            }
            AppMessage::DismissToast => {
                self.notifications_state
                    .update(NotificationsMessage::DismissToast);
                iced::Task::none()
            }
            AppMessage::ShowRemoveFriendConfirm
            | AppMessage::CancelRemoveFriend
            | AppMessage::ConfirmRemoveFriend
            | AppMessage::ShowBlockFriendConfirm
            | AppMessage::CancelBlockFriend
            | AppMessage::ShowRenameFriendInput
            | AppMessage::ConfirmBlockFriend => self.update_contacts(message),
            AppMessage::RequestFileDownload { .. } => self.update_files(message),
            // ── Image lightbox (state layer) ──────────────────
            AppMessage::OpenImageLightbox(_) | AppMessage::CloseImageLightbox => {
                self.update_chat(message)
            }

            // ── GUI test actions (MCP-driven) ──
            AppMessage::GuiTestActionReceived(action) => {
                // Record receipt only after the message has entered the normal
                // Iced event loop. The subscription itself remains side-effect
                // free; validation and state changes happen below in `update`.
                self.iced_diagnostics.record(
                    "GuiTestActionReceived",
                    FailureLayer::IcedUpdate,
                    true,
                    "",
                    None,
                );
                let action_id = action.action_id.clone();
                let was_already_processed = self
                    .gui_action_history
                    .get(&action_id)
                    .is_some_and(|status| !matches!(status.state, GuiActionState::Queued));
                let _ = self.gui_action_history.record(action.clone());
                // MCP records the request before it reaches Iced. Once the
                // normal update path has advanced that same idempotency key,
                // a repeated delivery must not submit the composer again.
                if was_already_processed {
                    return iced::Task::none();
                }

                let command = match serde_json::from_str::<GuiTestCommand>(&action.command) {
                    Ok(command) => command,
                    Err(error) => {
                        let _ = self.gui_action_history.set_error(
                            &action_id,
                            GuiActionError::new(
                                GuiActionErrorCode::UnknownCommand,
                                format!("Invalid GUI test command: {error}"),
                            ),
                        );
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                };

                if let Err(error) = command.validate() {
                    let _ = self.gui_action_history.set_error(
                        &action_id,
                        GuiActionError::new(GuiActionErrorCode::InvalidArgument, error),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Rejected);
                    return iced::Task::none();
                }

                // Persist the command's declared post-condition before routing.
                // This keeps the expected state visible for every action, including
                // commands whose dedicated handler is added later.
                if let Some(expected) = command.expected_state() {
                    let _ = self
                        .gui_action_history
                        .set_expected_state(&action_id, expected);
                }

                if let GuiTestCommand::OpenConversation { conversation_id } = &command {
                    if let Err(error) = self.validate_gui_test_command(&command) {
                        let _ = self.gui_action_history.set_error(&action_id, error);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                    let peer = match conversation_id.parse::<PublicKey>() {
                        Ok(peer) => peer,
                        Err(error) => {
                            let _ = self.gui_action_history.set_error(
                                &action_id,
                                GuiActionError::new(
                                    GuiActionErrorCode::InvalidArgument,
                                    format!("Invalid conversation_id: {error}"),
                                ),
                            );
                            let _ = self
                                .gui_action_history
                                .set_state(&action_id, GuiActionState::Rejected);
                            return iced::Task::none();
                        }
                    };
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_open_conversation_action = Some((action_id, peer));
                    return iced::Task::done(AppMessage::OpenConversation(peer));
                }

                if matches!(command, GuiTestCommand::GoToChatList) {
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::ScreenIs("ChatList".to_string()),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_chat_list_action = Some(action_id);
                    return iced::Task::done(
                        gui_navigation_message(&command).expect("GoToChatList mapping"),
                    );
                }

                // Route these commands through the same messages emitted by
                // the real sidebar buttons. The action is completed by the
                // ordinary message handler after the screen changes.
                if matches!(command, GuiTestCommand::OpenFriends) {
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::ScreenIs(
                            "FriendRequests".to_string(),
                        ),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_open_friends_action = Some(action_id);
                    return iced::Task::done(
                        gui_navigation_message(&command).expect("OpenFriends mapping"),
                    );
                }

                if matches!(command, GuiTestCommand::OpenSettings) {
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::ScreenIs("Settings".to_string()),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_open_settings_action = Some(action_id);
                    return iced::Task::done(
                        gui_navigation_message(&command).expect("OpenSettings mapping"),
                    );
                }

                if matches!(command, GuiTestCommand::OpenFileSharing) {
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::ScreenIs("FileSharing".to_string()),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_open_file_sharing_action = Some(action_id);
                    return iced::Task::done(
                        gui_navigation_message(&command).expect("OpenFileSharing mapping"),
                    );
                }

                if let GuiTestCommand::OpenDashboardTab { tab } = &command {
                    // Mirror the real sidebar flow: open the File Sharing screen
                    // first, then select the requested tab.
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::Generic(format!(
                            "dashboard_tab_{}",
                            tab.as_str()
                        )),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_dashboard_tab_action = Some(action_id);
                    return iced::Task::batch(vec![
                        iced::Task::done(AppMessage::OpenFileSharing),
                        iced::Task::done(AppMessage::DashboardTabSelected(
                            dashboard_tab_from_name(*tab),
                        )),
                    ]);
                }

                if let GuiTestCommand::TestShareFile { path } = &command {
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::Generic("file_shared".to_string()),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_share_file_action = Some(action_id);
                    return iced::Task::done(AppMessage::SharedFilePicked(path.clone()));
                }

                if matches!(command, GuiTestCommand::CloseDialog) {
                    let close_message = match self.close_current_dialog() {
                        Ok(message) => message,
                        Err(error) => {
                            let _ = self.gui_action_history.set_error(&action_id, error);
                            let _ = self
                                .gui_action_history
                                .set_state(&action_id, GuiActionState::Rejected);
                            return iced::Task::none();
                        }
                    };
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::Generic("dialog_closed".to_string()),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_close_dialog_action = Some(action_id);
                    return iced::Task::done(close_message);
                }

                if let GuiTestCommand::SetComposerText { text } = &command {
                    if let Err(error) = self.validate_gui_test_command(&command) {
                        let _ = self.gui_action_history.set_error(&action_id, error);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::ComposerTextIs(text.clone()),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_set_composer_action = Some((action_id, text.clone()));
                    return iced::Task::done(AppMessage::InputChanged(text.clone()));
                }

                if matches!(command, GuiTestCommand::ClearComposer) {
                    if let Err(error) = self.validate_gui_test_command(&command) {
                        let _ = self.gui_action_history.set_error(&action_id, error);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::ComposerTextIs(String::new()),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_set_composer_action = Some((action_id, String::new()));
                    return iced::Task::done(AppMessage::InputChanged(String::new()));
                }

                if matches!(command, GuiTestCommand::FocusComposer) {
                    if let Err(error) = self.validate_gui_test_command(&command) {
                        let _ = self.gui_action_history.set_error(&action_id, error);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::widget::operation::focus(COMPOSER_INPUT);
                }

                if matches!(command, GuiTestCommand::SubmitComposer) {
                    if let Err(error) = self.validate_gui_test_command(&command) {
                        let _ = self.gui_action_history.set_error(&action_id, error);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::MessageSent,
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_submit_composer_action = Some(action_id);
                    return iced::Task::done(AppMessage::SendPressed);
                }

                if matches!(command, GuiTestCommand::CreateNewRoom) {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_create_room_action = Some(action_id);
                    return iced::Task::done(AppMessage::CreateNewRoom);
                }

                if let GuiTestCommand::SetCreateRoomName { name } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::CreateNewRoomNameChanged(name.clone()));
                }

                if let GuiTestCommand::SetCreateRoomAdvertise { enabled } = &command {
                    // Backward-compatible alias: the old boolean checkbox
                    // mapped to discoverability.  `true` → PublicDiscoverable,
                    // `false` → Private (the old "unchecked = private room"
                    // behaviour).
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    let visibility = if *enabled {
                        RoomVisibility::PublicDiscoverable
                    } else {
                        RoomVisibility::Private
                    };
                    return iced::Task::done(AppMessage::CreateNewRoomVisibilityChanged(
                        visibility,
                    ));
                }

                if let GuiTestCommand::SetCreateRoomVisibility { visibility } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::CreateNewRoomVisibilityChanged(
                        *visibility,
                    ));
                }

                if let GuiTestCommand::SetCreateRoomDescription { description } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::CreateNewRoomDescriptionChanged(
                        description.clone(),
                    ));
                }

                if let GuiTestCommand::SetCreateRoomTags { tags } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::CreateNewRoomTagsChanged(tags.clone()));
                }

                // ── Room directory visibility (BORU-DIR-06) ──────────────
                if let GuiTestCommand::OpenRoomSettings { room_id } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    if let Ok(topic) = TopicId::from_str(room_id) {
                        return iced::Task::done(AppMessage::OpenRoomSettings(topic));
                    }
                    return iced::Task::none();
                }

                if let GuiTestCommand::SetRoomSettingsName { name } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::RoomSettingsNameChanged(name.clone()));
                }

                if let GuiTestCommand::SetRoomSettingsDescription { description } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::RoomSettingsDescriptionChanged(
                        description.clone(),
                    ));
                }

                if let GuiTestCommand::SetRoomSettingsTags { tags } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::RoomSettingsTagsChanged(tags.clone()));
                }

                if let GuiTestCommand::SetRoomSettingsVisibility { visibility } = &command {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::RoomSettingsVisibilityChanged(
                        *visibility,
                    ));
                }

                if matches!(command, GuiTestCommand::ConfirmRoomSettings) {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::done(AppMessage::ConfirmRoomSettings);
                }

                if let GuiTestCommand::SetRoomDirectoryVisibility {
                    room_id,
                    visibility,
                } = &command
                {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    if let Ok(topic) = TopicId::from_str(room_id) {
                        return iced::Task::done(AppMessage::SetRoomDirectoryVisibility {
                            topic,
                            visibility: *visibility,
                        });
                    }
                    return iced::Task::none();
                }

                if matches!(command, GuiTestCommand::ConfirmCreateNewRoom) {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_confirm_create_room_action = Some(action_id);
                    return iced::Task::done(AppMessage::ConfirmCreateNewRoom);
                }

                if let Some(AppMessage::ToggleDark(enabled)) = gui_dark_mode_message(&command) {
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::DarkModeIs(enabled),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    return iced::Task::done(AppMessage::ToggleDark(enabled));
                }

                if let Some(AppMessage::ToggleHelp) = gui_help_message(&command) {
                    // ToggleHelp flips the overlay; declare the post-state so
                    // the action status is truthful for both directions.
                    let target = !self.help_overlay.visible();
                    let _ = self.gui_action_history.set_expected_state(
                        &action_id,
                        boru_core::diagnostics::ExpectedState::HelpVisible(target),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    self.pending_toggle_help_action = Some(action_id);
                    return iced::Task::done(AppMessage::ToggleHelp);
                }

                if let GuiTestCommand::SetPeerPresence { peer_id, online } = &command {
                    if let Err(error) = self.validate_gui_test_command(&command) {
                        let _ = self.gui_action_history.set_error(&action_id, error);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                    let peer = match peer_id.parse::<PublicKey>() {
                        Ok(peer) => peer,
                        Err(error) => {
                            let _ = self.gui_action_history.set_error(
                                &action_id,
                                GuiActionError::new(
                                    GuiActionErrorCode::InvalidArgument,
                                    format!("Invalid peer_id for SetPeerPresence: {error}"),
                                ),
                            );
                            let _ = self
                                .gui_action_history
                                .set_state(&action_id, GuiActionState::Rejected);
                            return iced::Task::none();
                        }
                    };
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageQueued);
                    // Route through the same friend-status path real network
                    // events use, so the header presence (dot + label) is
                    // derived from the production peer_presence_map.
                    let status = if *online {
                        FriendStatus::Online
                    } else {
                        FriendStatus::Offline
                    };
                    self.handle_friend_event(FriendEvent::StatusChanged { peer, status });
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::none();
                }

                if let GuiTestCommand::ClearMeshEventLog = &command {
                    // Test-only: clear the live mesh event log so evidence
                    // harnesses can capture the card's intentional no-events
                    // state. This never fabricates events.
                    self.mesh_event_log.clear();
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                    return iced::Task::none();
                }

                let GuiTestCommand::OpenRoom { room_id } = command else {
                    if let GuiTestCommand::SelectPeer { ref peer_id } = command {
                        if let Err(error) = self.validate_gui_test_command(&command) {
                            let _ = self.gui_action_history.set_error(&action_id, error);
                            let _ = self
                                .gui_action_history
                                .set_state(&action_id, GuiActionState::Rejected);
                            return iced::Task::none();
                        }
                        let peer = match peer_id.parse::<PublicKey>() {
                            Ok(peer) => peer,
                            Err(error) => {
                                let _ = self.gui_action_history.set_error(
                                    &action_id,
                                    GuiActionError::new(
                                        GuiActionErrorCode::InvalidArgument,
                                        format!("Invalid peer_id: {error}"),
                                    ),
                                );
                                let _ = self
                                    .gui_action_history
                                    .set_state(&action_id, GuiActionState::Rejected);
                                return iced::Task::none();
                            }
                        };
                        let _ = self.gui_action_history.set_expected_state(
                            &action_id,
                            boru_core::diagnostics::ExpectedState::ScreenIs(format!(
                                "PeerProfile({peer})"
                            )),
                        );
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageQueued);
                        self.pending_select_peer_action = Some((action_id, peer));
                        return iced::Task::done(AppMessage::OpenPeerProfile(peer));
                    }
                    if let GuiTestCommand::BrowseCatalogue { ref peer_id } = command {
                        let peer = match peer_id.parse::<PublicKey>() {
                            Ok(peer) => peer,
                            Err(error) => {
                                let _ = self.gui_action_history.set_error(
                                    &action_id,
                                    GuiActionError::new(
                                        GuiActionErrorCode::InvalidArgument,
                                        format!("Invalid peer_id for BrowseCatalogue: {error}"),
                                    ),
                                );
                                let _ = self
                                    .gui_action_history
                                    .set_state(&action_id, GuiActionState::Rejected);
                                return iced::Task::none();
                            }
                        };
                        let _ = self.gui_action_history.set_expected_state(
                            &action_id,
                            boru_core::diagnostics::ExpectedState::ScreenIs(format!(
                                "PeerCatalogue({peer})"
                            )),
                        );
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageQueued);
                        return iced::Task::done(AppMessage::BrowsePeerCatalogue(peer));
                    }
                    if let GuiTestCommand::DownloadFile {
                        ref peer_id,
                        ref content_hash,
                    } = command
                    {
                        let peer = match peer_id.parse::<PublicKey>() {
                            Ok(peer) => peer,
                            Err(error) => {
                                let _ = self.gui_action_history.set_error(
                                    &action_id,
                                    GuiActionError::new(
                                        GuiActionErrorCode::InvalidArgument,
                                        format!("Invalid peer_id for DownloadFile: {error}"),
                                    ),
                                );
                                let _ = self
                                    .gui_action_history
                                    .set_state(&action_id, GuiActionState::Rejected);
                                return iced::Task::none();
                            }
                        };
                        // Look up cached catalogue metadata if available
                        let file = self.files_state
                            .peer_catalogue_view
                            .as_ref()
                            .and_then(|(cached_peer, files)| {
                                if *cached_peer == peer {
                                    files
                                        .iter()
                                        .find(|f| f.content_hash == *content_hash)
                                        .cloned()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| {
                                // Construct a minimal RemoteSharedFile from just the hash
                                RemoteSharedFile::new(
                                    content_hash.clone(),
                                    content_hash.clone(),
                                    None,
                                    0,
                                    "application/octet-stream",
                                    None,
                                    0,
                                )
                            });
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageQueued);
                        self.pending_download_action = Some(action_id);
                        return iced::Task::done(AppMessage::RequestFileDownload { peer, file });
                    }
                    // Other GUI commands retain their existing diagnostic-only
                    // behavior until their dedicated action handlers land.
                    return iced::Task::none();
                };

                let topic = match room_id.parse::<TopicId>() {
                    Ok(topic) => topic,
                    Err(error) => {
                        let _ = self.gui_action_history.set_error(
                            &action_id,
                            GuiActionError::new(
                                GuiActionErrorCode::InvalidArgument,
                                format!("Invalid room_id: {error}"),
                            ),
                        );
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Rejected);
                        return iced::Task::none();
                    }
                };

                // A room must already be known (active, in the conversation
                // map, or in room history) before the diagnostic MCP action
                // can open it. The internal discovery topic is never a
                // user-facing room, and the old auto-joined lobby no longer
                // exists at startup (BORU-DISC-12), so there is no
                // bootstrap-free lobby special case here.
                let known_room = (self.sender.is_some() && topic == self.topic)
                    || self.conversations.contains_key(&topic)
                    || self
                        .room_history
                        .rooms
                        .iter()
                        .any(|room| room.topic == topic);
                if !known_room {
                    let _ = self.gui_action_history.set_error(
                        &action_id,
                        GuiActionError::new(
                            GuiActionErrorCode::UnknownRoom,
                            format!("Room {topic} has not been joined"),
                        ),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Rejected);
                    return iced::Task::none();
                }

                if self.rooms_state.show_create_room_dialog
                    || self.history_confirm_clear
                    || self.room_delete_confirm_topic.is_some()
                {
                    let _ = self.gui_action_history.set_error(
                        &action_id,
                        GuiActionError::new(
                            GuiActionErrorCode::BlockingDialogOpen,
                            "A blocking dialog is open",
                        ),
                    );
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Rejected);
                    return iced::Task::none();
                }

                let _ = self.gui_action_history.set_expected_state(
                    &action_id,
                    boru_core::diagnostics::ExpectedState::RoomSelected(topic.to_string()),
                );
                let _ = self
                    .gui_action_history
                    .set_state(&action_id, GuiActionState::AppMessageQueued);
                self.pending_open_room_action = Some((action_id, topic));
                iced::Task::done(AppMessage::OpenRoom(topic))
            }

            AppMessage::GuiActionTimeout(action_id) => {
                if let Some(status) = self.gui_action_history.expire(&action_id) {
                    let expected = status
                        .expected_state
                        .as_ref()
                        .map(|state| state.description())
                        .unwrap_or_else(|| "unknown expected state".to_string());
                    self.iced_diagnostics.record(
                        "GuiActionTimedOut",
                        FailureLayer::IcedUpdate,
                        false,
                        format!("action_id={action_id}; expected={expected}"),
                        None,
                    );
                    // Publish the state observed at the timeout boundary. The
                    // watch channel retains this latest snapshot for MCP
                    // callers; no GUI state or unrelated task is cancelled.
                    self.publish_gui_state();
                }
                iced::Task::none()
            }

            AppMessage::GuiTestWaitSatisfied(_key) => {
                // Placeholder: wait conditions are tracked externally.
                iced::Task::none()
            }

            AppMessage::GuiTestWaitTimedOut {
                idempotency_key,
                condition,
                expected,
                elapsed_ms,
            } => {
                warn!(
                    "GUI test wait timed out: key={} condition={} expected={} elapsed={}ms",
                    idempotency_key, condition, expected, elapsed_ms
                );
                iced::Task::none()
            }

            AppMessage::FriendAdded { .. } => self.update_contacts(message),

            AppMessage::RemoveFriend(_) => self.update_contacts(message),

            AppMessage::FriendRemoved { .. } => self.update_contacts(message),

            AppMessage::DeleteRoom(_) => self.update_chat(message),

            AppMessage::FriendListResult(_) => self.update_contacts(message),

            AppMessage::SplashTick => {
                // Skip animations when OS reduced-motion is preferred.
                if !self.reduced_motion {
                    // Advance spinner animations.
                    self.splash_spinner_frame = (self.splash_spinner_frame + 1) % 10;
                    // Advance the connecting animation while the gossip sender
                    // hasn't arrived yet (conversation opened but peer not connected).
                    if self.sender.is_none() && matches!(self.screen, Screen::Chat { .. }) {
                        self.connecting_spinner_frame = (self.connecting_spinner_frame + 1) % 10;
                    }
                    // Advance the main screen reconnecting animation when on
                    // the ChatList screen and not yet fully connected.
                    if matches!(self.screen, Screen::ChatList) {
                        let is_connected = !self.neighbors.is_empty()
                            || self.relayed_peers > 0
                            || self.direct_peers > 0;
                        if !is_connected {
                            self.main_screen_reconnect_frame =
                                (self.main_screen_reconnect_frame + 1) % 10;
                        }
                    }
                    // Advance sidebar section fade-in counters so sections
                    // that just gained their first item finish their
                    // appearance animation. The `SplashTick` subscription in
                    // `main.rs` stays alive while any counter is below
                    // `SIDEBAR_FADE_FRAMES`.
                    for frame in self.sidebar_fade_frame.iter_mut() {
                        if *frame < crate::ui_components::SIDEBAR_FADE_FRAMES {
                            *frame += 1;
                        }
                    }
                } // end !reduced_motion guard
                iced::Task::none()
            }

            // The activity feed formats timestamps relative to the current
            // wall clock. Bumping the tick revision changes the Recent
            // Activity and Tunnels card dependencies so `iced::lazy` rebuilds
            // those subtrees — and only those — with fresh relative
            // timestamps / expiry labels while the app is otherwise idle. The
            // Online Peers dependency deliberately excludes the tick, so the
            // peers card stays memoized across idle seconds.
            AppMessage::ActivityTick => {
                self.notifications_state
                    .update(NotificationsMessage::ActivityTick);
                iced::Task::none()
            }

            AppMessage::ConnMonitorTick => {
                self.typing_peers.expire(std::time::Instant::now());
                // BORU-DIR-12 (PDF Task 4.3): keep the bounded
                // room-directory cache's per-room local relationship state
                // derived from the real local room database (joined rooms
                // from the conversation store + persisted hide preference).
                self.sync_directory_local_states();
                // BORU-DIR-08 (PDF Task 3.2 step 4): evict advertisements
                // whose TTL elapsed since the last valid refresh.  This
                // replaces the old fixed 1-hour window with the
                // advertisement's own `expires_after_secs` (300 s policy
                // TTL): a room whose advertiser disappears leaves the active
                // directory after the TTL, while refreshes arriving within
                // the TTL (refresh interval 60 s << TTL) keep it live — no
                // flicker on temporary packet loss.
                let evicted = {
                    let mut store = self.directory_store.lock().unwrap();
                    store.evict_expired()
                };
                if !evicted.is_empty() {
                    tracing::debug!(
                        count = evicted.len(),
                        "evicted expired directory advertisements"
                    );
                    if let Some(storage) = self.storage.as_ref() {
                        if let Err(err) = storage.with_conn(|conn| {
                            for (topic, author) in &evicted {
                                conn.execute(
                                    "DELETE FROM directory_ads WHERE topic = ?1 AND author = ?2",
                                    rusqlite::params![topic.as_bytes(), author.as_bytes()],
                                )
                                .map_err(n0_error::AnyError::from_std)?;
                            }
                            Ok(())
                        }) {
                            warn!("failed to delete stale directory advertisements: {err}");
                        }
                    }
                    self.invalidate_prewarm(&[Screen::Discover]);
                }
                self.save_directory_store();
                let pruned_file_offers = {
                    let mut registry = self.file_offer_registry.lock().unwrap();
                    registry.prune_stale()
                };
                if pruned_file_offers > 0 {
                    tracing::debug!(
                        count = pruned_file_offers,
                        "pruned stale direct file offers"
                    );
                }
                self.refresh_missing_downloads();
                // Re-broadcast an active short-code announcement so receivers
                // that join the rendezvous topic late can still pick it up
                // (the topic stays subscribed because short_code_sender is
                // held while the dialog is open).
                if let Some(share) = self.files_state.short_code_active.clone() {
                    if let Some(sender) = self.files_state.short_code_sender.clone() {
                        let sk = self.secret_key.clone();
                        let announcement = boru_core::short_code::ShortCodeAnnouncement {
                            code: share.code.clone(),
                            name: share.name.clone(),
                            ticket: share.ticket.clone(),
                            size: share.size,
                            created_at_ms: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        };
                        let encoded = match boru_core::short_code::SignedShortCodeAnnouncement::sign(
                            &sk,
                            &announcement,
                        ) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                tracing::warn!("short-code: sign failed: {e}");
                                return iced::Task::none();
                            }
                        };
                        let sender = sender.clone();
                        tokio::task::spawn(async move {
                            if let Err(e) = sender.broadcast(bytes::Bytes::from(encoded)).await {
                                tracing::warn!("short-code: broadcast failed: {e}");
                            }
                        });
                    }
                }
                if self.pending_image_upload.is_some() {
                    self.image_upload_spinner_frame = (self.image_upload_spinner_frame + 1) % 10;
                }
                if self.pending_file_upload.is_some() {
                    self.file_upload_spinner_frame = (self.file_upload_spinner_frame + 1) % 10;
                }
                if self.gif_loading {
                    self.gif_spinner_frame = (self.gif_spinner_frame + 1) % 10;
                }
                // Flush debounced neighbor status changes — batch rapid
                // online/offline transitions into one visible update per tick.
                self.flush_pending_neighbor_status();

                // Peer presence: downgrade peers whose last-seen timestamp
                // has gone stale (> AWAY_THRESHOLD_MS) to Away.  They stay in
                // the presence map — removal happens on NeighborDown only.
                self.refresh_peer_presence();

                #[cfg(feature = "video-playback")]
                self.reconcile_inline_video_viewport();

                // Auto-dismiss toast after ~2 seconds (120 ticks at 60fps → ~120 frames,
                // but ConnMonitorTick fires at 1 Hz, so effectively ~120 seconds would be too
                // long. We tick at 1 Hz here, so ~2 ticks = ~2 seconds for a 120-counter toast.
                // Actually the counter was intended for 60fps rendering ticks, but we don't
                // have a per-frame tick. Using ConnMonitorTick (~1 Hz) we decrement by 60
                // per tick to match the original ~2-second intent.
                self.notifications_state.tick_toast_auto_dismiss();

                // Auto-dismiss a terminal screen-share notice (Stopped/Error)
                // after ~8 seconds so a stale status never blocks a fresh
                // share (the panel also offers Dismiss/retry immediately).
                #[cfg(feature = "screen-sharing")]
                self.tick_screen_share_notice();

                // Keep discovered peers as a session-wide list.  Gossip
                // neighbors belong to the selected room and may be empty
                // while another room is displayed; replacing this list on
                // every tick made the sidebar appear empty and discarded
                // DHT discoveries.
                for peer in &self.neighbors {
                    if !self.discovered_peers.contains(peer) {
                        self.discovered_peers.push(*peer);
                    }
                }
                self.discovered_online_cache = self.neighbors.clone();
                self.discovered_sidebar_revision = self.discovered_sidebar_revision.wrapping_add(1);
                self.refresh_sidebar_counts();

                let mut tasks: Vec<iced::Task<AppMessage>> = Vec::new();

                // Retry durable gossip outbox entries for ALL subscribed
                // conversations — not just the active room. Background
                // conversations also accumulate outbox entries (e.g. from
                // MCP-triggered sends) and must retry when the peer reconnects.
                let all_retries: Vec<(TopicId, Vec<(u64, Vec<u8>)>)> =
                    if let Some(storage) = &self.storage {
                        match storage.list_pending_outgoing() {
                            Ok(rows) => {
                                let mut by_topic: std::collections::BTreeMap<
                                    TopicId,
                                    Vec<(u64, Vec<u8>)>,
                                > = std::collections::BTreeMap::new();
                                for row in &rows {
                                    by_topic
                                        .entry(row.topic)
                                        .or_default()
                                        .push((row.event_id, row.signed_bytes.clone()));
                                }
                                by_topic.into_iter().collect()
                            }
                            Err(_) => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    };
                if !all_retries.is_empty() {
                    // Collect senders for all subscribed conversations
                    let topic_senders: Vec<(TopicId, GossipSender, bool, usize)> = {
                        let mut pairs = Vec::new();
                        if let Some(ref sender) = self.sender {
                            pairs.push((
                                self.topic,
                                sender.clone(),
                                self.sender_ready,
                                self.neighbors.len(),
                            ));
                        }
                        for (topic, conv) in &self.conversations {
                            if topic != &self.topic {
                                if let Some(ref sender) = conv.sender {
                                    pairs.push((
                                        *topic,
                                        sender.clone(),
                                        conv.sender_ready,
                                        conv.neighbors.len(),
                                    ));
                                }
                            }
                        }
                        pairs
                    };
                    let _history = self.chat_history.clone();
                    tasks.push(iced::Task::perform(
                        async move {
                            let mut results = Vec::new();
                            for (topic, entries) in all_retries {
                                let sender = topic_senders
                                    .iter()
                                    .find(|(t, _, ready, neighbors)| {
                                        *t == topic && *ready && *neighbors > 0
                                    })
                                    .map(|(_, s, _, _)| s.clone());
                                if let Some(sender) = sender {
                                    for (event_id, bytes) in entries {
                                        let delivered =
                                            sender.broadcast(bytes.into()).await.is_ok();
                                        results.push((topic, event_id, delivered));
                                    }
                                }
                            }
                            results
                        },
                        |results| AppMessage::OutboxRetryResult(results),
                    ));
                }

                // Retry mailbox delivery for offline peers that just came online.
                // Only attempt delivery when the recipient is currently in our
                // gossip mesh (neighbors) — skip disconnected peers to avoid
                // hanging on QUIC connect.
                if !self.neighbors.is_empty() {
                    let data_dir = self.data_dir.clone();
                    let secret_key = self.secret_key.clone();
                    let endpoint = self.endpoint.clone();
                    let online_peers: Vec<PublicKey> = self.neighbors.iter().copied().collect();
                    tasks.push(iced::Task::perform(
                        async move {
                            let mut store = match boru_core::mailbox::MailboxStore::load(&data_dir)
                            {
                                Ok(Some(s)) => s,
                                _ => return Vec::new(),
                            };
                            let pending = match store.pending() {
                                Ok(p) => p,
                                Err(_) => return Vec::new(),
                            };
                            let mut results = Vec::new();
                            for envelope in pending {
                                let peer_key = envelope.recipient().identity;
                                if !online_peers.contains(&peer_key) {
                                    continue;
                                }
                                match send_deliver(
                                    &endpoint,
                                    &secret_key,
                                    peer_key,
                                    envelope.clone(),
                                )
                                .await
                                {
                                    Ok(()) => {
                                        results.push((envelope.message_id(), true));
                                    }
                                    Err(_) => {
                                        results.push((envelope.message_id(), false));
                                    }
                                }
                            }
                            results
                        },
                        |_results| AppMessage::Noop,
                    ));
                }

                // Periodic presence heartbeat — broadcasts Message::Presence every ~5s.

                // Periodic connection type refresh (~60s) or on-demand
                // (needs_conn_refresh set by on_neighbor_up/down).
                let should_refresh = self.conn_refresh_counter == 0 || self.needs_conn_refresh;
                if should_refresh && !self.conn_refresh_in_flight {
                    self.conn_refresh_in_flight = true;
                    self.conn_refresh_counter = 60;
                    self.needs_conn_refresh = false;
                    let endpoint = self.endpoint.clone();
                    let neighbors: Vec<iroh::PublicKey> = self.neighbors.iter().copied().collect();
                    tasks.push(iced::Task::perform(
                        async move {
                            let mut direct = 0usize;
                            let mut relayed = 0usize;
                            for peer in &neighbors {
                                let has_direct = endpoint
                                    .remote_info(*peer)
                                    .await
                                    .map(|info| info.addrs().any(|a| !a.addr().is_relay()))
                                    .unwrap_or(false);
                                if has_direct {
                                    direct += 1;
                                } else {
                                    relayed += 1;
                                }
                            }
                            AppMessage::ConnCountsResult { direct, relayed }
                        },
                        |msg| msg,
                    ));
                } else if self.conn_refresh_counter > 0 {
                    self.conn_refresh_counter -= 1;
                }

                // Resolve pending ticket peers (deferred from on_neighbor_up
                // because `endpoint.remote_info()` is async) into concrete
                // EndpointAddrs so the next presence broadcast can embed them
                // as extra bootstrap nodes in the regenerated room ticket.
                if !self.pending_ticket_peers.is_empty() && !self.ticket_resolve_in_flight {
                    self.ticket_resolve_in_flight = true;
                    let pending: Vec<PublicKey> = std::mem::take(&mut self.pending_ticket_peers);
                    let endpoint = self.endpoint.clone();
                    tasks.push(iced::Task::perform(
                        async move {
                            let mut resolved = Vec::new();
                            let mut unresolved = Vec::new();
                            for peer in &pending {
                                match endpoint.remote_info(*peer).await {
                                    Some(info) => resolved.push(EndpointAddr::from_parts(
                                        info.id(),
                                        info.into_addrs().map(|a| a.into_addr()),
                                    )),
                                    None => unresolved.push(*peer),
                                }
                            }
                            AppMessage::TicketPeersResolved {
                                resolved,
                                unresolved,
                            }
                        },
                        |msg| msg,
                    ));
                }

                // Relay selection and direct addresses are learned asynchronously.
                // Keep the room ticket shown in the UI (and therefore copied to the
                // clipboard) aligned with the endpoint's current address.
                // Personal-room tickets are no longer broadcast publicly;
                // they must be shared through direct (whisper) channels.
                if self.refresh_local_peer_addr() {
                    if let Some(ref sender) = self.sender {
                        let sk = self.secret_key.clone();
                        let s = sender.clone();
                        tasks.push(iced::Task::perform(
                            async move {
                                if let Ok(encoded) =
                                    SignedMessage::sign_and_encode(&sk, &crate::Message::Presence)
                                {
                                    s.broadcast(encoded).await.ok();
                                }
                            },
                            |_| AppMessage::Noop,
                        ));
                    }
                }
                if self.presence_counter == 0 {
                    self.presence_counter = 5;
                    // Mesh membership changed (neighbor up/down or a pending
                    // peer resolved) → regenerate the room ticket with the
                    // latest extra bootstrap peers before broadcasting.
                    // personal_room_ticket() reads ticket_extra_peers live.
                    self.ticket_needs_regeneration = false;
                    if let Some(ref sender) = self.sender {
                        let sk = self.secret_key.clone();
                        let ticket = self.personal_room_ticket();
                        let profile_image_ticket = self.settings_state.profile_image_ticket.clone();
                        let label = self.local_label.clone();
                        let s = sender.clone();
                        tasks.push(iced::Task::perform(
                            async move {
                                // PresenceWithTicket is sent frequently for liveness, but
                                // it does not carry profile metadata. Re-announce AboutMe
                                // here so peers that joined after the initial room
                                // broadcast still learn (and can download) our avatar.
                                if let Ok(encoded) = SignedMessage::sign_and_encode(
                                    &sk,
                                    &crate::Message::AboutMe {
                                        name: label,
                                        profile_image_ticket,
                                    },
                                ) {
                                    s.broadcast(encoded).await.ok();
                                }
                                if let Ok(encoded) = SignedMessage::sign_and_encode(
                                    &sk,
                                    &crate::Message::PresenceWithTicket { ticket },
                                ) {
                                    s.broadcast(encoded).await.ok();
                                }
                            },
                            |_| AppMessage::Noop,
                        ));
                    }
                } else {
                    self.presence_counter -= 1;
                }

                // Periodic invisible keepalive heartbeat — broadcasts Message::Heartbeat
                // every ~2s to keep connections warm and update mesh health timestamps
                // without producing any chat log entry or UI notification.
                if self.heartbeat_counter == 0 {
                    self.heartbeat_counter = 2;
                    if let Some(ref sender) = self.sender {
                        let sk = self.secret_key.clone();
                        let s = sender.clone();
                        tasks.push(iced::Task::perform(
                            async move {
                                if let Ok(encoded) =
                                    SignedMessage::sign_and_encode(&sk, &crate::Message::Heartbeat)
                                {
                                    s.broadcast(encoded).await.ok();
                                }
                            },
                            |_| AppMessage::Noop,
                        ));
                    }
                } else {
                    self.heartbeat_counter -= 1;
                }

                // ── Periodic latency ping (~15s) ──
                // Broadcasts a LatencyPing to measure round-trip time to all
                // connected peers.  Peers respond with a LatencyPong, and the
                // pong handler in record_latency stores the measured RTT.
                if self.latency_ping_counter == 0 {
                    self.latency_ping_counter = 15;
                    if let Some(ref sender) = self.sender {
                        let sk = self.secret_key.clone();
                        let s = sender.clone();
                        tasks.push(iced::Task::perform(
                            async move {
                                let sent_at_ms = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as u64;
                                if let Ok(encoded) = SignedMessage::sign_and_encode(
                                    &sk,
                                    &crate::Message::LatencyPing { sent_at_ms },
                                ) {
                                    s.broadcast(encoded).await.ok();
                                }
                            },
                            |_| AppMessage::Noop,
                        ));
                    }
                } else {
                    self.latency_ping_counter -= 1;
                }

                // ── Periodic room-advertisement broadcast (~60s) ──
                // For each room the user has enabled for directory advertising,
                // sign and broadcast a RoomAdvertisement into the directory
                // topic.  The advertisement carries the room's name,
                // description, member count, and a join ticket (BORU-APP-006).
                if let Some(task) = self.periodic_room_advertisement() {
                    tasks.push(task);
                }

                // ── Periodic global-registry DHT lookup (~120s) ──
                // Enlist the relay-independent registry namespace and merge
                // discovered rooms into directory_store so they appear in
                // PUBLIC ROOMS even across different relays.
                if let Some(task) = self.periodic_registry_lookup() {
                    tasks.push(task);
                }

                // ── Profile cache eviction + ProfileUpdate broadcast ──
                // Evict stale entries for peers whose cached profile data is
                // older than 1 hour (i.e. they've been offline that long).
                self.evict_stale_profile_cache();
                // Periodically broadcast our own profile metadata via gossip
                // (rate-limited internally to at most once per 30 seconds).
                tasks.push(self.broadcast_profile_update());

                // ── Drain directory room channel ──────────────────────
                // Poll the directory gossip channel for new room advertisements
                // received from other peers.  Each ad is stored and queued for
                // a background subscription so discovery does not require a
                // manual ticket exchange.
                let mut discovered_room_tasks = Vec::new();
                let mut directory_changed = false;
                // PUBLIC-02: descriptions collected while the directory rx
                // lock is held (cannot call &mut self push_activity there);
                // flushed into the Recent Activity feed after the scope.
                let mut announced_rooms: Vec<String> = Vec::new();
                {
                    let mut dir_guard = self.directory_room_rx.try_lock();
                    if let Ok(ref mut rx) = dir_guard {
                        while let Ok(item) = rx.try_recv() {
                            match item {
                                DirectoryRoomEvent::Advertisement(ad, from) => {
                                    info!(from = %from, topic = %ad.topic, "received room advertisement");
                                    // BORU-DIR-19 (PDF Task 7.1): the bounded
                                    // receive gate enforces metadata bounds, clamps
                                    // absurd TTLs, rate-limits per author,
                                    // deduplicates identical broadcasts, and caps
                                    // the store. The outcome drives the UI: only a
                                    // genuinely new advertisement announces +
                                    // auto-subscribes once; a metadata refresh
                                    // updates the card; a duplicate, a rate-limited
                                    // flood, or an oversized advertisement produces
                                    // no UI event at all (no constant re-rendering,
                                    // no forced subscriptions from repeated
                                    // broadcasts).
                                    let outcome = self.directory_store.lock().unwrap().receive(
                                        ad.clone(),
                                        from,
                                        Instant::now(),
                                    );
                                    match outcome {
                                        LegacyAdmitOutcome::Added => {
                                            // PUBLIC-02: surface genuinely new
                                            // public-room announcements in the
                                            // home screen's Recent Activity feed.
                                            // The same author re-broadcasts every
                                            // ~60 s; only the first sighting is a
                                            // fresh event worth showing.
                                            let creator = self.resolve_name(&from);
                                            announced_rooms.push(format!(
                                                "{creator} announced public room \"{}\"",
                                                ad.room_name
                                            ));
                                            directory_changed = true;

                                            // Parse the authenticated ticket and use
                                            // its topic; never subscribe to an
                                            // untrusted raw advertisement topic when
                                            // the ticket disagrees with it.
                                            let Ok(ticket) = ad.ticket.parse::<Ticket>() else {
                                                warn!(from = %from, "ignoring room advertisement with invalid ticket");
                                                continue;
                                            };
                                            let topic = ticket.topic;
                                            if topic == self.topic
                                                || self.conversations.contains_key(&topic)
                                                || !self.rooms_state.auto_subscribed_rooms.insert(topic)
                                            {
                                                continue;
                                            }

                                            let mut entry =
                                                ConversationEntry::new(topic, "", ad.room_name);
                                            entry.archived = true;
                                            self.conversation_store.upsert(entry);
                                            self.chats_sidebar_revision =
                                                self.chats_sidebar_revision.wrapping_add(1);
                                            discovered_room_tasks.push(iced::Task::done(
                                                AppMessage::BackgroundSubscribe(
                                                    topic,
                                                    self.discovered_peers.clone(),
                                                ),
                                            ));
                                        }
                                        LegacyAdmitOutcome::Refreshed => {
                                            // Known room, changed metadata: refresh
                                            // the card but never re-announce or
                                            // re-subscribe.
                                            directory_changed = true;
                                        }
                                        LegacyAdmitOutcome::Duplicate => {
                                            trace!(from = %from.fmt_short(), topic = %ad.topic,
                                        "duplicate room advertisement; no UI churn");
                                        }
                                        LegacyAdmitOutcome::RateLimited => {
                                            debug!(from = %from.fmt_short(),
                                        "room advertisement rate-limited");
                                        }
                                        LegacyAdmitOutcome::Rejected(violation) => {
                                            debug!(from = %from.fmt_short(), violation = ?violation,
                                        "room advertisement rejected by metadata bounds");
                                        }
                                    }
                                }
                                // BORU-DIR-09 (PDF Task 3.3): a verified room
                                // withdrawal removes the matching advertisement
                                // immediately — the receiver loop already
                                // verified the withdrawal signature before
                                // sending this message. TTL expiry remains the
                                // safety net if a withdrawal is missed.
                                DirectoryRoomEvent::Withdrawal(topic, from) => {
                                    let removed =
                                        self.directory_store.lock().unwrap().withdraw(topic, from);
                                    if removed {
                                        info!(from = %from, topic = %topic, "room advertisement withdrawn");
                                        directory_changed = true;
                                    }
                                }
                            }
                        }
                    }
                }
                if !discovered_room_tasks.is_empty() {}
                tasks.extend(discovered_room_tasks);
                // PUBLIC-02: flush collected room announcements into the
                // Recent Activity feed now that the directory rx lock scope
                // has ended (push_activity takes &mut self).
                for description in announced_rooms {
                    self.notifications_state.push_activity(description, ActivityKind::Generic);
                }
                if directory_changed {
                    // The Discover screen's room list changed.
                    self.invalidate_prewarm(&[Screen::Discover]);
                }

                // ── Profile image download: drain pending queue ─────────
                // Processed here (on ConnMonitorTick) as a fallback path in
                // case a ticket is pushed without a subsequent NetEvent to
                // trigger the NetEvent handler's own queue drain.
                if let Some((peer, ticket_str)) = self.pending_profile_image_tickets.pop_front() {
                    let blob_store = self.blob_store.clone();
                    let endpoint = self.endpoint.clone();
                    let memory_lookup = self.memory_lookup.clone();
                    let neighbors = self.neighbors.clone();
                    let failed_peer = peer;
                    let safety = self.public_room_safety.clone();
                    tasks.push(iced::Task::perform(
                        async move {
                            use boru_core::chat_callbacks::TransferKind;
                            let ticket: BlobTicket = ticket_str
                                .parse::<BlobTicket>()
                                .map_err(|e| format!("Parse profile image ticket: {e}"))?;
                            seed_memory_lookup(&memory_lookup, &[ticket.addr().clone()]);
                            let peer_id = ticket.addr().id;
                            let candidates = download_candidates(peer_id, &neighbors);
                            download_blob_with_safety(
                                &blob_store,
                                &endpoint,
                                ticket.hash(),
                                candidates,
                                "profile-image".into(),
                                TransferKind::Image,
                                |_| {},
                                safety.as_deref(),
                                peer_id,
                            )
                            .await
                            .map_err(|e| format!("Download profile image: {e}"))?;
                            let mut reader = blob_store.blobs().reader(ticket.hash());
                            let mut buf = Vec::new();
                            use tokio::io::AsyncReadExt;
                            reader
                                .read_to_end(&mut buf)
                                .await
                                .map_err(|e| format!("Read profile image: {e}"))?;
                            Ok((peer, buf))
                        },
                        move |r: Result<(PublicKey, Vec<u8>), String>| match r {
                            Ok((peer, data)) => AppMessage::ProfileImageDownloaded(peer, data),
                            Err(_) => AppMessage::ProfileImageDownloadFailed(failed_peer),
                        },
                    ));
                }

                if let Ok(mut queue) = self.files_state.download_progress_queue.lock() {
                    // Coalesce Progress events per transfer ID: only the latest
                    // progress per active download per tick survives.  Terminal
                    // events (Started, Completed, Failed, Cancelled) always pass
                    // through so the UI stays correct.
                    use std::collections::HashMap;
                    let mut latest: HashMap<TransferId, TransferProgress> = HashMap::new();
                    let mut terminals: Vec<TransferProgress> = Vec::new();
                    for progress in queue.drain(..) {
                        match &progress {
                            TransferProgress::Progress { id, .. } => {
                                latest.insert(*id, progress);
                            }
                            _ => {
                                terminals.push(progress);
                            }
                        }
                    }
                    for progress in terminals {
                        tasks.push(iced::Task::done(AppMessage::DownloadProgress(progress)));
                    }
                    for progress in latest.into_values() {
                        tasks.push(iced::Task::done(AppMessage::DownloadProgress(progress)));
                    }
                }

                // ── Video poster results from detached ingest tasks ──────
                // The DirectOffer send path generates posters inside a
                // background tokio task that cannot touch UI state; drain
                // the results here so the sender's own card renders the
                // same preview receivers see.
                if let Ok(mut queue) = self.files_state.poster_result_queue.lock() {
                    for (name, bytes, dimensions) in queue.drain(..) {
                        tasks.push(iced::Task::done(AppMessage::PosterGenerated {
                            name,
                            poster: Ok((bytes, dimensions)),
                        }));
                    }
                }

                // ── Seen-on-visibility: when user is at bottom of log,
                // mark Delivered entries as Seen ──
                if self.follow_latest {
                    for ui_entry in self.entries.iter_mut() {
                        if ui_entry.delivery_state == DeliveryState::Delivered
                            && ui_entry.event_id > 0
                        {
                            ui_entry.delivery_state = DeliveryState::Seen;
                            ui_entry.bump_gen();
                            let mut store = self.chat_history.lock().unwrap();
                            let _ =
                                store.update_delivery_state(ui_entry.event_id, DeliveryState::Seen);
                        }
                    }
                }
                // Retry failed profile image downloads for friends whose
                // image ticket is stored but handle is not yet cached.
                self.retry_stale_profile_images();
                self.enforce_image_budget();
                self.enforce_entry_cap();

                // ── Directory advertisement eviction ──
                // Remove ads that haven't been refreshed in 2 minutes.
                {
                    let mut store = self.directory_store.lock().unwrap();
                    store.evict_stale(Duration::from_secs(120));
                }

                // ── Periodic chat history persistence ──
                self.send_save_chat_history();

                if tasks.is_empty() {
                    iced::Task::none()
                } else {
                    iced::Task::batch(tasks)
                }
            }

            AppMessage::ConnCountsResult { direct, relayed } => {
                let had_peers = self.direct_peers > 0 || self.relayed_peers > 0;
                self.direct_peers = direct;
                self.relayed_peers = relayed;
                let now_has_peers = direct > 0 || relayed > 0;
                if !had_peers && now_has_peers {
                    self.push_mesh_event(format!(
                        "Discovered {direct} direct, {relayed} relayed peer{}",
                        if direct + relayed == 1 { "" } else { "s" },
                    ));
                }
                self.conn_refresh_in_flight = false;
                iced::Task::none()
            }

            AppMessage::TicketPeersResolved {
                resolved,
                unresolved,
            } => {
                self.ticket_resolve_in_flight = false;
                // Merge resolved peer addressing info into the ticket's extra
                // bootstrap list (dedupe by endpoint id), then flag the ticket
                // for regeneration on the next presence broadcast.
                for addr in resolved {
                    if addr.id != self.local_public
                        && !self.ticket_extra_peers.iter().any(|e| e.id == addr.id)
                    {
                        self.ticket_extra_peers.push(addr);
                    }
                }
                // Re-queue peers whose addressing info was not available yet;
                // the next ConnMonitorTick will retry the lookup.
                for peer in unresolved {
                    if !self.pending_ticket_peers.contains(&peer) {
                        self.pending_ticket_peers.push(peer);
                    }
                }
                self.ticket_needs_regeneration = true;
                iced::Task::none()
            }

            AppMessage::ConnectionsResult(lines) => {
                for line in lines {
                    self.push_system(line);
                }
                iced::Task::none()
            }

            AppMessage::MeshWatchdogTick => {
                // Periodic mesh quiescence check — monitors for prolonged inactivity.
                let new_health = if self.sender.is_none() {
                    MeshHealth::Offline("Not connected to any room".to_string())
                } else if self.neighbors.is_empty() {
                    MeshHealth::Degraded("No peers in the mesh".to_string())
                } else {
                    MeshHealth::Good
                };
                // ── Auto-subscribe to stored conversations once the mesh has peers ──
                if self.conversation_subscription_pending
                    && (!self.neighbors.is_empty()
                        || self.relayed_peers > 0
                        || self.direct_peers > 0)
                {
                    self.conversation_subscription_pending = false;
                    let count = self.conversation_store.active_iter().into_iter().count();
                    info!(count, "mesh ready: subscribing to stored conversations");
                    self.push_mesh_event(
                        format!("Subscribing to {count} stored conversation(s)…",),
                    );
                    if count > 0 {
                        self.update_mesh_connected_state(&new_health);
                        self.mesh_health = new_health;
                        return iced::Task::done(AppMessage::SubscribeStoredConversations);
                    }
                }
                // Detect transitions and push system notifications.
                let notification = match (&self.last_mesh_health, &new_health) {
                    (Some(MeshHealth::Good), MeshHealth::Degraded(reason)) => {
                        Some(format!("Mesh degraded: {reason}"))
                    }
                    (Some(MeshHealth::Good), MeshHealth::Offline(reason)) => {
                        Some(format!("Mesh offline: {reason}"))
                    }
                    (Some(MeshHealth::Degraded(_)), MeshHealth::Good) => {
                        Some("Mesh recovered: all peers active.".to_string())
                    }
                    (Some(MeshHealth::Offline(_)), MeshHealth::Good) => {
                        Some("Mesh recovered: endpoint back online.".to_string())
                    }
                    (None, _) => None,
                    _ => None,
                };

                self.update_mesh_connected_state(&new_health);

                self.mesh_health = new_health;
                self.last_mesh_health = Some(self.mesh_health.clone());

                if let Some(ref msg) = notification {
                    self.push_system(msg.clone());
                }
                // Log mesh health transitions to the event log (capacity 50).
                if let Some(log_msg) = &notification {
                    self.push_mesh_event(log_msg.clone());
                }

                iced::Task::none()
            }

            AppMessage::OutboxRetryTick => {
                // Periodic retry of undelivered outgoing mailbox envelopes.
                // Collect friends with mailbox keys and attempt delivery of
                // any pending envelopes.
                let endpoint = self.endpoint.clone();
                let secret_key = self.secret_key.clone();
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.files_state.download_progress_queue.clone();
                let peers_with_mailbox: Vec<PublicKey> = self
                    .friends
                    .iter()
                    .filter_map(|(fid, rec)| rec.mailbox_public_key.map(|mb| (fid, mb.identity)))
                    .map(|(_, pk)| pk)
                    .collect();

                if peers_with_mailbox.is_empty() {
                    iced::Task::none()
                } else {
                    iced::Task::perform(
                        async move {
                            // Load the local mailbox store (shared across all outgoing envelopes).
                            let s =
                                MailboxStore::load(&data_dir)
                                    .ok()
                                    .flatten()
                                    .unwrap_or_else(|| {
                                        MailboxStore::for_recipient(&data_dir, secret_key.public())
                                    });
                            let mut store = s;
                            for peer in &peers_with_mailbox {
                                let pending = store.pending_for_recipient(*peer);
                                for envelope in pending {
                                    let msg_id = envelope.message_id();
                                    match send_deliver(&endpoint, &secret_key, *peer, envelope)
                                        .await
                                    {
                                        Ok(()) => {
                                            // Keep the envelope until the recipient's signed
                                            // acknowledgement arrives via InboxEvent::AckReceived.
                                            debug!("mailbox: retry delivered envelope {}", msg_id);
                                        }
                                        Err(_) => {
                                            // Leave in store for next retry.
                                        }
                                    }
                                }
                            }
                            AppMessage::Noop
                        },
                        |msg| msg,
                    )
                }
            }

            // ── Settings (state layer) ─────────────────────────────
            AppMessage::ToggleDark(_)
            | AppMessage::ToggleAccentColorPicker
            | AppMessage::AccentColorSelected(_)
            | AppMessage::AccentColorCancelled
            | AppMessage::SetNickname(_)
            | AppMessage::SetChatTextSize(_) => self.update_settings(message),
            // ── Dev UI theme watcher (BORU-UI-06) ───────────────────
            AppMessage::UiThemeReloaded { generation, result } => {
                self.update_ui_theme_reloaded(generation, result)
            }
            // ── Dev layout watcher (BORU-LAYOUT-06) ────────────────────
            AppMessage::LayoutReloaded { generation, result } => {
                self.update_layout_reloaded(generation, result)
            }
            // ── Dev UI Inspector (BORU-UI-09) ───────────────────────
            #[cfg(feature = "dev-ui")]
            AppMessage::Inspector(msg) => self.update_inspector(msg),
            // ── File sharing dashboard (state layer) ────────────────
            AppMessage::OpenDownloadsFolder
            | AppMessage::DashboardSearchChanged(_)
            | AppMessage::DashboardSearchCleared
            | AppMessage::DashboardSharedByMeSortClicked(_)
            | AppMessage::DashboardDownloadedSortClicked(_)
            | AppMessage::DashboardActivitySortClicked(_)
            | AppMessage::TransferProjectionUpdate(_)
            | AppMessage::TransferSnapshotResync
            | AppMessage::DownloadingCancel(_)
            | AppMessage::DownloadingPause(_)
            | AppMessage::DownloadingResume(_)
            | AppMessage::DownloadingStop(_)
            | AppMessage::OpenDownloadManager
            | AppMessage::CloseDownloadManager
            | AppMessage::SharedByMeMenuToggle(_)
            | AppMessage::SharedByMeDetails(_)
            | AppMessage::SharedByMeCloseDetails
            | AppMessage::SharedByMeReveal(_)
            | AppMessage::SharedByMeConfirmStopSharing(_)
            | AppMessage::SharedByMeCancelStopSharing
            | AppMessage::SharedByMeRevokeAccess(..)
            | AppMessage::SharedByMeLoaded(_)
            | AppMessage::SharedByMeThumbnailReady { .. }
            | AppMessage::DashboardRecentActivityLoaded(_)
            | AppMessage::DashboardSharingSummaryLoaded(_)
            | AppMessage::DashboardDownloadedRefresh
            | AppMessage::DashboardDownloadedLoaded(_)
            | AppMessage::DownloadedOpen(_)
            | AppMessage::DownloadedReveal(_)
            | AppMessage::DownloadedRemoveHistory(_)
            | AppMessage::DashboardTabSelected(_)
            | AppMessage::ActivityLogLoaded(_)
            | AppMessage::ActivityLogRefresh
            | AppMessage::ActivityLogFilterSelected(_)
            | AppMessage::ActivityLogPageSelected(_)
            | AppMessage::ActivityLogDetailsToggled(_)
            | AppMessage::ActivityLogClearRequested
            | AppMessage::ActivityLogClearCancelled
            | AppMessage::ActivityLogClearConfirmed
            | AppMessage::DashboardConnectivityDismissed
            | AppMessage::DashboardDownloadingRefresh => self.update_files(message),
            AppMessage::CatalogueFetchFailed(_) | AppMessage::CatalogueErrorDismissed => {
                self.update_discover(message)
            }
            AppMessage::WindowResized { width, height } => {
                let old_mode = ResponsiveMode::of(self.window_width);
                self.window_width = width;
                self.window_height = height;
                // Window width is not part of the dependency snapshots (except
                // FileSharing's own band), so a pre-warmed tree built at
                // another responsive band is stale once the window crosses a
                // breakpoint.
                if ResponsiveMode::of(width) != old_mode {
                    self.invalidate_prewarm(PREWARM_ORDER);
                }
                iced::Task::none()
            }

            AppMessage::ReportBug => {
                let url = report_bug_url(&self.data_dir);
                let url2 = url.clone();
                iced::Task::perform(
                    async move {
                        let result = open::that(&url2);
                        if let Err(e) = result {
                            tracing::warn!(url = %url2, error = %e, "failed to open bug report URL");
                        }
                    },
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::SaveSupportBundle => {
                let Some(path) = rfd::FileDialog::new()
                    .set_title("Save redacted Boru support bundle")
                    .set_file_name("boru-support-bundle.json")
                    .save_file()
                else { return iced::Task::none(); };
                let input = boru_core::support_bundle::SupportBundleInput {
                    build_sha: option_env!("GIT_HASH").unwrap_or("unknown").into(),
                    os: std::env::consts::OS.into(), arch: std::env::consts::ARCH.into(),
                    enabled_features: vec!["net".into(), "gui".into()], endpoint_id: self.local_public.to_string(),
                    relay_transport: format!("{:?}", self.relay_mode), dht_health: format!("mesh: {:?}", self.mesh_health),
                    connection_paths: vec![format!("direct_peers={}", self.direct_peers), format!("relayed_peers={}", self.relayed_peers)],
                    schema_version: "runtime diagnostics".into(), active_subscription_count: self.room_neighbor_counts.len(),
                    active_task_count: self.background_subscriptions_in_flight.len(), diagnostics: Some(DIAGNOSTICS.clone()), journal: Some(self.iced_diagnostics.clone()),
                };
                self.connection_details_announcement = match boru_core::support_bundle::export_json(&path, &input) {
                    Ok(()) => Some(format!("Support bundle saved to {}", path.display())),
                    Err(error) => Some(format!("Support bundle failed: {error}")),
                };
                iced::Task::none()
            }

            AppMessage::OpenUrl(url) => {
                // Open the URL in the system default browser.
                // Use xdg-open on Linux, open on macOS, etc.
                let url2 = url.clone();
                iced::Task::perform(
                    async move {
                        let result = open::that(&url2);
                        if let Err(e) = result {
                            tracing::warn!(url = %url2, error = %e, "failed to open URL");
                        }
                    },
                    |_| AppMessage::Noop,
                )
            }

            // ── Link preview (state layer) ────────────────
            AppMessage::LinkPreviewLoaded(..) => self.update_chat(message),

            // ── Pre-warm (PERF-4R-B) ───────────────────────────────────
            AppMessage::UserActivity => {
                // Any keyboard/mouse event pauses pre-warming until the
                // user has been idle for 2+ seconds again.
                self.idle_timer.note_activity();
                iced::Task::none()
            }

            AppMessage::IdleTick => {
                // Fired every 500 ms; builds at most one screen per tick so
                // a single idle tick never causes a perceptible frame hitch.
                self.pre_warm_next_screen();
                iced::Task::none()
            }

            AppMessage::Noop => iced::Task::none(),

            #[cfg(feature = "dev-ui")]
            AppMessage::ToggleGallery => {
                self.screen = match self.screen {
                    Screen::Gallery => Screen::ChatList,
                    _ => Screen::Gallery,
                };
                iced::Task::none()
            }

            #[cfg(feature = "dev-ui")]
            AppMessage::GalleryPreset(preset) => {
                self.settings_state.gallery_state.preset = preset;
                iced::Task::none()
            }

            #[cfg(feature = "dev-ui")]
            AppMessage::GalleryCustomWidth(width) => {
                self.settings_state.gallery_state.preset = crate::component_gallery::GalleryWidthPreset::Custom;
                self.settings_state.gallery_state.custom_width = width;
                iced::Task::none()
            }

            #[cfg(feature = "dev-ui")]
            AppMessage::GalleryLayoutPreset(preset) => {
                self.settings_state.gallery_state.layout_preset = preset;
                // Gallery presets are previews of the same typed layout used
                // by production screens, so selecting one updates the shared
                // live configuration rather than a gallery-only copy.
                self.set_layout_overrides(preset.overrides());
                iced::Task::none()
            }

            // ── Shared file catalogue management ──
            // ── Shared file catalogue management (state layer) ──
            AppMessage::SharedByMeToggleShareMenu
            | AppMessage::AddSharedFile
            | AppMessage::AddSharedFolder
            | AppMessage::SharedFolderPicked(_)
            | AppMessage::SharedFilePicked(_)
            | AppMessage::SharedFileAddFailed(_)
            | AppMessage::SharedFileAdded(_)
            | AppMessage::RemoveSharedFile(_)
            | AppMessage::SharedFileRemoved(_) => self.update_files(message),

            AppMessage::NewDiscoveredPeers(_) => self.update_discover(message),
            // BORU-CP-07: backend reconnection success — ensure the direct
            // topic is joined/subscribed (data-plane action, friend-scoped).
            AppMessage::ReconnectPeerReady(_) => self.update_discover(message),

            // ── Chat log scroll (state layer) ──────────────────
            AppMessage::Scrolled(..) => self.update_chat(message),

            AppMessage::CopyToClipboard(text) => {
                self.video_card_menu_open = None;
                return iced::clipboard::write(text);
            }

            // ── Chat context menu + emoji/gif picker (state layer) ──
            AppMessage::CopyMessage(_)
            | AppMessage::RightClickText(_)
            | AppMessage::RightClickImage(_)
            | AppMessage::ContextCopyText(_)
            | AppMessage::ContextCopyImage(_)
            | AppMessage::CloseContextMenu
            | AppMessage::ToggleVideoCardMenu(_)
            | AppMessage::ToggleEmojiPicker
            | AppMessage::InsertEmoji(_)
            | AppMessage::SelectEmojiCategory(_)
            | AppMessage::EmojiSearchChanged(_)
            | AppMessage::ToggleGifPicker
            | AppMessage::GifSearchChanged(_)
            | AppMessage::GifSearchDebounced(_)
            | AppMessage::GifSearchSubmit
            | AppMessage::GifRetry
            | AppMessage::GifSearchResults { .. }
            | AppMessage::GifTrendingResults { .. }
            | AppMessage::GifSearchFailed { .. }
            | AppMessage::GifPreviewLoaded(..)
            | AppMessage::GifLoadMore
            | AppMessage::SendGif(_) => self.update_chat(message),

            AppMessage::CopyFriendId => self.update_contacts(message),

            AppMessage::FriendIdCopiedClear => self.update_contacts(message),

            // ── SENDME-02: BlobTicket wormhole sharing ────────────────────
            // ── SENDME-02 ticket sharing (state layer) ─────────
            AppMessage::CopyShareTicket(_)
            | AppMessage::OpenReceiveTicketDialog
            | AppMessage::CloseReceiveTicketDialog
            | AppMessage::ReceiveTicketInputChanged(_)
            | AppMessage::ReceiveTicketPreflight
            | AppMessage::ReceiveTicketPreflightDone(_)
            | AppMessage::ConfirmReceiveTicket => self.update_files(message),

            // ── Settings profile/home (state layer) ────────────────
            AppMessage::ToggleSound(_)
            | AppMessage::SetNotificationPolicy(_)
            | AppMessage::SetConversationNotificationPolicy(_, _)
            | AppMessage::TogglePresenceIndicator(_)
            | AppMessage::ToggleTypingIndicators(_)
            | AppMessage::ToggleInviteAddressSharing(_)
            | AppMessage::PickProfileImage
            | AppMessage::ProfileImagePicked(_)
            | AppMessage::PickHomeBackgroundImage
            | AppMessage::HomeBackgroundImagePicked(_)
            | AppMessage::HomeBackgroundImageReady { .. }
            | AppMessage::RemoveHomeBackgroundImage
            | AppMessage::SetHomeMenuItemOpacity(_)
            | AppMessage::ProfileImagePersisted { .. }
            | AppMessage::ProfileImageUploaded(_)
            | AppMessage::RemoveProfileImage
            | AppMessage::ProfileImageRemoved => self.update_settings(message),
            // ── History / conversation management (state layer) ──
            AppMessage::ClearHistoryRequested
            | AppMessage::ConfirmClearHistory
            | AppMessage::ClearHistoryFinished { .. }
            | AppMessage::ClearHistoryFailed { .. }
            | AppMessage::DeleteRoomRequested(_)
            | AppMessage::ConfirmDeleteRoom(_)
            | AppMessage::MailboxReplayed { .. }
            | AppMessage::OpenConversation(_)
            | AppMessage::SelectConversation(_)
            | AppMessage::CloseConversation(_) => self.update_chat(message),

            // ── SendMessage (state layer) ─────────────────────
            AppMessage::SendMessage { .. } => self.update_chat(message),

            AppMessage::ProfileSaved => {
                // Profile was saved — nothing more to do. The broadcast
                // already happened as part of SaveProfile handling.
                iced::Task::none()
            }
        };
        // Consume a pending scroll-to-bottom snap request. The flag is armed
        // when a conversation opens in follow-latest mode or when a live
        // entry is appended while the user is at the bottom. The windowed chat
        // log is top-anchored, so content growth alone cannot move the Iced
        // scrollable's viewport; the snap task actually drives the scrollable
        // to the latest entry so the newest message stays visible.
        let task = self.with_pending_snap(task);
        // Publish after applying the message so diagnostics observe the
        // resulting state (not the state that existed before the update).
        self.publish_gui_state();
        if let Some(action_id) = gui_action_timeout_id {
            with_gui_action_timeout(action_id, task)
        } else {
            task
        }
    }

    /// Shared broadcast helper: if sender_ready, spawn an async broadcast and
    /// return MessageSent; otherwise queue the message silently (it will be
    /// retried by the periodic retry loop). The preview_task is chained when
    /// present. Used by both the normal composer path and SendMessage.
    fn broadcast_or_queue(
        encoded: bytes::Bytes,
        sender: Option<GossipSender>,
        sender_ready: bool,
        neighbor_count: usize,
        text: String,
        event_id: u64,
        msg_hash: MessageHash,
        preview_task: Option<iced::Task<AppMessage>>,
    ) -> iced::Task<AppMessage> {
        if sender_ready && neighbor_count > 0 {
            if let Some(sender) = sender {
                let main_task = iced::Task::perform(
                    async move {
                        let accepted = match sender.broadcast(encoded).await {
                            Ok(()) => true,
                            Err(e) => {
                                warn!("broadcast failed: {e}");
                                false
                            }
                        };
                        if !accepted {
                            return None;
                        }
                        Some((text, event_id, msg_hash))
                    },
                    |result| match result {
                        Some((t, eid, mh)) => AppMessage::MessageSent(t, eid, mh),
                        None => AppMessage::Noop,
                    },
                );
                if let Some(pt) = preview_task {
                    iced::Task::batch([main_task, pt])
                } else {
                    main_task
                }
            } else {
                // sender_ready was true but sender was None — shouldn't happen,
                // but handle gracefully by dropping the message into the outbox
                // for retry.
                preview_task.unwrap_or(iced::Task::none())
            }
        } else {
            // No active sender or neighbors — message stays in outbox, retry
            // loop picks it up after the mesh becomes available.
            preview_task.unwrap_or(iced::Task::none())
        }
    }

    /// Purge every persisted and in-memory store associated with a room.
    ///
    /// Room deletion is deliberately centralized in the core cleanup helper:
    /// removing only the visible room-list entry leaves chat history, queued
    /// messages, friend room metadata, or the active-room file behind.
    fn purge_room_history(&mut self, topic: TopicId) -> Result<(), String> {
        let event_ids: Vec<u64> = self
            .chat_history
            .lock()
            .unwrap()
            .for_topic(&topic)
            .into_iter()
            .map(|entry| entry.event_id)
            .collect();
        if let Some(storage) = &self.storage {
            storage
                .delete_chat_history(topic.as_bytes(), &event_ids)
                .map_err(|err| err.to_string())?;
        }

        // Clean up outgoing messages in storage before mutating in-memory stores
        if let Some(storage) = &self.storage {
            let _ = storage.delete_outgoing_for_topic(&topic);
        }

        let report = {
            let mut chat_history = self.chat_history.lock().unwrap();
            delete_room_history(
                &self.data_dir,
                topic,
                &mut self.room_history,
                &mut chat_history,
                None,
                Some(&mut self.friends),
            )
            .map_err(|err| err.to_string())?
        };

        // The cleanup helper mutates the stores first; persist each store whose
        // contents changed so a restart cannot resurrect the deleted room data.
        if report.chat_entries_removed > 0 {}
        if report.friend_records_updated > 0 {
            self.mark_friends_sidebar_dirty();
            self.send_save_friends();
            self.send_save_conversations();
            self.send_save_chat_history();
            self.friends_dirty = false;
        }

        // RoomHistoryStore::save is intentionally a no-op for the removed
        // legacy file; the core helper has already removed the active-room file.
        self.room_history_dirty = false;
        Ok(())
    }

    fn persist_room_history(&mut self) {
        self.room_history_dirty = false;
    }

    fn update_room_preview(&mut self, topic: &TopicId, event: &NetEvent) {
        if let NetEvent::Message {
            from,
            message: Message::Message { text },
            ..
        } = event
        {
            let preview = if text.len() > 60 {
                format!("{}…", &text[..60])
            } else {
                text.clone()
            };

            // Check if this is a group conversation to capture sender name
            let is_group = self
                .conversation_store
                .find(topic)
                .map(|entry| {
                    matches!(
                        entry.kind,
                        boru_core::conversations::ConversationKind::Group
                    )
                })
                .unwrap_or(false);

            if is_group {
                // Look up sender display name from friends or profile cache
                let sender_name = if *from == self.local_public {
                    self.local_label.clone()
                } else {
                    let fid = FriendId::from_public_key(*from);
                    self.friends
                        .get(&fid)
                        .map(|record| record.display_label(&fid, from))
                        .unwrap_or_else(|| {
                            self.profile_cache
                                .get(from)
                                .map(|p| p.display_name.clone())
                                .unwrap_or_else(|| from.fmt_short().to_string())
                        })
                };
                self.room_history
                    .update_preview_with_sender(topic, &sender_name, &preview);
            } else {
                self.room_history.update_preview(topic, &preview);
            }
            self.room_history_dirty = true;
        }
    }

    /// Process a single `NetEvent` with all synchronous post-processing:
    /// conversation ordering, room preview, callback dispatch, delivery
    /// state transitions, and persistence saves.
    ///
    /// Async operations (auto ReadReceipt broadcast) are returned as an
    /// optional `Task`; the caller should batch it alongside other pending
    /// Tasks via `iced::Task::batch()`.
    ///
    /// This is the shared kernel used both by the single-event handler
    /// and by the batch-replay during room switch, ensuring consistent
    /// logic across both paths.
    /// Returns true if the event should increment the unread counter —
    /// only actual user messages (text, file, image, whisper), not
    /// gossip protocol events like AboutMe/Presence/Heartbeat.
    fn _is_user_visible_event(event: &NetEvent) -> bool {
        match event {
            NetEvent::Message { message, .. } => matches!(
                message,
                crate::Message::Message { .. }
                    | crate::Message::FileShare { .. }
                    | crate::Message::ImageShare { .. }
                    | crate::Message::SharedGif { .. }
            ),
            // NeighborUp/Down, Closed, Error are never user messages.
            _ => false,
        }
    }

    fn process_net_event_sync(
        &mut self,
        topic: &TopicId,
        event: &NetEvent,
    ) -> Option<iced::Task<AppMessage>> {
        // BORU-CP-13: feed the per-peer diagnostics snapshot from real
        // data-plane events — inbound gossip (any topic event from the
        // peer) and successfully decoded application messages. Timestamp
        // only; these never move the connectivity state machine and never
        // carry message content.
        self.report_net_diagnostics(event);
        if let NetEvent::Message { from, message, .. } = event {
            let msg_hash = message_hash(message);
            if Self::_is_user_visible_event(event) {
                info!(
                    topic = %topic,
                    message_hash = ?msg_hash,
                    local_peer = %self.local_public.fmt_short(),
                    neighbor_count = self.neighbors.len(),
                    sender_ready = self.sender_ready,
                    receive_decode_result = "ok",
                    persistence_result = "pending",
                    "message delivery telemetry"
                );
            } else {
                // Protocol traffic (AboutMe, Presence, Heartbeat,
                // LatencyPing, NeighborUp/Down, ...) is exchanged ~1/s
                // between peers while a conversation is active;
                // log it at trace so INFO stays readable.
                trace!(
                    topic = %topic,
                    message_hash = ?msg_hash,
                    local_peer = %self.local_public.fmt_short(),
                    neighbor_count = self.neighbors.len(),
                    sender_ready = self.sender_ready,
                    "protocol message delivery telemetry"
                );
            }
            debug!(from = %from.fmt_short(), "decoded gossip message");
        }
        if let NetEvent::Message { from, .. } = event {
            if *from != self.local_public && direct_topic(&self.local_public, from) == *topic {
                let fid = FriendId::from_public_key(*from);
                let label = self
                    .friends
                    .get(&fid)
                    .map(|record| record.display_label(&fid, from))
                    .unwrap_or_else(|| from.fmt_short().to_string());
                self.friends.ensure_friend(fid.clone());
                // Only auto-create / bump the conversation entry when the
                // direct conversation is still Active.  Archived means the
                // user explicitly deleted the chat and we must not resurrect
                // it on the next incoming message.
                let is_active = self
                    .friends
                    .get(&fid)
                    .and_then(|r| r.direct_conversation())
                    .is_some_and(|conv| {
                        conv.topic == *topic && conv.state == DirectConversationState::Active
                    });
                if is_active {
                    self.conversation_store.upsert(ConversationEntry::new(
                        *topic,
                        from.to_string(),
                        label,
                    ));
                    self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                }
            }
        }
        // ── RoomAdvertisement handling ──
        if let NetEvent::Message {
            from,
            message: Message::RoomAdvertisement { ad, signature },
            ..
        } = event
        {
            if verify_advertisement(ad, signature, *from) {
                let mut store = self.directory_store.lock().unwrap();
                store.upsert(ad.clone(), *from);
                trace!(
                    "upserted RoomAdvertisement from {} for room {}",
                    from.fmt_short(),
                    ad.room_name
                );
            } else {
                trace!(
                    "RoomAdvertisement signature verification failed from {}",
                    from.fmt_short()
                );
            }
            return None;
        }

        // ── RoomWithdrawal handling (BORU-DIR-09, PDF Task 3.3) ──
        // A verified withdrawal removes the matching advertisement
        // immediately; TTL expiry remains the safety net if it is missed.
        if let NetEvent::Message {
            from,
            message:
                Message::RoomWithdrawal {
                    topic: withdrawn_topic,
                    signature,
                },
            ..
        } = event
        {
            if verify_room_withdrawal(withdrawn_topic, signature, *from) {
                let removed = self
                    .directory_store
                    .lock()
                    .unwrap()
                    .withdraw(*withdrawn_topic, *from);
                if removed {
                    trace!(
                        "removed RoomAdvertisement from {} for room {}",
                        from.fmt_short(),
                        withdrawn_topic
                    );
                }
            } else {
                trace!(
                    "RoomWithdrawal signature verification failed from {}",
                    from.fmt_short()
                );
            }
            return None;
        }

        // Only bump conversation ordering for user-visible messages, not
        // protocol noise (NeighborUp/Down, Presence, AboutMe, etc.).
        if Self::_is_user_visible_event(event) {
            self.conversation_store.touch_and_bump(topic);
            self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        }
        self.update_room_preview(topic, event);
        let safety = self.public_room_safety.clone();
        if let Err(err) = handle_net_event_with_safety_for_topic(
            event.clone(),
            self,
            safety.as_deref(),
            Some(*topic),
        ) {
            warn!(error = %err, "failed to handle network event");
        }

        // Route ordinary incoming content through the notification service.
        // Keep protocol/control traffic silent: receipts, presence, typing,
        // reactions and pin operations are state updates, not new messages.
        if let NetEvent::Message { from, message, .. } = event {
            let notifiable = matches!(
                message,
                Message::Message { .. }
                    | Message::Reply { .. }
                    | Message::FileShare { .. }
                    | Message::ImageShare { .. }
                    | Message::SharedGif { .. }
                    | Message::MessageWithMentions { .. }
            );
            if *from != self.local_public && notifiable {
                self.emit_message_notification(topic, from, message);
            }
        }

        // ── Delivery state transitions ──
        // Echo: our own broadcast returning via gossip → Delivered
        if let NetEvent::Message { from, message, .. } = event {
            if *from == self.local_public {
                let msg_hash = message_hash(message);
                if let Some(&event_id) = self.self_sent_events.get(&msg_hash) {
                    if let Some(&index) = self.event_id_to_index.get(&event_id) {
                        if let Some(entry) = self.entries.get_mut(index) {
                            if entry.delivery_state == DeliveryState::Sent {
                                entry.delivery_state = DeliveryState::Delivered;
                                entry.bump_gen();
                                let mut store = self.chat_history.lock().unwrap();
                                let _ =
                                    store.update_delivery_state(event_id, DeliveryState::Delivered);
                                // Keep SQLite outgoing_messages in sync
                                if let Some(storage) = &self.storage {
                                    let _ = storage
                                        .update_outgoing_delivery_state(event_id, "delivered");
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── Auto ReadReceipt: when user is viewing the chat,
        // send ReadReceipt for incoming remote text messages ──
        let read_receipt_task = if self.follow_latest {
            if let NetEvent::Message { from, message, .. } = event {
                if *from != self.local_public {
                    if let crate::Message::Message { .. } = message {
                        let msg_hash = message_hash(message);
                        if let Some(ref sender) = self.sender {
                            let sk = self.secret_key.clone();
                            let s = sender.clone();
                            Some(iced::Task::perform(
                                async move {
                                    if let Ok(encoded) = SignedMessage::sign_and_encode(
                                        &sk,
                                        &crate::Message::ReadReceipt {
                                            message_hash: msg_hash,
                                        },
                                    ) {
                                        s.broadcast(encoded).await.ok();
                                    }
                                },
                                |_| AppMessage::Noop,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // ReadReceipt from peer → Seen
        if let NetEvent::Message {
            message:
                Message::ReadReceipt {
                    message_hash: receipt_hash,
                },
            from: receipt_from,
            ..
        } = event
        {
            if *receipt_from != self.local_public {
                if let Some(&index) = self.message_hash_to_index.get(receipt_hash) {
                    if let Some(entry) = self.entries.get_mut(index) {
                        if entry.delivery_state.can_transition_to(&DeliveryState::Seen) {
                            entry.delivery_state = DeliveryState::Seen;
                            entry.bump_gen();
                            let mut store = self.chat_history.lock().unwrap();
                            let _ =
                                store.update_delivery_state(entry.event_id, DeliveryState::Seen);
                            // Keep SQLite outgoing_messages in sync
                            if let Some(storage) = &self.storage {
                                let _ =
                                    storage.update_outgoing_delivery_state(entry.event_id, "seen");
                            }
                        }
                    }
                }
            }
        }

        // A neighbor disappearing is not a permanent delivery failure.  Keep
        // queued messages queued so the outbox can retry when the mesh returns.

        // ── LatencyPing → auto-respond with LatencyPong ──
        // When we receive a LatencyPing, broadcast a pong with the same
        // sent_at_ms so the sender can measure round-trip time.
        let latency_pong_task = match event {
            NetEvent::Message {
                from,
                message: crate::Message::LatencyPing { sent_at_ms },
                ..
            } if *from != self.local_public => {
                // Copy before entering the 'static closure
                let ping_ts = *sent_at_ms;
                if let Some(ref sender) = self.sender {
                    let sk = self.secret_key.clone();
                    let s = sender.clone();
                    Some(iced::Task::perform(
                        async move {
                            if let Ok(encoded) = SignedMessage::sign_and_encode(
                                &sk,
                                &crate::Message::LatencyPong {
                                    sent_at_ms: ping_ts,
                                },
                            ) {
                                s.broadcast(encoded).await.ok();
                            }
                        },
                        |_| AppMessage::Noop,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        };

        self.try_save_friends();
        // Combine tasks: if both read_receipt and latency_pong exist, chain them.
        // Otherwise return whichever is Some.
        match (read_receipt_task, latency_pong_task) {
            (Some(a), Some(b)) => Some(iced::Task::batch(vec![a, b])),
            (a, b) => a.or(b),
        }
    }

    /// If the entry at `entry_index` contains a URL, check the cache and
    /// spawn a background task to fetch a link preview.
    /// Returns `Some(task)` if a fetch was started, `None` otherwise.
    fn maybe_fetch_link_preview(&mut self, entry_index: usize) -> Option<iced::Task<AppMessage>> {
        if entry_index >= self.entries.len() {
            return None;
        }
        // Don't re-fetch if we already tried and failed, or are still loading
        if self.entries[entry_index].link_preview_loading
            || self.entries[entry_index].link_preview_error
            || self.entries[entry_index].link_preview.is_some()
        {
            return None;
        }
        let body = self.entries[entry_index].body.clone();
        let first_url = match link_preview::find_first_url(&body) {
            Some(url) => {
                // Do not put query strings, fragments, or tracking tokens in
                // local logs. The fetcher performs the actual URL validation.
                tracing::info!(entry_index, "link preview: found URL in message body");
                url
            }
            None => return None,
        };

        // Check cache first
        if self
            .link_preview_cache
            .lock()
            .ok()?
            .get(&first_url)
            .is_some()
        {
            // Already cached, mark the entry
            if let Some(data) = self.link_preview_cache.lock().ok()?.get(&first_url) {
                if let link_preview::LinkPreviewResult::Success(d) = data {
                    self.entries[entry_index].link_preview = Some(d);
                }
            }
            return None;
        }

        // In-flight dedup and concurrency limiting are handled inside
        // `fetch_link_preview` — it registers the URL in a shared static set
        // and acquires a semaphore permit. If the URL is already being fetched,
        // it returns `LinkPreviewResult::Pending`, which the message handler
        // treats as a no-op (the first fetch will populate the cache).
        self.entries[entry_index].link_preview_loading = true;

        let cache = self.link_preview_cache.clone();
        Some(iced::Task::perform(
            async move {
                let url_to_fetch = first_url.clone();
                let result = link_preview::fetch_link_preview(&url_to_fetch).await;
                // Cache the result regardless of success/failure.
                // `Pending` results are not stored (cache internals filter them).
                cache.lock().ok()?.insert(&url_to_fetch, result.clone());
                Some(AppMessage::LinkPreviewLoaded(entry_index, result))
            },
            |opt_msg| opt_msg.unwrap_or(AppMessage::Noop),
        ))
    }

    /// Create a background task to download a profile image blob from a peer.
    /// Returns an `AppMessage::ProfileImageDownloaded` or
    /// `AppMessage::ProfileImageDownloadFailed` when done.
    fn download_profile_image_task(
        blob_store: &FsStore,
        endpoint: &iroh::Endpoint,
        memory_lookup: &MemoryLookup,
        neighbors: &HashSet<PublicKey>,
        safety: &Option<Arc<PublicRoomSafety>>,
        peer: PublicKey,
        ticket_str: String,
    ) -> iced::Task<AppMessage> {
        let blob_store = blob_store.clone();
        let endpoint = endpoint.clone();
        let memory_lookup = memory_lookup.clone();
        let neighbors = neighbors.clone();
        let safety = safety.clone();
        let failed_peer = peer;
        iced::Task::perform(
            async move {
                use boru_core::chat_callbacks::TransferKind;
                let ticket: BlobTicket = ticket_str
                    .parse::<BlobTicket>()
                    .map_err(|e| format!("Parse profile image ticket: {e}"))?;
                seed_memory_lookup(&memory_lookup, &[ticket.addr().clone()]);
                let peer_id = ticket.addr().id;
                let candidates = download_candidates(peer_id, &neighbors);
                download_blob_with_safety(
                    &blob_store,
                    &endpoint,
                    ticket.hash(),
                    candidates,
                    "profile-image".into(),
                    TransferKind::Image,
                    |_| {},
                    safety.as_deref(),
                    peer_id,
                )
                .await
                .map_err(|e| format!("Download profile image: {e}"))?;
                let mut reader = blob_store.blobs().reader(ticket.hash());
                let mut buf = Vec::new();
                use tokio::io::AsyncReadExt;
                reader
                    .read_to_end(&mut buf)
                    .await
                    .map_err(|e| format!("Read profile image: {e}"))?;
                Ok((peer, buf))
            },
            move |r: Result<(PublicKey, Vec<u8>), String>| match r {
                Ok((peer, data)) => AppMessage::ProfileImageDownloaded(peer, data),
                Err(_) => AppMessage::ProfileImageDownloadFailed(failed_peer),
            },
        )
    }

    fn retry_stale_profile_images(&mut self) {
        use std::time::Instant;
        const RETRY_COOLDOWN_SECS: u64 = 5;
        let now = Instant::now();
        for (fid, record) in self.friends.iter() {
            let Some(ticket) = record.last_announced_profile_image_ticket.as_ref() else {
                continue;
            };
            if ticket.is_empty() {
                continue;
            }
            let Ok(peer) = fid.parse_public_key() else {
                continue;
            };
            // Skip if we already have a cached handle for this peer
            if self
                .friend_image_handles
                .get(&peer)
                .is_some_and(|h| h.is_some())
            {
                continue;
            }
            // If never retried before, allow immediate retry
            let never_retried = !self.last_failed_profile_retry.contains_key(&peer);
            let last = self
                .last_failed_profile_retry
                .get(&peer)
                .copied()
                .unwrap_or(now);
            if !never_retried && now.duration_since(last).as_secs() < RETRY_COOLDOWN_SECS {
                continue;
            }
            self.last_failed_profile_retry.insert(peer, now);
            // Avoid re-pushing if already queued
            let already_queued = self
                .pending_profile_image_tickets
                .iter()
                .any(|(p, _)| *p == peer);
            if !already_queued {
                self.pending_profile_image_tickets
                    .push_back((peer, ticket.clone()));
            }
        }
    }

    fn try_save_friends(&mut self) {
        if self.friends_dirty {
            // Authorization is derived from friends.json at inbox receipt
            // time, so persist relationship/key changes before returning to
            // the event loop.  An async snapshot can race an incoming
            // message and leave a newly accepted contact unauthorized.
            self.friends_dirty = false;
            self.send_save_friends();
            self.send_save_conversations();
            self.send_save_chat_history();
        }
    }

    /// Flush any pending neighbor status changes from the debounce buffer.
    ///
    /// For each peer with a pending change, applies the *latest* state
    /// (online/offline) — intermediate transitions during the debounce
    /// window are collapsed into one visible transition.
    fn flush_pending_neighbor_status(&mut self) {
        let pending: Vec<(PublicKey, bool)> = self.pending_neighbor_status.drain().collect();
        if pending.is_empty() {
            return;
        }
        // Neighbor status changes affect friend online state, which the chats
        // and friends sidebars display — bump both revisions.
        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        self.friends_sidebar_revision = self.friends_sidebar_revision.wrapping_add(1);
        for (peer, online) in &pending {
            let fid = FriendId::from_public_key(*peer);
            if self.is_friend(peer) {
                if *online {
                    self.friends.mark_online(fid);
                } else {
                    self.friends.mark_offline(fid);
                }
                self.friends_dirty = true;
            }
            if *online {
                // PUBLIC-03: a peer never seen before (across restarts)
                // gets a distinct "New user" entry; known peers keep the
                // plain came-online entry on reconnects.
                if !self.note_peer_first_seen(*peer) {
                    let name = self.resolve_name(peer);
                    self.notifications_state.push_activity(format!("{name} came online"), ActivityKind::Online);
                }
            } else {
                let name = self.resolve_name(peer);
                self.notifications_state.push_activity(format!("{name} went offline"), ActivityKind::Offline);
            }
        }
    }
}

// ── Net event handling ────────────────────────────────────────────────

fn confirmed_direct_invite_addrs(
    local_public: PublicKey,
    friends: &FriendsStore,
    sender: PublicKey,
    topic: TopicId,
    addrs: &[EndpointAddr],
) -> Option<Vec<EndpointAddr>> {
    if topic != direct_topic(&local_public, &sender) {
        return None;
    }
    let fid = FriendId::from_public_key(sender);
    let known_addrs = friends
        .get(&fid)
        .map(|record| record.known_addrs.clone())
        .unwrap_or_default();
    Some(merge_bootstrap_peer_addrs(&known_addrs, addrs))
}

impl IcedChat {
    /// Decide whether a first-seen `peer` should trigger a "new peer" system
    /// message.
    ///
    /// Guards (mirrors the `new_starters` pattern):
    /// - Never announce our own node.
    /// - Never announce a peer more than once per room (dedup via
    ///   [`IcedChat::known_peers`], cleared on room leave).
    /// - Only announce friends, or peers that are part of the current room's
    ///   gossip mesh.  Random public-room participants who are not friends
    ///   stay silent — a busy public room churns with strangers and would
    ///   spam the log.
    fn should_announce_new_peer(&self, peer: &PublicKey) -> bool {
        if *peer == self.local_public {
            return false;
        }
        if self.known_peers.contains(peer) {
            return false;
        }
        if self.is_friend(peer) {
            return true;
        }
        if self.topic == Self::default_lobby_topic() {
            return false;
        }
        self.neighbors.contains(peer)
    }

    /// Resolve a peer identifier (public key string or friend alias) to a [`PublicKey`].
    fn resolve_peer_key(&self, target: &str) -> Option<PublicKey> {
        if let Ok(pk) = target.parse::<PublicKey>() {
            return Some(pk);
        }
        // Try to resolve by friend alias.
        self.friends
            .iter()
            .find(|(_, rec)| rec.label.as_deref() == Some(target))
            .and_then(|(fid, _)| fid.parse_public_key().ok())
    }
    fn handle_friend_event(&mut self, event: FriendEvent) {
        info!(?event, "friend event received");
        match event {
            FriendEvent::StatusChanged { peer, status } => {
                let fid = FriendId::from_public_key(peer);
                let label = self
                    .friends
                    .get(&fid)
                    .map(|r| r.display_label(&fid, &peer))
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                // Only show system messages for runtime transitions, not the
                // initial scan. A friend with no last_seen_at or last_offline_at
                // is being heard from for the first time.
                let has_been_seen = self
                    .friends
                    .get(&fid)
                    .map(|r| {
                        r.status.last_seen_at_unix_ms.is_some()
                            || r.status.last_offline_at_unix_ms.is_some()
                    })
                    .unwrap_or(false);

                match status {
                    FriendStatus::Online => {
                        self.friends.mark_online(fid);
                        self.mark_friends_sidebar_dirty();
                        self.peer_presence_map.insert(peer, now_ms().max(0) as u64);
                        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                        if self.note_peer_first_seen(peer) {
                            // PUBLIC-03: first-ever sighting of this peer —
                            // the helper already pushed the "New user" Recent
                            // Activity entry and persisted the seen set. No
                            // system message (there is no prior state to
                            // announce a transition from) and no duplicate
                            // generic entry.
                        } else if has_been_seen {
                            self.push_system(format!("Friend {label} is now ONLINE"));
                            self.notifications_state.push_activity(
                                format!("{label} came online"),
                                ActivityKind::Online,
                            );
                        }
                    }
                    FriendStatus::Offline => {
                        self.friends.mark_offline(fid);
                        self.mark_friends_sidebar_dirty();
                        self.peer_presence_map.remove(&peer);
                        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                        if has_been_seen {
                            self.push_system(format!("Friend {label} is now offline"));
                            self.notifications_state.push_activity(
                                format!("{label} went offline"),
                                ActivityKind::Offline,
                            );
                        }
                    }
                    FriendStatus::Unknown => {}
                }
            }
            FriendEvent::AddressUpdated { peer, addr } => {
                self.friends
                    .ensure_friend(FriendId::from_public_key(peer))
                    .record_addrs([addr]);
                self.mark_friends_sidebar_dirty();
            }
        }
    }
}

// ── Profile cache methods ──────────────────────────────────────────────

impl IcedChat {
    /// Broadcast our own profile metadata (name, bio) via gossip.
    fn broadcast_profile_update(&mut self) -> iced::Task<AppMessage> {
        let sender = match self.sender.clone() {
            Some(s) => s,
            None => return iced::Task::none(),
        };
        let sk = self.secret_key.clone();
        let profile = self.profile_store.profile();
        let display_name = profile.display_name.clone();
        let bio = profile.bio.clone();
        let user_id = self.local_public;
        let shared_path = profile.shared_folder_path.clone();
        let shared_enabled = profile.file_sharing_enabled;
        let shared_files: Vec<_> = self
            .profile_store
            .shared_files()
            .iter()
            .filter(|file| file.is_announceable())
            .map(SharedFile::to_shared_file_meta)
            .collect();

        iced::Task::perform(
            async move {
                let profile = UserProfile {
                    user_id,
                    display_name,
                    bio,
                    avatar_identifier: None,
                    shared_folder_path: shared_path,
                    file_sharing_enabled: shared_enabled,
                    allow_downloads: false,
                    max_file_size: 100 * 1024 * 1024,
                    allowed_extensions: Vec::new(),
                    shared_files,
                };
                if let Ok(encoded) =
                    SignedMessage::sign_and_encode(&sk, &crate::Message::ProfileUpdate(profile))
                {
                    sender.broadcast(encoded).await.ok();
                }
            },
            |_| AppMessage::Noop,
        )
    }

    /// Remove cached profile entries for peers whose cached profile data is
    /// older than 1 hour (i.e. they've been offline longer than that).
    fn evict_stale_profile_cache(&mut self) {
        let cutoff = SystemTime::now() - Duration::from_secs(3600); // 1 hour
        self.profile_cache
            .retain(|_, data| data.last_updated >= cutoff);
    }
}

// ── ChatCallbacks impl for IcedChat ────────────────────────────────────

impl ChatCallbacks for IcedChat {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }

    fn on_typing(&mut self, topic: Option<TopicId>, peer: PublicKey, active: bool) {
        let Some(topic) = topic else { return };
        if active {
            self.typing_peers.set(topic, peer, Instant::now());
        } else {
            self.typing_peers.clear(topic, &peer);
        }
    }

    fn clear_typing_peer(&mut self, peer: &PublicKey) {
        self.typing_peers.clear_peer(peer);
    }

    fn room_allows(
        &self,
        topic: Option<TopicId>,
        peer: &PublicKey,
        permission: Permission,
    ) -> bool {
        topic
            .and_then(|topic| self.room_authorization.get(&topic))
            .map_or(true, |state| state.allows(peer, permission))
    }

    fn apply_room_authorization(&mut self, topic: Option<TopicId>, event: AuthorizationEvent) -> bool {
        let Some(topic) = topic else { return false; };
        let Some(state) = self.room_authorization.get_mut(&topic) else {
            tracing::debug!(%topic, "rejecting authorization event for unmanaged room");
            return false;
        };
        let previous = state.clone();
        if state.apply(&event).is_err() {
            return false;
        }
        if let Some(storage) = self.storage.as_ref() {
            if let Err(error) = storage.save_room_authorization(&topic, state, &event) {
                *state = previous;
                tracing::warn!(%topic, %error, "failed to persist room authorization event");
                return false;
            }
        }
        true
    }

    fn resolve_name(&self, peer: &PublicKey) -> String {
        // Priority: friend label > friend's last announced name > session name > short key.
        let fid = FriendId::from_public_key(*peer);
        if let Some(record) = self.friends.get(&fid) {
            if let Some(label) = &record.label {
                return label.clone();
            }
            if let Some(name) = &record.last_announced_name {
                return name.clone();
            }
        }
        self.names
            .get(peer)
            .cloned()
            .unwrap_or_else(|| peer.fmt_short().to_string())
    }

    fn last_announced_name(&self, peer: &PublicKey) -> Option<String> {
        let fid = FriendId::from_public_key(*peer);
        self.friends
            .get(&fid)
            .and_then(|record| record.last_announced_name.clone())
            .or_else(|| self.names.get(peer).cloned())
    }

    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
    }

    fn is_friend(&self, peer: &PublicKey) -> bool {
        let fid = FriendId::from_public_key(*peer);
        self.friends.get(&fid).is_some()
    }

    fn friend_mark_online(&mut self, fid: FriendId) {
        self.friends.mark_online(fid);
    }

    fn friend_mark_offline(&mut self, fid: FriendId) {
        self.friends.mark_offline(fid);
    }

    fn friend_set_name(&mut self, fid: FriendId, name: String) {
        self.friends.set_last_announced_name(fid, name);
    }

    fn mark_friends_dirty(&mut self) {
        self.friends_dirty = true;
        self.friends_sidebar_revision = self.friends_sidebar_revision.wrapping_add(1);
    }

    fn record_profile_image_ticket(&mut self, peer: PublicKey, ticket: String) {
        let fid = FriendId::from_public_key(peer);
        self.friends
            .set_last_announced_profile_image_ticket(fid, &ticket);
        // Compare against the last ticket seen for this peer to avoid
        // re-invalidating + re-downloading when the same ticket is
        // re-announced in a periodic AboutMe broadcast (every ~5s via
        // ConnMonitorTick).  Repeated invalidation causes a flicker
        // between the avatar image and the fallback emoji while the
        // redundant download is in flight.
        if self.friend_image_tickets.get(&peer) == Some(&ticket) {
            return;
        }
        self.mark_friends_sidebar_dirty();
        self.friend_image_tickets.insert(peer, ticket.clone());
        // Keep the old handle while the new image downloads in the
        // background.  Only seed a None entry if we have never seen a
        // handle for this peer (first-time download), so the colored
        // fallback circle shows during the initial fetch.
        self.friend_image_handles.entry(peer).or_insert(None);
        // Bump the profile version so sidebar lazy dependencies
        // invalidate their cached elements and re-render with the
        // updated avatar as soon as the download completes.
        let ver = self.friend_profile_versions.entry(peer).or_insert(0);
        *ver = ver.wrapping_add(1);
        self.pending_profile_image_tickets.push_back((peer, ticket));
    }

    fn clear_profile_image(&mut self, peer: PublicKey) {
        let fid = FriendId::from_public_key(peer);
        self.friends
            .set_last_announced_profile_image_ticket(fid, "");
        self.mark_friends_sidebar_dirty();
        self.friend_image_handles.remove(&peer);
        self.friend_image_tickets.remove(&peer);
        self.friend_profile_versions.remove(&peer);
        self.pending_profile_image_tickets
            .retain(|(queued_peer, _)| *queued_peer != peer);
        // Remove the cached file so the stale image doesn't reappear on restart.
        let path = friend_profile_image_path(&self.data_dir, &peer);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("tmp"));
    }

    fn on_profile_update(&mut self, peer: PublicKey, profile: UserProfile) {
        // Skip blocked sharers
        if self.files_state.blocked_sharers.contains(&peer) {
            return;
        }
        // Store in cache for UI consumption
        self.profile_cache.insert(
            peer,
            PeerProfileData {
                display_name: profile.display_name,
                bio: profile.bio,
                last_updated: SystemTime::now(),
            },
        );
    }

    /// Debounced neighbor status change — queues the update instead of
    /// immediately marking friend status and pushing a system message.
    ///
    /// The queue is flushed on every [`AppMessage::ConnMonitorTick`] (~1s),
    /// so rapid flapping results in at most one visible transition per peer
    /// per second.
    fn on_neighbor_status_change(&mut self, peer: PublicKey, online: bool) {
        self.pending_neighbor_status.insert(peer, online);
        // Still update the neighbors set and needs_conn_refresh immediately
        // — these drive mesh health and connection counts, which need
        // real-time accuracy regardless of debouncing.
        if online {
            self.on_neighbor_up(peer);
        } else {
            self.on_neighbor_down(peer);
        }
    }

    fn push_system(&mut self, text: String) {
        let entry = ChatEntry::system(text);
        self.entries_push(entry);
    }

    fn push_remote(
        &mut self,
        peer: PublicKey,
        label: String,
        text: String,
        hash: Option<MessageHash>,
        sent_at: Option<u64>,
    ) {
        let entry = ChatEntry::remote(label, text, hash, sent_at, Some(peer));
        self.entries_push(entry);
    }

    fn persist_remote_message(
        &mut self,
        topic: Option<boru_core::proto::TopicId>,
        peer: PublicKey,
        hash: MessageHash,
        sent_at: u64,
        text: &str,
        signed_bytes: Option<Vec<u8>>,
        message_id: boru_core::chat_core::MessageId,
        reply_to: Option<boru_core::chat_core::MessageId>,
    ) {
        let _ = (message_id, reply_to);
        // Gossip and backfill both converge on handle_net_event, so this one
        // callback makes received messages durable across process restarts.
        let Some(topic) = topic else {
            warn!("received message without topic; skipping durable persistence");
            return;
        };
        // ── BORU-DISC-13 guard ────────────────────────────────────────
        // The internal discovery topic is NOT a conversation store. Even if
        // a discovery-topic payload somehow reached protocol dispatch (the
        // NetEvent guard should prevent it), never write it into the message
        // DB — discovery traffic must leave zero history side effects.
        if boru_core::discovery_topic::is_discovery_topic(topic) {
            warn!(
                topic = %topic,
                "skipping message-DB persistence for discovery-topic payload"
            );
            return;
        }
        let store_path = self.data_dir.join("message_store.db");
        match MessageStore::open(&store_path).and_then(|store| {
            store.insert_chat_message(
                &hash,
                topic.as_bytes(),
                &peer.as_bytes(),
                sent_at.saturating_mul(1000),
                "text",
                text,
                signed_bytes.as_deref(),
                None,
                &self.local_public.as_bytes(),
            )
        }) {
            Ok(true) => trace!(peer = %peer.fmt_short(), "persisted received message"),
            Ok(false) => trace!(peer = %peer.fmt_short(), "received message already persisted"),
            Err(err) => warn!(error = %err, "failed to persist received message"),
        }
    }

    fn persist_remote_thread_message(
        &mut self,
        topic: Option<boru_core::proto::TopicId>,
        peer: PublicKey,
        hash: MessageHash,
        sent_at: u64,
        text: &str,
        signed_bytes: Option<Vec<u8>>,
        target: Option<boru_core::threads::ThreadTarget>,
    ) {
        self.persist_remote_message(
            topic,
            peer,
            hash,
            sent_at,
            text,
            signed_bytes.clone(),
            hash,
            None,
        );
        if let Some(target) = target {
            let store_path = self.data_dir.join("message_store.db");
            if let Err(error) = MessageStore::open(&store_path)
                .and_then(|store| store.set_thread_target(&hash, &target))
            {
                warn!(%error, "failed to project incoming thread target");
            }
        }
        let (Some(storage), Some(topic), Some(target)) = (&self.storage, topic, target) else {
            return;
        };
        if boru_core::discovery_topic::is_discovery_topic(topic) {
            return;
        }
        if let Some(bytes) = signed_bytes {
            if let Err(error) = storage.record_thread_reply(
                topic.as_bytes(),
                &target.thread_root_id,
                false,
            ) {
                warn!(%error, "failed to update incoming thread unread state");
            }
            if let Err(error) = storage.insert_thread_message(
                &hash,
                topic.as_bytes(),
                peer.as_bytes(),
                sent_at.saturating_mul(1000),
                &bytes,
                Some(target),
            ) {
                warn!(%error, "failed to persist incoming thread relation");
            }
        }
    }

    fn persist_remote_file_share(
        &mut self,
        topic: Option<boru_core::proto::TopicId>,
        peer: PublicKey,
        hash: MessageHash,
        sent_at: u64,
        name: &str,
        signed_bytes: Option<Vec<u8>>,
    ) {
        let Some(topic) = topic else {
            warn!("received file share without topic; skipping durable persistence");
            return;
        };
        // ── BORU-DISC-13 guard ────────────────────────────────────────
        // Same invariant as persist_remote_message: the discovery topic is
        // never a conversation store, so file-share payloads on it must not
        // be persisted (or surface download cards / notifications).
        if boru_core::discovery_topic::is_discovery_topic(topic) {
            warn!(
                topic = %topic,
                "skipping message-DB persistence for discovery-topic file share"
            );
            return;
        }
        let store_path = self.data_dir.join("message_store.db");
        let result = MessageStore::open(&store_path).and_then(|store| {
            store.insert_chat_message(
                &hash,
                topic.as_bytes(),
                &peer.as_bytes(),
                sent_at.saturating_mul(1000),
                "file",
                name,
                signed_bytes.as_deref(),
                None,
                &self.local_public.as_bytes(),
            )
        });
        match result {
            Ok(true) => trace!(peer = %peer.fmt_short(), "persisted received file share"),
            Ok(false) => trace!(peer = %peer.fmt_short(), "received file share already persisted"),
            Err(err) => warn!(error = %err, "failed to persist received file share"),
        }
    }

    fn is_known_file_ticket(&self, ticket: &str) -> bool {
        // A video-poster follow-up re-announces the same ticket while the
        // original card is still pending (no thumbnail yet). Only treat
        // that exact case as known — a deliberate re-share of the same
        // file must still create a fresh card.
        self.entries.iter().any(|entry| {
            entry.download.as_ref().is_some_and(|dl| {
                dl.ticket == ticket && dl.thumbnail_hash.is_none() && !dl.state.is_terminal()
            })
        })
    }

    fn set_pending_file(
        &mut self,
        name: String,
        ticket: String,
        size: u64,
        thumbnail_hash: Option<MessageHash>,
        sender_label: Option<String>,
    ) {
        // Video-poster follow-up: the sender re-announces the same ticket
        // once the poster blob is ready. Upgrade the existing card instead
        // of pushing a duplicate entry.
        if let Some(idx) = self.entries.iter().position(|entry| {
            entry.download.as_ref().is_some_and(|dl| {
                dl.ticket == ticket && dl.thumbnail_hash.is_none() && !dl.state.is_terminal()
            })
        }) {
            if let Some(entry) = self.entries.get_mut(idx) {
                if let Some(dl) = entry.download.as_mut() {
                    dl.thumbnail_hash = thumbnail_hash;
                }
            }
            // Queue the sender's poster blob for an off-thread fetch, same
            // as the initial-card path below.
            if let Some(hash) = thumbnail_hash {
                self.pending_thumbnail_fetch
                    .push_back((idx, hash, ticket.clone()));
            }
            return;
        }
        self.pending_file = Some((name.clone(), ticket.clone()));
        self.download_entry_index = Some(self.entries.len());
        let xfer_kind = if classify_attachment(None, &name) == MediaKind::Video {
            TransferKind::Video
        } else {
            TransferKind::File
        };
        let mut entry = ChatEntry::system_download(
            format!("File received: {name}"),
            xfer_kind,
            name,
            ticket.clone(),
            sender_label.unwrap_or_default(),
            None, // thumbnail fetched asynchronously via blob
        );
        // Store the thumbnail hash so the poster can be fetched on demand.
        if let Some(dl) = entry.download.as_mut() {
            dl.state = DownloadState::Ready { total: Some(size) };
            dl.thumbnail_hash = thumbnail_hash;
        }
        let entry_index = self.download_entry_index.unwrap_or(self.entries.len());
        self.entries_push(entry);
        // Queue the sender's poster blob for an off-thread fetch. The card
        // shows the file-type placeholder while it is pending; on success
        // ThumbnailFetched populates the handle + dimensions.
        if let Some(hash) = thumbnail_hash {
            self.pending_thumbnail_fetch
                .push_back((entry_index, hash, ticket.clone()));
        }
    }

    fn set_pending_direct_offer(
        &mut self,
        offer_id: FileOfferId,
        name: String,
        size: u64,
        owner: PublicKey,
        sender_label: Option<String>,
    ) {
        if self.entries.iter().any(|entry| {
            entry
                .download
                .as_ref()
                .is_some_and(|download| download.direct_offer_key == Some((owner, offer_id)))
        }) {
            return;
        }
        self.download_entry_index = Some(self.entries.len());
        let mut entry = ChatEntry::system_download(
            format!("File received: {name}"),
            if classify_attachment(None, &name) == MediaKind::Video {
                TransferKind::Video
            } else {
                TransferKind::File
            },
            name,
            format!("direct-offer:{owner}:{offer_id:?}"),
            sender_label.unwrap_or_default(),
            None,
        );
        if let Some(download) = entry.download.as_mut() {
            download.state = DownloadState::Ready { total: Some(size) };
            download.availability = AttachmentAvailability::DirectOffer { owner, offer_id };
            download.direct_offer_key = Some((owner, offer_id));
        }
        self.entries_push(entry);
    }

    fn set_pending_direct_offer_ready(
        &mut self,
        offer_id: FileOfferId,
        ticket: String,
        thumbnail_hash: Option<MessageHash>,
        owner: PublicKey,
        sender_label: Option<String>,
    ) {
        if let Some(index) = self.entries.iter().position(|entry| {
            entry
                .download
                .as_ref()
                .is_some_and(|download| download.direct_offer_key == Some((owner, offer_id)))
        }) {
            let mut queue_thumbnail = false;
            if let Some(download) = self.entries[index].download.as_mut() {
                download.ticket = ticket.clone();
                download.expected_content_hash = content_hash_from_ticket(&ticket);
                download.availability = AttachmentAvailability::Hybrid {
                    owner,
                    offer_id,
                    ticket: ticket.clone(),
                };
                if thumbnail_hash.is_some() && download.thumbnail_hash != thumbnail_hash {
                    download.thumbnail_hash = thumbnail_hash;
                    queue_thumbnail = true;
                }
            }
            if queue_thumbnail {
                if let Some(hash) = thumbnail_hash {
                    self.pending_thumbnail_fetch
                        .push_back((index, hash, ticket));
                }
            }
            return;
        }

        // A ready event can arrive after the announcement was dropped. Keep
        // it as a normal blob card, but retain the key for ready-event replay
        // idempotency after the card reaches a terminal state.
        self.set_pending_file(
            "Shared file".to_owned(),
            ticket.clone(),
            0,
            thumbnail_hash,
            sender_label,
        );
        if let Some(index) = self.download_entry_index {
            if let Some(download) = self
                .entries
                .get_mut(index)
                .and_then(|entry| entry.download.as_mut())
            {
                download.direct_offer_key = Some((owner, offer_id));
            }
        }
    }

    fn set_pending_folder(
        &mut self,
        name: String,
        ticket: String,
        size: u64,
        collection_hash: Option<MessageHash>,
        collection_entries: u64,
        sender_label: Option<String>,
    ) {
        self.pending_file = Some((name.clone(), ticket.clone()));
        self.download_entry_index = Some(self.entries.len());
        let mut entry = ChatEntry::system_download(
            format!("Folder received: {name}"),
            TransferKind::File,
            name,
            ticket.clone(),
            sender_label.unwrap_or_default(),
            None,
        );
        if let Some(dl) = entry.download.as_mut() {
            dl.is_folder = true;
            dl.collection_entries = collection_entries;
            dl.expected_content_hash = collection_hash.map(|h| hex::encode(h));
            dl.state = DownloadState::Ready { total: Some(size) };
        }
        let entry_index = self.download_entry_index.unwrap_or(self.entries.len());
        self.entries_push(entry);
        let _ = entry_index;
    }

    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_image.push_back((name, hash, from));
    }

    fn set_pending_gif(
        &mut self,
        gif: boru_core::gif_provider::SharedGif,
        from: PublicKey,
        message_hash: MessageHash,
    ) {
        self.pending_gif.push_back((gif, from, message_hash));
    }

    fn has_message(&self, hash: &MessageHash) -> bool {
        self.message_hash_to_index.contains_key(hash)
    }

    fn edit_message(&mut self, hash: &MessageHash, new_text: String) {
        if let Some(&index) = self.message_hash_to_index.get(hash) {
            if let Some(entry) = self.entries.get_mut(index) {
                entry.body = new_text.clone();
                entry.edited = true;
                entry.bump_gen();
                self.layout_cache.borrow_mut().invalidate_all();
            }
        }
    }

    fn delete_message(&mut self, hash: &MessageHash) {
        if let Some(&index) = self.message_hash_to_index.get(hash) {
            #[cfg(feature = "video-playback")]
            if self.inline_video.as_ref().is_some_and(|session| {
                self.entries.get(index).is_some_and(|entry| {
                    session.key.conversation_id == self.topic
                        && session.key.message_id == entry.event_id
                })
            }) {
                self.stop_inline_video();
            }
            if let Some(entry) = self.entries.get_mut(index) {
                entry.body = "[message deleted]".to_string();
                entry.edited = false;
                entry.reactions.clear();
                entry.bump_gen();
                // Reactions cleared → height changes. Invalidating the whole
                // cache is fine since this is a rare user action.
                self.layout_cache.borrow_mut().invalidate_all();
            }
        }
    }

    fn add_reaction(&mut self, hash: &MessageHash, emoji: String) {
        if let Some(&index) = self.message_hash_to_index.get(hash) {
            if let Some(entry) = self.entries.get_mut(index) {
                entry.reactions.push(emoji);
                entry.bump_gen();
                // Reaction added → height may change (REACTION_EXTRA).
                self.layout_cache.borrow_mut().invalidate_all();
            }
        }
    }

    fn pin_message(
        &mut self,
        topic: TopicId,
        hash: MessageHash,
        author: PublicKey,
        sent_at: u64,
    ) {
        self.pinned_state
            .apply_authenticated(topic, hash, PinAction::Pin, author, sent_at);
        if let Some(storage) = &self.storage {
            let _ = storage.reconcile_pinned_message(topic, hash, author, "pin", sent_at);
        }
    }

    fn unpin_message(
        &mut self,
        topic: TopicId,
        hash: MessageHash,
        author: PublicKey,
        sent_at: u64,
    ) {
        self.pinned_state
            .apply_authenticated(topic, hash, PinAction::Unpin, author, sent_at);
        if let Some(storage) = &self.storage {
            let _ = storage.reconcile_pinned_message(topic, hash, author, "unpin", sent_at);
        }
    }

    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbors.insert(peer);
        self.peer_presence_map.insert(peer, now_ms().max(0) as u64);
        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        self.needs_conn_refresh = true;

        // Announce genuinely-new peers (first seen this session/room).
        // `should_announce_new_peer` filters out our own node, duplicates,
        // and non-friend public-room strangers.
        if self.should_announce_new_peer(&peer) {
            self.known_peers.insert(peer);
            let name = self.resolve_name(&peer);
            self.push_system(format!("{name} joined"));
        }

        // Queue the new neighbor for ticket regeneration. The actual
        // `endpoint.remote_info()` lookup is deferred to ConnMonitorTick
        // because this callback is sync; once resolved, the peer's
        // addressing info is embedded in the room ticket as an extra
        // bootstrap node.
        if !self.pending_ticket_peers.contains(&peer) {
            self.pending_ticket_peers.push(peer);
        }
        self.ticket_needs_regeneration = true;

        // For each pending backfill topic, check if we still need history
        // (the count may have been satisfied by a previous backfill) and
        // spawn a request if not.  Retain topics that still need backfill
        // so the next neighbor-up event can retry with a different peer.
        let mut i = 0;
        while i < self.pending_backfill_topics.len() {
            let topic = self.pending_backfill_topics[i];
            let count = {
                let store = self.chat_history.lock().unwrap();
                store.count_for_topic(&topic)
            };
            if count >= BACKFILL_TRIGGER_THRESHOLD {
                // Already satisfied — remove from pending.
                self.pending_backfill_topics.remove(i);
                continue;
            }
            IcedChat::spawn_backfill_request(self, topic, peer, count);
            i += 1;
        }
    }

    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbors.remove(&peer);
        self.peer_presence_map.remove(&peer);
        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        self.needs_conn_refresh = true;
        self.peer_latencies.remove(&peer);

        // Remove the peer from the ticket's extra bootstrap nodes (both
        // the resolved list and any pending resolution) and flag the ticket
        // for regeneration.
        self.ticket_extra_peers.retain(|addr| addr.id != peer);
        self.pending_ticket_peers.retain(|p| *p != peer);
        self.ticket_needs_regeneration = true;
    }

    fn record_activity(&mut self, peer: PublicKey) {
        // Update mesh health timestamp for this peer so the mesh
        // watchdog doesn't falsely flag them as stale.
        self.peer_presence_map.insert(peer, now_ms().max(0) as u64);
        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        self.neighbors.insert(peer);
    }

    fn record_presence(&mut self, peer: PublicKey) {
        // A Presence heartbeat proves the peer is still alive and
        // connected.  Update the presence map so the friend list
        // shows them as online, and ensure they're tracked as a
        // neighbor for mesh health purposes.
        self.peer_presence_map.insert(peer, now_ms().max(0) as u64);
        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        self.neighbors.insert(peer);

        // Announce genuinely-new peers (first presence seen this
        // session/room).  Same guard as on_neighbor_up: no self, no
        // duplicates, no non-friend public-room strangers.
        if self.should_announce_new_peer(&peer) {
            self.known_peers.insert(peer);
            let name = self.resolve_name(&peer);
            self.push_system(format!("{name} is online"));
        }
    }

    fn store_peer_ticket(&mut self, peer: PublicKey, ticket: Ticket) -> bool {
        let fid = FriendId::from_public_key(peer);
        let record = self.friends.ensure_friend(fid);
        record.record_addrs(ticket.peers.clone());
        record.record_room(ticket.topic, ticket);
        true
    }

    fn request_quit(&mut self) {
        // IcedChat handles window close through the iced framework.
    }

    fn on_latency_ping(&mut self, _peer: PublicKey, _sent_at_ms: u64) {
        // Pong sending is handled at the frontend event-loop level
        // in process_net_event where we have access to the gossip sender.
    }

    fn record_latency(&mut self, peer: PublicKey, latency: Duration) {
        self.peer_latencies.insert(peer, latency);
    }
}

impl IcedChat {
    /// Presence state for a peer, derived from the presence map's last-seen
    /// timestamp. Absent from the map → Offline; stale timestamp →
    /// Away; fresh timestamp → Online.
    fn peer_presence(&self, peer: &PublicKey) -> PeerPresence {
        match self.peer_presence_map.get(peer) {
            None => PeerPresence::Offline,
            Some(&last_seen) => {
                let now = now_ms().max(0) as u64;
                if now.saturating_sub(last_seen) > AWAY_THRESHOLD_MS {
                    PeerPresence::Away
                } else {
                    PeerPresence::Online
                }
            }
        }
    }

    /// UI presence for a peer (BORU-CP-06, PDF 2.3).
    ///
    /// When the optional presence indicator is enabled AND the backend
    /// connectivity store handle is available, the badge is derived from
    /// the BORU-CP-05 state machine (`Reachable`/`DirectTopicReady` →
    /// Online, `Discovered` → Recently seen, `Connecting` → Connecting,
    /// everything else → Offline). Otherwise it falls back to the legacy
    /// timestamp-based model — disabling the indicator never affects
    /// discovery or reconnection.
    fn ui_presence(&self, peer: &PublicKey) -> PeerPresence {
        if self.settings_state.show_presence_indicator {
            if let Some(store) = &self.connectivity_store {
                let state = {
                    let guard = store.lock().expect("connectivity store lock poisoned");
                    guard.state(peer)
                };
                return peer_presence_from_connectivity(state);
            }
        }
        self.peer_presence(peer)
    }

    /// BORU-CP-12 (PDF Task 4.3): the negotiated version of `feature` for
    /// `peer`, or `None` when the capability gate is absent, the peer is
    /// unknown/stale, does not advertise the feature, or shares no
    /// compatible version. `None` fails closed — the action is not
    /// offered/performed.
    fn negotiated_feature_version(&self, peer: &PublicKey, feature: &str) -> Option<u16> {
        self.capability_gate
            .as_ref()
            .and_then(|gate| gate.peer_supports(peer, feature))
    }

    /// BORU-CP-12: whether a peer-facing optional feature may be offered to
    /// `peer`. With no gate wired (unit tests / discovery unavailable) the
    /// legacy un-gated behaviour is kept; with a gate, the feature is only
    /// offered when the peer negotiates a compatible version.
    fn feature_offered(&self, peer: &PublicKey, feature: &str) -> bool {
        self.capability_gate.is_none() || self.negotiated_feature_version(peer, feature).is_some()
    }

    /// The direct-chat peer for the currently selected topic, when the
    /// selected conversation is a direct conversation. Used to gate
    /// optional features (file transfer) on the peer's negotiated support;
    /// returns `None` for groups/public rooms.
    fn current_direct_peer(&self) -> Option<PublicKey> {
        self.conversation_store
            .active_iter()
            .into_iter()
            .find(|entry| entry.topic == self.topic)
            .and_then(|entry| PublicKey::from_str(&entry.peer_id).ok())
    }

    /// BORU-CP-13: feed the per-peer diagnostics snapshot from real inbound
    /// data-plane events — any gossip event from the peer (message,
    /// NeighborUp, NeighborDown) refreshes `last_inbound_gossip`, and a
    /// decoded application message additionally refreshes
    /// `last_decoded_message`. Timestamp-only: never moves the
    /// connectivity state machine, never stores message content. No-op when
    /// no connectivity store is attached (unit tests, headless builds).
    fn report_net_diagnostics(&self, event: &NetEvent) {
        let Some(store) = &self.connectivity_store else {
            return;
        };
        let mut store = store.lock().unwrap();
        let now = Instant::now();
        match event {
            NetEvent::Message { from, .. } => {
                if *from == self.local_public {
                    return;
                }
                store.apply(*from, ConnectivityEvent::InboundGossipEvent, now);
                store.apply(*from, ConnectivityEvent::ApplicationMessageDecoded, now);
            }
            NetEvent::NeighborUp { peer, .. } | NetEvent::NeighborDown { peer, .. } => {
                if *peer == self.local_public {
                    return;
                }
                store.apply(*peer, ConnectivityEvent::InboundGossipEvent, now);
            }
            NetEvent::Closed | NetEvent::Error(_) => {}
        }
    }

    /// BORU-CP-13: record an outbound direct broadcast into the per-peer
    /// diagnostics snapshot (`last_outbound_direct`). Timestamp-only; no-op
    /// when no connectivity store is attached.
    fn report_direct_broadcast(&self, peer: PublicKey) {
        if let Some(store) = &self.connectivity_store {
            let mut store = store.lock().unwrap();
            store.apply(peer, ConnectivityEvent::DirectMessageSent, Instant::now());
        }
    }

    /// Downgrade stale peers to Away. Called on each ConnMonitorTick:
    /// peers whose last-seen is older than AWAY_THRESHOLD_MS are marked
    /// Away (but stay in the map — removal happens on NeighborDown only).
    /// Bumps sidebar revisions when the away set changes so the UI
    /// re-renders the Online → Away transition.
    fn refresh_peer_presence(&mut self) {
        let now = now_ms().max(0) as u64;
        let mut away: HashSet<PublicKey> = HashSet::new();
        for (peer, &last_seen) in &self.peer_presence_map {
            if now.saturating_sub(last_seen) > AWAY_THRESHOLD_MS {
                away.insert(*peer);
            }
        }
        if away != self.presence_away_peers {
            self.presence_away_peers = away;
            self.mark_friends_sidebar_dirty();
            self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        }
    }

    /// Current item count for a sidebar section index
    /// (0 chats, 1 groups, 2 friends, 3 discover, 4 requests, 5 public rooms).
    fn sidebar_section_count(&self, index: usize) -> usize {
        match index {
            0 => self.cached_chat_count,
            1 => self.cached_group_count,
            2 => self.cached_friend_count,
            3 => self.cached_discover_count,
            4 => self.cached_request_count,
            5 => self.cached_public_room_count,
            _ => 0,
        }
    }

    /// True while any sidebar section is playing its appearance animation.
    /// The `SplashTick` subscription in `main.rs` stays alive while this is
    /// true so the fade counters keep advancing to `SIDEBAR_FADE_FRAMES`.
    pub(crate) fn sidebar_fade_active(&self) -> bool {
        self.sidebar_fade_frame
            .iter()
            .any(|&frame| frame < crate::ui_components::SIDEBAR_FADE_FRAMES)
    }

    /// Recompute all cached sidebar counts from the underlying store data.
    /// Call this when a sidebar revision counter changes so the next
    /// `view_sidebar()` render uses up-to-date cached values.
    ///
    /// SIDEBAR-01: while here, detect sections that just gained their first
    /// item (count 0 → > 0) and auto-expand them (empty sections are rendered
    /// collapsed) while starting their appearance animation.
    fn refresh_sidebar_counts(&mut self) {
        let new_chat_count = self
            .conversation_store
            .active_iter()
            .into_iter()
            .filter(|e| !matches!(e.kind, ConversationKind::Group))
            .count();
        let new_group_count = self
            .conversation_store
            .active_iter()
            .into_iter()
            .filter(|e| matches!(e.kind, ConversationKind::Group))
            .count();
        let new_friend_count = self
            .friends
            .iter()
            .filter(|(_, r)| r.relationship.can_message())
            .count();
        let new_discover_count = self.discovered_peers.len();
        // BORU-DIR-13 (PDF 5.1): the PUBLIC ROOMS count badge reflects the
        // browse surface (bounded RoomDirectory cache) when the discovery
        // service provided a read handle; fall back to the legacy directory
        // store (tests / discovery service unavailable).
        let new_public_room_count = match &self.room_directory {
            Some(dir) => dir.lock().unwrap().snapshot().len(),
            None => self.directory_store.lock().unwrap().len(),
        };
        let new_request_count = self
            .friend_request_store
            .list_incoming_by_status(
                &self.local_public.to_string(),
                boru_core::friend_request::FriendRequestStatus::Pending,
            )
            .len()
            + self
                .storage
                .as_ref()
                .map(|st| {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    st.get_pending_group_invites(&self.local_public.to_vec(), now_ms)
                        .unwrap_or_default()
                        .len()
                })
                .unwrap_or(0);

        // NOTE: array position must match the canonical sidebar section index
        // used by `sidebar_section_count` and the render code — 4 = REQUESTS,
        // 5 = PUBLIC ROOMS. Swapping these two makes the 0 → >0 auto-expand /
        // fade transition fire on the wrong section (SIDEBAR-01 review fix).
        let old_counts = [
            self.cached_chat_count,
            self.cached_group_count,
            self.cached_friend_count,
            self.cached_discover_count,
            self.cached_request_count,
            self.cached_public_room_count,
        ];
        let new_counts = [
            new_chat_count,
            new_group_count,
            new_friend_count,
            new_discover_count,
            new_request_count,
            new_public_room_count,
        ];
        let fade_frames = crate::ui_components::SIDEBAR_FADE_FRAMES;
        for (i, (&old, &new)) in old_counts.iter().zip(new_counts.iter()).enumerate() {
            if old == 0 && new > 0 {
                // First item arrived: auto-expand (empty sections are rendered
                // collapsed) and start the appearance animation. With
                // reduced-motion the section just appears instantly.
                self.sidebar_section_collapsed[i] = false;
                self.sidebar_fade_frame[i] = if self.reduced_motion { fade_frames } else { 0 };
            }
        }

        self.cached_chat_count = new_chat_count;
        self.cached_group_count = new_group_count;
        self.cached_friend_count = new_friend_count;
        self.cached_discover_count = new_discover_count;
        self.cached_public_room_count = new_public_room_count;
        self.cached_request_count = new_request_count;
    }
}

// ── View ──────────────────────────────────────────────────────────────

#[expect(dead_code)]
impl IcedChat {
    /// Spawn a background task to request history backfill from a peer.
    pub(crate) fn spawn_backfill_request(
        &self,
        topic: TopicId,
        peer: PublicKey,
        local_count: usize,
    ) {
        let bf_handle = self.backfill_handle.clone();
        let endpoint = self.endpoint.clone();
        let net_tx = self.net_tx.clone();
        tokio::task::spawn(async move {
            let (bf_tx, mut bf_rx) = tokio::sync::mpsc::channel::<crate::NetEvent>(256);
            match bf_handle
                .try_backfill_from_peer(&endpoint, peer, local_count, topic, bf_tx, None)
                .await
            {
                Ok(Some(count)) => {
                    tracing::debug!(
                        ?count,
                        topic = %topic,
                        peer = %peer.fmt_short(),
                        "backfill: received history"
                    );
                }
                Ok(None) => {
                    tracing::debug!(
                        topic = %topic,
                        peer = %peer.fmt_short(),
                        "backfill: peer had no remote_info or not needed"
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        %e,
                        topic = %topic,
                        peer = %peer.fmt_short(),
                        "backfill: request failed"
                    );
                }
            }
            // Forward backfill messages into the conversation event
            // channel so the app processes them as if they arrived over
            // gossip.
            drop(bf_handle);
            while let Some(event) = bf_rx.recv().await {
                let conv_event = boru_core::conversations::ConversationNetEvent::new(topic, event);
                if net_tx.send(conv_event).await.is_err() {
                    break;
                }
            }
        });
    }

    /// Muted secondary text color, adapted to current theme.
    fn color_muted(&self) -> Color {
        if self.dark_mode {
            Color::from_rgb(0.6, 0.6, 0.6)
        } else {
            Color::from_rgb(0.4, 0.4, 0.4)
        }
    }

    /// Return the iced Theme enum matching the current dark_mode toggle.
    fn theme(&self) -> iced::Theme {
        if self.dark_mode {
            iced::Theme::Dark
        } else {
            iced::Theme::Light
        }
    }

    /// Typed Boru theme matching the current dark-mode toggle.
    ///
    /// This is the seam view/style code consumes instead of raw literals
    /// (BORU-UI-02). BORU-UI-07 stores the LIVE merged theme
    /// (`BoruTheme::default()`/`for_theme` + `boru-ui.toml` overrides) in
    /// `self.active_theme`; this accessor is a single-field Copy read per
    /// frame. `set_ui_theme_config` / the reload handler replace it so the
    /// normal Iced state/update/view cycle redraws affected widgets.
    pub(crate) fn boru_theme(&self) -> crate::theme::BoruTheme {
        self.active_theme
    }

    /// Typed structural layout matching the current app state (BORU-LAYOUT-03).
    ///
    /// Mirror of [`IcedChat::boru_theme`]: view code reads the LIVE layout
    /// (`LayoutConfig::default()` now; `boru-layout.toml` overrides in a
    /// later BORU-LAYOUT task) through this accessor each frame. The layout
    /// is Clone-only (not Copy), so callers that need to move it into a
    /// renderer closure clone the slice they consume.
    pub(crate) fn boru_layout(&self) -> &crate::layout::LayoutConfig {
        &self.active_layout
    }

    #[cfg(feature = "dev-ui")]
    fn update_home_drag(&mut self, current: iced::Point) {
        let Some(operation) = self.settings_state.designer.drag_operation.as_mut() else {
            return;
        };
        // The whole-card overlay reports pointer positions relative to the
        // card widget, and StartDrag passes Point::ORIGIN, so anchor the
        // semantic origin at the first tracked move (the grab point). The
        // raw pointer position itself stays transient.
        if operation.origin == iced::Point::ORIGIN {
            operation.origin = current;
            operation.current = current;
            return;
        }
        operation.current = current;
        let Some(section) = operation.section else {
            return;
        };
        let order = &self.active_layout.home.section_order;
        let Some(source) = order.iter().position(|candidate| *candidate == section) else {
            return;
        };
        // Pointer coordinates are transient interaction data. The semantic
        // result is only an insertion index in the typed sections array.
        let shift = crate::designer::snap_layout_slot(
            (current.y - operation.origin.y) / 120.0,
            1.0,
            self.settings_state.designer.fine_adjust,
        ) as isize;
        let max_index = order.len().saturating_sub(1) as isize;
        operation.proposed_index = Some((source as isize + shift).clamp(0, max_index) as usize);
    }

    #[cfg(feature = "dev-ui")]
    fn commit_home_drag(&mut self) {
        let Some(operation) = self.settings_state.designer.drag_operation.clone() else {
            return;
        };
        let (Some(section), Some(target)) = (operation.section, operation.proposed_index) else {
            self.settings_state.designer
                .reject("Drop rejected: no valid Home layout slot was selected");
            return;
        };
        let Some(source) = self
            .active_layout
            .home
            .section_order
            .iter()
            .position(|candidate| *candidate == section)
        else {
            self.settings_state.designer
                .reject("Drop rejected: the selected section is not in the Home layout");
            return;
        };
        if source == target {
            return;
        }
        let mut layout = self.active_layout.clone();
        let moved = layout.home.section_order.remove(source);
        layout.home.section_order.insert(target, moved);
        let mut overrides = self.layout_overrides.clone();
        overrides
            .home
            .get_or_insert_with(Default::default)
            .section_order = Some(layout.home.section_order.clone());
        self.set_layout_overrides(overrides);
        self.settings_state.designer.update(DesignerMessage::MarkDirty);
    }

    #[cfg(feature = "dev-ui")]
    fn reorder_home_from_tree(&mut self, index: usize, delta: isize) {
        let len = self.active_layout.home.section_order.len();
        if index >= len {
            return;
        }
        let target = (index as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        if target == index {
            return;
        }
        let before = self.active_layout.clone();
        let mut layout = before.clone();
        let section = layout.home.section_order.remove(index);
        layout.home.section_order.insert(target, section);
        let mut overrides = self.layout_overrides.clone();
        overrides
            .home
            .get_or_insert_with(Default::default)
            .section_order = Some(layout.home.section_order.clone());
        self.set_layout_overrides(overrides);
        self.settings_state.designer_history.record(&before, &self.active_layout);
        self.settings_state.designer.update(DesignerMessage::MarkDirty);
    }

    /// Apply a resize gesture to the semantic layout field exposed by the
    /// selected component. Pointer coordinates are transient; only the typed
    /// width value is written back to the live LayoutConfig.
    #[cfg(feature = "dev-ui")]
    fn update_resize(&mut self, current: iced::Point) {
        let Some(operation) = self.settings_state.designer.resize_operation.as_ref() else {
            return;
        };
        // Same anchoring as the drag gesture: StartResize passes
        // Point::ORIGIN and the overlay reports card-relative positions, so
        // the first tracked move establishes the grab anchor. Without this
        // the first delta would jump by the whole card width.
        if operation.origin == iced::Point::ORIGIN {
            if let Some(op) = self.settings_state.designer.resize_operation.as_mut() {
                op.origin = current;
                op.current = current;
            }
            return;
        }
        let delta = current.x - operation.origin.x;
        let component = operation.component;
        let mut layout = self.active_layout.clone();
        let value = match component {
            crate::designer::ComponentId::Sidebar => {
                let value = crate::designer::snap_layout_dimension(
                    layout.sidebar.width + delta,
                    8.0,
                    self.settings_state.designer.fine_adjust,
                );
                if value < layout.sidebar.width_min || value > layout.sidebar.width_max {
                    self.settings_state.designer.reject(format!(
                        "Resize rejected: sidebar width must stay between {:.0}px and {:.0}px",
                        layout.sidebar.width_min, layout.sidebar.width_max
                    ));
                    return;
                }
                layout.sidebar.width = value;
                value
            }
            crate::designer::ComponentId::ChatMessageList => {
                let value = crate::designer::snap_layout_dimension(
                    layout.chat.message_max_width + delta,
                    8.0,
                    self.settings_state.designer.fine_adjust,
                );
                if value < 1.0 || value > layout.chat.bubble_max_width {
                    self.settings_state.designer.reject(format!(
                        "Resize rejected: message width must stay between 1px and {:.0}px",
                        layout.chat.bubble_max_width
                    ));
                    return;
                }
                layout.chat.message_max_width = value;
                value
            }
            crate::designer::ComponentId::ChatComposer => {
                let value = crate::designer::snap_layout_dimension(
                    layout.chat.bubble_max_width + delta,
                    8.0,
                    self.settings_state.designer.fine_adjust,
                );
                if value < 1.0 || value > 1200.0 {
                    self.settings_state.designer
                        .reject("Resize rejected: composer width must stay between 1px and 1200px");
                    return;
                }
                layout.chat.bubble_max_width = value;
                value
            }
            _ => return,
        };
        if let Some(operation) = self.settings_state.designer.resize_operation.as_mut() {
            operation.current = current;
            operation.origin = current;
        }
        let mut overrides = self.layout_overrides.clone();
        match component {
            crate::designer::ComponentId::Sidebar => {
                overrides.sidebar.get_or_insert_with(Default::default).width = Some(value);
            }
            crate::designer::ComponentId::ChatMessageList => {
                overrides
                    .chat
                    .get_or_insert_with(Default::default)
                    .message_max_width = Some(value);
            }
            crate::designer::ComponentId::ChatComposer => {
                overrides
                    .chat
                    .get_or_insert_with(Default::default)
                    .bubble_max_width = Some(value);
            }
            _ => return,
        }
        self.set_layout_overrides(overrides);
        self.settings_state.designer.update(DesignerMessage::MarkDirty);
        debug!(component = %component, value, "designer resize updated");
    }

    /// BORU-LAYOUT-03: replace the live layout AND bump `layout_revision` so
    /// lazy/prewarm caches rebuild. Only layout state is touched — theme,
    /// networking, gossip, rooms, tunnels, media playback, chat history and
    /// composer input are all untouched. Called at startup and (in later
    /// BORU-LAYOUT tasks) when a validated `boru-layout.toml` reload lands.
    pub(crate) fn set_layout_config(&mut self, config: crate::layout::LayoutConfig) {
        self.active_layout = config;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        // BORU-LAYOUT-06: layout changes also invalidate pre-warmed trees
        // (the prewarm dependency snapshots do not embed layout_revision).
        // Coalesced exactly like the theme path: the pending flag is
        // consumed on the next idle tick, so one burst = one invalidation.
        self.prewarm_invalidate_pending = true;
    }

    /// BORU-LAYOUT-08: replace the editable layout override set AND the
    /// live merged layout in one step, and bump the layout revision so
    /// lazy/prewarm caches rebuild. This is the seam both the
    /// `boru-layout.toml` watcher and the inspector's layout edits use
    /// (mirror of [`IcedChat::set_ui_theme_config`]): defaults +
    /// overrides → merged [`LayoutConfig`](crate::layout::LayoutConfig),
    /// clamping warnings surfaced under `dev-ui`. Only layout state is
    /// touched — networking, gossip, rooms, tunnels, media playback, chat
    /// history, the selected conversation, scroll position and composer
    /// input are all untouched.
    pub(crate) fn set_layout_overrides(&mut self, overrides: crate::layout::LayoutOverrides) {
        let validation_errors = crate::layout_config::validate_layout_overrides(&overrides);
        if !validation_errors.is_empty() {
            #[cfg(feature = "dev-ui")]
            self.settings_state.designer.update(DesignerMessage::SetValidationErrors(
                validation_errors.clone(),
            ));
            tracing::warn!(issues = ?validation_errors, "layout override rejected by validation");
            return;
        }
        #[cfg(feature = "dev-ui")]
        {
            let serialized = match crate::layout_config::layout_config_to_toml(&overrides) {
                Ok(text) => text,
                Err(error) => {
                    self.settings_state.designer.reject(format!(
                        "Layout rejected: cannot serialize configuration: {error}"
                    ));
                    return;
                }
            };
            let round_tripped = match crate::layout_config::parse_layout_config(&serialized) {
                Ok(candidate) => candidate,
                Err(error) => {
                    self.settings_state.designer.reject(format!(
                        "Layout rejected: serialized configuration cannot be reloaded: {error}"
                    ));
                    return;
                }
            };
            if let Some(error) =
                crate::layout_config::validate_layout_overrides(&round_tripped).first()
            {
                self.settings_state.designer
                    .reject(format!("Layout rejected after serialization: {error}"));
                return;
            }
        }
        self.layout_overrides = overrides;
        #[cfg(feature = "dev-ui")]
        self.settings_state.designer.validation_errors.clear();
        let (merged, warnings) = crate::layout_merge::merge_layout_config(
            &crate::layout::LayoutConfig::default(),
            &self.layout_overrides,
        );
        for w in &warnings {
            tracing::warn!(override = %w, "layout override adjusted during merge");
        }
        #[cfg(feature = "dev-ui")]
        {
            self.settings_state.inspector_draft.layout_merge_warnings = warnings;
        }
        self.set_layout_config(merged);
        #[cfg(feature = "dev-ui")]
        self.settings_state.designer.update(DesignerMessage::ClearValidationErrors);
    }

    /// BORU-UI-07: recompute `active_theme` from the current dark-mode base
    /// and the stored `ui_theme_config` overrides. Called at startup (via
    /// [`Self::set_ui_theme_config`]) and on dark-mode toggle so the cached
    /// merged theme always reflects both the mode and the dev overrides.
    ///
    /// BORU-UI-18: every value the merge had to clamp or fall back (an
    /// invalid colour channel, absurd width, unknown font) is logged as a
    /// developer warning with the field name, and (dev-ui) recorded on the
    /// inspector draft so the panel can show the adjustment.
    fn recompute_active_theme(&mut self) {
        let base = crate::theme::BoruTheme::for_theme(&self.theme());
        let (merged, warnings) = crate::theme_merge::merge_ui_theme(&base, &self.ui_theme_config);
        for w in &warnings {
            tracing::warn!(field = %w, "boru-ui.toml value adjusted during merge");
        }
        #[cfg(feature = "dev-ui")]
        {
            self.settings_state.inspector_draft.merge_warnings = warnings;
        }
        self.active_theme = merged;
    }

    /// BORU-UI-07: replace the stored dev-theme config AND the live merged
    /// theme in one step, and bump the theme revision so lazy/prewarm caches
    /// rebuild. Only theme state is touched — networking, gossip, rooms,
    /// tunnels, media playback, chat history, selected conversation, scroll
    /// position and composer input are all untouched.
    pub(crate) fn set_ui_theme_config(&mut self, config: crate::theme_config::UiThemeConfig) {
        self.ui_theme_config = config;
        self.recompute_active_theme();
        self.theme_revision = self.theme_revision.wrapping_add(1);
        // BORU-UI-19: coalesce prewarm invalidation instead of clearing
        // the cache on every theme edit. A slider drag emits dozens of
        // InspectorMsg::SetFloat messages per second; clearing the cache
        // each time would churn it (and the rebuilds are the expensive
        // secondary work). The flag is consumed on the next idle tick, so
        // one burst = one invalidation + rebuild cycle. Correctness is
        // unaffected: `serve_prewarmed` hash-checks `theme_revision`, so a
        // stale pre-warmed tree is never served after a theme change.
        self.prewarm_invalidate_pending = true;
    }

    /// Handle a debounced `boru-ui.toml` reload (BORU-UI-06).
    ///
    /// The file watcher thread parsed the file away from the rendering
    /// path; this is the only place shared UI state may change. Stale
    /// results (an older save racing a newer one) are dropped via the
    /// generation tracker. BORU-UI-07 applies a valid reload to the live
    /// theme in-place (via [`Self::set_ui_theme_config`]) — only the
    /// theme/config state is replaced, never networking, gossip, rooms,
    /// tunnels, media playback, chat history, the selected conversation,
    /// scroll position or composer input. On an error the last known-good
    /// theme stays active (BORU-UI-04/18).
    ///
    /// BORU-UI-18: a failed reload is logged with the structured developer
    /// error (file path, failure kind, parser line/column where available)
    /// and, in dev-ui builds, surfaced on the inspector's reload-status
    /// line so the developer sees the parse error in the panel too.
    fn update_ui_theme_reloaded(
        &mut self,
        generation: u64,
        result: Result<crate::theme_config::UiThemeConfig, crate::theme_config::ThemeReloadError>,
    ) -> iced::Task<AppMessage> {
        if !self.ui_theme_reload_tracker.should_apply(generation) {
            tracing::debug!(
                generation,
                "boru-ui.toml reload dropped (stale; newer generation already accepted)"
            );
            return iced::Task::none();
        }
        self.ui_theme_reload_tracker.mark_applied(generation);
        match result {
            Ok(config) => {
                #[cfg(feature = "dev-ui")]
                if self.settings_state.designer.dirty {
                    let message =
                        "external boru-ui.toml change conflicts with unsaved designer edits"
                            .to_string();
                    self.settings_state.inspector_draft.reload_status =
                        crate::inspector::ThemeReloadStatus::Conflict(message.clone());
                    tracing::warn!(generation, "{message}");
                    return iced::Task::none();
                }
                tracing::info!(generation, "boru-ui.toml reloaded; applying live theme");
                // Replace ONLY theme state. `set_ui_theme_config` also bumps
                // `theme_revision` so lazy/prewarm caches rebuild on the
                // next frame.
                self.set_ui_theme_config(config);
            }
            Err(e) => {
                tracing::warn!(
                    generation,
                    path = %e.path.display(),
                    kind = ?e.kind,
                    line = ?e.line,
                    column = ?e.column,
                    error = %e.message,
                    "boru-ui.toml reload failed; keeping last known-good theme"
                );
                #[cfg(feature = "dev-ui")]
                {
                    self.settings_state.inspector_draft.reload_status =
                        crate::inspector::ThemeReloadStatus::Failed(e.message);
                }
            }
        }
        iced::Task::none()
    }

    /// Handle a debounced `boru-layout.toml` reload (BORU-LAYOUT-06).
    ///
    /// The layout watcher thread parsed the file away from the rendering
    /// loop. Generation-based staleness filtering mirrors the theme watcher:
    /// only the newest reload wins, older generations are dropped. A valid
    /// parse is merged onto [`crate::layout::LayoutConfig::default()`] and
    /// applied via `set_layout_config` (which bumps `layout_revision` so
    /// lazy/prewarm caches rebuild). A parse/IO error keeps the last
    /// known-good layout and is logged with the file path + line/column —
    /// only validated layouts are ever applied.
    fn update_layout_reloaded(
        &mut self,
        generation: u64,
        result: Result<crate::layout::LayoutOverrides, crate::layout_config::LayoutReloadError>,
    ) -> iced::Task<AppMessage> {
        if !self.layout_reload_tracker.should_apply(generation) {
            tracing::debug!(
                generation,
                "boru-layout.toml reload dropped (stale generation)"
            );
            return iced::Task::none();
        }
        self.layout_reload_tracker.mark_applied(generation);

        match result {
            Ok(overrides) => {
                #[cfg(feature = "dev-ui")]
                if self.settings_state.designer.dirty {
                    let message =
                        "external boru-layout.toml change conflicts with unsaved designer edits"
                            .to_string();
                    self.settings_state.inspector_draft.layout_reload_status =
                        crate::layout_inspector::LayoutReloadStatus::Conflict(message.clone());
                    tracing::warn!(generation, "{message}");
                    return iced::Task::none();
                }
                tracing::info!(
                    generation,
                    "boru-layout.toml reloaded; merging + applying live layout"
                );
                // BORU-LAYOUT-07: semantic validation (duplicate section
                // ids) is enforced at the load layer AND re-checked here,
                // so an invalid override set can never be applied no matter
                // how it reached the update loop. A validation failure is
                // treated exactly like a parse failure: keep the last
                // known-good layout and log every issue.
                let issues = crate::layout_config::validate_layout_overrides(&overrides);
                if !issues.is_empty() {
                    tracing::warn!(
                        generation,
                        issues = ?issues,
                        "boru-layout.toml failed validation; keeping last known-good layout"
                    );
                    return iced::Task::none();
                }
                // BORU-LAYOUT-08: route the reload through
                // `set_layout_overrides` so the editable override set in
                // app state matches the file (the inspector reflects disk
                // state). That seam re-merges defaults + overrides and
                // logs any clamping warnings. BORU-LAYOUT-06: replacing
                // the layout bumps `layout_revision` AND marks the prewarm
                // cache stale, so the next idle tick invalidates
                // pre-warmed trees and rebuilds with the new layout.
                self.set_layout_overrides(overrides);
            }
            Err(e) => {
                tracing::warn!(
                    generation,
                    path = %e.path.display(),
                    kind = ?e.kind,
                    line = ?e.line,
                    column = ?e.column,
                    error = %e.message,
                    "boru-layout.toml reload failed; keeping last known-good layout"
                );
            }
        }
        iced::Task::none()
    }

    /// Return the iced Theme enum for an arbitrary dark-mode flag.
    fn theme_from_dark(dark_mode: bool) -> iced::Theme {
        if dark_mode {
            iced::Theme::Dark
        } else {
            iced::Theme::Light
        }
    }

    /// Muted secondary text color for an arbitrary dark-mode flag.
    fn muted_color(dark_mode: bool) -> Color {
        if dark_mode {
            Color::from_rgb(0.6, 0.6, 0.6)
        } else {
            Color::from_rgb(0.4, 0.4, 0.4)
        }
    }

    /// Return a time-of-day greeting ("morning", "afternoon", "evening").
    fn time_of_day_greeting(&self) -> &'static str {
        use chrono::Timelike;
        let hour = chrono::Local::now().hour();
        if hour < 12 {
            "morning"
        } else if hour < 17 {
            "afternoon"
        } else {
            "evening"
        }
    }

    fn sidebar_avatar_handle(handle: Option<&iced::widget::image::Handle>) -> SidebarAvatarHandle {
        let handle = handle.cloned();
        let key = handle.as_ref().map(|h| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            h.id().hash(&mut hasher);
            hasher.finish()
        });
        SidebarAvatarHandle { handle, key }
    }

    fn mark_friends_sidebar_dirty(&mut self) {
        self.mark_friends_dirty();
        self.friends_sidebar_revision = self.friends_sidebar_revision.wrapping_add(1);
    }

    // ── Friend request UI helpers ────────────────────────────────────────

    /// Label text for an outgoing request state shown in the sidebar.
    pub fn outgoing_request_label(state: Option<&OutgoingRequestState>) -> &'static str {
        match state {
            None => "",
            Some(OutgoingRequestState::Pending) => "Pending",
            Some(OutgoingRequestState::Accepted) => "Accepted",
            Some(OutgoingRequestState::Declined) => "Declined",
            Some(OutgoingRequestState::Failed(_)) => "Failed",
        }
    }

    /// Color for an outgoing request state indicator.
    pub fn outgoing_request_color(state: Option<&OutgoingRequestState>) -> Color {
        match state {
            None => Color::from_rgb(0.5, 0.5, 0.5),
            Some(OutgoingRequestState::Pending) => Color::from_rgb(0.9, 0.7, 0.1),
            Some(OutgoingRequestState::Accepted) => Color::from_rgb(0.2, 0.7, 0.2),
            Some(OutgoingRequestState::Declined) => Color::from_rgb(0.8, 0.2, 0.2),
            Some(OutgoingRequestState::Failed(_)) => Color::from_rgb(0.8, 0.2, 0.2),
        }
    }

    /// Human-readable label for a join request state.
    pub fn join_request_state_label(state: &OutgoingRequestState) -> String {
        match state {
            OutgoingRequestState::Pending => crate::i18n::t("common.pending"),
            OutgoingRequestState::Accepted => crate::i18n::t("common.accepted"),
            OutgoingRequestState::Declined => crate::i18n::t("common.rejected"),
            OutgoingRequestState::Failed(_) => crate::i18n::t("common.failed"),
        }
    }

    /// Color indicator for a join request state.
    pub fn join_request_state_color(state: &OutgoingRequestState) -> Color {
        match state {
            OutgoingRequestState::Pending => Color::from_rgb(0.88, 0.67, 0.10),
            OutgoingRequestState::Accepted => Color::from_rgb(0.18, 0.68, 0.28),
            OutgoingRequestState::Declined => Color::from_rgb(0.53, 0.53, 0.53),
            OutgoingRequestState::Failed(_) => Color::from_rgb(0.80, 0.22, 0.22),
        }
    }

    /// Border color for a failed request state.
    pub fn join_request_border_color(state: &OutgoingRequestState) -> Color {
        match state {
            OutgoingRequestState::Failed(_) => Color::from_rgb(0.80, 0.22, 0.22),
            _ => Color::from_rgb(0.5, 0.5, 0.5),
        }
    }

    /// Section title string for the join requests list.
    pub fn join_request_section_title() -> String {
        crate::i18n::t("contacts.join_requests")
    }

    /// Total count label for the join requests section.
    pub fn join_request_total_label(count: usize) -> String {
        crate::i18n::t_args("contacts.total", &[("count", &count.to_string())])
    }

    /// Prefix label for the target user field in a join request row.
    pub fn join_request_target_user_prefix() -> String {
        crate::i18n::t("contacts.target_user")
    }

    /// Prefix label for the chat identifier in a join request row.
    pub fn join_request_chat_prefix() -> String {
        crate::i18n::t("contacts.chat")
    }

    /// Label for the \"open chat\" action button.
    pub fn join_request_open_chat_label() -> String {
        crate::i18n::t("contacts.open_chat")
    }

    /// Label for the retry action button on failed requests.
    pub fn join_request_retry_label() -> String {
        crate::i18n::t("common.retry")
    }

    /// Prefix label for the failure reason in a failed request row.
    pub fn join_request_failure_prefix() -> String {
        crate::i18n::t("contacts.failure")
    }

    /// A single animation frame character for the pending spinner indicator.
    pub fn join_request_spinner_frame() -> &'static str {
        "."
    }

    /// Parse the `target_user` field of a `JoinRequestItem` into a [`PublicKey`].
    pub fn join_request_peer(item: &JoinRequestItem) -> Option<PublicKey> {
        PublicKey::from_str(&item.target_user).ok()
    }

    pub fn view(&self) -> iced::Element<'_, AppMessage> {
        let _timer = PerfTracker::timer("view", format!("{:?}", self.screen));
        use iced::widget::{container, row, text};
        use iced::Length;

        // Always show sidebar on the left.
        let sidebar = self.view_sidebar();

        // Main panel depends on the active screen.
        let main_panel: iced::Element<'_, AppMessage> = match &self.screen {
            Screen::ChatList => {
                if let Some(handle) = &self.home_background_handle {
                    // Home-screen background image: draw it as the bottom
                    // layer behind the empty-state dashboard so all cards,
                    // text and controls stay on top of the image.
                    let bg = iced::widget::image(handle.clone())
                        .content_fit(iced::ContentFit::Cover)
                        .width(Length::Fill)
                        .height(Length::Fill);
                    iced::widget::Stack::new()
                        .push(bg)
                        .push(self.view_main_empty_state())
                        .into()
                } else {
                    self.view_main_empty_state()
                }
            }
            // PERF-4R-B: pre-warmable screens are served from the app-state
            // cache when the cached tree's dependency hash still matches the
            // current state; otherwise the live view runs (and lazily caches).
            Screen::FileSharing => {
                self.serve_prewarmed(Screen::FileSharing, || self.view_file_sharing())
            }
            Screen::DownloadManager => self.view_download_manager(),
            Screen::Chat { .. } => self.view_chat_panel(),
            Screen::OutgoingCall => self.view_outgoing_call(),
            Screen::ActiveCall => self.view_active_call(),
            Screen::FriendRequests => {
                self.serve_prewarmed(Screen::FriendRequests, || self.view_friend_requests())
            }
            Screen::Settings => {
                self.serve_prewarmed(Screen::Settings, || self.view_settings_screen())
            }
            Screen::PeerProfile(peer) => self.view_peer_profile(*peer),
            Screen::PeerCatalogue(peer) => self.view_peer_catalogue(*peer),
            Screen::FriendProfile(peer) => self.view_friend_profile(*peer),
            Screen::Discover => self.serve_prewarmed(Screen::Discover, || self.view_discover()),
            Screen::Groups => self.serve_prewarmed(Screen::Groups, || self.view_groups_screen()),
            #[cfg(feature = "terminal")]
            Screen::Terminal => match self.terminal.as_ref() {
                Some(term) => term.view().map(AppMessage::TerminalEvent),
                // Terminal could not be spawned (e.g. no shell on this
                // platform) — show a plain notice instead of a blank pane.
                None => crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Body,
                    "Terminal unavailable on this platform.",
                )
                .into(),
            },
            #[cfg(feature = "dev-ui")]
            Screen::Gallery => crate::component_gallery::view_gallery_with_designer(
                &self.settings_state.gallery_state,
                self.window_width,
                &self.boru_theme(),
                &self.active_layout,
                &self.settings_state.designer,
            ),
        };
        // BORU-UI-11: when inspection mode is enabled, tag the sidebar and
        // main panel with their component IDs so hovering/clicking them shows
        // the component name and can jump the inspector to its section. When
        // disabled, `inspect_region` returns the content untouched — no mouse
        // areas are added and normal clicks pass through exactly as before.
        #[cfg(feature = "dev-ui")]
        let sidebar = self.inspect_region(crate::inspector::ComponentId::Sidebar, sidebar);
        #[cfg(feature = "dev-ui")]
        let sidebar = crate::designer::overlay(
            crate::designer::ComponentId::Sidebar,
            sidebar,
            self.settings_state.designer.enabled,
            self.settings_state.designer.hovered_component,
            self.settings_state.designer.selected_component,
            self.settings_state.designer.resize_operation.as_ref().and_then(|op| {
                (op.component == crate::designer::ComponentId::Sidebar)
                    .then_some(self.boru_layout().sidebar.width)
            }),
        );
        #[cfg(feature = "dev-ui")]
        let main_panel = self.inspect_region(self.component_id_for_screen(), main_panel);

        // Resolve the shell width from the live structural layout so TOML
        // overrides remain effective after a watcher reload.
        let layout = self.boru_layout();
        let sidebar_w = layout
            .sidebar
            .width_for_window(self.window_width, &layout.responsive);
        // Capture the merged theme (BoruTheme::default/for_theme + boru-ui.toml
        // overrides) so the app-shell containers (sidebar, divider, canvas,
        // details panel) reflect live theme edits instead of the hardcoded
        // `design_tokens` dark/light palettes.
        let btheme = self.boru_theme();

        let content = row![
            container(sidebar)
                .width(Length::Fixed(sidebar_w))
                .height(Length::Fill)
                .style(move |_t| {
                    iced::widget::container::Style {
                        background: Some(iced::Background::Color(btheme.colors.sidebar)),
                        ..Default::default()
                    }
                }),
            // 1 px vertical divider between sidebar and main content.
            container(
                iced::widget::Space::new()
                    .width(Length::Fixed(1.0))
                    .height(Length::Fill)
            )
            .width(Length::Fixed(1.0))
            .height(Length::Fill)
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(btheme.colors.border_muted)),
                ..Default::default()
            }),
            container(main_panel)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(btheme.colors.canvas)),
                    ..Default::default()
                }),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        // Resolve details-panel geometry from the live structural layout. The
        // panel is optional: never let it consume the minimum conversation
        // width on compact windows.
        let chat_layout = self.boru_layout().chat.clone();
        let details_width = chat_layout.details_panel_width;
        let details_usable = self.window_width
            >= sidebar_w + details_width + self.boru_layout().responsive.viewport_min_width;
        let base = if self.details_panel_open
            && matches!(self.screen, Screen::Chat { .. })
            && details_usable
        {
            container(
                row![
                    content,
                    container(self.view_details_panel())
                        .width(Length::Fixed(details_width))
                        .height(Length::Fill)
                        .style(move |_t| {
                            iced::widget::container::Style {
                                background: Some(iced::Background::Color(
                                    btheme.colors.surface,
                                )),
                                ..Default::default()
                            }
                        }),
                ]
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
        } else {
            container(content)
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
        };

        // BORU-UI-09: dev UI Inspector panel — a fixed-width right column
        // overlaid on the whole app (every screen) while visible. The app
        // stays fully interactive beside it; closing (Ctrl+Shift+D or the ×
        // button) returns to the exact layout. Compiled only with dev-ui.
        #[cfg(feature = "dev-ui")]
        let base = if self.settings_state.inspector_visible || self.settings_state.designer.enabled {
            let inspector = self.view_inspector_panel();
            container(
                row![
                    base,
                    container(inspector)
                        .width(iced::Length::Fixed(crate::inspector::INSPECTOR_PANEL_WIDTH))
                        .height(iced::Length::Fill),
                ]
                .width(iced::Length::Fill)
                .height(iced::Length::Fill),
            )
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
        } else {
            base
        };

        // BORU-UI-11: while inspection mode is on and the cursor is over a
        // supported component, float a small overlay pill at the top of the
        // app showing the hovered component's ID/name. The pill is a plain
        // non-interactive container, so it never intercepts clicks; it just
        // makes the hovered component visible even when the inspector panel
        // is closed. Applied to the final element (after dialogs) so it never
        // changes the `base` container the dialog functions expect.
        #[cfg(feature = "video-playback")]
        if self.inline_video_expanded {
            return self.view_expanded_inline_video(base);
        }
        #[cfg(feature = "screen-sharing")]
        if self.calls_state.screen_share_fullscreen && self.calls_state.screen_share_viewing {
            // Fullscreen is a presentation mode, not a second copy of the
            // chat layout.  Render only the viewer here; when the toggle is
            // reversed this same `view()` path rebuilds `base`, restoring the
            // card, history, composer, and footer from their normal state.
            return self.view_screen_share_fullscreen();
        }

        let result = if self.calls_state.incoming_call.is_some() {
            self.view_incoming_call_overlay(base)
        } else if self.connection_details_dialog.is_some() {
            self.view_connection_details_dialog(base)
        } else if self.rooms_state.show_room_settings_dialog {
            self.view_room_settings_dialog(base)
        } else if self.rooms_state.show_create_room_dialog {
            self.view_create_room_dialog(base)
        } else if self.show_create_group_dialog {
            self.view_create_group_dialog(base)
        } else if self.tunnels_state.show_create_tunnel_dialog {
            self.view_create_tunnel_dialog(base)
        } else if self.show_invite_member_dialog {
            self.view_invite_member_dialog(base)
        } else if self.show_receive_ticket_dialog {
            self.view_receive_ticket_dialog(base)
        } else if self.files_state.show_short_code_dialog {
            self.view_short_code_dialog(base)
        } else if self.files_state.show_redeem_code_dialog {
            self.view_redeem_code_dialog(base)
        } else if let Some(entry_index) = self.lightbox_image {
            self.view_image_lightbox(base, entry_index)
        } else {
            base.into()
        };

        #[cfg(feature = "dev-ui")]
        let result = if self.settings_state.inspect_ui_enabled && self.settings_state.inspect_hover.is_some() {
            let hover = self.settings_state.inspect_hover.unwrap();
            let pill = container(
                text(format!("🔍 {}", hover.label()))
                    .size(12.0)
                    .color(Color::WHITE),
            )
            .padding(iced::Padding::from(4.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.1, 0.45, 0.28))),
                border: iced::Border {
                    color: Color::from_rgb(0.8, 0.95, 0.85),
                    width: 1.0,
                    radius: iced::border::Radius::from(12.0),
                },
                ..Default::default()
            });
            let top = container(pill)
                .width(iced::Length::Fill)
                .align_x(iced::Alignment::Center)
                .padding(iced::Padding {
                    top: 8.0,
                    ..Default::default()
                });
            iced::widget::Stack::new()
                .push(result)
                .push(top)
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .into()
        } else {
            result
        };

        #[cfg(feature = "dev-ui")]
        let result = if self.settings_state.designer.enabled {
            let has_errors = !self.settings_state.designer.validation_errors.is_empty();
            let banner_text = if has_errors {
                format!(
                    "DESIGNER ERROR: {}",
                    self.settings_state.designer
                        .validation_errors
                        .first()
                        .cloned()
                        .unwrap_or_default()
                )
            } else {
                "VISUAL DESIGNER ACTIVE".to_string()
            };
            let banner = container(text(banner_text).size(12.0).color(Color::WHITE))
                .padding(iced::Padding::from(5.0))
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(if has_errors {
                        Color::from_rgb(0.58, 0.12, 0.12)
                    } else {
                        Color::from_rgb(0.12, 0.42, 0.28)
                    })),
                    border: iced::Border {
                        color: if has_errors {
                            Color::from_rgb(1.0, 0.55, 0.55)
                        } else {
                            Color::from_rgb(0.55, 0.95, 0.7)
                        },
                        width: 1.0,
                        radius: iced::border::Radius::from(4.0),
                    },
                    ..Default::default()
                });
            let top = container(banner)
                .width(iced::Length::Fill)
                .align_x(iced::Alignment::Center)
                .padding(iced::Padding {
                    top: 8.0,
                    ..Default::default()
                });
            iced::widget::Stack::new()
                .push(result)
                .push(top)
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .into()
        } else {
            result
        };

        #[cfg(feature = "dev-ui")]
        let result = if self.settings_state.designer.enabled {
            iced::widget::mouse_area(result)
                .on_press(AppMessage::Designer(DesignerMessage::Select(None)))
                .into()
        } else {
            result
        };

        result
    }

    // ── Home-rail card selectors (fine-grained state slices) ─────────────
    // Each selector returns ONLY the data its card renders: the Online Peers
    // selector never reads `recent_activity` or the tunnel service, the
    // Recent Activity selector never reads friends/presence, and the Tunnels
    // selector never reads friends/activity. The matching builders run inside
    // `iced::widget::lazy`, which compares the fresh selector value with the
    // previous frame and reuses the built subtree when nothing in the slice
    // changed — so a data change in one card rebuilds exactly that card.

    // ── Chat panel (main panel when a conversation is selected) ──────────
}

// ── Global keyboard shortcuts subscription ─────────────────────────────

/// Pure mapping from a keyboard event to the shortcut it triggers.
///
/// Kept as a standalone function so the focus/keyboard behaviour can be
/// unit-tested without an event loop (UI-19: focus order + keyboard
/// activation). Returns `None` for keys the app deliberately ignores.
pub fn shortcut_from_key(
    key: &iced::keyboard::key::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Option<Shortcut> {
    use iced::keyboard::key;
    let ctrl = modifiers.control();
    match key {
        #[cfg(feature = "dev-ui")]
        key::Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => {
            if modifiers.shift() {
                Some(Shortcut::DesignerRedo)
            } else {
                Some(Shortcut::DesignerUndo)
            }
        }
        #[cfg(feature = "dev-ui")]
        key::Key::Character(c) if ctrl && c.eq_ignore_ascii_case("y") => {
            Some(Shortcut::DesignerRedo)
        }
        #[cfg(feature = "dev-ui")]
        key::Key::Character(c) if ctrl && c.eq_ignore_ascii_case("s") => {
            Some(Shortcut::DesignerSave)
        }
        #[cfg(feature = "dev-ui")]
        key::Key::Named(key::Named::ArrowUp) if !ctrl => Some(Shortcut::DesignerNudgeUp),
        #[cfg(feature = "dev-ui")]
        key::Key::Named(key::Named::ArrowDown) if !ctrl => Some(Shortcut::DesignerNudgeDown),
        #[cfg(feature = "dev-ui")]
        key::Key::Named(key::Named::ArrowLeft) if !ctrl => Some(Shortcut::DesignerNudgeLeft),
        #[cfg(feature = "dev-ui")]
        key::Key::Named(key::Named::ArrowRight) if !ctrl => Some(Shortcut::DesignerNudgeRight),
        #[cfg(feature = "dev-ui")]
        key::Key::Named(key::Named::Delete) => Some(Shortcut::DesignerDelete),
        key::Key::Named(key::Named::Escape) => Some(Shortcut::Escape),
        key::Key::Named(key::Named::Backspace) if ctrl => Some(Shortcut::BackToChatList),
        key::Key::Character(c) if ctrl && c.eq_ignore_ascii_case("n") => Some(Shortcut::NewChat),
        key::Key::Character(c) if c == "/" => Some(Shortcut::QuickCommand),
        key::Key::Named(key::Named::Tab) => {
            // Tab / Shift+Tab move focus between text inputs (the only
            // focusable widgets in iced 0.14). This defines the logical
            // focus order through the composer, search fields and dialogs.
            if modifiers.shift() {
                Some(Shortcut::FocusPrevious)
            } else {
                Some(Shortcut::FocusNext)
            }
        }
        // Ctrl+Left / Ctrl+Right: cycle through dashboard tabs when on the
        // File Sharing screen (also works as Alt+Left / Alt+Right on most DEs).
        key::Key::Named(key::Named::ArrowLeft) if ctrl => Some(Shortcut::DashboardTabPrevious),
        key::Key::Named(key::Named::ArrowRight) if ctrl => Some(Shortcut::DashboardTabNext),
        _ => None,
    }
}

/// Subscribe to global keyboard shortcuts (Escape, Ctrl+N, Ctrl+Backspace, /, Tab).
///
/// Uses `filter_map` so non-matching key events are silently dropped without
/// producing `AppMessage::Noop`, avoiding unnecessary event-loop wakeups
/// while the user is typing in the chat input field.
pub fn keyboard_shortcuts_subscription() -> iced::Subscription<AppMessage> {
    use iced::keyboard::{self, key};
    keyboard::listen().filter_map(|event: keyboard::Event| -> Option<AppMessage> {
        match event {
            keyboard::Event::KeyPressed { key, modifiers, .. } => {
                let ctrl = modifiers.control();
                match key {
                    // Developer gallery: Ctrl+Shift+G (documented in
                    // component_gallery.rs). Dev-ui-only harness — release
                    // builds have no gallery screen at all.
                    #[cfg(feature = "dev-ui")]
                    key::Key::Character(c)
                        if ctrl && modifiers.shift() && c.eq_ignore_ascii_case("g") =>
                    {
                        return Some(AppMessage::ToggleGallery);
                    }
                    // BORU-UI-09: dev UI Inspector panel (Ctrl+Shift+D).
                    // Compiled only with the dev-ui feature — release builds
                    // have no inspector code at all.
                    #[cfg(feature = "dev-ui")]
                    key::Key::Character(c)
                        if ctrl && modifiers.shift() && c.eq_ignore_ascii_case("d") =>
                    {
                        return Some(AppMessage::Inspector(
                            crate::inspector::InspectorMsg::ToggleVisible,
                        ));
                    }
                    #[cfg(feature = "dev-ui")]
                    key::Key::Named(key::Named::Shift) | key::Key::Named(key::Named::Alt) => {
                        return Some(AppMessage::Designer(DesignerMessage::SetFineAdjust(true)));
                    }
                    _ => {}
                }
                shortcut_from_key(&key, modifiers).map(AppMessage::Shortcut)
            }
            #[cfg(feature = "dev-ui")]
            keyboard::Event::KeyReleased { key, .. }
                if matches!(key, key::Key::Named(key::Named::Shift | key::Named::Alt)) =>
            {
                Some(AppMessage::Designer(DesignerMessage::SetFineAdjust(false)))
            }
            _ => None,
        }
    })
}


struct RxHandle(Arc<Mutex<Receiver<ConversationNetEvent>>>);

impl std::hash::Hash for RxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

struct FriendRxHandle(Arc<Mutex<Receiver<FriendEvent>>>);

impl std::hash::Hash for FriendRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

struct WhisperRxHandle(Arc<Mutex<Receiver<WhisperEvent>>>);

impl std::hash::Hash for WhisperRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

struct InboxRxHandle(Arc<Mutex<Receiver<InboxEvent>>>);

impl std::hash::Hash for InboxRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

/// Wrapper for the continuous tracker's discovered-peers channel.
/// Uses a bounded mpsc receiver wrapped in Arc<Mutex<>>.
struct DiscoveredPeersRxHandle(Arc<Mutex<tokio::sync::mpsc::Receiver<DiscoveredPeersUpdate>>>);
/// Subscription-stream handle for BORU-CP-07 reconnection signals (peers
/// whose endpoint connectivity was re-established by the backend).
struct ReconnectReadyRxHandle(Arc<Mutex<tokio::sync::mpsc::Receiver<PublicKey>>>);

impl std::hash::Hash for ReconnectReadyRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

impl std::hash::Hash for DiscoveredPeersRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

/// Wrapper for the directory room channel (bounded mpsc).
#[expect(dead_code)]
struct DirectoryRoomRxHandle(Arc<Mutex<tokio::sync::mpsc::Receiver<DirectoryRoomEvent>>>);

impl std::hash::Hash for DirectoryRoomRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

/// Wrapper for the GUI test-actions channel.
/// Uses a bounded mpsc receiver wrapped in Arc<Mutex<>>.
struct GuiActionHandle(Arc<Mutex<tokio::sync::mpsc::Receiver<GuiActionRequest>>>);

impl std::hash::Hash for GuiActionHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

/// Wrapper for the FS-05 transfer projection broadcast channel.
/// The broadcast receiver is not `Hash`, so the handle hashes by Arc
/// pointer (stable identity → the subscription is not recreated each frame).
struct TransferProjectionHandle(Arc<Mutex<TransferUpdateReceiver>>);

impl std::hash::Hash for TransferProjectionHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

/// Wrapper for the dev theme reload channel (BORU-UI-06). Used as the
/// iced subscription identity so the stream restarts on re-subscribe.
struct UiThemeRxHandle(
    Arc<Mutex<tokio::sync::mpsc::Receiver<crate::theme_watcher::UiThemeReloadMsg>>>,
);
impl std::hash::Hash for UiThemeRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

/// Wrapper for the dev layout reload channel (BORU-LAYOUT-06). Used as the
/// iced subscription identity so the stream restarts on re-subscribe.
struct LayoutRxHandle(
    Arc<Mutex<tokio::sync::mpsc::Receiver<crate::layout_watcher::LayoutReloadMsg>>>,
);
impl std::hash::Hash for LayoutRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::sync::Arc::as_ptr(&self.0).hash(state);
    }
}

/// Dev layout reload subscription (BORU-LAYOUT-06): a standalone stream that
/// forwards `LayoutReloadMsg`s from the watcher thread into the update loop
/// as `AppMessage::LayoutReloaded`. Mirrors the theme reload delivery; kept
/// as its own subscription so the app-lifetime combined stream select does
/// not need to grow.
fn layout_subscription(
    layout_rx: Option<
        Arc<Mutex<tokio::sync::mpsc::Receiver<crate::layout_watcher::LayoutReloadMsg>>>,
    >,
) -> iced::Subscription<AppMessage> {
    // When the layout watcher is not running (dev-ui gate off, headless
    // launch, tests), the fallback receiver is intentionally closed. Its
    // one-shot stream ends immediately and stays ended because the recipe
    // hash does not change — no reload message can ever reach the loop.
    let layout_rx = layout_rx.unwrap_or_else(|| {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        Arc::new(tokio::sync::Mutex::new(rx))
    });
    iced::Subscription::run_with(LayoutRxHandle(layout_rx), |handle| {
        let layout_rx = Arc::clone(&handle.0);
        Box::pin(n0_future::stream::unfold(
            layout_rx,
            |layout_rx| async move {
                let msg = layout_rx.lock().await.recv().await?;
                Some((
                    AppMessage::LayoutReloaded {
                        generation: msg.generation,
                        result: msg.result,
                    },
                    layout_rx,
                ))
            },
        ))
    })
}

/// Convert one channel item into an Iced message without performing any
/// application work. Validation and handling remain in `update`.
fn map_gui_action(action: GuiActionRequest) -> AppMessage {
    AppMessage::GuiTestActionReceived(action)
}

#[cfg(feature = "screen-sharing")]
/// Drain inbound media for one viewer session into the bounded decode
/// pipeline, publishing the newest decoded frame to `watch_tx`. Runs on the
/// tokio runtime (never the UI thread); exits when the session ends, the
/// channel closes, or `stop` is set. When the pipeline detects missing or
/// corrupt frames it emits a `KeyframeRequest` on the control channel so the
/// host forces the next unit to be independently decodable (PDF Task 8.1).
async fn decode_worker(
    media_rx: Arc<Mutex<Receiver<InboundMedia>>>,
    session_id: ScreenShareSessionId,
    mut pipeline: ViewerPipeline<OpenH264Decoder>,
    watch_tx: tokio::sync::watch::Sender<Option<CapturedFrame>>,
    stats_watch_tx: tokio::sync::watch::Sender<Option<ScreenShareStatsSnapshot>>,
    protocol: Option<ScreenShareProtocol>,
    stop: Arc<AtomicBool>,
) {
    let mut last_stats = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // PDF Phase 12: publish viewer pipeline stats (~1 Hz) for the
        // developer diagnostics overlay and log the eight developer metrics.
        // The pipeline stats snapshot is local-only and carries no media data.
        if last_stats.elapsed() >= Duration::from_secs(1) {
            last_stats = std::time::Instant::now();
            let snapshot = pipeline.stats();
            let _ = stats_watch_tx.send(Some(snapshot));
            tracing::info!(
                capture_fps = snapshot.sender_fps,
                encode_fps = snapshot.encoded_fps,
                encode_avg_us = snapshot.encode_time_avg_us,
                bytes_per_sec = snapshot.bitrate_bps / 8,
                dropped_frames = snapshot.dropped_frames,
                queue_depth = snapshot.send_queue_depth,
                decode_fps = snapshot.receiver_fps,
                latency_us = snapshot.frame_age_us,
                "screen-share: viewer performance metrics"
            );
        }
        // Hold the receiver guard across the select so the recv() future does
        // not borrow a temporary guard (single media consumer in M7).
        let mut guard = media_rx.lock().await;
        let unit = tokio::select! {
            unit = guard.recv() => match unit {
                Some(unit) => unit,
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(50)) => continue,
        };
        drop(guard);
        if unit.session_id != session_id {
            continue;
        }
        if pipeline.enqueue(unit.header, unit.payload).is_err() {
            // Session ended or authorization revoked — stop decoding.
            break;
        }
        pipeline.process();
        // Corrupt/missing frames recovered via a fresh keyframe: emit the
        // request on the reliable control channel so the host resynchronises
        // without waiting for the next periodic keyframe.
        if pipeline.take_keyframe_request() {
            if let Some(protocol) = &protocol {
                if let Err(error) = protocol
                    .send_screen_share(
                        session_id,
                        ScreenShareMessage::KeyframeRequest {
                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                            session_id,
                        },
                    )
                    .await
                {
                    tracing::warn!(error = %error, "screen-share: viewer keyframe request send failed");
                }
            }
        }
        if let Some(frame) = pipeline.take_frame() {
            let _ = watch_tx.send(Some(frame));
        }
    }
}

#[cfg(feature = "screen-sharing")]
/// Drain inbound audio for one viewer session (BORU-SS-37): decode each Opus
/// packet and push PCM into the cpal playback sink. Runs on the tokio runtime
/// (never the UI thread); exits when the session ends, the channel closes, or
/// `stop` is set. Missing/corrupt audio packets are dropped (drop-tolerant
/// path) and never affect the video decode worker. When no output device is
/// available, playback fails with a typed unavailable error and audio is
/// silently dropped (the session continues view-only).
async fn audio_worker(
    audio_rx: Arc<Mutex<Receiver<InboundAudio>>>,
    session_id: ScreenShareSessionId,
    stop: Arc<AtomicBool>,
) {
    // Open the output sink once. On headless/no-device environments this
    // returns a typed AudioUnavailable error; log it and drop audio for the
    // rest of the session instead of failing the viewer.
    let mut output = match AudioOutput::open() {
        Ok(output) => {
            tracing::info!("screen-share: viewer audio playback started");
            Some(output)
        }
        Err(error) => {
            tracing::warn!(kind = ?error.kind(), error = %error, "screen-share: viewer audio playback unavailable; dropping audio");
            None
        }
    };
    let mut decoder: Option<OpusAudioDecoder> = None;
    let mut frame = vec![0.0f32; AUDIO_SAMPLES_PER_FRAME];
    let mut dropped_packets: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut guard = audio_rx.lock().await;
        let unit = tokio::select! {
            unit = guard.recv() => match unit {
                Some(unit) => unit,
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(50)) => continue,
        };
        drop(guard);
        if unit.session_id != session_id {
            continue;
        }
        if output.is_none() {
            continue;
        }
        // The decoder is created with the first packet's format; a stream
        // format change would need a fresh decoder (v1: fixed 48k stereo).
        if decoder.is_none() {
            match OpusAudioDecoder::new(unit.header.sample_rate, unit.header.channels) {
                Ok(decoder_instance) => decoder = Some(decoder_instance),
                Err(error) => {
                    tracing::warn!(error = %error, "screen-share: viewer audio decoder unavailable");
                    break;
                }
            }
        }
        let Some(decoder_instance) = decoder.as_mut() else {
            continue;
        };
        match decoder_instance.decode_frame(&unit.payload, &mut frame) {
            Ok(decoded) if decoded > 0 => {
                if let Some(output) = output.as_mut() {
                    let _ = output.push_pcm(&frame[..decoded]);
                }
            }
            Ok(_) => {}
            Err(error) => {
                dropped_packets += 1;
                if dropped_packets == 1 || dropped_packets % 500 == 0 {
                    tracing::warn!(dropped_packets, error = %error, "screen-share: viewer dropped audio packet (decode failed)");
                }
            }
        }
    }
}


fn subscription_stream(
    rx: &RxHandle,
    friend_rx: &FriendRxHandle,
    whisper_rx: &WhisperRxHandle,
    inbox_rx: &InboxRxHandle,
    discovered_rx: &DiscoveredPeersRxHandle,
    reconnect_rx: &ReconnectReadyRxHandle,
    gui_action_rx: &GuiActionHandle,
    transfer_rx: &TransferProjectionHandle,
    ui_theme_rx: &UiThemeRxHandle,
) -> Pin<Box<dyn Stream<Item = AppMessage> + Send>> {
    let rx = Arc::clone(&rx.0);
    let friend_rx = Arc::clone(&friend_rx.0);
    let whisper_rx = Arc::clone(&whisper_rx.0);
    let inbox_rx = Arc::clone(&inbox_rx.0);
    let discovered_rx = Arc::clone(&discovered_rx.0);
    let reconnect_rx = Arc::clone(&reconnect_rx.0);
    let gui_action_rx = Arc::clone(&gui_action_rx.0);
    let transfer_rx = Arc::clone(&transfer_rx.0);
    let ui_theme_rx = Arc::clone(&ui_theme_rx.0);
    Box::pin(n0_future::stream::unfold(
        (
            rx,
            friend_rx,
            whisper_rx,
            inbox_rx,
            discovered_rx,
            reconnect_rx,
            gui_action_rx,
            transfer_rx,
            ui_theme_rx,
        ),
        |(
            rx,
            friend_rx,
            whisper_rx,
            inbox_rx,
            discovered_rx,
            reconnect_rx,
            gui_action_rx,
            transfer_rx,
            ui_theme_rx,
        )| async move {
            // A closed GUI-action sender is a normal shutdown condition.  Do
            // not let it terminate this combined subscription: the network
            // and friend streams still belong to the application and must
            // remain live until their own receivers close.  The flag is
            // intentionally local to this unfold iteration; on the next
            // item we re-check the receiver once and disable the branch again.
            // A closed auxiliary channel must not terminate the combined
            // application subscription.  Some optional subsystems (notably
            // inbox/discovery in headless or feature-disabled launches) can
            // close their sender before the GUI action channel is used.  If a
            // closed receiver remains in `select!`, it is immediately ready on
            // every poll and either spins or, previously, ended this stream.
            let mut rx_open = true;
            let mut friend_open = true;
            let mut whisper_open = true;
            let mut inbox_open = true;
            let mut discovered_open = true;
            let mut reconnect_open = true;
            let mut gui_action_open = true;
            let mut transfer_open = true;
            let mut ui_theme_open = true;
            // Identify this stream instance so duplicate/competing consumers
            // (two live subscription streams fighting over the same receiver
            // mutexes would wedge both) are visible in logs.
            static STREAM_INSTANCE: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let instance = STREAM_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::debug!(
                instance,
                net_rx = format!("{:p}", rx.as_ref() as *const _),
                friend_rx = format!("{:p}", friend_rx.as_ref() as *const _),
                whisper_rx = format!("{:p}", whisper_rx.as_ref() as *const _),
                inbox_rx = format!("{:p}", inbox_rx.as_ref() as *const _),
                discovered_rx = format!("{:p}", discovered_rx.as_ref() as *const _),
                reconnect_rx = format!("{:p}", reconnect_rx.as_ref() as *const _),
                gui_action_rx = format!("{:p}", gui_action_rx.as_ref() as *const _),
                transfer_rx = format!("{:p}", transfer_rx.as_ref() as *const _),
                "SUBSCRIPTION_STREAM_START: combined subscription stream running",
            );
            loop {
                // Each branch acquires its own receiver mutex inside its own
                // select arm.  Locking all receivers serially before select!
                // turns the independent channels into one lock chain: if any
                // mutex is held by another task (e.g. a diagnostic tool or a
                // competing stream), the stream blocks before ever reaching
                // select! and ALL channels go silent — the observed wedge.
                tokio::select! {
                    event = async { rx.lock().await.recv().await }, if rx_open => {
                        match event {
                            Some(e) => {
                                tracing::debug!(
                                    "APP_NET_RX: combined stream received net event",
                                );
                                return Some((AppMessage::NetEvent(e), (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx)));
                            }
                            None => {
                                tracing::warn!(
                                    "subscription_stream: net_rx channel closed — net events permanently disabled"
                                );
                                rx_open = false;
                                continue;
                            }
                        }
                    }
                    event = async { friend_rx.lock().await.recv().await }, if friend_open => {
                        match event {
                            Some(e) => return Some((AppMessage::FriendEvent(e), (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx))),
                            None => { friend_open = false; continue; }
                        }
                    }
                    event = async { whisper_rx.lock().await.recv().await }, if whisper_open => {
                        match event {
                            Some(e) => return Some((AppMessage::WhisperEvent(e), (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx))),
                            None => { whisper_open = false; continue; }
                        }
                    }
                    event = async { inbox_rx.lock().await.recv().await }, if inbox_open => {
                        match event {
                            Some(e) => return Some((AppMessage::InboxEvent(e), (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx))),
                            None => { inbox_open = false; continue; }
                        }
                    }
                    peers = async { discovered_rx.lock().await.recv().await }, if discovered_open => {
                        match peers {
                            Some(peers) => return Some((AppMessage::NewDiscoveredPeers(peers), (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx))),
                            None => { discovered_open = false; continue; }
                        }
                    }
                    reconnect_peer = async { reconnect_rx.lock().await.recv().await }, if reconnect_open => {
                        match reconnect_peer {
                            Some(peer) => return Some((AppMessage::ReconnectPeerReady(peer), (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx))),
                            None => { reconnect_open = false; continue; }
                        }
                    }
                    action = async { gui_action_rx.lock().await.recv().await }, if gui_action_open => {
                        match action {
                            Some(a) => return Some((
                                map_gui_action(a),
                                (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx),
                            )),
                            None => {
                                // The MCP/GUI sender was dropped.  Disable only
                                // this branch and keep waiting on application
                                // event channels.
                                gui_action_open = false;
                                continue;
                            }
                        }
                    }
                    transfer_update = async { transfer_rx.lock().await.recv().await }, if transfer_open => {
                        match transfer_update {
                            Ok(update) => return Some((AppMessage::TransferProjectionUpdate(update), (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx))),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                // The broadcast receiver fell behind (progress is
                                // coalesced to 250 ms but a long UI stall can still
                                // drop messages). The snapshot is authoritative, so
                                // rebuild the panel maps instead of replaying.
                                return Some((AppMessage::TransferSnapshotResync, (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx)));
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                // The projection store was dropped (shutdown). Keep
                                // the other channels alive.
                                transfer_open = false;
                                continue;
                            }
                        }
                    }
                    reload = async { ui_theme_rx.lock().await.recv().await }, if ui_theme_open => {
                        match reload {
                            // BORU-UI-06: a debounced boru-ui.toml reload from
                            // the watcher thread. update() drops stale
                            // generations; no UI state is touched here.
                            Some(msg) => return Some((
                                AppMessage::UiThemeReloaded {
                                    generation: msg.generation,
                                    result: msg.result,
                                },
                                (rx, friend_rx, whisper_rx, inbox_rx, discovered_rx, reconnect_rx, gui_action_rx, transfer_rx, ui_theme_rx),
                            )),
                            None => {
                                // Watcher thread stopped (shutdown). Disable only
                                // this branch and keep the app channels alive.
                                ui_theme_open = false;
                                continue;
                            }
                        }
                    }
                }
            }
        },
    ))
}


impl IcedChat {
    pub fn subscription(
        rx: Arc<Mutex<Receiver<ConversationNetEvent>>>,
        friend_rx: Arc<Mutex<Receiver<FriendEvent>>>,
        whisper_rx: Arc<Mutex<Receiver<WhisperEvent>>>,
        inbox_rx: Arc<Mutex<Receiver<InboxEvent>>>,
        discovered_peers_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<DiscoveredPeersUpdate>>>,
        reconnect_ready_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<PublicKey>>>,
        gui_action_rx: Option<Arc<Mutex<tokio::sync::mpsc::Receiver<GuiActionRequest>>>>,
        transfer_rx: Arc<Mutex<TransferUpdateReceiver>>,
        ui_theme_rx: Option<
            Arc<Mutex<tokio::sync::mpsc::Receiver<crate::theme_watcher::UiThemeReloadMsg>>>,
        >,
        layout_rx: Option<
            Arc<Mutex<tokio::sync::mpsc::Receiver<crate::layout_watcher::LayoutReloadMsg>>>,
        >,
        call_events_rx: Arc<Mutex<Receiver<CallEvent>>>,
        #[cfg(feature = "screen-sharing")] screen_share_events_rx: Option<
            Arc<Mutex<Receiver<SessionEvent>>>,
        >,
        #[cfg(feature = "screen-sharing")] screen_share_frame_watch: Option<
            Arc<Mutex<tokio::sync::watch::Receiver<Option<CapturedFrame>>>>,
        >,
        #[cfg(feature = "screen-sharing")] screen_share_stats_watch: Option<
            Arc<Mutex<tokio::sync::watch::Receiver<Option<ScreenShareStatsSnapshot>>>>,
        >,
    ) -> iced::Subscription<AppMessage> {
        let mut subs: Vec<iced::Subscription<AppMessage>> = vec![
            iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| AppMessage::ConnMonitorTick),
            iced::time::every(std::time::Duration::from_secs(1)).map(|_| AppMessage::CallUiTick),
            iced::time::every(std::time::Duration::from_secs(30))
                .map(|_| AppMessage::MeshWatchdogTick),
            iced::time::every(std::time::Duration::from_secs(30))
                .map(|_| AppMessage::OutboxRetryTick),
            // PERF-4R-B: pre-warm tick. Fires every 500 ms; the update handler
            // only builds screens while the user has been idle for 2+ seconds.
            iced::time::every(std::time::Duration::from_millis(500)).map(|_| AppMessage::IdleTick),
            iced::window::resize_events().map(|(_id, size)| AppMessage::WindowResized {
                width: size.width as f32,
                height: size.height as f32,
            }),
            // Window file drag/drop + IME composition state.  iced 0.14 maps
            // winit `HoveredFile`/`DroppedFile`/`HoveredFileCancelled` and
            // IME events into `window::Event` / `input_method::Event` which
            // arrive through the generic event listen subscription.
            iced::event::listen().filter_map(|event| match event {
                iced::Event::Window(iced::window::Event::FileHovered(_)) => {
                    Some(AppMessage::ComposerDragOver(true))
                }
                iced::Event::Window(iced::window::Event::FilesHoveredLeft) => {
                    Some(AppMessage::ComposerDragOver(false))
                }
                iced::Event::Window(iced::window::Event::FileDropped(path)) => {
                    Some(AppMessage::ComposerFileDropped(path))
                }
                // PERF-4R-B: any keyboard/mouse event counts as user activity and
                // resets the idle timer so pre-warming pauses while active.
                iced::Event::Keyboard(_) | iced::Event::Mouse(_) => Some(AppMessage::UserActivity),
                iced::Event::InputMethod(ev) => match ev {
                    // Only an active preedit (composition) must block sending.
                    // `Opened` fires merely when a text field gains focus and
                    // enables the input method — treating it as composing
                    // would freeze the composer in environments without a
                    // real IME session.
                    iced::advanced::input_method::Event::Opened => {
                        Some(AppMessage::ComposerImeActive(false))
                    }
                    iced::advanced::input_method::Event::Preedit(_, _) => {
                        Some(AppMessage::ComposerImeActive(true))
                    }
                    iced::advanced::input_method::Event::Closed
                    | iced::advanced::input_method::Event::Commit(_) => {
                        Some(AppMessage::ComposerImeActive(false))
                    }
                },
                _ => None,
            }),
        ];
        // Main subscription stream — only added when gui_action_rx is available,
        // because the unfold state cannot be expressed conditionally within the
        // stream type.  If gui_action_rx is None we still need a stream, so we
        // fall back to a dummy receiver created by dropping the sender.
        let gui_action_inner: Arc<Mutex<tokio::sync::mpsc::Receiver<GuiActionRequest>>> =
            gui_action_rx.unwrap_or_else(|| {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                drop(tx);
                Arc::new(Mutex::new(rx))
            });
        // BORU-UI-06: dev theme reload channel. When the watcher is absent
        // (headless launches, tests) fall back to a closed dummy receiver so
        // the combined stream still has a stable ui_theme branch.
        let ui_theme_inner: Arc<
            Mutex<tokio::sync::mpsc::Receiver<crate::theme_watcher::UiThemeReloadMsg>>,
        > = ui_theme_rx.unwrap_or_else(|| {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            drop(tx);
            Arc::new(Mutex::new(rx))
        });
        subs.push(iced::Subscription::run_with(
            (
                RxHandle(rx),
                FriendRxHandle(friend_rx),
                WhisperRxHandle(whisper_rx),
                InboxRxHandle(inbox_rx),
                DiscoveredPeersRxHandle(discovered_peers_rx),
                ReconnectReadyRxHandle(reconnect_ready_rx),
                GuiActionHandle(gui_action_inner),
                TransferProjectionHandle(transfer_rx),
                UiThemeRxHandle(ui_theme_inner),
            ),
            |(
                rx,
                friend_rx,
                whisper_rx,
                inbox_rx,
                discovered_rx,
                reconnect_rx,
                gui_action_rx,
                transfer_rx,
                ui_theme_rx,
            )| {
                subscription_stream(
                    rx,
                    friend_rx,
                    whisper_rx,
                    inbox_rx,
                    discovered_rx,
                    reconnect_rx,
                    gui_action_rx,
                    transfer_rx,
                    ui_theme_rx,
                )
            },
        ));
        subs.push(call_subscription(call_events_rx));
        // BORU-LAYOUT-06: dev layout reload channel. Mirrors the theme
        // reload delivery; update_layout_reloaded merges + applies.
        subs.push(layout_subscription(layout_rx));
        #[cfg(feature = "screen-sharing")]
        subs.push(screen_share_events_subscription(screen_share_events_rx));
        #[cfg(feature = "screen-sharing")]
        subs.push(screen_share_frame_subscription(screen_share_frame_watch));
        #[cfg(feature = "screen-sharing")]
        subs.push(screen_share_stats_subscription(screen_share_stats_watch));
        #[cfg(feature = "screen-sharing")]
        subs.push(screen_share_keyboard_subscription());
        iced::Subscription::batch(subs)
    }

    // ── Pre-warm (PERF-4R-B) ─────────────────────────────────────────────

    /// Build the next un-warmed screen's fully-materialized widget tree during
    /// idle and store it in `prewarm_cache`, so the first `view()` frame after
    /// navigation can serve it without rebuilding. Called from
    /// `AppMessage::IdleTick`; each call builds at most one screen so a single
    /// idle tick never causes a perceptible frame hitch.
    fn pre_warm_next_screen(&mut self) {
        if self.prewarming {
            return;
        }
        if !self.idle_timer.is_idle() {
            return;
        }
        // BORU-UI-19: theme edits (inspector slider storms, boru-ui.toml
        // reloads) only set the pending flag; the actual invalidation is
        // coalesced here, once per idle period after the burst settles.
        // This keeps the expensive prewarm rebuilds off the per-event
        // slider path while leaving the visual feedback (active_theme +
        // theme_revision) immediate.
        if self.prewarm_invalidate_pending {
            self.prewarm_invalidate_pending = false;
            self.invalidate_prewarm(PREWARM_ORDER);
        }
        // Responsive mode is not part of the per-screen dependency snapshots
        // (except FileSharing's own band), so pre-warmed trees built for
        // another width band are stale — rebuild everything on a band flip.
        let mode = ResponsiveMode::of(self.window_width);
        if self.prewarm_window_mode.is_some_and(|m| m != mode) {
            self.prewarm_cache.clear();
        }
        self.prewarm_window_mode = Some(mode);

        let Some(screen) = PREWARM_ORDER
            .iter()
            .cloned()
            .find(|s| !self.prewarm_cache.contains_key(s))
        else {
            // All pre-warmable screens are already in the cache.
            return;
        };

        // FileSharing is only pre-warmed while the default Files tab is
        // active; owned tabs render entirely different trees (live path).
        if screen == Screen::FileSharing
            && self.files_state.dashboard_active_tab != crate::dashboard_view_model::DashboardTab::SharedByMe
        {
            return;
        }

        // ICEDAW-01: Settings is only pre-warmed while the ColorPicker is
        // closed. When it is open the live `lazy` path must render (Prebuilt
        // drops overlays, which would make the picker invisible).
        if screen == Screen::Settings && self.settings_state.show_accent_picker {
            return;
        }

        self.prewarming = true;
        let (hash, element) = self.build_prewarm_entry(screen.clone());
        self.prewarm_cache.insert(
            screen,
            (hash, std::rc::Rc::new(std::cell::RefCell::new(element))),
        );
        self.prewarming = false;
    }

    /// Build the dependency snapshot for one pre-warmable screen and return it
    /// with its FxHash, calling the STATIC content function directly (never
    /// through `iced::widget::lazy`, whose closure only runs during
    /// `state()`/`diff()` — building an element in `update()` and dropping it
    /// would populate no cache).
    fn build_prewarm_entry(&self, screen: Screen) -> (u64, iced::Element<'static, AppMessage>) {
        match screen {
            Screen::Settings => {
                let dep = self.settings_dependency();
                let hash = fxhash_of(&dep);
                let btheme = self.boru_theme();
                let element = Self::view_settings_screen_content(
                    &dep,
                    self.settings_state.profile_image_handle.clone(),
                    btheme,
                );
                (hash, element)
            }
            Screen::Discover => {
                let dep = self.discover_dependency();
                let hash = fxhash_of(&dep);
                let element = Self::view_discover_content(&dep);
                (hash, element)
            }
            Screen::Groups => {
                let dep = self.groups_dependency();
                let hash = fxhash_of(&dep);
                let element = Self::view_groups_screen_content(&dep);
                (hash, element)
            }
            Screen::FriendRequests => {
                let dep = self.friend_requests_dependency();
                let hash = fxhash_of(&dep);
                let element = Self::view_friend_requests_content(&dep);
                (hash, element)
            }
            Screen::FileSharing => {
                let dep = self.file_sharing_dependency();
                let hash = fxhash_of(&dep);
                let element = Self::view_file_sharing_content(&dep);
                (hash, element)
            }
            // PREWARM_ORDER only ever contains the parameterless screens
            // above; anything else would be a programming error.
            _ => unreachable!("build_prewarm_entry called for a non-prewarmable screen"),
        }
    }

    /// Serve a pre-warmed screen tree from `prewarm_cache` when its stored
    /// dependency hash matches the CURRENT state; otherwise fall back to the
    /// live view (which keeps its own `iced::widget::lazy` caching). The
    /// cached element is handed out each frame wrapped in [`Prebuilt`], so the
    /// expensive widget construction never runs again while the hash matches.
    fn serve_prewarmed<'a>(
        &'a self,
        screen: Screen,
        live: impl FnOnce() -> iced::Element<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        let Some((cached_hash, element)) = self.prewarm_cache.get(&screen) else {
            return live();
        };
        let current_hash = match screen {
            Screen::Settings => fxhash_of(&self.settings_dependency()),
            Screen::Discover => fxhash_of(&self.discover_dependency()),
            Screen::Groups => fxhash_of(&self.groups_dependency()),
            Screen::FriendRequests => fxhash_of(&self.friend_requests_dependency()),
            Screen::FileSharing => fxhash_of(&self.file_sharing_dependency()),
            _ => return live(),
        };
        if *cached_hash == current_hash {
            iced::Element::new(Prebuilt(element.clone()))
        } else {
            live()
        }
    }

    /// Forget pre-warmed screens whose dependency would have changed, so the
    /// next idle cycle rebuilds them with fresh state. `screens` is the set of
    /// affected screens; pass `PREWARM_ORDER` to invalidate everything
    /// (dark-mode toggle, responsive breakpoint crossing).
    fn invalidate_prewarm(&mut self, screens: &[Screen]) {
        for screen in screens {
            self.prewarm_cache.remove(screen);
        }
    }

    /// Rebuild the internal join-request list from `outgoing_request_states`
    /// and the friend request store.
    fn rebuild_join_request_list(&mut self) {
        let local_pk = self.local_public;
        let mut items: Vec<JoinRequestItem> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        for (peer, state) in &self.outgoing_request_states {
            let peer_str = peer.to_string();
            let request_id = self
                .friend_request_store
                .iter()
                .find(|r| r.requester == local_pk.to_string() && r.recipient == peer_str)
                .map(|r| r.id.clone())
                .unwrap_or_else(|| format!("outgoing:{}", &peer_str[..8]));

            if !seen_ids.insert(request_id.clone()) {
                continue;
            }

            let chat_id = direct_topic(&local_pk, peer);
            items.push(JoinRequestItem::new(
                request_id,
                peer_str,
                chat_id,
                state.clone(),
            ));
        }

        items.sort_by_key(|item| match item.state {
            OutgoingRequestState::Pending => 0u8,
            OutgoingRequestState::Failed(_) => 1,
            OutgoingRequestState::Accepted => 2,
            OutgoingRequestState::Declined => 3,
        });

        self.join_request_list = items;
    }

    /// Return a reference to the structured join-request list.
    #[expect(dead_code)]
    pub fn join_requests(&self) -> &[JoinRequestItem] {
        &self.join_request_list
    }

    /// Confirmation overlay for removing a friend.
    fn view_remove_confirm_overlay<'a>(
        &self,
        _peer: PublicKey,
        name: &str,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, column, container, row, text, Space};
        use iced::{Alignment, Length};

        let dialog = column![]
            .push(
                text(crate::i18n::t_args(
                    "dialogs.remove_confirm.friend_message",
                    &[("name", name)],
                ))
                .size(TYPO_SM)
                .width(Length::Shrink),
            )
            .push(Space::new().height(SPACE_16))
            .push(
                row![]
                    .push(
                        button(text(crate::i18n::t("common.cancel")).size(TYPO_SM))
                            .on_press(AppMessage::CancelRemoveFriend)
                            .padding([SPACE_6, SPACE_12])
                            .width(Length::Fill)
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(bg_surface(t))),
                                text_color: text_muted(t),
                                border: iced::Border {
                                    color: border_muted(t),
                                    width: 1.0,
                                    radius: SPACE_6.into(),
                                },
                                ..Default::default()
                            }),
                    )
                    .push(
                        button(text(crate::i18n::t("common.remove")).size(TYPO_SM))
                            .on_press(AppMessage::ConfirmRemoveFriend)
                            .padding([SPACE_6, SPACE_12])
                            .width(Length::Fill)
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(color_error(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_6.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
            )
            .spacing(SPACE_8)
            .align_x(Alignment::Center);

        let overlay = container(dialog)
            .width(Length::Fixed(360.0))
            .height(Length::Shrink)
            .padding(SPACE_24)
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    radius: 12.0.into(),
                    width: 1.0,
                    color: border_muted(t),
                },
                ..Default::default()
            });

        iced::widget::stack![
            base,
            container(overlay)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// Confirmation overlay for blocking a friend.
    fn view_block_confirm_overlay<'a>(
        &self,
        _peer: PublicKey,
        name: &str,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, column, container, row, text, Space};
        use iced::{Alignment, Length};

        let dialog = column![]
            .push(
                text(crate::i18n::t_args(
                    "dialogs.block_confirm.message_with_name",
                    &[("name", name)],
                ))
                .size(TYPO_SM)
                .width(Length::Shrink),
            )
            .push(Space::new().height(SPACE_16))
            .push(
                row![]
                    .push(
                        button(text(crate::i18n::t("common.cancel")).size(TYPO_SM))
                            .on_press(AppMessage::CancelBlockFriend)
                            .padding([SPACE_6, SPACE_12])
                            .width(Length::Fill)
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(bg_surface(t))),
                                text_color: text_muted(t),
                                border: iced::Border {
                                    color: border_muted(t),
                                    width: 1.0,
                                    radius: SPACE_6.into(),
                                },
                                ..Default::default()
                            }),
                    )
                    .push(
                        button(text(crate::i18n::t("common.block")).size(TYPO_SM))
                            .on_press(AppMessage::ConfirmBlockFriend)
                            .padding([SPACE_6, SPACE_12])
                            .width(Length::Fill)
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(color_error(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_6.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
            )
            .spacing(SPACE_8)
            .align_x(Alignment::Center);

        let overlay = container(dialog)
            .width(Length::Fixed(360.0))
            .height(Length::Shrink)
            .padding(SPACE_24)
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    radius: 12.0.into(),
                    width: 1.0,
                    color: border_muted(t),
                },
                ..Default::default()
            });

        iced::widget::stack![
            base,
            container(overlay)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }
}

/// Format a byte count as a human-readable string.
fn format_file_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

#[cfg(test)]
mod tests;
