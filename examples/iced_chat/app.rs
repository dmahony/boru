//! The iced Application for the gossip chat frontend.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use iroh::{PublicKey, RelayMode, SecretKey};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket};
use iroh_gossip::api::GossipSender;
use iroh_gossip::chat_core::friend_ping::{
    FriendEvent, FriendPingManager, FriendStatus,
};
use iroh_gossip::friends::{FriendId, FriendsStore};
use iroh_gossip::proto::TopicId;
use n0_future::Stream;
use tokio::sync::{mpsc::UnboundedReceiver, Mutex};

use crate::{fmt_relay_mode, Message, NetEvent, SignedMessage};

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

// ── Application state ─────────────────────────────────────────────────

pub struct IcedChat {
    entries: Vec<ChatEntry>,
    composer_text: String,
    help_visible: bool,
    pending_file: Option<(String, String)>,
    names: HashMap<PublicKey, String>,
    secret_key: SecretKey,
    sender: GossipSender,
    blob_store: MemStore,
    endpoint: iroh::Endpoint,
    local_label: String,
    local_public: PublicKey,
    topic: TopicId,
    relay_mode: RelayMode,
    _ticket_str: String,
    _peer_count: usize,
    pub net_rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
    friends: FriendsStore,
    friends_dirty: bool,
    friend_mgr: FriendPingManager,
    pub friend_events_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
}

#[derive(Debug, Clone)]
pub enum AppMessage {
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
    FriendAdded {
        fid: String,
        label: String,
        was_new: bool,
    },
    FriendRemoved {
        label: String,
    },
    FriendListResult(Vec<(String, String)>),
}

impl IcedChat {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        secret_key: SecretKey,
        sender: GossipSender,
        blob_store: MemStore,
        endpoint: iroh::Endpoint,
        local_label: String,
        local_public: PublicKey,
        topic: TopicId,
        relay_mode: RelayMode,
        net_rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
        ticket_str: String,
        peer_count: usize,
        friends: FriendsStore,
        friend_mgr: FriendPingManager,
        friend_events_rx: Arc<Mutex<UnboundedReceiver<FriendEvent>>>,
    ) -> Self {
        let mut app = Self {
            entries: Vec::new(),
            composer_text: String::new(),
            help_visible: false,
            pending_file: None,
            names: HashMap::new(),
            secret_key,
            sender,
            blob_store,
            endpoint,
            local_label: local_label.clone(),
            local_public,
            topic,
            relay_mode,
            _ticket_str: ticket_str,
            _peer_count: peer_count,
            net_rx,
            friends,
            friends_dirty: false,
            friend_mgr,
            friend_events_rx,
        };
        app.push_system(format!("Connected as {local_label}.  Topic: {}", app.topic));
        if peer_count == 0 {
            app.push_system("Waiting for peers to join us...");
        } else {
            app.push_system(format!(
                "Connecting to {peer_count} peers from the ticket..."
            ));
        }
        app.push_system("Type a message and press Enter to send.  /help for commands.");
        app
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

// ── Update ────────────────────────────────────────────────────────────

impl IcedChat {
    pub fn update(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
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

                // ── Friend commands ────────────────────────────────
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
                            // We resolve by public key first, then alias.
                            // Unfortunately we can't access self.friends from here,
                            // so we try parsing as a public key directly.
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
                        let sender = self.sender.clone();
                        iced::Task::perform(
                            async move {
                                sender.broadcast(encoded).await.ok();
                                text
                            },
                            AppMessage::MessageSent,
                        )
                    }
                    Err(e) => iced::Task::done(AppMessage::ErrorMsg(e.to_string())),
                }
            }

            AppMessage::ToggleHelp => {
                self.help_visible = !self.help_visible;
                iced::Task::none()
            }

            AppMessage::NetEvent(event) => {
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
                        sender.broadcast(encoded).await.ok();
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
                    // Handled via NeighborDown (cleaner, covers both clean and unclean exits)
                }
            },
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
        use iced::{widget, Length};

        let content = widget::column![
            self.view_header(),
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

    fn view_header(&self) -> iced::Element<'_, AppMessage> {
        let relay = fmt_relay_mode(&self.relay_mode);
        iced::widget::column![
            iced::widget::text("Iroh Gossip Chat").size(18),
            iced::widget::text(format!(
                "Topic: {}  |  Identity: {}",
                self.topic, self.local_label
            ))
            .size(11),
            iced::widget::text(format!("Relay: {relay}  |  Peers: {}", self._peer_count)).size(11),
        ]
        .spacing(2)
        .into()
    }

    fn view_chat_log(&self) -> iced::widget::Scrollable<'_, AppMessage> {
        use iced::widget::{scrollable, text, Column, Row};
        use iced::Color;

        let mut col = Column::new().spacing(2).width(iced::Length::Fill);

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
                .push(text(format!(" {}", entry.body)).color(body_c))
                .spacing(0)
                .width(iced::Length::Fill);
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

/// Wrapper so we can satisfy `Hash` for `Subscription::run_with`.
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

/// Build the async stream that bridges gossip events and friend events into iced messages.
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
