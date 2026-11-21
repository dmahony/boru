#![cfg(feature = "net")]

#[tokio::test]
async fn desktop_runtime_starts_and_stops_without_gui() {
    let dir = tempfile::tempdir().expect("temporary data directory");
    let mut runtime = boru_core::desktop_runtime::DesktopRuntime::start(dir.path())
        .await
        .expect("headless runtime starts");

    let duplicate = match boru_core::desktop_runtime::DesktopRuntime::start(dir.path()).await {
        Ok(_) => panic!("a second runtime must not claim the profile"),
        Err(error) => error,
    };
    assert!(duplicate.to_string().contains("profile is already in use"));

    let worker_stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_stopped_clone = worker_stopped.clone();
    runtime.spawn_worker(move |cancel| async move {
        cancel.cancelled().await;
        worker_stopped_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let _endpoint_addr = runtime.endpoint().addr();

    runtime.stop().await.expect("runtime shuts down cleanly");
    assert!(worker_stopped.load(std::sync::atomic::Ordering::SeqCst));
    runtime.stop().await.expect("shutdown is idempotent");

    // The profile lock is held throughout the runtime and released on stop.
    let restarted = boru_core::desktop_runtime::DesktopRuntime::start(dir.path())
        .await
        .expect("runtime restarts after shutdown");
    let mut restarted = restarted;
    restarted
        .stop()
        .await
        .expect("restarted runtime shuts down");
}

#[tokio::test]
async fn config_runtime_connects_forwards_remote_whisper_event_and_stops() {
    let dir = tempfile::tempdir().expect("temporary data directory");
    let config = boru_core::desktop_runtime::DesktopRuntimeConfig {
        data_dir: dir.path().to_path_buf(),
        secret_key: None,
        relay_mode: iroh::RelayMode::Disabled,
        bind_port: 0,
        enable_mdns: false,
        enable_dht: false,
        publish_direct_addresses: false,
    };
    let (mut runtime, _handles) =
        boru_core::desktop_runtime::DesktopRuntime::start_from_config(config)
            .await
            .expect("config-based headless runtime starts");
    let addr = runtime.endpoint().addr();
    assert!(!addr.id.to_string().is_empty());

    let client_key = iroh::SecretKey::generate();
    let client = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(client_key.clone())
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .expect("loopback client endpoint binds");
    let (client_whisper, _client_events) =
        boru_core::whisper::WhisperBuilder::new(client.clone(), client_key).spawn();
    let mut ui_events = runtime.subscribe_ui_events();
    client_whisper
        .connect_to(addr.id, addr)
        .await
        .expect("client connects to runtime whisper protocol");
    client_whisper
        .send_dm(
            runtime.endpoint().secret_key().public(),
            "headless integration".into(),
        )
        .await
        .expect("remote DM is sent");

    let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match ui_events.recv().await {
                Ok(
                    event @ boru_core::ui_events::UiEvent::ProtocolEvent {
                        source: "whisper", ..
                    },
                ) => break event,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(error) => panic!("runtime UI event channel closed: {error}"),
            }
        }
    })
    .await
    .expect("remote whisper protocol event reaches headless subscriber");
    assert!(matches!(
        event,
        boru_core::ui_events::UiEvent::ProtocolEvent {
            source: "whisper",
            ..
        }
    ));

    runtime.stop().await.expect("runtime shuts down cleanly");
    runtime
        .stop()
        .await
        .expect("runtime shutdown is idempotent");
    client.close().await;
}

