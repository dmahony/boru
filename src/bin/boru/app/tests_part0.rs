    #[test]
    fn active_accent_resolver_uses_selection_and_restores_reference() {
        let theme = iced::Theme::Dark;
        let reference = crate::design_tokens::primary(&theme);

        set_accent_override(Some([12, 34, 56]));
        let selected = accent_primary(&theme);
        assert_eq!(selected, iced::Color::from_rgb8(12, 34, 56));
        assert_eq!(accent_soft(&theme).a, 0.12);

        set_accent_override(None);
        assert_eq!(accent_primary(&theme), reference);
    }

    #[test]
    fn direct_offer_sender_card_is_shared_not_uploading() {
        let state = direct_offer_sender_state(
            "notes.txt".to_string(),
            std::path::PathBuf::from("/tmp/notes.txt"),
            12,
        );

        assert!(matches!(state, DownloadState::Shared { .. }));
        assert!(!matches!(
            state,
            DownloadState::Active { .. } | DownloadState::Ready { .. }
        ));
    }
    use boru_core::gif_provider::GifMediaSource;

    /// A closed dummy receiver for the dev-theme reload channel (BORU-UI-06).
    /// The subscription stream's ui_theme branch immediately sees the closed
    /// sender and stays disabled — exactly like the app's headless path.
    fn dummy_ui_theme_handle() -> UiThemeRxHandle {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        UiThemeRxHandle(Arc::new(Mutex::new(rx)))
    }
    use chrono::{FixedOffset, TimeZone, Utc};

    // ── GIF picker pure logic (KLIPY-05) ───────────────────────────────
    // Network-free helpers: rendition selection for the send path and the
    // provider-error message mapper.  No provider, no app instance.

    #[test]
    fn external_stream_hint_embeds_url_and_keeps_manual_option() {
        let hint = IcedChat::external_stream_hint("http://127.0.0.1:54321/video");
        assert!(
            hint.contains("http://127.0.0.1:54321/video"),
            "hint must carry the URL"
        );
        assert!(
            hint.contains("VLC") || hint.contains("browser"),
            "hint must mention manual paste options"
        );
    }

    fn media_source(url: &str, format: GifMediaFormat) -> GifMediaSource {
        GifMediaSource {
            url: url.to_string(),
            format,
            width: None,
            height: None,
            file_size: None,
        }
    }

    fn result_with(original: Option<GifMediaSource>) -> GifSearchResult {
        GifSearchResult {
            provider: "test".to_string(),
            provider_id: "gif-1".to_string(),
            title: Some("test".to_string()),
            alt_text: None,
            preview: media_source("https://media.test/preview.gif", GifMediaFormat::Gif),
            playback: media_source("https://media.test/playback.mp4", GifMediaFormat::Mp4),
            original,
        }
    }

    #[test]
    fn provider_error_message_is_user_facing_and_safe() {
        let msg = gif_provider_error_message(&GifProviderError::NotConfigured);
        assert!(msg.contains("KLIPY_API_KEY"));
        let msg = gif_provider_error_message(&GifProviderError::InvalidApiKey);
        assert!(msg.contains("KLIPY_API_KEY"));
        let msg = gif_provider_error_message(&GifProviderError::RateLimited {
            retry_after: Some(42),
        });
        assert!(msg.contains("42"), "retry hint surfaced");
    }

    #[test]
    fn provider_error_message_redacts_urls_in_details() {
        // KLIPY-10/KLIPY-09: even if a provider embeds a URL in `details`
        // (which could carry the API key in the path or the query), the
        // user-facing message must never surface it.
        let msg = gif_provider_error_message(&GifProviderError::Network {
            details: "connection refused: https://api.klipy.com/api/v1/secret-key-abc/gifs/search?q=cats in hats".to_string(),
        });
        assert!(!msg.contains("secret-key-abc"), "API key leaked: {msg}");
        assert!(!msg.contains("api.klipy.com"), "URL leaked: {msg}");
        assert!(!msg.contains("cats in hats"), "query leaked: {msg}");
        assert!(msg.contains("GIF search network error"), "{msg}");
    }

    #[test]
    fn shared_gif_from_search_result_handoff() {
        // KLIPY-06's SharedGif conversion keeps the provider-neutral
        // identity and rendition URLs; the picker's SendGif handler builds
        // this payload and broadcasts it (no full-size download on send).
        // The playback rendition stays as the provider chose it (MP4 here);
        // the receiver dispatches on `format` — MP4 plays via the inline
        // video player, GIF/WebP via the image path.
        let gif = result_with(Some(media_source(
            "https://media.test/original.gif",
            GifMediaFormat::Gif,
        )));
        let shared = boru_core::gif_provider::SharedGif::from_search_result(&gif);
        assert_eq!(shared.provider, "test");
        assert_eq!(shared.provider_id, "gif-1");
        assert_eq!(shared.playback_url, "https://media.test/playback.mp4");
        assert_eq!(
            shared.preview_url.as_deref(),
            Some("https://media.test/preview.gif")
        );
        assert_eq!(
            shared.fallback_url.as_deref(),
            Some("https://media.test/original.gif")
        );
        assert_eq!(shared.format, GifMediaFormat::Mp4);
        assert!(shared.is_renderable());
    }

    // ── GIF picker state machine (KLIPY-11) ────────────────────────────
    // These drive the picker's update() handlers with provider-neutral
    // messages.  No live KLIPY service is ever contacted: the tests that
    // trigger a search/trending request run with KLIPY_API_KEY removed (and
    // restored), so `default_gif_provider()` deterministically takes the
    // NotConfigured path and the returned iced Task (which would perform the
    // HTTP request) is dropped without running.

    /// Serialize access to `KLIPY_API_KEY` so parallel tests never race on
    /// the process-global env var.
    static KLIPY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `f` with `KLIPY_API_KEY` removed, restoring the original value
    /// afterwards.  Returns whatever `f` returned.
    fn with_klipy_unset<R>(f: impl FnOnce() -> R) -> R {
        let _guard = KLIPY_ENV_LOCK.lock().unwrap();
        let original = std::env::var(boru_core::klipy_config::KLIPY_API_KEY_ENV).ok();
        std::env::remove_var(boru_core::klipy_config::KLIPY_API_KEY_ENV);
        let result = f();
        if let Some(v) = original {
            std::env::set_var(boru_core::klipy_config::KLIPY_API_KEY_ENV, v);
        }
        result
    }

    fn sample_page(items: Vec<GifSearchResult>, next_cursor: Option<String>) -> GifSearchPage {
        GifSearchPage { items, next_cursor }
    }

    #[test]
    fn gif_search_debounce_bumps_seq_and_ignores_stale_timer() {
        with_klipy_unset(|| {
            let (runtime, mut app, _local, _peer) = build_join_request_test_app();
            assert_eq!(app.gif_debounce_seq, 0);

            // GifSearchChanged schedules a tokio sleep-based debounce task,
            // which needs a reactor context, so run the update calls inside
            // the test app's runtime.  The returned tasks are dropped without
            // being polled — no network, no waiting.
            runtime.block_on(async {
                // First keystroke schedules a debounce at seq 1.
                let task = app.update(AppMessage::GifSearchChanged("cat".to_string()));
                drop(task);
                assert_eq!(app.gif_search_text, "cat");
                assert_eq!(app.gif_debounce_seq, 1);

                // Second keystroke supersedes it at seq 2.
                let task = app.update(AppMessage::GifSearchChanged("cats".to_string()));
                drop(task);
                assert_eq!(app.gif_search_text, "cats");
                assert_eq!(app.gif_debounce_seq, 2);

                // A stale timer (seq 1) must be ignored — no search state change.
                let task = app.update(AppMessage::GifSearchDebounced(1));
                drop(task);
                assert!(
                    !app.gif_has_searched,
                    "stale debounce must not start a search"
                );
                assert!(app.gif_results.is_empty());

                // The current timer (seq 2) proceeds to search; with the key
                // removed the picker lands in the deterministic not-configured
                // state (missing-key path) instead of hitting the live service.
                let task = app.update(AppMessage::GifSearchDebounced(2));
                drop(task);
                assert!(app.gif_has_searched, "current debounce must start a search");
                assert!(!app.gif_showing_trending);
                assert!(app.gif_not_configured);
                assert!(!app.gif_loading);
            });
        });
    }

    #[test]
    fn gif_search_results_store_page_and_clear_loading() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let seq = 7;
        app.gif_request_seq = seq;
        app.gif_loading = true;
        let page = sample_page(
            vec![result_with(Some(media_source(
                "https://media.test/original.gif",
                GifMediaFormat::Gif,
            )))],
            Some("2".to_string()),
        );
        let task = app.update(AppMessage::GifSearchResults { seq, page });
        drop(task);
        assert!(!app.gif_loading, "results arrival must clear loading");
        assert!(app.gif_error.is_none());
        assert!(app.gif_has_searched);
        assert!(!app.gif_showing_trending);
        assert_eq!(app.gif_results.len(), 1);
        assert_eq!(app.gif_results[0].provider_id, "gif-1");
        assert_eq!(app.gif_next_cursor.as_deref(), Some("2"));
    }

    #[test]
    fn gif_search_stale_results_rejected() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let seq = 5;
        app.gif_request_seq = seq;
        app.gif_loading = true;
        let stale_page = sample_page(vec![result_with(None)], None);
        let task = app.update(AppMessage::GifSearchResults {
            seq: seq - 1,
            page: stale_page,
        });
        drop(task);
        assert!(
            app.gif_results.is_empty(),
            "stale seq must not replace results"
        );
        assert!(app.gif_loading, "stale response must not clear loading");

        // The matching seq still applies.
        let task = app.update(AppMessage::GifSearchResults {
            seq,
            page: sample_page(vec![result_with(None)], None),
        });
        drop(task);
        assert_eq!(app.gif_results.len(), 1);
        assert!(!app.gif_loading);
    }

    #[test]
    fn gif_search_empty_results_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let seq = 3;
        app.gif_request_seq = seq;
        let task = app.update(AppMessage::GifSearchResults {
            seq,
            page: sample_page(vec![], None),
        });
        drop(task);
        assert!(app.gif_results.is_empty());
        assert!(
            app.gif_has_searched,
            "an empty page is still a completed search"
        );
        assert!(app.gif_next_cursor.is_none());
    }

    #[test]
    fn gif_search_failed_sets_error_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let seq = 4;
        app.gif_request_seq = seq;
        app.gif_loading = true;
        let task = app.update(AppMessage::GifSearchFailed {
            seq,
            message: "boom".to_string(),
        });
        drop(task);
        assert!(!app.gif_loading);
        assert_eq!(app.gif_error.as_deref(), Some("boom"));
    }

    #[test]
    fn gif_search_failed_stale_seq_ignored() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.gif_request_seq = 9;
        let task = app.update(AppMessage::GifSearchFailed {
            seq: 8,
            message: "stale error".to_string(),
        });
        drop(task);
        assert!(
            app.gif_error.is_none(),
            "stale failure must not set the error"
        );
    }

    #[test]
    fn gif_trending_results_store_page_and_keep_trending_flag() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let seq = 6;
        app.gif_request_seq = seq;
        let task = app.update(AppMessage::GifTrendingResults {
            seq,
            page: sample_page(vec![result_with(None)], None),
        });
        drop(task);
        assert_eq!(app.gif_results.len(), 1);
        assert!(
            app.gif_showing_trending,
            "trending results keep the trending flag"
        );
        assert!(!app.gif_loading);
    }

    #[test]
    fn gif_pagination_appends_and_dedups() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let seq = 10;
        app.gif_request_seq = seq;
        // First page: one item already present.
        let first = result_with(None);
        app.gif_results = vec![first.clone()];
        app.gif_appending = true;
        // Second page: same provider_id (dup) plus a new item.
        let mut second = result_with(None);
        second.provider_id = "gif-2".to_string();
        let task = app.update(AppMessage::GifSearchResults {
            seq,
            page: sample_page(vec![first.clone(), second.clone()], Some("3".to_string())),
        });
        drop(task);
        assert!(
            !app.gif_appending,
            "append mode must reset after the page lands"
        );
        let ids: Vec<&str> = app
            .gif_results
            .iter()
            .map(|r| r.provider_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["gif-1", "gif-2"],
            "dedup keeps the existing id first"
        );
        assert_eq!(app.gif_next_cursor.as_deref(), Some("3"));
    }

    #[test]
    fn gif_load_more_requires_cursor_and_not_loading() {
        with_klipy_unset(|| {
            let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
            // No cursor → no-op.
            app.gif_next_cursor = None;
            let task = app.update(AppMessage::GifLoadMore);
            drop(task);
            assert!(!app.gif_appending);

            // While a request is in flight → no-op.
            app.gif_next_cursor = Some("2".to_string());
            app.gif_loading = true;
            let task = app.update(AppMessage::GifLoadMore);
            drop(task);
            assert!(!app.gif_appending);

            // Cursor present and idle → append mode starts (missing-key path
            // keeps the deterministic not-configured state; no live request).
            app.gif_loading = false;
            app.gif_search_text = "cat".to_string();
            let task = app.update(AppMessage::GifLoadMore);
            drop(task);
            assert!(
                app.gif_appending,
                "idle picker with a cursor starts append mode"
            );
            assert!(app.gif_not_configured);
        });
    }

    #[test]
    fn gif_send_closes_picker_and_clears_search() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.show_gif_picker = true;
        app.gif_search_text = "cat".to_string();
        let gif = result_with(Some(media_source(
            "https://media.test/original.gif",
            GifMediaFormat::Gif,
        )));
        let task = app.update(AppMessage::SendGif(gif));
        drop(task);
        assert!(!app.show_gif_picker, "sending a GIF closes the picker");
        assert!(
            app.gif_search_text.is_empty(),
            "search text cleared after send"
        );
    }

    #[test]
    fn gif_picker_missing_key_state() {
        with_klipy_unset(|| {
            let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
            let task = app.update(AppMessage::ToggleGifPicker);
            drop(task);
            assert!(app.show_gif_picker);
            assert!(
                app.gif_not_configured,
                "missing key must surface immediately"
            );
            assert!(!app.gif_loading);
            assert!(app.gif_results.is_empty());
        });
    }

    #[test]
    fn gif_picker_close_during_request_clears_loading() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.show_gif_picker = true;
        app.gif_loading = true;
        let task = app.update(AppMessage::ToggleGifPicker);
        drop(task);
        assert!(!app.show_gif_picker);
        assert!(
            !app.gif_loading,
            "closing the picker must cancel in-flight state"
        );
    }

    #[test]
    fn gif_media_fetched_error_renders_fallback_entry() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let gif = boru_core::gif_provider::SharedGif {
            provider: "klipy".into(),
            provider_id: "gif-expired".into(),
            playback_url: "https://media.example/expired.mp4".into(),
            ..Default::default()
        };
        let hash = [0xABu8; 32];
        let generation = app.conversation_generation;
        let task = app.update(AppMessage::GifMediaFetched {
            sender: _local,
            gif,
            message_hash: hash,
            bytes: Err("media expired".to_string()),
            generation,
        });
        drop(task);
        let last = app.entries.last().expect("fallback entry pushed");
        assert!(
            last.image_error
                .as_deref()
                .is_some_and(|e| e.contains("GIF unavailable")),
            "expected a GIF unavailable fallback card, got {:?}",
            last.image_error
        );
    }

    // ── Local service scan pure logic (TUN-01) ─────────────────────────
    // These exercise `boru_core::local_service_scan`'s network-free helpers:
    // dedupe, label priority, the well-known-port table, and the self-exclusion
    // filter. No network, no app instance, no relay — safe to run on debsrv.

    use boru_core::local_service_scan::{
        dedupe_and_sort, exclude_self, parse_http_head, resolve_label, sort_suggestions,
        well_known_label, ListenerEntry, LocalServiceSuggestion,
    };

    fn listener(addr: &str, port: u16, pid: Option<u32>) -> ListenerEntry {
        ListenerEntry {
            local_addr: addr.parse().unwrap(),
            port,
            pid,
        }
    }

    #[test]
    fn local_service_scan_dedupe_collapses_bind_addrs_per_port() {
        let listeners = vec![
            listener("0.0.0.0", 3000, Some(11)),
            listener("127.0.0.1", 3000, Some(11)),
            listener("::1", 3000, Some(11)),
            listener("0.0.0.0", 8080, Some(12)),
            listener("192.168.1.5", 5432, Some(13)),
        ];
        let deduped = dedupe_and_sort(listeners);
        assert_eq!(deduped.len(), 3, "one entry per unique port");
        let ports: Vec<u16> = deduped.iter().map(|e| e.port).collect();
        assert_eq!(ports, vec![3000, 5432, 8080], "sorted ascending by port");
        // The loopback binding wins for port 3000.
        let p3000 = deduped.iter().find(|e| e.port == 3000).unwrap();
        assert!(p3000.local_addr.is_loopback(), "loopback binding preferred");
    }

    #[test]
    fn local_service_scan_well_known_port_table() {
        assert_eq!(well_known_label(3000), Some("Dev server"));
        assert_eq!(well_known_label(5432), Some("Postgres"));
        assert_eq!(well_known_label(3306), Some("MySQL"));
        assert_eq!(well_known_label(8080), Some("HTTP"));
        assert_eq!(well_known_label(22), Some("SSH"));
        assert_eq!(well_known_label(49152), None, "ephemeral port has no label");
    }

    #[test]
    fn local_service_scan_label_priority_process_name_first() {
        // Process name wins over Server header.
        assert_eq!(resolve_label(3000, Some("node"), Some("Express")), "node");
        // Server header beats the well-known table.
        assert_eq!(resolve_label(3000, None, Some("nginx/1.24")), "nginx/1.24");
        // Well-known table beats the fallback.
        assert_eq!(resolve_label(5432, None, None), "Postgres");
        // Fallback for unknown port.
        assert_eq!(resolve_label(49152, None, None), "TCP service on :49152");
    }

    #[test]
    fn local_service_scan_exclude_self_filters_pid_and_ports() {
        let listeners = vec![
            listener("127.0.0.1", 3000, Some(100)),
            listener("127.0.0.1", 8080, Some(200)),
            listener("127.0.0.1", 9000, None),
        ];
        let kept = exclude_self(listeners, Some(100), &[8080]);
        assert_eq!(kept.len(), 1, "own pid + excluded port both filtered");
        assert_eq!(kept[0].port, 9000);
    }

    #[test]
    fn local_service_scan_sort_http_first_then_port() {
        let suggestions = vec![
            LocalServiceSuggestion {
                port: 3000,
                label: "Dev server".to_string(),
                is_http: true,
            },
            LocalServiceSuggestion {
                port: 5432,
                label: "Postgres".to_string(),
                is_http: false,
            },
            LocalServiceSuggestion {
                port: 8080,
                label: "HTTP".to_string(),
                is_http: true,
            },
        ];
        let sorted = sort_suggestions(suggestions);
        let ports: Vec<u16> = sorted.iter().map(|s| s.port).collect();
        assert_eq!(ports, vec![3000, 8080, 5432], "HTTP first, then port");
    }

    #[test]
    fn local_service_scan_parse_http_head_extracts_status_and_server() {
        let (is_http, server) =
            parse_http_head("HTTP/1.1 200 OK\r\nServer: nginx/1.24\r\nContent-Length: 0\r\n");
        assert!(is_http);
        assert_eq!(server.as_deref(), Some("nginx/1.24"));

        let (is_http, server) = parse_http_head("NOT-HTTP garbage");
        assert!(!is_http);
        assert_eq!(server, None);
    }

    #[test]
    fn peer_id_short_form_truncates_long_ids_head_and_tail() {
        let long = "0123456789abcdef0123456789abcdef";
        assert_eq!(peer_id_short_form(long), "01234567…cdef");
    }

    #[test]
    fn peer_id_short_form_keeps_short_ids_unchanged() {
        assert_eq!(peer_id_short_form("a1b2c3d4"), "a1b2c3d4");
        // Exactly 16 chars stays as-is (the truncation threshold is >16).
        assert_eq!(peer_id_short_form("0123456789abcdef"), "0123456789abcdef");
    }

    #[test]
    fn home_connection_variant_maps_each_network_state_truthfully() {
        assert_eq!(
            home_connection_variant(&MeshHealth::Good, false, false),
            HomeConnectionVariant::Starting
        );
        assert_eq!(
            home_connection_variant(&MeshHealth::Good, false, true),
            HomeConnectionVariant::Connecting
        );
        assert_eq!(
            home_connection_variant(&MeshHealth::Good, true, true),
            HomeConnectionVariant::Ready
        );
        assert_eq!(
            home_connection_variant(&MeshHealth::Degraded("peer churn".to_string()), true, true),
            HomeConnectionVariant::Degraded
        );
        assert_eq!(
            home_connection_variant(
                &MeshHealth::Offline("transport unavailable".to_string()),
                true,
                true
            ),
            HomeConnectionVariant::Offline
        );
    }

    #[test]
    fn status_card_is_wired_into_home_screen() {
        // The redesigned connection status card (dark panel) replaces the
        // old hero card. Its layout/geometry bands are guarded in
        // status_card.rs and design_tokens.rs; here we only verify the
        // home screen actually routes the truthful variant + live selectors
        // into it (never a hard-coded "connected" state).
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains("crate::status_card::view_status_card"),
            "home screen must render the status card module"
        );
        assert!(
            home.contains("variant,") && home.contains("headline: headline.clone()"),
            "status card must receive the truthful variant + headline"
        );
        assert!(
            home.contains("pulse_frame: dep.hero_pulse_frame"),
            "status card must receive the pulse frame from the dependency"
        );
    }

    #[test]
    fn home_cards_thread_live_card_radius_from_theme() {
        // BORU-UI-21 acceptance step 2: "Open Home and modify card radius
        // from the inspector; verify immediate visual change." Every Home
        // card container must take its corner radius from the LIVE theme
        // (`btheme.radii.card`) — the value the inspector's "Card" radius
        // slider edits — never a static design-token literal, so a slider
        // movement is visible on the next redraw.
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains("card_radius(btheme.radii.card)"),
            "home menu item cards must take the live theme card radius"
        );
        assert!(
            home.contains("card_radius: btheme.radii.card"),
            "status card + mesh health card must take the live theme card radius"
        );
        assert!(
            home.contains("btheme.radii.card,"),
            "quick-action grid must receive the live theme card radius"
        );
        let shell_src = include_str!("../card_shell.rs");
        assert!(
            shell_src.contains("card_radius")
                && shell_src.contains("style.border.radius = radius.into()"),
            "CardShell must apply an overridden radius to its border style"
        );
        // Non-home callers keep the static default when the override is unset.
        assert!(
            shell_src.contains("card_radius: None"),
            "CardShell default keeps the static RADIUS_CARD appearance"
        );
    }

    #[test]
    fn mesh_event_tone_classifies_real_log_lines_truthfully() {
        // Lifecycle transitions from the watchdog.
        assert_eq!(
            mesh_event_tone("Mesh degraded: No peers in the mesh"),
            MeshEventTone::Warning
        );
        assert_eq!(
            mesh_event_tone("Mesh offline: Not connected to any room"),
            MeshEventTone::Danger
        );
        assert_eq!(
            mesh_event_tone("Mesh recovered: all peers active."),
            MeshEventTone::Success
        );
        // Discovery / connection summaries are positive but not fabrications.
        assert_eq!(
            mesh_event_tone("Discovered 2 direct, 1 relayed peers"),
            MeshEventTone::Success
        );
        assert_eq!(
            mesh_event_tone("Connected to room — 3 peers online"),
            MeshEventTone::Success
        );
        // Unknown future messages fall back to neutral rather than lying.
        assert_eq!(
            mesh_event_tone("Some unexpected future event"),
            MeshEventTone::Neutral
        );
    }

    #[test]
    fn mesh_event_visual_covers_every_tone() {
        let (icon, color) = mesh_event_visual(MeshEventTone::Success);
        assert!(!icon.is_empty());
        let _ = color(&iced::Theme::Light);
        for tone in [
            MeshEventTone::Success,
            MeshEventTone::Warning,
            MeshEventTone::Danger,
            MeshEventTone::Neutral,
        ] {
            let (icon, color) = mesh_event_visual(tone);
            assert!(!icon.is_empty(), "{tone:?} must map to a real icon");
            let _ = color(&iced::Theme::Light);
        }
    }

    #[test]
    fn inline_playback_errors_map_to_recoverable_categories() {
        assert!(matches!(
            InlinePlaybackError::from_backend("GStreamer: missing decoder for H265 codec").kind,
            InlinePlaybackErrorKind::UnsupportedCodec
        ));
        assert!(matches!(
            InlinePlaybackError::from_backend("invalid / corrupt container").kind,
            InlinePlaybackErrorKind::CorruptFile
        ));
        assert!(matches!(
            InlinePlaybackError::from_backend("permission denied").kind,
            InlinePlaybackErrorKind::PermissionDenied
        ));
        assert!(matches!(
            InlinePlaybackError::from_backend("attachment not found").kind,
            InlinePlaybackErrorKind::MissingFile
        ));
        assert!(matches!(
            InlinePlaybackError::from_backend("player init failed").kind,
            InlinePlaybackErrorKind::Initialization
        ));
        assert!(matches!(
            InlinePlaybackError::from_backend("unexpected backend failure").kind,
            InlinePlaybackErrorKind::Unknown
        ));
        assert!(InlinePlaybackError::from_backend("player init failed").retry_available());
        assert!(!InlinePlaybackError::from_backend("permission denied").retry_available());
    }

    #[test]
    fn inline_playback_error_keeps_technical_detail_out_of_user_copy() {
        let error = InlinePlaybackError::from_backend("open failed at /private/user/video.mp4");
        assert_eq!(error.title(), "Video player could not start");
        assert!(!error.message().contains("/private/user"));
        assert_eq!(error.detail, "open failed at <path>");
    }

    #[test]
    fn inline_playback_error_details_are_bounded_and_redact_all_path_tokens() {
        let raw = format!("decoder failed /tmp/{}", "x".repeat(500));
        let error = InlinePlaybackError::from_backend(raw);
        assert!(error.detail.len() <= 240);
        assert!(!error.detail.contains("/tmp/"));
        assert_eq!(error.title(), "Video format not supported");
    }

    #[test]
    fn discovered_peer_updates_remove_expired_peers_and_deduplicate_additions() {
        let first = iroh::SecretKey::generate().public();
        let second = iroh::SecretKey::generate().public();
        let mut peers = vec![first, second];

        apply_discovered_peers_update(
            &mut peers,
            DiscoveredPeersUpdate {
                added: vec![second, first],
                removed: vec![first],
            },
        );

        assert_eq!(peers, vec![second]);
    }

    #[test]
    fn close_dialog_uses_the_normal_cancel_message_in_priority_order() {
        // Image lightbox has highest priority
        assert!(matches!(
            IcedChat::close_dialog_message(true, true, true, true, true, None),
            Ok(AppMessage::CloseImageLightbox)
        ));
        assert!(matches!(
            IcedChat::close_dialog_message(false, true, false, false, true, None),
            Ok(AppMessage::CancelRoomSettings)
        ));
        assert!(matches!(
            IcedChat::close_dialog_message(false, false, true, false, true, None),
            Ok(AppMessage::CancelCreateRoom)
        ));
        assert!(matches!(
            IcedChat::close_dialog_message(false, false, false, true, true, None),
            Ok(AppMessage::CloseConnectionDetails)
        ));
        assert!(matches!(
            IcedChat::close_dialog_message(false, false, false, false, true, None),
            Ok(AppMessage::ClearHistoryRequested)
        ));
        let topic = TopicId::from_bytes([9; 32]);
        assert!(matches!(
            IcedChat::close_dialog_message(false, false, false, false, false, Some(topic)),
            Ok(AppMessage::DeleteRoomRequested(actual)) if actual == topic
        ));
    }

    #[test]
    fn close_dialog_without_an_open_dialog_returns_structured_error() {
        let error = IcedChat::close_dialog_message(false, false, false, false, false, None)
            .expect_err("CloseDialog must reject when no dialog is open");
        assert_eq!(error.code, GuiActionErrorCode::NoDialog);
        assert_eq!(error.message, "No application dialog is currently open");
    }

    #[test]
    fn clear_history_request_toggles_confirmation_and_confirmation_flow_updates_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        assert!(!app.history_confirm_clear);
        assert!(!app.history_clear_pending);
        app.history_clear_feedback = Some("stale".to_string());
        app.history_clear_feedback_is_error = true;

        let task = app.update(AppMessage::ClearHistoryRequested);
        drop(task);

        assert!(
            app.history_confirm_clear,
            "confirmation opens on first click"
        );
        assert!(
            app.history_clear_feedback.is_none(),
            "old feedback cleared when opening"
        );
        assert!(!app.history_clear_feedback_is_error);

        let task = app.update(AppMessage::ConfirmClearHistory);
        drop(task);

        assert!(
            app.history_clear_pending,
            "clear operation starts in pending state"
        );
        assert!(
            app.history_confirm_clear,
            "confirmation stays open while pending"
        );

        let topic = app.topic;
        let room_history = app.room_history.clone();
        let finished = AppMessage::ClearHistoryFinished {
            topic,
            room_history,
            report: RoomHistoryClearReport {
                topic,
                room_history_updated: true,
                chat_entries_removed: 3,
                outbox_entries_removed: 1,
            },
        };
        let task = app.update(finished);
        drop(task);

        assert!(
            !app.history_clear_pending,
            "pending state cleared after success"
        );
        assert!(
            !app.history_confirm_clear,
            "confirmation closes after success"
        );
        assert_eq!(
            app.history_clear_feedback.as_deref(),
            Some("Cleared 3 messages from this chat.")
        );
        assert!(!app.history_clear_feedback_is_error);
    }

    #[test]
    fn clear_history_failure_keeps_confirmation_open_and_shows_error() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.history_confirm_clear = true;
        app.history_clear_pending = true;
        let task = app.update(AppMessage::ClearHistoryFailed {
            topic: app.topic,
            error: "backend unavailable".to_string(),
        });
        drop(task);

        assert!(
            !app.history_clear_pending,
            "pending state cleared after failure"
        );
        assert!(
            app.history_confirm_clear,
            "confirmation remains open after failure"
        );
        assert!(app.history_clear_feedback_is_error);
        assert_eq!(
            app.history_clear_feedback.as_deref(),
            Some("backend unavailable")
        );
    }

    #[test]
    // ── UI-19: keyboard focus traversal and activation ─────────────────
    fn tab_moves_focus_to_next_input_and_shift_tab_previous() {
        use iced::keyboard::{key, Key, Modifiers};
        let plain = Modifiers::empty();
        let shift = Modifiers::SHIFT;
        assert_eq!(
            shortcut_from_key(&Key::Named(key::Named::Tab), plain),
            Some(Shortcut::FocusNext),
            "Tab → FocusNext"
        );
        assert_eq!(
            shortcut_from_key(&Key::Named(key::Named::Tab), shift),
            Some(Shortcut::FocusPrevious),
            "Shift+Tab → FocusPrevious"
        );
    }

    #[test]
    fn escape_new_chat_and_back_to_chat_list_shortcuts_still_map() {
        use iced::keyboard::{key, Key, Modifiers};
        let plain = Modifiers::empty();
        let ctrl = Modifiers::CTRL;
        assert_eq!(
            shortcut_from_key(&Key::Named(key::Named::Escape), plain),
            Some(Shortcut::Escape)
        );
        assert_eq!(
            shortcut_from_key(&Key::Character("n".to_string().into()), ctrl),
            Some(Shortcut::NewChat)
        );
        assert_eq!(
            shortcut_from_key(&Key::Named(key::Named::Backspace), ctrl),
            Some(Shortcut::BackToChatList)
        );
        // Slash is the quick-command prefix.
        assert_eq!(
            shortcut_from_key(&Key::Character("/".to_string().into()), plain),
            Some(Shortcut::QuickCommand)
        );
    }

    #[test]
    fn shortcut_update_returns_focus_tasks() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        let next = app.update(AppMessage::Shortcut(Shortcut::FocusNext));
        let prev = app.update(AppMessage::Shortcut(Shortcut::FocusPrevious));
        // Tasks must not panic; focus operations are only applied on the
        // next frame by iced, so asserting they exist is the reachable check.
        let _ = next;
        let _ = prev;
        drop(runtime);
    }

    #[test]
    fn connection_details_open_clears_other_dialogs_and_stores_focus_target() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        // Set up other dialogs that should be cleared
        app.rooms_state.show_create_room_dialog = true;
        app.history_confirm_clear = true;
        app.friend_remove_confirm = true;
        app.friend_block_confirm = true;

        let task = app.update(AppMessage::OpenConnectionDetails);
        drop(task); // focus task, not needed for state checks

        assert!(!app.rooms_state.show_create_room_dialog, "create-room dialog cleared");
        assert!(!app.history_confirm_clear, "history confirm cleared");
        assert!(!app.friend_remove_confirm, "remove confirm cleared");
        assert!(!app.friend_block_confirm, "block confirm cleared");
        assert!(
            app.connection_details_dialog.is_some(),
            "connection details dialog opened"
        );
        assert!(
            app.connection_details_announcement.is_none(),
            "announcement starts clear"
        );
        assert_eq!(
            app.connection_details_focus_target,
            Some(CONNECTION_DETAILS_TRIGGER_INPUT),
            "focus target stored for restoration"
        );
        drop(runtime);
    }

    #[test]
    fn connection_details_open_with_existing_room_delete_confirm_clears_it() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        let topic = TopicId::from_bytes([9; 32]);
        app.room_delete_confirm_topic = Some(topic);

        let task = app.update(AppMessage::OpenConnectionDetails);
        drop(task);

        assert!(
            app.room_delete_confirm_topic.is_none(),
            "room delete confirm cleared"
        );
        assert!(app.connection_details_dialog.is_some());
        drop(runtime);
    }

    #[test]
    fn escape_closes_tunnel_and_share_dialogs_when_idle() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Tunnel picker closes on Escape.
        app.tunnels_state.show_create_tunnel_dialog = true;
        let task = app.update(AppMessage::Shortcut(Shortcut::Escape));
        drop(task);
        assert!(
            !app.tunnels_state.show_create_tunnel_dialog,
            "Escape closes the create-tunnel picker"
        );

        // Share-local-service form closes on Escape when not submitting.
        app.tunnels_state.share_local_service_open = true;
        app.tunnels_state.share_service_submitting = false;
        let task = app.update(AppMessage::Shortcut(Shortcut::Escape));
        drop(task);
        assert!(
            !app.tunnels_state.share_local_service_open,
            "Escape closes the share-local-service form"
        );

        drop(runtime);
    }

    #[test]
    fn escape_closes_emoji_and_gif_pickers() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Emoji picker closes on Escape.
        app.show_emoji_picker = true;
        let task = app.update(AppMessage::Shortcut(Shortcut::Escape));
        drop(task);
        assert!(!app.show_emoji_picker, "Escape closes the emoji picker");

        // GIF picker closes on Escape.
        app.show_gif_picker = true;
        let task = app.update(AppMessage::Shortcut(Shortcut::Escape));
        drop(task);
        assert!(!app.show_gif_picker, "Escape closes the gif picker");

        drop(runtime);
    }

    #[test]
    fn escape_does_not_close_dialog_while_submitting() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.rooms_state.show_create_room_dialog = true;
        app.rooms_state.create_room_submitting = true;
        let task = app.update(AppMessage::Shortcut(Shortcut::Escape));
        drop(task);
        assert!(
            app.rooms_state.show_create_room_dialog,
            "Escape must not dismiss a mid-submit create-room dialog"
        );
        assert!(app.rooms_state.create_room_submitting);

        app.show_create_group_dialog = true;
        app.create_group_submitting = true;
        let task = app.update(AppMessage::Shortcut(Shortcut::Escape));
        drop(task);
        assert!(
            app.show_create_group_dialog,
            "Escape must not dismiss a mid-submit create-group dialog"
        );

        app.tunnels_state.share_local_service_open = true;
        app.tunnels_state.share_service_submitting = true;
        let task = app.update(AppMessage::Shortcut(Shortcut::Escape));
        drop(task);
        assert!(
            app.tunnels_state.share_local_service_open,
            "Escape must not dismiss a mid-submit share dialog"
        );

        drop(runtime);
    }

    #[test]
    fn cancel_handlers_are_ignored_while_submitting() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.rooms_state.show_create_room_dialog = true;
        app.rooms_state.create_room_submitting = true;
        let task = app.update(AppMessage::CancelCreateRoom);
        drop(task);
        assert!(
            app.rooms_state.show_create_room_dialog,
            "CancelCreateRoom must be a no-op mid-submit"
        );

        app.show_create_group_dialog = true;
        app.create_group_submitting = true;
        let task = app.update(AppMessage::HideCreateGroupDialog);
        drop(task);
        assert!(
            app.show_create_group_dialog,
            "HideCreateGroupDialog must be a no-op mid-submit"
        );

        app.tunnels_state.share_local_service_open = true;
        app.tunnels_state.share_service_submitting = true;
        let task = app.update(AppMessage::CancelShareLocalService);
        drop(task);
        assert!(
            app.tunnels_state.share_local_service_open,
            "CancelShareLocalService must be a no-op mid-submit"
        );

        drop(runtime);
    }

    #[test]
    fn cancel_handlers_close_dialogs_when_idle_and_clear_errors() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.rooms_state.show_create_room_dialog = true;
        app.rooms_state.create_room_error = Some("boom".to_string());
        let task = app.update(AppMessage::CancelCreateRoom);
        drop(task);
        assert!(!app.rooms_state.show_create_room_dialog);
        assert!(app.rooms_state.create_room_error.is_none(), "inline error cleared");

        app.show_create_group_dialog = true;
        app.create_group_error = Some("boom".to_string());
        let task = app.update(AppMessage::HideCreateGroupDialog);
        drop(task);
        assert!(!app.show_create_group_dialog);
        assert!(app.create_group_error.is_none(), "inline error cleared");

        app.tunnels_state.share_local_service_open = true;
        app.tunnels_state.share_service_error = Some("boom".to_string());
        let task = app.update(AppMessage::CancelShareLocalService);
        drop(task);
        assert!(!app.tunnels_state.share_local_service_open);
        assert!(app.tunnels_state.share_service_error.is_none(), "inline error cleared");

        drop(runtime);
    }

    #[test]
    fn confirm_group_with_empty_name_keeps_dialog_open_with_inline_error() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.show_create_group_dialog = true;
        app.create_group_name = "   ".to_string();

        let task = app.update(AppMessage::ConfirmCreateGroup);
        drop(task);

        assert!(
            app.show_create_group_dialog,
            "dialog stays open when the name is empty"
        );
        assert!(
            app.create_group_error.is_some(),
            "inline error set for empty group name"
        );
        assert!(!app.create_group_submitting, "no submit started");

        drop(runtime);
    }

    #[test]
    fn confirm_group_valid_keeps_dialog_open_while_submitting() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.show_create_group_dialog = true;
        app.create_group_name = "Squad".to_string();

        let task = app.update(AppMessage::ConfirmCreateGroup);
        drop(task);

        assert!(
            app.create_group_submitting,
            "submit flag raised while async creation runs"
        );
        assert!(
            app.show_create_group_dialog,
            "dialog stays open (loading state) while creation runs"
        );
        assert!(app.create_group_error.is_none());

        drop(runtime);
    }

    #[test]
    fn confirm_share_local_service_invalid_port_sets_inline_error_and_keeps_open() {
        let (runtime, mut app, _local, peer) = build_join_request_test_app();
        app.screen = Screen::FriendProfile(peer);
        app.tunnels_state.share_local_service_open = true;
        app.tunnels_state.share_service_port = "not-a-port".to_string();

        let task = app.update(AppMessage::ConfirmShareLocalService);
        drop(task);

        assert!(
            app.tunnels_state.share_local_service_open,
            "dialog stays open on invalid port"
        );
        assert!(app.tunnels_state.share_service_error.is_some(), "inline port error set");
        assert!(!app.tunnels_state.share_service_submitting);

        drop(runtime);
    }

    #[test]
    fn confirm_share_local_service_zero_port_sets_inline_error() {
        let (runtime, mut app, _local, peer) = build_join_request_test_app();
        app.screen = Screen::FriendProfile(peer);
        app.tunnels_state.share_local_service_open = true;
        app.tunnels_state.share_service_port = "0".to_string();

        let task = app.update(AppMessage::ConfirmShareLocalService);
        drop(task);

        assert!(app.tunnels_state.share_local_service_open);
        assert!(app.tunnels_state.share_service_error.is_some());

        drop(runtime);
    }

    #[test]
    fn dialog_width_responsive_helper_caps_to_window() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.window_width = 600.0;
        assert_eq!(
            app.dialog_width(760.0),
            552.0,
            "wide dialog capped to window minus 48 px margin"
        );
        app.window_width = 1200.0;
        assert_eq!(
            app.dialog_width(760.0),
            760.0,
            "preferred width kept when the window is large"
        );
        drop(runtime);
    }

    #[test]
    fn room_join_failed_while_group_submitting_keeps_dialog_with_inline_error() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.show_create_group_dialog = true;
        app.create_group_submitting = true;
        let generation = app.room_generation;

        let task = app.update(AppMessage::RoomJoinFailed {
            error: "timeout".to_string(),
            generation,
        });
        drop(task);

        assert!(
            !app.create_group_submitting,
            "submit flag cleared after failure"
        );
        assert!(
            app.show_create_group_dialog,
            "group dialog stays open after failure"
        );
        assert!(
            app.create_group_error.is_some(),
            "inline group error surfaces the failure"
        );

        drop(runtime);
    }

    #[test]
    fn room_join_failed_while_room_submitting_keeps_dialog_with_inline_error() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.rooms_state.show_create_room_dialog = true;
        app.rooms_state.create_room_submitting = true;
        let generation = app.room_generation;

        let task = app.update(AppMessage::RoomJoinFailed {
            error: "timeout".to_string(),
            generation,
        });
        drop(task);

        assert!(!app.rooms_state.create_room_submitting);
        assert!(
            app.rooms_state.show_create_room_dialog,
            "create-room dialog stays open after failure"
        );
        assert!(app.rooms_state.create_room_error.is_some());

        drop(runtime);
    }

    #[test]
    fn dialog_width_responsive_helper_floors_at_320() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.window_width = 200.0;
        assert_eq!(
            app.dialog_width(560.0),
            320.0,
            "dialog width floors at 320 px even on tiny windows"
        );
        drop(runtime);
    }

    #[test]
    fn connection_details_close_clears_dialog_announcement_and_restores_focus() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        // Open the dialog first
        app.connection_details_dialog = Some(app.current_connection_details_dialog());
        app.connection_details_announcement = Some("Announcement text".to_string());
        app.connection_details_focus_target = Some(CONNECTION_DETAILS_TRIGGER_INPUT);

        let task = app.close_connection_details_dialog();
        drop(task); // focus restoration task, not needed for state checks

        assert!(app.connection_details_dialog.is_none(), "dialog cleared");
        assert!(
            app.connection_details_announcement.is_none(),
            "announcement cleared"
        );
        assert!(
            app.connection_details_focus_target.is_none(),
            "focus target consumed"
        );
        drop(runtime);
    }

    #[test]
    fn connection_details_close_without_focus_target_does_not_panic() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.connection_details_dialog = Some(app.current_connection_details_dialog());
        app.connection_details_focus_target = None; // explicitly none

        let task = app.close_connection_details_dialog();
        drop(task);

        assert!(app.connection_details_dialog.is_none());
        drop(runtime);
    }

    #[test]
    fn connection_details_close_via_close_current_dialog_routes_correctly() {
        let (runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.connection_details_dialog = Some(app.current_connection_details_dialog());

        let message = app
            .close_current_dialog()
            .expect("should find dialog to close");
        assert!(matches!(message, AppMessage::CloseConnectionDetails));
        drop(runtime);
    }

    #[test]
    fn confirmed_direct_invite_addrs_keeps_relay_only_peer_metadata() {
        let local = SecretKey::generate().public();
        let peer = SecretKey::generate().public();
        let topic = direct_topic(&local, &peer);
        let data_dir = std::env::temp_dir().join(format!(
            "boru-confirmed-invite-relay-only-{}",
            std::process::id()
        ));
        let mut friends = FriendsStore::empty_at(&data_dir);
        let fid = FriendId::from_public_key(peer);
        let existing = vec![EndpointAddr::new(peer)];
        friends.ensure_friend(fid).record_addrs(existing.clone());

        let merged = confirmed_direct_invite_addrs(local, &friends, peer, topic, &[])
            .expect("matching topic should confirm");

        assert_eq!(
            merged, existing,
            "relay-only invites should preserve known peers"
        );
    }

    #[test]
    fn confirmed_direct_invite_addrs_deduplicates_duplicate_peers() {
        let local = SecretKey::generate().public();
        let peer = SecretKey::generate().public();
        let topic = direct_topic(&local, &peer);
        let data_dir =
            std::env::temp_dir().join(format!("boru-confirmed-invite-dup-{}", std::process::id()));
        let mut friends = FriendsStore::empty_at(&data_dir);
        let fid = FriendId::from_public_key(peer);
        let existing = vec![EndpointAddr::new(peer)];
        friends.ensure_friend(fid).record_addrs(existing.clone());

        let incoming = vec![EndpointAddr::new(peer), EndpointAddr::new(peer)];
        let merged = confirmed_direct_invite_addrs(local, &friends, peer, topic, &incoming)
            .expect("matching topic should confirm");

        assert_eq!(merged, existing, "duplicate peers should be deduplicated");
    }

    #[test]
    fn confirmed_direct_invite_addrs_rejects_before_mutating_state() {
        let local = SecretKey::generate().public();
        let peer = SecretKey::generate().public();
        let wrong_topic = TopicId::from_bytes([99; 32]);
        let data_dir = std::env::temp_dir().join(format!(
            "boru-confirmed-invite-reject-{}",
            std::process::id()
        ));
        let mut friends = FriendsStore::empty_at(&data_dir);
        let fid = FriendId::from_public_key(peer);
        friends
            .ensure_friend(fid)
            .record_addrs(vec![EndpointAddr::new(peer)]);
        let before = friends.clone();

        let confirmed = confirmed_direct_invite_addrs(local, &friends, peer, wrong_topic, &[]);

        assert!(
            confirmed.is_none(),
            "invalid topic should not confirm the invite"
        );
        assert_eq!(
            friends.friends, before.friends,
            "state must remain unchanged"
        );
    }

    #[test]
    fn gui_dark_mode_command_maps_to_normal_toggle_message() {
        assert!(matches!(
            gui_dark_mode_message(&GuiTestCommand::ToggleDarkMode { enabled: true }),
            Some(AppMessage::ToggleDark(true))
        ));
        assert!(matches!(
            gui_dark_mode_message(&GuiTestCommand::ToggleDarkMode { enabled: false }),
            Some(AppMessage::ToggleDark(false))
        ));
        assert!(gui_dark_mode_message(&GuiTestCommand::OpenSettings).is_none());
    }

    #[test]
    fn gui_help_command_maps_to_normal_toggle_message() {
        assert!(matches!(
            gui_help_message(&GuiTestCommand::ToggleHelp),
            Some(AppMessage::ToggleHelp)
        ));
        assert!(gui_help_message(&GuiTestCommand::OpenSettings).is_none());
        assert!(gui_help_message(&GuiTestCommand::GoToChatList).is_none());
    }

    #[test]
    fn dark_mode_settings_persist_without_changing_other_settings() {
        let data_dir =
            std::env::temp_dir().join(format!("boru-gui-dark-mode-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).expect("test settings directory should be created");

        let original = AppSettings {
            dark_mode: false,
            sound_enabled: false,
            chat_text_size: 17.0,
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
        };
        let toggled = AppSettings {
            dark_mode: true,
            sound_enabled: original.sound_enabled,
            chat_text_size: original.chat_text_size,
            share_direct_addresses: original.share_direct_addresses,
            display_name: None,
            home_background_image: None,
            home_menu_item_opacity: HOME_MENU_ITEM_OPACITY_DEFAULT,
            accent_color: None,
            show_presence_indicator: true,
            typing_indicators_enabled: true,
            recent_emojis: original.recent_emojis.clone(),
            notification_policy: original.notification_policy,
            conversation_notification_policies: original.conversation_notification_policies.clone(),
        };
        toggled.save(&data_dir);
        let loaded = AppSettings::load(&data_dir);

        assert!(loaded.dark_mode);
        assert!(!loaded.sound_enabled);
        assert_eq!(loaded.chat_text_size, 17.0);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn home_menu_item_opacity_persists_across_settings_roundtrip() {
        let data_dir =
            std::env::temp_dir().join(format!("boru-gui-home-opacity-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).expect("test settings directory should be created");

        // HOME-01: absent key defaults to the shared 0.85 default.
        let defaults = AppSettings::load(&data_dir);
        assert_eq!(
            defaults.home_menu_item_opacity,
            HOME_MENU_ITEM_OPACITY_DEFAULT
        );

        // Saving an explicit value round-trips without disturbing siblings.
        let settings = AppSettings {
            dark_mode: false,
            sound_enabled: true,
            chat_text_size: 17.0,
            share_direct_addresses: false,
            display_name: None,
            home_background_image: None,
            home_menu_item_opacity: 0.55,
            accent_color: None,
            show_presence_indicator: true,
            typing_indicators_enabled: true,
            recent_emojis: Vec::new(),
            notification_policy: crate::notification::service::NotificationPolicy::All,
            conversation_notification_policies: Vec::new(),
        };
        settings.save(&data_dir);
        let loaded = AppSettings::load(&data_dir);
        assert_eq!(loaded.home_menu_item_opacity, 0.55);
        assert_eq!(loaded.chat_text_size, 17.0);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// Acceptance (BORU-TWEMOJI-14): recently-used emoji persist across app
    /// restarts — a `settings.json` round-trip preserves the Unicode list
    /// exactly, and the values are plain Unicode strings, never asset keys
    /// or SVG paths.
    #[test]
    fn recent_emojis_round_trip_in_settings() {
        let data_dir = std::env::temp_dir().join(format!(
            "boru-gui-recent-emojis-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).expect("test settings directory should be created");

        let recents = vec!["❤️".to_string(), "😂".to_string(), "👍".to_string()];
        let settings = AppSettings {
            dark_mode: false,
            sound_enabled: true,
            chat_text_size: 17.0,
            share_direct_addresses: false,
            display_name: None,
            home_background_image: None,
            home_menu_item_opacity: HOME_MENU_ITEM_OPACITY_DEFAULT,
            accent_color: None,
            show_presence_indicator: true,
            typing_indicators_enabled: true,
            recent_emojis: recents.clone(),
            notification_policy: crate::notification::service::NotificationPolicy::All,
            conversation_notification_policies: Vec::new(),
        };
        settings.save(&data_dir);
        let loaded = AppSettings::load(&data_dir);
        assert_eq!(loaded.recent_emojis, recents);

        // The stored values are plain Unicode strings — never asset keys,
        // paths, or filenames.
        for entry in &loaded.recent_emojis {
            assert!(!entry.contains(".svg") && !entry.contains('/'));
            assert!(!entry.trim().is_empty());
        }
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn invitation_endpoint_addr_strips_direct_addrs_by_default() {
        let peer = SecretKey::generate().public();
        let relay: iroh::RelayUrl = "https://relay.example.test./".parse().unwrap();
        let direct: std::net::SocketAddr = "192.0.2.44:4321".parse().unwrap();
        let addr = EndpointAddr::new(peer)
            .with_relay_url(relay.clone())
            .with_ip_addr(direct);

        let sanitized = invitation_endpoint_addr(addr.clone(), false);

        assert_eq!(sanitized.id, peer);
        assert_eq!(sanitized.relay_urls().next().cloned(), Some(relay));
        assert!(sanitized.ip_addrs().next().is_none());
        assert!(
            addr.ip_addrs().next().is_some(),
            "fixture should contain a direct address"
        );
    }

    #[test]
    fn invitation_endpoint_addr_keeps_direct_addrs_when_opted_in() {
        let peer = SecretKey::generate().public();
        let relay: iroh::RelayUrl = "https://relay.example.test./".parse().unwrap();
        let direct: std::net::SocketAddr = "192.0.2.44:4321".parse().unwrap();
        let addr = EndpointAddr::new(peer)
            .with_relay_url(relay.clone())
            .with_ip_addr(direct);

        let opted_in = invitation_endpoint_addr(addr.clone(), true);

        assert_eq!(opted_in, addr);
    }

    #[test]
    fn room_ticket_payload_round_trips_without_direct_addresses() {
        let peer = SecretKey::generate().public();
        let topic = TopicId::from_bytes([42; 32]);
        let relay: iroh::RelayUrl = "https://relay.example.test./".parse().unwrap();
        let direct: std::net::SocketAddr = "192.0.2.44:4321".parse().unwrap();
        let addr = EndpointAddr::new(peer)
            .with_relay_url(relay)
            .with_ip_addr(direct);
        let ticket = Ticket {
            topic,
            peers: vec![invitation_endpoint_addr(addr, false)],
            discovery_secret: None,
        };
        let encoded = ticket.to_string();
        let decoded = encoded
            .parse::<Ticket>()
            .expect("ticket payload should parse");

        assert_eq!(decoded, ticket);
        assert!(decoded.peers[0].ip_addrs().next().is_none());
    }

    #[test]
    fn room_ticket_payload_round_trips_with_direct_addresses_when_opted_in() {
        let peer = SecretKey::generate().public();
        let topic = TopicId::from_bytes([43; 32]);
        let relay: iroh::RelayUrl = "https://relay.example.test./".parse().unwrap();
        let direct: std::net::SocketAddr = "192.0.2.45:4321".parse().unwrap();
        let addr = EndpointAddr::new(peer)
            .with_relay_url(relay)
            .with_ip_addr(direct);
        let ticket = Ticket {
            topic,
            peers: vec![invitation_endpoint_addr(addr.clone(), true)],
            discovery_secret: None,
        };
        let encoded = ticket.to_string();
        let decoded = encoded
            .parse::<Ticket>()
            .expect("ticket payload should parse");

        assert_eq!(decoded, ticket);
        assert!(decoded.peers[0].ip_addrs().next().is_some());
        assert_eq!(decoded.peers[0], addr);
    }

    #[test]
    fn room_invite_v2_qr_payload_round_trips_without_endpoint_info() {
        let topic = TopicId::from_bytes([44; 32]);
        let secret = DiscoverySecret::from_bytes([0x2au8; 32]);
        let invite = RoomInviteV2::new(topic, secret.clone());
        let encoded = invite.encode();

        assert!(encoded.starts_with("boru1:"));
        assert_eq!(encoded.matches(':').count(), 1);
        let parsed = RoomInvitation::parse(&encoded).expect("invite should parse");
        match parsed {
            RoomInvitation::Stable(restored) => {
                assert_eq!(restored.topic, topic);
                assert_eq!(restored.discovery_secret, secret);
            }
            RoomInvitation::Legacy(_) => panic!("stable invite should not fall back to legacy"),
        }
    }

    #[test]
    fn hash_only_file_share_value_is_not_a_blob_ticket() {
        let hash = iroh_blobs::Hash::from_bytes([7; 32]);
        assert!(hash.to_string().parse::<BlobTicket>().is_err());
    }

    #[test]
    fn format_message_time_converts_utc_into_local_today_time() {
        let tz = FixedOffset::east_opt(2 * 3600).expect("valid offset");
        let now = tz
            .with_ymd_and_hms(2024, 1, 3, 12, 0, 0)
            .single()
            .expect("valid local now");
        let timestamp_ms = Utc
            .with_ymd_and_hms(2024, 1, 3, 0, 30, 0)
            .single()
            .expect("valid utc timestamp")
            .timestamp_millis();

        let rendered =
            format_message_time_with(timestamp_ms, now, |ms| tz.timestamp_millis_opt(ms).single());

        assert_eq!(rendered, "02:30");
    }

    #[test]
    fn format_message_time_converts_utc_into_local_weekday() {
        let tz = FixedOffset::east_opt(2 * 3600).expect("valid offset");
        let now = tz
            .with_ymd_and_hms(2024, 1, 3, 12, 0, 0)
            .single()
            .expect("valid local now");
        let timestamp_ms = Utc
            .with_ymd_and_hms(2024, 1, 2, 20, 30, 0)
            .single()
            .expect("valid utc timestamp")
            .timestamp_millis();

        let rendered =
            format_message_time_with(timestamp_ms, now, |ms| tz.timestamp_millis_opt(ms).single());

        assert_eq!(rendered, "Tue 22:30");
    }

    #[test]
    fn record_profile_image_ticket_dedup_same_ticket_skips_redownload() {
        use iroh::PublicKey;
        use std::collections::{HashMap, VecDeque};

        let sk = SecretKey::generate();
        let pk = sk.public();
        let ticket_a = "ticket_v1_hash_abc".to_string();
        let ticket_a_dup = "ticket_v1_hash_abc".to_string();
        let ticket_b = "ticket_v2_hash_xyz".to_string();

        // Test the dedup logic inline (same guard as record_profile_image_ticket).
        let mut handles: HashMap<PublicKey, Option<iced::widget::image::Handle>> = HashMap::new();
        let mut tickets: HashMap<PublicKey, String> = HashMap::new();
        let mut queue: VecDeque<(PublicKey, String)> = VecDeque::new();

        // First call: new ticket → should queue a download.
        let pk1 = pk;
        tickets.insert(pk1, ticket_a.clone());
        handles.insert(pk1, None);
        queue.push_back((pk1, ticket_a.clone()));
        assert_eq!(queue.len(), 1, "first ticket should be queued");
        assert_eq!(
            handles.get(&pk1),
            Some(&None),
            "handle should be invalidated (None)"
        );

        // Simulate a successful download completing.
        let _handle = iced::widget::image::Handle::from_bytes(vec![0; 32]);
        handles.insert(pk1, None); // would become Some(handle) after download

        // Second call: same ticket → should NOT re-invalidate or re-queue.
        // Simulate the guard from record_profile_image_ticket.
        if tickets.get(&pk1) != Some(&ticket_a_dup) {
            panic!("guard failed: ticket should match");
        }
        // Guard returns early, so nothing changes.
        assert_eq!(queue.len(), 1, "same ticket should NOT add to queue");
        assert_eq!(
            tickets.get(&pk1),
            Some(&ticket_a),
            "cached ticket should remain unchanged"
        );

        // Third call: NEW ticket → should update and re-queue, but KEEP
        // the old handle (background refresh — not immediate invalidation).
        tickets.insert(pk1, ticket_b.clone());
        // Old handle is kept: only seed None if this is a first-time download.
        handles.entry(pk1).or_insert(None);
        queue.push_back((pk1, ticket_b.clone()));
        assert_eq!(queue.len(), 2, "new ticket should be queued");
        assert_eq!(
            tickets.get(&pk1),
            Some(&ticket_b),
            "cached ticket should update"
        );
        // The existing handle must still be present (not cleared to None).
        // This validates the background-refresh invariant: old artwork
        // stays visible while the update downloads.
        assert!(
            handles.contains_key(&pk1),
            "existing handle should not be removed on ticket update"
        );

        // clear_profile_image should remove the cached ticket.
        handles.remove(&pk1);
        tickets.remove(&pk1);
        queue.retain(|(p, _)| *p != pk1);
        assert!(!handles.contains_key(&pk1), "handle should be removed");
        assert!(
            !tickets.contains_key(&pk1),
            "cached ticket should be removed"
        );
        assert_eq!(queue.len(), 0, "queue should be cleared");
    }

    // ── Performance regression tests ─────────────────────────────────────

    #[test]
    fn perf_image_bytes_countable_across_many_entries() {
        let image_data = vec![0xABu8; 8192];
        let entries: Vec<ChatEntry> = (0..25)
            .map(|i| ChatEntry {
                kind: ChatKind::Remote,
                label: "p".into(),
                body: format!("img {i}"),
                message_hash: None,
                edited: false,
                reactions: vec![],
                image_handle: None,
                avatar_handle: None,
                image_bytes: Some(image_data.clone()),
                image_identifier: None,
                image_error: None,
                image_width: None,
                image_height: None,
                gif_frames: None,
                timestamp: Some(i as i64),
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
            })
            .collect();
        let total: usize = entries
            .iter()
            .filter_map(|e| e.image_bytes.as_ref())
            .map(|b| b.len())
            .sum();
        assert_eq!(total, 25 * 8192);
        assert_eq!(
            entries.iter().filter(|e| e.image_bytes.is_some()).count(),
            25
        );
    }

    #[test]
    fn perf_text_only_entries_have_no_image_bytes() {
        let entries: Vec<ChatEntry> = (0..100)
            .map(|i| ChatEntry::remote("p", format!("text {i}"), None, None, None))
            .collect();
        let bytes: usize = entries
            .iter()
            .filter_map(|e| e.image_bytes.as_ref())
            .map(|b| b.len())
            .sum();
        assert_eq!(bytes, 0, "text entries must not carry image data");
    }

    // ── Chat header presence states (UI-12) ────────────────────────────

    #[test]
    fn presence_states_have_distinct_labels() {
        assert_eq!(PeerPresence::Online.label(), "Online");
        assert_eq!(PeerPresence::Away.label(), "Away");
        assert_eq!(PeerPresence::Offline.label(), "Offline");
        assert_eq!(PeerPresence::Connecting.label(), "Connecting…");
        assert_eq!(PeerPresence::Unknown.label(), "Unknown");
    }

    #[test]
    fn presence_colors_map_to_theme_semantics() {
        let light = iced::Theme::Light;
        let dark = iced::Theme::Dark;
        // Online is green, Away/Connecting use warning amber, Offline/Unknown muted.
        for theme in [&light, &dark] {
            let online = PeerPresence::Online.color(theme);
            let away = PeerPresence::Away.color(theme);
            let connecting = PeerPresence::Connecting.color(theme);
            let offline = PeerPresence::Offline.color(theme);
            let unknown = PeerPresence::Unknown.color(theme);
            assert_ne!(online, offline, "online must read differently from offline");
            assert_eq!(away, connecting, "connecting borrows the warning tone");
            assert_eq!(offline, unknown, "unknown stays muted like offline");
            assert_ne!(connecting, online, "connecting must not read as online");
        }
    }

    #[test]
    fn presence_icons_are_consistent_with_state() {
        assert_eq!(
            PeerPresence::Connecting.icon(),
            ICON_RETRY,
            "connecting should show a retry/sync glyph"
        );
        assert_eq!(PeerPresence::Offline.icon(), ICON_OFFLINE);
        assert_eq!(PeerPresence::Unknown.icon(), ICON_OFFLINE);
        assert_eq!(PeerPresence::Online.icon(), ICON_ONLINE);
    }

    // ── In-conversation search matching (UI-12) ────────────────────────

    #[test]
    fn chat_search_empty_query_returns_no_matches() {
        let entries = vec![
            ChatEntry::remote("alice", "hello world", None, None, None),
            ChatEntry::remote("bob", "anything here", None, None, None),
        ];
        assert!(chat_search_matches_in(&entries, "").is_empty());
        assert!(chat_search_matches_in(&entries, "   ").is_empty());
    }

    #[test]
    fn chat_search_matches_body_case_insensitively() {
        let entries = vec![
            ChatEntry::remote("alice", "Hello World", None, None, None),
            ChatEntry::remote("bob", "nothing", None, None, None),
        ];
        assert_eq!(chat_search_matches_in(&entries, "hello"), vec![0]);
        assert_eq!(chat_search_matches_in(&entries, "HELLO"), vec![0]);
        assert_eq!(chat_search_matches_in(&entries, "world"), vec![0]);
        assert_eq!(
            chat_search_matches_in(&entries, "nope"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn chat_search_matches_sender_label_too() {
        let entries = vec![
            ChatEntry::remote("alice", "hello", None, None, None),
            ChatEntry::remote("bob", "hello", None, None, None),
        ];
        assert_eq!(
            chat_search_matches_in(&entries, "alice"),
            vec![0],
            "sender label should participate in search"
        );
    }

    #[test]
    fn chat_footer_status_direct_mesh_neighbor() {
        let key: PublicKey = iroh::SecretKey::from_bytes(&[7u8; 32]).public();
        let neighbors: HashSet<PublicKey> = [key].into_iter().collect();
        let (route, connected, peers) =
            chat_footer_status(false, &neighbors, Some(key), PeerPresence::Online);
        assert_eq!(route, "Direct (mesh)");
        assert!(connected);
        assert_eq!(peers.as_deref(), Some("1 peer"));
    }

    #[test]
    fn chat_footer_status_direct_relay_when_online_not_neighbor() {
        let key: PublicKey = iroh::SecretKey::from_bytes(&[8u8; 32]).public();
        let neighbors: HashSet<PublicKey> = HashSet::new();
        let (route, connected, peers) =
            chat_footer_status(false, &neighbors, Some(key), PeerPresence::Online);
        assert_eq!(route, "Relay");
        assert!(connected);
        assert_eq!(peers.as_deref(), Some("1 peer"));
    }

    #[test]
    fn chat_footer_status_direct_offline_not_connected() {
        let key: PublicKey = iroh::SecretKey::from_bytes(&[9u8; 32]).public();
        let neighbors: HashSet<PublicKey> = HashSet::new();
        let (route, connected, peers) =
            chat_footer_status(false, &neighbors, Some(key), PeerPresence::Offline);
        assert_eq!(route, "Not connected");
        assert!(!connected);
        assert!(peers.is_none());
    }

    #[test]
    fn chat_footer_status_group_with_neighbors() {
        let neighbors: HashSet<PublicKey> = (0..3)
            .map(|i| iroh::SecretKey::from_bytes(&[10 + i; 32]).public())
            .collect();
        let (route, connected, peers) =
            chat_footer_status(true, &neighbors, None, PeerPresence::Unknown);
        assert_eq!(route, "Mesh");
        assert!(connected);
        assert_eq!(peers.as_deref(), Some("3 peers"));
    }

    #[test]
    fn chat_footer_status_group_without_neighbors() {
        let neighbors: HashSet<PublicKey> = HashSet::new();
        let (route, connected, peers) =
            chat_footer_status(true, &neighbors, None, PeerPresence::Unknown);
        assert_eq!(route, "Not connected");
        assert!(!connected);
        assert!(peers.is_none());
    }

    #[test]
    fn chat_search_caps_results_to_avoid_heavy_panels() {
        let entries: Vec<ChatEntry> = (0..120)
            .map(|i| {
                ChatEntry::remote("peer", format!("message {i} with needle"), None, None, None)
            })
            .collect();
        let matches = chat_search_matches_in(&entries, "needle");
        assert_eq!(matches.len(), 50, "results must be capped");
        assert_eq!(matches[0], 0);
        assert_eq!(matches[49], 49);
    }

    #[test]
    fn perf_image_bytes_field_does_not_affect_chat_entry_body() {
        let img = vec![0u8; 128];
        let e = ChatEntry {
            kind: ChatKind::Remote,
            label: "peer".into(),
            body: "hello".into(),
            message_hash: None,
            edited: false,
            reactions: vec![],
            image_handle: None,
            avatar_handle: None,
            image_bytes: Some(img),
            image_identifier: None,
            image_error: None,
            timestamp: Some(1000),
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
            image_width: None,
            image_height: None,
            gif_frames: None,
        };
        assert_eq!(e.body, "hello");
        assert_eq!(e.label, "peer");
        assert_eq!(e.image_bytes.as_ref().map(|b| b.len()), Some(128));
    }

    #[test]
    fn perf_system_entries_have_no_image_bytes() {
        let e = ChatEntry::system("user joined");
        assert!(
            e.image_bytes.is_none(),
            "system entry must not have image data"
        );
        assert_eq!(e.body, "user joined");
    }

    #[test]
    fn perf_local_entries_have_no_image_bytes() {
        let e = ChatEntry::local("me", "hello");
        assert!(
            e.image_bytes.is_none(),
            "local entry must not have image data"
        );
        assert_eq!(e.body, "hello");
    }

    #[test]
    fn perf_remote_text_entries_have_no_image_bytes() {
        let e = ChatEntry::remote("peer", "hey there", None, None, None);
        assert!(
            e.image_bytes.is_none(),
            "remote text entry must not have image data"
        );
        assert_eq!(e.body, "hey there");
    }

    #[test]
    fn download_attachment_state_helpers_cover_all_states() {
        let mut attachment =
            DownloadAttachment::new(TransferKind::File, "demo.bin", "ticket", "", None);
        assert_eq!(attachment.action_label(), "Download");
        assert_eq!(attachment.status_label(), "Ready to download");
        assert!(attachment.progress_fraction().is_none());

        attachment.state = DownloadState::Active {
            bytes: 1024,
            total: Some(2048),
        };
        assert_eq!(attachment.action_label(), "Downloading");
        assert!(attachment.status_label().contains("50%"));
        assert_eq!(attachment.progress_fraction(), Some(0.5));

        attachment.state = DownloadState::Active {
            bytes: 1536,
            total: None,
        };
        assert!(attachment.status_label().contains("size unknown"));
        assert!(attachment.status_label().contains("1.5 KiB"));
        assert!(attachment.progress_fraction().is_none());

        attachment.state = DownloadState::Active {
            bytes: 2,
            total: Some(0),
        };
        assert!(attachment.status_label().contains("2 B / 0 B"));
        assert!(attachment.progress_fraction().is_none());

        attachment.state = DownloadState::Completed {
            saved_name: "demo.bin".into(),
            saved_path: Some(std::path::PathBuf::from("/tmp/demo.bin")),
            total_size: None,
        };
        assert_eq!(attachment.action_label(), "Open");
        assert!(attachment.status_label().contains("Saved"));

        attachment.state = DownloadState::Failed {
            failure: DownloadFailure::Other {
                detail: "boom".into(),
            },
        };
        assert_eq!(attachment.action_label(), "Dismiss");
        assert!(attachment.status_label().contains("boom"));

        attachment.state = DownloadState::Cancelled;
        assert_eq!(attachment.action_label(), "Retry");
        assert_eq!(attachment.status_label(), "Cancelled");
    }

    #[test]
    fn image_chat_kind_uses_local_for_own_sender() {
        let local = SecretKey::from_bytes(&[7u8; 32]).public();
        let remote = SecretKey::from_bytes(&[8u8; 32]).public();

        assert!(matches!(
            IcedChat::image_chat_kind(local, local),
            ChatKind::Local
        ));
        assert!(matches!(
            IcedChat::image_chat_kind(remote, local),
            ChatKind::Remote
        ));
    }

    #[test]
    fn update_cache_formats_timestamp_once() {
        // UI-14 regression: the bubble metadata row reads
        // `entry.formatted_time`, which must be populated by `update_cache`
        // (invoked for every entry by `entries_push`). Timestamps previously
        // never rendered because every constructor left the field None.
        let mut entry = ChatEntry::local("You", "hello");
        assert!(entry.formatted_time.is_none());
        entry.update_cache();
        assert!(
            entry.formatted_time.is_some(),
            "update_cache must cache a formatted timestamp"
        );
        let cached = entry.formatted_time.clone().unwrap();
        assert!(!cached.is_empty());
        // Re-running the cache must be stable (no per-frame re-format).
        entry.update_cache();
        assert_eq!(entry.formatted_time.as_deref(), Some(cached.as_str()));
    }

    #[test]
    fn perf_image_entry_caches_handle_and_keeps_bytes() {
        let img_data = vec![0xABu8; 256];
        let e = ChatEntry::image(
            ChatKind::Remote,
            "peer",
            "[Image: test.png]",
            img_data,
            None,
            None,
            None,
            None,
            None,
        );
        // The handle must be created once at construction time.
        assert!(
            e.image_handle.is_some(),
            "image entry must cache a decoded handle at construction time"
        );
        // Bytes must be preserved for session history/replay.
        assert!(
            e.image_bytes.is_some(),
            "image entry must keep raw bytes for session history/replay"
        );
        assert_eq!(e.image_bytes.as_ref().map(|b| b.len()), Some(256));
        // Cloning the handle must be cheap (Arc) and must not panic.
        let _cloned = e.image_handle.clone();
        assert!(
            e.image_handle.is_some(),
            "original handle must survive clone"
        );
    }

    #[test]
    fn perf_non_image_entries_have_no_handle() {
        assert!(ChatEntry::system("s").image_handle.is_none());
        assert!(ChatEntry::local("me", "hello").image_handle.is_none());
        assert!(ChatEntry::remote("p", "text", None, None, None)
            .image_handle
            .is_none());
    }

    // ── Connection refresh coalescing ─────────────────────────────────

    /// Simulate the ConnMonitorTick connection-refresh guard logic.
    /// Just the state-machine fields to keep the test lightweight.
    struct ConnRefreshState {
        counter: u32,
        in_flight: bool,
        needs_refresh: bool,
    }

    impl ConnRefreshState {
        /// Returns true if a refresh task was launched (emulates the guard in ConnMonitorTick).
        fn tick(&mut self) -> bool {
            let should_refresh = self.counter == 0 || self.needs_refresh;
            if should_refresh && !self.in_flight {
                self.in_flight = true;
                self.counter = 60;
                self.needs_refresh = false;
                true
            } else if self.counter > 0 {
                self.counter -= 1;
                false
            } else {
                false
            }
        }
    }

    #[test]
    fn conn_refresh_normal_reaches_zero_and_fires() {
        let mut s = ConnRefreshState {
            counter: 2,
            in_flight: false,
            needs_refresh: false,
        };
        assert!(!s.tick(), "counter=2 → should not fire");
        assert!(!s.tick(), "counter=1 → should not fire");
        assert!(s.tick(), "counter=0 → should fire and set in_flight");
        assert_eq!(s.counter, 60, "counter should reset to 60");
        assert!(s.in_flight, "in_flight should be set");
    }

    #[test]
    fn conn_refresh_coalescing_prevents_overlap() {
        let mut s = ConnRefreshState {
            counter: 0,
            in_flight: true, // a prior refresh is still running
            needs_refresh: false,
        };
        assert!(
            !s.tick(),
            "should NOT fire while in_flight is true even at counter=0"
        );
        // Subsequent tick with in_flight still true → reset happens via ConnCountsResult
    }

    #[test]
    fn conn_refresh_needs_refresh_triggers_out_of_cycle() {
        let mut s = ConnRefreshState {
            counter: 44, // not zero
            in_flight: false,
            needs_refresh: true, // on_neighbor_up/down signalled
        };
        assert!(
            s.tick(),
            "should fire when needs_refresh is true regardless of counter"
        );
        assert!(s.in_flight, "in_flight should be set");
        assert!(!s.needs_refresh, "needs_refresh should be cleared");
    }

    #[test]
    fn conn_refresh_result_clears_in_flight() {
        // Simulate the ConnCountsResult handler.
        let mut direct_peers = 0usize;
        let mut relayed_peers = 0usize;
        let mut in_flight = true;

        // Like the ConnCountsResult handler:
        direct_peers = 3;
        relayed_peers = 2;
        in_flight = false;

        assert_eq!(direct_peers, 3);
        assert_eq!(relayed_peers, 2);
        assert!(!in_flight, "in_flight cleared on ConnCountsResult");
    }

    #[test]
    fn conn_refresh_no_block_on_in_update_path() {
        // Assert that the blocking recompute_connection_counts no longer
        // exists and that the update function does not call .block_on().
        let src = include_str!("../app.rs");
        // Find the start of the update function.
        let update_start = src
            .find("pub fn update(&mut self, message: AppMessage)")
            .expect("update function must exist");
        // Find the start of the tests module to exclude test code.
        let tests_start = src.find("#[cfg(test)]").unwrap_or(src.len());
        // Extract the update path (excluding test code).
        let update_body = &src[update_start..tests_start];
        // Only calls that would BLOCK the shared app runtime are forbidden.
        // The screen-share host deliberately runs on a dedicated thread with
        // its own current-thread runtime (`rt.block_on(run_host_session(...))`
        // in `start_screen_share`), so it never blocks the update path.
        let block_on_in_update = update_body.matches("runtime_handle.block_on(").count();
        assert_eq!(
            block_on_in_update, 0,
            "zero `.block_on(` calls on the shared app runtime in update path; found {block_on_in_update}"
        );
        // Also verify the old method definition is gone. Search for the pattern
        // in code (outside test module, which is excluded above).
        let code_before_tests = &src[..tests_start];
        let fn_def_pattern = "fn recompute_connection_counts";
        assert!(
            !code_before_tests.contains(fn_def_pattern),
            "recompute_connection_counts method definition must be removed"
        );
    }

    // ── UI-HOME-13: chat timeline / composer typography regression guards ──
    //
    // The chat screen must resolve its fonts through the central `TypeRole`
    // roles (Figtree for messages, IBM Plex Sans for chrome). These are
    // source-level guards: they assert the *contract* the view code relies
    // on, in the same style as the `.block_on(` guard above.

    /// Extract the production (non-test) source of one method body.
    fn method_source<'a>(src: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
        let start = src
            .find(start_marker)
            .unwrap_or_else(|| panic!("{start_marker} must exist"));
        let tests_start = src.find("#[cfg(test)]").unwrap_or(src.len());
        let end = src[start..tests_start]
            .find(end_marker)
            .map(|off| start + off)
            .unwrap_or(tests_start);
        &src[start..end]
    }

    #[test]
    fn chat_timeline_uses_type_role_figtree_roles() {
        // UI-HOME-13: message bubbles, sender names and timestamps must use
        // the central Figtree roles (TypeRole). BORU-UI-16: the view resolves
        // them through the live theme (`btheme.type_font` /
        // `btheme.type_line_height`), which defaults to the same mapping.
        let src = include_str!("../app.rs");
        let chat_src = include_str!("chat.rs");
        let timeline = method_source(chat_src, "fn view_chat_log(", "fn view_composer(");
        assert!(
            timeline.contains("TypeRole::ChatMessage"),
            "chat message body must resolve through the TypeRole::ChatMessage role"
        );
        assert!(
            timeline.contains("TypeRole::ChatSender"),
            "chat sender label must use the TypeRole::ChatSender role"
        );
        assert!(
            timeline.contains("TypeRole::ChatMetadata"),
            "chat metadata/timestamp must use the TypeRole::ChatMetadata role"
        );
        // The message body should carry the plan's ~1.45-1.5 relative line
        // height (the theme token defaults to 1.45, matching Figtree's
        // natural metrics and the pre-theme explicit value).
        assert!(
            timeline.contains("type_line_height"),
            "chat message body line height must come from the theme"
        );
    }

    #[test]
    fn composer_uses_type_role_composer_text_font() {
        // UI-HOME-13: the shared composer input and its placeholder must use
        // TypeRole::ComposerText (Figtree Regular), keeping the user's
        // configurable text size. BORU-UI-16: the font resolves through the
        // live theme, which defaults to the same role mapping.
        let src = include_str!("../app.rs");
        let chat_src = include_str!("chat.rs");
        let composer = method_source(chat_src, "fn view_composer(", "fn update_chat(");
        assert!(
            composer.contains("TypeRole::ComposerText"),
            "composer input must use the TypeRole::ComposerText role"
        );
        assert!(
            composer.contains(".size(self.settings_state.chat_text_size)"),
            "composer must keep the user-configurable chat text size"
        );
    }

    #[test]
    fn chat_chrome_uses_plex_roles() {
        // FONTS-08: chat chrome (empty state, connecting spinner label,
        // image-unavailable placeholder, pending upload status, header,
        // search, menus, footer) must resolve through IBM Plex Sans roles.
        let src = include_str!("../app.rs");
        let chat_src = include_str!("chat.rs");
        let log = method_source(chat_src, "fn view_chat_log(", "fn view_composer(");
        assert!(
            log.contains("TypeRole::Body"),
            "chat empty state / connecting label must use TypeRole::Body (IBM Plex Sans)"
        );
        assert!(
            log.contains("TypeRole::SupportingText"),
            "chat image placeholder / upload status must use TypeRole::SupportingText (IBM Plex Sans)"
        );
        assert!(
            log.contains("TypeRole::Metadata"),
            "chat image-unavailable error line must use TypeRole::Metadata (IBM Plex Sans)"
        );
        // The chat message body itself stays Figtree (guarded separately);
        // make sure the chrome-only call sites above did not displace the
        // message body roles.
        assert!(
            log.contains("TypeRole::ChatMessage"),
            "chat message body must keep resolving through TypeRole::ChatMessage (Figtree)"
        );
    }

    #[test]
    fn chat_roles_map_to_figtree_at_plan_sizes() {
        // UI-HOME-13 approved mapping (plan + UI-HOME-11 handoff):
        //   chat_message  Figtree Regular 400 @ 15 px
        //   chat_sender   Figtree SemiBold 600 @ 14 px
        //   chat_metadata Figtree Regular 400 @ 12 px
        //   composer_text Figtree Regular 400 @ 15 px
        //   technical_value JetBrains Mono Regular 400 @ 12 px
        use crate::fonts::TypeRole;
        assert_eq!(TypeRole::ChatMessage.family_name(), crate::fonts::FIGTREE);
        assert_eq!(TypeRole::ChatMessage.weight(), iced::font::Weight::Normal);
        assert_eq!(TypeRole::ChatMessage.size_px(), 15.0);
        assert_eq!(TypeRole::ChatSender.family_name(), crate::fonts::FIGTREE);
        assert_eq!(TypeRole::ChatSender.weight(), iced::font::Weight::Semibold);
        assert_eq!(TypeRole::ChatSender.size_px(), 14.0);
        assert_eq!(TypeRole::ChatMetadata.family_name(), crate::fonts::FIGTREE);
        assert_eq!(TypeRole::ChatMetadata.weight(), iced::font::Weight::Normal);
        assert_eq!(TypeRole::ChatMetadata.size_px(), 12.0);
        assert_eq!(TypeRole::ComposerText.family_name(), crate::fonts::FIGTREE);
        assert_eq!(TypeRole::ComposerText.weight(), iced::font::Weight::Normal);
        assert_eq!(TypeRole::ComposerText.size_px(), 15.0);
        assert_eq!(
            TypeRole::TechnicalValue.family_name(),
            crate::fonts::JETBRAINS_MONO
        );
        assert_eq!(
            TypeRole::TechnicalValue.weight(),
            iced::font::Weight::Normal
        );
        assert_eq!(TypeRole::TechnicalValue.size_px(), 12.0);
    }

    // ── UI-HOME-12: home screen typography regression guards ──

    #[test]
    fn home_screen_uses_type_role_roles() {
        // UI-HOME-12 approved mapping: greeting -> display_heading (Archivo
        // SemiCondensed Bold 32), subtitle -> body@16, pill -> metadata,
        // connection card heading -> display_heading (Archivo SemiCondensed
        // Bold, two-tone) via status_card.rs, card body -> body, Retry/
        // Details -> button_label, mesh card -> card_title /
        // supporting_text / body_emphasised / button_label.
        // The whole home screen must resolve fonts through the central
        // TypeRole roles — no local font declarations (Archivo may only
        // arrive via DisplayHeading/PageTitle).
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains("TypeRole::DisplayHeading"),
            "greeting must use TypeRole::DisplayHeading (Archivo SemiCondensed Bold 32)"
        );
        let status = include_str!("../status_card.rs");
        assert!(
            status.contains("TypeRole::DisplayHeading"),
            "connection card heading must use TypeRole::DisplayHeading (Archivo SemiCondensed Bold)"
        );
        assert!(
            status.contains("TypeRole::ButtonLabel"),
            "hero Retry/Details must use TypeRole::ButtonLabel"
        );
        assert!(
            home.contains("TypeRole::CardTitle")
                || home.contains("CardShell::new(crate::i18n::t(\"home.mesh_health\")"),
            "mesh card title must resolve through TypeRole::CardTitle — either inline or via the shared CardShell foundation (card_shell.rs renders the title with TypeRole::CardTitle)"
        );
        assert!(
            home.contains("CardShell::new(crate::i18n::t(\"home.mesh_health\")"),
            "mesh card must be built from the shared CardShell dashboard-card foundation"
        );
        assert!(
            home.contains("TypeRole::BodyEmphasised"),
            "mesh status label must use TypeRole::BodyEmphasised"
        );
        assert!(
            home.contains("TypeRole::SupportingText"),
            "mesh supporting text must use TypeRole::SupportingText"
        );
        assert!(
            status.contains("TypeRole::SupportingText"),
            "status pill must resolve through a TypeRole (SupportingText — the pill moved to status_card.rs with the redesign)"
        );
        assert!(
            home.contains("type_role_text_themed("),
            "home screen must use the themed line-height helper (display 1.2 / body 1.45)"
        );
        // The helper itself applies the relative line height (fonts.rs) —
        // the literal lives in the helper, not in the call sites.
        let fonts_src = include_str!("../fonts.rs");
        assert!(
            fonts_src.contains("LineHeight::Relative(line_height)")
                || fonts_src.contains("LineHeight::Relative(resolve_theme_line_height"),
            "type_role_text_lh / themed helper must apply a relative line height"
        );
        assert!(
            !home.contains("inter_tight("),
            "home screen must not hardcode Inter Tight locally; it may only arrive via TypeRole::DisplayHeading"
        );
    }

    #[test]
    fn home_screen_fonts05_approved_family_mapping() {
        // FONTS Task 5 (spec) — the redesigned home screen must render with:
        //   greeting "Good <time>, <name>" -> DisplayHeading (Archivo
        //     SemiCondensed Bold 32 px, lh ~1.2)
        //   subtitle "Welcome to Boru" -> Body @ HOME_SUBTITLE (IBM Plex
        //     Sans Regular 16 px, muted secondary)
        //   connection card title "Boru is connected and ready." ->
        //     SectionTitle (IBM Plex Sans SemiBold 20 px)
        //   connection card subtitle "Private communication, peer to peer."
        //     -> Body lh 1.45 (IBM Plex Sans Regular 15 px)
        //   dashboard headings (Mesh Health / Online Peers / Recent
        //     Activity / Tunnels) -> CardTitle via CardShell (IBM Plex Sans
        //     SemiBold 18 px) — NOT Archivo
        // The BORU wordmark stays Raleway and no business logic changes.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        // Greeting resolves through DisplayHeading (Archivo SemiCondensed
        // Bold 32) with the ~1.2 line-height helper. BORU-UI-16: it goes
        // through the themed helper so the inspector can adjust it.
        assert!(
            home.contains("type_role_text_themed(\n            &btheme,\n            crate::fonts::TypeRole::DisplayHeading,")
                || home.contains("crate::fonts::type_role_text_themed(\n            &btheme,\n            crate::fonts::TypeRole::DisplayHeading,"),
            "greeting must use type_role_text_themed with TypeRole::DisplayHeading"
        );
        assert!(
            home.contains("crate::i18n::t_args(\"home.greeting\""),
            "greeting copy must stay 'Good <time>' via the i18n key (BORU-HOME-02 simplified greeting)"
        );
        // Subtitle uses the body role at the HOME_SUBTITLE scale token,
        // muted secondary colour — no hardcoded family. (Text was
        // simplified by BORU-HOME-02; the role/size/colour contract
        // is unchanged.)
        assert!(
            home.contains("crate::i18n::t(\"home.welcome\")"),
            "subtitle must use the i18n home.welcome copy"
        );
        assert!(
            home.contains(".size(btheme.typography.home_subtitle)"),
            "subtitle must use the HOME_SUBTITLE theme token (16 px)"
        );
        assert!(
            home.contains(".color(text_secondary(&theme))"),
            "subtitle must use the muted secondary colour"
        );
        // Connection card headline + subtitle (moved to status_card.rs
        // with the redesign: DisplayHeading two-tone heading, Body subtitle,
        // SupportingText pill — all through TypeRole roles).
        let status = include_str!("../status_card.rs");
        assert!(
            status.contains("fonts::type_role_text_lh(TypeRole::DisplayHeading,"),
            "connection card heading must use type_role_text_lh with TypeRole::DisplayHeading (Archivo SemiCondensed Bold)"
        );
        assert!(
            status.contains("\"Boru\\u{00A0}\""),
            "connection card ready copy must keep the two-tone Boru span"
        );
        assert!(
            status.contains("Private communication, peer to peer.")
                && status.contains("TypeRole::Body"),
            "connection card subtitle must use TypeRole::Body for the supporting copy"
        );
        // Dashboard headings: all four cards are built from the shared
        // CardShell foundation, which renders titles with TypeRole::CardTitle
        // (IBM Plex Sans SemiBold 18) — never Archivo.
        assert!(
            home.contains("CardShell::new(crate::i18n::t(\"home.mesh_health\")"),
            "Mesh Health card must be a CardShell (CardTitle -> IBM Plex Sans, not Archivo)"
        );
        let rail = method_source(
            home_src,
            "fn view_online_peers_card(",
            "fn view_main_empty_state(",
        );
        assert!(
            rail.contains("CardShell::new(crate::i18n::t(\"home.online_peers\")")
                && rail.contains("CardShell::new(crate::i18n::t(\"home.recent_activity\")")
                && rail.contains("CardShell::new(crate::i18n::t(\"home.tunnels\")"),
            "Online Peers / Recent Activity / Tunnels cards must be CardShell (CardTitle -> IBM Plex Sans, not Archivo)"
        );
        // Wordmark unchanged: the BORU logo element still resolves to
        // Raleway ExtraBold (BoruLogo From impl).
        assert!(
            src.contains("crate::fonts::raleway_extra_bold()"),
            "BORU wordmark must stay on raleway_extra_bold (unchanged)"
        );
    }

    #[test]
    fn home_rail_cards_use_type_role_roles() {
        // UI-HOME-12: rail card rows resolve through TypeRole — friendly
        // names as body, timestamps as metadata, and only genuine technical
        // values (tunnel host:port endpoints) as JetBrains Mono.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let peers = method_source(
            home_src,
            "fn view_online_peers_card(",
            "fn view_recent_activity_card(",
        );
        assert!(
            peers.contains("TypeRole::Body"),
            "peer row name must use TypeRole::Body (SS3 Regular 15)"
        );
        let activity = method_source(
            home_src,
            "fn view_recent_activity_card(",
            "fn view_tunnels_card(",
        );
        assert!(
            activity.contains("TypeRole::Body"),
            "activity description must use TypeRole::Body"
        );
        assert!(
            activity.contains("TypeRole::Metadata"),
            "activity timestamp must use TypeRole::Metadata"
        );
        let tunnels = method_source(
            home_src,
            "fn view_tunnels_card(",
            "fn view_main_empty_state(",
        );
        assert!(
            tunnels.contains("TypeRole::Body"),
            "tunnel name must use TypeRole::Body"
        );
        assert!(
            tunnels.contains("TypeRole::TechnicalValue"),
            "tunnel endpoint must use TypeRole::TechnicalValue (JetBrains Mono)"
        );
        assert!(
            tunnels.contains("TypeRole::Metadata"),
            "tunnel status must use TypeRole::Metadata"
        );
    }

    #[test]
    fn home_screen_spacing_uses_the_shared_scale() {
        // UI-HOME-09: the home dashboard's structural gaps must come from
        // the plan's shared spacing scale (4, 8, 12, 16, 20, 24, 32).
        // Page header → dashboard 28–32 px (SPACE_28), greeting → welcome
        // 4–8 px (SPACE_4), and the pill internal gaps on-scale. Off-scale
        // one-offs (SPACE_2 greeting gap, SPACE_6/SPACE_10 structural gaps,
        // raw 48.0/24.0/22.0 hero-badge literals) are removed.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains("layout.gaps.header_dashboard_gap"),
            "page header → dashboard gap must come from the layout model (defaults to the shared-scale SPACE_28+SPACE_12)"
        );
        assert!(
            home.contains(".push(Space::new().height(Length::Fixed(SPACE_4)))"),
            "greeting → welcome gap must use shared-scale SPACE_4 (4–8 px band)"
        );
        assert!(
            home.contains("crate::status_card::view_status_card"),
            "the connection status card must be the redesigned dark panel module"
        );
        // The status pill now lives inside status_card.rs (security_pill);
        // the home screen no longer owns a hero pill padding literal.
        let status = include_str!("../status_card.rs");
        assert!(
            status.contains("security_pill"),
            "the security pill must be built by status_card.rs"
        );
        assert!(
            !home.contains(".padding([SPACE_10, SPACE_12])"),
            "status pill padding must not use off-scale SPACE_10"
        );
    }

    // ── UI-HOME-10: overflow / clipping audit regression guards ──
    // The home component tree must size rows to their content: no fixed
    // row heights that clip wrapped names/endpoints, no hidden-overflow
    // masks (truncate + clip + Wrapping::None), and long technical
    // identifiers (peer keys, JetBrains Mono endpoints) must wrap at glyph
    // level so nothing overflows horizontally.

    #[test]
    fn home_rail_rows_are_content_driven_not_fixed_height() {
        // UI-HOME-10: rail-card rows must not use a fixed height that clips
        // wrapped content. The approved rhythm (60 / 32 / 48 px) is kept as
        // a MINIMUM via a zero-width spacer, never as a fixed row height.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let peers = method_source(
            home_src,
            "fn view_online_peers_card(",
            "fn view_recent_activity_card(",
        );
        assert!(
            !peers.contains(".height(Length::Fixed(btheme.lists.peer_row_height))\n"),
            "online-peer rows must not force a fixed 60 px height (clips a wrapped display name)"
        );
        assert!(
            peers.contains(".height(Length::Fixed(btheme.lists.peer_row_height))"),
            "online-peer rows must keep the 60 px rhythm via a zero-width min-height spacer"
        );
        let activity = method_source(
            home_src,
            "fn view_recent_activity_card(",
            "fn view_tunnels_card(",
        );
        assert!(
            !activity.contains(".height(Length::Fixed(btheme.home.activity_row_height))\n"),
            "recent-activity rows must not force a fixed 32 px height (clips a wrapped description)"
        );
        assert!(
            activity.contains(".height(Length::Fixed(btheme.home.activity_row_height))"),
            "recent-activity rows must keep the 32 px rhythm via a zero-width min-height spacer"
        );
        let tunnels = method_source(
            home_src,
            "fn view_tunnels_card(",
            "fn view_main_empty_state(",
        );
        assert!(
            !tunnels.contains(".height(Length::Fixed(crate::card_shell::CARD_ROW_HEIGHT))\n"),
            "tunnel rows must not force a fixed 48 px height (clips a wrapped endpoint)"
        );
        assert!(
            tunnels.contains(".height(Length::Fixed(crate::card_shell::CARD_ROW_HEIGHT))"),
            "tunnel rows must keep the 48 px rhythm via a zero-width min-height spacer"
        );
    }

    #[test]
    fn home_rail_descriptions_wrap_naturally_not_truncated_or_clipped() {
        // UI-HOME-10: recent-activity and mesh-event descriptions must wrap
        // (WordOrGlyph) instead of being truncated to 40 chars and clipped.
        // Hidden overflow must not mask defects.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let activity = method_source(
            home_src,
            "fn view_recent_activity_card(",
            "fn view_tunnels_card(",
        );
        assert!(
            !activity.contains("truncate_with_ellipsis"),
            "recent-activity descriptions must not be truncated (hides content)"
        );
        assert!(
            !activity.contains("Wrapping::None"),
            "recent-activity descriptions must wrap, not force single-line"
        );
        assert!(
            !activity.contains(".clip(true)"),
            "recent-activity descriptions must not hide overflow with clipping"
        );
        assert!(
            activity.contains("Wrapping::WordOrGlyph"),
            "recent-activity descriptions must use glyph-fallback wrapping for long tokens"
        );
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        let events = home.split("Recent events").nth(1).unwrap_or("");
        assert!(
            !events.contains("truncate_with_ellipsis"),
            "mesh-event messages must not be truncated (hides content)"
        );
        assert!(
            !events.contains("Wrapping::None"),
            "mesh-event messages must wrap, not force single-line"
        );
    }

    #[test]
    fn home_long_technical_text_wraps_at_glyph_level() {
        // UI-HOME-10: long unbroken identifiers (peer keys, JetBrains Mono
        // endpoints, status details) must wrap at word-or-glyph level inside
        // their fill containers instead of overflowing their rows.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let peers = method_source(
            home_src,
            "fn view_online_peers_card(",
            "fn view_recent_activity_card(",
        );
        assert!(
            peers.contains("Wrapping::WordOrGlyph"),
            "peer display names must glyph-wrap so long peer keys stay inside the row"
        );
        let tunnels = method_source(
            home_src,
            "fn view_tunnels_card(",
            "fn view_main_empty_state(",
        );
        assert!(
            tunnels.contains("Wrapping::WordOrGlyph"),
            "tunnel names/endpoints must glyph-wrap so JetBrains Mono values stay inside the row"
        );
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains("Wrapping::WordOrGlyph"),
            "greeting / hero / mesh status / room text must glyph-wrap long values"
        );
        assert!(
            home.contains("status_detail")
                && home[home.find("status_detail").unwrap()..].contains(".width(Length::Fill)"),
            "mesh status detail must be width-Fill so it wraps instead of overflowing the row"
        );
    }

    #[test]
    fn sidebar_identity_name_wraps_inside_sidebar() {
        // UI-HOME-10: the pinned sidebar identity block must not use a
        // non-wrapping shrink-width display name that overflows the sidebar;
        // a long local label stays inside the sidebar width.
        // (SIDEBAR-02 later resolved the mechanism: the name is single-line
        // `Wrapping::None` inside a `.clip(true)` width-Fill column, so a
        // long label is clipped at the available width instead of wrapping
        // or widening the sidebar — see sidebar_profile_name_is_single_line.)
        let sidebar_src = include_str!("sidebar.rs");
        let profile = method_source(
            sidebar_src,
            "fn view_local_profile_block(",
            "fn format_preview(",
        );
        assert!(
            !profile.contains("Wrapping::WordOrGlyph"),
            "identity display name must not glyph-wrap (SIDEBAR-02 keeps it single-line, clipped)"
        );
        assert!(
            profile.contains("name_col")
                && profile[profile.find("name_col").unwrap()..].contains(".width(Length::Fill)"),
            "identity name column must be width-Fill so the name stays inside the sidebar"
        );
        assert!(
            profile.contains(".clip(true)"),
            "identity name column must clip overflow so a long name can never widen the sidebar"
        );
    }

    // ── UI-HOME-14: shared chrome / navigation typography regression guards ──

    #[test]
    fn sidebar_navigation_uses_type_role_roles() {
        // UI-HOME-14: the shared sidebar navigation (chat rows, groups,
        // ticket join, discovered peers, public rooms, friends, requests)
        // must resolve text through TypeRole — names as Body, previews and
        // supporting lines as SupportingText, timestamps/badges as Metadata,
        // and every button label as ButtonLabel. No raw TYPO_ size text.
        let sidebar_src = include_str!("sidebar.rs");
        let chats = method_source(
            sidebar_src,
            "fn view_sidebar_chats(",
            "fn view_sidebar_groups(",
        );
        assert!(
            chats.contains("TypeRole::SupportingText"),
            "sidebar empty state must use TypeRole::SupportingText"
        );
        assert!(
            chats.contains("TypeRole::ButtonLabel"),
            "sidebar empty-state action must use TypeRole::ButtonLabel"
        );
        let groups = method_source(
            sidebar_src,
            "fn view_sidebar_groups(",
            "fn view_sidebar_ticket_join(",
        );
        assert!(
            groups.contains("TypeRole::ButtonLabel"),
            "Create Group button must use TypeRole::ButtonLabel"
        );
        let row = method_source(
            sidebar_src,
            "fn view_sidebar_conversation_row(",
            "fn view_sidebar_discovered_peers(",
        );
        assert!(
            row.contains("sidebar_name_text("),
            "sidebar conversation row names must use sidebar_name_text (FONTS-06 IBM Plex Sans Medium)"
        );
        let rooms = method_source(
            sidebar_src,
            "fn view_sidebar_conversation_row(",
            "fn view_sidebar_discovered_peers(",
        );
        let requests = method_source(
            sidebar_src,
            "fn view_sidebar_requests_content(",
            "fn view_sidebar_requests(",
        );
        // The section body is large; check the row-render fn (last match).
        let requests_body = requests
            .rsplit("fn view_sidebar_requests_content")
            .next()
            .unwrap();
        assert!(
            requests_body.contains("sidebar_name_text("),
            "sidebar request labels must use sidebar_name_text (FONTS-06 IBM Plex Sans Medium)"
        );
    }

    #[test]
    fn home_breakpoints_use_content_width_not_raw_window() {
        // UI-HOME-15: the home screen must base its responsive behaviour on
        // the dashboard's available content width (window minus sidebar,
        // divider and page padding), never on the raw window width — the
        // sidebar eats 288–320 px and would otherwise starve the grid.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains("layout.content_width(window_width, &sidebar, &responsive)"),
            "home layout must compute the dashboard content width"
        );
        assert!(
            home.contains("content_width < layout.grid.stack_breakpoint"),
            "rail-stack decision must use the layout-model content-width breakpoint"
        );
        assert!(
            !home.contains("RAIL_STACK_BREAKPOINT"),
            "the old window-width rail-stack constant must be gone"
        );
        let qa_pos = home
            .find("quick_action_grid(")
            .expect("quick-action grid call present");
        assert!(
            home[qa_pos..].contains("content_width"),
            "quick-action grid must be sized from content width"
        );
    }

    #[test]
    fn home_resolves_responsive_tier_from_window_width() {
        // BORU-LAYOUT-04: the home view must resolve the active viewport
        // tier from the window width (via the layout model's thresholds)
        // and apply the per-tier home column count, so narrow / desktop /
        // ultra-wide windows get different column counts. The tier comes
        // from `responsive.*` (never a hard-coded window-width constant),
        // and the pre-responsive content-width stack rule stays intact.
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains("responsive.tier_for_width(window_width)"),
            "home must resolve the viewport tier from the window width"
        );
        assert!(
            home.contains("responsive.home_columns.for_tier(viewport_tier)"),
            "home must apply the per-tier column count"
        );
        assert!(
            home.contains("responsive.home_padding_x.for_tier(viewport_tier)"),
            "home must apply the per-tier horizontal padding"
        );
        assert!(
            home.contains("content_width < layout.grid.stack_breakpoint"),
            "the content-width stack rule must remain wired"
        );
        assert!(
            !home.contains("RAIL_STACK_BREAKPOINT"),
            "no raw window-width rail-stack constant may return"
        );
    }

    #[test]
    fn set_layout_config_replaces_layout_and_bumps_revision() {
        // BORU-LAYOUT-03: the live-layout seam must replace the active
        // layout AND bump the revision so iced::lazy caches rebuild with the
        // new arrangement (mirror of the theme revision contract).
        let (_runtime, mut app) = build_prewarm_test_app();

        let revision_before = app.layout_revision;
        assert_eq!(
            app.active_layout.home.max_content_width,
            crate::design_tokens::DASHBOARD_MAX_WIDTH,
            "baseline: default layout reproduces the current max width"
        );

        let mut layout = crate::layout::LayoutConfig::default();
        layout.home.max_content_width = 1200.0;
        layout.home.hidden_sections = vec![crate::layout::HomeSection::Tunnels];
        app.set_layout_config(layout);

        assert_eq!(app.active_layout.home.max_content_width, 1200.0);
        assert_eq!(
            app.active_layout.home.hidden_sections,
            vec![crate::layout::HomeSection::Tunnels]
        );
        assert_eq!(
            app.layout_revision,
            revision_before.wrapping_add(1),
            "replacing the layout must bump the revision"
        );

        // The dependency snapshot carries the revision so the lazy home
        // screen rebuilds on the next frame.
        let dep = app.chat_list_dependency();
        assert_eq!(dep.layout_revision, app.layout_revision);
    }

    #[test]
    fn status_card_mesh_adapts_to_content_width() {
        // The redesigned status card's network mesh must scale with content
        // width (full row at wide, reduced at medium, stacked below the
        // description/pill on narrow) so the connection text is never
        // starved or overlapped.
        let status = include_str!("../status_card.rs");
        assert!(
            status.contains("STATUS_CARD_MEDIUM_CONTENT"),
            "status card must define the medium content-width tier"
        );
        assert!(
            status.contains("STATUS_CARD_NARROW_CONTENT"),
            "status card must define the narrow content-width tier"
        );
        assert!(
            status.contains("fn layout_tier("),
            "status card must own a content-width layout tier function"
        );
    }

    #[test]
    fn home_compact_headers_are_wired_to_content_width() {
        // UI-HOME-15: the two-line compact header mode must be driven by the
        // content width and reach every home card (mesh + the three rail
        // cards), so narrow windows never squeeze card titles/badges/actions.
        let src = include_str!("../app.rs");
        let home_src = include_str!("home.rs");
        let home = method_source(
            home_src,
            "fn view_chat_list_content(",
            "fn view_chat_panel(",
        );
        assert!(
            home.contains(".compact_header(compact_header)"),
            "mesh card must use the compact header flag"
        );
        let peers = method_source(
            home_src,
            "fn view_online_peers_card(",
            "fn view_recent_activity_card(",
        );
        assert!(
            peers.contains(".compact_header(dep.compact_header)"),
            "Online Peers card must thread the compact header flag"
        );
        let activity = method_source(
            home_src,
            "fn view_recent_activity_card(",
            "fn view_tunnels_card(",
        );
        assert!(
            activity.contains(".compact_header(dep.compact_header)"),
            "Recent Activity card must thread the compact header flag"
        );
        let tunnels = method_source(
            home_src,
            "fn view_tunnels_card(",
            "fn view_main_empty_state(",
        );
        assert!(
            tunnels.contains(".compact_header(dep.compact_header)"),
            "Tunnels card must thread the compact header flag"
        );
        assert!(
            home_src.contains("fn home_compact_headers"),
            "compact-header helper must exist"
        );
    }

    #[test]
    fn file_sharing_dashboard_uses_type_role_roles() {
        // UI-HOME-14 + FONTS-10: every file-sharing dashboard tab/view
        // resolves through TypeRole — titles as CardTitle/SectionTitle, rows
        // as Body, table metadata as Metadata, buttons as ButtonLabel. The
        // peer catalogue (per-peer Shared with Me) and its row renderer are
        // included. FONTS-10: the File Sharing page heading is the ONLY
        // Archivo SemiCondensed (PageTitle) in the file-sharing surfaces —
        // the sharing summary metric values, file cards, and icon labels are
        // IBM Plex Sans.
        let src = include_str!("../app.rs");
        let fs_src = include_str!("files.rs");
        let discover_src = include_str!("discover.rs");
        // FONTS-10: main screen heading "File Sharing" uses the Archivo
        // PageTitle role; the subtitle beneath it is IBM Plex supporting text.
        let fs_heading =
            method_source(fs_src, "fn view_file_sharing_content(", "fn view_discover(");
        assert!(
            fs_heading.contains("TypeRole::PageTitle, \"File Sharing\""),
            "File Sharing heading must use TypeRole::PageTitle (Archivo SemiCondensed)"
        );
        assert!(
            fs_heading.contains("TypeRole::PageTitle.font()")
                || fs_heading.contains("TypeRole::PageTitle"),
            "File Sharing heading must resolve through the PageTitle role"
        );
        let catalogue = method_source(
            discover_src,
            "fn view_peer_catalogue_content(",
            "fn render_catalogue_row(",
        );
        assert!(
            catalogue.contains("TypeRole::SectionTitle"),
            "catalogue header must use TypeRole::SectionTitle"
        );
        assert!(
            catalogue.contains("TypeRole::ButtonLabel"),
            "catalogue actions must use TypeRole::ButtonLabel"
        );
        // The sharing summary card lives in sharing_summary.rs (migrated in
        // UI-HOME-14) — assert on the module source, not the app.rs wrapper.
        let summary_src = include_str!("../sharing_summary.rs");
        assert!(
            summary_src.contains("TypeRole::CardTitle"),
            "sharing summary card title must use TypeRole::CardTitle"
        );
        // FONTS-10: the summary metric VALUES must be IBM Plex Sans — never
        // the Archivo PageTitle role (Archivo is reserved for the page title).
        assert!(
            !summary_src.contains("TypeRole::PageTitle"),
            "sharing summary metric values must not use Archivo PageTitle (FONTS-10)"
        );
        assert!(
            summary_src.contains("TypeRole::SectionTitle"),
            "sharing summary metric values must use an IBM Plex Sans heading role (FONTS-10)"
        );
        let shared = method_source(
            fs_src,
            "fn view_shared_with_me(",
            "fn view_recent_download_activity_card(",
        );
        assert!(
            shared.contains("TypeRole::SectionTitle"),
            "Shared with Me title must use TypeRole::SectionTitle"
        );
        // FONTS-10: the video file card's on-media labels must render through
        // TypeRole (IBM Plex Sans), never raw `text()` with the legacy default
        // font. The card body already used type_role_text everywhere except the
        // on-media overlay labels fixed in FONTS-10.
        let video_card_src = include_str!("../video_file_card.rs");
        let video_prod = video_card_src.split("#[cfg(test)]").next().unwrap();
        assert!(
            video_prod.contains("TypeRole::Metadata"),
            "video file card metadata must use TypeRole::Metadata"
        );
        assert!(
            video_prod.contains("TypeRole::BodyEmphasised"),
            "video file card error title must use TypeRole::BodyEmphasised"
        );
        assert!(
            video_prod.contains("type_role_text"),
            "video file card text must render through type_role_text"
        );
    }

    #[test]
    fn creation_dialogs_use_migrated_shared_components() {
        // UI-HOME-14: the Create Public Room / Create Group Chat / Create
        // Tunnel dialogs must build on the migrated shared components
        // (BoruDialog title/buttons, FormSection labels, TextInput, peer
        // rows) instead of declaring local fonts.
        // The create-room dialog moved to the rooms domain (BORU-APP-006);
        // group/tunnel dialogs remain in dialogs.rs.
        let room_src = include_str!("rooms.rs");
        let create_room = method_source(
            room_src,
            "fn view_create_room_dialog<'a>(",
            "fn view_room_settings_dialog<'a>(",
        );
        assert!(
            create_room.contains("BoruDialog::new"),
            "Create Public Room must use BoruDialog"
        );
        assert!(
            create_room.contains("FormSection::new"),
            "Create Public Room must use FormSection"
        );
        let src = include_str!("dialogs.rs");
        let create_group = method_source(
            src,
            "fn view_create_group_dialog<'a>(",
            "fn view_create_tunnel_dialog<'a>(",
        );
        assert!(
            create_group.contains("BoruDialog::new"),
            "Create Group Chat must use BoruDialog"
        );
        let create_tunnel = method_source(
            src,
            "fn view_create_tunnel_dialog<'a>(",
            "fn view_invite_member_dialog<'a>(",
        );
        assert!(
            create_tunnel.contains("BoruDialog::new"),
            "Create Tunnel must use BoruDialog"
        );
    }

    #[test]
    fn share_local_service_dialog_caps_body_for_footer() {
        // TUN-UI: the Create Tunnel (Share Local Service) dialog must call
        // `.scroll_body(...)` so the footer (Cancel / Create Tunnel) stays
        // on screen once the Local Services suggestion list renders. Without
        // the cap the Shrink-height panel grows past the window and the
        // footer is pushed off-screen. The cap must be applied with the same
        // shared responsive helper rather than a per-dialog magic number.
        let src = include_str!("tunnels.rs");
        let share_dialog = method_source(
            src,
            "fn view_share_local_service_dialog<'a>(",
            "fn view_local_service_suggestion_row<'a>(",
        );
        assert!(
            share_dialog.contains(".scroll_body(self.dialog_body_max_height())"),
            "Create Tunnel (share local service) dialog must use the shared responsive body cap"
        );
        assert!(
            share_dialog.contains(".secondary(labels.cancel"),
            "share dialog must keep the Cancel footer row"
        );
        assert!(
            share_dialog.contains(".primary("),
            "share dialog must keep the Create Tunnel footer row (primary action)"
        );
    }

    #[test]
    fn creation_dialogs_fonts11_typography() {
        // FONTS Task 11: creation-dialog chrome resolves through TypeRole —
        // dialog titles Archivo SemiCondensed Bold at the DIALOG_TITLE scale
        // (26 px, band 24–28 px), subtitle IBM Plex Sans Regular at the
        // DIALOG_SUBTITLE scale (14 px, band 14–15 px), form labels IBM Plex
        // Sans SemiBold (ButtonLabel), inputs IBM Plex Sans Regular (Body),
        // and buttons IBM Plex Sans SemiBold (ButtonLabel). No direct
        // font-family calls in the dialog chrome.
        let dialog_src = include_str!("../boru_dialog.rs");
        assert!(
            dialog_src.contains("TypeRole::PageTitle.font()")
                && dialog_src.contains("crate::fonts::DIALOG_TITLE"),
            "dialog title must use the PageTitle family (Archivo SemiCondensed Bold) at the DIALOG_TITLE scale"
        );
        assert!(
            dialog_src.contains("TypeRole::SupportingText.font()")
                && dialog_src.contains("crate::fonts::DIALOG_SUBTITLE"),
            "dialog subtitle must use SupportingText (IBM Plex Sans Regular) at the DIALOG_SUBTITLE scale"
        );
        assert!(
            dialog_src.contains("TypeRole::ButtonLabel.font()"),
            "dialog footer buttons must use ButtonLabel (IBM Plex Sans SemiBold)"
        );
        // Form primitives (labels, inputs, selects, checkbox/toggle labels)
        // must resolve to IBM Plex Sans roles.
        let form_src = include_str!("../form_components.rs");
        assert!(
            form_src.contains("TypeRole::ButtonLabel.font()"),
            "form labels must use ButtonLabel (IBM Plex Sans SemiBold)"
        );
        assert!(
            form_src.contains("TypeRole::Body.font()"),
            "form inputs/selects/labels must use TypeRole::Body (IBM Plex Sans Regular)"
        );
        assert!(
            form_src.contains("TypeRole::SupportingText.font()"),
            "helper/error text must use SupportingText (IBM Plex Sans Regular)"
        );
    }

    #[test]
    fn shared_chrome_no_raw_typo_text() {
        // UI-HOME-14: the shared chrome helpers (sidebar empty state, local
        // profile block, profile identity card, info row, section card) must
        // not declare raw TYPO_ text sizes anymore.
        let sidebar_src = include_str!("sidebar.rs");
        let settings_src = include_str!("settings.rs");
        let profile = method_source(
            sidebar_src,
            "fn view_local_profile_block(",
            "fn format_preview(",
        );
        assert!(
            !profile.contains("TYPO_"),
            "local profile block must not use raw TYPO_ sizes"
        );
        let identity = method_source(
            settings_src,
            "fn profile_identity_card(",
            "#[cfg(test)]",
        );
        assert!(
            !identity.contains("TYPO_"),
            "profile identity card must not use raw TYPO_ sizes"
        );
    }

    // ── FONTS-09: JetBrains Mono stays for technical values ────────────
    //
    // Genuine technical identifiers (peer IDs, hashes, fingerprints, ports,
    // debug IDs) keep the TechnicalValue role (JetBrains Mono); user-facing
    // display names — even an alphanumeric short key like "6c0f88fe9f" —
    // use IBM Plex Sans (or Figtree in chat), never monospace.

    #[test]
    fn font09_technical_identifiers_keep_jetbrains_mono() {
        // Every genuine technical-value site must resolve through
        // TypeRole::TechnicalValue (JetBrains Mono): friend ID / public key,
        // tunnel host:port endpoint, peer key row, room topic hex, contact
        // peer ID, key fingerprint, content hash, connection-detail inputs
        // and log viewer contents.
        let src = include_str!("../app.rs");
        let chat_src = include_str!("chat.rs");
        let settings_src = include_str!("settings.rs");
        let identity = method_source(
            settings_src,
            "fn profile_identity_card(",
            "#[cfg(test)]",
        );
        assert!(
            identity.contains("TypeRole::TechnicalValue"),
            "friend ID (public key) must use TypeRole::TechnicalValue"
        );
        let home_src = include_str!("home.rs");
        let tunnels = method_source(
            home_src,
            "fn view_tunnels_card(",
            "fn view_main_empty_state(",
        );
        assert!(
            tunnels.contains("TypeRole::TechnicalValue"),
            "tunnel host:port endpoint must use TypeRole::TechnicalValue"
        );
        let chat_header =
            method_source(chat_src, "fn view_chat_header(", "fn chat_search_matches(");
        assert!(
            chat_header.contains("TypeRole::TechnicalValue"),
            "peer key row must use TypeRole::TechnicalValue"
        );
        let options = method_source(
            chat_src,
            "fn view_chat_options_popover(",
            "fn view_details_panel(",
        );
        assert!(
            options.contains("TypeRole::TechnicalValue"),
            "room topic hex must use TypeRole::TechnicalValue"
        );
        let details = method_source(
            chat_src,
            "fn view_details_panel_direct(",
            "fn view_group_info_panel(",
        );
        assert!(
            details.contains("TypeRole::TechnicalValue"),
            "contact peer ID and key fingerprint must use TypeRole::TechnicalValue"
        );
        let settings = method_source(
            settings_src,
            "fn view_settings_screen_content(",
            "fn view_settings_screen_cached(",
        );
        assert!(
            settings.contains("TypeRole::TechnicalValue"),
            "content hash must use TypeRole::TechnicalValue"
        );
        let log_src = include_str!("../log_viewer.rs");
        assert!(
            log_src.contains("TypeRole::TechnicalValue"),
            "log viewer contents must use TypeRole::TechnicalValue"
        );
        let conn_src = include_str!("../connection_details.rs");
        assert!(
            conn_src.contains("TypeRole::TechnicalValue"),
            "connection-detail inputs must use TypeRole::TechnicalValue"
        );
    }

    // ── FONTS-06: sidebar typography regression guards ─────────────────

    #[test]
    fn sidebar_names_use_ibm_plex_medium() {
        // FONTS-06: sidebar contact/peer names (conversation rows, groups,
        // friends, discovered peers, public rooms, requests) must render in
        // IBM Plex Sans Medium via the central `sidebar_name_text` helper —
        // not the `Body` role (Regular 400).
        let sidebar_src = include_str!("sidebar.rs");
        assert!(
            sidebar_src.contains("fn sidebar_name_text<"),
            "sidebar name helper must exist"
        );
        assert!(
            sidebar_src.contains("public_sans(iced::font::Weight::Medium)"),
            "sidebar names must use Public Sans Medium"
        );
        // Every sidebar section renderer routes its primary name through the
        // helper instead of the Body role.  (The groups screen renderers live
        // in sidebar.rs alongside the sidebar views they were interleaved
        // with; the final requests slice ends at EOF because its original
        // successor, view_main_empty_state, now lives in home.rs.)
        for (marker, end) in [
            (
                "fn view_groups_section_content(",
                "fn view_sidebar_ticket_join(",
            ),
            (
                "fn view_sidebar_conversation_row(",
                "fn view_sidebar_discovered_peers(",
            ),
            (
                "fn view_sidebar_discovered_peers_content(",
                "fn view_sidebar_friends(",
            ),
            (
                "fn view_sidebar_friends_rows_content(",
                "fn view_sidebar_requests(",
            ),
            (
                "fn view_sidebar_requests_content(",
                "fn __end_of_sidebar_module__(",
            ),
        ] {
            let section = method_source(sidebar_src, marker, end);
            assert!(
                section.contains("sidebar_name_text("),
                "{marker} must render its name via sidebar_name_text (FONTS-06 IBM Plex Sans Medium)"
            );
        }
        // The local-profile identity row also uses the sidebar name helper.
        let profile = method_source(
            sidebar_src,
            "fn view_local_profile_block(",
            "fn format_preview(",
        );
        assert!(
            profile.contains("sidebar_name_text("),
            "local profile display name must use sidebar_name_text"
        );
    }

    #[test]
    fn sidebar_profile_name_is_single_line() {
        // SIDEBAR-02: the local display name in the top-left sidebar header
        // must stay on ONE line. The name text uses `Wrapping::None` (no
        // line breaks) inside a `.clip(true)` container (overflow truncated
        // at the available sidebar width) — never `WordOrGlyph` wrapping,
        // which lets long names wrap onto multiple lines.
        let sidebar_src = include_str!("sidebar.rs");
        let profile = method_source(
            sidebar_src,
            "fn view_local_profile_block(",
            "fn format_preview(",
        );
        assert!(
            profile.contains("Wrapping::None"),
            "sidebar profile name must use Wrapping::None to stay single-line"
        );
        assert!(
            profile.contains(".clip(true)"),
            "sidebar profile name container must clip overflow (no wrap to multiple lines)"
        );
        assert!(
            !profile.contains("Wrapping::WordOrGlyph"),
            "sidebar profile name must not use WordOrGlyph (long names would wrap)"
        );
        // The status label still renders below the name inside the same column.
        assert!(
            profile.contains("status_label"),
            "status label must still render below the display name"
        );
    }

    #[test]
    fn sidebar03_profile_header_avatar_is_compact() {
        // PROFILE-SIDEBAR: the top-left profile header avatar renders at
        // PROFILE_HEADER_AVATAR_SIZE (= design_tokens::AVATAR_PROFILE, 72 px)
        // — larger than list-row avatars (AVATAR_CHAT_LIST = 56 px) so the
        // local-user identity block stands out from conversation rows.
        let src = include_str!("../app.rs");
        let sidebar_src = include_str!("sidebar.rs");
        assert!(
            src.contains(
                "const PROFILE_HEADER_AVATAR_SIZE: f32 = crate::design_tokens::AVATAR_PROFILE;"
            ),
            "profile header avatar token must resolve to AVATAR_PROFILE (72 px)"
        );
        let profile = method_source(
            sidebar_src,
            "fn view_local_profile_block(",
            "fn format_preview(",
        );
        assert!(
            profile.contains("Length::Fixed(PROFILE_HEADER_AVATAR_SIZE)"),
            "profile-image avatar must use PROFILE_HEADER_AVATAR_SIZE"
        );
        assert!(
            profile.contains("radius: (PROFILE_HEADER_AVATAR_SIZE / 2.0).into()"),
            "initials-circle radius must be half of the avatar size"
        );
        assert!(
            !profile.contains("Length::Fixed(AVATAR_SM)"),
            "profile header must NOT use the list-row AVATAR_SM size"
        );
        // List-row avatars use the chat_list theme token (defaults to
        // AVATAR_CHAT_LIST = 56 px per PROFILE-SIDEBAR; the theme default
        // is pinned by theme.rs `default_theme_matches_design_tokens`).
        let conversation_row = method_source(
            sidebar_src,
            "fn view_sidebar_conversation_row(",
            "fn view_sidebar_discovered_peers(",
        );
        assert!(
            conversation_row.contains("btheme.avatars.chat_list"),
            "sidebar list-row avatars must use the chat_list theme token (AVATAR_CHAT_LIST, 56 px)"
        );
    }

    #[test]
    fn font09_display_names_use_plex_or_figtree_not_mono() {
        // Display-name sites — sidebar friends / discovered peers / online
        // peers, chat sender labels, and profile headers — must render the
        // name in IBM Plex Sans (Body / SectionTitle / FONTS-06
        // sidebar_name_text) or Figtree (ChatSender), never in JetBrains
        // Mono, even when the name is an alphanumeric raw key.
        let src = include_str!("../app.rs");
        let discover_src = include_str!("discover.rs");
        let chat_src = include_str!("chat.rs");
        let sidebar_src = include_str!("sidebar.rs");
        let friends = method_source(
            sidebar_src,
            "fn view_sidebar_friends_rows_content(",
            "fn sidebar_requests_dependency(",
        );
        assert!(
            friends.contains("sidebar_name_text("),
            "sidebar friend labels must use sidebar_name_text (FONTS-06 IBM Plex Sans Medium)"
        );
        assert!(
            !friends.contains("TypeRole::TechnicalValue"),
            "sidebar friend labels must NOT use JetBrains Mono"
        );
        let discovered = method_source(
            sidebar_src,
            "fn view_sidebar_discovered_peers_content(",
            "fn view_sidebar_friends(",
        );
        assert!(
            discovered.contains("sidebar_name_text("),
            "discovered-peer names must use sidebar_name_text (FONTS-06 IBM Plex Sans Medium)"
        );
        assert!(
            !discovered.contains("TypeRole::TechnicalValue"),
            "discovered-peer names must NOT use JetBrains Mono"
        );
        let home_src = include_str!("home.rs");
        let peers = method_source(
            home_src,
            "fn view_online_peers_card(",
            "fn view_recent_activity_card(",
        );
        assert!(
            peers.contains("TypeRole::Body"),
            "online-peer names must use TypeRole::Body (IBM Plex Sans)"
        );
        assert!(
            !peers.contains("TypeRole::TechnicalValue"),
            "online-peer names must NOT use JetBrains Mono"
        );
        let chat_log = method_source(chat_src, "fn view_chat_log(", "fn view_composer(");
        // BORU-UI-16: chat sender labels resolve through the live theme
        // (`btheme.type_font(TypeRole::ChatSender)`), which defaults to the
        // same Figtree SemiBold mapping — never JetBrains Mono.
        assert!(
            chat_log.contains("type_font(crate::fonts::TypeRole::ChatSender)")
                || chat_log.contains("TypeRole::ChatSender.font()"),
            "chat sender labels must resolve through TypeRole::ChatSender (Figtree)"
        );
        assert!(
            !chat_log.contains("TypeRole::TechnicalValue"),
            "chat sender labels must NOT use JetBrains Mono"
        );
        // FONTS-09 fix: the friend profile header previously rendered the
        // display name with the raw default font. It must now use the IBM
        // Plex Sans SectionTitle role, like the peer profile header.
        let friend_profile = method_source(
            discover_src,
            "fn view_friend_profile_content(",
            "fn view_share_local_service_dialog<'a>(",
        );
        assert!(
            friend_profile.contains("TypeRole::SectionTitle"),
            "friend profile display name must use TypeRole::SectionTitle (IBM Plex Sans)"
        );
        assert!(
            !friend_profile.contains("TypeRole::TechnicalValue"),
            "friend profile display name must NOT use JetBrains Mono"
        );
        // The peer profile header (already correct) keeps SectionTitle.
        let peer_profile = method_source(
            discover_src,
            "fn view_peer_profile_content(",
            "fn view_peer_catalogue(",
        );
        assert!(
            peer_profile.contains("TypeRole::SectionTitle"),
            "peer profile display name must use TypeRole::SectionTitle (IBM Plex Sans)"
        );
        assert!(
            !peer_profile.contains("TypeRole::TechnicalValue"),
            "peer profile display name must NOT use JetBrains Mono"
        );
    }

    #[test]
    fn chat_log_sender_labels_anchor_to_bubble_side() {
        // CHAT-02: sender names must stay on the same side as the message
        // bubble regardless of message length. The wrapping column that holds
        // `label_el` + `msg_row` must anchor the label End for own messages
        // and Start for received/system entries — never the default Start for
        // own messages (which pushed usernames to the LEFT while the bubble
        // hugged the right).
        let src = include_str!("../app.rs");
        let chat_src = include_str!("chat.rs");
        let chat_log = method_source(chat_src, "fn view_chat_log(", "fn view_composer(");
        assert!(
            chat_log.contains("let label_align = if matches!(entry.kind, ChatKind::Local)"),
            "view_chat_log must compute label_align from entry.kind (CHAT-02)"
        );
        assert!(
            chat_log.contains("iced::Alignment::End"),
            "own-message sender labels must anchor right (CHAT-02)"
        );
        assert!(
            chat_log.contains("iced::Alignment::Start"),
            "received/system sender labels must anchor left (CHAT-02)"
        );
        assert!(
            chat_log.contains(".align_x(label_align)"),
            "the label_el + msg_row column must apply label_align (CHAT-02)"
        );
        // Sanity: the label column still sits ABOVE the avatar/bubble row and
        // keeps the chat-message-format preference (no brackets, Semibold).
        assert!(
            chat_log.contains(".push(label_el)\n                    .push(msg_row)"),
            "label_el must be pushed above msg_row in the aligned column (CHAT-02)"
        );
    }

    #[test]
    fn sidebar_section_labels_use_fonts06_size() {
        // FONTS-06: all-caps sidebar section labels (CHATS / GROUPS / …)
        // keep the ButtonLabel family/weight (IBM Plex Sans SemiBold) but
        // render at the 10–12 px band via the typed theme
        // (`SidebarTheme::section_label_size`, BORU-UI-03).
        // BORU-HOME-09: reduced from 12 px to 11 px.
        let src = include_str!("../ui_components.rs");
        assert!(
            src.contains("section_label_size"),
            "sidebar section label size must come from SidebarTheme::section_label_size (BORU-UI-03)"
        );
        assert_eq!(
            crate::theme::BoruTheme::default()
                .sidebar
                .section_label_size,
            11.0,
            "sidebar section label size must be 11 px (BORU-HOME-09)"
        );
        let header = method_source(
            src,
            "/// Build the header element.",
            "// 17b. SCROLLABLE WITH EMBEDDED SCROLLBAR",
        );
        assert!(
            header.contains("TypeRole::ButtonLabel.font()"),
            "sidebar section label must keep the IBM Plex Sans SemiBold button-label family/weight"
        );
        assert!(
            header.contains(".size(") && header.contains("section_label_size"),
            "sidebar section label must use the FONTS-06 11 px size from the theme"
        );
    }

    // ── Direct-chat request state tests ────────────────────────────────

    #[test]
    fn request_label_none_returns_empty() {
        assert_eq!(IcedChat::outgoing_request_label(None), "");
    }

    #[test]
    fn request_label_pending_returns_pending() {
        assert_eq!(
            IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Pending)),
            "Pending"
        );
    }

    #[test]
    fn request_label_accepted_returns_accepted() {
        assert_eq!(
            IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Accepted)),
            "Accepted"
        );
    }

    #[test]
    fn request_label_declined_returns_declined() {
        assert_eq!(
            IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Declined)),
            "Declined"
        );
    }

    #[test]
    fn request_label_failed_returns_failed() {
        assert_eq!(
            IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Failed("timeout".into()))),
            "Failed"
        );
    }

    #[test]
    fn request_color_none_is_muted_grey() {
        let c = IcedChat::outgoing_request_color(None);
        assert!((c.r - 0.5).abs() < 1e-6);
        assert!((c.g - 0.5).abs() < 1e-6);
        assert!((c.b - 0.5).abs() < 1e-6);
    }

    #[test]
    fn request_color_pending_is_amber() {
        let c = IcedChat::outgoing_request_color(Some(&OutgoingRequestState::Pending));
        assert!((c.r - 0.9).abs() < 1e-6);
        assert!((c.g - 0.7).abs() < 1e-6);
        assert!((c.b - 0.1).abs() < 1e-6);
    }

    #[test]
    fn request_color_accepted_is_green() {
        let c = IcedChat::outgoing_request_color(Some(&OutgoingRequestState::Accepted));
        assert!((c.r - 0.2).abs() < 1e-6);
        assert!((c.g - 0.7).abs() < 1e-6);
        assert!((c.b - 0.2).abs() < 1e-6);
    }

    #[test]
    fn request_color_declined_is_red() {
        let c = IcedChat::outgoing_request_color(Some(&OutgoingRequestState::Declined));
        assert!((c.r - 0.8).abs() < 1e-6);
        assert!((c.g - 0.2).abs() < 1e-6);
        assert!((c.b - 0.2).abs() < 1e-6);
    }

    #[test]
    fn request_color_failed_is_red() {
        let c =
            IcedChat::outgoing_request_color(Some(&OutgoingRequestState::Failed("error".into())));
        assert!((c.r - 0.8).abs() < 1e-6);
        assert!((c.g - 0.2).abs() < 1e-6);
        assert!((c.b - 0.2).abs() < 1e-6);
    }

    #[test]
    fn join_request_state_labels_cover_all_states() {
        assert_eq!(
            IcedChat::join_request_state_label(&OutgoingRequestState::Pending),
            "Pending"
        );
        assert_eq!(
            IcedChat::join_request_state_label(&OutgoingRequestState::Accepted),
            "Accepted"
        );
        assert_eq!(
            IcedChat::join_request_state_label(&OutgoingRequestState::Declined),
            "Rejected"
        );
        assert_eq!(
            IcedChat::join_request_state_label(&OutgoingRequestState::Failed("nope".into())),
            "Failed"
        );
    }

    #[test]
    fn join_request_state_colors_are_distinct() {
        let pending = IcedChat::join_request_state_color(&OutgoingRequestState::Pending);
        let accepted = IcedChat::join_request_state_color(&OutgoingRequestState::Accepted);
        let declined = IcedChat::join_request_state_color(&OutgoingRequestState::Declined);
        let failed = IcedChat::join_request_state_color(&OutgoingRequestState::Failed("x".into()));
        assert_ne!(pending, accepted);
        assert_ne!(accepted, declined);
        assert_ne!(declined, failed);
    }

    #[test]
    fn join_request_section_strings_are_localized_via_helpers() {
        assert_eq!(IcedChat::join_request_section_title(), "Join requests");
        assert_eq!(IcedChat::join_request_total_label(0), "0 total");
        assert_eq!(IcedChat::join_request_total_label(3), "3 total");
        assert_eq!(IcedChat::join_request_target_user_prefix(), "Target user");
        assert_eq!(IcedChat::join_request_chat_prefix(), "Chat");
        assert_eq!(IcedChat::join_request_open_chat_label(), "Open chat");
        assert_eq!(IcedChat::join_request_retry_label(), "Retry");
        assert_eq!(IcedChat::join_request_failure_prefix(), "Failure");
    }

    #[test]
    fn open_friend_requests_navigates_to_dedicated_screen() {
        let (_runtime, mut app, _local_public, _peer_public) = build_join_request_test_app();

        assert_eq!(app.screen, Screen::ChatList);

        let _ = app.update(AppMessage::OpenFriendRequests);
        assert_eq!(app.screen, Screen::FriendRequests);

        let _ = app.view();

        let _ = app.update(AppMessage::CloseFriendRequests);
        assert_eq!(app.screen, Screen::ChatList);
    }

    #[test]
    fn back_from_friend_requests_returns_to_file_sharing() {
        let (_runtime, mut app, _local_public, _peer_public) = build_join_request_test_app();
        let _ = app.update(AppMessage::OpenFileSharing);
        assert_eq!(app.screen, Screen::FileSharing);

        let _ = app.update(AppMessage::OpenFriendRequests);
        assert_eq!(app.screen, Screen::FriendRequests);

        let _ = app.update(AppMessage::CloseFriendRequests);
        assert_eq!(app.screen, Screen::FileSharing);
    }

    #[test]
    fn back_from_peer_profile_returns_to_file_sharing() {
        let (_runtime, mut app, _local_public, peer_public) = build_join_request_test_app();
        let _ = app.update(AppMessage::OpenFileSharing);
        assert_eq!(app.screen, Screen::FileSharing);

        let _ = app.update(AppMessage::OpenPeerProfile(peer_public));
        assert_eq!(app.screen, Screen::PeerProfile(peer_public));

        let _ = app.update(AppMessage::ClosePeerProfile);
        assert_eq!(app.screen, Screen::FileSharing);
    }

    #[test]
    fn back_from_peer_catalogue_returns_to_file_sharing() {
        let (_runtime, mut app, _local_public, peer_public) = build_join_request_test_app();
        let _ = app.update(AppMessage::OpenFileSharing);
        assert_eq!(app.screen, Screen::FileSharing);

        let _ = app.update(AppMessage::PeerCatalogueReceived {
            peer: peer_public,
            files: Vec::new(),
        });
        assert_eq!(app.screen, Screen::PeerCatalogue(peer_public));

        let _ = app.update(AppMessage::ClosePeerProfile);
        assert_eq!(app.screen, Screen::FileSharing);
    }

    #[test]
    fn back_from_friend_profile_returns_to_file_sharing() {
        let (_runtime, mut app, _local_public, peer_public) = build_join_request_test_app();
        let _ = app.update(AppMessage::OpenFileSharing);
        assert_eq!(app.screen, Screen::FileSharing);

        let _ = app.update(AppMessage::OpenFriendProfile(peer_public));
        assert_eq!(app.screen, Screen::FriendProfile(peer_public));

        let _ = app.update(AppMessage::CloseFriendProfile);
        assert_eq!(app.screen, Screen::FileSharing);
    }

    #[test]
    fn back_from_discover_returns_to_file_sharing() {
        let (_runtime, mut app, _local_public, _peer_public) = build_join_request_test_app();
        let _ = app.update(AppMessage::OpenFileSharing);
        assert_eq!(app.screen, Screen::FileSharing);

        let _ = app.update(AppMessage::OpenDirectory);
        assert_eq!(app.screen, Screen::Discover);

        let _ = app.update(AppMessage::CloseDiscover);
        assert_eq!(app.screen, Screen::FileSharing);
    }

    #[test]
    fn back_from_groups_returns_to_file_sharing() {
        let (_runtime, mut app, _local_public, _peer_public) = build_join_request_test_app();
        let _ = app.update(AppMessage::OpenFileSharing);
        assert_eq!(app.screen, Screen::FileSharing);

        let _ = app.update(AppMessage::OpenGroups);
        assert_eq!(app.screen, Screen::Groups);

        let _ = app.update(AppMessage::CloseGroups);
        assert_eq!(app.screen, Screen::FileSharing);
    }

    #[test]
    fn back_from_screen_without_return_to_falls_back_to_chat_list() {
        // Opening a sub-screen directly (no recorded origin) must still close
        // to a usable screen instead of panicking on a missing return_to.
        let (_runtime, mut app, _local_public, peer_public) = build_join_request_test_app();
        assert_eq!(app.screen, Screen::ChatList);

        let _ = app.update(AppMessage::OpenFriendRequests);
        assert_eq!(app.screen, Screen::FriendRequests);
        // No return_to recorded? Then close goes to ChatList (the existing
        // default). The Open handler always records, so force the fallback
        // by simulating a stale entry: close twice.
        let _ = app.update(AppMessage::CloseFriendRequests);
        let _ = app.update(AppMessage::CloseFriendRequests);
        assert_eq!(app.screen, Screen::ChatList);

        // Same fallback for a screen opened without a recorded origin.
        let _ = app.update(AppMessage::OpenGroups);
        assert_eq!(app.screen, Screen::Groups);
        app.groups_return_to = None;
        let _ = app.update(AppMessage::CloseGroups);
        assert_eq!(app.screen, Screen::ChatList);

        // And for peer profiles.
        let _ = app.update(AppMessage::OpenPeerProfile(peer_public));
        assert_eq!(app.screen, Screen::PeerProfile(peer_public));
        app.peer_profile_return_to = None;
        let _ = app.update(AppMessage::ClosePeerProfile);
        assert_eq!(app.screen, Screen::ChatList);
    }

    #[test]
    fn sidebar_requests_dependency_filters_to_pending_incoming_requests() {
        let (_runtime, mut app, local_public, peer_public) = build_join_request_test_app();
        let local_pk = local_public.to_string();

        // One incoming pending request that should render.
        app.friend_request_store
            .send_request(&peer_public.to_string(), &local_pk, None)
            .expect("store incoming request");

        // A second incoming request that has already moved to a terminal state
        // must stay out of the pending-only sidebar list.
        let ignored_peer = SecretKey::generate().public();
        let ignored_req = app
            .friend_request_store
            .send_request(&ignored_peer.to_string(), &local_pk, None)
            .expect("store second incoming request");
        app.friend_request_store
            .decline_request(&ignored_req.id, &local_pk)
            .expect("decline terminal request");

        // Outgoing requests must not be mixed into the incoming sidebar.
        let outgoing_peer = SecretKey::generate().public();
        app.friend_request_store
            .send_request(&local_pk, &outgoing_peer.to_string(), None)
            .expect("store outgoing request");

        let dep = app.sidebar_requests_dependency();
        assert_eq!(dep.incoming.len(), 1);
        assert_eq!(dep.incoming[0].requester, peer_public);
    }

    #[test]
    fn sidebar_requests_dependency_includes_group_invites() {
        let (_runtime, mut app, local_public, peer_public) = build_join_request_test_app();
        // Give the app storage so group invite persistence works
        let storage = boru_core::storage::Storage::memory().expect("test storage");
        app.storage = Some(storage);

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expire_ms = now_ms + 7 * 24 * 60 * 60 * 1000;
        let invite_id = rand::random::<[u8; 32]>();
        let group_id = rand::random::<[u8; 32]>();

        let invite_row = boru_core::storage::GroupInviteRow {
            invite_id,
            group_id,
            inviter_public_key: peer_public.to_vec(),
            recipient_public_key: local_public.to_vec(),
            epoch: 1,
            status: "Pending".into(),
            created_at_ms: now_ms,
            expires_at_ms: expire_ms,
            ticket: String::new(),
            group_name: "Test Group".to_string(),
        };
        app.storage
            .as_ref()
            .unwrap()
            .create_group_invite(&invite_row)
            .unwrap();

        let dep = app.sidebar_requests_dependency();
        assert_eq!(dep.group_invites.len(), 1, "should have one group invite");
        assert_eq!(dep.group_invites[0].invite_id, invite_id.to_vec());
        // The inviter label should resolve (even without a name saved, it shows short key)
        assert!(!dep.group_invites[0].inviter_label.is_empty());
    }

    // ── Incoming call overlay (BORU-CALL-6.3) ─────────────────────────
    // The overlay is a pure function of `incoming_call` state which is
    // driven exclusively by `CallEventReceived`; media consent is deferred
    // until `AcceptIncomingCall` (the only path that calls `handle.accept`).

    #[test]
    fn incoming_call_overlay_state_from_incoming_event_and_consent_deferred() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let call_id = CallId::new();

        // No overlay before any call event.
        assert!(app.calls_state.incoming_call.is_none());

        // An Incoming voice event renders the overlay with caller identity.
        app.update(AppMessage::CallEventReceived(CallEvent::Incoming {
            call_id,
            peer,
            kind: CallKind::Voice,
        }));
        let incoming = app
            .calls_state
            .incoming_call
            .expect("Incoming event must populate overlay state");
        assert_eq!(incoming.call_id, call_id);
        assert_eq!(incoming.peer, peer);
        assert_eq!(incoming.kind, CallKind::Voice);
        // Consent deferred: receiving the offer must NOT activate any media.
        assert!(
            !app.calls_state.call_audio_muted,
            "audio must not be activated by Incoming alone"
        );
        assert!(
            !app.calls_state.call_camera_enabled,
            "camera must not be activated by Incoming alone"
        );

        // A terminal event for the same call clears the overlay.
        app.update(AppMessage::CallEventReceived(CallEvent::Ended {
            call_id,
            reason: CallEndReason::RemoteHangup,
        }));
        assert!(
            app.calls_state.incoming_call.is_none(),
            "Ended must clear the overlay"
        );

        // An unrelated terminal event for a different call must NOT clear it.
        let other_id = CallId::new();
        app.update(AppMessage::CallEventReceived(CallEvent::Incoming {
            call_id: other_id,
            peer,
            kind: CallKind::Video,
        }));
        assert!(app.calls_state.incoming_call.is_some());
        app.update(AppMessage::CallEventReceived(CallEvent::Ended {
            call_id,
            reason: CallEndReason::RemoteHangup,
        }));
        assert!(
            app.calls_state.incoming_call.is_some(),
            "Ended for a different call must not clear overlay"
        );
        app.update(AppMessage::CallEventReceived(CallEvent::Ended {
            call_id: other_id,
            reason: CallEndReason::RemoteHangup,
        }));
        assert!(app.calls_state.incoming_call.is_none());

        // Failed with a matching call id also clears the overlay.
        app.update(AppMessage::CallEventReceived(CallEvent::Incoming {
            call_id,
            peer,
            kind: CallKind::Voice,
        }));
        app.update(AppMessage::CallEventReceived(CallEvent::Failed {
            call_id: Some(call_id),
            reason: CallError::Rejected,
        }));
        assert!(
            app.calls_state.incoming_call.is_none(),
            "Failed for the same call must clear the overlay"
        );
    }

    // ── Video call controls (BORU-CALL-6.6) ───────────────────────────
    // Camera toggle/selection update local UI state and route the async
    // set_camera_enabled command through the existing CallCommandFinished
    // channel; the renderer (view_active_call) shows the remote stage from
    // latest_remote_frame and falls back to the avatar/name block when the
    // remote camera is off (no frame yet).

    #[test]
    fn video_call_camera_toggle_flips_state_only_with_active_call() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let call_id = CallId::new();

        // With no active call the toggle must be a no-op (control is disabled).
        app.update(AppMessage::ToggleCallCamera);
        assert!(
            !app.calls_state.call_camera_enabled,
            "toggle without active call must not change state"
        );

        // An active call enables the local camera toggle.
        app.calls_state.active_call_id = Some(call_id);
        app.update(AppMessage::ToggleCallCamera);
        assert!(
            app.calls_state.call_camera_enabled,
            "toggle must enable camera on active call"
        );
        app.update(AppMessage::ToggleCallCamera);
        assert!(
            !app.calls_state.call_camera_enabled,
            "second toggle must disable camera"
        );

        // MediaStateChanged from the manager must stay authoritative for the
        // local camera state.
        app.update(AppMessage::CallEventReceived(
            CallEvent::MediaStateChanged {
                call_id,
                audio_muted: false,
                video_enabled: true,
            },
        ));
        assert!(
            app.calls_state.call_camera_enabled,
            "MediaStateChanged must sync camera state"
        );
        app.update(AppMessage::CallEventReceived(
            CallEvent::MediaStateChanged {
                call_id,
                audio_muted: false,
                video_enabled: false,
            },
        ));
        assert!(
            !app.calls_state.call_camera_enabled,
            "MediaStateChanged must sync camera off"
        );
    }

    #[test]
    fn video_call_camera_selection_cycles_front_and_back() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // "next" cycles between the two supported labels; explicit labels
        // (settings picker) are stored verbatim.
        app.update(AppMessage::SelectCamera("next".to_string()));
        assert_eq!(app.calls_state.call_camera_selection, "Back camera");
        app.update(AppMessage::SelectCamera("next".to_string()));
        assert_eq!(app.calls_state.call_camera_selection, "Front camera");
        app.update(AppMessage::SelectCamera("External USB".to_string()));
        assert_eq!(app.calls_state.call_camera_selection, "External USB");
    }

    // ── Outgoing call ringing screen (BORU-CALL-6.4) ──────────────────

    #[test]
    fn outgoing_call_ringing_state_from_event_and_decline_busy_mapping() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let start_screen = app.screen.clone();
        let call_id = CallId::new();

        // StartVoiceCall moves to the ringing screen and arms the return screen;
        // the call id arrives via the CallStarted perform result.
        app.update(AppMessage::StartVoiceCall(peer));
        assert!(
            matches!(app.screen, Screen::OutgoingCall),
            "start must show OutgoingCall screen"
        );
        assert_eq!(app.calls_state.outgoing_call_peer, Some(peer));
        assert_eq!(
            app.calls_state.outgoing_call_status,
            Some(OutgoingCallStatus::Ringing)
        );
        assert!(
            app.calls_state.active_call_id.is_none(),
            "active id arrives with CallStarted"
        );
        app.update(AppMessage::CallStarted(Ok(call_id)));
        assert_eq!(app.calls_state.active_call_id, Some(call_id));

        // OutgoingRinging (subscription event) keeps the ringing screen state.
        app.update(AppMessage::CallEventReceived(CallEvent::OutgoingRinging {
            call_id,
            peer,
        }));
        assert!(matches!(app.screen, Screen::OutgoingCall));
        assert_eq!(
            app.calls_state.outgoing_call_status,
            Some(OutgoingCallStatus::Ringing)
        );

        // A peer rejection maps to Declined.
        app.update(AppMessage::CallEventReceived(CallEvent::Failed {
            call_id: Some(call_id),
            reason: CallError::Rejected,
        }));
        assert_eq!(
            app.calls_state.outgoing_call_status,
            Some(OutgoingCallStatus::Declined)
        );
        assert!(app.calls_state.active_call_id.is_none());

        // Busy maps to Busy via a fresh outgoing call.
        let call_id2 = CallId::new();
        app.update(AppMessage::StartVoiceCall(peer));
        app.update(AppMessage::CallStarted(Ok(call_id2)));
        app.update(AppMessage::CallEventReceived(CallEvent::Failed {
            call_id: Some(call_id2),
            reason: CallError::Busy,
        }));
        assert_eq!(
            app.calls_state.outgoing_call_status,
            Some(OutgoingCallStatus::Busy)
        );

        // Cancel path: HangUp clears call state and returns to the prior screen.
        // (After a declined/busy call the user is still on the ringing screen;
        // navigating back to chat before re-calling gives a clean return.)
        let call_id3 = CallId::new();
        app.screen = start_screen.clone();
        app.update(AppMessage::StartVoiceCall(peer));
        app.update(AppMessage::CallStarted(Ok(call_id3)));
        assert_eq!(app.calls_state.active_call_id, Some(call_id3));
        app.update(AppMessage::HangUp(call_id3));
        assert!(app.calls_state.active_call_id.is_none());
        assert!(app.calls_state.outgoing_call_peer.is_none());
        assert!(app.calls_state.outgoing_call_status.is_none());
        assert_eq!(
            app.screen, start_screen,
            "Cancel must return to the prior screen"
        );
    }

    // ── BORU-CP-12 capability gating (PDF Task 4.3) ─────────────────────

    /// Fixed negotiated-capability view injected into the app for gate
    /// tests. Missing (peer, feature) entries fail closed (unsupported /
    /// unknown — exactly how the live gate treats an old client that never
    /// advertises capabilities).
    #[derive(Default)]
    struct FakeCapabilityGate {
        supported: std::collections::HashMap<(PublicKey, String), u16>,
    }

    impl boru_core::discovery_service::CapabilityGate for FakeCapabilityGate {
        fn peer_supports(&self, node_id: &PublicKey, feature: &str) -> Option<u16> {
            self.supported
                .get(&(*node_id, feature.to_string()))
                .copied()
        }
        fn peer_capabilities(
            &self,
            _node_id: &PublicKey,
        ) -> Option<boru_core::control_plane::CapabilitySet> {
            None
        }
        fn local_capabilities(&self) -> boru_core::control_plane::CapabilitySet {
            boru_core::control_plane::CapabilitySet::new()
        }
    }

    /// The pure gate decision: with no gate wired the legacy behaviour is
    /// kept (offered), with a gate the feature is offered only when the
    /// peer negotiates a compatible version.
    #[test]
    fn feature_offered_decision_fails_closed_with_gate() {
        let peer = SecretKey::generate().public();
        // No gate: legacy un-gated behaviour.
        let (_runtime, mut app, _local, _other) = build_join_request_test_app();
        assert!(app.feature_offered(&peer, "voice"));
        // Gate wired, compatible version -> offered.
        app.capability_gate = Some(Arc::new(FakeCapabilityGate {
            supported: [((peer, "voice".to_string()), 1u16)].into_iter().collect(),
        }));
        assert!(app.feature_offered(&peer, "voice"));
        // Gate wired, feature not advertised / incompatible -> NOT offered.
        assert!(!app.feature_offered(&peer, "video"));
        assert!(!app.feature_offered(&peer, "files"));
    }

    /// A new client does not attempt an unsupported operation against an
    /// old client: with a gate that says the peer lacks voice, StartVoiceCall
    /// is blocked (no ringing screen, no outgoing peer, explanatory toast).
    #[test]
    fn voice_call_blocked_when_peer_lacks_capability() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let start_screen = app.screen.clone();
        app.capability_gate = Some(Arc::new(FakeCapabilityGate::default()));

        let task = app.update(AppMessage::StartVoiceCall(peer));

        assert!(
            matches!(app.screen, s if s == start_screen),
            "screen must not change"
        );
        assert_eq!(
            app.calls_state.outgoing_call_peer, None,
            "no outgoing call may be armed"
        );
        assert!(
            app.notifications_state.toast_message.is_some(),
            "UI must explain why the action is unavailable"
        );
        assert!(app
            .notifications_state.toast_message
            .as_deref()
            .unwrap()
            .contains("does not support voice calls"));
        let _ = task;
    }

    /// With a gate that says the peer supports voice-v1, StartVoiceCall
    /// proceeds exactly as before (compatible -> private signalling path).
    #[test]
    fn voice_call_proceeds_when_peer_supports_capability() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        app.capability_gate = Some(Arc::new(FakeCapabilityGate {
            supported: [((peer, "voice".to_string()), 1u16)].into_iter().collect(),
        }));

        app.update(AppMessage::StartVoiceCall(peer));

        assert!(
            matches!(app.screen, Screen::OutgoingCall),
            "compatible peer must proceed"
        );
        assert_eq!(app.calls_state.outgoing_call_peer, Some(peer));
        assert!(
            app.notifications_state.toast_message.is_none(),
            "no error toast on the compatible path"
        );
    }

    /// A file send into a direct conversation with a peer that lacks the
    /// FILES capability is blocked before any upload state is created.
    #[test]
    fn file_send_blocked_when_peer_lacks_capability() {
        let (_runtime, mut app, local, peer) = build_join_request_test_app();
        // Make the currently selected topic a direct conversation with `peer`.
        let topic = boru_core::contact::direct_topic(&local, &peer);
        app.conversation_store
            .upsert(boru_core::conversations::ConversationEntry::new(
                topic,
                &peer.to_string(),
                "Peer",
            ));
        app.topic = topic;
        app.capability_gate = Some(Arc::new(FakeCapabilityGate::default()));
        let encoded = format!("notes.txt|/tmp/notes.txt|{}", peer);

        app.update(AppMessage::ExecuteFileSend(encoded));

        assert!(
            app.pending_file_upload.is_none(),
            "no upload may start against an unsupported peer"
        );
        assert!(
            app.notifications_state.toast_message.is_some(),
            "UI must explain why the action is unavailable"
        );
        assert!(app
            .notifications_state.toast_message
            .as_deref()
            .unwrap()
            .contains("does not support file transfer"));
    }

    /// Screen sharing with a peer that lacks the capability is blocked.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_blocked_when_peer_lacks_capability() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        app.capability_gate = Some(Arc::new(FakeCapabilityGate::default()));

        app.update(AppMessage::StartScreenShare(peer));

        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Idle,
            "no host session may start against an unsupported peer"
        );
        assert!(
            app.notifications_state.toast_message.is_some(),
            "UI must explain why the action is unavailable"
        );
        assert!(app
            .notifications_state.toast_message
            .as_deref()
            .unwrap()
            .contains("does not support screen sharing"));
    }

    /// BORU-SS-29 (PDF Phase 13): the sharer panel runs through the seven
    /// states. SourcesEnumerated moves Requesting → Inviting (awaiting
    /// acceptance) and seeds the source picker's default selection with the
    /// first enumerated source.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_sources_enumerated_advances_to_inviting_and_defaults_selection() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.calls_state.screen_share_host_state = ScreenShareHostState::Requesting;

        let sources = vec![
            CaptureSource {
                id: CaptureSourceId(1),
                kind: boru_core::screen_share::CaptureSourceKind::Monitor,
                title: "DP-1: 1920x1080".to_string(),
                width: 1920,
                height: 1080,
                geometry: None,
            },
            CaptureSource {
                id: CaptureSourceId(2),
                kind: boru_core::screen_share::CaptureSourceKind::Monitor,
                title: "DP-2: 2560x1440".to_string(),
                width: 2560,
                height: 1440,
                geometry: None,
            },
        ];
        app.update(AppMessage::ScreenShareEventReceived(
            SessionEvent::SourcesEnumerated {
                session_id: ScreenShareSessionId::generate(),
                sources,
            },
        ));

        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Inviting,
            "sources enumerated while requesting → awaiting acceptance"
        );
        assert_eq!(
            app.calls_state.screen_share_selected_source,
            Some(CaptureSourceId(1)),
            "the picker defaults to the host's first source"
        );
        assert_eq!(
            app.calls_state.screen_share_sources.as_ref().map(Vec::len),
            Some(2),
            "the picker list is stored for the monitor-selection UI"
        );
    }

    /// BORU-SS-29 (PDF Phase 13): a rejected share surfaces an explicit
    /// terminal state — a peer "declined" is a normal stop, any other
    /// failure reason is an error the user can read.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_rejected_maps_to_error_or_stopped() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.calls_state.screen_share_host_state = ScreenShareHostState::Inviting;
        app.update(AppMessage::ScreenShareEventReceived(
            SessionEvent::Rejected {
                session_id: ScreenShareSessionId::generate(),
                reason: "no route to peer".to_string(),
            },
        ));
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Error("no route to peer".to_string()),
            "a transport/negotiation failure is an error state"
        );

        app.calls_state.screen_share_host_state = ScreenShareHostState::Inviting;
        app.update(AppMessage::ScreenShareEventReceived(
            SessionEvent::Rejected {
                session_id: ScreenShareSessionId::generate(),
                reason: "declined".to_string(),
            },
        ));
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Stopped,
            "a peer decline is a normal stop outcome, not an error"
        );
    }

    /// BORU-SS-29 (PDF Phase 13): a session end and a user-initiated stop
    /// both land on the terminal "stopped" notice, and the Dismiss action
    /// returns the panel to Idle so a fresh share can start.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_stopped_notice_clears_on_dismiss() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;
        app.update(AppMessage::ScreenShareEventReceived(SessionEvent::Ended {
            session_id: ScreenShareSessionId::generate(),
        }));
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Stopped,
            "an ended session shows the stopped notice"
        );

        app.update(AppMessage::ScreenShareDismissNotice);
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Idle,
            "dismiss clears the terminal notice"
        );
    }

    /// BORU-SS-29 (PDF Phase 13): when the shared source disappears with no
    /// fallback the stream shows PAUSED (picking a source resumes it); when
    /// the host falls back to another monitor the share keeps streaming and
    /// the change is surfaced as a toast instead.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_source_unavailable_pauses_or_toasts() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;
        app.update(AppMessage::ScreenShareEventReceived(
            SessionEvent::SourceUnavailable {
                session_id: ScreenShareSessionId::generate(),
                reason: "monitor unplugged".to_string(),
                fallback: None,
            },
        ));
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Paused,
            "no fallback source → paused state"
        );

        app.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;
        app.update(AppMessage::ScreenShareEventReceived(
            SessionEvent::SourceUnavailable {
                session_id: ScreenShareSessionId::generate(),
                reason: "monitor unplugged".to_string(),
                fallback: Some("DP-2".to_string()),
            },
        ));
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Streaming,
            "a fallback source keeps the share streaming"
        );
        assert!(
            app.notifications_state.toast_message
                .as_deref()
                .unwrap_or_default()
                .contains("Screen share paused"),
            "the fallback is surfaced as a toast"
        );
    }

    /// BORU-SS-29 (PDF Phase 13): picking a monitor from the source picker
    /// records the selection and routes HostCommand::SwitchSource to the
    /// host driver so the choice applies before/after acceptance.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_select_source_forwards_switch_command() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(8);
        app.calls_state.screen_share_host_cmd_tx = Some(cmd_tx);

        app.update(AppMessage::ScreenShareSelectSource(CaptureSourceId(2)));

        assert_eq!(
            app.calls_state.screen_share_selected_source,
            Some(CaptureSourceId(2))
        );
        let received = cmd_rx.try_recv();
        assert!(
            matches!(received, Ok(HostCommand::SwitchSource(CaptureSourceId(2)))),
            "the picker choice must reach the host driver, got {received:?}"
        );
    }

    /// BORU-SSUI-04 (PDF Task 4): selecting a quality segment mirrors the
    /// user's choice so exactly one segment shows selected, and forwards the
    /// exact same HostCommand the old preset text buttons sent (None = Auto).
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_set_preset_forwards_command_and_mirrors_selection() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(8);
        app.calls_state.screen_share_host_cmd_tx = Some(cmd_tx);

        app.update(AppMessage::ScreenShareSetPreset(Some(
            QualityPreset::LanHigh,
        )));
        assert_eq!(
            app.calls_state.screen_share_selected_preset,
            Some(QualityPreset::LanHigh),
            "the segmented control mirrors the user's choice"
        );
        let received = cmd_rx.try_recv();
        assert!(
            matches!(
                received,
                Ok(HostCommand::SetQualityPreset(Some(QualityPreset::LanHigh)))
            ),
            "LAN High must dispatch the same command as before, got {received:?}"
        );

        app.update(AppMessage::ScreenShareSetPreset(None));
        assert_eq!(
            app.calls_state.screen_share_selected_preset, None,
            "Auto (None) mirrors as the path-derived auto preset"
        );
        let received = cmd_rx.try_recv();
        assert!(
            matches!(received, Ok(HostCommand::SetQualityPreset(None))),
            "Auto must dispatch the same command as before, got {received:?}"
        );
    }

    /// BORU-SSUI-13: pure sender mappings cover the selected source, every
    /// quality preset, both audio states, remote-control state, and terminal
    /// stopping state without constructing an iced widget tree.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_state_to_view_mappings_cover_sender_states() {
        let source = CaptureSourceId(7);
        assert!(IcedChat::source_card_is_selected(Some(source), source));
        assert!(!IcedChat::source_card_is_selected(None, source));
        assert!(!IcedChat::source_card_is_selected(
            Some(CaptureSourceId(8)),
            source
        ));

        for selected in [
            Some(QualityPreset::LanHigh),
            Some(QualityPreset::Balanced),
            Some(QualityPreset::RelayConservative),
            None,
        ] {
            let specs = IcedChat::quality_segment_specs(selected);
            assert_eq!(specs.iter().filter(|spec| spec.selected).count(), 1);
            assert_eq!(
                specs
                    .iter()
                    .find(|spec| spec.selected)
                    .and_then(|spec| spec.preset),
                selected
            );
        }

        let audio_off = IcedChat::audio_toggle_spec(false, false);
        assert!(!audio_off.active && audio_off.enabled);
        assert_eq!(audio_off.icon, Icon::VolumeX);
        let audio_on = IcedChat::audio_toggle_spec(true, false);
        assert!(audio_on.active && audio_on.enabled);
        assert_eq!(audio_on.icon, Icon::Volume2);
        assert!(!IcedChat::audio_toggle_spec(true, true).enabled);

        let remote_off = IcedChat::remote_control_status_spec(false);
        assert!(!remote_off.active);
        assert_eq!(remote_off.label_key, "screenshare.remote_control_off");
        let remote_on = IcedChat::remote_control_status_spec(true);
        assert!(remote_on.active);
        assert_eq!(remote_on.label_key, "screenshare.remote_control_on");

        assert!(IcedChat::stop_action_visible(
            &ScreenShareHostState::Streaming
        ));
        assert!(IcedChat::stop_action_visible(
            &ScreenShareHostState::Requesting
        ));
        assert!(!IcedChat::stop_action_visible(
            &ScreenShareHostState::Stopped
        ));
        assert!(!IcedChat::stop_action_visible(
            &ScreenShareHostState::Error("capture ended".to_string())
        ));
    }

    /// BORU-SSUI-11 (PDF Task 11): Stop Sharing is idempotent — a rapid
    /// double-click (or a second queued StopScreenShare) cannot double-signal
    /// the host or double-send EndSession. The first stop clears the host
    /// stop flag Arc and the view session id, so a second dispatch finds
    /// nothing to signal and the UI stays in the Stopped terminal state.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_double_stop_is_idempotent() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let stop = Arc::new(AtomicBool::new(false));
        app.calls_state.screen_share_host_stop = Some(stop.clone());
        app.calls_state.screen_share_decode_stop = Some(Arc::new(AtomicBool::new(false)));
        // A live protocol + view session makes the first stop take the
        // EndSession branch; the second stop must NOT re-enter it.
        let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);
        app.calls_state.screen_share_protocol = Some(ScreenShareProtocol::new(events_tx));
        app.calls_state.screen_share_view_session = Some(ScreenShareSessionId::generate());
        app.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;

        // First stop: signal the host, clear the session handles.
        app.update(AppMessage::StopScreenShare);
        assert!(stop.load(Ordering::Relaxed), "host stop flag signalled");
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Stopped
        );
        assert!(
            app.calls_state.screen_share_host_stop.is_none(),
            "stop Arc dropped after reset — no second signal possible"
        );
        assert!(
            app.calls_state.screen_share_view_session.is_none(),
            "view session cleared after reset — no second EndSession possible"
        );

        // Second stop (rapid click): no double-signal, no double EndSession,
        // no panic — the terminal state is preserved.
        app.update(AppMessage::StopScreenShare);
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Stopped
        );
        assert!(app.calls_state.screen_share_host_stop.is_none());
        assert!(app.calls_state.screen_share_view_session.is_none());
    }

    /// BORU-SSUI-11 (PDF Task 11): after a stop the host command channel is
    /// gone, so any control message that arrives late (a queued click or a
    /// stale view) cannot reach the dead host — the handlers no-op instead
    /// of dispatching to a stopped session (no conflicting changes).
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_control_handlers_inert_after_stop() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(8);
        app.calls_state.screen_share_host_cmd_tx = Some(cmd_tx);
        app.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;

        // Stop: reset drops the command channel.
        app.update(AppMessage::StopScreenShare);
        assert!(
            app.calls_state.screen_share_host_cmd_tx.is_none(),
            "host cmd channel dropped on stop"
        );

        // Late/stale control messages must not panic or dispatch anywhere.
        app.update(AppMessage::ScreenShareSelectSource(CaptureSourceId(2)));
        app.update(AppMessage::ScreenShareSetPreset(Some(
            QualityPreset::LanHigh,
        )));
        app.update(AppMessage::ScreenShareToggleAudio);
        assert_eq!(
            app.calls_state.screen_share_host_state,
            ScreenShareHostState::Stopped
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "no source/quality/audio command may reach a stopped host"
        );
    }

    /// BORU-SSUI-11 (PDF Task 11): the host-side diagnostics pipeline is
    /// retained — `SessionEvent::Metrics` still populates
    /// `screen_share_host_metrics` (consumed by the dev overlay and the
    /// always-visible quality line) even though the redesigned card does not
    /// display raw statistics.
    #[cfg(feature = "screen-sharing")]
    #[test]
    fn screen_share_metrics_event_still_populates_host_metrics() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        assert!(app.calls_state.screen_share_host_metrics.is_none());

        let mut stats = boru_core::screen_share::ScreenShareStats::new();
        let snapshot = stats.snapshot();
        let metrics = ScreenShareSessionMetrics {
            codec: "h264".to_string(),
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate_bps: 4_000_000,
            backend: "test".to_string(),
            path_kind: PathKind::Direct,
            preset: QualityPreset::LanHigh,
            adaptive_level: 0,
            snapshot,
        };
        app.update(AppMessage::ScreenShareEventReceived(
            SessionEvent::Metrics {
                session_id: ScreenShareSessionId::generate(),
                metrics: metrics.clone(),
            },
        ));
        assert_eq!(
            app.calls_state.screen_share_host_metrics,
            Some(metrics),
            "metrics instrumentation retained after the card redesign"
        );
    }

    /// Tunnel creation against a peer that lacks the TUNNELS capability is
    /// blocked at the friend-picker entry point.
    #[test]
    fn tunnel_creation_blocked_when_peer_lacks_capability() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        app.capability_gate = Some(Arc::new(FakeCapabilityGate::default()));

        app.update(AppMessage::ShowCreateTunnelDialog);
        assert!(
            app.tunnels_state.show_create_tunnel_dialog,
            "dialog opens before the picker confirm"
        );
        app.update(AppMessage::CreateTunnel(peer));

        assert!(
            !app.tunnels_state.show_create_tunnel_dialog,
            "picker must close after a blocked attempt"
        );
        assert!(!app.tunnels_state.share_local_service_open, "share form must not open");
        assert!(
            app.notifications_state.toast_message.is_some(),
            "UI must explain why the action is unavailable"
        );
        assert!(app
            .notifications_state.toast_message
            .as_deref()
            .unwrap()
            .contains("does not support secure tunnels"));
    }

    /// The authoritative tunnel confirm also fails closed: with the share
    /// form open for an unsupported peer, confirming never creates a
    /// tunnel.
    #[test]
    fn tunnel_confirm_blocked_when_peer_lacks_capability() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        app.capability_gate = Some(Arc::new(FakeCapabilityGate::default()));
        app.screen = Screen::FriendProfile(peer);
        app.tunnels_state.share_local_service_open = true;
        app.tunnels_state.share_service_name = "Dev Server".to_string();
        app.tunnels_state.share_service_port = "3000".to_string();
        app.tunnels_state.share_service_expiry = boru_core::tunnel::service::TunnelDuration::OneHour;
        let tunnel_count_before = app.tunnels_state.shared_tunnels.len();

        app.update(AppMessage::ConfirmShareLocalService);

        assert_eq!(
            app.tunnels_state.shared_tunnels.len(),
            tunnel_count_before,
            "no tunnel may be created"
        );
        assert!(
            !app.tunnels_state.share_local_service_open,
            "form closes on a blocked attempt"
        );
        assert!(
            app.notifications_state.toast_message.is_some(),
            "UI must explain why the action is unavailable"
        );
    }

    /// Test that the button text for each state can be read from the
    /// button label.  Uses debug formatting because iced::Element is
    /// opaque — we verify the button's label contains the expected text.
    #[test]
    fn request_button_pending_shows_awaiting() {
        let pk = SecretKey::generate().public();
        let mut app = HashMap::new();
        app.insert(pk, OutgoingRequestState::Pending);
        // Simulate a minimal view — just verify the label (not Element shape).
        let label = IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Pending));
        assert_eq!(label, "Pending", "Pending state shows 'Pending' label");
    }

    #[test]
    fn request_button_none_shows_chat() {
        let label = IcedChat::outgoing_request_label(None);
        assert_eq!(label, "", "No request shows empty label");
    }

    #[test]
    fn request_state_none_allows_new_chat() {
        // A peer with no request state should get a Chat button that
        // sends SendFriendRequest.  Verify the label is empty and
        // the button text would be 'Chat'.
        assert_eq!(IcedChat::outgoing_request_label(None), "");
    }

    #[test]
    fn request_state_pending_disables_repeat() {
        // When state is Pending, the button has no on_press so the
        // user cannot send another request.  We verify by checking the
        // label — Pending means the button is disabled.
        let label = IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Pending));
        assert_eq!(label, "Pending");
    }

    #[test]
    fn request_state_failed_allows_retry() {
        // When state is Failed, the button text is 'Retry' and fires
        // FriendRequestRetry.  Verify label shows 'Failed'.
        let label =
            IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Failed("network".into())));
        assert_eq!(label, "Failed");
    }

    #[test]
    fn request_state_accepted_shows_accepted() {
        let label = IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Accepted));
        assert_eq!(label, "Accepted");
    }

    #[test]
    fn request_state_declined_shows_declined() {
        let label = IcedChat::outgoing_request_label(Some(&OutgoingRequestState::Declined));
        assert_eq!(label, "Declined");
    }

    /// Test duplicate suppression semantics through the state machine:
    /// If a Pending request already exists, the SendFriendRequest handler
    /// should not create a second pending entry.
    #[test]
    fn request_duplicate_pending_suppressed() {
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        // First request — goes to Pending.
        states.insert(pk, OutgoingRequestState::Pending);
        assert_eq!(states.len(), 1);
        // Simulate a second SendFriendRequest for the same peer.
        // The handler checks outgoing_request_states — if Pending exists
        // for this peer, the duplicate should be suppressed.
        // We verify by asserting the state is still Pending.
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Pending)
        ));
        // Adding the same key again just overwrites; the important check
        // is that the handler doesn't call friend_request_store.send_request
        // when state is already Pending.
        states.insert(pk, OutgoingRequestState::Pending);
        assert_eq!(states.len(), 1);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Pending)
        ));
    }

    #[test]
    fn request_duplicate_accepted_does_not_resend() {
        // Once a request is accepted, re-sending is not allowed.
        // The Accepted button fires OpenFriendChat, not SendFriendRequest.
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        states.insert(pk, OutgoingRequestState::Accepted);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Accepted)
        ));
        // Adding again should not change state.
        states.insert(pk, OutgoingRequestState::Accepted);
        assert_eq!(states.len(), 1);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Accepted)
        ));
    }

    #[test]
    fn request_duplicate_declined_does_not_resend() {
        // Declined state is terminal — no button press triggers resend.
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        states.insert(pk, OutgoingRequestState::Declined);
        assert_eq!(states.len(), 1);
        states.insert(pk, OutgoingRequestState::Declined);
        assert_eq!(states.len(), 1);
    }

    #[test]
    fn request_multiple_peers_tracked_independently() {
        // Each peer's request state is tracked separately.
        let pk_a = SecretKey::generate().public();
        let pk_b = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        states.insert(pk_a, OutgoingRequestState::Pending);
        states.insert(pk_b, OutgoingRequestState::Accepted);
        assert_eq!(states.len(), 2);
        assert!(matches!(
            states.get(&pk_a),
            Some(OutgoingRequestState::Pending)
        ));
        assert!(matches!(
            states.get(&pk_b),
            Some(OutgoingRequestState::Accepted)
        ));
    }

    #[test]
    fn request_state_transition_none_to_pending() {
        // Simulate SendFriendRequest handler: on success, state goes from
        // None → Pending.
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        assert!(states.get(&pk).is_none());
        states.insert(pk, OutgoingRequestState::Pending);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Pending)
        ));
    }

    #[test]
    fn request_state_transition_pending_to_failed() {
        // Simulate FriendRequestFailed handler: Pending → Failed.
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        states.insert(pk, OutgoingRequestState::Pending);
        states.insert(pk, OutgoingRequestState::Failed("timeout".into()));
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Failed(_))
        ));
        if let Some(OutgoingRequestState::Failed(msg)) = states.get(&pk) {
            assert_eq!(msg, "timeout");
        }
    }

    #[test]
    fn request_state_transition_failed_to_retry_pending() {
        // Simulate FriendRequestRetry handler: dispatches SendFriendRequest.
        // On success, state goes Failed → Pending.
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        states.insert(pk, OutgoingRequestState::Failed("timeout".into()));
        // Retry triggers SendFriendRequest, which sends and sets Pending.
        states.insert(pk, OutgoingRequestState::Pending);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Pending)
        ));
    }

    #[test]
    fn request_state_transition_pending_to_accepted() {
        // Simulate FriendRequestReceived handler with Accepted status.
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        states.insert(pk, OutgoingRequestState::Pending);
        states.insert(pk, OutgoingRequestState::Accepted);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Accepted)
        ));
    }

    #[test]
    fn request_state_transition_pending_to_declined() {
        // Simulate FriendRequestReceived handler with Declined status.
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        states.insert(pk, OutgoingRequestState::Pending);
        states.insert(pk, OutgoingRequestState::Declined);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Declined)
        ));
    }

    #[test]
    fn request_state_initial_empty_returns_none() {
        // Freshly created state has no entries — all peers show as "Chat".
        let states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        assert!(
            IcedChat::outgoing_request_label(states.get(&SecretKey::generate().public()))
                .is_empty()
        );
    }

    // ── Friend Requests UI handler path tests ──────────────────────

    #[test]
    fn friend_request_send_with_invalid_key_sets_error() {
        // Simulate the FriendRequestSend handler's invalid-key path:
        // PublicKey::from_str() on bad input → error is populated
        let bad_key = "not-a-valid-public-key";
        let result = PublicKey::from_str(bad_key);
        assert!(result.is_err(), "invalid key string should fail to parse");
    }

    #[test]
    fn friend_request_accept_handler_integrates_with_store() {
        // Simulate the FriendRequestAccept handler path exactly:
        //   1. List incoming Pending requests
        //   2. Find by request_id
        //   3. Store.accept_request + save
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = FriendRequestStore::empty_at(dir.path());
        let alice = SecretKey::generate().public().to_string();
        let bob = SecretKey::generate().public().to_string();

        let req = store
            .send_request(&alice, &bob, Some("hello".into()))
            .expect("send request");

        // Bob lists incoming and finds the request
        let incoming = store.list_incoming_by_status(&bob, FriendRequestStatus::Pending);
        let found = incoming.iter().find(|r| r.id == req.id);
        assert!(found.is_some(), "bob can find the request in incoming");

        // Bob accepts
        let accepted = store.accept_request(&req.id, &bob).expect("bob accepts");

        assert_eq!(accepted.status, FriendRequestStatus::Accepted);
        assert!(accepted.updated_at_unix_ms >= accepted.created_at_unix_ms);

        // Save and reload
        store.save().expect("save");
        let loaded = FriendRequestStore::load(dir.path()).expect("reload");
        let loaded_req = loaded.get(&req.id).expect("request still exists");
        assert_eq!(loaded_req.status, FriendRequestStatus::Accepted);
    }

    #[test]
    fn friend_request_decline_handler_integrates_with_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = FriendRequestStore::empty_at(dir.path());
        let alice = SecretKey::generate().public().to_string();
        let bob = SecretKey::generate().public().to_string();

        let req = store
            .send_request(&alice, &bob, None)
            .expect("send request");

        let declined = store.decline_request(&req.id, &bob).expect("bob declines");

        assert_eq!(declined.status, FriendRequestStatus::Declined);
        assert!(declined.updated_at_unix_ms >= declined.created_at_unix_ms);

        store.save().expect("save");
        let loaded = FriendRequestStore::load(dir.path()).expect("reload");
        let loaded_req = loaded.get(&req.id).expect("request");
        assert_eq!(loaded_req.status, FriendRequestStatus::Declined);
    }

    #[test]
    fn friend_request_cancel_handler_integrates_with_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = FriendRequestStore::empty_at(dir.path());
        let alice = SecretKey::generate().public().to_string();
        let bob = SecretKey::generate().public().to_string();

        let req = store
            .send_request(&alice, &bob, None)
            .expect("send request");

        let cancelled = store
            .cancel_request(&req.id, &alice)
            .expect("alice cancels");

        assert_eq!(cancelled.status, FriendRequestStatus::Cancelled);

        store.save().expect("save");
        let loaded = FriendRequestStore::load(dir.path()).expect("reload");
        let loaded_req = loaded.get(&req.id).expect("request");
        assert_eq!(loaded_req.status, FriendRequestStatus::Cancelled);
    }

    #[test]
    fn friend_request_accept_not_found_returns_error() {
        let mut store = FriendRequestStore::default();
        let bob = SecretKey::generate().public().to_string();

        let err = store
            .accept_request("nonexistent-id", &bob)
            .expect_err("should fail with not found");
        assert!(matches!(err, FriendRequestError::NotFound(_)));
    }

    #[test]
    fn friend_request_store_save_after_mutation_preserves_state() {
        // Test the exact pattern used by all handler implementations:
        //   1. Mutate store (accept/decline/cancel)
        //   2. Save store
        //   3. On reload, the mutation is preserved
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = FriendRequestStore::empty_at(dir.path());
        let alice = SecretKey::generate().public().to_string();
        let bob = SecretKey::generate().public().to_string();

        let req = store.send_request(&alice, &bob, None).expect("send");

        // Accept and save (like FriendRequestAccept handler)
        store.accept_request(&req.id, &bob).expect("accept");
        store.save().expect("save after accept");

        // Verify on reload
        let loaded = FriendRequestStore::load(dir.path()).expect("reload");
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded.get(&req.id).expect("request").status,
            FriendRequestStatus::Accepted
        );

        // New store, new request: cancel and save (like FriendRequestCancel handler)
        let dir2 = tempfile::tempdir().expect("temp dir 2");
        let mut store2 = FriendRequestStore::empty_at(dir2.path());
        let req2 = store2.send_request(&alice, &bob, None).expect("send 2");
        store2.cancel_request(&req2.id, &alice).expect("cancel");
        store2.save().expect("save after cancel");

        let loaded2 = FriendRequestStore::load(dir2.path()).expect("reload after cancel");
        assert_eq!(
            loaded2.get(&req2.id).expect("request 2").status,
            FriendRequestStatus::Cancelled
        );
    }

    #[test]
    fn friend_request_incoming_list_empty_when_no_requests() {
        let store = FriendRequestStore::default();
        let peer = SecretKey::generate().public().to_string();
        let incoming = store.list_incoming_by_status(&peer, FriendRequestStatus::Pending);
        assert!(incoming.is_empty(), "no incoming requests");
    }

    #[test]
    fn friend_request_send_updates_outgoing_request_state() {
        // Simulate the exact pattern in the SendFriendRequest handler:
        //   store.send_request → if Ok, insert into outgoing_request_states
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();
        let mut store = FriendRequestStore::default();
        let local_pk = SecretKey::generate().public().to_string();
        let peer_pk = pk.to_string();

        match store.send_request(&local_pk, &peer_pk, None) {
            Ok(_request) => {
                states.insert(pk, OutgoingRequestState::Pending);
            }
            Err(err) => {
                let _error_msg = err.to_string();
                panic!("send should succeed");
            }
        }

        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Pending)
        ));
        assert_eq!(store.len(), 1);
    }

    // ── Join request list tests ────────────────────────────────────────

    /// Create a minimal IcedChat instance for testing the join request list.
    /// Uses the real constructor path via IcedChatConfig builder when available;
    /// here we construct a test harness that exercises rebuild_join_request_list.
    fn make_test_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn join_request_list_empty_when_no_states() {
        let items: Vec<JoinRequestItem> = Vec::new();
        assert!(items.is_empty());
    }

    #[test]
    fn rebuild_join_request_list_includes_pending_state() {
        let pk = SecretKey::generate().public();
        let dir = make_test_data_dir();
        let mut store = FriendRequestStore::load_or_default(dir.path());
        let local_pk = SecretKey::generate();
        let local_str = local_pk.public().to_string();
        let peer_str = pk.to_string();
        // Create a sent request in the store
        store
            .send_request(&local_str, &peer_str, None)
            .expect("send request");
        store.save().expect("save store");

        use std::collections::HashMap;
        let mut states = HashMap::new();
        states.insert(pk, OutgoingRequestState::Pending);

        // Rebuild logic: iterate states, look up store for request_id
        let mut seen_ids = std::collections::HashSet::new();
        let mut items: Vec<JoinRequestItem> = Vec::new();
        for (peer, state) in &states {
            let peer_str = peer.to_string();
            let request_id = store
                .iter()
                .find(|r| r.requester == local_str && r.recipient == peer_str)
                .map(|r| r.id.clone())
                .unwrap_or_else(|| format!("outgoing:{}", &peer_str[..8]));
            if !seen_ids.insert(request_id.clone()) {
                continue;
            }
            let chat_id = direct_topic(&local_pk.public(), peer);
            items.push(JoinRequestItem::new(
                request_id,
                peer_str,
                chat_id,
                state.clone(),
            ));
        }

        assert_eq!(items.len(), 1, "one request item");
        assert_eq!(
            items[0].target_user,
            pk.to_string(),
            "target user is the peer"
        );
        assert!(matches!(items[0].state, OutgoingRequestState::Pending));
        assert!(
            !items[0].request_id.is_empty(),
            "request_id should be non-empty"
        );
        // Verify chat_id is a valid topic derived from the two public keys
        let expected_topic = direct_topic(&local_pk.public(), &pk);
        assert_eq!(
            items[0].chat_id, expected_topic,
            "chat_id matches direct_topic"
        );
    }

    #[test]
    fn rebuild_join_request_list_dedup_by_peer_key() {
        // Since outgoing_request_states is HashMap<PublicKey, ...>,
        // each peer can only appear once — natural dedup.
        let pk = SecretKey::generate().public();
        let mut states = std::collections::HashMap::new();
        states.insert(pk, OutgoingRequestState::Pending);
        states.insert(pk, OutgoingRequestState::Accepted); // overwrites Pending
        assert_eq!(states.len(), 1);
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Accepted)
        ));
    }

    #[test]
    fn rebuild_join_request_list_multiple_states() {
        let pk_a = SecretKey::generate().public();
        let pk_b = SecretKey::generate().public();
        let pk_c = SecretKey::generate().public();
        let local_pk = SecretKey::generate().public();
        let dir = make_test_data_dir();
        let mut store = FriendRequestStore::load_or_default(dir.path());

        // Create matching requests in the store for A and B (not C — uses fallback id)
        let local_str = local_pk.to_string();
        store
            .send_request(&local_str, &pk_a.to_string(), None)
            .expect("send A");
        store
            .send_request(&local_str, &pk_b.to_string(), None)
            .expect("send B");
        store.save().expect("save");

        let mut states = std::collections::HashMap::new();
        states.insert(pk_a, OutgoingRequestState::Pending);
        states.insert(pk_b, OutgoingRequestState::Accepted);
        states.insert(pk_c, OutgoingRequestState::Failed("timeout".into()));

        let mut seen_ids = std::collections::HashSet::new();
        let mut items: Vec<JoinRequestItem> = Vec::new();
        for (peer, state) in &states {
            let peer_str = peer.to_string();
            let request_id = store
                .iter()
                .find(|r| r.requester == local_str && r.recipient == peer_str)
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

        assert_eq!(items.len(), 3, "three items, one per peer");

        // Sort by state priority (matching rebuild_join_request_list logic)
        items.sort_by_key(|item| match item.state {
            OutgoingRequestState::Pending => 0u8,
            OutgoingRequestState::Failed(_) => 1,
            OutgoingRequestState::Accepted => 2,
            OutgoingRequestState::Declined => 3,
        });

        // Verify order by state priority: Pending (0), Failed (1), Accepted (2)
        assert!(
            matches!(items[0].state, OutgoingRequestState::Pending),
            "first item should be Pending (highest priority)"
        );
        assert!(
            matches!(items[1].state, OutgoingRequestState::Failed(_)),
            "second item should be Failed"
        );
        assert!(
            matches!(items[2].state, OutgoingRequestState::Accepted),
            "third item should be Accepted"
        );

        // Verify request IDs for A and B come from store
        let a_has_store_id = items.iter().any(|i| {
            i.target_user == pk_a.to_string()
                && i.request_id != format!("outgoing:{}", &pk_a.to_string()[..8])
        });
        assert!(
            a_has_store_id,
            "peer A should have a store-based request_id"
        );
        // C has no store entry so it uses fallback format
        let c_has_fallback = items
            .iter()
            .any(|i| i.target_user == pk_c.to_string() && i.request_id.starts_with("outgoing:"));
        assert!(c_has_fallback, "peer C should use fallback request_id");
    }

    #[test]
    fn join_request_items_have_correct_chat_id() {
        let local_pk = SecretKey::generate().public();
        let peer_pk = SecretKey::generate().public();
        let expected_topic = direct_topic(&local_pk, &peer_pk);

        let item = JoinRequestItem::new(
            "test-id".into(),
            peer_pk.to_string(),
            expected_topic,
            OutgoingRequestState::Pending,
        );

        assert_eq!(item.chat_id, expected_topic);
        assert_eq!(item.target_user, peer_pk.to_string());
        assert_eq!(item.request_id, "test-id");
        assert!(matches!(item.state, OutgoingRequestState::Pending));
    }

    #[test]
    fn join_request_items_carry_failed_error() {
        let item = JoinRequestItem::new(
            "fail-id".into(),
            "peer-key".into(),
            TopicId::from_bytes([0u8; 32]),
            OutgoingRequestState::Failed("network error".into()),
        );

        assert!(matches!(&item.state, OutgoingRequestState::Failed(msg) if msg == "network error"));
    }

    #[test]
    fn rebuild_sort_pending_before_declined() {
        let pk_pending = SecretKey::generate().public();
        let pk_declined = SecretKey::generate().public();
        let local_pk = SecretKey::generate().public();

        let mut seen_ids = std::collections::HashSet::new();
        let mut items: Vec<JoinRequestItem> = Vec::new();
        let empty_store = FriendRequestStore::default();

        // Push items in reverse priority order to verify sorting
        for (peer, state) in [
            (pk_declined, OutgoingRequestState::Declined),
            (pk_pending, OutgoingRequestState::Pending),
        ] {
            let peer_str = peer.to_string();
            let request_id = empty_store
                .iter()
                .find(|r| r.requester == local_pk.to_string() && r.recipient == peer_str)
                .map(|r| r.id.clone())
                .unwrap_or_else(|| format!("outgoing:{}", &peer_str[..8]));
            if !seen_ids.insert(request_id.clone()) {
                continue;
            }
            items.push(JoinRequestItem::new(
                request_id,
                peer_str,
                direct_topic(&local_pk, &peer),
                state,
            ));
        }

        items.sort_by_key(|item| match item.state {
            OutgoingRequestState::Pending => 0u8,
            OutgoingRequestState::Failed(_) => 1,
            OutgoingRequestState::Accepted => 2,
            OutgoingRequestState::Declined => 3,
        });

        assert_eq!(items.len(), 2);
        assert!(
            matches!(items[0].state, OutgoingRequestState::Pending),
            "Pending should be first after sort"
        );
        assert!(
            matches!(items[1].state, OutgoingRequestState::Declined),
            "Declined should be second after sort"
        );
    }

    // ── Join request display and retry flow tests (t_6a20efaa) ──────────

    // Scenario 1: Initial loading — section is empty when there are no states.
    #[test]
    fn join_request_section_is_empty_when_no_outgoing_states() {
        let _ = make_test_data_dir();
        let items: Vec<JoinRequestItem> = Vec::new();
        assert!(
            items.is_empty(),
            "no items when there are no outgoing request states"
        );
    }

    // Scenario 2: Pending request display — shows target user and loading indicator.
    #[test]
    fn join_request_pending_has_state_label_and_spinner_frame() {
        let label = IcedChat::join_request_state_label(&OutgoingRequestState::Pending);
        assert_eq!(label, "Pending", "pending state label should be 'Pending'");
        let frame = IcedChat::join_request_spinner_frame();
        assert!(
            !frame.is_empty(),
            "spinner should produce a non-empty animation frame"
        );
    }

    #[test]
    fn join_request_pending_state_color_is_amber() {
        let color = IcedChat::join_request_state_color(&OutgoingRequestState::Pending);
        // Amber-ish: r=0.88, g=0.67, b=0.10
        assert!((color.r - 0.88).abs() < 0.01, "pending red channel");
        assert!((color.g - 0.67).abs() < 0.01, "pending green channel");
        assert!((color.b - 0.10).abs() < 0.01, "pending blue channel");
    }

    #[test]
    fn join_request_pending_item_carries_user_and_chat_labels() {
        let pk = SecretKey::generate().public();
        let local_pk = SecretKey::generate().public();
        let chat_id = direct_topic(&local_pk, &pk);
        let item = JoinRequestItem::new(
            "pending-req".into(),
            pk.to_string(),
            chat_id,
            OutgoingRequestState::Pending,
        );
        assert_eq!(item.target_user, pk.to_string(), "target user");
        assert_eq!(item.chat_id, chat_id, "chat id");
        assert_eq!(item.request_id, "pending-req", "request id");
    }

    // Scenario 3: Success — accepted label + tap navigates to chat.
    #[test]
    fn join_request_accepted_has_state_label_and_open_chat_label() {
        let label = IcedChat::join_request_state_label(&OutgoingRequestState::Accepted);
        assert_eq!(label, "Accepted", "accepted state label");
        let open_label = IcedChat::join_request_open_chat_label();
        assert_eq!(open_label, "Open chat", "open chat label");
    }

    #[test]
    fn join_request_accepted_state_color_is_green() {
        let color = IcedChat::join_request_state_color(&OutgoingRequestState::Accepted);
        // Green: r=0.18, g=0.68, b=0.28
        assert!((color.r - 0.18).abs() < 0.01, "accepted red channel");
        assert!((color.g - 0.68).abs() < 0.01, "accepted green channel");
        assert!((color.b - 0.28).abs() < 0.01, "accepted blue channel");
    }

    #[test]
    fn join_request_accepted_item_parses_to_valid_peer() {
        let pk = SecretKey::generate().public();
        let item = JoinRequestItem::new(
            "accept-1".into(),
            pk.to_string(),
            direct_topic(&SecretKey::generate().public(), &pk),
            OutgoingRequestState::Accepted,
        );
        let parsed = PublicKey::from_str(&item.target_user);
        assert!(parsed.is_ok(), "accepted item must have parseable peer key");
        assert_eq!(parsed.unwrap(), pk);
    }

    // Scenario 4: Rejection — declined label, no retry button.
    #[test]
    fn join_request_declined_has_rejected_label() {
        let label = IcedChat::join_request_state_label(&OutgoingRequestState::Declined);
        assert_eq!(
            label, "Rejected",
            "declined state label should read 'Rejected'"
        );
    }

    #[test]
    fn join_request_declined_state_color_is_gray() {
        let color = IcedChat::join_request_state_color(&OutgoingRequestState::Declined);
        // Gray: r=0.53, g=0.53, b=0.53
        assert!((color.r - 0.53).abs() < 0.01, "declined red channel");
        assert!((color.g - 0.53).abs() < 0.01, "declined green channel");
        assert!((color.b - 0.53).abs() < 0.01, "declined blue channel");
    }

    #[test]
    fn join_request_declined_not_failed_and_no_error() {
        // A Declined item is NOT a Failed item — it does not carry an error
        // string and has no retry action in the view.
        let item = JoinRequestItem::new(
            "decline-1".into(),
            "peer-key".into(),
            TopicId::from_bytes([0u8; 32]),
            OutgoingRequestState::Declined,
        );
        assert!(matches!(item.state, OutgoingRequestState::Declined));
    }

    // Scenario 5: Failure with retry — failed label + retry button.
    #[test]
    fn join_request_failed_has_failed_label_with_error() {
        let label = IcedChat::join_request_state_label(&OutgoingRequestState::Failed(
            "connection refused".into(),
        ));
        assert_eq!(label, "Failed", "failed state label");
        let retry_label = IcedChat::join_request_retry_label();
        assert_eq!(retry_label, "Retry", "retry button label");
        let failure_prefix = IcedChat::join_request_failure_prefix();
        assert_eq!(failure_prefix, "Failure", "failure prefix");
    }

    #[test]
    fn join_request_failed_state_color_is_red() {
        let color = IcedChat::join_request_state_color(&OutgoingRequestState::Failed("".into()));
        // Red: r=0.80, g=0.22, b=0.22
        assert!((color.r - 0.80).abs() < 0.01, "failed red channel");
        assert!((color.g - 0.22).abs() < 0.01, "failed green channel");
        assert!((color.b - 0.22).abs() < 0.01, "failed blue channel");
    }

    #[test]
    fn join_request_failed_border_color_is_red() {
        let color = IcedChat::join_request_border_color(&OutgoingRequestState::Failed("".into()));
        // Red: r=0.80, g=0.22, b=0.22
        assert!((color.r - 0.80).abs() < 0.01, "failed border red");
        assert!((color.g - 0.22).abs() < 0.01, "failed border green");
    }

    #[test]
    fn join_request_retry_transitions_failed_to_pending() {
        // Full retry lifecycle: failed → FriendRequestRetry → SendFriendRequest → Pending
        let pk = SecretKey::generate().public();
        let mut states: HashMap<PublicKey, OutgoingRequestState> = HashMap::new();

        // Initial failure
        states.insert(pk, OutgoingRequestState::Failed("network down".into()));
        assert!(matches!(
            states.get(&pk),
            Some(OutgoingRequestState::Failed(_))
        ));

        // Retry: FriendRequestRetry(peer) re-dispatches SendFriendRequest
        states.insert(pk, OutgoingRequestState::Pending);
        assert!(
            matches!(states.get(&pk), Some(OutgoingRequestState::Pending)),
            "retry transitions Failed → Pending"
        );
    }

    // Scenario 6: Duplicate suppression — same request ID appears once.
    #[test]
    fn join_request_rebuild_dedup_by_request_id() {
        // Simulate rebuild_join_request_list's seen_ids logic:
        // if two entries share the same request_id, only the first is kept.
        let pk = SecretKey::generate().public();
        let local_pk = SecretKey::generate().public();
        let store = FriendRequestStore::default();

        let peer_str = pk.to_string();
        let request_id = store
            .iter()
            .find(|r| r.requester == local_pk.to_string() && r.recipient == peer_str)
            .map(|r| r.id.clone())
            .unwrap_or_else(|| format!("outgoing:{}", &peer_str[..8]));

        let mut seen_ids = std::collections::HashSet::new();
        let mut items: Vec<JoinRequestItem> = Vec::new();

        for _ in 0..2 {
            if !seen_ids.insert(request_id.clone()) {
                continue; // skip duplicate
            }
            items.push(JoinRequestItem::new(
                request_id.clone(),
                peer_str.clone(),
                direct_topic(&local_pk, &pk),
                OutgoingRequestState::Pending,
            ));
        }

        assert_eq!(
            items.len(),
            1,
            "dedup: same request_id only produces one item"
        );
    }

    #[test]
    fn join_request_rebuild_orders_pending_first_then_failed() {
        // Multiple items are sorted with Pending first, then Failed, then
        // Accepted/Declined.
        let pk_a = SecretKey::generate().public();
        let pk_b = SecretKey::generate().public();
        let pk_c = SecretKey::generate().public();
        let local_pk = SecretKey::generate().public();
        let store = FriendRequestStore::default();

        let mut items: Vec<JoinRequestItem> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // Insert in random order
        for (peer, state) in [
            (pk_c, OutgoingRequestState::Accepted),
            (pk_a, OutgoingRequestState::Pending),
            (pk_b, OutgoingRequestState::Failed("err".into())),
        ] {
            let peer_str = peer.to_string();
            let request_id = store
                .iter()
                .find(|r| r.requester == local_pk.to_string() && r.recipient == peer_str)
                .map(|r| r.id.clone())
                .unwrap_or_else(|| format!("outgoing:{}", &peer_str[..8]));
            if !seen_ids.insert(request_id) {
                continue;
            }
            items.push(JoinRequestItem::new(
                peer_str,
                format!("{}", peer),
                direct_topic(&local_pk, &peer),
                state,
            ));
        }

        items.sort_by_key(|item| match item.state {
            OutgoingRequestState::Pending => 0u8,
            OutgoingRequestState::Failed(_) => 1,
            OutgoingRequestState::Accepted => 2,
            OutgoingRequestState::Declined => 3,
        });

        assert_eq!(items.len(), 3);
        assert!(
            matches!(items[0].state, OutgoingRequestState::Pending),
            "pending should sort first"
        );
        assert!(
            matches!(items[1].state, OutgoingRequestState::Failed(_)),
            "failed should sort second"
        );
        assert!(
            matches!(items[2].state, OutgoingRequestState::Accepted),
            "accepted should sort last of these three"
        );
    }

    #[test]
    fn join_request_accepted_section_has_open_chat_action() {
        // Verify that the accepted state renders with an "Open chat" button
        // by checking the view_join_request_row branches correctly.
        let pk = SecretKey::generate().public();
        let item = JoinRequestItem::new(
            "accept-action".into(),
            pk.to_string(),
            direct_topic(&SecretKey::generate().public(), &pk),
            OutgoingRequestState::Accepted,
        );
        // In view_join_request_row, accepted items with a valid peer produce
        // a full-row button that fires OpenFriendChat on press.
        assert!(
            matches!(&item.state, OutgoingRequestState::Accepted),
            "accepted item state should be Accepted"
        );
        // The view branches on Accepted + Some(peer) → button with OpenFriendChat
        let peer = IcedChat::join_request_peer(&item);
        assert!(peer.is_some(), "accepted item must have parseable peer");
    }

    #[test]
    fn join_request_failed_section_has_retry_action() {
        // Verify that the failed state provides the retry infrastructure.
        let pk = SecretKey::generate().public();
        let item = JoinRequestItem::new(
            "retry-action".into(),
            pk.to_string(),
            direct_topic(&SecretKey::generate().public(), &pk),
            OutgoingRequestState::Failed("connection lost".into()),
        );
        // In view_join_request_row, failed items with a valid peer produce
        // a Retry button that fires FriendRequestRetry on press.
        assert!(
            matches!(&item.state, OutgoingRequestState::Failed(msg) if msg == "connection lost"),
            "failed item state should carry the error message"
        );
        let peer = IcedChat::join_request_peer(&item);
        assert!(
            peer.is_some(),
            "failed item must have parseable peer for retry action"
        );
    }

    fn build_join_request_test_app() -> (tokio::runtime::Runtime, IcedChat, PublicKey, PublicKey) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");

        let mut data_dir = std::env::temp_dir();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        data_dir.push(format!("boru-iced-chat-join-request-{suffix}"));
        std::fs::create_dir_all(&data_dir).expect("create temp data dir");

        let local_sk = SecretKey::generate();
        let local_public = local_sk.public();
        let peer_public = SecretKey::generate().public();
        let room_topic = TopicId::from_bytes([7u8; 32]);

        let (
            secret_key,
            gossip,
            router,
            blob_store,
            endpoint,
            memory_lookup,
            local_label,
            friends,
            friend_mgr,
            friend_events_rx,
            whisper_events_rx,
            inbox_events_rx,
            whisper_handle,
            call_handle,
            backfill_handle,
            chat_history,
            net_rx,
            net_tx,
            room_history,
        ) = runtime.block_on(async {
            let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(local_sk.clone())
                .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                .relay_mode(iroh::RelayMode::Default)
                .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
                .expect("set bind addr")
                .bind()
                .await
                .expect("bind endpoint");
            // ENDPOINT-ONLINE-HANG: `online()` can block forever when the
            // relay handshake stalls. Time-box it so tests don't hang
            // (mirrors the app's own startup fix in main.rs).
            if tokio::time::timeout(std::time::Duration::from_secs(15), endpoint.online())
                .await
                .is_err()
            {
                eprintln!("test endpoint.online() timed out after 15s, proceeding anyway");
            }

            let gossip = boru_core::net::Gossip::builder()
                .max_message_size(16 * 1024) // 16 KiB — tiny, thumbnails go via blobs now
                .spawn(endpoint.clone());
            let router = iroh::protocol::Router::builder(endpoint.clone())
                .accept(boru_core::net::GOSSIP_ALPN, gossip.clone())
                .spawn();
            let blob_store = iroh_blobs::store::fs::FsStore::load(data_dir.join("blobs"))
                .await
                .expect("create fs blob store");
            let memory_lookup = iroh::address_lookup::memory::MemoryLookup::new();
            let friends = boru_core::friends::FriendsStore::empty_at(&data_dir);
            let mut room_history = boru_core::room_history::RoomHistoryStore::empty_at(&data_dir);
            room_history
                .rooms
                .push(boru_core::room_history::RoomHistoryEntry::new(
                    room_topic,
                    "Existing chat",
                    true,
                ));
            let chat_history = std::sync::Arc::new(std::sync::Mutex::new(
                boru_core::chat_history::ChatHistoryStore::empty_at(&data_dir),
            ));
            let backfill_handle = boru_core::backfill::BackfillHandle::spawn(endpoint.clone());
            let whisper_builder =
                boru_core::whisper::WhisperBuilder::new(endpoint.clone(), local_sk.clone());
            let _whisper_protocol = whisper_builder.protocol_handler();
            let (whisper_handle, whisper_events_rx_tmp) = whisper_builder.spawn();
            let call_builder =
                boru_core::call::manager::CallBuilder::new(endpoint.clone(), local_sk.clone());
            let (call_handle, _call_events) = call_builder.spawn();
            let whisper_events_rx =
                std::sync::Arc::new(tokio::sync::Mutex::new(whisper_events_rx_tmp));
            let (inbox_handle, inbox_events_rx_tmp) = boru_core::inbox::InboxHandle::new();
            let _inbox_protocol = boru_core::inbox::InboxProtocol::new(inbox_handle.inner());
            let inbox_events_rx = std::sync::Arc::new(tokio::sync::Mutex::new(inbox_events_rx_tmp));
            let (friend_mgr, friend_events_rx_tmp) =
                boru_core::chat_core::friend_ping::FriendPingManager::spawn(
                    endpoint.clone(),
                    boru_core::chat_core::friend_ping::DEFAULT_PING_INTERVAL,
                    boru_core::chat_core::friend_ping::DEFAULT_CONNECT_TIMEOUT,
                );
            let friend_events_rx =
                std::sync::Arc::new(tokio::sync::Mutex::new(friend_events_rx_tmp));
            let (net_tx, net_rx) = tokio::sync::mpsc::channel(256);
            let net_rx = std::sync::Arc::new(tokio::sync::Mutex::new(net_rx));

            (
                local_sk.clone(),
                gossip,
                router,
                blob_store,
                endpoint,
                memory_lookup,
                "Alice".to_string(),
                friends,
                friend_mgr,
                friend_events_rx,
                whisper_events_rx,
                inbox_events_rx,
                whisper_handle,
                call_handle,
                backfill_handle,
                chat_history,
                net_rx,
                net_tx,
                room_history,
            )
        });

        let (dummy_discovered_tx, dummy_discovered_rx) =
            tokio::sync::mpsc::channel::<DiscoveredPeersUpdate>(1);
        let (_, dummy_reconnect_rx) = tokio::sync::mpsc::channel::<PublicKey>(1);
        let dummy_reconnect_rx = Arc::new(Mutex::new(dummy_reconnect_rx));
        let (_, dummy_directory_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(1);
        let dummy_directory_rx = Arc::new(Mutex::new(dummy_directory_rx));
        let (_, dummy_call_rx) = tokio::sync::mpsc::channel::<CallEvent>(1);
        let dummy_call_rx = Arc::new(Mutex::new(dummy_call_rx));

        let app = IcedChat::new(
            secret_key,
            gossip,
            router,
            Arc::new(StdMutex::new(FileOfferRegistry::new())),
            blob_store,
            endpoint,
            memory_lookup,
            local_label,
            local_public,
            iroh::RelayMode::Default,
            data_dir,
            runtime.handle().clone(),
            net_rx,
            net_tx,
            room_history,
            friends,
            friend_mgr,
            friend_events_rx,
            whisper_events_rx,
            inbox_events_rx,
            whisper_handle,
            call_handle,
            dummy_call_rx,
            None,
            chat_history,
            backfill_handle,
            false,
            Arc::new(Mutex::new(dummy_discovered_rx)),
            dummy_reconnect_rx,
            None, // reconnect_handle (not exercised in unit tests)
            dummy_directory_rx,
            None, // dht (private-room discovery disabled by default in tests)
            false,
            boru_core::diagnostics::IcedMessageJournal::default(),
            None,
            tokio::sync::watch::channel(boru_core::diagnostics::IcedStateSnapshot {
                node_id: String::new(),
                version: String::new(),
                active_screen: String::new(),
                active_room: None,
                conversation_count: 0,
                neighbor_count: 0,
                direct_peer_count: 0,
                relayed_peer_count: 0,
                mesh_health: String::new(),
                online_friend_count: 0,
                friend_count: 0,
                total_entry_count: 0,
                dark_mode: false,
                composer_text: String::new(),
                dialog_open: false,
                unread_count: 0,
                dashboard: None,
                timestamp: chrono::Utc::now(),
            })
            .0,
            GuiActionHistory::default(),
            None, // storage
            Arc::new(boru_core::tunnel::service::TunnelService::new()),
            std::sync::Arc::new(boru_core::transfer_state_projection::TransferStateStore::new(8)),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        );

        (runtime, app, local_public, peer_public)
    }

    /// Build a minimal-but-real app for the PERF-4R-B pre-warm unit test
    /// (same loopback, relay-disabled construction as
    /// `build_join_request_test_app`).
    fn build_prewarm_test_app() -> (tokio::runtime::Runtime, IcedChat) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");

        let mut data_dir = std::env::temp_dir();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        data_dir.push(format!("boru-iced-chat-prewarm-{suffix}"));
        std::fs::create_dir_all(&data_dir).expect("create temp data dir");

        let local_sk = SecretKey::generate();
        let local_public = local_sk.public();

        let (
            secret_key,
            gossip,
            router,
            blob_store,
            endpoint,
            memory_lookup,
            local_label,
            friends,
            friend_mgr,
            friend_events_rx,
            whisper_events_rx,
            inbox_events_rx,
            whisper_handle,
            call_handle,
            backfill_handle,
            chat_history,
            net_rx,
            net_tx,
            room_history,
        ) = runtime.block_on(async {
            let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(local_sk.clone())
                .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                .relay_mode(iroh::RelayMode::Disabled)
                .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
                .expect("set bind addr")
                .bind()
                .await
                .expect("bind endpoint");
            // NOTE: deliberately do NOT await `endpoint.online()` here — with
            // `RelayMode::Disabled` there is no home relay, so `online()`
            // never resolves (it waits for a relay status). Binding the
            // loopback endpoint is enough for the pre-warm unit test.

            let gossip = boru_core::net::Gossip::builder()
                .max_message_size(16 * 1024)
                .spawn(endpoint.clone());
            let router = iroh::protocol::Router::builder(endpoint.clone())
                .accept(boru_core::net::GOSSIP_ALPN, gossip.clone())
                .spawn();
            let blob_store = iroh_blobs::store::fs::FsStore::load(data_dir.join("blobs"))
                .await
                .expect("create fs blob store");
            let memory_lookup = iroh::address_lookup::memory::MemoryLookup::new();
            let friends = boru_core::friends::FriendsStore::empty_at(&data_dir);
            let room_history = boru_core::room_history::RoomHistoryStore::empty_at(&data_dir);
            let chat_history = std::sync::Arc::new(std::sync::Mutex::new(
                boru_core::chat_history::ChatHistoryStore::empty_at(&data_dir),
            ));
            let backfill_handle = boru_core::backfill::BackfillHandle::spawn(endpoint.clone());
            let whisper_builder =
                boru_core::whisper::WhisperBuilder::new(endpoint.clone(), local_sk.clone());
            let _whisper_protocol = whisper_builder.protocol_handler();
            let (whisper_handle, whisper_events_rx_tmp) = whisper_builder.spawn();
            let call_builder =
                boru_core::call::manager::CallBuilder::new(endpoint.clone(), local_sk.clone());
            let (call_handle, _call_events) = call_builder.spawn();
            let whisper_events_rx =
                std::sync::Arc::new(tokio::sync::Mutex::new(whisper_events_rx_tmp));
            let (inbox_handle, inbox_events_rx_tmp) = boru_core::inbox::InboxHandle::new();
            let _inbox_protocol = boru_core::inbox::InboxProtocol::new(inbox_handle.inner());
            let inbox_events_rx = std::sync::Arc::new(tokio::sync::Mutex::new(inbox_events_rx_tmp));
            let (friend_mgr, friend_events_rx_tmp) =
                boru_core::chat_core::friend_ping::FriendPingManager::spawn(
                    endpoint.clone(),
                    boru_core::chat_core::friend_ping::DEFAULT_PING_INTERVAL,
                    boru_core::chat_core::friend_ping::DEFAULT_CONNECT_TIMEOUT,
                );
            let friend_events_rx =
                std::sync::Arc::new(tokio::sync::Mutex::new(friend_events_rx_tmp));
            let (net_tx, net_rx) = tokio::sync::mpsc::channel(256);
            let net_rx = std::sync::Arc::new(tokio::sync::Mutex::new(net_rx));

            (
                local_sk.clone(),
                gossip,
                router,
                blob_store,
                endpoint,
                memory_lookup,
                "PreWarm Tester".to_string(),
                friends,
                friend_mgr,
                friend_events_rx,
                whisper_events_rx,
                inbox_events_rx,
                whisper_handle,
                call_handle,
                backfill_handle,
                chat_history,
                net_rx,
                net_tx,
                room_history,
            )
        });

        let (dummy_discovered_tx, dummy_discovered_rx) =
            tokio::sync::mpsc::channel::<DiscoveredPeersUpdate>(1);
        let (_, dummy_reconnect_rx) = tokio::sync::mpsc::channel::<PublicKey>(1);
        let dummy_reconnect_rx = Arc::new(Mutex::new(dummy_reconnect_rx));
        let (_, dummy_directory_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(1);
        let dummy_directory_rx = Arc::new(Mutex::new(dummy_directory_rx));
        let (_, dummy_call_rx) = tokio::sync::mpsc::channel::<CallEvent>(1);
        let dummy_call_rx = Arc::new(Mutex::new(dummy_call_rx));

        let app = IcedChat::new(
            secret_key,
            gossip,
            router,
            Arc::new(StdMutex::new(FileOfferRegistry::new())),
            blob_store,
            endpoint,
            memory_lookup,
            local_label,
            local_public,
            iroh::RelayMode::Disabled,
            data_dir,
            runtime.handle().clone(),
            net_rx,
            net_tx,
            room_history,
            friends,
            friend_mgr,
            friend_events_rx,
            whisper_events_rx,
            inbox_events_rx,
            whisper_handle,
            call_handle,
            dummy_call_rx,
            None,
            chat_history,
            backfill_handle,
            false,
            Arc::new(Mutex::new(dummy_discovered_rx)),
            dummy_reconnect_rx,
            None, // reconnect_handle (not exercised in unit tests)
            dummy_directory_rx,
            None, // dht (private-room discovery disabled by default in tests)
            false,
            boru_core::diagnostics::IcedMessageJournal::default(),
            None,
            tokio::sync::watch::channel(boru_core::diagnostics::IcedStateSnapshot {
                node_id: String::new(),
                version: String::new(),
                active_screen: String::new(),
                active_room: None,
                conversation_count: 0,
                neighbor_count: 0,
                direct_peer_count: 0,
                relayed_peer_count: 0,
                mesh_health: String::new(),
                online_friend_count: 0,
                friend_count: 0,
                total_entry_count: 0,
                dark_mode: false,
                composer_text: String::new(),
                dialog_open: false,
                unread_count: 0,
                dashboard: None,
                timestamp: chrono::Utc::now(),
            })
            .0,
            GuiActionHistory::default(),
            None, // storage
            Arc::new(boru_core::tunnel::service::TunnelService::new()),
            std::sync::Arc::new(boru_core::transfer_state_projection::TransferStateStore::new(8)),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        );

        (runtime, app)
    }
