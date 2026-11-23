//! GUI-independent owner for Boru's transport, persistence, and core event pump.
//!
//! The desktop frontend may consume the returned handles, but this module has no
//! dependency on any GUI toolkit.

use std::{
    fs::File,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result};
use fs2::FileExt;
use iroh::{
    address_lookup::{AddrFilter, DnsAddressLookup, PkarrResolver},
    endpoint::presets,
    Endpoint, RelayMode,
};
use iroh_mainline_address_lookup::DhtAddressLookup;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{
    application_commands::ApplicationCommands,
    application_service::ConversationApplicationService,
    net::{Gossip, GOSSIP_ALPN},
    storage::Storage,
    ui_events::UiEvent,
};

/// Production startup settings for a desktop runtime.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct DesktopRuntimeConfig {
    pub data_dir: PathBuf,
    pub secret_key: Option<iroh::SecretKey>,
    pub relay_mode: RelayMode,
    pub bind_port: u16,
    pub enable_mdns: bool,
    pub enable_dht: bool,
    pub publish_direct_addresses: bool,
}

/// Headless-capable core transport and persistence runtime.
#[allow(missing_debug_implementations)]
pub struct DesktopRuntime {
    endpoint: Option<Endpoint>,
    router: Option<iroh::protocol::Router>,
    gossip: Gossip,
    message_store: crate::store::MessageStore,
    secret_key: iroh::SecretKey,
    room_senders: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<crate::TopicId, crate::api::GossipSender>>>,
    storage: std::sync::Arc<Storage>,
    data_dir: PathBuf,
    transfer_store: std::sync::Arc<crate::transfer_state_projection::TransferStateStore>,
    ui_event_tx: broadcast::Sender<UiEvent>,
    ui_events: broadcast::Receiver<UiEvent>,
    event_journal: crate::ui_events::UiEventJournal,
    sequenced_event_tx: broadcast::Sender<crate::ui_events::SequencedUiEvent>,
    profile_lock: Option<ProfileLock>,
    cancellation: CancellationToken,
    workers: JoinSet<()>,
    stopped: bool,
}

/// Inputs required to build the production protocol graph around the caller's endpoint.
#[allow(missing_debug_implementations, missing_docs)]
pub struct DesktopRuntimeParts {
    pub endpoint: Endpoint,
    pub secret_key: iroh::SecretKey,
    pub storage: std::sync::Arc<Storage>,
    pub data_dir: std::path::PathBuf,
    pub friends: crate::friends::FriendsStore,
    pub file_offer_registry: std::sync::Arc<std::sync::Mutex<crate::file_offer::FileOfferRegistry>>,
    pub transfer_store: std::sync::Arc<crate::transfer_state_projection::TransferStateStore>,
    pub outbound_item_labels:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
}

/// Constructed protocol handles and their event streams; no GUI types are exposed.
#[allow(missing_debug_implementations)]
#[allow(missing_docs)]
pub struct DesktopRuntimeHandles {
    pub blobs: iroh_blobs::store::fs::FsStore,
    pub file_offer_registry: std::sync::Arc<std::sync::Mutex<crate::file_offer::FileOfferRegistry>>,
    pub transfer_store: std::sync::Arc<crate::transfer_state_projection::TransferStateStore>,
    pub outbound_item_labels:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    pub friend_mgr: crate::chat_core::friend_ping::FriendPingManager,
    pub friend_events_rx: std::sync::Arc<
        tokio::sync::Mutex<tokio::sync::mpsc::Receiver<crate::chat_core::friend_ping::FriendEvent>>,
    >,
    pub backfill_handle: crate::backfill::BackfillHandle,
    pub whisper_handle: crate::whisper::WhisperHandle,
    pub whisper_events_rx: std::sync::Arc<
        tokio::sync::Mutex<tokio::sync::mpsc::Receiver<crate::whisper::WhisperEvent>>,
    >,
    pub inbox_handle: crate::inbox::InboxHandle,
    pub inbox_events_rx:
        std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<crate::inbox::InboxEvent>>>,
    pub tunnel_service: std::sync::Arc<crate::tunnel::service::TunnelService>,
    pub call_handle: crate::call::manager::CallHandle,
    pub call_events_rx: std::sync::Arc<
        tokio::sync::Mutex<tokio::sync::mpsc::Receiver<crate::call::manager::CallEvent>>,
    >,
    /// Screen-share protocol registered by this runtime (`screen-sharing` only).
    #[cfg(feature = "screen-sharing")]
    pub screen_share: crate::screen_share::protocol::ScreenShareProtocol,
    #[cfg(feature = "screen-sharing")]
    pub screen_share_events:
        tokio::sync::mpsc::Receiver<crate::screen_share::session::SessionEvent>,
    #[cfg(feature = "screen-sharing")]
    pub screen_share_media:
        tokio::sync::mpsc::Receiver<crate::screen_share::protocol::InboundMedia>,
    #[cfg(feature = "screen-sharing")]
    pub screen_share_audio:
        tokio::sync::mpsc::Receiver<crate::screen_share::protocol::InboundAudio>,
    #[cfg(feature = "screen-sharing")]
    pub screen_share_events_tx:
        tokio::sync::mpsc::Sender<crate::screen_share::session::SessionEvent>,
}

/// Advisory process-wide lock for one on-disk Boru profile.
struct ProfileLock {
    _file: File,
}

impl ProfileLock {
    fn acquire(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("create profile directory {}", data_dir.display()))?;
        let path = data_dir.join(".boru-profile.lock");
        let file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open profile lock {}", path.display()))?;
        file.try_lock_exclusive()
            .with_context(|| format!("profile is already in use: {}", data_dir.display()))?;
        Ok(Self { _file: file })
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._file);
    }
}

