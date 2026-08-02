//! Bounded bidirectional forwarding between a local TCP socket and an Iroh
//! QUIC stream.

use iroh::endpoint::{RecvStream, SendStream};
use tokio::{io, net::TcpStream};
use tokio_util::sync::CancellationToken;

/// Forward bytes in both directions until EOF, cancellation, or an I/O error.
///
/// The two directions are polled concurrently in this task; no per-direction
/// tasks are spawned. Once either direction finishes, the other is cancelled
/// and awaited before returning, which prevents orphaned forwarding tasks.
pub(crate) async fn forward_bidirectional(
    local: TcpStream,
    remote_send: SendStream,
    remote_recv: RecvStream,
    cancellation: CancellationToken,
) {
    let (local_read, local_write) = local.into_split();
    let send_direction = forward_to_remote(local_read, remote_send, cancellation.clone());
    let receive_direction = forward_to_local(remote_recv, local_write, cancellation.clone());
    tokio::pin!(send_direction);
    tokio::pin!(receive_direction);
    let outcome = tokio::select! {
        result = &mut send_direction => ("local_to_remote", result),
        result = &mut receive_direction => ("remote_to_local", result),
        _ = cancellation.cancelled() => ("cancelled", Ok(())),
    };
    cancellation.cancel();
    let _ = (&mut send_direction).await;
    let _ = (&mut receive_direction).await;
    if let (direction, Err(error)) = outcome {
        tracing::debug!(direction, %error, "tunnel forwarding stopped");
    }
}

async fn forward_to_remote(
    mut local_read: tokio::net::tcp::OwnedReadHalf,
    mut remote_send: SendStream,
    cancellation: CancellationToken,
) -> io::Result<()> {
    tokio::select! {
        result = io::copy(&mut local_read, &mut remote_send) => {
            result?;
            remote_send.finish().map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
        }
        _ = cancellation.cancelled() => Ok(()),
    }
}

async fn forward_to_local(
    mut remote_recv: RecvStream,
    mut local_write: tokio::net::tcp::OwnedWriteHalf,
    cancellation: CancellationToken,
) -> io::Result<()> {
    tokio::select! {
        result = io::copy(&mut remote_recv, &mut local_write) => {
            result?;
            io::AsyncWriteExt::shutdown(&mut local_write).await
        }
        _ = cancellation.cancelled() => Ok(()),
    }
}
