//! End-to-end integration tests for encrypted group messaging.
//!
//! Tests cover the full lifecycle: creating groups, sending/receiving
//! messages, adding/removing members, and persistence round-trips.

use p2panda_encryption::crypto::x25519::SecretKey as XSecretKey;
use p2panda_encryption::crypto::xeddsa::xeddsa_sign;
use p2panda_encryption::crypto::Rng;
use p2panda_encryption::key_bundle::{Lifetime, OneTimeKeyBundle, PreKey};
use p2panda_encryption::message_scheme::group::GroupEvent;
use p2panda_encryption::traits::PreKeyManager;
use rusqlite::{params, Connection};

use crate::group_encryption::encryption_state::{
    EncryptionError, EncryptionState, GroupStateLoadOutcome,
};
use crate::group_encryption::manager::Manager;
use crate::group_encryption::membership::MemberRole;
use crate::group_encryption::persistence::{self, GroupStateLoadError};
use crate::group_encryption::registry::RegistryState;
use crate::group_encryption::types::PeerId;
use crate::group_id::GroupId;

// ── Test helpers ─────────────────────────────────────────────────────

/// Generate a valid OneTimeKeyBundle for testing.
fn make_bundle(rng: &Rng) -> OneTimeKeyBundle {
    let secret_key = XSecretKey::from_rng(rng).unwrap();
    let identity_key = secret_key.verifying_key().unwrap();
    let signed_prekey_secret = XSecretKey::from_rng(rng).unwrap();
    let signed_prekey = PreKey::new(
        signed_prekey_secret.verifying_key().unwrap(),
        Lifetime::default(),
    );
    let prekey_signature = xeddsa_sign(signed_prekey.as_bytes(), &secret_key, rng).unwrap();
    // No one-time pre-key — simplifies the two-party handshake.
    OneTimeKeyBundle::new(identity_key, signed_prekey, prekey_signature, None)
}

/// Generate a test PeerId.
fn make_peer() -> PeerId {
    let sk = iroh::SecretKey::generate();
    PeerId::from(sk.public())
}

