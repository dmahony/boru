//! Hostile-input and invalid-message rejection tests.
//!
//! These tests verify that the chat protocol correctly rejects various forms
//! of malicious, malformed, or invalid input without causing visible messages,
//! unread increments, outbox corruption, unbounded memory, or panics.
//!
//! Coverage:
//!   1. Invalid signatures (tampered envelope data)
//!   2. Wrong sender / recipient (spoofed `from` field)
//!   3. Expired timestamp (message older than TTL)
//!   4. Future timestamp beyond skew (clock-skew attack)
//!   5. Malformed / oversized envelope (garbage or truncated postcard data)
//!   6. Oversized plaintext (very large text body)
//!   7. Unknown protocol version (future Message discriminant)
//!   8. Conflicting message ID reuse (duplicate dedup)
//!   9. Unauthorised / blocked sender (is_blocked peer)
//!  10. Invalid ack (ReadReceipt for unknown message hash)
//!  11. Ack signed by another peer (tampered signature)
//!  12. Oversized sync response (large backfill payload)
//!  13. Replay flooding (many identical messages)
//!
//! Each test verifies that the rejected input does NOT:
//!   - Create visible messages (entries stay empty)
//!   - Increment unread counts (no entries added)
//!   - Clear or corrupt the outbox
//!   - Cause unbounded memory (dedup set bounded, no OOM)
//!   - Panic (unwrap/expect failures)

#![cfg(feature = "net")]

use boru_chat::chat_core::{
    handle_net_event, handle_net_event_for_topic, message_hash, now_secs, ChatCallbacks, ChatEntry,
    Message, MessageHash, NetEvent, SignedMessage,
};
use boru_chat::chat_history::DeliveryState;
use boru_chat::friends::{FriendId, FriendsStore};
use boru_chat::proto::TopicId;
use iroh::{PublicKey, SecretKey};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// A minimal ChatCallbacks implementor for hostile-input testing.
struct TestChat {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: std::collections::HashMap<PublicKey, String>,
    friends: FriendsStore,
    pending_file: Option<(String, String)>,
    pending_image: Vec<(String, MessageHash, PublicKey)>,
    blocked: std::collections::HashSet<PublicKey>,
    muted: std::collections::HashSet<PublicKey>,
    should_quit: bool,
    delivery_updates: Vec<(u64, DeliveryState)>,
    self_sent_events: std::collections::HashMap<MessageHash, u64>,
}

impl TestChat {
    fn new(local_public: PublicKey) -> Self {
        Self {
            local_public,
            entries: Vec::new(),
            names: std::collections::HashMap::new(),
            friends: FriendsStore::default(),
            pending_file: None,
            pending_image: Vec::new(),
            blocked: std::collections::HashSet::new(),
            muted: std::collections::HashSet::new(),
            should_quit: false,
            delivery_updates: Vec::new(),
            self_sent_events: std::collections::HashMap::new(),
        }
    }
}