#[tokio::test]
async fn production_handler_graph_starts_and_stops_headlessly() {
    let dir = tempfile::tempdir().expect("temporary data directory");
    let secret_key = iroh::SecretKey::generate();
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret_key.clone())
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .expect("loopback endpoint binds");
    let parts = boru_core::desktop_runtime::DesktopRuntimeParts {
        endpoint,
        secret_key,
        storage: std::sync::Arc::new(
            boru_core::storage::Storage::open(dir.path()).expect("storage opens"),
        ),
        data_dir: dir.path().to_path_buf(),
        friends: boru_core::friends::FriendsStore::empty_at(dir.path()),
        file_offer_registry: std::sync::Arc::new(std::sync::Mutex::new(
            boru_core::file_offer::FileOfferRegistry::new(),
        )),
        transfer_store: std::sync::Arc::new(
            boru_core::transfer_state_projection::TransferStateStore::new(16),
        ),
        outbound_item_labels: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
    };
    let (mut runtime, handles) = boru_core::desktop_runtime::DesktopRuntime::start_with(parts)
        .await
        .expect("production handler graph starts");
    let addr = runtime.endpoint().addr();
    assert!(!addr.id.to_string().is_empty());

    // Exercise every production ALPN registered by start_with. A successful
    // connect means the remote router negotiated and accepted that protocol.
    let alpns: &[&[u8]] = &[
        boru_core::net::GOSSIP_ALPN,
        iroh_blobs::ALPN,
        boru_core::chat_core::friend_ping::FRIEND_PING_ALPN,
        boru_core::backfill::BACKFILL_ALPN,
        boru_core::whisper::WHISPER_ALPN,
        boru_core::inbox::INBOX_ALPN,
        boru_core::protocol_version::CATALOGUE_ALPN,
        boru_core::net::FILE_ACCESS_ALPN,
        boru_core::file_offer_protocol::FILE_OFFER_ALPN,
        boru_core::tunnel::BORU_TUNNEL_ALPN,
        boru_core::call::manager::CALL_ALPN,
        boru_core::companion_protocol::COMPANION_ALPN,
    ];
    let client = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .expect("loopback client endpoint binds");
    for alpn in alpns {
        let connection = client
            .connect(addr.clone(), alpn)
            .await
            .unwrap_or_else(|error| panic!("production ALPN {:?} was not accepted: {error}", alpn));
        connection.close(0u32.into(), b"test complete");
    }

    // BRT-008b: the blob provider stream is now consumed by a runtime-owned
    // worker that projects served transfers into `TransferStateStore` and the
    // durable `transfer_activity` table. The former debug-only pump — which
    // echoed a `blob` ProtocolEvent onto the UI broadcast — is gone, so a bare
    // connection must NOT produce a UI event. Ownership is asserted by the
    // worker count below; a connection alone performs no blob GET, so the
    // projection stays empty.
    assert!(
        handles.transfer_store.snapshot().is_empty(),
        "a connection with no blob GET must not project a transfer"
    );

    drop(handles);
    assert!(
        runtime.active_worker_count() >= 5,
        "protocol event pumps are runtime-owned workers"
    );
    runtime.stop().await.expect("production runtime stops");
    assert_eq!(
        runtime.active_worker_count(),
        0,
        "shutdown joins every runtime worker"
    );
    runtime
        .stop()
        .await
        .expect("production runtime stop is idempotent");
    assert_eq!(
        runtime.active_worker_count(),
        0,
        "repeated shutdown leaves no workers"
    );
}

/// Source-level audit of the headless handler graph (BRT-008b).
///
/// The runtime module must own every production ALPN registration without
/// depending on Iced, and must keep the exact residual anchors documented in
/// `docs/brt-008b-handler-inventory.md`.
#[test]
fn desktop_runtime_owns_full_alpn_graph_without_gui() {
    let source = include_str!("../src/desktop_runtime.rs");

    assert!(
        !source.contains("iced::"),
        "headless runtime must not depend on Iced"
    );

    let graph = source
        .split_once("let router_builder = iroh::protocol::Router::builder")
        .and_then(|(_, rest)| rest.split_once("let router = router_builder.spawn();"))
        .map(|(graph, _)| graph)
        .expect("runtime builds the production router graph");
    assert_eq!(
        graph.matches(".accept(").count(),
        13,
        "runtime must register all 13 production ALPNs"
    );
    for alpn in [
        "GOSSIP_ALPN",
        "iroh_blobs::ALPN",
        "FRIEND_PING_ALPN",
        "BACKFILL_ALPN",
        "WHISPER_ALPN",
        "INBOX_ALPN",
        "CATALOGUE_ALPN",
        "FILE_ACCESS_ALPN",
        "FILE_OFFER_ALPN",
        "BORU_TUNNEL_ALPN",
        "CALL_ALPN",
        "COMPANION_ALPN",
        "SCREEN_SHARE_ALPN",
    ] {
        assert!(graph.contains(alpn), "runtime must register {alpn}");
        assert!(
            graph.contains("#[cfg(feature = \"screen-sharing\")]") || alpn != "SCREEN_SHARE_ALPN",
            "screen-share registration must stay feature-gated"
        );
    }

    // Two of the four original residuals are delivered by runtime construction;
    // the remaining two stay explicit anchors until their seams are moved.
    for anchor in ["1833-1848"] {
        assert!(
            source.contains(&format!("RESIDUAL BRT-008b: src/bin/boru/main.rs:{anchor}")),
            "missing residual anchor main.rs:{anchor}"
        );
    }
    for delivered in [
        "pub async fn install_inbox_replay_callbacks(",
        "pub(crate) async fn consume_outbound_provider_events(",
    ] {
        assert!(
            source.contains(delivered),
            "runtime must own the delivered BRT-008b seam: {delivered}"
        );
    }
    for anchor in ["1228-1277", "1135-1142"] {
        assert!(
            !source.contains(&format!("RESIDUAL BRT-008b: src/bin/boru/main.rs:{anchor}")),
            "main.rs:{anchor} is runtime-owned now and must not be listed as residual"
        );
    }
    assert!(
        source.contains("runtime.spawn_worker(move |cancel|")
            && source.contains("consume_outbound_provider_events("),
        "the provider consumer must be a runtime-owned, cancellation-aware worker"
    );
}
