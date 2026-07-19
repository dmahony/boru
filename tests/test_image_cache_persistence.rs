use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use boru_chat::chat_history::{ChatHistoryStore, HistoryEntry};
use boru_chat::image_store::ImageStore;
use boru_chat::proto::TopicId;

const CHAT_IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;

fn load_stored_chat_image(
    image_store: &ImageStore,
    user: &str,
    identifier: &str,
) -> Option<Vec<u8>> {
    let path = image_store.resolve_absolute_path(user, identifier).ok()?;
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > CHAT_IMAGE_MAX_BYTES {
        return None;
    }
    Some(bytes)
}

fn user_hash(user: &str) -> String {
    blake3::hash(user.as_bytes()).to_hex().to_string()
}

#[test]
fn image_cache_round_trip_rehydrates_after_restart_and_blocks_other_users() {
    let dir = tempfile::tempdir().unwrap();
    let store = ImageStore::at(dir.path());
    let user_a = "alice";
    let user_b = "bob";
    let image_bytes = b"fake-png-bytes-1234567890-abcdef".to_vec();

    let image_id = store.save_image(user_a, "photo.png", &image_bytes).unwrap();
    let expected_prefix = format!("{}/", user_hash(user_a));
    assert!(image_id.starts_with(&expected_prefix));

    let abs = store.resolve_absolute_path(user_a, &image_id).unwrap();
    assert!(abs.starts_with(dir.path().join("files")));
    assert_eq!(fs::read(&abs).unwrap(), image_bytes);
    assert!(store.image_exists(user_a, &image_id).unwrap());
    assert!(store.resolve_absolute_path(user_b, &image_id).is_err());
    assert!(!store.image_exists(user_b, &image_id).unwrap_or(false));

    let topic = TopicId::from_bytes([7u8; 32]);
    let mut history = ChatHistoryStore::empty_at(dir.path());
    let mut entry = HistoryEntry::new(
        topic,
        user_a.to_string(),
        b"signed-message-bytes".to_vec(),
        "image",
        "[Image: photo.png]",
    );
    entry.image_identifier = Some(image_id.clone());
    entry.image_bytes = Some(image_bytes.clone());
    history.push_with_id(entry);
    let history_path = history.save().unwrap();
    assert!(history_path.is_file());

    let loaded_history = ChatHistoryStore::load_or_default(dir.path());
    assert_eq!(loaded_history.entries.len(), 1);
    let loaded_entry = &loaded_history.entries[0];
    assert_eq!(
        loaded_entry.image_identifier.as_deref(),
        Some(image_id.as_str())
    );
    // image_bytes has #[serde(skip)], so it is intentionally not persisted
    assert!(
        loaded_entry.image_bytes.is_none(),
        "image_bytes should be None after reload because it has #[serde(skip)]"
    );

    let mut reloaded_entry = loaded_entry.clone();
    reloaded_entry.image_bytes = None;
    let mut reloaded_history = ChatHistoryStore::empty_at(dir.path());
    reloaded_history.push(reloaded_entry.clone());
    reloaded_history.save().unwrap();

    let restarted_history = ChatHistoryStore::load_or_default(dir.path());
    assert_eq!(restarted_history.entries.len(), 1);
    assert!(
        restarted_history.entries[0].image_bytes.is_none(),
        "restarted chat should hydrate images from cache instead of history bytes"
    );
    assert_eq!(
        restarted_history.entries[0].image_identifier.as_deref(),
        Some(image_id.as_str())
    );

    let cache_hit = load_stored_chat_image(
        &store,
        user_a,
        reloaded_entry.image_identifier.as_deref().unwrap(),
    );
    assert_eq!(cache_hit.as_deref(), Some(image_bytes.as_slice()));

    let repeated = [reloaded_entry.clone(), reloaded_entry.clone()];
    for (idx, item) in repeated.iter().enumerate() {
        let hydrated =
            load_stored_chat_image(&store, user_a, item.image_identifier.as_deref().unwrap())
                .unwrap_or_else(|| panic!("reference {idx} should hydrate from cache"));
        assert_eq!(
            hydrated, image_bytes,
            "reference {idx} should read same cached bytes"
        );
    }

    fs::remove_file(&abs).unwrap();
    assert!(load_stored_chat_image(&store, user_a, &image_id).is_none());
}

#[test]
fn concurrent_directory_creation_is_safe_for_parallel_saves() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ImageStore::at(dir.path()));
    let barrier = Arc::new(Barrier::new(8));

    let handles: Vec<_> = (0..8)
        .map(|i| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let bytes = format!("payload-{i}").into_bytes();
                store
                    .save_image("alice", &format!("image-{i}.png"), &bytes)
                    .unwrap()
            })
        })
        .collect();

    let ids: Vec<String> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread should not panic"))
        .collect();

    let user_dir = dir.path().join("files").join(user_hash("alice"));
    assert!(user_dir.is_dir(), "user directory should be created once");
    assert_eq!(ids.len(), 8);

    for id in &ids {
        let abs = store.resolve_absolute_path("alice", id).unwrap();
        assert!(abs.is_file(), "{} should exist", abs.display());
    }
}
