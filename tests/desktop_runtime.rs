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
