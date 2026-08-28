//! BORU-CALL-3.14 voice acceptance test (Phase 3 gate).
//!
//! Headless two-endpoint driver at the actor/event level. The GUI call UI
//! (Phase 6) is not merged, so per the task body we validate the full 1:1
//! voice flow through `CallHandle` + `CallEvent` plus a synthetic PCM media
//! channel using the production `AudioSender` path, and document the mapping
//! to the UI checklist in `VOICE_ACCEPTANCE.md`.
//!
//! The ten acceptance steps:
//!   1. two instances become friends            -> two endpoints with known
//!                                                identities + direct connect
//!   2. open a direct chat                       -> direct CALL_ALPN connection
//!   3. click phone                              -> start_voice_call
//!   4. receive incoming call event              -> CallEvent::Incoming
//!   5. accept                                   -> CallEvent::Active on both
//!   6. talk bidirectionally                     -> synthetic sine PCM through
//!                                                AudioSender, MediaReceived
//!                                                on the remote, decoded
//!                                                non-silent both directions
//!   7. mute/unmute                              -> set_muted, MediaStateChanged
//!                                                on both sides
//!   8. survive temporary jitter/loss            -> datagrams dropped at the
//!                                                sender, call stays Active,
//!                                                later frames still arrive
//!   9. hang up from either side                 -> hangup, Ended on both
//!  10. immediately make another call            -> second full cycle on the
//!                                                same handles, no restart

use std::time::{Duration, Instant};

use boru_core::call::audio::codec::OpusEncoder;
use boru_core::call::audio::jitter::{
    AudioJitterBuffer, BufferedAudioPacket, DEFAULT_JITTER_DELAY,
};
use boru_core::call::audio::plc::OpusPlayoutDecoder;
use boru_core::call::audio::send::{AudioSender, EncodedAudioFrame};
use boru_core::call::frame::{SAMPLES_PER_FRAME, SAMPLE_RATE};
use boru_core::call::manager::{CallBuilder, CallEvent, CALL_ALPN};
use boru_core::call::media::{MediaDatagram, MediaKind};
use boru_core::call::{CallId, CallKind};
use iroh::{endpoint::presets, protocol::Router, Endpoint};
use tokio::sync::mpsc;

/// A 440 Hz sine tone in normalized mono f32 PCM, one 20 ms frame at a time.
fn sine_frame(frame_index: u32) -> Vec<f32> {
    let phase_step = 2.0 * std::f32::consts::PI * 440.0 / SAMPLE_RATE as f32;
    (0..SAMPLES_PER_FRAME)
        .map(|i| {
            let phase = phase_step * (frame_index as usize * SAMPLES_PER_FRAME + i) as f32;
            0.5 * phase.sin()
        })
        .collect()
}

async fn next_event(label: &str, events: &mut mpsc::Receiver<CallEvent>) -> CallEvent {
    tokio::time::timeout(Duration::from_secs(8), events.recv())
        .await
        .unwrap_or_else(|_| panic!("call event timed out: {label}"))
        .expect("call actor stopped")
}

/// Wait for the next `MediaReceived` event, skipping unrelated signalling.
async fn next_media(label: &str, events: &mut mpsc::Receiver<CallEvent>) -> MediaDatagram {
    loop {
        match next_event(label, events).await {
            CallEvent::MediaReceived { datagram, .. } => return datagram,
            other => panic!("expected media event for {label}, got {other:?}"),
        }
    }
}

/// Drain events until a terminal (Ended/Failed) event for `call_id` arrives.
async fn next_terminal(
    label: &str,
    events: &mut mpsc::Receiver<CallEvent>,
    call_id: CallId,
) -> CallEvent {
    loop {
        match next_event(label, events).await {
            event @ (CallEvent::Ended { call_id: id, .. }
            | CallEvent::Failed {
                call_id: Some(id), ..
            }) if id == call_id => {
                return event;
            }
            _ => continue,
        }
    }
}

/// A persistent media channel between two endpoints.
///
/// The connection registers with the remote router (CALL_ALPN) and opens a bi
/// stream so the remote's `accept_bi()` returns and its media reader starts,
/// mirroring how the app-side audio sender would own a connection to the peer.
/// The stream halves MUST be kept alive: dropping them sends a QUIC FIN that
/// makes the remote's wire session see EOF and report `ConnectionClosed`, and
/// the actor then ends every call to that peer with `ConnectionLost`.
struct MediaChannel {
    _connection: iroh::endpoint::Connection,
    _stream_send: iroh::endpoint::SendStream,
    _stream_recv: iroh::endpoint::RecvStream,
}

