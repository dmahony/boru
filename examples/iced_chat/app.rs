//! The iced Application for the gossip chat frontend.
//!
//! Supports a chat-list (inbox) screen and individual chat-room screens,
//! with dynamic room switching — like Telegram/Signal.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use iroh::{EndpointAddr, PublicKey, RelayMode, SecretKey};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket};
use iroh_gossip::api::GossipSender;
use iroh_gossip::chat_core::friend_ping::{
    FriendEvent, FriendPingManager, FriendStatus,
};
use iroh_gossip::friends::{FriendId, FriendsStore};
use iroh_gossip::net::Gossip;
use iroh_gossip::proto::TopicId;
use iroh_gossip::room_history::{RoomHistoryEntry, RoomHistoryStore};
use n0_future::Stream;
use n0_future::task;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

use crate::{fmt_relay_mode, forward_gossip_events, Message, NetEvent, SignedMessage, Ticket};

// ── Chat entry types ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum ChatKind {
    System,
    Local,
    Remote,
}

#[derive(Clone, Debug)]
struct ChatEntry {
    kind: ChatKind,
    label: String,
    body: String,
}

impl ChatEntry {
    fn system(text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::System,
            label: "System".into(),
            body: text.into(),
        }
    }
    fn local(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Local,
            label: label.into(),
            body: text.into(),
        }
    }
    fn remote(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ChatKind::Remote,
            label: label.into(),
            body: text.into(),
        }
    }
}

// ── Screen navigation ─────────────────────────────────────────────────

/// The active screen in the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The chat-list / inbox showing recent rooms.
    ChatList,
    /// An individual chat room with a given topic.
    Chat { topic: TopicId },
}

// ── Application state ─────────────────────────────────────────────────

pub struct IcedChat {
    // ── Navigation ──
    screen: Screen,
    /// Pending topic we're connecting to (used during the async handoff
    /// from clicking a room to actually subscribing).
    pending_topic: Option<TopicId>,

    // ── ChatList state ──
    room_history: RoomHistoryStore,
    room_history_dirty: bool,
    /// Text input for the "Join via ticket" field in the chat list.
    join_ticket_input: String,
    /// Optional error message shown in the chat list.
    chat_list_error: String,

    // ── Chat state (active room) ──
    entries: Vec<ChatEntry>,
    composer_text: String,
    help_visible: bool,
    pending_file: Option<(String, String)>,
    names: HashMap<PublicKey, String>,
    topic: TopicId,
    ticket_str: String,

    // ── Shared network state ──
    secret_key: SecretKey,
    gossip: Gossip,
    sender: Option<GossipSender>,
    blob_store: MemStore,
    endpoint: iroh::Endpoint,
    local_label: String,
    local_public: PublicKey,
    relay_mode: RelayMode,
    pub net_rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
    net_tx: UnboundedSender<NetEvent>,
    /// JoinHandle to abort the current forward_gossip_events task when
    /// switching rooms.
    forward_handle: Option<task::JoinHandle<()>>,
    friends: FriendsStore,
    friends_dirty: bool,
    friend_mgr: FriendPingManager,
    pub friend_events_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
}

#[derive(Debug, Clone)]
pub enum AppMessage {
    // ── Navigation ──
    /// Open the chat list screen (go back from a chat).
    GoToChatList,
    /// Open a specific room.
    OpenRoom(TopicId),
    /// A new room was created and we're now connected to it.
    RoomOpened {
        topic: TopicId,
        ticket: String,
        sender: GossipSender,
    },
    /// Finished creating a new room (random topic).
    CreateNewRoom,
    /// Join a room from a ticket string.
    JoinFromTicket,
    /// The room switch / join failed.
    RoomJoinFailed(String),

    // ── ChatList ──
    JoinTicketInputChanged(String),
    NewChatCreated,
    RoomSelected(TopicId),

    // ── Chat ──
    InputChanged(String),
    SendPressed,
    ToggleHelp,
    NetEvent(NetEvent),
    FriendEvent(FriendEvent),
    MessageSent(String),
    FileSent(String),
    DownloadDone(String),
    ErrorMsg(String),
    ExecuteFileSend(String),
    ExecuteDownload,
    FriendAdded { fid: String, label: String, was_new: bool },
    FriendRemoved { label: String },
    FriendListResult(Vec<(String, String)>),
}

