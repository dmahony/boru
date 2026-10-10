#![allow(deprecated)]

//! Integration tests for onboarding persistence, migration inference,
//! and first-launch detection across the settings/storage boundary.
//!
//! These tests simulate the full app startup flow that `src/bin/boru`
//! performs: loading the profile store, inferring onboarding state from
//! profile content and external context (friends, rooms, conversations),
//! persisting the inferred state, and verifying that state survives
//! save/reload and "app restart" cycles.
//!
//! Test categories:
//! 1. Fresh profile detection — no files → onboarding=false
//! 2. Persistence — onboarding state survives save/reload/restart
//! 3. Reset — setting onboarding=false persists and allows re-onboarding
//! 4. Migration inference (legacy profiles without onboarding_completed)
//! 5. External context inference (friends, rooms, conversations)
//! 6. Empty legacy data stays a fresh profile
//! 7. App startup simulation (full sequence)
//! 8. Onboarding failure/dismissal does not prevent normal operation

use boru_core::user_profile::UserProfileStore;
use iroh::{PublicKey, SecretKey};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ── Test helpers ───────────────────────────────────────────────────────────

/// Create a deterministic public key for testing.
fn test_key() -> PublicKey {
    PublicKey::from_bytes(&[1u8; 32]).expect("32 one-bytes is a valid ed25519 public key")
}

/// Create a deterministic secondary key (for testing two profiles).
fn test_key_2() -> PublicKey {
    SecretKey::generate().public()
}

/// Temporary directory helper — creates a unique temp dir and returns the path.
fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("boru-onboarding-{name}-{suffix}"));
    dir
}

/// Clean up a temp directory recursively.
fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Write a profile.json to the given data directory with the specified fields.
/// Omits `onboarding_completed` to simulate a legacy profile.
fn write_legacy_profile(
    data_dir: &Path,
    key: &PublicKey,
    display_name: &str,
    bio: &str,
    shared_files: bool,
) {
    fs::create_dir_all(data_dir).unwrap();
    let path = data_dir.join("profile.json");
    let key_hex = key.to_string();
    let files_json = if shared_files {
        r#"[
            {"id": "abc123", "filename": "doc.pdf", "size": 5000,
             "mime_type": "application/pdf",
             "modified_time": 1700000000, "path": "/tmp/doc.pdf"}
        ]"#
    } else {
        "[]"
    };
    let json = format!(
        r#"{{
            "schema_version": 1,
            "profile": {{
                "user_id": "{key_hex}",
                "display_name": "{display_name}",
                "bio": "{bio}"
            }},
            "shared_files": {files_json}
        }}"#
    );
    fs::write(&path, &json).unwrap();
}

/// Write a settings.json to simulate an existing app installation.
fn write_settings_json(data_dir: &Path, onboarding_completed: Option<bool>) {
    fs::create_dir_all(data_dir).unwrap();
    let path = data_dir.join("settings.json");
    let onboarding = match onboarding_completed {
        Some(true) => r#""onboarding_completed": true"#,
        Some(false) => r#""onboarding_completed": false"#,
        None => "", // legacy settings.json — no field
    };
    let json = format!(
        r#"{{"dark_mode": false, "sound_enabled": true, "chat_text_size": 13.0{} }}"#,
        if onboarding.is_empty() {
            String::new()
        } else {
            format!(", {}", onboarding)
        }
    );
    fs::write(&path, &json).unwrap();
}

/// Simulate the app startup sequence for onboarding detection.
///
/// This mirrors the logic in `src/bin/boru/app.rs`:
/// 1. Load or create the profile store
/// 2. Infer onboarding from external data (friends, rooms, conversations)
/// 3. Invoke the deprecated legacy save shim (a no-op)
/// 4. Return the store and its onboarding state
fn simulate_app_startup(data_dir: &Path, has_existing_data: bool) -> (UserProfileStore, bool) {
    let key = test_key();
    let mut profile_store = UserProfileStore::load_or_default(data_dir, key);

    // Infer from external context (the app checks friends, rooms, etc.)
    profile_store.infer_onboarding_from_external(has_existing_data);

    // Retain startup call sequencing; the deprecated store save is a no-op.
    let onboarding = profile_store.onboarding_completed();
    let _ = profile_store.save();

    (profile_store, onboarding)
}