impl DesktopRuntime {
    /// Build production-owned identity, endpoint, and stores, then start the runtime.
    pub async fn start_from_config(
        config: DesktopRuntimeConfig,
    ) -> Result<(Self, DesktopRuntimeHandles)> {
        anyhow::ensure!(
            !config.publish_direct_addresses || config.enable_dht,
            "publishing direct addresses requires DHT discovery"
        );
        let data_dir = config.data_dir;
        let profile_lock = ProfileLock::acquire(&data_dir)?;
        let secret_key = match config.secret_key {
            Some(key) => key,
            None => {
                let path = data_dir.join("secret_key.txt");
                if path.exists() {
                    let value = std::fs::read_to_string(&path)
                        .with_context(|| format!("read secret key {}", path.display()))?;
                    iroh::SecretKey::from_str(value.trim()).context("parse profile secret key")?
                } else {
                    let key = iroh::SecretKey::generate();
                    std::fs::write(
                        &path,
                        format!("{}\n", data_encoding::HEXLOWER.encode(&key.to_bytes())),
                    )
                    .with_context(|| format!("write secret key {}", path.display()))?;
                    key
                }
            }
        };
        // mDNS: build the lookup and register its discovery subscriber BEFORE the
        // endpoint binds. The mDNS service caches advertisements but does not
        // replay them to subscribers created later, so subscribing after bind
        // would miss the loopback advertisement (see src/bin/boru/main.rs:1009-1016).
        // This API owns no discovery-event consumer, so the receiver is released
        // once the endpoint is bound; the lookup stays registered with the
        // endpoint's address book and keeps resolving peers.
        let mut endpoint_builder = Endpoint::builder(presets::Minimal);
        let mdns_events = if config.enable_mdns {
            let mdns = MdnsAddressLookup::builder().build(secret_key.public())?;
            let events = mdns.subscribe().await;
            endpoint_builder = endpoint_builder.address_lookup(mdns);
            Some(events)
        } else {
            None
        };
        // This is the sole production Endpoint construction site for this API.
        let endpoint = endpoint_builder
            .secret_key(secret_key.clone())
            // Read-only DNS/pkarr resolution stays available even with mDNS and
            // DHT off; it only resolves peer IDs that already carry a relay URL.
            .address_lookup(PkarrResolver::n0_dns())
            .address_lookup(DnsAddressLookup::n0_dns())
            .relay_mode(config.relay_mode.clone())
            .bind_addr(std::net::SocketAddr::from(([0, 0, 0, 0], config.bind_port)))?
            .bind()
            .await?;
        drop(mdns_events);
        if !matches!(config.relay_mode, RelayMode::Disabled) {
            match tokio::time::timeout(std::time::Duration::from_secs(15), endpoint.online()).await
            {
                Ok(()) => {}
                Err(_) => tracing::warn!("relay online() timed out after 15s; proceeding anyway"),
            }
        }
        if config.enable_dht {
            // Relay-only by default (privacy-preserving); publishing direct
            // addresses is opt-in and only meaningful with DHT enabled.
            let addr_filter = if config.publish_direct_addresses {
                AddrFilter::unfiltered()
            } else {
                AddrFilter::relay_only()
            };
            let dht = DhtAddressLookup::builder()
                .secret_key(endpoint.secret_key().clone())
                .addr_filter(addr_filter)
                .build()
                .context("build DHT address lookup")?;
            endpoint
                .address_lookup()
                .context("address lookup registry unavailable")?
                .add(dht);
        }
        // This is the sole production Storage construction site for this API.
        let storage = std::sync::Arc::new(
            Storage::open(&data_dir).map_err(|e| anyhow::anyhow!("open profile storage: {e}"))?,
        );
        let friends = crate::friends::FriendsStore::load_from_sqlite(&storage, &data_dir);
        let parts = DesktopRuntimeParts {
            endpoint,
            secret_key,
            storage,
            data_dir,
            friends,
            file_offer_registry: std::sync::Arc::new(std::sync::Mutex::new(
                crate::file_offer::FileOfferRegistry::new(),
            )),
            transfer_store: std::sync::Arc::new(
                crate::transfer_state_projection::TransferStateStore::new(256),
            ),
            outbound_item_labels: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        };
        let (mut runtime, handles) = Self::start_with(parts).await?;
        runtime.profile_lock = Some(profile_lock);
        Ok((runtime, handles))
    }