impl IcedChat {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        secret_key: SecretKey,
        gossip: Gossip,
        blob_store: MemStore,
        endpoint: iroh::Endpoint,
        local_label: String,
        local_public: PublicKey,
        relay_mode: RelayMode,
        net_rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
        net_tx: UnboundedSender<NetEvent>,
        room_history: RoomHistoryStore,
        friends: FriendsStore,
        friend_mgr: FriendPingManager,
        friend_events_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
        initial_topic: Option<TopicId>,
    ) -> Self {
        Self {
            screen: Screen::ChatList,
            pending_topic: None,
            room_history,
            room_history_dirty: false,
            join_ticket_input: String::new(),
            chat_list_error: String::new(),
            entries: Vec::new(),
            composer_text: String::new(),
            help_visible: false,
            pending_file: None,
            names: HashMap::new(),
            topic: initial_topic.unwrap_or(TopicId::from_bytes([0u8; 32])),
            ticket_str: String::new(),
            secret_key,
            gossip,
            sender: None,
            blob_store,
            endpoint,
            local_label,
            local_public,
            relay_mode,
            net_rx,
            net_tx,
            forward_handle: None,
            friends,
            friends_dirty: false,
            friend_mgr,
            friend_events_rx,
        }
    }

    fn push_system(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry::system(text));
    }
    fn push_local(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry::local(&self.local_label, text));
    }
    fn push_remote(&mut self, label: impl Into<String>, text: impl Into<String>) {
        self.entries.push(ChatEntry::remote(label, text));
    }
}

// ── Room switching helpers ───────────────────────────────────────────

impl IcedChat {
    fn leave_current_room(&mut self) {
        // Abort the forwarding task
        if let Some(handle) = self.forward_handle.take() {
            handle.abort();
        }
        self.sender = None;
        self.entries.clear();
        self.names.clear();
        self.pending_file = None;
    }

    /// Save the current room to history.
    fn save_room_to_history(&mut self) {
        let topic = self.topic;
        let name = self
            .names
            .get(&self.local_public)
            .cloned()
            .unwrap_or_default();
        let preview = self
            .entries
            .last()
            .map(|e| {
                let t = e.body.clone();
                if t.len() > 60 {
                    format!("{}…", &t[..60])
                } else {
                    t
                }
            })
            .unwrap_or_default();

        self.room_history.upsert(topic, &name, true);
        if !preview.is_empty() {
            self.room_history.update_preview(&topic, &preview);
        }
        self.room_history_dirty = true;
    }
}

// ── Update ────────────────────────────────────────────────────────────

impl IcedChat {
    pub fn update(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            // ── Navigation ────────────────────────────────────────────
            AppMessage::GoToChatList => {
                // Save current room to history
                self.save_room_to_history();
                self.persist_room_history();

                // Leave the current room
                self.leave_current_room();
                self.screen = Screen::ChatList;
                iced::Task::none()
            }

            AppMessage::CreateNewRoom => {
                let topic = TopicId::from_bytes(rand::random());
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let ep_id = self.endpoint.id();

                iced::Task::perform(
                    async move {
                        // Subscribe to the new topic
                        let sub = gossip.subscribe(topic, vec![]).await.map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let ticket = Ticket {
                            topic,
                            peers: vec![EndpointAddr::new(ep_id)],
                        };
                        let ticket_str = ticket.to_string();

                        // Spawn forwarding
                        let _ = task::spawn(forward_gossip_events(receiver, net_tx));

                        // Broadcast our presence
                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe { name: label },
                        ).map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;

                        Ok::<(GossipSender, TopicId, String), String>((sender, topic, ticket_str))
                    },
                    |result| match result {
                        Ok((sender, topic, ticket_str)) => {
                            AppMessage::RoomOpened {
                                topic,
                                ticket: ticket_str,
                                sender,
                            }
                        }
                        Err(e) => AppMessage::RoomJoinFailed(e),
                    },
                )
            }

