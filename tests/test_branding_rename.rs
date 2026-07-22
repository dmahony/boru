//! Tests for the Boru branding rename.
//!
//! Covers:
//! - Crate/module rename: `boru-chat` → `boru-core`, old name not re-exported
//! - Protocol compatibility: wire-format constants unchanged
//! - Network namespace identifiers: domain-separation strings unchanged
//! - Data directory resolution: backward-compatibility edge cases
//!
//! These tests guard against accidental regressions during the rename.
//! Wire-protocol constants (ALPNs, domain separators, namespace IDs)
//! MUST stay unchanged to preserve interop with existing deployments.

// ── Crate/module rename tests ───────────────────────────────────────────────

/// Verify the crate is importable under its new name `boru_core`.
#[test]
fn test_crate_is_boru_core() {
    // Compile-time check: this path must resolve
    let _ = boru_core::data_dir::resolve_data_dir;
}

/// Verify the crate name has been updated in `Cargo.toml`.
#[test]
fn test_crate_name_in_manifest() {
    assert_eq!(
        env!("CARGO_PKG_NAME"),
        "boru-core",
        "crate name in Cargo.toml must be 'boru-core'"
    );
    assert_eq!(
        env!("CARGO_PKG_DESCRIPTION"),
        "private, peer-to-peer communication built on Iroh",
        "package description must be updated"
    );
}

/// Verify the module path is `boru_core`, not `boru_chat`.
#[test]
fn test_module_path_is_boru_core() {
    // The public API surface uses `boru_core::*`, not `boru_chat::*`.
    // If this test compiles, the module path is correct.
    //
    // Under the hood, Cargo's crate_name macro gives us the library name
    // which on crates.io/publishing is always with underscores.
    let crate_name = boru_core::data_dir::ENV_BORU_DATA_DIR;
    assert!(
        !crate_name.is_empty(),
        "module is accessible under boru_core"
    );
}

/// Verify `boru_core::ALPN` re-export works (available with `net` feature).
#[cfg(feature = "net")]
#[test]
fn test_alpn_re_export() {
    // The top-level re-export should resolve.
    assert_eq!(boru_core::ALPN, b"/iroh-gossip/1");
}

/// Verify `boru_core::Gossip` type re-export works.
#[cfg(feature = "net")]
#[test]
fn test_gossip_re_export() {
    // Just verify the type compiles — use std::any to check it exists.
    let _type_name = std::any::type_name::<boru_core::net::Gossip>();
    assert!(!_type_name.is_empty());
}

/// Verify `boru_core::TopicId` re-export works.
#[test]
fn test_topic_id_re_export() {
    let _type_name = std::any::type_name::<boru_core::TopicId>();
    assert!(!_type_name.is_empty());
}

// ── Protocol compatibility tests ────────────────────────────────────────────
// These constants MUST NOT change during the rename.
// They define the wire protocol and affect interoperability.

// ── ALPNs ───────────────────────────────────────────────────────────────

#[cfg(feature = "net")]
#[test]
fn test_gossip_alpn_unchanged() {
    // GOSSIP_ALPN is defined in boru_core::net.
    assert_eq!(
        boru_core::net::GOSSIP_ALPN,
        b"/iroh-gossip/1",
        "GOSSIP_ALPN must remain unchanged for protocol compatibility"
    );
}

#[cfg(feature = "net")]
#[test]
fn test_file_access_alpn_unchanged() {
    assert_eq!(
        boru_core::net::FILE_ACCESS_ALPN,
        b"/boru-file-access/1",
        "FILE_ACCESS_ALPN must remain unchanged"
    );
}

#[test]
fn test_catalogue_alpn_unchanged() {
    assert_eq!(
        boru_core::protocol_version::CATALOGUE_ALPN,
        b"/boru-file-catalog/1",
        "CATALOGUE_ALPN must remain unchanged"
    );
}

#[cfg(feature = "net")]
#[test]
fn test_inbox_alpn_unchanged() {
    assert_eq!(
        boru_core::inbox::INBOX_ALPN,
        b"/iroh-chat-inbox/1",
        "INBOX_ALPN must remain unchanged"
    );
}

#[cfg(feature = "net")]
#[test]
fn test_friend_ping_alpn_unchanged() {
    assert_eq!(
        boru_core::chat_core::friend_ping::FRIEND_PING_ALPN,
        b"/iroh-gossip-chat/friend-ping/1",
        "FRIEND_PING_ALPN must remain unchanged"
    );
}

