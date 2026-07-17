use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use boru_chat::image_store::ImageStore;

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

    let cache_hit = load_stored_chat_image(&store, user_a, &image_id);
    assert_eq!(cache_hit.as_deref(), Some(image_bytes.as_slice()));

    for idx in 0..2 {
        let hydrated = load_stored_chat_image(&store, user_a, &image_id)
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
