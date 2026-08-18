# App.rs Responsibility Inventory (BORU-APP-001)

Machine-assisted inventory of `src/bin/boru/app.rs` — the ~1.82 MB Iced
application shell — taken before any code movement. This document is the map that
BORU-APP-002 (routing pattern) and BORU-APP-003..010 (extractions) will follow.

- Created: 2026-08-18 (BORU-ARCH-03, task t_dd923afe)
- PDF source: `Boru_Code_Improvement_Action_Plan.pdf`, Phase 1 task BORU-APP-001
- Pairs with: `docs/architecture-refactor/baseline.md` (BORU-ARCH-001),
  `docs/architecture-refactor/architecture-boundaries.md` (BORU-ARCH-002)
- Method: line/brace-aware extraction of `app.rs` at `origin/main` (9124d8c8) —
  struct span, enum span, `update()` match block, module delegation arms, view fns.
- No production code changed by this task.

## 1. Snapshot (what is being inventoried)

| Item | Value |
|------|-------|
| `src/bin/boru/app.rs` | 41,831 lines / ~1.82 MB |
| `pub struct IcedChat` | lines 3746–5136 |
| `pub enum AppMessage` | lines 5691–6992 |
| `pub fn update()` | lines 12786–18043 (match at 12793–18027) |
| `pub fn view()` | lines 21400–21733 |
| `pub fn subscription()` | lines 23393–23540 |
| `impl IcedChat` methods in app.rs | 819 (incl. tests after line ~23700) |
| Delegated module `update_*` helpers | 9 (`update_calls/chat/contacts/discover/files/groups/home/settings/tunnels`) |
| `app/` module tree | calls, chat, contacts, dialogs, discover, files, groups, home, screen_share_surface, screen_share_ui, settings, sidebar, tunnels |

**Counts captured by this inventory:**

| Domain | Fields | Message variants |
|---|---|---|
| App shell / navigation | 30 | 15 |
| Conversations (chat) | 76 | 88 |
| Files / transfers | 61 | 89 |
| Rooms / groups / directory | 80 | 75 |
| Friends | 22 | 41 |
| Calls | 13 | 14 |
| Screen sharing | 44 | 32 |
| Tunnels | 17 | 29 |
| Discovery / presence / mesh | 40 | 16 |
| Settings | 23 | 21 |
| Notifications | 4 | 1 |
| Diagnostics / dev UI | 33 | 10 |
| Shared infrastructure (remain shared) | 34 | — |
| **Total** | **477** | **431** |

Note on shared infrastructure: 34 fields are the network substrate / service handles /
stores that ARCH-002 §4 marks **remain shared** (read-only context). They are listed in
§2 for completeness but are not extraction targets.

## 2. Field inventory (App struct fields by domain)

Every field of `IcedChat` (lines 3746–5136) assigned to a domain. Line numbers are
app.rs line numbers at `origin/main` (9124d8c8).

### 2.1 Summary

| Domain | Fields | Notes |
|---|---|---|
| App shell / navigation | 30 | Screen routing, return pointers, prewarm, splash, window |
| Conversations (chat) | 76 | Conversation map + active-chat projection, composer, inline video, GIF/emoji |
| Files / transfers | 61 | Dashboard + download/upload + catalogue + short codes + activity log |
| Rooms / groups / directory | 80 | Chat-list/sidebar caches, create room/group dialogs, discover filters, directory advertising |
| Friends | 22 | Friends store cache, requests, peer profiles, friend images |
| Calls | 13 | Active/outgoing/incoming call state, media selection, frames |
| Screen sharing | 44 | Host/viewer session, cursor, control, source, pan |
| Tunnels | 17 | Create-tunnel dialog, share-local-service, received/shared tunnel maps |
| Discovery / presence / mesh | 40 | Presence maps, mesh health, neighbor/latency counters, reconnect, directory handles |
| Settings | 23 | AppSettings + theme/layout active values, sound/address toggles, profile image |
| Notifications | 4 | Notification service, focus tracker, toast |
| Diagnostics / dev UI | 33 | GUI-test action queue, snapshot publishing, inspector, designer, perf |
| Shared infrastructure | 34 | Endpoint/relay/blob-store, service handles, stores, receiver channels |

### 2.2 Field tables

#### App shell / navigation (30 fields)

| Field | Line |
|---|---|
| `designer` | 3750 |
| `designer_history` | 3752 |
| `screen` | 3754 |
| `terminal` | 3760 |
| `pending_topic` | 3773 |
| `room_loading` | 3776 |
| `room_generation` | 3784 |
| `conversation_generation` | 3792 |
| `settings_return_to` | 3794 |
| `friend_requests_return_to` | 3796 |
| `peer_profile_return_to` | 3799 |
| `friend_profile_return_to` | 3801 |
| `discover_return_to` | 3803 |
| `groups_return_to` | 3817 |
| `download_manager_return_to` | 3819 |
| `prewarm_cache` | 3833 |
| `prewarming` | 3841 |
| `idle_timer` | 3843 |
| `prewarm_window_mode` | 3846 |
| `prewarm_invalidate_pending` | 3854 |
| `notice` | 4201 |
| `return_to_chat_list_after_open` | 4291 |
| `call_return_screen` | 4301 |
| `first_run` | 4544 |
| `window_width` | 4971 |
| `window_height` | 4973 |
| `splash_start_time` | 4976 |
| `splash_has_rendered` | 4980 |
| `splash_spinner_frame` | 4982 |
| `reduced_motion` | 4991 |

#### Conversations (chat) (76 fields)

| Field | Line |
|---|---|
| `lightbox_image` | 3762 |
| `lightbox_close_snap_guard` | 3770 |
| `conversations` | 3859 |
| `room_history` | 3862 |
| `room_history_dirty` | 3863 |
| `topic` | 3881 |
| `ticket_str` | 3883 |
| `entries` | 3885 |
| `composer_text` | 3887 |
| `composer_sending` | 3890 |
| `composer_drag_over` | 3893 |
| `composer_ime_active` | 3896 |
| `help_visible` | 3897 |
| `pending_file` | 3898 |
| `pending_image` | 3900 |
| `pending_gif` | 3905 |
| `pending_thumbnail_fetch` | 3910 |
| `pending_image_upload` | 3912 |
| `image_upload_spinner_frame` | 3914 |
| `pending_file_upload` | 3916 |
| `file_upload_spinner_frame` | 3918 |
| `download_entry_index` | 3920 |
| `active_download_transfer_id` | 3923 |
| `inline_video` | 3925 |
| `playback_coordinator` | 3927 |
| `inline_video_seek` | 3929 |
| `inline_video_expanded` | 3931 |
| `inline_video_resume` | 3935 |
| `video_runtime` | 3937 |
| `external_stream_server` | 3943 |
| `transfer_id_to_index` | 3946 |
| `names` | 3947 |
| `sender` | 3949 |
| `sender_ready` | 3953 |
| `forward_handle` | 3955 |
| `forward_handle_slot` | 3957 |
| `self_sent_events` | 4052 |
| `event_id_to_index` | 4054 |
| `message_hash_to_index` | 4055 |
| `pending_offline_ids` | 4060 |
| `follow_latest` | 4063 |
| `total_content_height` | 4066 |
| `history_saved_count` | 4224 |
| `scroll_offset` | 4226 |
| `viewport_height` | 4228 |
| `scroll_to_bottom_pending` | 4233 |
| `local_mailbox_key` | 4520 |
| `context_menu` | 4760 |
| `video_card_menu_open` | 4762 |
| `connecting_spinner_frame` | 4985 |
| `link_preview_cache` | 4993 |
| `link_preview_fetch_index` | 4998 |
| `show_chat_options` | 5000 |
| `show_chat_search` | 5002 |
| `chat_search_query` | 5004 |
| `show_member_list` | 5006 |
| `show_emoji_picker` | 5008 |
| `emoji_category` | 5011 |
| `emoji_search_query` | 5014 |
| `recent_emojis` | 5018 |
| `show_gif_picker` | 5020 |
| `gif_search_text` | 5022 |
| `gif_results` | 5024 |
| `gif_preview_cache` | 5027 |
| `gif_loading` | 5029 |
| `gif_showing_trending` | 5031 |
| `gif_has_searched` | 5034 |
| `gif_error` | 5036 |
| `gif_next_cursor` | 5038 |
| `gif_appending` | 5041 |
| `gif_append_error` | 5045 |
| `gif_request_seq` | 5049 |
| `gif_debounce_seq` | 5051 |
| `gif_spinner_frame` | 5053 |
| `gif_not_configured` | 5057 |
| `details_panel_open` | 5063 |

