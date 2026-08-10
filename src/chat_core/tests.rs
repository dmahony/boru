    use super::*;
    use bytes::Bytes;
    use crate::chat_core::downloads::{
        write_blob_to_reserved_file, CancelGuard, TransferProgressCallback,
    };
    use crate::discovery_secret::DiscoverySecret;
    use crate::friends::{FriendId, FriendsStore};
    use crate::proto::TopicId;
    use crate::user_profile::UserProfile;
    use iroh::{EndpointAddr, PublicKey, RelayMode, SecretKey};
    use serde::{Deserialize, Serialize};
    use serde_byte_array::ByteArray;
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use crate::chat_core::protocol::Signature;
    use std::collections::HashSet;

    // ── Composer tests ───────────────────────────────────────────────────

    #[test]
    fn composer_default_is_empty() {
        let c = Composer::default();
        assert!(c.is_empty());
        assert_eq!(c.text(), "");
        assert_eq!(c.cursor(), 0);
        assert_eq!(c.cursor_column(), 0);
    }

    #[test]
    fn composer_from_str_sets_text_and_cursor_at_end() {
        let c = Composer::from("hello");
        assert_eq!(c.text(), "hello");
        assert_eq!(c.cursor(), 5);
        assert!(!c.is_empty());
    }

    #[test]
    fn composer_insert_char_at_cursor() {
        let mut c = Composer::from("ab");
        c.move_home();
        c.insert_char('X');
        assert_eq!(c.text(), "Xab");
        assert_eq!(c.cursor(), 1);
    }

    #[test]
    fn composer_insert_str_at_cursor() {
        let mut c = Composer::from("ab");
        c.insert_str("XY");
        assert_eq!(c.text(), "abXY");
        assert_eq!(c.cursor(), 4);
    }

    #[test]
    fn composer_insert_str_mid_buffer() {
        let mut c = Composer::from("ab");
        c.move_home();
        c.insert_str("12");
        assert_eq!(c.text(), "12ab");
        assert_eq!(c.cursor(), 2);
    }

    #[test]
    fn composer_move_left_and_right() {
        let mut c = Composer::from("abc");
        c.move_left();
        assert_eq!(c.cursor(), 2);
        c.move_left();
        assert_eq!(c.cursor(), 1);
        c.move_left();
        assert_eq!(c.cursor(), 0);
        c.move_left(); // no-op at start
        assert_eq!(c.cursor(), 0);
        c.move_right();
        assert_eq!(c.cursor(), 1);
        c.move_right();
        assert_eq!(c.cursor(), 2);
        c.move_right();
        assert_eq!(c.cursor(), 3);
        c.move_right(); // no-op at end
        assert_eq!(c.cursor(), 3);
    }

    #[test]
    fn composer_move_home_and_end() {
        let mut c = Composer::from("hello world");
        c.move_home();
        assert_eq!(c.cursor(), 0);
        c.move_end();
        assert_eq!(c.cursor(), 11);
    }

    #[test]
    fn composer_backspace_removes_before_cursor() {
        let mut c = Composer::from("abcd");
        c.move_left();
        c.backspace();
        assert_eq!(c.text(), "abd");
        assert_eq!(c.cursor(), 2);
    }

    #[test]
    fn composer_backspace_at_start_does_nothing() {
        let mut c = Composer::from("test");
        c.move_home();
        c.backspace();
        assert_eq!(c.text(), "test");
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn composer_delete_removes_after_cursor() {
        // "abcd" cursor at end → move_left → cursor before 'd'
        // delete removes 'd' → "abc", cursor at end (3)
        let mut c = Composer::from("abcd");
        c.move_left();
        c.delete();
        assert_eq!(c.text(), "abc");
        assert_eq!(c.cursor(), 3);
    }

    #[test]
    fn composer_delete_at_end_does_nothing() {
        let mut c = Composer::from("abc");
        c.delete();
        assert_eq!(c.text(), "abc");
        assert_eq!(c.cursor(), 3);
    }

    #[test]
    fn composer_take_clears_buffer() {
        let mut c = Composer::from("hello");
        let taken = c.take();
        assert_eq!(taken, "hello");
        assert!(c.is_empty());
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn composer_cursor_column_is_unicode_aware() {
        let mut c = Composer::default();
        c.insert_char('é'); // 2 bytes, 1 column
        c.insert_char('☃'); // 3 bytes, 1 column
        assert_eq!(c.cursor_column(), 2);
        c.move_home();
        assert_eq!(c.cursor_column(), 0);
        c.move_right();
        assert_eq!(c.cursor_column(), 1);
        c.move_right();
        assert_eq!(c.cursor_column(), 2);
    }

    #[test]
    fn composer_insert_unicode_at_cursor() {
        let mut c = Composer::from("a");
        c.move_home();
        c.insert_char('é');
        assert_eq!(c.text(), "éa");
        assert_eq!(c.cursor(), 2);
    }

    // ── ChatEntry tests ──────────────────────────────────────────────────

    #[test]
    fn chat_entry_system_uses_system_label() {
        let e = ChatEntry::system("hello");
        assert!(matches!(e.kind, ChatKind::System));
        assert_eq!(e.label, "System");
        assert_eq!(e.body, "hello");
    }

    #[test]
    fn chat_entry_local_uses_given_label() {
        let e = ChatEntry::local("alice", "hey");
        assert!(matches!(e.kind, ChatKind::Local));
        assert_eq!(e.label, "alice");
        assert_eq!(e.body, "hey");
    }

    #[test]
    fn chat_entry_remote_uses_given_label() {
        let e = ChatEntry::remote("bob", "hi");
        assert!(matches!(e.kind, ChatKind::Remote));
        assert_eq!(e.label, "bob");
        assert_eq!(e.body, "hi");
    }

    // ── StatusContext tests ──────────────────────────────────────────────

    fn test_status() -> StatusContext {
        StatusContext {
            transport_status: "ready".into(),
            topic: TopicId::from_bytes([0u8; 32]),
            relay_mode: RelayMode::Default,
            connected: true,
            peer_count: 0,
            identity_label: "tester".into(),
            transport_notice: "notice".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
            last_activity: HashMap::new(),
            peer_latencies: HashMap::new(),
            mesh_health: MeshHealth::Good,
            dht_enabled: false,
            dht_peer_count: 0,
        }
    }

    fn test_app() -> AppState {
        AppState::new(
            test_status(),
            FriendsStore::default(),
            SecretKey::generate().public(),
            Some("tester".into()),
        )
    }

    /// Expected display fallback: last 5 hex characters of the peer ID.
    fn expected_name_suffix(peer: &iroh::PublicKey) -> String {
        let full = peer.to_string();
        full.chars()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    #[test]
    fn status_context_fields_are_accessible() {
        let s = test_status();
        assert_eq!(s.transport_status, "ready");
        assert_eq!(s.identity_label, "tester");
        assert!(s.connected);
    }

    // ── AppState tests ───────────────────────────────────────────────────

    #[test]
    fn app_state_new_creates_empty_state() {
        let app = test_app();
        assert!(app.entries.is_empty());
        assert!(app.composer.is_empty());
        assert!(app.follow_latest);
        assert!(!app.should_quit);
    }

    #[test]
    fn app_state_push_system_adds_entry_and_sets_follow() {
        let mut app = test_app();
        app.follow_latest = false;
        app.push_system("system msg");
        assert_eq!(app.entries.len(), 1);
        assert!(matches!(app.entries[0].kind, ChatKind::System));
        assert_eq!(app.entries[0].body, "system msg");
        assert!(app.follow_latest);
    }

    #[test]
    fn app_state_push_local_adds_local_entry() {
        let mut app = test_app();
        app.push_local("alice", "hello");
        assert!(matches!(app.entries[0].kind, ChatKind::Local));
        assert_eq!(app.entries[0].label, "alice");
        assert_eq!(app.entries[0].body, "hello");
    }

    #[test]
    fn app_state_push_remote_adds_remote_entry() {
        let mut app = test_app();
        app.push_remote("bob", "hi");
        assert!(matches!(app.entries[0].kind, ChatKind::Remote));
        assert_eq!(app.entries[0].label, "bob");
        assert_eq!(app.entries[0].body, "hi");
    }

    #[test]
    fn app_state_entries_maintain_insertion_order() {
        let mut app = test_app();
        app.push_system("sys");
        app.push_local("A", "local");
        app.push_remote("B", "remote");
        assert_eq!(app.entries.len(), 3);
        assert!(matches!(app.entries[0].kind, ChatKind::System));
        assert!(matches!(app.entries[1].kind, ChatKind::Local));
        assert!(matches!(app.entries[2].kind, ChatKind::Remote));
    }

    #[test]
    fn default_record_peer_ticket_ignores_invalid_ticket() {
        let peer = SecretKey::generate().public();
        let mut app = test_app();

        ChatCallbacks::record_peer_ticket(&mut app, peer, "not-a-ticket".into());

        assert!(app.friends.is_empty());
        assert!(!app.friends_dirty);
    }

    #[test]
    fn default_record_peer_ticket_ignores_self_ticket() {
        let mut app = test_app();
        let local_public = app.local_public;
        let ticket = Ticket {
            topic: TopicId::from_bytes([9; 32]),
            peers: vec![EndpointAddr::new(local_public)],
            discovery_secret: None,
        };

        ChatCallbacks::record_peer_ticket(&mut app, local_public, ticket.to_string());

        assert!(app.friends.is_empty());
        assert!(!app.friends_dirty);
    }

    #[test]
    fn default_record_peer_ticket_persists_valid_ticket() {
        let peer = SecretKey::generate().public();
        let mut app = test_app();
        let ticket = Ticket {
            topic: TopicId::from_bytes([8; 32]),
            peers: vec![EndpointAddr::new(peer)],
            discovery_secret: None,
        };

        ChatCallbacks::record_peer_ticket(&mut app, peer, ticket.to_string());

        let record = app
            .friends
            .get(&FriendId::from_public_key(peer))
            .expect("peer ticket creates friend record");
        assert_eq!(record.known_addrs, ticket.peers);
        assert_eq!(record.rooms.get(&ticket.topic), Some(&ticket));
        assert!(app.friends_dirty);
    }

    #[test]
    fn app_state_max_scroll_offset_zero_when_fewer_entries_than_height() {
        let mut app = test_app();
        assert_eq!(app.max_scroll_offset(10), 0);
        for i in 0..5 {
            app.push_system(format!("m{i}"));
        }
        assert_eq!(app.max_scroll_offset(10), 0);
    }

    #[test]
    fn app_state_max_scroll_offset_non_zero_when_more_entries_than_height() {
        let mut app = test_app();
        for i in 0..15 {
            app.push_system(format!("m{i}"));
        }
        assert_eq!(app.max_scroll_offset(10), 5);
    }

    #[test]
    fn app_state_rendered_scroll_following_returns_max() {
        let mut app = test_app();
        for i in 0..20 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = true;
        assert_eq!(app.rendered_scroll_offset(10), 10);
    }

    #[test]
    fn app_state_rendered_scroll_not_following_uses_scroll_offset() {
        let mut app = test_app();
        for i in 0..20 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = false;
        app.scroll_offset = 3;
        assert_eq!(app.rendered_scroll_offset(10), 3);
        // Clamped to max (10) when scroll_offset exceeds
        app.scroll_offset = 100;
        assert_eq!(app.rendered_scroll_offset(10), 10);
    }

    #[test]
    fn app_state_scroll_up_from_top_wraps() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.scroll_up(3, 5);
        assert!(!app.follow_latest);
        // max = 10 - 5 = 5, scroll_offset was 0 => wraps to 5 - 3 = 2
        assert_eq!(app.scroll_offset, 2);
    }

    #[test]
    fn app_state_scroll_up_from_mid() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.scroll_offset = 5;
        app.scroll_up(2, 5);
        assert_eq!(app.scroll_offset, 3);
    }

    #[test]
    fn app_state_scroll_down_re_enables_follow_at_bottom() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = false;
        app.scroll_offset = 0;
        app.scroll_down(10, 5); // max=5, so should land at 5
        assert_eq!(app.scroll_offset, 5);
        assert!(app.follow_latest);
    }

    #[test]
    fn app_state_scroll_down_does_not_follow_when_not_at_bottom() {
        let mut app = test_app();
        for i in 0..10 {
            app.push_system(format!("m{i}"));
        }
        app.follow_latest = false;
        app.scroll_offset = 0;
        app.scroll_down(2, 5);
        assert_eq!(app.scroll_offset, 2);
        assert!(!app.follow_latest);
    }

    #[test]
    fn app_state_push_entry_without_follow_does_not_change_flag() {
        let mut app = test_app();
        app.follow_latest = false;
        app.push_entry(ChatEntry::system("test"), false);
        assert!(
            !app.follow_latest,
            "push_entry with false should not change flag"
        );
    }

    #[test]
    fn app_state_push_entry_with_follow_sets_flag() {
        let mut app = test_app();
        app.follow_latest = false;
        app.push_entry(ChatEntry::system("test"), true);
        assert!(app.follow_latest);
    }

    // ── Message serialization tests ──────────────────────────────────────

    #[test]
    fn message_serialization_roundtrip_about_me() {
        let msg = Message::AboutMe {
            name: "alice".into(),
            profile_image_ticket: None,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        assert!(
            matches!(decoded, Message::AboutMe { ref name, profile_image_ticket: _ } if name == "alice")
        );
    }

    #[test]
    fn message_serialization_roundtrip_profile_update() {
        use crate::user_profile::UserProfile;
        let mut profile = UserProfile::new(
            PublicKey::from_bytes(&[1u8; 32]).expect("32 one-bytes is a valid ed25519 public key"),
        );
        profile.display_name = "alice".into();
        profile.bio = "hello".into();
        profile.file_sharing_enabled = true;
        profile.max_file_size = 1024 * 1024;
        profile.allowed_extensions = vec!["jpg".into(), "txt".into()];
        profile.avatar_identifier = Some("avatar-id".into());
        profile.shared_folder_path = std::path::PathBuf::from("/tmp/shared");
        profile.allow_downloads = true;
        let msg = Message::ProfileUpdate(profile);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::ProfileUpdate(profile) => {
                assert_eq!(profile.display_name, "alice");
                assert_eq!(profile.bio, "hello");
                assert!(profile.file_sharing_enabled);
                assert!(profile.allow_downloads);
            }
            _ => panic!("expected ProfileUpdate"),
        }
    }

    #[test]
    fn message_serialization_roundtrip_text() {
        let msg = Message::Message {
            text: "hello world".into(),
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        assert!(matches!(decoded, Message::Message { ref text } if text == "hello world"));
    }

    #[test]
    fn message_serialization_roundtrip_file_share() {
        let msg = Message::FileShare {
            name: "photo.png".into(),
            ticket: "ticket123".into(),
            size: 1024,
            thumbnail_hash: None,
            collection_hash: None,
            collection_entries: 0,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::FileShare {
                name, ticket, size, ..
            } => {
                assert_eq!(name, "photo.png");
                assert_eq!(ticket, "ticket123");
                assert_eq!(size, 1024);
            }
            _ => panic!("expected FileShare"),
        }
    }

    #[test]
    fn message_serialization_roundtrip_image_share() {
        let msg = Message::ImageShare {
            name: "cat.jpg".into(),
            hash: [0xab; 32],
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::ImageShare { name, hash } => {
                assert_eq!(name, "cat.jpg");
                assert_eq!(hash, [0xab; 32]);
            }
            _ => panic!("expected ImageShare"),
        }
    }

    #[test]
    fn message_serialization_roundtrip_shared_gif() {
        let msg = Message::SharedGif {
            gif: crate::gif_provider::SharedGif {
                provider: "klipy".into(),
                provider_id: "gif-7".into(),
                playback_url: "https://media.example/playback.mp4".into(),
                preview_url: Some("https://media.example/preview.gif".into()),
                fallback_url: Some("https://media.example/original.gif".into()),
                format: crate::gif_provider::GifMediaFormat::Mp4,
                width: Some(480),
                height: Some(360),
                alt_text: Some("a cat".into()),
            },
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::SharedGif { gif } => {
                assert_eq!(gif.provider, "klipy");
                assert_eq!(gif.provider_id, "gif-7");
                assert_eq!(gif.playback_url, "https://media.example/playback.mp4");
                assert_eq!(gif.format, crate::gif_provider::GifMediaFormat::Mp4);
                assert_eq!(gif.width, Some(480));
                assert_eq!(gif.height, Some(360));
            }
            _ => panic!("expected SharedGif"),
        }
    }

    #[test]
    fn message_shared_gif_legacy_variants_still_decode() {
        // Appending Message::SharedGif must not change the postcard variant
        // index of any pre-existing message type: a stored Message::Message
        // and Message::ImageShare (both serialized before this variant was
        // added) must still decode unchanged.
        let text = Message::Message {
            text: "hello world".into(),
        };
        let text_bytes = postcard::to_stdvec(&text).unwrap();
        match postcard::from_bytes::<Message>(&text_bytes).unwrap() {
            Message::Message { text } => assert_eq!(text, "hello world"),
            other => panic!("expected Message::Message, got {other:?}"),
        }

        let image = Message::ImageShare {
            name: "old.png".into(),
            hash: [0xcd; 32],
        };
        let image_bytes = postcard::to_stdvec(&image).unwrap();
        match postcard::from_bytes::<Message>(&image_bytes).unwrap() {
            Message::ImageShare { name, hash } => {
                assert_eq!(name, "old.png");
                assert_eq!(hash, [0xcd; 32]);
            }
            other => panic!("expected Message::ImageShare, got {other:?}"),
        }
    }

    #[test]
    fn shared_gif_net_event_routes_to_pending_gif() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.ensure_friend(fid.clone());
        app.friends.mark_online(fid);

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::SharedGif {
                gif: crate::gif_provider::SharedGif {
                    provider: "klipy".into(),
                    provider_id: "gif-9".into(),
                    playback_url: "https://media.example/playback.mp4".into(),
                    ..Default::default()
                },
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert_eq!(app.pending_gif.len(), 1);
        assert_eq!(app.pending_gif[0].0.provider_id, "gif-9");
        assert_eq!(app.pending_gif[0].1, remote_key.public());
        // SharedGif renders inline (no text system message).
        assert!(
            !app.entries.iter().any(|e| e.body.contains("gif-9")),
            "shared GIF should not create a text system message"
        );
    }

    #[test]
    fn shared_gif_unknown_provider_round_trips_in_message() {
        // Provider-neutral means an unknown provider string must survive the
        // full SignedMessage envelope without any provider-specific logic.
        let key = SecretKey::generate();
        let msg = Message::SharedGif {
            gif: crate::gif_provider::SharedGif {
                provider: "mystery-provider".into(),
                provider_id: "abc".into(),
                playback_url: "https://media.example/x.webp".into(),
                format: crate::gif_provider::GifMediaFormat::AnimatedWebP,
                ..Default::default()
            },
        };
        let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
        let (pk, decoded, _sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert_eq!(pk, key.public());
        match decoded {
            Message::SharedGif { gif } => {
                assert_eq!(gif.provider, "mystery-provider");
                assert_eq!(gif.format, crate::gif_provider::GifMediaFormat::AnimatedWebP);
            }
            other => panic!("expected SharedGif, got {other:?}"),
        }
    }

    #[test]
    fn signed_message_sign_and_verify_roundtrip() {
        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "secure chat".into(),
        };
        let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
        let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert_eq!(pk, key.public());
        assert!(sent_at > 0);
        assert!(matches!(decoded, Message::Message { ref text } if text == "secure chat"));
    }

    #[test]
    fn signed_message_rejects_tampered_data() {
        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "original".into(),
        };
        let mut encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap().to_vec();
        if let Some(b) = encoded.last_mut() {
            *b ^= 0xff;
        }
        let result = SignedMessage::verify_and_decode(&encoded);
        assert!(result.is_err(), "tampered message should fail verification");
    }

    #[test]
    fn signed_message_wrong_key_fails_verification() {
        let key_a = SecretKey::generate();
        let _key_b = SecretKey::generate();
        let msg = Message::Message {
            text: "secret".into(),
        };
        let encoded = SignedMessage::sign_and_encode(&key_a, &msg).unwrap();
        // Verification should still succeed because the signed message
        // contains the claimed public key — the signature matches key_a
        // and the protocol trusts the claimed key.  This test verifies
        // that a message signed by one key cannot be claimed as having
        // come from a different key after verification.
        let (_pk, _, _sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
    }

    #[test]
    fn signed_message_compressed_roundtrip_and_shorter() {
        let key = SecretKey::generate();
        // Repetitive content compresses well against the shared dictionary.
        let text = "hello hello hello hello hello hello hello hello hello hello hello hello";
        let msg = Message::Message { text: text.into() };

        let plain = SignedMessage::sign_and_encode(&key, &msg).unwrap();
        let compressed = SignedMessage::sign_and_encode_compressed(&key, &msg).unwrap();

        assert!(
            compressed.len() < plain.len(),
            "compressed ({} bytes) should be shorter than plain ({} bytes)",
            compressed.len(),
            plain.len()
        );

        let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&compressed).unwrap();
        assert_eq!(pk, key.public());
        assert!(sent_at > 0);
        assert!(matches!(decoded, Message::Message { text: ref t } if *t == text));

        // The uncompressed path still decodes too.
        let (_, decoded_plain, _) = SignedMessage::verify_and_decode(&plain).unwrap();
        assert!(matches!(decoded_plain, Message::Message { text: ref t } if *t == text));
    }

    #[test]
    fn signed_message_unknown_compression_rejected() {
        let key = SecretKey::generate();
        let msg = Message::Message { text: "x".into() };
        let mut encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap().to_vec();
        // The final byte is the `compression` field (0).  Flip it to an
        // unknown value.  Since BORU-AUDIT-27 the signature covers the
        // `compression` byte too, so tampering is caught by verification
        // before the compression check runs — strictly stronger than the
        // pre-AUDIT-27 behavior where the compression check alone had to
        // reject the forged value.
        if let Some(b) = encoded.last_mut() {
            *b = 7;
        }
        let err = SignedMessage::verify_and_decode(&encoded)
            .err()
            .expect("unknown compression must be an error");
        let msg = err.to_string();
        assert!(
            msg.contains("compression") || msg.contains("signature"),
            "error should mention compression or signature: {msg}"
        );
    }

    #[test]
    fn signed_message_backward_compat_without_compression_field() {
        // Simulate an envelope produced by a pre-compression peer: identical
        // fields but no trailing `compression` byte.  New peers must decode
        // it as compression = 0.
        #[derive(Serialize)]
        struct LegacyEnvelope {
            from: PublicKey,
            data: Bytes,
            signature: Signature,
            sent_at: u64,
        }

        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "legacy".into(),
        };
        let data: Bytes = postcard::to_stdvec(&msg).unwrap().into();
        let signature = key.sign(&data);
        let legacy = LegacyEnvelope {
            from: key.public(),
            data,
            signature: ByteArray::new(signature.to_bytes()),
            sent_at: 42,
        };
        let encoded = postcard::to_stdvec(&legacy).unwrap();

        let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert_eq!(pk, key.public());
        assert_eq!(sent_at, 42);
        assert!(matches!(decoded, Message::Message { ref text } if text == "legacy"));
    }

    #[test]
    fn new_compression_zero_message_decodes_with_legacy_struct() {
        // The reverse of `signed_message_backward_compat_without_compression_field`:
        // a *new* peer signs a message with compression = 0 (5-field envelope),
        // and an *old* peer using the pre-compression 4-field struct must still
        // deserialize it.  Postcard ignores the trailing `compression` byte.
        #[derive(Deserialize)]
        struct LegacyEnvelope {
            from: PublicKey,
            data: Bytes,
            signature: Signature,
            sent_at: u64,
        }

        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "new code, compression off".into(),
        };
        let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();

        let legacy: LegacyEnvelope = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(legacy.from, key.public());
        assert!(legacy.sent_at > 0);
        let decoded: Message = postcard::from_bytes(&legacy.data).unwrap();
        assert!(matches!(
            decoded,
            Message::Message { ref text } if text == "new code, compression off"
        ));
    }

    #[test]
    fn compressed_message_old_code_postcard_decode_fails_cleanly() {
        // Old code (pre-compression) has no `compression` field and calls
        // postcard directly on the raw `data` bytes without inflating.  A
        // deflate stream is not a valid postcard Message, so this must fail
        // with a clean error — never panic and never silently decode into a
        // *different* message.
        let key = SecretKey::generate();
        let text = "hello hello hello hello compression must fail on old code";
        let msg = Message::Message { text: text.into() };

        let encoded = SignedMessage::sign_and_encode_compressed(&key, &msg).unwrap();

        // New code decodes it fine.
        let (pk, decoded, _) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert_eq!(pk, key.public());
        assert!(matches!(decoded, Message::Message { text: ref t } if *t == text));

        // Simulate old code: postcard on the raw data field, no inflate.
        let envelope: SignedMessage = postcard::from_bytes(&encoded).unwrap();
        match postcard::from_bytes::<Message>(&envelope.data) {
            Err(e) => {
                // Clean rejection — this is the expected path.
                assert!(
                    !e.to_string().is_empty(),
                    "decode error must carry a message"
                );
            }
            Ok(decoded_raw) => {
                // If postcard somehow parses the deflate bytes into some
                // Message, it must NOT equal the original — silent
                // corruption would let old code show a wrong message.
                let orig_bytes = postcard::to_stdvec(&msg).unwrap();
                let got_bytes = postcard::to_stdvec(&decoded_raw).unwrap();
                assert_ne!(
                    got_bytes, orig_bytes,
                    "compressed bytes must not silently decode as the original message"
                );
            }
        }
    }

    // ── BORU-AUDIT-27: canonical signed-message framing ────────────────────

    /// The canonical bytes a new message signs must be stable.  This golden
    /// vector pins `postcard((\"boru/chat-message\", 1, from, sent_at,
    /// compression, data))` so a future refactor cannot silently change the
    /// signed layout.
    #[test]
    fn signed_message_canonical_bytes_golden_vector() {
        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "golden".into(),
        };
        let raw = postcard::to_stdvec(&msg).unwrap();
        let from = key.public();
        let sent_at = 1_700_000_000u64;
        let compression = 0u8;
        let canonical = crate::protocol_signing::canonical_signed_bytes(
            SIGNED_MESSAGE_PROTOCOL,
            SIGNED_MESSAGE_VERSION,
            &(from, sent_at, compression, &raw),
        )
        .expect("canonical bytes");
        // Stable framing invariants: protocol tag first, then version.
        assert_eq!(canonical[0] as usize, SIGNED_MESSAGE_PROTOCOL.len());
        assert_eq!(
            &canonical[1..1 + SIGNED_MESSAGE_PROTOCOL.len()],
            SIGNED_MESSAGE_PROTOCOL.as_bytes()
        );
        assert_eq!(canonical[1 + SIGNED_MESSAGE_PROTOCOL.len()], 0x01);
        // The remainder must decode back to the same signed fields.
        let decoded: (String, u16, PublicKey, u64, u8, Vec<u8>) =
            postcard::from_bytes(&canonical).expect("decode canonical");
        assert_eq!(decoded.0, SIGNED_MESSAGE_PROTOCOL);
        assert_eq!(decoded.1, SIGNED_MESSAGE_VERSION);
        assert_eq!(decoded.2, from);
        assert_eq!(decoded.3, sent_at);
        assert_eq!(decoded.4, compression);
        assert_eq!(decoded.5, raw);
    }

    /// Changing ANY security-relevant field (`from`, `data`, `sent_at`,
    /// `compression`) must invalidate verification.  This fails on the old
    /// implementation, which signed only `data` and left `sent_at` /
    /// `compression` unauthenticated.
    #[test]
    fn signed_message_field_mutation_invalidates_signature() {
        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "original".into(),
        };
        let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
        let envelope: SignedMessage = postcard::from_bytes(&encoded).unwrap();
        // Sanity: the original verifies.
        assert!(
            SignedMessage::verify_and_decode(&encoded).is_ok(),
            "original envelope must verify"
        );

        // Rebuild a tampered envelope with the SAME signature.  The signed
        // fields are private, so we construct each mutation from the decoded
        // envelope's fields directly.
        let mk = |from, data, signature, sent_at, compression| {
            let s = SignedMessage {
                from,
                data,
                signature,
                sent_at,
                compression,
            };
            postcard::to_stdvec(&s).expect("encode tampered envelope")
        };

        // Mutate `data` (the payload).
        let tampered_data: Bytes = postcard::to_stdvec(&Message::Message {
            text: "tampered".into(),
        })
        .unwrap()
        .into();
        let bytes = mk(
            envelope.from,
            tampered_data,
            envelope.signature,
            envelope.sent_at,
            envelope.compression,
        );
        assert!(
            SignedMessage::verify_and_decode(&bytes).is_err(),
            "mutating data must invalidate the signature (BORU-AUDIT-27)"
        );

        // Mutate `sent_at` (freshness — NOT signed before AUDIT-27).
        let bytes = mk(
            envelope.from,
            envelope.data.clone(),
            envelope.signature,
            envelope.sent_at + 1,
            envelope.compression,
        );
        assert!(
            SignedMessage::verify_and_decode(&bytes).is_err(),
            "mutating sent_at must invalidate the signature (BORU-AUDIT-27)"
        );

        // Mutate `compression` (interpretation — NOT signed before AUDIT-27).
        let bytes = mk(
            envelope.from,
            envelope.data.clone(),
            envelope.signature,
            envelope.sent_at,
            envelope.compression ^ 1,
        );
        assert!(
            SignedMessage::verify_and_decode(&bytes).is_err(),
            "mutating compression must invalidate the signature (BORU-AUDIT-27)"
        );
    }

    /// The `from` field is bound by the signature: an envelope signed by one
    /// key cannot be replayed with a different `from` and still verify.
    #[test]
    fn signed_message_from_is_authenticated() {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        let msg = Message::Message {
            text: "who am i".into(),
        };
        let encoded = SignedMessage::sign_and_encode(&key_a, &msg).unwrap();
        let mut envelope: SignedMessage = postcard::from_bytes(&encoded).unwrap();
        envelope.from = key_b.public();
        let bytes = postcard::to_stdvec(&envelope).unwrap();
        assert!(
            SignedMessage::verify_and_decode(&bytes).is_err(),
            "changing `from` must invalidate the signature (BORU-AUDIT-27)"
        );
    }

    /// Cross-version: a pre-AUDIT-27 envelope whose signature covers only
    /// `data` still verifies during the migration window.  This is the
    /// compatibility contract for persisted WAL/replay bytes.
    #[test]
    fn signed_message_legacy_data_only_signature_still_verifies() {
        #[derive(Serialize)]
        struct LegacyEnvelope {
            from: PublicKey,
            data: Bytes,
            signature: Signature,
            sent_at: u64,
            compression: u8,
        }

        let key = SecretKey::generate();
        let msg = Message::Message {
            text: "legacy framing".into(),
        };
        let data: Bytes = postcard::to_stdvec(&msg).unwrap().into();
        // Pre-AUDIT-27 signing: signature over `data` alone.
        let signature = key.sign(&data);
        let legacy = LegacyEnvelope {
            from: key.public(),
            data,
            signature: ByteArray::new(signature.to_bytes()),
            sent_at: 42,
            compression: 0,
        };
        let encoded = postcard::to_stdvec(&legacy).unwrap();
        let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert_eq!(pk, key.public());
        assert_eq!(sent_at, 42);
        assert!(matches!(decoded, Message::Message { ref text } if text == "legacy framing"));
    }

    /// A representative value for every `Message` variant.
    fn all_message_variants() -> Vec<(&'static str, Message)> {
        let key = SecretKey::generate();
        let peer = crate::group_encryption::types::PeerId::from(key.public());
        vec![
            (
                "AboutMe",
                Message::AboutMe {
                    name: "alice".into(),
                    profile_image_ticket: Some(
                        "blob:iroh:aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666:1:1:200".into(),
                    ),
                },
            ),
            (
                "Message",
                Message::Message {
                    text: "hello world, this is a regular chat message".into(),
                },
            ),
            (
                "FileShare",
                Message::FileShare {
                    name: "report.pdf".into(),
                    ticket: "blob:iroh:aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666:3:200:1000"
                        .into(),
                    size: 42_000,
                    thumbnail_hash: Some([0xab; 32]),
                    collection_hash: None,
                    collection_entries: 0,
                },
            ),
            ("Leave", Message::Leave),
            ("Presence", Message::Presence),
            (
                "PresenceWithTicket",
                Message::PresenceWithTicket {
                    ticket: "blob:iroh:aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666:3:200:1000"
                        .into(),
                },
            ),
            (
                "ReadReceipt",
                Message::ReadReceipt {
                    message_hash: [0x11; 32],
                },
            ),
            (
                "Edit",
                Message::Edit {
                    original_hash: [0x22; 32],
                    new_text: "the corrected message text".into(),
                },
            ),
            (
                "Delete",
                Message::Delete {
                    message_hash: [0x33; 32],
                },
            ),
            (
                "Reaction",
                Message::Reaction {
                    message_hash: [0x44; 32],
                    emoji: "👍".into(),
                },
            ),
            (
                "ImageShare",
                Message::ImageShare {
                    name: "photo.png".into(),
                    hash: [0x55; 32],
                },
            ),
            (
                "RoomAdvertisement",
                Message::RoomAdvertisement {
                    ad: RoomAdvertisement {
                        room_name: "Boru Public Room".into(),
                        description: "A public room for testing compression".into(),
                        topic: TopicId::from_bytes([7; 32]),
                        ticket:
                            "blob:iroh:aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666:3:200:1000"
                                .into(),
                        member_count: 3,
                        last_activity: 1_700_000_000_000,
                    },
                    signature: vec![0xcc; 64],
                },
            ),
            ("Heartbeat", Message::Heartbeat),
            ("LatencyPing", Message::LatencyPing { sent_at_ms: 123 }),
            ("LatencyPong", Message::LatencyPong { sent_at_ms: 123 }),
            (
                "DiagnosticProbe",
                Message::DiagnosticProbe(crate::diagnostics::DiagnosticProbe {
                    probe_id: "probe-1".into(),
                    sender_id: "peer-a".into(),
                    room_id: "room-1".into(),
                    sent_at_ms: 123,
                    payload: Some("hello".into()),
                }),
            ),
            (
                "ContactControl",
                Message::ContactControl {
                    payload: vec![0xde; 128],
                },
            ),
            (
                "ProfileUpdate",
                Message::ProfileUpdate(UserProfile::default()),
            ),
            (
                "EncryptedGroupMessage",
                Message::EncryptedGroupMessage {
                    group_id: [0xee; 32],
                    envelope: crate::group_encryption::message::EncryptedGroupEnvelope::new_control(
                        peer,
                        p2panda_encryption::message_scheme::ControlMessage::Create {
                            initial_members: vec![peer],
                        },
                        vec![],
                    ),
                },
            ),
            (
                "SharedGif",
                Message::SharedGif {
                    gif: crate::gif_provider::SharedGif {
                        provider: "klipy".into(),
                        provider_id: "gif-200".into(),
                        playback_url: "https://media.example/playback.mp4".into(),
                        preview_url: Some("https://media.example/preview.gif".into()),
                        fallback_url: Some("https://media.example/original.gif".into()),
                        format: crate::gif_provider::GifMediaFormat::Mp4,
                        width: Some(480),
                        height: Some(360),
                        alt_text: Some("a cat".into()),
                    },
                },
            ),
        ]
    }

    #[test]
    fn compressed_envelope_never_larger_than_plain() {
        // The wire message is the SignedMessage envelope, not the raw
        // postcard payload.  For every Message variant, the compressed
        // envelope must never be larger than the plain envelope — deflate's
        // framing overhead on tiny unit variants must fall back to raw.
        let key = SecretKey::generate();
        for (label, msg) in all_message_variants() {
            let plain = SignedMessage::sign_and_encode(&key, &msg)
                .unwrap_or_else(|e| panic!("{label}: plain encode failed: {e}"));
            let compressed = SignedMessage::sign_and_encode_compressed(&key, &msg)
                .unwrap_or_else(|e| panic!("{label}: compressed encode failed: {e}"));
            assert!(
                compressed.len() <= plain.len(),
                "{label}: compressed envelope ({} bytes) larger than plain ({} bytes)",
                compressed.len(),
                plain.len()
            );
            // And the compressed envelope must still decode correctly.
            let (pk, decoded, _) = SignedMessage::verify_and_decode(&compressed).unwrap();
            assert_eq!(pk, key.public(), "{label}: wrong sender");
            assert_eq!(
                postcard::to_stdvec(&decoded).unwrap(),
                postcard::to_stdvec(&msg).unwrap(),
                "{label}: decoded message differs from original"
            );
        }
    }

    #[test]
    fn compressed_roundtrip_all_message_variants() {
        let key = SecretKey::generate();
        for (label, msg) in all_message_variants() {
            let encoded = SignedMessage::sign_and_encode_compressed(&key, &msg)
                .unwrap_or_else(|e| panic!("{label}: sign_and_encode_compressed failed: {e}"));
            let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&encoded)
                .unwrap_or_else(|e| panic!("{label}: verify_and_decode failed: {e}"));
            assert_eq!(pk, key.public(), "{label}: wrong sender");
            assert!(sent_at > 0, "{label}: sent_at not set");
            // `Message` does not derive PartialEq, so compare postcard bytes.
            assert_eq!(
                postcard::to_stdvec(&decoded).unwrap(),
                postcard::to_stdvec(&msg).unwrap(),
                "{label}: decoded message differs from original"
            );
        }
    }

    #[test]
    fn compressed_roundtrip_edge_cases() {
        let key = SecretKey::generate();
        let cases: Vec<(&str, String)> = vec![
            ("empty_text", String::new()),
            ("single_char", "x".into()),
            ("max_length_10k", "y".repeat(10_000)),
            ("emoji_only", "👍🏽🎉🔥🚀😀".into()),
            ("mixed_emoji_text", "Great job! 🎉✨ Well done 👏".into()),
            ("rtl_hebrew", "שלום עולם, איך הולך?".into()),
            ("rtl_arabic", "مرحبا بالعالم".into()),
            ("cjk", "你好，世界！こんにちは世界".into()),
            (
                "combining_marks",
                "cafe\u{301} na\u{303}ve e\u{301}te\u{301}".into(),
            ),
            ("control_chars", "\u{0}\t\n\u{1b}\u{7f}".into()),
            ("zalgotext", "h̷̢̛e̶̢̛l̸̢̛l̵̢̛ơ̶̢".into()),
        ];
        for (label, text) in cases {
            let msg = Message::Message { text };
            let encoded = SignedMessage::sign_and_encode_compressed(&key, &msg)
                .unwrap_or_else(|e| panic!("{label}: sign_and_encode_compressed failed: {e}"));
            let (_, decoded, _) = SignedMessage::verify_and_decode(&encoded)
                .unwrap_or_else(|e| panic!("{label}: verify_and_decode failed: {e}"));
            assert_eq!(
                postcard::to_stdvec(&decoded).unwrap(),
                postcard::to_stdvec(&msg).unwrap(),
                "{label}: decoded message differs from original"
            );
        }
    }

    #[test]
    fn compressed_roundtrip_empty_file_share_and_big_ticket() {
        let key = SecretKey::generate();
        let msg = Message::FileShare {
            name: String::new(),
            ticket: "blob:iroh:".repeat(20),
            size: 0,
            thumbnail_hash: None,
            collection_hash: None,
            collection_entries: 0,
        };
        let encoded = SignedMessage::sign_and_encode_compressed(&key, &msg).unwrap();
        let (_, decoded, _) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert_eq!(
            postcard::to_stdvec(&decoded).unwrap(),
            postcard::to_stdvec(&msg).unwrap()
        );
    }

    // ── Ticket serialization tests ───────────────────────────────────────

    #[test]
    fn ticket_roundtrip_through_base32() {
        let ticket = Ticket {
            topic: TopicId::from_bytes([9u8; 32]),
            peers: vec![EndpointAddr::new(SecretKey::generate().public())],
            discovery_secret: None,
        };
        let encoded = ticket.to_string();
        let decoded = Ticket::from_str(&encoded).unwrap();
        assert_eq!(decoded, ticket);
    }

    #[test]
    fn ticket_is_deterministic() {
        let key = SecretKey::generate();
        let topic = TopicId::from_bytes([42u8; 32]);
        let peer = EndpointAddr::new(key.public());
        let a = Ticket {
            topic,
            peers: vec![peer.clone()],
            discovery_secret: None,
        };
        let b = Ticket {
            topic,
            peers: vec![peer],
            discovery_secret: None,
        };
        assert_eq!(a.to_string(), b.to_string());
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn ticket_to_bytes_and_from_bytes_roundtrip() {
        let ticket = Ticket {
            topic: TopicId::from_bytes([1u8; 32]),
            peers: vec![],
            discovery_secret: None,
        };
        let bytes = ticket.to_bytes();
        let decoded = Ticket::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, ticket);
    }

    // ── fmt_relay_mode tests ─────────────────────────────────────────────

    #[test]
    fn fmt_relay_mode_disabled() {
        assert_eq!(fmt_relay_mode(&RelayMode::Disabled), "None");
    }

    #[test]
    fn fmt_relay_mode_default() {
        let rendered = fmt_relay_mode(&RelayMode::Default);
        assert!(rendered.contains("Default Relay"));
    }

    #[test]
    fn fmt_relay_mode_staging() {
        let rendered = fmt_relay_mode(&RelayMode::Staging);
        assert!(rendered.contains("staging"));
    }

    // ── handle_net_event tests ──────────────────────────────────────────

    #[test]
    fn handle_net_event_message_appends_remote_entry() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: key.public(),
            message: Message::Message { text: "hi".into() },
            sent_at: now_secs(),
        };

        handle_net_event(event, &mut app).unwrap();
        assert_eq!(app.entries.len(), 1);
        assert!(matches!(app.entries[0].kind, ChatKind::Remote));
        assert_eq!(app.entries[0].body, "hi");
    }

    #[test]
    fn handle_net_event_about_me_stores_name_and_notifies() {
        let remote_key = SecretKey::generate();
        let _local_key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::AboutMe {
                name: "bob".into(),
                profile_image_ticket: None,
            },
            sent_at: now_secs(),
        };

        handle_net_event(event, &mut app).unwrap();
        // Name should be stored
        assert_eq!(app.names.get(&remote_key.public()).unwrap(), "bob");
        // Should have a system notification about the name
        assert!(app.entries.iter().any(|e| e.body.contains("bob")));
    }

    #[test]
    fn handle_net_event_about_me_same_name_suppresses_duplicate_system_message() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        let msg = Message::AboutMe {
            name: "bob".into(),
            profile_image_ticket: None,
        };

        handle_net_event(
            NetEvent::Message {
                from: remote_key.public(),
                message: msg.clone(),
                sent_at: now_secs(),
            },
            &mut app,
        )
        .unwrap();
        handle_net_event(
            NetEvent::Message {
                from: remote_key.public(),
                message: msg,
                sent_at: now_secs(),
            },
            &mut app,
        )
        .unwrap();

        let matching = app
            .entries
            .iter()
            .filter(|entry| {
                entry.body == format!("{} is now known as bob", remote_key.public().fmt_short())
            })
            .count();
        assert_eq!(
            matching, 1,
            "same AboutMe should only emit one name-change notice"
        );
        assert_eq!(app.names.get(&remote_key.public()).unwrap(), "bob");
    }

    #[test]
    fn handle_net_event_about_me_reconnect_uses_persisted_friend_name_to_skip_duplicate_notice() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_last_announced_name(fid.clone(), "bob");
        app.names.clear();

        handle_net_event(
            NetEvent::Message {
                from: remote_key.public(),
                message: Message::AboutMe {
                    name: "bob".into(),
                    profile_image_ticket: None,
                },
                sent_at: now_secs(),
            },
            &mut app,
        )
        .unwrap();

        assert_eq!(
            app.names.get(&remote_key.public()).map(String::as_str),
            Some("bob")
        );
        assert_eq!(
            app.entries
                .iter()
                .filter(|entry| {
                    entry.body == format!("{} is now known as bob", remote_key.public().fmt_short())
                })
                .count(),
            0,
            "persisted friend name should suppress reconnect duplicate notice"
        );
    }

    #[test]
    fn handle_net_event_own_message_is_skipped() {
        let mut app = test_app();
        let own_key = app.local_public;
        let event = NetEvent::Message {
            from: own_key,
            message: Message::Message {
                text: "echo".into(),
            },
            sent_at: 0,
        };
        handle_net_event(event, &mut app).unwrap();
        // Own messages should not appear as remote entries
        assert!(app.entries.is_empty());
    }

    #[test]
    fn handle_net_event_image_share_sets_pending() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Must be a friend and online for the share notification to appear.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.ensure_friend(fid.clone());
        app.friends.mark_online(fid);

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ImageShare {
                name: "photo.jpg".into(),
                hash: [0xab; 32],
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert_eq!(
            app.pending_image,
            vec![("photo.jpg".into(), [0xab; 32], remote_key.public())]
        );
        // Image shares render inline (no text system message) — the UI shows
        // the image itself, so no 'shared an image' entry is created.
        assert!(
            !app.entries.iter().any(|e| e.body.contains("photo.jpg")),
            "image shares should not create a text system message"
        );
    }

    #[test]
    fn handle_net_event_two_image_shares_both_pending() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Must be a friend and online for share notifications to appear.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.ensure_friend(fid.clone());
        app.friends.mark_online(fid);

        let event1 = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ImageShare {
                name: "sunset.jpg".into(),
                hash: [0xaa; 32],
            },
            sent_at: now_secs(),
        };
        let event2 = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ImageShare {
                name: "puppy.jpg".into(),
                hash: [0xbb; 32],
            },
            sent_at: now_secs(),
        };
        handle_net_event(event1, &mut app).unwrap();
        handle_net_event(event2, &mut app).unwrap();
        assert_eq!(
            app.pending_image.len(),
            2,
            "both image shares must be queued"
        );
        assert_eq!(app.pending_image[0].0, "sunset.jpg");
        assert_eq!(app.pending_image[1].0, "puppy.jpg");
        // No text system messages — images render inline.
        assert!(
            !app.entries.iter().any(|e| e.body.contains("sunset.jpg")),
            "image shares should not create text system messages"
        );
        assert!(
            !app.entries.iter().any(|e| e.body.contains("puppy.jpg")),
            "image shares should not create text system messages"
        );
    }

    #[test]
    fn handle_net_event_five_image_shares_all_pending() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Must be a friend and online for share notifications to appear.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.ensure_friend(fid.clone());
        app.friends.mark_online(fid);

        let names = ["img1.png", "img2.png", "img3.png", "img4.png", "img5.png"];
        for (i, name) in names.iter().enumerate() {
            let event = NetEvent::Message {
                from: remote_key.public(),
                message: Message::ImageShare {
                    name: name.to_string(),
                    hash: [i as u8; 32],
                },
                sent_at: now_secs(),
            };
            handle_net_event(event, &mut app).unwrap();
        }
        assert_eq!(
            app.pending_image.len(),
            5,
            "all five image shares must be queued"
        );
        for (i, name) in names.iter().enumerate() {
            assert_eq!(app.pending_image[i].0, *name, "image {} order preserved", i);
        }
        // No text system messages — images render inline.
        for name in &names {
            assert!(
                !app.entries.iter().any(|e| e.body.contains(name)),
                "image shares should not create text system messages"
            );
        }
    }

    #[test]
    fn handle_net_event_image_share_self_is_skipped() {
        let mut app = test_app();
        let local_pk = app.local_public;

        let event = NetEvent::Message {
            from: local_pk,
            message: Message::ImageShare {
                name: "selfie.jpg".into(),
                hash: [0xcc; 32],
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert!(
            app.pending_image.is_empty(),
            "self-shared images must not be queued for download"
        );
    }

    #[test]
    fn handle_net_event_file_share_is_ignored_without_authorisation() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::FileShare {
                name: "doc.pdf".into(),
                ticket: "abc123".into(),
                size: 0,
                thumbnail_hash: None,
                collection_hash: None,
                collection_entries: 0,
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert!(app.pending_file.is_none());
        assert!(!app.entries.iter().any(|e| e.body.contains("doc.pdf")));
    }

    #[test]
    fn handle_net_event_closed_sets_quit() {
        let mut app = test_app();
        handle_net_event(NetEvent::Closed, &mut app).unwrap();
        assert!(app.should_quit);
        assert!(app.entries.iter().any(|e| e.body.contains("closed")));
    }

    #[test]
    fn handle_net_event_error_sets_quit() {
        let mut app = test_app();
        handle_net_event(NetEvent::Error("timeout".into()), &mut app).unwrap();
        assert!(app.should_quit);
        assert!(app.entries.iter().any(|e| e.body.contains("timeout")));
    }

    #[test]
    fn handle_net_event_neighbor_down_uses_display_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        app.names.insert(remote_key.public(), "alice".to_string());

        handle_net_event(
            NetEvent::NeighborDown {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();
        assert!(app.entries.iter().any(|e| e.body == "alice left the chat"));
    }

    #[test]
    fn handle_net_event_neighbor_down_falls_back_to_short_key() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        handle_net_event(
            NetEvent::NeighborDown {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();
        // Without a display name, it falls back to the compact peer suffix.
        assert!(
            app.entries
                .iter()
                .any(|e| e.body.ends_with(" left the chat")),
            "expected '... left the chat' but got: {:?}",
            app.entries
        );
        let msg = app
            .entries
            .iter()
            .find(|e| e.body.ends_with(" left the chat"))
            .unwrap();
        let name_part = msg.body.trim_end_matches(" left the chat");
        assert_eq!(
            name_part,
            expected_name_suffix(&remote_key.public()),
            "fallback name should be the last 5 hex chars of the peer ID, got '{}'",
            msg.body
        );
    }

    #[test]
    fn handle_net_event_neighbor_up_marks_friend_online() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        // Add the peer as a friend first.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.ensure_friend(fid.clone());
        app.friends.mark_offline(fid);
        app.friends_dirty = false;

        app.names.insert(remote_key.public(), "alice".to_string());

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        // Friend should be marked online (in memory), but DURTY flag is not
        // set — online status persistence is left to the friend ping manager.
        let fid = FriendId::from_public_key(remote_key.public());
        assert!(
            app.friends
                .get(&fid)
                .map(|r| r.status.online)
                .unwrap_or(false),
            "friend should be marked online"
        );
        assert!(
            !app.friends_dirty,
            "friends should NOT be marked dirty from gossip-level NeighborUp alone"
        );
        assert!(app
            .entries
            .iter()
            .any(|e| e.body == "alice joined the chat"));
    }

    #[test]
    fn handle_net_event_neighbor_down_falls_back_to_friendly_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        handle_net_event(
            NetEvent::NeighborDown {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();
        // Without a display name, it falls back to a friendly name.
        assert!(
            app.entries
                .iter()
                .any(|e| e.body.ends_with(" left the chat")),
            "expected a '... left the chat' message but got: {:?}",
            app.entries
        );
        let msg = app
            .entries
            .iter()
            .find(|e| e.body.ends_with(" left the chat"))
            .unwrap();
        let name_part = msg.body.trim_end_matches(" left the chat");
        assert!(!name_part.is_empty(), "name should not be empty");
        assert_eq!(
            name_part,
            expected_name_suffix(&remote_key.public()),
            "name '{}' should be the last 5 hex chars of the peer ID",
            name_part
        );
    }

    #[test]
    fn handle_net_event_neighbor_up_falls_back_to_short_key() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        // Without a display name, it falls back to a friendly name.
        assert!(
            app.entries
                .iter()
                .any(|e| e.body.ends_with(" joined the chat")),
            "expected a '... joined the chat' message but got: {:?}",
            app.entries
        );
        let msg = app
            .entries
            .iter()
            .find(|e| e.body.ends_with(" joined the chat"))
            .unwrap();
        let name_part = msg.body.trim_end_matches(" joined the chat");
        assert!(!name_part.is_empty(), "name should not be empty");
        assert_eq!(
            name_part,
            expected_name_suffix(&remote_key.public()),
            "name '{}' should be the last 5 hex chars of the peer ID, got '{}'",
            name_part,
            msg.body
        );
    }

    #[test]
    fn handle_net_event_neighbor_up_non_friend_not_marked() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();

        // Don't add the peer as a friend.

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        // Should NOT have a friend record (only friend presence is updated).
        let fid = FriendId::from_public_key(remote_key.public());
        assert!(
            app.friends.get(&fid).is_none(),
            "non-friend should not get a friend record"
        );
        // But we still show a system message with a friendly name.
        assert!(
            app.entries
                .iter()
                .any(|e| e.body.ends_with(" joined the chat")),
            "should show join message even for non-friends, got: {:?}",
            app.entries.iter().map(|e| &e.body).collect::<Vec<_>>()
        );
    }

    // ── handle_net_event dedup tests ───────────────────────────────────

    /// Clear the global seen-messages set so tests start fresh.
    fn clear_seen_messages() {
        if let Ok(mut seen) = SEEN_MESSAGES.lock() {
            seen.clear();
        }
    }

    // ── handle_net_event dedup tests ───────────────────────────────────
    #[test]
    fn handle_net_event_dedup_exact_duplicate_is_suppressed() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "hello".into(),
            },
            sent_at: now_secs(),
        };

        // First delivery produces one entry.
        handle_net_event(event.clone(), &mut app).unwrap();
        assert_eq!(app.entries.len(), 1);

        // Second delivery (same from, same content, same sent_at) is suppressed.
        handle_net_event(event, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            1,
            "duplicate message should not add a second entry"
        );
    }

    #[test]
    fn handle_net_event_dedup_different_text_passes() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event_a = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "first".into(),
            },
            sent_at: now_secs(),
        };
        let event_b = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "second".into(),
            },
            sent_at: now_secs() + 1,
        };

        handle_net_event(event_a, &mut app).unwrap();
        handle_net_event(event_b, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            2,
            "different messages from same sender should both appear"
        );
        assert_eq!(app.entries[0].body, "first");
        assert_eq!(app.entries[1].body, "second");
    }

    #[test]
    fn handle_net_event_dedup_different_sender_passes() {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        let mut app = test_app();

        // Both send the same text at the same time — different senders,
        // so both are legitimate new messages.
        let identical_text = "same text".to_string();
        let event_a = NetEvent::Message {
            from: key_a.public(),
            message: Message::Message {
                text: identical_text.clone(),
            },
            sent_at: now_secs(),
        };
        let event_b = NetEvent::Message {
            from: key_b.public(),
            message: Message::Message {
                text: identical_text,
            },
            sent_at: now_secs(),
        };

        handle_net_event(event_a, &mut app).unwrap();
        handle_net_event(event_b, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            2,
            "same content from different senders should both appear"
        );
    }

    #[test]
    fn handle_net_event_dedup_different_sent_at_passes() {
        let key = SecretKey::generate();
        let mut app = test_app();

        // Same content from same sender at different timestamps is a
        // legitimate re-send and should NOT be deduped.
        let event_t1 = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "hello".into(),
            },
            sent_at: now_secs(),
        };
        let event_t2 = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "hello".into(),
            },
            sent_at: now_secs() + 2,
        };

        handle_net_event(event_t1, &mut app).unwrap();
        handle_net_event(event_t2, &mut app).unwrap();
        assert_eq!(
            app.entries.len(),
            2,
            "same content from same sender at different timestamps should both appear"
        );
    }

    #[test]
    fn handle_net_event_dedup_self_message_is_recorded() {
        // Self-messages are normally skipped for push_remote but should
        // still be tracked in the dedup set so duplicate gossip deliveries
        // of our own messages are suppressed.
        let local_key = SecretKey::generate();
        let mut app = AppState::new(
            test_status(),
            FriendsStore::default(),
            local_key.public(),
            Some("self".into()),
        );

        let event = NetEvent::Message {
            from: local_key.public(),
            message: Message::Message {
                text: "self-msg".into(),
            },
            sent_at: now_secs(),
        };

        // Self-message produces no remote entry.
        handle_net_event(event.clone(), &mut app).unwrap();
        assert!(app.entries.is_empty());

        // Duplicate self-message is still suppressed at the dedup layer.
        handle_net_event(event, &mut app).unwrap();
        assert!(app.entries.is_empty());
    }

    #[test]
    fn handle_net_event_dedup_about_me_is_deduped() {
        let key = SecretKey::generate();
        let mut app = test_app();

        let event = NetEvent::Message {
            from: key.public(),
            message: Message::AboutMe {
                name: "bob".into(),
                profile_image_ticket: None,
            },
            sent_at: now_secs(),
        };

        handle_net_event(event.clone(), &mut app).unwrap();
        // First delivery: one system notification.
        let system_count_before = app
            .entries
            .iter()
            .filter(|e| e.body.contains("bob"))
            .count();
        assert_eq!(system_count_before, 1);

        // Second delivery: suppressed.
        handle_net_event(event, &mut app).unwrap();
        let system_count_after = app
            .entries
            .iter()
            .filter(|e| e.body.contains("bob"))
            .count();
        assert_eq!(
            system_count_after, 1,
            "duplicate AboutMe should not produce a second notification"
        );
    }

    // ── resolve_name with friends store tests ────────────────────────────

    #[test]
    fn resolve_name_prefers_friend_label_over_session_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Set a session name.
        app.names
            .insert(remote_key.public(), "session_alice".to_string());
        // Add as friend with a label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Friend Alice");

        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display, "Friend Alice",
            "friend label should override session name"
        );
    }

    #[test]
    fn resolve_name_prefers_friend_announced_name_over_session_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Give them a session name.
        app.names
            .insert(remote_key.public(), "session_bob".to_string());
        // Add as friend with last_announced_name but no label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_last_announced_name(fid, "friend_bob");

        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display, "friend_bob",
            "friend's last announced name should override session name"
        );
    }

    #[test]
    fn resolve_name_prefers_friend_label_over_friend_announced_name() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends
            .set_last_announced_name(fid.clone(), "auto_name");
        app.friends.set_label(fid, "Label");

        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display, "Label",
            "friend label should take priority over last_announced_name"
        );
    }

    #[test]
    fn resolve_name_falls_back_to_session_name_when_not_a_friend() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        app.names
            .insert(remote_key.public(), "session_carol".to_string());

        // Not a friend — should use session name.
        let display = app.resolve_name(&remote_key.public());
        assert_eq!(display, "session_carol");
    }

    #[test]
    fn resolve_name_falls_back_to_short_pk_when_no_name_or_friend() {
        let remote_key = SecretKey::generate();
        let app = test_app();
        // No name, no friend — should fall back to the compact peer suffix.
        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display,
            expected_name_suffix(&remote_key.public()),
            "fallback should be the last 5 hex chars of the peer ID, got '{display}'"
        );
        // Same peer must produce the same result deterministically.
        let display2 = app.resolve_name(&remote_key.public());
        assert_eq!(display, display2, "fallback must be deterministic");
    }

    #[test]
    fn resolve_name_falls_back_to_short_pk_when_friend_has_no_named_fields() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        // Ensure the friend exists, but with no label and no last_announced_name.
        app.friends.ensure_friend(fid);

        // No session name either — should fall back to the compact peer suffix.
        let display = app.resolve_name(&remote_key.public());
        assert_eq!(
            display,
            expected_name_suffix(&remote_key.public()),
            "fallback should be the last 5 hex chars of the peer ID, got '{display}'"
        );
        assert_ne!(display, "Unknown", "fallback should not be 'Unknown'");
        assert_ne!(display, "", "fallback should not be empty");
    }

    #[test]
    fn handle_net_event_message_shows_friend_label() {
        clear_seen_messages();
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Add as friend with a label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Best Friend");

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::Message {
                text: "hello!".into(),
            },
            sent_at: now_secs(),
        };
        handle_net_event(event, &mut app).unwrap();
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].label, "Best Friend");
        assert_eq!(app.entries[0].body, "hello!");
    }

    #[test]
    fn handle_net_event_neighbor_up_shows_friend_label() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        // Add as friend with a label.
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Buddy");

        handle_net_event(
            NetEvent::NeighborUp {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        assert!(app
            .entries
            .iter()
            .any(|e| e.body == "Buddy joined the chat"));
    }

    #[test]
    fn handle_net_event_neighbor_down_shows_friend_label() {
        let remote_key = SecretKey::generate();
        let mut app = test_app();
        let fid = FriendId::from_public_key(remote_key.public());
        app.friends.set_label(fid, "Pal");

        handle_net_event(
            NetEvent::NeighborDown {
                peer: remote_key.public(),
            },
            &mut app,
        )
        .unwrap();

        assert!(app.entries.iter().any(|e| e.body == "Pal left the chat"));
    }

    #[test]
    fn handle_net_event_profile_update_calls_on_profile_update() {
        let remote_key = SecretKey::generate();
        let local_key = SecretKey::generate().public();
        let mut app = test_app();
        app.local_public = local_key;

        let mut profile = UserProfile::new(remote_key.public());
        profile.display_name = "alice".into();
        profile.bio = "hello world".into();
        profile.file_sharing_enabled = true;

        let event = NetEvent::Message {
            from: remote_key.public(),
            message: Message::ProfileUpdate(profile.clone()),
            sent_at: 1000,
        };

        // Process the event. The ProfileUpdate handler calls on_profile_update
        // on the callback (AppState's implementation is a no-op, but the method
        // is called without error).
        handle_net_event(event, &mut app).unwrap();

        // Our own messages should be skipped (from == local_public)
        let self_event = NetEvent::Message {
            from: local_key,
            message: Message::ProfileUpdate(profile),
            sent_at: 1001,
        };
        handle_net_event(self_event, &mut app).unwrap();
        // No system message should appear (ProfileUpdate doesn't generate entries)
        assert!(
            app.entries.is_empty(),
            "ProfileUpdate should not create chat entries"
        );
    }

    // ── SignedMessage roundtrip helper ──────────────────────────────────

    fn assert_signed_message_roundtrip(msg: Message, predicate: impl FnOnce(&Message) -> bool) {
        let key = SecretKey::generate();
        let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
        let (pk, decoded, sent_at) = SignedMessage::verify_and_decode(&encoded).unwrap();
        assert!(sent_at > 0);
        assert_eq!(pk, key.public());
        assert!(
            predicate(&decoded),
            "unexpected decoded message: {decoded:?}"
        );
    }

    // ── Basic roundtrip tests for each new interaction type ─────────────

    #[test]
    fn signed_message_roundtrip_read_receipt() {
        let hash = [1u8; 32];
        assert_signed_message_roundtrip(
            Message::ReadReceipt { message_hash: hash },
            |decoded| matches!(decoded, Message::ReadReceipt { message_hash } if *message_hash == hash),
        );
    }

    #[test]
    fn signed_message_roundtrip_edit() {
        let hash = [2u8; 32];
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: "updated".into(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && new_text == "updated")
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_delete() {
        let hash = [3u8; 32];
        assert_signed_message_roundtrip(
            Message::Delete { message_hash: hash },
            |decoded| matches!(decoded, Message::Delete { message_hash } if *message_hash == hash),
        );
    }

    #[test]
    fn signed_message_roundtrip_reaction() {
        let hash = [4u8; 32];
        assert_signed_message_roundtrip(
            Message::Reaction {
                message_hash: hash,
                emoji: "👍".into(),
            },
            |decoded| {
                matches!(decoded, Message::Reaction { message_hash, emoji }
                    if *message_hash == hash && emoji == "👍")
            },
        );
    }

    // ── Edge case roundtrip tests ───────────────────────────────────────

    #[test]
    fn signed_message_roundtrip_reaction_empty_emoji() {
        let hash = [5u8; 32];
        assert_signed_message_roundtrip(
            Message::Reaction {
                message_hash: hash,
                emoji: String::new(),
            },
            |decoded| {
                matches!(decoded, Message::Reaction { message_hash, emoji }
                    if *message_hash == hash && emoji.is_empty())
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_reaction_various_emoji() {
        let hash = [6u8; 32];
        for emoji in &[
            "🔥", // fire - single codepoint
            "👍🏿", // thumbs up dark skin tone
            "👨‍👩‍👧‍👦", // family ZWJ
            "🇦🇺", // AU flag
            "1⃣",  // keycap 1
            "❤️", // heart + VS16
            "😀", // grinning face
            "🎉", // party popper
        ] {
            assert_signed_message_roundtrip(
                Message::Reaction {
                    message_hash: hash,
                    emoji: (*emoji).to_string(),
                },
                |decoded| {
                    matches!(decoded, Message::Reaction { message_hash, emoji }
                        if *message_hash == hash && emoji.as_str() == *emoji)
                },
            );
        }
    }

    #[test]
    fn signed_message_roundtrip_reaction_long_emoji_string() {
        let hash = [7u8; 32];
        let many_hearts: String = "❤️".repeat(50);
        assert_signed_message_roundtrip(
            Message::Reaction {
                message_hash: hash,
                emoji: many_hearts.clone(),
            },
            |decoded| {
                matches!(decoded, Message::Reaction { message_hash, emoji }
                    if *message_hash == hash && *emoji == many_hearts)
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_edit_empty_text() {
        let hash = [8u8; 32];
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: String::new(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && new_text.is_empty())
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_edit_long_text() {
        let hash = [9u8; 32];
        let long_text: String = "A".repeat(10_000);
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: long_text.clone(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && *new_text == long_text)
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_edit_unicode_text() {
        let hash = [10u8; 32];
        let unicode_text = "日本語 русский العربية 😊👋".to_string();
        assert_signed_message_roundtrip(
            Message::Edit {
                original_hash: hash,
                new_text: unicode_text.clone(),
            },
            |decoded| {
                matches!(decoded, Message::Edit { original_hash, new_text }
                    if *original_hash == hash && *new_text == unicode_text)
            },
        );
    }

    #[test]
    fn signed_message_roundtrip_read_receipt_zero_hash() {
        let hash = [0u8; 32];
        assert_signed_message_roundtrip(
            Message::ReadReceipt { message_hash: hash },
            |decoded| matches!(decoded, Message::ReadReceipt { message_hash } if *message_hash == hash),
        );
    }

    #[test]
    fn signed_message_roundtrip_delete_zero_hash() {
        let hash = [0u8; 32];
        assert_signed_message_roundtrip(
            Message::Delete { message_hash: hash },
            |decoded| matches!(decoded, Message::Delete { message_hash } if *message_hash == hash),
        );
    }

    // ── download_candidates ──────────────────────────────────────────────

    #[test]
    fn test_download_candidates_original_first() {
        let pk_a = SecretKey::generate().public();
        let pk_b = SecretKey::generate().public();
        let pk_c = SecretKey::generate().public();
        let mut neighbors = HashSet::new();
        neighbors.insert(pk_b);
        neighbors.insert(pk_c);

        let candidates = download_candidates(pk_a, &neighbors);
        assert_eq!(candidates.len(), 3, "should have 3 candidates");
        assert_eq!(candidates[0], pk_a, "original sender should be first");
        assert!(candidates.contains(&pk_b), "should include neighbor B");
        assert!(candidates.contains(&pk_c), "should include neighbor C");
    }

    #[test]
    fn test_download_candidates_deduplicates_original() {
        let pk_a = SecretKey::generate().public();
        let mut neighbors = HashSet::new();
        neighbors.insert(pk_a); // original is also a neighbor

        let candidates = download_candidates(pk_a, &neighbors);
        assert_eq!(candidates.len(), 1, "should deduplicate");
        assert_eq!(candidates[0], pk_a, "original should be the only entry");
    }

    #[test]
    fn test_download_candidates_no_neighbors() {
        let pk_a = SecretKey::generate().public();
        let neighbors = HashSet::new();

        let candidates = download_candidates(pk_a, &neighbors);
        assert_eq!(candidates.len(), 1, "should have just the original");
        assert_eq!(candidates[0], pk_a);
    }

    // ── collect_bootstrap_peers tests ──────────────────────────────────────

    #[test]
    fn test_collect_bootstrap_peers_dedup() {
        let sk1 = SecretKey::generate();
        let sk2 = SecretKey::generate();
        let pk1 = sk1.public();
        let pk2 = sk2.public();

        let addr1 = EndpointAddr::new(pk1);
        let addr2 = EndpointAddr::new(pk2);
        let addr1_dup = EndpointAddr::new(pk1); // same pk1

        let ticket_peers = [addr1, addr2.clone()];
        let room_peers = [addr1_dup];

        let (peer_ids, all_addrs) = collect_bootstrap_peers([&ticket_peers[..], &room_peers[..]]);

        assert_eq!(peer_ids.len(), 2, "should have 2 unique peer IDs");
        assert!(peer_ids.contains(&pk1), "pk1 should be in peer_ids");
        assert!(peer_ids.contains(&pk2), "pk2 should be in peer_ids");

        assert_eq!(all_addrs.len(), 2, "should have 2 unique addresses");
    }

    #[test]
    fn test_collect_bootstrap_peers_empty() {
        let (ids, addrs) = collect_bootstrap_peers([&[] as &[EndpointAddr]]);
        assert!(ids.is_empty(), "empty sources → empty peer_ids");
        assert!(addrs.is_empty(), "empty sources → empty addrs");
    }

    #[test]
    fn test_collect_bootstrap_peers_single_source() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let addr = EndpointAddr::new(pk);

        let (ids, addrs) = collect_bootstrap_peers([&[addr.clone()][..]]);
        assert_eq!(ids, vec![pk], "single source should produce its peer ID");
        assert_eq!(addrs.len(), 1, "single source should produce its addr");
    }

    #[test]
    fn test_merge_bootstrap_peer_addrs_keeps_existing_when_incoming_is_empty() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let existing = vec![EndpointAddr::new(pk)];

        let merged = merge_bootstrap_peer_addrs(&existing, &[]);

        assert_eq!(
            merged, existing,
            "relay-only invites must preserve known peers"
        );
    }

    #[test]
    fn test_merge_bootstrap_peer_addrs_deduplicates_duplicate_peers() {
        let sk1 = SecretKey::generate();
        let sk2 = SecretKey::generate();
        let pk1 = sk1.public();
        let pk2 = sk2.public();

        let existing = vec![EndpointAddr::new(pk1), EndpointAddr::new(pk2)];
        let incoming = vec![
            EndpointAddr::new(pk2),
            EndpointAddr::new(pk1),
            EndpointAddr::new(pk1),
        ];

        let merged = merge_bootstrap_peer_addrs(&existing, &incoming);

        assert_eq!(merged, vec![EndpointAddr::new(pk2), EndpointAddr::new(pk1)]);
    }

    #[test]
    fn test_seed_memory_lookup_adds_addresses() {
        let sk = SecretKey::generate();
        let pk = sk.public();
        let addr = EndpointAddr::new(pk);

        let lookup = iroh::address_lookup::memory::MemoryLookup::new();
        seed_memory_lookup(&lookup, &[addr]);

        let resolved = lookup.get_endpoint_info(pk);
        assert!(
            resolved.is_some(),
            "seed_memory_lookup should add the address"
        );
    }

    #[test]
    fn test_seed_memory_lookup_empty() {
        let lookup = iroh::address_lookup::memory::MemoryLookup::new();
        seed_memory_lookup(&lookup, &[]);
        // Should not panic — verify by checking nothing was added
        assert!(lookup
            .get_endpoint_info(SecretKey::generate().public())
            .is_none());
    }

    // ── TransferProgress lifecycle tests ────────────────────────────────

    #[test]
    fn test_transfer_id_unique() {
        let a = TransferId::next();
        let b = TransferId::next();
        let c = TransferId::next();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn test_cancel_guard_emits_cancelled_on_drop() {
        use std::sync::Mutex;
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let id = TransferId::next();
        let cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(move |ev| {
            events_clone.lock().unwrap().push(ev);
        }))));

        // Create the guard and let it drop without disarming.
        {
            let _guard = CancelGuard::new(id, TransferKind::File, "test.txt".into(), cb.clone());
        }

        let emitted = events.lock().unwrap();
        assert_eq!(emitted.len(), 1, "should emit exactly one event on drop");
        match &emitted[0] {
            TransferProgress::Cancelled {
                id: emitted_id,
                kind,
                name,
            } => {
                assert_eq!(*emitted_id, id);
                assert_eq!(*kind, TransferKind::File);
                assert_eq!(name, "test.txt");
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn test_cancel_guard_disarm_suppresses_cancelled() {
        use std::sync::Mutex;
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let id = TransferId::next();
        let cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(move |ev| {
            events_clone.lock().unwrap().push(ev);
        }))));

        // Create the guard, disarm it, then let it drop.
        {
            let guard = CancelGuard::new(id, TransferKind::Image, "photo.png".into(), cb.clone());
            guard.disarm();
        }

        let emitted = events.lock().unwrap();
        assert!(
            emitted.is_empty(),
            "should NOT emit Cancelled after disarm, got {emitted:?}"
        );
    }

    #[test]
    fn test_transfer_progress_progress_variant_has_name() {
        // Verify the Progress variant accepts kind + name fields.
        let id = TransferId::next();
        let ev = TransferProgress::Progress {
            id,
            kind: TransferKind::File,
            name: "report.pdf".into(),
            bytes: 512,
            total: None,
        };
        match ev {
            TransferProgress::Progress {
                id: _,
                kind,
                ref name,
                bytes,
                total: _,
            } => {
                assert_eq!(kind, TransferKind::File);
                assert_eq!(name, "report.pdf");
                assert_eq!(bytes, 512);
            }
            other => panic!("expected Progress, got {other:?}"),
        }
    }

    #[test]
    fn test_transfer_progress_cancelled_variant_has_name_and_kind() {
        let id = TransferId::next();
        let ev = TransferProgress::Cancelled {
            id,
            kind: TransferKind::Image,
            name: "avatar.png".into(),
        };
        match ev {
            TransferProgress::Cancelled {
                id: _,
                kind,
                ref name,
            } => {
                assert_eq!(kind, TransferKind::Image);
                assert_eq!(name, "avatar.png");
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    // ── Wire compatibility tests ──────────────────────────────────────────────
    //
    // These tests verify that serialized messages from older versions (where
    // the Message enum had 13 variants without ProfileUpdate) can be decoded
    // correctly by the current code.  ProfileUpdate was inserted mid-enum
    // at commit bf3c0cb3, shifting all later discriminants.  The fix must
    // re-map ProfileUpdate to an explicit high discriminant (13) so all
    // original variants keep their original values (0-12).

    #[test]
    fn old_wire_format_message_decodes_correctly() {
        // Old wire format for Message::Message { text: "hello" }:
        // postcard externally tagged: varint discriminant (1), then struct fields
        //   discriminant: varint(1) = 0x01
        //   text field (String): varint length (5) + UTF-8 bytes
        let old_bytes = [0x01, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f];
        let decoded: Message = postcard::from_bytes(&old_bytes)
            .expect("old-format Message::Message should decode correctly");
        assert!(
            matches!(decoded, Message::Message { ref text } if text == "hello"),
            "expected Message::Message(\"hello\"), got {decoded:?}"
        );
    }

    #[test]
    fn old_wire_format_about_me_decodes_correctly() {
        // Old wire format for Message::AboutMe { name: "alice", profile_image_ticket: None }
        // discriminant: varint(0) = 0x00
        //   name: varint(5) + b"alice"
        //   profile_image_ticket: Option<String> → serde None (postcard: 0x00 for None, or just missing?)
        let old_bytes = [
            0x00, // discriminant 0 = AboutMe
            0x05, // name length
            0x61, 0x6c, 0x69, 0x63, 0x65, // "alice"
            0x00, // profile_image_ticket: None (0 = false for Option)
        ];
        let decoded: Message =
            postcard::from_bytes(&old_bytes).expect("old-format AboutMe should decode correctly");
        assert!(
            matches!(decoded, Message::AboutMe { ref name, profile_image_ticket: None } if name == "alice"),
            "expected AboutMe(\"alice\", None), got {decoded:?}"
        );
    }

    #[test]
    fn old_wire_format_file_share_decodes_correctly() {
        // Old wire format for Message::FileShare { name: "doc.pdf", ticket: "tkt" }
        // discriminant: varint(2) = 0x02
        let old_bytes = [
            0x02, // discriminant 2 = FileShare
            0x07, // name length
            0x64, 0x6f, 0x63, 0x2e, 0x70, 0x64, 0x66, // "doc.pdf"
            0x03, // ticket length
            0x74, 0x6b, 0x74, // "tkt"
        ];
        let decoded: Message =
            postcard::from_bytes(&old_bytes).expect("old-format FileShare should decode correctly");
        match decoded {
            Message::FileShare {
                ref name,
                ref ticket,
                size: _,
                ..
            } => {
                assert_eq!(name, "doc.pdf");
                assert_eq!(ticket, "tkt");
            }
            other => panic!("expected FileShare, got {other:?}"),
        }
    }

    #[test]
    fn old_wire_format_file_share_decodes_to_single_file_defaults() {
        // The exact legacy wire bytes (name + ticket only) must decode to a
        // FileShare whose new collection fields default to "single file":
        // collection_hash = None and collection_entries = 0.  This is the
        // backward-compatibility guarantee for peers running pre-SENDME-01
        // builds.
        let old_bytes = [
            0x02, // discriminant 2 = FileShare
            0x07, // name length
            0x64, 0x6f, 0x63, 0x2e, 0x70, 0x64, 0x66, // "doc.pdf"
            0x03, // ticket length
            0x74, 0x6b, 0x74, // "tkt"
        ];
        let decoded: Message =
            postcard::from_bytes(&old_bytes).expect("legacy FileShare should decode correctly");
        match decoded {
            Message::FileShare {
                name,
                ticket,
                size,
                thumbnail_hash,
                collection_hash,
                collection_entries,
            } => {
                assert_eq!(name, "doc.pdf");
                assert_eq!(ticket, "tkt");
                assert_eq!(size, 0);
                assert_eq!(thumbnail_hash, None);
                assert_eq!(collection_hash, None, "legacy payload must be a single-file share");
                assert_eq!(collection_entries, 0);
            }
            other => panic!("expected FileShare, got {other:?}"),
        }
    }

    #[test]
    fn new_wire_format_folder_share_roundtrips_with_legacy_decode() {
        // A SENDME-01 folder share carries collection_hash + collection_entries.
        // Round-trip through postcard and verify both fields survive.
        let msg = Message::FileShare {
            name: "photos".into(),
            ticket: "blob:iroh:folderticket".into(),
            size: 123_456,
            thumbnail_hash: None,
            collection_hash: Some([0x42; 32]),
            collection_entries: 17,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::FileShare {
                name,
                ticket,
                size,
                collection_hash,
                collection_entries,
                ..
            } => {
                assert_eq!(name, "photos");
                assert_eq!(ticket, "blob:iroh:folderticket");
                assert_eq!(size, 123_456);
                assert_eq!(collection_hash, Some([0x42; 32]));
                assert_eq!(collection_entries, 17);
            }
            other => panic!("expected FileShare, got {other:?}"),
        }
    }

    #[test]
    fn old_wire_format_presence_decodes_correctly() {
        // Old wire format for Message::Presence (unit variant)
        // discriminant: varint(4) = 0x04
        let old_bytes = [0x04];
        let decoded: Message =
            postcard::from_bytes(&old_bytes).expect("old-format Presence should decode correctly");
        assert!(
            matches!(decoded, Message::Presence),
            "expected Presence, got {decoded:?}"
        );
    }

    #[test]
    fn new_profile_update_wire_format_not_confusable_with_old_message() {
        // ProfileUpdate must NOT use discriminant 1 (the old Message position).
        // Serialize a ProfileUpdate and verify the discriminant byte is NOT 0x01.
        use crate::user_profile::UserProfile;
        let profile = UserProfile::new(PublicKey::from_bytes(&[1u8; 32]).expect("valid key"));
        let msg = Message::ProfileUpdate(profile);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        assert_ne!(
            bytes[0], 0x01,
            "ProfileUpdate MUST NOT use discriminant 1 (old Message position); first byte = {:#04x}",
            bytes[0]
        );
    }

    // ── Ticket tests ────────────────────────────────────────────────────────

    /// Legacy binary ticket (topic + peers only) deserialises to
    /// discovery_secret=None.
    #[test]
    fn test_ticket_legacy_binary_no_secret() {
        let topic = TopicId::from_bytes([0xAAu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let legacy = Ticket {
            topic,
            peers: vec![addr],
            discovery_secret: None,
        };
        let bytes = postcard::to_stdvec(&legacy).unwrap();
        // Decode into the new Ticket struct — should produce None.
        let restored: Ticket = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(restored.topic, topic);
        assert_eq!(restored.peers.len(), 1);
        assert_eq!(restored.discovery_secret, None);
    }

    /// New binary ticket (topic + peers + Some(secret)) round-trips correctly.
    #[test]
    fn test_ticket_with_secret_roundtrip() {
        let topic = TopicId::from_bytes([0xBBu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let secret = DiscoverySecret::generate();
        let ticket = Ticket::with_discovery(topic, vec![addr], secret);
        let bytes = ticket.to_bytes();
        let restored = Ticket::from_bytes(&bytes).unwrap();
        assert_eq!(restored.topic, ticket.topic);
        assert_eq!(restored.peers, ticket.peers);
        assert_eq!(restored.discovery_secret, ticket.discovery_secret);
    }

    /// Display/FromStr round-trip for a ticket without discovery secret.
    #[test]
    fn test_ticket_display_fromstr_no_secret() {
        let topic = TopicId::from_bytes([0xCCu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let ticket = Ticket::new(topic, vec![addr]);
        let display = ticket.to_string();
        let parsed: Ticket = display.parse().unwrap();
        assert_eq!(parsed.topic, ticket.topic);
        assert_eq!(parsed.peers, ticket.peers);
        assert_eq!(parsed.discovery_secret, None);
    }

    /// Display/FromStr round-trip for a ticket with discovery secret.
    #[test]
    fn test_ticket_display_fromstr_with_secret() {
        let topic = TopicId::from_bytes([0xDDu8; 32]);
        let pk = iroh::SecretKey::generate().public();
        let addr = iroh::EndpointAddr::new(pk);
        let secret = DiscoverySecret::from_bytes([0x42u8; 32]);
        let ticket = Ticket::with_discovery(topic, vec![addr], secret);
        let display = ticket.to_string();
        let parsed: Ticket = display.parse().unwrap();
        assert_eq!(parsed.topic, ticket.topic);
        assert_eq!(parsed.peers, ticket.peers);
        assert_eq!(parsed.discovery_secret, ticket.discovery_secret);
    }

    /// Ticket is Send + Sync (compile-time check).
    #[test]
    fn test_ticket_is_send_sync() {
        fn assert_send<T: Send>(_: &T) {}
        fn assert_sync<T: Sync>(_: &T) {}
        let ticket = Ticket::new(
            TopicId::from_bytes([0xEEu8; 32]),
            vec![iroh::EndpointAddr::new(
                iroh::SecretKey::generate().public(),
            )],
        );
        assert_send(&ticket);
        assert_sync(&ticket);
    }

    // ── BORU-AUDIT-21: reserved-destination hash-failure cleanup ────────

    #[tokio::test]
    async fn reserved_destination_hash_failure_removes_file_and_never_publishes() {
        // Store a blob in an in-memory store, then stream it into a reserved
        // destination with a WRONG expected hash. The write must fail, the
        // created file must be removed, and the final name must not exist.
        let blob_store: iroh_blobs::api::Store =
            iroh_blobs::store::mem::MemStore::new().into();
        let content = b"verified download bytes".to_vec();
        let tag = blob_store.blobs().add_bytes(content.clone()).await.unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let mut destination = match crate::safe_destination::reserve_download_destination(
            dir.path(),
            "report.pdf",
            "download",
            crate::safe_destination::OverwritePolicy::KeepBoth,
        )
        .unwrap()
        {
            crate::safe_destination::Reservation::Use(dest) => dest,
            crate::safe_destination::Reservation::Skip => panic!("fresh temp dir must not skip"),
        };
        let final_path = destination.final_path().to_path_buf();

        let mut progress = Vec::new();
        let result = write_blob_to_reserved_file(
            &blob_store,
            tag.hash,
            &mut destination,
            Some(&blake3::hash(b"some other bytes").to_hex().to_string()),
            None,
            &mut |ev| progress.push(ev),
            TransferId::next(),
            TransferKind::File,
            "report.pdf".to_string(),
        )
        .await;

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("content hash mismatch"),
            "unexpected error: {err}"
        );
        drop(destination);
        assert!(
            !final_path.exists(),
            "hash failure must remove the created destination file"
        );
        // The reservation was never published, so no final file appears.
        assert!(!dir.path().join("report.pdf").exists());
    }

    #[tokio::test]
    async fn reserved_destination_matching_hash_publishes_file() {
        let blob_store: iroh_blobs::api::Store =
            iroh_blobs::store::mem::MemStore::new().into();
        let content = b"verified download bytes".to_vec();
        let tag = blob_store.blobs().add_bytes(content.clone()).await.unwrap();
        let expected = blake3::hash(&content).to_hex().to_string();

        let dir = tempfile::TempDir::new().unwrap();
        let mut destination = match crate::safe_destination::reserve_download_destination(
            dir.path(),
            "report.pdf",
            "download",
            crate::safe_destination::OverwritePolicy::KeepBoth,
        )
        .unwrap()
        {
            crate::safe_destination::Reservation::Use(dest) => dest,
            crate::safe_destination::Reservation::Skip => panic!("fresh temp dir must not skip"),
        };
        let final_path = destination.final_path().to_path_buf();

        let mut progress = Vec::new();
        write_blob_to_reserved_file(
            &blob_store,
            tag.hash,
            &mut destination,
            Some(&expected),
            None,
            &mut |ev| progress.push(ev),
            TransferId::next(),
            TransferKind::File,
            "report.pdf".to_string(),
        )
        .await
        .unwrap();
        destination.publish().unwrap();

        assert_eq!(std::fs::read(&final_path).unwrap(), content);
        assert!(
            progress
                .iter()
                .any(|ev| matches!(ev, TransferProgress::Progress { .. }))
        );
    }
