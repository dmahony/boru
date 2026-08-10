# app.rs Module Map (BORU-AUDIT-22)

Status: in progress — this document is the module map produced before the
decomposition started (spec step 1). It records the pre-refactor structure of
`examples/iced_chat/app.rs` (54,101 lines, ~2.4 MB) so extractions can be
verified against a stable baseline.

## Extraction progress (branch wt/t_08debaa8)

| Commit | Extraction | app.rs before → after |
|---|---|---|
| 27893331 | home screen (rail cards + chat-list) → `app/home.rs` | 54,101 → 52,625 |
| 12e87eff | sidebar (chats/groups/friends/discover/rooms/requests) → `app/sidebar.rs` | 52,625 → 50,532 |
| 0888c101 | fix: sidebar snapshot fields pub(crate) + stale UI-HOME-10 test | — |
| ce1e87c5 | settings screen → `app/settings.rs` | 50,532 → 49,320 |
| 77dd716b | friend requests screen → `app/contacts.rs` | 49,320 → 48,980 |
| (this run) | file sharing dashboard → `app/files.rs` | 48,980 → 44,637 |
| (this run) | chat log/composer → `app/chat.rs` | 44,637 → 40,814 |
| (this run) | discover + peer/friend profiles → `app/discover.rs` | 40,814 → 39,382 |
| (this run) | call screens → `app/calls.rs` | 39,382 → 39,252 |
| 8523fce7 | shared shell/dialog overlays → `app/dialogs.rs` | 39,252 → 38,513 |
| a0c2561c | tunnel share views → `app/tunnels.rs` | 38,513 → 38,266 |
| f6bae940 | calls update arms → `update_calls` in `app/calls.rs` | 38,266 → 38,102 |
| 3dfd10f3 | tunnels update arms → `update_tunnels` in `app/tunnels.rs` | 38,102 → 37,591 |
| bb81ea55 | friend-request update arms → `update_contacts` in `app/contacts.rs` | 37,591 → 37,437 |
| 0c4dab11 | file-sharing dashboard update arms → `update_files` in `app/files.rs` | 37,437 → 37,146 |
| 164c76a3 | discover/directory update arms → `update_discover` in `app/discover.rs` | 37,146 → 36,925 |
| 4b5e68a7 | settings update arms → `update_settings` in `app/settings.rs` | 36,925 → 36,434 |
| 51daa14a | group creation update arms → `update_groups` in new `app/groups.rs` | 36,434 → 36,215 |
| faadb970 | non-dashboard file transfer update arms → `update_files` in `app/files.rs` | 36,215 → 35,065 |
| 2b606724 | invite member + accept group invite update arms → `update_groups` | 35,065 → 34,868 |
| f9695050 | chat composer/send update arms → `update_chat` in `app/chat.rs` | 34,868 → 34,128 |
| acbdbeaf | conversation event update arms (net/whisper/inbox/outbox) → `update_chat` | 34,128 → 33,201 |
| 314f6d42 | chat context menu + emoji/gif picker update arms → `update_chat` | 33,201 → 32,899 |
| 20de6c8d | inline video playback update arms → `update_chat` | 32,899 → 32,017 |
| 54cfb6ac | file/media state update arms (short codes, downloads, images) → `update_files` | 32,017 → 31,261 |
| e59ea8bd | SENDME-02 ticket sharing update arms → `update_files` | 31,261 → 31,063 |
| 15f4a568 | lightbox + history/conversation management update arms → `update_chat` | 31,063 → 30,862 |
| 32bc067a | shared file catalogue + chat log scroll update arms → `update_files`/`update_chat` | 30,862 → 30,488 |
| 8a7aaee4 | friend profile/management update arms → `update_contacts` | 30,488 → 30,334 |
| b3e5da07 | friend request send/accept/decline update arms → `update_contacts` | 30,334 → 30,094 |
| a0cb2427 | group-created + send-message update arms → `update_groups`/`update_chat` | 30,094 → 29,880 |
| 4de4be2d | friend-chat, join-ticket, groups nav, file-download, delete-room → feature modules | 29,880 → 29,658 |
| d94741aa | new-discovered-peers update arm → `update_discover` | 29,658 → 29,625 |
| a3d85712 | settings/terminal navigation update arms → `update_settings` | 29,625 → 29,587 |
| aa8ad2f7 | friend confirm/block/rename update arms → `update_contacts` | 29,587 → 29,425 |
| e4d27e85 | background subscription arms (SubscribeStoredConversations / BackgroundSubscribe / BackgroundSubscribed) → `update_discover` | 29,425 → 29,292 |
| c3b22772 | import-friend-from-file update arms → `update_contacts` | 29,292 → 29,258 |
| dfe8b2a6 | catalogue error update arms → `update_discover` | 29,258 → 29,251 |

