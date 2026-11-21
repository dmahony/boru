//! GUI-independent owner for Boru's transport, persistence, and core event pump.
//!
//! The desktop frontend may consume the returned handles, but this module has no
//! dependency on any GUI toolkit.

use std::path::Path;

use anyhow::Result;
use iroh::{endpoint::presets, Endpoint, RelayMode};
use tokio::sync::broadcast;

use crate::{net::{Gossip, GOSSIP_ALPN}, storage::Storage, ui_events::UiEvent};

/// Headless-capable core transport and persistence runtime.
#[allow(missing_debug_implementations)]
pub struct DesktopRuntime {
    endpoint: Option<Endpoint>,
    router: Option<iroh::protocol::Router>,
    gossip: Gossip,
    storage: std::sync::Arc<Storage>,
    ui_event_tx: broadcast::Sender<UiEvent>,
    ui_events: broadcast::Receiver<UiEvent>,
}

impl DesktopRuntime {
    /// Start a headless-capable runtime using an ephemeral network identity.
    ///
    /// The default Iroh preset (including relay behavior) is retained; bind port
    /// zero asks the operating system for an available port.
    pub async fn start(data_dir: impl AsRef<Path>) -> Result<Self> {
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
    pub fn publish_ui_event(&self, event: UiEvent) -> Result<usize, broadcast::error::SendError<UiEvent>> {
        self.ui_event_tx.send(event)
    }

    /// Receive events through this runtime's event-pump subscription.
    pub fn ui_events(&mut self) -> &mut broadcast::Receiver<UiEvent> {
        &mut self.ui_events
    }

    /// Stop the protocol router before closing the transport endpoint.
    pub async fn stop(mut self) -> Result<()> {
        if let Some(router) = self.router.take() {
            router.shutdown().await?;
        }
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.close().await;
        }
        Ok(())
    }
}
