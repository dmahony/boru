//! Debug test: trace why two peers don't connect via gossip.
//! Creates two peers with relay enabled, subscribes both, sends a message.
//!
//! Shared scaffolding (peer fixture creation) comes from the `test_support`
//! module (BORU-TEST-010).

mod support;
use support::peers::spawn_peer_relay;

use boru_core::{
    api::Event as GossipEvent,
    chat_core::{Message, SignedMessage},
    proto::TopicId,
};
use iroh::address_lookup::memory::MemoryLookup;
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn test_two_peers_with_relay() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    let a = spawn_peer_relay(&mut rng).await?;
    let router_a = a.router;
    let ep_a = a.endpoint;
    let sk_a = a.secret_key;
    let gossip_a = a.gossip;

    let b = spawn_peer_relay(&mut rng).await?;
    let router_b = b.router;
    let ep_b = b.endpoint;
    let _sk_b = b.secret_key;
    let gossip_b = b.gossip;

    println!("Peer A id: {}", ep_a.id().fmt_short());
    println!("Peer B id: {}", ep_b.id().fmt_short());

    let topic = TopicId::from_bytes(rng.random());
    println!("Topic: {topic}");

    // Both subscribe to the same topic
    let mut sub_a = gossip_a.subscribe(topic, vec![ep_a.id()]).await?;
    println!("Peer A subscribed");
    sleep(Duration::from_millis(500)).await;
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    memory_lookup.set_endpoint_info(ep_a.addr());
    let mut sub_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;
    println!("Peer B subscribed");

    // Wait for connections
    println!("Waiting for connections...");
    for i in 0..20 {
        sleep(Duration::from_millis(200)).await;
        let joined_a = sub_a.is_joined();
        let joined_b = sub_b.is_joined();
        println!("  tick {i}: A joined={joined_a}, B joined={joined_b}");

        // Drain events
        loop {
            match timeout(Duration::from_millis(200), sub_a.try_next()).await {
                Ok(Ok(Some(ev))) => match ev {
                    GossipEvent::Received(msg) => {
                        println!("  A: received {} bytes", msg.content.len());
                    }
                    GossipEvent::NeighborUp(id) => println!("  A: NeighborUp {}", id.fmt_short()),
                    GossipEvent::NeighborDown(id) => {
                        println!("  A: NeighborDown {}", id.fmt_short())
                    }
                    GossipEvent::Lagged => println!("  A: Lagged"),
                    GossipEvent::MissingMessages { .. } => { /* recovery callback, no action needed */
                    }
                },
                Ok(Ok(None)) | Err(_) => break,
                Ok(Err(e)) => return Err(e.into()),
            }
        }
        loop {
            match timeout(Duration::from_millis(200), sub_b.try_next()).await {
                Ok(Ok(Some(ev))) => match ev {
                    GossipEvent::Received(msg) => {
                        println!("  B: received {} bytes", msg.content.len());
                    }
                    GossipEvent::NeighborUp(id) => println!("  B: NeighborUp {}", id.fmt_short()),
                    GossipEvent::NeighborDown(id) => {
                        println!("  B: NeighborDown {}", id.fmt_short())
                    }
                    GossipEvent::Lagged => println!("  B: Lagged"),
                    GossipEvent::MissingMessages { .. } => { /* recovery callback, no action needed */
                    }
                },
                Ok(Ok(None)) | Err(_) => break,
                Ok(Err(e)) => return Err(e.into()),
            }
        }
    }

    println!("\nPeer A broadcasting AboutMe...");
    let msg = Message::AboutMe {
        name: "PeerA".into(),
        profile_image_ticket: None,
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg).expect("sign");
    println!("  encoded {} bytes", encoded.len());
    sub_a.broadcast(encoded).await?;

    sleep(Duration::from_secs(3)).await;

    println!("\nDraining events after broadcast...");
    let mut received_b = false;
    loop {
        match timeout(Duration::from_millis(300), sub_b.try_next()).await {
            Ok(Ok(Some(ev))) => match ev {
                GossipEvent::Received(msg) => {
                    println!("  B: received {} bytes", msg.content.len());
                    match SignedMessage::verify_and_decode(&msg.content) {
                        Ok((from, decoded, _sent_at)) => {
                            println!("  B: decoded from={} msg={:?}", from.fmt_short(), decoded);
                            received_b = true;
                        }
                        Err(e) => println!("  B: DECODE FAILED: {e}"),
                    }
                }
                other => println!("  B: event: {other:?}"),
            },
            Ok(Ok(None)) | Err(_) => break,
            Ok(Err(e)) => return Err(e.into()),
        }
    }
    loop {
        match timeout(Duration::from_millis(300), sub_a.try_next()).await {
            Ok(Ok(Some(GossipEvent::NeighborUp(id)))) => {
                println!("  A: NeighborUp {}", id.fmt_short())
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Err(_) => break,
            Ok(Err(e)) => return Err(e.into()),
        }
    }

    assert!(received_b, "Peer B should receive the message from Peer A");

    drop(sub_a);
    drop(sub_b);
    drop(router_a);
    drop(ep_a);
    drop(router_b);
    drop(ep_b);
    println!("✓ TEST PASSED");
    Ok(())
}