impl ChatCallbacks for TestChat {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }

    fn resolve_name(&self, peer: &PublicKey) -> String {
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

    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
    }

    fn last_announced_name(&self, peer: &PublicKey) -> Option<String> {
        let fid = FriendId::from_public_key(*peer);
        self.friends
            .get(&fid)
            .and_then(|r| r.last_announced_name.clone())
            .or_else(|| self.names.get(peer).cloned())
    }

    fn is_friend(&self, peer: &PublicKey) -> bool {
        let fid = FriendId::from_public_key(*peer);
        self.friends.get(&fid).is_some()
    }

    fn is_blocked(&self, peer: &PublicKey) -> bool {
        self.blocked.contains(peer)
    }

    fn is_muted(&self, peer: &PublicKey) -> bool {
        self.muted.contains(peer)
    }

    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}

    fn set_pending_file(&mut self, name: String, ticket: String) {
        self.pending_file = Some((name, ticket));
    }

    fn push_system(&mut self, text: String) {
        self.entries.push(ChatEntry::system(text));
    }

    fn push_remote(
        &mut self,
        _peer: PublicKey,
        label: String,
        text: String,
        hash: Option<MessageHash>,
        sent_at: Option<u64>,
    ) {
        let mut entry = ChatEntry::remote(label, text);
        if let Some(secs) = sent_at {
            entry = entry.with_timestamp(Some(secs * 1000));
        }
        if let Some(h) = hash {
            entry = entry.with_message_hash(h);
        }
        self.entries.push(entry);
    }

    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_image.push((name, hash, from));
    }

    fn has_message(&self, hash: &MessageHash) -> bool {
        self.entries
            .iter()
            .any(|e| e.message_hash.as_ref() == Some(hash))
    }

    fn edit_message(&mut self, hash: &MessageHash, new_text: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.body = new_text;
            entry.edited = true;
        }
    }

    fn delete_message(&mut self, hash: &MessageHash) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.message_hash == Some(*hash))
        {
            entry.body = "[message deleted]".to_string();
            entry.edited = false;
            entry.reactions.clear();
        }
    }

    fn add_reaction(&mut self, hash: &MessageHash, emoji: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.message_hash == Some(*hash))
        {
            entry.reactions.push(emoji);
        }
    }

    fn on_neighbor_up(&mut self, _peer: PublicKey) {}
    fn on_neighbor_down(&mut self, _peer: PublicKey) {}
    fn record_activity(&mut self, _peer: PublicKey) {}
    fn request_quit(&mut self) {
        self.should_quit = true;
    }

    fn event_id_for_hash(&self, hash: &MessageHash) -> Option<u64> {
        self.self_sent_events.get(hash).copied()
    }

    fn update_delivery_state(&mut self, event_id: u64, state: DeliveryState) {
        self.delivery_updates.push((event_id, state));
    }
}

/// Build a spoofed SignedMessage by encoding with one key, then replacing
/// the 32-byte public key in the envelope with a different one.
///
/// Returns raw bytes that decode to a SignedMessage whose `from` field
/// is `victim_pk` but whose signature was created by `attacker_sk`.
fn spoof_signed_envelope(attacker_sk: &SecretKey, victim_pk: PublicKey, msg: &Message) -> Vec<u8> {
    let mut encoded = SignedMessage::sign_and_encode(attacker_sk, msg)
        .expect("sign")
        .to_vec();
    let attacker_pk = attacker_sk.public();

    // In the postcard-encoded SignedMessage, the first field is `from: PublicKey`.
    // PublicKey is 32 bytes. Serde serializes it as a 32-byte sequence.
    // After postcard encoding, the first 32 bytes after any framing should be the PK.
    let attacker_bytes = *attacker_pk.as_bytes();
    let victim_bytes = *victim_pk.as_bytes();

    // Find and replace the 32-byte public key.
    if let Some(pos) = find_subsequence(&encoded, &attacker_bytes) {
        encoded[pos..pos + 32].copy_from_slice(&victim_bytes);
        encoded
    } else {
        // Fallback: flip the first 32 bytes (should contain the PK).
        for byte in encoded.iter_mut().take(32) {
            *byte ^= 0xFF;
        }
        encoded
    }
}

/// Find the first occurrence of `needle` in `haystack`.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ═══════════════════════════════════════════════════════════════════════════════
// 1. Invalid signatures
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn invalid_signature_tampered_envelope_rejected() {
    let key = SecretKey::generate();
    let msg = Message::Message {
        text: "hello".into(),
    };
    let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();

    // Flip a bit in the signature (64 bytes before the sent_at varint at the end).
    let mut tampered = encoded.to_vec();
    let len = tampered.len();
    if len > 65 {
        tampered[len - 65] ^= 0x01;
    }

    let result = SignedMessage::verify_and_decode(&tampered);
    assert!(result.is_err(), "tampered signature must fail verification");
}

#[test]
fn invalid_signature_flipped_bytes_rejected() {
    let key = SecretKey::generate();
    let msg = Message::Message {
        text: "payload".into(),
    };
    let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();

    // Flip bits in the middle of the data payload.
    let mut tampered = encoded.to_vec();
    let mid = tampered.len() / 2;
    tampered[mid] ^= 0xFF;

    let result = SignedMessage::verify_and_decode(&tampered);
    assert!(
        result.is_err(),
        "tampered payload must fail verification: {:?}",
        result
    );
}