            AppMessage::OpenRoom(topic) => {
                // Save the current room first
                self.save_room_to_history();
                self.leave_current_room();

                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let ep_id = self.endpoint.id();

                iced::Task::perform(
                    async move {
                        let sub = gossip.subscribe(topic, vec![]).await.map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let ticket = Ticket {
                            topic,
                            peers: vec![EndpointAddr::new(ep_id)],
                        };
                        let ticket_str = ticket.to_string();

                        let _ = task::spawn(forward_gossip_events(receiver, net_tx));

                        // Broadcast our presence
                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe { name: label },
                        ).map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;

                        Ok::<(GossipSender, TopicId, String), String>((sender, topic, ticket_str))
                    },
                    |result| match result {
                        Ok((sender, topic, ticket_str)) => AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                            sender,
                        },
                        Err(e) => AppMessage::RoomJoinFailed(e),
                    },
                )
            }

            AppMessage::RoomOpened { topic, ticket, sender } => {
                self.pending_topic = None;
                self.sender = Some(sender);

                self.screen = Screen::Chat { topic };
                self.topic = topic;
                self.ticket_str = ticket.clone();
                self.entries.clear();
                self.names.clear();
                self.composer_text.clear();
                self.push_system(format!("Connected as {}.  Topic: {topic}", self.local_label));
                self.push_system("Type a message and press Enter to send.  /help for commands.");
                self.push_system(format!("Ticket to join this room: {ticket}"));

                // Update room history
                self.room_history.upsert(topic, &self.local_label, true);
                self.room_history_dirty = true;
                self.persist_room_history();

                iced::Task::none()
            }

            AppMessage::RoomJoinFailed(e) => {
                self.pending_topic = None;
                self.chat_list_error = format!("Failed to join room: {e}");
                self.screen = Screen::ChatList;
                iced::Task::none()
            }

            AppMessage::JoinFromTicket => {
                let ticket_input = self.join_ticket_input.clone();
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let ep_id = self.endpoint.id();

                iced::Task::perform(
                    async move {
                        let ticket: Ticket = ticket_input.parse().map_err(|e: n0_error::AnyError| e.to_string())?;
                        let topic = ticket.topic;
                        let peers: Vec<_> = ticket.peers.iter().map(|p| p.id).collect();

                        let sub = gossip.subscribe(topic, peers).await.map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let new_ticket = Ticket {
                            topic,
                            peers: vec![EndpointAddr::new(ep_id)],
                        };
                        let ticket_str = new_ticket.to_string();

                        let _ = task::spawn(forward_gossip_events(receiver, net_tx));

                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe { name: label },
                        ).map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;

                        Ok::<(GossipSender, TopicId, String), String>((sender, topic, ticket_str))
                    },
                    |result| match result {
                        Ok((sender, topic, ticket_str)) => AppMessage::RoomOpened {
                            topic,
                            ticket: ticket_str,
                            sender,
                        },
                        Err(e) => AppMessage::RoomJoinFailed(e),
                    },
                )
            }

            AppMessage::NewChatCreated => {
                // Navigate to the newly created room — handled via OpenRoom
                iced::Task::done(AppMessage::CreateNewRoom)
            }

            AppMessage::RoomSelected(topic) => {
                if let Screen::ChatList = self.screen {
                    iced::Task::done(AppMessage::OpenRoom(topic))
                } else {
                    iced::Task::none()
                }
            }

            // ── ChatList ─────────────────────────────────────────────
            AppMessage::JoinTicketInputChanged(text) => {
                self.join_ticket_input = text;
                iced::Task::none()
            }

            // ── Chat ─────────────────────────────────────────────────
            AppMessage::InputChanged(text) => {
                self.composer_text = text;
                iced::Task::none()
            }

            AppMessage::SendPressed => {
                let trimmed = self.composer_text.trim().to_string();
                if trimmed.is_empty() {
                    return iced::Task::none();
                }
                self.composer_text.clear();

                if let Some(path) = trimmed.strip_prefix("/send ") {
                    let path = path.trim().to_string();
                    return iced::Task::perform(
                        async move {
                            let path_buf = std::path::PathBuf::from(&path);
                            let abs_path = std::path::absolute(&path_buf)
                                .map_err(|_| format!("Invalid path: {path}"))?;
                            if !abs_path.exists() {
                                return Err(format!("File not found: {path}"));
                            }
                            let filename = path_buf
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            if filename.is_empty() {
                                return Err("Invalid file path.".to_string());
                            }
                            Ok(format!("{filename}|{}|{path}", abs_path.display()))
                        },
                        |r: Result<String, String>| match r {
                            Ok(v) => AppMessage::ExecuteFileSend(v),
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    );
                }

                if trimmed == "/download" {
                    return iced::Task::done(AppMessage::ExecuteDownload);
                }
                if trimmed == "/help" {
                    self.help_visible = !self.help_visible;
                    return iced::Task::none();
                }

                // ── Friend commands ──────────────────
                if let Some(pubkey_str) = trimmed.strip_prefix("/friend add ") {
                    let pubkey_str = pubkey_str.trim().to_string();
                    let (key_part, alias) =
                        if let Some((key_part, rest)) = pubkey_str.split_once(char::is_whitespace) {
                            (key_part.to_string(), Some(rest.trim().to_string()))
                        } else {
                            (pubkey_str, None)
                        };
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match key_part.parse::<PublicKey>() {
                                Ok(peer) => {
                                    let fid = FriendId::from_public_key(peer);
                                    let label = alias
                                        .clone()
                                        .unwrap_or_else(|| peer.fmt_short().to_string());
                                    let was_new = mgr.add_friend(peer, None).await.unwrap_or(false);
                                    AppMessage::FriendAdded {
                                        fid: fid.as_str().to_string(),
                                        label,
                                        was_new,
                                    }
                                }
                                Err(e) => AppMessage::ErrorMsg(format!("Invalid public key: {e}")),
                            }
                        },
                        |msg| msg,
                    );
                }

                if let Some(target) = trimmed.strip_prefix("/friend remove ") {
                    let target = target.trim().to_string();
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match target.parse::<PublicKey>() {
                                Ok(peer) => {
                                    let removed = mgr.remove_friend(&peer).await.unwrap_or(false);
                                    let label = if removed {
                                        peer.fmt_short().to_string()
                                    } else {
                                        target.clone()
                                    };
                                    AppMessage::FriendRemoved { label }
                                }
                                Err(_) => {
                                    AppMessage::ErrorMsg(format!("Friend not found: {target}"))
                                }
                            }
                        },
                        |msg| msg,
                    );
                }

                if trimmed == "/friend list" {
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match mgr.list_friends().await {
                                Ok(list) => {
                                    let items: Vec<(String, String)> = list
                                        .into_iter()
                                        .map(|(pk, status)| {
                                            let status_str = match status {
                                                FriendStatus::Unknown => "?".to_string(),
                                                FriendStatus::Online => "ONLINE".to_string(),
                                                FriendStatus::Offline => "offline".to_string(),
                                            };
                                            (pk.fmt_short().to_string(), status_str)
                                        })
                                        .collect();
                                    AppMessage::FriendListResult(items)
                                }
                                Err(e) => {
                                    AppMessage::ErrorMsg(format!("Failed to list friends: {e}"))
                                }
                            }
                        },
                        |msg| msg,
                    );
                }

                // Normal text message
                let text = trimmed.clone();
                match SignedMessage::sign_and_encode(
                    &self.secret_key,
                    &crate::Message::Message { text: trimmed },
                ) {
                    Ok(encoded) => {
                        if let Some(ref sender) = self.sender {
                            let sender = sender.clone();
                            iced::Task::perform(
                                async move {
                                    sender.broadcast(encoded).await.ok();
                                    text
                                },
                                AppMessage::MessageSent,
                            )
                        } else {
                            iced::Task::done(AppMessage::ErrorMsg(
                                "Not connected to any room.".into(),
                            ))
                        }
                    }
                    Err(e) => iced::Task::done(AppMessage::ErrorMsg(e.to_string())),
                }
            }

            AppMessage::ToggleHelp => {
                self.help_visible = !self.help_visible;
                iced::Task::none()
            }

            AppMessage::NetEvent(event) => {
                self.update_room_preview(&event);
                self.handle_net_event(event);
                self.try_save_friends();
                iced::Task::none()
            }

            AppMessage::FriendEvent(event) => {
                self.handle_friend_event(event);
                self.try_save_friends();
                iced::Task::none()
            }

            AppMessage::MessageSent(text) => {
                self.push_local(text);
                iced::Task::none()
            }

            AppMessage::ExecuteFileSend(encoded) => {
                let parts: Vec<&str> = encoded.splitn(3, '|').collect();
                if parts.len() < 3 {
                    return iced::Task::none();
                }
                let filename = parts[0].to_string();
                let abs_path = parts[1].to_string();

                let blob_store = self.blob_store.clone();
                let sender = self.sender.clone();
                let secret_key = self.secret_key.clone();
                let fname = filename.clone();

                iced::Task::perform(
                    async move {
                        let tag = blob_store
                            .blobs()
                            .add_path(std::path::PathBuf::from(&abs_path))
                            .await
                            .map_err(|e| format!("Failed to hash file: {e}"))?;
                        let ticket_str = format!("blob:{:?}", tag.hash);
                        let msg = crate::Message::FileShare {
                            name: filename.clone(),
                            ticket: ticket_str,
                        };
                        let encoded = SignedMessage::sign_and_encode(&secret_key, &msg)
                            .map_err(|e| format!("Failed to sign: {e}"))?;
                        if let Some(ref sender) = sender {
                            sender.broadcast(encoded).await.ok();
                        }
                        Ok(fname)
                    },
                    |r: Result<String, String>| match r {
                        Ok(name) => AppMessage::FileSent(name),
                        Err(e) => AppMessage::ErrorMsg(e),
                    },
                )
            }

            AppMessage::ExecuteDownload => {
                let pending = self.pending_file.clone();
                match pending {
                    Some((filename, ticket_str)) => {
                        let blob_store = self.blob_store.clone();
                        let endpoint = self.endpoint.clone();
                        iced::Task::perform(
                            async move {
                                let ticket: BlobTicket = ticket_str
                                    .parse::<BlobTicket>()
                                    .map_err(|e| format!("Parse ticket: {e}"))?;
                                let peer_id = ticket.addr().id;
                                blob_store
                                    .downloader(&endpoint)
                                    .download(ticket.hash(), Some(peer_id))
                                    .await
                                    .map_err(|e| format!("Download: {e}"))?;
                                let dest =
                                    std::env::current_dir().unwrap_or_default().join(&filename);
                                blob_store
                                    .blobs()
                                    .export(ticket.hash(), dest)
                                    .await
                                    .map_err(|e| format!("Export: {e}"))?;
                                Ok(filename)
                            },
                            |r: Result<String, String>| match r {
                                Ok(name) => AppMessage::DownloadDone(name),
                                Err(e) => AppMessage::ErrorMsg(e),
                            },
                        )
                    }
                    None => iced::Task::done(AppMessage::ErrorMsg(
                        "No pending file to download.".into(),
                    )),
                }
            }

            AppMessage::FileSent(name) => {
                self.push_system(format!("Sharing: {name}"));
                iced::Task::none()
            }
            AppMessage::DownloadDone(name) => {
                self.push_system(format!("Saved: {name}"));
                self.pending_file = None;
                iced::Task::none()
            }
            AppMessage::ErrorMsg(msg) => {
                self.push_system(msg);
                iced::Task::none()
            }

            AppMessage::FriendAdded { fid, label, was_new } => {
                let friend_id = FriendId::new(fid);
                self.friends.ensure_friend(friend_id.clone());
                if self.friends.get(&friend_id).and_then(|r| r.label.clone()).is_some() {
                    // Already has a label
                } else if label != friend_id.as_str().chars().take(12).collect::<String>() {
                    self.friends.set_label(friend_id, &label);
                }
                self.friends_dirty = true;
                if was_new {
                    self.push_system(format!("Added friend: {label}"));
                } else {
                    self.push_system(format!("Updated friend: {label}"));
                }
                self.try_save_friends();
                iced::Task::none()
            }

            AppMessage::FriendRemoved { label } => {
                self.push_system(format!("Removed friend: {label}"));
                iced::Task::none()
            }

            AppMessage::FriendListResult(items) => {
                if items.is_empty() {
                    self.push_system("No friends tracked yet.");
                } else {
                    self.push_system(format!("Friends ({}):", items.len()));
                    for (peer, status) in &items {
                        self.push_system(format!("  {peer}: {status}"));
                    }
                }
                iced::Task::none()
            }
        }
    }

    fn persist_room_history(&mut self) {
        if self.room_history_dirty {
            let _ = self.room_history.save();
            self.room_history_dirty = false;
        }
    }

    fn update_room_preview(&mut self, event: &NetEvent) {
        if let NetEvent::Message { from: _, message } = event {
            if let Message::Message { text } = message {
                let preview = if text.len() > 60 {
                    format!("{}…", &text[..60])
                } else {
                    text.clone()
                };
                self.room_history.update_preview(&self.topic, &preview);
                self.room_history_dirty = true;
            }
        }
    }

    fn try_save_friends(&mut self) {
        if self.friends_dirty {
            let _ = self.friends.save();
            self.friends_dirty = false;
        }
    }
}