// ── 1. FRESH PROFILE DETECTION ─────────────────────────────────────────────

#[test]
fn fresh_profile_has_no_onboarding() {
    // A directory with no profile.json should produce a store with
    // onboarding_completed = false.
    let dir = temp_dir("fresh_no_onboarding");
    let store = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(
        !store.onboarding_completed(),
        "fresh profile should not have onboarding completed"
    );
    cleanup(&dir);
}

#[test]
fn fresh_profile_empty_at_is_incomplete() {
    // A store created via empty_at() (like load does for missing files)
    // should report onboarding as incomplete.
    let dir = temp_dir("fresh_empty_at");
    let store = UserProfileStore::empty_at(&dir, test_key());
    assert!(
        !store.onboarding_completed(),
        "empty_at store should have onboarding_completed = false"
    );
    cleanup(&dir);
}

// ── 2. PERSISTENCE ACROSS SAVE/RELOAD ──────────────────────────────────────

#[test]
fn deprecated_json_save_does_not_persist_onboarding() {
    let dir = temp_dir("persist_save_reload");
    fs::create_dir_all(&dir).unwrap();

    // Create store with onboarding completed
    let mut store = UserProfileStore::empty_at(&dir, test_key());
    store.set_onboarding_completed(true);
    store.profile_mut().display_name = "TestUser".into();
    store.save().unwrap();

    // JSON persistence was retired in favor of SQLite. The deprecated shim
    // returns its legacy path but must not claim to have written the store.
    let path = store.save().unwrap();
    assert_eq!(path, store.file_path());
    assert!(!path.exists());
    assert!(store.onboarding_completed());

    // A new legacy store is fresh because there is no JSON file to reload.
    let loaded = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(!loaded.onboarding_completed());
    assert!(loaded.profile().display_name.is_empty());
    cleanup(&dir);
}

#[test]
fn onboarding_state_remains_mutable_after_deprecated_save() {
    let dir = temp_dir("mutable_after_legacy_save");
    fs::create_dir_all(&dir).unwrap();
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(true);
    store.save().unwrap();
    assert!(store.onboarding_completed());
    assert!(!store.file_path().exists());

    // The in-memory state remains mutable even though the deprecated JSON
    // save shim deliberately does not persist it.
    store.set_onboarding_completed(false);
    store.save().unwrap();
    assert!(!store.onboarding_completed());
    store.set_onboarding_completed(true);
    store.save().unwrap();
    assert!(store.onboarding_completed());
    assert!(!store.file_path().exists());
    assert!(!UserProfileStore::load(&dir, key)
        .unwrap()
        .onboarding_completed());
    cleanup(&dir);
}

#[test]
fn legacy_json_save_does_not_survive_app_restart() {
    let dir = temp_dir("persist_restart");
    fs::create_dir_all(&dir).unwrap();
    let key = test_key();

    // "First launch" — complete onboarding
    {
        let mut store = UserProfileStore::empty_at(&dir, key);
        store.set_onboarding_completed(true);
        store.profile_mut().display_name = "Alice".into();
        store.save().unwrap();
    } // store dropped here — simulates app exit

    assert!(!UserProfileStore::empty_at(&dir, key).file_path().exists());

    // "Second launch" — no legacy JSON file exists to restore state from.
    {
        let store = UserProfileStore::load(&dir, key).unwrap();
        assert!(!store.onboarding_completed());
        assert!(store.profile().display_name.is_empty());
    } // store dropped here — simulates another exit
    cleanup(&dir);
}

// ── 3. RESET ───────────────────────────────────────────────────────────────