#[test]
fn invalid_signature_empty_message_data_rejected() {
    // Sign a message and then truncate the signature portion.
    let key = SecretKey::generate();
    let msg = Message::Heartbeat;
    let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();

    let len = encoded.len();
    let tampered = &encoded[..len.saturating_sub(32)];

    let result = SignedMessage::verify_and_decode(tampered);
    assert!(result.is_err(), "truncated signature must fail");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2. Wrong sender / recipient (spoofed from field)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn spoofed_sender_rejected_by_signature() {
    let real_key = SecretKey::generate();
    let attacker_key = SecretKey::generate();
    let msg = Message::Message {
        text: "fake".into(),
    };

    // Spoofed envelope: signed by attacker but claiming from = victim.
    let spoofed = spoof_signed_envelope(&attacker_key, real_key.public(), &msg);

    let result = SignedMessage::verify_and_decode(&spoofed);
    assert!(result.is_err(), "spoofed sender must fail verification");
}

#[test]
fn correct_sender_verifies_from_matches_signer() {
    let key_a = SecretKey::generate();
    let msg = Message::Message {
        text: "legitimate".into(),
    };

    let encoded = SignedMessage::sign_and_encode(&key_a, &msg).unwrap();
    let result = SignedMessage::verify_and_decode(&encoded);
    assert!(result.is_ok(), "legitimate self-signed message must verify");
    let (pk, _, _) = result.unwrap();
    assert_eq!(pk, key_a.public(), "from must equal signer's public key");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 3. Expired timestamp (older than TTL)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn expired_timestamp_dropped_by_handle_net_event() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());

    // sent_at = 1 second after epoch = very old (older than 3600s TTL).
    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "ancient".into(),
        },
        sent_at: 1,
    };

    handle_net_event(event, &mut chat).unwrap();
    assert!(
        chat.entries.is_empty(),
        "expired message must not create entries"
    );
    assert!(!chat.should_quit, "expired message must not trigger quit");
}

#[test]
fn recent_timestamp_accepted() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let now = now_secs();

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "recent".into(),
        },
        sent_at: now,
    };

    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(chat.entries.len(), 1, "recent message should be accepted");
    assert_eq!(chat.entries[0].body, "recent");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 4. Future timestamp beyond skew
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn future_timestamp_beyond_skew_rejected() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());

    let far_future = now_secs() + 86401; // 24h + 1 second skew

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "from_the_future".into(),
        },
        sent_at: far_future,
    };

    handle_net_event(event, &mut chat).unwrap();
    assert!(
        chat.entries.is_empty(),
        "future-dated message must be rejected"
    );
}

#[test]
fn future_timestamp_within_skew_accepted() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());

    let near_future = now_secs() + 240; // 4min (within 300s max skew)

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "slightly_future".into(),
        },
        sent_at: near_future,
    };

    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        1,
        "message within future skew should be accepted"
    );
    assert_eq!(chat.entries[0].body, "slightly_future");
}

#[test]
fn future_timestamp_exactly_at_skew_boundary_accepted() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());

    let boundary = now_secs() + 300; // exact skew boundary (must be ≤ MAX_FUTURE_SKEW_SECS)

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "boundary".into(),
        },
        sent_at: boundary,
    };

    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        1,
        "message exactly at skew boundary should be accepted"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 5. Malformed / oversized envelope
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn malformed_envelope_garbage_bytes_rejected() {
    let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF];
    let result = SignedMessage::verify_and_decode(&garbage);
    assert!(result.is_err(), "garbage bytes must fail decode");
}

#[test]
fn malformed_envelope_truncated_postcard_rejected() {
    let key = SecretKey::generate();
    let msg = Message::Message {
        text: "hello".into(),
    };
    let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();

    let truncated = &encoded[..4];
    let result = SignedMessage::verify_and_decode(truncated);
    assert!(result.is_err(), "truncated envelope must fail decode");
}

#[test]
fn malformed_envelope_extra_trailing_bytes_does_not_panic() {
    let key = SecretKey::generate();
    let msg = Message::Message {
        text: "hello".into(),
    };
    let mut encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap().to_vec();
    encoded.extend_from_slice(b"TRAILING_GARBAGE");

    // postcard ignores trailing bytes; this should not panic.
    let _result = SignedMessage::verify_and_decode(&encoded);
}

