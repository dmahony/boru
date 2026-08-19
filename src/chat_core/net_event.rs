//! Network event processing and the gossip→[`NetEvent`] bridge.
//!
//! Contains the public-room safety filter, [`handle_net_event_for_topic`]
//! (the shared event dispatcher used by both frontends), the gossip receiver
//! forwarders and connection-type accounting.  Everything here is orchestration
//! over the [`ChatCallbacks`] trait — no storage, no UI.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use iroh::{Endpoint, SecretKey};
use n0_error::Result;
use n0_future::StreamExt;

use super::dedup::{get_signed_message, remember_signed_message};
use super::protocol::{message_hash, Message, NetEvent, SignedMessage};
use super::status::{ConnectionType, StatusContext};
use super::{
    prune_seen_messages, ChatCallbacks, DIAGNOSTICS, DEDUP_SWEEP_THRESHOLD,
    DIAGNOSTIC_SEEN_MESSAGES, SEEN_MESSAGES,
};
use crate::api::{Event, GossipReceiver};
use crate::diagnostics::{DiagnosticEventKind, ReceivedProbe};
use crate::friends::FriendId;
use crate::proto::TopicId;
use crate::public_room_safety::PublicRoomSafety;

/// Maximum clock-skew tolerance for future-dated messages (5 minutes).
const MAX_FUTURE_SKEW_SECS: u64 = 300;

/// Apply public-room safety checks to a [`NetEvent`].
///
/// Returns `Some(event)` if the event passes all checks, or `None` if the
/// event should be silently dropped (message too large, rate limited, peer
/// exceeded blob announcement limit, etc.).
///
/// Non-message events (NeighborUp, NeighborDown, Closed, Error) always pass
/// through unchanged — they are control-plane signals, not untrusted content.
///
/// Pass `&PublicRoomSafety` for public rooms, or simply do not call this
/// function for private rooms to skip every check.
///
/// # Tracing
///
/// Each dropped event is logged at `debug` level with the reason, so operators
/// can monitor whether a peer is abusing the room without noise from legitimate
/// traffic.
pub fn filter_net_event_with_safety(
    event: NetEvent,
    safety: &PublicRoomSafety,
) -> Option<NetEvent> {
    match event {
        NetEvent::Message {
            from,
            message,
            sent_at,
        } => {
            // ── Message size check (text messages) ─────────────
            if let Message::Message { ref text } = message {
                if !safety.check_message_size(text.as_bytes()) {
                    tracing::debug!(
                        "safety: dropped oversized message ({} B) from {}",
                        text.len(),
                        from.fmt_short(),
                    );
                    return None;
                }
            }

            // ── Nickname length check ──────────────────────────
            if let Message::AboutMe { ref name, .. } = message {
                if name.len() > safety.config().nickname_length_limit {
                    tracing::debug!(
                        "safety: dropped long nickname ({} B) from {}",
                        name.len(),
                        from.fmt_short(),
                    );
                    return None;
                }
            }

            // ── Blob announcement check ────────────────────────
            let is_blob = matches!(
                message,
                Message::ImageShare { .. } | Message::FileShare { .. }
            );
            if is_blob && !safety.check_blob_announcement(&from) {
                tracing::debug!(
                    "safety: dropped blob announcement from {}",
                    from.fmt_short(),
                );
                return None;
            }

            // ── Per-peer rate limit ───────────────────────────
            if !safety.check_rate_limit(&from) {
                tracing::debug!("safety: rate-limited message from {}", from.fmt_short(),);
                return None;
            }

            Some(NetEvent::Message {
                from,
                message,
                sent_at,
            })
        }
        // ── Control-plane events always pass through ───────────
        other => Some(other),
    }
}

/// Process a decoded [`NetEvent`] against a [`ChatCallbacks`] implementor,
/// optionally applying public-room safety checks first.
///
/// When `safety` is `Some(...)`, the event is first run through
/// [`filter_net_event_with_safety`]; if it is rejected (rate-limited,
/// oversized, etc.) the function returns `Ok(())` without calling the
/// callback — the event is silently dropped.
///
/// When `safety` is `None` (private-room path), every event is forwarded
/// to `handle_net_event` unchanged.
///
/// `topic` is the optional room/topic context used when recording
/// diagnostic events (`PeerJoinedRoom`/`PeerLeftRoom`) so queries
/// scoped to a specific room can find them.
pub fn handle_net_event_with_safety(
    event: NetEvent,
    cb: &mut impl ChatCallbacks,
    safety: Option<&PublicRoomSafety>,
) -> Result<()> {
    handle_net_event_with_safety_for_topic(event, cb, safety, None)
}

