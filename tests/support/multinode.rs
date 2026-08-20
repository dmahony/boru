//! Isolated, unattended two/three-node gossip scenarios.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::anyhow;
use boru_core::{
    api::{Event, GossipTopic},
    net::{Gossip, GOSSIP_ALPN},
    TopicId,
};
use bytes::Bytes;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, RelayMode,
    SecretKey,
};
use n0_error::Result;
use n0_future::StreamExt;
use rand::{RngExt, SeedableRng};
use tempfile::TempDir;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct HarnessNode {
    pub name: String,
    pub data_dir: PathBuf,
    pub log_path: PathBuf,
    pub endpoint: Endpoint,
    pub secret_key: SecretKey,
    gossip: Option<Gossip>,
    router: Option<Router>,
    topic: Option<GossipTopic>,
    log: File,
}

impl HarnessNode {
    pub fn id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    pub async fn send(&mut self, payload: impl Into<Bytes>) -> Result<()> {
        let topic = self
            .topic
            .as_mut()
            .ok_or_else(|| anyhow!("{} has not joined", self.name))?;
        topic.broadcast(payload.into()).await?;
        self.record("send");
        Ok(())
    }

    pub async fn receive(&mut self, timeout: Duration) -> Result<Bytes> {
        let name = self.name.clone();
        let topic = self
            .topic
            .as_mut()
            .ok_or_else(|| anyhow!("{name} has not joined"))?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                n0_error::bail_any!("{name} timed out waiting for a message");
            }
            match tokio::time::timeout(remaining, topic.next()).await {
                Ok(Some(Ok(Event::Received(message)))) => {
                    self.record("receive");
                    return Ok(message.content);
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(error))) => return Err(error.into()),
                Ok(None) => n0_error::bail_any!("{name} gossip stream closed"),
                Err(_) => n0_error::bail_any!("{name} timed out waiting for a message"),
            }
        }
    }

    fn record(&mut self, event: &str) {
        let _ = writeln!(self.log, "{event}");
        let _ = self.log.flush();
    }

    pub async fn shutdown(&mut self) {
        self.topic.take();
        self.gossip.take();
        if let Some(router) = self.router.take() {
            let _ = router.shutdown().await;
        }
        let _ = self.endpoint.close().await;
        self.record("shutdown");
    }
}

#[derive(Debug)]
pub struct MultiNodeHarness {
    root: TempDir,
    pub nodes: Vec<HarnessNode>,
}

impl MultiNodeHarness {
    pub async fn new(count: usize) -> Result<Self> {
        if !(2..=3).contains(&count) {
            n0_error::bail_any!("harness supports 2 or 3 nodes, got {count}");
        }
        let root = tempfile::Builder::new()
            .prefix("boru-multinode-")
            .tempdir()?;
        let lookup = MemoryLookup::new();
        let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xB02E);
        let mut nodes = Vec::with_capacity(count);
        for index in 0..count {
            let name = format!("node-{index}");
            let data_dir = root.path().join(&name);
            std::fs::create_dir(&data_dir)?;
            let log_path = data_dir.join("node.log");
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            let secret_key = SecretKey::from_bytes(&rng.random());
            let endpoint = Endpoint::builder(presets::N0DisableRelay)
                .secret_key(secret_key.clone())
                .address_lookup(lookup.clone())
                .relay_mode(RelayMode::Disabled)
                .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())?
                .bind()
                .await?;
            lookup.set_endpoint_info(endpoint.addr());
            let gossip = Gossip::builder().spawn(endpoint.clone());
            let router = Router::builder(endpoint.clone())
                .accept(GOSSIP_ALPN, gossip.clone())
                .spawn();
            nodes.push(HarnessNode {
                name,
                data_dir,
                log_path,
                endpoint,
                secret_key,
                gossip: Some(gossip),
                router: Some(router),
                topic: None,
                log,
            });
        }
        Ok(Self { root, nodes })
    }

    pub async fn join_room(&mut self, topic: TopicId) -> Result<()> {
        let bootstrap = self.nodes[0].id();
        for index in 0..self.nodes.len() {
            let node = &mut self.nodes[index];
            let gossip = node
                .gossip
                .as_ref()
                .ok_or_else(|| anyhow!("{} is shut down", node.name))?;
            let peers = if index == 0 {
                Vec::new()
            } else {
                vec![bootstrap]
            };
            node.topic = Some(gossip.subscribe(topic, peers).await?);
        }
        for node in &mut self.nodes {
            if let Some(topic) = node.topic.as_mut() {
                tokio::time::timeout(DEFAULT_TIMEOUT, topic.joined())
                    .await
                    .map_err(|_| anyhow!("{} timed out joining", node.name))??;
                node.record("joined");
            }
        }
        Ok(())
    }

    pub fn peer_ids(&self) -> Vec<iroh::EndpointId> {
        self.nodes.iter().map(HarnessNode::id).collect()
    }

    pub async fn disconnect(&mut self, index: usize) -> Result<()> {
        let node = self
            .nodes
            .get_mut(index)
            .ok_or_else(|| anyhow!("unknown node {index}"))?;
        node.topic.take();
        node.record("disconnect");
        Ok(())
    }

    pub async fn reconnect(&mut self, index: usize, topic: TopicId) -> Result<()> {
        let bootstrap = self.nodes[0].id();
        let node = self
            .nodes
            .get_mut(index)
            .ok_or_else(|| anyhow!("unknown node {index}"))?;
        let gossip = node
            .gossip
            .as_ref()
            .ok_or_else(|| anyhow!("{} is shut down", node.name))?;
        let peers = if index == 0 {
            Vec::new()
        } else {
            vec![bootstrap]
        };
        node.topic = Some(gossip.subscribe(topic, peers).await?);
        let topic_handle = node.topic.as_mut().expect("topic assigned above");
        tokio::time::timeout(DEFAULT_TIMEOUT, topic_handle.joined())
            .await
            .map_err(|_| anyhow!("{} timed out reconnecting", node.name))??;
        node.record("reconnect");
        Ok(())
    }

    pub fn root_path(&self) -> &Path {
        self.root.path()
    }

    pub async fn shutdown(&mut self) {
        for node in &mut self.nodes {
            node.shutdown().await;
        }
    }
}
