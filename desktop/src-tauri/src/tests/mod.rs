pub(crate) mod common;

#[tokio::test]
async fn get_bootstrap_smoke_test() {
    let harness = common::Harness::new().await;
    let response = harness.invoke("get_bootstrap", serde_json::json!({}));
    assert!(response.is_ok(), "get_bootstrap IPC response: {response:?}");
}