    /// Construct and start the production protocol graph on the supplied endpoint.
    pub async fn start_with(parts: DesktopRuntimeParts) -> Result<(Self, DesktopRuntimeHandles)> {
        // BRT-008b residual ownership: inbox replay/pending callbacks (1st item)
        // and the blob provider consumer (3rd item) are now runtime-owned by
        // this constructor via `install_inbox_replay_callbacks` and
        // `consume_outbound_provider_events`. Remaining residuals:
        // RESIDUAL BRT-008b: src/bin/boru/main.rs:1833-1848 — inbox authorization callback closes over the live SQLite Storage and data_dir; supply these dependencies or move callback wiring with runtime construction.
        // RESIDUAL BRT-008b: screen-share channels are exposed via `DesktopRuntimeHandles`; only the Iced-specific receiver adoption (`calls_state`) remains in main until a runtime result is wired to app.
        use crate::{
            backfill::{BackfillProtocolHandler, BACKFILL_ALPN},
            catalogue_handler::CatalogueHandler,
            chat_core::friend_ping::{
                FriendPingManager, PingHandler, DEFAULT_CONNECT_TIMEOUT, DEFAULT_PING_INTERVAL,
                FRIEND_PING_ALPN,
            },
            companion_protocol::{
                CompanionPolicy, CompanionProtocolHandler, COMPANION_ALPN,
                COMPANION_DEFAULT_ENABLED,
            },
            file_access_handler::{FileAccessHandler, NonceStore},
            file_offer_protocol::{FileOfferProtocolHandler, FILE_OFFER_ALPN},
            inbox::{InboxHandle, InboxProtocol, INBOX_ALPN},
            protocol_version::CATALOGUE_ALPN,
            tunnel::{TunnelProtocol, BORU_TUNNEL_ALPN},
            whisper::{WhisperBuilder, WHISPER_ALPN},
        };
        use iroh_blobs::{
            provider::events::{ConnectMode, EventMask, EventSender, RequestMode},
            store::fs::FsStore,
            BlobsProtocol,
        };

        let DesktopRuntimeParts {
            endpoint,
            secret_key,
            storage,
            data_dir,
            friends,
            file_offer_registry,
            transfer_store,
            outbound_item_labels,
        } = parts;
        let runtime_data_dir = data_dir.clone();
        let local_public = secret_key.public();
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let blobs = FsStore::load(data_dir.join("blobs")).await?;
        let (event_sender, provider_events) = EventSender::channel(
            128,
            EventMask {
                connected: ConnectMode::Notify,
                get: RequestMode::NotifyLog,
                get_many: RequestMode::NotifyLog,
                push: RequestMode::Disabled,
                observe: iroh_blobs::provider::events::ObserveMode::None,
                throttle: iroh_blobs::provider::events::ThrottleMode::None,
            },
        );
        let blobs_protocol = BlobsProtocol::new(&blobs, Some(event_sender));
        let (friend_mgr, friend_events) = FriendPingManager::spawn(
            endpoint.clone(),
            DEFAULT_PING_INTERVAL,
            DEFAULT_CONNECT_TIMEOUT,
        );
        let friend_events_rx = std::sync::Arc::new(tokio::sync::Mutex::new(friend_events));
        let message_store = crate::store::MessageStore::open(data_dir.join("message_store.db"))?;
        let backfill_handler =
            BackfillProtocolHandler::new(message_store.clone(), storage.clone(), local_public);
        let backfill_handle = crate::backfill::BackfillHandle::spawn(endpoint.clone());
        let whisper_builder = WhisperBuilder::new(endpoint.clone(), secret_key.clone());
        let whisper_handler = whisper_builder.protocol_handler();
        let (whisper_handle, whisper_events) = whisper_builder.spawn();
        let (inbox_handle, inbox_events) = InboxHandle::new();
        // Runtime construction owns the offline-replay callbacks: the same
        // helper the legacy binary path uses, so the two cannot drift.
        install_inbox_replay_callbacks(&inbox_handle, data_dir.clone()).await;
        let inbox_protocol =
            InboxProtocol::new(inbox_handle.inner()).with_secret_key(secret_key.clone());
        let catalogue = CatalogueHandler::new(
            storage.clone(),
            secret_key.clone(),
            local_public.to_string(),
            friends.clone(),
        );
        let file_access = FileAccessHandler::new(
            storage.clone(),
            secret_key.clone(),
            local_public.to_string(),
            friends,
            std::sync::Arc::new(NonceStore::new()),
            std::sync::Arc::new(blobs.clone().into()),
        );
        let file_offer =
            FileOfferProtocolHandler::new(file_offer_registry.clone(), transfer_store.clone());
        let tunnel_service = std::sync::Arc::new(
            crate::tunnel::service::TunnelService::with_enrollment_store(std::sync::Arc::new(
                crate::tunnel::enrollment::EnrollmentTokenStore::load_or_default(&data_dir),
            )),
        );
        let tunnel = TunnelProtocol::with_service(tunnel_service.clone(), local_public);
        let call_builder = crate::call::manager::CallBuilder::new(endpoint.clone(), secret_key.clone());
        let call_handler = call_builder.protocol_handler();
        let (call_handle, call_events_rx) = call_builder.spawn();
        let companion =
            CompanionProtocolHandler::new(CompanionPolicy::new(), COMPANION_DEFAULT_ENABLED);
        #[cfg(feature = "screen-sharing")]
        let screen_share = {
            let (events_tx, events_rx) = tokio::sync::mpsc::channel(32);
            let (media_tx, media_rx) = tokio::sync::mpsc::channel(64);
            let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(32);
            let protocol = crate::screen_share::protocol::ScreenShareProtocol::with_channels(
                events_tx.clone(),
                media_tx,
                audio_tx,
            );
            (protocol, events_rx, media_rx, audio_rx, events_tx)
        };
        let router_builder = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(iroh_blobs::ALPN, blobs_protocol)
            .accept(FRIEND_PING_ALPN, PingHandler)
            .accept(BACKFILL_ALPN, backfill_handler)
            .accept(WHISPER_ALPN, whisper_handler)
            .accept(INBOX_ALPN, inbox_protocol)
            .accept(CATALOGUE_ALPN, catalogue)
            .accept(crate::net::FILE_ACCESS_ALPN, file_access)
            .accept(FILE_OFFER_ALPN, file_offer)
            .accept(BORU_TUNNEL_ALPN, tunnel)
            .accept(crate::call::manager::CALL_ALPN, call_handler)
            .accept(COMPANION_ALPN, companion);
        #[cfg(feature = "screen-sharing")]
        let router_builder = router_builder.accept(
            crate::screen_share::protocol::SCREEN_SHARE_ALPN,
            screen_share.0.clone(),
        );
        let router = router_builder.spawn();
        let (ui_event_tx, ui_events) = broadcast::channel(256);
        let (sequenced_event_tx, _) = broadcast::channel(256);
        let event_journal = crate::ui_events::UiEventJournal::new(
            format!(
                "{}:{}",
                endpoint.id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            4096,
        );
        let mut runtime = Self {
            endpoint: Some(endpoint),
            router: Some(router),
            gossip,
            message_store,
            secret_key,
            room_senders: std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            storage,
            data_dir: runtime_data_dir,
            transfer_store: transfer_store.clone(),
            ui_event_tx,
            ui_events,
            event_journal,
            sequenced_event_tx,
            profile_lock: None,
            cancellation: CancellationToken::new(),
            workers: JoinSet::new(),
            stopped: false,
        };
        let handles = DesktopRuntimeHandles {
            blobs,
            file_offer_registry,
            transfer_store,
            outbound_item_labels,

            friend_mgr,
            friend_events_rx,
            backfill_handle,
            whisper_handle,
            whisper_events_rx: std::sync::Arc::new(tokio::sync::Mutex::new(whisper_events)),
            inbox_handle,
            inbox_events_rx: std::sync::Arc::new(tokio::sync::Mutex::new(inbox_events)),
            tunnel_service,
            call_handle,
            call_events_rx: std::sync::Arc::new(tokio::sync::Mutex::new(call_events_rx)),
            #[cfg(feature = "screen-sharing")]
            screen_share: screen_share.0,
            #[cfg(feature = "screen-sharing")]
            screen_share_events: screen_share.1,
            #[cfg(feature = "screen-sharing")]
            screen_share_media: screen_share.2,
            #[cfg(feature = "screen-sharing")]
            screen_share_audio: screen_share.3,
            #[cfg(feature = "screen-sharing")]
            screen_share_events_tx: screen_share.4,
        };
        // Each source has its own ordered pump; workers are runtime-owned and
        // cancellation-aware, so startup never waits for protocol traffic.
        runtime.pump_events("friend", handles.friend_events_rx.clone());
        runtime.pump_events("whisper", handles.whisper_events_rx.clone());
        runtime.pump_events("inbox", handles.inbox_events_rx.clone());
        runtime.pump_events("call", handles.call_events_rx.clone());
        // Runtime construction owns the outbound blob-provider consumer: it
        // projects served transfers into the live store and the durable
        // activity table. Registered as a tracked worker so shutdown cancels
        // and joins it with the other pumps.
        let consumer_store = handles.transfer_store.clone();
        let consumer_labels = handles.outbound_item_labels.clone();
        let consumer_storage = runtime.storage.clone();
        runtime.spawn_worker(move |cancel| {
            consume_outbound_provider_events(
                provider_events,
                consumer_store,
                consumer_labels,
                Some(consumer_storage),
                local_public,
                Some(cancel),
            )
        });
        Ok((runtime, handles))
    }

    fn pump_events<T>(
        &mut self,
        source: &'static str,
        receiver: std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<T>>>,
    ) where
        T: std::fmt::Debug + Send + 'static,
    {
        let tx = self.ui_event_tx.clone();
        let journal = self.event_journal.clone();
        let sequenced_tx = self.sequenced_event_tx.clone();
        self.spawn_worker(move |cancel| async move {
            let mut receiver = receiver.lock().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    event = receiver.recv() => match event {
                        Some(_event) => {
                            let event = UiEvent::ProtocolEvent {
                                source,
                                detail: "state changed; refresh the authoritative projection".into(),
                            };
                            let item = journal.append(event.clone());
                            let _ = sequenced_tx.send(item);
                            let _ = tx.send(event);
                        }
                        None => break,
                    }
                }
            }
        });
    }

    /// Start a headless-capable runtime using an ephemeral network identity.
    ///
    /// The default Iroh preset (including relay behavior) is retained; bind port
    /// zero asks the operating system for an available port.
    pub async fn start(data_dir: impl AsRef<Path>) -> Result<Self> {
        let runtime_data_dir = data_dir.as_ref().to_path_buf();
        let profile_lock = ProfileLock::acquire(data_dir.as_ref())?;
        let storage = std::sync::Arc::new(Storage::open(data_dir.as_ref())?);
        let secret_key = iroh::SecretKey::generate();
        let message_store = crate::store::MessageStore::open(runtime_data_dir.join("message_store.db"))?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key.clone())
            .relay_mode(RelayMode::Default)
            .bind()
            .await?;
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .spawn();
        let (ui_event_tx, ui_events) = broadcast::channel(256);
        let (sequenced_event_tx, _) = broadcast::channel(256);
        let event_journal = crate::ui_events::UiEventJournal::new(
            format!(
                "{}:{}",
                endpoint.id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            4096,
        );
        Ok(Self {
            endpoint: Some(endpoint),
            router: Some(router),
            gossip,
            message_store,
            secret_key,
            room_senders: std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            storage,
            data_dir: runtime_data_dir,
            transfer_store: std::sync::Arc::new(
                crate::transfer_state_projection::TransferStateStore::new(256),
            ),
            ui_event_tx,
            ui_events,
            event_journal,
            sequenced_event_tx,
            profile_lock: Some(profile_lock),
            cancellation: CancellationToken::new(),
            workers: JoinSet::new(),
            stopped: false,
        })
    }

    /// Access the endpoint while the runtime is active.
    pub fn endpoint(&self) -> &Endpoint {
        self.endpoint.as_ref().expect("runtime is active")
    }

    /// Access the gossip service while the runtime is active.
    pub fn gossip(&self) -> &Gossip {
        &self.gossip
    }

    /// Return the shared SQLite message store.
    pub fn message_store(&self) -> crate::store::MessageStore { self.message_store.clone() }

    /// Build conversation commands over this runtime's stores and identity.
    pub fn application_commands(&self) -> ApplicationCommands {
        ApplicationCommands::new(ConversationApplicationService::new(
            self.message_store(), (*self.storage).clone(), self.secret_key.clone(), self.endpoint().id(),
        ))
    }

    /// Mark a conversation read and return its previous unread count.
    pub fn mark_read(&self, conversation_id: &[u8; 32]) -> n0_error::Result<Option<u32>> {
        self.message_store.mark_conversation_read(conversation_id)
    }

    /// Subscribe to a room once and retain an owned sender.
    pub async fn join_room(&self, conversation_id: [u8; 32]) -> Result<()> {
        let topic = crate::TopicId::from_bytes(conversation_id);
        let mut senders = self.room_senders.lock().await;
        if !senders.contains_key(&topic) {
            let sender = self.gossip.subscribe(topic, Vec::new()).await?.split().0;
            senders.insert(topic, sender);
        }
        Ok(())
    }

    /// Broadcast encoded protocol bytes to a room, joining it when needed.
    pub async fn broadcast_room(&self, conversation_id: [u8; 32], encoded: Vec<u8>) -> Result<()> {
        self.join_room(conversation_id).await?;
        let topic = crate::TopicId::from_bytes(conversation_id);
        let sender = self.room_senders.lock().await.get(&topic).cloned().expect("joined room sender");
        sender.broadcast(bytes::Bytes::from(encoded)).await?;
        Ok(())
    }

    /// Access the shared SQLite store.
    pub fn storage(&self) -> &std::sync::Arc<Storage> {
        &self.storage
    }

    /// Read the persisted conversation catalog.
    pub fn conversations(&self) -> crate::conversations::ConversationStore {
        crate::conversations::ConversationStore::load_from_sqlite(&self.storage, &self.data_dir)
    }

    /// Read current transfer projection records.
    pub fn transfer_snapshot(&self) -> Vec<crate::transfer_state_projection::TransferRecord> {
        self.transfer_store.snapshot()
    }

    /// Read the configured user profile, if available.
    pub fn profile_snapshot(&self) -> Option<crate::user_profile::UserProfile> {
        crate::user_profile::UserProfileStore::load(&self.data_dir, self.endpoint().id())
            .ok()
            .map(|store| store.profile().clone())
    }

    /// Subscribe to core persistence/UI events.
    pub fn subscribe_ui_events(&self) -> broadcast::Receiver<UiEvent> {
        self.ui_event_tx.subscribe()
    }

    /// Subscribe to live events for snapshot-then-replay reconciliation.
    pub fn subscribe_sequenced_ui_events(
        &self,
    ) -> broadcast::Receiver<crate::ui_events::SequencedUiEvent> {
        self.sequenced_event_tx.subscribe()
    }

    /// Read the revision watermark paired with the next authoritative snapshot.
    pub fn event_revision(&self) -> u64 {
        self.event_journal.revision()
    }

    /// Read a runtime-scoped cursor for snapshot/replay across reconnects.
    pub fn event_cursor(&self) -> crate::ui_events::EventCursor {
        self.event_journal.cursor()
    }

    /// Replay events from a cursor; epoch changes and overflow require resnapshot.
    pub fn replay_ui_events_from(
        &self,
        cursor: &crate::ui_events::EventCursor,
    ) -> crate::ui_events::ReplayResult {
        self.event_journal.replay_cursor(cursor)
    }

    /// Replay events after a snapshot watermark; overflow requires resnapshot.
    pub fn replay_ui_events_after(&self, revision: u64) -> crate::ui_events::ReplayResult {
        self.event_journal.replay_after(revision)
    }

    /// Publish a core state-change event to frontend subscribers.
    pub fn publish_ui_event(
        &self,
        event: UiEvent,
    ) -> Result<usize, broadcast::error::SendError<UiEvent>> {
        let item = self.event_journal.append(event.clone());
        let _ = self.sequenced_event_tx.send(item);
        self.ui_event_tx.send(event)
    }

    /// Receive events through this runtime's event-pump subscription.
    pub fn ui_events(&mut self) -> &mut broadcast::Receiver<UiEvent> {
        &mut self.ui_events
    }

    /// Cancellation token for runtime-owned background workers.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.child_token()
    }

    /// Spawn a persistent worker whose lifetime is tied to this runtime.
    /// The worker must observe its cancellation token and terminate promptly.
    pub fn spawn_worker<F, Fut>(&mut self, worker: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        assert!(!self.stopped, "cannot spawn workers after runtime shutdown");
        let token = self.cancellation.child_token();
        self.workers.spawn(worker(token));
    }

    /// Number of runtime-owned workers that have not yet been joined.
    pub fn active_worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Stop workers, router, and endpoint in order. Repeated calls are harmless.
    pub async fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let mut shutdown_error = None;
        // Stop accepting protocol traffic before cancelling and joining pumps.
        if let Some(router) = self.router.take() {
            if let Err(error) = router.shutdown().await {
                shutdown_error = Some(error);
            }
        }
        self.cancellation.cancel();
        while self.workers.join_next().await.is_some() {}
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.close().await;
        }
        // Unlock only after all network resources are released.
        self.profile_lock.take();
        if let Some(error) = shutdown_error {
            return Err(error.into());
        }
        Ok(())
    }
}

