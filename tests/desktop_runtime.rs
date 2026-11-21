#![cfg(feature = "net")]

#[tokio::test]
async fn desktop_runtime_starts_and_stops_without_gui() {
    let dir = tempfile::tempdir().expect("temporary data directory");
    let runtime = boru_core::desktop_runtime::DesktopRuntime::start(dir.path())
        .await
        .expect("headless runtime starts");

    let _endpoint_addr = runtime.endpoint().addr();

    runtime
        .stop()
        .await
        .expect("runtime shuts down cleanly");
}