State layer (spec steps 4–6) started: the calls, tunnels, contacts, files,
discover, settings, groups, chat, home and media features' update arms were
moved into per-feature `update_calls` / `update_tunnels` / `update_contacts` /
`update_files` / `update_discover` / `update_settings` / `update_groups` /
`update_chat` / `update_home` methods; app.rs's `update()` now dispatches
those variants via combined match arms. State ownership stays on the root
`IcedChat` for now (spec step 3 constraint: keep state ownership unchanged
during the first pass). What remains inline in `update()` is mostly
navigation/room-open/join-from-ticket arms, global keyboard shortcuts,
dashboard tab navigation, GUI test actions, tick handlers (splash/activity/
conn monitor/mesh watchdog/outbox retry), pre-warm, connection details and
generic shell helpers — the composition-layer surface per spec step 8.
Remaining work: per-feature subscriptions (step 7) and optional feature
state structs (steps 4–6).

## Overview

`app.rs` is the root Iced application module for the `boru` example binary. It
contains the root `IcedChat` state struct, the 364-variant `AppMessage` enum,
the ~370-arm `update()` match, the `view()` tree, the subscription batch, the
`ChatCallbacks` implementation, and ~11,000 lines of unit tests.

Sibling modules already extracted (declared in `examples/iced_chat/main.rs`):
`activity_log_view_model`, `boru_dialog`, `card_shell`, `component_gallery`,
`connection_details`, `dashboard_filters`, `dashboard_view_model`,
`design_tokens`, `download_progress_view`, `downloaded_view_model`,
`downloading_view_model`, `file_category`, `file_type_icon`,
`file_type_resolver`, `focusable_button`, `fonts`, `form_components`,
`gui_test_actions`, `icon_system`, `link_preview`, `log_viewer`, `mcp_server`,
`notification` (subdirectory), `peers_downloading_view_model`, `perf_tracker`,
`presentation`, `quick_actions`, `recent_activity_view_model`,
`shared_by_me_table`, `sharing_summary`, `status_card`, `terminal_view`,
`ui_components`, `video_file_card`.

## Top-level layout (line numbers on branch wt/t_08debaa8)