#### Files / transfers (61 fields)

| Field | Line |
|---|---|
| `paused_inbound_transfer_ids` | 3825 |
| `download_progress_queue` | 4571 |
| `poster_result_queue` | 4577 |
| `last_download_progress_at` | 4579 |
| `last_download_progress_bytes` | 4581 |
| `blocked_sharers` | 4764 |
| `pending_downloads` | 4770 |
| `catalogue_downloads` | 4773 |
| `shared_folder_enabled` | 4781 |
| `shared_folder_path` | 4784 |
| `file_indexer` | 4787 |
| `shared_files` | 4790 |
| `boru_downloads_dir` | 4792 |
| `dashboard_active_tab` | 4796 |
| `dashboard_search_input` | 4798 |
| `dashboard_shared_by_me_sort` | 4802 |
| `dashboard_downloaded_sort` | 4804 |
| `dashboard_activity_sort` | 4806 |
| `dashboard_shared_by_me_filter` | 4812 |
| `transfer_store` | 4814 |
| `transfer_update_rx` | 4819 |
| `outbound_item_labels` | 4822 |
| `outbound_active` | 4824 |
| `outbound_history` | 4826 |
| `inbound_item_labels` | 4829 |
| `inbound_active` | 4831 |
| `inbound_history` | 4833 |
| `shared_by_me_ui` | 4836 |
| `shared_by_me_loading` | 4839 |
| `shared_by_me_rows` | 4842 |
| `shared_by_me_error` | 4845 |
| `shared_by_me_thumbnails` | 4850 |
| `dashboard_recent_activity` | 4853 |
| `dashboard_sharing_summary` | 4856 |
| `downloaded_history` | 4859 |
| `downloaded_history_loaded` | 4862 |
| `downloaded_history_error` | 4865 |
| `activity_log_rows` | 4869 |
| `activity_log_loaded` | 4872 |
| `activity_log_error` | 4874 |
| `activity_log_filter` | 4876 |
| `activity_log_page` | 4878 |
| `activity_log_details_open` | 4880 |
| `activity_log_clear_confirm` | 4882 |
| `peer_catalogue_view` | 4887 |
| `catalogue_loading` | 4889 |
| `catalogue_scroll_offset` | 4891 |
| `catalogue_viewport_height` | 4893 |
| `catalogue_error` | 4896 |
| `dashboard_connectivity_dismissed` | 4899 |
| `show_short_code_dialog` | 5081 |
| `short_code_dialog_code` | 5083 |
| `short_code_dialog_error` | 5085 |
| `short_code_minting` | 5087 |
| `short_code_sender` | 5092 |
| `short_code_active` | 5095 |
| `show_redeem_code_dialog` | 5097 |
| `redeem_code_input` | 5099 |
| `redeem_code_error` | 5101 |
| `redeem_code_busy` | 5103 |
| `redeemed_codes` | 5105 |

#### Rooms / groups / directory (80 fields)

| Field | Line |
|---|---|
| `discover_search_query` | 3806 |
| `discover_filter_compatible` | 3809 |
| `discover_filter_not_joined` | 3810 |
| `discover_filter_recently_seen` | 3811 |
| `discover_selected_tags` | 3813 |
| `discover_sort` | 3815 |
| `join_ticket_input` | 3865 |
| `chat_list_error` | 3867 |
| `sidebar_selected_topic` | 3870 |
| `sidebar_section_collapsed` | 3872 |
| `sidebar_fade_frame` | 3877 |
| `history_confirm_clear` | 4190 |
| `history_clear_pending` | 4192 |
| `history_clear_feedback` | 4194 |
| `history_clear_feedback_is_error` | 4196 |
| `room_delete_confirm_topic` | 4198 |
| `friends_sidebar_revision` | 4251 |
| `chats_sidebar_revision` | 4253 |
| `discovered_sidebar_revision` | 4255 |
| `public_rooms_sidebar_revision` | 4257 |
| `requests_sidebar_revision` | 4260 |
| `cached_chat_count` | 4263 |
| `cached_group_count` | 4264 |
| `cached_friend_count` | 4265 |
| `cached_discover_count` | 4266 |
| `cached_public_room_count` | 4267 |
| `cached_request_count` | 4268 |
| `cached_chats_revision` | 4272 |
| `cached_chats_dep` | 4273 |
| `cached_discovered_revision` | 4275 |
| `cached_discovered_dep` | 4276 |
| `cached_public_rooms_revision` | 4278 |
| `cached_public_rooms_dep` | 4279 |
| `cached_friends_rows_revision` | 4281 |
| `cached_friends_rows_dep` | 4282 |
| `cached_requests_revision` | 4284 |
| `cached_requests_dep` | 4285 |
| `home_background_path` | 4502 |
| `home_background_handle` | 4503 |
| `home_menu_item_opacity` | 4506 |
| `create_room_dht_enabled` | 4630 |
| `create_room_name` | 4632 |
| `create_room_visibility` | 4635 |
| `create_room_description` | 4638 |
| `create_room_tags` | 4641 |
| `room_trackers` | 4645 |
| `show_room_settings_dialog` | 4649 |
| `room_settings_topic` | 4651 |
| `room_settings_name` | 4653 |
| `room_settings_description` | 4655 |
| `room_settings_tags` | 4657 |
| `room_settings_visibility` | 4659 |
| `room_settings_error` | 4661 |
| `show_create_room_dialog` | 4663 |
| `create_room_submitting` | 4667 |
| `create_room_error` | 4669 |
| `show_create_group_dialog` | 4671 |
| `create_group_submitting` | 4675 |
| `create_group_error` | 4677 |
| `create_group_name` | 4679 |
| `create_group_description` | 4681 |
| `create_group_selected_members` | 4683 |
| `create_group_search` | 4685 |
| `recent_activity` | 4961 |
| `activity_tick` | 4968 |
| `main_screen_reconnect_frame` | 4988 |
| `show_invite_member_dialog` | 5059 |
| `invite_member_selected` | 5061 |
| `show_receive_ticket_dialog` | 5067 |
| `receive_ticket_input` | 5069 |
| `receive_ticket_preflight` | 5071 |
| `receive_ticket_error` | 5073 |
| `receive_ticket_preflight_busy` | 5075 |
| `receive_ticket_downloading` | 5077 |
| `advertised_rooms` | 5109 |
| `advertise_counter` | 5112 |
| `last_advertised_fingerprint` | 5117 |
| `last_advertised_at` | 5120 |
| `startup_advertise_swept` | 5124 |
| `auto_subscribed_rooms` | 5136 |

#### Friends (22 fields)

| Field | Line |
|---|---|
| `friends` | 3982 |
| `friends_dirty` | 3983 |
| `friend_mgr` | 3984 |
| `friend_image_handles` | 4523 |
| `friend_image_tickets` | 4527 |
| `pending_profile_image_tickets` | 4532 |
| `friend_profile_versions` | 4537 |
| `last_failed_profile_retry` | 4540 |
| `friend_request_store` | 4549 |
| `outgoing_request_states` | 4553 |
| `join_request_list` | 4556 |
| `friend_request_search_input` | 4566 |
| `friend_request_error` | 4568 |
| `friend_id_copied` | 4619 |
| `show_invite_menu` | 4621 |
| `invite_whisper_input` | 4623 |
| `friend_profile_menu_open` | 4704 |
| `friend_profile_rename_input` | 4706 |
| `friend_profile_renaming` | 4708 |
| `friend_remove_confirm` | 4710 |
| `friend_block_confirm` | 4754 |
| `profile_cache` | 4766 |

#### Calls (13 fields)

| Field | Line |
|---|---|
| `active_call_id` | 4298 |
| `outgoing_call_peer` | 4299 |
| `outgoing_call_status` | 4300 |
| `call_audio_muted` | 4302 |
| `call_camera_enabled` | 4303 |
| `call_camera_selection` | 4307 |
| `latest_remote_frame` | 4309 |
| `latest_local_frame` | 4311 |
| `call_started_at` | 4313 |
| `call_kind` | 4316 |
| `call_was_incoming` | 4317 |
| `call_declined` | 4318 |
| `incoming_call` | 4321 |

