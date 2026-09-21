//! Companion-device protocol on the existing authenticated Iroh endpoint.
//!
//! The protocol is intentionally small and fail-closed. `/boru/companion/1`
//! uses length-prefixed JSON frames (big-endian `u32` length followed by one
//! JSON value). Frame length is checked before allocation. Pairing is a
//! separate unauthorised phase: it can report only that approval is required.
//! Profile, contact, and message data are unavailable until the remote endpoint
//! has been explicitly approved.

use std::{collections::HashSet, sync::Arc, time::Duration};

use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
    EndpointId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
    time::timeout,
};

/// ALPN for the companion-device protocol.
pub const COMPANION_ALPN: &[u8] = b"/boru/companion/1";
/// Current companion wire version.
pub const COMPANION_WIRE_VERSION: u16 = 1;
/// Maximum encoded JSON frame, checked before allocation.
pub const MAX_COMPANION_FRAME_BYTES: usize = 64 * 1024;
/// Deadline for each handshake/request frame.
pub const COMPANION_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// Capabilities exposed before approval. These are protocol names only.
pub const PUBLIC_CAPABILITIES: &[&str] = &["pairing"];

/// A companion request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompanionRequest {
    /// Initial negotiation. No account data is included.
    Hello {
        /// Protocol versions understood by the peer.
        versions: Vec<u16>,
        /// Protocol capability names understood by the peer.
        capabilities: Vec<String>,
    },
    /// Unauthorised request to begin pairing approval.
    PairingRequest {
        /// User-facing device label; never used as an identity.
        device_name: String,
    },
    /// Request requiring prior explicit approval.
    AuthenticatedRequest {
        /// Caller-chosen request correlation identifier.
        request_id: String,
        /// Requested companion method.
        method: String,
        /// Method parameters, interpreted only after approval.
        params: Value,
    },
}

/// A companion response frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompanionResponse {
    /// Version and public protocol capabilities accepted.
    HelloAccepted {
        /// Negotiated protocol version.
        version: u16,
        /// Capability names safe to expose before approval.
        capabilities: Vec<String>,
    },
    /// The peer must be approved before authenticated methods are available.
    ApprovalRequired,
    /// Pairing was accepted for later approval; no profile data is disclosed.
    PairingPending,
    /// Stable protocol errors that do not disclose local state.
    Error {
        /// Stable error code.
        code: CompanionErrorCode,
    },
    /// Successful method response (currently reserved for approved extensions).
    Result {
        /// Request correlation identifier.
        request_id: String,
        /// Result payload.
        value: Value,
    },
}

/// Stable companion protocol errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanionErrorCode {
    /// The frame could not be decoded as the expected JSON request.
    MalformedFrame,
    /// The length prefix exceeded the allocation limit.
    FrameTooLarge,
    /// No mutually supported protocol version was advertised.
    IncompatibleVersion,
    /// The requested method is not implemented.
    UnknownMethod,
    /// The endpoint has not received explicit user approval.
    NotApproved,
    /// The listener is disabled by configuration.
    Disabled,
}

/// Policy/state used by the protocol handler.
#[derive(Debug, Clone, Default)]
pub struct CompanionPolicy {
    approved: Arc<RwLock<HashSet<EndpointId>>>,
}

impl CompanionPolicy {
    /// Create an empty policy. Pairing never implicitly approves a device.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the authenticated endpoint identity approved after user consent.
    pub async fn approve(&self, endpoint: EndpointId) {
        self.approved.write().await.insert(endpoint);
    }

    async fn is_approved(&self, endpoint: EndpointId) -> bool {
        self.approved.read().await.contains(&endpoint)
    }
}

/// Handler registered on the shared Iroh router when the feature is enabled.
#[derive(Debug, Clone)]
pub struct CompanionProtocolHandler {
    policy: CompanionPolicy,
    enabled: bool,
}

impl CompanionProtocolHandler {
    /// Build a handler. `enabled = false` rejects without registering data paths.
    pub fn new(policy: CompanionPolicy, enabled: bool) -> Self {
        Self { policy, enabled }
    }
}

impl ProtocolHandler for CompanionProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let policy = self.policy.clone();
        let enabled = self.enabled;
        let remote = connection.remote_id();
        let lifetime = connection.clone();
        loop {
            let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                break;
            };
            let response = if !enabled {
                CompanionResponse::Error {
                    code: CompanionErrorCode::Disabled,
                }
            } else {
                match timeout(COMPANION_FRAME_TIMEOUT, read_frame(&mut recv)).await {
                    Ok(Ok(request)) => handle_request(request, remote, &policy).await,
                    Ok(Err(FrameError::TooLarge)) => CompanionResponse::Error {
                        code: CompanionErrorCode::FrameTooLarge,
                    },
                    Ok(Err(FrameError::Malformed)) | Err(_) => CompanionResponse::Error {
                        code: CompanionErrorCode::MalformedFrame,
                    },
                    Ok(Err(FrameError::Io)) => break,
                }
            };
            if write_frame(&mut send, &response).await.is_err() {
                break;
            }
            let _ = send.finish();
        }
        let _ = lifetime.closed().await;
        Ok(())
    }
}

