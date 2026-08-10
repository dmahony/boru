//! Host-side screen-share session driver: invitation, negotiation, and the
//! capture → encode → transport loop, plus consent-gated remote input.
//!
//! This is the app-facing counterpart of [`super::protocol::ScreenShareProtocol`].
//! The app talks to the driver through [`HostCommand`]s and receives lifecycle
//! events on the shared session-events channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::{
    codec::{CodecConfig, OpenH264Encoder, VideoEncoder},
    permissions::Capability,
    protocol::{self, ControlMessage, SCREEN_SHARE_PROTOCOL_VERSION},
    remote_input::{self, create_platform_backend, InputEvent, NormalizedPointer},
    session::{ScreenShareSessionId, SessionEvent, SessionManager, SessionState},
    transport::{read_unit, QuicScreenTransport, ReadUnit},
    ScreenCapture, ScreenShareError, TestPatternCapture, SCREEN_SHARE_ALPN,
};
use iroh::endpoint::Endpoint;
use iroh::PublicKey;

/// Demo capture geometry: 640x360 @ 15fps keeps encode cost and bandwidth
/// modest on the test machines while still exercising the full pipeline.
pub const DEMO_WIDTH: u32 = 640;
/// Height of the demo capture source.
pub const DEMO_HEIGHT: u32 = 360;
/// Frame rate of the demo capture source.
pub const DEMO_FPS: u32 = 15;

/// Commands the app sends into the host driver task.
#[derive(Debug, Clone)]
pub enum HostCommand {
    /// Host grants control capabilities to the viewer (emits GrantControl).
    GrantControl(Vec<Capability>),
    /// Host revokes control without ending view-only sharing (emits RevokeControl).
    RevokeControl,
}

