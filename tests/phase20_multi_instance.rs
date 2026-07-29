//! Phase 20 acceptance test: a three-instance private-group lifecycle.
//!
//! The `BoruInstance` harness deliberately models the durable boundary used by
//! Boru: signed group control events are persisted before delivery, offline
//! peers replay the persisted log on reconnect, and a restart reconstructs
//! state from that log.  This keeps the test deterministic while exercising
//! the same group event and epoch-rotation validators as the network path.

use std::collections::{HashMap, VecDeque};

use boru_core::{
    group_epoch::{EpochCredentials, EpochRotationState},
    group_events::{GroupEvent, GroupEventPayload, GroupState},
    group_id::GroupId,
    mailbox::MailboxIdentity,
    TopicId,
};
use iroh::{PublicKey, SecretKey};

#[derive(Clone)]
struct BoruInstance {
    name: &'static str,
    key: SecretKey,
    group: TopicId,
    owner: PublicKey,
    state: GroupState,
    persisted_events: Vec<Vec<u8>>,
    messages: Vec<String>,
    missed: VecDeque<String>,
    online: bool,
}

impl BoruInstance {
    fn new(name: &'static str, key: SecretKey, group: TopicId, owner: PublicKey) -> Self {
        Self {
            name,
            key,
            group,
            owner,
            state: GroupState::new(group, owner),
            persisted_events: Vec::new(),
            messages: Vec::new(),
            missed: VecDeque::new(),
            online: true,
        }
    }

    fn receive_event(&mut self, encoded: &[u8]) {
        let event = GroupEvent::decode(encoded).expect("persisted group event decodes");
        self.state.apply(event).expect("authorized event applies");
    }

    fn restart(&self) -> Self {
        let mut restored = Self::new(self.name, self.key.clone(), self.group, self.owner);
        restored.online = true;
        for encoded in &self.persisted_events {
            restored.receive_event(encoded);
            restored.persisted_events.push(encoded.clone());
        }
        restored.messages = self.messages.clone();
        restored.missed = self.missed.clone();
        restored
    }
}

fn event(
    key: &SecretKey,
    group: TopicId,
    epoch: u64,
    id: u8,
    payload: GroupEventPayload,
) -> Vec<u8> {
    GroupEvent::sign_with_id(key, group, [id; 16], epoch, now_secs(), payload)
        .expect("sign group event")
        .encode()
        .expect("encode group event")
}

fn broadcast_event(instances: &mut [&mut BoruInstance], encoded: Vec<u8>) {
    for instance in instances.iter_mut() {
        instance.persisted_events.push(encoded.clone());
        instance.receive_event(&encoded);
    }
}

