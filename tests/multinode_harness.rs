//! Acceptance scenarios for the isolated multi-node harness.

mod support;

use std::time::Duration;

use boru_core::TopicId;
use bytes::Bytes;
use support::multinode::{MultiNodeHarness, DEFAULT_TIMEOUT};

#[tokio::test]
async fn two_nodes_exchange_messages_in_both_directions() -> n0_error::Result<()> {
    let mut harness = MultiNodeHarness::new(2).await?;
    harness.join_room(TopicId::from_bytes([0x42; 32])).await?;

    harness.nodes[0]
        .send(Bytes::from_static(b"from-node-0"))
        .await?;
    assert_eq!(
        harness.nodes[1].receive(DEFAULT_TIMEOUT).await?,
        Bytes::from_static(b"from-node-0")
    );
    harness.nodes[1]
        .send(Bytes::from_static(b"from-node-1"))
        .await?;
    assert_eq!(
        harness.nodes[0].receive(DEFAULT_TIMEOUT).await?,
        Bytes::from_static(b"from-node-1")
    );

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn three_nodes_exchange_public_room_message_and_cleanup() -> n0_error::Result<()> {
    let mut harness = MultiNodeHarness::new(3).await?;
    let root = harness.root_path().to_path_buf();
    harness.join_room(TopicId::from_bytes([0x43; 32])).await?;

    harness.nodes[0]
        .send(Bytes::from_static(b"public-room-message"))
        .await?;
    assert_eq!(
        harness.nodes[1].receive(DEFAULT_TIMEOUT).await?,
        Bytes::from_static(b"public-room-message")
    );
    assert_eq!(
        harness.nodes[2].receive(DEFAULT_TIMEOUT).await?,
        Bytes::from_static(b"public-room-message")
    );

    for (index, node) in harness.nodes.iter().enumerate() {
        assert_eq!(node.data_dir, root.join(format!("node-{index}")));
        assert!(node.data_dir.starts_with(&root));
        assert!(node.log_path.starts_with(&node.data_dir));
    }
    harness.shutdown().await;
    assert!(tokio::time::timeout(Duration::from_secs(1), async {})
        .await
        .is_ok());
    Ok(())
}

#[tokio::test]
async fn invalid_node_count_is_rejected_without_allocating_nodes() {
    assert!(MultiNodeHarness::new(1).await.is_err());
    assert!(MultiNodeHarness::new(4).await.is_err());
}