/// Process an event with safety checks and explicit room/topic context.
pub fn handle_net_event_with_safety_for_topic(
    event: NetEvent,
    cb: &mut impl ChatCallbacks,
    safety: Option<&PublicRoomSafety>,
    topic: Option<TopicId>,
) -> Result<()> {
    let event = match safety {
        Some(s) => match filter_net_event_with_safety(event, s) {
            Some(ev) => ev,
            None => return Ok(()),
        },
        None => event,
    };
    handle_net_event_for_topic(event, cb, topic)
}

/// Process a decoded [`NetEvent`] against a [`ChatCallbacks`] implementor.
///
/// Handles common logic: friend tracking, name resolution, message
/// modification (edit/delete/reaction), typing indicators, and file
/// sharing. Frontend-specific side-effects (persistence, connection
/// counting, room previews) are delegated to the callbacks.
///
/// `topic` is the optional room/topic context used when recording
/// diagnostic events (`PeerJoinedRoom`/`PeerLeftRoom`) so queries
/// scoped to a specific room can find them.
pub fn handle_net_event(event: NetEvent, cb: &mut impl ChatCallbacks) -> Result<()> {
    handle_net_event_for_topic(event, cb, None)
}

/// Process a decoded event with explicit room/topic context.
pub fn handle_net_event_for_topic(
    event: NetEvent,
    cb: &mut impl ChatCallbacks,
    topic: Option<TopicId>,
) -> Result<()> {
    let event_label = match &event {
        NetEvent::Message { .. } => "Message",
        NetEvent::NeighborUp { .. } => "NeighborUp",
        NetEvent::NeighborDown { .. } => "NeighborDown",
        NetEvent::Closed => "Closed",
        NetEvent::Error(_) => "Error",
    };
    let _timer = crate::perf::PerfTracker::timer("handle_net_event", event_label);
    match event {
        NetEvent::Message {
            from,
            message,
            sent_at,
        } => {
            let incoming_hash = message_hash(&message);

            // ── Deduplication ──────────────────────────────────────────
            // Suppress duplicate deliveries from gossip fan-out, backfill,
            // and reconnection paths without dropping legitimate new messages.
            let dedup_key = (from, incoming_hash, sent_at);
            {
                let mut seen = SEEN_MESSAGES.lock().unwrap();
                if seen.insert(dedup_key, Instant::now()).is_none() {
                    // First time — continue processing below.
                } else {
                    // Dedup detected — record diagnostic event and suppress.
                    DIAGNOSTICS.record_with_peer(
                        None,
                        Some(from.to_string()),
                        DiagnosticEventKind::DuplicateMessage,
                    );
                    tracing::debug!(
                        "dedup: duplicate message from {} (hash={}, sent_at={})",
                        from.fmt_short(),
                        hex::encode(incoming_hash),
                        sent_at,
                    );
                    return Ok(());
                }
                // Periodic eviction of stale entries to bound memory growth.
                if seen.len() >= DEDUP_SWEEP_THRESHOLD {
                    drop(seen);
                    prune_seen_messages();
                }
            }

            cb.record_activity(from);

            // ── Blocked peer check ──────────────────────────────
            // Silently drop all messages from blocked peers.
            if cb.is_blocked(&from) {
                tracing::debug!("dropping message from blocked peer {}", from.fmt_short(),);
                return Ok(());
            }

            // ── Muted peer check ────────────────────────────────
            // Muted peers still have text messages shown, but
            // system notifications (name changes, file shares, image
            // shares) are suppressed.
            let is_muted = if from != cb.local_public() {
                cb.is_muted(&from)
            } else {
                false
            };

            if from != cb.local_public() {
                let now_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let ttl_secs = cb.message_ttl().as_secs();

                // Check for past-expired messages.
                let age_secs = now_secs.saturating_sub(sent_at);
                if age_secs > ttl_secs {
                    tracing::debug!(
                        "dropping stale message from {} (age {}s > TTL {}s)",
                        from.fmt_short(),
                        age_secs,
                        ttl_secs,
                    );
                    return Ok(());
                }

                // Check for future-dated messages beyond clock-skew tolerance.
                let skew_secs = sent_at.saturating_sub(now_secs);
                if skew_secs > MAX_FUTURE_SKEW_SECS {
                    tracing::debug!(
                        "dropping future-dated message from {} (skew {}s > MAX_FUTURE_SKEW_SECS {}s)",
                        from.fmt_short(),
                        skew_secs,
                        MAX_FUTURE_SKEW_SECS,
                    );
                    return Ok(());
                }
            }
            // ── Diagnostics message-received dedup ────────────────────────
            // Suppress duplicate diagnostic events for the same
            // (message_hash, sender_id) — regardless of sent_at — to
            // prevent the diagnostics buffer from being saturated by
            // replayed stale messages bouncing through the gossip mesh.
            // TTL is 60 seconds (generous for catching bursts).
            let mut diag_had_hash = false;
            const DIAG_DEDUP_TTL_S: u64 = 60;
            {
                let mut diag_seen = DIAGNOSTIC_SEEN_MESSAGES.lock().unwrap();
                let diag_key = (incoming_hash, from);
                let now = Instant::now();
                if let Some(prev) = diag_seen.get_mut(&diag_key) {
                    let expired = now.duration_since(*prev).as_secs() > DIAG_DEDUP_TTL_S;
                    if !expired {
                        diag_had_hash = true;
                    } else {
                        *prev = now; // refresh for the new window
                    }
                } else {
                    diag_seen.insert(diag_key, now);
                }
                // Periodic eviction of stale entries to bound memory growth.
                if diag_seen.len() >= 256 {
                    diag_seen.retain(|_, seen_at| {
                        now.duration_since(*seen_at).as_secs() <= DIAG_DEDUP_TTL_S
                    });
                }
            }

            let reply_to = message.reply_to_message_id();
            match message {
                Message::AboutMe {
                    name,
                    profile_image_ticket,
                } => {
                    let prior_announced_name = cb.last_announced_name(&from);
                    let old_name = cb.set_name(from, name.clone());
                    let old_name = prior_announced_name.or(old_name);
                    match profile_image_ticket {
                        Some(ticket) => cb.record_profile_image_ticket(from, ticket),
                        None => cb.clear_profile_image(from),
                    }
                    if from != cb.local_public() {
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) || cb.accepts_group_peer(topic, &from) {
                            cb.friend_set_name(fid, name.clone());
                            cb.mark_friends_dirty();
                        }
                        if old_name.as_deref() != Some(&name) && !is_muted {
                            cb.push_system(format!(
                                "{} is now known as {}",
                                from.fmt_short(),
                                name
                            ));
                        }
                    }
                }
                Message::ProfileUpdate(profile) => {
                    if from != cb.local_public() {
                        cb.on_profile_update(from, profile);
                    }
                }
                Message::Message { text } | Message::Reply { text, .. } => {
                    if from != cb.local_public() {
                        let signed_bytes = get_signed_message(from, incoming_hash, sent_at);
                        let message_id = signed_bytes
                            .as_deref()
                            .and_then(|bytes| SignedMessage::verify_and_decode_with_id(bytes).ok())
                            .map(|(_, _, _, id)| id)
                            .unwrap_or(incoming_hash);
                        cb.persist_remote_message(
                            topic,
                            from,
                            incoming_hash,
                            sent_at,
                            &text,
                            signed_bytes,
                            message_id,
                            reply_to,
                        );
                        // Record diagnostic event for real chat messages from
                        // remote peers, subject to the per-key cooldown.
                        if !diag_had_hash {
                            DIAGNOSTICS.record_with_peer(
                                topic,
                                Some(from.to_string()),
                                DiagnosticEventKind::MessageReceived {
                                    message_id: Some(hex::encode(incoming_hash)),
                                    message_hash: Some(hex::encode(incoming_hash)),
                                    probe_id: None,
                                    sender_id: Some(from.to_string()),
                                },
                            );
                        }
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) || cb.accepts_group_peer(topic, &from) {
                            cb.friend_mark_online(fid);
                            // NOT mark_friends_dirty — online status is
                            // determined by the dedicated friend ping manager
                            // (FriendPingManager), not by gossip activity.
                        }
                        let display_name = cb.resolve_name(&from);
                        cb.push_remote(
                            from,
                            display_name,
                            text,
                            Some(incoming_hash),
                            Some(sent_at),
                        );
                    }
                }
                Message::MessageWithMentions { text, mentions } => {
                    if from != cb.local_public() {
                        cb.persist_remote_message(
                            topic,
                            from,
                            incoming_hash,
                            sent_at,
                            &text,
                            get_signed_message(from, incoming_hash, sent_at),
                        );
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) || cb.accepts_group_peer(topic, &from) {
                            cb.friend_mark_online(fid);
                        }
                        let display_name = cb.resolve_name(&from);
                        cb.push_remote_with_mentions(
                            from,
                            display_name,
                            text,
                            mentions,
                            Some(incoming_hash),
                            Some(sent_at),
                        );
                    }
                }
                Message::FileShare {
                    name,
                    ticket,
                    size,
                    thumbnail_hash,
                    collection_hash,
                    collection_entries,
                } => {
                    if from != cb.local_public() {
                        cb.persist_remote_file_share(
                            topic,
                            from,
                            incoming_hash,
                            sent_at,
                            &name,
                            get_signed_message(from, incoming_hash, sent_at),
                        );
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) || cb.accepts_group_peer(topic, &from) {
                            cb.friend_mark_online(fid);
                            if !is_muted {
                                let sender_name = cb.resolve_name(&from);
                                // VIDCARD-12: the download card renders the
                                // filename prominently, so the surrounding
                                // system line must not repeat the full
                                // filename (long names would duplicate).
                                if collection_hash.is_some() {
                                    cb.push_system(format!("{} shared a folder", sender_name));
                                    cb.set_pending_folder(
                                        name,
                                        ticket,
                                        size,
                                        collection_hash,
                                        collection_entries,
                                        Some(sender_name),
                                    );
                                } else {
                                    // The sender may re-announce the same
                                    // ticket once a video poster is ready;
                                    // skip the system line for that
                                    // follow-up so it doesn't read as a
                                    // second share.
                                    if !cb.is_known_file_ticket(&ticket) {
                                        cb.push_system(format!("{} shared a file", sender_name));
                                    }
                                    cb.set_pending_file(
                                        name,
                                        ticket,
                                        size,
                                        thumbnail_hash,
                                        Some(sender_name),
                                    );
                                }
                            }
                        }
                    }
                }
                Message::FileOffer {
                    offer_id,
                    name,
                    size,
                } => {
                    if from != cb.local_public()
                        && (cb.is_friend(&from) || cb.accepts_group_peer(topic, &from))
                    {
                        tracing::info!(
                            target: "boru::file_offer",
                            event = crate::diagnostics::event_names::OFFER_RECEIVED,
                            offer_id = ?offer_id,
                            bytes = size,
                            peer = %from,
                            "direct file offer received"
                        );
                        let fid = FriendId::from_public_key(from);
                        cb.friend_mark_online(fid);
                        let sender_label = cb.resolve_name(&from);
                        cb.set_pending_direct_offer(offer_id, name, size, from, Some(sender_label));
                    }
                }
                Message::FileOfferReady {
                    offer_id,
                    ticket,
                    thumbnail_hash,
                } => {
                    if from != cb.local_public()
                        && (cb.is_friend(&from) || cb.accepts_group_peer(topic, &from))
                    {
                        let fid = FriendId::from_public_key(from);
                        cb.friend_mark_online(fid);
                        let sender_label = cb.resolve_name(&from);
                        cb.set_pending_direct_offer_ready(
                            offer_id,
                            ticket,
                            thumbnail_hash,
                            from,
                            Some(sender_label),
                        );
                    }
                }
                Message::ImageShare { name, hash } => {
                    if from != cb.local_public() {
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) || cb.accepts_group_peer(topic, &from) {
                            cb.friend_mark_online(fid);
                            if !is_muted {
                                cb.set_pending_image(name, hash, from);
                            }
                        }
                    }
                }
                Message::SharedGif { gif } => {
                    if from != cb.local_public() {
                        let fid = FriendId::from_public_key(from);
                        if cb.is_friend(&from) || cb.accepts_group_peer(topic, &from) {
                            cb.friend_mark_online(fid);
                            if !is_muted {
                                cb.set_pending_gif(gif, from, incoming_hash);
                            }
                        }
                    }
                }
                Message::Leave => {
                    // Handled via NetEvent::NeighborDown, which fires for
                    // both clean (Leave) and unclean (crash/disconnect)
                    // departures.
                }
                Message::Presence => {
                    cb.record_presence(from);
                }
                Message::PresenceWithTicket { ticket } => {
                    cb.record_presence(from);
                    cb.record_peer_ticket(from, ticket);
                }
                Message::Heartbeat => {
                    // Heartbeat is invisible — record activity to update
                    // mesh health timestamps, but never push to the chat log.
                    cb.record_activity(from);
                }
                Message::LatencyPing { sent_at_ms } => {
                    // Record activity and let the frontend respond with a pong.
                    cb.record_activity(from);
                    cb.on_latency_ping(from, sent_at_ms);
                }
                Message::LatencyPong { sent_at_ms } => {
                    // We sent a ping earlier — this pong lets us compute RTT.
                    cb.record_activity(from);
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    if now_ms >= sent_at_ms {
                        let rtt = Duration::from_millis(now_ms - sent_at_ms);
                        cb.record_latency(from, rtt);
                    }
                }
                Message::DiagnosticProbe(ref probe) => {
                    // Diagnostic probes travel through the normal gossip path
                    // but are NOT displayed as ordinary chat messages.  They
                    // are recorded in the diagnostics store with full metadata
                    // (latency, message hash, duplicate tracking).
                    let received_at_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;

                    // Compute the message hash from the probe content
                    let hash = message_hash(&message);

                    // Compute latency; return None if clock skew produces
                    // a negative value.
                    let latency_ms = if received_at_ms >= probe.sent_at_ms {
                        Some(received_at_ms - probe.sent_at_ms)
                    } else {
                        None
                    };

                    let hash_hex = hex::encode(hash);

                    // Record the received probe in diagnostics storage
                    let received = ReceivedProbe {
                        probe_id: probe.probe_id.clone(),
                        room_id: probe.room_id.clone(),
                        sender_id: probe.sender_id.clone(),
                        sent_at_ms: probe.sent_at_ms,
                        received_at_ms,
                        latency_ms,
                        message_hash: hash_hex.clone(),
                        duplicate_count: 0,
                        timestamp: chrono::Utc::now(),
                        room_id_opt: None,
                    };
                    DIAGNOSTICS.record_received_probe_enhanced(received);

                    // Emit diagnostic events
                    DIAGNOSTICS.record(
                        None,
                        DiagnosticEventKind::ProbeReceived {
                            probe_id: probe.probe_id.clone(),
                            message_hash: hash_hex.clone(),
                            sender_id: probe.sender_id.clone(),
                        },
                    );
                }
                Message::ReadReceipt { message_hash: _ } => {
                    // Read receipts update delivery state icons only —
                    // no system message needed since the 👁 icon is visible.
                }
                Message::Typing { active } => {
                    if from != cb.local_public() {
                        cb.on_typing(topic, from, active);
                    }
                }
                Message::Edit {
                    original_hash,
                    new_text,
                } => {
                    if from != cb.local_public() {
                        cb.edit_message(&original_hash, new_text);
                    }
                }
                Message::Delete { message_hash } => {
                    if from != cb.local_public() {
                        cb.delete_message(&message_hash);
                    }
                }
                Message::Reaction {
                    message_hash,
                    emoji,
                } => {
                    if from != cb.local_public() {
                        cb.add_reaction(&message_hash, emoji);
                    }
                }
                Message::ReactionAdd { message_id, emoji } => {
                    if from != cb.local_public() {
                        cb.apply_reaction_event(crate::reactions::ReactionEvent::add(
                            message_id,
                            *from.as_bytes(),
                            emoji,
                        ));
                    }
                }
                Message::ReactionRemove { message_id, emoji } => {
                    if from != cb.local_public() {
                        cb.apply_reaction_event(crate::reactions::ReactionEvent::remove(
                            message_id,
                            *from.as_bytes(),
                            emoji,
                        ));
                    }
                }
                Message::PinMessage {
                    topic: pin_topic,
                    message_hash,
                } => {
                    if from == cb.local_public()
                        || cb.is_friend(&from)
                        || cb.accepts_group_peer(Some(pin_topic), &from)
                    {
                        cb.pin_message(pin_topic, message_hash, from, sent_at);
                    }
                }
                Message::UnpinMessage {
                    topic: pin_topic,
                    message_hash,
                } => {
                    if from == cb.local_public()
                        || cb.is_friend(&from)
                        || cb.accepts_group_peer(Some(pin_topic), &from)
                    {
                        cb.unpin_message(pin_topic, message_hash, from, sent_at);
                    }
                }
                Message::ContactControl { .. } => {
                    // Handled at the frontend layer.
                }
                Message::RoomAdvertisement { .. } => {
                    // Room advertisements are handled at the frontend layer.
                }
                Message::RoomWithdrawal { .. } => {
                    // Room withdrawals are handled at the frontend layer.
                }
                Message::EncryptedGroupMessage { .. } => {
                    // Encrypted group messages are handled at the group encryption
                    // layer once the membership/ordering modules are wired in.
                }
            }
        }
        NetEvent::NeighborUp { peer } => {
            // NeighborUp is the first reliable application-level indication
            // that the gossip transport has discovered and admitted a peer
            // to this topic.  Address lookup/source details are deliberately
            // left unreported here (they are owned by iroh), but the
            // connection, subscription, and topic-membership stages are
            // observable and should be reflected in diagnostics.
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerDiscovered,
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::ConnectionEstablished {
                    remote_address: None,
                    transport: None,
                    used_relay: None,
                },
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::RoomSubscriptionJoined,
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerAddedToTopic,
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerJoinedRoom,
            );
            cb.on_neighbor_status_change(peer, true);
        }
        NetEvent::NeighborDown { peer } => {
            cb.clear_typing_peer(&peer);
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerLeftRoom,
            );
            DIAGNOSTICS.record_with_peer(
                topic,
                Some(peer.to_string()),
                DiagnosticEventKind::PeerRemovedFromTopic {
                    reason: Some("neighbor_down".to_string()),
                },
            );
            cb.on_neighbor_status_change(peer, false);
        }
        NetEvent::Closed => {
            DIAGNOSTICS.record(
                None,
                DiagnosticEventKind::Error("gossip receiver closed".to_string()),
            );
            cb.push_system("The gossip receiver closed.".into());
            cb.request_quit();
        }
        NetEvent::Error(err) => {
            DIAGNOSTICS.record(None, DiagnosticEventKind::Error(err.to_string()));
            cb.push_system(format!("Network error: {err}"));
            cb.request_quit();
        }
    }
    Ok(())
}

