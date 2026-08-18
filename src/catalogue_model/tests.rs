//! Unit tests for the catalogue data model (catalogue_model).
//!
//! Covers RemoteSharedFile validation, signed-catalogue framing
//! (BORU-AUDIT-27 canonical framing), folder children, TryFrom<SharedFile>,
//! SignedFileCatalogue, and SignedCatalogueCursor.

use super::signed::{
    legacy_signing_payload, signing_payload, CATALOGUE_PROTOCOL, CATALOGUE_VERSION,
};
use super::*;
use iroh::{PublicKey, SecretKey};
use serde_byte_array::ByteArray;

// ── RemoteSharedFile validation ─────────────────────────────────────

#[test]
fn valid_default_remote_shared_file() {
    let f = RemoteSharedFile::new("abc123", "photo.jpg", None, 42_000, "image/jpeg", None, 1);
    assert!(f.validate().is_ok());
}

#[test]
fn empty_shared_file_id_rejected() {
    let f = RemoteSharedFile {
        shared_file_id: String::new(),
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn empty_display_name_rejected() {
    let f = RemoteSharedFile {
        display_name: String::new(),
        ..RemoteSharedFile::new("hash", "x", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn path_separator_in_display_name_rejected() {
    let f = RemoteSharedFile {
        display_name: "../secret.txt".into(),
        ..RemoteSharedFile::new("hash", "x", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err(), "path separator must be rejected");
}

#[test]
fn path_separator_in_shared_file_id_rejected() {
    for sep in &["/", "\\"] {
        let f = RemoteSharedFile {
            shared_file_id: format!("sub{}dir/id", sep),
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(
            f.validate().is_err(),
            "shared_file_id containing '{}' must be rejected",
            sep
        );
    }
}

#[test]
fn empty_mime_type_rejected() {
    let f = RemoteSharedFile {
        mime_type: String::new(),
        ..RemoteSharedFile::new("hash", "name", None, 100, "x", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn mime_type_without_slash_rejected() {
    let f = RemoteSharedFile {
        mime_type: "application".into(),
        ..RemoteSharedFile::new("hash", "name", None, 100, "x", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn empty_content_hash_rejected() {
    let f = RemoteSharedFile {
        content_hash: String::new(),
        ..RemoteSharedFile::new("x", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn field_length_limits_enforced() {
    // shared_file_id too long
    let long = "x".repeat(MAX_SHARED_FILE_ID_LENGTH + 1);
    let f = RemoteSharedFile {
        shared_file_id: long,
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());

    // display_name too long
    let long = "x".repeat(MAX_DISPLAY_NAME_LENGTH + 1);
    let f = RemoteSharedFile {
        display_name: long,
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());

    // mime_type too long
    let long = format!("{}/x", "a".repeat(MAX_MIME_TYPE_LENGTH));
    let f = RemoteSharedFile {
        mime_type: long,
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());

    // content_hash too long
    let long = "x".repeat(MAX_CONTENT_HASH_LENGTH + 1);
    let f = RemoteSharedFile {
        content_hash: long,
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn description_length_limits_enforced() {
    let long = "x".repeat(MAX_DESCRIPTION_LENGTH + 1);
    let f = RemoteSharedFile {
        description: Some(long),
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn descriptions_allow_multiline_text_but_reject_controls_and_formats() {
    let multiline = "first line\r\n\tsecond line\nthird line";
    let file = RemoteSharedFile {
        description: Some(multiline.into()),
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(
        file.validate().is_ok(),
        "documented multiline text is valid"
    );

    for description in ["bad\0text", "bad\u{7f}text", "bad\u{200b}text"] {
        let file = RemoteSharedFile {
            description: Some(description.into()),
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        };
        assert!(
            file.validate().is_err(),
            "unsafe file description accepted: {description:?}"
        );
    }

    let file_collection = FileCatalogueCollection {
        collection_id: "docs".into(),
        name: "Documents".into(),
        description: Some(multiline.into()),
    };
    assert!(file_collection.validate().is_ok());
    let file_collection = FileCatalogueCollection {
        description: Some("bad\u{202e}text".into()),
        ..file_collection
    };
    assert!(file_collection.validate().is_err());

    let remote_collection = RemoteCollection {
        collection_id: "docs".into(),
        name: "Documents".into(),
        description: Some(multiline.into()),
        sort_order: 0,
    };
    assert!(remote_collection.validate().is_ok());
    let remote_collection = RemoteCollection {
        description: Some("bad\u{2066}text".into()),
        ..remote_collection
    };
    assert!(remote_collection.validate().is_err());
}

#[test]
fn signed_catalogue_rejects_malformed_description_before_display() {
    let sk = SecretKey::generate();
    let catalogue = SignedFileCatalogue::sign(
        &sk,
        1,
        now_ms(),
        vec![],
        vec![RemoteSharedFile {
            description: Some("forged\u{200b}description".into()),
            ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
        }],
    );

    assert!(
        catalogue.verify().is_ok(),
        "the fixture is correctly signed"
    );
    assert!(
        catalogue.validate().is_err(),
        "signed but malformed metadata must be rejected before display"
    );
}

#[test]
fn oversized_collection_list_rejected() {
    let ids: Vec<String> = (0..MAX_COLLECTION_IDS + 1)
        .map(|i| format!("col-{}", i))
        .collect();
    let f = RemoteSharedFile {
        collection_ids: ids,
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(
        f.validate().is_err(),
        "oversized collection list must be rejected"
    );
}

// ── BORU-AUDIT-27: canonical catalogue framing ─────────────────────────

/// The canonical bytes a catalogue signs must be stable: domain tag,
/// version, then every security-relevant field.
#[test]
fn signed_catalogue_canonical_bytes_golden_vector() {
    let sk = SecretKey::generate();
    let catalogue = SignedFileCatalogue::sign(&sk, 3, 1_700_000_000, vec![], vec![]);
    let canonical = signing_payload(&catalogue);
    assert_eq!(canonical[0] as usize, CATALOGUE_PROTOCOL.len());
    assert_eq!(
        &canonical[1..1 + CATALOGUE_PROTOCOL.len()],
        CATALOGUE_PROTOCOL.as_bytes()
    );
    assert_eq!(canonical[1 + CATALOGUE_PROTOCOL.len()], 0x01);
    let decoded: (
        String,
        u16,
        PublicKey,
        u64,
        u64,
        Vec<FileCatalogueCollection>,
        Vec<RemoteSharedFile>,
    ) = postcard::from_bytes(&canonical).expect("decode canonical catalogue bytes");
    assert_eq!(decoded.0, CATALOGUE_PROTOCOL);
    assert_eq!(decoded.1, CATALOGUE_VERSION);
    assert_eq!(decoded.2, catalogue.owner_id);
    assert_eq!(decoded.3, catalogue.revision);
    assert_eq!(decoded.4, catalogue.generated_at_ms);
}

/// Cross-version: a pre-AUDIT-27 catalogue signed over the bare tuple
/// (no domain tag) still verifies during the migration window.
#[test]
fn signed_catalogue_legacy_framing_still_verifies() {
    let sk = SecretKey::generate();
    let mut catalogue = SignedFileCatalogue::sign(&sk, 3, 1_700_000_000, vec![], vec![]);
    let legacy = legacy_signing_payload(&catalogue);
    catalogue.signature = ByteArray::new(sk.sign(&legacy).to_bytes());
    assert!(
        catalogue.verify().is_ok(),
        "legacy-framed catalogue must verify during migration (BORU-AUDIT-27)"
    );
}

#[test]
fn empty_collection_id_rejected() {
    let f = RemoteSharedFile {
        collection_ids: vec!["valid".into(), String::new(), "also-valid".into()],
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn valid_collection_ids_ok() {
    let f = RemoteSharedFile {
        collection_ids: vec!["photos".into(), "documents".into()],
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_ok());
}

#[test]
fn metadata_rejects_control_characters_and_unsafe_ids() {
    let base = RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1);
    for value in ["name\n.txt", "name\0.txt", "name\u{7f}.txt"] {
        let f = RemoteSharedFile {
            display_name: value.into(),
            ..base.clone()
        };
        assert!(f.validate().is_err(), "unsafe filename accepted: {value:?}");
    }
    for value in ["id with spaces", "id/with-slash", "id\\with-slash", "id\n"] {
        let f = RemoteSharedFile {
            shared_file_id: value.into(),
            ..base.clone()
        };
        assert!(f.validate().is_err(), "unsafe id accepted: {value:?}");
    }
}

#[test]
fn metadata_rejects_invalid_mime_types() {
    let base = RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1);
    for mime in [
        "text/",
        "/plain",
        "text/plain;\n",
        "text plain",
        "TEXT/PLAIN",
    ] {
        let f = RemoteSharedFile {
            mime_type: mime.into(),
            ..base.clone()
        };
        assert!(f.validate().is_err(), "invalid MIME accepted: {mime:?}");
    }
}

#[test]
fn metadata_rejects_oversized_files_and_future_timestamps() {
    let f = RemoteSharedFile {
        size_bytes: crate::catalogue_limits::MAX_FILE_SIZE_BYTES + 1,
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
    // Double the allowed skew so the value is deterministically beyond
    // the bound even though validate() re-reads the clock moments later
    // (adding exactly SKEW+1ms races the clock and flakes).
    let f = RemoteSharedFile {
        updated_at_ms: now_ms().saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS * 2),
        ..RemoteSharedFile::new("hash", "name", None, 100, "text/plain", None, 1)
    };
    assert!(f.validate().is_err());
}

#[test]
fn collection_validation_rejects_unsafe_metadata() {
    let collection = FileCatalogueCollection {
        collection_id: "collection id".into(),
        name: "safe".into(),
        description: None,
    };
    assert!(collection.validate().is_err());
    let collection = FileCatalogueCollection {
        collection_id: "collection".into(),
        name: "name\n".into(),
        description: None,
    };
    assert!(collection.validate().is_err());
}

// ── Folder share (SENDME-01 children) ──────────────────────────────

#[test]
fn folder_entry_with_children_validates_and_is_folder() {
    let child = RemoteSharedFile::new(
        "child-hash",
        "report.pdf",
        None,
        1024,
        "application/pdf",
        None,
        1,
    );
    let folder = RemoteSharedFile {
        display_name: "documents".into(),
        size_bytes: 2048,
        children: vec![child],
        ..RemoteSharedFile::new(
            "collection-root",
            "documents",
            None,
            2048,
            "inode/directory",
            None,
            1,
        )
    };
    assert!(folder.is_folder(), "entry with children is a folder");
    assert_eq!(folder.folder_children().len(), 1);
    assert!(folder.validate().is_ok());
}

#[test]
fn single_file_entry_is_not_a_folder() {
    let file = RemoteSharedFile::new("hash", "photo.jpg", None, 100, "image/jpeg", None, 1);
    assert!(!file.is_folder());
    assert!(file.folder_children().is_empty());
    assert!(file.validate().is_ok());
}

#[test]
fn folder_child_with_invalid_fields_rejected() {
    // A folder entry whose child fails validation must fail as a whole:
    // hostile children (bad ids, names, sizes) cannot ride inside a
    // valid-looking folder.
    let bad_child = RemoteSharedFile {
        shared_file_id: String::new(), // empty → invalid
        ..RemoteSharedFile::new("h", "x", None, 1, "text/plain", None, 1)
    };
    let folder = RemoteSharedFile {
        children: vec![bad_child],
        ..RemoteSharedFile::new(
            "collection-root",
            "docs",
            None,
            1,
            "inode/directory",
            None,
            1,
        )
    };
    assert!(folder.validate().is_err());
}

#[test]
fn folder_depth_and_entry_count_bounded() {
    // Depth: a 33-level chain must be rejected (bound is 32).
    let mut entry = RemoteSharedFile::new("h", "leaf", None, 1, "text/plain", None, 1);
    for i in 0..33 {
        entry = RemoteSharedFile {
            display_name: format!("level-{i}"),
            children: vec![entry],
            ..RemoteSharedFile::new("h", "folder", None, 1, "inode/directory", None, 1)
        };
    }
    assert!(
        entry.validate().is_err(),
        "excessively deep folder must be rejected"
    );

    // Entry count: a folder with more than MAX_ENTRIES_PER_COLLECTION
    // children must be rejected (allocating 10_001 entries is fine for
    // a test but keep it deterministic — construct just over the bound
    // using the constant directly).
    let too_many = crate::catalogue_limits::MAX_ENTRIES_PER_COLLECTION + 1;
    let children: Vec<RemoteSharedFile> = (0..too_many)
        .map(|i| RemoteSharedFile::new(format!("h{i}"), "x", None, 1, "text/plain", None, 1))
        .collect();
    let folder = RemoteSharedFile {
        children,
        ..RemoteSharedFile::new("root", "big", None, 1, "inode/directory", None, 1)
    };
    assert!(folder.validate().is_err());
}

#[test]
fn folder_children_survive_postcard_roundtrip_and_default_to_empty() {
    let child = RemoteSharedFile::new("ch", "a.txt", None, 10, "text/plain", None, 1);
    let folder = RemoteSharedFile {
        display_name: "photos".into(),
        children: vec![child],
        ..RemoteSharedFile::new("root", "photos", None, 10, "inode/directory", None, 1)
    };
    let bytes = postcard::to_stdvec(&folder).unwrap();
    let decoded: RemoteSharedFile = postcard::from_bytes(&bytes).unwrap();
    assert!(decoded.is_folder());
    assert_eq!(decoded.folder_children()[0].display_name, "a.txt");
    assert!(decoded.validate().is_ok());

    // A legacy payload (no `children` field) decodes with an empty vec —
    // same serde-default pattern as `description`/`collection_ids`.
    let legacy = RemoteSharedFile::new(
        "old",
        "legacy.bin",
        None,
        10,
        "application/octet-stream",
        None,
        1,
    );
    let legacy_bytes = postcard::to_stdvec(&legacy).unwrap();
    let decoded_legacy: RemoteSharedFile = postcard::from_bytes(&legacy_bytes).unwrap();
    assert!(!decoded_legacy.is_folder());
    assert!(decoded_legacy.folder_children().is_empty());
}

// ── TryFrom<SharedFile> ─────────────────────────────────────────────

#[test]
fn absolute_path_rejected() {
    let local = SharedFile {
        id: "file-1".into(),
        filename: "doc.pdf".into(),
        path: std::path::PathBuf::from("/etc/passwd"),
        size: 100,
        mime_type: "application/pdf".into(),
        modified_time: UNIX_EPOCH,
        hash: Some([1u8; 32]),
        blob_id: None,
        over_limit: false,
        extension_blocked: false,
    };
    let result = RemoteSharedFile::try_from(&local);
    assert!(
        result.is_err(),
        "absolute paths must not be convertible to remote-safe entries"
    );
    let err = result.unwrap_err();
    assert!(
        err.reason.contains("absolute"),
        "error should mention 'absolute': {}",
        err.reason
    );
}

#[test]
fn parent_dir_path_rejected() {
    let local = SharedFile {
        id: "file-2".into(),
        filename: "leak.pdf".into(),
        path: std::path::PathBuf::from("../sensitive/data.pdf"),
        size: 200,
        mime_type: "application/pdf".into(),
        modified_time: UNIX_EPOCH,
        hash: Some([2u8; 32]),
        blob_id: None,
        over_limit: false,
        extension_blocked: false,
    };
    let result = RemoteSharedFile::try_from(&local);
    assert!(
        result.is_err(),
        "paths with parent-dir components must be rejected"
    );
}

#[test]
fn relative_path_converts_ok() {
    let local = SharedFile {
        id: "file-3".into(),
        filename: "safe.pdf".into(),
        path: std::path::PathBuf::from("shared/safe.pdf"),
        size: 300,
        mime_type: "application/pdf".into(),
        modified_time: UNIX_EPOCH,
        hash: Some([3u8; 32]),
        blob_id: None,
        over_limit: false,
        extension_blocked: false,
    };
    let remote = RemoteSharedFile::try_from(&local).expect("relative path should convert");
    assert_eq!(remote.shared_file_id, "file-3");
    assert_eq!(remote.display_name, "safe.pdf");
    assert_eq!(remote.mime_type, "application/pdf");
    assert_eq!(remote.size_bytes, 300);
    assert_eq!(remote.content_hash, hex::encode([3u8; 32]));
    assert_eq!(remote.version_number, 1);
    assert!(remote.collection_ids.is_empty());
}

#[test]
fn empty_path_converts_ok() {
    let local = SharedFile {
        id: "file-4".into(),
        filename: "empty_path.txt".into(),
        path: std::path::PathBuf::from(""),
        size: 50,
        mime_type: "text/plain".into(),
        modified_time: UNIX_EPOCH,
        hash: None,
        blob_id: None,
        over_limit: false,
        extension_blocked: false,
    };
    let remote = RemoteSharedFile::try_from(&local).expect("empty path should convert safely");
    assert!(remote.content_hash.is_empty(), "no hash -> empty string");
}

// ── SignedFileCatalogue ─────────────────────────────────────────────

#[test]
fn sign_and_verify_roundtrip() {
    let sk = SecretKey::generate();
    let files = vec![RemoteSharedFile::new(
        "hash1",
        "file1.txt",
        None,
        100,
        "text/plain",
        None,
        1,
    )];
    let catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
    assert!(
        catalogue.verify().is_ok(),
        "freshly-signed catalogue verifies"
    );
}

#[test]
fn tampered_revision_fails_verification() {
    let sk = SecretKey::generate();
    let files = vec![RemoteSharedFile::new(
        "hash1",
        "file1.txt",
        None,
        100,
        "text/plain",
        None,
        1,
    )];
    let mut catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
    catalogue.revision = 9_999_999;
    assert!(
        catalogue.verify().is_err(),
        "tampered revision must fail verification"
    );
}

#[test]
fn tampered_owner_id_fails_verification() {
    let sk = SecretKey::generate();
    let wrong_sk = SecretKey::generate();
    let files = vec![RemoteSharedFile::new(
        "hash1",
        "file1.txt",
        None,
        100,
        "text/plain",
        None,
        1,
    )];
    let mut catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
    catalogue.owner_id = wrong_sk.public();
    assert!(
        catalogue.verify().is_err(),
        "tampered owner_id must fail verification"
    );
}

#[test]
fn tampered_files_fails_verification() {
    let sk = SecretKey::generate();
    let files = vec![RemoteSharedFile::new(
        "hash1",
        "file1.txt",
        None,
        100,
        "text/plain",
        None,
        1,
    )];
    let mut catalogue = SignedFileCatalogue::sign(&sk, 1, 1000, vec![], files);
    catalogue.files.push(RemoteSharedFile::new(
        "tampered",
        "injected.txt",
        None,
        999,
        "text/plain",
        None,
        2,
    ));
    assert!(
        catalogue.verify().is_err(),
        "tampered files list must fail verification"
    );
}

#[test]
fn collections_roundtrip() {
    let sk = SecretKey::generate();
    let collections = vec![FileCatalogueCollection {
        collection_id: "col-1".into(),
        name: "Photos".into(),
        description: Some("My shared photos".into()),
    }];
    let files = vec![RemoteSharedFile::new(
        "hash1",
        "photo.jpg",
        None,
        5000,
        "image/jpeg",
        Some("col-1".into()),
        1,
    )];
    let catalogue = SignedFileCatalogue::sign(&sk, 2, 2000, collections, files);
    assert!(catalogue.verify().is_ok());
    assert_eq!(catalogue.revision, 2);
    assert_eq!(catalogue.collections.len(), 1);
    assert_eq!(catalogue.files.len(), 1);
    assert_eq!(catalogue.files[0].collection_ids, vec!["col-1".to_string()]);
}
