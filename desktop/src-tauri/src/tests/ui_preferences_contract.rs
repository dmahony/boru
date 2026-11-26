use super::common::Harness;

#[tokio::test]
async fn preferences_commands_roundtrip_through_registered_handler() {
    let harness = Harness::new().await;
    let preferences = serde_json::json!({
        "drafts": {"room-a": "unsent"},
        "activeConversationId": "room-a",
        "activeNavigation": "chat",
        "sidebarWidth": 280,
        "window": {"x": 12, "y": 24, "width": 1200, "height": 800}
    });
    let saved = harness.invoke(
        "save_ui_preferences",
        serde_json::json!({"request": {"preferences": preferences}}),
    );
    assert_eq!(saved.unwrap(), serde_json::Value::Null);
    let loaded = harness
        .invoke("load_ui_preferences", serde_json::json!({}))
        .unwrap();
    assert_eq!(loaded["drafts"]["room-a"], "unsent");
    assert_eq!(loaded["activeConversationId"], "room-a");
    assert_eq!(loaded["window"]["width"], 1200);
}