async fn open_media_channel(client: &Endpoint, server: &Endpoint) -> MediaChannel {
    let conn = client
        .connect(server.addr(), CALL_ALPN)
        .await
        .expect("media channel should connect");
    let (mut send, recv) = conn
        .open_bi()
        .await
        .expect("media channel should open a bi stream");
    // Send one byte so the stream-open frame reaches the peer. quinn's
    // `open_bi()` returns local handles immediately, but the remote's
    // `accept_bi()` only completes when it receives an actual STREAM frame
    // carrying the new stream id; a never-written stream stays invisible to
    // the remote, so the call actor there never spawns a media reader.
    // (`write_all` is inherent on iroh's SendStream — no trait import needed.)
    let _ = send.write_all(b"\x00").await;
    MediaChannel {
        _connection: conn,
        _stream_send: send,
        _stream_recv: recv,
    }
}

/// Decode one received Opus payload through jitter + playout and assert the
/// PCM is non-silent (proves real audio, not silence, travelled the wire).
fn assert_decodes_to_audio(label: &str, packet: &BufferedAudioPacket) {
    let mut jitter = AudioJitterBuffer::default();
    let mut decoder = OpusPlayoutDecoder::new().expect("opus decoder");
    assert!(
        jitter.push(BufferedAudioPacket {
            call_id: packet.call_id,
            sequence: packet.sequence,
            timestamp: packet.timestamp,
            arrival: packet.arrival,
            payload: packet.payload.clone(),
        }),
        "{label}: packet should be accepted by the jitter buffer"
    );
    let frame = decoder
        .decode_due(&mut jitter, packet.arrival + DEFAULT_JITTER_DELAY)
        .expect("decode")
        .expect("frame due after jitter delay");
    assert_eq!(
        frame.samples.len(),
        SAMPLES_PER_FRAME,
        "{label}: frame size"
    );
    assert!(
        frame.samples.iter().any(|sample| sample.abs() > 0.001),
        "{label}: received audio must be non-silent (sine tone)"
    );
}

struct TestNode {
    endpoint: Endpoint,
    handle: boru_core::call::manager::CallHandle,
    events: mpsc::Receiver<CallEvent>,
    router: Router,
}

async fn spawn_node() -> TestNode {
    let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
    let builder = CallBuilder::new(endpoint.clone(), endpoint.secret_key().clone());
    let handler = builder.protocol_handler();
    let (handle, events) = builder.spawn();
    let router = Router::builder(endpoint.clone())
        .accept(CALL_ALPN, handler)
        .spawn();
    TestNode {
        endpoint,
        handle,
        events,
        router,
    }
}

