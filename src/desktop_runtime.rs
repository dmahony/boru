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

    /// Stop workers, router, and endpoint in order. Repeated calls are harmless.
    pub async fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        self.cancellation.cancel();
        while self.workers.join_next().await.is_some() {}
        let mut shutdown_error = None;
        if let Some(router) = self.router.take() {
            if let Err(error) = router.shutdown().await {
                shutdown_error = Some(error);
            }
        }
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
