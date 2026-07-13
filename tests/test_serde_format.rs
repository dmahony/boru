//! Quick test to verify serialization format of TopicId and other types.
#[test]
fn check_conversation_format() {
    use boru_chat::conversations::{ConversationEntry, ConversationStore};
    use boru_chat::proto::TopicId;
    use iroh::SecretKey;

    let topic = TopicId::from_bytes([0xBBu8; 32]);
    let pk = SecretKey::generate().public();

    // Create a direct conversation entry
    let entry = ConversationEntry::new(topic, pk.to_string(), "Alice");
    let json = serde_json::to_string_pretty(&entry).unwrap();
    println!("ConversationEntry (Direct):\n{json}\n");

    // Create a group conversation entry
    let topic2 = TopicId::from_bytes([0xCCu8; 32]);
    let entry2 = ConversationEntry::new_group(topic2, "The Room");
    let json2 = serde_json::to_string_pretty(&entry2).unwrap();
    println!("ConversationEntry (Group):\n{json2}\n");

    // Full store
    let tmp = tempfile::tempdir().unwrap();
    let mut store = ConversationStore::empty_at(tmp.path());
    store.upsert(entry);
    store.upsert(entry2);
    let json3 = serde_json::to_string_pretty(&store).unwrap();
    println!("ConversationStore:\n{json3}\n");
}