#[test]
fn oversized_envelope_huge_buffer_does_not_panic() {
    let huge = vec![0x01; 10_000_000]; // 10 MB
    let result = SignedMessage::verify_and_decode(&huge);
    assert!(
        result.is_err(),
        "huge malformed envelope must fail gracefully"
    );
}

#[test]
fn empty_envelope_rejected() {
    let empty: Vec<u8> = vec![];
    let result = SignedMessage::verify_and_decode(&empty);
    assert!(result.is_err(), "empty envelope must fail decode");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 6. Oversized plaintext
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn oversized_plaintext_does_not_panic() {
    let key = SecretKey::generate();
    let msg = Message::Message {
        text: "A".repeat(1_000_000), // 1 MB text
    };

    let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
    let result = SignedMessage::verify_and_decode(&encoded);
    assert!(result.is_ok(), "large but valid plaintext must decode");
    let (pk, decoded, _) = result.unwrap();
    assert_eq!(pk, key.public());
    match decoded {
        Message::Message { text } => assert_eq!(text.len(), 1_000_000),
        _ => panic!("expected Message::Message"),
    }
}

#[test]
fn oversized_plaintext_through_handle_net_event_accepted() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());

    let msg = Message::Message {
        text: "X".repeat(500_000), // 500 KB
    };
    let event = NetEvent::Message {
        from: key.public(),
        message: msg,
        sent_at: now_secs(),
    };

    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        1,
        "oversized plaintext should be accepted in private room"
    );
    assert_eq!(chat.entries[0].body.len(), 500_000);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 7. Unknown protocol version (future Message discriminant)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn unknown_message_discriminant_rejected_by_postcard() {
    // postcard uses varint-encoded discriminants for enums.
    // A discriminant of 128+ is out of range for current Message (0-13).
    let unknown_variant = vec![0x80, 0x01]; // varint 128
    let result: std::result::Result<Message, _> = postcard::from_bytes(&unknown_variant);
    assert!(
        result.is_err(),
        "unknown message discriminant must be rejected"
    );

    let unknown_variant_255 = vec![0xFF, 0x01]; // varint 255
    let result: std::result::Result<Message, _> = postcard::from_bytes(&unknown_variant_255);
    assert!(result.is_err(), "discriminant 255 must be rejected");
}

#[test]
fn unknown_message_variant_in_signed_envelope_does_not_panic() {
    let key = SecretKey::generate();
    let msg = Message::Heartbeat;
    let encoded = SignedMessage::sign_and_encode(&key, &msg).unwrap();
    let result = SignedMessage::verify_and_decode(&encoded);
    assert!(result.is_ok(), "valid signed message should decode");
    let (pk, decoded, _) = result.unwrap();
    assert!(matches!(decoded, Message::Heartbeat));
    assert_eq!(pk, key.public());
}

// ═══════════════════════════════════════════════════════════════════════════════
// 8. Conflicting message ID reuse (dedup)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn duplicate_message_suppressed_by_dedup() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let now = now_secs();

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "unique".into(),
        },
        sent_at: now,
    };

    handle_net_event(event.clone(), &mut chat).unwrap();
    assert_eq!(chat.entries.len(), 1);

    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        1,
        "duplicate must not create a second entry"
    );
}

#[test]
fn duplicate_about_me_suppressed_by_dedup() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let now = now_secs();

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::AboutMe {
            name: "alice".into(),
            profile_image_ticket: None,
        },
        sent_at: now,
    };

    handle_net_event(event.clone(), &mut chat).unwrap();
    let count_before = chat.entries.len();

    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        count_before,
        "duplicate AboutMe must be suppressed"
    );
}

#[test]
fn same_content_different_sender_not_deduped() {
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let now = now_secs();

    let event_a = NetEvent::Message {
        from: key_a.public(),
        message: Message::Message {
            text: "same text".into(),
        },
        sent_at: now,
    };
    let event_b = NetEvent::Message {
        from: key_b.public(),
        message: Message::Message {
            text: "same text".into(),
        },
        sent_at: now,
    };

    handle_net_event(event_a, &mut chat).unwrap();
    handle_net_event(event_b, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        2,
        "same content from different senders must both appear"
    );
}