async fn handle_request(
    request: CompanionRequest,
    remote: EndpointId,
    policy: &CompanionPolicy,
) -> CompanionResponse {
    match request {
        CompanionRequest::Hello { versions, .. } if versions.contains(&COMPANION_WIRE_VERSION) => {
            CompanionResponse::HelloAccepted {
                version: COMPANION_WIRE_VERSION,
                capabilities: PUBLIC_CAPABILITIES
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
            }
        }
        CompanionRequest::Hello { .. } => CompanionResponse::Error {
            code: CompanionErrorCode::IncompatibleVersion,
        },
        CompanionRequest::PairingRequest { .. } => CompanionResponse::PairingPending,
        CompanionRequest::AuthenticatedRequest {
            request_id, method, ..
        } => {
            if !policy.is_approved(remote).await {
                return CompanionResponse::ApprovalRequired;
            }
            let _ = (request_id, method);
            CompanionResponse::Error {
                code: CompanionErrorCode::UnknownMethod,
            }
        }
    }
}

#[derive(Debug)]
enum FrameError {
    TooLarge,
    Malformed,
    Io,
}

async fn read_frame(recv: &mut RecvStream) -> Result<CompanionRequest, FrameError> {
    let length = recv.read_u32().await.map_err(|_| FrameError::Io)? as usize;
    if length > MAX_COMPANION_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    let mut bytes = vec![0; length];
    recv.read_exact(&mut bytes)
        .await
        .map_err(|_| FrameError::Io)?;
    serde_json::from_slice(&bytes).map_err(|_| FrameError::Malformed)
}

async fn write_frame(send: &mut SendStream, value: &CompanionResponse) -> Result<(), FrameError> {
    let bytes = serde_json::to_vec(value).map_err(|_| FrameError::Malformed)?;
    if bytes.len() > MAX_COMPANION_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    send.write_u32(bytes.len() as u32)
        .await
        .map_err(|_| FrameError::Io)?;
    send.write_all(&bytes).await.map_err(|_| FrameError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{endpoint::presets, protocol::Router, Endpoint};

    async fn endpoints(enabled: bool) -> (Endpoint, Router, CompanionPolicy) {
        let server = Endpoint::bind(presets::Minimal).await.unwrap();
        let policy = CompanionPolicy::new();
        let router = Router::builder(server.clone())
            .accept(
                COMPANION_ALPN,
                CompanionProtocolHandler::new(policy.clone(), enabled),
            )
            .spawn();
        (server, router, policy)
    }

    async fn round_trip(
        client: &Endpoint,
        router: &Router,
        request: &CompanionRequest,
    ) -> CompanionResponse {
        let conn = client
            .connect(router.endpoint().addr(), COMPANION_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let bytes = serde_json::to_vec(request).unwrap();
        send.write_u32(bytes.len() as u32).await.unwrap();
        send.write_all(&bytes).await.unwrap();
        let len = recv.read_u32().await.unwrap() as usize;
        let mut response = vec![0; len];
        recv.read_exact(&mut response).await.unwrap();
        serde_json::from_slice(&response).unwrap()
    }

    #[tokio::test]
    async fn v1_negotiation_and_pairing_are_data_free() {
        let (server, router, _) = endpoints(true).await;
        let client = Endpoint::bind(presets::Minimal).await.unwrap();
        let response = round_trip(
            &client,
            &router,
            &CompanionRequest::Hello {
                versions: vec![1],
                capabilities: vec!["pairing".into()],
            },
        )
        .await;
        assert!(matches!(
            response,
            CompanionResponse::HelloAccepted { version: 1, .. }
        ));
        let response = round_trip(
            &client,
            &router,
            &CompanionRequest::PairingRequest {
                device_name: "phone".into(),
            },
        )
        .await;
        assert_eq!(response, CompanionResponse::PairingPending);
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn incompatible_unknown_and_disabled_requests_are_rejected() {
        let (server, router, _) = endpoints(true).await;
        let client = Endpoint::bind(presets::Minimal).await.unwrap();
        assert_eq!(
            round_trip(
                &client,
                &router,
                &CompanionRequest::Hello {
                    versions: vec![99],
                    capabilities: vec![]
                }
            )
            .await,
            CompanionResponse::Error {
                code: CompanionErrorCode::IncompatibleVersion
            }
        );
        assert_eq!(
            round_trip(
                &client,
                &router,
                &CompanionRequest::AuthenticatedRequest {
                    request_id: "1".into(),
                    method: "messages.list".into(),
                    params: Value::Null
                }
            )
            .await,
            CompanionResponse::ApprovalRequired
        );
        client.close().await;
        server.close().await;

        let (server, router, _) = endpoints(false).await;
        let client = Endpoint::bind(presets::Minimal).await.unwrap();
        assert_eq!(
            round_trip(
                &client,
                &router,
                &CompanionRequest::Hello {
                    versions: vec![1],
                    capabilities: vec![]
                }
            )
            .await,
            CompanionResponse::Error {
                code: CompanionErrorCode::Disabled
            }
        );
        client.close().await;
        server.close().await;
    }

    #[test]
    fn wire_shape_is_stable() {
        let json = serde_json::to_string(&CompanionRequest::Hello {
            versions: vec![1],
            capabilities: vec!["pairing".into()],
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","versions":[1],"capabilities":["pairing"]}"#
        );
    }
}