#[test]
fn onboarding_reset_allows_reonboarding_in_memory() {
    // Simulate the "Show onboarding again" flow: user who was onboarded
    // hits the reset button, which sets onboarding=false.
    let dir = temp_dir("reset_reonboard");
    fs::create_dir_all(&dir).unwrap();
    let key = test_key();

    // Start as onboarded
    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(true);
    store.profile_mut().display_name = "Bob".into();
    store.save().unwrap();

    // Reset (like ShowOnboardingAgain) on the active store.
    store.set_onboarding_completed(false);
    store.save().unwrap();
    assert!(
        !store.onboarding_completed(),
        "onboarding should be false after reset"
    );

    // Complete onboarding again (simulating user going through flow again)
    store.set_onboarding_completed(true);
    store.save().unwrap();

    assert!(
        store.onboarding_completed(),
        "user should be able to complete onboarding again after reset"
    );
    cleanup(&dir);
}

// ── 4. MIGRATION INFERENCE (LEGACY PROFILES) ───────────────────────────────

#[test]
fn legacy_profile_with_display_name_infers_onboarding() {
    let dir = temp_dir("legacy_display_name");
    write_legacy_profile(&dir, &test_key(), "ExistingUser", "", false);

    let store = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(
        store.onboarding_completed(),
        "legacy profile with display_name should infer onboarding"
    );
    cleanup(&dir);
}

#[test]
fn legacy_profile_with_bio_infers_onboarding() {
    let dir = temp_dir("legacy_bio");
    write_legacy_profile(&dir, &test_key(), "", "I have a bio", false);

    let store = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(
        store.onboarding_completed(),
        "legacy profile with bio should infer onboarding"
    );
    cleanup(&dir);
}

#[test]
fn legacy_profile_with_shared_files_infers_onboarding() {
    let dir = temp_dir("legacy_shared_files");
    write_legacy_profile(&dir, &test_key(), "", "", true);

    let store = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(
        store.onboarding_completed(),
        "legacy profile with shared_files should infer onboarding"
    );
    cleanup(&dir);
}

#[test]
fn legacy_profile_with_all_fields_infers_onboarding() {
    let dir = temp_dir("legacy_all_fields");
    write_legacy_profile(&dir, &test_key(), "FullUser", "Full bio", true);

    let store = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(
        store.onboarding_completed(),
        "legacy profile with all meaningful fields should infer onboarding"
    );
    cleanup(&dir);
}

#[test]
fn legacy_empty_profile_remains_fresh() {
    // Write a minimal profile.json with no meaningful data and no
    // onboarding_completed field → should stay incomplete.
    let dir = temp_dir("legacy_empty");
    let key = test_key();
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profile.json");
    let key_hex = key.to_string();
    let json = format!(
        r#"{{
            "schema_version": 1,
            "profile": {{
                "user_id": "{key_hex}"
            }},
            "shared_files": []
        }}"#
    );
    fs::write(&path, &json).unwrap();

    let store = UserProfileStore::load(&dir, key).unwrap();
    assert!(
        !store.onboarding_completed(),
        "empty legacy profile should remain incomplete"
    );
    cleanup(&dir);
}

// ── 5. EXTERNAL CONTEXT INFERENCE ──────────────────────────────────────────

#[test]
fn external_friends_triggers_onboarding() {
    // Simulate the app detecting existing friends data.
    let dir = temp_dir("external_friends");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    assert!(!store.onboarding_completed());

    // App detects existing friends (stored elsewhere) and infers
    let changed = store.infer_onboarding_from_external(true);
    assert!(changed, "inference should return true when state changed");
    assert!(
        store.onboarding_completed(),
        "external friends data should trigger onboarding"
    );
    cleanup(&dir);
}

#[test]
fn external_rooms_triggers_onboarding() {
    let dir = temp_dir("external_rooms");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    assert!(!store.onboarding_completed());

    // Existing room history
    let changed = store.infer_onboarding_from_external(true);
    assert!(changed);
    assert!(store.onboarding_completed());
    cleanup(&dir);
}