| Lines | Section |
|---|---|
| 1–150 | imports (boru_core, iroh, iced, sibling modules) |
| 151–260 | `InlineVideoSession`, `InlineVideoEvent`, `SharedTracker` |
| 263–387 | `AppSettings` (+ Default, load/save) |
| 387–491 | `LogoSize`, `BoruLogo` widget, `version_tag()` |
| 491–1005 | theme helpers (`text_muted`, `accent_*`, `color_*`), icon consts, `FriendsSidebarCacheKey` |
| 1005–1470 | presence / sidebar cache helpers, `PeerPresence` |
| 1471–1575 | `PeerPresence` enum+impl, `HomeConnectionVariant`, `home_connection_variant()`, `peer_id_short_form()` |
| 1576–2005 | `MeshEvent`, `MeshEventTone`, `ChatKind`, `InlinePlaybackError`, `DownloadFailure`, `DownloadState` |
| 2006–2503 | `DownloadAttachment` (+ Hash), `ChatEntry` (2504) |
| 2504–2943 | `ChatEntry` struct + impl (system/local/remote/image constructors, `estimated_height`, `update_cache`) |
| 2943–3118 | `Screen` enum, `OutgoingCallStatus`, `RoomSnapshot`, `ConversationLive` |
| 3118–3485 | `ConversationLive` impl, `ContextMenuKind`, `IdleTimer`, `ResponsiveMode`, `Prebuilt` widget, `IncomingCall` |
| 3485–4510 | `IcedChat` root state struct (all fields) |
| 4511–4734 | `PeerProfileData`, `OutgoingRequestState`, `RecentActivityEvent`, sidebar row structs, `MeshHealthSnapshot` |
| 4734–5695 | Dependency snapshot structs for lazy screens (`ChatListDependency`, `FileSharingDependency`, `DownloadsCardDependency`, `SharedByMeCardDependency`, `PeersCardDependency`, `RecentActivityCardDependency`, `SharingSummaryCardDependency`, `FriendRequestsDependency`, `SettingsDependency`, `PeerProfileDependency`, `CatalogueRowSnapshot`, home card data structs) |
| 5695–7240 | `AppMessage` enum (364 variants) |
| 7241–7727 | `view_local_profile_block` + helpers |
| 7727–10376 | `impl IcedChat` #1: `new()`, persistence helpers, image hydration, download progress |
| 10376–11616 | `impl IcedChat` #2: misc |
| 11404–11616 | `impl IcedChat` #3 |
| 11616–24852 | `update(&mut self, message: AppMessage) -> Task<AppMessage>` (~370 arms) |
| 24852–25492 | `update_room_preview` |
| 25492–26607 | `impl IcedChat` #4/#5 |
| 25745–26238 | `impl ChatCallbacks for IcedChat` |
| 26607–26864 | `view_splash` |
| 26672–... | `view_incoming_call_overlay` |
| 26706–26864 | `pub fn view()` |
| 26864–27606 | expanded video, dialogs (connection details, lightbox, create room/group, receive ticket, short code, redeem, create tunnel, invite member) |
| 27606–29705 | sidebar views: `view_sidebar`, `view_sidebar_chats`, groups, discovered peers, public rooms, friends, requests |
| 29705–30125 | home rail cards: `view_online_peers_card`, `view_recent_activity_card`, `view_tunnels_card`, `view_main_empty_state` |
| 30125–30868 | `view_chat_list_content` (home screen) |
| 30868–31837 | calls: `view_outgoing_call`, `view_active_call`, `view_chat_panel` |
| 31337–33564 | context menu, emoji picker, gif picker, chat header/footer/search, group member list, options popover, details panels, group info |
| 33564–34505 | `view_chat_log` (with layout cache) |
| 34505–34981 | `view_composer`, `view_help` |
| 34981–35927 | settings screen |
| 35927–36432 | friend requests |
| 36432–36583 | `subscription_stream` |
| 36583–36868 | `subscription()` |
| 36868–41011 | peer profile, peer catalogue, sharing summary, shared with me, recent download activity, peers downloading, downloaded, downloads card, download manager, downloading, activity log, shared by me card, peers card |
| 41011–42756 | file sharing, discover, friend profile, share local service dialogs |
| 42756–54101 | `mod tests` (~11,000 lines, ~280 tests) |

## Feature → code map