/// Every served blob request becomes an `Outbound` transfer record in the
/// live `TransferStateStore` (the "Peers Downloading from Me" panel) and a
/// privacy-filtered lifecycle event in the durable `transfer_activity`
/// table (the Activity Log's "By others" view). Progress checkpoints are fed
/// to the live store but intentionally NOT persisted, so a long upload cannot
/// flood the bounded activity table with thousands of rows.
pub fn spawn_outbound_provider_consumer(
    runtime: &tokio::runtime::Handle,
    rx: tokio::sync::mpsc::Receiver<iroh_blobs::provider::events::ProviderMessage>,
    store: std::sync::Arc<crate::transfer_state_projection::TransferStateStore>,
    item_labels: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    storage: Option<std::sync::Arc<crate::storage::Storage>>,
    local_public: iroh::PublicKey,
    cancel: Option<tokio_util::sync::CancellationToken>,
) {
    let task_handle = runtime.clone();
    task_handle.spawn(consume_outbound_provider_events(
        rx,
        store,
        item_labels,
        storage,
        local_public,
        cancel,
    ));
}

/// Drive the outbound blob-provider event stream to completion.
///
/// Split out from [`spawn_outbound_provider_consumer`] so runtime
/// construction can own the task through `DesktopRuntime::spawn_worker`
/// (cancellation-aware and joined on shutdown), while the legacy binary
/// startup path keeps a plain handle spawn with identical behaviour.
pub(crate) async fn consume_outbound_provider_events(
    mut rx: tokio::sync::mpsc::Receiver<iroh_blobs::provider::events::ProviderMessage>,
    store: std::sync::Arc<crate::transfer_state_projection::TransferStateStore>,
    item_labels: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    storage: Option<std::sync::Arc<crate::storage::Storage>>,
    local_public: iroh::PublicKey,
    cancel: Option<tokio_util::sync::CancellationToken>,
) {
    use crate::diagnostics::{event_names, TransferLifecycleEvent};
    use crate::transfer_state_projection::{EventName, TransferDirection, TransferEvent};
    use iroh_blobs::provider::events::{ProviderMessage, RequestUpdate};
    // Nested spawns inside the event handlers must not borrow the caller's
    // handle across the 'static task boundary.
    let task_handle = tokio::runtime::Handle::current();
    let mut peers: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    let mut transfers: std::collections::HashMap<(u64, u64), String> =
        std::collections::HashMap::new();
    loop {
        let msg = match recv_provider_event(&mut rx, cancel.as_ref()).await {
            Some(msg) => msg,
            None => break,
        };
        match msg {
            // The EventMask requests ConnectMode::Notify, so the plain
            // ClientConnected variant is not expected; both variants carry
            // the same fields (Notify<T> derefs to T).
            ProviderMessage::ClientConnected(msg) => {
                if let Some(peer) = msg.inner.endpoint_id {
                    peers.insert(msg.inner.connection_id, peer.to_string());
                }
            }
            // Notify wrapper derefs to the same ClientConnected payload.
            ProviderMessage::ClientConnectedNotify(msg) => {
                if let Some(peer) = msg.inner.endpoint_id {
                    peers.insert(msg.inner.connection_id, peer.to_string());
                }
            }
            ProviderMessage::ConnectionClosed(msg) => {
                if let Some(peer) = peers.get(&msg.inner.connection_id).cloned() {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    store.disconnect_peer(&peer, now_ms);
                }
                peers.remove(&msg.inner.connection_id);
                transfers.retain(|(conn, _), _| *conn != msg.inner.connection_id);
            }
            ProviderMessage::GetRequestReceivedNotify(msg) => {
                let key = (msg.inner.connection_id, msg.inner.request_id);
                let transfer_id = transfers
                    .entry(key)
                    .or_insert_with(|| {
                        format!("serve:{}-{}", msg.inner.connection_id, msg.inner.request_id)
                    })
                    .clone();
                let peer_id = peers.get(&msg.inner.connection_id).cloned();
                let mut update_rx = msg.rx;
                let store = store.clone();
                let item_labels = item_labels.clone();
                let storage = storage.clone();
                task_handle.spawn(async move {
                    let mut sequence = 0u64;
                    let mut current_hash: Option<String> = None;
                    let mut current_total: Option<u64> = None;
                    while let Ok(Some(update)) = update_rx.recv().await {
                        sequence += 1;
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let (kind, bytes, total_bytes, error): (
                            EventName,
                            u64,
                            Option<u64>,
                            Option<String>,
                        ) = match update {
                            RequestUpdate::Started(started) => {
                                let hash_hex = started.hash.to_string();
                                if let Ok(mut labels) = item_labels.lock() {
                                    labels.entry(hash_hex.clone()).or_insert_with(|| {
                                        storage
                                            .as_ref()
                                            .and_then(|stg| {
                                                stg.get_shared_file(
                                                    &local_public.to_string(),
                                                    &hash_hex,
                                                )
                                                .ok()
                                                .flatten()
                                            })
                                            .map(|row| row.display_filename)
                                            .unwrap_or_else(|| {
                                                let prefix: String =
                                                    hash_hex.chars().take(12).collect();
                                                format!("file {prefix}…")
                                            })
                                    });
                                }
                                current_hash = Some(hash_hex.clone());
                                current_total = Some(started.size);
                                (EventName::Started, 0, Some(started.size), None)
                            }
                            RequestUpdate::Progress(progress) => (
                                EventName::Progress,
                                progress.end_offset,
                                current_total,
                                None,
                            ),
                            RequestUpdate::Completed(completed) => (
                                EventName::Completed,
                                completed.stats.payload_bytes_sent,
                                None,
                                None,
                            ),
                            RequestUpdate::Aborted(_) => (
                                EventName::Failed,
                                0,
                                None,
                                Some("Transfer aborted before completion".to_string()),
                            ),
                        };
                        let event = TransferEvent {
                            event_id: format!("serve:{transfer_id}:{sequence}:{now_ms}"),
                            transfer_id: transfer_id.clone(),
                            item_id: current_hash.clone().unwrap_or_default(),
                            direction: TransferDirection::Outbound,
                            peer_id: peer_id.clone(),
                            sequence,
                            attempt: 1,
                            occurred_at_ms: now_ms,
                            kind: kind.clone(),
                            bytes,
                            total_bytes,
                            error: error.clone(),
                        };
                        let is_progress = matches!(kind, EventName::Progress);
                        store.publish(event);
                        // Durable activity: persist non-progress lifecycle
                        // points with direction=outbound. Progress is live
                        // only, so the bounded table stays useful.
                        if !is_progress {
                            let (event_name, payload) = match kind {
                                EventName::Started => (
                                    event_names::TRANSFER_STARTED,
                                    Some(serde_json::json!({
                                        "direction": "outbound",
                                        "total_bytes": current_total.unwrap_or(0),
                                    })),
                                ),
                                EventName::Completed => (
                                    event_names::COMPLETION,
                                    Some(serde_json::json!({
                                        "direction": "outbound",
                                        "bytes_transferred": bytes,
                                    })),
                                ),
                                EventName::Failed => (
                                    event_names::FAILURE,
                                    Some(serde_json::json!({
                                        "direction": "outbound",
                                        "error_category": "protocol_error",
                                        "reason": error.unwrap_or_default(),
                                    })),
                                ),
                                _ => (event_names::TRANSFER_STARTED, None),
                            };
                            if let Some(stg) = storage.as_ref() {
                                let _ = stg.record_transfer_activity(&TransferLifecycleEvent {
                                    schema_version: 1,
                                    event_id: format!("serve:{transfer_id}:{sequence}:{now_ms}"),
                                    event_name: event_name.to_string(),
                                    transfer_id: transfer_id.clone(),
                                    sequence,
                                    occurred_at_ms: now_ms,
                                    attempt: 1,
                                    payload,
                                });
                            }
                        }
                    }
                });
            }
            ProviderMessage::GetManyRequestReceivedNotify(msg) => {
                let key = (msg.inner.connection_id, msg.inner.request_id);
                let transfer_id = transfers
                    .entry(key)
                    .or_insert_with(|| {
                        format!("serve:{}-{}", msg.inner.connection_id, msg.inner.request_id)
                    })
                    .clone();
                let peer_id = peers.get(&msg.inner.connection_id).cloned();
                let mut update_rx = msg.rx;
                let store = store.clone();
                let item_labels = item_labels.clone();
                let storage = storage.clone();
                task_handle.spawn(async move {
                    let mut sequence = 0u64;
                    let mut current_hash: Option<String> = None;
                    let mut current_total: Option<u64> = None;
                    while let Ok(Some(update)) = update_rx.recv().await {
                        sequence += 1;
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let (kind, bytes, total_bytes, error): (
                            EventName,
                            u64,
                            Option<u64>,
                            Option<String>,
                        ) = match update {
                            RequestUpdate::Started(started) => {
                                let hash_hex = started.hash.to_string();
                                if let Ok(mut labels) = item_labels.lock() {
                                    labels.entry(hash_hex.clone()).or_insert_with(|| {
                                        storage
                                            .as_ref()
                                            .and_then(|stg| {
                                                stg.get_shared_file(
                                                    &local_public.to_string(),
                                                    &hash_hex,
                                                )
                                                .ok()
                                                .flatten()
                                            })
                                            .map(|row| row.display_filename)
                                            .unwrap_or_else(|| {
                                                let prefix: String =
                                                    hash_hex.chars().take(12).collect();
                                                format!("file {prefix}…")
                                            })
                                    });
                                }
                                current_hash = Some(hash_hex.clone());
                                current_total = Some(started.size);
                                (EventName::Started, 0, Some(started.size), None)
                            }
                            RequestUpdate::Progress(progress) => (
                                EventName::Progress,
                                progress.end_offset,
                                current_total,
                                None,
                            ),
                            RequestUpdate::Completed(completed) => (
                                EventName::Completed,
                                completed.stats.payload_bytes_sent,
                                None,
                                None,
                            ),
                            RequestUpdate::Aborted(_) => (
                                EventName::Failed,
                                0,
                                None,
                                Some("Transfer aborted before completion".to_string()),
                            ),
                        };
                        let event = TransferEvent {
                            event_id: format!("serve:{transfer_id}:{sequence}:{now_ms}"),
                            transfer_id: transfer_id.clone(),
                            item_id: current_hash.clone().unwrap_or_default(),
                            direction: TransferDirection::Outbound,
                            peer_id: peer_id.clone(),
                            sequence,
                            attempt: 1,
                            occurred_at_ms: now_ms,
                            kind: kind.clone(),
                            bytes,
                            total_bytes,
                            error: error.clone(),
                        };
                        let is_progress = matches!(kind, EventName::Progress);
                        store.publish(event);
                        // Durable activity: persist non-progress lifecycle
                        // points with direction=outbound. Progress is live
                        // only, so the bounded table stays useful.
                        if !is_progress {
                            let (event_name, payload) = match kind {
                                EventName::Started => (
                                    event_names::TRANSFER_STARTED,
                                    Some(serde_json::json!({
                                        "direction": "outbound",
                                        "total_bytes": current_total.unwrap_or(0),
                                    })),
                                ),
                                EventName::Completed => (
                                    event_names::COMPLETION,
                                    Some(serde_json::json!({
                                        "direction": "outbound",
                                        "bytes_transferred": bytes,
                                    })),
                                ),
                                EventName::Failed => (
                                    event_names::FAILURE,
                                    Some(serde_json::json!({
                                        "direction": "outbound",
                                        "error_category": "protocol_error",
                                        "reason": error.unwrap_or_default(),
                                    })),
                                ),
                                _ => (event_names::TRANSFER_STARTED, None),
                            };
                            if let Some(stg) = storage.as_ref() {
                                let _ = stg.record_transfer_activity(&TransferLifecycleEvent {
                                    schema_version: 1,
                                    event_id: format!("serve:{transfer_id}:{sequence}:{now_ms}"),
                                    event_name: event_name.to_string(),
                                    transfer_id: transfer_id.clone(),
                                    sequence,
                                    occurred_at_ms: now_ms,
                                    attempt: 1,
                                    payload,
                                });
                            }
                        }
                    }
                });
            }
            _ => {}
        }
    }
}