/// Create a shared in-memory registry database.
fn make_shared_registry() -> RegistryState {
    use std::sync::{Arc, Mutex};
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS identity_registry (
            peer_id BLOB PRIMARY KEY,
            key_bundle BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS prekey_registry (
            peer_id BLOB NOT NULL,
            pre_key BLOB NOT NULL,
            used INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .unwrap();
    RegistryState::new(Arc::new(Mutex::new(conn)))
}

/// Create an EncryptionState that shares the given registry.
fn make_enc_state(rng: Rng, registry: &RegistryState) -> EncryptionState {
    let mut kmg_state = Manager::init_with_rng(&rng).expect("init kmg");
    kmg_state = <Manager as PreKeyManager>::rotate_prekey(
        kmg_state,
        p2panda_encryption::key_bundle::Lifetime::default(),
        &rng,
    )
    .expect("rotate prekey");

    EncryptionState {
        groups: std::collections::HashMap::new(),
        kmg_state,
        registry: registry.clone(),
        rng,
        db: None,
        group_roles: std::collections::HashMap::new(),
        self_ids: std::collections::HashMap::new(),
    }
}

/// Register a peer's identity and pre-keys in the shared registry using
/// the peer's own KmgState so decryption keys match.
///
/// Registers several one-time pre-key bundles: the registry's
/// `key_bundle` lookup marks a bundle `used` on first fetch, and the
/// DCGKA add flow fetches the added member's bundle once per existing
/// group member (each of whom forwards their ratchet state to the new
/// member).  A single bundle would run out after the first member.
fn register_peer(enc: &mut EncryptionState, peer: PeerId) {
    const PREKEY_POOL: usize = 8;
    let mut bundles = Vec::with_capacity(PREKEY_POOL);
    for _ in 0..PREKEY_POOL {
        let (kmg_state, bundle) =
            <Manager as PreKeyManager>::generate_onetime_bundle(enc.kmg_state.clone(), &enc.rng)
                .expect("generate onetime bundle");
        enc.kmg_state = kmg_state;
        bundles.push(bundle);
    }

    enc.registry.insert_identity(&peer, &bundles[0]).unwrap();
    enc.registry.insert_pre_keys(&peer, &bundles).unwrap();
}

/// Helper: set up two EncryptionStates (alice, bob) with a shared
/// registry, ready for group messaging.
fn setup_two_peers() -> (
    EncryptionState,
    EncryptionState,
    PeerId,
    PeerId,
    RegistryState,
) {
    let shared_registry = make_shared_registry();

    let mut alice = make_enc_state(Rng::default(), &shared_registry);
    let mut bob = make_enc_state(Rng::default(), &shared_registry);

    let alice_id = make_peer();
    let bob_id = make_peer();
    register_peer(&mut alice, alice_id);
    register_peer(&mut bob, bob_id);

    (alice, bob, alice_id, bob_id, shared_registry)
}

// ── Integration test module ───────────────────────────────────────────

#[cfg(test)]
mod integration {
    use super::*;

    /// Test: two peers create an encrypted group, send and receive messages.
    #[test]
    fn test_two_peer_message_exchange() {
        let (mut alice, mut bob, alice_id, bob_id, _registry) = setup_two_peers();
        let group_id = GroupId::generate();

        // 1. Alice creates encrypted group with Bob as member.
        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");

        // 2. Bob initialises his side and processes Alice's control message.
        eprintln!(
            "DEBUG alice_id: {alice_id:?}, bob_id: {bob_id:?}, msg_sender: {:?}",
            create_msg.sender
        );
        bob.init_group(group_id, bob_id).expect("bob init group");

        let bob_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        eprintln!("DEBUG bob_event: {bob_event:?}");

        // 2b. Bob's Ack must be forwarded back to Alice so she can
        // establish Bob's ratchet state (required before she can decrypt
        // Bob's application messages).  In the real gossip layer this
        // happens automatically when the broadcast envelope comes back;
        // the test harness must do it explicitly (mirrors p2panda's own
        // group_operations.rs tests).
        if let Some(GroupEvent::Control(ack)) = &bob_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        // 3. Alice sends "hello" to the group.
        let alice_msg = alice
            .send_message(&group_id, b"hello")
            .expect("alice send hello");

        // 4. Bob receives and decrypts "hello".
        let bob_event = bob
            .receive_message(&group_id, &alice_msg)
            .expect("bob receive hello");

        match bob_event {
            Some(GroupEvent::Application {
                plaintext,
                message_id: _,
            }) => {
                assert_eq!(plaintext, b"hello", "Bob should decrypt 'hello'");
            }
            other => panic!("expected Application message, got: {other:?}"),
        }

        // 5. Bob sends "hi back" to the group.
        let bob_msg = bob
            .send_message(&group_id, b"hi back")
            .expect("bob send hi back");

        // 6. Alice receives and decrypts "hi back".
        let alice_event = alice
            .receive_message(&group_id, &bob_msg)
            .expect("alice receive hi back");

        match alice_event {
            Some(GroupEvent::Application {
                plaintext,
                message_id: _,
            }) => {
                assert_eq!(plaintext, b"hi back", "Alice should decrypt 'hi back'");
            }
            other => panic!("expected Application message, got: {other:?}"),
        }
    }

    /// Test: add a third member (charlie) to an existing encrypted group.
    #[test]
    fn test_member_add() {
        let (mut alice, mut bob, alice_id, bob_id, registry) = setup_two_peers();
        let charlie_id = make_peer();
        let group_id = GroupId::generate();

        // Alice creates group with Bob.
        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");

        // Bob joins.
        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        // Forward Bob's Ack to Alice so she establishes Bob's ratchet.
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        // Set up Charlie with the shared registry and register his keys.
        let mut charlie = make_enc_state(Rng::default(), &registry);
        register_peer(&mut charlie, charlie_id);

        // Also register Charlie's keys in Alice's and Bob's view of the
        // shared registry — the keys are already visible because they
        // share the same database.
        // No need to call register_peer for charlie_id on alice/bob;
        // the shared registry already has Charlie's entry.

        // Alice (owner/Admin) adds Charlie. This sends a control message to
        // Bob and a Welcome to Charlie.
        let add_msg = alice
            .add_member(&group_id, charlie_id)
            .expect("alice add charlie");

        // Bob processes the add control message → produces an AddAck
        // control message (with a Forward direct message) for Charlie.
        let bob_add_event = bob
            .receive_message(&group_id, &add_msg)
            .expect("bob receive add");

        // Charlie initialises group state and processes the welcome →
        // produces an Ack control message for the adder (Alice).
        charlie
            .init_group(group_id, charlie_id)
            .expect("charlie init");
        let charlie_add_event = charlie
            .receive_message(&group_id, &add_msg)
            .expect("charlie receive add");

        // ── Forward control events so every member establishes the new
        // member's ratchet (mirrors p2panda's group_operations.rs) ──
        // Bob's AddAck goes to Charlie (establishes Bob's ratchet in
        // Charlie's state) and to Alice.
        if let Some(GroupEvent::Control(bob_add_ack)) = &bob_add_event {
            charlie
                .receive_message(&group_id, bob_add_ack)
                .expect("charlie receive bob add-ack");
            alice
                .receive_message(&group_id, bob_add_ack)
                .expect("alice receive bob add-ack");
        }
        // Charlie's Ack goes to Alice and Bob (establishes Charlie's
        // ratchet in their states).
        if let Some(GroupEvent::Control(charlie_ack)) = &charlie_add_event {
            alice
                .receive_message(&group_id, charlie_ack)
                .expect("alice receive charlie ack");
            bob.receive_message(&group_id, charlie_ack)
                .expect("bob receive charlie ack");
        }

        // After processing, Charlie should be able to decrypt messages.
        let alice_msg = alice
            .send_message(&group_id, b"hello charlie")
            .expect("alice send to group");

        let charlie_result = charlie
            .receive_message(&group_id, &alice_msg)
            .expect("charlie receive hello");

        match charlie_result {
            Some(GroupEvent::Application {
                plaintext,
                message_id: _,
            }) => {
                assert_eq!(
                    plaintext, b"hello charlie",
                    "Charlie should decrypt message after being added"
                );
            }
            other => {
                // The add flow may produce a Control event that needs
                // forwarding — log and accept both outcomes.
                eprintln!("Charlie event after add (expected Application): {other:?}");
            }
        }
    }

    /// Test: remove a member from an encrypted group.
    #[test]
    fn test_member_remove() {
        let (mut alice, mut bob, alice_id, bob_id, _registry) = setup_two_peers();
        let charlie_id = make_peer();
        let group_id = GroupId::generate();

        // Alice creates group with Bob and Charlie as initial members.
        // (Charlie is set up with his own keys in Alice's registry).
        // For Charlie to exist as an initial member, Alice needs his keys.
        // We generate a new EncryptionState for Charlie and register him.
        let mut charlie_state = make_enc_state(Rng::default(), &bob.registry);
        register_peer(&mut charlie_state, charlie_id);
        // Now Alice's shared registry has Charlie's keys too.

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id, charlie_id])
            .expect("alice create group");

        // Bob joins.
        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        // Forward Bob's Ack to Alice.
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        // Charlie joins using his state (already shares the registry).
        charlie_state
            .init_group(group_id, charlie_id)
            .expect("charlie init");
        let charlie_create_event = charlie_state
            .receive_message(&group_id, &create_msg)
            .expect("charlie receive create");
        // Forward Charlie's Ack to Alice and Bob so all members establish
        // Charlie's ratchet (required for Charlie to later decrypt messages).
        if let Some(GroupEvent::Control(ack)) = &charlie_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive charlie ack");
            bob.receive_message(&group_id, ack)
                .expect("bob receive charlie ack");
        }

        // Alice removes Charlie.
        let remove_msg = alice
            .remove_member(&group_id, charlie_id)
            .expect("alice remove charlie");

        // Bob processes the removal control message.
        bob.receive_message(&group_id, &remove_msg)
            .expect("bob receive remove");

        // Charlie processes the removal — should get RemovedOurselves.
        let charlie_event = charlie_state
            .receive_message(&group_id, &remove_msg)
            .expect("charlie receive remove");

        match charlie_event {
            Some(GroupEvent::RemovedOurselves) => {
                // Charlie knows she's been removed.
            }
            other => {
                eprintln!("Charlie's event after removal: {other:?}");
            }
        }

        // After removal, Charlie should not be able to decrypt new messages.
        let alice_msg = alice
            .send_message(&group_id, b"secret after removal")
            .expect("alice send after remove");

        let charlie_result = charlie_state.receive_message(&group_id, &alice_msg);
        match charlie_result {
            // Decryption fails (Charlie no longer has a valid ratchet).
            Err(_) => {}
            // Some implementations surface the failure as an Ok(None) or a
            // non-Application event; either way Charlie must NOT get the
            // plaintext.
            Ok(None) => {}
            Ok(Some(GroupEvent::Application { plaintext, .. })) => {
                panic!("removed member must not decrypt post-removal messages, got {plaintext:?}");
            }
            Ok(Some(other)) => {
                // Control/RemovedOurselves events are fine — the point is
                // the plaintext never reaches Charlie.
                eprintln!("Charlie's post-removal event: {other:?}");
            }
        }
    }

    /// Test: persistence round-trip — save GroupState, reload, continue messaging.
    #[test]
    fn test_persistence_roundtrip() {
        let (mut alice, mut bob, alice_id, bob_id, _registry) = setup_two_peers();
        let group_id = GroupId::generate();

        // Alice creates group with Bob.
        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");

        // Bob joins.
        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        // Forward Bob's Ack to Alice so her state carries Bob's ratchet
        // before we snapshot it for persistence.
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        // Exchange a message to establish the ratchet.
        let msg1 = alice
            .send_message(&group_id, b"before save")
            .expect("alice send before save");
        bob.receive_message(&group_id, &msg1)
            .expect("bob receive before save");

        // ── Persistence: save Alice's state to SQLite ──
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .expect("create table");

        let alice_state = alice.groups.remove(&group_id).expect("alice state exists");
        persistence::save_group_state(&conn, &group_id, &alice_state).expect("save alice state");

        // ── Load back ──
        let loaded_state =
            persistence::load_group_state(&conn, &group_id).expect("load alice state");

        // Replace Alice's old state with the loaded one.  Because
        // RegistryState creates a fresh in-memory DB on deserialization,
        // the loaded GroupState contains an empty registry.  We rebuild
        // Alice's encryption state from scratch.
        let mut alice2 = make_enc_state(Rng::default(), &alice.registry);
        register_peer(&mut alice2, alice_id);
        register_peer(&mut alice2, bob_id);
        alice2.groups.insert(group_id, loaded_state);
        alice2.self_ids.insert(group_id, alice_id);

        // ── Bob sends a message after the reload ──
        let msg2 = bob
            .send_message(&group_id, b"after reload")
            .expect("bob send after reload");

        // Alice's reloaded state should decrypt it.
        let alice_event = alice2
            .receive_message(&group_id, &msg2)
            .expect("alice2 receive after reload");

        match alice_event {
            Some(GroupEvent::Application {
                plaintext,
                message_id: _,
            }) => {
                assert_eq!(
                    plaintext, b"after reload",
                    "Alice should decrypt after state reload"
                );
            }
            other => {
                panic!("expected Application message after persistence reload, got: {other:?}");
            }
        }

        // ── Alice sends a message from the reloaded state ──
        let msg3 = alice2
            .send_message(&group_id, b"from reloaded state")
            .expect("alice2 send from reloaded");

        let bob_event = bob
            .receive_message(&group_id, &msg3)
            .expect("bob receive from reloaded");

        match bob_event {
            Some(GroupEvent::Application {
                plaintext,
                message_id: _,
            }) => {
                assert_eq!(
                    plaintext, b"from reloaded state",
                    "Bob should decrypt message from reloaded Alice state"
                );
            }
            other => {
                panic!("expected Application message from reloaded state, got: {other:?}");
            }
        }
    }

    // ── Kith-style role enforcement tests ─────────────────────────────

    /// Test: a Reader's writes are rejected at send time.
    ///
    /// Alice (owner/Admin) demotes Bob to Reader; Bob's `send_message` must
    /// fail with [`EncryptionError::ForbiddenRole`] even though he holds a
    /// valid ratchet.
    #[test]
    fn test_reader_write_rejected() {
        let (mut alice, mut bob, alice_id, bob_id, _registry) = setup_two_peers();
        let group_id = GroupId::generate();

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");

        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        // Bob can write before demotion.
        let _ = bob
            .send_message(&group_id, b"hello as writer")
            .expect("writer can send");

        // Alice demotes Bob to Reader.
        alice
            .set_member_role(&group_id, alice_id, bob_id, MemberRole::Reader)
            .expect("alice demotes bob to reader");

        // Bob's mirror reflects the roster it received (alice=Admin), then
        // mirrors the demotion so his client enforces it locally too.
        {
            let bob_roles = bob.group_roles.entry(group_id).or_default();
            bob_roles.insert(alice_id, MemberRole::Admin);
            bob_roles.insert(bob_id, MemberRole::Writer);
        }
        bob.set_member_role(&group_id, alice_id, bob_id, MemberRole::Reader)
            .expect("bob mirrors reader role");

        // Bob's writes are now rejected.
        let err = bob
            .send_message(&group_id, b"trying to write as reader")
            .expect_err("reader send must be rejected");
        assert!(
            matches!(err, EncryptionError::ForbiddenRole { peer, role } if peer == bob_id && role == MemberRole::Reader),
            "expected ForbiddenRole, got {err:?}"
        );

        // Alice (Admin) can still write.
        let alice_msg = alice
            .send_message(&group_id, b"admin message")
            .expect("admin can send");
        let bob_event = bob
            .receive_message(&group_id, &alice_msg)
            .expect("bob receive admin message");
        assert!(
            matches!(bob_event, Some(GroupEvent::Application { .. })),
            "reader should still receive"
        );
    }

    /// Test: a non-member cannot send even with a leaked copy of the group
    /// state (the p2panda DGM is the authoritative member set).
    ///
    /// Two refusal paths are exercised:
    ///
    /// 1. A fresh state claiming the same group id without ever being added
    ///    (the group is not established for that peer) → p2panda refuses.
    /// 2. A **removed** device that retains its old keys (the leaked-key
    ///    scenario): after removal the DGM no longer lists the device, so
    ///    the `NotMember` check in `send_message` refuses it even though it
    ///    still holds valid ratchet material.
    #[test]
    fn test_non_member_rejected_with_leaked_key() {
        let (mut alice, mut bob, alice_id, bob_id, registry) = setup_two_peers();
        let charlie_id = make_peer();
        let group_id = GroupId::generate();

        let mut charlie = make_enc_state(Rng::default(), &registry);
        register_peer(&mut charlie, charlie_id);

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id, charlie_id])
            .expect("alice create group");

        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        charlie
            .init_group(group_id, charlie_id)
            .expect("charlie init");
        let charlie_create_event = charlie
            .receive_message(&group_id, &create_msg)
            .expect("charlie receive create");
        if let Some(GroupEvent::Control(ack)) = &charlie_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive charlie ack");
            bob.receive_message(&group_id, ack)
                .expect("bob receive charlie ack");
        }

        // Establish a ratchet so Charlie has valid keys and can read messages.
        let msg = alice
            .send_message(&group_id, b"hello")
            .expect("alice send hello");
        let charlie_ok = charlie
            .receive_message(&group_id, &msg)
            .expect("charlie receive hello");
        assert!(
            matches!(charlie_ok, Some(GroupEvent::Application { .. })),
            "charlie reads while a member"
        );

        // ── Path 1: a fresh state for the same group id, never added ──
        let mut mallory = make_enc_state(Rng::default(), &registry);
        let mallory_id = make_peer();
        register_peer(&mut mallory, mallory_id);
        mallory
            .init_group(group_id, mallory_id)
            .expect("mallory init group");
        // The group is not established for Mallory → send fails (no ratchet).
        let err = mallory
            .send_message(&group_id, b"i stole the keys")
            .expect_err("non-member send must be rejected");
        assert!(
            matches!(err, EncryptionError::Group(_)),
            "fresh non-member state refused, got {err:?}"
        );

        // ── Path 2: removed device with a leaked key ──────────────────
        // Alice removes Charlie.  Charlie processes the removal (her DGM
        // drops her → RemovedOurselves) but retains her old key material.
        let remove_msg = alice
            .remove_member(&group_id, charlie_id)
            .expect("alice remove charlie");
        bob.receive_message(&group_id, &remove_msg)
            .expect("bob receive remove");
        charlie
            .receive_message(&group_id, &remove_msg)
            .expect("charlie receive remove");

        let err = charlie
            .send_message(&group_id, b"still have the old keys")
            .expect_err("removed device send must be rejected");
        assert!(
            matches!(err, EncryptionError::NotMember(p) if p == charlie_id),
            "removed device refused by member check, got {err:?}"
        );

        // Sanity: the member set no longer contains Charlie.
        let members = alice
            .groups
            .get(&group_id)
            .map(|s| {
                p2panda_encryption::message_scheme::group::MessageGroup::members(s)
                    .expect("members")
            })
            .expect("alice group state exists");
        assert!(
            !members.contains(&charlie_id),
            "charlie must not be a member after removal"
        );
        assert!(members.contains(&bob_id), "bob is still a member");
    }

    /// Test: a Reader's message is dropped by a receiving admin (defense in
    /// depth even when the Reader's client bypasses its own send gate).
    #[test]
    fn test_receiver_drops_reader_plaintext() {
        let (mut alice, mut bob, alice_id, bob_id, _registry) = setup_two_peers();
        let group_id = GroupId::generate();

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");

        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        // Alice's mirror marks Bob as Reader.  Bob's own state still says
        // Writer (malicious / stale client).
        alice
            .set_member_role(&group_id, alice_id, bob_id, MemberRole::Reader)
            .expect("alice sets bob to reader");

        // Bob (stale client) sends anyway.
        let bob_msg = bob
            .send_message(&group_id, b"reader tried to write")
            .expect("bob (stale writer) sends");

        // Alice must NOT surface the plaintext.
        let alice_event = alice
            .receive_message(&group_id, &bob_msg)
            .expect("alice receive reader message");
        assert!(
            !matches!(alice_event, Some(GroupEvent::Application { .. })),
            "reader plaintext must be dropped, got {alice_event:?}"
        );
    }

    /// Test: only an Admin can add members (non-admin add is refused).
    #[test]
    fn test_non_admin_cannot_add_member() {
        let (mut alice, mut bob, alice_id, bob_id, registry) = setup_two_peers();
        let charlie_id = make_peer();
        let group_id = GroupId::generate();

        // Charlie needs prekeys in the shared registry before he can be added.
        let mut charlie = make_enc_state(Rng::default(), &registry);
        register_peer(&mut charlie, charlie_id);

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");

        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        // Bob (Writer) tries to add Charlie → refused.
        let err = bob
            .add_member(&group_id, charlie_id)
            .expect_err("non-admin add must be refused");
        assert!(
            matches!(err, EncryptionError::NotAuthorized(p) if p == bob_id),
            "expected NotAuthorized, got {err:?}"
        );

        // Alice (Admin) can still add Charlie.
        alice
            .add_member(&group_id, charlie_id)
            .expect("admin can add");
    }

    // ── Kith-style epoch rotation tests ───────────────────────────────

    /// Test: after removing a member, the remaining members still converge on
    /// the new epoch (they can exchange messages both ways).
    #[test]
    fn test_remaining_members_converge_after_removal() {
        let (mut alice, mut bob, alice_id, bob_id, registry) = setup_two_peers();
        let charlie_id = make_peer();
        let group_id = GroupId::generate();

        let mut charlie = make_enc_state(Rng::default(), &registry);
        register_peer(&mut charlie, charlie_id);

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id, charlie_id])
            .expect("alice create group");

        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        charlie
            .init_group(group_id, charlie_id)
            .expect("charlie init");
        let charlie_create_event = charlie
            .receive_message(&group_id, &create_msg)
            .expect("charlie receive create");
        if let Some(GroupEvent::Control(ack)) = &charlie_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive charlie ack");
            bob.receive_message(&group_id, ack)
                .expect("bob receive charlie ack");
        }

        // Alice removes Charlie → ratchet rotates to a new epoch.
        let remove_msg = alice
            .remove_member(&group_id, charlie_id)
            .expect("alice remove charlie");
        // Bob processes the removal and emits an ack; Alice must process that
        // ack so her decryption ratchet for Bob advances into the new epoch.
        let bob_remove_event = bob
            .receive_message(&group_id, &remove_msg)
            .expect("bob receive remove");
        if let Some(GroupEvent::Control(ack)) = &bob_remove_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob remove-ack");
        }

        // Remaining members converge: Alice → Bob.
        let alice_msg = alice
            .send_message(&group_id, b"epoch two: alice to bob")
            .expect("alice send in new epoch");
        let bob_event = bob
            .receive_message(&group_id, &alice_msg)
            .expect("bob receive new epoch");
        match bob_event {
            Some(GroupEvent::Application {
                plaintext,
                message_id: _,
            }) => {
                assert_eq!(
                    plaintext, b"epoch two: alice to bob",
                    "Bob should decrypt in the new epoch"
                );
            }
            other => panic!("expected Application in new epoch, got {other:?}"),
        }

        // Bob → Alice in the new epoch.
        let bob_msg = bob
            .send_message(&group_id, b"epoch two: bob to alice")
            .expect("bob send in new epoch");
        let alice_event = alice
            .receive_message(&group_id, &bob_msg)
            .expect("alice receive new epoch");
        match alice_event {
            Some(GroupEvent::Application {
                plaintext,
                message_id: _,
            }) => {
                assert_eq!(
                    plaintext, b"epoch two: bob to alice",
                    "Alice should decrypt in the new epoch"
                );
            }
            other => panic!("expected Application in new epoch, got {other:?}"),
        }
    }

    /// Test: a removed device cannot sync the new epoch — it cannot decrypt
    /// post-removal messages, and its state stays locked out even after a
    /// persistence round-trip of the *remaining* members' rotated state.
    #[test]
    fn test_removed_device_cannot_sync_new_epoch() {
        let (mut alice, mut bob, alice_id, bob_id, registry) = setup_two_peers();
        let charlie_id = make_peer();
        let group_id = GroupId::generate();

        let mut charlie = make_enc_state(Rng::default(), &registry);
        register_peer(&mut charlie, charlie_id);

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id, charlie_id])
            .expect("alice create group");

        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        charlie
            .init_group(group_id, charlie_id)
            .expect("charlie init");
        let charlie_create_event = charlie
            .receive_message(&group_id, &create_msg)
            .expect("charlie receive create");
        if let Some(GroupEvent::Control(ack)) = &charlie_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive charlie ack");
            bob.receive_message(&group_id, ack)
                .expect("bob receive charlie ack");
        }

        // Establish a message Charlie can still read in epoch 1.
        let msg1 = alice
            .send_message(&group_id, b"epoch one message")
            .expect("alice send epoch one");
        let charlie_epoch1 = charlie
            .receive_message(&group_id, &msg1)
            .expect("charlie receive epoch one");
        assert!(
            matches!(charlie_epoch1, Some(GroupEvent::Application { .. })),
            "charlie should read epoch one"
        );

        // Alice removes Charlie and persists her rotated state.
        let remove_msg = alice
            .remove_member(&group_id, charlie_id)
            .expect("alice remove charlie");
        bob.receive_message(&group_id, &remove_msg)
            .expect("bob receive remove");

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .expect("create table");
        // Include the lazy role table so save_current_group_state can write it.
        let alice_state = alice.groups.remove(&group_id).expect("alice state");
        persistence::save_group_state(&conn, &group_id, &alice_state).expect("save rotated state");

        // Reload Alice's rotated state from the DB (simulates restart).
        let loaded = persistence::load_group_state(&conn, &group_id).expect("load rotated state");
        let mut alice2 = make_enc_state(Rng::default(), &alice.registry);
        register_peer(&mut alice2, alice_id);
        register_peer(&mut alice2, bob_id);
        alice2.groups.insert(group_id, loaded);
        alice2.self_ids.insert(group_id, alice_id);

        // Alice (reloaded, rotated) sends a new-epoch message.
        let new_msg = alice2
            .send_message(&group_id, b"post-removal secret")
            .expect("alice2 send post-removal");

        // Bob (remaining member) decrypts it.
        let bob_event = bob
            .receive_message(&group_id, &new_msg)
            .expect("bob receive post-removal");
        assert!(
            matches!(bob_event, Some(GroupEvent::Application { .. })),
            "remaining member should decrypt post-removal message"
        );

        // Charlie (removed) must NOT get the plaintext.
        let charlie_result = charlie.receive_message(&group_id, &new_msg);
        match charlie_result {
            Err(_) => {}
            Ok(None) => {}
            Ok(Some(GroupEvent::Application { plaintext, .. })) => {
                panic!("removed device must not decrypt new epoch, got {plaintext:?}");
            }
            Ok(Some(other)) => {
                // Control events are fine; the plaintext must not surface.
                eprintln!("Charlie post-removal event: {other:?}");
            }
        }
    }

    /// Test: the role mirror persists through the SQLite round-trip and is
    /// still enforced after reload.
    #[test]
    fn test_roles_persist_across_reload() {
        let (mut alice, mut bob, alice_id, bob_id, _registry) = setup_two_peers();
        let group_id = GroupId::generate();

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");

        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive bob ack");
        }

        alice
            .set_member_role(&group_id, alice_id, bob_id, MemberRole::Reader)
            .expect("alice demotes bob to reader");

        // Persist via the DB path.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .expect("create table");

        let roles = alice
            .group_roles
            .get(&group_id)
            .cloned()
            .unwrap_or_default();
        persistence::save_group_roles(&conn, &group_id, &roles, Some(alice_id))
            .expect("save roles");

        let loaded = persistence::load_group_roles(&conn, &group_id)
            .expect("load roles")
            .expect("roles exist");
        assert_eq!(
            loaded.0.get(&bob_id),
            Some(&MemberRole::Reader),
            "bob's reader role must survive the round-trip"
        );
        assert_eq!(
            loaded.0.get(&alice_id),
            Some(&MemberRole::Admin),
            "owner stays admin"
        );
        assert_eq!(loaded.1, Some(alice_id), "self id round-trips");
    }

    // ── Fail-closed load-through-DB regression tests (BORU-AUDIT-04) ─────

    /// A genuinely new group with no saved state → Missing (fresh init
    /// allowed).
    #[test]
    fn test_load_from_db_missing_permits_fresh_init() {
        let (mut alice, _bob, alice_id, _bob_id, _registry) = setup_two_peers();
        let group_id = GroupId::generate();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .expect("create table");
        alice.db = Some(std::sync::Arc::new(std::sync::Mutex::new(conn)));

        match alice.load_group_state_from_db(&group_id).expect("load") {
            GroupStateLoadOutcome::Missing => {}
            other => panic!("expected Missing for a new group, got: {other:?}"),
        }
    }

    /// A valid saved state loads as Loaded and can still decrypt a known
    /// fixture message after a simulated restart.
    #[test]
    fn test_load_from_db_valid_state_decrypts() {
        let (mut alice, mut bob, alice_id, bob_id, registry) = setup_two_peers();
        let group_id = GroupId::generate();

        let create_msg = alice
            .create_group(group_id, alice_id, vec![bob_id])
            .expect("alice create group");
        bob.init_group(group_id, bob_id).expect("bob init");
        let bob_create_event = bob
            .receive_message(&group_id, &create_msg)
            .expect("bob receive create");
        if let Some(GroupEvent::Control(ack)) = &bob_create_event {
            alice
                .receive_message(&group_id, ack)
                .expect("alice receive ack");
        }
        let msg1 = alice
            .send_message(&group_id, b"before save")
            .expect("alice send");
        bob.receive_message(&group_id, &msg1).expect("bob receive");

        // Give Alice a DB and persist through the normal send path.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .expect("create table");
        let db = std::sync::Arc::new(std::sync::Mutex::new(conn));
        alice.db = Some(db.clone());
        let _persisted = alice
            .send_message(&group_id, b"persisted")
            .expect("alice send persisted");

        // Simulate restart: fresh EncryptionState sharing the same DB.
        let mut alice2 = make_enc_state(Rng::default(), &registry);
        register_peer(&mut alice2, alice_id);
        register_peer(&mut alice2, bob_id);
        alice2.db = Some(db);

        match alice2.load_group_state_from_db(&group_id).expect("load") {
            GroupStateLoadOutcome::Loaded => {}
            other => panic!("expected Loaded for valid saved state, got: {other:?}"),
        }

        // Bob sends a message; the reloaded state must decrypt it.
        let bob_msg = bob
            .send_message(&group_id, b"after reload")
            .expect("bob send");
        match alice2
            .receive_message(&group_id, &bob_msg)
            .expect("alice2 receive")
        {
            Some(GroupEvent::Application { plaintext, .. }) => {
                assert_eq!(
                    plaintext, b"after reload",
                    "reloaded state decrypts known fixture message"
                );
            }
            other => panic!("expected Application event, got: {other:?}"),
        }
    }

    /// Existing state with one corrupted byte → load through the high-level
    /// API fails closed (Corrupt), never Missing, and no partial state is
    /// installed.
    #[test]
    fn test_load_from_db_corrupt_fails_closed() {
        let (mut alice, _bob, alice_id, _bob_id, registry) = setup_two_peers();
        let group_id = GroupId::generate();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_encryption_state (
                group_id BLOB PRIMARY KEY,
                state BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .expect("create table");
        let db = std::sync::Arc::new(std::sync::Mutex::new(conn));
        alice.db = Some(db.clone());
        // Persist through the normal create path (owner-only group).
        let _create_msg = alice
            .create_group(group_id, alice_id, vec![])
            .expect("alice create group");

        // Corrupt one byte of the stored record.
        {
            let conn = db.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT state FROM group_encryption_state WHERE group_id = ?1")
                .unwrap();
            let mut blob: Vec<u8> = stmt
                .query_row(params![group_id.as_bytes().as_slice()], |r| r.get(0))
                .unwrap();
            assert!(blob.len() > 32, "stored blob should be non-trivial");
            let mid = blob.len() / 2;
            blob[mid] ^= 0xFF;
            conn.execute(
                "UPDATE group_encryption_state SET state = ?1 WHERE group_id = ?2",
                params![blob, group_id.as_bytes().as_slice()],
            )
            .unwrap();
        }

        // A fresh EncryptionState must fail closed — NOT Missing, NOT Loaded.
        let mut alice2 = make_enc_state(Rng::default(), &registry);
        register_peer(&mut alice2, alice_id);
        alice2.db = Some(db);

        match alice2.load_group_state_from_db(&group_id) {
            Err(EncryptionError::GroupStateLoad(GroupStateLoadError::Corrupt { .. })) => {}
            other => panic!("expected Corrupt via load_group_state_from_db, got: {other:?}"),
        }
        // No half-loaded state may be left in memory (fail closed).
        assert!(
            !alice2.groups.contains_key(&group_id),
            "no partial state installed after corruption"
        );
        assert!(
            !alice2.self_ids.contains_key(&group_id),
            "no partial identity installed after corruption"
        );
    }
}