### Navigation / shell (shared)
- State: `screen`, `settings_return_to`, `friend_requests_return_to`, `peer_profile_return_to`, `friend_profile_return_to`, `discover_return_to`, `groups_return_to`, `download_manager_return_to`, `pending_topic`, `room_loading`, `room_generation`, `conversation_generation`, `sidebar_section_collapsed`, `sidebar_fade_frame`, `prewarm_cache`, `prewarming`, `idle_timer`, `prewarm_window_mode`
- Messages: `GoToChatList`, `Shortcut`, `OpenRoom`, `RoomOpened`, `CreateNewRoom`, `ConfirmCreateNewRoom`, `CancelCreateRoom`, `CreateNewRoomDhtToggled`, `CreateNewRoomNameChanged`, `CreateNewRoomAdvertiseToggled`, `JoinFromTicket`, `RoomJoinFailed`, `WindowFocusChanged`, `ToggleHelp`, `ToggleSidebarSectionCollapsed`, `WindowResized`, `Scrolled`, `IdleTick`, `UserActivity`, `SplashTick`, `OpenTerminal`, `TerminalEvent`
- Update sections: `// ── Navigation ──`, `// ── ChatList ──`, `// ── Global keyboard shortcuts ──`, `// ── Pre-warm (PERF-4R-B) ──`
- Views: `view_splash`, `view_sidebar`, `view_main_empty_state`, `view_chat_list_content`

### Chat (active room)
- State: `topic`, `ticket_str`, `entries`, `composer_text`, `composer_sending`, `composer_drag_over`, `composer_ime_active`, `pending_file`, `pending_image`, `pending_gif`, `pending_thumbnail_fetch`, `pending_image_upload`, `pending_file_upload`, `download_entry_index`, `active_download_transfer_id`, `transfer_id_to_index`, `names`, `sender`, `sender_ready`, `forward_handle`, `forward_handle_slot`, `follow_latest`, `total_content_height`, `conversations`, `event_id_to_index`, `message_hash_to_index`, `self_sent_events`, `pending_offline_ids`, `help_visible`, `chat_search*`, `context_menu_*`, `emoji_picker_*`, `gif_*`, `lightbox_image`
- Messages: `InputChanged`, `SendPressed`, `AttachPressed`, `ComposerSendFinished`, `ComposerDragOver`, `ComposerFileDropped`, `ComposerImeActive`, `SendMessage`, `ToggleChatOptions`, `ToggleChatSearch`, `ChatSearchQueryChanged`, `ClearConversation`, `ToggleDetailsPanel`, `ToggleEmojiPicker`, `InsertEmoji`, `ToggleGifPicker`, `GifSearch*`, `SendGif`, `GifRetry`, `OpenImageLightbox`, `CloseImageLightbox`, `ToggleMemberList`, `ToggleVideoCardMenu`, `NetEvent`, `ReplayPendingEvents`, `ConversationNetEvent`-ish
- Update sections: `// ── Chat ──`, `// ── Reactions ──`, `// ── Edit ──`, `// ── Delete ──`
- Views: `view_chat_panel`, `view_chat_header`, `view_chat_footer`, `view_chat_search_panel`, `view_chat_log`, `view_composer`, `view_emoji_picker`, `view_gif_picker`, `view_context_menu`, `view_details_panel`, `view_group_member_list`, `view_chat_options_popover`, `view_image_lightbox`
- Types: `ChatEntry`, `ChatKind`, `ConversationLive`, `LayoutCache`, `InlineVideoSession`, `DownloadAttachment`, `DownloadState`

### Contacts / friends
- State: `friends`, `friends_dirty`, `friend_mgr`, `friend_events_rx`, `friend_search*`, `friend_profile_*`, `friend_requests_*`, `rename_*`, `block_*`, `remove_*`, `peer_profile_*`
- Messages: `ImportFriendFromFile`, `ImportFriendFromFilePicked`, `FriendEvent`, `SendFriendRequest`, `FriendRequestSent`, `FriendRequestFailed`, `FriendRequestReceived`, `FriendRequestRetry`, `IncomingFriendRequestAccept`, `IncomingFriendRequestDecline`, `IncomingFriendRequestProcessed`, `OpenFriendProfile`, `CloseFriendProfile`, `ToggleFriendProfileMenu`, `FriendRenameInputChanged`, `FriendRenameConfirm`, `ShowRemoveFriendConfirm`, `ConfirmRemoveFriend`, `CancelRemoveFriend`, `RemoveFriend`, `ShowBlockFriendConfirm`, `ConfirmBlockFriend`, `CancelBlockFriend`, `OpenPeerProfile`, `ClosePeerProfile`, `CopyPeerId`, `CopyFriendId`, `FriendIdCopiedClear`, `OpenFriendChat`, `FriendAdded`, `FriendRemoved`, `FriendListResult`, `FriendRequestSearchChanged`, `FriendRequestSend`, `FriendRequestAccept`, `FriendRequestDecline`, `FriendRequestCancel`, `FriendRequestSentResult`, `FriendRequestActionResult`, `OpenFriendRequests`, `CloseFriendRequests`
- Update sections: `// ── Friend commands ──`, `// ── Friend Requests ──`, `// ── Friend Profile Navigation ──`
- Views: `view_friend_requests`, `view_friend_requests_content`, `view_friend_profile`, `view_friend_profile_content`, `view_peer_profile`, `view_peer_profile_content`, `view_sidebar_friends*`, `view_sidebar_requests*`

