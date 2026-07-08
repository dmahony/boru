//! Tor transport scaffolding for the iroh-gossip redesign.
//!
//! This module defines Tor-specific address and ticket types plus a Tor-backed
//! custom transport for iroh's unstable custom transport API.

use std::{fmt, str::FromStr};

use iroh_base::{CustomAddr, EndpointAddr, PublicKey, TransportAddr};
use n0_error::{bail_any, AnyError, Result, StdResultExt};
#[cfg(feature = "tor-transport")]
use n0_watcher::Watchable;
use serde::{Deserialize, Serialize};
#[cfg(feature = "tor-transport")]
use tor_rtcompat::PreferredRuntime;

use crate::proto::TopicId;

/// Transport id reserved for the Tor-backed ticket/address format used by this crate.
///
/// This is only a local convention for now; once the custom transport is implemented
/// and stabilized, it can be registered formally if desired.
pub const TOR_TRANSPORT_ID: u64 = 0x746f725f63686174; // "tor_chat"

/// A peer address that is meant to be reached via Tor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorPeerAddr {
    /// Endpoint identity of the peer.
    endpoint_id: PublicKey,
    /// The .onion hostname.
    onion: String,
    /// The onion-service port.
    port: u16,
}

impl TorPeerAddr {
    /// Create a new Tor peer address.
    pub fn new(endpoint_id: PublicKey, onion: impl Into<String>, port: u16) -> Result<Self> {
        let onion = onion.into();
        if !onion.ends_with(".onion") {
            bail_any!("Tor peer addresses must use a .onion hostname");
        }
        Ok(Self {
            endpoint_id,
            onion,
            port,
        })
    }

    /// Return the endpoint id.
    pub fn endpoint_id(&self) -> PublicKey {
        self.endpoint_id
    }

    /// Return the .onion hostname.
    pub fn onion(&self) -> &str {
        &self.onion
    }

    /// Return the port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Return the iroh endpoint address for this peer.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        EndpointAddr::from_parts(
            self.endpoint_id,
            [TransportAddr::Custom(self.to_custom_addr())],
        )
    }

    /// Encode this address into an opaque iroh custom address.
    pub fn to_custom_addr(&self) -> CustomAddr {
        let mut data = Vec::with_capacity(2 + self.onion.len());
        data.extend_from_slice(&self.port.to_be_bytes());
        data.extend_from_slice(self.onion.as_bytes());
        CustomAddr::from_parts(TOR_TRANSPORT_ID, &data)
    }

    /// Decode a Tor peer address from an opaque iroh custom address and an endpoint id.
    pub fn from_custom_addr(endpoint_id: PublicKey, addr: &CustomAddr) -> Result<Self> {
        let (onion, port) = Self::decode_custom_addr(addr)?;
        Self::new(endpoint_id, onion, port)
    }

    fn decode_custom_addr(addr: &CustomAddr) -> Result<(String, u16)> {
        if addr.id() != TOR_TRANSPORT_ID {
            bail_any!("unexpected transport id for Tor peer address");
        }
        let data = addr.data();
        if data.len() < 2 {
            bail_any!("Tor peer address payload is too short");
        }
        let port = u16::from_be_bytes(data[..2].try_into().expect("length checked"));
        let onion = std::str::from_utf8(&data[2..])
            .std_context("decode Tor peer onion hostname")?
            .to_string();
        if !onion.ends_with(".onion") {
            bail_any!("Tor peer addresses must use a .onion hostname");
        }
        Ok((onion, port))
    }
}

impl fmt::Display for TorPeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.onion, self.port)
    }
}

/// A Tor-native ticket that can be exchanged between peers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorTicket {
    /// Topic this ticket belongs to.
    pub topic: TopicId,
    /// Tor-backed peer addresses that participate in this topic.
    pub peers: Vec<TorPeerAddr>,
}

impl TorTicket {
    /// Deserializes from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).std_context("decode Tor ticket")
    }

    /// Serializes to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("postcard::to_stdvec is infallible")
    }
}

impl fmt::Display for TorTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = data_encoding::BASE32_NOPAD.encode(&self.to_bytes()[..]);
        text.make_ascii_lowercase();
        write!(f, "{text}")
    }
}

impl FromStr for TorTicket {
    type Err = AnyError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .std_context("decode Tor ticket base32")?;
        Self::from_bytes(&bytes)
    }
}

