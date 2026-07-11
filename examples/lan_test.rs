//! LAN interop test between two machines.
//!
//! Run on machine A (opener):
//!   cargo run --features examples --example lan_test -- --relay http://<local-ip>:3340 open
//!
//! Run on machine B (joiner):
//!   cargo run --features examples --example lan_test -- --relay http://<local-ip>:3340 join <ticket>
//!
//! The opener prints a ticket; the joiner reads it. Both print neighbor and
//! connection-type info as events arrive, then exit after 60s or when Ctrl-C is pressed.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, EndpointId,
    PublicKey, RelayMode, RelayUrl, SecretKey, TransportAddr,
};
use iroh_gossip::api::Event;
use iroh_gossip::chat_core::{check_peer_connection_type, ConnectionType, Message, SignedMessage};
use iroh_gossip::net::{Gossip, GOSSIP_ALPN};
use iroh_gossip::proto::TopicId;
use iroh_mainline_address_lookup::DhtAddressLookup;
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};

/// How long to wait for a peer to connect (seconds).
const CONNECT_TIMEOUT: f64 = 30.0;

#[derive(Parser, Debug)]
struct Args {
    /// Relay server URL (e.g., http://172.16.0.119:3340).
    #[clap(short, long, default_value = "https://relay1.iroh.network")]
    relay: String,

    /// Bind port for the iroh endpoint.
    #[clap(long, default_value = "0")]
    bind_port: u16,

    /// Human-readable name for this peer.
    #[clap(short, long, default_value = "lan-test")]
    name: String,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    /// Open a new room (creator).
    Open,
    /// Join an existing room via ticket (topic hex string).
    Join {
        ticket: String,
        /// Bootstrap peer endpoint ID (hex string of the opener).
        #[clap(short, long)]
        bootstrap: Option<String>,
    },
}

/// Tracks neighbor state and reports events.
#[derive(Debug)]
#[expect(dead_code)]
struct PeerState {
    local_pk: PublicKey,
    endpoint: Endpoint,
    neighbors: HashSet<EndpointId>,
    neighbor_order: Vec<EndpointId>,
    received_messages: Vec<String>,
    remote_pk_map: HashMap<EndpointId, PublicKey>,
}

impl PeerState {
    fn new(local_pk: PublicKey, endpoint: Endpoint) -> Self {
        Self {
            local_pk,
            endpoint,
            neighbors: HashSet::new(),
            neighbor_order: Vec::new(),
            received_messages: Vec::new(),
            remote_pk_map: HashMap::new(),
        }
    }

    fn on_neighbor_up(&mut self, peer_id: EndpointId) {
        if self.neighbors.insert(peer_id) {
            self.neighbor_order.push(peer_id);
            println!(
                "[NEIGHBOR_UP] {} (total: {})",
                fmt_id(&peer_id),
                self.neighbors.len()
            );
        }
    }

    fn on_neighbor_down(&mut self, peer_id: EndpointId) {
        if self.neighbors.remove(&peer_id) {
            println!(
                "[NEIGHBOR_DOWN] {} (total: {})",
                fmt_id(&peer_id),
                self.neighbors.len()
            );
        }
    }

    fn neighbor_list(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.neighbors.iter().map(fmt_id).collect();
        keys.sort();
        keys
    }

    async fn check_connections(&self) {
        for peer_id in &self.neighbors {
            // Convert EndpointId to PublicKey via the remote_pk_map
            if let Some(pk) = self.remote_pk_map.get(peer_id) {
                match check_peer_connection_type(&self.endpoint, *pk).await {
                    ConnectionType::Direct => {
                        println!("  [TRANSPORT] → {}: DIRECT", fmt_id(peer_id));
                    }
                    ConnectionType::Relayed => {
                        println!("  [TRANSPORT] → {}: RELAYED", fmt_id(peer_id));
                    }
                    ConnectionType::Unknown => {
                        println!("  [TRANSPORT] → {}: UNKNOWN", fmt_id(peer_id));
                    }
                }
            } else {
                // Try resolving anyway — check_peer_connection_type might work
                // by looking up the endpoint id
                println!(
                    "  [TRANSPORT] → {}: no public key mapping yet",
                    fmt_id(peer_id)
                );
            }
        }
        if self.neighbors.is_empty() {
            println!("  [TRANSPORT] No neighbors to check.");
        }
    }
}

