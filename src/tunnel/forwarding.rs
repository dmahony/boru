//! Bounded bidirectional forwarding between a local TCP socket and an Iroh
//! QUIC stream.
//!
//! Forwarding is activity-aware: every successfully transferred chunk resets a
//! shared idle timer. When the tunnel has been completely idle for the
//! configured duration both halves are shut down cleanly and
//! [`ForwardEnd::IdleTimeout`] is reported instead of a hard lifetime expiry.
//! I/O errors are returned to the caller rather than swallowed.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::{net::TcpStream, time};
use tokio_util::sync::CancellationToken;

use super::service::TunnelLiveInfo;

/// Chunk size used by the activity-aware copy loop.
const COPY_CHUNK_SIZE: usize = 64 * 1024;

/// How bidirectional forwarding ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForwardEnd {
    /// Both directions reached EOF and the tunnel closed gracefully.
    Eof,
    /// No bytes were transferred in either direction for the idle duration.
    IdleTimeout,
    /// The tunnel was cancelled (e.g. the tunnel was removed or revoked).
    Cancelled,
}

/// Whether copied bytes are counted as sent or received for live metrics.
#[derive(Debug, Clone, Copy)]
enum LiveDirection {
    Sent,
    Received,
}

/// Per-direction termination cause, used to classify the combined outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectionEnd {
    Eof,
    Cancelled,
}

/// Shared record of the last instant at which either direction transferred
/// bytes, used to implement a resettable idle timeout.
///
/// `touch` is called after every successful chunk transfer in either
/// direction; the idle watchdog polls [`ActivityTracker::idle_for`] to decide
/// when the tunnel has been idle long enough to close.
#[derive(Clone, Debug)]
pub(crate) struct ActivityTracker {
    last_activity: Arc<Mutex<time::Instant>>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self {
            last_activity: Arc::new(Mutex::new(time::Instant::now())),
        }
    }

    /// Record that bytes were successfully transferred just now.
    pub fn touch(&self) {
        *self
            .last_activity
            .lock()
            .expect("tunnel activity tracker lock poisoned") = time::Instant::now();
    }

    /// Duration since the last recorded activity.
    pub fn idle_for(&self) -> Duration {
        self.last_activity
            .lock()
            .expect("tunnel activity tracker lock poisoned")
            .elapsed()
    }
}

/// Forward bytes in both directions until both sides reach EOF, cancellation,
/// the configured idle duration elapses, or an I/O error occurs.
///
/// The two directions are polled concurrently in this task; no per-direction
/// tasks are spawned. EOF in one direction half-closes that direction while
/// allowing the other direction to drain. An error or cancellation stops both
/// directions before this function returns.
///
/// A background watchdog watches for inactivity. Whenever bytes are
/// successfully transferred in either direction the idle timer resets; when
/// the tunnel has been idle for `idle_timeout` the watchdog cancels the shared
/// stop token so both halves shut down cleanly and [`ForwardEnd::IdleTimeout`]
/// is returned.
///
/// When `live` is provided, forwarded byte counts are accumulated into it so
/// the GUI can display lightweight transfer metrics.
pub(crate) async fn forward_bidirectional(
    local: TcpStream,
    remote_send: SendStream,
    remote_recv: RecvStream,
    cancellation: CancellationToken,
    idle_timeout: Duration,
    live: Option<Arc<TunnelLiveInfo>>,
) -> io::Result<ForwardEnd> {
    let (local_read, local_write) = local.into_split();
    let stop = CancellationToken::new();
    let activity = ActivityTracker::new();
    let idle_fired = Arc::new(AtomicBool::new(false));

    let watchdog = tokio::spawn(idle_watchdog(
        activity.clone(),
        idle_timeout,
        stop.clone(),
        Arc::clone(&idle_fired),
    ));
    let send_direction = forward_to_remote(
        local_read,
        remote_send,
        cancellation.clone(),
        stop.clone(),
        activity.clone(),
        live.clone(),
    );
    let receive_direction = forward_to_local(
        remote_recv,
        local_write,
        cancellation,
        stop.clone(),
        activity,
        live,
    );
    let (send_result, receive_result) = tokio::join!(send_direction, receive_direction);
    watchdog.abort();

    // An I/O error in either direction is the most important signal.
    let error = match (&send_result, &receive_result) {
        (Err(error), _) | (_, Err(error)) => Some(io::Error::new(error.kind(), error.to_string())),
        _ => None,
    };
    if let Some(error) = error {
        return Err(error);
    }
    if idle_fired.load(Ordering::Relaxed) {
        return Ok(ForwardEnd::IdleTimeout);
    }
    match (send_result, receive_result) {
        (Ok(DirectionEnd::Eof), Ok(DirectionEnd::Eof)) => Ok(ForwardEnd::Eof),
        _ => Ok(ForwardEnd::Cancelled),
    }
}