### Groups
- State: `group_*` (create dialog), `groups_return_to`, invite member state
- Messages: `ShowCreateGroupDialog`, `HideCreateGroupDialog`, `CreateGroupNameChanged`, `CreateGroupDescriptionChanged`, `CreateGroupMemberToggled`, `CreateGroupSearchChanged`, `ConfirmCreateGroup`, `GroupCreated`, `OpenGroupChat`, `ToggleMemberList`, `ConfirmInviteMember`, `HideInviteMemberDialog`, `ShowInviteMemberDialog`, `InviteMemberToggled`, `AcceptGroupInvite`, `OpenGroups`, `CloseGroups`, `GroupInviteParsed`
- Update sections: `// ── Group Creation ──`, `// ── Group invite parsing ──`, `// ── Invite Member ──`, `// ── End Invite Member ──`
- Views: `view_groups_screen`, `view_groups_screen_content`, `view_groups_section_content`, `view_sidebar_groups`, `view_create_group_dialog`, `view_group_info_panel`, `view_invite_member_dialog`

### Public chats / discover / directory
- State: `directory_*`, `discover_return_to`, `public_room_*`, `seen_peers`, `room_history`
- Messages: `ToggleAdvertiseRoom`, `SubscribeDirectoryTopic`, `DirectorySubscribed`, `OpenDirectory`, `CloseDiscover`, `DirectoryRoomJoin`, `DeleteDirectoryRoom`, `DirectoryRoomUpdate`, `OpenDiscover`, `BrowsePeerCatalogue`, `PeerCatalogueReceived`, `PeerCatalogueFailed`, `CatalogueScrolled`, `RequestFileDownload`, `NewDiscoveredPeers`, `SubscribeStoredConversations`, `BackgroundSubscribe`, `BackgroundSubscribed`
- Views: `view_discover`, `view_discover_content`, `view_sidebar_discovered_peers*`, `view_sidebar_public_rooms*`, `view_peer_catalogue*`