/// Await the next provider event, honouring an optional cancellation token.
///
/// `None` for `cancel` keeps the legacy binary behaviour exactly: the loop
/// runs until the provider event channel closes.
async fn recv_provider_event<T>(
    rx: &mut tokio::sync::mpsc::Receiver<T>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Option<T> {
    match cancel {
        None => rx.recv().await,
        Some(token) => tokio::select! {
            _ = token.cancelled() => None,
            msg = rx.recv() => msg,
        },
    }
}

/// Install the offline-replay callbacks on an inbox handle.
///
/// Single implementation shared by runtime construction (`start_with`) and the
/// legacy binary startup path, so the served-ID replay guard and the pending
/// page provider can never drift between the two.
///
/// - `set_record_sync_served_fn` records message IDs only after the requester
///   acknowledges each `SyncResponse`.
/// - `set_pending_fn` serves reconnect sync from the durable mailbox owner,
///   filtering out IDs already served via a previous response.
pub async fn install_inbox_replay_callbacks(
    handle: &crate::inbox::InboxHandle,
    data_dir: std::path::PathBuf,
) {
    use crate::inbox::{inbox_message_id, InboxMessageId};
    use crate::mailbox::{MailboxStore, MAX_SYNC_ENVELOPES};
    use std::collections::HashSet;

    let served_ids: std::sync::Arc<std::sync::Mutex<HashSet<InboxMessageId>>> =
        std::sync::Arc::new(std::sync::Mutex::new(HashSet::new()));

    let served_ids_for_record = served_ids.clone();
    handle
        .set_record_sync_served_fn(Some(std::sync::Arc::new(move |_peer, msg_ids| {
            let mut set = served_ids_for_record.lock().unwrap();
            for id in msg_ids {
                set.insert(*id);
            }
            Ok(())
        })))
        .await;

    let mailbox_data_dir = data_dir;
    let served_ids_for_filter = served_ids.clone();
    handle
        .set_pending_fn(Some(std::sync::Arc::new(move |requester, since_ms| {
            let mut page = MailboxStore::load(&mailbox_data_dir)
                .ok()
                .flatten()
                .map(|mut store| store.pending_for_recipient_since(requester, since_ms))
                .unwrap_or_default();
            // Filter out envelopes already served via a previous SyncResponse
            // (replay protection). The same inbox_message_id hash is used when
            // recording served IDs and when filtering, so dedup is consistent.
            let served = served_ids_for_filter.lock().unwrap();
            page.retain(|env| {
                let bytes = postcard::to_stdvec(env).expect("envelope encoding cannot fail");
                !served.contains(&inbox_message_id(&bytes))
            });
            drop(served);
            // Best-effort has_more signal; true pagination requires the
            // SQLite-backed provider.
            let has_more = page.len() >= MAX_SYNC_ENVELOPES;
            (page, has_more)
        })))
        .await;
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn runtime_exposes_shared_store_read_state_and_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = DesktopRuntime::start(dir.path()).await.expect("runtime starts");
        let first = runtime.message_store();
        let second = runtime.message_store();
        let conversation = [71u8; 32];
        let message_id = [72u8; 32];
        let sender = [73u8; 32];
        let local = *runtime.endpoint().id().as_bytes();
        first.insert_chat_message(
            &message_id, &conversation, &sender, 1, "text", "incoming", None, None, &local,
        ).unwrap();
        assert_eq!(second.get_conversation_meta(&conversation).unwrap().unwrap().last_message_preview, "incoming");
        assert_eq!(runtime.mark_read(&conversation).unwrap(), Some(1));

        let outcome = runtime.application_commands().send_message(crate::application_commands::SendMessage {
            idempotency_key: "runtime-send".into(),
            conversation_id: conversation,
            text: "outgoing".into(),
            thread_target: None,
        });
        let crate::application_commands::CommandOutcome::Applied(prepared) = outcome else {
            panic!("runtime command should admit message");
        };
        assert_eq!(second.get_conversation_meta(&conversation).unwrap().unwrap().last_message_preview, "outgoing");
        assert_eq!(runtime.storage().list_outgoing_for_topic(&crate::TopicId::from_bytes(conversation)).unwrap().len(), 1);
        assert!(!prepared.encoded.is_empty());
        runtime.shutdown().await.unwrap();
    }

    /// Source audit: the config-based production startup path must build the
    /// network identity and the SQLite store exactly once, and must leave the
    /// protocol graph construction to [`DesktopRuntime::start_with`].
    ///
    /// `DesktopRuntime::start` is the pre-existing ephemeral-identity helper for
    /// headless callers and is deliberately out of scope here.
    #[test]
    fn start_from_config_is_the_single_production_construction_site() {
        const SOURCE: &str = include_str!("desktop_runtime.rs");
        let production = SOURCE
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .unwrap_or(SOURCE);

        let config_path = production
            .split_once("pub async fn start_from_config(")
            .expect("start_from_config must exist")
            .1;
        let (config_body, graph) = config_path
            .split_once("pub async fn start_with(")
            .expect("start_with must follow start_from_config");
        let (graph_body, after_graph) = graph
            .split_once("fn pump_events<T>(")
            .expect("pump_events must follow start_with");
        let start_body = after_graph
            .split_once("pub async fn start(")
            .expect("start helper must follow event pump")
            .1
            .split_once("#[cfg(test)]")
            .map(|(body, _)| body)
            .unwrap_or(after_graph);

        for (path, body, expected_endpoint, expected_storage) in [
            ("start_from_config", config_body, 1, 1),
            ("start_with", graph_body, 0, 0),
            ("start", start_body, 1, 1),
        ] {
            assert_eq!(
                body.matches("Endpoint::builder(").count(),
                expected_endpoint,
                "{path} must contain exactly {expected_endpoint} Endpoint builder site(s)"
            );
            assert_eq!(
                body.matches("Storage::open(").count(),
                expected_storage,
                "{path} must contain exactly {expected_storage} Storage open site(s)"
            );
        }

        assert_eq!(
            config_body.matches("Self::start_with(parts)").count(),
            1,
            "start_from_config must delegate the protocol graph to start_with"
        );
        assert_eq!(
            graph_body.matches("Endpoint::builder(").count(),
            0,
            "start_with must not build another Endpoint"
        );
        assert_eq!(
            graph_body.matches("Storage::open(").count(),
            0,
            "start_with must not build another Storage"
        );

        // Discovery flags must reach endpoint construction rather than being
        // accepted and silently ignored by the config wrapper.
        assert!(
            config_body.contains("MdnsAddressLookup::builder()"),
            "enable_mdns must build an mDNS address lookup"
        );
        assert!(
            config_body.contains(".subscribe().await"),
            "the mDNS subscriber must be registered before bind"
        );
        assert!(
            config_body.contains("DhtAddressLookup::builder()"),
            "enable_dht must register a Mainline DHT lookup"
        );
        assert!(
            config_body.contains("AddrFilter::unfiltered()"),
            "publish_direct_addresses must select the unfiltered DHT address filter"
        );
    }
}