// ── Net event handling ────────────────────────────────────────────────

impl IcedChat {
    fn handle_net_event(&mut self, event: NetEvent) {
        match event {
            NetEvent::Message { from, message } => match message {
                Message::AboutMe { name } => {
                    self.names.insert(from, name.clone());
                    // Track friend state
                    let fid = FriendId::from_public_key(from);
                    if self.friends.get(&fid).is_some() {
                        self.friends.set_last_announced_name(fid, name.clone());
                        self.friends_dirty = true;
                    }
                    if from != self.local_public {
                        self.push_system(format!("{} is now known as {}", from.fmt_short(), name));
                    }
                }
                Message::Message { text } => {
                    if from != self.local_public {
                        // Track friend state
                        let fid = FriendId::from_public_key(from);
                        if self.friends.get(&fid).is_some() {
                            self.friends.mark_online(fid);
                            self.friends_dirty = true;
                        }
                        let name = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());
                        self.push_remote(name, text);
                    }
                }
                Message::FileShare { name, ticket } => {
                    if from != self.local_public {
                        // Track friend state
                        let fid = FriendId::from_public_key(from);
                        if self.friends.get(&fid).is_some() {
                            self.friends.mark_online(fid);
                            self.friends_dirty = true;
                        }
                        let sender_name = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());
                        self.push_system(format!(
                            "{} shared a file: {} (type /download to fetch it)",
                            sender_name, name
                        ));
                        self.pending_file = Some((name, ticket));
                    }
                }
                Message::Goodbye => {
                    // Handled via NeighborDown
                }
            },
            NetEvent::NeighborUp { peer } => {
                // Track friend state
                let fid = FriendId::from_public_key(peer);
                if self.friends.get(&fid).is_some() {
                    self.friends.mark_online(fid);
                    self.friends_dirty = true;
                }
                let name = self
                    .names
                    .get(&peer)
                    .cloned()
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                self.push_system(format!("{name} joined the chat"));
            }
            NetEvent::NeighborDown { peer } => {
                // Track friend state
                let fid = FriendId::from_public_key(peer);
                if self.friends.get(&fid).is_some() {
                    self.friends.mark_offline(fid);
                    self.friends_dirty = true;
                }
                let name = self
                    .names
                    .get(&peer)
                    .cloned()
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                self.push_system(format!("{name} left the chat"));
            }
            NetEvent::Closed => self.push_system("The gossip receiver closed."),
            NetEvent::Error(err) => self.push_system(format!("Network error: {err}")),
        }
    }

    fn handle_friend_event(&mut self, event: FriendEvent) {
        match event {
            FriendEvent::StatusChanged { peer, status } => {
                let fid = FriendId::from_public_key(peer);
                let label = self
                    .friends
                    .get(&fid)
                    .map(|r| r.display_label(&fid))
                    .unwrap_or_else(|| peer.fmt_short().to_string());

                match status {
                    FriendStatus::Online => {
                        self.friends.mark_online(fid);
                        self.friends_dirty = true;
                        self.push_system(format!("Friend {label} is now ONLINE"));
                    }
                    FriendStatus::Offline => {
                        self.friends.mark_offline(fid);
                        self.friends_dirty = true;
                        self.push_system(format!("Friend {label} is now offline"));
                    }
                    FriendStatus::Unknown => {}
                }
            }
        }
    }
}

