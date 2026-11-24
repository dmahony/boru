use crate::{command_handler, test_fixture, AppState};
use serde_json::Value;

/// One isolated mock app and database-backed runtime per test.
pub(crate) struct Harness {
    _fixture: test_fixture::TestApp,
    window: tauri::WebviewWindow<tauri::test::MockRuntime>,
}

impl Harness {
    pub(crate) async fn new() -> Self {
        let fixture = test_fixture::test_app_state().await;
        Self::from_fixture(fixture)
    }

    pub(crate) async fn new_at(data_dir: &std::path::Path) -> Self {
        let fixture = test_fixture::test_app_state_at(data_dir, None).await;
        Self::from_fixture(fixture)
    }

    fn from_fixture(fixture: test_fixture::TestApp) -> Self {
        let state = AppState {
            runtime: fixture.state.runtime.clone(),
            page_requests: fixture.state.page_requests.clone(),
            sent_command_ids: fixture.state.sent_command_ids.clone(),
        };
        let app = tauri::test::mock_builder()
            .manage(state)
            .invoke_handler(command_handler())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock Tauri app");
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("build mock webview");
        Self { _fixture: fixture, window }
    }

    pub(crate) async fn restart_at(&mut self, data_dir: &std::path::Path) {
        self._fixture.runtime().take().expect("old runtime available");
        self._fixture = test_fixture::test_app_state_at(data_dir, None).await;
        let state = AppState {
            runtime: self._fixture.state.runtime.clone(),
            page_requests: self._fixture.state.page_requests.clone(),
            sent_command_ids: self._fixture.state.sent_command_ids.clone(),
        };
        let app = tauri::test::mock_builder().manage(state).invoke_handler(command_handler())
            .build(tauri::test::mock_context(tauri::test::noop_assets())).expect("build mock app after restart");
        self.window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().expect("build mock window after restart");
    }

    /// Invoke a registered command through Tauri's actual IPC dispatch.
    pub(crate) fn runtime(&self) -> std::sync::MutexGuard<'_, Option<boru_core::desktop_runtime::DesktopRuntime>> {
        self._fixture.runtime()
    }

    pub(crate) fn invoke(&self, command: &str, args: Value) -> Result<Value, String> {
        self.invoke_raw(command, tauri::ipc::InvokeBody::Json(args))
    }

    /// Raw IPC entry point for malformed bodies and payload validation tests.
    pub(crate) fn invoke_raw(
        &self,
        command: &str,
        body: tauri::ipc::InvokeBody,
    ) -> Result<Value, String> {
        let request = tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("valid test URL"),
            body,
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_owned(),
        };
        tauri::test::get_ipc_response(&self.window, request)
            .map(|response| response.deserialize::<Value>().expect("JSON IPC response"))
            .map_err(|error| format!("{error:?}"))
    }
}