/// Watch for inactivity and cancel the forwarding directions when the tunnel
/// has been idle for `idle_timeout`.
///
/// Returns as soon as `stop` is cancelled by the directions themselves (EOF or
/// error), so it never lingers after forwarding ends; the caller also aborts it
/// defensively.
async fn idle_watchdog(
    activity: ActivityTracker,
    idle_timeout: Duration,
    stop: CancellationToken,
    idle_fired: Arc<AtomicBool>,
) {
    loop {
        let remaining = idle_timeout.saturating_sub(activity.idle_for());
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = time::sleep(remaining) => {
                // Re-check after sleeping: traffic may have arrived while we
                // were waiting, in which case the timer restarts.
                if activity.idle_for() >= idle_timeout {
                    idle_fired.store(true, Ordering::Relaxed);
                    stop.cancel();
                    return;
                }
            }
        }
    }
}

async fn forward_to_remote(
    mut local_read: tokio::net::tcp::OwnedReadHalf,
    mut remote_send: SendStream,
    cancellation: CancellationToken,
    stop: CancellationToken,
    activity: ActivityTracker,
    live: Option<Arc<TunnelLiveInfo>>,
) -> io::Result<DirectionEnd> {
    let result: io::Result<DirectionEnd> = tokio::select! {
        result = copy_with_activity(
            &mut local_read,
            &mut remote_send,
            &activity,
            live.as_ref(),
            LiveDirection::Sent,
        ) => match result {
            Ok(_copied) => remote_send
                .finish()
                .map_err(|error| {
                    io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())
                })
                .map(|()| DirectionEnd::Eof),
            Err(error) => Err(error),
        },
        _ = cancellation.cancelled() => Ok(DirectionEnd::Cancelled),
        _ = stop.cancelled() => Ok(DirectionEnd::Cancelled),
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
    activity: ActivityTracker,
    live: Option<Arc<TunnelLiveInfo>>,
) -> io::Result<DirectionEnd> {
    let result: io::Result<DirectionEnd> = tokio::select! {
        result = copy_with_activity(
            &mut remote_recv,
            &mut local_write,
            &activity,
            live.as_ref(),
            LiveDirection::Received,
        ) => match result {
            Ok(_copied) => {
                io::AsyncWriteExt::shutdown(&mut local_write)
                    .await
                    .map(|()| DirectionEnd::Eof)
            }
            Err(error) => Err(error),
        },
        _ = cancellation.cancelled() => Ok(DirectionEnd::Cancelled),
        _ = stop.cancelled() => Ok(DirectionEnd::Cancelled),
    };
    if result.is_err() {
        stop.cancel();
    }
    result
}