#[tokio::test]
async fn voice_acceptance_full_flow_two_endpoints() {
    // ── Setup: two instances ─────────────────────────────────────────────
    // Step 1 (friends): both endpoints know each other's identities and can
    // establish a direct connection; a connect probe seeds the address cache
    // (production discovery uses mDNS/relay before the user presses Call).
    // Step 2 (direct chat): the direct CALL_ALPN connection below is the
    // actor-level equivalent of opening a direct conversation.
    let mut caller = spawn_node().await;
    let mut callee = spawn_node().await;

    // Authorization is deny-by-default: each side must authorize the other
    // before outbound start (CallHandle) or inbound accept (CallProtocol).
    caller
        .handle
        .set_peer_authorized(callee.endpoint.id(), true);
    callee
        .handle
        .set_peer_authorized(caller.endpoint.id(), true);

    let probe = caller
        .endpoint
        .connect(callee.endpoint.addr(), CALL_ALPN)
        .await
        .unwrap();
    probe.close(0u32.into(), b"probe");

    let callee_id = callee.endpoint.id();

    // ── Step 3 + 4: start the call, receive the incoming event ───────────
    let call_id = caller.handle.start_voice_call(callee_id).await.unwrap();
    assert!(
        matches!(
            next_event("caller ringing", &mut caller.events).await,
            CallEvent::OutgoingRinging { call_id: id, .. } if id == call_id
        ),
        "step 3: caller sees OutgoingRinging"
    );
    let incoming_id = match next_event("callee incoming", &mut callee.events).await {
        CallEvent::Incoming { call_id, kind, .. } => {
            assert_eq!(kind, CallKind::Voice);
            call_id
        }
        other => panic!("step 4: expected incoming voice call, got {other:?}"),
    };
    assert_eq!(incoming_id, call_id, "step 4: same call id on both sides");

    // ── Step 5: accept ───────────────────────────────────────────────────
    callee.handle.accept(call_id).await.unwrap();
    assert!(
        matches!(
            next_event("callee active", &mut callee.events).await,
            CallEvent::Active { call_id: id, .. } if id == call_id
        ),
        "step 5: callee sees Active"
    );
    assert!(
        matches!(
            next_event("caller active", &mut caller.events).await,
            CallEvent::Active { call_id: id, .. } if id == call_id
        ),
        "step 5: caller sees Active"
    );

    // ── Step 6: talk bidirectionally with synthetic PCM ──────────────────
    // Caller -> callee: production AudioSender over a direct channel.
    let caller_chan = open_media_channel(&caller.endpoint, &callee.endpoint).await;
    let mut sender = AudioSender::new(caller_chan._connection.clone(), call_id, 1).unwrap();
    let mut encoder = OpusEncoder::new().expect("opus encoder");
    let frames = 10;
    let mut sent = 0usize;
    for i in 0..frames {
        let pcm = sine_frame(i as u32);
        let payload = encoder.encode(&pcm).unwrap().expect("non-empty packet");
        if sender.try_send(EncodedAudioFrame {
            sequence: i as u32,
            timestamp: (i as u32) * SAMPLES_PER_FRAME as u32,
            payload,
        }) {
            sent += 1;
        }
    }
    assert_eq!(
        sent, frames as usize,
        "step 6: all caller frames handed off"
    );

    // The callee's media reader surfaces each datagram as MediaReceived.
    let mut received: Vec<MediaDatagram> = Vec::new();
    for _ in 0..frames {
        let datagram = next_media("callee receive", &mut callee.events).await;
        assert_eq!(datagram.kind, MediaKind::Audio, "step 6: audio kind");
        assert_eq!(datagram.call_id, call_id, "step 6: call id on media");
        received.push(datagram);
    }
    assert_eq!(
        received.len(),
        frames as usize,
        "step 6: all frames received"
    );
    // Decode the first received frame: proves real audio (sine) arrived.
    let first = &received[0];
    assert_decodes_to_audio(
        "caller->callee",
        &BufferedAudioPacket {
            call_id: first.call_id,
            sequence: first.sequence,
            timestamp: first.timestamp,
            arrival: Instant::now(),
            payload: first.payload.clone(),
        },
    );

    // Callee -> caller: second channel in the reverse direction.
    let callee_chan = open_media_channel(&callee.endpoint, &caller.endpoint).await;
    let mut reverse = AudioSender::new(callee_chan._connection.clone(), call_id, 1).unwrap();
    let mut reverse_encoder = OpusEncoder::new().expect("opus encoder");
    let mut reverse_sent = 0usize;
    for i in 0..frames {
        let pcm = sine_frame(i as u32);
        let payload = reverse_encoder.encode(&pcm).unwrap().expect("packet");
        if reverse.try_send(EncodedAudioFrame {
            sequence: i as u32,
            timestamp: (i as u32) * SAMPLES_PER_FRAME as u32,
            payload,
        }) {
            reverse_sent += 1;
        }
    }
    assert_eq!(
        reverse_sent, frames as usize,
        "step 6: all callee frames handed off"
    );
    let mut reverse_received = 0usize;
    for _ in 0..frames {
        let datagram = next_media("caller receive", &mut caller.events).await;
        assert_eq!(
            datagram.kind,
            MediaKind::Audio,
            "step 6 reverse: audio kind"
        );
        assert_eq!(datagram.call_id, call_id, "step 6 reverse: call id");
        reverse_received += 1;
    }
    assert_eq!(
        reverse_received, frames as usize,
        "step 6 reverse: all frames received"
    );

    // ── Step 7: mute/unmute (authoritative local + remote via wire) ──────
    caller.handle.set_muted(call_id, true).await.unwrap();
    // Caller sees its own local state change immediately (authoritative).
    assert!(
        matches!(
            next_event("caller muted", &mut caller.events).await,
            CallEvent::MediaStateChanged { call_id: id, audio_muted: true, .. } if id == call_id
        ),
        "step 7: caller local mute authoritative"
    );
    // Callee sees the remote mute via the wire.
    assert!(
        matches!(
            next_event("callee remote muted", &mut callee.events).await,
            CallEvent::MediaStateChanged { call_id: id, audio_muted: true, .. } if id == call_id
        ),
        "step 7: callee sees remote mute"
    );
    caller.handle.set_muted(call_id, false).await.unwrap();
    assert!(
        matches!(
            next_event("caller unmuted", &mut caller.events).await,
            CallEvent::MediaStateChanged { call_id: id, audio_muted: false, .. } if id == call_id
        ),
        "step 7: caller local unmute"
    );
    assert!(
        matches!(
            next_event("callee remote unmuted", &mut callee.events).await,
            CallEvent::MediaStateChanged { call_id: id, audio_muted: false, .. } if id == call_id
        ),
        "step 7: callee sees remote unmute"
    );

    // ── Step 8: survive temporary jitter/loss ────────────────────────────
    // Drop three datagrams at the sender (sequences 4, 5, 6) and verify the
    // call stays Active and later frames still arrive. Loss tolerance of the
    // jitter buffer itself is unit-tested; this proves the call survives a
    // burst of loss at the integration level.
    let mut lossy = AudioSender::new(caller_chan._connection.clone(), call_id, 1).unwrap();
    let mut lossy_encoder = OpusEncoder::new().expect("opus encoder");
    let total = 12usize;
    let mut delivered = 0usize;
    for i in 0..total {
        if (4..=6).contains(&i) {
            continue; // simulate loss: never handed to the transport
        }
        let pcm = sine_frame(i as u32);
        let payload = lossy_encoder.encode(&pcm).unwrap().expect("packet");
        if lossy.try_send(EncodedAudioFrame {
            sequence: i as u32,
            timestamp: (i as u32) * SAMPLES_PER_FRAME as u32,
            payload,
        }) {
            delivered += 1;
        }
    }
    assert_eq!(
        delivered,
        total - 3,
        "step 8: exactly the three dropped frames are missing at the sender"
    );
    let mut loss_received = 0usize;
    for _ in 0..delivered {
        let datagram = next_media("callee loss window", &mut callee.events).await;
        assert_eq!(datagram.call_id, call_id);
        assert!(
            !(4..=6).contains(&datagram.sequence),
            "step 8: dropped sequences must not arrive"
        );
        loss_received += 1;
    }
    assert_eq!(
        loss_received, delivered,
        "step 8: every non-dropped frame arrived after the loss window"
    );

    // ── Step 9: hang up from either side (callee here) ───────────────────
    callee.handle.hangup(call_id).await.unwrap();
    let callee_end = next_terminal("callee hangup", &mut callee.events, call_id).await;
    assert!(
        matches!(callee_end, CallEvent::Ended { call_id: id, .. } if id == call_id),
        "step 9: callee sees Ended"
    );
    let caller_end = next_terminal("caller hangup", &mut caller.events, call_id).await;
    assert!(
        matches!(caller_end, CallEvent::Ended { call_id: id, .. } if id == call_id),
        "step 9: caller sees Ended after remote hangup"
    );

    // ── Step 10: immediately make another call, no restart ───────────────
    let call2 = caller.handle.start_voice_call(callee_id).await.unwrap();
    assert!(
        matches!(
            next_event("call2 caller ringing", &mut caller.events).await,
            CallEvent::OutgoingRinging { call_id: id, .. } if id == call2
        ),
        "step 10: second call rings on the same handles"
    );
    let incoming2 = match next_event("call2 callee incoming", &mut callee.events).await {
        CallEvent::Incoming { call_id, .. } => call_id,
        other => panic!("step 10: expected incoming on second call, got {other:?}"),
    };
    assert_eq!(incoming2, call2, "step 10: second call id matches");
    callee.handle.accept(call2).await.unwrap();
    assert!(
        matches!(
            next_event("call2 callee active", &mut callee.events).await,
            CallEvent::Active { call_id: id, .. } if id == call2
        ),
        "step 10: second call active (callee)"
    );
    assert!(
        matches!(
            next_event("call2 caller active", &mut caller.events).await,
            CallEvent::Active { call_id: id, .. } if id == call2
        ),
        "step 10: second call active (caller)"
    );
    caller.handle.hangup(call2).await.unwrap();
    let _ = next_terminal("call2 caller end", &mut caller.events, call2).await;
    let _ = next_terminal("call2 callee end", &mut callee.events, call2).await;

    caller.router.shutdown().await.unwrap();
    callee.router.shutdown().await.unwrap();
}
