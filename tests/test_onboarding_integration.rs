//! Integration tests for onboarding persistence, migration inference,
//! and first-launch detection across the settings/storage boundary.
//!
//! These tests simulate the full app startup flow that `examples/iced_chat`
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

use boru_chat::user_profile::UserProfileStore;
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
/// This mirrors the logic in `examples/iced_chat/app.rs`:
/// 1. Load or create the profile store
/// 2. Infer onboarding from external data (friends, rooms, conversations)
/// 3. Save the inferred state to disk
/// 4. Return the store and its onboarding state
fn simulate_app_startup(data_dir: &Path, has_existing_data: bool) -> (UserProfileStore, bool) {
    let key = test_key();
    let mut profile_store = UserProfileStore::load_or_default(data_dir, key);

    // Infer from external context (the app checks friends, rooms, etc.)
    profile_store.infer_onboarding_from_external(has_existing_data);

    // Persist if inference changed anything
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
fn onboarding_persists_across_save_and_reload() {
    let dir = temp_dir("persist_save_reload");
    fs::create_dir_all(&dir).unwrap();

    // Create store with onboarding completed
    let mut store = UserProfileStore::empty_at(&dir, test_key());
    store.set_onboarding_completed(true);
    store.profile_mut().display_name = "TestUser".into();
    store.save().unwrap();

    // Reload from same directory
    let loaded = UserProfileStore::load(&dir, test_key()).unwrap();
    assert!(
        loaded.onboarding_completed(),
        "onboarding should survive save/load round-trip"
    );

    // Verify the profile data is intact too
    assert_eq!(loaded.profile().display_name, "TestUser");
    cleanup(&dir);
}

#[test]
fn onboarding_persists_across_multiple_save_cycles() {
    // Test the full lifecycle: true → save → reload → false → save → reload
    let dir = temp_dir("persist_multi_cycle");
    fs::create_dir_all(&dir).unwrap();
    let key = test_key();

    // Cycle 1: set and persist onboarding=true
    {
        let mut store = UserProfileStore::empty_at(&dir, key);
        store.set_onboarding_completed(true);
        store.profile_mut().display_name = "User".into();
        store.save().unwrap();
    }

    // Verify after cycle 1
    {
        let loaded = UserProfileStore::load(&dir, key).unwrap();
        assert!(
            loaded.onboarding_completed(),
            "cycle 1: should be onboarded"
        );
    }

    // Cycle 2: reset to false
    {
        let mut store = UserProfileStore::load(&dir, key).unwrap();
        store.set_onboarding_completed(false);
        store.save().unwrap();
    }

    // Verify after cycle 2
    {
        let loaded = UserProfileStore::load(&dir, key).unwrap();
        assert!(
            !loaded.onboarding_completed(),
            "cycle 2: onboarding should be reset to false"
        );
    }

    // Cycle 3: set back to true
    {
        let mut store = UserProfileStore::load(&dir, key).unwrap();
        store.set_onboarding_completed(true);
        store.save().unwrap();
    }

    // Verify after cycle 3
    {
        let loaded = UserProfileStore::load(&dir, key).unwrap();
        assert!(
            loaded.onboarding_completed(),
            "cycle 3: onboarding should be true again"
        );
    }
    cleanup(&dir);
}

#[test]
fn onboarding_survives_app_restart() {
    // Simulate a full app restart: create the store, save, drop it,
    // then create a new store instance loading from the same directory.
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

    // "Second launch" — reload
    {
        let store = UserProfileStore::load(&dir, key).unwrap();
        assert!(
            store.onboarding_completed(),
            "onboarding should survive app restart"
        );
        assert_eq!(store.profile().display_name, "Alice");
    } // store dropped here — simulates another exit

    // "Third launch" — reload again to prove stable
    {
        let store = UserProfileStore::load(&dir, key).unwrap();
        assert!(
            store.onboarding_completed(),
            "onboarding should be stable across multiple restarts"
        );
        assert_eq!(store.profile().display_name, "Alice");
    }
    cleanup(&dir);
}

// ── 3. RESET ───────────────────────────────────────────────────────────────

#[test]
fn onboarding_reset_allows_reonboarding() {
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

    // Reset (like ShowOnboardingAgain)
    let mut store = UserProfileStore::load(&dir, key).unwrap();
    store.set_onboarding_completed(false);
    store.save().unwrap();

    // Reload — should be false
    let loaded = UserProfileStore::load(&dir, key).unwrap();
    assert!(
        !loaded.onboarding_completed(),
        "onboarding should be false after reset"
    );

    // Complete onboarding again (simulating user going through flow again)
    let mut store = UserProfileStore::load(&dir, key).unwrap();
    store.set_onboarding_completed(true);
    store.save().unwrap();

    // Verify re-onboarding worked
    let loaded = UserProfileStore::load(&dir, key).unwrap();
    assert!(
        loaded.onboarding_completed(),
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
fn app_startup_persists_inferred_state() {
    // Simulate full cycle: first launch has existing data → onboarding is inferred.
    // Second launch should see the persisted onboarding state.
    let dir = temp_dir("startup_persist_inferred");
    fs::create_dir_all(&dir).unwrap();
    let key = test_key();

    // "First launch" — create profile with meaningful data, no onboarding field
    {
        let mut store = UserProfileStore::empty_at(&dir, key);
        store.profile_mut().display_name = "MigratedUser".into();
        // Don't set onboarding_completed — let inference handle it
        store.save().unwrap();
    }

    // "Second launch" — load and inference should fire
    {
        let store = UserProfileStore::load(&dir, key).unwrap();
        assert!(
            store.onboarding_completed(),
            "inference should fire on second launch with meaningful data"
        );
        // Persistence happens during load's inference
    }

    // "Third launch" — the state is now persisted, reload should be stable
    {
        let store = UserProfileStore::load(&dir, key).unwrap();
        assert!(
            store.onboarding_completed(),
            "persisted onboarding should be stable across restarts"
        );
    }
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
fn onboarding_dismissal_allows_normal_usage() {
    // Verify that having onboarding=false does not prevent the store from
    // functioning normally — settings can be changed, profile saved, etc.
    let dir = temp_dir("dismissal_normal");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(false); // dismissed onboarding
    store.profile_mut().display_name = "DismissedUser".into();
    store.profile_mut().bio = "Skipped onboarding".into();
    store.profile_mut().file_sharing_enabled = true;
    store.save().unwrap();

    // Reload — profile data intact, onboarding still false
    let loaded = UserProfileStore::load(&dir, key).unwrap();
    assert!(!loaded.onboarding_completed(), "onboarding remains false");
    assert_eq!(loaded.profile().display_name, "DismissedUser");
    assert_eq!(loaded.profile().bio, "Skipped onboarding");
    assert!(loaded.profile().file_sharing_enabled);

    // User can continue using the app: change settings, re-save
    let mut store = UserProfileStore::load(&dir, key).unwrap();
    store.profile_mut().display_name = "StillUsing".into();
    store.profile_mut().max_file_size = 200 * 1024 * 1024;
    store.save().unwrap();

    let loaded = UserProfileStore::load(&dir, key).unwrap();
    assert_eq!(loaded.profile().display_name, "StillUsing");
    assert_eq!(loaded.profile().max_file_size, 200 * 1024 * 1024);
    assert!(!loaded.onboarding_completed(), "onboarding still false");
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
    store.profile_mut().max_file_size = 500 * 1024 * 1024;
    store.profile_mut().allowed_extensions = vec!["pdf".into(), "txt".into()];
    store.profile_mut().allow_downloads = true;
    store.save().unwrap();

    // All advanced settings survive save/reload regardless of onboarding state
    let loaded = UserProfileStore::load(&dir, key).unwrap();
    assert!(!loaded.onboarding_completed());
    assert_eq!(loaded.profile().max_file_size, 500 * 1024 * 1024);
    assert_eq!(
        loaded.profile().allowed_extensions,
        vec!["pdf".to_string(), "txt".to_string()]
    );
    assert!(loaded.profile().allow_downloads);
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

    // Reload both
    let l1 = UserProfileStore::load(&dir1, key1).unwrap();
    let l2 = UserProfileStore::load(&dir2, key2).unwrap();

    // User 1 has explicit true, User 2's meaningful data triggers inference
    assert!(l1.onboarding_completed(), "User1 should be onboarded");
    assert!(
        l2.onboarding_completed(),
        "User2 should be onboarded via inference from display_name"
    );

    cleanup(&dir1);
    cleanup(&dir2);
}

#[test]
fn onboarding_serde_skip_keeps_field_out_of_json() {
    // Verify that onboarding_completed (which uses #[serde(skip)]) is NOT
    // included in the UserProfile's JSON representation, but IS included
    // in the UserProfileStore's JSON representation (at the store level).
    let dir = temp_dir("serde_skip_json");
    let key = test_key();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(true);
    store.profile_mut().display_name = "SafeUser".into();
    store.save().unwrap();

    // Read the raw JSON and inspect it
    let path = dir.join("profile.json");
    let raw = fs::read_to_string(&path).unwrap();

    // The store-level onboarding_completed field should be serialized
    assert!(
        raw.contains("\"onboarding_completed\": true"),
        "store-level onboarding_completed should be present in JSON, got: {raw}"
    );

    // The profile's onboarding_completed field (from the inner profile struct)
    // should NOT appear in the JSON because of #[serde(skip)]
    // We can verify this by parsing and checking the inner struct doesn't have it
    // Actually, serde(skip) means the field simply isn't in the JSON output
    // The store serializes its own onboarding_completed field which covers this.

    // Parse the JSON and verify the profile object doesn't have the field
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
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
fn onboarding_true_stays_true_on_subsequent_saves() {
    // Once onboarding is true and saved, subsequent saves (e.g. profile
    // changes) should not inadvertently reset it.
    let dir = temp_dir("stays_true");
    let key = test_key();
    fs::create_dir_all(&dir).unwrap();

    let mut store = UserProfileStore::empty_at(&dir, key);
    store.set_onboarding_completed(true);
    store.profile_mut().display_name = "StaysTrue".into();
    store.save().unwrap();

    // Profile edit + re-save
    let mut store = UserProfileStore::load(&dir, key).unwrap();
    store.profile_mut().bio = "Updated bio".into();
    store.save().unwrap();

    let loaded = UserProfileStore::load(&dir, key).unwrap();
    assert!(
        loaded.onboarding_completed(),
        "onboarding should remain true after subsequent saves"
    );
    assert_eq!(loaded.profile().display_name, "StaysTrue");
    assert_eq!(loaded.profile().bio, "Updated bio");
    cleanup(&dir);
}
