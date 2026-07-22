//! Manual live Mainline DHT test harness.
//!
//! Connects to the real Mainline BitTorrent DHT and verifies that the
//! boru-chat public-room discovery system can publish and lookup records.
//!
//! # WARNING — Firewall and UDP caveats
//!
//! The Mainline DHT uses **UDP** on a random ephemeral port (by default
//! the OS-assigned port range).  Most corporate, ISP, and home router
//! firewalls block or rate-limit outbound UDP traffic to unknown ports,
//! and many block UDP entirely.
//!
//! **If DHT operations fail (timeout, no peers discovered):**
//!   - Check that your outbound UDP is not blocked by a corporate firewall,
//!     VPN, or NAT policy.
//!   - Some environments (shared hosting, Docker, certain VPS providers)
//!     drop all non-DNS UDP traffic — DHT will not work there.
//!   - If you are behind a symmetric NAT, DHT put/get may succeed but
//!     external peers may not find your published records.
//!   - Running multiple DHT test instances on the same host is fine — each
//!     creates its own ephemeral UDP socket.
//!
//! # Usage
//!
//! ```text
//! # Run the full publish + discover cycle (development namespace):
//! cargo run --example dht_harness --features net
//!
//! # Discover only (skip publish — useful for quick connectivity checks):
//! cargo run --example dht_harness --features net -- --discover-only
//!
//! # Publish only (skip discovery):
//! cargo run --example dht_harness --features net -- --publish-only
//!
//! # Use the test namespace (even more isolated than development):
//! cargo run --example dht_harness --features net -- --network test
//!
//! # Custom DHT timeout (seconds, default 10):
//! cargo run --example dht_harness --features net -- --timeout 20
//!
//! # Verbose tracing output:
//! RUST_LOG=debug cargo run --example dht_harness --features net
//! ```
//!
//! # How it works
//!
//! 1. Creates a real `mainline::Dht` client that joins the global Mainline
//!    DHT network via UDP.
//! 2. Wraps it in a `MainlineDhtBackend` using the **Development** network
//!    namespace (never touches the production Mainnet public-lobby).
//! 3. Generates a fresh, ephemeral `SecretKey` (never stored to disk).
//! 4. Creates a `PublicRoomTracker` and calls `publish_once()` to write a
//!    discovery record to the DHT.
//! 5. Calls `discover_once()` to read back records from the same namespace.
//! 6. Prints a safe diagnostic summary (peer count, timing, no secrets).
//!
//! # Safety
//!
//! - Records are published under the **Development** discovery key, which
//!   is computed deterministically but shares no bits with the Mainnet
//!   public-lobby key.  Production records are never touched.
//! - The `--network test` flag uses the Test key for even stronger isolation.
//! - The ephemeral `SecretKey` is dropped at exit; no disk state is modified.
//! - Printed diagnostics show only counts, timing, and short peer IDs —
//!   never full secret keys, discovery keys, or raw DHT payloads.

use std::time::{Duration, Instant};

use clap::Parser;
use iroh::{EndpointId, SecretKey};
use n0_error::{bail_any, Result, StdResultExt};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use boru_core::discovery_backend::MainlineDhtBackend;
use boru_core::public_room::{public_room_identity, PublicNetwork};
use boru_core::public_room_tracker::PublicRoomTracker;
use distributed_topic_tracker::{Dht, DhtConfig};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "dht_harness",
    about = "Manual live Mainline DHT test harness for boru-chat",
    version,
    long_about = None
)]
struct Args {
    /// Only publish, skip discovery.
    #[arg(long)]
    publish_only: bool,

    /// Only discover, skip publishing.
    #[arg(long)]
    discover_only: bool,

    /// DHT timeout in seconds for get/put operations (default: 10).
    #[arg(long, default_value = "10")]
    timeout: u64,

    /// Network namespace to use: "development" (default) or "test".
    ///
    /// Both are isolated from the production Mainnet public-lobby.
    #[arg(long, default_value = "development")]
    network: String,
}

// ---------------------------------------------------------------------------
// Diagnostic helpers
// ---------------------------------------------------------------------------

