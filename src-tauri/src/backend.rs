//! Chat backend — wraps iroh-gossip for the Tauri desktop app.
//!
//! Manages the iroh Endpoint, Gossip, Router, and event forwarding
//! using the same pattern as the CLI chat example (`examples/chat.rs`).

use std::{
    collections::HashSet,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use anyhow::{Context, Result};
use iroh::{
    endpoint::presets,
    protocol::Router,
    Endpoint, PublicKey, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use iroh_gossip::{
    chat_core::{
        self,
        friend_ping::{FRIEND_PING_ALPN, PingHandler},
        handle_net_event,
        AppState as ChatAppState, ChatEntry, ChatCallbacks, Message, NetEvent, SignedMessage,
        StatusContext, Ticket,
    },
    friends::FriendsStore,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    room::RoomStore,
};
use tokio::sync::mpsc;
use tracing::info;

/// Result type for Tauri IPC handlers.
pub type IpcResult<T> = Result<T, String>;

/// Events forwarded to the frontend (serialized as JSON over Tauri events).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum FrontendEvent {
    /// A new message entry was added to the log.
    NewEntry {
        kind: String,
        label: String,
        body: String,
    },
    /// Connection status changed.
    StatusUpdate {
        connected: bool,
        peer_count: usize,
        direct_peers: usize,
        relayed_peers: usize,
        neighbor_count: usize,
    },
    /// The ticket for the current room.
    Ticket { ticket: String },
    /// The gossip topic ID for the current room.
    Topic { topic: String },
    /// Our display name changed.
    Nickname { name: String },
    /// Disconnected from the gossip mesh.
    Disconnected,
    /// An error message.
    Error { message: String },
}

/// Manages the iroh node lifecycle and chat room state.
pub struct ChatBackend {
    endpoint: Endpoint,
    gossip: Gossip,
    router: Router,
    secret_key: SecretKey,
    data_dir: PathBuf,

    // Runtime state
    sender: Option<iroh_gossip::api::GossipSender>,
    current_topic: Option<TopicId>,
    current_ticket: Option<String>,
    app_state: Arc<tokio::sync::Mutex<ChatAppState>>,
    friends: Option<FriendsStore>,

    // Event channel — sender side, used internally and for forwarding
    event_tx: mpsc::UnboundedSender<FrontendEvent>,

    // Handle for the event processing task
    _event_task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ChatBackend {
    /// Create a new iroh node for chat.
    ///
    /// `event_tx` is a channel sender that the backend uses to emit
    /// [`FrontendEvent`] items.  The caller (typically the Tauri setup
    /// function) should create a receiver for this channel and forward
    /// events to the frontend via `app_handle.emit()`.
    pub async fn new(
        data_dir: PathBuf,
        event_tx: mpsc::UnboundedSender<FrontendEvent>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(&data_dir).await?;

        // Load or generate secret key (using the same format as examples/chat.rs)
        let (secret_key, _key_path) = load_or_generate_secret_key_at(&data_dir)
            .map_err(|e| anyhow::anyhow!("secret key: {e}"))?;

        let local_public = secret_key.public();
        info!("our public key: {local_public}");

        // Build the iroh endpoint
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key.clone())
            .bind()
            .await
            .context("bind endpoint")?;
        endpoint.online().await;

        let my_addr = endpoint.addr();
        let endpoint_id = endpoint.id();
        info!("endpoint id: {endpoint_id}");

        // Create gossip protocol
        let gossip = Gossip::builder().spawn(endpoint.clone());

        // In-memory blobs for file transfer
        let blob_store = MemStore::new();
        let blobs_protocol = BlobsProtocol::new(&blob_store, None);

        // Build router
        let router = Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(iroh_blobs::ALPN, blobs_protocol.clone())
            .accept(FRIEND_PING_ALPN, PingHandler)
            .spawn();

        // Load friends list
        let friends = FriendsStore::load_or_default(&data_dir);

        let app_state = Arc::new(tokio::sync::Mutex::new(
            ChatAppState::new(
                StatusContext {
                    transport_status: "Starting...".to_string(),
                    topic: TopicId::from_bytes([0u8; 32]),
                    relay_mode: iroh::RelayMode::Default,
                    connected: false,
                    peer_count: 0,
                    identity_label: local_public.fmt_short().to_string(),
                    transport_notice: "iroh gossip chat — Tauri desktop app".to_string(),
                    direct_peers: 0,
                    relayed_peers: 0,
                    neighbors: HashSet::new(),
                    peer_connection_types: std::collections::HashMap::new(),
                    last_activity: std::collections::HashMap::new(),
                    mesh_health: iroh_gossip::chat_core::MeshHealth::Good,
                },
                friends.clone(),
                local_public,
                Some(local_public.fmt_short().to_string()),
            )
        ));

        // Suppress unused-variable warnings for items kept for future use
        let _ = (blob_store, blobs_protocol, my_addr);

        Ok(Self {
            endpoint,
            gossip,
            router,
            secret_key,
            data_dir,
            sender: None,
            current_topic: None,
            current_ticket: None,
            app_state,
            friends: Some(friends),
            event_tx,
            _event_task_handle: None,
        })
    }

    /// The event channel sender — the caller uses this to forward events.
    pub fn event_tx(&self) -> mpsc::UnboundedSender<FrontendEvent> {
        self.event_tx.clone()
    }

    /// Create a new chat room.
    pub async fn create_room(&mut self, topic: Option<TopicId>) -> IpcResult<String> {
        let topic = match topic {
            Some(t) => t,
            None => {
                match RoomStore::load_or_none(&self.data_dir) {
                    Some(store) => store.topic,
                    None => {
                        let t = TopicId::from_bytes(rand::random());
                        let room = RoomStore::new(&self.data_dir, t);
                        let _ = room.save();
                        t
                    }
                }
            }
        };
        self.enter_room(topic, vec![]).await?;

        let _ = self.event_tx.send(FrontendEvent::Topic { topic: topic.to_string() });
        if let Some(tk) = &self.current_ticket {
            let _ = self.event_tx.send(FrontendEvent::Ticket { ticket: tk.clone() });
        }

        Ok(self.current_ticket.clone().unwrap_or_default())
    }

    /// Join an existing chat room from a ticket string.
    pub async fn join_room(&mut self, ticket_str: &str) -> IpcResult<String> {
        let ticket = Ticket::from_str(ticket_str)
            .map_err(|e| format!("invalid ticket: {e}"))?;
        let peers = ticket.peers.clone();
        self.enter_room(ticket.topic, peers).await?;

        let _ = self.event_tx.send(FrontendEvent::Topic { topic: ticket.topic.to_string() });
        if let Some(tk) = &self.current_ticket {
            let _ = self.event_tx.send(FrontendEvent::Ticket { ticket: tk.clone() });
        }

        Ok(self.current_ticket.clone().unwrap_or_default())
    }

    /// Internal: join a gossip topic and set up event forwarding.
    async fn enter_room(&mut self, topic: TopicId, peers: Vec<iroh::EndpointAddr>) -> IpcResult<()> {
        let peer_ids: Vec<PublicKey> = peers.iter().map(|p| p.id).collect();
        let peer_count = peer_ids.len();

        let gossip_topic = self.gossip.subscribe_and_join(topic, peer_ids.clone()).await
            .map_err(|e| format!("failed to join gossip topic: {e}"))?;
        let (sender, receiver) = gossip_topic.split();
        self.sender = Some(sender.clone());
        self.current_topic = Some(topic);

        let my_addr = self.endpoint.addr();
        let ticket_obj = Ticket {
            topic,
            peers: vec![my_addr],
        };
        self.current_ticket = Some(ticket_obj.to_string());

        info!("entered room: {topic}, ticket: {ticket_obj}");

        {
            let mut state = self.app_state.lock().await;
            state.status.topic = topic;
            state.status.connected = true;
            state.status.peer_count = peer_count;
            state.status.transport_status = "Connected".to_string();
            state.push_system(format!("Joined room: {topic}"));
            state.push_system(format!("Ticket: {ticket_obj}"));
            if peers.is_empty() {
                state.push_system("Waiting for peers to join...");
            } else {
                state.push_system(format!("Connecting to {} peer(s)...", peer_count));
            }
        }

        // Spawn event processing
        let (net_tx, net_rx) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            chat_core::forward_gossip_events(receiver, net_tx).await;
        });

        let app_state = self.app_state.clone();
        let event_tx = self.event_tx.clone();
        let endpoint = self.endpoint.clone();

        let handle = tokio::spawn(async move {
            process_net_events(net_rx, app_state, event_tx, endpoint).await;
        });
        self._event_task_handle = Some(handle);

        Ok(())
    }

    /// Send a text message to the current room.
    pub async fn send_message(&mut self, text: &str) -> IpcResult<()> {
        let sender = self.sender.as_ref()
            .ok_or_else(|| "not in a room".to_string())?;

        let message = Message::Message { text: text.to_string() };
        let encoded = SignedMessage::sign_and_encode(&self.secret_key, &message)
            .map_err(|e| format!("sign error: {e}"))?;

        sender.broadcast(encoded).await
            .map_err(|e| format!("broadcast error: {e}"))?;

        let mut state = self.app_state.lock().await;
        let label = ChatCallbacks::resolve_name(&*state, &state.local_public);
        state.push_local(label, text);

        let _ = self.event_tx.send(FrontendEvent::NewEntry {
            kind: "local".to_string(),
            label: ChatCallbacks::resolve_name(&*state, &state.local_public),
            body: text.to_string(),
        });

        Ok(())
    }

    /// Announce a display name change.
    pub async fn set_nickname(&mut self, name: &str) -> IpcResult<()> {
        let sender = self.sender.as_ref()
            .ok_or_else(|| "not in a room".to_string())?;

        let message = Message::AboutMe { name: name.to_string() };
        let encoded = SignedMessage::sign_and_encode(&self.secret_key, &message)
            .map_err(|e| format!("sign error: {e}"))?;

        sender.broadcast(encoded).await
            .map_err(|e| format!("broadcast error: {e}"))?;

        let mut state = self.app_state.lock().await;
        let local_pk = state.local_public;
        state.set_name(local_pk, name.to_string());
        state.push_system(format!("You are now known as {name}"));

        let _ = self.event_tx.send(FrontendEvent::Nickname { name: name.to_string() });

        Ok(())
    }

    /// Get the current room ticket.
    pub fn get_ticket_string(&self) -> Option<String> {
        self.current_ticket.clone()
    }

    /// Get the current chat log entries.
    pub async fn get_entries(&self) -> Vec<ChatEntry> {
        let state = self.app_state.lock().await;
        state.entries.clone()
    }

    /// Get connection status.
    pub async fn get_status(&self) -> StatusSnapshot {
        let state = self.app_state.lock().await;
        StatusSnapshot {
            connected: state.status.connected,
            peer_count: state.status.peer_count,
            direct_peers: state.status.direct_peers,
            relayed_peers: state.status.relayed_peers,
            neighbor_count: state.status.neighbors.len(),
            topic: state.status.topic.to_string(),
            identity_label: state.status.identity_label.clone(),
            transport_status: state.status.transport_status.clone(),
        }
    }

    /// Shut down the iroh node cleanly.
    pub async fn shutdown(self) {
        if let Err(e) = self.router.shutdown().await {
            tracing::warn!("router shutdown error: {e}");
        }
        self.endpoint.close().await;
    }
}

