//! Reproduction test: simulates the exact chat-gui gossip flow
//! to catch "decode signed message" failures.
//!
//! Creates two peers, both subscribe to the same gossip topic,
//! sends signed AboutMe, and verifies the other peer decodes it.

use std::time::Duration;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, SecretKey,
    RelayMode, Endpoint,
};
use iroh_gossip::{
    api::Event as GossipEvent,
    chat_core::{Message, SignedMessage},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};
use rand::{Rng, RngExt, SeedableRng};

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
    let (router_b, ep_b, sk_b, gossip_b) = spawn_peer(&mut rng).await?;
    
    let topic = TopicId::from_bytes(rng.random());
    
    // Both subscribe to the same topic.  Peer A bootstraps from itself.
    let mut sub_a = gossip_a.subscribe(topic, vec![ep_a.id()]).await?;
    // Peer B needs an extra wait before subscribing so the topic is discoverable.
    sleep(Duration::from_millis(500)).await;
    let mut sub_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;
    
    // Wait a moment for subscriptions to connect
    sleep(Duration::from_millis(200)).await;
    
    // Peer A sends a signed AboutMe (identical to what chat-gui does)
    let msg = Message::AboutMe {
        name: "PeerA".to_string(),
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg)
        .expect("sign_and_encode should work");
    println!("Peer A broadcasts {} bytes of AboutMe", encoded.len());
    
    sub_a.broadcast(encoded).await?;
    
    // Wait for gossip propagation
    sleep(Duration::from_secs(2)).await;
    
    // Check that Peer B received and decoded the message
    let mut received = Vec::new();
    while let Ok(Some(ev)) = sub_b.try_next().await {
        match ev {
            GossipEvent::Received(msg) => {
                println!("Peer B received a gossip event ({} bytes)", msg.content.len());
                match SignedMessage::verify_and_decode(&msg.content) {
                    Ok((from, decoded)) => {
                        println!("  ✓ Decoded: from={}, msg={:?}", from.fmt_short(), decoded);
                        received.push((from, decoded));
                    }
                    Err(e) => {
                        // THIS IS THE BUG THE USER SEES
                        println!("  ✗ DECODE ERROR: {e}");
                        println!("  Raw bytes hex: {:02x?}", &msg.content[..msg.content.len().min(32)]);
                    }
                }
            }
            GossipEvent::NeighborUp(id) => println!("Peer B: NeighborUp {}", id.fmt_short()),
            GossipEvent::NeighborDown(id) => println!("Peer B: NeighborDown {}", id.fmt_short()),
            GossipEvent::Lagged => println!("Peer B: Lagged"),
        }
    }
    
    assert!(!received.is_empty(), "Peer B should have received at least one message");
    assert_eq!(received[0].0, sk_a.public(), "From should be Peer A");
    
    // Cleanup
    drop(router_a);
    drop(ep_a);
    drop(router_b);
    drop(ep_b);
    
    println!("✓✓ TEST PASSED ✓✓");
    Ok(())
}