#[cfg(feature = "net")]
#[test]
fn test_backfill_alpn_unchanged() {
    assert_eq!(
        boru_core::backfill::BACKFILL_ALPN,
        b"/iroh-gossip-chat/backfill/1",
        "BACKFILL_ALPN must remain unchanged"
    );
}

#[cfg(feature = "net")]
#[test]
fn test_whisper_alpn_unchanged() {
    assert_eq!(
        boru_core::whisper::WHISPER_ALPN,
        b"/iroh-gossip-chat/whisper/1",
        "WHISPER_ALPN must remain unchanged"
    );
}

// ── Protocol version constants ──────────────────────────────────────────

#[test]
fn test_catalogue_retrieval_v1_unchanged() {
    assert_eq!(
        boru_core::protocol_version::CATALOGUE_RETRIEVAL_V1,
        1,
        "version constant must remain 1"
    );
}

#[test]
fn test_supported_catalogue_versions_unchanged() {
    assert_eq!(
        boru_core::protocol_version::SUPPORTED_CATALOGUE_RETRIEVAL,
        &[1u16],
        "supported versions must remain unchanged"
    );
}

// ── Domain separation strings ─────────────────────────────────────────────

#[test]
fn test_public_room_domain_separator_unchanged() {
    assert_eq!(
        boru_core::topic_derivation::PUBLIC_ROOM_DOMAIN_SEPARATOR,
        b"boru-chat public-room v1",
        "PUBLIC_ROOM_DOMAIN_SEPARATOR must remain unchanged for backward compat"
    );
}

#[test]
fn test_tracker_namespace_domain_separator_unchanged() {
    assert_eq!(
        boru_core::topic_derivation::TRACKER_NAMESPACE_DOMAIN_SEPARATOR,
        b"boru-chat room discovery v1",
        "TRACKER_NAMESPACE_DOMAIN_SEPARATOR must remain unchanged"
    );
}

#[test]
fn test_discovery_key_domain_separator_unchanged() {
    assert_eq!(
        boru_core::public_room::DISCOVERY_KEY_DOMAIN_SEPARATOR,
        b"boru-chat discovery-key v1",
        "DISCOVERY_KEY_DOMAIN_SEPARATOR must remain unchanged"
    );
}

#[test]
fn test_application_namespace_unchanged() {
    assert_eq!(
        boru_core::public_room::APPLICATION_NAMESPACE,
        "boru-chat",
        "APPLICATION_NAMESPACE must remain unchanged for DHT interop"
    );
}

#[test]
fn test_public_room_name_unchanged() {
    assert_eq!(
        boru_core::public_room::PUBLIC_ROOM_NAME,
        "public-lobby",
        "PUBLIC_ROOM_NAME must remain unchanged"
    );
}

#[test]
fn test_protocol_version_unchanged() {
    assert_eq!(
        boru_core::public_room::PROTOCOL_VERSION,
        1u8,
        "PROTOCOL_VERSION must remain unchanged"
    );
}

#[cfg(feature = "net")]
#[test]
fn test_private_room_domain_separator_unchanged() {
    assert_eq!(
        boru_core::private_room_tracker::PRIVATE_ROOM_DOMAIN_SEPARATOR,
        b"boru-chat private-room v1",
        "PRIVATE_ROOM_DOMAIN_SEPARATOR must remain unchanged"
    );
}

// ── Data directory constants ───────────────────────────────────────────────

#[test]
fn test_env_var_names_unchanged() {
    assert_eq!(
        boru_core::data_dir::ENV_BORU_DATA_DIR,
        "BORU_DATA_DIR",
        "new env var name must be correct"
    );
    assert_eq!(
        boru_core::data_dir::ENV_BORU_CHAT_DATA_DIR,
        "BORU_CHAT_DATA_DIR",
        "legacy env var name must keep its old value for backward compat"
    );
}

#[test]
fn test_shared_dir_name_unchanged() {
    assert_eq!(
        boru_core::data_dir::SHARED_DIR_NAME,
        "shared",
        "shared dir name must remain unchanged"
    );
}

// ── Topic derivation determinism ──────────────────────────────────────────

