//! Reproduction test: simulates the exact chat-gui gossip flow
//! to catch "decode signed message" failures.
//!
//! Creates two peers, both subscribe to the same gossip topic,
//! sends signed AboutMe, and verifies the other peer decodes it.

use boru_chat::{
    api::Event as GossipEvent,
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
use tokio::time::timeout;

fn make_sk(rng: &mut impl rand::Rng) -> SecretKey {
    SecretKey::from_bytes(&rng.random())
}

async fn spawn_peer(rng: &mut impl rand::Rng) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(make_sk(rng))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip))
}

#[tokio::test]
async fn test_signed_message_gossip_flow() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Spawn two peers
    let (router_a, ep_a, sk_a, gossip_a) = spawn_peer(&mut rng).await?;
    let (router_b, ep_b, _sk_b, gossip_b) = spawn_peer(&mut rng).await?;

    let topic = TopicId::from_bytes(rng.random());

    // Both subscribe to the same topic.  Peer A bootstraps from itself.
    let mut sub_a = gossip_a.subscribe(topic, vec![ep_a.id()]).await?;
    // Peer B needs an extra wait before subscribing so the topic is discoverable.
    sleep(Duration::from_millis(500)).await;
    let memory_lookup = MemoryLookup::new();
    if let Ok(addr_lookup) = ep_b.address_lookup() {
        addr_lookup.add(memory_lookup.clone());
    }
    memory_lookup.set_endpoint_info(ep_a.addr());
    let mut sub_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;

    // Wait for peer A to see peer B as a neighbor (gossip connection established)
    // before broadcasting, so the message isn't sent on a not-yet-ready connection.
    let deadline_a = tokio::time::Instant::now() + Duration::from_secs(10);
    let b_id = ep_b.id();
    let mut a_saw_b = false;
    while tokio::time::Instant::now() < deadline_a {
        match timeout(Duration::from_millis(200), sub_a.try_next()).await {
            Ok(Ok(Some(GossipEvent::NeighborUp(id)))) if id == b_id => {
                a_saw_b = true;
                break;
            }
            Ok(Ok(Some(_))) => {} // drain other events
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_elapsed) => {} // timeout is expected — keep waiting
        }
    }
    assert!(
        a_saw_b,
        "Peer A should see Peer B as a neighbor before broadcasting"
    );

    // Peer A sends a signed AboutMe (identical to what chat-gui does)
    let msg = Message::AboutMe {
        name: "PeerA".to_string(),
        profile_image_ticket: None,
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg).expect("sign_and_encode should work");
    println!("Peer A broadcasts {} bytes of AboutMe", encoded.len());

    sub_a.broadcast(encoded).await?;

    // Wait for gossip propagation
    sleep(Duration::from_secs(2)).await;

    // Check that Peer B received and decoded the message
    let mut received = Vec::new();
    loop {
        match timeout(Duration::from_millis(300), sub_b.try_next()).await {
            Ok(Ok(Some(ev))) => match ev {
                GossipEvent::Received(msg) => {
                    println!(
                        "Peer B received a gossip event ({} bytes)",
                        msg.content.len()
                    );
                    match SignedMessage::verify_and_decode(&msg.content) {
                        Ok((from, decoded, _sent_at)) => {
                            println!("  ✓ Decoded: from={}, msg={:?}", from.fmt_short(), decoded);
                            received.push((from, decoded));
                        }
                        Err(e) => {
                            // THIS IS THE BUG THE USER SEES
                            println!("  ✗ DECODE ERROR: {e}");
                            println!(
                                "  Raw bytes hex: {:02x?}",
                                &msg.content[..msg.content.len().min(32)]
                            );
                        }
                    }
                }
                GossipEvent::NeighborUp(id) => println!("Peer B: NeighborUp {}", id.fmt_short()),
                GossipEvent::NeighborDown(id) => {
                    println!("Peer B: NeighborDown {}", id.fmt_short())
                }
                GossipEvent::Lagged => println!("Peer B: Lagged"),
            },
            Ok(Ok(None)) => break,
            _ => break,
        }
    }

    assert!(
        !received.is_empty(),
        "Peer B should have received at least one message"
    );
    assert_eq!(received[0].0, sk_a.public(), "From should be Peer A");

    // Cleanup
    drop(router_a);
    drop(ep_a);
    drop(router_b);
    drop(ep_b);

    println!("✓✓ TEST PASSED ✓✓");
    Ok(())
}