#### Screen sharing (44 fields)

| Field | Line |
|---|---|
| `screen_share_events_rx` | 4324 |
| `screen_share_media_rx` | 4327 |
| `screen_share_audio_rx` | 4331 |
| `screen_share_audio_stop` | 4334 |
| `screen_share_audio_active` | 4340 |
| `screen_share_audio_error` | 4348 |
| `screen_share_events_tx` | 4351 |
| `screen_share_protocol` | 4354 |
| `screen_share_frame_watch` | 4357 |
| `screen_share_stats_watch` | 4362 |
| `screen_share_viewer_stats` | 4366 |
| `screen_share_host_metrics` | 4370 |
| `screen_share_dev_overlay` | 4375 |
| `screen_share_host_state` | 4378 |
| `screen_share_host_stop` | 4381 |
| `screen_share_invite` | 4385 |
| `screen_share_viewing` | 4388 |
| `screen_share_view_session` | 4391 |
| `screen_share_decode_stop` | 4394 |
| `screen_share_fullscreen` | 4397 |
| `screen_share_last_frame_ts` | 4400 |
| `screen_share_frame_handle` | 4403 |
| `screen_share_cursor_sprite` | 4409 |
| `screen_share_cursor_pos` | 4413 |
| `screen_share_cursor_visible` | 4416 |
| `screen_share_cursor_enabled` | 4419 |
| `screen_share_cursor_frame_rgba` | 4425 |
| `screen_share_control_request` | 4428 |
| `screen_share_control_active` | 4431 |
| `screen_share_clipboard_active` | 4436 |
| `screen_share_host_cmd_tx` | 4439 |
| `screen_share_last_pointer_sent` | 4442 |
| `screen_share_last_pointer_pos` | 4445 |
| `screen_share_modifiers` | 4449 |
| `screen_share_view_mode` | 4452 |
| `screen_share_pan` | 4455 |
| `screen_share_drag` | 4458 |
| `screen_share_hover` | 4461 |
| `screen_share_src_size` | 4465 |
| `screen_share_sources` | 4471 |
| `screen_share_selected_source` | 4476 |
| `screen_share_selected_preset` | 4482 |
| `screen_share_viewing_peer` | 4487 |
| `screen_share_notice_ticks` | 4492 |

#### Tunnels (17 fields)

| Field | Line |
|---|---|
| `show_create_tunnel_dialog` | 4687 |
| `create_tunnel_port` | 4691 |
| `create_tunnel_port_error` | 4693 |
| `tunnel_requests` | 4695 |
| `share_local_service_open` | 4712 |
| `share_service_submitting` | 4716 |
| `share_service_error` | 4718 |
| `share_service_name` | 4720 |
| `share_service_port` | 4722 |
| `share_service_expiry` | 4724 |
| `share_expiry_combo` | 4726 |
| `share_service_is_http` | 4730 |
| `share_service_suggestions` | 4733 |
| `share_service_scanning` | 4735 |
| `share_service_scan_cached_at` | 4738 |
| `received_tunnels` | 4745 |
| `shared_tunnels` | 4752 |

#### Discovery / presence / mesh (40 fields)

| Field | Line |
|---|---|
| `neighbors` | 3987 |
| `known_peers` | 3992 |
| `room_neighbor_counts` | 3994 |
| `direct_peers` | 3996 |
| `relayed_peers` | 3998 |
| `conn_refresh_counter` | 4000 |
| `mesh_health` | 4002 |
| `last_mesh_health` | 4004 |
| `mesh_connected_at` | 4008 |
| `conversation_subscription_pending` | 4011 |
| `mesh_event_log` | 4013 |
| `presence_counter` | 4016 |
| `heartbeat_counter` | 4019 |
| `latency_ping_counter` | 4022 |
| `peer_latencies` | 4024 |
| `conn_refresh_in_flight` | 4026 |
| `needs_conn_refresh` | 4029 |
| `ticket_extra_peers` | 4034 |
| `pending_ticket_peers` | 4039 |
| `ticket_needs_regeneration` | 4043 |
| `ticket_resolve_in_flight` | 4045 |
| `background_subscriptions_in_flight` | 4050 |
| `peer_presence_map` | 4239 |
| `presence_away_peers` | 4243 |
| `seen_peers` | 4249 |
| `initial_bootstrap_peers` | 4289 |
| `discovered_peers` | 4590 |
| `discovered_online_cache` | 4594 |
| `reconnect_handle` | 4606 |
| `pending_neighbor_status` | 4609 |
| `pending_backfill_topics` | 4615 |
| `dht` | 4625 |
| `private_dht_disabled` | 4627 |
| `connection_details_dialog` | 4697 |
| `connection_details_announcement` | 4699 |
| `connection_details_focus_target` | 4701 |
| `directory_topic` | 5126 |
| `directory_sender` | 5129 |
| `directory_store` | 5131 |
| `directory_room_rx` | 5134 |

#### Settings (23 fields)

| Field | Line |
|---|---|
| `settings` | 4069 |
| `dark_mode` | 4072 |
| `ui_theme_config` | 4077 |
| `active_theme` | 4083 |
| `active_layout` | 4089 |
| `layout_overrides` | 4096 |
| `layout_revision` | 4101 |
| `theme_revision` | 4107 |
| `ui_theme_rx` | 4111 |
| `ui_theme_reload_tracker` | 4115 |
| `layout_rx` | 4120 |
| `layout_reload_tracker` | 4125 |
| `sound_enabled` | 4157 |
| `share_direct_addresses` | 4159 |
| `show_presence_indicator` | 4163 |
| `chat_text_size` | 4188 |
| `profile_image_handle` | 4498 |
| `accent_color` | 4509 |
| `show_accent_picker` | 4511 |
| `profile_image_ticket` | 4513 |
| `profile_image_identifier` | 4516 |
| `profile_store` | 4775 |
| `profile_bio_input` | 4778 |

#### Notifications (4 fields)

| Field | Line |
|---|---|
| `notification_service` | 4560 |
| `window_focus_tracker` | 4563 |
| `toast_message` | 4756 |
| `toast_counter` | 4758 |

#### Diagnostics / dev UI (33 fields)

| Field | Line |
|---|---|
| `inspector_visible` | 4130 |
| `inspector_draft` | 4134 |
| `inspect_ui_enabled` | 4141 |
| `inspect_hover` | 4146 |
| `inspect_selected` | 4151 |
| `gallery_state` | 4155 |
| `perf` | 4542 |
| `layout_cache` | 4547 |
| `iced_diagnostics` | 4903 |
| `gui_action_rx` | 4905 |
| `gui_action_history` | 4907 |
| `pending_open_room_action` | 4911 |
| `pending_open_conversation_action` | 4913 |
| `pending_set_composer_action` | 4915 |
| `pending_submit_composer_action` | 4917 |
| `pending_chat_list_action` | 4919 |
| `pending_open_friends_action` | 4921 |
| `pending_open_settings_action` | 4923 |
| `pending_open_file_sharing_action` | 4925 |
| `pending_dashboard_tab_action` | 4927 |
| `pending_share_file_action` | 4929 |
| `pending_close_dialog_action` | 4931 |
| `pending_toggle_help_action` | 4933 |
| `pending_select_peer_action` | 4935 |
| `pending_create_room_action` | 4937 |
| `pending_confirm_create_room_action` | 4939 |
| `pending_download_action` | 4941 |
| `gui_state_tx` | 4944 |
| `gui_state_enabled` | 4947 |
| `last_snapshot` | 4950 |
| `last_snapshot_at` | 4953 |
| `gui_snapshot_throttle_ms` | 4956 |
| `gui_snapshot_pending` | 4959 |

#### Shared infrastructure (remain shared, read-only) (34 fields)