/// Format a duration as a human-readable string.
fn fmt_duration(d: Duration) -> String {
    let total_ms = d.as_millis();
    if total_ms < 1000 {
        format!("{total_ms}ms")
    } else if total_ms < 60_000 {
        format!("{:.1}s", total_ms as f64 / 1000.0)
    } else {
        format!("{:.1}m", total_ms as f64 / 60_000.0)
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // ── Logging ─────────────────────────────────────────────────────
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    let args = Args::parse();
    let start = Instant::now();

    if args.publish_only && args.discover_only {
        bail_any!("Cannot set both --publish-only and --discover-only");
    }

    // ── Determine network ───────────────────────────────────────────
    let network = match args.network.to_lowercase().as_str() {
        "development" => PublicNetwork::Development,
        "test" => PublicNetwork::Test,
        other => bail_any!("Unknown network '{other}'. Use 'development' (default) or 'test'."),
    };
    let network_label = match network {
        PublicNetwork::Development => "development",
        PublicNetwork::Test => "test",
        _ => unreachable!(),
    };
    let identity = public_room_identity(network);

    let short_id = identity.short_id();
    println!("─── boru-chat DHT Test Harness ──────────────────────");
    println!("  Network:      {network_label}");
    println!("  Room ID:      {short_id}");
    println!();

    // ── Create DHT client ───────────────────────────────────────────
    println!("  [1/4] Connecting to Mainline DHT...");
    let dht_config = DhtConfig::builder()
        .get_timeout(Duration::from_secs(args.timeout))
        .put_timeout(Duration::from_secs(args.timeout))
        .build();
    let dht = Dht::new(&dht_config);
    println!("  └─ DHT client created (timeout: {}s)", args.timeout);
    println!();

    // ── Create backend ──────────────────────────────────────────────
    println!("  [2/4] Creating MainlineDhtBackend...");
    let backend = MainlineDhtBackend::new(dht);
    println!("  └─ Backend ready");
    println!();

    // ── Generate ephemeral identity ─────────────────────────────────
    let sk = SecretKey::generate();
    let ep: EndpointId = sk.public();
    println!("  [3/4] Ephemeral identity generated");
    println!("  └─ EndpointId: {}", ep.fmt_short());
    println!();

    // ── Create tracker ──────────────────────────────────────────────
    let tracker = PublicRoomTracker::start(Box::new(backend), network, ep, sk)
        .await
        .std_context("Failed to start PublicRoomTracker")?;
    println!("  └─ Tracker started: {short_id}");

    // ── Publish ─────────────────────────────────────────────────────
    let do_publish = !args.discover_only;
    if do_publish {
        let t = Instant::now();
        match tracker.publish_once().await {
            Ok(()) => {
                let elapsed = t.elapsed();
                println!("  ✓ Published record ({})", fmt_duration(elapsed));
            }
            Err(e) => {
                println!("  ✗ Publish failed: {e}",);
                println!("    └─ This is expected if UDP is blocked or the DHT is unreachable.");
            }
        }
    } else {
        println!("  ─ Skipping publish (--discover-only)");
    }
    println!();

    // ── Discover ────────────────────────────────────────────────────
    let do_discover = !args.publish_only;
    if do_discover {
        let t = Instant::now();
        match tracker.discover_once().await {
            Ok(peers) => {
                let elapsed = t.elapsed();
                let count = peers.len();
                print!("  ✓ Discovered {count} peer(s) ({})", fmt_duration(elapsed));

                if count > 0 {
                    // Print short IDs of discovered peers.
                    let short_ids: Vec<String> =
                        peers.iter().map(|p| p.fmt_short().to_string()).collect();
                    print!(": {}", short_ids.join(", "));
                }
                println!();
            }
            Err(e) => {
                println!("  ✗ Discover failed: {e}",);
                println!(
                    "    └─ This is expected if UDP is blocked or no other test peers are running."
                );
            }
        }
    } else {
        println!("  ─ Skipping discovery (--publish-only)");
    }
    println!();

    // ── Summary ─────────────────────────────────────────────────────
    let total = fmt_duration(start.elapsed());
    println!("─── Done in {total} ─────────────────────────────────");
    println!();
    println!("  DHT operations can take several seconds to complete.");
    println!("  If both publish and discover succeeded, the DHT is");
    println!("  reachable and the public-room system is functional.");
    println!();
    println!("  UDP caveats:");
    println!("    • Corporate firewalls often block non-DNS UDP.");
    println!("    • Docker / VPS may drop DHT traffic.");
    println!("    • Symmetric NAT peers may not be reachable.");
    println!("    • Timeouts are normal on slow or congested networks.");
    println!();
    println!("  For a deterministic test without Mainline DHT, run:");
    println!("    cargo test --features net public_lobby_integration");

    // ── Shutdown ────────────────────────────────────────────────────
    tracker.shutdown().await;
    Ok(())
}
