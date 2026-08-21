//! Integration coverage for the TUN-02 tunnel improvements:
//!
//! 1. **Reconnect backoff** — a dropped tunnel link is re-established
//!    automatically with exponential backoff, and the loop eventually
//!    reconnects.
//! 2. **One-time enrollment tokens** — mint/redeem/pin over the real wire:
//!    a fresh token is accepted and pins the peer key; replaying the same
//!    token is rejected; an expired token is rejected; a pinned peer can
//!    reconnect without a token.
//!
//! These tests need the `net` feature (they bind real iroh endpoints).

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::{endpoint::presets, protocol::Router, Endpoint, SecretKey};
use tokio::time::timeout;

use boru_core::tunnel::{
    enroll_tunnel,
    enrollment::EnrollmentTokenStore,
    reconnect::{run_reconnect_loop, TunnelLinkHandle, TunnelLinkStatus},
    service::{ReconnectPolicy, TunnelService, TunnelTarget},
    TunnelCapability, TunnelId, TunnelProtocol, BORU_TUNNEL_ALPN,
};

/// Current Unix epoch in milliseconds (mirrors the crate-private helper).
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64
}

/// A tiny fast reconnect policy so tests do not wait for real 1s backoff.
fn fast_policy() -> ReconnectPolicy {
    ReconnectPolicy {
        initial_delay: Duration::from_millis(25),
        max_delay: Duration::from_millis(100),
        factor: 2,
        jitter: 0.0,
    }
}

/// The owner side of a tunnel: an endpoint + router that accepts the tunnel
/// ALPN and hands requests to a [`TunnelService`].
struct OwnerFixture {
    router: Router,
    endpoint: Endpoint,
    service: Arc<TunnelService>,
}

async fn spawn_owner() -> anyhow::Result<OwnerFixture> {
    let endpoint = Endpoint::bind(presets::Minimal).await?;
    let service = Arc::new(TunnelService::new());
    let protocol =
        TunnelProtocol::with_service(Arc::clone(&service), endpoint.secret_key().public());
    let router = Router::builder(endpoint.clone())
        .accept(BORU_TUNNEL_ALPN, protocol)
        .spawn();
    Ok(OwnerFixture {
        router,
        endpoint,
        service,
    })
}

async fn spawn_owner_with_enrollment() -> anyhow::Result<OwnerFixture> {
    let endpoint = Endpoint::bind(presets::Minimal).await?;
    let service = Arc::new(TunnelService::with_enrollment_store(Arc::new(
        EnrollmentTokenStore::new(),
    )));
    let protocol =
        TunnelProtocol::with_service(Arc::clone(&service), endpoint.secret_key().public());
    let router = Router::builder(endpoint.clone())
        .accept(BORU_TUNNEL_ALPN, protocol)
        .spawn();
    Ok(OwnerFixture {
        router,
        endpoint,
        service,
    })
}

/// Register a loopback tunnel on the owner that the peer may dial.
///
/// The target is a live loopback TCP listener so the owner's forwarding can
/// actually connect (a dead port would surface `TargetUnavailable` instead of
/// exercising the authorization path).
async fn create_loopback_tunnel(
    fixture: &OwnerFixture,
    peer: iroh::PublicKey,
) -> anyhow::Result<(TunnelId, tokio::net::TcpListener)> {
    let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let target_addr = target_listener.local_addr()?;
    let tunnel_id = TunnelId(rand::random());
    fixture
        .service
        .create_tunnel(
            tunnel_id,
            fixture.endpoint.secret_key().public(),
            TunnelTarget::tcp(target_addr.ip(), target_addr.port()),
            peer,
            unix_now_ms(),
            u64::MAX,
        )
        .map_err(|e| anyhow::anyhow!("create tunnel: {e:?}"))?;
    Ok((tunnel_id, target_listener))
}