| Field | Line |
|---|---|
| `secret_key` | 3960 |
| `gossip` | 3961 |
| `_router` | 3964 |
| `file_offer_registry` | 3967 |
| `blob_store` | 3968 |
| `endpoint` | 3969 |
| `memory_lookup` | 3970 |
| `local_label` | 3971 |
| `local_public` | 3972 |
| `relay_mode` | 3973 |
| `runtime_handle` | 3974 |
| `net_rx` | 3975 |
| `net_tx` | 3976 |
| `tunnel_service` | 3980 |
| `backfill_handle` | 3981 |
| `friend_events_rx` | 3985 |
| `connectivity_store` | 4169 |
| `capability_gate` | 4177 |
| `room_directory` | 4186 |
| `data_dir` | 4202 |
| `persist_tx` | 4206 |
| `image_store` | 4208 |
| `chat_history` | 4210 |
| `storage` | 4214 |
| `download_manager` | 4219 |
| `whisper_handle` | 4293 |
| `call_handle` | 4295 |
| `call_events_rx` | 4297 |
| `inbox_events_rx` | 4494 |
| `whisper_events_rx` | 4496 |
| `public_room_safety` | 4584 |
| `conversation_store` | 4587 |
| `discovered_peers_rx` | 4597 |
| `reconnect_ready_rx` | 4602 |


## 3. Message variant inventory (AppMessage variants by domain)

Every variant of `AppMessage` (lines 5691–6992) assigned to a domain. "Handled by"
is the **arm-start handler**: the module's `update_*` match that has an arm
beginning with this variant, or `app.rs (inline)` when the arm lives in the
top-level `update()`. Each variant has exactly one arm-start handler; a module may
still *produce* a variant that another module handles (e.g. chat views emit
`Task::done(AppMessage::OpenRoom(...))` while app.rs handles `OpenRoom`).

### 3.1 Summary

| Domain | Variants | Primary handler today |
|---|---|---|
| App shell / navigation | 15 | app.rs inline |
| Conversations (chat) | 88 | `app/chat.rs` + inline net-event replay |
| Files / transfers | 89 | `app/files.rs` |
| Rooms / groups / directory | 75 | app.rs inline (create/edit), `app/discover.rs`, `app/groups.rs` |
| Friends | 41 | `app/contacts.rs` |
| Calls | 14 | `app/calls.rs` |
| Screen sharing | 32 | app.rs inline (start/stop/events) + `app/chat.rs` views |
| Tunnels | 29 | `app/tunnels.rs` (+ `TunnelRequestReceived` in chat) |
| Discovery / presence / mesh | 16 | `app/discover.rs`, app.rs inline (ticks) |
| Settings | 21 | `app/settings.rs` (+ inline theme/layout reload) |
| Notifications | 1 | app.rs inline |
| Diagnostics / dev UI | 10 | app.rs inline |
| **Total** | **431** | |

### 3.2 Variant tables

#### App shell / navigation (15 variants)

| Variant | Line | Handled by |
|---|---|---|
| `GoToChatList` | 5698 | app.rs (inline) |
| `Shortcut` | 5700 | app.rs (inline) |
| `RoomOpened` | 5704 | app.rs (inline) |
| `ErrorMsg` | 6216 | app.rs (inline) |
| `DismissToast` | 6390 | app.rs (inline) |
| `SplashTick` | 6452 | app.rs (inline) |
| `WindowResized` | 6499 | app.rs (inline) |
| `Noop` | 6505 | app.rs (inline) |
| `CopyToClipboard` | 6547 | app.rs (inline) |
| `ReportBug` | 6900 | app.rs (inline) |
| `OpenUrl` | 6904 | app.rs (inline) |
| `TerminalEvent` | 6981 | settings |
| `OpenTerminal` | 6984 | settings |
| `IdleTick` | 6989 | app.rs (inline) |
| `UserActivity` | 6992 | app.rs (inline) |

#### Conversations (chat) (88 variants)

| Variant | Line | Handled by |
|---|---|---|
| `OpenRoom` | 5702 | app.rs (inline) |
| `InputChanged` | 5995 | chat |
| `SendPressed` | 5996 | chat |
| `AttachPressed` | 5997 | chat |
| `ComposerSendFinished` | 6000 | chat |
| `ComposerDragOver` | 6002 | chat |
| `ComposerFileDropped` | 6005 | chat |
| `ComposerImeActive` | 6007 | chat |
| `ToggleHelp` | 6008 | chat |
| `ToggleChatOptions` | 6010 | chat |
| `ToggleChatSearch` | 6012 | chat |
| `ChatSearchQueryChanged` | 6014 | chat |
| `ClearConversation` | 6016 | chat |
| `ToggleDetailsPanel` | 6018 | chat |
| `ToggleMemberList` | 6077 | chat |
| `NetEvent` | 6131 | chat |
| `ReplayPendingEvents` | 6134 | app.rs (inline) |
| `WhisperEvent` | 6137 | chat |
| `InboxEvent` | 6139 | chat |
| `OutboxRetryResult` | 6141 | chat |
| `RetryOutgoingMessage` | 6143 | chat |
| `MessageSent` | 6144 | chat |
| `DownloadDone` | 6146 | files |
| `DownloadFailed` | 6163 | files |
| `OpenDownloadedFile` | 6165 | files |
| `PlayInlineVideo` | 6167 | chat |
| `StreamInlineVideo` | 6171 | chat |
| `StreamingServerReady` | 6174 | chat |
| `StreamingServerFailed` | 6181 | chat |
| `InlineVideoTick` | 6186 | chat |
| `InlineVideoShowControls` | 6187 | chat |
| `InlineVideoControlsFocused` | 6191 | chat |
| `InlineVideoSeekChanged` | 6193 | chat |
| `InlineVideoSeekReleased` | 6195 | chat |
| `InlineVideoSeekRelative` | 6198 | chat |
| `InlineVideoToggleMute` | 6200 | chat |
| `InlineVideoAdjustVolume` | 6203 | chat |
| `InlineVideoSetVolume` | 6205 | chat |
| `InlineVideoToggleExpanded` | 6207 | chat |
| `CloseInlineVideo` | 6210 | chat |
| `StreamUrl` | 6212 | chat |
| `InlineVideoEvent` | 6214 | chat |
| `ExecuteFileSend` | 6217 | files |
| `AttachFolderPressed` | 6220 | chat |
| `ExecuteFolderSend` | 6223 | files |
| `ExecuteDownload` | 6224 | files |
| `ExecuteImageSend` | 6407 | files |
| `FriendAdded` | 6438 | contacts |
| `FriendRemoved` | 6443 | contacts |
| `FriendListResult` | 6448 | contacts |
| `OutboxRetryTick` | 6460 | app.rs (inline) |
| `CopyMessage` | 6549 | chat |
| `RightClickText` | 6551 | chat |
| `RightClickImage` | 6553 | chat |
| `ContextCopyText` | 6555 | chat |
| `ContextCopyImage` | 6557 | chat |
| `CloseContextMenu` | 6559 | chat |
| `ToggleVideoCardMenu` | 6561 | chat |
| `ToggleEmojiPicker` | 6563 | chat |
| `InsertEmoji` | 6569 | chat |
| `SelectEmojiCategory` | 6573 | chat |
| `EmojiSearchChanged` | 6577 | chat |
| `ToggleGifPicker` | 6579 | chat |
| `GifSearchChanged` | 6581 | chat |
| `SendGif` | 6583 | chat |
| `GifSearchSubmit` | 6585 | chat |
| `GifRetry` | 6590 | chat |
| `GifSearchDebounced` | 6592 | chat |
| `GifSearchResults` | 6594 | chat |
| `GifTrendingResults` | 6599 | chat |
| `GifSearchFailed` | 6604 | chat |
| `GifPreviewLoaded` | 6609 | chat |
| `GifLoadMore` | 6611 | chat |
| `ImageHydrated` | 6690 | files |
| `MailboxReplayed` | 6715 | chat |
| `OfflineDMStatus` | 6722 | chat |
| `Scrolled` | 6732 | chat |
| `ConnectionsResult` | 6747 | app.rs (inline) |
| `OpenConversation` | 6787 | chat |
| `SelectConversation` | 6789 | chat |
| `CloseConversation` | 6791 | chat |
| `SendMessage` | 6793 | chat |
| `ToggleInviteMenu` | 6828 | chat |
| `InviteWhisperInputChanged` | 6830 | chat |
| `InviteSendWhisper` | 6832 | chat |
| `OpenImageLightbox` | 6834 | chat |
| `CloseImageLightbox` | 6836 | chat |
| `LinkPreviewLoaded` | 6906 | chat |

#### Files / transfers (89 variants)