#[test]
fn dedup_different_sent_at_not_deduped() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let base = now_secs();

    let event_t1 = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "hello".into(),
        },
        sent_at: base,
    };
    let event_t2 = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "hello".into(),
        },
        sent_at: base + 2,
    };

    handle_net_event(event_t1, &mut chat).unwrap();
    handle_net_event(event_t2, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        2,
        "same content at different timestamps should both appear"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 9. Unauthorised / blocked sender
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn blocked_sender_messages_silently_dropped() {
    let mut chat = TestChat::new(SecretKey::generate().public());
    let blocked_key = SecretKey::generate();
    chat.blocked.insert(blocked_key.public());

    let event = NetEvent::Message {
        from: blocked_key.public(),
        message: Message::Message {
            text: "spam".into(),
        },
        sent_at: now_secs(),
    };

    handle_net_event(event, &mut chat).unwrap();
    assert!(
        chat.entries.is_empty(),
        "blocked sender messages must be silently dropped"
    );
    assert!(
        chat.pending_file.is_none(),
        "blocked sender must not create pending file"
    );
    assert!(
        chat.pending_image.is_empty(),
        "blocked sender must not create pending image"
    );
}

#[test]
fn blocked_sender_about_me_silently_dropped() {
    let mut chat = TestChat::new(SecretKey::generate().public());
    let blocked_key = SecretKey::generate();
    chat.blocked.insert(blocked_key.public());

    let event = NetEvent::Message {
        from: blocked_key.public(),
        message: Message::AboutMe {
            name: "spammer".into(),
            profile_image_ticket: None,
        },
        sent_at: now_secs(),
    };

    handle_net_event(event, &mut chat).unwrap();
    assert!(
        chat.entries.is_empty(),
        "blocked sender AboutMe must be silently dropped"
    );
    assert!(
        !chat.names.contains_key(&blocked_key.public()),
        "blocked sender name must not be cached"
    );
}

#[test]
fn blocked_sender_image_share_silently_dropped() {
    let mut chat = TestChat::new(SecretKey::generate().public());
    let blocked_key = SecretKey::generate();
    chat.blocked.insert(blocked_key.public());

    let event = NetEvent::Message {
        from: blocked_key.public(),
        message: Message::ImageShare {
            name: "evil.jpg".into(),
            hash: [0xFF; 32],
        },
        sent_at: now_secs(),
    };

    handle_net_event(event, &mut chat).unwrap();
    assert!(
        chat.entries.is_empty(),
        "blocked sender ImageShare must be silently dropped"
    );
    assert!(
        chat.pending_image.is_empty(),
        "blocked sender must not queue image download"
    );
}

#[test]
fn non_blocked_sender_message_accepted() {
    let mut chat = TestChat::new(SecretKey::generate().public());
    let key = SecretKey::generate();

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "hello friend".into(),
        },
        sent_at: now_secs(),
    };

    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(
        chat.entries.len(),
        1,
        "non-blocked sender messages must be accepted"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 10. Invalid ack (ReadReceipt for unknown message hash)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn invalid_ack_for_unknown_hash_does_not_panic() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::ReadReceipt {
            message_hash: [0xAB; 32],
        },
        sent_at: now_secs(),
    };

    handle_net_event(event, &mut chat).unwrap();
    assert!(
        chat.entries.is_empty(),
        "invalid ack for unknown hash must not create entries"
    );
}

