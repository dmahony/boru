//! The iced Application for the gossip chat frontend.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use iroh::{PublicKey, RelayMode, SecretKey};
use iroh_blobs::{store::mem::MemStore, ticket::BlobTicket};
use iroh_gossip::api::GossipSender;
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
        Self { kind: ChatKind::System, label: "System".into(), body: text.into() }
    }
    fn local(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self { kind: ChatKind::Local, label: label.into(), body: text.into() }
    }
    fn remote(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self { kind: ChatKind::Remote, label: label.into(), body: text.into() }
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
}

#[derive(Debug, Clone)]
pub enum AppMessage {
    InputChanged(String),
    SendPressed,
    ToggleHelp,
    NetEvent(NetEvent),
    MessageSent(String),
    FileSent(String),
    DownloadDone(String),
    ErrorMsg(String),
    ExecuteFileSend(String),
    ExecuteDownload,
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
        };
        app.push_system(format!("Connected as {local_label}.  Topic: {}", app.topic));
        if peer_count == 0 {
            app.push_system("Waiting for peers to join us...");
        } else {
            app.push_system(format!("Connecting to {peer_count} peers from the ticket..."));
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

                let text = trimmed.clone();
                match SignedMessage::sign_and_encode(
                    &self.secret_key,
                    &crate::Message::Message { text: trimmed },
                ) {
                    Ok(encoded) => {
                        let sender = self.sender.clone();
                        iced::Task::perform(
                            async move { sender.broadcast(encoded).await.ok(); text },
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
                iced::Task::none()
            }

            AppMessage::MessageSent(text) => {
                self.push_local(text);
                iced::Task::none()
            }

            AppMessage::ExecuteFileSend(encoded) => {
                let parts: Vec<&str> = encoded.splitn(3, '|').collect();
                if parts.len() < 3 { return iced::Task::none(); }
                let filename = parts[0].to_string();
                let abs_path = parts[1].to_string();

                let blob_store = self.blob_store.clone();
                let sender = self.sender.clone();
                let secret_key = self.secret_key.clone();
                let fname = filename.clone();

                iced::Task::perform(
                    async move {
                        let tag = blob_store.blobs()
                            .add_path(std::path::PathBuf::from(&abs_path)).await
                            .map_err(|e| format!("Failed to hash file: {e}"))?;
                        let ticket_str = format!("blob:{:?}", tag.hash);
                        let msg = crate::Message::FileShare { name: filename.clone(), ticket: ticket_str };
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
                                let ticket: BlobTicket = ticket_str.parse::<BlobTicket>()
                                    .map_err(|e| format!("Parse ticket: {e}"))?;
                                let peer_id = ticket.addr().id;
                                blob_store.downloader(&endpoint)
                                    .download(ticket.hash(), Some(peer_id)).await
                                    .map_err(|e| format!("Download: {e}"))?;
                                let dest = std::env::current_dir().unwrap_or_default().join(&filename);
                                blob_store.blobs().export(ticket.hash(), dest).await
                                    .map_err(|e| format!("Export: {e}"))?;
                                Ok(filename)
                            },
                            |r: Result<String, String>| match r {
                                Ok(name) => AppMessage::DownloadDone(name),
                                Err(e) => AppMessage::ErrorMsg(e),
                            },
                        )
                    }
                    None => iced::Task::done(AppMessage::ErrorMsg("No pending file to download.".into())),
                }
            }

            AppMessage::FileSent(name) => { self.push_system(format!("Sharing: {name}")); iced::Task::none() }
            AppMessage::DownloadDone(name) => { self.push_system(format!("Saved: {name}")); self.pending_file = None; iced::Task::none() }
            AppMessage::ErrorMsg(msg) => { self.push_system(msg); iced::Task::none() }
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
                    if from != self.local_public {
                        self.push_system(format!("{} is now known as {}", from.fmt_short(), name));
                    }
                }
                Message::Message { text } => {
                    if from != self.local_public {
                        let name = self.names.get(&from).cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());
                        self.push_remote(name, text);
                    }
                }
                Message::FileShare { name, ticket } => {
                    if from != self.local_public {
                        let sender_name = self.names.get(&from).cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());
                        self.push_system(format!(
                            "{} shared a file: {} (type /download to fetch it)", sender_name, name
                        ));
                        self.pending_file = Some((name, ticket));
                    }
                }
                Message::Goodbye => {
                    // Handled via NeighborDown (cleaner, covers both clean and unclean exits)
                }
            },
            NetEvent::NeighborDown { peer } => {
                let name = self.names.get(&peer).cloned()
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                self.push_system(format!("{name} left the chat"));
            }
            NetEvent::Closed => self.push_system("The gossip receiver closed."),
            NetEvent::Error(err) => self.push_system(format!("Network error: {err}")),
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
                .width(Length::Fill).height(Length::Fill)
                .center_x(Length::Fill).center_y(Length::Fill)
                .into()
        } else {
            widget::container(content).width(Length::Fill).height(Length::Fill).into()
        }
    }

    fn view_header(&self) -> iced::Element<'_, AppMessage> {
        let relay = fmt_relay_mode(&self.relay_mode);
        iced::widget::column![
            iced::widget::text("Iroh Gossip Chat").size(18),
            iced::widget::text(format!("Topic: {}  |  Identity: {}", self.topic, self.local_label)).size(11),
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
                ChatKind::System => (Color::from_rgb(0.5, 0.5, 0.5), Color::from_rgb(0.5, 0.5, 0.5)),
                ChatKind::Local => (Color::from_rgb(0.0, 0.7, 0.0), Color::from_rgb(0.2, 0.8, 0.2)),
                ChatKind::Remote => (Color::from_rgb(0.0, 0.4, 0.8), Color::from_rgb(0.8, 0.8, 0.8)),
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

        scrollable(col).width(iced::Length::Fill).height(iced::Length::Fill)
    }

    fn view_composer(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, text_input, Row};
        use iced::Alignment;

        Row::new()
            .push(text_input("Type a message...", &self.composer_text)
                .on_input(AppMessage::InputChanged)
                .on_submit(AppMessage::SendPressed)
                .width(iced::Length::Fill))
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

/// Build the async stream that bridges gossip events into iced messages.
fn subscription_stream(
    rx: &RxHandle,
) -> Pin<Box<dyn Stream<Item = AppMessage> + Send>> {
    let rx = Arc::clone(&rx.0);
    Box::pin(n0_future::stream::unfold(rx, |rx| async move {
        let event = {
            let mut guard = rx.lock().await;
            guard.recv().await
        };
        event.map(|e| (AppMessage::NetEvent(e), rx))
    }))
}

impl IcedChat {
    pub fn subscription(
        rx: Arc<Mutex<UnboundedReceiver<NetEvent>>>,
    ) -> iced::Subscription<AppMessage> {
        iced::Subscription::run_with(RxHandle(rx), subscription_stream)
    }
}
