//! GUI-independent owner for Boru's transport, persistence, and core event pump.
//!
//! The desktop frontend may consume the returned handles, but this module has no
//! dependency on any GUI toolkit.

use std::{fs::File, path::Path};

use anyhow::{Context, Result};
use fs2::FileExt;
use iroh::{endpoint::presets, Endpoint, RelayMode};
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{
    net::{Gossip, GOSSIP_ALPN},
    storage::Storage,
    ui_events::UiEvent,
};

/// Headless-capable core transport and persistence runtime.
#[allow(missing_debug_implementations)]
pub struct DesktopRuntime {
    endpoint: Option<Endpoint>,
    router: Option<iroh::protocol::Router>,
    gossip: Gossip,
    storage: std::sync::Arc<Storage>,
    ui_event_tx: broadcast::Sender<UiEvent>,
    ui_events: broadcast::Receiver<UiEvent>,
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
    /// Construct and start the production protocol graph on the supplied endpoint.
    pub async fn start_with(parts: DesktopRuntimeParts) -> Result<(Self, DesktopRuntimeHandles)> {
        // RESIDUAL BRT-008b: src/bin/boru/main.rs:1228-1277 — inbox replay/pending callbacks close over process-local served-ID state and data_dir; inject the mailbox provider and replay guard or move callback wiring with runtime construction.
        // RESIDUAL BRT-008b: src/bin/boru/main.rs:1833-1848 — inbox authorization callback closes over the live SQLite Storage and data_dir; supply these dependencies or move callback wiring with runtime construction.
        // RESIDUAL BRT-008b: src/bin/boru/main.rs:1135-1142 — blob provider event consumer requires runtime.handle(), transfer_store, outbound_item_labels, Storage, and local_public; move task ownership together or pass each dependency and retain its lifetime.
        // RESIDUAL BRT-008b: src/bin/boru/main.rs:2235-2247 — screen-share channels are adopted into Iced calls_state here; protocol construction/registration can move, but Iced-specific receiver adoption remains in main until a runtime result is wired to app.
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
            BackfillProtocolHandler::new(message_store, storage.clone(), local_public);
        let backfill_handle = crate::backfill::BackfillHandle::spawn(endpoint.clone());
        let whisper_builder = WhisperBuilder::new(endpoint.clone(), secret_key.clone());
        let whisper_handler = whisper_builder.protocol_handler();
        let (whisper_handle, whisper_events) = whisper_builder.spawn();
        let (inbox_handle, inbox_events) = InboxHandle::new();
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
        let call_builder = crate::call::manager::CallBuilder::new(endpoint.clone(), secret_key);
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
        let mut runtime = Self {
            endpoint: Some(endpoint),
            router: Some(router),
            gossip,
            storage,
            ui_event_tx,
            ui_events,
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
        let tx = runtime.ui_event_tx.clone();
        let mut provider_events = provider_events;
        runtime.spawn_worker(move |cancel| async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    event = provider_events.recv() => match event {
                        Some(event) => { let _ = tx.send(UiEvent::ProtocolEvent { source: "blob", detail: format!("{event:?}") }); }
                        None => break,
                    }
                }
            }
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
        self.spawn_worker(move |cancel| async move {
            let mut receiver = receiver.lock().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    event = receiver.recv() => match event {
                        Some(event) => { let _ = tx.send(UiEvent::ProtocolEvent { source, detail: format!("{event:?}") }); }
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
        let profile_lock = ProfileLock::acquire(data_dir.as_ref())?;
        let storage = std::sync::Arc::new(Storage::open(data_dir.as_ref())?);
        let endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Default)
            .bind()
            .await?;
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .spawn();
        let (ui_event_tx, ui_events) = broadcast::channel(256);
        Ok(Self {
            endpoint: Some(endpoint),
            router: Some(router),
            gossip,
            storage,
            ui_event_tx,
            ui_events,
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

    /// Access the shared SQLite store.
    pub fn storage(&self) -> &std::sync::Arc<Storage> {
        &self.storage
    }

    /// Subscribe to core persistence/UI events.
    pub fn subscribe_ui_events(&self) -> broadcast::Receiver<UiEvent> {
        self.ui_event_tx.subscribe()
    }

    /// Publish a core state-change event to frontend subscribers.
    pub fn publish_ui_event(
        &self,
        event: UiEvent,
    ) -> Result<usize, broadcast::error::SendError<UiEvent>> {
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