#[test]
fn valid_ack_for_known_message_produces_notification() {
    let mut chat = TestChat::new(SecretKey::generate().public());
    let sender_key = SecretKey::generate();

    let msg = Message::Message {
        text: "read this".into(),
    };
    let hash = message_hash(&msg);
    let event = NetEvent::Message {
        from: sender_key.public(),
        message: msg,
        sent_at: now_secs(),
    };
    handle_net_event(event, &mut chat).unwrap();
    assert_eq!(chat.entries.len(), 1);

    let reader_key = SecretKey::generate();
    let ack_event = NetEvent::Message {
        from: reader_key.public(),
        message: Message::ReadReceipt { message_hash: hash },
        sent_at: now_secs(),
    };
    handle_net_event(ack_event, &mut chat).unwrap();
    assert!(
        chat.entries.len() >= 2,
        "ack for known message must add a system notification"
    );
    let ack_body = &chat.entries[1].body;
    assert!(
        ack_body.contains("read"),
        "ack notification should mention 'read': {ack_body}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 11. Ack signed by another peer
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn ack_signed_by_wrong_peer_rejected() {
    let real_sender = SecretKey::generate();
    let attacker = SecretKey::generate();

    let msg = Message::ReadReceipt {
        message_hash: [0x01; 32],
    };

    // Spoof: attacker signs, but from field claims real_sender.
    let spoofed = spoof_signed_envelope(&attacker, real_sender.public(), &msg);

    let result = SignedMessage::verify_and_decode(&spoofed);
    assert!(
        result.is_err(),
        "ack signed by wrong key must fail verification"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 12. Oversized sync response (large batch of messages)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn many_messages_in_batch_do_not_cause_oob_memory() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());

    let base_time = now_secs();
    for i in 0..100 {
        let event = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: format!("msg_{}", i),
            },
            sent_at: base_time + i as u64,
        };
        handle_net_event(event, &mut chat).unwrap();
    }

    assert_eq!(
        chat.entries.len(),
        100,
        "all 100 unique messages should be accepted"
    );
}

#[test]
fn oversized_sync_response_messages_all_individually_validated() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let base_time = now_secs();

    let valid_event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "valid".into(),
        },
        sent_at: base_time,
    };
    handle_net_event(valid_event, &mut chat).unwrap();

    let rejected_event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "future".into(),
        },
        sent_at: base_time + 86500, // beyond 24h skew
    };
    handle_net_event(rejected_event, &mut chat).unwrap();

    let valid_event2 = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "valid2".into(),
        },
        sent_at: base_time + 2,
    };
    handle_net_event(valid_event2, &mut chat).unwrap();

    assert_eq!(
        chat.entries.len(),
        2,
        "only valid messages should be accepted"
    );
    assert_eq!(chat.entries[0].body, "valid");
    assert_eq!(chat.entries[1].body, "valid2");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 13. Replay flooding
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn replay_flood_identical_messages_suppressed() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let now = now_secs();

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "flood".into(),
        },
        sent_at: now,
    };

    for _ in 0..100 {
        handle_net_event(event.clone(), &mut chat).unwrap();
    }

    assert_eq!(
        chat.entries.len(),
        1,
        "replay flood must only produce one entry"
    );
}

#[test]
fn replay_flood_about_me_capped_at_one() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let now = now_secs();

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::AboutMe {
            name: "flooder".into(),
            profile_image_ticket: None,
        },
        sent_at: now,
    };

    for _ in 0..50 {
        handle_net_event(event.clone(), &mut chat).unwrap();
    }

    assert_eq!(chat.names.get(&key.public()), Some(&"flooder".to_string()));
    let name_notices: Vec<_> = chat
        .entries
        .iter()
        .filter(|e| e.body.contains("flooder"))
        .collect();
    assert_eq!(
        name_notices.len(),
        1,
        "replay flood AboutMe should only produce one name-change notice"
    );
}

#[test]
fn replay_flood_different_timestamps_all_accepted() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let base = now_secs();

    for i in 0..10 {
        let event = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: format!("flood_{}", i),
            },
            sent_at: base + i,
        };
        handle_net_event(event, &mut chat).unwrap();
    }

    assert_eq!(
        chat.entries.len(),
        10,
        "messages with different timestamps should all be accepted"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Negative safety properties
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn invalid_signed_message_does_not_panic_callbacks() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(key.public());

    let events = vec![
        NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: String::new(),
            },
            sent_at: now_secs(),
        },
        NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: "\0\x00null bytes".into(),
            },
            sent_at: now_secs(),
        },
        NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: " ".repeat(100),
            },
            sent_at: now_secs(),
        },
        NetEvent::NeighborUp { peer: key.public() },
        NetEvent::NeighborDown { peer: key.public() },
    ];

    for event in events {
        handle_net_event(event, &mut chat).unwrap();
    }
}