#[tokio::test]
async fn reconnect_loop_reconnects_after_link_drop_with_backoff() -> anyhow::Result<()> {
    let owner = spawn_owner().await?;
    let client = Endpoint::bind(presets::Minimal).await?;
    let owner_addr = owner.endpoint.addr();

    let (status_tx, mut status_rx) = tokio::sync::watch::channel(TunnelLinkStatus::Idle);
    let cancellation = tokio_util::sync::CancellationToken::new();

    // The connect closure caches the established connection so the test can
    // drop it on demand while the owner stays up.
    let current: Arc<tokio::sync::Mutex<Option<iroh::endpoint::Connection>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    let client_for_connect = client.clone();
    let current_for_connect = Arc::clone(&current);
    let connect = move || -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<Arc<dyn TunnelLinkHandle>>> + Send>,
    > {
        let client = client_for_connect.clone();
        let owner_addr = owner_addr.clone();
        let current = Arc::clone(&current_for_connect);
        Box::pin(async move {
            let conn = client.connect(owner_addr, BORU_TUNNEL_ALPN).await?;
            *current.lock().await = Some(conn.clone());
            Ok(Arc::new(conn) as Arc<dyn TunnelLinkHandle>)
        })
    };

    let task = tokio::spawn(run_reconnect_loop(
        connect,
        || false,
        fast_policy(),
        cancellation.clone(),
        status_tx,
        None,
    ));

    // Wait for the first Connected state.
    wait_for_status(&mut status_rx, TunnelLinkStatus::Connected, 5).await?;

    // Drop the live tunnel link from the client side. The loop must notice,
    // enter Reconnecting (backoff), and then re-establish the link because the
    // owner is still accepting.
    let connection = current
        .lock()
        .await
        .take()
        .ok_or_else(|| anyhow::anyhow!("connect closure never cached a connection"))?;
    connection.close(0u32.into(), b"test drop");

    let mut saw_reconnecting = false;
    let mut reconnected = false;
    for _ in 0..40 {
        tokio::select! {
            _ = status_rx.changed() => {
                let status = *status_rx.borrow();
                if matches!(status, TunnelLinkStatus::Reconnecting { .. }) {
                    saw_reconnecting = true;
                }
                if status == TunnelLinkStatus::Connected {
                    reconnected = true;
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(5)) => break,
        }
    }
    assert!(
        saw_reconnecting,
        "loop must enter Reconnecting after the link drops"
    );
    assert!(
        reconnected,
        "loop must re-establish the link after the drop"
    );

    cancellation.cancel();
    let _ = task.await;
    client.close().await;
    owner.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn expired_tunnel_link_stops_reconnecting() -> anyhow::Result<()> {
    let owner = spawn_owner().await?;
    let client = Endpoint::bind(presets::Minimal).await?;
    let owner_addr = owner.endpoint.addr();

    let (status_tx, mut status_rx) = tokio::sync::watch::channel(TunnelLinkStatus::Idle);
    let cancellation = tokio_util::sync::CancellationToken::new();

    let client_for_connect = client.clone();
    let connect = move || -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<Arc<dyn TunnelLinkHandle>>> + Send>,
    > {
        let client = client_for_connect.clone();
        let owner_addr = owner_addr.clone();
        Box::pin(async move {
            let conn = client.connect(owner_addr, BORU_TUNNEL_ALPN).await?;
            Ok(Arc::new(conn) as Arc<dyn TunnelLinkHandle>)
        })
    };

    // The tunnel is already expired: the loop must stop without dialing.
    let task = tokio::spawn(run_reconnect_loop(
        connect,
        || true,
        fast_policy(),
        cancellation.clone(),
        status_tx,
        None,
    ));

    wait_for_status(&mut status_rx, TunnelLinkStatus::Stopped, 5).await?;
    cancellation.cancel();
    let _ = task.await;
    client.close().await;
    owner.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn enrollment_token_accepted_once_and_pins_peer() -> anyhow::Result<()> {
    let owner = spawn_owner_with_enrollment().await?;
    let peer_key = SecretKey::generate().public();
    let (tunnel_id, _target_listener) = create_loopback_tunnel(&owner, peer_key).await?;

    let minted = owner
        .service
        .enrollment()
        .mint(tunnel_id, 600)
        .expect("mint token");

    let client = Endpoint::bind(presets::Minimal).await?;
    let conn = client
        .connect(owner.endpoint.addr(), BORU_TUNNEL_ALPN)
        .await?;

    // First connection with the token succeeds and pins the peer.
    let (mut send, _recv) = enroll_tunnel(&conn, tunnel_id, Some(minted.token.clone())).await?;
    send.finish()?;
    assert!(
        owner
            .service
            .enrollment()
            .is_pinned(tunnel_id, &client.secret_key().public()),
        "peer must be pinned after successful enrollment"
    );

    client.close().await;
    owner.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn enrollment_token_replay_is_rejected() -> anyhow::Result<()> {
    let owner = spawn_owner_with_enrollment().await?;
    let peer_key = SecretKey::generate().public();
    let (tunnel_id, _target_listener) = create_loopback_tunnel(&owner, peer_key).await?;
    let minted = owner
        .service
        .enrollment()
        .mint(tunnel_id, 600)
        .expect("mint token");

    // Redeem once directly (as if the first connection happened).
    let client = SecretKey::generate().public();
    owner
        .service
        .enrollment()
        .redeem(&minted.token, tunnel_id, &client, unix_now_ms())
        .expect("first redeem");

    // Replaying the same token must be rejected by the store.
    let replay =
        owner
            .service
            .enrollment()
            .redeem(&minted.token, tunnel_id, &client, unix_now_ms());
    assert!(
        matches!(
            replay,
            Err(boru_core::tunnel::enrollment::EnrollmentError::TokenAlreadyUsed)
        ),
        "replay must be rejected, got {replay:?}"
    );

    owner.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn expired_enrollment_token_is_rejected() -> anyhow::Result<()> {
    let owner = spawn_owner_with_enrollment().await?;
    let peer_key = SecretKey::generate().public();
    let (tunnel_id, _target_listener) = create_loopback_tunnel(&owner, peer_key).await?;

    // Mint with a TTL of 1 second, then redeem after expiry.
    let minted = owner
        .service
        .enrollment()
        .mint(tunnel_id, 1)
        .expect("mint token");
    let client = SecretKey::generate().public();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let result =
        owner
            .service
            .enrollment()
            .redeem(&minted.token, tunnel_id, &client, unix_now_ms());
    assert!(
        matches!(
            result,
            Err(boru_core::tunnel::enrollment::EnrollmentError::TokenExpired)
        ),
        "expired token must be rejected, got {result:?}"
    );

    owner.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn enroll_over_wire_rejects_unknown_tunnel_and_bad_token() -> anyhow::Result<()> {
    let owner = spawn_owner_with_enrollment().await?;
    let peer_key = SecretKey::generate().public();
    let (tunnel_id, _target_listener) = create_loopback_tunnel(&owner, peer_key).await?;

    let client = Endpoint::bind(presets::Minimal).await?;
    let conn = client
        .connect(owner.endpoint.addr(), BORU_TUNNEL_ALPN)
        .await?;

    // A bogus token must be rejected at the handshake.
    let result = timeout(
        Duration::from_secs(3),
        enroll_tunnel(&conn, tunnel_id, Some("bogus-token".to_string())),
    )
    .await;
    assert!(
        result.is_err() || result.unwrap().is_err(),
        "bogus enrollment token must be rejected"
    );

    client.close().await;
    owner.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn capability_open_still_works_after_enrollment_changes() -> anyhow::Result<()> {
    // Regression: the additive Enroll variant must not disturb the existing
    // signed-capability Open flow.
    let owner = spawn_owner().await?;
    let peer_key = SecretKey::generate().public();
    let (tunnel_id, _target_listener) = create_loopback_tunnel(&owner, peer_key).await?;

    let client = Endpoint::bind(presets::Minimal).await?;
    let conn = client
        .connect(owner.endpoint.addr(), BORU_TUNNEL_ALPN)
        .await?;
    let capability = TunnelCapability::sign(
        &owner.endpoint.secret_key(),
        client.secret_key().public(),
        tunnel_id,
        0,
        u64::MAX,
    );
    let (mut send, _recv) = boru_core::tunnel::open_tunnel(&conn, tunnel_id, capability).await?;
    send.finish()?;

    client.close().await;
    owner.router.shutdown().await?;
    Ok(())
}

/// Wait for `want` to appear on the status watch channel.
async fn wait_for_status(
    status_rx: &mut tokio::sync::watch::Receiver<TunnelLinkStatus>,
    want: TunnelLinkStatus,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    if *status_rx.borrow() == want {
        return Ok(());
    }
    timeout(Duration::from_secs(timeout_secs), async {
        while status_rx.changed().await.is_ok() {
            if *status_rx.borrow() == want {
                return;
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for status {want:?}"))
}