### Files / media
- State: `pending_file_upload`, `pending_image_upload`, `file_upload_spinner_frame`, `image_upload_spinner_frame`, `shared_files`, `shared_folders`, `dashboard_*` (search, tab, sorts), `downloaded_*`, `shared_by_me_*`, `download_manager_*`, `paused_inbound_transfer_ids`
- Messages: `ExecuteFileSend`, `AttachFolderPressed`, `ExecuteFolderSend`, `ExecuteDownload`, `ExecuteDownloadAt`, `PauseDownloadAt`, `ResumeDownloadAt`, `CancelDownloadAt`, `ReshareFile`, `DownloadProgress`, `DownloadDone`, `DownloadDonePeerFile`, `DownloadFailed`, `DownloadInitiated`, `DownloadInitiationFailed`, `DownloadingCancel`, `DownloadingPause`, `DownloadingResume`, `DownloadingStop`, `OpenDownloadManager`, `CloseDownloadManager`, `OpenDownloadsFolder`, `OpenDownloadedFile`, `FileDownloaded`, `ThumbnailFetched`, `ImageHydrated`, `FileSent`, `FileUploadFailed`, `ImageUploadFailed`, `PosterGenerated`, `VideoMetadataProbed`, `TransferProjectionUpdate`, `TransferSnapshotResync`, `AddSharedFile`, `AddSharedFolder`, `SharedFolderPicked`, `SharedFilePicked`, `SharedFileAdded`, `SharedFileAddFailed`, `RemoveSharedFile`, `SharedFileRemoved`, `SharedByMe*`, `Dashboard*`, `ActivityLog*`, `Downloaded*`, `SetOverwritePolicy`, `MintShortCode`, `ShortCodeMinted`, `RedeemShortCode`, `ShortCodeRedeemed`, `OpenRedeemCodeDialog`, `RedeemCodeInputChanged`, `CopyShortCode`, `CloseShortCodeDialog`, `OpenReceiveTicketDialog`, `ReceiveTicket*`, `ConfirmReceiveTicket`, `CopyShareTicket`
- Views: `view_file_sharing`, `view_file_sharing_content`, `view_downloads_card`, `view_download_manager`, `view_downloading`, `view_downloaded`, `view_activity_log`, `view_shared_by_me_card`, `view_shared_with_me`, `view_peers_downloading_from_me`, `view_recent_download_activity_card`, `view_sharing_summary_card`, `view_peers_card`, `view_online_peers_card`, `view_recent_activity_card`, `view_tunnels_card`
- Types: `DownloadAttachment`, `DownloadState`, `CatalogueRowSnapshot`, `CatalogueDownloadState`, `RemoteSharedFile`

### Calls
- State: `incoming_call`, `outgoing_call_*`, `active_call_*`, `call_*` (mute/camera/device selection), `call_events_rx`
- Messages: `StartVoiceCall`, `StartVideoCall`, `CallEventReceived`, `AcceptIncomingCall`, `RejectIncomingCall`, `HangUp`, `ToggleCallMute`, `ToggleCallCamera`, `SelectMicrophone`, `SelectSpeaker`, `SelectCamera`, `CallUiTick`, `CallStarted`, `CallCommandFinished`
- Views: `view_outgoing_call`, `view_active_call`, `view_incoming_call_overlay`

### Tunnels
- State: `tunnel_service`, `create_tunnel_*`, `share_local_service_*`, `received_tunnel_*`
- Messages: `ShowCreateTunnelDialog`, `CreateTunnelPortChanged`, `CreateTunnel`, `CancelCreateTunnel`, `TunnelRequestReceived`, `AcceptTunnelRequest`, `DeclineTunnelRequest`, `CloseTunnel`, `OpenShareLocalService`, `ShareLocalService*`, `ConfirmShareLocalService`, `CancelShareLocalService`, `ShareLocalServiceScanDone`, `SelectShareLocalServiceSuggestion`, `TunnelShared`, `TunnelShareFailed`, `TunnelOfferSent`, `TunnelOfferSendFailed`, `ShareLocalServiceHttpToggled`, `ConnectReceivedTunnel`, `ReceivedTunnelConnected`, `ReceivedTunnelConnectFailed`, `DisconnectReceivedTunnel`, `StopSharingTunnel`, `OpenReceivedTunnel`, `CopyReceivedTunnelAddress`
- Views: `view_create_tunnel_dialog`, `view_tunnels_card`, `view_share_local_service_dialog`, `view_local_service_suggestion_row`