| Variant | Line | Handled by |
|---|---|---|
| `TransferProjectionUpdate` | 6020 | files |
| `TransferSnapshotResync` | 6023 | files |
| `DownloadingCancel` | 6025 | files |
| `DownloadingPause` | 6027 | files |
| `DownloadingResume` | 6029 | files |
| `DownloadingStop` | 6031 | files |
| `OpenDownloadManager` | 6033 | files |
| `CloseDownloadManager` | 6035 | files |
| `SharedByMeMenuToggle` | 6037 | files |
| `SharedByMeDetails` | 6039 | files |
| `SharedByMeCloseDetails` | 6041 | files |
| `SharedByMeReveal` | 6043 | files |
| `SharedByMeConfirmStopSharing` | 6045 | files |
| `SharedByMeCancelStopSharing` | 6047 | files |
| `SharedByMeRevokeAccess` | 6049 | files |
| `SharedByMeLoaded` | 6051 | files |
| `SharedByMeThumbnailReady` | 6055 | files |
| `DashboardRecentActivityLoaded` | 6060 | files |
| `DashboardSharingSummaryLoaded` | 6063 | files |
| `DashboardDownloadedRefresh` | 6065 | files |
| `DashboardDownloadedLoaded` | 6067 | files |
| `DownloadedOpen` | 6071 | files |
| `DownloadedReveal` | 6073 | files |
| `DownloadedRemoveHistory` | 6075 | files |
| `OpenFileSharing` | 6083 | app.rs (inline) |
| `DashboardSearchChanged` | 6085 | files |
| `DashboardSearchCleared` | 6088 | files |
| `DashboardSharedByMeSortClicked` | 6090 | files |
| `DashboardDownloadedSortClicked` | 6092 | files |
| `DashboardActivitySortClicked` | 6094 | files |
| `DashboardTabSelected` | 6096 | files |
| `ActivityLogLoaded` | 6098 | files |
| `ActivityLogRefresh` | 6100 | files |
| `ActivityLogFilterSelected` | 6102 | files |
| `ActivityLogPageSelected` | 6104 | files |
| `ActivityLogDetailsToggled` | 6106 | files |
| `ActivityLogClearRequested` | 6108 | files |
| `ActivityLogClearCancelled` | 6110 | files |
| `ActivityLogClearConfirmed` | 6112 | files |
| `DashboardConnectivityDismissed` | 6114 | files |
| `DashboardDownloadingRefresh` | 6116 | files |
| `CatalogueFetchFailed` | 6118 | discover |
| `CatalogueErrorDismissed` | 6120 | discover |
| `FileSent` | 6145 | files |
| `DownloadDonePeerFile` | 6149 | files |
| `PosterGenerated` | 6151 | files |
| `VideoMetadataProbed` | 6159 | files |
| `OpenDownloadsFolder` | 6215 | files |
| `ExecuteDownloadAt` | 6228 | files |
| `PauseDownloadAt` | 6230 | files |
| `ResumeDownloadAt` | 6232 | files |
| `CancelDownloadAt` | 6234 | files |
| `ReshareFile` | 6236 | files |
| `MintShortCode` | 6238 | files |
| `ShortCodeMinted` | 6241 | files |
| `CloseShortCodeDialog` | 6243 | files |
| `CopyShortCode` | 6245 | files |
| `OpenRedeemCodeDialog` | 6247 | files |
| `CloseRedeemCodeDialog` | 6249 | files |
| `RedeemCodeInputChanged` | 6251 | files |
| `RedeemShortCode` | 6253 | files |
| `ShortCodeRedeemed` | 6256 | files |
| `SetOverwritePolicy` | 6258 | files |
| `DownloadInitiated` | 6260 | files |
| `DownloadInitiationFailed` | 6269 | files |
| `ImageDownloaded` | 6408 | files |
| `GifMediaFetched` | 6430 | files |
| `AddSharedFile` | 6522 | files |
| `AddSharedFolder` | 6526 | files |
| `SharedFolderPicked` | 6528 | files |
| `SharedByMeToggleShareMenu` | 6530 | files |
| `SharedFilePicked` | 6532 | files |
| `SharedFileAdded` | 6534 | files |
| `SharedFileAddFailed` | 6536 | files |
| `RemoveSharedFile` | 6538 | files |
| `SharedFileRemoved` | 6540 | files |
| `DownloadProgress` | 6785 | files |
| `BrowsePeerCatalogue` | 6806 | discover |
| `PeerCatalogueReceived` | 6808 | discover |
| `PeerCatalogueFailed` | 6815 | discover |
| `CatalogueScrolled` | 6817 | discover |
| `RequestFileDownload` | 6819 | files |
| `ImageUploadFailed` | 6838 | files |
| `FileUploadFailed` | 6840 | files |
| `FileOfferAnnounced` | 6842 | files |
| `FileOfferCached` | 6846 | files |
| `FileOfferCacheFailed` | 6853 | files |
| `FileDownloaded` | 6857 | files |
| `ThumbnailFetched` | 6865 | files |

#### Rooms / groups / directory (75 variants)

| Variant | Line | Handled by |
|---|---|---|
| `CreateNewRoom` | 5726 | app.rs (inline) |
| `ConfirmCreateNewRoom` | 5728 | app.rs (inline) |
| `CancelCreateRoom` | 5730 | app.rs (inline) |
| `CreateNewRoomDhtToggled` | 5732 | app.rs (inline) |
| `CreateNewRoomNameChanged` | 5734 | app.rs (inline) |
| `CreateNewRoomVisibilityChanged` | 5736 | app.rs (inline) |
| `CreateNewRoomDescriptionChanged` | 5738 | app.rs (inline) |
| `CreateNewRoomTagsChanged` | 5740 | app.rs (inline) |
| `OpenRoomSettings` | 5744 | app.rs (inline) |
| `RoomSettingsNameChanged` | 5746 | app.rs (inline) |
| `RoomSettingsDescriptionChanged` | 5748 | app.rs (inline) |
| `RoomSettingsTagsChanged` | 5750 | app.rs (inline) |
| `RoomSettingsVisibilityChanged` | 5752 | app.rs (inline) |
| `ConfirmRoomSettings` | 5755 | app.rs (inline) |
| `CancelRoomSettings` | 5757 | app.rs (inline) |
| `SetRoomDirectoryVisibility` | 5763 | app.rs (inline) |
| `JoinFromTicket` | 5770 | app.rs (inline) |
| `RoomJoinFailed` | 5772 | app.rs (inline) |
| `ShowCreateGroupDialog` | 5787 | groups |
| `HideCreateGroupDialog` | 5789 | groups |
| `CreateGroupNameChanged` | 5791 | groups |
| `CreateGroupDescriptionChanged` | 5793 | groups |
| `CreateGroupMemberToggled` | 5795 | groups |
| `CreateGroupSearchChanged` | 5797 | groups |
| `ConfirmCreateGroup` | 5799 | groups |
| `GroupCreated` | 5801 | groups |
| `JoinTicketInputChanged` | 5836 | home |
| `NewChatCreated` | 5837 | app.rs (inline) |
| `RoomSelected` | 5838 | app.rs (inline) |
| `OpenGroupChat` | 5840 | app.rs (inline) |
| `ToggleSidebarSectionCollapsed` | 6122 | app.rs (inline) |
| `ShowInviteMemberDialog` | 6372 | groups |
| `HideInviteMemberDialog` | 6374 | groups |
| `InviteMemberToggled` | 6376 | groups |
| `ConfirmInviteMember` | 6378 | groups |
| `AcceptGroupInvite` | 6380 | groups |
| `DeleteRoom` | 6450 | chat |
| `ActivityTick` | 6454 | app.rs (inline) |
| `CopyShareTicket` | 6618 | files |
| `OpenReceiveTicketDialog` | 6621 | files |
| `CloseReceiveTicketDialog` | 6623 | files |
| `ReceiveTicketInputChanged` | 6625 | files |
| `ReceiveTicketPreflight` | 6627 | files |
| `ReceiveTicketPreflightDone` | 6630 | files |
| `ConfirmReceiveTicket` | 6633 | files |
| `PickHomeBackgroundImage` | 6650 | settings |
| `HomeBackgroundImagePicked` | 6653 | settings |
| `HomeBackgroundImageReady` | 6656 | settings |
| `RemoveHomeBackgroundImage` | 6661 | settings |
| `SetHomeMenuItemOpacity` | 6664 | settings |
| `SystemMsg` | 6680 | app.rs (inline) |
| `ClearHistoryRequested` | 6696 | chat |
| `ConfirmClearHistory` | 6698 | chat |
| `ClearHistoryFinished` | 6700 | chat |
| `ClearHistoryFailed` | 6706 | chat |
| `DeleteRoomRequested` | 6711 | chat |
| `ConfirmDeleteRoom` | 6713 | chat |
| `ToggleAdvertiseRoom` | 6913 | discover |
| `OpenDirectory` | 6919 | discover |
| `CloseDiscover` | 6922 | discover |
| `DiscoverSearchChanged` | 6926 | discover |
| `DiscoverFilterToggled` | 6929 | discover |
| `DiscoverTagToggled` | 6931 | discover |
| `DiscoverSortChanged` | 6933 | discover |
| `DiscoverClearFilters` | 6935 | discover |
| `OpenGroups` | 6937 | groups |
| `CloseGroups` | 6939 | groups |
| `DirectoryRoomJoin` | 6941 | discover |
| `DirectoryRoomJoinById` | 6949 | discover |
| `DirectoryRoomHideById` | 6956 | discover |
| `DirectoryRoomUnhideById` | 6961 | discover |
| `DirectoryRoomUnhideAll` | 6965 | discover |
| `DeleteDirectoryRoom` | 6967 | discover |
| `DirectoryRoomUpdate` | 6969 | discover |
| `DirectoryRoomWithdrawal` | 6976 | app.rs (inline) |