/// Serializable status snapshot for the frontend.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusSnapshot {
    pub connected: bool,
    pub peer_count: usize,
    pub direct_peers: usize,
    pub relayed_peers: usize,
    pub neighbor_count: usize,
    pub topic: String,
    pub identity_label: String,
    pub transport_status: String,
}

/// Process net events from the gossip receiver, update state, and forward to frontend.
async fn process_net_events(
    mut net_rx: mpsc::UnboundedReceiver<NetEvent>,
    app_state: Arc<tokio::sync::Mutex<ChatAppState>>,
    event_tx: mpsc::UnboundedSender<FrontendEvent>,
    endpoint: Endpoint,
) {
    let mut last_peer_count: usize = 0;

    while let Some(event) = net_rx.recv().await {
        let mut state = app_state.lock().await;

        if let Err(e) = handle_net_event(event, &mut *state) {
            tracing::warn!("handle_net_event error: {e}");
            let _ = event_tx.send(FrontendEvent::Error {
                message: format!("event error: {e}"),
            });
        }

        // Forward the last entry (if any) for real-time updates
        if let Some(entry) = state.entries.last() {
            let kind = match entry.kind {
                chat_core::ChatKind::System => "system",
                chat_core::ChatKind::Local => "local",
                chat_core::ChatKind::Remote => "remote",
            };
            let _ = event_tx.send(FrontendEvent::NewEntry {
                kind: kind.to_string(),
                label: entry.label.clone(),
                body: entry.body.clone(),
            });
        }

        // Periodically update connection counts when neighbors change
        let current_ncount = state.status.neighbors.len();
        drop(state); // release lock

        if current_ncount > 0 && current_ncount != last_peer_count {
            last_peer_count = current_ncount;
            let mut state2 = app_state.lock().await;
            chat_core::update_connection_counts(&endpoint, &mut state2.status).await;
            let _ = event_tx.send(FrontendEvent::StatusUpdate {
                connected: state2.status.connected,
                peer_count: state2.status.peer_count,
                direct_peers: state2.status.direct_peers,
                relayed_peers: state2.status.relayed_peers,
                neighbor_count: state2.status.neighbors.len(),
            });
        }

        // Check quit signal
        let should_quit = app_state.lock().await.should_quit;
        if should_quit {
            let _ = event_tx.send(FrontendEvent::Disconnected);
            break;
        }
    }

    let _ = event_tx.send(FrontendEvent::Disconnected);
}

/// Load or generate a secret key, matching the CLI example's format.
fn load_or_generate_secret_key_at(data_dir: &std::path::Path) -> anyhow::Result<(SecretKey, PathBuf)> {
    let key_path = data_dir.join("secret_key.txt");

    if key_path.exists() {
        let key_str = std::fs::read_to_string(&key_path)
            .context("failed to read secret key file")?;
        let key_str = key_str.trim();
        let key = SecretKey::from_str(key_str)
            .context("failed to parse secret key from file")?;
        Ok((key, key_path))
    } else {
        let key = SecretKey::generate();
        let key_str = data_encoding::HEXLOWER.encode(&key.to_bytes());
        std::fs::create_dir_all(data_dir)
            .context("failed to create data directory")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700));
        }

        let content = format!("{key_str}\n");
        std::fs::write(&key_path, &content)
            .context("failed to write secret key file")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }

        Ok((key, key_path))
    }
}
