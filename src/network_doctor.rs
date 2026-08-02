//! Optional network diagnostics built on Boru's tunnel raw-stream primitive.
//!
//! [`NetworkDoctor`] deliberately does not create an endpoint or duplicate
//! connection setup. It uses an already-established Iroh [`Connection`] and
//! the [`TunnelProtocol`] raw stream queue, so diagnostics exercise the same
//! transport path as tunnels. Throughput is a separate, explicitly-invoked
//! bounded operation.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use iroh::endpoint::Connection;

use crate::tunnel::{read_frame, write_frame, TunnelProtocol, TunnelStream};

/// Maximum payload accepted by an opt-in throughput sample.
pub const MAX_THROUGHPUT_SAMPLE_BYTES: usize = 256 * 1024;
/// Maximum time spent waiting for a diagnostic response.
pub const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(10);

const DIAGNOSTIC_PROTOCOL_VERSION: u16 = 1;
const PING_PAYLOAD: &[u8] = b"boru-network-doctor-ping";

/// The route used by an established Iroh connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRoute {
    /// The connection uses a direct IP path.
    Direct,
    /// The connection uses an Iroh relay.
    Relay,
    /// A custom Iroh transport was selected.
    Custom,
}

/// Result of the identity/address/connection/stream/round-trip checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkDiagnosticReport {
    /// Whether the remote endpoint identity was authenticated.
    pub identity: bool,
    /// Whether the remote address was available.
    pub address_resolution: bool,
    /// Whether the QUIC connection was established.
    pub connection: bool,
    /// Whether a tunnel raw stream was opened and completed a handshake.
    pub stream: bool,
    /// Measured round-trip time for the bounded ping, if successful.
    pub round_trip_ms: Option<u64>,
    /// The transport route observed for the connection.
    pub route: Option<DiagnosticRoute>,
    /// Authenticated remote endpoint identity, when available.
    pub remote_id: Option<String>,
}

/// An explicitly requested, bounded throughput sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThroughputSample {
    /// Number of payload bytes echoed by the peer.
    pub bytes: usize,
    /// Elapsed round-trip time in milliseconds.
    pub elapsed_ms: u64,
    /// Calculated rate in bytes per second.
    pub bytes_per_second: u64,
}

#[derive(Debug, Serialize, Deserialize)]
enum DiagnosticRequest {
    Ping { version: u16, payload: Vec<u8> },
    Throughput { version: u16, payload: Vec<u8> },
}

#[derive(Debug, Serialize, Deserialize)]
enum DiagnosticResponse {
    Pong { version: u16, payload: Vec<u8> },
}

/// Client-side network doctor using an existing shared endpoint connection.
#[derive(Debug)]
pub struct NetworkDoctor;

