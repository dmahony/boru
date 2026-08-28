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

//! Rooms & directory domain.
//!
//! Extracted from app.rs (BORU-APP-006). Owns the create-room and
//! room-settings dialogs, the room-directory advertising state
//! (advertised set, dedupe fingerprints, periodic refresh counter,
//! startup sweep, auto-subscribed set, DHT trackers) and the
//! `impl IcedChat` methods that drive them. Reads app state via
//! `use super::*`; app.rs re-exports the pub(crate) items it still
//! references with `use rooms::*`.

use super::*;

pub(crate) const STARTUP_ADVERT_JITTER_MAX_MS: u64 = 2_000;
/// Dedupe window: an unchanged room advertisement is not re-broadcast
/// within this window of the last broadcast (PDF Task 3.1 step 5 —
/// "avoid repeatedly publishing unchanged room metadata more often than
/// necessary"). Shorter than the ~60 s periodic refresh cadence, so the
/// periodic tick still refreshes each room once per cadence while the
/// startup sweep and immediate publishes never double-broadcast
/// identical metadata back-to-back.
pub(crate) const ADVERT_DEDUPE_WINDOW: Duration = Duration::from_secs(30);

// ── Advertisement lifetime (BORU-DIR-08, PDF Task 3.2) ───────────────
/// Advertisement TTL (seconds) — the expiry/refresh mechanism. Every
/// published advertisement carries `expires_after_secs = ADVERT_TTL_SECS`;
/// directory clients consider it stale `ADVERT_TTL_SECS` after receipt and
/// evict it unless the advertiser refreshes first. Defined here as the
/// crate protocol default (300 s) so publishers and receivers agree.
///
/// Deliberately **much longer than the refresh interval** (60 s — a 5:1
/// margin): a few lost refreshes from temporary packet loss must not flicker
/// a room out of the directory (PDF Task 3.2 step 5). A room only leaves
/// after its advertiser stops refreshing for the full TTL.
pub(crate) const ADVERT_TTL_SECS: u32 = boru_core::chat_core::DEFAULT_ADVERT_TTL_SECS;

/// Periodic refresh interval (seconds) — how often the app re-broadcasts
/// advertisements for rooms in [`IcedChat::advertised_rooms`]. The
/// `advertise_counter` counts 1-second ConnMonitorTicks and resets to this
/// value (plus jitter) after each broadcast, so the wire cadence is
/// 60–65 s — significantly shorter than [`ADVERT_TTL_SECS`].
pub(crate) const ADVERT_REFRESH_INTERVAL_SECS: u64 = 60;

/// Periodic cadence (seconds) for global-registry DHT lookups.
///
/// Every interval the app enlists the registry namespace to discover public
/// rooms that were published over the DHT (relay-independent), merging them
/// into the local `directory_store` so they surface in PUBLIC ROOMS alongside
/// the gossip-directory advertisements.
pub(crate) const REGISTRY_LOOKUP_INTERVAL_SECS: u32 = 120;

/// Maximum extra jitter (seconds) added to the periodic refresh cadence.
/// Each cycle resets the counter to `ADVERT_REFRESH_INTERVAL_SECS +
/// random(0..=ADVERT_REFRESH_JITTER_SECS)`, so advertisers that start at
/// similar times drift out of phase and do not re-broadcast in synchronized
/// bursts (PDF Task 3.2 step 3).
pub(crate) const ADVERT_REFRESH_JITTER_SECS: u64 = 5;

/// Stable fingerprint of the advertised metadata that would be broadcast
/// for a room (BORU-DIR-07 dedupe). Computed over the fields that define
/// the *advertisement content* (room identity, name, description, join
/// ticket) — NOT the volatile counters (member count, last activity) that
/// legitimately change between refreshes. Two broadcasts with the same
/// fingerprint carry the same room metadata, so a second one within
/// [`ADVERT_DEDUPE_WINDOW`] is redundant.
fn startup_advertisement_fingerprint(
    topic: TopicId,
    room_name: &str,
    description: &str,
    ticket: &str,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    topic.hash(&mut hasher);
    room_name.hash(&mut hasher);
    description.hash(&mut hasher);
    ticket.hash(&mut hasher);
    hasher.finish()
}

/// Stable widget ID used to restore focus to the create-room name input.
const CREATE_ROOM_NAME_INPUT: &str = "create-room-name-input";

/// DomainState for the rooms/directory domain (BORU-APP-006).
///
/// Owns the create-room dialog state, the room-settings dialog state,
/// and the room-directory advertising state (advertised set, dedupe
/// fingerprints, periodic refresh counter, startup sweep, auto-subscribed
/// set, per-room DHT trackers). `IcedChat` holds exactly one instance
/// (`self.rooms_state`); there is no mirror of this state anywhere else
/// (PDF §14 "same state in both modules" stop condition).
///
/// ## What stays on the App shell
///
/// - `directory_topic` / `directory_sender` / `directory_store` /
///   `directory_room_rx` remain on `IcedChat` — they are shared
///   discovery infrastructure read by the net layer, `discover.rs`,
///   MCP, and tests (architecture-boundaries §3.9).
/// - The `conversation_store`, `storage`, and sidebar revision counters
///   are shared read/write context owned by the shell.
#[derive(Debug)]
pub(crate) struct RoomsState {
    /// Whether the "Enable DHT discovery" checkbox is checked in the
    /// create-room dialog.  Default: on (DHT discovery enabled).
    pub(crate) create_room_dht_enabled: bool,
    /// Name for the new room entered in the create-room dialog.
    pub(crate) create_room_name: String,
    /// Visibility selected for the new room in the create-room dialog
    /// (BORU-DIR-05, PDF Task 2.2). Conservative default: PublicUnlisted.
    pub(crate) create_room_visibility: RoomVisibility,
    /// Optional description entered in the create-room dialog
    /// (BORU-DIR-05, PDF Task 2.2).
    pub(crate) create_room_description: String,
    /// Optional comma-separated tags entered in the create-room dialog
    /// (BORU-DIR-05, PDF Task 2.2).
    pub(crate) create_room_tags: String,
    /// Whether the create-room dialog is currently shown.
    pub(crate) show_create_room_dialog: bool,
    /// Whether the create-room submit is in flight (async subscribe).
    /// While true the Create button shows a loading state, is disabled, and
    /// the dialog cannot be dismissed (Escape/backdrop/Cancel are no-ops).
    pub(crate) create_room_submitting: bool,
    /// Inline error shown inside the create-room dialog (name field area).
    pub(crate) create_room_error: Option<String>,
    /// Whether the room-settings dialog is currently shown (BORU-DIR-06).
    /// Owner/admin-only: lets the owner switch directory visibility and edit
    /// advertised metadata (name / description / tags) for a public room.
    pub(crate) show_room_settings_dialog: bool,
    /// Topic of the room being edited in the room-settings dialog.
    pub(crate) room_settings_topic: Option<TopicId>,
    /// Pre-filled room name in the room-settings dialog.
    pub(crate) room_settings_name: String,
    /// Pre-filled description in the room-settings dialog.
    pub(crate) room_settings_description: String,
    /// Pre-filled comma-separated tags in the room-settings dialog.
    pub(crate) room_settings_tags: String,
    /// Visibility selected in the room-settings dialog (BORU-DIR-06).
    pub(crate) room_settings_visibility: RoomVisibility,
    /// Inline error shown inside the room-settings dialog.
    pub(crate) room_settings_error: Option<String>,
    /// Which rooms are being advertised into the directory topic.
    pub(crate) advertised_rooms: HashSet<TopicId>,
    /// Counter for periodic room-advertisement broadcast (decremented per
    /// ConnMonitorTick; broadcasts when it hits 0, resets to 60).
    pub(crate) advertise_counter: u32,
    /// Seconds until the next periodic global-registry discovery lookup
    /// (decremented per ConnMonitorTick; on 0, spawn a DHT registry lookup
    /// and reset to [`REGISTRY_LOOKUP_INTERVAL_SECS`]).
    pub(crate) registry_lookup_counter: u32,
    /// BORU-DIR-07 (PDF Task 3.1): fingerprint of the last advertisement
    /// actually broadcast per room (topic + name + description + ticket).
    /// Used to avoid repeatedly publishing unchanged room metadata more
    /// often than necessary (dedupe unchanged advertisements).
    pub(crate) last_advertised_fingerprint: HashMap<TopicId, u64>,
    /// When each room's advertisement was last broadcast (BORU-DIR-07
    /// dedupe window; [`ADVERT_DEDUPE_WINDOW`]).
    pub(crate) last_advertised_at: HashMap<TopicId, Instant>,
    /// BORU-DIR-07 (PDF Task 3.1): true once the startup discoverable-room
    /// advertisement sweep has run, so a directory re-subscribe or tick
    /// cannot re-trigger the burst.
    pub(crate) startup_advertise_swept: bool,
    /// Topics already queued for automatic subscription after discovery.
    pub(crate) auto_subscribed_rooms: HashSet<TopicId>,
    /// Per-room continuous DHT trackers for private rooms with discovery enabled.
    /// Started when creating/joining a DHT-enabled room; shut down when
    /// leaving or deleting the room.
    pub(crate) room_trackers: HashMap<TopicId, SharedTracker>,
}