#### Friends (41 variants)

| Variant | Line | Handled by |
|---|---|---|
| `ImportFriendFromFile` | 5782 | contacts |
| `ImportFriendFromFilePicked` | 5784 | contacts |
| `OpenFriendRequests` | 6081 | contacts |
| `CloseFriendRequests` | 6123 | contacts |
| `FriendRequestSearchChanged` | 6124 | contacts |
| `FriendRequestSend` | 6125 | contacts |
| `FriendRequestAccept` | 6126 | contacts |
| `FriendRequestDecline` | 6127 | contacts |
| `FriendRequestCancel` | 6128 | contacts |
| `FriendRequestSentResult` | 6129 | contacts |
| `FriendRequestActionResult` | 6130 | contacts |
| `FriendEvent` | 6135 | contacts |
| `OpenPeerProfile` | 6278 | contacts |
| `OpenFriendProfile` | 6280 | contacts |
| `CloseFriendProfile` | 6282 | contacts |
| `ToggleFriendProfileMenu` | 6284 | contacts |
| `FriendRenameInputChanged` | 6361 | contacts |
| `FriendRenameConfirm` | 6363 | contacts |
| `CopyPeerId` | 6365 | contacts |
| `ShowRemoveFriendConfirm` | 6392 | contacts |
| `CancelRemoveFriend` | 6394 | contacts |
| `ConfirmRemoveFriend` | 6396 | contacts |
| `ShowBlockFriendConfirm` | 6398 | contacts |
| `ShowRenameFriendInput` | 6400 | contacts |
| `CancelBlockFriend` | 6402 | contacts |
| `ConfirmBlockFriend` | 6404 | contacts |
| `ClosePeerProfile` | 6406 | contacts |
| `RemoveFriend` | 6447 | contacts |
| `CopyFriendId` | 6613 | contacts |
| `FriendIdCopiedClear` | 6615 | contacts |
| `OpenFriendChat` | 6635 | contacts |
| `ProfileImageDownloaded` | 6682 | files |
| `ProfileImageDownloadFailed` | 6685 | files |
| `SendFriendRequest` | 6749 | contacts |
| `FriendRequestSent` | 6751 | contacts |
| `FriendRequestFailed` | 6756 | contacts |
| `FriendRequestReceived` | 6761 | contacts |
| `FriendRequestRetry` | 6767 | contacts |
| `IncomingFriendRequestAccept` | 6769 | contacts |
| `IncomingFriendRequestDecline` | 6774 | contacts |
| `IncomingFriendRequestProcessed` | 6779 | contacts |

#### Calls (14 variants)

| Variant | Line | Handled by |
|---|---|---|
| `StartVoiceCall` | 5844 | calls |
| `StartVideoCall` | 5845 | calls |
| `CallEventReceived` | 5981 | app.rs (inline) |
| `AcceptIncomingCall` | 5984 | calls |
| `RejectIncomingCall` | 5985 | calls |
| `HangUp` | 5986 | calls |
| `ToggleCallMute` | 5987 | calls |
| `ToggleCallCamera` | 5988 | calls |
| `SelectMicrophone` | 5989 | calls |
| `SelectSpeaker` | 5990 | calls |
| `SelectCamera` | 5991 | calls |
| `CallUiTick` | 5992 | calls |
| `CallStarted` | 5993 | calls |
| `CallCommandFinished` | 5994 | calls |

#### Screen sharing (32 variants)

| Variant | Line | Handled by |
|---|---|---|
| `StartScreenShare` | 5848 | app.rs (inline) |
| `StopScreenShare` | 5851 | app.rs (inline) |
| `AcceptScreenShare` | 5854 | app.rs (inline) |
| `DeclineScreenShare` | 5857 | app.rs (inline) |
| `ToggleScreenShareFullscreen` | 5860 | app.rs (inline) |
| `ScreenShareEventReceived` | 5863 | app.rs (inline) |
| `ScreenShareFrameReceived` | 5866 | app.rs (inline) |
| `ScreenShareStatsReceived` | 5870 | app.rs (inline) |
| `ScreenShareCommandFinished` | 5873 | app.rs (inline) |
| `ScreenShareRequestControl` | 5876 | app.rs (inline) |
| `ScreenShareRequestClipboard` | 5880 | app.rs (inline) |
| `ScreenShareSendClipboard` | 5883 | app.rs (inline) |
| `ScreenShareHostSendClipboard` | 5886 | app.rs (inline) |
| `ScreenShareClipboardRead` | 5889 | app.rs (inline) |
| `ScreenShareGrantControl` | 5892 | app.rs (inline) |
| `ScreenShareDenyControl` | 5895 | app.rs (inline) |
| `ScreenShareToggleAudio` | 5901 | app.rs (inline) |
| `ScreenShareRevokeControl` | 5904 | app.rs (inline) |
| `ScreenShareLowerQuality` | 5908 | app.rs (inline) |
| `ScreenShareFullQuality` | 5911 | app.rs (inline) |
| `ScreenShareSelectSource` | 5918 | app.rs (inline) |
| `ScreenShareSetPreset` | 5923 | app.rs (inline) |
| `ScreenShareDismissNotice` | 5926 | app.rs (inline) |
| `ScreenSharePointerMove` | 5929 | app.rs (inline) |
| `ScreenSharePointerButton` | 5935 | app.rs (inline) |
| `ScreenShareKeyEvent` | 5943 | app.rs (inline) |
| `ScreenShareWheel` | 5950 | app.rs (inline) |
| `ScreenShareSetView` | 5958 | app.rs (inline) |
| `ScreenSharePanStart` | 5964 | app.rs (inline) |
| `ScreenSharePanMove` | 5970 | app.rs (inline) |
| `ScreenSharePanEnd` | 5976 | app.rs (inline) |
| `ToggleScreenShareCursor` | 5979 | app.rs (inline) |

#### Tunnels (29 variants)