/// Copy bytes from `reader` to `writer` in bounded chunks, recording activity
/// and live byte counters after each successful transfer.
///
/// A chunk only counts as activity once the write succeeds, so slow or stuck
/// peers do not keep the tunnel alive by trickling partial reads.
async fn copy_with_activity<R, W>(
    reader: &mut R,
    writer: &mut W,
    activity: &ActivityTracker,
    live: Option<&Arc<TunnelLiveInfo>>,
    live_direction: LiveDirection,
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; COPY_CHUNK_SIZE];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(total);
        }
        writer.write_all(&buffer[..read]).await?;
        total += read as u64;
        activity.touch();
        if let Some(live) = live {
            match live_direction {
                LiveDirection::Sent => live.add_bytes(read as u64, 0),
                LiveDirection::Received => live.add_bytes(0, read as u64),
            }
        }
    }
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

    use super::{forward_bidirectional, ForwardEnd};
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

    /// Drain the fixture's initial "seed" bytes from the local client side and
    /// assert they arrived intact.
    async fn consume_seed(local_client: &mut TcpStream) -> anyhow::Result<()> {
        let mut seed = [0u8; 4];
        local_client.read_exact(&mut seed).await?;
        anyhow::ensure!(&seed == b"seed", "fixture seed did not arrive");
        Ok(())
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
            Duration::from_secs(30),
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
            Duration::from_secs(30),
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
            Duration::from_secs(30),
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

    #[tokio::test]
    async fn idle_period_without_traffic_closes_tunnel() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let Fixture {
            mut local_client,
            local_server,
            remote_client_send,
            remote_client_recv: _,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        } = fixture;
        // Keep the remote send half alive so it does not reset the stream while
        // the owner is still forwarding; it is only used to check shutdown.
        let _remote_client_send = remote_client_send;
        let cancellation = CancellationToken::new();
        let forwarding = tokio::spawn(forward_bidirectional(
            local_server,
            remote_server_send,
            remote_server_recv,
            cancellation,
            Duration::from_millis(150),
            None,
        ));

        // No traffic at all: the tunnel must close itself with an idle timeout
        // well before the old hard five-minute lifetime would have fired.
        let end = timeout(Duration::from_secs(2), forwarding)
            .await?
            .map_err(|error| anyhow::anyhow!("forwarding task panicked: {error}"))??;
        assert_eq!(
            end,
            ForwardEnd::IdleTimeout,
            "an idle tunnel must report IdleTimeout, got {end:?}"
        );

        // Both halves shut down cleanly: the local TCP socket reaches EOF
        // (or reset) promptly instead of staying open.
        let mut buf = [0u8; 4];
        match timeout(Duration::from_secs(2), local_client.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Ok(_)) | Ok(Err(_)) => {}
            Err(_) => anyhow::bail!("local half was not shut down after idle timeout"),
        }
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn continuous_traffic_keeps_tunnel_alive_past_idle_boundary() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let Fixture {
            mut local_client,
            local_server,
            mut remote_client_send,
            mut remote_client_recv,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        } = fixture;
        let cancellation = CancellationToken::new();
        let mut forwarding = tokio::spawn(forward_bidirectional(
            local_server,
            remote_server_send,
            remote_server_recv,
            cancellation.clone(),
            Duration::from_millis(100),
            None,
        ));
        consume_seed(&mut local_client).await?;

        // Keep traffic flowing for several idle periods. With a true idle
        // timeout the tunnel must survive; a hard lifetime limit of the same
        // size would have torn it down mid-transfer.
        for round in 0..6u32 {
            let payload = round.to_be_bytes();
            local_client.write_all(&payload).await?;
            let mut echoed = [0u8; 4];
            remote_client_recv.read_exact(&mut echoed).await?;
            assert_eq!(echoed, payload);
            remote_client_send.write_all(&payload).await?;
            local_client.read_exact(&mut echoed).await?;
            assert_eq!(echoed, payload);
            sleep(Duration::from_millis(40)).await;
        }

        // Still alive after ~6 * (40 ms + round trips) >> the 100 ms idle
        // timeout.
        assert!(
            timeout(Duration::from_millis(50), &mut forwarding)
                .await
                .is_err(),
            "an active tunnel must not be closed by the idle timeout"
        );

        // Once traffic stops and the tunnel is cancelled, it ends via
        // cancellation rather than idle.
        cancellation.cancel();
        let end = timeout(Duration::from_secs(2), forwarding)
            .await?
            .map_err(|error| anyhow::anyhow!("forwarding task panicked: {error}"))??;
        assert_eq!(end, ForwardEnd::Cancelled, "got {end:?}");
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn one_direction_traffic_keeps_tunnel_alive() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let Fixture {
            mut local_client,
            local_server,
            remote_client_send,
            mut remote_client_recv,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        } = fixture;
        // Keep the unused remote send half alive: dropping it would reset the
        // owner's receive direction mid-test.
        let _remote_client_send = remote_client_send;
        let cancellation = CancellationToken::new();
        let mut forwarding = tokio::spawn(forward_bidirectional(
            local_server,
            remote_server_send,
            remote_server_recv,
            cancellation.clone(),
            Duration::from_millis(100),
            None,
        ));
        consume_seed(&mut local_client).await?;

        // Pump ONLY local -> remote. The reverse direction stays completely
        // idle, but any activity in either direction must keep the tunnel
        // alive.
        for round in 0..6u32 {
            let payload = round.to_be_bytes();
            local_client.write_all(&payload).await?;
            let mut echoed = [0u8; 4];
            remote_client_recv.read_exact(&mut echoed).await?;
            assert_eq!(echoed, payload);
            sleep(Duration::from_millis(40)).await;
        }
        assert!(
            timeout(Duration::from_millis(50), &mut forwarding)
                .await
                .is_err(),
            "one-direction traffic must keep the tunnel alive"
        );

        cancellation.cancel();
        let end = timeout(Duration::from_secs(2), forwarding)
            .await?
            .map_err(|error| anyhow::anyhow!("forwarding task panicked: {error}"))??;
        assert_eq!(end, ForwardEnd::Cancelled, "got {end:?}");
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn injected_io_error_is_surfaced_not_swallowed() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let Fixture {
            mut local_client,
            local_server,
            mut remote_client_send,
            remote_client_recv,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        } = fixture;
        // Keep the remote recv half alive so only the explicit reset below can
        // fault the owner's receive direction.
        let _remote_client_recv = remote_client_recv;
        let cancellation = CancellationToken::new();
        let forwarding = tokio::spawn(forward_bidirectional(
            local_server,
            remote_server_send,
            remote_server_recv,
            cancellation,
            Duration::from_secs(30),
            None,
        ));
        consume_seed(&mut local_client).await?;

        // Reset the remote's send stream: the owner-side forwarding reads from
        // it and must surface the I/O error instead of converting it to
        // success.
        remote_client_send.reset(iroh::endpoint::VarInt::from_u32(1))?;

        let end = timeout(Duration::from_secs(2), forwarding)
            .await?
            .map_err(|error| anyhow::anyhow!("forwarding task panicked: {error}"))?;
        assert!(
            end.is_err(),
            "an injected I/O error must be surfaced as Err, got {end:?}"
        );
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn remote_graceful_close_is_distinguished_from_idle_timeout() -> anyhow::Result<()> {
        let fixture = fixture().await?;
        let Fixture {
            mut local_client,
            local_server,
            mut remote_client_send,
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
            cancellation,
            // Far longer than the test runs, so a graceful close can never be
            // confused with an idle timeout.
            Duration::from_secs(30),
            None,
        ));
        consume_seed(&mut local_client).await?;

        // The remote half-closes its send side...
        remote_client_send.finish()?;
        // ...and the local application half-closes its side: both directions
        // reach EOF.
        local_client.shutdown().await?;

        let end = timeout(Duration::from_secs(2), forwarding)
            .await?
            .map_err(|error| anyhow::anyhow!("forwarding task panicked: {error}"))??;
        assert_eq!(
            end,
            ForwardEnd::Eof,
            "graceful close must report Eof, not IdleTimeout (got {end:?})"
        );
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }

    /// Regression for the old implementation: wrapping the whole forwarding
    /// future in a hard five-minute lifetime timeout tore down healthy tunnels
    /// with continuous traffic at exactly five minutes. This test keeps a
    /// ping-pong flowing for longer than that boundary and asserts the tunnel
    /// is still connected. Deliberately slow (~5.5 minutes); runs on debsrv
    /// via `rb`.
    #[tokio::test]
    async fn active_tunnel_survives_beyond_old_lifetime_boundary() -> anyhow::Result<()> {
        use crate::tunnel::TUNNEL_IDLE_TIMEOUT;

        let fixture = fixture().await?;
        let Fixture {
            mut local_client,
            local_server,
            mut remote_client_send,
            mut remote_client_recv,
            remote_server_send,
            remote_server_recv,
            router,
            client,
        } = fixture;
        let cancellation = CancellationToken::new();
        let mut forwarding = tokio::spawn(forward_bidirectional(
            local_server,
            remote_server_send,
            remote_server_recv,
            cancellation.clone(),
            TUNNEL_IDLE_TIMEOUT,
            None,
        ));
        consume_seed(&mut local_client).await?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5 * 60 + 20);
        let mut round = 0u32;
        while tokio::time::Instant::now() < deadline {
            let payload = round.to_be_bytes();
            local_client.write_all(&payload).await?;
            let mut echoed = [0u8; 4];
            remote_client_recv.read_exact(&mut echoed).await?;
            assert_eq!(echoed, payload);
            remote_client_send.write_all(&payload).await?;
            local_client.read_exact(&mut echoed).await?;
            assert_eq!(echoed, payload);
            round += 1;
            sleep(Duration::from_millis(250)).await;
        }

        // The tunnel must still be alive past the old five-minute boundary.
        assert!(
            timeout(Duration::from_secs(2), &mut forwarding)
                .await
                .is_err(),
            "a tunnel with continuous traffic was closed by the old lifetime timeout"
        );

        cancellation.cancel();
        let end = timeout(Duration::from_secs(2), forwarding)
            .await?
            .map_err(|error| anyhow::anyhow!("forwarding task panicked: {error}"))??;
        assert_eq!(end, ForwardEnd::Cancelled, "got {end:?}");
        router.shutdown().await?;
        client.close().await;
        Ok(())
    }
}