/// Run a full host session: dial the viewer, negotiate consent, then stream
/// capture frames and apply consent-gated remote input. Emits session events
/// on `events` and honours `stop`/`commands` from the app.
#[allow(clippy::too_many_arguments)]
pub async fn run_host_session(
    endpoint: Endpoint,
    peer: PublicKey,
    local_public: PublicKey,
    conversation_id: u64,
    events: mpsc::Sender<SessionEvent>,
    stop: Arc<AtomicBool>,
    mut commands: mpsc::Receiver<HostCommand>,
) {
    let session_id = ScreenShareSessionId::generate();
    let mut manager = SessionManager::default();
    manager.start_invitation(session_id, local_public, conversation_id);
    let Some(hello) = manager.hello(session_id, vec!["h264".to_string()], DEMO_WIDTH as u16, DEMO_HEIGHT as u16, DEMO_FPS as u16) else { return };
    let addr = endpoint
        .remote_info(peer)
        .await
        .map(|info| iroh::EndpointAddr::from_parts(info.id(), info.into_addrs().map(|a| a.into_addr())))
        .unwrap_or_else(|| iroh::EndpointAddr::new(peer));
    let connection = match endpoint.connect(addr, SCREEN_SHARE_ALPN).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return;
        }
    };
    let transport = match QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()) {
        Ok(transport) => transport,
        Err(error) => {
            let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
            return;
        }
    };
    if let Err(error) = transport.send_control(&ControlMessage::Hello(hello)).await {
        let _ = events.send(SessionEvent::Rejected { session_id, reason: error.to_string() }).await;
        return;
    }
    // Negotiation: wait for the viewer's explicit Accept/Reject, honouring
    // host commands (grants can be issued once streaming starts).
    while !stop.load(Ordering::Relaxed) {
        let accepted = tokio::select! {
            r = connection.accept_bi() => match r {
                Ok((mut send, recv)) => match read_unit(recv).await {
                    Ok(ReadUnit::Control(message)) => {
                        let response = manager.apply_remote(peer, message, &events);
                        if let Some(response) = response {
                            let _ = write_control_response(&mut send, &response).await;
                        }
                        manager.state(session_id) == Some(SessionState::Streaming)
                    }
                    Ok(ReadUnit::Media(_, _)) => false,
                    Err(_) => return,
                },
                Err(_) => return,
            },
            cmd = commands.recv() => match cmd {
                Some(HostCommand::GrantControl(capabilities)) => {
                    if let Some(message) = manager.grant_control(session_id, capabilities, &events) {
                        let _ = transport.send_control(&message).await;
                    }
                    false
                }
                Some(HostCommand::RevokeControl) => {
                    if let Some(message) = manager.revoke_control(session_id, &events) {
                        let _ = transport.send_control(&message).await;
                    }
                    false
                }
                None => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => false,
        };
        if accepted { break; }
        if !matches!(manager.state(session_id), Some(SessionState::AwaitingAcceptance) | Some(SessionState::Connecting) | Some(SessionState::Streaming)) {
            return;
        }
    }
    if stop.load(Ordering::Relaxed) {
        let _ = transport.send_control(&ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id }).await;
        return;
    }
    if manager.state(session_id) != Some(SessionState::Streaming) { return; }
    // Streaming: capture → encode → send, apply consent-gated input, honour
    // host commands and stop.
    let config = CodecConfig { width: DEMO_WIDTH, height: DEMO_HEIGHT, target_fps: DEMO_FPS, ..CodecConfig::default() };
    let Ok(mut capture) = TestPatternCapture::new(DEMO_WIDTH, DEMO_HEIGHT) else { return };
    let Ok(mut encoder) = OpenH264Encoder::new(config) else { return };
    let mut backend = create_platform_backend((DEMO_WIDTH, DEMO_HEIGHT)).await;
    let mut interval = tokio::time::interval(Duration::from_micros(1_000_000 / DEMO_FPS as u64));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if stop.load(Ordering::Relaxed) {
            backend.shutdown().await;
            let _ = transport.send_control(&ControlMessage::EndSession { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id }).await;
            return;
        }
        tokio::select! {
            r = connection.accept_bi() => {
                match r {
                    Ok((mut send, recv)) => match read_unit(recv).await {
                        Ok(ReadUnit::Control(ControlMessage::Input { version: _, session_id: sid, capability, nonce, code, x, y, pressed })) => {
                            // Every input must carry the current grant nonce.
                            let authorized = manager.permissions(sid).is_some_and(|permissions| {
                                remote_input::authorize_nonce(permissions, sid, peer, capability, nonce).is_ok()
                            });
                            if !authorized { continue; }
                            match capability {
                                Capability::ControlPointer => {
                                    if let Some((px, py)) = remote_input::normalize_to_capture(NormalizedPointer { x, y }, (DEMO_WIDTH, DEMO_HEIGHT)) {
                                        let _ = backend.apply(InputEvent { code, capability, token: None, x: px as f32, y: py as f32, pressed }).await;
                                    }
                                }
                                Capability::ControlKeyboard => {
                                    let _ = backend.apply(InputEvent { code, capability, token: None, x: 0.0, y: 0.0, pressed }).await;
                                }
                                _ => {}
                            }
                        }
                        Ok(ReadUnit::Control(message)) => {
                            let response = manager.apply_remote(peer, message, &events);
                            if let Some(response) = response { let _ = write_control_response(&mut send, &response).await; }
                            if manager.state(session_id) == Some(SessionState::Ended) { return; }
                        }
                        Ok(ReadUnit::Media(_, _)) => {}
                        Err(_) => return,
                    },
                    Err(_) => return,
                }
            }
            cmd = commands.recv() => match cmd {
                Some(HostCommand::GrantControl(capabilities)) => {
                    if let Some(message) = manager.grant_control(session_id, capabilities, &events) {
                        let _ = transport.send_control(&message).await;
                    }
                }
                Some(HostCommand::RevokeControl) => {
                    backend.shutdown().await;
                    if let Some(message) = manager.revoke_control(session_id, &events) {
                        let _ = transport.send_control(&message).await;
                    }
                }
                None => return,
            },
            _ = interval.tick() => {
                match capture.capture() {
                    Ok(Some(frame)) => match encoder.encode(&frame) {
                        Ok(encoded) => { if transport.send_frame(&encoded).await.is_err() { return; } }
                        Err(_) => {}
                    },
                    Ok(None) => {}
                    Err(_) => return,
                }
            }
        }
    }
}

/// Write one control response on an accepted stream (mirrors the protocol
/// handler's framing; used for protocol-level rejections during negotiation).
async fn write_control_response(send: &mut iroh::endpoint::SendStream, message: &ControlMessage) -> Result<(), ScreenShareError> {
    use tokio::io::AsyncWriteExt;
    let bytes = protocol::encode(message).map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_u8(0x01).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_u32(bytes.len() as u32).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.write_all(&bytes).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    send.finish().map_err(|e| ScreenShareError::new(e.to_string()))?;
    Ok(())
}