#[cfg(feature = "tor-transport")]
mod tor_transport_impl {
    use super::*;
    use std::{
        env, fs,
        io::{self, IoSliceMut},
        net::{Ipv4Addr, SocketAddr},
        num::NonZeroUsize,
        path::PathBuf,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
        time::Duration,
    };

    use arti_client::{
        config::{TorClientConfig, TorClientConfigBuilder},
        BootstrapBehavior, TorClient,
    };
    use futures::{Stream, StreamExt};
    use iroh::{
        endpoint::transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo},
        PublicKey,
    };
    use noq_udp::RecvMeta;
    use safelog::DisplayRedacted;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
    use tor_cell::relaycell::msg::Connected;
    use tor_hsservice::{
        config::OnionServiceConfigBuilder, handle_rend_requests, HsNickname, StreamRequest,
    };

    #[derive(Debug, Clone)]
    struct OutgoingPacket {
        dst: CustomAddr,
        src: CustomAddr,
        payload: Vec<u8>,
    }

    #[derive(Debug, Clone)]
    struct IncomingPacket {
        src: CustomAddr,
        payload: Vec<u8>,
    }

    /// Tor-backed custom transport factory.
    #[derive(Clone)]
    pub struct TorTransport {
        endpoint_id: PublicKey,
        tor_client: Arc<TorClient<PreferredRuntime>>,
        service_port: u16,
        local_peer_addr: Watchable<Option<TorPeerAddr>>,
    }

    impl fmt::Debug for TorTransport {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TorTransport")
                .field("endpoint_id", &self.endpoint_id)
                .field("service_port", &self.service_port)
                .finish_non_exhaustive()
        }
    }

    impl TorTransport {
        /// Create a new Tor custom transport factory.
        pub fn new(
            endpoint_id: PublicKey,
            tor_client: Arc<TorClient<PreferredRuntime>>,
            service_port: u16,
        ) -> Self {
            Self {
                endpoint_id,
                tor_client,
                service_port: if service_port == 0 { 80 } else { service_port },
                local_peer_addr: Watchable::new(None),
            }
        }

        /// Watch the local Tor peer address once the onion service has been launched.
        pub fn watch_local_peer_addr(&self) -> n0_watcher::Direct<Option<TorPeerAddr>> {
            self.local_peer_addr.watch()
        }

        fn nickname(&self) -> Result<HsNickname> {
            let short = &self.endpoint_id.as_bytes()[..4];
            let suffix = u32::from_be_bytes(short.try_into().expect("slice length checked"));
            HsNickname::try_from(format!("iroh{:08x}", suffix)).std_context("build onion nickname")
        }

        fn local_custom_addr(&self, onion: &str) -> Result<CustomAddr> {
            let peer = TorPeerAddr::new(self.endpoint_id, onion, self.service_port)?;
            Ok(peer.to_custom_addr())
        }
    }

    struct TorEndpoint {
        local_addrs: n0_watcher::Watchable<Vec<CustomAddr>>,
        local_custom_addr: CustomAddr,
        sender: Arc<TorSender>,
        recv_rx: Mutex<UnboundedReceiver<IncomingPacket>>,
        _service: Arc<tor_hsservice::RunningOnionService>,
    }

    impl fmt::Debug for TorEndpoint {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TorEndpoint")
                .field("local_custom_addr", &self.local_custom_addr)
                .finish_non_exhaustive()
        }
    }

    #[derive(Clone)]
    struct TorSender {
        local_custom_addr: CustomAddr,
        tx: UnboundedSender<OutgoingPacket>,
    }

    impl fmt::Debug for TorSender {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TorSender")
                .field("local_custom_addr", &self.local_custom_addr)
                .finish_non_exhaustive()
        }
    }

    impl CustomTransport for TorTransport {
        fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
            let nickname = self
                .nickname()
                .map_err(|e| io::Error::other(e.to_string()))?;
            let onion_config = OnionServiceConfigBuilder::default()
                .nickname(nickname)
                .build()
                .map_err(|e| io::Error::other(e.to_string()))?;

            let launched = self
                .tor_client
                .launch_onion_service(onion_config)
                .map_err(|e| io::Error::other(e.to_string()))?
                .ok_or_else(|| io::Error::other("tor onion service disabled"))?;
            let (service, rend_requests) = launched;
            let onion = service
                .onion_address()
                .ok_or_else(|| io::Error::other("tor onion service has no address"))?;
            let onion = onion.display_unredacted().to_string();
            let local_custom_addr = self
                .local_custom_addr(&onion)
                .map_err(|e| io::Error::other(e.to_string()))?;

            tracing::info!(
                endpoint = %self.endpoint_id.fmt_short(),
                onion = %onion,
                port = self.service_port,
                "tor onion service launched"
            );

            let service_for_watch = Arc::clone(&service);
            let local_peer_addr = self.local_peer_addr.clone();
            let endpoint_id = self.endpoint_id;
            let onion_for_watch = onion.clone();
            let service_port = self.service_port;
            tokio::spawn(async move {
                tracing::debug!(
                    endpoint = %endpoint_id.fmt_short(),
                    onion = %onion_for_watch,
                    port = service_port,
                    "waiting for tor onion service to become reachable"
                );
                let mut status_events = service_for_watch.status_events();
                while !service_for_watch.status().state().is_fully_reachable() {
                    if status_events.next().await.is_none() {
                        break;
                    }
                }
                let peer_addr = TorPeerAddr::new(endpoint_id, onion_for_watch, service_port)
                    .expect("valid Tor peer addr");
                tracing::info!(
                    endpoint = %endpoint_id.fmt_short(),
                    peer = %peer_addr,
                    "tor onion service is reachable"
                );
                let _ = local_peer_addr.set(Some(peer_addr));
            });

            let local_addrs = n0_watcher::Watchable::new(vec![local_custom_addr.clone()]);
            let (incoming_tx, incoming_rx) = unbounded_channel();
            let (outgoing_tx, outgoing_rx) = unbounded_channel();
            let sender = Arc::new(TorSender {
                local_custom_addr: local_custom_addr.clone(),
                tx: outgoing_tx,
            });

            tokio::spawn(run_rendezvous_loop(
                rend_requests,
                incoming_tx.clone(),
                local_custom_addr.clone(),
            ));
            tokio::spawn(run_outgoing_loop(
                Arc::clone(&self.tor_client),
                local_custom_addr.clone(),
                outgoing_rx,
            ));

            Ok(Box::new(TorEndpoint {
                local_addrs,
                local_custom_addr,
                sender,
                recv_rx: Mutex::new(incoming_rx),
                _service: service,
            }))
        }
    }

    impl CustomEndpoint for TorEndpoint {
        fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
            self.local_addrs.watch()
        }

        fn create_sender(&self) -> Arc<dyn CustomSender> {
            self.sender.clone()
        }

        fn poll_recv(
            &mut self,
            cx: &mut Context,
            bufs: &mut [IoSliceMut<'_>],
            metas: &mut [RecvMeta],
            recv_infos: &mut [RecvInfo],
        ) -> Poll<io::Result<usize>> {
            assert_eq!(bufs.len(), metas.len(), "non matching bufs & metas");
            assert_eq!(
                bufs.len(),
                recv_infos.len(),
                "non matching bufs & recv_infos"
            );
            if bufs.is_empty() {
                return Poll::Ready(Ok(0));
            }

            let mut guard = self.recv_rx.lock().expect("poisoned");
            match Pin::new(&mut *guard).poll_recv(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(Ok(0)),
                Poll::Ready(Some(packet)) => {
                    if packet.payload.len() > bufs[0].len() {
                        return Poll::Ready(Err(io::Error::other(
                            "Tor packet does not fit into receive buffer",
                        )));
                    }
                    let len = packet.payload.len();
                    bufs[0][..len].copy_from_slice(&packet.payload);
                    let mut meta = RecvMeta::default();
                    meta.addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
                    meta.len = len;
                    meta.stride = len;
                    metas[0] = meta;
                    recv_infos[0] = RecvInfo::new(packet.src, Some(self.local_custom_addr.clone()));
                    Poll::Ready(Ok(1))
                }
            }
        }

        fn max_transmit_segments(&self) -> NonZeroUsize {
            NonZeroUsize::MIN
        }
    }

    impl CustomSender for TorSender {
        fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
            addr.id() == TOR_TRANSPORT_ID
        }

        fn poll_send(
            &self,
            _cx: &mut Context,
            dst: &CustomAddr,
            src: Option<&CustomAddr>,
            transmit: &iroh::endpoint::transports::Transmit<'_>,
        ) -> Poll<io::Result<()>> {
            if !self.is_valid_send_addr(dst) {
                return Poll::Ready(Err(io::Error::other("invalid Tor destination address")));
            }
            let src = src
                .cloned()
                .unwrap_or_else(|| self.local_custom_addr.clone());
            tracing::debug!(dst = %dst, src = %src, bytes = transmit.contents.len(), "queueing tor transport packet");
            let packet = OutgoingPacket {
                dst: dst.clone(),
                src,
                payload: transmit.contents.to_vec(),
            };
            self.tx.send(packet).map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "Tor sender channel closed")
            })?;
            Poll::Ready(Ok(()))
        }
    }

    async fn run_outgoing_loop(
        tor_client: Arc<TorClient<PreferredRuntime>>,
        _local_custom_addr: CustomAddr,
        mut rx: UnboundedReceiver<OutgoingPacket>,
    ) {
        while let Some(packet) = rx.recv().await {
            if let Err(err) = send_packet(&tor_client, packet).await {
                tracing::warn!(error = %err, "tor transport outgoing send failed");
            }
        }
    }

    async fn send_packet(
        tor_client: &TorClient<PreferredRuntime>,
        packet: OutgoingPacket,
    ) -> io::Result<()> {
        let (onion, port) = TorPeerAddr::decode_custom_addr(&packet.dst)
            .map_err(|e| io::Error::other(e.to_string()))?;
        tracing::debug!(dst = %packet.dst, onion = %onion, port, bytes = packet.payload.len(), "opening tor connection for packet");
        let mut stream = tor_client
            .connect((onion.as_str(), port))
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        write_frame(&mut stream, &packet.src, &packet.payload).await?;
        AsyncWriteExt::shutdown(&mut stream)
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        tracing::debug!(dst = %packet.dst, "tor packet sent");
        Ok(())
    }

    async fn write_frame(
        stream: &mut (impl tokio::io::AsyncWrite + Unpin),
        src: &CustomAddr,
        payload: &[u8],
    ) -> io::Result<()> {
        let src_bytes = src.to_vec();
        let src_len: u16 = src_bytes
            .len()
            .try_into()
            .map_err(|_| io::Error::other("custom source address too large"))?;
        let payload_len: u32 = payload
            .len()
            .try_into()
            .map_err(|_| io::Error::other("Tor packet too large"))?;
        stream.write_all(&src_len.to_be_bytes()).await?;
        stream.write_all(&src_bytes).await?;
        stream.write_all(&payload_len.to_be_bytes()).await?;
        stream.write_all(payload).await?;
        stream.flush().await?;
        Ok(())
    }

    async fn run_rendezvous_loop(
        rend_requests: impl Stream<Item = tor_hsservice::RendRequest>,
        incoming_tx: UnboundedSender<IncomingPacket>,
        local_custom_addr: CustomAddr,
    ) {
        let mut stream_requests = Box::pin(handle_rend_requests(rend_requests));
        while let Some(stream_request) = stream_requests.as_mut().next().await {
            let incoming_tx = incoming_tx.clone();
            let local_custom_addr = local_custom_addr.clone();
            tokio::spawn(async move {
                if let Err(err) =
                    handle_stream_request(stream_request, incoming_tx, local_custom_addr).await
                {
                    tracing::warn!(error = %err, "tor transport incoming receive failed");
                }
            });
        }
    }

    async fn handle_stream_request(
        stream_request: StreamRequest,
        incoming_tx: UnboundedSender<IncomingPacket>,
        local_custom_addr: CustomAddr,
    ) -> io::Result<()> {
        tracing::debug!(peer = %local_custom_addr, "accepting tor rendezvous stream");
        let mut stream = stream_request
            .accept(Connected::new_empty())
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        let (src, payload) = read_frame(&mut stream).await?;
        tracing::debug!(src = %src, bytes = payload.len(), "received tor packet");
        let _ = incoming_tx.send(IncomingPacket { src, payload });
        let _ = local_custom_addr;
        Ok(())
    }

    async fn read_frame(
        stream: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> io::Result<(CustomAddr, Vec<u8>)> {
        let mut len_buf = [0u8; 2];
        stream.read_exact(&mut len_buf).await?;
        let src_len = u16::from_be_bytes(len_buf) as usize;
        let mut src_bytes = vec![0u8; src_len];
        stream.read_exact(&mut src_bytes).await?;
        let src =
            CustomAddr::from_bytes(&src_bytes).map_err(|err| io::Error::other(err.to_string()))?;
        let mut payload_len_buf = [0u8; 4];
        stream.read_exact(&mut payload_len_buf).await?;
        let payload_len = u32::from_be_bytes(payload_len_buf) as usize;
        let mut payload = vec![0u8; payload_len];
        stream.read_exact(&mut payload).await?;
        Ok((src, payload))
    }

    // ── Tor bootstrap / reconnection helpers (exponential backoff) ─────

    /// Compute exponential backoff delay for the given attempt number.
    ///
    /// Starts at 1 second, doubles each attempt, capped at `max_delay`.
    pub fn backoff_delay(attempt: u32, max_delay: Duration) -> Duration {
        let secs = 1u64 << attempt.saturating_sub(1).min(30);
        Duration::from_secs(secs).min(max_delay)
    }

    /// Storage directories for the Tor client state and cache.
    #[derive(Debug)]
    pub struct TorStorageDirs {
        #[allow(dead_code)]
        root: PathBuf,
        state_dir: PathBuf,
        cache_dir: PathBuf,
    }

    impl TorStorageDirs {
        /// Create temporary storage directories for a Tor client session.
        pub fn new() -> Result<Self> {
            let root = env::temp_dir().join(format!(
                "iroh-gossip-chat-tor-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            let state_dir = root.join("state");
            let cache_dir = root.join("cache");
            fs::create_dir_all(&state_dir)?;
            fs::create_dir_all(&cache_dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
                fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
                fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o700))?;
            }
            Ok(Self {
                root,
                state_dir,
                cache_dir,
            })
        }

        /// Return the state directory path.
        pub fn state_dir(&self) -> &PathBuf {
            &self.state_dir
        }

        /// Return the cache directory path.
        pub fn cache_dir(&self) -> &PathBuf {
            &self.cache_dir
        }
    }

    /// Build a [`TorClientConfig`] from the given storage directories.
    pub fn tor_client_config(tor_dirs: &TorStorageDirs) -> Result<TorClientConfig> {
        TorClientConfigBuilder::from_directories(tor_dirs.state_dir(), tor_dirs.cache_dir())
            .build()
            .std_context("build Arti Tor client config")
    }

    /// Format a Tor bootstrap status line for display.
    pub fn format_tor_bootstrap_status_line(status: impl std::fmt::Display) -> String {
        format!("> Tor bootstrap status: {status}")
    }

    /// Print a Tor bootstrap status line, deduplicating consecutive identical lines.
    pub fn print_tor_bootstrap_status(
        status: impl std::fmt::Display,
        last_rendered: &mut Option<String>,
    ) {
        let rendered = format_tor_bootstrap_status_line(status);
        if last_rendered.as_deref() != Some(rendered.as_str()) {
            println!("{rendered}");
            *last_rendered = Some(rendered);
        }
    }

    /// Bootstrap Tor with exponential backoff on failure.
    ///
    /// Retries the bootstrap process when:
    /// - The bootstrap task itself returns an error
    /// - Bootstrap completes but Tor is not ready for traffic
    ///
    /// Uses exponential backoff starting at 1s, doubling each attempt,
    /// capped at 120s. Prints status updates after each failed attempt.
    pub async fn bootstrap_tor(
        tor_dirs: &TorStorageDirs,
    ) -> Result<(Arc<TorClient<PreferredRuntime>>, String)> {
        let tor_config = tor_client_config(tor_dirs)?;
        let tor_client = TorClient::builder()
            .config(tor_config)
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped_async()
            .await
            .anyerr()?;

        let max_retries = 10u32;
        for attempt in 1..=max_retries {
            let mut last_bootstrap_status = None;
            print_tor_bootstrap_status(tor_client.bootstrap_status(), &mut last_bootstrap_status);
            let mut bootstrap_events = tor_client.bootstrap_events();
            let mut bootstrap_task = {
                let tor_client = Arc::clone(&tor_client);
                tokio::spawn(async move { tor_client.bootstrap().await })
            };
            let mut bootstrap_task_done = false;

            let result = 'inner: loop {
                if tor_client.bootstrap_status().ready_for_traffic() {
                    break 'inner Ok(());
                }

                if bootstrap_task_done {
                    match bootstrap_events.next().await {
                        Some(status) => {
                            print_tor_bootstrap_status(status, &mut last_bootstrap_status);
                            continue;
                        }
                        None => {
                            break 'inner Err("bootstrap event stream ended unexpectedly".to_string());
                        }
                    }
                }

                tokio::select! {
                    biased;
                    result = &mut bootstrap_task => {
                        match result {
                            Ok(Ok(())) => {
                                bootstrap_task_done = true;
                                print_tor_bootstrap_status(tor_client.bootstrap_status(), &mut last_bootstrap_status);
                            }
                            Ok(Err(err)) => {
                                break 'inner Err(format!("bootstrap task failed: {err:#}"));
                            }
                            Err(err) => {
                                break 'inner Err(format!("join bootstrap task: {err}"));
                            }
                        }
                    }
                    maybe_status = bootstrap_events.next() => {
                        if let Some(status) = maybe_status {
                            print_tor_bootstrap_status(status, &mut last_bootstrap_status);
                        }
                    }
                }
            };

            match result {
                Ok(()) if tor_client.bootstrap_status().ready_for_traffic() => {
                    return Ok((tor_client, "> Tor is ready.".to_string()));
                }
                Ok(()) => {
                    println!(
                        "> Tor bootstrap attempt {attempt}/{max_retries}: completed but not ready, retrying..."
                    );
                }
                Err(msg) => {
                    println!(
                        "> Tor bootstrap attempt {attempt}/{max_retries} failed: {msg}"
                    );
                }
            }

            if attempt < max_retries {
                let delay = backoff_delay(attempt, Duration::from_secs(120));
                println!("> Retrying Tor bootstrap in {}s...", delay.as_secs());
                tokio::time::sleep(delay).await;
            }
        }

        bail_any!(
            "Tor bootstrap failed after {max_retries} attempts — check your Tor network connectivity"
        )
    }

    /// Background task that monitors Tor client health and reconnects with
    /// exponential backoff if the Tor connection drops.
    ///
    /// Checks every 30 seconds whether Tor is still ready for traffic.
    /// When a drop is detected, re-bootstraps using exponential backoff
    /// (1s base, 120s cap) and sends status updates through `status_tx`.
    pub async fn monitor_tor_health(
        tor_client: Arc<TorClient<PreferredRuntime>>,
        status_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) {
        let mut check_interval = tokio::time::interval(Duration::from_secs(30));
        check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = check_interval.tick() => {
                    if !tor_client.bootstrap_status().ready_for_traffic() {
                        let _ = status_tx.send("⚠ Tor connection lost. Reconnecting...".to_string());

                        for attempt in 1u32.. {
                            match tor_client.bootstrap().await {
                                Ok(()) if tor_client.bootstrap_status().ready_for_traffic() => {
                                    let _ = status_tx.send("✓ Tor reconnected successfully.".to_string());
                                    break;
                                }
                                Ok(()) => {
                                    let _ = status_tx.send(
                                        format!("Tor re-bootstrap attempt {attempt}: completed but not ready, retrying...")
                                    );
                                }
                                Err(err) => {
                                    let _ = status_tx.send(
                                        format!("Tor re-bootstrap attempt {attempt} failed: {err:#}")
                                    );
                                }
                            }
                            let delay = backoff_delay(attempt, Duration::from_secs(120));
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "tor-transport")]
pub use tor_transport_impl::TorTransport;

#[cfg(feature = "tor-transport")]
pub use tor_transport_impl::{
    backoff_delay, bootstrap_tor, format_tor_bootstrap_status_line, monitor_tor_health,
    print_tor_bootstrap_status, tor_client_config, TorStorageDirs,
};

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    #[test]
    fn tor_peer_addr_roundtrips_via_custom_addr() {
        let endpoint_id = SecretKey::generate().public();
        let peer = TorPeerAddr::new(endpoint_id, "examplehiddenservice.onion", 9735)
            .expect("valid .onion address");
        let encoded = peer.to_custom_addr();
        let decoded = TorPeerAddr::from_custom_addr(endpoint_id, &encoded).expect("roundtrip");
        assert_eq!(decoded, peer);
        assert_eq!(peer.endpoint_addr().id, endpoint_id);
    }

    #[test]
    fn tor_ticket_roundtrips_through_base32() {
        let endpoint_a = SecretKey::generate().public();
        let endpoint_b = SecretKey::generate().public();
        let ticket = TorTicket {
            topic: TopicId::from_bytes([9u8; 32]),
            peers: vec![
                TorPeerAddr::new(endpoint_a, "examplehiddenservice.onion", 9735).unwrap(),
                TorPeerAddr::new(endpoint_b, "secondhiddenservice.onion", 9977).unwrap(),
            ],
        };
        let encoded = ticket.to_string();
        let decoded = TorTicket::from_str(&encoded).expect("decode Tor ticket");
        assert_eq!(decoded, ticket);
    }
}
