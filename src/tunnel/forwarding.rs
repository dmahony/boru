//! Bounded bidirectional forwarding between a local TCP socket and an Iroh
//! QUIC stream.

use std::sync::Arc;

use iroh::endpoint::{RecvStream, SendStream};
use tokio::{io, net::TcpStream};
use tokio_util::sync::CancellationToken;

use super::service::TunnelLiveInfo;

/// Forward bytes in both directions until both sides reach EOF, cancellation,
/// or an I/O error.
///
/// The two directions are polled concurrently in this task; no per-direction
/// tasks are spawned. EOF in one direction half-closes that direction while
/// allowing the other direction to drain. An error or cancellation stops both
/// directions before this function returns.
///
/// When `live` is provided, forwarded byte counts are accumulated into it so
/// the GUI can display lightweight transfer metrics.
pub(crate) async fn forward_bidirectional(
    local: TcpStream,
    remote_send: SendStream,
    remote_recv: RecvStream,
    cancellation: CancellationToken,
    live: Option<Arc<TunnelLiveInfo>>,
) {
    let (local_read, local_write) = local.into_split();
    let stop = CancellationToken::new();
    let send_direction = forward_to_remote(
        local_read,
        remote_send,
        cancellation.clone(),
        stop.clone(),
        live.clone(),
    );
    let receive_direction =
        forward_to_local(remote_recv, local_write, cancellation, stop.clone(), live);
    let (send_result, receive_result) = tokio::join!(send_direction, receive_direction);

    for (direction, result) in [
        ("local_to_remote", send_result),
        ("remote_to_local", receive_result),
    ] {
        if let Err(error) = result {
            tracing::debug!(direction, %error, "tunnel forwarding stopped");
        }
    }
}

async fn forward_to_remote(
    mut local_read: tokio::net::tcp::OwnedReadHalf,
    mut remote_send: SendStream,
    cancellation: CancellationToken,
    stop: CancellationToken,
    live: Option<Arc<TunnelLiveInfo>>,
) -> io::Result<()> {
    let result = tokio::select! {
        result = io::copy(&mut local_read, &mut remote_send) => {
            let copied = result?;
            if let Some(live) = live.as_ref() {
                live.add_bytes(copied as u64, 0);
            }
            remote_send.finish().map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
        }
        _ = cancellation.cancelled() => Ok(()),
        _ = stop.cancelled() => Ok(()),
    };
    if result.is_err() {
        stop.cancel();
    }
    result
}

async fn forward_to_local(
    mut remote_recv: RecvStream,
    mut local_write: tokio::net::tcp::OwnedWriteHalf,
    cancellation: CancellationToken,
    stop: CancellationToken,
    live: Option<Arc<TunnelLiveInfo>>,
) -> io::Result<()> {
    let result = tokio::select! {
        result = io::copy(&mut remote_recv, &mut local_write) => {
            let copied = result?;
            if let Some(live) = live.as_ref() {
                live.add_bytes(0, copied as u64);
            }
            io::AsyncWriteExt::shutdown(&mut local_write).await
        }
        _ = cancellation.cancelled() => Ok(()),
        _ = stop.cancelled() => Ok(()),
    };
    if result.is_err() {
        stop.cancel();
    }
    result
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh::{endpoint::presets, protocol::Router, Endpoint};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        time::{sleep, timeout},
    };
    use tokio_util::sync::CancellationToken;

    use super::forward_bidirectional;
    use crate::tunnel::{TunnelProtocol, BORU_TUNNEL_ALPN};

    struct Fixture {
        local_client: TcpStream,
        local_server: TcpStream,
        remote_client_send: iroh::endpoint::SendStream,
        remote_client_recv: iroh::endpoint::RecvStream,
        remote_server_send: iroh::endpoint::SendStream,
        remote_server_recv: iroh::endpoint::RecvStream,
        router: Router,
        client: Endpoint,
    }

    async fn fixture() -> anyhow::Result<Fixture> {
        let tcp_listener = TcpListener::bind("127.0.0.1:0").await?;
        let tcp_addr = tcp_listener.local_addr()?;
        let local_client = TcpStream::connect(tcp_addr).await?;
        let (local_server, _) = tcp_listener.accept().await?;

        let listener = Endpoint::bind(presets::Minimal).await?;
        let client = Endpoint::bind(presets::Minimal).await?;
        let tunnel = TunnelProtocol::new();
        let router = Router::builder(listener)
            .accept(BORU_TUNNEL_ALPN, tunnel.clone())
            .spawn();
        let connection = client
            .connect(router.endpoint().addr(), BORU_TUNNEL_ALPN)
            .await?;
        let (remote_client_send, remote_client_recv) = connection.open_bi().await?;
        let mut remote_client_send = remote_client_send;
        remote_client_send.write_all(b"seed").await?;
        let (remote_server_send, remote_server_recv) =
            timeout(Duration::from_secs(2), tunnel.accept_stream())
                .await?
                .ok_or_else(|| anyhow::anyhow!("tunnel stream channel closed"))?;

        Ok(Fixture {
            local_client,
            local_server,
            remote_client_send,
            remote_client_recv,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        })
    }

    #[tokio::test]
    async fn eof_finishes_remote_send_and_preserves_reverse_direction() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let Fixture {
            local_client,
            local_server,
            mut remote_client_send,
            mut remote_client_recv,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        } = fixture;
        let cancellation = CancellationToken::new();
        let forwarding = tokio::spawn(forward_bidirectional(
            local_server,
            remote_server_send,
            remote_server_recv,
            cancellation,
            None,
        ));

        let mut local_client = local_client;
        let mut seed = [0; 4];
        local_client.read_exact(&mut seed).await?;
        assert_eq!(&seed, b"seed");
        local_client.write_all(b"request").await?;
        local_client.shutdown().await?;
        assert_eq!(remote_client_recv.read_to_end(1024).await?, b"request");
        remote_client_send.write_all(b"response").await?;
        remote_client_send.finish()?;
        let mut response = Vec::new();
        local_client.read_to_end(&mut response).await?;
        assert_eq!(response, b"response");
        timeout(Duration::from_secs(2), forwarding).await??;
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_returns_without_orphaning_forwarding() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let cancellation = CancellationToken::new();
        let forwarding = tokio::spawn(forward_bidirectional(
            fixture.local_server,
            fixture.remote_server_send,
            fixture.remote_server_recv,
            cancellation.clone(),
            None,
        ));
        sleep(Duration::from_millis(20)).await;
        cancellation.cancel();
        timeout(Duration::from_secs(2), forwarding).await??;
        fixture.router.shutdown().await?;
        fixture.client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn remote_reset_stops_both_directions() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let Fixture {
            local_client: _,
            local_server,
            remote_client_send,
            remote_client_recv: _,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        } = fixture;
        let cancellation = CancellationToken::new();
        let forwarding = tokio::spawn(forward_bidirectional(
            local_server,
            remote_server_send,
            remote_server_recv,
            cancellation.clone(),
            None,
        ));
        let mut remote_client_send = remote_client_send;
        sleep(Duration::from_millis(20)).await;
        remote_client_send.reset(iroh::endpoint::VarInt::from_u32(1))?;
        sleep(Duration::from_millis(20)).await;
        cancellation.cancel();
        timeout(Duration::from_secs(2), forwarding).await??;
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }
}