#[test]
fn external_conversations_triggers_onboarding() {
    let dir = temp_dir("external_conversations");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    assert!(!store.onboarding_completed());

    // Existing conversations
    let changed = store.infer_onboarding_from_external(true);
    assert!(changed);
    assert!(store.onboarding_completed());
    cleanup(&dir);
}

#[test]
fn external_inference_noop_when_no_data() {
    let dir = temp_dir("external_noop");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    assert!(!store.onboarding_completed());

    let changed = store.infer_onboarding_from_external(false);
    assert!(
        !changed,
        "inference should return false when has_existing_data is false"
    );
    assert!(!store.onboarding_completed());
    cleanup(&dir);
}

#[test]
fn external_inference_noop_when_already_onboarded() {
    let dir = temp_dir("external_already");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(true);

    let changed = store.infer_onboarding_from_external(true);
    assert!(
        !changed,
        "inference should return false when already onboarded"
    );
    assert!(store.onboarding_completed());
    cleanup(&dir);
}

// ── 6. EXPLICIT FALSE SURVIVES LOAD ────────────────────────────────────────

#[test]
fn explicit_false_not_overridden_by_display_name() {
    // If a user explicitly set onboarding=false and has a display_name,
    // the load inference should NOT override the explicit false.
    let dir = temp_dir("explicit_false_data");
    let key = test_key();
    fs::create_dir_all(&dir).unwrap();

    // Write profile.json with explicit false + display_name
    let key_hex = key.to_string();
    let json = format!(
        r#"{{
            "schema_version": 1,
            "onboarding_completed": false,
            "profile": {{
                "user_id": "{key_hex}",
                "display_name": "WantsReonboarding"
            }},
            "shared_files": []
        }}"#
    );
    fs::write(dir.join("profile.json"), &json).unwrap();

    let store = UserProfileStore::load(&dir, key).unwrap();
    assert!(
        !store.onboarding_completed(),
        "explicit false should survive load even with meaningful data"
    );
    cleanup(&dir);
}

#[test]
fn explicit_false_not_overridden_by_bio() {
    let dir = temp_dir("explicit_false_bio");
    let key = test_key();
    fs::create_dir_all(&dir).unwrap();

    let key_hex = key.to_string();
    let json = format!(
        r#"{{
            "schema_version": 1,
            "onboarding_completed": false,
            "profile": {{
                "user_id": "{key_hex}",
                "bio": "Returning user who wants onboarding"
            }},
            "shared_files": []
        }}"#
    );
    fs::write(dir.join("profile.json"), &json).unwrap();

    let store = UserProfileStore::load(&dir, key).unwrap();
    assert!(
        !store.onboarding_completed(),
        "explicit false with bio should not be overridden"
    );
    cleanup(&dir);
}

// ── 7. APP STARTUP SIMULATION ──────────────────────────────────────────────

#[test]
fn app_startup_fresh_dir_stays_incomplete() {
    // Simulate a brand-new user: no settings, no profile, no friends.
    let dir = temp_dir("startup_fresh");
    fs::create_dir_all(&dir).unwrap();

    let (_store, onboarded) = simulate_app_startup(&dir, false);
    assert!(!onboarded, "fresh app startup should not be onboarded");
    cleanup(&dir);
}

#[test]
fn app_startup_with_external_data_infers_onboarding() {
    // Simulate a returning user: existing friends/rooms trigger inference.
    let dir = temp_dir("startup_external");
    fs::create_dir_all(&dir).unwrap();

    let (_store, onboarded) = simulate_app_startup(&dir, true);
    assert!(
        onboarded,
        "app startup with external data should infer onboarding"
    );
    cleanup(&dir);
}

#[test]
fn app_startup_with_legacy_profile_infers_onboarding() {
    // Simulate a user upgrading from an older version that has a profile
    // but no onboarding_completed field.
    let dir = temp_dir("startup_legacy");
    write_legacy_profile(&dir, &test_key(), "LegacyUser", "Upgraded", false);

    let (_store, onboarded) = simulate_app_startup(&dir, false);
    assert!(
        onboarded,
        "app startup with legacy profile should infer onboarding"
    );
    cleanup(&dir);
}

