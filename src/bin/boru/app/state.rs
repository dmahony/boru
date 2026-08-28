#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

//! Coordinator-owned state value types.
//!
//! This module keeps persistence-oriented state definitions out of the
//! application coordinator. The types remain re-exported by `app.rs` so the
//! existing construction and dispatch surface is unchanged.

use super::*;

// ── Shared ContinuousTracker wrapper ─────────────────────────────────
/// Wraps [`PrivateContinuousTracker`] so it can be stored in the Clone-derived
/// [`AppMessage`] enum. Inner tracker is accessed via `shutdown_shared`.
#[derive(Debug)]
pub struct SharedTracker {
    /// The underlying continuous tracker (publish + discover loops).
    tracker: Arc<tokio::sync::Mutex<Option<TrackerInner>>>,
    /// Cancellation token for the join-fanout background task, so it
    /// exits promptly when the room is left or deleted.
    join_cancel: Arc<tokio_util::sync::CancellationToken>,
}

#[derive(Debug)]
enum TrackerInner {
    Private(PrivateContinuousTracker),
    Public(PublicContinuousTracker),
}

impl SharedTracker {
    pub(crate) fn new(
        tracker: PrivateContinuousTracker,
        join_cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            tracker: Arc::new(tokio::sync::Mutex::new(Some(TrackerInner::Private(
                tracker,
            )))),
            join_cancel: Arc::new(join_cancel),
        }
    }

    pub(crate) fn new_public(tracker: PublicContinuousTracker) -> Self {
        Self {
            tracker: Arc::new(tokio::sync::Mutex::new(Some(TrackerInner::Public(tracker)))),
            join_cancel: Arc::new(tokio_util::sync::CancellationToken::new()),
        }
    }

    /// Shutdown the tracker and cancel the join-fanout task (fire-and-forget via task::spawn).
    pub(crate) fn shutdown_shared(&self) {
        self.join_cancel.cancel();
        let inner = self.tracker.clone();
        task::spawn(async move {
            if let Some(tracker) = inner.lock().await.take() {
                match tracker {
                    TrackerInner::Private(tracker) => tracker.shutdown().await,
                    TrackerInner::Public(tracker) => tracker.shutdown().await,
                }
            }
        });
    }
}

impl Clone for SharedTracker {
    fn clone(&self) -> Self {
        Self {
            tracker: self.tracker.clone(),
            join_cancel: self.join_cancel.clone(),
        }
    }
}

// ── Settings persistence ─────────────────────────────────────────
/// On-disk settings stored as JSON in the application data directory.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub dark_mode: bool,
    pub sound_enabled: bool,
    pub chat_text_size: f32,
    pub share_direct_addresses: bool,
    pub display_name: Option<String>,
    /// Absolute path to the home-screen background image (None = default).
    pub home_background_image: Option<String>,
    /// Opacity (0.0–1.0) of the home-screen menu/action item card
    /// backgrounds when a home background image is set. 1.0 = fully
    /// opaque; lower values let the background image show through.
    pub home_menu_item_opacity: f32,
    /// Optional user-selected accent color as RGB bytes (None = theme default).
    /// Wired through the iced_aw ColorPicker in Settings → APPEARANCE.
    pub accent_color: Option<[u8; 3]>,
    /// Whether the optional BORU-CP-06 presence indicator is shown in the
    /// UI. Disabling it only hides the presentation — it never affects
    /// discovery or reconnection (PDF 2.3 guardrail).
    pub show_presence_indicator: bool,
    /// Whether ephemeral typing indicators may be sent to peers.
    pub typing_indicators_enabled: bool,
    /// Recently-used emoji as plain Unicode strings (BORU-TWEMOJI-14).
    /// Local settings only — this list is never transmitted on the wire and
    /// never stores asset keys, SVG paths or image bytes. The picker renders
    /// each entry through the shared resolver/fallback pipeline.
    pub recent_emojis: Vec<String>,
    /// Global message notification policy (all, mentions-only, or muted).
    pub notification_policy: crate::notification::service::NotificationPolicy,
    /// Explicit per-conversation overrides keyed by stable TopicId hex.
    pub conversation_notification_policies:
        Vec<(String, crate::notification::service::NotificationPolicy)>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            dark_mode: false,
            sound_enabled: true,
            chat_text_size: TYPO_SM,
            share_direct_addresses: false,
            display_name: None,
            home_background_image: None,
            home_menu_item_opacity: HOME_MENU_ITEM_OPACITY_DEFAULT,
            accent_color: None,
            show_presence_indicator: true,
            typing_indicators_enabled: true,
            recent_emojis: Vec::new(),
            notification_policy: crate::notification::service::NotificationPolicy::All,
            conversation_notification_policies: Vec::new(),
        }
    }
}

/// Default opacity of home-screen menu/action card backgrounds (0.85 =
/// 85% opaque) used when no explicit value is persisted in settings.json.
pub(crate) const HOME_MENU_ITEM_OPACITY_DEFAULT: f32 = 0.85;

impl AppSettings {
    const FILE_NAME: &'static str = "settings.json";

    /// Load settings from disk, or return defaults if the file doesn't exist.
    pub fn load(data_dir: &std::path::Path) -> Self {
        let path = data_dir.join(Self::FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save settings to disk in the application data directory.
    pub fn save(&self, data_dir: &std::path::Path) {
        let path = data_dir.join(Self::FILE_NAME);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}