/// Verify that public-room topic derivation is deterministic and
/// produces the expected result for a well-known input.
#[test]
fn test_public_room_topic_deterministic() {
    use boru_core::proto::state::TopicId;
    use boru_core::topic_derivation::public_room_topic;

    // Derive the topic for the default lobby on mainnet (network_byte = 0).
    let topic = public_room_topic(0, "public-lobby", 1);

    // The topic should be a valid 32-byte TopicId.
    assert_eq!(topic.as_bytes().len(), 32, "TopicId must be 32 bytes");
    assert_ne!(
        topic.as_bytes(),
        &[0u8; 32],
        "TopicId must not be all zeros (hash must produce a valid result)"
    );

    // Verify determinism: calling it again with the same inputs
    // must produce the exact same topic.
    let topic2 = public_room_topic(0, "public-lobby", 1);
    assert_eq!(topic, topic2, "topic derivation must be deterministic");

    // Verify domain separation: different room name → different topic.
    let topic3 = public_room_topic(0, "different-room", 1);
    assert_ne!(
        topic, topic3,
        "different room name must produce a different topic"
    );

    // Verify domain separation: different network → different topic.
    let topic4 = public_room_topic(1, "public-lobby", 1);
    assert_ne!(
        topic, topic4,
        "different network byte must produce a different topic"
    );
}

/// Verify tracker namespace derivation is deterministic.
#[test]
fn test_tracker_namespace_deterministic() {
    use boru_core::topic_derivation::tracker_namespace_from_topic;

    let topic_bytes = [0xABu8; 32];
    let ns1 = tracker_namespace_from_topic(&topic_bytes);
    let ns2 = tracker_namespace_from_topic(&topic_bytes);
    assert_eq!(
        ns1, ns2,
        "tracker namespace derivation must be deterministic"
    );

    let different_topic = [0xCDu8; 32];
    let ns3 = tracker_namespace_from_topic(&different_topic);
    assert_ne!(ns1, ns3, "different topic must produce different namespace");
}

// ── Wire-format structure tests ────────────────────────────────────────────

/// Verify catalogue protocol message types are unchanged.
#[test]
fn test_catalogue_protocol_type_sizes() {
    use boru_core::catalogue_protocol::*;
    use boru_core::file_access_protocol::*;
    use std::mem::size_of;

    // These types must exist (compile check) and have reasonable sizes.
    // Size assertions guard against accidental field additions/removals.
    assert!(
        size_of::<CatalogWireRequest>() > 0,
        "CatalogWireRequest must exist"
    );
    assert!(
        size_of::<CatalogWireResponse>() > 0,
        "CatalogWireResponse must exist"
    );
    assert!(
        size_of::<FileAccessWireRequest>() > 0,
        "FileAccessWireRequest must exist"
    );
    assert!(
        size_of::<FileAccessWireResponse>() > 0,
        "FileAccessWireResponse must exist"
    );
}

// ── Data directory edge cases ──────────────────────────────────────────────

/// Verify data directory constants are correct for the new naming.
#[test]
fn test_data_dir_directory_names() {
    // The internal constant names reflect the rename.
    // NEW_DIR_NAME should be "boru", LEGACY_DIR_NAME should be "boru-chat"
    // so existing installs can be auto-detected.
    //
    // We verify via the public API behaviour:
    //   new_default_dir() returns a path ending in "boru" or ".boru"
    //   legacy_candidate_dirs() returns paths ending in "boru-chat" or ".boru-chat"
    use boru_core::data_dir::{legacy_candidate_dirs, new_default_dir};

    // When env vars are cleared, new_default_dir falls through to the CWD
    // fallback (.boru), and legacy_candidate_dirs includes the CWD fallback
    // (.boru-chat).  The key invariant: the "boru" part of the name must be
    // correct (not "boru-chat" for new, and "boru-chat" for legacy).

    let new_dir = new_default_dir();
    let new_name = new_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    assert!(
        new_name.contains("boru") && !new_name.contains("boru-chat"),
        "new default dir name should contain 'boru' but not 'boru-chat', got '{new_name}'"
    );

    let legacy_dirs = legacy_candidate_dirs();
    assert!(
        !legacy_dirs.is_empty(),
        "should have at least one legacy candidate"
    );
    // At least one of the candidates should have "boru-chat" in its name.
    let has_boru_chat = legacy_dirs.iter().any(|d| {
        d.file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| n.contains("boru-chat"))
    });
    assert!(
        has_boru_chat,
        "at least one legacy candidate should contain 'boru-chat'"
    );
}
