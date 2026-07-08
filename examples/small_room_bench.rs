//! Small-room latency benchmark harness.
//!
//! Spawns N peers, connects them in a small room, sends a configurable number
//! of test messages, and reports per-peer latency statistics.
//!
//! Usage:
//!   cargo run --example small_room_bench --features net -- --peers 5 --messages 20
//!
//! All peers run in the same process (same machine), so `Instant` timestamps
//! are directly comparable -- this gives true one-way latency measurements
//! without clock-skew issues.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use iroh::{endpoint::presets, protocol::Router, Endpoint, PublicKey, RelayMode, SecretKey};
use iroh_gossip::small_room::{
    room_size_fits_small_room, SmallRoomBuilder, SmallRoomEvent, SmallRoomHandle, SMALL_ROOM_ALPN,
    SMALL_ROOM_MAX_SIZE,
};
use n0_error::Result;
use tokio::sync::Mutex;

// -- CLI -----------------------------------------------------------------------

#[derive(Parser, Debug)]
struct Args {
    #[clap(long, default_value = "3")]
    peers: usize,

    #[clap(long, default_value = "10")]
    messages: usize,

    #[clap(long, default_value = "1000")]
    settle_ms: u64,

    /// Only peer 0 broadcasts; all other peers only listen.
    #[clap(long)]
    single_sender: bool,
}

// -- Node state ----------------------------------------------------------------

struct NodeState {
    router: Router,
    handle: SmallRoomHandle,
    sent_count: Arc<Mutex<usize>>,
    recv_count: Arc<Mutex<usize>>,
    latencies: Arc<Mutex<HashMap<PublicKey, Vec<Duration>>>>,
}

