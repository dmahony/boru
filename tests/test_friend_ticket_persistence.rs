//! Integration coverage for peer-ticket callbacks and the frontend save path.

use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{EndpointAddr, PublicKey, SecretKey};
use iroh_gossip::{
    chat_callbacks::ChatCallbacks,
    chat_core::{handle_net_event, Message, MessageHash, NetEvent, Ticket},
    friends::{FriendId, FriendsStore},
    proto::TopicId,
};

struct TestFrontend {
    local_public: PublicKey,
    friends: FriendsStore,
    friends_dirty: bool,
}

impl TestFrontend {
    fn save_if_dirty(&mut self) {
        if self.friends_dirty {
            self.friends.save().expect("save friends store");
            self.friends_dirty = false;
        }
    }
}

impl ChatCallbacks for TestFrontend {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }

    fn set_name(&mut self, _peer: PublicKey, _name: String) {}

    fn is_friend(&self, peer: &PublicKey) -> bool {
        self.friends
            .get(&FriendId::from_public_key(*peer))
            .is_some()
    }

    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {
        self.friends_dirty = true;
    }
    fn push_system(&mut self, _text: String) {}
    fn push_remote(
        &mut self,
        _label: String,
        _text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
    }
    fn set_pending_file(&mut self, _name: String, _ticket: String) {}
    fn set_pending_image(&mut self, _name: String, _hash: MessageHash, _from: PublicKey) {}
    fn has_message(&self, _hash: &MessageHash) -> bool {
        false
    }
    fn edit_message(&mut self, _hash: &MessageHash, _new_text: String) {}
    fn delete_message(&mut self, _hash: &MessageHash) {}
    fn add_reaction(&mut self, _hash: &MessageHash, _emoji: String) {}
    fn on_neighbor_up(&mut self, _peer: PublicKey) {}
    fn on_neighbor_down(&mut self, _peer: PublicKey) {}
    fn record_activity(&mut self, _peer: PublicKey) {}
    fn record_presence(&mut self, _peer: PublicKey) {}

    fn record_peer_ticket(&mut self, peer: PublicKey, ticket: String) {
        let Ok(ticket) = ticket.parse::<Ticket>() else {
            return;
        };
        let record = self.friends.ensure_friend(FriendId::from_public_key(peer));
        record.record_addrs(ticket.peers.clone());
        record.record_room(ticket.topic, ticket);
        self.friends_dirty = true;
    }

    fn request_quit(&mut self) {}
}

#[test]
fn peer_ticket_survives_callback_and_debounced_frontend_save() {
    let tempdir = tempfile::tempdir().expect("create temporary data directory");
    let local = SecretKey::generate().public();
    let peer = SecretKey::generate().public();
    let topic = TopicId::from_bytes([0x42; 32]);
    let ticket = Ticket {
        topic,
        peers: vec![EndpointAddr::new(peer)],
    };
    let ticket_text = ticket.to_string();

    let mut frontend = TestFrontend {
        local_public: local,
        friends: FriendsStore::empty_at(tempdir.path()),
        friends_dirty: false,
    };
    let sent_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_secs();

    handle_net_event(
        NetEvent::Message {
            from: peer,
            message: Message::PresenceWithTicket {
                ticket: ticket_text,
            },
            sent_at,
        },
        &mut frontend,
    )
    .expect("process peer ticket callback");

    assert!(frontend.friends_dirty, "callback must schedule a save");
    frontend.save_if_dirty();
    assert!(!frontend.friends_dirty);

    let reloaded = FriendsStore::load(tempdir.path()).expect("reload friends store");
    let record = reloaded
        .get(&FriendId::from_public_key(peer))
        .expect("peer record persisted");
    assert_eq!(record.rooms.get(&topic), Some(&ticket));
    assert_eq!(record.known_addrs, ticket.peers);
}
