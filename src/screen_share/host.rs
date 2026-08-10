//! Host-side screen-share session driver: invitation, negotiation, and the
//! capture → encode → transport loop.
//!
//! This is the app-facing counterpart of [`super::protocol::ScreenShareProtocol`]
//! (which handles inbound connections on the viewer side). One task runs per
//! local sharing session; it dials the peer over QUIC, sends a Hello, waits
//! for the viewer's explicit Accept/Reject, then streams synthetic frames
//! until stopped or the viewer ends the session.
#![allow(missing_docs)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use super::{
    codec::{CodecConfig, OpenH264Encoder, VideoEncoder},
    protocol::{self, ControlMessage, SCREEN_SHARE_PROTOCOL_VERSION},
    session::{ScreenShareSessionId, SessionEvent, SessionManager, SessionState},
    transport::{read_unit, QuicScreenTransport, ReadUnit},
    ScreenCapture, ScreenShareError, TestPatternCapture, SCREEN_SHARE_ALPN,
};

/// Demo stream dimensions. Small enough for interactive encode/decode on the
/// test VMs while still showing the full pipeline.
pub const DEMO_WIDTH: u32 = 640;
/// Demo stream height.
pub const DEMO_HEIGHT: u32 = 360;
/// Demo stream frame rate.
pub const DEMO_FPS: u32 = 15;

/// Run one local sharing session until accepted+streamed, stopped, or failed.
///
/// The task drives its own `SessionManager` (the host's view of the session)
/// and emits `SessionEvent`s on the shared events channel so the app can
/// update the persistent sharing indicator and stop controls.
pub async fn run_host_session(
    endpoint: iroh::Endpoint,
    peer: iroh::PublicKey,
    local_public: iroh::PublicKey,
    conversation_id: u64,
    events: mpsc::Sender<SessionEvent>,
    stop: Arc<AtomicBool>,
) {
    let session_id = ScreenShareSessionId::generate();
    let mut manager = SessionManager::default();
    manager.start_invitation(session_id, local_public, conversation_id);
    let Some(hello) = manager.hello(
        session_id,
        vec!["h264".to_string()],
        DEMO_WIDTH as u16,
        DEMO_HEIGHT as u16,
        DEMO_FPS as u16,
    ) else {
        return;
    };

    let addr = endpoint
        .remote_info(peer)
        .await
        .map(|info| {
            iroh::EndpointAddr::from_parts(
                info.id(),
                info.into_addrs().map(|addr| addr.into_addr()),
            )
        })
        .unwrap_or_else(|| iroh::EndpointAddr::new(peer));
    let connection = match endpoint.connect(addr, SCREEN_SHARE_ALPN).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = events
                .send(SessionEvent::Rejected { session_id, reason: error.to_string() })
                .await;
            return;
        }
    };
    let transport = match QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()) {
        Ok(transport) => transport,
        Err(error) => {
            let _ = events
                .send(SessionEvent::Rejected { session_id, reason: error.to_string() })
                .await;
            return;
        }
    };
    if let Err(error) = transport
        .send_control(&ControlMessage::Hello(hello))
        .await
    {
        let _ = events
            .send(SessionEvent::Rejected { session_id, reason: error.to_string() })
            .await;
        return;
    }

    // Negotiation: wait for the viewer's explicit Accept or Reject.
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = transport
                .send_control(&ControlMessage::EndSession {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                })
                .await;
            return;
        }
        let control = tokio::select! {
            r = connection.accept_bi() => match r {
                Ok((mut send, recv)) => match read_unit(recv).await {
                    Ok(ReadUnit::Control(message)) => {
                        let response = manager.apply_remote(peer, message, &events);
                        if let Some(response) = response {
                            let _ = write_control_response(&mut send, &response).await;
                        }
                        Some(())
                    }
                    Ok(ReadUnit::Media(_, _)) => None,
                    Err(_) => return,
                },
                Err(_) => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => None,
        };
        if control.is_some() {
            match manager.state(session_id) {
                Some(SessionState::Streaming) => break,
                Some(SessionState::Ended) | None => return,
                _ => {}
            }
        }
    }

    // Streaming: capture → encode → send until stopped or the viewer ends.
    let config = CodecConfig {
        width: DEMO_WIDTH,
        height: DEMO_HEIGHT,
        target_fps: DEMO_FPS,
        ..CodecConfig::default()
    };
    let Ok(mut capture) = TestPatternCapture::new(DEMO_WIDTH, DEMO_HEIGHT) else { return };
    let Ok(mut encoder) = OpenH264Encoder::new(config) else { return };
    let mut interval = tokio::time::interval(Duration::from_micros(1_000_000 / DEMO_FPS as u64));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = transport
                .send_control(&ControlMessage::EndSession {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                })
                .await;
            return;
        }
        tokio::select! {
            r = connection.accept_bi() => {
                match r {
                    Ok((mut send, recv)) => match read_unit(recv).await {
                        Ok(ReadUnit::Control(message)) => {
                            let response = manager.apply_remote(peer, message, &events);
                            if let Some(response) = response {
                                let _ = write_control_response(&mut send, &response).await;
                            }
                            if matches!(manager.state(session_id), Some(SessionState::Ended) | None) {
                                return;
                            }
                        }
                        Ok(ReadUnit::Media(_, _)) => {}
                        Err(_) => return,
                    },
                    Err(_) => return,
                }
            }
            _ = interval.tick() => {
                match capture.capture() {
                    Ok(Some(frame)) => match encoder.encode(&frame) {
                        Ok(encoded) => {
                            if transport.send_frame(&encoded).await.is_err() {
                                return;
                            }
                        }
                        Err(_) => {}
                    },
                    Ok(None) => {}
                    Err(_) => return,
                }
            }
        }
    }
}

/// Write one control response on a stream the peer already opened.
async fn write_control_response(
    send: &mut iroh::endpoint::SendStream,
    message: &ControlMessage,
) -> Result<(), ScreenShareError> {
    let bytes = protocol::encode(message).map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_u8(0x01).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_u32(bytes.len() as u32).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_all(&bytes).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.finish().map_err(|e| ScreenShareError::new(e.to_string()))?;
    Ok(())
}