// ── View ──────────────────────────────────────────────────────────────

impl IcedChat {
    pub fn view(&self) -> iced::Element<'_, AppMessage> {
        match self.screen {
            Screen::ChatList => self.view_chat_list(),
            Screen::Chat { .. } => self.view_chat_screen(),
        }
    }

    // ── Chat list (inbox) view ───────────────────────────────────────

    fn view_chat_list(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, row, scrollable, text, text_input, Column};
        use iced::{Alignment, Color, Length};

        let mut content = Column::new().spacing(8).padding(12);

        // Header
        content = content.push(
            row![
                text("Iroh Gossip Chat").size(22),
            ]
            .spacing(8),
        );

        // Identity info
        content = content.push(
            text(format!("Identity: {}  |  Relay: {}", self.local_label, fmt_relay_mode(&self.relay_mode)))
                .size(11)
                .color(Color::from_rgb(0.5, 0.5, 0.5)),
        );

        content = content.push(text("")); // spacer

        // ── New Chat / Join buttons ──
        content = content.push(
            row![
                button(
                    row![
                        text(" + ").size(16),
                        text("New Chat").size(14),
                    ]
                    .align_y(Alignment::Center)
                    .spacing(4),
                )
                .on_press(AppMessage::NewChatCreated)
                .padding(8),
                button(
                    row![
                        text(" ⇄ ").size(16),
                        text("Join via Ticket").size(14),
                    ]
                    .align_y(Alignment::Center)
                    .spacing(4),
                )
                .on_press(AppMessage::JoinFromTicket)
                .padding(8),
            ]
            .spacing(8),
        );

        // ── Join ticket input ──
        content = content.push(
            row![
                text_input("Paste ticket to join a room…", &self.join_ticket_input)
                    .on_input(AppMessage::JoinTicketInputChanged)
                    .on_submit(AppMessage::JoinFromTicket)
                    .width(Length::Fill),
            ]
            .spacing(4),
        );

        // Error message
        if !self.chat_list_error.is_empty() {
            content = content.push(
                text(&self.chat_list_error)
                    .color(Color::from_rgb(0.8, 0.2, 0.2))
                    .size(12),
            );
        }

        // ── Recent chats list ──
        content = content.push(text("Recent Chats").size(16));

        if self.room_history.is_empty() {
            content = content.push(
                text("No recent chats. Create a new chat or join an existing one.")
                    .color(Color::from_rgb(0.5, 0.5, 0.5))
                    .size(13),
            );
        } else {
            let mut list = Column::new().spacing(2).width(Length::Fill);
            for room in &self.room_history.rooms {
                list = list.push(self.view_room_row(room));
            }
            content = content.push(scrollable(list).height(Length::Fill));
        }

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_room_row(&self, room: &RoomHistoryEntry) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text};
        use iced::{Color, Length};

        let topic = room.topic;
        let display_name = room.display_name();

        let preview = if room.last_preview.is_empty() {
            if room.is_owner {
                "Created this room".to_string()
            } else {
                "Joined this room".to_string()
            }
        } else {
            room.last_preview.clone()
        };

        let btn = button(
            column![
                row![
                    text(display_name).size(14).width(Length::Fill),
                ],
                row![
                    text(preview)
                        .size(11)
                        .color(Color::from_rgb(0.5, 0.5, 0.5))
                        .width(Length::Fill),
                ],
            ]
            .spacing(2)
            .padding(8),
        )
        .on_press(AppMessage::RoomSelected(topic))
        .width(Length::Fill)
        .padding(0);

        container(btn)
            .width(Length::Fill)
            .into()
    }

    // ── Chat screen view ─────────────────────────────────────────────

    fn view_chat_screen(&self) -> iced::Element<'_, AppMessage> {
        use iced::{widget, Length};

        let content = widget::column![
            self.view_chat_header(),
            self.view_chat_log(),
            widget::container(self.view_composer()).width(Length::Fill),
        ]
        .spacing(4)
        .padding(8);

        if self.help_visible {
            widget::container(self.view_help())
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        } else {
            widget::container(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }

    fn view_chat_header(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, row, text};
        use iced::Length;

        let room_name = self.room_history.find(&self.topic)
            .map(|r| r.display_name())
            .unwrap_or_else(|| format!("Room: {}", self.topic));

        column![
            row![
                button(" ← ").on_press(AppMessage::GoToChatList),
                text(room_name).size(18).width(Length::Fill),
            ]
            .spacing(4),
            text(format!("Topic: {}  |  Identity: {}", self.topic, self.local_label)).size(11),
            text(format!("Relay: {}  |  Ticket: {}", fmt_relay_mode(&self.relay_mode), self.ticket_str)).size(10),
        ]
        .spacing(2)
        .into()
    }

    fn view_chat_log(&self) -> iced::widget::Scrollable<'_, AppMessage> {
        use iced::widget::{scrollable, text, Column, Row};
        use iced::widget::text::Wrapping;
        use iced::{Color, Length};

        let mut col = Column::new().spacing(2).width(Length::Fill);

        for entry in &self.entries {
            let (label_c, body_c) = match entry.kind {
                ChatKind::System => (
                    Color::from_rgb(0.5, 0.5, 0.5),
                    Color::from_rgb(0.5, 0.5, 0.5),
                ),
                ChatKind::Local => (
                    Color::from_rgb(0.0, 0.7, 0.0),
                    Color::from_rgb(0.2, 0.8, 0.2),
                ),
                ChatKind::Remote => (
                    Color::from_rgb(0.0, 0.4, 0.8),
                    Color::from_rgb(0.8, 0.8, 0.8),
                ),
            };
            let line = Row::new()
                .push(text(format!("[{}]", entry.label)).color(label_c))
                .push(
                    text(format!(" {}", entry.body))
                        .color(body_c)
                        .wrapping(Wrapping::Word)
                        .width(Length::Fill),
                )
                .spacing(0)
                .width(Length::Fill);
            col = col.push(line);
        }

        if self.entries.is_empty() {
            col = col.push(text("No messages yet.").color(Color::from_rgb(0.5, 0.5, 0.5)));
        }

        scrollable(col)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
    }

    fn view_composer(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, text_input, Row};
        use iced::Alignment;

        Row::new()
            .push(
                text_input("Type a message...", &self.composer_text)
                    .on_input(AppMessage::InputChanged)
                    .on_submit(AppMessage::SendPressed)
                    .width(iced::Length::Fill),
            )
            .push(button("Send").on_press(AppMessage::SendPressed))
            .push(button("?").on_press(AppMessage::ToggleHelp))
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
    }

    fn view_help(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, text, Column};
        use iced::{Alignment, Length};

        let col = Column::new()
            .push(text("Help").size(20))
            .push(text(""))
            .push(text("/send <path>    Share a file with peers"))
            .push(text("/download       Fetch the last shared file"))
            .push(text("/help           Toggle this menu"))
            .push(text("/friend add <pk> [alias]  Track a friend's online status"))
            .push(text("/friend remove <pk|alias> Stop tracking a friend"))
            .push(text("/friend list    List tracked friends and their status"))
            .push(text(""))
            .push(text("Type a message and press Enter to send."))
            .push(text(""))
            .push(button("Close").on_press(AppMessage::ToggleHelp))
            .spacing(4)
            .padding(16)
            .align_x(Alignment::Center);

        container(col)
            .width(Length::Shrink)
            .height(Length::Shrink)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}