| Variant | Line | Handled by |
|---|---|---|
| `ShowCreateTunnelDialog` | 5817 | tunnels |
| `CreateTunnelPortChanged` | 5819 | tunnels |
| `CreateTunnel` | 5821 | tunnels |
| `CancelCreateTunnel` | 5823 | tunnels |
| `TunnelRequestReceived` | 5825 | tunnels |
| `AcceptTunnelRequest` | 5830 | tunnels |
| `DeclineTunnelRequest` | 5832 | tunnels |
| `CloseTunnel` | 5834 | tunnels |
| `OpenShareLocalService` | 6286 | tunnels |
| `OpenShareVncTunnel` | 6288 | tunnels |
| `ShareLocalServiceNameChanged` | 6290 | tunnels |
| `ShareLocalServicePortChanged` | 6292 | tunnels |
| `ShareLocalServiceExpiryChanged` | 6294 | tunnels |
| `ConfirmShareLocalService` | 6296 | tunnels |
| `CancelShareLocalService` | 6298 | tunnels |
| `ShareLocalServiceScanDone` | 6300 | tunnels |
| `SelectShareLocalServiceSuggestion` | 6302 | tunnels |
| `TunnelShared` | 6304 | tunnels |
| `TunnelShareFailed` | 6313 | tunnels |
| `TunnelOfferSent` | 6317 | tunnels |
| `TunnelOfferSendFailed` | 6319 | tunnels |
| `ShareLocalServiceHttpToggled` | 6325 | tunnels |
| `ConnectReceivedTunnel` | 6328 | tunnels |
| `ReceivedTunnelConnected` | 6330 | tunnels |
| `ReceivedTunnelConnectFailed` | 6346 | tunnels |
| `DisconnectReceivedTunnel` | 6353 | tunnels |
| `StopSharingTunnel` | 6355 | tunnels |
| `OpenReceivedTunnel` | 6357 | tunnels |
| `CopyReceivedTunnelAddress` | 6359 | tunnels |

#### Discovery / presence / mesh (16 variants)

| Variant | Line | Handled by |
|---|---|---|
| `OpenConnectionDetails` | 6367 | app.rs (inline) |
| `CloseConnectionDetails` | 6369 | app.rs (inline) |
| `CopyConnectionDetails` | 6383 | app.rs (inline) |
| `CopyConnectionDetailsValue` | 6385 | app.rs (inline) |
| `ConnMonitorTick` | 6456 | app.rs (inline) |
| `MeshWatchdogTick` | 6458 | app.rs (inline) |
| `ConnCountsResult` | 6734 | app.rs (inline) |
| `TicketPeersResolved` | 6742 | app.rs (inline) |
| `NewDiscoveredPeers` | 6798 | discover |
| `ReconnectPeerReady` | 6802 | discover |
| `RetryConnection` | 6886 | app.rs (inline) |
| `BackgroundSubscribe` | 6888 | discover |
| `BackgroundSubscribed` | 6892 | discover |
| `SubscribeStoredConversations` | 6909 | discover |
| `SubscribeDirectoryTopic` | 6915 | discover |
| `DirectorySubscribed` | 6917 | discover |

#### Settings (21 variants)

| Variant | Line | Handled by |
|---|---|---|
| `OpenSettings` | 6078 | settings |
| `CloseSettings` | 6079 | settings |
| `ToggleDark` | 6463 | settings |
| `UiThemeReloaded` | 6469 | app.rs (inline) |
| `LayoutReloaded` | 6479 | app.rs (inline) |
| `ToggleAccentColorPicker` | 6489 | settings |
| `AccentColorSelected` | 6491 | settings |
| `AccentColorCancelled` | 6493 | settings |
| `SetNickname` | 6495 | settings |
| `SaveProfile` | 6543 | app.rs (inline) |
| `ProfileSaved` | 6545 | app.rs (inline) |
| `ToggleSound` | 6637 | settings |
| `TogglePresenceIndicator` | 6640 | settings |
| `ToggleInviteAddressSharing` | 6642 | settings |
| `SetChatTextSize` | 6644 | settings |
| `PickProfileImage` | 6646 | settings |
| `ProfileImagePicked` | 6648 | settings |
| `ProfileImageUploaded` | 6667 | settings |
| `RemoveProfileImage` | 6669 | settings |
| `ProfileImageRemoved` | 6671 | settings |
| `ProfileImagePersisted` | 6675 | settings |

#### Notifications (1 variants)

| Variant | Line | Handled by |
|---|---|---|
| `WindowFocusChanged` | 5983 | app.rs (inline) |

#### Diagnostics / dev UI (10 variants)

| Variant | Line | Handled by |
|---|---|---|
| `Designer` | 5695 | app.rs (inline) |
| `Inspector` | 6487 | app.rs (inline) |
| `ToggleGallery` | 6509 | app.rs (inline) |
| `GalleryPreset` | 6512 | app.rs (inline) |
| `GalleryCustomWidth` | 6515 | app.rs (inline) |
| `GalleryLayoutPreset` | 6518 | app.rs (inline) |
| `GuiTestActionReceived` | 6872 | app.rs (inline) |
| `GuiActionTimeout` | 6875 | app.rs (inline) |
| `GuiTestWaitSatisfied` | 6877 | app.rs (inline) |
| `GuiTestWaitTimedOut` | 6879 | app.rs (inline) |


## 4. Update branches and view functions by domain

### 4.1 update() structure

`pub fn update()` (12786–18043) is a single `match message` (12793–18027) with 164
arm groups covering all 431 variants. Arms fall into three styles:

1. **Inline handlers** — the arm body directly mutates `self`. 100 variants are
   arm-start handled inline in app.rs: shell/nav/lifecycle (`GoToChatList`,
   `Shortcut`, `RoomOpened`, `OpenRoom`, `SplashTick`, `IdleTick`, `UserActivity`,
   `WindowResized`), screen share (all 32 `ScreenShare*`), create/edit room flow
   (`CreateNewRoom*`, `RoomSettings*`, `SetRoomDirectoryVisibility`),
   `JoinFromTicket`, connection details, GUI-test/ticks, theme/layout reload,
   save-profile, and the global error/toast surface.
2. **Delegation to module `update_*`** — the arm forwards a group of variants to
   `self.update_chat(message)` / `update_files` / `update_contacts` /
   `update_discover` / `update_groups` / `update_settings` / `update_tunnels` /
   `update_calls` / `update_home`. 331 variants are handled by the 9 module
   delegates (counts below).
3. **Short-circuit routing** — `AppMessage::RoomSelected(topic) => Task::done(OpenRoom(topic))`
   and similar one-liners.

Module delegates and the variants they handle (arm-start counts):

| Module | `update_*` fn | Variants handled | Domain |
|---|---|---|---|
| `app/files.rs` | `update_files` | 99 | Files |
| `app/chat.rs` | `update_chat` | 80 | Conversations |
| `app/contacts.rs` | `update_contacts` | 42 | Friends |
| `app/tunnels.rs` | `update_tunnels` | 29 | Tunnels |
| `app/discover.rs` | `update_discover` | 28 | Rooms (browse/directory) + Discovery |
| `app/settings.rs` | `update_settings` | 24 | Settings |
| `app/groups.rs` | `update_groups` | 15 | Rooms (groups) |
| `app/calls.rs` | `update_calls` | 13 | Calls |
| `app/home.rs` | `update_home` | 1 | Rooms (chat-list) |

Because arm-start handlers are unique, there is no variant that two modules both
*handle*. The cross-domain surface is in the opposite direction: a module **produces**
variants that another module (or the shell) handles. Notable production edges:

- Chat views/files dashboard produce file actions (`ExecuteFileSend`,
  `ExecuteFolderSend`, `ExecuteImageSend`, `ExecuteDownload`,
  `ExecuteDownloadAt`, `PauseDownloadAt`, `ResumeDownloadAt`, `CancelDownloadAt`)
  that `app/files.rs` handles.
- `chat.rs`/`discover.rs`/`home.rs` produce `OpenRoom` (handled inline in app.rs).
- `groups.rs` produces `JoinFromTicket` (handled inline in app.rs); `contacts.rs`
  produces `OpenFriendChat` (handled in `contacts.rs` itself).
- Every module may produce `ErrorMsg` / `Noop` (handled inline in app.rs).

These production edges are exactly the messages that need typed routing in
BORU-APP-002 before extraction: the shell should route a produced command to the
owning domain without the producing module knowing the handler.

### 4.2 view() structure

`pub fn view()` (21400–21733) is a screen router: it renders the sidebar
(`view_sidebar()` from `app/sidebar.rs`) then `match &self.screen` selects the main
panel:

| Screen | View fn | Module |
|---|---|---|
| `ChatList` | `view_main_empty_state` | app/home.rs |
| `FileSharing` | `view_file_sharing` (prewarmed) | app/files.rs |
| `DownloadManager` | `view_download_manager` | app/files.rs |
| `Chat { .. }` | `view_chat_panel` | app/chat.rs |
| `OutgoingCall` / `ActiveCall` | `view_outgoing_call` / `view_active_call` | app/calls.rs |
| `FriendRequests` | `view_friend_requests` (prewarmed) | app/contacts.rs |
| `Settings` | `view_settings_screen` (prewarmed) | app/settings.rs |
| `PeerProfile` / `PeerCatalogue` | `view_peer_profile` / `view_peer_catalogue` | app/discover.rs |
| `FriendProfile` | `view_friend_profile` | app/discover.rs |
| `Discover` | `view_discover` (prewarmed) | app/discover.rs |
| `Groups` | `view_groups_screen` (prewarmed) | app/sidebar.rs |
| `Terminal` | `term.view()` (feature `terminal`) | main.rs terminal_view |
| `Gallery` (dev-ui) | `view_gallery_with_designer` | main.rs component_gallery |

The view-layer split is already domain-aligned at the screen level; what is missing
is state ownership (all fields still live on `IcedChat`), which §6 addresses.

## 5. Lifecycle code that must remain top-level

Per ARCH-002 §3.1 and the PDF (startup/shutdown, route switching, global error
surface), the following stays in the shell — it is *not* a candidate for domain
extraction:

- **Startup / construction** — `main.rs::main()` (line 550): CLI parse, data-dir
  migration, i18n init, logging, theme/dev-ui config load, router+endpoint+store
  wiring, `IcedChat::new()` (app.rs 7968). The shared handles created here feed
  every domain (ARCH-002 §4 read-only context).
- **Subscription wiring** — `subscription()` (23393–23540): builds the combined
  stream of receiver channels (net, friend, whisper, inbox, discovered-peers,
  reconnect-ready, GUI-action, transfer, ui-theme, layout, call, screen-share) and
  the timer ticks. It must stay the shell's because it maps external channels to
  `AppMessage` and hands them to `update()`.
- **Top-level route switching** — `view()` screen router (§4.2) and the navigation
  messages (`GoToChatList`, `OpenRoom`, `RoomSelected`, `OpenGroupChat`,
  `OpenFileSharing`, `OpenSettings`, `OpenFriendRequests`, `*_return_to` pointers,
  `WindowResized`).
- **Global error surface** — `notice` field, `ErrorMsg` variant, `DismissToast`,
  toast state.
- **Splash / idle / prewarm** — `SplashTick`, `IdleTick`, `UserActivity`,
  `prewarm_*` fields, `pre_warm_next_screen`/`serve_prewarmed`/`invalidate_prewarm`
  (after line 23540).
- **Ticks that coordinate across domains** — `ConnMonitorTick`, `MeshWatchdogTick`,
  `OutboxRetryTick`, `CallUiTick`, `ActivityTick` are pure timers; their handlers
  may be *thin* while the domain work moves, but the tick plumbing stays in the
  shell.
- **Shutdown** — `shutdown_shared` helper (app.rs ~line 3778 zone) and
  `request_quit` (view-helpers zone).
- **Feature-gate ownership** — `dev-ui` (`designer`, `inspector`, `gallery`) and
  `terminal` (`terminal`, `OpenTerminal`/`TerminalEvent`) are shell-level features,
  not product domains.

## 6. Dependency map between domains

Edges below are *current* data-flow / call edges observed in app.rs; the target
ownership matrix in ARCH-002 §5 is the destination. Read "→" as "reads/triggers".

```
shell (navigation, subscription, prewarm, error surface)
  │ routes AppMessage to every domain; owns net-event → navigation mapping
  ▼
chat ──────► files   (ExecuteFileSend/DownloadDone/DownloadFailed/OpenDownloadedFile)
  │          rooms   (OpenRoom, DeleteRoom, ClearHistory)
  │          friends (names, DM topics, friend image)
  │          discovery (neighbor/presence events via NetEvent)
  │          notifications (emit_message_notification)
  ▼
files ─────► chat    (download cards rendered in chat; ImageHydrated)
  │          rooms   (ReceiveTicket/ConfirmReceiveTicket for room join)
  │          notifications (transfer completion)
  ▼
rooms ─────► chat    (RoomOpened/JoinFromTicket lands in conversation)
  │          discovery (SubscribeDirectoryTopic, advertise)
  │          notifications (room events)
  ▼
friends ───► rooms   (OpenFriendChat, peer catalogue)
  │          chat    (whisper events, DM subscription)
  │          discovery (presence broadcasts)
  ▼
calls ─────► friends (auth), settings (device prefs), shell (screen transition)
screen_share ► friends (capability), calls (media), shell (composition)
tunnels ───► friends (peer picker), settings (prefs), shell (navigation)
discovery ─► rooms (directory sync), friends (presence), shell (read handles)
settings ──► all (read-only config context)
notifications ► settings (prefs), shell (focus)
```

Key observations for extraction:

1. **Chat is the hub** — the conversation map + active-chat projection touches
   every other domain. It must be extracted last or with the shared
   conversation-state convergence from ARCH-002 §3.2; moving it first would drag
   the whole dependency web along.
2. **Files is already nearly self-contained** — 61 fields / 89 variants are handled
   by `app/files.rs` (99 arm-start variants) and the dashboard views; its
   cross-domain edges are narrow (chat download cards, receive-ticket). Best first
   candidate.
3. **Tunnels is the smallest true domain** — 17 fields / 29 variants, one module,
   edges only to friends/settings/shell.
4. **Calls is small and isolated** — 13 fields / 14 variants, one module; edges to
   friends/settings/shell.
5. **Screen sharing is state-heavy but mostly inline** — 44 fields / 32 variants
   handled directly in app.rs `update()`; `app/chat.rs` already renders the
   in-chat share panel, so extraction means moving the inline arms + 44 fields into
   a `ScreenShare` domain (or folding into the existing `screen_share_*` modules).

## 7. Recommended first three low-risk extraction targets

Consistent with ARCH-002 boundaries and the PDF's own PR order (APP-003 settings,
APP-004 notifications, APP-005 files), but re-ranked by measured cohesion:

**1. Tunnels (BORU-APP-009 early) — lowest risk, self-contained.**
- 17 fields + 29 variants + `app/tunnels.rs` `update_tunnels` + `view_share_local_service_dialog`.
- Cross-domain edges: friends (peer picker), settings (read), shell (navigation).
- No chat/files state is touched by any tunnel arm.
- Test hooks already exist (`vr_create_tunnel_*` in app.rs tests).

**2. Files (BORU-APP-005) — largest single-domain win, already module-shaped.**
- 61 fields + 89 variants + `app/files.rs` `update_files` (99 arm-start variants)
  + 13 view fns.
- The dashboard / download / activity-log state is cohesive; the only leaky edges
  are the chat-side download cards (`ExecuteFileSend`, `DownloadDone`,
  `DownloadFailed`, `OpenDownloadedFile`, `ExecuteDownload`, `ExecuteImageSend`,
  `ExecuteFolderSend`) and `ImageHydrated`. These become typed commands from Chat →
  Files in BORU-APP-002's routing pattern.
- Moving Files first removes the largest field block (~13% of IcedChat) from app.rs.

**3. Calls (BORU-APP-008 partial) — small, isolated, low blast radius.**
- 13 fields + 14 variants + `app/calls.rs` `update_calls` + `view_outgoing_call`/
  `view_active_call`.
- Edges: friends (auth), settings (device prefs), shell (call_return_screen).
- The incoming-call overlay lives in `app/dialogs.rs` and stays a shell-composed
  dialog; the call *state* moves into the domain.

Deliberately **not** first: Chat (hub, needs the ARCH-002 §3.2 state convergence),
Screen sharing (44 fields handled inline, entangled with chat views), Rooms
(create-room + sidebar caches are spread across app.rs inline and 3 modules),
Discovery (needs the BORU-CP-17 test fix from baseline §3.2 first).

## 8. How this inventory feeds the next tasks

- **BORU-APP-002** should pick the routing pattern on the production-edge list (§4.1)
  first (`Execute*Send`, `OpenRoom`, `JoinFromTicket`, `ErrorMsg`, `Noop`) — these
  define the typed-command surface.
- **BORU-APP-003..010** each take one domain from §2/§3 tables as the "state +
  messages + update logic" bundle to move, per ARCH-002 §6 rules (move, don't copy;
  keep AppMessage; compile with default features + relevant features; tests first).
- Stop conditions (PDF §14) apply unchanged; this inventory itself changes no bytes.
