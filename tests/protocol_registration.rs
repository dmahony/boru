//! Characterization tests for the network protocol router assembled by the GUI.
//!
//! The production startup path lives in the `boru` example rather than in the
//! library.  These tests therefore pin both sides of that boundary: the source
//! registration list, and Iroh's runtime behaviour when all registered ALPNs
//! are installed on one router.

#![cfg(feature = "net")]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use iroh::{
    endpoint::presets,
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint,
};

/// A minimal handler used to exercise the same Router registration/startup
/// path as the application.  It deliberately returns immediately: successful
/// admission is the behaviour this characterization test needs to observe.
#[derive(Clone, Debug)]
struct CountingHandler {
    accepted: Arc<AtomicUsize>,
}

impl CountingHandler {
    fn new() -> Self {
        Self {
            accepted: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn count(&self) -> usize {
        self.accepted.load(Ordering::Acquire)
    }
}

impl ProtocolHandler for CountingHandler {
    async fn accept(&self, _connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        self.accepted.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

const REGISTERED_PROTOCOLS: &[(&str, &[u8])] = &[
    ("gossip", boru_core::net::GOSSIP_ALPN),
    ("blobs", iroh_blobs::ALPN),
    (
        "friend ping",
        boru_core::chat_core::friend_ping::FRIEND_PING_ALPN,
    ),
    ("backfill", boru_core::backfill::BACKFILL_ALPN),
    ("whisper", boru_core::whisper::WHISPER_ALPN),
    ("inbox", boru_core::inbox::INBOX_ALPN),
    (
        "catalogue",
        boru_core::protocol_version::CATALOGUE_ALPN,
    ),
    ("file access", boru_core::net::FILE_ACCESS_ALPN),
    ("tunnel", boru_core::tunnel::BORU_TUNNEL_ALPN),
];

#[test]
fn gui_startup_registers_every_existing_protocol() {
    // Keep this as a source characterization rather than duplicating the GUI's
    // large startup dependency graph in an integration test.  A missing entry
    // here would make an otherwise compiling new protocol silently unreachable.
    let startup = include_str!("../examples/iced_chat/main.rs");
    let registrations = [
        ".accept(GOSSIP_ALPN,",
        ".accept(iroh_blobs::ALPN,",
        ".accept(FRIEND_PING_ALPN,",
        ".accept(BACKFILL_ALPN,",
        ".accept(WHISPER_ALPN,",
        ".accept(INBOX_ALPN,",
        ".accept(CATALOGUE_ALPN,",
        ".accept(boru_core::net::FILE_ACCESS_ALPN,",
        ".accept(BORU_TUNNEL_ALPN,",
    ];

    for registration in registrations {
        assert!(
            startup.contains(registration),
            "GUI startup router is missing registration: {registration}"
        );
    }
    assert!(
        startup.contains(".spawn();")
            && startup.contains("splash_send(\"Protocol router ready\")"),
        "GUI startup must spawn the protocol router before reporting readiness"
    );

    assert_eq!(
        registrations.len(),
        REGISTERED_PROTOCOLS.len(),
        "update this test when a protocol is intentionally added or removed"
    );
}

#[tokio::test]
async fn router_starts_and_routes_each_registered_alpn() -> anyhow::Result<()> {
    let listener = Endpoint::bind(presets::Minimal).await?;
    let client = Endpoint::bind(presets::Minimal).await?;

    let handlers: Vec<(&str, &[u8], CountingHandler)> = REGISTERED_PROTOCOLS
        .iter()
        .map(|(name, alpn)| (*name, *alpn, CountingHandler::new()))
        .collect();

    let mut router_builder = Router::builder(listener);
    for (_, alpn, handler) in &handlers {
        router_builder = router_builder.accept(*alpn, handler.clone());
    }
    let router = router_builder.spawn();

    for (name, alpn, _) in &handlers {
        client
            .connect(router.endpoint().addr(), *alpn)
            .await
            .map_err(|error| anyhow::anyhow!("connect {name} protocol: {error:?}"))?;
    }

    for (name, _, handler) in &handlers {
        tokio::time::timeout(Duration::from_secs(2), async {
            while handler.count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("{name} handler did not accept its connection"))?;
    }

    router.shutdown().await?;
    client.close().await;
    Ok(())
}