/// Room-doc messages on the gossip topic use marker prefixes.
/// Metadata updates start with 0xFE, roster updates start with 0xFF.
/// These are handled by the room_docs layer and are not SignedMessages.
const METADATA_MARKER: u8 = 0xFE;
const ROSTER_MARKER: u8 = 0xFF;


/// Return the current Unix epoch time in seconds.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Return the current Unix epoch time in milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Create and sign a diagnostic probe message suitable for gossip broadcast.
///
/// Returns the signed, encoded bytes ready to send via `gossip.broadcast()`.
/// The probe reuses the same serialisation and signing path as ordinary
/// room messages — no separate protocol is needed.
///
/// When `payload` is `None`, the probe carries no diagnostic text.  The
/// `probe_id` is auto-generated via [`crate::diagnostics::generate_probe_id`]
/// unless one is supplied.
pub fn broadcast_diagnostic_probe(
    secret_key: &SecretKey,
    room_id: &str,
    payload: Option<String>,
    probe_id_override: Option<String>,
) -> Result<Bytes> {
    let probe = crate::diagnostics::DiagnosticProbe {
        probe_id: probe_id_override.unwrap_or_else(crate::diagnostics::generate_probe_id),
        sender_id: secret_key.public().to_string(),
        room_id: room_id.to_string(),
        sent_at_ms: now_ms() as i64,
        payload,
    };

    // Record the broadcast event in diagnostics
    let hash_hex = {
        let msg = Message::DiagnosticProbe(probe.clone());
        let hash = message_hash(&msg);
        hex::encode(hash)
    };
    DIAGNOSTICS.record(
        None,
        DiagnosticEventKind::ProbeBroadcast {
            probe_id: probe.probe_id.clone(),
            message_hash: hash_hex,
        },
    );

    let message = Message::DiagnosticProbe(probe);
    SignedMessage::sign_and_encode(secret_key, &message)
}