// -- Main ----------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let num_peers = args.peers.clamp(2, 10);
    let num_msgs = args.messages;

    // Integration hook: check whether this room size fits the small-room
    // protocol.  For room sizes ≤ SMALL_ROOM_MAX_SIZE, use the direct-connect
    // small-room module instead of the gossip broadcast tree.
    if !room_size_fits_small_room(num_peers) {
        eprintln!(
            "WARNING: {num_peers} peers exceeds SMALL_ROOM_MAX_SIZE ({SMALL_ROOM_MAX_SIZE}); \
             large-room fallback to gossip is not implemented in this harness."
        );
    }

    eprintln!(
        "=== Small-Room Latency Benchmark ===\n\
         Peers: {num_peers}, Messages per peer: {num_msgs}\n"
    );

    // -- Spawn peers --------------------------------------------------------

    let mut nodes: Vec<NodeState> = Vec::with_capacity(num_peers);
    let mut peer_addrs = Vec::with_capacity(num_peers);
    let mut peer_keys = Vec::with_capacity(num_peers);

    for i in 0..num_peers {
        let secret_key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key.clone())
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;

        let builder = SmallRoomBuilder::new(endpoint.clone(), secret_key.clone());
        let handler = builder.protocol_handler();
        let (handle, event_rx) = builder.spawn().await;

        let router = Router::builder(endpoint.clone())
            .accept(SMALL_ROOM_ALPN, handler)
            .spawn();

        let sent_count = Arc::new(Mutex::new(0));
        let recv_count = Arc::new(Mutex::new(0));
        let latencies: Arc<Mutex<HashMap<PublicKey, Vec<Duration>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let recv_count_c = recv_count.clone();
        let latencies_c = latencies.clone();
        tokio::task::spawn(handle_events(event_rx, recv_count_c, latencies_c));

        eprintln!("  [{i}] peer {} bound", secret_key.public().fmt_short());

        peer_addrs.push(endpoint.addr());
        peer_keys.push(secret_key.public());

        nodes.push(NodeState {
            router,
            handle,
            sent_count,
            recv_count,
            latencies,
        });
    }

    // -- Connect all peers (star from peer 0) -------------------------------

    eprintln!("\n  Connecting peers...");
    for i in 1..num_peers {
        nodes[0].handle.connect_to(peer_addrs[i].clone()).await?;
        eprintln!("    peer 0 -> peer {i}: connected");
    }

    let settle = Duration::from_millis(args.settle_ms);
    eprintln!("\n  Settling for {} ms...", settle.as_millis());
    tokio::time::sleep(settle).await;

    // -- Check connectivity -------------------------------------------------

    eprintln!("\n  Connected peers per node:");
    for (i, node) in nodes.iter().enumerate() {
        let peers = node.handle.connected_peers().await;
        eprintln!("    [{i}] {} connected peer(s)", peers.len());
    }

    // -- Broadcast messages -------------------------------------------------
    let senders: Vec<usize> = if args.single_sender {
        vec![0]
    } else {
        (0..num_peers).collect()
    };

    let total_messages = num_msgs * senders.len();
    eprintln!(
        "  Broadcasting {num_msgs} messages from {} sender(s)...",
        senders.len()
    );
    let start = Instant::now();

    for m in 0..num_msgs {
        for &i in &senders {
            let text = format!("msg-{i}-{m}");
            nodes[i].handle.broadcast(text).await?;
            *nodes[i].sent_count.lock().await += 1;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let broadcast_duration = start.elapsed();
    eprintln!(
        "  Broadcast took {:?} (avg {:?} per msg)",
        broadcast_duration,
        broadcast_duration / (num_msgs * num_peers) as u32
    );

    // Allow messages to arrive
    tokio::time::sleep(Duration::from_secs(2)).await;

    // -- Report statistics --------------------------------------------------

    eprintln!("\n=== Results ===");

    for (i, node) in nodes.iter().enumerate() {
        let latencies = node.latencies.lock().await;
        let total_recv: usize = *node.recv_count.lock().await;
        eprintln!(
            "  [{i}] {} received: {total_recv}",
            peer_keys[i].fmt_short()
        );

        if latencies.is_empty() {
            eprintln!("        No latency data collected");
            continue;
        }

        for (peer_key, samples) in latencies.iter() {
            if samples.is_empty() {
                continue;
            }
            let min_ns = samples.iter().map(|d| d.as_nanos()).min().unwrap_or(0);
            let max_ns = samples.iter().map(|d| d.as_nanos()).max().unwrap_or(0);
            let avg_ns = samples.iter().map(|d| d.as_nanos()).sum::<u128>() / samples.len() as u128;

            let p50 = percentile_ns(samples, 50);
            let p95 = percentile_ns(samples, 95);
            let p99 = percentile_ns(samples, 99);

            eprintln!(
                "        -> {}  samples={}  min={}us  avg={}us  max={}us  p50={}us  p95={}us  p99={}us",
                peer_key.fmt_short(),
                samples.len(),
                min_ns / 1000,
                avg_ns / 1000,
                max_ns / 1000,
                p50 / 1000,
                p95 / 1000,
                p99 / 1000,
            );
        }
    }

    // -- Shutdown -----------------------------------------------------------

    eprintln!("\n  Shutting down...");
    for node in &nodes {
        let _ = node.router.shutdown().await;
    }

    eprintln!("  Done.");
    Ok(())
}

// -- Event handler -------------------------------------------------------------

async fn handle_events(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<SmallRoomEvent>,
    recv_count: Arc<Mutex<usize>>,
    latencies: Arc<Mutex<HashMap<PublicKey, Vec<Duration>>>>,
) {
    while let Some(event) = rx.recv().await {
        match event {
            SmallRoomEvent::Message {
                from,
                text: _,
                sent_at,
                received_at,
            } => {
                *recv_count.lock().await += 1;
                if received_at > sent_at {
                    let latency = received_at.duration_since(sent_at);
                    let mut map = latencies.lock().await;
                    map.entry(from).or_default().push(latency);
                }
            }
            SmallRoomEvent::Closed => break,
            SmallRoomEvent::Error(e) => {
                eprintln!("  error: {e}");
            }
            _ => {}
        }
    }
}

fn percentile_ns(samples: &[Duration], p: u8) -> u128 {
    if samples.is_empty() {
        return 0;
    }
    let mut ns: Vec<u128> = samples.iter().map(|d| d.as_nanos()).collect();
    ns.sort_unstable();
    let idx = ((ns.len() - 1) * p as usize) / 100;
    ns[idx]
}
