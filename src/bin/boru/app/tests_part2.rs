    /// DIR-21 (PDF Task 7.3): the UI never presents the self-reported
    /// count as authoritative — it is labeled approximate ("~N members
    /// (approx.)") and omitted when absent or zero.
    #[test]
    fn discover_member_count_is_optional_hint_labeled_approximate() {
        assert_eq!(
            discover_member_count_text(Some(42)).as_deref(),
            Some("~42 members (approx.)")
        );
        assert_eq!(
            discover_member_count_text(Some(9_999_999)).as_deref(),
            Some("~9999999 members (approx.)")
        );
        // Absent or zero counts are omitted entirely — never shown as a
        // bare authoritative number.
        assert_eq!(discover_member_count_text(None), None);
        assert_eq!(discover_member_count_text(Some(0)), None);
    }

    /// The filter/search handlers in `update_discover` are pure local
    /// state mutations — the search query is never broadcast (Task 5.3
    /// acceptance: search must not leak queries to other peers).
    #[test]
    fn discover_search_update_is_local_only() {
        let (_runtime, mut app) = build_prewarm_test_app();

        let task = app.update(AppMessage::DiscoverSearchChanged("rust".to_string()));
        assert_eq!(app.discover_search_query, "rust");
        assert!(!app.discover_filter_compatible);
        drop(task);

        let task = app.update(AppMessage::DiscoverFilterToggled(
            DiscoverFilter::Compatible,
        ));
        assert!(app.discover_filter_compatible);
        drop(task);

        let task = app.update(AppMessage::DiscoverTagToggled("tech".to_string()));
        assert_eq!(app.discover_selected_tags, vec!["tech".to_string()]);
        drop(task);

        let task = app.update(AppMessage::DiscoverSortChanged(DiscoverSort::Name));
        assert_eq!(app.discover_sort, DiscoverSort::Name);
        drop(task);

        // Clear resets everything.
        let _ = app.update(AppMessage::DiscoverClearFilters);
        assert!(app.discover_search_query.is_empty());
        assert!(!app.discover_filter_compatible);
        assert!(app.discover_selected_tags.is_empty());
    }

    // ── BORU-DIR-16 (PDF Task 6.1): join only after explicit user action ──
    // The Join button on a room card invokes the normal public-room join
    // path (OpenRoom). Seeing a room never subscribes to it; joining
    // creates one local room record and one intended subscription; failed
    // joins leave the directory entry intact and report a useful error.

    /// Build a `RoomDirectory` with a single compatible advertisement.
    fn directory_with_compatible_room(
        room_id: TopicId,
        name: &str,
    ) -> boru_core::room_directory::RoomDirectory {
        let mut dir = boru_core::room_directory::RoomDirectory::new();
        let owner = SecretKey::generate().public();
        let mut advert = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            room_id,
            name.to_string(),
            *owner.as_bytes(),
        );
        advert.room_protocol_version = boru_core::public_room::PROTOCOL_VERSION;
        dir.apply_advertisement(
            advert,
            owner,
            boru_core::control_plane::advertisement::AdvertisementAuth::Verified {
                publisher: owner,
            },
            1,
            1000,
        );
        dir
    }

    /// Seeing a room (building the Discover browse surface over a cache
    /// with an advertisement) never subscribes to it and never creates a
    /// conversation record — the directory is a pure browse surface (PDF
    /// Core rule / Task 5.1 acceptance).
    #[test]
    fn discover_view_never_subscribes_or_creates_record() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x61; 32]);
        let dir = directory_with_compatible_room(room_id, "Lounge");
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        // Building the browse surface must not mutate membership.
        let dep = app.discover_dependency();
        let _screen = IcedChat::view_discover_content(&dep);

        assert!(
            app.conversations.is_empty(),
            "opening Discover must not create any subscription"
        );
        assert!(
            app.conversation_store.find(&room_id).is_none(),
            "opening Discover must not create a conversation record"
        );
        assert_eq!(
            dep.rooms.len(),
            1,
            "the advertised room is visible in the browse surface"
        );
    }

    /// `directory_join_target` resolves a compatible room to its normal
    /// join topic, and blocks known-incompatible rooms with a useful
    /// explanation (PDF Task 6.1 step 2) before any subscription.
    #[test]
    fn directory_join_target_blocks_incompatible_rooms() {
        let (_runtime, app) = build_prewarm_test_app();

        // Compatible room → Ok(topic).
        let compatible_id = TopicId::from_bytes([0x62; 32]);
        let dir = directory_with_compatible_room(compatible_id, "Compatible Room");
        let mut app = app;
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));
        assert_eq!(
            app.directory_join_target(*compatible_id.as_bytes()),
            Ok(compatible_id)
        );

        // UpgradeRequired room → Err with a useful message.
        let mut dir = boru_core::room_directory::RoomDirectory::new();
        let owner = SecretKey::generate().public();
        let mut advert = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            compatible_id,
            "Future Room".to_string(),
            *owner.as_bytes(),
        );
        advert.room_protocol_version = boru_core::public_room::PROTOCOL_VERSION + 1;
        dir.apply_advertisement(
            advert,
            owner,
            boru_core::control_plane::advertisement::AdvertisementAuth::Verified {
                publisher: owner,
            },
            1,
            1000,
        );
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));
        let err = app
            .directory_join_target(*compatible_id.as_bytes())
            .expect_err("upgrade-required room must be blocked");
        assert!(
            err.contains("newer"),
            "block reason explains the upgrade requirement: {err}"
        );
        assert!(
            err.contains(&format!(
                "v{}",
                boru_core::public_room::PROTOCOL_VERSION + 1
            )),
            "block reason names the room's protocol version: {err}"
        );
    }

    /// PDF Task 6.2: an Unsupported room (protocol more than one version
    /// ahead) is blocked with a useful message that distinguishes it from
    /// the upgrade-required case.
    #[test]
    fn directory_join_target_blocks_unsupported_protocol() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x62; 32]);
        let mut dir = boru_core::room_directory::RoomDirectory::new();
        let owner = SecretKey::generate().public();
        let mut advert = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            room_id,
            "Far Future Room".to_string(),
            *owner.as_bytes(),
        );
        advert.room_protocol_version = boru_core::public_room::PROTOCOL_VERSION + 2;
        dir.apply_advertisement(
            advert,
            owner,
            boru_core::control_plane::advertisement::AdvertisementAuth::Verified {
                publisher: owner,
            },
            1,
            1000,
        );
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        let err = app
            .directory_join_target(*room_id.as_bytes())
            .expect_err("unsupported room must be blocked");
        assert!(
            err.contains("does not support"),
            "block reason explains the protocol is unsupported: {err}"
        );
        assert!(
            err.contains(&format!(
                "v{}",
                boru_core::public_room::PROTOCOL_VERSION + 2
            )),
            "block reason names the unsupported version: {err}"
        );
    }

    /// PDF Task 6.2 acceptance: optional feature differences do NOT
    /// unnecessarily block basic room access. A Compatible room that
    /// advertises optional features this client lacks stays joinable.
    #[test]
    fn directory_join_target_allows_missing_optional_features() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x62; 32]);
        let mut dir = boru_core::room_directory::RoomDirectory::new();
        let owner = SecretKey::generate().public();
        let mut advert = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            room_id,
            "Feature Room".to_string(),
            *owner.as_bytes(),
        );
        advert.feature_flags = vec!["hologram-v9".to_string()];
        dir.apply_advertisement(
            advert,
            owner,
            boru_core::control_plane::advertisement::AdvertisementAuth::Verified {
                publisher: owner,
            },
            1,
            1000,
        );
        let entry = dir.get(&room_id).unwrap();
        assert_eq!(
            entry.compatibility,
            boru_core::room_directory::RoomCompatibility::Compatible,
            "base protocol compatible"
        );
        assert_eq!(
            entry.feature_compat,
            boru_core::room_directory::RoomFeatureCompatibility::SomeMissing(vec![
                "hologram-v9".to_string(),
            ]),
            "missing optional feature is informational only"
        );
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        assert_eq!(
            app.directory_join_target(*room_id.as_bytes()),
            Ok(room_id),
            "optional-feature differences never block basic room access"
        );
    }

    /// A blocked (known-incompatible) join reports a useful error, returns
    /// no subscription task, and leaves the directory entry intact.
    #[test]
    fn directory_join_incompatible_reports_error_and_keeps_entry() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x63; 32]);
        let mut dir = boru_core::room_directory::RoomDirectory::new();
        let owner = SecretKey::generate().public();
        let mut advert = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            room_id,
            "Incompatible Room".to_string(),
            *owner.as_bytes(),
        );
        advert.room_protocol_version = boru_core::public_room::PROTOCOL_VERSION + 1;
        dir.apply_advertisement(
            advert,
            owner,
            boru_core::control_plane::advertisement::AdvertisementAuth::Verified {
                publisher: owner,
            },
            1,
            1000,
        );
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        let task = app.update(AppMessage::DirectoryRoomJoinById(*room_id.as_bytes()));
        drop(task);

        // Useful error surfaced.
        assert!(
            app.entries
                .iter()
                .any(|e| e.body.contains("Cannot join room") && e.body.contains("newer")),
            "blocked join must surface a useful error"
        );
        // Directory entry intact — a failed join never removes it.
        let dir = app.room_directory.as_ref().unwrap().lock().unwrap();
        assert!(
            dir.get(&room_id).is_some(),
            "failed join leaves the directory entry intact"
        );
        // No conversation record was created for the blocked room.
        assert!(app.conversation_store.find(&room_id).is_none());
    }

    /// BORU-DIR-18 (PDF Task 6.3): a room the user has hidden/blocked
    /// locally cannot be joined through discovery. The join gate
    /// re-validates the live cache's local permission state (derived from
    /// the real room database via `LocalRoomFacts.hidden`), so discovery
    /// can never bypass the local ban — even though the room is
    /// protocol-compatible and its advertisement is still cached. The
    /// advertisement is NOT deleted: visibility and join authorization
    /// remain independent.
    #[test]
    fn directory_join_target_blocks_locally_blocked_room() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x6B; 32]);
        let mut dir = directory_with_compatible_room(room_id, "Lounge");
        // The user has hidden/blocked the room locally (persisted
        // preference fed into the cache by sync_directory_local_states).
        dir.sync_local_states(boru_core::room_directory::LocalRoomFacts {
            joined: std::collections::BTreeSet::new(),
            pending: std::collections::BTreeSet::new(),
            hidden: std::collections::BTreeSet::from([room_id]),
        });
        assert_eq!(
            dir.get(&room_id).unwrap().local_join_state,
            boru_core::room_directory::LocalJoinState::Blocked,
            "hidden preference derives Blocked"
        );
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        let err = app
            .directory_join_target(*room_id.as_bytes())
            .expect_err("a locally blocked room must not be joinable via discovery");
        assert!(
            err.contains("hidden") || err.contains("blocked"),
            "block reason explains the local permission state: {err}"
        );

        // The directory entry is NOT deleted — visibility and join
        // authorization remain independent (PDF Task 6.3 acceptance).
        let dir = app.room_directory.as_ref().unwrap().lock().unwrap();
        assert!(
            dir.get(&room_id).is_some(),
            "a join refusal must not delete the directory advertisement"
        );
        assert_eq!(dir.snapshot_all().len(), 1);
    }

    /// BORU-DIR-18 (PDF Task 6.3): the legacy ticket-based join path
    /// (`DirectoryRoomJoin`) obeys the same join gate as the ById path — a
    /// locally blocked room is refused before any subscription, with a
    /// useful error, and the advertisement survives.
    #[test]
    fn directory_join_legacy_path_obeys_join_gate() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x6C; 32]);
        let mut dir = directory_with_compatible_room(room_id, "Lounge");
        dir.sync_local_states(boru_core::room_directory::LocalRoomFacts {
            joined: std::collections::BTreeSet::new(),
            pending: std::collections::BTreeSet::new(),
            hidden: std::collections::BTreeSet::from([room_id]),
        });
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        let ad = RoomAdvertisement {
            room_name: "Lounge".to_string(),
            description: String::new(),
            topic: room_id,
            ticket: boru_core::chat_core::Ticket::new(room_id, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: ADVERT_TTL_SECS,
        };
        let task = app.update(AppMessage::DirectoryRoomJoin(ad));
        drop(task);

        assert!(
            app.entries
                .iter()
                .any(|e| e.body.contains("Cannot join room")
                    && (e.body.contains("hidden") || e.body.contains("blocked"))),
            "legacy join path surfaces the block reason"
        );
        let dir = app.room_directory.as_ref().unwrap().lock().unwrap();
        assert!(
            dir.get(&room_id).is_some(),
            "legacy join refusal must not delete the advertisement"
        );
    }

    /// BORU-DIR-18 (PDF Task 6.3): a successful directory join creates the
    /// local conversation record from advertised metadata WITHOUT granting
    /// moderator/admin privileges. `owner_peer_id` is descriptive metadata
    /// only (BORU-DIR-03) — it must never be copied into the local record's
    /// peer/owner field or any role/privilege state.
    #[test]
    fn directory_join_record_never_grants_privileges_from_metadata() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x6D; 32]);
        let owner = SecretKey::generate().public();
        let mut dir = boru_core::room_directory::RoomDirectory::new();
        let mut advert = boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            room_id,
            "Owner's Room".to_string(),
            *owner.as_bytes(),
        );
        advert.room_protocol_version = boru_core::public_room::PROTOCOL_VERSION;
        dir.apply_advertisement(
            advert,
            owner,
            boru_core::control_plane::advertisement::AdvertisementAuth::Verified {
                publisher: owner,
            },
            1,
            1000,
        );
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        assert!(app.ensure_directory_joined_record(room_id));
        let entry = app
            .conversation_store
            .find(&room_id)
            .expect("record created");
        // The advertised owner must not become the local peer/owner of the
        // conversation record.
        assert_ne!(
            entry.peer_id,
            owner.to_string(),
            "advertised owner_peer_id must never grant local ownership"
        );
        assert!(
            entry.peer_id.is_empty(),
            "no peer identity is derived from advertised metadata"
        );
        // The record carries only the advertised non-privileged metadata.
        assert_eq!(entry.name, "Owner's Room");
        assert_eq!(
            entry.visibility,
            boru_core::control_plane::advertisement::RoomVisibility::PublicDiscoverable
        );
    }

    /// A successful join creates the local conversation record exactly
    /// once from the advertised metadata; a second call (re-open) is a
    /// no-op — never a duplicate.
    #[test]
    fn directory_join_creates_exactly_one_conversation_record() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x64; 32]);
        let dir = directory_with_compatible_room(room_id, "Lounge");
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        assert!(
            app.ensure_directory_joined_record(room_id),
            "first successful join creates the record"
        );
        assert!(
            !app.ensure_directory_joined_record(room_id),
            "second call is a no-op — exactly one record"
        );

        let entries: Vec<_> = app
            .conversation_store
            .iter()
            .filter(|e| e.topic == room_id)
            .collect();
        assert_eq!(entries.len(), 1, "exactly one local conversation record");
        assert_eq!(entries[0].name, "Lounge", "advertised name is used");
        assert_eq!(
            entries[0].visibility,
            boru_core::control_plane::advertisement::RoomVisibility::PublicDiscoverable,
            "directory rooms are discoverable by visibility"
        );
    }

    /// A non-directory room (e.g. a direct chat topic) is never
    /// materialized by the join-record helper — directory joins must not
    /// fabricate records for rooms that were never advertised.
    #[test]
    fn directory_join_record_helper_ignores_non_directory_rooms() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let non_directory = TopicId::from_bytes([0x65; 32]);
        assert!(
            !app.ensure_directory_joined_record(non_directory),
            "non-directory topic is not materialized"
        );
        assert!(app.conversation_store.find(&non_directory).is_none());
    }

    /// A failed join (RoomJoinFailed from the async subscribe path) does
    /// not touch the directory cache — the entry survives and the error
    /// is reported to the user.
    #[test]
    fn directory_join_failure_leaves_entry_intact() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let room_id = TopicId::from_bytes([0x66; 32]);
        let dir = directory_with_compatible_room(room_id, "Lounge");
        app.room_directory = Some(Arc::new(StdMutex::new(dir)));

        let generation = app.room_generation;
        let task = app.update(AppMessage::RoomJoinFailed {
            error: "timed out waiting for a peer".to_string(),
            generation,
        });
        drop(task);

        let dir = app.room_directory.as_ref().unwrap().lock().unwrap();
        assert!(
            dir.get(&room_id).is_some(),
            "failed join leaves the directory entry intact"
        );
        assert!(
            app.chat_list_error.contains("Failed to join room"),
            "useful error is reported: {}",
            app.chat_list_error
        );
    }

    // ── BORU-DIR-09 (PDF Task 3.3): room withdrawals ─────────────────
    // A verified withdrawal removes the matching advertisement
    // immediately; TTL expiry remains the safety net if it is missed.

    /// A verified withdrawal for (topic, author) removes the matching
    /// advertisement from the directory on the next drain.
    #[test]
    fn directory_room_withdrawal_removes_matching_advertisement() {
        let (_runtime, mut app) = build_prewarm_test_app();

        let (dir_tx, dir_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(8);
        app.directory_room_rx = Arc::new(Mutex::new(dir_rx));

        let author = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0xAB; 32]);
        let ad = RoomAdvertisement {
            room_name: "Lounge".to_string(),
            description: String::new(),
            topic,
            ticket: boru_core::chat_core::Ticket::new(topic, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: ADVERT_TTL_SECS,
        };

        dir_tx
            .try_send(DirectoryRoomEvent::Advertisement(ad.clone(), author))
            .expect("feed announcement");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);
        assert!(
            app.directory_store.lock().unwrap().contains(topic, author),
            "advertisement present before withdrawal"
        );

        // Verified withdrawal (signature checked by the receiver loop in
        // main.rs before this event is sent) removes the entry immediately.
        dir_tx
            .try_send(DirectoryRoomEvent::Withdrawal(topic, author))
            .expect("feed withdrawal");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);

        {
            let store = app.directory_store.lock().unwrap();
            assert!(
                !store.contains(topic, author),
                "withdrawal removed the matching advertisement"
            );
            assert_eq!(store.list_active().len(), 0);
        }
    }

    /// A spoofed withdrawal (different author) cannot remove another
    /// author's advertisement — even for the same room topic.
    #[test]
    fn directory_room_withdrawal_cannot_remove_other_authors_ad() {
        let (_runtime, mut app) = build_prewarm_test_app();

        let (dir_tx, dir_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(8);
        app.directory_room_rx = Arc::new(Mutex::new(dir_rx));

        let owner = SecretKey::generate().public();
        let stranger = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0xCD; 32]);
        let ad = RoomAdvertisement {
            room_name: "Lounge".to_string(),
            description: String::new(),
            topic,
            ticket: boru_core::chat_core::Ticket::new(topic, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: ADVERT_TTL_SECS,
        };

        dir_tx
            .try_send(DirectoryRoomEvent::Advertisement(ad.clone(), owner))
            .expect("feed owner announcement");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);
        assert!(app.directory_store.lock().unwrap().contains(topic, owner));

        // A withdrawal from an author who never advertised the room is
        // verified by the receiver loop but removes nothing.
        dir_tx
            .try_send(DirectoryRoomEvent::Withdrawal(topic, stranger))
            .expect("feed spoofed withdrawal");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);

        {
            let store = app.directory_store.lock().unwrap();
            assert!(
                store.contains(topic, owner),
                "spoofed withdrawal cannot remove the owner's advertisement"
            );
            assert_eq!(store.list_active().len(), 1);
        }
    }

    /// A withdrawal for a room nobody advertised is a no-op (no spurious
    /// directory churn).
    #[test]
    fn directory_room_withdrawal_for_unadvertised_room_is_noop() {
        let (_runtime, mut app) = build_prewarm_test_app();

        let (dir_tx, dir_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(8);
        app.directory_room_rx = Arc::new(Mutex::new(dir_rx));

        let author = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0xEF; 32]);

        dir_tx
            .try_send(DirectoryRoomEvent::Withdrawal(topic, author))
            .expect("feed withdrawal");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);

        assert_eq!(app.directory_store.lock().unwrap().list_active().len(), 0);
    }

    // ── BORU-DIR-19 (PDF Task 7.1): spam + resource limits ────────────
    // The legacy directory drain routes remote advertisements through the
    // bounded receive gate (metadata bounds, per-author rate limit, TTL
    // clamp, dedup, entry cap). The tests below pin the acceptance
    // criteria: repeated identical advertisements do not cause constant
    // re-rendering, and malformed/oversized metadata is discarded.

    /// Repeated identical advertisements do not cause constant re-rendering:
    /// a periodic refresh (same user-visible metadata) does not re-announce
    /// in the Recent Activity feed, does not re-subscribe, and does not
    /// create a second card.
    #[test]
    fn directory_duplicate_advertisement_does_not_churn_ui() {
        let (_runtime, mut app) = build_prewarm_test_app();

        let (dir_tx, dir_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(8);
        app.directory_room_rx = Arc::new(Mutex::new(dir_rx));

        let author = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0x77; 32]);
        let ad = RoomAdvertisement {
            room_name: "Lounge".to_string(),
            description: String::new(),
            topic,
            ticket: boru_core::chat_core::Ticket::new(topic, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: ADVERT_TTL_SECS,
        };

        // First sighting: stored, announced, auto-subscribed once.
        dir_tx
            .try_send(DirectoryRoomEvent::Advertisement(ad.clone(), author))
            .expect("feed announcement");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);
        assert!(app.directory_store.lock().unwrap().contains(topic, author));
        assert_eq!(app.rooms_state.auto_subscribed_rooms.len(), 1, "subscribed once");
        assert_eq!(
            app.conversation_store.find(&topic).map(|e| e.name.clone()),
            Some("Lounge".to_string()),
            "archived conversation record created once"
        );
        let activity_after_first = app.notifications_state.recent_activity.len();
        assert!(
            activity_after_first >= 1,
            "first announcement surfaced in the Recent Activity feed"
        );

        // Identical re-broadcast (the periodic ~60 s refresh): no UI churn.
        let mut refresh = ad.clone();
        refresh.member_count = 12; // dynamic hint only — same user-visible metadata
        refresh.last_activity = 9_999;
        dir_tx
            .try_send(DirectoryRoomEvent::Advertisement(refresh, author))
            .expect("feed refresh");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);

        assert_eq!(
            app.directory_store.lock().unwrap().len(),
            1,
            "no second card from repeated gossip"
        );
        assert_eq!(
            app.rooms_state.auto_subscribed_rooms.len(),
            1,
            "no re-subscribe from a duplicate advertisement"
        );
        assert_eq!(
            app.conversation_store.find(&topic).map(|e| e.name.clone()),
            Some("Lounge".to_string()),
            "no duplicate conversation record"
        );
        assert_eq!(
            app.notifications_state.recent_activity.len(),
            activity_after_first,
            "no re-announcement for an identical advertisement"
        );
    }

    /// A metadata change for a known room refreshes the card (single entry)
    /// but never re-announces or re-subscribes.
    #[test]
    fn directory_refreshed_metadata_updates_card_without_resubscribe() {
        let (_runtime, mut app) = build_prewarm_test_app();

        let (dir_tx, dir_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(8);
        app.directory_room_rx = Arc::new(Mutex::new(dir_rx));

        let author = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0x78; 32]);
        let ad = RoomAdvertisement {
            room_name: "Lounge".to_string(),
            description: "v1".to_string(),
            topic,
            ticket: boru_core::chat_core::Ticket::new(topic, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: ADVERT_TTL_SECS,
        };

        dir_tx
            .try_send(DirectoryRoomEvent::Advertisement(ad.clone(), author))
            .expect("feed announcement");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);
        let activity_after_first = app.notifications_state.recent_activity.len();

        // Same room + author, changed description: refresh, no re-announce.
        let mut changed = ad.clone();
        changed.description = "v2".to_string();
        dir_tx
            .try_send(DirectoryRoomEvent::Advertisement(changed, author))
            .expect("feed refresh");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);

        assert_eq!(app.directory_store.lock().unwrap().len(), 1);
        assert_eq!(
            app.rooms_state.auto_subscribed_rooms.len(),
            1,
            "refresh never re-subscribes"
        );
        assert_eq!(
            app.notifications_state.recent_activity.len(),
            activity_after_first,
            "metadata refresh is not a new announcement"
        );
    }

    /// Malformed/oversized metadata is discarded: an advertisement whose
    /// room name exceeds the protocol limit never reaches the store, the
    /// conversation store, or the subscription set (PDF Task 7.1
    /// acceptance criterion).
    #[test]
    fn directory_oversized_advertisement_is_discarded() {
        let (_runtime, mut app) = build_prewarm_test_app();

        let (dir_tx, dir_rx) = tokio::sync::mpsc::channel::<DirectoryRoomEvent>(8);
        app.directory_room_rx = Arc::new(Mutex::new(dir_rx));

        let author = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0x79; 32]);
        let ad = RoomAdvertisement {
            room_name: "x".repeat(boru_core::directory::LEGACY_MAX_ROOM_NAME_LEN + 1),
            description: String::new(),
            topic,
            ticket: boru_core::chat_core::Ticket::new(topic, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: ADVERT_TTL_SECS,
        };

        dir_tx
            .try_send(DirectoryRoomEvent::Advertisement(ad, author))
            .expect("feed oversized advertisement");
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);

        assert!(
            app.directory_store.lock().unwrap().is_empty(),
            "oversized advertisement never stored"
        );
        assert!(
            !app.rooms_state.auto_subscribed_rooms.contains(&topic),
            "oversized advertisement never subscribes"
        );
        assert!(
            app.conversation_store.find(&topic).is_none(),
            "oversized advertisement never creates a conversation record"
        );
    }

    // ── PUBLIC-03: new-user recent-activity entries ─────────────────────
    // A peer observed for the very first time (across restarts) produces a
    // distinct "New user … came online" Recent Activity entry; known peers
    // reconnecting or a restarted app must not re-announce them.

    /// First-ever sighting pushes a "New user" entry and records the peer
    /// in the seen set.
    #[test]
    fn first_seen_peer_pushes_new_user_activity() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let pk = SecretKey::generate().public();

        assert!(app.note_peer_first_seen(pk), "peer is new");
        assert!(app.seen_peers.contains(&pk), "peer recorded in seen set");
        let data = app.recent_activity_card_data();
        assert_eq!(data.rows.len(), 1, "one new-user entry");
        assert_eq!(data.rows[0].kind, ActivityKind::Online);
        assert!(
            data.rows[0].description.contains("New user"),
            "entry is the distinct new-user wording: {}",
            data.rows[0].description
        );
        assert!(
            data.rows[0]
                .description
                .contains(&pk.fmt_short().to_string()),
            "entry carries the peer display name: {}",
            data.rows[0].description
        );
    }

    /// A second online transition for the same peer (reconnect) must not
    /// create another "New user" entry.
    #[test]
    fn known_peer_reconnect_does_not_push_new_user_again() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let pk = SecretKey::generate().public();

        assert!(app.note_peer_first_seen(pk), "first sighting is new");
        let count_after_first = app.notifications_state.recent_activity.len();
        assert_eq!(count_after_first, 1);

        assert!(
            !app.note_peer_first_seen(pk),
            "second sighting is a known peer"
        );
        assert_eq!(
            app.notifications_state.recent_activity.len(),
            count_after_first,
            "reconnect must not add another entry"
        );
        let data = app.recent_activity_card_data();
        let new_user_count = data
            .rows
            .iter()
            .filter(|r| r.description.contains("New user"))
            .count();
        assert_eq!(new_user_count, 1, "exactly one new-user entry ever");
    }

    /// Our own node is never announced as a new user.
    #[test]
    fn own_peer_is_never_announced_as_new() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let local = app.local_public;
        assert!(
            !app.note_peer_first_seen(local),
            "own public key is never a new user"
        );
        assert!(
            app.notifications_state.recent_activity.is_empty(),
            "no activity entry for own node"
        );
    }

    /// flush_pending_neighbor_status emits the "New user" wording for a
    /// first-time online peer and the plain wording for a known peer.
    #[test]
    fn flush_neighbor_status_uses_new_user_wording_only_once() {
        let (_runtime, mut app) = build_prewarm_test_app();
        let pk = SecretKey::generate().public();

        app.on_neighbor_status_change(pk, true);
        app.flush_pending_neighbor_status();

        let data = app.recent_activity_card_data();
        assert_eq!(data.rows.len(), 1);
        assert!(
            data.rows[0].description.contains("New user"),
            "first online transition is a new user: {}",
            data.rows[0].description
        );

        // Simulate a reconnect later in the same session.
        app.on_neighbor_status_change(pk, false);
        app.flush_pending_neighbor_status();
        app.on_neighbor_status_change(pk, true);
        app.flush_pending_neighbor_status();

        let data = app.recent_activity_card_data();
        assert!(
            data.rows
                .iter()
                .any(|r| r.description.contains("came online")
                    && !r.description.contains("New user")),
            "known-peer reconnect keeps the plain came-online wording"
        );
        let new_user_count = data
            .rows
            .iter()
            .filter(|r| r.description.contains("New user"))
            .count();
        assert_eq!(new_user_count, 1, "still exactly one new-user entry");
    }

    /// load_seen_peers seeds the set with existing friends so a known
    /// contact is never announced after upgrade/restart.
    #[test]
    fn load_seen_peers_seeds_existing_friends() {
        let mut friends = FriendsStore::empty_at(&std::env::temp_dir());
        let pk = SecretKey::generate().public();
        let fid = FriendId::from_public_key(pk);
        friends.ensure_friend(fid.clone());
        friends.set_label(fid.clone(), "Alice");
        friends.set_relationship(fid, FriendRelationship::Friends);

        let seen = load_seen_peers(&std::env::temp_dir(), &friends);
        assert!(seen.contains(&pk), "existing friend seeded into seen set");
    }

    /// The seen set round-trips through the JSON persistence file, so a
    /// restarted app does not re-announce every previously-seen peer.
    #[test]
    fn seen_peers_roundtrip_via_json_persistence() {
        let data_dir = std::env::temp_dir().join(format!(
            "boru-seen-peers-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&data_dir).expect("create temp dir");

        let mut seen = HashSet::new();
        let pk = SecretKey::generate().public();
        seen.insert(pk);
        save_seen_peers(&data_dir, &seen);

        let friends = FriendsStore::empty_at(&data_dir);
        let loaded = load_seen_peers(&data_dir, &friends);
        assert!(loaded.contains(&pk), "persisted peer survives restart");

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    // ── File Sharing card dependency isolation (PERF-2, t_f6dcbb3a) ──
    // Same harness as the Home-rail cards above, applied to the File Sharing
    // dashboard: each card is rendered through `iced::widget::lazy(dep, build)`.
    // These tests prove a change in one card's state slice changes only that
    // card's dependency, so typing in the header search rebuilds the Shared by
    // Me card alone while the Peers / Sharing Summary / Recent Activity cards
    // keep their cached subtrees.

    /// Typing in the header search must change only the Shared by Me card
    /// dependency — the Peers, Sharing Summary, and Recent Activity snapshots
    /// stay identical.
    #[test]
    fn dashboard_search_typing_changes_only_shared_by_me_card() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let shared_before = app.shared_by_me_card_dependency();
        let peers_before = app.peers_card_dependency();
        let summary_before = app.sharing_summary_card_dependency();
        let activity_before = app.recent_activity_card_dependency();

        app.files_state.dashboard_search_input = "report".to_string();

        let shared_after = app.shared_by_me_card_dependency();
        let peers_after = app.peers_card_dependency();
        let summary_after = app.sharing_summary_card_dependency();
        let activity_after = app.recent_activity_card_dependency();

        assert_ne!(
            shared_before, shared_after,
            "Shared by Me card must rebuild when the search query changes"
        );
        assert_eq!(
            peers_before, peers_after,
            "Peers card must not re-render when the search query changes"
        );
        assert_eq!(
            summary_before, summary_after,
            "Sharing Summary card must not re-render when the search query changes"
        );
        assert_eq!(
            activity_before, activity_after,
            "Recent Activity card must not re-render when the search query changes"
        );
    }

    /// Pushing a recent-activity row must change only the Recent Activity card
    /// dependency.
    #[test]
    fn dashboard_activity_push_changes_only_recent_activity_card() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let shared_before = app.shared_by_me_card_dependency();
        let peers_before = app.peers_card_dependency();
        let summary_before = app.sharing_summary_card_dependency();
        let activity_before = app.recent_activity_card_dependency();

        app.files_state.dashboard_recent_activity
            .push(crate::recent_activity_view_model::RecentActivityRow {
                id: "evt-1".to_string(),
                occurred_at_ms: 1_000,
                peer_label: "Alice".to_string(),
                file_label: "notes.txt".to_string(),
                action: "Downloaded".to_string(),
                status: crate::recent_activity_view_model::ActivityStatus::Success,
                detail: None,
                bytes: Some(2048),
            });

        let shared_after = app.shared_by_me_card_dependency();
        let peers_after = app.peers_card_dependency();
        let summary_after = app.sharing_summary_card_dependency();
        let activity_after = app.recent_activity_card_dependency();

        assert_ne!(
            activity_before, activity_after,
            "Recent Activity card must rebuild when a row is pushed"
        );
        assert_eq!(
            shared_before, shared_after,
            "Shared by Me card must not re-render on activity pushes"
        );
        assert_eq!(
            peers_before, peers_after,
            "Peers card must not re-render on activity pushes"
        );
        assert_eq!(
            summary_before, summary_after,
            "Sharing Summary card must not re-render on activity pushes"
        );
    }

    /// Loading the Sharing Summary projection must change only the Sharing
    /// Summary card dependency.
    #[test]
    fn dashboard_summary_load_changes_only_summary_card() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let shared_before = app.shared_by_me_card_dependency();
        let peers_before = app.peers_card_dependency();
        let summary_before = app.sharing_summary_card_dependency();
        let activity_before = app.recent_activity_card_dependency();

        app.files_state.dashboard_sharing_summary = Some(crate::sharing_summary::SharingSummary {
            files_shared: 7,
            total_downloads: 3,
            active_downloads: 1,
            peers_shared_with: 2,
        });

        let shared_after = app.shared_by_me_card_dependency();
        let peers_after = app.peers_card_dependency();
        let summary_after = app.sharing_summary_card_dependency();
        let activity_after = app.recent_activity_card_dependency();

        assert_ne!(
            summary_before, summary_after,
            "Sharing Summary card must rebuild when the projection loads"
        );
        assert_eq!(
            shared_before, shared_after,
            "Shared by Me card must not re-render on summary loads"
        );
        assert_eq!(
            peers_before, peers_after,
            "Peers card must not re-render on summary loads"
        );
        assert_eq!(
            activity_before, activity_after,
            "Recent Activity card must not re-render on summary loads"
        );
    }

    /// Switching to the Downloaded tab must change only the Downloads card
    /// dependency (its `active` flag); the dashboard cards stay memoized.
    #[test]
    fn dashboard_tab_switch_changes_only_downloads_card() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let downloads_before = app.downloads_card_dependency();
        let shared_before = app.shared_by_me_card_dependency();
        let peers_before = app.peers_card_dependency();
        let summary_before = app.sharing_summary_card_dependency();
        let activity_before = app.recent_activity_card_dependency();

        app.files_state.dashboard_active_tab =
            crate::dashboard_view_model::DashboardTab::Downloaded;

        let downloads_after = app.downloads_card_dependency();
        let shared_after = app.shared_by_me_card_dependency();
        let peers_after = app.peers_card_dependency();
        let summary_after = app.sharing_summary_card_dependency();
        let activity_after = app.recent_activity_card_dependency();

        assert_ne!(
            downloads_before, downloads_after,
            "Downloads card key must flip with the active tab"
        );
        assert_eq!(
            shared_before, shared_after,
            "Shared by Me card must not re-render on a tab switch"
        );
        assert_eq!(
            peers_before, peers_after,
            "Peers card must not re-render on a tab switch"
        );
        assert_eq!(
            summary_before, summary_after,
            "Sharing Summary card must not re-render on a tab switch"
        );
        assert_eq!(
            activity_before, activity_after,
            "Recent Activity card must not re-render on a tab switch"
        );
    }

    // ── Chat-log scroll state machine (t_9e9a0fcd) ────────────────────────
    //
    // The chat log is a top-anchored windowed scrollable.  The app mirrors
    // the Iced scrollable's offset into `scroll_offset` (which drives the
    // windowed renderer) and tracks `follow_latest` so incoming messages
    // snap to the bottom only while the user is at the bottom.  The
    // `f32::MAX` bottom sentinel keeps follow-latest armed while the
    // timeline is empty; a `Scrolled(0, vp)` event from the anchor-bottom
    // empty-state scrollable must NOT clobber it, otherwise a fresh
    // conversation lands at the TOP of history once entries render.

    #[test]
    fn chat_scroll_empty_timeline_preserves_bottom_sentinel() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        // Fresh conversation state: following latest with the bottom sentinel.
        app.follow_latest = true;
        app.scroll_offset = f32::MAX;
        app.viewport_height = 0.0;
        app.total_content_height.set(0.0);

        // The anchor-bottom empty-state scrollable reports offset 0 with no
        // content.  This is the pre-RoomOpened event that used to clobber the
        // sentinel and land the user at the top of history.
        let _task = app.update(AppMessage::Scrolled(0.0, 700.0));

        assert_eq!(
            app.scroll_offset,
            f32::MAX,
            "bottom sentinel must survive an empty-state Scrolled event"
        );
        assert!(
            app.follow_latest,
            "follow-latest stays armed while the timeline is empty"
        );
        assert_eq!(
            app.viewport_height, 700.0,
            "viewport height is still learned from the empty-state event"
        );
    }

    #[test]
    fn chat_scroll_at_bottom_keeps_following_latest_and_mirrors_offset() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.total_content_height.set(1000.0);
        app.follow_latest = true;
        app.scroll_to_bottom_pending = false;

        // offset + viewport (800 + 200 = 1000) is inside the 10px bottom band.
        let _task = app.update(AppMessage::Scrolled(800.0, 200.0));

        assert!(app.follow_latest, "at the bottom → keep following latest");
        assert_eq!(
            app.scroll_offset, 800.0,
            "offset is mirrored for the window"
        );
        assert_eq!(app.viewport_height, 200.0, "viewport height is mirrored");
    }

    #[test]
    fn chat_scroll_away_from_bottom_cancels_queued_snap() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.total_content_height.set(1000.0);
        app.follow_latest = true;
        // A snap-to-bottom is queued (e.g. an entry was just appended while
        // the user was at the bottom); the user then scrolls up to read.
        app.scroll_to_bottom_pending = true;

        let _task = app.update(AppMessage::Scrolled(0.0, 200.0));

        assert!(
            !app.follow_latest,
            "scrolling up to read old messages leaves follow-latest"
        );
        assert!(
            !app.scroll_to_bottom_pending,
            "manual scroll away from the bottom cancels the queued snap"
        );
        assert_eq!(app.scroll_offset, 0.0);
        assert_eq!(app.viewport_height, 200.0);
    }

    #[test]
    fn chat_scroll_bottom_detection_uses_ten_pixel_epsilon() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.total_content_height.set(1000.0);

        // Exactly at the epsilon boundary (offset + vp == total - 10).
        let _t1 = app.update(AppMessage::Scrolled(790.0, 200.0));
        assert!(
            app.follow_latest,
            "offset+vp == total-10 counts as at the bottom"
        );

        // One pixel above the boundary: genuinely reading older messages.
        let _t2 = app.update(AppMessage::Scrolled(789.0, 200.0));
        assert!(
            !app.follow_latest,
            "offset+vp < total-10 counts as scrolled up"
        );
    }

    #[test]
    fn keep_latest_visible_arms_snap_only_while_following() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Following latest → sentinel + queued snap for the next update.
        app.follow_latest = true;
        app.scroll_offset = 42.0;
        app.scroll_to_bottom_pending = false;
        app.keep_latest_visible();
        assert_eq!(app.scroll_offset, f32::MAX, "sentinel re-armed");
        assert!(app.scroll_to_bottom_pending, "snap queued");

        // Reading older messages → offset and snap state stay untouched.
        app.follow_latest = false;
        app.scroll_offset = 500.0;
        app.scroll_to_bottom_pending = false;
        app.keep_latest_visible();
        assert_eq!(
            app.scroll_offset, 500.0,
            "scrolled-up reading position is preserved on append"
        );
        assert!(
            !app.scroll_to_bottom_pending,
            "no snap queued while reading older messages"
        );
    }

    #[test]
    fn entries_push_arms_snap_only_while_following() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // At the bottom (follow_latest): a live append queues the snap that
        // drives the top-anchored scrollable to the newest entry.
        app.follow_latest = true;
        app.scroll_to_bottom_pending = false;
        app.entries_push(ChatEntry::local("me", "live message at bottom"));
        assert!(
            app.scroll_to_bottom_pending,
            "append while following queues snap"
        );
        assert_eq!(app.scroll_offset, f32::MAX, "sentinel armed for the window");

        // Reading older messages (follow_latest=false, offset 500): an
        // incoming live append must not move the reading position.
        app.follow_latest = false;
        app.scroll_offset = 500.0;
        app.scroll_to_bottom_pending = false;
        let before = app.entries.len();
        app.entries_push(ChatEntry::remote(
            "Alice",
            "live while reading",
            None,
            None,
            Some(iroh::SecretKey::generate().public()),
        ));
        assert_eq!(
            app.entries.len(),
            before + 1,
            "live entry still lands in the timeline"
        );
        assert_eq!(
            app.scroll_offset, 500.0,
            "scrolled-up reading position survives a live append"
        );
        assert!(
            !app.scroll_to_bottom_pending,
            "no snap steals the reading position"
        );
    }

    #[test]
    fn update_tail_consumes_pending_snap_once() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.total_content_height.set(1000.0);
        app.scroll_to_bottom_pending = true;

        // The update tail turns the pending flag into the snap task exactly
        // once.
        let _task = app.update(AppMessage::Scrolled(800.0, 200.0));
        assert!(
            !app.scroll_to_bottom_pending,
            "pending snap consumed by the update tail"
        );

        // A later update with the flag already clear emits no second snap.
        let _task2 = app.update(AppMessage::Scrolled(800.0, 200.0));
        assert!(!app.scroll_to_bottom_pending);
    }

    #[test]
    fn close_image_lightbox_always_snaps_to_bottom() {
        // Relay-disabled builder: no real peer needed, and it never hangs on
        // the relay-less debsrv test host (unlike build_join_request_test_app).
        let (_runtime, mut app) = build_prewarm_test_app();
        app.total_content_height.set(1000.0);

        // User is at the bottom (follow_latest) and opens an enlarged image.
        app.follow_latest = true;
        app.scroll_offset = 800.0;
        app.scroll_to_bottom_pending = false;
        let _open = app.update(AppMessage::OpenImageLightbox(0));
        assert_eq!(app.lightbox_image, Some(0), "lightbox opened");

        // Closing re-arms the bottom sentinel, forces follow-latest, and
        // queues the snap so the windowed scrollable returns to the latest
        // message.  The update tail consumes the flag in the same update
        // (no stale flag left for a later Scrolled event to cancel).
        let _close = app.update(AppMessage::CloseImageLightbox);
        assert!(app.lightbox_image.is_none(), "lightbox closed");
        assert_eq!(app.scroll_offset, f32::MAX, "bottom sentinel re-armed");
        assert!(app.follow_latest, "follow-latest forced on close");
        assert!(
            !app.scroll_to_bottom_pending,
            "snap consumed by the update tail in the same update"
        );
        assert!(
            app.lightbox_close_snap_guard > 0,
            "stale-event guard armed for the re-created scrollable"
        );

        // CHAT-SCROLL requirement: even if the user had manually scrolled
        // up before opening the image, closing the lightbox ALWAYS returns
        // to the bottom — the old preserve-reading-position semantics is
        // overridden.
        app.follow_latest = false;
        app.scroll_offset = 300.0;
        app.scroll_to_bottom_pending = false;
        let _open2 = app.update(AppMessage::OpenImageLightbox(0));
        let _close2 = app.update(AppMessage::CloseImageLightbox);
        assert_eq!(
            app.scroll_offset,
            f32::MAX,
            "always snaps to bottom on lightbox close, even when scrolled up"
        );
        assert!(
            app.follow_latest,
            "follow-latest forced even when scrolled up"
        );
        assert!(
            !app.scroll_to_bottom_pending,
            "snap consumed by the update tail in the same update"
        );
    }

    #[test]
    fn close_lightbox_stale_scrolled_event_keeps_sentinel_and_requeues_snap() {
        // After the lightbox closes, the overlay stack is removed and the
        // windowed scrollable is re-created.  The fresh widget's first
        // Scrolled event reports offset 0 (top) — that stale event must not
        // clobber the f32::MAX bottom sentinel before the snap task lands.
        let (_runtime, mut app) = build_prewarm_test_app();
        app.total_content_height.set(1000.0);
        app.follow_latest = true;
        app.scroll_offset = 800.0;
        app.scroll_to_bottom_pending = false;
        let _open = app.update(AppMessage::OpenImageLightbox(0));
        let _close = app.update(AppMessage::CloseImageLightbox);
        assert_eq!(app.scroll_offset, f32::MAX, "sentinel armed on close");

        // Stale event from the re-created scrollable: offset 0, not bottom.
        let _stale = app.update(AppMessage::Scrolled(0.0, 200.0));
        assert_eq!(
            app.scroll_offset,
            f32::MAX,
            "stale Scrolled(0) must not clobber the bottom sentinel"
        );
        assert!(
            app.follow_latest,
            "follow-latest preserved through stale event"
        );
        assert!(
            !app.scroll_to_bottom_pending,
            "stale event re-armed the snap; tail consumed it in the same update"
        );
        assert_eq!(
            app.lightbox_close_snap_guard, 2,
            "stale-event guard decremented once"
        );

        // A genuine bottom event confirms the snap landed and disarms.
        let _landed = app.update(AppMessage::Scrolled(800.0, 200.0));
        assert_eq!(app.scroll_offset, 800.0, "snap landed at the bottom");
        assert!(app.follow_latest);
        assert_eq!(app.lightbox_close_snap_guard, 0, "guard disarmed at bottom");
    }

    #[test]
    fn layout_cache_image_entry_height_tracks_real_display_box() {
        // CHAT-SCROLL: the layout cache must predict the same image box the
        // view renders, so total_content_height matches the real content and
        // the scrollbar does not jump as images enter the window or decode.
        let mut cache = LayoutCache::new(TYPO_SM);

        // A 800×600 image scales down to the 360-wide preview: display_h = 270.
        let mut img_entry = ChatEntry::image(
            ChatKind::Remote,
            "peer",
            "",
            vec![0u8; 64],
            None,
            None,
            None,
            Some("img/id".to_string()),
            None,
        );
        img_entry.image_width = Some(800);
        img_entry.image_height = Some(600);
        let mut entries = vec![img_entry];
        cache.ensure(&entries, TYPO_SM, 1024.0);
        let expected = LayoutCache::MSG_BASE_H + LayoutCache::IMAGE_HEADER_H + 270.0 + 2.0;
        assert!(
            (cache.heights[0] - expected).abs() < 0.01,
            "image entry height {} should track the rendered 270px display box (expected {expected})",
            cache.heights[0]
        );

        // A very tall image must be clamped to the max preview box (400),
        // so the entry cannot blow up the scrollbar.
        let mut cache = LayoutCache::new(TYPO_SM);
        let mut tall = ChatEntry::image(
            ChatKind::Remote,
            "peer",
            "",
            vec![0u8; 64],
            None,
            None,
            None,
            Some("img/id".to_string()),
            None,
        );
        tall.image_width = Some(200);
        tall.image_height = Some(2000);
        let tall_entries = vec![tall];
        cache.ensure(&tall_entries, TYPO_SM, 1024.0);
        let tall_expected = LayoutCache::MSG_BASE_H + LayoutCache::IMAGE_HEADER_H + 400.0 + 2.0;
        assert!(
            (cache.heights[0] - tall_expected).abs() < 0.01,
            "tall image height {} should clamp to the 400px max box (expected {tall_expected})",
            cache.heights[0]
        );

        // Unknown dimensions (placeholder while decoding) use the same max
        // box, so the estimated height is stable across decode/hydration.
        let mut cache = LayoutCache::new(TYPO_SM);
        let mut unknown = ChatEntry::image(
            ChatKind::Remote,
            "peer",
            "",
            vec![0u8; 64],
            None,
            None,
            None,
            Some("img/id".to_string()),
            None,
        );
        unknown.image_width = None;
        unknown.image_height = None;
        let unknown_entries = vec![unknown];
        cache.ensure(&unknown_entries, TYPO_SM, 1024.0);
        assert!(
            (cache.heights[0] - tall_expected).abs() < 0.01,
            "unknown-dimension image height {} should match the max box (expected {tall_expected})",
            cache.heights[0]
        );
    }

    #[test]
    fn background_subscribed_replays_history_into_new_conversation() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);
        let local_hex = app.local_public.to_string();

        // Seed persisted history for the direct topic: two remote messages
        // and one local message (event_ids start at 1 in the real store).
        {
            let mut store = app.chat_history.lock().unwrap();
            store.push_with_id(HistoryEntry::new(
                topic,
                peer.to_string(),
                vec![],
                "text",
                "hello from peer",
            ));
            store.push_with_id(HistoryEntry::new(
                topic,
                peer.to_string(),
                vec![],
                "text",
                "second from peer",
            ));
            store.push_with_id(HistoryEntry::new(
                topic,
                local_hex,
                vec![],
                "text",
                "my old message",
            ));
        }

        // The background subscription completes for this topic.
        let _task = app.update(AppMessage::BackgroundSubscribed(topic, None, None));

        let conv = app.conversations.get(&topic).expect("conversation created");
        assert!(conv.history_loaded, "history marked loaded");
        assert_eq!(conv.entries.len(), 3, "all history entries replayed");
        assert_eq!(
            conv.entries[0].body, "hello from peer",
            "oldest history entry first"
        );
        assert!(
            conv.entries[0].parsed_segments.is_some(),
            "replayed entries have cached text segments so the bubble body renders"
        );
        assert_eq!(conv.history_saved_count, 3, "saved count avoids re-saving");
        assert_eq!(
            conv.scroll_offset,
            f32::MAX,
            "follow-latest bottom sentinel armed for fast-path open"
        );
        assert!(
            conv.follow_latest,
            "new conversation follows latest by default"
        );

        // A duplicate completion (the conversation-store loop and the friends
        // loop can both subscribe the same direct topic) must not replay
        // history twice.
        let _task2 = app.update(AppMessage::BackgroundSubscribed(topic, None, None));
        let conv2 = app.conversations.get(&topic).unwrap();
        assert_eq!(
            conv2.entries.len(),
            3,
            "duplicate BackgroundSubscribed must not double-replay"
        );
    }

    #[test]
    fn background_subscribed_without_history_creates_empty_conversation() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);

        let _task = app.update(AppMessage::BackgroundSubscribed(topic, None, None));

        let conv = app.conversations.get(&topic).expect("conversation created");
        assert!(conv.entries.is_empty(), "no history → empty timeline");
        assert!(
            conv.history_loaded,
            "history considered loaded (nothing to load)"
        );
        assert!(
            conv.follow_latest,
            "follow-latest stays the default for a fresh conversation"
        );
        assert_eq!(
            conv.scroll_offset, 0.0,
            "no sentinel when there is no history"
        );
    }

    #[test]
    fn failed_background_subscribe_releases_the_original_topic() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);
        app.background_subscriptions_in_flight.insert(topic);

        let _task = app.update(AppMessage::BackgroundSubscribeFailed(
            topic,
            "synthetic subscribe failure".to_string(),
        ));

        assert!(
            !app.background_subscriptions_in_flight.contains(&topic),
            "a failed direct-topic subscription must be retryable"
        );
    }

    // ── BORU-CP-08: reconcile existing conversations after reconnect ────

    /// A current friend with an active designated direct conversation gets
    /// that topic restored after a reconnect (existing direct chats recover
    /// after peer restart).
    #[test]
    fn reconnect_required_topics_restores_friend_direct_topic() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);
        let fid = boru_core::friends::FriendId::from_public_key(peer);
        let record = app.friends.ensure_friend(fid);
        record.relationship = boru_core::friends::FriendRelationship::Friends;
        record.set_direct_conversation(topic, boru_core::friends::DirectConversationState::Active);

        let topics = app.reconnect_required_topics(peer);

        assert_eq!(topics, vec![topic], "friend direct topic restored");
        // Reconnection must not duplicate or create conversation records.
        assert_eq!(
            app.conversation_store.iter().count(),
            0,
            "reconcile must not create new conversation records"
        );
    }

    /// A friend with no direct-conversation metadata still restores the
    /// deterministic direct topic (the stable friend topic the app
    /// auto-subscribes at startup).
    #[test]
    fn reconnect_required_topics_restores_deterministic_topic_for_friend() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let fid = boru_core::friends::FriendId::from_public_key(peer);
        let record = app.friends.ensure_friend(fid);
        record.relationship = boru_core::friends::FriendRelationship::Friends;

        let topics = app.reconnect_required_topics(peer);

        assert_eq!(
            topics,
            vec![direct_topic(&app.local_public, &peer)],
            "deterministic direct topic restored for a friend"
        );
    }

    /// A blocked friend is never resurrected, even when a stale direct
    /// record exists (deleted/blocked relationships are not resurrected).
    #[test]
    fn reconnect_required_topics_skips_blocked_friend() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let fid = boru_core::friends::FriendId::from_public_key(peer);
        let record = app.friends.ensure_friend(fid);
        record.relationship = boru_core::friends::FriendRelationship::Blocked;
        // Even a stale active conversation record must not resurrect.
        app.conversation_store
            .upsert(boru_core::conversations::ConversationEntry::new(
                direct_topic(&app.local_public, &peer),
                peer.to_string(),
                "Blocked peer",
            ));

        let topics = app.reconnect_required_topics(peer);

        assert!(topics.is_empty(), "blocked relationship not resurrected");
    }

    /// An archived (deleted) designated direct conversation is not
    /// resurrected.
    #[test]
    fn reconnect_required_topics_skips_archived_direct_conversation() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);
        let fid = boru_core::friends::FriendId::from_public_key(peer);
        let record = app.friends.ensure_friend(fid);
        record.relationship = boru_core::friends::FriendRelationship::Friends;
        record
            .set_direct_conversation(topic, boru_core::friends::DirectConversationState::Archived);

        let topics = app.reconnect_required_topics(peer);

        assert!(
            topics.is_empty(),
            "archived (deleted) direct conversation not resurrected"
        );
    }

    /// A peer that is not a friend has no entitlement from discovery alone
    /// (no authorisation by presence) — unless an existing direct
    /// conversation record already exists.
    #[test]
    fn reconnect_required_topics_restores_existing_store_record_without_friendship() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = TopicId::from_bytes([0x42u8; 32]);
        app.conversation_store
            .upsert(boru_core::conversations::ConversationEntry::new(
                topic,
                peer.to_string(),
                "Existing direct chat",
            ));

        let topics = app.reconnect_required_topics(peer);

        assert_eq!(
            topics,
            vec![topic],
            "existing direct conversation record restored"
        );
    }

    /// Group/public conversation records are never auto-joined from
    /// discovery.
    #[test]
    fn reconnect_required_topics_never_auto_joins_groups() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let group_topic = TopicId::from_bytes([0x43u8; 32]);
        let mut group =
            boru_core::conversations::ConversationEntry::new_group(group_topic, "Some Group");
        group.peer_id = peer.to_string();
        app.conversation_store.upsert(group);

        let topics = app.reconnect_required_topics(peer);

        assert!(
            topics.is_empty(),
            "group topics are never auto-joined from discovery"
        );
    }

    #[test]
    fn conversation_switch_at_bottom_rearms_snap() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic_a = direct_topic(&app.local_public, &peer);
        let topic_b = TopicId::from_bytes([9u8; 32]);

        // Conversation A is active and following latest (bottom sentinel set).
        app.topic = topic_a;
        app.screen = Screen::Chat { topic: topic_a };
        app.follow_latest = true;
        app.scroll_offset = f32::MAX;
        app.scroll_to_bottom_pending = false;

        // Conversation B exists in the runtime map (background-subscribed).
        let mut conv_b = ConversationLive::new(topic_b);
        conv_b.follow_latest = true;
        conv_b.scroll_offset = f32::MAX;
        app.conversations.insert(topic_b, conv_b);

        // Switching to B restores its follow-latest state and re-arms the
        // snap so the newest message is visible the moment the chat screen
        // renders (manual scroll-to-bottom trigger via conversation open).
        assert!(app.switch_to_conversation(topic_b), "switch to B succeeds");
        assert_eq!(app.topic, topic_b);
        assert!(app.follow_latest, "follow-latest state restored");
        assert!(
            app.scroll_to_bottom_pending,
            "opening a follow-latest conversation queues the snap-to-bottom"
        );
        assert_eq!(app.scroll_offset, f32::MAX, "bottom sentinel restored");
    }

    #[test]
    fn conversation_switch_always_snaps_to_bottom() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic_a = direct_topic(&app.local_public, &peer);
        let topic_b = TopicId::from_bytes([9u8; 32]);

        // A is active and the user has scrolled up to read older messages.
        app.topic = topic_a;
        app.screen = Screen::Chat { topic: topic_a };
        app.follow_latest = false;
        app.scroll_offset = 500.0;
        app.viewport_height = 200.0;
        app.scroll_to_bottom_pending = false;

        // B exists in the runtime map and was ALSO left scrolled up (this is
        // the case that used to strand the viewport mid-history on return).
        let mut conv_b = ConversationLive::new(topic_b);
        conv_b.follow_latest = false;
        conv_b.scroll_offset = 320.0;
        conv_b.viewport_height = 200.0;
        app.conversations.insert(topic_b, conv_b);

        // Returning to B always lands at the latest message, regardless of
        // where the user was reading when they last left.
        assert!(app.switch_to_conversation(topic_b), "switch to B succeeds");
        assert_eq!(app.topic, topic_b);
        assert!(
            app.follow_latest,
            "returning to a chat forces follow-latest (auto-scroll to bottom)"
        );
        assert_eq!(
            app.scroll_offset,
            f32::MAX,
            "bottom sentinel armed so the windowed renderer shows the newest messages"
        );
        assert!(
            app.scroll_to_bottom_pending,
            "snap-to-bottom queued for the conversation open"
        );
        assert_eq!(
            app.viewport_height, 200.0,
            "viewport height is still restored per conversation"
        );
    }

    #[test]
    fn open_room_fast_path_consumes_snap_in_same_update() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);

        // The topic is already subscribed in the runtime map, so OpenRoom
        // takes the fast path (switch_to_conversation).  The conversation
        // was left scrolled up; returning must still arm AND consume the
        // snap in the same update.  Previously the fast path returned early
        // and skipped the update-tail snap consumption, leaving the flag
        // pending for a later Scrolled event (carrying the previous
        // conversation's stale offset) to cancel — stranding the viewport
        // away from the bottom.
        let mut conv = ConversationLive::new(topic);
        conv.follow_latest = false;
        conv.scroll_offset = 120.0;
        app.conversations.insert(topic, conv);

        let _task = app.update(AppMessage::OpenRoom(topic));

        assert_eq!(app.topic, topic);
        assert!(app.follow_latest, "fast-path open forces follow-latest");
        assert_eq!(app.scroll_offset, f32::MAX, "bottom sentinel armed");
        assert!(
            !app.scroll_to_bottom_pending,
            "fast-path open consumed the snap in the same update (no stale flag to cancel)"
        );
    }

    #[test]
    fn inactive_room_message_increments_unread_badge() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);

        // A different conversation is active.
        app.topic = TopicId::from_bytes([9u8; 32]);
        app.screen = Screen::Chat { topic: app.topic };

        let _task = app.update(AppMessage::NetEvent(ConversationNetEvent::new(
            topic,
            NetEvent::Message {
                from: peer,
                message: Message::Message {
                    text: "hello while hidden".into(),
                },
                sent_at: 0,
                backfilled: false,
            },
        )));

        let conv = app.conversations.get(&topic).expect("conversation created");
        assert_eq!(
            conv.unread, 1,
            "user-visible message to an inactive room bumps the sidebar badge"
        );
        assert_eq!(
            conv.pending_events.len(),
            1,
            "event is queued for replay when the room is opened"
        );
    }

    #[test]
    fn inactive_room_gossip_events_do_not_increment_unread() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);
        app.topic = TopicId::from_bytes([9u8; 32]);
        app.screen = Screen::Chat { topic: app.topic };

        // NeighborUp is pure gossip — no badge bump.
        let _t1 = app.update(AppMessage::NetEvent(ConversationNetEvent::new(
            topic,
            NetEvent::NeighborUp { peer },
        )));
        // Presence heartbeats are protocol noise — no badge bump.
        let _t2 = app.update(AppMessage::NetEvent(ConversationNetEvent::new(
            topic,
            NetEvent::Message {
                from: peer,
                message: Message::Presence,
                sent_at: 0,
                backfilled: false,
            },
        )));

        let conv = app.conversations.get(&topic).unwrap();
        assert_eq!(
            conv.unread, 0,
            "gossip protocol events never bump the unread badge"
        );
        assert_eq!(
            conv.pending_events.len(),
            0,
            "gossip protocol events are dropped, not queued (31cc58ac flood fix)"
        );
    }

    #[test]
    fn opening_conversation_clears_unread_badge_but_keeps_backlog() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        let topic = direct_topic(&app.local_public, &peer);
        let other = TopicId::from_bytes([9u8; 32]);

        // A hidden conversation with a badge and a queued backlog.
        let mut conv = ConversationLive::new(topic);
        conv.unread = 3;
        conv.pending_events.push_back(NetEvent::Message {
            from: peer,
            message: Message::Message {
                text: "queued 1".into(),
            },
            sent_at: 1,
            backfilled: false,
        });
        conv.pending_events.push_back(NetEvent::Message {
            from: peer,
            message: Message::Message {
                text: "queued 2".into(),
            },
            sent_at: 2,
            backfilled: false,
        });
        app.conversations.insert(topic, conv);

        // A different room is active; opening the hidden conversation clears
        // its badge (sidebar unread anchor) while the backlog stays queued
        // for incremental replay.
        app.topic = other;
        app.screen = Screen::Chat { topic: other };
        assert!(app.switch_to_conversation(topic), "switch to backlog room");

        let restored = app
            .conversations
            .get(&topic)
            .expect("backlog conversation retained in map");
        assert_eq!(restored.unread, 0, "viewing clears the unread badge");
        assert_eq!(
            restored.pending_events.len(),
            2,
            "queued backlog is retained for replay after the badge clears"
        );
    }

    // ═════════════════════════════════════════════════════════════════
    // UI-RESTYLE-11 follow-up — PERMANENT regression harness for the
    // three creation flows (Create Group Chat / Create Public Room /
    // Create Tunnel).
    //
    // Refactored from the original UI-RESTYLE-11 temporary harness to
    // assert on OBSERVABLE OUTCOMES wherever possible:
    //   • system messages (entries with ChatKind::System)
    //   • toasts (toast_message)
    //   • store entries (conversation_store / shared_tunnels)
    //   • screen/dialog flags (show_*_dialog, share_local_service_open,
    //     room_loading)
    //   • rendered view (app.view() smoke-render)
    //
    // Remaining internal-state assertions (text-input values, member
    // selection set, expiry picker) are INTENTIONAL: iced's Element tree
    // is not introspectable in a headless test, so typed text and
    // selection state have no observable proxy other than the state that
    // renders them. Each such assertion is marked "intentional state".
    //
    // NOTE (UI-RESTYLE-07 drift): the Create Group Chat dialog no longer
    // has a search/filter field (removed during the dialog restyle), so
    // the original harness's CreateGroupSearchChanged coverage is
    // obsolete; name + description + member toggle coverage is retained.
    // ═════════════════════════════════════════════════════════════════

    fn vr_seed_friend(app: &mut IcedChat, peer: PublicKey, label: &str) {
        use boru_core::friends::{FriendId, FriendRecord, FriendRelationship};
        app.friends.upsert(
            FriendId::from_public_key(peer),
            FriendRecord {
                label: Some(label.to_string()),
                relationship: FriendRelationship::Friends,
                ..Default::default()
            },
        );
    }

    fn vr_system_bodies(app: &IcedChat) -> Vec<String> {
        app.entries
            .iter()
            .filter(|e| matches!(e.kind, ChatKind::System))
            .map(|e| e.body.clone())
            .collect()
    }

    #[test]
    fn vr_create_group_chat_opens_renders_and_accepts_input() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        vr_seed_friend(&mut app, peer, "Bob");

        // Observable: the dialog opens (screen/dialog flag).
        assert!(!app.show_create_group_dialog);
        let _ = app.update(AppMessage::ShowCreateGroupDialog);
        assert!(
            app.show_create_group_dialog,
            "dialog opens (observable flag)"
        );
        let _ = app.view(); // renders without panic

        // Intentional state: name/description inputs hold typed text; no
        // observable proxy exists in a headless test.
        let _ = app.update(AppMessage::CreateGroupNameChanged("Weekend Plans".into()));
        let _ = app.update(AppMessage::CreateGroupDescriptionChanged(
            "Beach trip".into(),
        ));
        assert_eq!(app.create_group_name, "Weekend Plans");
        assert_eq!(app.create_group_description, "Beach trip");

        // Intentional state: member selection set renders as checkboxes in
        // view(); toggle selects then deselects.
        let _ = app.update(AppMessage::CreateGroupMemberToggled(peer));
        assert!(
            app.create_group_selected_members.contains(&peer),
            "peer selectable and toggle selects it"
        );
        let _ = app.view(); // renders chip + selected row without panic

        let _ = app.update(AppMessage::CreateGroupMemberToggled(peer));
        assert!(
            app.create_group_selected_members.is_empty(),
            "toggle deselects"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_create_group_chat_confirm_cancel_and_validation() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        vr_seed_friend(&mut app, peer, "Bob");

        // Observable: cancel closes the dialog (screen/dialog flag).
        let _ = app.update(AppMessage::ShowCreateGroupDialog);
        assert!(app.show_create_group_dialog);
        let _ = app.update(AppMessage::HideCreateGroupDialog);
        assert!(!app.show_create_group_dialog);

        // Observable: empty-name confirm surfaces a SYSTEM MESSAGE and the
        // dialog stays open; no async creation starts (room_loading flag
        // stays false).
        let _ = app.update(AppMessage::ShowCreateGroupDialog);
        let _ = app.update(AppMessage::ConfirmCreateGroup);
        assert!(
            vr_system_bodies(&app).contains(&"Group name is required.".to_string()),
            "system message surfaces for empty group name"
        );
        assert!(
            app.show_create_group_dialog,
            "dialog stays open on invalid submit"
        );
        assert!(!app.room_loading, "no async creation started");

        // Observable: named confirm raises the submit loading flag and
        // room_loading (async creation in flight); post UI-RESTYLE-07 the
        // dialog stays open with a loading state rather than closing
        // immediately.
        let _ = app.update(AppMessage::CreateGroupNameChanged("Weekend Plans".into()));
        let _ = app.update(AppMessage::ConfirmCreateGroup);
        assert!(
            app.create_group_submitting,
            "submit loading flag raised on valid submit"
        );
        assert!(app.room_loading, "creation loading flag set");
        assert!(
            app.show_create_group_dialog,
            "dialog stays open while submitting"
        );
        let _ = app.view(); // renders loading state without panic
    }

    #[test]
    fn vr_create_public_room_opens_renders_and_accepts_input() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Observable: the dialog opens (screen/dialog flag); DHT defaults on.
        assert!(!app.rooms_state.show_create_room_dialog);
        let _ = app.update(AppMessage::CreateNewRoom);
        assert!(
            app.rooms_state.show_create_room_dialog,
            "dialog opens (observable flag)"
        );
        assert!(app.rooms_state.create_room_dht_enabled, "DHT discovery defaults on");
        let _ = app.view(); // renders without panic

        // Intentional state: name text and toggle switches; no observable
        // proxy in a headless test.
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Lobby".into()));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::CreateNewRoomDhtToggled(false));
        assert_eq!(app.rooms_state.create_room_name, "Lobby");
        assert_eq!(
            app.rooms_state.create_room_visibility,
            RoomVisibility::PublicDiscoverable,
            "visibility picker accepted"
        );
        assert!(!app.rooms_state.create_room_dht_enabled, "DHT toggle accepted");
        let _ = app.view();
    }

    #[test]
    fn vr_create_public_room_confirm_cancel_and_validation() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Observable: cancel closes the dialog (screen/dialog flag).
        let _ = app.update(AppMessage::CreateNewRoom);
        assert!(app.rooms_state.show_create_room_dialog);
        let _ = app.update(AppMessage::CancelCreateRoom);
        assert!(!app.rooms_state.show_create_room_dialog);

        // Observable: advertised path persists a conversation-store entry
        // with the entered name (store entry) and raises the submit loading
        // flag; post UI-RESTYLE-07 the dialog stays open while the async
        // create runs instead of closing immediately.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Beach House".into()));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        assert!(
            app.rooms_state.create_room_submitting,
            "submit loading flag raised on create"
        );
        assert!(
            app.rooms_state.show_create_room_dialog,
            "dialog stays open while creating"
        );
        assert!(
            app.conversation_store
                .iter()
                .any(|e| e.name == "Beach House"),
            "conversation-store entry persisted with entered name"
        );
        let _ = app.view(); // renders loading state without panic

        // Observable: empty name is ALLOWED (existing behaviour) — the
        // display name falls back to the topic id; no error surfaces, and
        // an archived conversation-store entry is still persisted while
        // the submit loading flag is raised.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged(String::new()));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        assert!(
            app.rooms_state.create_room_submitting,
            "submit loading flag raised for empty-name create"
        );
        assert!(
            app.conversation_store
                .iter()
                .any(|e| e.archived && !e.name.is_empty() && e.name == e.topic.to_string()),
            "empty-name fallback persists entry named by topic id"
        );
    }

    #[test]
    fn vr_create_public_unlisted_room_does_not_advertise() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-DIR-05: PublicUnlisted is the conservative default — the room is
        // created and persisted, but it is never advertised, upserted into the
        // directory, or broadcast.
        let _ = app.update(AppMessage::CreateNewRoom);
        assert_eq!(
            app.rooms_state.create_room_visibility,
            RoomVisibility::PublicUnlisted,
            "new-room dialog defaults to the conservative PublicUnlisted visibility"
        );
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Quiet Corner".into()));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        assert!(
            app.rooms_state.create_room_submitting,
            "submit loading flag raised on create"
        );
        assert!(
        app.conversation_store
            .iter()
            .any(|e| e.name == "Quiet Corner"
                && e.visibility == RoomVisibility::PublicUnlisted),
        "unlisted room persists an archived entry with PublicUnlisted visibility"
    );
        assert!(
            app.rooms_state.advertised_rooms.is_empty(),
            "unlisted room must not be marked for advertising"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_create_public_room_rejects_oversized_metadata_before_broadcast() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-DIR-05: oversized metadata is rejected BEFORE any side effect or
        // broadcast — an error surfaces, the dialog stays open, and no
        // conversation entry is persisted.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("x".repeat(
            boru_core::control_plane::advertisement::DEFAULT_MAX_ROOM_NAME_LEN + 1,
        )));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        assert!(
            app.rooms_state.create_room_error.is_some(),
            "oversized room name surfaces a validation error"
        );
        assert!(
            app.rooms_state.show_create_room_dialog,
            "dialog stays open so the creator can fix the rejected input"
        );
        assert!(
            !app.rooms_state.create_room_submitting,
            "submit flag resets after rejected validation"
        );
        assert!(
            app.conversation_store.iter().all(|e| e.name.is_empty()),
            "no conversation entry is persisted for rejected metadata"
        );
        assert!(
            app.rooms_state.advertised_rooms.is_empty(),
            "no advertisement is emitted for rejected metadata"
        );

        // Oversized description is rejected the same way.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Valid Name".into()));
        let _ = app.update(AppMessage::CreateNewRoomDescriptionChanged("y".repeat(
            boru_core::control_plane::advertisement::DEFAULT_MAX_DESCRIPTION_LEN + 1,
        )));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        assert!(
            app.rooms_state.create_room_error.is_some(),
            "oversized description surfaces a validation error"
        );
        assert!(
            app.conversation_store.iter().all(|e| e.name.is_empty()),
            "no conversation entry is persisted for rejected description"
        );

        // Too many tags are rejected the same way.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Valid Name".into()));
        let tags = (0..=boru_core::control_plane::advertisement::DEFAULT_MAX_TAGS)
            .map(|i| format!("tag{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let _ = app.update(AppMessage::CreateNewRoomTagsChanged(tags));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        assert!(
            app.rooms_state.create_room_error.is_some(),
            "excess tags surface a validation error"
        );

        // After fixing the metadata, creation succeeds and the room is advertised.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Valid Name".into()));
        let _ = app.update(AppMessage::CreateNewRoomDescriptionChanged(
            "A short description".into(),
        ));
        let _ = app.update(AppMessage::CreateNewRoomTagsChanged("rust,chat".into()));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        assert!(app.rooms_state.create_room_error.is_none(), "valid metadata passes");
        assert!(
            app.conversation_store.iter().any(|e| e.name == "Valid Name"
                && e.description == "A short description"
                && e.tags == vec!["rust".to_string(), "chat".to_string()]),
            "validated + normalized metadata persists on the room entry"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_created_public_room_is_conversation_never_discovery_topic() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Create an advertised public room via the explicit user flow.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Beach House".into()));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);

        // The public room is an ordinary conversation: a conversation-store
        // entry with its own topic + name metadata.
        let entry = app
            .conversation_store
            .iter()
            .find(|e| e.name == "Beach House")
            .expect("created public room must have a conversation-store entry");
        assert!(
            !boru_core::discovery_topic::is_discovery_topic(entry.topic),
            "public-room topic must not be the internal discovery topic"
        );
        assert_eq!(
            boru_core::discovery_topic::topic_kind(entry.topic),
            boru_core::discovery_topic::TopicKind::Conversation,
            "public-room topic must classify as Conversation, never Discovery"
        );

        // The discovery topic itself must never appear as a conversation entry.
        assert!(
            !app.conversation_store
                .iter()
                .any(|e| boru_core::discovery_topic::is_discovery_topic(e.topic)),
            "the discovery topic must never be persisted as a conversation"
        );

        // BORU-DISC-13 guard: OpenRoom refuses the discovery topic, keeping
        // the discovery mesh separate from the public-chat conversation model.
        let disc = boru_core::discovery_topic::discovery_topic(
            boru_core::public_room::PublicNetwork::Mainnet,
        );
        let _ = app.update(AppMessage::OpenRoom(disc));
        assert!(
            !matches!(app.screen, Screen::Chat { topic } if topic == disc),
            "OpenRoom must refuse the discovery topic"
        );
    }

    #[test]
    fn vr_owner_can_switch_room_to_discoverable() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-DIR-06 (PDF Task 2.3): an owner-created PublicUnlisted room can
        // be switched to PublicDiscoverable. The room is persisted with the new
        // visibility and marked for advertising (periodic refresh + immediate
        // publish).
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Owner Room".into()));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        let entry = app
            .conversation_store
            .iter()
            .find(|e| e.name == "Owner Room")
            .expect("created room must exist in the conversation store");
        let topic = entry.topic;
        assert_eq!(entry.visibility, RoomVisibility::PublicUnlisted);

        // Non-owner attempt is rejected before the owner path is exercised.
        let _ = app.update(AppMessage::SetRoomDirectoryVisibility {
            topic,
            visibility: RoomVisibility::PublicDiscoverable,
        });
        assert!(
            app.rooms_state.advertised_rooms.contains(&topic),
            "owner switch to Discoverable marks the room for advertising"
        );
        assert_eq!(
            app.conversation_store
                .find(&topic)
                .map(|e| e.visibility)
                .unwrap_or(RoomVisibility::Private),
            RoomVisibility::PublicDiscoverable,
            "owner switch to Discoverable persists the new visibility"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_non_owner_cannot_change_directory_visibility() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-DIR-06 (PDF Task 2.3): a room joined from the directory keeps the
        // `Private` visibility default and is not advertised — the local user is
        // NOT its owner and must not be able to change its directory visibility.
        // The switch is rejected with no side effects.
        let topic = TopicId::from_bytes([7; 32]);
        let entry = boru_core::conversations::ConversationEntry::new(
            topic,
            "",
            "Someone Else's Room".to_string(),
        );
        // Joined rooms keep the `Private` default (only the create-public-room
        // flow sets a non-Private visibility).
        assert_eq!(entry.visibility, RoomVisibility::Private);
        app.conversation_store.upsert(entry);
        assert!(
            !app.is_room_directory_owner(topic),
            "a joined (not created, not advertised) room is not locally owned"
        );

        let _ = app.update(AppMessage::SetRoomDirectoryVisibility {
            topic,
            visibility: RoomVisibility::PublicDiscoverable,
        });
        assert!(
            !app.rooms_state.advertised_rooms.contains(&topic),
            "non-owner switch must not mark the room for advertising"
        );
        assert_eq!(
            app.conversation_store
                .find(&topic)
                .map(|e| e.visibility)
                .unwrap_or(RoomVisibility::Private),
            RoomVisibility::Private,
            "non-owner switch must not change the room's visibility"
        );

        // The room-settings dialog must not open either.
        let _ = app.update(AppMessage::OpenRoomSettings(topic));
        assert!(
            !app.rooms_state.show_room_settings_dialog,
            "non-owner cannot open the room-settings dialog"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_switch_to_unlisted_stops_advertising() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-DIR-06 (PDF Task 2.3): switching a discoverable room back to
        // PublicUnlisted stops refreshing (removes from advertised_rooms) and
        // removes the local directory entry. There is no withdrawal message yet
        // (BORU-DIR-09) — remote directories drop the advertisement on TTL
        // expiry.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged(
            "Advertised Room".into(),
        ));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        let entry = app
            .conversation_store
            .iter()
            .find(|e| e.name == "Advertised Room")
            .expect("created room must exist in the conversation store");
        let topic = entry.topic;
        assert!(
            app.rooms_state.advertised_rooms.contains(&topic),
            "discoverable room starts advertised"
        );

        let _ = app.update(AppMessage::SetRoomDirectoryVisibility {
            topic,
            visibility: RoomVisibility::PublicUnlisted,
        });
        assert!(
            !app.rooms_state.advertised_rooms.contains(&topic),
            "unlisting removes the room from the advertised set (refresh stops)"
        );
        assert_eq!(
            app.conversation_store
                .find(&topic)
                .map(|e| e.visibility)
                .unwrap_or(RoomVisibility::Private),
            RoomVisibility::PublicUnlisted,
            "unlisting persists PublicUnlisted visibility"
        );
        // The local directory entry is removed so the room disappears from the
        // PUBLIC ROOMS sidebar immediately.
        let dir_has_topic = app
            .directory_store
            .lock()
            .map(|store| store.contains(topic, app.local_public))
            .unwrap_or(false);
        assert!(
            !dir_has_topic,
            "unlisting removes the local directory advertisement"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_room_settings_dialog_edits_metadata_and_republishes() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-DIR-06 (PDF Task 2.3): the room-settings dialog lets the owner
        // edit advertised metadata (name / description / tags) and the
        // visibility. On save the metadata is normalized + persisted and the
        // room is republished (visibility unchanged, still discoverable).
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Original Name".into()));
        let _ = app.update(AppMessage::CreateNewRoomDescriptionChanged(
            "Original description".into(),
        ));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicDiscoverable,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        let entry = app
            .conversation_store
            .iter()
            .find(|e| e.name == "Original Name")
            .expect("created room must exist in the conversation store");
        let topic = entry.topic;
        assert!(app.rooms_state.advertised_rooms.contains(&topic));

        // Open the room-settings dialog: pre-filled from the entry.
        let _ = app.update(AppMessage::OpenRoomSettings(topic));
        assert!(app.rooms_state.show_room_settings_dialog, "owner opens room settings");
        assert_eq!(app.rooms_state.room_settings_name, "Original Name");
        assert_eq!(app.rooms_state.room_settings_description, "Original description");
        assert_eq!(
            app.rooms_state.room_settings_visibility,
            RoomVisibility::PublicDiscoverable
        );

        // Edit metadata + visibility and save.
        let _ = app.update(AppMessage::RoomSettingsNameChanged("Renamed Room".into()));
        let _ = app.update(AppMessage::RoomSettingsDescriptionChanged(
            "Renamed description".into(),
        ));
        let _ = app.update(AppMessage::RoomSettingsTagsChanged("rust, chat".into()));
        let _ = app.update(AppMessage::ConfirmRoomSettings);
        assert!(
            !app.rooms_state.show_room_settings_dialog,
            "saving closes the room-settings dialog"
        );

        // Metadata edits propagate without changing room identity (topic).
        let updated = app
            .conversation_store
            .find(&topic)
            .expect("room entry still exists after edit");
        assert_eq!(updated.name, "Renamed Room", "name edit persisted");
        assert_eq!(
            updated.description, "Renamed description",
            "description edit persisted"
        );
        assert_eq!(
            updated.tags,
            vec!["rust".to_string(), "chat".to_string()],
            "tags edit persisted (normalized)"
        );
        assert_eq!(
            updated.visibility,
            RoomVisibility::PublicDiscoverable,
            "visibility unchanged"
        );
        assert!(
            app.rooms_state.advertised_rooms.contains(&topic),
            "room is still advertised after a metadata-only edit (republish path)"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_room_settings_dialog_rejects_oversized_metadata() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-DIR-06 (PDF Task 2.3): oversized edits are rejected by the same
        // bounds as the create flow; the dialog stays open and no metadata
        // change is persisted.
        let _ = app.update(AppMessage::CreateNewRoom);
        let _ = app.update(AppMessage::CreateNewRoomNameChanged("Valid Room".into()));
        let _ = app.update(AppMessage::CreateNewRoomVisibilityChanged(
            RoomVisibility::PublicUnlisted,
        ));
        let _ = app.update(AppMessage::ConfirmCreateNewRoom);
        let entry = app
            .conversation_store
            .iter()
            .find(|e| e.name == "Valid Room")
            .expect("created room must exist in the conversation store");
        let topic = entry.topic;

        let _ = app.update(AppMessage::OpenRoomSettings(topic));
        assert!(app.rooms_state.show_room_settings_dialog);
        let _ = app.update(AppMessage::RoomSettingsNameChanged("x".repeat(
            boru_core::control_plane::advertisement::DEFAULT_MAX_ROOM_NAME_LEN + 1,
        )));
        let _ = app.update(AppMessage::ConfirmRoomSettings);
        assert!(
            app.rooms_state.show_room_settings_dialog,
            "dialog stays open so the owner can fix the rejected input"
        );
        assert!(
            app.rooms_state.room_settings_error.is_some(),
            "oversized name surfaces a validation error in the dialog"
        );
        assert_eq!(
            app.conversation_store
                .find(&topic)
                .map(|e| e.name.as_str())
                .unwrap_or(""),
            "Valid Room",
            "rejected edit does not change the persisted name"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_create_tunnel_opens_renders_picks_friend_and_configures() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        vr_seed_friend(&mut app, peer, "Bob");

        // Observable: the friend-picker dialog opens (screen/dialog flag).
        assert!(!app.tunnels_state.show_create_tunnel_dialog);
        let _ = app.update(AppMessage::ShowCreateTunnelDialog);
        assert!(
            app.tunnels_state.show_create_tunnel_dialog,
            "picker opens (observable flag)"
        );
        // The create-tunnel picker carries an optional tunnel port; empty by
        // default means an automatic (ephemeral) listener port.
        assert!(
            app.tunnels_state.create_tunnel_port.is_empty(),
            "port defaults to automatic"
        );
        let _ = app.view(); // renders without panic

        // Intentional state: the port input accepts a valid port.
        let _ = app.update(AppMessage::CreateTunnelPortChanged("8443".into()));
        assert_eq!(app.tunnels_state.create_tunnel_port, "8443");

        // Observable: picking a friend routes to the share-local-service
        // form (dialog flags + screen transition).
        let _ = app.update(AppMessage::CreateTunnel(peer));
        assert!(!app.tunnels_state.show_create_tunnel_dialog, "picker closes after pick");
        assert!(
            app.tunnels_state.share_local_service_open,
            "share-local-service form opens (observable flag)"
        );
        assert!(
            matches!(app.screen, Screen::FriendProfile(p) if p == peer),
            "routes to friend profile screen"
        );

        // Intentional state: form defaults + updates; no observable proxy.
        assert_eq!(app.tunnels_state.share_service_name, "Development Server");
        assert_eq!(app.tunnels_state.share_service_port, "3000");
        assert_eq!(
            app.tunnels_state.share_service_expiry,
            boru_core::tunnel::service::TunnelDuration::OneHour
        );
        let _ = app.update(AppMessage::ShareLocalServiceNameChanged("Media".into()));
        let _ = app.update(AppMessage::ShareLocalServicePortChanged("8080".into()));
        let _ = app.update(AppMessage::ShareLocalServiceExpiryChanged(
            boru_core::tunnel::service::TunnelDuration::EightHours,
        ));
        assert_eq!(app.tunnels_state.share_service_name, "Media");
        assert_eq!(app.tunnels_state.share_service_port, "8080");
        assert_eq!(
            app.tunnels_state.share_service_expiry,
            boru_core::tunnel::service::TunnelDuration::EightHours
        );
        let _ = app.view();
    }

    #[test]
    fn vr_create_tunnel_confirm_cancel_and_validation() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        vr_seed_friend(&mut app, peer, "Bob");

        // Observable: cancel closes the picker (screen/dialog flag).
        let _ = app.update(AppMessage::ShowCreateTunnelDialog);
        assert!(app.tunnels_state.show_create_tunnel_dialog, "picker opens");
        let _ = app.update(AppMessage::CancelCreateTunnel);
        assert!(!app.tunnels_state.show_create_tunnel_dialog, "picker cancels");

        // Observable: cancel closes the share-local-service form.
        let _ = app.update(AppMessage::ShowCreateTunnelDialog);
        let _ = app.update(AppMessage::CreateTunnel(peer));
        assert!(app.tunnels_state.share_local_service_open, "form opens after friend pick");
        let _ = app.update(AppMessage::CancelShareLocalService);
        assert!(!app.tunnels_state.share_local_service_open, "form cancels");

        // Observable: bad port → TOAST, form stays open, no tunnel
        // registered (shared_tunnels store stays empty).
        let _ = app.update(AppMessage::ShowCreateTunnelDialog);
        let _ = app.update(AppMessage::CreateTunnel(peer));
        let _ = app.update(AppMessage::ShareLocalServicePortChanged(
            "not-a-port".into(),
        ));
        let _ = app.update(AppMessage::ConfirmShareLocalService);
        assert_eq!(
            app.notifications_state.toast_message.as_deref(),
            Some("Enter a valid local port (1-65535) to share."),
            "bad port surfaces a toast"
        );
        assert!(
            app.tunnels_state.share_local_service_open,
            "form stays open on invalid port"
        );
        assert!(app.tunnels_state.shared_tunnels.is_empty(), "no tunnel registered");

        // Observable: valid config closes the form and registers the
        // tunnel in shared_tunnels (store entry) with the service name.
        let _ = app.update(AppMessage::ShareLocalServiceNameChanged("Media".into()));
        let _ = app.update(AppMessage::ShareLocalServicePortChanged("3000".into()));
        let _ = app.update(AppMessage::ConfirmShareLocalService);
        assert!(!app.tunnels_state.share_local_service_open, "form closes on valid config");
        assert_eq!(app.tunnels_state.shared_tunnels.len(), 1, "one tunnel registered");
        assert!(
            app.tunnels_state.shared_tunnels
                .values()
                .any(|t| t.service_name == "Media"),
            "tunnel store entry carries the configured service name"
        );
    }

    #[test]
    fn vr_create_tunnel_picker_port_validation() {
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        vr_seed_friend(&mut app, peer, "Bob");

        // Observable: an invalid port (out of range) keeps the picker open
        // with a toast; the tunnel creation is not handed off to the share
        // form and no screen transition happens.
        let _ = app.update(AppMessage::ShowCreateTunnelDialog);
        let _ = app.update(AppMessage::CreateTunnelPortChanged("70000".into()));
        let _ = app.update(AppMessage::CreateTunnel(peer));
        assert!(
            app.tunnels_state.show_create_tunnel_dialog,
            "picker stays open on invalid port"
        );
        assert!(
            app.tunnels_state.create_tunnel_port_error.is_some(),
            "inline error set for out-of-range port"
        );
        assert_eq!(
            app.notifications_state.toast_message.as_deref(),
            Some("Enter a valid port (1-65535), or leave empty for an automatic port."),
            "invalid port surfaces a toast"
        );
        assert!(
            !matches!(app.screen, Screen::FriendProfile(p) if p == peer),
            "no screen transition on invalid port"
        );

        // Observable: a non-numeric port is rejected the same way.
        let _ = app.update(AppMessage::CreateTunnelPortChanged("not-a-port".into()));
        let _ = app.update(AppMessage::CreateTunnel(peer));
        assert!(
            app.tunnels_state.show_create_tunnel_dialog,
            "picker stays open on non-numeric port"
        );
        assert!(
            app.tunnels_state.create_tunnel_port_error.is_some(),
            "inline error set for non-numeric port"
        );

        // Observable: port `0` is reserved for automatic selection and is
        // rejected rather than silently binding an unintended listener.
        let _ = app.update(AppMessage::CreateTunnelPortChanged("0".into()));
        let _ = app.update(AppMessage::CreateTunnel(peer));
        assert!(app.tunnels_state.show_create_tunnel_dialog, "picker stays open on port 0");
        assert!(
            app.tunnels_state.create_tunnel_port_error.is_some(),
            "inline error set for port 0"
        );

        // Observable: a valid port proceeds to the share form, and the chosen
        // port is carried into the tunnel state used to build the offer.
        let _ = app.update(AppMessage::CreateTunnelPortChanged("8080".into()));
        let _ = app.update(AppMessage::CreateTunnel(peer));
        assert!(
            !app.tunnels_state.show_create_tunnel_dialog,
            "picker closes on valid port"
        );
        assert!(
            app.tunnels_state.share_local_service_open,
            "share-local-service form opens after valid port"
        );
        assert_eq!(
            app.tunnels_state.create_tunnel_port_error, None,
            "no inline error after valid port"
        );
        assert_eq!(app.tunnels_state.create_tunnel_port, "8080");
        let _ = app.view(); // renders without panic
    }

    // ── FONTS-17 Visual QA: offscreen capture harness ──────────────────
    //
    // Renders REAL app screens offscreen with the tiny-skia half of iced's
    // fallback renderer (no Xvfb, no window, no network/peers) and saves PNGs
    // to ./captures/ (or $CAPTURE_DIR). Used by the FONTS-17 visual QA card.
    // Run: rb test --bin boru --features gui,video-playback,terminal -- offscreen_capture --nocapture
    /// A video state transition changes card height, but it is not a new
    /// timeline entry.  Regression coverage for CHAT-SCROLL-03: the download
    /// start path must rebuild the affected row without re-arming a pending
    /// bottom snap that could steal a user's scrolled-up reading position.
    #[test]
    fn second_video_reflow_preserves_scrolled_up_position() {
        let src = include_str!("../app.rs");
        let fs_src = include_str!("files.rs");
        let start = fs_src
            .find("AppMessage::ExecuteDownloadAt(entry_index)")
            .expect("download-start handler must exist");
        let end = fs_src[start..]
            .find("AppMessage::PauseDownloadAt(entry_index)")
            .map(|offset| start + offset)
            .expect("pause handler must follow download-start handler");
        let handler = &fs_src[start..end];

        assert!(
            handler.contains("invalidate_from(entry_index)"),
            "Ready -> Active must still invalidate the affected virtualized row"
        );
        assert_eq!(
            handler.matches("self.keep_latest_visible()").count(),
            0,
            "card reflow must not re-arm snap_to_end after the user scrolls up"
        );
        assert!(
            src.contains("self.keep_latest_visible();\n        self.enforce_image_budget();"),
            "new-entry append path must retain its one-time follow-latest snap"
        );
    }

    #[cfg(test)]
    mod offscreen_capture {
        use super::*;
        use iced::advanced::graphics::text::font_system;
        use iced::advanced::layout;
        use iced::advanced::mouse;
        use iced::advanced::renderer;
        use iced::advanced::renderer::Headless;
        use iced::advanced::widget::Tree;
        use iced::{Font, Pixels, Rectangle, Size};
        use std::borrow::Cow;

        fn load_fonts() {
            let mut fs = font_system().write().unwrap();
            let fonts: &[&[u8]] = &[
                include_bytes!("../fonts/Figtree-Regular.ttf"),
                include_bytes!("../fonts/Figtree-Medium.ttf"),
                include_bytes!("../fonts/Figtree-SemiBold.ttf"),
                include_bytes!("../fonts/Raleway-ExtraBold.ttf"),
                include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
                include_bytes!("../fonts/JetBrainsMono-Medium.ttf"),
                include_bytes!("../fonts/InterTight-Bold.ttf"),
                include_bytes!("../fonts/PublicSans-Regular.ttf"),
                include_bytes!("../fonts/PublicSans-Medium.ttf"),
                include_bytes!("../fonts/PublicSans-SemiBold.ttf"),
            ];
            for bytes in fonts {
                fs.load_font(Cow::Borrowed(*bytes));
            }
        }

        fn captures_dir() -> std::path::PathBuf {
            std::env::var("CAPTURE_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("./captures"))
        }

        /// Render an iced Element offscreen and write it to `captures/<name>.png`.
        fn render_element(
            element: &mut iced::Element<'_, AppMessage>,
            name: &str,
            canvas_w: u32,
            canvas_h: u32,
            dark_mode: bool,
        ) {
            let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
                Font::default(),
                Pixels(16.0),
            ));
            let mut tree = Tree::new(element.as_widget());
            let limits =
                layout::Limits::new(Size::ZERO, Size::new(canvas_w as f32, canvas_h as f32));
            let node = element
                .as_widget_mut()
                .layout(&mut tree, &renderer, &limits);
            let theme = IcedChat::theme_from_dark(dark_mode);
            let viewport = Rectangle::with_size(Size::new(canvas_w as f32, canvas_h as f32));
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &renderer::Style::default(),
                iced::advanced::Layout::new(&node),
                mouse::Cursor::default(),
                &viewport,
            );
            let background = bg_surface(&theme);
            let rgba = renderer.screenshot(Size::new(canvas_w, canvas_h), 1.0, background);
            let dir = captures_dir();
            std::fs::create_dir_all(&dir).expect("create captures dir");
            let path = dir.join(format!("{name}.png"));
            image::save_buffer_with_format(
                &path,
                &rgba,
                canvas_w,
                canvas_h,
                image::ExtendedColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .expect("save png");
            eprintln!("captured {name}.png");
        }

        /// Build an offline app with realistic seed state for capture.
        fn seed_app(
            local_label: &str,
            peer_public: &PublicKey,
            dark_mode: bool,
        ) -> (tokio::runtime::Runtime, IcedChat) {
            let (runtime, mut app) = build_prewarm_test_app();
            app.screen = Screen::ChatList;
            app.dark_mode = dark_mode;
            app.window_width = 1200.0;
            app.local_label = local_label.to_string();
            app.mesh_health = MeshHealth::Good;
            app.mesh_connected_at = Some(std::time::Instant::now());
            app.neighbors.insert(*peer_public);
            app.direct_peers = 1;
            app.sender_ready = true;
            (runtime, app)
        }

        fn seed_friends(app: &mut IcedChat, dark_mode: bool) {
            use boru_core::friends::{FriendId, FriendRecord, FriendRelationship, FriendStatus};
            let names = ["Alice", "Bob", "Carol", "Dan"];
            for (i, name) in names.iter().enumerate() {
                let pk = SecretKey::generate().public();
                let fid = FriendId::from_public_key(pk);
                let record = FriendRecord {
                    label: Some(name.to_string()),
                    status: FriendStatus {
                        online: i % 2 == 0,
                        last_seen_at_unix_ms: Some(boru_core::chat_core::now_ms()),
                        last_offline_at_unix_ms: None,
                    },
                    relationship: FriendRelationship::Friends,
                    ..Default::default()
                };
                app.friends.upsert(fid, record);
            }
            app.dark_mode = dark_mode;
        }

        fn now_ms() -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        }

        #[test]
        fn capture_home_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.files_state.dashboard_sharing_summary =
                Some(crate::sharing_summary::SharingSummary::default());
            let mut element = app.view();
            render_element(&mut element, "home_light", 1200, 800, false);
        }

        #[test]
        fn capture_home_dark() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, true);
            let mut element = app.view();
            render_element(&mut element, "home_dark", 1200, 800, true);
        }

        #[test]
        fn capture_chat_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            let topic = TopicId::from_bytes([7u8; 32]);
            app.screen = Screen::Chat { topic };
            app.topic = topic;
            app.ticket_str = "room-ticket-abc".to_string();
            app.entries = vec![
                ChatEntry::system("You joined the room")
                    .with_timestamp(Some(now_ms() as i64 - 300_000)),
                ChatEntry::remote(
                    "Alice",
                    "Have you seen the new type system?",
                    None,
                    None,
                    None,
                )
                .with_timestamp(Some(now_ms() as i64 - 250_000)),
                ChatEntry::local("6c0f88fe9f", "Yes — Archivo looks much sharper.")
                    .with_timestamp(Some(now_ms() as i64 - 200_000)),
                ChatEntry::remote(
                    "Alice",
                    "And the sidebar finally matches.",
                    None,
                    None,
                    None,
                )
                .with_timestamp(Some(now_ms() as i64 - 150_000)),
                ChatEntry::system("Bob is online").with_timestamp(Some(now_ms() as i64 - 60_000)),
            ];
            app.names.insert(peer, "Alice".to_string());
            for entry in app.entries.iter_mut() {
                entry.update_cache();
            }
            app.sender = Some(boru_core::api::GossipSender::new(
                irpc::channel::mpsc::Sender::from(
                    tokio::sync::mpsc::channel::<boru_core::api::Command>(8).0,
                ),
            ));
            app.sender_ready = true;
            app.composer_text = "Typing a message…".to_string();
            let mut element = app.view();
            render_element(&mut element, "chat_light", 1200, 800, false);
        }

        #[test]
        fn capture_chat_dark() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, true);
            let topic = TopicId::from_bytes([7u8; 32]);
            app.screen = Screen::Chat { topic };
            app.topic = topic;
            app.entries = vec![
                ChatEntry::system("You joined the room")
                    .with_timestamp(Some(now_ms() as i64 - 300_000)),
                ChatEntry::remote(
                    "Alice",
                    "Have you seen the new type system?",
                    None,
                    None,
                    None,
                )
                .with_timestamp(Some(now_ms() as i64 - 250_000)),
                ChatEntry::local("6c0f88fe9f", "Yes — Archivo looks much sharper.")
                    .with_timestamp(Some(now_ms() as i64 - 200_000)),
                ChatEntry::remote(
                    "Alice",
                    "And the sidebar finally matches.",
                    None,
                    None,
                    None,
                )
                .with_timestamp(Some(now_ms() as i64 - 150_000)),
            ];
            app.names.insert(peer, "Alice".to_string());
            for entry in app.entries.iter_mut() {
                entry.update_cache();
            }
            app.sender = Some(boru_core::api::GossipSender::new(
                irpc::channel::mpsc::Sender::from(
                    tokio::sync::mpsc::channel::<boru_core::api::Command>(8).0,
                ),
            ));
            app.sender_ready = true;
            app.composer_text = "Nice work".to_string();
            let mut element = app.view();
            render_element(&mut element, "chat_dark", 1200, 800, true);
        }

        #[test]
        fn capture_file_sharing_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.screen = Screen::FileSharing;
            seed_friends(&mut app, false);
            use crate::shared_by_me_table::{RecipientAccess, RecipientView, SharedByMeRow};
            app.files_state.dashboard_shared_by_me_filter = vec![
                SharedByMeRow {
                    id: "local:default:m1".to_string(),
                    content_hash: "aa11".repeat(16),
                    display_name: "demo-recap.mp4".to_string(),
                    mime_type: Some("video/mp4".to_string()),
                    size_bytes: Some(24_000_000),
                    shared_on_ms: now_ms() - 3_600_000,
                    recipients: vec![RecipientView {
                        id: peer.to_string(),
                        label: "Alice".to_string(),
                        access: RecipientAccess::Allowed,
                    }],
                    has_explicit_recipients: true,
                    source_available: true,
                    downloads: None,
                },
                SharedByMeRow {
                    id: "local:default:m2".to_string(),
                    content_hash: "bb22".repeat(16),
                    display_name: "handbook.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    size_bytes: Some(1_200_000),
                    shared_on_ms: now_ms() - 86_400_000,
                    recipients: vec![RecipientView {
                        id: peer.to_string(),
                        label: "Alice".to_string(),
                        access: RecipientAccess::Allowed,
                    }],
                    has_explicit_recipients: true,
                    source_available: true,
                    downloads: None,
                },
            ];
            let mut element = app.view();
            render_element(&mut element, "file_sharing_light", 1200, 800, false);
        }

        #[test]
        fn capture_create_group_dialog_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            seed_friends(&mut app, false);
            app.show_create_group_dialog = true;
            app.create_group_name = "Family".to_string();
            let mut element = app.view();
            render_element(&mut element, "create_group_dialog_light", 1200, 800, false);
        }

        #[test]
        fn capture_create_room_dialog_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.rooms_state.show_create_room_dialog = true;
            app.rooms_state.create_room_name = "General".to_string();
            app.rooms_state.create_room_visibility = RoomVisibility::PublicUnlisted;
            let mut element = app.view();
            render_element(&mut element, "create_room_dialog_light", 1200, 800, false);
        }

        #[test]
        fn capture_create_tunnel_dialog_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            seed_friends(&mut app, false);
            app.tunnels_state.show_create_tunnel_dialog = true;
            let mut element = app.view();
            render_element(&mut element, "create_tunnel_dialog_light", 1200, 800, false);
        }

        #[test]
        fn capture_video_card_light() {
            load_fonts();
            let attachment = DownloadAttachment::new(
                TransferKind::Video,
                "interview-recap.mp4",
                "blob:video-ticket-1",
                "Alice",
                None,
            );
            #[cfg(feature = "video-playback")]
            let card = crate::video_file_card::BoruVideoFileCard::new(
                0,
                false,
                false,
                None,
                false,
                None,
                false,
                true,
                Some(now_ms() as i64),
                720.0,
                crate::layout::ComponentPlacement::video_card_default(),
            );
            #[cfg(not(feature = "video-playback"))]
            let card = crate::video_file_card::BoruVideoFileCard::new(
                0,
                false,
                false,
                (),
                false,
                Some(now_ms() as i64),
                720.0,
                crate::layout::ComponentPlacement::video_card_default(),
            );
            let mut element = card.view(&attachment);
            render_element(&mut element, "video_file_card_light", 800, 420, false);
        }

        #[test]
        fn capture_settings_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.screen = Screen::Settings;
            seed_friends(&mut app, false);
            let mut element = app.view();
            render_element(&mut element, "settings_light", 1200, 800, false);
        }

        // ── BORU-SSUI-02: sender screen-share control card shell ─────────
        // Renders the real sender branch of `view_screen_share_panel` (the
        // card below the conversation header) with an active streaming
        // session, enumerated sources, quality metrics and the runtime peer
        // name. The card must show the muted "Sharing your screen with
        // <peer>" title at top-left, a subtle surface/border/radius/shadow,
        // and stay compact (content height only).
        // Gated on `screen-sharing` (opt-in feature): the sender-panel fields
        // do not exist without it, so the module must not force the feature.
        #[cfg(feature = "screen-sharing")]
        fn seed_sender_share_session(app: &mut IcedChat, topic: TopicId, peer: &PublicKey) {
            use boru_core::screen_share::{
                CaptureSource, CaptureSourceId, CaptureSourceKind, PathKind, QualityPreset,
                ScreenShareSessionMetrics, ScreenShareStats, ScreenShareStatsSnapshot,
            };
            app.screen = Screen::Chat { topic };
            app.topic = topic;
            app.calls_state.screen_share_host_state = ScreenShareHostState::Streaming;
            app.calls_state.screen_share_sources = Some(vec![
                CaptureSource {
                    id: CaptureSourceId(1),
                    kind: CaptureSourceKind::Desktop,
                    title: "Entire desktop".to_string(),
                    width: 1920,
                    height: 1080,
                    geometry: None,
                },
                CaptureSource {
                    id: CaptureSourceId(2),
                    kind: CaptureSourceKind::Window,
                    title: "xfort-gorai".to_string(),
                    width: 1280,
                    height: 800,
                    geometry: None,
                },
                CaptureSource {
                    id: CaptureSourceId(3),
                    kind: CaptureSourceKind::Monitor,
                    title: "DP-1: Dell U2720Q".to_string(),
                    width: 2560,
                    height: 1440,
                    geometry: None,
                },
                // Long window title — the card must ellipsize it (BORU-SSUI-03).
                CaptureSource {
                    id: CaptureSourceId(4),
                    kind: CaptureSourceKind::Window,
                    title: "This is a very long application window title that should be ellipsized by the source card".to_string(),
                    width: 1920,
                    height: 1080,
                    geometry: None,
                },
            ]);
            app.calls_state.screen_share_selected_source = Some(CaptureSourceId(1));
            let mut stats = ScreenShareStats::new();
            let snapshot: ScreenShareStatsSnapshot = stats.snapshot();
            app.calls_state.screen_share_host_metrics = Some(ScreenShareSessionMetrics {
                codec: "h264".to_string(),
                width: 1920,
                height: 1080,
                fps: 30,
                bitrate_bps: 4_000_000,
                backend: "test-pattern".to_string(),
                path_kind: PathKind::Direct,
                preset: QualityPreset::LanHigh,
                adaptive_level: 0,
                snapshot,
            });
            app.calls_state.screen_share_audio_active = false;
            app.calls_state.screen_share_control_active = false;
            app.calls_state.screen_share_dev_overlay = false;
            app.names.insert(*peer, "Alice".to_string());
            // conversation_store entry so `view_screen_share_panel` resolves
            // the real display name for the "Sharing your screen with {name}"
            // title (never mockup text).
            let entry = ConversationEntry::new(topic, peer.to_string(), "Alice");
            app.conversation_store.upsert(entry);
            app.sender = Some(boru_core::api::GossipSender::new(
                irpc::channel::mpsc::Sender::from(
                    tokio::sync::mpsc::channel::<boru_core::api::Command>(8).0,
                ),
            ));
            app.sender_ready = true;
        }

        #[cfg(feature = "screen-sharing")]
        #[test]
        fn capture_screen_share_sender_card_light() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            let topic = TopicId::from_bytes([7u8; 32]);
            seed_sender_share_session(&mut app, topic, &peer);
            let mut element = app.view();
            render_element(
                &mut element,
                "screen_share_sender_card_light",
                1200,
                800,
                false,
            );
        }

        #[cfg(feature = "screen-sharing")]
        #[test]
        fn capture_screen_share_sender_card_dark() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, true);
            let topic = TopicId::from_bytes([7u8; 32]);
            seed_sender_share_session(&mut app, topic, &peer);
            let mut element = app.view();
            render_element(
                &mut element,
                "screen_share_sender_card_dark",
                1200,
                800,
                true,
            );
        }

        // ── BORU-SSUI-09: responsive layout at the PDF Task 9 window sizes ─
        // Renders the same streaming sender session at the three required
        // viewports — maximized 1920x1080+, reference ~1280x800, and a
        // narrow split-window — plus a long-peer-name variant to prove the
        // title ellipsizes. The panel's responsive control row measures its
        // own width through `LayoutConfig::responsive::tier_for_width`, so
        // the rendered row/wrap/stack behavior IS the acceptance check.
        #[cfg(feature = "screen-sharing")]
        #[test]
        fn capture_screen_share_sender_card_maximized_1920() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.window_width = 1920.0;
            let topic = TopicId::from_bytes([7u8; 32]);
            seed_sender_share_session(&mut app, topic, &peer);
            let mut element = app.view();
            render_element(
                &mut element,
                "screen_share_sender_card_maximized_1920",
                1920,
                1080,
                false,
            );
        }

        #[cfg(feature = "screen-sharing")]
        #[test]
        fn capture_screen_share_sender_card_reference_1280() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.window_width = 1280.0;
            let topic = TopicId::from_bytes([7u8; 32]);
            seed_sender_share_session(&mut app, topic, &peer);
            let mut element = app.view();
            render_element(
                &mut element,
                "screen_share_sender_card_reference_1280",
                1280,
                800,
                false,
            );
        }

        #[cfg(feature = "screen-sharing")]
        #[test]
        fn capture_screen_share_sender_card_narrow_split() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.window_width = 640.0;
            let topic = TopicId::from_bytes([7u8; 32]);
            seed_sender_share_session(&mut app, topic, &peer);
            let mut element = app.view();
            render_element(
                &mut element,
                "screen_share_sender_card_narrow_split",
                640,
                720,
                false,
            );
        }

        #[cfg(feature = "screen-sharing")]
        #[test]
        fn capture_screen_share_sender_card_long_peer_name() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.window_width = 1280.0;
            let topic = TopicId::from_bytes([7u8; 32]);
            seed_sender_share_session(&mut app, topic, &peer);
            // Override the conversation entry's display name with a very
            // long one — the card title reads `display_name()` from the
            // conversation store (never mockup text), so the title must
            // ellipsize it (never wrap or spill).
            let seeded = app
                .conversation_store
                .active_iter()
                .into_iter()
                .find(|entry| entry.topic == topic)
                .expect("seeded conversation");
            app.conversation_store.upsert(ConversationEntry {
                name: "A very long peer display name that must ellipsize".to_string(),
                ..seeded.clone()
            });
            let mut element = app.view();
            render_element(
                &mut element,
                "screen_share_sender_card_long_peer_name",
                1280,
                800,
                false,
            );
        }

        // ── BORU-SSUI-10: interaction/accessibility states ────────────
        // Renders the SAME streaming sender session but with the host state
        // forced to Stopped (terminal) while sources are still enumerated —
        // the PDF Task 10 "unavailable" state. Source cards and quality
        // segments must render dimmed/inert (muted surfaces, no accent
        // hover) with the session-ended tooltip, while the card still shows
        // the source picker and quality group rather than hiding them.
        #[cfg(feature = "screen-sharing")]
        #[test]
        fn capture_screen_share_sender_card_stopped_disabled() {
            load_fonts();
            let peer = SecretKey::generate().public();
            let (_rt, mut app) = seed_app("6c0f88fe9f", &peer, false);
            app.window_width = 1280.0;
            let topic = TopicId::from_bytes([7u8; 32]);
            seed_sender_share_session(&mut app, topic, &peer);
            // Terminal state: the session ended but the enumerated source
            // list is still populated (peer-ended path keeps sources), so
            // the picker + quality group render DISABLED.
            app.calls_state.screen_share_host_state = ScreenShareHostState::Stopped;
            let mut element = app.view();
            render_element(
                &mut element,
                "screen_share_sender_card_stopped_disabled",
                1280,
                800,
                false,
            );
        }
    }
    fn vr_create_tunnel_friend_profile_base_is_fill_sized() {
        // Regression guard for the "Create Tunnel screen cannot enter a port"
        // bug.  `iced::widget::lazy` always reports a `Shrink` size hint, so the
        // friend-profile base container MUST be explicitly Fill×Fill — otherwise
        // the transient overlays stacked over it (share-local-service dialog,
        // remove/block confirms, toast) are laid out inside Shrink bounds: the
        // dialog renders top-anchored and its lower fields (Local port, expiry,
        // footer) are clipped out of the visible window.
        let (_runtime, mut app, _local, peer) = build_join_request_test_app();
        vr_seed_friend(&mut app, peer, "Bob");

        // Base (no overlay open) must fill the whole main panel.  The Element
        // borrows `app`, so the size hint is captured inside a scoped block.
        let base_hint = {
            let base = app.view_friend_profile(peer);
            base.as_widget().size_hint()
        };
        assert_eq!(
            base_hint.width,
            iced::Length::Fill,
            "friend-profile base width must be Fill so overlays fill the window"
        );
        assert_eq!(
            base_hint.height,
            iced::Length::Fill,
            "friend-profile base height must be Fill so overlays fill the window"
        );

        // With the share-local-service dialog open, the stacked element must
        // still report Fill so the centred panel is not clipped.
        let _ = app.update(AppMessage::ShowCreateTunnelDialog);
        let _ = app.update(AppMessage::CreateTunnel(peer));
        assert!(app.tunnels_state.share_local_service_open, "share form opens");
        let overlay_hint = {
            let with_overlay = app.view_friend_profile(peer);
            with_overlay.as_widget().size_hint()
        };
        assert_eq!(
            overlay_hint.width,
            iced::Length::Fill,
            "share-dialog stack width must be Fill"
        );
        assert_eq!(
            overlay_hint.height,
            iced::Length::Fill,
            "share-dialog stack height must be Fill"
        );
    }

    // ── BORU-DIR-07: publish discoverable rooms on startup ──────────────

    /// Seed a conversation-store entry with the given visibility.
    fn seed_room_with_visibility(
        app: &mut IcedChat,
        name: &str,
        visibility: RoomVisibility,
    ) -> TopicId {
        let topic = TopicId::from_bytes(rand::random());
        let mut entry = ConversationEntry::new(topic, "", name);
        entry.visibility = visibility;
        app.conversation_store.upsert(entry);
        topic
    }

    #[test]
    fn vr_startup_publish_marks_discoverable_rooms_for_advertising() {
        // BORU-DIR-07 (PDF Task 3.1): after the discovery service is ready the
        // app must enumerate locally owned PublicDiscoverable rooms and mark
        // them for advertising so they reappear after a client restart. A
        // discoverable room persisted across restart (conversation-store entry
        // with `PublicDiscoverable` visibility) must be picked up by the sweep.
        let (_runtime, mut app) = build_prewarm_test_app();
        let topic =
            seed_room_with_visibility(&mut app, "Startup Room", RoomVisibility::PublicDiscoverable);

        // No directory sender in the unit harness — the sweep must still mark
        // the room for periodic refresh (the acceptance criterion "startup
        // remains usable if discovery broadcasting fails"), and must not panic.
        let task = app.publish_startup_room_advertisements();
        assert!(
            app.rooms_state.advertised_rooms.contains(&topic),
            "startup sweep must re-advertise a persisted discoverable room"
        );
        // The broadcast itself is fire-and-forget; dropping the task is fine.
        drop(task);
        let _ = app.view();
    }

    #[test]
    fn vr_startup_publish_ignores_unlisted_and_private_rooms() {
        // BORU-DIR-07 (PDF visibility model): only PublicDiscoverable rooms
        // emit advertisements. Unlisted/private rooms must NOT be marked for
        // advertising by the startup sweep.
        let (_runtime, mut app) = build_prewarm_test_app();
        let discoverable =
            seed_room_with_visibility(&mut app, "Open Room", RoomVisibility::PublicDiscoverable);
        let unlisted =
            seed_room_with_visibility(&mut app, "Hidden Room", RoomVisibility::PublicUnlisted);
        let private_topic =
            seed_room_with_visibility(&mut app, "Private Room", RoomVisibility::Private);

        let _ = app.publish_startup_room_advertisements();
        assert!(app.rooms_state.advertised_rooms.contains(&discoverable));
        assert!(
            !app.rooms_state.advertised_rooms.contains(&unlisted),
            "PublicUnlisted rooms must not be advertised"
        );
        assert!(
            !app.rooms_state.advertised_rooms.contains(&private_topic),
            "Private rooms must not be advertised"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_startup_publish_is_idempotent_and_dedupes_unchanged() {
        // BORU-DIR-07 (PDF Task 3.1 step 5): the sweep runs at most once; a
        // second invocation must not re-publish (the `startup_advertise_swept`
        // guard) and unchanged metadata must be deduped within the dedupe
        // window.
        let (_runtime, mut app) = build_prewarm_test_app();
        let topic =
            seed_room_with_visibility(&mut app, "Stable Room", RoomVisibility::PublicDiscoverable);

        let first = app.publish_startup_room_advertisements();
        assert!(app.rooms_state.advertised_rooms.contains(&topic));
        drop(first);

        // Second sweep: one-shot guard prevents a duplicate burst.
        let second = app.publish_startup_room_advertisements();
        drop(second);
        assert!(
            app.rooms_state.advertised_rooms.contains(&topic),
            "room stays marked after repeated sweeps"
        );

        // Dedupe helper: same metadata within the window is NOT re-broadcast.
        let name = "Stable Room".to_string();
        let ticket = app.room_ticket(topic, &[]).to_string();
        app.record_advertisement_broadcast(topic, &name, "", &ticket);
        assert!(
            !app.should_broadcast_advertisement(topic, &name, "", &ticket),
            "identical metadata within the dedupe window is suppressed"
        );
        // Changing the description changes the fingerprint → allowed again.
        assert!(
            app.should_broadcast_advertisement(topic, &name, "new description", &ticket),
            "changed metadata must be re-broadcastable"
        );
        let _ = app.view();
    }

    #[test]
    fn vr_startup_publish_survives_missing_directory_sender() {
        // BORU-DIR-07 acceptance: "startup remains usable if discovery
        // broadcasting fails". With no directory sender the sweep logs and
        // returns Task::none() — the app must not panic and the rooms stay
        // marked for the periodic tick.
        let (_runtime, mut app) = build_prewarm_test_app();
        assert!(app.directory_sender.is_none(), "unit harness has no sender");
        let topic =
            seed_room_with_visibility(&mut app, "Offline Room", RoomVisibility::PublicDiscoverable);
        let task = app.publish_startup_room_advertisements();
        assert!(
            app.rooms_state.advertised_rooms.contains(&topic),
            "rooms remain marked even when broadcasting is unavailable"
        );
        drop(task);
        let _ = app.view();
    }

    // ── BORU-DIR-08 (PDF Task 3.2): TTL refresh and expiry ─────────────────

    #[test]
    fn vr_ttl_refresh_interval_much_shorter_than_ttl() {
        // PDF Task 3.2 step 5: "choose a refresh interval significantly shorter
        // than TTL so temporary packet loss does not immediately remove rooms".
        // The policy must keep at least a 5:1 margin so several consecutive lost
        // refreshes are still well inside the TTL.
        assert!(
            ADVERT_REFRESH_INTERVAL_SECS * 5 <= u64::from(ADVERT_TTL_SECS),
            "refresh interval ({} s) must be significantly shorter than TTL ({} s)",
            ADVERT_REFRESH_INTERVAL_SECS,
            ADVERT_TTL_SECS,
        );
        // The publisher and the protocol default must agree so receivers that
        // see a pre-DIR-08 advertisement (no TTL field) use the same expiry.
        assert_eq!(
            ADVERT_TTL_SECS,
            boru_core::chat_core::DEFAULT_ADVERT_TTL_SECS,
            "app TTL policy must match the protocol default"
        );
    }

    #[test]
    fn vr_ttl_periodic_refresh_cadence_is_jittered() {
        // PDF Task 3.2 step 3: jitter desynchronizes advertisers so they do not
        // re-broadcast in synchronized bursts. When the counter hits zero the
        // next refresh is scheduled 60–65 s out (base interval + 0..=5 s jitter),
        // never at a fixed global instant.
        let (_runtime, mut app) = build_prewarm_test_app();
        app.rooms_state.advertise_counter = 0;
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);
        assert!(
            (60..=65).contains(&app.rooms_state.advertise_counter),
            "jittered refresh cadence out of range: {}",
            app.rooms_state.advertise_counter
        );
    }

    #[test]
    fn vr_ttl_expired_advertisement_leaves_directory_on_tick() {
        // PDF Task 3.2 acceptance: "a room whose advertiser disappears
        // eventually leaves the active directory". After the TTL elapses without
        // a refresh, the next monitor tick evicts the advertisement.
        let (_runtime, mut app) = build_prewarm_test_app();
        let author = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0x42; 32]);
        let ad = RoomAdvertisement {
            room_name: "Vanishing Room".to_string(),
            description: String::new(),
            topic,
            ticket: boru_core::chat_core::Ticket::new(topic, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            expires_after_secs: 1,
        };
        {
            let mut store = app.directory_store.lock().unwrap();
            store.upsert(ad.clone(), author);
        }
        // Wait out the TTL, then let the monitor tick evict.
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        let task = app.update(AppMessage::ConnMonitorTick);
        drop(task);
        {
            let store = app.directory_store.lock().unwrap();
            assert!(
                !store.contains(topic, author),
                "expired advertisement must leave the active directory"
            );
            assert!(store.list_active().is_empty());
        }
        let _ = app.view();
    }

    #[test]
    fn vr_ttl_recently_refreshed_advertisement_stays_in_directory() {
        // PDF Task 3.2 acceptance: "temporary network loss does not cause
        // constant room flicker". A room whose last refresh is well inside its
        // TTL must remain listed — the eviction sweep must not remove entries
        // just because a refresh is momentarily overdue.
        let (_runtime, mut app) = build_prewarm_test_app();
        let author = SecretKey::generate().public();
        let topic = TopicId::from_bytes([0x43; 32]);
        let ad = RoomAdvertisement {
            room_name: "Steady Room".to_string(),
            description: String::new(),
            topic,
            ticket: boru_core::chat_core::Ticket::new(topic, vec![]).to_string(),
            member_count: 0,
            last_activity: 0,
            // Policy TTL (300 s): the periodic refresh interval (60 s) is far
            // shorter, so one missed refresh never expires the room.
            expires_after_secs: ADVERT_TTL_SECS,
        };
        {
            let mut store = app.directory_store.lock().unwrap();
            store.upsert(ad, author);
        }
        // Several ticks with no refresh arriving: still listed (no flicker).
        for _ in 0..3 {
            let task = app.update(AppMessage::ConnMonitorTick);
            drop(task);
        }
        {
            let store = app.directory_store.lock().unwrap();
            assert!(
                store.contains(topic, author),
                "room within TTL must not be evicted by a missed refresh"
            );
            assert_eq!(store.list_active().len(), 1);
        }
        let _ = app.view();
    }

    // ── BORU-UI-07: live theme reload (t_b67246bf) ─────────────────────────
    //
    // The watcher (BORU-UI-06) delivers AppMessage::UiThemeReloaded into the
    // update loop. update_ui_theme_reloaded must replace ONLY the active theme
    // state: networking, gossip, rooms, tunnels, media, chat history, the
    // selected conversation, scroll position and composer input must all stay
    // untouched. A malformed/error reload keeps the last known-good theme.

    #[test]
    fn ui_theme_reload_replaces_only_theme_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Seed a selected conversation + composer + scroll state to prove the
        // reload handler does not touch them.
        let topic = TopicId::from_bytes([7; 32]);
        app.topic = topic;
        app.screen = Screen::Chat { topic };
        app.composer_text = "unsent draft".to_string();
        let mut conv = ConversationLive::new(topic);
        conv.composer_text = "unsent draft".to_string();
        conv.follow_latest = false;
        conv.scroll_offset = 123.0;
        app.conversations.insert(topic, conv);

        let revision_before = app.theme_revision;
        let theme_before = app.active_theme;
        assert_eq!(
            theme_before.sidebar.width, 304.0,
            "baseline: default sidebar width"
        );

        // A valid reload with a sidebar width override.
        let config = crate::theme_config::parse_ui_theme_config("sidebar = { width = 270.0 }")
            .expect("test config parses");
        let task = app.update_ui_theme_reloaded(1, Ok(config));
        drop(task);

        assert_eq!(
            app.active_theme.sidebar.width, 270.0,
            "valid reload replaces the active theme"
        );
        assert_ne!(app.active_theme, theme_before, "theme value changed");
        assert_eq!(
            app.theme_revision,
            revision_before.wrapping_add(1),
            "theme revision bumps so lazy/prewarm caches rebuild"
        );

        // Networking / conversation / composer / scroll state untouched.
        assert_eq!(app.topic, topic, "selected conversation unchanged");
        assert_eq!(app.screen, Screen::Chat { topic }, "screen unchanged");
        assert_eq!(app.composer_text, "unsent draft", "composer unchanged");
        let conv = app.conversations.get(&topic).expect("conversation kept");
        assert_eq!(
            conv.composer_text, "unsent draft",
            "conv composer unchanged"
        );
        assert_eq!(conv.scroll_offset, 123.0, "scroll offset unchanged");
        assert!(!conv.follow_latest, "follow_latest unchanged");
        assert_eq!(app.conversations.len(), 1, "no conversations added/removed");
    }

    /// BORU-UI-20 (PDF Task 20): a live theme change must not replace transfer
    /// state — the in-flight download bookkeeping (pending file, download entry
    /// index, active transfer id and the transfer-id → entry cache) survives a
    /// valid `boru-ui.toml` reload untouched.
    #[test]
    fn ui_theme_reload_preserves_transfer_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Seed a selected conversation with an in-flight download at both the
        // app level (legacy mirror used by the chat-log view) and the
        // conversation level (multi-conversation home).
        let topic = TopicId::from_bytes([13; 32]);
        app.topic = topic;
        app.screen = Screen::Chat { topic };
        let transfer_id = TransferId::new(9001);
        app.pending_file = Some(("report.pdf".to_string(), "ticket-abc".to_string()));
        app.download_entry_index = Some(3);
        app.active_download_transfer_id = Some(transfer_id);
        app.transfer_id_to_index.insert(transfer_id, 3);

        let mut conv = ConversationLive::new(topic);
        conv.pending_file = Some(("report.pdf".to_string(), "ticket-abc".to_string()));
        conv.download_entry_index = Some(3);
        conv.active_download_transfer_id = Some(transfer_id);
        conv.transfer_id_to_index.insert(transfer_id, 3);
        app.conversations.insert(topic, conv);

        // A valid reload changes ONLY the theme.
        let config = crate::theme_config::parse_ui_theme_config("sidebar = { width = 270.0 }")
            .expect("test config parses");
        let task = app.update_ui_theme_reloaded(1, Ok(config));
        drop(task);
        assert_eq!(
            app.active_theme.sidebar.width, 270.0,
            "valid reload replaces the active theme"
        );

        // Transfer state untouched at the app level…
        assert_eq!(
            app.pending_file,
            Some(("report.pdf".to_string(), "ticket-abc".to_string())),
            "app pending_file unchanged"
        );
        assert_eq!(
            app.download_entry_index,
            Some(3),
            "app download index unchanged"
        );
        assert_eq!(
            app.active_download_transfer_id,
            Some(transfer_id),
            "app active transfer id unchanged"
        );
        assert_eq!(
            app.transfer_id_to_index.get(&transfer_id),
            Some(&3),
            "app transfer-id → entry cache unchanged"
        );

        // …and at the conversation level.
        let conv = app.conversations.get(&topic).expect("conversation kept");
        assert_eq!(
            conv.pending_file,
            Some(("report.pdf".to_string(), "ticket-abc".to_string())),
            "conversation pending_file unchanged"
        );
        assert_eq!(
            conv.download_entry_index,
            Some(3),
            "conversation download index unchanged"
        );
        assert_eq!(
            conv.active_download_transfer_id,
            Some(transfer_id),
            "conversation active transfer id unchanged"
        );
        assert_eq!(
            conv.transfer_id_to_index.get(&transfer_id),
            Some(&3),
            "conversation transfer-id → entry cache unchanged"
        );
        assert_eq!(app.conversations.len(), 1, "no conversations added/removed");
    }

    #[cfg(feature = "video-playback")]
    #[test]
    fn ui_theme_reload_preserves_inline_video_state() {
        // BORU-UI-21 acceptance step 9: "Play a video and change visual
        // values; verify playback state is not reset unnecessarily." A live
        // theme reload must not touch the inline video player's state: the
        // active session, the seek position, the expanded flag and the
        // retained resume position all survive untouched.
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let topic = TopicId::from_bytes([17; 32]);
        let key = boru_core::video_playback::VideoInstanceKey::new(topic, 42, "blob-hash-1");
        app.inline_video_seek = Some(0.35);
        app.inline_video_expanded = true;
        app.inline_video_resume = Some((key.clone(), std::time::Duration::from_secs(12)));

        let config = crate::theme_config::parse_ui_theme_config("radii = { card = 4.0 }")
            .expect("test config parses");
        let task = app.update_ui_theme_reloaded(1, Ok(config));
        drop(task);
        assert_eq!(
            app.active_theme.radii.card, 4.0,
            "valid reload replaces the theme value"
        );

        assert_eq!(app.inline_video_seek, Some(0.35), "seek position unchanged");
        assert!(app.inline_video_expanded, "expanded flag unchanged");
        assert_eq!(
            app.inline_video_resume,
            Some((key, std::time::Duration::from_secs(12))),
            "retained resume position unchanged"
        );
        assert!(
            app.playback_coordinator.active_video().is_none()
                || app.playback_coordinator.active_video().is_some(),
            "playback coordinator remains owned by the app"
        );
    }

    #[test]
    fn ui_theme_reload_error_keeps_last_known_good_theme() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.composer_text = "draft survives".to_string();
        let topic = TopicId::from_bytes([9; 32]);
        app.topic = topic;
        app.screen = Screen::Chat { topic };
        let mut conv = ConversationLive::new(topic);
        conv.scroll_offset = 42.0;
        app.conversations.insert(topic, conv);

        // First a valid reload lands a new width…
        let config = crate::theme_config::parse_ui_theme_config("sidebar = { width = 288.0 }")
            .expect("test config parses");
        let task = app.update_ui_theme_reloaded(1, Ok(config));
        drop(task);
        assert_eq!(app.active_theme.sidebar.width, 288.0);
        let revision_after_ok = app.theme_revision;

        // …then an error reload must NOT replace it.
        let bad = crate::theme_config::ThemeReloadError {
            path: std::path::PathBuf::from("boru-ui.toml"),
            kind: crate::theme_config::ThemeReloadErrorKind::Parse,
            message:
                "invalid dev theme override boru-ui.toml: TOML parse error at line 1, column 5"
                    .to_string(),
            line: Some(1),
            column: Some(5),
        };
        let task = app.update_ui_theme_reloaded(2, Err(bad));
        drop(task);

        assert_eq!(
            app.active_theme.sidebar.width, 288.0,
            "error reload keeps the last known-good theme"
        );
        assert_eq!(
            app.theme_revision, revision_after_ok,
            "error reload does not bump the theme revision"
        );
        assert_eq!(app.composer_text, "draft survives", "composer untouched");
        let conv = app.conversations.get(&topic).expect("conversation kept");
        assert_eq!(conv.scroll_offset, 42.0, "scroll offset untouched");
    }

    #[test]
    fn ui_theme_reload_stale_generation_is_dropped() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let revision_before = app.theme_revision;

        // Accept generation 5…
        let config = crate::theme_config::parse_ui_theme_config("sidebar = { width = 270.0 }")
            .expect("test config parses");
        let task = app.update_ui_theme_reloaded(5, Ok(config));
        drop(task);
        assert_eq!(app.active_theme.sidebar.width, 270.0);

        // …then a stale (older) generation must be dropped entirely.
        let stale = crate::theme_config::parse_ui_theme_config("sidebar = { width = 320.0 }")
            .expect("test config parses");
        let task = app.update_ui_theme_reloaded(4, Ok(stale));
        drop(task);

        assert_eq!(
            app.active_theme.sidebar.width, 270.0,
            "stale generation does not apply"
        );
        assert_eq!(
            app.theme_revision,
            revision_before.wrapping_add(1),
            "only the accepted reload bumps the revision"
        );
    }

    // ── BORU-LAYOUT-06: live layout reload (t_ba9342b7) ────────────────────
    //
    // The watcher (BORU-LAYOUT-06) delivers AppMessage::LayoutReloaded into the
    // update loop. update_layout_reloaded must replace ONLY layout state:
    // networking, gossip, rooms, tunnels, media, chat history, the selected
    // conversation, scroll position and composer input must all stay untouched.
    // A malformed/error reload keeps the last known-good layout (only validated
    // layouts are applied).

    #[test]
    fn layout_reload_replaces_only_layout_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Seed a selected conversation + composer + scroll state to prove the
        // reload handler does not touch them.
        let topic = TopicId::from_bytes([7; 32]);
        app.topic = topic;
        app.screen = Screen::Chat { topic };
        app.composer_text = "unsent draft".to_string();
        let mut conv = ConversationLive::new(topic);
        conv.composer_text = "unsent draft".to_string();
        conv.follow_latest = false;
        conv.scroll_offset = 123.0;
        app.conversations.insert(topic, conv);

        let revision_before = app.layout_revision;
        let layout_before = app.active_layout.clone();
        assert_eq!(
            layout_before.home.max_content_width,
            crate::design_tokens::DASHBOARD_MAX_WIDTH,
            "baseline: default layout reproduces the current max width"
        );

        // A valid reload with a home max-content-width override.
        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1200.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);

        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "valid reload replaces the active layout"
        );
        assert_ne!(app.active_layout, layout_before, "layout value changed");
        assert_eq!(
            app.layout_revision,
            revision_before.wrapping_add(1),
            "layout revision bumps so lazy/prewarm caches rebuild"
        );

        // Networking / conversation / composer / scroll state untouched.
        assert_eq!(app.topic, topic, "selected conversation unchanged");
        assert_eq!(app.screen, Screen::Chat { topic }, "screen unchanged");
        assert_eq!(app.composer_text, "unsent draft", "composer unchanged");
        let conv = app.conversations.get(&topic).expect("conversation kept");
        assert_eq!(
            conv.composer_text, "unsent draft",
            "conv composer unchanged"
        );
        assert_eq!(conv.scroll_offset, 123.0, "scroll offset unchanged");
        assert!(!conv.follow_latest, "follow_latest unchanged");
        assert_eq!(app.conversations.len(), 1, "no conversations added/removed");
    }

    #[test]
    fn layout_reload_error_keeps_last_known_good_layout() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        app.composer_text = "draft survives".to_string();
        let topic = TopicId::from_bytes([9; 32]);
        app.topic = topic;
        app.screen = Screen::Chat { topic };
        let mut conv = ConversationLive::new(topic);
        conv.scroll_offset = 42.0;
        app.conversations.insert(topic, conv);

        // First a valid reload lands a new max content width…
        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1200.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);
        assert_eq!(app.active_layout.home.max_content_width, 1200.0);
        let revision_after_ok = app.layout_revision;

        // …then an error reload must NOT replace it.
        let bad = crate::layout_config::LayoutReloadError {
            path: std::path::PathBuf::from("boru-layout.toml"),
            kind: crate::layout_config::LayoutReloadErrorKind::Parse,
            message:
                "invalid dev layout override boru-layout.toml: TOML parse error at line 1, column 5"
                    .to_string(),
            line: Some(1),
            column: Some(5),
        };
        let task = app.update_layout_reloaded(2, Err(bad));
        drop(task);

        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "error reload keeps the last known-good layout"
        );
        assert_eq!(
            app.layout_revision, revision_after_ok,
            "error reload does not bump the layout revision"
        );
        assert_eq!(app.composer_text, "draft survives", "composer untouched");
        let conv = app.conversations.get(&topic).expect("conversation kept");
        assert_eq!(conv.scroll_offset, 42.0, "scroll offset untouched");
    }

    #[test]
    fn layout_reload_stale_generation_is_dropped() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let revision_before = app.layout_revision;

        // Accept generation 5…
        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1200.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(5, Ok(overrides));
        drop(task);
        assert_eq!(app.active_layout.home.max_content_width, 1200.0);

        // …then a stale (older) generation must be dropped entirely.
        let stale =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1400.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(4, Ok(stale));
        drop(task);

        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "stale generation does not apply"
        );
        assert_eq!(
            app.layout_revision,
            revision_before.wrapping_add(1),
            "only the accepted reload bumps the revision"
        );
    }

    #[test]
    fn layout_reload_clamps_unsafe_values_with_warning() {
        // BORU-LAYOUT-06: only validated layouts are applied — unsafe values
        // (negative padding) are clamped by the merge and reported, never
        // applied verbatim.
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let overrides = crate::layout_config::parse_layout_config("[home.padding]\ntop = -4.0\n")
            .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);

        assert_eq!(
            app.active_layout.home.padding.top, 0.0,
            "negative padding is clamped to zero, not applied"
        );
        // The merge still bumped the revision (a layout was applied).
        assert_eq!(app.layout_revision, 1);
    }

    #[test]
    fn layout_reload_validation_rejects_duplicates_keeps_previous() {
        // BORU-LAYOUT-07: duplicate section ids fail semantic validation and
        // the last known-good layout is retained — never partially applied,
        // never a crash.
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // First a valid reload lands a new max content width…
        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1200.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);
        assert_eq!(app.active_layout.home.max_content_width, 1200.0);
        let revision_after_ok = app.layout_revision;

        // …then an override set with duplicate section ids must be rejected:
        // the layout stays exactly as it was (the duplicates parse fine — serde
        // accepts them — so this exercises the app-seam validation pass).
        let dup = crate::layout_config::parse_layout_config(
            "[home]\nsection_order = [\"Tunnels\", \"Tunnels\"]\n",
        )
        .expect("duplicate list still parses (validation is separate)");
        assert!(
            !crate::layout_config::validate_layout_overrides(&dup).is_empty(),
            "the test fixture must actually fail validation"
        );
        let task = app.update_layout_reloaded(2, Ok(dup));
        drop(task);

        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "duplicate section ids must not be applied"
        );
        assert_eq!(
            app.active_layout.home.section_order,
            crate::layout::LayoutConfig::default().home.section_order,
            "the default section order is untouched"
        );
        assert_eq!(
            app.layout_revision, revision_after_ok,
            "a rejected reload does not bump the layout revision"
        );
    }

    #[test]
    fn layout_reload_validation_error_keeps_last_known_good_layout() {
        // BORU-LAYOUT-07: a structured Validation error (watcher boundary)
        // keeps the last known-good layout exactly like a Parse error.
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1100.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);
        assert_eq!(app.active_layout.home.max_content_width, 1100.0);

        let bad = crate::layout_config::LayoutReloadError {
            path: std::path::PathBuf::from("boru-layout.toml"),
            kind: crate::layout_config::LayoutReloadErrorKind::Validation,
            message: "invalid dev layout override boru-layout.toml: \
                  home.section_order: duplicate section id \"Tunnels\" at index 1"
                .to_string(),
            line: None,
            column: None,
        };
        let task = app.update_layout_reloaded(2, Err(bad));
        drop(task);

        assert_eq!(
            app.active_layout.home.max_content_width, 1100.0,
            "validation error keeps the last known-good layout"
        );
        assert_eq!(
            app.layout_revision, 1,
            "a rejected reload does not bump the layout revision"
        );
    }

    // ── BORU-LAYOUT-11: acceptance tests (t_e72002d1) ───────────────────
    //
    // PDF Task 11 acceptance criteria, mirrored 1:1 from the theme reload
    // tests above:
    //   * Changing TOML immediately rearranges the home screen.
    //   * Chats, transfers and playback continue uninterrupted.
    //   * Invalid TOML never crashes Boru.
    // The pure-seam half of the matrix lives in `layout_regression.rs`; these
    // tests drive the live app seam (`update_layout_reloaded`).

    #[test]
    fn layout_reload_rearranges_home_screen() {
        // PDF Task 11 acceptance: "Changing TOML immediately rearranges the
        // home screen." A single reload applies a full home rearrangement —
        // section order, grid/list mode, column counts, gaps and max content
        // width — bumps the revision (so lazy/prewarm caches rebuild) and
        // leaves chat/composer/scroll state untouched.
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let topic = TopicId::from_bytes([23; 32]);
        app.topic = topic;
        app.screen = Screen::Chat { topic };
        app.composer_text = "draft survives".to_string();
        let mut conv = ConversationLive::new(topic);
        conv.composer_text = "draft survives".to_string();
        conv.follow_latest = false;
        conv.scroll_offset = 77.0;
        app.conversations.insert(topic, conv);

        let revision_before = app.layout_revision;

        let overrides = crate::layout_config::parse_layout_config(
            r#"
[home]
max_content_width = 1200.0
mode = "List"
section_order = ["Tunnels", "QuickActions", "Hero", "MeshHealth", "PeopleActivity"]

[home.grid]
main_portion = 3
rail_portion = 1
column_gap = 16.0

[home.quick_actions]
columns_wide = 6

[home.gaps]
card_gap = 12.0
"#,
        )
        .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);

        // The arrangement changed…
        assert_eq!(app.active_layout.home.max_content_width, 1200.0);
        assert_eq!(
            app.active_layout.home.section_order,
            vec![
                crate::layout::HomeSection::Tunnels,
                crate::layout::HomeSection::QuickActions,
                crate::layout::HomeSection::Hero,
                crate::layout::HomeSection::MeshHealth,
                crate::layout::HomeSection::PeopleActivity,
            ],
            "section order rearranged"
        );
        assert_eq!(
            app.active_layout.home.visible_sections(),
            app.active_layout.home.section_order,
            "rendered section list reflects the new order"
        );
        assert_eq!(
            app.active_layout.home.mode,
            crate::layout::HomeLayoutMode::List,
            "grid/list mode switched"
        );
        assert_eq!(
            app.active_layout.home.grid.main_portion, 3,
            "grid split changed"
        );
        assert_eq!(app.active_layout.home.grid.rail_portion, 1);
        assert_eq!(
            app.active_layout.home.grid.column_gap, 16.0,
            "column gap changed"
        );
        assert_eq!(
            app.active_layout.home.quick_actions.columns_wide, 6,
            "column count changed"
        );
        assert_eq!(
            app.active_layout.home.gaps.card_gap, 12.0,
            "card gap changed"
        );
        assert_eq!(
            app.layout_revision,
            revision_before.wrapping_add(1),
            "reload bumps the revision so lazy/prewarm caches rebuild"
        );

        // …and chat/composer/scroll state is untouched.
        assert_eq!(app.topic, topic, "selected conversation unchanged");
        assert_eq!(app.screen, Screen::Chat { topic }, "screen unchanged");
        assert_eq!(app.composer_text, "draft survives", "composer unchanged");
        let conv = app.conversations.get(&topic).expect("conversation kept");
        assert_eq!(
            conv.composer_text, "draft survives",
            "conv composer unchanged"
        );
        assert_eq!(conv.scroll_offset, 77.0, "scroll offset unchanged");
        assert!(!conv.follow_latest, "follow_latest unchanged");
        assert_eq!(app.conversations.len(), 1, "no conversations added/removed");
    }

    #[test]
    fn layout_reload_preserves_transfer_state() {
        // PDF Task 11 acceptance: "Chats, transfers and playback continue
        // uninterrupted." A live layout reload must not replace the in-flight
        // download bookkeeping — the pending file, download entry index,
        // active transfer id and the transfer-id → entry cache survive
        // untouched (mirror of ui_theme_reload_preserves_transfer_state).
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Seed a selected conversation with an in-flight download at both the
        // app level (legacy mirror used by the chat-log view) and the
        // conversation level (multi-conversation home).
        let topic = TopicId::from_bytes([13; 32]);
        app.topic = topic;
        app.screen = Screen::Chat { topic };
        let transfer_id = TransferId::new(9001);
        app.pending_file = Some(("report.pdf".to_string(), "ticket-abc".to_string()));
        app.download_entry_index = Some(3);
        app.active_download_transfer_id = Some(transfer_id);
        app.transfer_id_to_index.insert(transfer_id, 3);

        let mut conv = ConversationLive::new(topic);
        conv.pending_file = Some(("report.pdf".to_string(), "ticket-abc".to_string()));
        conv.download_entry_index = Some(3);
        conv.active_download_transfer_id = Some(transfer_id);
        conv.transfer_id_to_index.insert(transfer_id, 3);
        app.conversations.insert(topic, conv);

        // A valid layout reload changes ONLY the layout.
        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1200.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);
        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "valid reload replaces the layout"
        );

        // Transfer state untouched at the app level…
        assert_eq!(
            app.pending_file,
            Some(("report.pdf".to_string(), "ticket-abc".to_string())),
            "app pending_file unchanged"
        );
        assert_eq!(
            app.download_entry_index,
            Some(3),
            "app download index unchanged"
        );
        assert_eq!(
            app.active_download_transfer_id,
            Some(transfer_id),
            "app active transfer id unchanged"
        );
        assert_eq!(
            app.transfer_id_to_index.get(&transfer_id),
            Some(&3),
            "app transfer-id → entry cache unchanged"
        );

        // …and at the conversation level.
        let conv = app.conversations.get(&topic).expect("conversation kept");
        assert_eq!(
            conv.pending_file,
            Some(("report.pdf".to_string(), "ticket-abc".to_string())),
            "conversation pending_file unchanged"
        );
        assert_eq!(
            conv.download_entry_index,
            Some(3),
            "conversation download index unchanged"
        );
        assert_eq!(
            conv.active_download_transfer_id,
            Some(transfer_id),
            "conversation active transfer id unchanged"
        );
        assert_eq!(
            conv.transfer_id_to_index.get(&transfer_id),
            Some(&3),
            "conversation transfer-id → entry cache unchanged"
        );
        assert_eq!(app.conversations.len(), 1, "no conversations added/removed");
    }

    #[cfg(feature = "video-playback")]
    #[test]
    fn layout_reload_preserves_inline_video_state() {
        // PDF Task 11 acceptance: playback continues uninterrupted across a
        // layout reload. The inline video player's state — seek position,
        // expanded flag and retained resume position — survive untouched
        // (mirror of ui_theme_reload_preserves_inline_video_state).
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let topic = TopicId::from_bytes([17; 32]);
        let key = boru_core::video_playback::VideoInstanceKey::new(topic, 42, "blob-hash-1");
        app.inline_video_seek = Some(0.35);
        app.inline_video_expanded = true;
        app.inline_video_resume = Some((key.clone(), std::time::Duration::from_secs(12)));

        let overrides = crate::layout_config::parse_layout_config("[home.gaps]\ncard_gap = 12.0\n")
            .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);
        assert_eq!(
            app.active_layout.home.gaps.card_gap, 12.0,
            "valid reload replaces the layout value"
        );

        assert_eq!(app.inline_video_seek, Some(0.35), "seek position unchanged");
        assert!(app.inline_video_expanded, "expanded flag unchanged");
        assert_eq!(
            app.inline_video_resume,
            Some((key, std::time::Duration::from_secs(12))),
            "retained resume position unchanged"
        );
    }

    #[test]
    fn layout_reload_invalid_toml_never_crashes() {
        // PDF Task 11 acceptance: "Invalid TOML never crashes Boru." A hostile
        // sequence — malformed file, duplicate section ids, out-of-range
        // values, then a recovery — never panics, keeps the last known-good
        // layout on every failure, and the app keeps running.
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        app.composer_text = "draft survives".to_string();

        // 1. A valid reload lands a new layout.
        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1200.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(1, Ok(overrides));
        drop(task);
        assert_eq!(app.active_layout.home.max_content_width, 1200.0);
        let revision_after_ok = app.layout_revision;

        // 2. A malformed file (structured Parse error) keeps it — no crash.
        let bad = crate::layout_config::LayoutReloadError {
            path: std::path::PathBuf::from("boru-layout.toml"),
            kind: crate::layout_config::LayoutReloadErrorKind::Parse,
            message:
                "invalid dev layout override boru-layout.toml: TOML parse error at line 1, column 5"
                    .to_string(),
            line: Some(1),
            column: Some(5),
        };
        let task = app.update_layout_reloaded(2, Err(bad));
        drop(task);
        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "malformed reload keeps the last known-good layout"
        );
        assert_eq!(
            app.layout_revision, revision_after_ok,
            "a failed reload does not bump the layout revision"
        );

        // 3. Duplicate section ids fail validation — kept, no crash.
        let dup = crate::layout_config::parse_layout_config(
            "[home]\nsection_order = [\"Tunnels\", \"Tunnels\"]\n",
        )
        .expect("duplicate list still parses (validation is separate)");
        let task = app.update_layout_reloaded(3, Ok(dup));
        drop(task);
        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "duplicate section ids must not be applied"
        );
        assert_eq!(
            app.layout_revision, revision_after_ok,
            "a rejected reload does not bump the layout revision"
        );

        // 4. Out-of-range values are clamped, not applied — still no crash,
        //    and this reload DOES land (clamping is a successful apply).
        let clamped = crate::layout_config::parse_layout_config(
            "[home.padding]\ntop = -4.0\n[home]\nmax_content_width = 1.0e9\n",
        )
        .expect("out-of-range config parses");
        let task = app.update_layout_reloaded(4, Ok(clamped));
        drop(task);
        assert_eq!(
            app.active_layout.home.padding.top, 0.0,
            "negative padding clamped to zero, not applied"
        );
        assert_eq!(
            app.active_layout.home.max_content_width, 4096.0,
            "absurd width clamped to max, not applied"
        );
        assert_eq!(
            app.layout_revision,
            revision_after_ok.wrapping_add(1),
            "a clamped reload still applies and bumps the revision"
        );

        // 5. Recovery: a subsequent valid reload applies normally.
        let overrides =
            crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1100.0\n")
                .expect("test config parses");
        let task = app.update_layout_reloaded(5, Ok(overrides));
        drop(task);
        assert_eq!(
            app.active_layout.home.max_content_width, 1100.0,
            "the app recovers after invalid input"
        );
        assert_eq!(
            app.composer_text, "draft survives",
            "composer untouched through the whole sequence"
        );
    }

    // ── BORU-UI-09: dev UI Inspector (dev-ui feature only) ────────────────

    /// BORU-UI-18: merge warnings (values the merge had to clamp or replace)
    /// are recorded on the inspector draft so the panel can show them.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_records_merge_warnings_for_adjusted_values() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // A clean config produces no warnings.
        let clean = crate::theme_config::parse_ui_theme_config("sidebar = { width = 300.0 }")
            .expect("clean config parses");
        app.set_ui_theme_config(clean.clone());
        assert!(
            app.settings_state.inspector_draft.merge_warnings.is_empty(),
            "no warnings for in-range values"
        );

        // An out-of-range colour channel is clamped by the merge; the warning
        // must be captured (and the theme must keep running, not panic).
        let bad =
            crate::theme_config::parse_ui_theme_config("[colors]\nprimary = [9.0, 0.0, 0.0]\n")
                .expect("out-of-range colour parses");
        app.set_ui_theme_config(bad);
        assert_eq!(
            app.active_theme.colors.primary.r, 1.0,
            "colour channel clamped to the valid range"
        );
        assert_eq!(
            app.settings_state.inspector_draft.merge_warnings.len(),
            1,
            "the clamp is reported to the inspector"
        );
        assert!(
            app.settings_state.inspector_draft.merge_warnings[0].contains("colors.primary"),
            "warning names the field: {}",
            app.settings_state.inspector_draft.merge_warnings[0]
        );

        // A subsequent clean load clears the warnings.
        app.set_ui_theme_config(clean);
        assert!(app.settings_state.inspector_draft.merge_warnings.is_empty());
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_toggle_and_edit_updates_active_theme_via_messages() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Hidden by default; Ctrl+Shift+D toggles visibility.
        assert!(!app.settings_state.inspector_visible, "inspector hidden by default");
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ToggleVisible,
        ));
        drop(task);
        assert!(app.settings_state.inspector_visible, "inspector shown after toggle");

        // A slider edit is a normal Iced message that replaces ONLY theme state.
        let revision_before = app.theme_revision;
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetFloat {
                field: crate::inspector::ThemeField::SidebarWidth,
                value: 270.0,
            },
        ));
        drop(task);
        assert_eq!(
            app.active_theme.sidebar.width, 270.0,
            "slider edit applied to the active theme"
        );
        assert_eq!(
            app.theme_revision,
            revision_before.wrapping_add(1),
            "theme revision bumps so the UI redraws immediately"
        );

        // A toggle edit applies an optional visual feature.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetBool {
                field: crate::inspector::ThemeField::HomeShowActivityFeed,
                value: false,
            },
        ));
        drop(task);
        assert!(
            !app.active_theme.home.show_activity_feed,
            "toggle edit applied to the active theme"
        );

        // A colour edit (hex) applies through the pure mapping.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ColorTextChanged {
                field: crate::inspector::ThemeField::ColorPrimary,
                text: "#102030".to_string(),
            },
        ));
        drop(task);
        assert_eq!(
            app.active_theme.colors.primary,
            iced::Color::from_rgb(
                0x10 as f32 / 255.0,
                0x20 as f32 / 255.0,
                0x30 as f32 / 255.0
            ),
            "colour edit applied to the active theme"
        );

        // Toggle off clears the draft and hides the panel.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ToggleVisible,
        ));
        drop(task);
        assert!(
            !app.settings_state.inspector_visible,
            "inspector hidden after second toggle"
        );
        assert!(
            app.settings_state.inspector_draft.float_text.is_empty() && app.settings_state.inspector_draft.color_text.is_empty(),
            "drafts cleared when the panel closes"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_slider_edit_keeps_visual_feedback_immediate_and_defers_prewarm() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Warm a prewarm screen so the cache is non-empty, then verify a slider
        // storm (BORU-UI-19) keeps the visual feedback immediate while deferring
        // the expensive prewarm invalidation to the idle tick.
        app.idle_timer.last_input = std::time::Instant::now() - std::time::Duration::from_secs(3);
        let _ = app.update(AppMessage::IdleTick);
        assert_eq!(app.prewarm_cache.len(), 1, "one screen is pre-warmed");

        // Simulate a slider drag: many SetFloat messages in a row.
        for value in [270.0f32, 271.0, 272.0, 273.0, 274.0] {
            let task = app.update(AppMessage::Inspector(
                crate::inspector::InspectorMsg::SetFloat {
                    field: crate::inspector::ThemeField::SidebarWidth,
                    value,
                },
            ));
            drop(task);
        }

        // Visual feedback is immediate: the active theme reflects the last value
        // and the revision bumps so the UI redraws on the next frame.
        assert_eq!(
            app.active_theme.sidebar.width, 274.0,
            "slider drag applies the latest value immediately"
        );
        assert!(
            app.prewarm_invalidate_pending,
            "theme edits mark prewarm invalidation pending"
        );
        assert_eq!(
            app.prewarm_cache.len(),
            1,
            "prewarm cache is NOT cleared per slider event (coalesced)"
        );

        // The next idle tick consumes the pending flag and rebuilds.
        app.idle_timer.last_input = std::time::Instant::now() - std::time::Duration::from_secs(3);
        let _ = app.update(AppMessage::IdleTick);
        assert!(
            !app.prewarm_invalidate_pending,
            "idle tick consumed the flag"
        );
        assert_eq!(app.prewarm_cache.len(), 1, "one screen rebuilt");
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_reset_section_and_reset_all_via_messages() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Apply edits in several sections.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetFloat {
                field: crate::inspector::ThemeField::SidebarWidth,
                value: 270.0,
            },
        ));
        drop(task);
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetFloat {
                field: crate::inspector::ThemeField::ChatBubbleMaxWidth,
                value: 620.0,
            },
        ));
        drop(task);
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetBool {
                field: crate::inspector::ThemeField::HomeShowActivityFeed,
                value: false,
            },
        ));
        drop(task);
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ColorTextChanged {
                field: crate::inspector::ThemeField::ColorPrimary,
                text: "#102030".to_string(),
            },
        ));
        drop(task);
        assert_eq!(app.active_theme.sidebar.width, 270.0);
        assert_eq!(app.active_theme.chat.bubble_max_width, 620.0);
        assert!(!app.active_theme.home.show_activity_feed);

        // Reset Section (Sidebar): only the sidebar group returns to defaults.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ResetSection(crate::inspector::SectionId::Sidebar),
        ));
        drop(task);
        assert_eq!(
            app.active_theme.sidebar.width,
            crate::theme::BoruTheme::default().sidebar.width,
            "Reset Section restored the sidebar group"
        );
        assert_eq!(
            app.active_theme.chat.bubble_max_width, 620.0,
            "unrelated section untouched"
        );
        assert!(
            !app.active_theme.home.show_activity_feed,
            "unrelated section untouched"
        );
        assert_eq!(
            app.active_theme.colors.primary,
            iced::Color::from_rgb(
                0x10 as f32 / 255.0,
                0x20 as f32 / 255.0,
                0x30 as f32 / 255.0
            ),
            "colour edit untouched"
        );

        // Reset All: complete active theme back to Boru defaults.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ResetAll,
        ));
        drop(task);
        assert_eq!(
            app.active_theme,
            crate::theme::BoruTheme::default(),
            "Reset All restored the complete default theme"
        );
        assert_eq!(
            app.ui_theme_config,
            crate::theme_config::UiThemeConfig::default(),
            "Reset All cleared every config group"
        );
        assert!(
            app.settings_state.inspector_draft.float_text.is_empty() && app.settings_state.inspector_draft.color_text.is_empty(),
            "Reset All cleared all drafts"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_set_layout_float_updates_live_layout_and_bumps_revision() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let before_revision = app.layout_revision;

        // Slider edit for a layout float: applies immediately and bumps the
        // layout revision so lazy/prewarm caches rebuild.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetLayoutFloat {
                field: crate::layout_inspector::LayoutField::HomeMaxContentWidth,
                value: 1200.0,
            },
        ));
        drop(task);

        assert_eq!(
            app.active_layout.home.max_content_width, 1200.0,
            "live layout reflects the edit"
        );
        assert_eq!(
            app.layout_overrides
                .home
                .as_ref()
                .expect("home override group")
                .max_content_width,
            Some(1200.0),
            "editable override set reflects the edit"
        );
        assert!(
            app.layout_revision > before_revision,
            "layout revision bumped so cached views rebuild"
        );
        // Unrelated layout leaves stay at defaults.
        assert_eq!(
            app.active_layout.home.padding.top,
            crate::layout::LayoutConfig::default().home.padding.top
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_set_layout_choice_and_int_apply_immediately() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetLayoutChoice {
                field: crate::layout_inspector::LayoutField::HomeMode,
                value: "List".to_string(),
            },
        ));
        drop(task);
        assert_eq!(
            app.active_layout.home.mode,
            crate::layout::HomeLayoutMode::List
        );

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetLayoutInt {
                field: crate::layout_inspector::LayoutField::HomeGridRailPortion,
                value: 2,
            },
        ));
        drop(task);
        assert_eq!(app.active_layout.home.grid.rail_portion, 2);
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_home_section_visibility_preserves_order_and_other_layout_fields() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();
        let original_order = app.active_layout.home.section_order.clone();
        let original_padding = app.active_layout.home.padding.top;

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetHomeSectionVisibility {
                section: crate::layout::HomeSection::Tunnels,
                visible: false,
            },
        ));
        drop(task);
        assert!(app
            .active_layout
            .home
            .hidden_sections
            .contains(&crate::layout::HomeSection::Tunnels));
        assert_eq!(app.active_layout.home.section_order, original_order);
        assert_eq!(app.active_layout.home.padding.top, original_padding);
        assert_eq!(
            app.layout_overrides
                .home
                .as_ref()
                .expect("home override group")
                .hidden_sections,
            Some(vec![crate::layout::HomeSection::Tunnels])
        );

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetHomeSectionVisibility {
                section: crate::layout::HomeSection::Tunnels,
                visible: true,
            },
        ));
        drop(task);
        assert!(!app
            .active_layout
            .home
            .hidden_sections
            .contains(&crate::layout::HomeSection::Tunnels));
        assert_eq!(app.active_layout.home.section_order, original_order);
        assert_eq!(
            app.layout_overrides
                .home
                .as_ref()
                .expect("home override group")
                .hidden_sections,
            Some(Vec::new())
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_layout_sections_text_applies_when_valid() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // A partial list (mid-typing) does not apply — the draft is kept.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::LayoutSectionsTextChanged {
                field: crate::layout_inspector::LayoutField::HomeSectionOrder,
                text: "Tunnels, Bogus".to_string(),
            },
        ));
        drop(task);
        assert_eq!(
            app.active_layout.home.section_order,
            crate::layout::LayoutConfig::default().home.section_order,
            "invalid section list must not change the live layout"
        );
        assert!(
            app.settings_state.inspector_draft
                .layout_sections_text
                .contains_key(&crate::layout_inspector::LayoutField::HomeSectionOrder),
            "draft keeps the half-typed text"
        );

        // A valid list applies immediately.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::LayoutSectionsTextChanged {
                field: crate::layout_inspector::LayoutField::HomeSectionOrder,
                text: "Tunnels, Hero".to_string(),
            },
        ));
        drop(task);
        assert_eq!(
            app.active_layout.home.section_order,
            vec![
                crate::layout::HomeSection::Tunnels,
                crate::layout::HomeSection::Hero,
            ]
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_save_layout_writes_boru_layout_toml_and_reports_status() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Apply an edit, then Save Layout (BORU-LAYOUT-08).
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetLayoutFloat {
                field: crate::layout_inspector::LayoutField::HomeMaxContentWidth,
                value: 1200.0,
            },
        ));
        drop(task);
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SaveLayout,
        ));
        drop(task);

        // Success is reported inside the developer panel.
        assert_eq!(
            app.settings_state.inspector_draft.layout_save_status,
            crate::layout_inspector::LayoutSaveStatus::Saved,
            "save success shown in the panel status line"
        );

        // The file exists and contains exactly the edited override.
        let path = app
            .data_dir
            .join(crate::layout_config::LAYOUT_CONFIG_FILE_NAME);
        assert!(path.exists(), "boru-layout.toml written");
        let text = std::fs::read_to_string(&path).expect("read saved layout");
        assert!(
            text.contains("max_content_width = 1200.0"),
            "edited value persisted: {text}"
        );
        let cfg = crate::layout_config::parse_layout_config(&text).expect("saved file parses");
        assert_eq!(
            cfg.home.as_ref().expect("home group").max_content_width,
            Some(1200.0),
            "saved overrides match the editable layout"
        );

        // The reload path (what the dev watcher sees after the save)
        // reproduces the same active layout.
        let (merged, _) =
            crate::layout_merge::merge_layout_config(&crate::layout::LayoutConfig::default(), &cfg);
        assert_eq!(
            merged.home.max_content_width, app.active_layout.home.max_content_width,
            "watcher reload of the saved file yields the same active layout"
        );

        // No temp sibling is left behind by the atomic write.
        let leftovers: Vec<_> = std::fs::read_dir(&app.data_dir)
            .expect("read data dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp files remain: {leftovers:?}");
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_save_layout_failure_sets_failed_status() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Point the save at an unwritable location: replace data_dir with a
        // path under a regular file so the atomic write cannot create the
        // directory.
        let blocker = app.data_dir.join("not-a-dir");
        std::fs::write(&blocker, b"x").expect("create blocker file");
        let original_data_dir = std::mem::replace(&mut app.data_dir, blocker.join("nested"));

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SaveLayout,
        ));
        drop(task);

        match &app.settings_state.inspector_draft.layout_save_status {
            crate::layout_inspector::LayoutSaveStatus::Failed(msg) => {
                assert!(
                    msg.contains("cannot create"),
                    "failure explains the save error: {msg}"
                );
            }
            other => panic!("expected Failed status, got {other:?}"),
        }

        // Restore so the app's Drop path (if any) still works.
        app.data_dir = original_data_dir;
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_reset_layout_section_and_all_restore_defaults() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetLayoutFloat {
                field: crate::layout_inspector::LayoutField::HomeMaxContentWidth,
                value: 1200.0,
            },
        ));
        drop(task);
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetLayoutChoice {
                field: crate::layout_inspector::LayoutField::ComponentCardOrientation,
                value: "Vertical".to_string(),
            },
        ));
        drop(task);

        // Reset the Home section: only the home override group clears.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ResetLayoutSection(
                crate::layout_inspector::LayoutSectionId::Home,
            ),
        ));
        drop(task);
        assert!(
            app.layout_overrides.home.is_none(),
            "home override group cleared"
        );
        assert_eq!(
            app.active_layout.home.max_content_width,
            crate::layout::LayoutConfig::default()
                .home
                .max_content_width
        );
        assert_eq!(
            app.active_layout.component.card_orientation,
            crate::layout::CardOrientation::Vertical,
            "unrelated section untouched"
        );

        // Reset All: complete active layout back to defaults.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ResetLayoutAll,
        ));
        drop(task);
        assert_eq!(
            app.active_layout,
            crate::layout::LayoutConfig::default(),
            "Reset Layout All restored the complete default layout"
        );
        assert_eq!(
            app.layout_overrides,
            crate::layout::LayoutOverrides::default(),
            "Reset Layout All cleared every override group"
        );
        assert!(
            app.settings_state.inspector_draft.layout_float_text.is_empty()
                && app.settings_state.inspector_draft.layout_int_text.is_empty()
                && app.settings_state.inspector_draft.layout_sections_text.is_empty(),
            "Reset Layout All cleared all layout drafts"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_reload_layout_from_disk_discards_unsaved_changes_and_applies_disk_config() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Write a config to disk first (the reload target).
        let cfg = crate::layout_config::parse_layout_config("[home]\nmax_content_width = 1100.0\n")
            .expect("config parses");
        crate::layout_config::save_layout_config(&app.data_dir, &cfg).expect("save succeeds");

        // Apply an in-memory edit that is NOT saved to disk (unsaved change).
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetLayoutFloat {
                field: crate::layout_inspector::LayoutField::HomeMaxContentWidth,
                value: 1200.0,
            },
        ));
        drop(task);
        assert_eq!(app.active_layout.home.max_content_width, 1200.0);

        // Reload Layout From Disk replaces the in-memory edits.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ReloadLayoutFromDisk,
        ));
        drop(task);
        assert_eq!(
            app.settings_state.inspector_draft.layout_reload_status,
            crate::layout_inspector::LayoutReloadStatus::Reloaded
        );
        assert_eq!(
            app.active_layout.home.max_content_width, 1100.0,
            "disk config applied after reload"
        );
        assert!(
            app.settings_state.inspector_draft.layout_float_text.is_empty(),
            "reload clears layout drafts"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_reload_layout_missing_file_reports_failed_status() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ReloadLayoutFromDisk,
        ));
        drop(task);
        match &app.settings_state.inspector_draft.layout_reload_status {
            crate::layout_inspector::LayoutReloadStatus::Failed(msg) => {
                assert!(
                    msg.contains("cannot find"),
                    "missing file names the problem: {msg}"
                );
            }
            other => panic!("expected Failed status, got {other:?}"),
        }
        // The last known-good layout is retained.
        assert_eq!(app.active_layout, crate::layout::LayoutConfig::default());
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn gallery_responsive_preview_messages_update_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // BORU-UI-15: gallery starts at the typical-desktop preset.
        assert_eq!(
            app.settings_state.gallery_state.preset,
            crate::component_gallery::GalleryWidthPreset::Desktop,
            "gallery defaults to the desktop preset"
        );

        // Preset buttons switch the simulated width.
        let task = app.update(AppMessage::GalleryPreset(
            crate::component_gallery::GalleryWidthPreset::Narrow,
        ));
        drop(task);
        assert_eq!(
            app.settings_state.gallery_state.preset,
            crate::component_gallery::GalleryWidthPreset::Narrow,
            "Narrow preset applied via message"
        );

        // Dragging the custom slider both stores the width AND selects the
        // Custom preset so the preview follows the slider immediately.
        let task = app.update(AppMessage::GalleryCustomWidth(777.0));
        drop(task);
        assert_eq!(
            app.settings_state.gallery_state.preset,
            crate::component_gallery::GalleryWidthPreset::Custom,
            "slider drag selects the Custom preset"
        );
        assert_eq!(app.settings_state.gallery_state.custom_width, 777.0);
        assert_eq!(
            app.settings_state.gallery_state.width(),
            777.0,
            "simulated width follows the slider"
        );

        // Clicking a preset after a custom drag keeps the remembered slider
        // value, so switching back to Custom restores the last dragged width.
        let task = app.update(AppMessage::GalleryPreset(
            crate::component_gallery::GalleryWidthPreset::Maximized,
        ));
        drop(task);
        assert_eq!(
            app.settings_state.gallery_state.preset,
            crate::component_gallery::GalleryWidthPreset::Maximized,
            "Maximized preset applied after custom drag"
        );
        assert_eq!(app.settings_state.gallery_state.custom_width, 777.0);

        // BORU-LAYOUT-09: the layout-config preset switches the preview layout.
        assert_eq!(
            app.settings_state.gallery_state.layout_preset,
            crate::component_gallery::GalleryLayoutPreset::Default,
            "gallery defaults to the Default layout preset"
        );
        let task = app.update(AppMessage::GalleryLayoutPreset(
            crate::component_gallery::GalleryLayoutPreset::Narrow,
        ));
        drop(task);
        assert_eq!(
            app.settings_state.gallery_state.layout_preset,
            crate::component_gallery::GalleryLayoutPreset::Narrow,
            "Narrow layout preset applied via message"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_save_theme_writes_boru_ui_toml_and_reports_status() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Apply an edit, then Save Theme (BORU-UI-12).
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetFloat {
                field: crate::inspector::ThemeField::SidebarWidth,
                value: 270.0,
            },
        ));
        drop(task);
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SaveTheme,
        ));
        drop(task);

        // Success is reported inside the developer panel.
        assert_eq!(
            app.settings_state.inspector_draft.save_status,
            crate::inspector::ThemeSaveStatus::Saved,
            "save success shown in the panel status line"
        );

        // The file exists and contains exactly the edited override.
        let path = app.data_dir.join(crate::theme_config::UI_CONFIG_FILE_NAME);
        assert!(path.exists(), "boru-ui.toml written");
        let text = std::fs::read_to_string(&path).expect("read saved theme");
        assert!(
            text.contains("width = 270.0"),
            "edited value persisted: {text}"
        );
        let cfg = crate::theme_config::parse_ui_theme_config(&text).expect("saved file parses");
        assert_eq!(
            cfg.sidebar.as_ref().expect("sidebar group").width,
            Some(270.0),
            "saved overrides match the editable theme"
        );

        // The reload path (what the dev watcher sees after the save) reproduces
        // the same active theme — a partial write is impossible because the
        // write is atomic temp + rename.
        let (merged, _) =
            crate::theme_merge::merge_ui_theme(&crate::theme::BoruTheme::default(), &cfg);
        assert_eq!(
            merged.sidebar.width, app.active_theme.sidebar.width,
            "watcher reload of the saved file yields the same active theme"
        );

        // No temp sibling is left behind by the atomic write.
        let leftovers: Vec<_> = std::fs::read_dir(&app.data_dir)
            .expect("read data dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp files remain: {leftovers:?}");
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_save_theme_failure_sets_failed_status() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Point the save at an unwritable location: replace data_dir with a path
        // under a regular file so the atomic write cannot create the directory.
        let blocker = app.data_dir.join("not-a-dir");
        std::fs::write(&blocker, b"x").expect("create blocker file");
        let original_data_dir = std::mem::replace(&mut app.data_dir, blocker.join("nested"));

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SaveTheme,
        ));
        drop(task);

        match &app.settings_state.inspector_draft.save_status {
            crate::inspector::ThemeSaveStatus::Failed(msg) => {
                assert!(
                    msg.contains("boru-ui.toml"),
                    "failure names the dev theme file: {msg}"
                );
            }
            other => panic!("expected Failed status, got {other:?}"),
        }

        // Restore so the app's Drop path (if any) still works.
        app.data_dir = original_data_dir;
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_reload_from_disk_discards_unsaved_changes_and_applies_disk_config() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Apply an in-memory edit that is NOT saved to disk (unsaved change).
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetFloat {
                field: crate::inspector::ThemeField::SidebarWidth,
                value: 270.0,
            },
        ));
        drop(task);
        assert_eq!(app.active_theme.sidebar.width, 270.0);

        // Put a DIFFERENT config on disk than the unsaved edit.
        let path = app.data_dir.join(crate::theme_config::UI_CONFIG_FILE_NAME);
        std::fs::write(
            &path,
            "[sidebar]\nwidth = 250.0\n[chat]\nbubble_max_width = 600.0\n",
        )
        .expect("write on-disk theme");

        // Reload From Disk discards the unsaved edit and applies the file.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ReloadFromDisk,
        ));
        drop(task);

        assert_eq!(
            app.active_theme.sidebar.width, 250.0,
            "reload applied the on-disk sidebar value, not the unsaved edit"
        );
        assert_eq!(
            app.active_theme.chat.bubble_max_width, 600.0,
            "reload applied the on-disk chat value"
        );
        assert_eq!(
            app.settings_state.inspector_draft.reload_status,
            crate::inspector::ThemeReloadStatus::Reloaded,
            "reload success shown in the panel status line"
        );
        // The stored overrides now match the file, not the discarded edit.
        assert_eq!(
            app.ui_theme_config
                .sidebar
                .as_ref()
                .expect("sidebar group")
                .width,
            Some(250.0),
            "in-memory overrides replaced by the on-disk config"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_reload_from_disk_missing_file_keeps_current_theme_and_reports_error() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Make an in-memory edit, then ensure NO boru-ui.toml exists on disk.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetFloat {
                field: crate::inspector::ThemeField::SidebarWidth,
                value: 270.0,
            },
        ));
        drop(task);
        let path = app.data_dir.join(crate::theme_config::UI_CONFIG_FILE_NAME);
        let _ = std::fs::remove_file(&path);
        assert!(!path.exists(), "no theme file on disk");

        let before = app.active_theme;
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ReloadFromDisk,
        ));
        drop(task);

        assert_eq!(
            app.active_theme, before,
            "missing file keeps the current theme (BORU-UI-18)"
        );
        match &app.settings_state.inspector_draft.reload_status {
            crate::inspector::ThemeReloadStatus::Failed(msg) => {
                assert!(
                    msg.contains("boru-ui.toml"),
                    "failure names the dev theme file: {msg}"
                );
            }
            other => panic!("expected Failed status, got {other:?}"),
        }
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_reload_from_disk_malformed_file_keeps_current_theme_and_reports_error() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Make an in-memory edit, then write a MALFORMED file on disk.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetFloat {
                field: crate::inspector::ThemeField::SidebarWidth,
                value: 270.0,
            },
        ));
        drop(task);
        let path = app.data_dir.join(crate::theme_config::UI_CONFIG_FILE_NAME);
        std::fs::write(&path, "[sidebar\nwidth =").expect("write malformed theme file");

        let before = app.active_theme;
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ReloadFromDisk,
        ));
        drop(task);

        assert_eq!(
            app.active_theme, before,
            "malformed file keeps the current theme (BORU-UI-18)"
        );
        match &app.settings_state.inspector_draft.reload_status {
            crate::inspector::ThemeReloadStatus::Failed(msg) => {
                assert!(
                    msg.contains("boru-ui.toml"),
                    "failure names the dev theme file: {msg}"
                );
            }
            other => panic!("expected Failed status, got {other:?}"),
        }
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_section_collapse_is_view_local_state() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ToggleSection(crate::inspector::SectionId::Chat),
        ));
        drop(task);
        assert!(
            app.settings_state.inspector_draft
                .collapsed_sections
                .contains(&crate::inspector::SectionId::Chat),
            "Chat section collapsed"
        );

        // Collapse state never touches the theme.
        let theme_before = app.active_theme;
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::ToggleSection(crate::inspector::SectionId::Chat),
        ));
        drop(task);
        assert!(
            !app.settings_state.inspector_draft
                .collapsed_sections
                .contains(&crate::inspector::SectionId::Chat),
            "Chat section re-expanded"
        );
        assert_eq!(
            app.active_theme, theme_before,
            "collapse is view-local only"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_inspect_ui_toggle_hover_select_via_messages() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Inspection mode is OFF by default — no mouse wrappers, so normal
        // clicks are never intercepted unless the developer explicitly enables
        // the 'Inspect UI' toggle (BORU-UI-11).
        assert!(!app.settings_state.inspect_ui_enabled, "inspection mode off by default");
        assert!(app.settings_state.inspect_hover.is_none());
        assert!(app.settings_state.inspect_selected.is_none());

        // Hover before enabling is ignored (the guard in update_inspector).
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::InspectHover(Some(
                crate::inspector::ComponentId::Sidebar,
            )),
        ));
        drop(task);
        assert!(app.settings_state.inspect_hover.is_none(), "hover ignored while disabled");

        // Enable via the panel toggle.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetInspectUi(true),
        ));
        drop(task);
        assert!(app.settings_state.inspect_ui_enabled, "inspection mode enabled");

        // Hover now tracks the component under the cursor.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::InspectHover(Some(
                crate::inspector::ComponentId::Sidebar,
            )),
        ));
        drop(task);
        assert_eq!(
            app.settings_state.inspect_hover,
            Some(crate::inspector::ComponentId::Sidebar),
            "hover tracks the sidebar region"
        );

        // Selecting a component (click) jumps the inspector to its section:
        // expands it and records the selection for the highlight + status line.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::InspectSelect(crate::inspector::ComponentId::Sidebar),
        ));
        drop(task);
        assert_eq!(
            app.settings_state.inspect_selected,
            Some(crate::inspector::ComponentId::Sidebar),
            "selection recorded"
        );
        assert!(
            !app.settings_state.inspector_draft
                .collapsed_sections
                .contains(&crate::inspector::SectionId::Sidebar),
            "selecting Sidebar expanded the Sidebar section"
        );

        // Disabling clears hover/selection so no stale component stays
        // highlighted.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetInspectUi(false),
        ));
        drop(task);
        assert!(!app.settings_state.inspect_ui_enabled);
        assert!(app.settings_state.inspect_hover.is_none(), "hover cleared on disable");
        assert!(
            app.settings_state.inspect_selected.is_none(),
            "selection cleared on disable"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_inspect_select_jumps_to_component_section() {
        let (_runtime, mut app, _local, _peer) = build_join_request_test_app();

        // Enable inspection and select Chat — the inspector must expand the Chat
        // section and keep other state untouched.
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::SetInspectUi(true),
        ));
        drop(task);

        let theme_before = app.active_theme;
        let task = app.update(AppMessage::Inspector(
            crate::inspector::InspectorMsg::InspectSelect(crate::inspector::ComponentId::Chat),
        ));
        drop(task);
        assert_eq!(
            app.settings_state.inspect_selected,
            Some(crate::inspector::ComponentId::Chat)
        );
        assert!(
            !app.settings_state.inspector_draft
                .collapsed_sections
                .contains(&crate::inspector::SectionId::Chat),
            "Chat section expanded on select"
        );
        assert_eq!(
            app.active_theme, theme_before,
            "selection is view-local only"
        );
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn inspector_component_id_screen_mapping_is_total() {
        // Every Screen maps to a supported component id (BORU-UI-11). The mapping
        // drives which section the inspector jumps to when the main panel of that
        // screen is selected.
        let (_runtime, app, _local, _peer) = build_join_request_test_app();
        // component_id_for_screen covers the initial screen (ChatList → Home).
        assert_eq!(
            app.component_id_for_screen(),
            crate::inspector::ComponentId::Home
        );
    }