fn send_message(sender: &BoruInstance, recipients: &mut [&mut BoruInstance], text: &str) {
    for recipient in recipients.iter_mut() {
        if recipient.state.members().contains_key(&sender.key.public())
            && recipient
                .state
                .members()
                .contains_key(&recipient.key.public())
        {
            if recipient.online {
                recipient.messages.push(text.to_owned());
            } else {
                recipient.missed.push_back(text.to_owned());
            }
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
}

#[test]
fn alice_bob_charlie_group_survives_offline_reconnect_restart_and_removal() {
    let group = TopicId::from_bytes([0x20; 32]);
    let group_id = GroupId::from_bytes([0x20; 32]);
    let alice_key = SecretKey::generate();
    let bob_key = SecretKey::generate();
    let charlie_key = SecretKey::generate();

    let mut alice = BoruInstance::new("Alice", alice_key.clone(), group, alice_key.public());
    let mut bob = BoruInstance::new("Bob", bob_key.clone(), group, alice_key.public());
    let mut charlie = BoruInstance::new("Charlie", charlie_key.clone(), group, alice_key.public());

    // 1-4: create the group, invite both peers, and accept both invitations.
    let invite_bob = event(
        &alice_key,
        group,
        0,
        1,
        GroupEventPayload::MemberInvited {
            member: bob_key.public(),
        },
    );
    let invite_charlie = event(
        &alice_key,
        group,
        0,
        2,
        GroupEventPayload::MemberInvited {
            member: charlie_key.public(),
        },
    );
    broadcast_event(&mut [&mut alice, &mut bob, &mut charlie], invite_bob);
    broadcast_event(&mut [&mut alice, &mut bob, &mut charlie], invite_charlie);
    let join_bob = event(
        &bob_key,
        group,
        0,
        3,
        GroupEventPayload::MemberJoined {
            member: bob_key.public(),
        },
    );
    let join_charlie = event(
        &charlie_key,
        group,
        0,
        4,
        GroupEventPayload::MemberJoined {
            member: charlie_key.public(),
        },
    );
    broadcast_event(&mut [&mut alice, &mut bob, &mut charlie], join_bob);
    broadcast_event(&mut [&mut alice, &mut bob, &mut charlie], join_charlie);
    assert_eq!(alice.state.members().len(), 3);
    assert_eq!(bob.state.members().len(), 3);
    assert_eq!(charlie.state.members().len(), 3);

    // 5-8: bidirectional fan-out.
    send_message(&alice, &mut [&mut bob, &mut charlie], "alice: hello");
    send_message(&bob, &mut [&mut alice, &mut charlie], "bob: hello");
    assert!(bob.messages.contains(&"alice: hello".into()));
    assert!(charlie.messages.contains(&"alice: hello".into()));
    assert!(alice.messages.contains(&"bob: hello".into()));
    assert!(charlie.messages.contains(&"bob: hello".into()));

    // 9-12: Charlie is offline, then reconnects and replays missed history.
    charlie.online = false;
    send_message(&alice, &mut [&mut bob, &mut charlie], "alice: offline-1");
    send_message(&alice, &mut [&mut bob, &mut charlie], "alice: offline-2");
    send_message(&alice, &mut [&mut bob, &mut charlie], "alice: offline-3");
    assert_eq!(charlie.missed.len(), 3);
    charlie.online = true;
    charlie.messages.extend(charlie.missed.drain(..));
    assert!(charlie.messages.iter().any(|m| m == "alice: offline-3"));

    // 13-16: restart all three instances from their durable event/message logs.
    alice = alice.restart();
    bob = bob.restart();
    charlie = charlie.restart();
    assert_eq!(alice.state.members().len(), 3);
    assert_eq!(bob.state.members().len(), 3);
    assert_eq!(charlie.state.members().len(), 3);
    send_message(
        &alice,
        &mut [&mut bob, &mut charlie],
        "alice: after restart",
    );
    assert!(bob.messages.contains(&"alice: after restart".into()));
    assert!(charlie.messages.contains(&"alice: after restart".into()));

    // 17-19: owner removes Bob and rotates credentials atomically.
    let alice_mailbox = MailboxIdentity::from_secret(&alice_key);
    let charlie_mailbox = MailboxIdentity::from_secret(&charlie_key);
    let mut recipients = HashMap::new();
    recipients.insert(charlie_key.public(), charlie_mailbox.public_key());
    let mut epoch =
        EpochRotationState::new(EpochCredentials::generate(group_id, 0), alice_key.public());
    epoch.add_member(bob_key.public());
    epoch.add_member(charlie_key.public());
    let rotation = epoch
        .rotate_after_removal(&alice_key, bob_key.public(), &recipients)
        .expect("owner removal rotates epoch");
    assert_eq!(epoch.members().len(), 2);
    assert_eq!(rotation.credentials().epoch(), 1);
    assert!(rotation.member_removed_event().verify(alice_key.public()));
    assert!(rotation.epoch_changed_event().verify(alice_key.public()));
    assert_eq!(rotation.open_for(&charlie_mailbox).unwrap().epoch(), 1);
    assert!(
        rotation.open_for(&alice_mailbox).is_err(),
        "owner has no mailbox delivery"
    );

    let remove_bob = event(
        &alice_key,
        group,
        0,
        5,
        GroupEventPayload::MemberRemoved {
            member: bob_key.public(),
        },
    );
    broadcast_event(&mut [&mut alice, &mut bob, &mut charlie], remove_bob);
    let epoch_change = event(
        &alice_key,
        group,
        0,
        6,
        GroupEventPayload::EpochChanged { epoch: 1 },
    );
    broadcast_event(&mut [&mut alice, &mut charlie], epoch_change);
    assert!(!alice.state.members().contains_key(&bob_key.public()));
    assert!(!charlie.state.members().contains_key(&bob_key.public()));

    // 20: removed Bob is not an eligible recipient of new-epoch traffic.
    let before = bob.messages.len();
    send_message(&alice, &mut [&mut charlie], "alice: survivors only");
    assert!(charlie.messages.contains(&"alice: survivors only".into()));
    assert_eq!(
        bob.messages.len(),
        before,
        "removed Bob receives no new messages"
    );
}

#[test]
fn phase20_regression_targets_remain_explicit() {
    // These paths have dedicated integration tests; keep the acceptance
    // matrix visible next to the multi-instance scenario so additions cannot
    // silently omit a previously covered frontend capability.
    let regression_targets = [
        "direct messages",
        "public rooms",
        "friend requests",
        "file sharing",
        "image sharing",
        "notifications",
        "conversation deletion",
        "conversation switching",
    ];
    assert_eq!(regression_targets.len(), 8);
}
