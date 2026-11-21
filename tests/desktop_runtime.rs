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
    assert!(runtime.endpoint().addr().id.to_string().len() > 0);
    drop(handles);
    runtime.stop().await.expect("production runtime stops");
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
        .expect("runtime builds the production router graph")
        .1;
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

    for anchor in ["1228-1277", "1833-1848", "1135-1142", "2235-2247"] {
        assert!(
            source.contains(&format!("RESIDUAL BRT-008b: src/bin/boru/main.rs:{anchor}")),
            "missing residual anchor main.rs:{anchor}"
        );
    }
}