// ── Subscription ──────────────────────────────────────────────────────

struct RxHandle(Arc<Mutex<UnboundedReceiver<NetEvent>>>);

impl std::hash::Hash for RxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

struct FriendRxHandle(Arc<Mutex<UnboundedReceiver<FriendEvent>>>);

impl std::hash::Hash for FriendRxHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

fn subscription_stream(
    rx: &RxHandle,
    friend_rx: &FriendRxHandle,
) -> Pin<Box<dyn Stream<Item = AppMessage> + Send>> {
    let rx = Arc::clone(&rx.0);
    let friend_rx = Arc::clone(&friend_rx.0);
    Box::pin(n0_future::stream::unfold((rx, friend_rx), |(rx, friend_rx)| async move {
        let mut rx_guard = rx.lock().await;
        let mut friend_guard = friend_rx.lock().await;
        tokio::select! {
            event = rx_guard.recv() => {
                drop(friend_guard);
                drop(rx_guard);
                event.map(|e| (AppMessage::NetEvent(e), (rx, friend_rx)))
            }
            event = friend_guard.recv() => {
                drop(rx_guard);
                drop(friend_guard);
                event.map(|e| (AppMessage::FriendEvent(e), (rx, friend_rx)))
            }
        }
    }))
}

impl IcedChat {
    pub fn subscription(
        rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
        friend_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
    ) -> iced::Subscription<AppMessage> {
        iced::Subscription::run_with(
            (RxHandle(rx), FriendRxHandle(friend_rx)),
            |(rx, friend_rx)| subscription_stream(&rx, &friend_rx),
        )
    }
}