### Settings
- State: `settings`, `dark_mode`, `sound_enabled`, `share_direct_addresses`, `chat_text_size`, `accent_color_*`, `profile_image_*`, `home_background_*`, `home_menu_item_opacity`, `settings_return_to`
- Messages: `OpenSettings`, `CloseSettings`, `ToggleDark`, `ToggleAccentColorPicker`, `AccentColorSelected`, `AccentColorCancelled`, `SetNickname`, `ToggleSound`, `ToggleInviteAddressSharing`, `SetChatTextSize`, `PickProfileImage`, `ProfileImagePicked`, `PickHomeBackgroundImage`, `HomeBackgroundImagePicked`, `HomeBackgroundImageReady`, `RemoveHomeBackgroundImage`, `SetHomeMenuItemOpacity`, `ProfileImageUploaded`, `RemoveProfileImage`, `ProfileImageRemoved`, `ProfileImagePersisted`, `ProfileImageDownloaded`, `ProfileImageDownloadFailed`, `SaveProfile`, `ProfileSaved`, `CopyToClipboard`, `SystemMsg`
- Views: `view_settings_screen`, `view_settings_screen_content`, `view_settings_screen_cached`, `view_local_profile_block`

### Notifications
- State: `notification_service` (sibling module `notification/`), `WindowFocusTracker`
- Messages: `NotificationEvent`, `WindowFocusChanged`
- Subscriptions: notification event subscription

### ChatCallbacks impl (25745–26238)
- `push_remote`, `set_name`, `on_neighbor_up/down`, `set_pending_image`, `set_pending_thumbnail`, `set_pending_gif`, `set_pending_file`, `set_pending_share`, `download_progress`, `delivery_state_changed`, `transfer_started`, etc.

## Existing test coverage (in `mod tests`, ~11,000 lines)

High-value existing tests grouped by feature:
- Home/chat list: `home_connection_variant_maps_each_network_state_truthfully`, `status_card_is_wired_into_home_screen`, `home_online_peers_card_*`, `activity_push_changes_only_activity_card_data`, `tunnel_status_change_changes_only_tunnels_card_data`, `lazy_card_dependencies_are_stable_without_change`, `dashboard_search_typing_changes_only_shared_by_me_card`
- Chat: `normal_send_produces_local_entry_via_shared_path`, `send_pressed_skips_while_ime_composing`, `conversation_switch_*`, `inactive_room_message_increments_unread_badge`, `chat_scroll_*`, `close_lightbox_stale_scrolled_event_keeps_sentinel_and_requeues_snap`
- Presence: `peer_presence_*` (labels/icons/colors/transitions)
- Downloads: `download_manager_*`, `outbound_transfer_start_and_completion_push_recent_activity`, `replayed_terminal_outbound_update_is_idempotent`
- Dialogs: `confirm_group_with_empty_name_keeps_dialog_open_with_inline_error`, `confirm_share_local_service_invalid_port_sets_inline_error_and_keeps_open`, `vr_create_*` suite
- MCP/GUI actions: `gui_navigation_actions_reach_completed_via_normal_update_path`, `gui_open_room_action_*`, `gui_submit_composer_action_*`

## Extraction plan (ordered by risk)

1. **Pure view helpers + dependency snapshots** (lower risk): home rail cards
   (`view_online_peers_card`, `view_recent_activity_card`, `view_tunnels_card`,
   `view_chat_list_content`) + their `*CardData`/`ChatListDependency` structs →
   `app/home.rs`. Static functions over snapshot structs, no `&mut self`.
2. **Sidebar views** → `app/sidebar.rs` (mostly static content fns +
   dependency structs).
3. **Settings screen** → `app/settings.rs`.
4. **Friend requests** → `app/contacts.rs`.
5. **File sharing dashboard** → `app/files.rs` (depends on already-extracted
   view models).
6. **Chat log / composer** → `app/chat.rs` (harder: `&mut self`, layout cache).
7. Feature state structs + feature-local message enums (spec steps 4–6) after
   view extraction proves the seam.
8. Subscriptions per feature (spec step 7).
9. Keep `app.rs` as composition/router.

## Constraints

- No duplicate state cache: dependency structs are the single snapshot source
  for lazy screens; do not add a second cache.
- Keep message variants and state ownership unchanged during the first pass.
- Compile gate: `rb check --example boru --features gui,video-playback,terminal`.
- Tests gate: existing ~280 tests in `mod tests` must stay green.
