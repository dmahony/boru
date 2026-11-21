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