impl RoomsState {
    /// Create the rooms/directory domain state with the same defaults the
    /// inline `app.rs` fields used.
    pub(crate) fn new() -> Self {
        Self {
            create_room_dht_enabled: true,
            create_room_name: String::new(),
            create_room_visibility: RoomVisibility::PublicUnlisted,
            create_room_description: String::new(),
            create_room_tags: String::new(),
            show_create_room_dialog: false,
            create_room_submitting: false,
            create_room_error: None,
            show_room_settings_dialog: false,
            room_settings_topic: None,
            room_settings_name: String::new(),
            room_settings_description: String::new(),
            room_settings_tags: String::new(),
            room_settings_visibility: RoomVisibility::PublicUnlisted,
            room_settings_error: None,
            advertised_rooms: HashSet::new(),
            advertise_counter: 60,
            registry_lookup_counter: REGISTRY_LOOKUP_INTERVAL_SECS,
            last_advertised_fingerprint: HashMap::new(),
            last_advertised_at: HashMap::new(),
            startup_advertise_swept: false,
            auto_subscribed_rooms: HashSet::new(),
            room_trackers: HashMap::new(),
        }
    }

    /// Apply one domain message (state-only transitions).
    ///
    /// Only this domain's state is mutated. None of the current messages
    /// require a shell side effect, so no event is returned; the shell just
    /// routes the matching `AppMessage` variant here (converted to
    /// [`RoomsMessage`]) and returns `Task::none()`.
    pub(crate) fn update(&mut self, msg: RoomsMessage) {
        match msg {
            RoomsMessage::CreateNewRoomDhtToggled(enabled) => {
                self.create_room_dht_enabled = enabled;
            }
            RoomsMessage::CreateNewRoomNameChanged(name) => {
                self.create_room_name = name;
            }
            RoomsMessage::CreateNewRoomVisibilityChanged(visibility) => {
                self.create_room_visibility = visibility;
            }
            RoomsMessage::CreateNewRoomDescriptionChanged(description) => {
                self.create_room_description = description;
            }
            RoomsMessage::CreateNewRoomTagsChanged(tags) => {
                self.create_room_tags = tags;
            }
            RoomsMessage::RoomSettingsNameChanged(name) => {
                self.room_settings_name = name;
                self.room_settings_error = None;
            }
            RoomsMessage::RoomSettingsDescriptionChanged(description) => {
                self.room_settings_description = description;
                self.room_settings_error = None;
            }
            RoomsMessage::RoomSettingsTagsChanged(tags) => {
                self.room_settings_tags = tags;
                self.room_settings_error = None;
            }
            RoomsMessage::RoomSettingsVisibilityChanged(visibility) => {
                self.room_settings_visibility = visibility;
                self.room_settings_error = None;
            }
        }
    }
}

/// Domain messages for the rooms/directory domain (BORU-APP-006).
///
/// State-only transitions routed through [`RoomsState::update`]. Shell
/// context arms (create submit, settings save, directory visibility)
/// remain as `AppMessage` variants dispatched to [`IcedChat::update_rooms`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoomsMessage {
    /// The "Enable DHT discovery" checkbox changed in the create-room dialog.
    CreateNewRoomDhtToggled(bool),
    /// The room name input changed in the create-room dialog.
    CreateNewRoomNameChanged(String),
    /// The visibility radio changed in the create-room dialog.
    CreateNewRoomVisibilityChanged(RoomVisibility),
    /// The description input changed in the create-room dialog.
    CreateNewRoomDescriptionChanged(String),
    /// The tags input changed in the create-room dialog.
    CreateNewRoomTagsChanged(String),
    /// The room name input changed in the room-settings dialog.
    RoomSettingsNameChanged(String),
    /// The description input changed in the room-settings dialog.
    RoomSettingsDescriptionChanged(String),
    /// The tags input changed in the room-settings dialog.
    RoomSettingsTagsChanged(String),
    /// The visibility radio changed in the room-settings dialog.
    RoomSettingsVisibilityChanged(RoomVisibility),
}

