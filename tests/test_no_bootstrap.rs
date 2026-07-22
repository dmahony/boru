//! Test: can a peer that subscribed WITHOUT bootstrap peers
//! still receive messages? This tests the iced_chat OpenRoom flow.
//!
//! Scenario:
//!   Peer A creates room (subscribes with no bootstrap)
//!   Peer B joins via ticket (subscribes with A as bootstrap)
//!   A broadcasts -> B should receive
//!   B broadcasts -> A should receive

use boru_core::{
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

async fn spawn_relay(rng: &mut impl rand::Rng) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
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
            _ => break,
        }
    }
    events
}

#[tokio::test]
async fn test_no_bootstrap_peer_still_receives() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);

    // Spawn two peers
    let (router_a, ep_a, sk_a, gossip_a) = spawn_relay(&mut rng).await?;
    let (router_b, ep_b, sk_b, gossip_b) = spawn_relay(&mut rng).await?;

    println!("Peer A: {}", ep_a.id().fmt_short());
    println!("Peer B: {}", ep_b.id().fmt_short());

    let topic = TopicId::from_bytes(rng.random());
    println!("Topic: {topic}");

    // A subscribes with NO bootstrap peers — exactly like iced_chat's
    // CreateNewRoom and OpenRoom do.
    println!("\n--- A subscribes (no bootstrap peers, like CreateNewRoom) ---");
    let mut sub_a = gossip_a.subscribe(topic, vec![]).await?;
    println!("A subscribed");

    // B joins with A as bootstrap — like JoinFromTicket
    println!("\n--- B subscribes (with A as bootstrap, like JoinFromTicket) ---");
    sleep(Duration::from_millis(200)).await;
    let mut sub_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;
    println!("B subscribed");

    // Wait for both to join
    let short = Duration::from_millis(50);
    let tick_delay = Duration::from_millis(200);
    let max_ticks = 60;
    for i in 0..max_ticks {
        // Give the gossip actor wall-clock time to process connection events.
        // Without this sleep, the drain loop can outrun the Join/Neighbor handshake,
        // causing the test to fail intermittently.
        sleep(tick_delay).await;
        drain_events(&mut sub_a, short).await;
        drain_events(&mut sub_b, short).await;
        if sub_a.is_joined() && sub_b.is_joined() {
            println!("  Both joined at tick {i}");
            break;
        }
        if i % 10 == 9 {
            println!(
                "  tick {i}: A joined={} B joined={}",
                sub_a.is_joined(),
                sub_b.is_joined()
            );
        }
    }

    assert!(
        sub_a.is_joined(),
        "A should be joined after {} ticks",
        max_ticks
    );
    assert!(
        sub_b.is_joined(),
        "B should be joined after {} ticks",
        max_ticks
    );

    // Drain stale events
    drain_events(&mut sub_a, short).await;
    drain_events(&mut sub_b, short).await;

    // A sends a message to B
    println!("\n--- A broadcasts to B ---");
    let msg = Message::Message {
        text: "hello from A".into(),
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg).expect("sign");
    println!("  A broadcast ({} bytes)", encoded.len());
    sub_a.broadcast(encoded).await?;

    sleep(Duration::from_secs(3)).await;

    let ev_b = drain_events(&mut sub_b, Duration::from_secs(2)).await;
    let received_from_a = ev_b.iter().any(|ev| {
        if let GossipEvent::Received(msg) = ev {
            if let Ok((_from, decoded, _sent_at)) = SignedMessage::verify_and_decode(&msg.content) {
                println!("  B decoded: {decoded:?}");
                return true;
            }
        }
        false
    });
    println!("  B received A's message: {received_from_a}");
    assert!(received_from_a, "B should receive A's broadcast");

    // B sends a message to A
    println!("\n--- B broadcasts to A ---");
    let msg = Message::Message {
        text: "hello from B".into(),
    };
    let encoded = SignedMessage::sign_and_encode(&sk_b, &msg).expect("sign");
    println!("  B broadcast ({} bytes)", encoded.len());
    sub_b.broadcast(encoded).await?;

    sleep(Duration::from_secs(3)).await;

    let ev_a = drain_events(&mut sub_a, Duration::from_secs(2)).await;
    let received_from_b = ev_a.iter().any(|ev| {
        if let GossipEvent::Received(msg) = ev {
            if let Ok((_from, decoded, _sent_at)) = SignedMessage::verify_and_decode(&msg.content) {
                println!("  A decoded: {decoded:?}");
                return true;
            }
        }
        false
    });
    println!("  A received B's message: {received_from_b}");
    assert!(received_from_b, "A should receive B's broadcast");

    // Cleanup
    drop(sub_a);
    drop(sub_b);
    drop(router_a);
    drop(ep_a);
    drop(router_b);
    drop(ep_b);

    println!("\n✓ TEST PASSED — two-way communication works even when A subscribes with no bootstrap peers");
    Ok(())
}
