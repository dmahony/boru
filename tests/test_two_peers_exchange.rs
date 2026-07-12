//! Test: can two peers exchange messages via gossip?
//! Uses relay mode, waits for both to join, sends, asserts delivery.

use boru_chat::{
    api::{Event as GossipEvent, GossipTopic},
    chat_core::{Message, SignedMessage},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, RelayMode,
    SecretKey,
};
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};
use std::time::Duration;

fn make_sk(rng: &mut impl rand::Rng) -> SecretKey {
    SecretKey::from_bytes(&rng.random())
}

async fn spawn_peer_relay(
    rng: &mut impl rand::Rng,
) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0)
        .secret_key(make_sk(rng))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    ep.online().await;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip))
}

/// Non-blocking drain of events from a topic subscription.
/// Returns after `timeout` waiting for the first event.
async fn drain_events(sub: &mut GossipTopic, timeout: Duration) -> Vec<GossipEvent> {
    let mut events = Vec::new();
    loop {
        let item = tokio::time::timeout(timeout, sub.next()).await;
        match item {
            Ok(Some(Ok(ev))) => events.push(ev),
            Ok(Some(Err(e))) => {
                eprintln!("  drain error: {e}");
                break;
            }
            _ => break, // timeout or stream ended
        }
    }
    events
}

#[tokio::test]
async fn test_two_peers_exchange_messages() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    let (router_a, ep_a, sk_a, gossip_a) = spawn_peer_relay(&mut rng).await?;
    let (router_b, ep_b, _sk_b, gossip_b) = spawn_peer_relay(&mut rng).await?;

    println!("Peer A: {}", ep_a.id().fmt_short());
    println!("Peer B: {}", ep_b.id().fmt_short());

    let topic = TopicId::from_bytes(rng.random());
    println!("Topic: {topic}");

    // Both subscribe to the same topic
    let mut sub_a = gossip_a.subscribe(topic, vec![ep_a.id()]).await?;
    println!("A subscribed");
    sleep(Duration::from_millis(100)).await;
    let mut sub_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;
    println!("B subscribed");

    // Wait for peer discovery
    let short = Duration::from_millis(50);
    for i in 0..30 {
        let ev_a = drain_events(&mut sub_a, short).await;
        let ev_b = drain_events(&mut sub_b, short).await;
        let a_joined = sub_a.is_joined();
        let b_joined = sub_b.is_joined();
        if !ev_a.is_empty() || !ev_b.is_empty() {
            println!(
                "  tick {i}: A joined={a_joined}{} B joined={b_joined}{}",
                if ev_a.is_empty() {
                    String::new()
                } else {
                    format!(" ev={ev_a:?}")
                },
                if ev_b.is_empty() {
                    String::new()
                } else {
                    format!(" ev={ev_b:?}")
                },
            );
        }
        if a_joined && b_joined {
            println!("  Both joined at tick {i}");
            break;
        }
    }

    println!(
        "\nA joined: {} ({} neighbors), B joined: {} ({} neighbors)",
        sub_a.is_joined(),
        sub_a.neighbors().count(),
        sub_b.is_joined(),
        sub_b.neighbors().count(),
    );

    // Both should be joined by now
    assert!(sub_a.is_joined(), "A should be joined");
    assert!(sub_b.is_joined(), "B should be joined");

    // Drain any stale events
    drain_events(&mut sub_a, short).await;
    drain_events(&mut sub_b, short).await;

    // A broadcasts a signed AboutMe (exactly like iced_chat does)
    println!("\nA broadcasting AboutMe...");
    let msg = Message::AboutMe {
        name: "Alice".into(),
        profile_image_ticket: None,
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg).expect("sign");
    println!("  encoded {} bytes", encoded.len());
    sub_a.broadcast(encoded).await?;
    println!("  broadcast done");

    // Wait for propagation
    sleep(Duration::from_secs(3)).await;

    // Drain B's events
    println!("\nDraining B's events...");
    let ev_b = drain_events(&mut sub_b, Duration::from_secs(2)).await;
    println!("  B received {} events", ev_b.len());

    // Also drain A (it might receive its own msg back via gossip or Neighbor events)
    let ev_a = drain_events(&mut sub_a, short).await;
    if !ev_a.is_empty() {
        println!("  A also received: {ev_a:?}");
    }

    let mut received_signed = false;
    for ev in &ev_b {
        match ev {
            GossipEvent::Received(msg) => {
                println!(
                    "  B: Received {} bytes from {:?}",
                    msg.content.len(),
                    msg.delivered_from
                );
                match SignedMessage::verify_and_decode(&msg.content) {
                    Ok((from, decoded, _sent_at)) => {
                        println!("  B: decoded from={} msg={:?}", from.fmt_short(), decoded);
                        received_signed = true;
                    }
                    Err(e) => {
                        println!("  B: DECODE ERROR: {e}");
                        println!(
                            "  B: raw hex: {:02x?}",
                            &msg.content[..msg.content.len().min(32)]
                        );
                    }
                }
            }
            GossipEvent::NeighborUp(id) => println!("  B: NeighborUp {}", id.fmt_short()),
            GossipEvent::NeighborDown(id) => println!("  B: NeighborDown {}", id.fmt_short()),
            GossipEvent::Lagged => println!("  B: Lagged"),
        }
    }

    assert!(received_signed, "B should receive and decode A's message");

    // Cleanup
    drop(sub_a);
    drop(sub_b);
    drop(router_a);
    drop(ep_a);
    drop(router_b);
    drop(ep_b);

    println!("\n✓ TEST PASSED");
    Ok(())
}