/// Forward raw gossip events into a [`NetEvent`] channel.
///
/// Spawn this as a background task to bridge the gossip receiver
/// into a `NetEvent` stream.  Private-room callers use this; public-room
/// callers should use [`forward_gossip_events_with_safety`] instead.
pub async fn forward_gossip_events(
    receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::Sender<NetEvent>,
) {
    forward_gossip_events_with_safety(receiver, net_tx, None).await
}

/// Forward raw gossip events into a [`NetEvent`] channel, applying public-room
/// safety checks when a [`PublicRoomSafety`] is provided.
///
/// When `safety` is `None`, every decoded event passes through unchanged
/// (private-room path).  When `Some(...)`, each event is run through
/// [`filter_net_event_with_safety`] and silently dropped if it violates
/// the room's size, rate, or announcement limits.
pub async fn forward_gossip_events_with_safety(
    mut receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::Sender<NetEvent>,
    safety: Option<Arc<PublicRoomSafety>>,
) {
    while let Ok(Some(event)) = receiver.try_next().await {
        match event {
            Event::Received(msg) => {
                // Skip room-doc messages (metadata 0xFE, roster 0xFF) —
                // they are not SignedMessages and would fail decode.
                if let Some(&marker) = msg.content.first() {
                    if marker == METADATA_MARKER || marker == ROSTER_MARKER {
                        continue;
                    }
                }
                let _decode_timer =
                    crate::perf::PerfTracker::timer("forward_gossip_decode", "verify_and_decode");
                match SignedMessage::verify_and_decode(&msg.content) {
                    Ok((from, message, sent_at)) => {
                        remember_signed_message(from, &message, sent_at, &msg.content);
                        let net_event = NetEvent::Message {
                            from,
                            message,
                            sent_at,
                        };
                        // Apply safety filter for public rooms.
                        let net_event = match &safety {
                            Some(s) => match filter_net_event_with_safety(net_event, s) {
                                Some(ev) => ev,
                                None => continue,
                            },
                            None => net_event,
                        };
                        if net_tx.send(net_event).await.is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        // Log the error but keep running — a single bad
                        // message should not kill the network bridge task.
                        tracing::warn!("forward_gossip_events: decode error (dropped): {err}");
                        continue;
                    }
                }
            }
            Event::NeighborUp(id) => {
                if net_tx
                    .send(NetEvent::NeighborUp { peer: id })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Event::NeighborDown(id) => {
                if net_tx
                    .send(NetEvent::NeighborDown { peer: id })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Event::Lagged => {
                // Lagged warnings are protocol-level backpressure signals;
                // not forwarded to the frontend.
            }
            Event::MissingMessages { .. } => {
                // Round-gap detected; the protocol suggests missed messages.
                // Actual backfill logic can be added here later — for now,
                // just log at debug level so it's traceable but non-noisy.
                tracing::debug!("round gap detected in gossip events");
            }
        }
    }
    let _ = net_tx.send(NetEvent::Closed).await;
}

/// Update `StatusContext.direct_peers` and `.relayed_peers` by querying the
/// iroh [`Endpoint`] for each known neighbor.
///
/// For each peer in `status.neighbors` we ask the endpoint for remote info.
/// A peer with at least one direct (IP-based) transport address is counted
/// as `direct`; a peer reachable only via relay is counted as `relayed`.
///
/// Also populates `status.peer_connection_types` with per-peer granularity.
pub async fn update_connection_counts(endpoint: &Endpoint, status: &mut StatusContext) {
    let mut direct = 0usize;
    let mut relayed = 0usize;
    let peers: Vec<iroh::PublicKey> = status.neighbors.iter().copied().collect();
    for peer in &peers {
        let ctype = check_peer_connection_type(endpoint, *peer).await;
        match ctype {
            ConnectionType::Direct => direct += 1,
            ConnectionType::Relayed => relayed += 1,
            ConnectionType::Unknown => {}
        }
        if ctype != ConnectionType::Unknown {
            status.peer_connection_types.insert(*peer, ctype);
        }
    }
    status.direct_peers = direct;
    status.relayed_peers = relayed;
}

/// Query the iroh [`Endpoint`] for a single peer and return its connection type.
///
/// Returns:
/// - [`ConnectionType::Direct`] if the peer has at least one direct (IP-based) address.
/// - [`ConnectionType::Relayed`] if the peer is reachable only via relay.
/// - [`ConnectionType::Unknown`] if the peer is not known to the endpoint.
pub async fn check_peer_connection_type(
    endpoint: &Endpoint,
    peer: iroh::PublicKey,
) -> ConnectionType {
    match endpoint.remote_info(peer).await {
        Some(info) => {
            let has_direct = info
                .addrs()
                .any(|a| matches!(a.addr(), iroh::TransportAddr::Ip(_)));
            if has_direct {
                ConnectionType::Direct
            } else {
                ConnectionType::Relayed
            }
        }
        None => ConnectionType::Unknown,
    }
}