fn fmt_id(id: &EndpointId) -> String {
    let s = id.to_string();
    if s.len() > 12 {
        format!("{}…{}", &s[..6], &s[s.len() - 6..])
    } else {
        s
    }
}

/// Create and bind an iroh endpoint.
async fn create_endpoint(relay_url_str: &str, bind_port: u16) -> Result<(Endpoint, SecretKey)> {
    let secret_key = SecretKey::generate();
    let url: RelayUrl = relay_url_str.parse().expect("valid relay URL");
    let relay_map = url.into();

    let ep = Endpoint::builder(presets::Minimal)
        .secret_key(secret_key.clone())
        .relay_mode(RelayMode::Custom(relay_map))
        .bind_addr(
            format!("0.0.0.0:{}", bind_port)
                .parse::<SocketAddr>()
                .unwrap(),
        )?
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .bind()
        .await?;

    ep.online().await;
    Ok((ep, secret_key))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Quiet down logging unless RUST_LOG is set
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warn");
    }
    tracing_subscriber::fmt::try_init().ok();

    let args = Args::parse();
    let relay_url = args.relay.trim_end_matches('/').to_string();

    println!("╔══════════════════════════════════════════╗");
    println!("║  iroh-gossip-chat LAN Interop Test      ║");
    println!("║  Relay: {:<34} ║", &relay_url);
    println!("║  Name:  {:<34} ║", &args.name);
    println!("╚══════════════════════════════════════════╝");

    let (endpoint, secret_key) = create_endpoint(&relay_url, args.bind_port).await?;
    let pk: PublicKey = secret_key.public();
    let eid: EndpointId = endpoint.id();
    println!("Public key:    {pk}");
    println!("Endpoint ID:   {eid}");

    // Add DHT address lookup for global peer discovery via Mainline DHT
    if let Ok(addr_lookup) = endpoint.address_lookup().as_ref() {
        if let Ok(dht_lookup) = DhtAddressLookup::builder().build() {
            addr_lookup.add(dht_lookup);
        }
    }

    let gossip = Gossip::builder().spawn(endpoint.clone());
    let _router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    let mut state = PeerState::new(pk, endpoint.clone());

    match &args.command {
        Command::Open => {
            let topic = TopicId::from_bytes(rand::random());
            println!("\n--- OPENING ROOM ---");
            println!("Topic: {topic}");
            println!("\nThe ticket to share is the topic hex string and the opener's endpoint ID.");
            println!("Paste it as: cargo run --features examples --example lan_test -- \\");
            println!("  --relay {relay_url} join {topic} --bootstrap {eid}");
            println!();

            let sub = gossip.subscribe(topic, vec![]).await?;
            let (sender, mut receiver) = sub.split();
            println!("Waiting for a peer to join (timeout: {CONNECT_TIMEOUT}s)...");

            let start = tokio::time::Instant::now();
            let deadline = Duration::from_secs_f64(CONNECT_TIMEOUT);
            let mut received_any = false;

            loop {
                if start.elapsed() > deadline {
                    println!("\n[WARN] Timeout waiting for peer.");
                    break;
                }

                tokio::select! {
                    biased;
                    msg = receiver.next() => {
                        match msg {
                            Some(Ok(event)) => {
                                match event {
                                    Event::NeighborUp(peer_id) => {
                                        state.on_neighbor_up(peer_id);
                                        state.check_connections().await;
                                        // Send hello
                                        let hello = Message::Message {
                                            text: format!("hello from {}", args.name),
                                        };
                                        let encoded = SignedMessage::sign_and_encode(&secret_key, &hello)?;
                                        sender.broadcast(encoded).await?;
                                        println!("  [SENT] hello from opener");
                                    }
                                    Event::NeighborDown(peer_id) => {
                                        state.on_neighbor_down(peer_id);
                                    }
                                    Event::Received(msg) => {
                                        let from = fmt_id(&msg.delivered_from);
                                        let display = match SignedMessage::verify_and_decode(&msg.content) {
                                            Ok((sender_pk, inner_msg, _sent_at)) => {
                                                state.remote_pk_map.entry(msg.delivered_from).or_insert(sender_pk);
                                                match inner_msg {
                                                    Message::Message { text } => {
                                                        format!("[msg from {}] {}", sender_pk.fmt_short(), text)
                                                    }
                                                    Message::AboutMe { name } => {
                                                        format!("[AboutMe from {}] {}", sender_pk.fmt_short(), name)
                                                    }
                                                    Message::Presence => "[Presence]".to_string(),
                                                    _ => "[other]".to_string(),
                                                }
                                            }
                                            Err(e) => {
                                                format!("[raw {} bytes] (decode error: {e})", msg.content.len())
                                            }
                                        };
                                        state.received_messages.push(display.clone());
                                        received_any = true;
                                        println!("  [RECEIVED from {from}] {display}");
                                    }
                                    Event::Lagged => {
                                        println!("  [LAGGED] receiver overflow");
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                eprintln!("  [ERR] {e}");
                            }
                            None => {
                                println!("  [STREAM ENDED]");
                                break;
                            }
                        }
                    }
                    _ = sleep(Duration::from_secs(5)) => {
                        let nl = state.neighbor_list().join(", ");
                        println!("  [STATUS] neighbors=[{nl}] msgs={}", state.received_messages.len());
                        state.check_connections().await;
                    }
                }
            }

            // Final report
            println!("\n═══════════ OPENER REPORT ═══════════");
            println!("  Topic:              {topic}");
            println!("  Relay URL:          {relay_url}");
            println!("  Local PK:           {pk}");
            println!("  Neighbors seen:     {}", state.neighbor_order.len());
            for (i, n) in state.neighbor_order.iter().enumerate() {
                println!("    [{i}] {n}");
                if let Some(peer_pk) = state.remote_pk_map.get(n) {
                    let conn = check_peer_connection_type(&endpoint, *peer_pk).await;
                    println!("         PK: {peer_pk}  conn: {conn:?}");
                }
            }
            println!("  Messages received:  {}", state.received_messages.len());
            for m in &state.received_messages {
                println!("    {m}");
            }
            println!(
                "  Messages exchanged: {}",
                if received_any { "YES ✓" } else { "NO ✗" }
            );
            println!("═══════════════════════════════════════");
        }
        Command::Join { ticket, bootstrap } => {
            let topic: TopicId = ticket.parse().expect("valid topic hex string");
            println!("\n--- JOINING ROOM ---");
            println!("Topic: {topic}");

            // Parse bootstrap peers and seed the address lookup
            let bootstrap_peers: Vec<EndpointId> = if let Some(b) = bootstrap {
                let id: EndpointId = b.parse().expect("valid endpoint ID hex");

                // Seed the address lookup with the bootstrap peer's relay URL
                // so the gossip layer can resolve the endpoint ID to a relay address.
                let memory_lookup = MemoryLookup::new();
                let relay_url: RelayUrl = relay_url.parse().expect("valid relay URL");
                let addr = EndpointAddr::from_parts(id, [TransportAddr::Relay(relay_url)]);
                memory_lookup.add_endpoint_info(addr);
                if let Ok(als) = endpoint.address_lookup() {
                    als.add(memory_lookup);
                } else {
                    eprintln!("  [WARN] no address lookup services on endpoint");
                }
                println!(
                    "  [ADDR_LOOKUP] seeded bootstrap {} with relay {}",
                    fmt_id(&id),
                    args.relay
                );

                vec![id]
            } else {
                vec![]
            };

            // Subscribe with optional bootstrap peers.
            // We use subscribe() + manual joined() so we can capture
            // the first NeighborUp that subscribe_and_join would consume.
            let mut sub = gossip.subscribe(topic, bootstrap_peers.clone()).await?;
            // Wait for at least one connection to form, but record it
            // in our state so neighbor tracking is correct.
            if !bootstrap_peers.is_empty() {
                // joined() blocks until the first connection; after this
                // the consumed NeighborUp won't arrive in our event stream,
                // so we manually add the bootstrap peer as "seen".
                sub.joined().await?;
                println!("  [JOINED] connected to topic, waiting for peers...");
                // The bootstrap peer's endpoint ID was consumed by joined();
                // we can't add it to state because we don't know the peer's
                // PublicKey yet. Message events will map it for us.
            }
            let (sender, mut receiver) = sub.split();

            // Broadcast AboutMe so the opener can map us
            let about = Message::AboutMe {
                name: args.name.clone(),
            };
            let encoded = SignedMessage::sign_and_encode(&secret_key, &about)?;
            sender.broadcast(encoded).await?;
            println!("  [SENT AboutMe]");

            println!("Waiting for peer (timeout: {CONNECT_TIMEOUT}s)...");
            let start = tokio::time::Instant::now();
            let deadline = Duration::from_secs_f64(CONNECT_TIMEOUT);
            let mut received_any = false;

            loop {
                if start.elapsed() > deadline {
                    println!("\n[WARN] Timeout waiting for peer.");
                    break;
                }

                tokio::select! {
                    biased;
                    msg = receiver.next() => {
                        match msg {
                            Some(Ok(event)) => {
                                match event {
                                    Event::NeighborUp(peer_id) => {
                                        state.on_neighbor_up(peer_id);
                                        state.check_connections().await;
                                        // Reply
                                        let reply = Message::Message {
                                            text: format!("hello from {}", args.name),
                                        };
                                        let encoded = SignedMessage::sign_and_encode(&secret_key, &reply)?;
                                        sender.broadcast(encoded).await?;
                                        println!("  [SENT] hello from joiner");
                                    }
                                    Event::NeighborDown(peer_id) => {
                                        state.on_neighbor_down(peer_id);
                                    }
                                    Event::Received(msg) => {
                                        let from = fmt_id(&msg.delivered_from);
                                        let display = match SignedMessage::verify_and_decode(&msg.content) {
                                            Ok((sender_pk, inner_msg, _sent_at)) => {
                                                state.remote_pk_map.entry(msg.delivered_from).or_insert(sender_pk);
                                                match inner_msg {
                                                    Message::Message { text } => {
                                                        format!("[msg from {}] {}", sender_pk.fmt_short(), text)
                                                    }
                                                    Message::AboutMe { name } => {
                                                        format!("[AboutMe from {}] {}", sender_pk.fmt_short(), name)
                                                    }
                                                    Message::Presence => "[Presence]".to_string(),
                                                    _ => "[other]".to_string(),
                                                }
                                            }
                                            Err(e) => {
                                                format!("[raw {} bytes] (decode error: {e})", msg.content.len())
                                            }
                                        };
                                        state.received_messages.push(display.clone());
                                        received_any = true;
                                        println!("  [RECEIVED from {from}] {display}");
                                    }
                                    Event::Lagged => {
                                        println!("  [LAGGED] receiver overflow");
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                eprintln!("  [ERR] {e}");
                            }
                            None => {
                                println!("  [STREAM ENDED]");
                                break;
                            }
                        }
                    }
                    _ = sleep(Duration::from_secs(5)) => {
                        let nl = state.neighbor_list().join(", ");
                        println!("  [STATUS] neighbors=[{nl}] msgs={}", state.received_messages.len());
                    }
                }
            }

            // Final report
            println!("\n═══════════ JOINER REPORT ═══════════");
            println!("  Topic:              {topic}");
            println!("  Relay URL:          {relay_url}");
            println!("  Local PK:           {pk}");
            println!("  Neighbors seen:     {}", state.neighbor_order.len());
            for (i, n) in state.neighbor_order.iter().enumerate() {
                println!("    [{i}] {n}");
                if let Some(peer_pk) = state.remote_pk_map.get(n) {
                    let conn = check_peer_connection_type(&endpoint, *peer_pk).await;
                    println!("         PK: {peer_pk}  conn: {conn:?}");
                }
            }
            println!("  Messages received:  {}", state.received_messages.len());
            for m in &state.received_messages {
                println!("    {m}");
            }
            println!(
                "  Messages exchanged: {}",
                if received_any { "YES ✓" } else { "NO ✗" }
            );
            println!("═══════════════════════════════════════");
        }
    }

    sleep(Duration::from_secs(2)).await;
    println!("\nDone.");
    Ok(())
}