#[test]
fn app_startup_legacy_profile_plus_external_both_infer() {
    // Both profile content and external data should independently infer.
    let dir = temp_dir("startup_both");
    write_legacy_profile(&dir, &test_key(), "ExistingUser", "Has data", true);

    let (_store, onboarded) = simulate_app_startup(&dir, true);
    assert!(
        onboarded,
        "both legacy data and external context should yield onboarded"
    );
    cleanup(&dir);
}

#[test]
fn app_startup_reinfers_external_state_without_legacy_json_persistence() {
    let dir = temp_dir("startup_persist_inferred");
    fs::create_dir_all(&dir).unwrap();

    let (_first_store, first_launch) = simulate_app_startup(&dir, true);
    assert!(first_launch);
    assert!(!dir.join("profile.json").exists());

    // Inference is based on current external state, not a deprecated JSON
    // write. Reopening with that state still present infers completion again.
    let (_second_store, second_launch) = simulate_app_startup(&dir, true);
    assert!(second_launch);

    let (_without_context, fresh_launch) = simulate_app_startup(&dir, false);
    assert!(!fresh_launch);
    cleanup(&dir);
}

#[test]
fn app_startup_with_settings_file_but_no_profile() {
    // Simulate a user who has settings.json (from an old version or other
    // tool) but no profile.json — still a fresh profile.
    let dir = temp_dir("startup_settings_only");
    write_settings_json(&dir, None); // legacy settings, no onboarding field

    let store = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(
        !store.onboarding_completed(),
        "settings.json without profile.json should still be fresh"
    );
    cleanup(&dir);
}

// ── 8. ONBOARDING DISMISSAL DOES NOT BLOCK NORMAL OPERATION ────────────────

#[test]
fn onboarding_dismissal_does_not_gate_in_memory_profile_edits() {
    // Verify that having onboarding=false does not prevent the store from
    // functioning normally — settings can be changed, profile saved, etc.
    let dir = temp_dir("dismissal_normal");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(false); // dismissed onboarding
    store.profile_mut().display_name = "DismissedUser".into();
    store.profile_mut().bio = "Skipped onboarding".into();
    store.profile_mut().file_sharing_enabled = true;
    assert!(!store.onboarding_completed(), "onboarding remains false");
    assert_eq!(store.profile().display_name, "DismissedUser");
    assert_eq!(store.profile().bio, "Skipped onboarding");
    assert!(store.profile().file_sharing_enabled);

    // User can continue using the active store and change profile settings.
    store.profile_mut().display_name = "StillUsing".into();
    store.profile_mut().max_file_size = 200 * 1024 * 1024;
    store.save().unwrap();
    assert_eq!(store.profile().display_name, "StillUsing");
    assert_eq!(store.profile().max_file_size, 200 * 1024 * 1024);
    assert!(!store.onboarding_completed(), "onboarding still false");
    assert!(!store.file_path().exists());
    cleanup(&dir);
}

#[test]
fn onboarding_false_does_not_prevent_advanced_settings() {
    // Simulate a user who is not onboarded but still accesses advanced
    // settings (the app should not gate this behind onboarding).
    let dir = temp_dir("dismissal_advanced");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(false);
    store.profile_mut().max_file_size = 50 * 1024 * 1024;
    store.profile_mut().allowed_extensions = vec!["pdf".into(), "txt".into()];
    store.profile_mut().allow_downloads = true;
    store.save().unwrap();

    // Advanced fields remain editable irrespective of onboarding. The
    // deprecated JSON shim does not persist them.
    assert!(!store.onboarding_completed());
    assert_eq!(store.profile().max_file_size, 50 * 1024 * 1024);
    assert_eq!(
        store.profile().allowed_extensions,
        vec!["pdf".to_string(), "txt".to_string()]
    );
    assert!(store.profile().allow_downloads);
    assert!(!store.file_path().exists());
    cleanup(&dir);
}