impl NetworkDoctor {
    /// Run identity, address, connection, stream, route, and round-trip checks.
    pub async fn check(connection: &Connection) -> anyhow::Result<NetworkDiagnosticReport> {
        let remote_id = connection.remote_id().to_string();
        let route = route_for(connection);
        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &DiagnosticRequest::Ping {
                version: DIAGNOSTIC_PROTOCOL_VERSION,
                payload: PING_PAYLOAD.to_vec(),
            },
        )
        .await?;
        let started = Instant::now();
        let response = timeout(
            DIAGNOSTIC_TIMEOUT,
            read_frame::<DiagnosticResponse>(&mut recv),
        )
        .await
        .map_err(|_| anyhow::anyhow!("network diagnostic timed out"))??;
        let elapsed = started.elapsed();
        send.finish()?;
        match response {
            DiagnosticResponse::Pong { version, payload }
                if version == DIAGNOSTIC_PROTOCOL_VERSION && payload == PING_PAYLOAD =>
            {
                Ok(NetworkDiagnosticReport {
                    identity: true,
                    address_resolution: true,
                    connection: true,
                    stream: true,
                    round_trip_ms: Some(elapsed.as_millis() as u64),
                    route,
                    remote_id: Some(remote_id),
                })
            }
            _ => anyhow::bail!("invalid network diagnostic response"),
        }
    }

    /// Run a user-requested bounded throughput sample.
    ///
    /// This method is never called by [`check`]. Callers must explicitly opt
    /// in and the payload is capped to [`MAX_THROUGHPUT_SAMPLE_BYTES`].
    pub async fn throughput_sample(
        connection: &Connection,
        bytes: usize,
    ) -> anyhow::Result<ThroughputSample> {
        anyhow::ensure!(
            bytes <= MAX_THROUGHPUT_SAMPLE_BYTES,
            "throughput sample exceeds limit"
        );
        let payload = vec![0x5a; bytes];
        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(
            &mut send,
            &DiagnosticRequest::Throughput {
                version: DIAGNOSTIC_PROTOCOL_VERSION,
                payload: payload.clone(),
            },
        )
        .await?;
        let started = Instant::now();
        let response = timeout(
            DIAGNOSTIC_TIMEOUT,
            read_frame::<DiagnosticResponse>(&mut recv),
        )
        .await
        .map_err(|_| anyhow::anyhow!("throughput sample timed out"))??;
        let elapsed = started.elapsed();
        send.finish()?;
        match response {
            DiagnosticResponse::Pong {
                version,
                payload: echoed,
            } if version == DIAGNOSTIC_PROTOCOL_VERSION && echoed == payload => {
                let millis = elapsed.as_millis().max(1) as u64;
                Ok(ThroughputSample {
                    bytes: bytes,
                    elapsed_ms: millis,
                    bytes_per_second: (bytes as u64).saturating_mul(1000) / millis,
                })
            }
            _ => anyhow::bail!("invalid throughput response"),
        }
    }

    /// Serve diagnostic requests arriving on the tunnel raw-stream queue.
    pub async fn serve(protocol: &TunnelProtocol) -> anyhow::Result<()> {
        while let Some(stream) = protocol.accept_stream().await {
            tokio::spawn(async move {
                if let Err(error) = serve_stream(stream).await {
                    tracing::debug!(%error, "network diagnostic stream closed");
                }
            });
        }
        Ok(())
    }
}

async fn serve_stream((mut send, mut recv): TunnelStream) -> anyhow::Result<()> {
    let request = timeout(
        DIAGNOSTIC_TIMEOUT,
        read_frame::<DiagnosticRequest>(&mut recv),
    )
    .await
    .map_err(|_| anyhow::anyhow!("network diagnostic request timed out"))??;
    let (version, payload) = match request {
        DiagnosticRequest::Ping { version, payload }
        | DiagnosticRequest::Throughput { version, payload } => (version, payload),
    };
    anyhow::ensure!(
        version == DIAGNOSTIC_PROTOCOL_VERSION,
        "diagnostic protocol mismatch"
    );
    write_frame(&mut send, &DiagnosticResponse::Pong { version, payload }).await?;
    send.finish()?;
    Ok(())
}

fn route_for(connection: &Connection) -> Option<DiagnosticRoute> {
    let paths = connection.paths();
    paths
        .iter()
        .find(|path| path.is_selected())
        .or_else(|| paths.iter().next())
        .map(|path| {
            if path.is_ip() {
                DiagnosticRoute::Direct
            } else if path.is_relay() {
                DiagnosticRoute::Relay
            } else {
                DiagnosticRoute::Custom
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{endpoint::presets, protocol::Router, Endpoint};

    #[tokio::test]
    async fn doctor_checks_raw_stream_and_opt_in_throughput() -> anyhow::Result<()> {
        let server = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let protocol = TunnelProtocol::new();
        let router = Router::builder(server)
            .accept(crate::tunnel::BORU_TUNNEL_ALPN, protocol.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), crate::tunnel::BORU_TUNNEL_ALPN)
            .await?;
        let server_task = tokio::spawn(async move { NetworkDoctor::serve(&protocol).await });

        let report = NetworkDoctor::check(&connection).await?;
        assert!(report.identity && report.address_resolution && report.connection && report.stream);
        assert_eq!(report.route, Some(DiagnosticRoute::Direct));
        assert!(report.round_trip_ms.is_some());

        let sample = NetworkDoctor::throughput_sample(&connection, 1024).await?;
        assert_eq!(sample.bytes, 1024);
        assert!(sample.bytes_per_second > 0);
        assert!(
            NetworkDoctor::throughput_sample(&connection, MAX_THROUGHPUT_SAMPLE_BYTES + 1)
                .await
                .is_err()
        );

        connection.close(0u32.into(), b"done");
        server_task.abort();
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }
}
