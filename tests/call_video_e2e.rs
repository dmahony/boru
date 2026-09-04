//! Production two-peer live video-call acceptance tests.
//!
//! These tests deliberately use the real call actor, Iroh datagrams, bounded
//! media routing, and the OpenH264 encode/decode pipeline.  Seeing a
//! `MediaReceived` event is not sufficient: every accepted frame is also fed
//! through `LiveVideoPipeline`, and the test fails unless decoding produces an
//! RGB frame that can be applied to the presentation slot.

use std::time::Duration;

use boru_core::call::manager::{CallBuilder, CallEndReason, CallEvent, CALL_ALPN};
use boru_core::call::media::MediaKind;
use boru_core::call::video::capture::{CaptureConfig, CapturedFrame};
use boru_core::call::video::codec::OpenH264Encoder;
use boru_core::call::video::pipeline::{LiveVideoPipeline, LocalVideoPipeline};
use iroh::{endpoint::presets, protocol::Router, Endpoint};
use tokio::sync::mpsc;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

struct TestNode {
    endpoint: Endpoint,
    handle: boru_core::call::manager::CallHandle,
    events: mpsc::Receiver<CallEvent>,
    router: Router,
}

async fn spawn_node() -> TestNode {
    let endpoint = Endpoint::bind(presets::Minimal)
        .await
        .expect("bind endpoint");
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

async fn connect_probe(client: &Endpoint, server: &Endpoint) {
    let connection = client
        .connect(server.addr(), CALL_ALPN)
        .await
        .expect("probe connection");
    connection.close(0u32.into(), b"probe");
}

async fn next_event(label: &str, events: &mut mpsc::Receiver<CallEvent>) -> CallEvent {
    tokio::time::timeout(Duration::from_secs(8), events.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {label}"))
        .unwrap_or_else(|| panic!("call actor stopped while waiting for {label}"))
}

async fn wait_for_video(
    events: &mut mpsc::Receiver<CallEvent>,
    decoder: &mut LiveVideoPipeline,
    call_id: boru_core::call::CallId,
) -> (u32, u32) {
    loop {
        let event = next_event("decoded video", events).await;
        let CallEvent::MediaReceived { datagram, .. } = event else {
            continue;
        };
        if datagram.call_id != call_id || datagram.kind != MediaKind::Video {
            continue;
        }
        if let Some(frame) = decoder
            .receive_parsed(&datagram)
            .expect("received video must be valid")
        {
            // Application is part of the assertion: install the decoded frame
            // in the latest-frame presentation slot, not merely a counter.
            assert!(
                !frame.bytes.is_empty(),
                "decoded frame must contain RGB data"
            );
            assert_eq!(
                decoder.latest_frame().map(|f| (f.width, f.height)),
                Some((frame.width, frame.height))
            );
            return (frame.width, frame.height);
        }
    }
}

fn pattern_frame(seed: u8, timestamp_us: u64) -> CapturedFrame {
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            data.extend_from_slice(&[
                seed.wrapping_add(x as u8),
                seed.wrapping_add(y as u8),
                seed.wrapping_add((x ^ y) as u8),
            ]);
        }
    }
    CapturedFrame {
        width: WIDTH,
        height: HEIGHT,
        stride: WIDTH as usize * 3,
        timestamp_us,
        data,
    }
}

async fn establish_video_call(
    caller: &mut TestNode,
    callee: &mut TestNode,
) -> boru_core::call::CallId {
    caller
        .handle
        .set_peer_authorized(callee.endpoint.id(), true);
    callee
        .handle
        .set_peer_authorized(caller.endpoint.id(), true);
    connect_probe(&caller.endpoint, &callee.endpoint).await;

    let call_id = caller
        .handle
        .start_video_call(callee.endpoint.id())
        .await
        .expect("start video call");
    assert!(matches!(
        next_event("outgoing ringing", &mut caller.events).await,
        CallEvent::OutgoingRinging { call_id: id, .. } if id == call_id
    ));
    assert!(matches!(
        next_event("incoming video call", &mut callee.events).await,
        CallEvent::Incoming { call_id: id, .. } if id == call_id
    ));
    callee
        .handle
        .accept(call_id)
        .await
        .expect("accept video call");
    assert!(matches!(
        next_event("callee active", &mut callee.events).await,
        CallEvent::Active { call_id: id, .. } if id == call_id
    ));
    assert!(matches!(
        next_event("caller active", &mut caller.events).await,
        CallEvent::Active { call_id: id, .. } if id == call_id
    ));
    call_id
}