impl IcedChat {
    /// State-layer update for the rooms/directory domain (BORU-APP-006).
    ///
    /// Handles the create-room dialog (open/cancel/field changes/async
    /// submit), the room-settings dialog (open/edit/save), the directory
    /// visibility switch, and verified directory withdrawals. The root
    /// `update()` dispatches these variants here via combined match arms.
    /// State-only field transitions route through
    /// [`RoomsState::update`](RoomsState::update); heavier arms that need
    /// shell context (conversation store, storage, gossip senders) run
    /// inline and read/write the moved state through `self.rooms_state`.
    pub(crate) fn update_rooms(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::CreateNewRoom => {
                self.rooms_state.show_create_room_dialog = true;
                self.rooms_state.create_room_dht_enabled = true;
                self.rooms_state.create_room_name = String::new();
                // BORU-DIR-05 (PDF Task 2.2): conservative default — a new
                // public room is unlisted unless the creator explicitly opts
                // into discoverability.
                self.rooms_state.create_room_visibility = RoomVisibility::PublicUnlisted;
                self.rooms_state.create_room_description = String::new();
                self.rooms_state.create_room_tags = String::new();
                self.rooms_state.create_room_submitting = false;
                self.rooms_state.create_room_error = None;
                if let Some(action_id) = self.pending_create_room_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                // Auto-focus the first meaningful field (room name) so the
                // user can type immediately after opening the dialog.
                iced::widget::operation::focus(CREATE_ROOM_NAME_INPUT)
            }

            AppMessage::CancelCreateRoom => {
                // A dialog mid-submit must not be dismissed (Escape, backdrop
                // click and Cancel all route here); the submit is in flight.
                if self.rooms_state.create_room_submitting {
                    return iced::Task::none();
                }
                self.rooms_state.show_create_room_dialog = false;
                self.rooms_state.create_room_error = None;
                self.complete_close_dialog_action();
                iced::Task::none()
            }

            // ── Room directory visibility (BORU-DIR-06, PDF 2.3) ────────
            AppMessage::OpenRoomSettings(topic) => {
                // Owner/admin-only: only rooms the local user owns (created
                // as a public room, or already advertised) get the settings
                // dialog. Non-authorized users cannot change directory
                // visibility (PDF Task 2.3).
                if !self.is_room_directory_owner(topic) {
                    self.push_system("Only the room owner can change directory visibility.");
                    return iced::Task::none();
                }
                let (name, description, tags, visibility) = self
                    .conversation_store
                    .find(&topic)
                    .map(|e| {
                        (
                            e.name.clone(),
                            e.description.clone(),
                            e.tags.join(","),
                            e.visibility,
                        )
                    })
                    .unwrap_or_else(|| {
                        (
                            topic.to_string(),
                            String::new(),
                            String::new(),
                            RoomVisibility::PublicUnlisted,
                        )
                    });
                self.rooms_state.room_settings_topic = Some(topic);
                self.rooms_state.room_settings_name = name;
                self.rooms_state.room_settings_description = description;
                self.rooms_state.room_settings_tags = tags;
                self.rooms_state.room_settings_visibility = visibility;
                self.rooms_state.room_settings_error = None;
                self.rooms_state.show_room_settings_dialog = true;
                iced::Task::none()
            }

            AppMessage::RoomSettingsNameChanged(name) => {
                self.rooms_state
                    .update(RoomsMessage::RoomSettingsNameChanged(name));
                iced::Task::none()
            }

            AppMessage::RoomSettingsDescriptionChanged(description) => {
                self.rooms_state
                    .update(RoomsMessage::RoomSettingsDescriptionChanged(description));
                iced::Task::none()
            }

            AppMessage::RoomSettingsTagsChanged(tags) => {
                self.rooms_state
                    .update(RoomsMessage::RoomSettingsTagsChanged(tags));
                iced::Task::none()
            }

            AppMessage::RoomSettingsVisibilityChanged(visibility) => {
                self.rooms_state
                    .update(RoomsMessage::RoomSettingsVisibilityChanged(visibility));
                iced::Task::none()
            }

            AppMessage::CancelRoomSettings => {
                self.rooms_state.show_room_settings_dialog = false;
                self.rooms_state.room_settings_topic = None;
                self.rooms_state.room_settings_error = None;
                self.complete_close_dialog_action();
                iced::Task::none()
            }

            AppMessage::ConfirmRoomSettings => {
                let Some(topic) = self.rooms_state.room_settings_topic else {
                    return iced::Task::none();
                };
                // Owner gate (defence in depth — the dialog is only opened
                // for owners, but never trust the stored topic blindly).
                if !self.is_room_directory_owner(topic) {
                    self.push_system("Only the room owner can change directory visibility.");
                    self.rooms_state.show_room_settings_dialog = false;
                    self.rooms_state.room_settings_topic = None;
                    return iced::Task::none();
                }
                // Validate + normalize the edited metadata before persisting
                // or republishing (same bounds as the create flow).
                let raw_name = std::mem::take(&mut self.rooms_state.room_settings_name);
                let raw_description =
                    std::mem::take(&mut self.rooms_state.room_settings_description);
                let raw_tags = std::mem::take(&mut self.rooms_state.room_settings_tags);
                let requested_visibility = self.rooms_state.room_settings_visibility;
                let bounds = AdvertisementBounds::default();
                let normalized = match normalize_room_metadata(
                    &raw_name,
                    &raw_description,
                    &raw_tags,
                    &bounds,
                ) {
                    Ok(n) => n,
                    Err(violation) => {
                        // Restore the fields so the owner can fix the input.
                        self.rooms_state.room_settings_name = raw_name;
                        self.rooms_state.room_settings_description = raw_description;
                        self.rooms_state.room_settings_tags = raw_tags;
                        self.rooms_state.room_settings_error = Some(violation.to_string());
                        return iced::Task::none();
                    }
                };
                // Persist the edited metadata (name / description / tags)
                // and the new visibility on the room entry. Room identity
                // (topic) is unchanged — metadata edits never change it.
                {
                    let changed = self
                        .conversation_store
                        .find_mut(&topic)
                        .map(|entry| {
                            entry.name = normalized.room_name.clone();
                            entry.description = normalized.short_description.clone();
                            entry.tags = normalized.tags.clone();
                            entry.visibility = requested_visibility;
                            true
                        })
                        .unwrap_or(false);
                    if changed {
                        if let Some(ref st) = self.storage {
                            let _ = self.conversation_store.save_to_sqlite(st);
                        }
                    }
                }
                self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                // Close the dialog, then apply the visibility switch + publish.
                self.rooms_state.show_room_settings_dialog = false;
                self.rooms_state.room_settings_topic = None;
                self.rooms_state.room_settings_error = None;
                let task = self.apply_room_directory_visibility(topic, requested_visibility);
                // A discoverable room whose metadata changed must also
                // republish immediately, even when the visibility did not
                // change (PDF Task 2.3 step 5).
                if self
                    .conversation_store
                    .find(&topic)
                    .map(|e| e.visibility.is_discoverable())
                    .unwrap_or(false)
                {
                    if let Some(republish) = self.immediate_room_advertisement_task(topic) {
                        return iced::Task::batch(vec![task, republish]);
                    }
                }
                task
            }

            AppMessage::SetRoomDirectoryVisibility { topic, visibility } => {
                self.apply_room_directory_visibility(topic, visibility)
            }

            AppMessage::CreateNewRoomDhtToggled(enabled) => {
                self.rooms_state
                    .update(RoomsMessage::CreateNewRoomDhtToggled(enabled));
                iced::Task::none()
            }

            AppMessage::CreateNewRoomNameChanged(name) => {
                self.rooms_state
                    .update(RoomsMessage::CreateNewRoomNameChanged(name));
                iced::Task::none()
            }

            AppMessage::CreateNewRoomVisibilityChanged(visibility) => {
                self.rooms_state
                    .update(RoomsMessage::CreateNewRoomVisibilityChanged(visibility));
                iced::Task::none()
            }

            AppMessage::CreateNewRoomDescriptionChanged(description) => {
                self.rooms_state
                    .update(RoomsMessage::CreateNewRoomDescriptionChanged(description));
                iced::Task::none()
            }

            AppMessage::CreateNewRoomTagsChanged(tags) => {
                self.rooms_state
                    .update(RoomsMessage::CreateNewRoomTagsChanged(tags));
                iced::Task::none()
            }

            AppMessage::ConfirmCreateNewRoom => {
                // Guard: never re-enter while a submit is in flight.
                if self.rooms_state.create_room_submitting {
                    return iced::Task::none();
                }
                // Keep the dialog open while the room is created (async
                // subscribe + open); the primary button shows a loading state
                // and Escape/backdrop/Cancel are disabled until completion.
                self.rooms_state.create_room_submitting = true;
                self.rooms_state.create_room_error = None;
                let dht_enabled =
                    self.rooms_state.create_room_dht_enabled && !self.private_dht_disabled;
                let room_name = std::mem::take(&mut self.rooms_state.create_room_name);
                let description = std::mem::take(&mut self.rooms_state.create_room_description);
                let tags = std::mem::take(&mut self.rooms_state.create_room_tags);
                let visibility = self.rooms_state.create_room_visibility;

                // ── Public room: advertise without auto-joining ──────
                if visibility != RoomVisibility::Private {
                    let topic = TopicId::from_bytes(rand::random());
                    // Brand-new room: no mesh neighbors are subscribed to
                    // this topic yet, so no extra bootstrap peers apply.
                    let ticket = self.room_ticket(topic, &[]);
                    let ticket_str = ticket.to_string();
                    // Empty names fall back to the topic id (existing
                    // behaviour), so the advertisement always has a
                    // non-empty name.
                    let display_name = if room_name.trim().is_empty() {
                        topic.to_string()
                    } else {
                        room_name
                    };
                    // BORU-DIR-05 (PDF Task 2.2): validate and normalize
                    // creator metadata BEFORE any side effect or broadcast.
                    // Invalid/oversized metadata is rejected here — the
                    // dialog stays open and no advertisement is emitted.
                    let bounds = AdvertisementBounds::default();
                    let normalized = match normalize_room_metadata(
                        &display_name,
                        &description,
                        &tags,
                        &bounds,
                    ) {
                        Ok(n) => n,
                        Err(violation) => {
                            self.rooms_state.create_room_submitting = false;
                            self.rooms_state.create_room_error = Some(violation.to_string());
                            // Restore the form fields so the creator can
                            // correct the rejected input.
                            self.rooms_state.create_room_name = display_name;
                            self.rooms_state.create_room_description = description;
                            self.rooms_state.create_room_tags = tags;
                            return iced::Task::none();
                        }
                    };
                    let display_name = normalized.room_name.clone();
                    let is_discoverable = visibility == RoomVisibility::PublicDiscoverable;
                    // Persist a minimal RoomStore entry so the room and its
                    // ticket survive restarts (needed for periodic re-advertise
                    // and for sharing the room ID/link of unlisted rooms).
                    let _room = RoomStore::with_peers(
                        &self.data_dir,
                        topic,
                        vec![invitation_endpoint_addr(
                            self.endpoint.watch_addr().get(),
                            self.settings_state.share_direct_addresses,
                        )],
                    );
                    // Only discoverable rooms are marked for advertising.
                    if is_discoverable {
                        self.rooms_state.advertised_rooms.insert(topic);
                    }
                    // Create an archived conversation entry so the room name
                    // is available for the periodic advertisement tick and
                    // the room can be unarchived into the CHATS sidebar later.
                    let mut entry = ConversationEntry::new(topic, "", &display_name);
                    entry.archived = true;
                    // BORU-DIR-04/05: the visibility picked in the dialog is
                    // the room's persisted visibility. Only PublicDiscoverable
                    // rooms are allowed to emit directory advertisements.
                    entry.visibility = visibility;
                    entry.description = normalized.short_description.clone();
                    entry.tags = normalized.tags.clone();
                    self.conversation_store.upsert(entry);
                    self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                    // Upsert into the local directory store so the creator
                    // sees their own room in the PUBLIC ROOMS sidebar.
                    if is_discoverable {
                        {
                            let local_pk = self.endpoint.id();
                            let mut store = self.directory_store.lock().unwrap();
                            let ad = RoomAdvertisement {
                                room_name: display_name.clone(),
                                description: normalized.short_description.clone(),
                                topic,
                                ticket: ticket_str.clone(),
                                member_count: 0,
                                last_activity: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as u64,
                                // BORU-DIR-08: TTL so directory clients expire
                                // the ad if refreshes stop.
                                expires_after_secs: ADVERT_TTL_SECS,
                            };
                            store.upsert(ad, local_pk);
                        }
                        self.save_directory_store();
                    }

                    // PUBLIC-02: surface the local creation in the home
                    // screen's Recent Activity feed.
                    self.notifications_state.push_activity(
                        format!("You created public room \"{display_name}\""),
                        ActivityKind::Generic,
                    );

                    // Keep publishing this user-created public room after the
                    // initial directory advertisement.  The DHT record carries
                    // the room metadata so later discovery can present a
                    // directly joinable room. Unlisted rooms are not
                    // discoverable, so they get no DHT record.
                    if is_discoverable {
                        if let Some(dht) = self.dht.clone() {
                            let identity = PublicRoomIdentity::new(
                                topic,
                                public_discovery_key(
                                    Self::public_network(),
                                    &display_name,
                                    boru_core::public_room::PROTOCOL_VERSION,
                                ),
                            );
                            let tracker = PublicRoomTracker::new_with_metadata(
                                Box::new(MainlineDhtBackend::new(dht)),
                                identity,
                                self.endpoint.id(),
                                self.endpoint.secret_key().clone(),
                                Some(display_name.clone()),
                                Some(ticket_str.clone()),
                            );
                            let (new_peers_tx, mut new_peers_rx) =
                                tokio::sync::mpsc::channel::<Vec<iroh::EndpointId>>(64);
                            // Public rooms are not subscribed by the creator here;
                            // drain the discovery channel until the tracker is
                            // shut down rather than allowing it to back up.
                            tokio::spawn(
                                async move { while new_peers_rx.recv().await.is_some() {} },
                            );
                            self.rooms_state.room_trackers.insert(
                                topic,
                                SharedTracker::new_public(PublicContinuousTracker::start(
                                    tracker,
                                    // Adaptive DHT discovery cadence (BORU-DHT-05):
                                    // an isolated room probes fast, a healthy mesh
                                    // settles to a slow 2-5 min cadence.
                                    ContinuousTrackerConfig {
                                        cadence: Some(
                                            boru_core::discovery_cadence::CadencePolicyConfig::default(),
                                        ),
                                        ..Default::default()
                                    },
                                    new_peers_tx,
                                )),
                            );
                        }
                        // Publish the room into the global DHT registry namespace
                        // (relay-independent). While the per-room tracker above
                        // publishes under a name-derived namespace (only
                        // discoverable if the name is already known), the global
                        // registry lets any peer *enumerate* all public rooms by
                        // looking up a single well-known namespace — no shared
                        // relay, no prior room name required. A best-effort
                        // failure is logged, not fatal: the gossip directory and
                        // per-room DHT remain the other discovery surfaces.
                        if let Some(dht) = self.dht.clone() {
                            let backend = MainlineDhtBackend::new(dht);
                            let entry = boru_core::room_registry::RoomRegistryEntry::new(
                                &self.endpoint.id(),
                                *topic.as_bytes(),
                                display_name.clone(),
                                ticket_str.clone(),
                                Some(normalized.short_description.clone()),
                            );
                            let sk = self.endpoint.secret_key().clone();
                            let network = Self::public_network();
                            self.runtime_handle.spawn(async move {
                                match boru_core::room_registry::publish_registry_entry(
                                    &backend, network, &entry, &sk,
                                )
                                .await
                                {
                                    Ok(()) => tracing::debug!("room registry entry published"),
                                    Err(e) => tracing::warn!(
                                        %topic,
                                        error = %e,
                                        "room registry publish failed (non-fatal)"
                                    ),
                                }
                            });
                        }
                    }

                    // Broadcast an immediate advertisement on the directory
                    // topic so other peers see it without waiting for the
                    // ~60 s periodic tick.  If the directory sender is not
                    // yet available the periodic tick will pick it up.
                    // Unlisted rooms are never broadcast.
                    let advert_task = if is_discoverable {
                        if let Some(ref dir_sender) = self.directory_sender {
                            let sk = self.secret_key.clone();
                            let s = dir_sender.clone();
                            let ad_ticket = ticket_str.clone();
                            let ad_description = normalized.short_description.clone();
                            Some(iced::Task::perform(
                                async move {
                                    let ad = RoomAdvertisement {
                                        room_name: display_name,
                                        description: ad_description,
                                        topic,
                                        ticket: ad_ticket,
                                        member_count: 0,
                                        last_activity: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis()
                                            as u64,
                                        // BORU-DIR-08: TTL so directory clients
                                        // expire the ad if refreshes stop.
                                        expires_after_secs: ADVERT_TTL_SECS,
                                    };
                                    let ad_bytes = postcard::to_stdvec(&ad).unwrap_or_default();
                                    let signature = sk.sign(&ad_bytes);
                                    let msg = crate::Message::RoomAdvertisement {
                                        ad,
                                        signature: signature.to_bytes().to_vec(),
                                    };
                                    if let Ok(encoded) = SignedMessage::sign_and_encode(&sk, &msg) {
                                        let _ = s.broadcast(encoded).await;
                                    }
                                },
                                |_| AppMessage::Noop,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    // Complete the pending GUI test action before opening the room.
                    if let Some(action_id) = self.pending_confirm_create_room_action.take() {
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Completed);
                    }
                    // Open the room so the user goes straight into it.
                    let open_task = iced::Task::done(AppMessage::OpenRoom(topic));
                    return if let Some(advert_task) = advert_task {
                        iced::Task::batch(vec![advert_task, open_task])
                    } else {
                        open_task
                    };
                }

                // ── Private room: subscribe and join immediately ────
                // Leave the current room first — abort forward_handle, clear
                // sender + entries — so we don't have a zombie forward_handle
                // or broadcast to the wrong topic during the async gap.
                self.leave_current_room();
                if let Some(action_id) = self.pending_confirm_create_room_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }

                let topic = TopicId::from_bytes(rand::random());
                // Bump the room generation so a stale private-room creation
                // completion cannot clobber a newer room the user opened
                // while subscription was in flight.
                self.room_generation = self.room_generation.wrapping_add(1);
                let room_snapshot = RoomSnapshot {
                    topic,
                    generation: self.room_generation,
                };
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let personal_topic = self.personal_room_topic();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.files_state.download_progress_queue.clone();
                let endpoint = self.endpoint.clone();
                let profile_image_ticket = self.settings_state.profile_image_ticket.clone();
                let dht = self.dht.clone();

                let share_direct_addresses = self.settings_state.share_direct_addresses;
                // Show a loading spinner while the gossip subscription is in flight.
                self.room_loading = true;
                iced::Task::perform(
                    async move {
                        // Subscribe to the new topic
                        let sub = gossip
                            .subscribe(topic, vec![])
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let neighbor_ids: Vec<PublicKey> = receiver.neighbors().collect();
                        let neighbor_count = neighbor_ids.len();
                        let local_peer_addr = invitation_endpoint_addr(
                            endpoint.watch_addr().get(),
                            share_direct_addresses,
                        );

                        // Optionally publish to DHT for private-room discovery.
                        // Clone dht so we can also use it for continuous tracking.
                        let dht_for_publish = dht.clone();
                        let discovery_secret = if dht_enabled {
                            let Some(dht_for_publish) = dht_for_publish else {
                                return Err("DHT unavailable".to_string());
                            };
                            let secret = DiscoverySecret::generate();
                            let backend = MainlineDhtBackend::new(dht_for_publish);
                            let tracker = PrivateRoomTracker::new(
                                Box::new(backend),
                                topic,
                                secret.clone(),
                                endpoint.id(),
                                endpoint.secret_key().clone(),
                            );
                            match tracker.publish_once().await {
                                Ok(()) => Some(secret),
                                Err(error) => {
                                    tracing::warn!(
                                        room = %hex::encode(&topic.as_bytes()[..4]),
                                        operation = "initial_publish",
                                        fallback = "continue_without_dht_discovery_secret",
                                        error = %error,
                                        "DHT degraded; private-room discovery publish unavailable"
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        // Start continuous DHT publish/discover for this room.
                        // Clone the secret for the long-lived tracker; the
                        // original is encoded into the invitation ticket below.
                        let room_tracker = if let (Some(secret), Some(dht)) =
                            (discovery_secret.clone(), dht)
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
                            let _join_task = boru_core::public_room_continuous::spawn_join_fanout(
                                new_peers_rx,
                                sender.clone(),
                                join_cancel.clone(),
                            );
                            Some(SharedTracker::new(
                                PrivateContinuousTracker::start(
                                    tracker,
                                    // Adaptive DHT discovery cadence (BORU-DHT-05).
                                    ContinuousTrackerConfig {
                                        cadence: Some(
                                            boru_core::discovery_cadence::CadencePolicyConfig::default(),
                                        ),
                                        ..Default::default()
                                    },
                                    new_peers_tx,
                                ),
                                join_cancel,
                            ))
                        } else {
                            None
                        };

                        let ticket_str = Ticket {
                            topic,
                            peers: vec![local_peer_addr.clone()],
                            discovery_secret: discovery_secret.clone(),
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
                                name: if room_name.is_empty() {
                                    None
                                } else {
                                    Some(room_name)
                                },
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

                        let mut room =
                            RoomStore::with_peers(&data_dir, topic, vec![local_peer_addr]);
                        // The original secret was encoded into the ticket; the
                        // persisted store keeps its own copy.
                        room.discovery_secret = discovery_secret.clone();

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

            AppMessage::DirectoryRoomWithdrawal(topic, from) => {
                self.directory_store.lock().unwrap().withdraw(topic, from);
                iced::Task::none()
            }

            // update() only dispatches the rooms variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}

impl IcedChat {
    pub(crate) fn is_room_directory_owner(&self, topic: TopicId) -> bool {
        if self.rooms_state.advertised_rooms.contains(&topic) {
            return true;
        }
        self.conversation_store
            .find(&topic)
            .map(|entry| entry.visibility != RoomVisibility::Private)
            .unwrap_or(false)
    }

    /// Apply an owner/admin directory-visibility switch for `topic`
    /// (BORU-DIR-06, PDF Task 2.3).
    ///
    /// * **Switch to `PublicDiscoverable`** — persists the visibility,
    ///   marks the room for periodic re-advertisement, and immediately
    ///   publishes a fresh advertisement (the local directory upsert is done
    ///   by [`immediate_room_advertisement_task`](Self::immediate_room_advertisement_task)
    ///   callers; the periodic tick re-broadcasts on the ~60 s cadence).
    /// * **Switch to `PublicUnlisted`** — persists the visibility, removes
    ///   the room from the advertised set so refreshes stop, and removes the
    ///   local directory entry so the room disappears from the PUBLIC ROOMS
    ///   sidebar immediately. There is **no withdrawal/tombstone message
    ///   yet** (BORU-DIR-09); remote directories drop the advertisement on
    ///   TTL expiry — documented under
    ///   `docs/public-room-directory/visibility-switching.md`.
    /// * **Non-owner** — rejected with a system message and no side effects.
    ///
    /// Returns the immediate-advertisement task when the room is now
    /// discoverable, or `Task::none()` otherwise.
    pub(crate) fn apply_room_directory_visibility(
        &mut self,
        topic: TopicId,
        requested: RoomVisibility,
    ) -> iced::Task<AppMessage> {
        // Permission gate — existing room permission model.
        if !self.is_room_directory_owner(topic) {
            self.push_system("Only the room owner can change directory visibility.");
            return iced::Task::none();
        }
        let current = self
            .conversation_store
            .find(&topic)
            .map(|entry| entry.visibility)
            .unwrap_or(RoomVisibility::Private);
        let outcome = boru_core::control_plane::advertisement::plan_visibility_switch(
            current, requested, true,
        );
        match outcome {
            boru_core::control_plane::advertisement::VisibilitySwitchOutcome::Published => {
                // Persist discoverable visibility.
                let changed = self
                    .conversation_store
                    .find_mut(&topic)
                    .map(|entry| {
                        if entry.visibility != RoomVisibility::PublicDiscoverable {
                            entry.visibility = RoomVisibility::PublicDiscoverable;
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if changed {
                    if let Some(ref st) = self.storage {
                        let _ = self.conversation_store.save_to_sqlite(st);
                    }
                }
                // Mark for periodic refresh + surface in Recent Activity.
                self.rooms_state.advertised_rooms.insert(topic);
                let name = self
                    .conversation_store
                    .find(&topic)
                    .map(|e| {
                        if e.name.is_empty() {
                            topic.to_string()
                        } else {
                            e.name.clone()
                        }
                    })
                    .unwrap_or_else(|| topic.to_string());
                self.notifications_state.push_activity(
                    format!("You announced public room \"{name}\""),
                    ActivityKind::Generic,
                );
                // Upsert into the local directory store so the creator sees
                // their own room in the PUBLIC ROOMS sidebar immediately
                // (the gossip mesh does not echo our own broadcasts back).
                {
                    let local_pk = self.local_public;
                    let ticket = self.room_ticket(topic, &[]).to_string();
                    let member_count = self
                        .room_neighbor_counts
                        .get(&topic)
                        .copied()
                        .unwrap_or_default();
                    let description = self
                        .conversation_store
                        .find(&topic)
                        .map(|e| e.description.clone())
                        .unwrap_or_default();
                    if let Ok(mut store) = self.directory_store.lock() {
                        let ad = RoomAdvertisement {
                            room_name: name.clone(),
                            description,
                            topic,
                            ticket,
                            member_count,
                            last_activity: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                            // BORU-DIR-08: advertisements carry their TTL so
                            // directory clients can expire them without a
                            // refresh.
                            expires_after_secs: ADVERT_TTL_SECS,
                        };
                        store.upsert(ad, local_pk);
                    }
                    self.save_directory_store();
                }
                self.refresh_sidebar_counts();
                // Immediately publish a fresh advertisement.
                if let Some(task) = self.immediate_room_advertisement_task(topic) {
                    task
                } else if self.directory_sender.is_none() {
                    // No directory sender yet — subscribe so the periodic
                    // tick can broadcast the fresh advertisement.
                    iced::Task::done(AppMessage::SubscribeDirectoryTopic)
                } else {
                    iced::Task::none()
                }
            }
            boru_core::control_plane::advertisement::VisibilitySwitchOutcome::Unlisted => {
                // Persist unlisted visibility.
                let changed = self
                    .conversation_store
                    .find_mut(&topic)
                    .map(|entry| {
                        if entry.visibility != RoomVisibility::PublicUnlisted {
                            entry.visibility = RoomVisibility::PublicUnlisted;
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if changed {
                    if let Some(ref st) = self.storage {
                        let _ = self.conversation_store.save_to_sqlite(st);
                    }
                }
                // Stop refreshing: remove from the advertised set and stop the
                // DHT tracker (if any) so no advertisement is re-published.
                self.rooms_state.advertised_rooms.remove(&topic);
                if let Some(tracker) = self.rooms_state.room_trackers.remove(&topic) {
                    tracker.shutdown_shared();
                }
                // Remove the local directory entry so the room disappears
                // from the PUBLIC ROOMS sidebar immediately.
                let local_author = self.local_public;
                let _ = self
                    .directory_store
                    .lock()
                    .map(|mut store| store.remove(topic, local_author));
                if let Some(storage) = self.storage.as_ref() {
                    if let Err(err) = storage.with_conn(|conn| {
                        conn.execute(
                            "DELETE FROM directory_ads WHERE topic = ?1 AND author = ?2",
                            rusqlite::params![topic.as_bytes(), local_author.as_bytes()],
                        )
                        .map_err(n0_error::AnyError::from_std)?;
                        Ok(())
                    }) {
                        warn!("failed to delete directory advertisement: {err}");
                    }
                }
                // BORU-DIR-09 (PDF Task 3.3): emit a withdrawal so remote
                // directories remove the advertisement immediately instead
                // of waiting for TTL expiry. TTL remains the safety net if
                // the withdrawal is missed.
                self.broadcast_room_withdrawal(topic);
                self.refresh_sidebar_counts();
                self.notifications_state.push_activity(
                    "You unlisted a public room — a withdrawal was broadcast so other directories remove it immediately.",
                    ActivityKind::Generic,
                );
                iced::Task::none()
            }
            boru_core::control_plane::advertisement::VisibilitySwitchOutcome::NoChange => {
                iced::Task::none()
            }
            boru_core::control_plane::advertisement::VisibilitySwitchOutcome::Forbidden => {
                self.push_system(
                    "Only Public-Unlisted and Public-Discoverable rooms can be switched.",
                );
                iced::Task::none()
            }
        }
    }

    /// Feed the real local relationship facts into the bounded
    /// room-directory cache (BORU-DIR-12, PDF Task 4.3).
    ///
    /// The source of truth for `Joined` is the **local room database** —
    /// the persisted conversation store — never the advertisement. The
    /// persisted hide preference (BORU-DIR-20 controls write through
    /// [`Storage::set_room_hidden`]; the preference itself survives
    /// restarts) supplies `hidden`. The directory derives each cached
    /// room's `local_join_state` from these facts and never shows a
    /// joined room as Join (it shows Open) and never re-shows a hidden
    /// room.
    ///
    /// Called on every `ConnMonitorTick` (1 Hz) so new joins / hides are
    /// reflected promptly; the cache itself stores the facts so entries
    /// added between ticks are derived immediately.
    pub(crate) fn sync_directory_local_states(&mut self) {
        let Some(directory) = self.room_directory.clone() else {
            return;
        };
        // Joined = every topic with a local conversation record (the
        // real local room database). Direct chats have derived topics
        // that never collide with advertised room ids, so including all
        // topics is both simple and correct.
        let joined: std::collections::BTreeSet<TopicId> = self
            .conversation_store
            .iter()
            .map(|entry| entry.topic)
            .collect();
        // Hidden = persisted hide preference.
        let hidden: std::collections::BTreeSet<TopicId> = match self.storage.as_ref() {
            Some(storage) => storage
                .room_hidden_ids()
                .unwrap_or_default()
                .into_iter()
                .map(TopicId::from_bytes)
                .collect(),
            None => std::collections::BTreeSet::new(),
        };
        let facts = boru_core::room_directory::LocalRoomFacts {
            joined,
            pending: std::collections::BTreeSet::new(),
            hidden,
        };
        let mut guard = directory.lock().unwrap();
        guard.sync_local_states(facts);
    }

    /// Broadcast a signed room withdrawal for `topic` over the directory
    /// gossip topic (BORU-DIR-09, PDF Task 3.3). Fire-and-forget: remote
    /// directories remove the matching advertisement when the withdrawal
    /// verifies; TTL expiry remains the safety net if it is missed.
    pub(crate) fn broadcast_room_withdrawal(&self, topic: TopicId) {
        let Some(dir_sender) = self.directory_sender.clone() else {
            debug!(%topic, "room withdrawal: no directory sender yet; remote directories rely on TTL expiry");
            return;
        };
        let sk = self.secret_key.clone();
        self.runtime_handle.spawn(async move {
            let msg = crate::Message::RoomWithdrawal {
                topic,
                signature: boru_core::chat_core::sign_room_withdrawal(&topic, &sk),
            };
            match SignedMessage::sign_and_encode(&sk, &msg) {
                Ok(encoded) => {
                    if let Err(e) = dir_sender.broadcast(encoded).await {
                        debug!(%topic, error = %e, "room withdrawal broadcast failed (TTL remains the safety net)");
                    }
                }
                Err(e) => {
                    debug!(%topic, error = %e, "room withdrawal signing failed (TTL remains the safety net)");
                }
            }
        });
    }

    /// Build the immediate-broadcast task for a discoverable room's fresh
    /// advertisement (BORU-DIR-06 / DIR-05 immediate-publish path).
    ///
    /// Returns `None` when the room is not discoverable or the directory
    /// sender is unavailable (the periodic tick will pick the room up).
    fn immediate_room_advertisement_task(&self, topic: TopicId) -> Option<iced::Task<AppMessage>> {
        let entry = self.conversation_store.find(&topic)?;
        if !entry.visibility.is_discoverable() {
            return None;
        }
        let room_name = if entry.name.is_empty() {
            topic.to_string()
        } else {
            entry.name.clone()
        };
        let description = entry.description.clone();
        let dir_sender = self.directory_sender.clone()?;
        let sk = self.secret_key.clone();
        let neighbor_count = self
            .room_neighbor_counts
            .get(&topic)
            .copied()
            .unwrap_or_default();
        let ticket = self.room_ticket(topic, &[]).to_string();
        Some(iced::Task::perform(
            async move {
                let ad = boru_core::chat_core::RoomAdvertisement {
                    room_name,
                    description,
                    topic,
                    ticket,
                    member_count: neighbor_count,
                    last_activity: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                    // BORU-DIR-08: TTL so directory clients expire the ad
                    // if refreshes stop.
                    expires_after_secs: ADVERT_TTL_SECS,
                };
                let ad_bytes = postcard::to_stdvec(&ad).unwrap_or_default();
                let signature = sk.sign(&ad_bytes);
                let msg = crate::Message::RoomAdvertisement {
                    ad,
                    signature: signature.to_bytes().to_vec(),
                };
                match SignedMessage::sign_and_encode(&sk, &msg) {
                    Ok(encoded) => dir_sender.broadcast(encoded).await.is_ok(),
                    Err(_) => false,
                }
            },
            |ok| {
                if ok {
                    tracing::debug!("immediate room advertisement broadcast");
                } else {
                    tracing::warn!("immediate room advertisement broadcast failed");
                }
                AppMessage::Noop
            },
        ))
    }

    /// BORU-DIR-07 (PDF Task 3.1): publish locally hosted/owned
    /// discoverable rooms after the discovery service is ready.
    ///
    /// * Enumerates `conversation_store` for rooms whose persisted
    ///   visibility is [`RoomVisibility::PublicDiscoverable`] and that the
    ///   local user owns ([`Self::is_room_directory_owner`]).
    /// * Marks each for periodic refresh ([`Self::advertised_rooms`]) so
    ///   the periodic tick keeps them alive after the initial burst.
    /// * Broadcasts one bounded advertisement per eligible room (the same
    ///   legacy `RoomAdvertisement` gossip path DIR-06 uses), staggered
    ///   with random jitter so many rooms do not burst at the same instant.
    /// * Never blocks Boru startup: publish failures are logged, not fatal;
    ///   if the directory sender is unavailable the rooms are still marked
    ///   for the periodic tick.
    /// * Dedupes unchanged advertisements: a room whose metadata
    ///   fingerprint is unchanged since the last broadcast within
    ///   [`ADVERT_DEDUPE_WINDOW`] is not re-published more often than
    ///   necessary.
    pub(crate) fn publish_startup_room_advertisements(&mut self) -> iced::Task<AppMessage> {
        if self.rooms_state.startup_advertise_swept {
            return iced::Task::none();
        }
        self.rooms_state.startup_advertise_swept = true;

        // MCP-created legacy advertisements were historically persisted only
        // in DirectoryStore, without a ConversationEntry. Materialize local
        // authored entries before the startup sweep so they survive a restart
        // and continue through the normal periodic refresh path.
        let local_legacy_ads: Vec<RoomAdvertisement> = self
            .directory_store
            .lock()
            .unwrap()
            .list_active()
            .into_iter()
            .filter(|(_, author)| *author == self.local_public)
            .map(|(ad, _)| ad)
            .collect();
        let mut recovered = false;
        for ad in local_legacy_ads {
            if self.conversation_store.find(&ad.topic).is_none() {
                let mut entry = ConversationEntry::new(ad.topic, "", ad.room_name);
                entry.archived = true;
                entry.visibility = RoomVisibility::PublicDiscoverable;
                entry.description = ad.description;
                self.conversation_store.upsert(entry);
                recovered = true;
            }
        }
        if recovered {
            if let Some(storage) = self.storage.as_ref() {
                if let Err(error) = self.conversation_store.save_to_sqlite(storage) {
                    warn!(%error, "failed to persist recovered public room entries");
                }
            }
        }

        // Enumerate locally authorized PublicDiscoverable rooms.
        let eligible: Vec<TopicId> = self
            .conversation_store
            .iter()
            .filter(|e| e.visibility.is_discoverable() && self.is_room_directory_owner(e.topic))
            .map(|e| e.topic)
            .collect();
        if eligible.is_empty() {
            return iced::Task::none();
        }
        // Mark for periodic refresh so they stay advertised after the
        // initial burst (this is what makes them reappear after restart).
        for topic in &eligible {
            self.rooms_state.advertised_rooms.insert(*topic);
        }

        let Some(dir_sender) = self.directory_sender.clone() else {
            warn!(
                rooms = eligible.len(),
                "startup advertisement: no directory sender yet; periodic tick will publish"
            );
            return iced::Task::none();
        };

        let sk = self.secret_key.clone();
        let s = dir_sender;
        // Build one bounded advertisement per eligible room. Skip rooms
        // whose metadata is unchanged since the last broadcast within the
        // dedupe window (PDF Task 3.1 step 5).
        let rooms: Vec<(TopicId, String, String, String, u32)> = eligible
            .into_iter()
            .filter_map(|topic| {
                let entry = self.conversation_store.find(&topic)?;
                let name = if entry.name.is_empty() {
                    topic.to_string()
                } else {
                    entry.name.clone()
                };
                let description = entry.description.clone();
                let ticket = self.room_ticket(topic, &[]).to_string();
                if !self.should_broadcast_advertisement(topic, &name, &description, &ticket) {
                    debug!(%topic, "startup advertisement: unchanged metadata within dedupe window, skipping");
                    return None;
                }
                // Record the fingerprint *before* broadcasting so the
                // periodic tick (which shares the same dedupe state) does
                // not immediately re-broadcast identical metadata.
                self.record_advertisement_broadcast(topic, &name, &description, &ticket);
                let member_count = self
                    .room_neighbor_counts
                    .get(&topic)
                    .copied()
                    .unwrap_or_default();
                Some((topic, name, description, ticket, member_count))
            })
            .collect();
        if rooms.is_empty() {
            return iced::Task::none();
        }

        iced::Task::perform(
            async move {
                let mut results = Vec::new();
                for (topic, room_name, description, ticket_str, member_count) in rooms {
                    // Jitter: random per-room delay so many rooms do not
                    // burst at the same instant (PDF Task 3.1 step 4).
                    let jitter_ms = (rand::random::<u64>() % STARTUP_ADVERT_JITTER_MAX_MS) as u64;
                    if jitter_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                    }
                    let ad = boru_core::chat_core::RoomAdvertisement {
                        room_name,
                        description,
                        topic,
                        ticket: ticket_str,
                        member_count,
                        last_activity: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                        // BORU-DIR-08: TTL so directory clients expire the ad
                        // if refreshes stop.
                        expires_after_secs: ADVERT_TTL_SECS,
                    };
                    let ad_bytes = postcard::to_stdvec(&ad).unwrap_or_default();
                    let signature = sk.sign(&ad_bytes);
                    let msg = crate::Message::RoomAdvertisement {
                        ad,
                        signature: signature.to_bytes().to_vec(),
                    };
                    let delivered = match SignedMessage::sign_and_encode(&sk, &msg) {
                        Ok(encoded) => s.broadcast(encoded).await.is_ok(),
                        Err(_) => false,
                    };
                    results.push((topic, delivered));
                }
                results
            },
            |results| {
                for (topic, ok) in &results {
                    if *ok {
                        tracing::debug!(%topic, "startup room advertisement broadcast");
                    } else {
                        tracing::warn!(%topic, "startup room advertisement broadcast failed (non-fatal)");
                    }
                }
                AppMessage::Noop
            },
        )
    }

    /// Whether a room's advertisement should be broadcast right now
    /// (BORU-DIR-07 dedupe, PDF Task 3.1 step 5).
    ///
    /// Returns `false` when the room's current metadata fingerprint is
    /// identical to the last broadcast and that broadcast happened within
    /// [`ADVERT_DEDUPE_WINDOW`] — re-publishing the exact same metadata
    /// more often than necessary would only churn the directory mesh.
    pub(crate) fn should_broadcast_advertisement(
        &self,
        topic: TopicId,
        room_name: &str,
        description: &str,
        ticket: &str,
    ) -> bool {
        let fingerprint = startup_advertisement_fingerprint(topic, room_name, description, ticket);
        match self.rooms_state.last_advertised_fingerprint.get(&topic) {
            Some(last) if *last == fingerprint => self
                .rooms_state
                .last_advertised_at
                .get(&topic)
                .map(|at| at.elapsed() >= ADVERT_DEDUPE_WINDOW)
                .unwrap_or(true),
            _ => true,
        }
    }

    /// Record that a room's advertisement was broadcast just now, for the
    /// BORU-DIR-07 dedupe window.
    pub(crate) fn record_advertisement_broadcast(
        &mut self,
        topic: TopicId,
        room_name: &str,
        description: &str,
        ticket: &str,
    ) {
        let fingerprint = startup_advertisement_fingerprint(topic, room_name, description, ticket);
        self.rooms_state
            .last_advertised_fingerprint
            .insert(topic, fingerprint);
        self.rooms_state
            .last_advertised_at
            .insert(topic, Instant::now());
    }

    /// Immediately broadcast a fresh advertisement for every room currently
    /// marked for directory advertising (BORU-DIR-07 catch-up).
    ///
    /// Called when the directory gossip sender becomes available, so a room
    /// that was created (or switched to discoverable) *before* the directory
    /// topic subscription completed — or after a reconnect — is published
    /// right away instead of waiting for the next ~60 s periodic tick. The
    /// dedupe fingerprint shared with the periodic tick suppresses a
    /// redundant re-broadcast of unchanged metadata (so when the sender
    /// arrived before the create, this is a no-op).
    pub(crate) fn publish_all_advertised_now(&self) -> iced::Task<AppMessage> {
        if self.directory_sender.is_none() || self.rooms_state.advertised_rooms.is_empty() {
            return iced::Task::none();
        }
        let tasks: Vec<iced::Task<AppMessage>> = self
            .rooms_state
            .advertised_rooms
            .iter()
            .filter_map(|topic| self.immediate_room_advertisement_task(*topic))
            .collect();
        if tasks.is_empty() {
            iced::Task::none()
        } else {
            iced::Task::batch(tasks)
        }
    }

    /// The configured public-room network this app participates in.
    ///
    /// Central single source of truth for the public-room network across all
    /// discovery surfaces (gossip directory topic, per-room DHT namespaces,
    /// and the global room registry). Currently the app is Mainnet-only, so
    /// this returns [`PublicNetwork::Mainnet`]; changing it here re-targets
    /// every discovery channel consistently.
    pub(crate) fn public_network() -> PublicNetwork {
        PublicNetwork::Mainnet
    }

    /// Periodic global-registry DHT lookup (~120 s), called from the shell's
    /// `ConnMonitorTick` arm (decrements `registry_lookup_counter`).
    ///
    /// Enlists the relay-independent global registry namespace, then merges
    /// every discovered room into the local `directory_store` so it surfaces
    /// in PUBLIC ROOMS — filling the relay-scope gap that the gossip-directory
    /// topic cannot bridge. Best-effort: a DHT failure or drop is logged, not
    /// fatal, and the periodic tick simply tries again next interval.
    pub(crate) fn periodic_registry_lookup(&mut self) -> Option<iced::Task<AppMessage>> {
        if self.rooms_state.registry_lookup_counter > 0 {
            self.rooms_state.registry_lookup_counter -= 1;
            return None;
        }
        self.rooms_state.registry_lookup_counter = REGISTRY_LOOKUP_INTERVAL_SECS;
        Some(self.registry_lookup_task())
    }

    /// Immediate global-registry DHT lookup for the manual PUBLIC ROOMS
    /// refresh action. Resets the periodic cadence counter so the next
    /// periodic tick does not immediately fire a second lookup.
    pub(crate) fn refresh_room_registry_now(&mut self) -> iced::Task<AppMessage> {
        self.rooms_state.registry_lookup_counter = REGISTRY_LOOKUP_INTERVAL_SECS;
        self.registry_lookup_task()
    }

    /// Shared worker: enlists the global registry namespace on the configured
    /// network and merges discovered rooms into `directory_store`. Used by
    /// both the periodic (~120 s) tick and the manual refresh button.
    fn registry_lookup_task(&self) -> iced::Task<AppMessage> {
        let Some(dht) = self.dht.clone() else {
            return iced::Task::none();
        };
        let store = self.directory_store.clone();
        let endpoint = self.endpoint.clone();
        let local_pk = endpoint.id();
        let network = Self::public_network();
        iced::Task::perform(
            async move {
                let backend = MainlineDhtBackend::new(dht);
                match boru_core::room_registry::lookup_registry(&backend, network).await {
                    Ok(entries) => entries,
                    Err(e) => {
                        tracing::warn!(error = %e, "room registry lookup failed (non-fatal)");
                        Vec::new()
                    }
                }
            },
            move |entries| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                for entry in entries {
                    // Skip rooms already known (gossip-directory path or our
                    // own). DirectoryStore dedups, so this is redundant safety.
                    let mut guard = store.lock().unwrap();
                    let ad = RoomAdvertisement {
                        room_name: entry.room_name().to_owned(),
                        description: entry.description().unwrap_or("").to_owned(),
                        topic: TopicId::from_bytes(entry.room_topic()),
                        ticket: entry.ticket().to_owned(),
                        member_count: 0,
                        last_activity: now,
                        expires_after_secs: ADVERT_TTL_SECS,
                    };
                    guard.upsert(
                        ad,
                        iroh::PublicKey::from_bytes(&entry.owner()).unwrap_or(local_pk),
                    );
                    drop(guard);
                }
                AppMessage::Noop
            },
        )
    }
}

impl IcedChat {
    /// Periodic room-advertisement broadcast (~60 s cadence), called from the
    /// shell's `ConnMonitorTick` arm.
    ///
    /// For each room the user has enabled for directory advertising, sign and
    /// broadcast a `RoomAdvertisement` into the directory topic. Returns
    /// `Some(task)` when a broadcast fired (or `None` when the counter was
    /// only decremented).
    pub(crate) fn periodic_room_advertisement(&mut self) -> Option<iced::Task<AppMessage>> {
        if self.rooms_state.advertise_counter == 0 {
            // BORU-DIR-08 (PDF Task 3.2 step 3): jitter the periodic refresh
            // cadence so advertisers that started around the same time drift
            // out of phase instead of re-broadcasting in synchronized bursts.
            self.rooms_state.advertise_counter = ADVERT_REFRESH_INTERVAL_SECS as u32
                + rand::random::<u64>() as u32 % (ADVERT_REFRESH_JITTER_SECS as u32 + 1);
            let dir_sender = self.directory_sender.clone()?;
            if self.rooms_state.advertised_rooms.is_empty() {
                return None;
            }
            let advertised: Vec<TopicId> =
                self.rooms_state.advertised_rooms.iter().copied().collect();
            let sk = self.secret_key.clone();
            let s = dir_sender;
            // Collect room details for all advertised rooms, using the
            // conversation/room-history store to get names, and the endpoint
            // for ticket generation.
            let room_info: Vec<(TopicId, String, String, String, u32)> = advertised
                .into_iter()
                .filter_map(|topic| {
                    // BORU-DIR-04 (PDF 2.1): only PublicDiscoverable rooms are
                    // advertised. Rooms whose persisted visibility is Private
                    // or PublicUnlisted must not emit directory advertisements.
                    let entry = self.conversation_store.find(&topic)?;
                    if entry.visibility != RoomVisibility::PublicDiscoverable {
                        return None;
                    }
                    let name = if entry.name.is_empty() {
                        topic.to_string()
                    } else {
                        entry.name.clone()
                    };
                    // BORU-DIR-07 (PDF 3.1 step 5): dedupe unchanged
                    // advertisements within the dedupe window.
                    let ticket = self.room_ticket(topic, &[]).to_string();
                    let description = entry.description.clone();
                    if !self.should_broadcast_advertisement(topic, &name, &description, &ticket) {
                        trace!(%topic, "periodic advertisement: unchanged metadata within dedupe window, skipping");
                        return None;
                    }
                    self.record_advertisement_broadcast(topic, &name, &description, &ticket);
                    let neighbor_count = self
                        .room_neighbor_counts
                        .get(&topic)
                        .copied()
                        .unwrap_or_default();
                    Some((topic, name, description, ticket, neighbor_count))
                })
                .collect();
            let room_info_for_upsert = room_info.clone();
            let task = iced::Task::perform(
                async move {
                    let mut results = Vec::new();
                    for (topic, room_name, description, ticket_str, member_count) in room_info {
                        // BORU-DIR-08 (PDF Task 3.2 step 3): small per-room
                        // jitter inside the periodic refresh burst so multiple
                        // rooms do not re-broadcast at the same instant.
                        let jitter_ms =
                            (rand::random::<u64>() % STARTUP_ADVERT_JITTER_MAX_MS) as u64;
                        if jitter_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                        }
                        let ad = boru_core::chat_core::RoomAdvertisement {
                            room_name,
                            description,
                            topic,
                            ticket: ticket_str,
                            member_count,
                            last_activity: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                            // BORU-DIR-08: TTL so directory clients expire the
                            // ad if refreshes stop.
                            expires_after_secs: ADVERT_TTL_SECS,
                        };
                        let ad_bytes = postcard::to_stdvec(&ad).unwrap_or_default();
                        let signature = sk.sign(&ad_bytes);
                        let msg = crate::Message::RoomAdvertisement {
                            ad,
                            signature: signature.to_bytes().to_vec(),
                        };
                        if let Ok(encoded) = SignedMessage::sign_and_encode(&sk, &msg) {
                            let delivered = s.broadcast(encoded).await.is_ok();
                            results.push((topic, delivered));
                        } else {
                            results.push((topic, false));
                        }
                    }
                    results
                },
                |results| {
                    for (topic, ok) in &results {
                        if *ok {
                            tracing::debug!(%topic, "room advertisement broadcast");
                        } else {
                            tracing::warn!(%topic, "room advertisement broadcast failed");
                        }
                    }
                    AppMessage::Noop
                },
            );
            // Also upsert local rooms into directory_store so the creator sees
            // their own advertised rooms in the PUBLIC ROOMS sidebar (the
            // gossip mesh does not echo our own broadcasts back to us).
            let local_pk = self.endpoint.id();
            let mut store = self.directory_store.lock().unwrap();
            for (topic, room_name, description, ticket_str, member_count) in room_info_for_upsert {
                let ad = RoomAdvertisement {
                    room_name,
                    description,
                    topic,
                    ticket: ticket_str,
                    member_count,
                    last_activity: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                    expires_after_secs: ADVERT_TTL_SECS,
                };
                store.upsert(ad, local_pk);
            }
            drop(store);
            Some(task)
        } else {
            self.rooms_state.advertise_counter -= 1;
            None
        }
    }
}

impl IcedChat {
    pub(crate) fn view_create_room_dialog<'a>(
        &self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{
            checkbox_field, helper_text, radio_field, FormSection, TextInput,
        };
        use boru_core::control_plane::advertisement::{
            RoomVisibility, DEFAULT_MAX_DESCRIPTION_LEN, DEFAULT_MAX_ROOM_NAME_LEN,
            DEFAULT_MAX_TAGS, DEFAULT_MAX_TAG_LEN,
        };

        let theme = Self::theme_from_dark(self.dark_mode);

        // ── Room Details ────────────────────────────────────────────────
        let name_valid = !self.rooms_state.create_room_name.trim().is_empty();
        let submitting = self.rooms_state.create_room_submitting;
        let mut name_field = TextInput::new(
            "Room Name",
            "Room name…",
            &self.rooms_state.create_room_name,
            AppMessage::CreateNewRoomNameChanged,
        )
        .id(CREATE_ROOM_NAME_INPUT)
        .helper(format!(
            "A short name others will see in the directory (max {DEFAULT_MAX_ROOM_NAME_LEN} characters)."
        ));
        if let Some(error) = &self.rooms_state.create_room_error {
            name_field = name_field.error(error.clone());
        }
        // Enter submits only when the form is valid and not mid-submit.
        if name_valid && !submitting {
            name_field = name_field.on_submit(AppMessage::ConfirmCreateNewRoom);
        }
        let description_field = TextInput::new(
            "Description",
            "Optional — what is this room about?",
            &self.rooms_state.create_room_description,
            AppMessage::CreateNewRoomDescriptionChanged,
        )
        .helper(format!(
            "Optional short description shown in the directory (max {DEFAULT_MAX_DESCRIPTION_LEN} characters)."
        ));
        let tags_field = TextInput::new(
            "Tags",
            "Optional — e.g. rust, gaming",
            &self.rooms_state.create_room_tags,
            AppMessage::CreateNewRoomTagsChanged,
        )
        .helper(format!(
            "Optional comma-separated tags used to find the room (up to {DEFAULT_MAX_TAGS} tags, {DEFAULT_MAX_TAG_LEN} characters each)."
        ));
        let room_details = FormSection::new(crate::i18n::t("dialogs.create_room.room_details"))
            .push(name_field.build())
            .push(description_field.build())
            .push(tags_field.build());

        // ── Visibility / Discovery ──────────────────────────────────────
        let visibility = FormSection::new(crate::i18n::t("dialogs.create_room.visibility"))
            .helper(crate::i18n::t("dialogs.create_room.visibility_helper"))
            .push(radio_field(
                "Private",
                RoomVisibility::Private,
                Some(self.rooms_state.create_room_visibility),
                AppMessage::CreateNewRoomVisibilityChanged,
                Some("Invite-only. No directory listing; join by invitation or authorization."),
            ))
            .push(radio_field(
                "Public — Unlisted",
                RoomVisibility::PublicUnlisted,
                Some(self.rooms_state.create_room_visibility),
                AppMessage::CreateNewRoomVisibilityChanged,
                Some("Not listed in the directory. Others can join with the room ID, invite, or link."),
            ))
            .push(radio_field(
                "Public — Discoverable",
                RoomVisibility::PublicDiscoverable,
                Some(self.rooms_state.create_room_visibility),
                AppMessage::CreateNewRoomVisibilityChanged,
                Some("Listed in the directory. Other Boru users can find and join it."),
            ))
            .push(checkbox_field(
                crate::i18n::t("dialogs.create_room.dht_discovery"),
                self.rooms_state.create_room_dht_enabled,
                AppMessage::CreateNewRoomDhtToggled,
                Some(crate::i18n::t("dialogs.create_room.dht_discovery_helper")),
            ));

        // ── Access / Participation Options ──────────────────────────────
        // Public rooms are open by design; the backend exposes no join
        // limits, invite gates, or access rules, so this section is helper
        // text only.
        let access = FormSection::new(crate::i18n::t("dialogs.create_room.access")).push(
            helper_text(&crate::i18n::t("dialogs.create_room.access_helper")),
        );

        // ── Preview / Info ──────────────────────────────────────────────
        let info = FormSection::new(crate::i18n::t("dialogs.create_room.preview")).push(
            helper_text(&crate::i18n::t("dialogs.create_room.preview_helper")),
        );

        let overlay = BoruDialog::new(crate::i18n::t("dialogs.create_room.dialog_title"))
            .subtitle(crate::i18n::t("dialogs.create_room.dialog_subtitle"))
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(room_details.build())
            .push_body(visibility.build())
            .push_body(access.build())
            .push_body(info.build())
            .secondary("Cancel", AppMessage::CancelCreateRoom)
            .secondary_enabled(!submitting)
            .primary(
                if submitting {
                    "Creating…"
                } else {
                    "Create Room"
                },
                AppMessage::ConfirmCreateNewRoom,
            )
            .primary_enabled(name_valid && !submitting)
            .on_backdrop(AppMessage::CancelCreateRoom)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    pub(crate) fn view_room_settings_dialog<'a>(
        &self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{helper_text, radio_field, FormSection, TextInput};
        use boru_core::control_plane::advertisement::{
            RoomVisibility, DEFAULT_MAX_DESCRIPTION_LEN, DEFAULT_MAX_ROOM_NAME_LEN,
            DEFAULT_MAX_TAGS, DEFAULT_MAX_TAG_LEN,
        };

        let theme = Self::theme_from_dark(self.dark_mode);

        // ── Room Details ────────────────────────────────────────────────
        let name_valid = !self.rooms_state.room_settings_name.trim().is_empty();
        let mut name_field = TextInput::new(
            "Room Name",
            "Room name…",
            &self.rooms_state.room_settings_name,
            AppMessage::RoomSettingsNameChanged,
        )
        .helper(format!(
            "A short name others will see in the directory (max {DEFAULT_MAX_ROOM_NAME_LEN} characters)."
        ));
        if let Some(error) = &self.rooms_state.room_settings_error {
            name_field = name_field.error(error.clone());
        }
        if name_valid {
            name_field = name_field.on_submit(AppMessage::ConfirmRoomSettings);
        }
        let description_field = TextInput::new(
            "Description",
            "Optional — what is this room about?",
            &self.rooms_state.room_settings_description,
            AppMessage::RoomSettingsDescriptionChanged,
        )
        .helper(format!(
            "Optional short description shown in the directory (max {DEFAULT_MAX_DESCRIPTION_LEN} characters)."
        ));
        let tags_field = TextInput::new(
            "Tags",
            "Optional — e.g. rust, gaming",
            &self.rooms_state.room_settings_tags,
            AppMessage::RoomSettingsTagsChanged,
        )
        .helper(format!(
            "Optional comma-separated tags used to find the room (up to {DEFAULT_MAX_TAGS} tags, {DEFAULT_MAX_TAG_LEN} characters each)."
        ));
        let room_details = FormSection::new("Room Details")
            .push(name_field.build())
            .push(description_field.build())
            .push(tags_field.build());

        // ── Visibility / Discovery ──────────────────────────────────────
        // Only the two public states are offered: the directory controls
        // switch PublicDiscoverable <-> PublicUnlisted; Private rooms are
        // created private and are never part of the directory.
        let visibility = FormSection::new("Directory Visibility")
            .helper(
                "Choose how other Boru users can find this room. Discoverable rooms are advertised; unlisted rooms require a room ID/invite/link.",
            )
            .push(radio_field(
                "Public — Unlisted",
                RoomVisibility::PublicUnlisted,
                Some(self.rooms_state.room_settings_visibility),
                AppMessage::RoomSettingsVisibilityChanged,
                Some("Not listed in the directory. Others can join with the room ID, invite, or link."),
            ))
            .push(radio_field(
                "Public — Discoverable",
                RoomVisibility::PublicDiscoverable,
                Some(self.rooms_state.room_settings_visibility),
                AppMessage::RoomSettingsVisibilityChanged,
                Some("Listed in the directory. Other Boru users can find and join it."),
            ))
            .push(helper_text(
                "Switching to Discoverable publishes a fresh advertisement immediately. Switching to Unlisted stops refreshing; remote directories drop the room after the advertisement TTL (no withdrawal message yet).",
            ));

        let overlay = BoruDialog::new("Room Settings")
            .subtitle("Change how this room appears in the directory.")
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(room_details.build())
            .push_body(visibility.build())
            .secondary("Cancel", AppMessage::CancelRoomSettings)
            .primary("Save", AppMessage::ConfirmRoomSettings)
            .primary_enabled(name_valid)
            .on_backdrop(AppMessage::CancelRoomSettings)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_state_new_defaults_match_create_dialog() {
        let state = RoomsState::new();
        // BORU-DIR-05 (PDF Task 2.2): conservative default — new public room
        // is unlisted unless the creator explicitly opts into discoverability.
        assert_eq!(state.create_room_visibility, RoomVisibility::PublicUnlisted);
        assert!(state.create_room_dht_enabled, "DHT discovery defaults on");
        assert!(!state.show_create_room_dialog);
        assert!(!state.create_room_submitting);
        assert!(state.create_room_error.is_none());
        assert!(!state.show_room_settings_dialog);
        assert!(state.room_settings_topic.is_none());
        assert_eq!(
            state.room_settings_visibility,
            RoomVisibility::PublicUnlisted
        );
        assert!(state.advertised_rooms.is_empty());
        assert_eq!(state.advertise_counter, 60);
        assert!(state.last_advertised_fingerprint.is_empty());
        assert!(!state.startup_advertise_swept);
        assert!(state.auto_subscribed_rooms.is_empty());
        assert!(state.room_trackers.is_empty());
    }

    #[test]
    fn create_room_field_changes_route_through_domain_update() {
        let mut state = RoomsState::new();
        state.update(RoomsMessage::CreateNewRoomDhtToggled(false));
        assert!(!state.create_room_dht_enabled, "DHT toggle accepted");
        state.update(RoomsMessage::CreateNewRoomNameChanged("Lobby".to_string()));
        assert_eq!(state.create_room_name, "Lobby");
        state.update(RoomsMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        assert_eq!(
            state.create_room_visibility,
            RoomVisibility::PublicDiscoverable
        );
        state.update(RoomsMessage::CreateNewRoomDescriptionChanged(
            "desc".to_string(),
        ));
        assert_eq!(state.create_room_description, "desc");
        state.update(RoomsMessage::CreateNewRoomTagsChanged("a,b".to_string()));
        assert_eq!(state.create_room_tags, "a,b");
    }

    #[test]
    fn room_settings_field_changes_clear_error() {
        let mut state = RoomsState::new();
        state.update(RoomsMessage::RoomSettingsNameChanged("x".to_string()));
        assert_eq!(state.room_settings_name, "x");
        state.room_settings_error = Some("boom".to_string());
        state.update(RoomsMessage::RoomSettingsDescriptionChanged(
            "d".to_string(),
        ));
        assert!(state.room_settings_error.is_none(), "inline error cleared");
        state.room_settings_error = Some("boom".to_string());
        state.update(RoomsMessage::RoomSettingsTagsChanged("t".to_string()));
        assert!(
            state.room_settings_error.is_none(),
            "tags change clears error"
        );
        state.room_settings_error = Some("boom".to_string());
        state.update(RoomsMessage::RoomSettingsVisibilityChanged(
            RoomVisibility::PublicUnlisted,
        ));
        assert!(
            state.room_settings_error.is_none(),
            "visibility change clears error"
        );
    }
}