// ── 9. EDGE CASES ──────────────────────────────────────────────────────────

#[test]
fn two_independent_profiles_have_independent_onboarding() {
    // Two users in different directories should have independent state.
    let dir1 = temp_dir("indep_user1");
    let dir2 = temp_dir("indep_user2");
    let key1 = test_key();
    let key2 = test_key_2();

    // User 1 completes onboarding
    let mut s1 = UserProfileStore::empty_at(&dir1, key1);
    s1.set_onboarding_completed(true);
    s1.profile_mut().display_name = "User1".into();
    s1.save().unwrap();

    // User 2 stays fresh
    let mut s2 = UserProfileStore::empty_at(&dir2, key2);
    s2.profile_mut().display_name = "User2".into();
    // Don't set onboarding — let it be None
    s2.save().unwrap();

    // The active store instances remain independent. Their deprecated JSON
    // saves do not persist either instance to a new load.
    assert!(s1.onboarding_completed(), "User1 should be onboarded");
    assert!(
        !s2.onboarding_completed(),
        "User2 should remain fresh without explicit completion"
    );
    assert!(!UserProfileStore::load(&dir1, key1)
        .unwrap()
        .onboarding_completed());
    assert!(!UserProfileStore::load(&dir2, key2)
        .unwrap()
        .onboarding_completed());

    cleanup(&dir1);
    cleanup(&dir2);
}

#[test]
fn onboarding_serde_skip_keeps_field_out_of_json() {
    // Verify that the store-level flag serializes, while the inner profile
    // object does not acquire a duplicate onboarding field.
    let dir = temp_dir("serde_skip_json");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(true);
    store.profile_mut().display_name = "SafeUser".into();
    let raw = serde_json::to_string(&store).unwrap();

    // The store-level onboarding_completed field should be serialized.
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["onboarding_completed"], true);

    // The profile's onboarding_completed field (from the inner profile struct)
    // should NOT appear in the JSON because of #[serde(skip)]
    // We can verify this by parsing and checking the inner struct doesn't have it
    // Actually, serde(skip) means the field simply isn't in the JSON output
    // The store serializes its own onboarding_completed field which covers this.

    // Parse the JSON and verify the profile object doesn't have the field
    let profile_obj = value.get("profile").unwrap();
    assert!(
        !profile_obj
            .as_object()
            .unwrap()
            .contains_key("onboarding_completed"),
        "profile-level onboarding_completed should NOT appear in JSON (#[serde(skip)])"
    );
    cleanup(&dir);
}

#[test]
fn load_or_default_fallback_preserves_onboarding_state() {
    // When load fails (corrupt file), load_or_default returns empty_at.
    // Verify the resulting store correctly reports incomplete onboarding.
    let dir = temp_dir("load_fallback");
    let key = test_key();
    fs::create_dir_all(&dir).unwrap();

    // Write corrupt profile.json
    fs::write(dir.join("profile.json"), "This is not valid JSON").unwrap();

    let store = UserProfileStore::load_or_default(&dir, key);
    assert!(
        !store.onboarding_completed(),
        "fallback store should have incomplete onboarding"
    );
    cleanup(&dir);
}

#[test]
fn onboarding_true_stays_true_after_profile_edits_in_memory() {
    // Profile edits do not change onboarding in the same active store.
    let dir = temp_dir("stays_true");
    let key = test_key();
    fs::create_dir_all(&dir).unwrap();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(true);
    store.profile_mut().display_name = "StaysTrue".into();
    store.save().unwrap();

    // Profile edit + deprecated save on the same active store.
    store.profile_mut().bio = "Updated bio".into();
    store.save().unwrap();

    assert!(
        store.onboarding_completed(),
        "onboarding should remain true after profile edits"
    );
    assert_eq!(store.profile().display_name, "StaysTrue");
    assert_eq!(store.profile().bio, "Updated bio");
    assert!(!store.file_path().exists());
    cleanup(&dir);
}