#[tokio::test]
async fn two_peers_encode_send_decode_and_apply_video_frames() {
    let mut caller = spawn_node().await;
    let mut callee = spawn_node().await;
    let call_id = establish_video_call(&mut caller, &mut callee).await;

    let config = CaptureConfig {
        width: WIDTH,
        height: HEIGHT,
        frame_interval: Duration::from_millis(33),
    };
    let mut local = LocalVideoPipeline::with_encoder(
        config,
        call_id,
        1,
        256,
        OpenH264Encoder::new().expect("openh264 encoder"),
    );
    let mut decoder = LiveVideoPipeline::new().expect("openh264 decoder");
    let connection = caller
        .endpoint
        .connect(callee.endpoint.addr(), CALL_ALPN)
        .await
        .expect("media datagram connection");
    // An Iroh protocol handler starts its media reader after accepting a
    // bidirectional stream.  Opening and touching the stream mirrors the
    // production media channel and keeps both halves alive for the test.
    let (mut stream_send, _stream_recv) = connection.open_bi().await.expect("open media stream");
    stream_send
        .write_all(b"\x00")
        .await
        .expect("announce media stream");
    let datagrams = local
        .process_frame(pattern_frame(17, 1_000))
        .expect("encode synthetic camera frame");
    assert!(!datagrams.is_empty(), "camera frame must produce datagrams");
    // Send each production packet over QUIC.  The call actor's media reader
    // receives these datagrams, routes them through its bounded worker, and
    // emits the event only after admission checks have succeeded.
    for datagram in &datagrams {
        connection
            .send_datagram(datagram.encode().into())
            .expect("send video datagram");
    }
    assert_eq!(
        wait_for_video(&mut callee.events, &mut decoder, call_id).await,
        (WIDTH, HEIGHT)
    );
    assert_eq!(
        decoder.decoded_frames(),
        1,
        "one frame must be actually decoded"
    );

    caller
        .handle
        .set_camera_enabled(call_id, false)
        .await
        .expect("disable camera");
    assert!(matches!(
        next_event("camera disabled", &mut caller.events).await,
        CallEvent::MediaStateChanged { call_id: id, video_enabled: false, .. } if id == call_id
    ));
    caller
        .handle
        .set_camera_enabled(call_id, true)
        .await
        .expect("enable camera");
    assert!(matches!(
        next_event("camera enabled", &mut caller.events).await,
        CallEvent::MediaStateChanged { call_id: id, video_enabled: true, .. } if id == call_id
    ));

    caller.handle.hangup(call_id).await.expect("hang up");
    assert!(matches!(
        next_event("caller ended", &mut caller.events).await,
        CallEvent::Ended { call_id: id, reason: CallEndReason::LocalHangup } if id == call_id
    ));
    loop {
        if matches!(next_event("callee ended", &mut callee.events).await, CallEvent::Ended { call_id: id, .. } if id == call_id)
        {
            break;
        }
    }
    caller
        .router
        .shutdown()
        .await
        .expect("caller router shutdown");
    callee
        .router
        .shutdown()
        .await
        .expect("callee router shutdown");
}

#[test]
fn codec_negotiation_has_explicit_h264_fallback_and_audio() {
    use boru_core::call::wire::{negotiate, v1_defaults, AudioCodec, VideoCodec};
    let mut caller = v1_defaults();
    let mut callee = v1_defaults();
    let selected = negotiate(&caller, &callee).expect("v1 capabilities must negotiate");
    assert_eq!(selected.audio_codec, AudioCodec::Opus);
    assert_eq!(
        selected.video.as_ref().map(|v| v.codec),
        Some(VideoCodec::H264)
    );

    // A peer that cannot decode video must fall back to a voice-only call,
    // rather than emitting video events that cannot be applied.
    callee.video = None;
    assert_eq!(
        negotiate(&caller, &callee).expect("audio fallback").video,
        None
    );
    caller.video = None;
    assert_eq!(
        negotiate(&caller, &callee)
            .expect("voice-only fallback")
            .video,
        None
    );
}