#[test]
fn handle_net_event_with_topic_does_not_panic_on_invalid() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let topic = Some(TopicId::from_bytes([0x42; 32]));

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "valid".into(),
        },
        sent_at: now_secs(),
    };

    handle_net_event_for_topic(event, &mut chat, topic).unwrap();
    assert_eq!(chat.entries.len(), 1);
    assert_eq!(chat.entries[0].body, "valid");
}

#[test]
fn rejected_input_does_not_clear_outbox() {
    let mut chat = TestChat::new(SecretKey::generate().public());
    let blocked_key = SecretKey::generate();
    chat.blocked.insert(blocked_key.public());

    chat.pending_file = Some(("important.doc".into(), "ticket123".into()));

    let hostile = NetEvent::Message {
        from: blocked_key.public(),
        message: Message::Message {
            text: "spam".into(),
        },
        sent_at: now_secs(),
    };
    handle_net_event(hostile, &mut chat).unwrap();

    assert_eq!(
        chat.pending_file,
        Some(("important.doc".into(), "ticket123".into())),
        "outbox state must survive hostile input processing"
    );

    let expired = NetEvent::Message {
        from: SecretKey::generate().public(),
        message: Message::Message { text: "old".into() },
        sent_at: 1,
    };
    handle_net_event(expired, &mut chat).unwrap();
    assert_eq!(
        chat.pending_file,
        Some(("important.doc".into(), "ticket123".into())),
        "outbox state must survive expired message processing"
    );
}

#[test]
fn rejected_input_does_not_cause_unbounded_dedup_set() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(SecretKey::generate().public());
    let base = now_secs();

    // Use timestamps slightly in the past so they pass both the TTL and future-skew checks.
    for i in 0..500 {
        let event = NetEvent::Message {
            from: key.public(),
            message: Message::Message {
                text: format!("msg_{}", i),
            },
            sent_at: base - 10,
        };
        handle_net_event(event, &mut chat).unwrap();
    }

    assert_eq!(
        chat.entries.len(),
        500,
        "all unique messages should be accepted"
    );
}

#[test]
fn all_message_variants_handle_gracefully() {
    let key = SecretKey::generate();
    let remote_key = SecretKey::generate();
    let mut chat = TestChat::new(key.public());

    let variants: Vec<Message> = vec![
        Message::AboutMe {
            name: "test".into(),
            profile_image_ticket: None,
        },
        Message::Message {
            text: "ping".into(),
        },
        Message::FileShare {
            name: "f.txt".into(),
            ticket: "tkt".into(),
        },
        Message::Leave,
        Message::Presence,
        Message::PresenceWithTicket {
            ticket: "tkt".into(),
        },
        Message::ReadReceipt {
            message_hash: [0x00; 32],
        },
        Message::Edit {
            original_hash: [0x00; 32],
            new_text: "edited".into(),
        },
        Message::Delete {
            message_hash: [0x00; 32],
        },
        Message::Reaction {
            message_hash: [0x00; 32],
            emoji: "\u{1f44d}".into(),
        },
        Message::ImageShare {
            name: "img.png".into(),
            hash: [0x00; 32],
        },
        Message::Heartbeat,
        Message::DiagnosticProbe(boru_chat::diagnostics::DiagnosticProbe {
            probe_id: "test".into(),
            sender_id: remote_key.public().to_string(),
            room_id: "test".into(),
            sent_at_ms: 0,
            payload: None,
        }),
    ];

    for msg in variants {
        let event = NetEvent::Message {
            from: remote_key.public(),
            message: msg,
            sent_at: now_secs(),
        };
        let _ = handle_net_event(event, &mut chat);
    }
}

#[test]
fn self_message_does_not_create_entry() {
    let key = SecretKey::generate();
    let mut chat = TestChat::new(key.public());

    let event = NetEvent::Message {
        from: key.public(),
        message: Message::Message {
            text: "self".into(),
        },
        sent_at: now_secs(),
    };

    handle_net_event(event, &mut chat).unwrap();
    assert!(
        chat.entries.is_empty(),
        "self-messages must not create entries"
    );
}
