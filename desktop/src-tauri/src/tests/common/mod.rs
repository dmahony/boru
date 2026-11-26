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

    /// Seed one remote-sender message to exercise a concrete unread transition.
    pub(crate) fn seed_remote_unread(&self, topic: [u8; 32]) {
        let runtime = self._fixture.runtime();
        let runtime = runtime.as_ref().expect("fixture runtime");
        runtime
            .message_store()
            .insert_chat_message(
                &[0xF6; 32],
                &topic,
                &[0xD4; 32],
                1_700_000_000_001,
                "text",
                "unread fixture message",
                None,
                None,
                &[0xE5; 32],
            )
            .expect("seed one remote unread message");
    }

    /// Inspect the feature-gated subscription registry used by this fixture.
    #[cfg(test)]
    pub(crate) async fn is_subscribed(&self, topic: boru_core::TopicId) -> bool {
        let runtime = self._fixture.state.runtime.lock().unwrap();
        let session = runtime.as_ref().unwrap().room_sender_session();
        drop(runtime);
        session.is_subscribed(topic).await
    }

    /// Invoke a registered command through Tauri's actual IPC dispatch.
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
