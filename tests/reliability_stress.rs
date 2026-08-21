//! Deterministic multi-node reliability stress suite.
//!
//! The default run is bounded for CI. Set `BORU_RELIABILITY_SOAK=1` for the
//! longer manual soak. Every failure includes the seed, operation index, node,
//! and topic so it can be reproduced without logging message contents.

use std::{collections::HashSet, env, fs, path::Path, time::Instant};

use rand::{rngs::ChaCha12Rng, RngExt, SeedableRng};

const DEFAULT_SEED: u64 = 0xB0A7_5479;
const SHORT_NODES: usize = 4;
const SHORT_TOPICS: usize = 3;
const SHORT_OPS: usize = 96;
const SOAK_NODES: usize = 6;
const SOAK_TOPICS: usize = 5;
const SOAK_OPS: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Direct {
        from: usize,
        to: usize,
        topic: usize,
    },
    Disconnect(usize),
    Reconnect(usize),
    Restart(usize),
}

#[derive(Debug, Default, Clone)]
struct Metrics {
    operations: usize,
    direct_messages: usize,
    delivered: usize,
    duplicate_deliveries: usize,
    disconnects: usize,
    reconnects: usize,
    restarts: usize,
    max_pending: usize,
    max_live_nodes: usize,
    elapsed_ms: u128,
}

#[derive(Debug)]
struct Node {
    live: bool,
    generation: u64,
    received: HashSet<(usize, u64)>,
    pending: Vec<(usize, u64)>,
}

impl Node {
    fn new() -> Self {
        Self {
            live: true,
            generation: 0,
            received: HashSet::new(),
            pending: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct Run {
    seed: u64,
    nodes: Vec<Node>,
    next_message: u64,
    metrics: Metrics,
    trace_digest: u64,
}

impl Run {
    fn new(seed: u64, nodes: usize) -> Self {
        Self {
            seed,
            nodes: (0..nodes).map(|_| Node::new()).collect(),
            next_message: 0,
            metrics: Metrics::default(),
            trace_digest: 0xcbf2_9ce4_8422_2325,
        }
    }

    fn record(&mut self, op_index: usize, op: Op) {
        let text = format!("{op_index}:{op:?}");
        for byte in text.bytes() {
            self.trace_digest ^= u64::from(byte);
            self.trace_digest = self.trace_digest.wrapping_mul(0x1000_0000_01b3);
        }
    }

    fn deliver(&mut self, node: usize, topic: usize, message: u64) {
        if !self.nodes[node].live {
            self.nodes[node].pending.push((topic, message));
            return;
        }
        self.metrics.delivered += 1;
        if !self.nodes[node].received.insert((topic, message)) {
            self.metrics.duplicate_deliveries += 1;
        }
    }

    fn direct(&mut self, from: usize, to: usize, topic: usize) {
        let message = self.next_message;
        self.next_message += 1;
        self.metrics.direct_messages += 1;
        // The sender's local projection is also durable and deduplicated.
        self.deliver(from, topic, message);
        if self.nodes[from].live && self.nodes[to].live {
            self.deliver(to, topic, message);
        } else {
            self.nodes[to].pending.push((topic, message));
        }
    }

    fn reconnect(&mut self, node: usize) {
        self.nodes[node].live = true;
        let pending = std::mem::take(&mut self.nodes[node].pending);
        for (topic, message) in pending {
            self.deliver(node, topic, message);
        }
        self.metrics.reconnects += 1;
    }

    fn apply(&mut self, op_index: usize, op: Op) {
        self.record(op_index, op);
        match op {
            Op::Direct { from, to, topic } => self.direct(from, to, topic),
            Op::Disconnect(node) => {
                self.nodes[node].live = false;
                self.metrics.disconnects += 1;
            }
            Op::Reconnect(node) => self.reconnect(node),
            Op::Restart(node) => {
                self.nodes[node].live = false;
                self.nodes[node].generation += 1;
                self.reconnect(node);
                self.metrics.restarts += 1;
            }
        }
        self.metrics.operations = op_index + 1;
        self.metrics.max_pending = self
            .metrics
            .max_pending
            .max(self.nodes.iter().map(|n| n.pending.len()).sum());
        self.metrics.max_live_nodes = self
            .metrics
            .max_live_nodes
            .max(self.nodes.iter().filter(|n| n.live).count());
        self.assert_invariants(op_index);
    }

    fn assert_invariants(&self, op_index: usize) {
        assert_eq!(
            self.metrics.duplicate_deliveries, 0,
            "seed={} operation={} duplicate delivery (node/topic/message are retained in the deterministic state)",
            self.seed,
            op_index
        );
        for (node, state) in self.nodes.iter().enumerate() {
            assert!(
                state.pending.len() <= self.next_message as usize,
                "seed={} operation={} node={} pending queue exceeded sent messages",
                self.seed,
                op_index,
                node
            );
        }
    }
}

fn generated_ops(seed: u64, nodes: usize, topics: usize, count: usize) -> Vec<Op> {
    let mut rng = ChaCha12Rng::seed_from_u64(seed);
    (0..count)
        .map(|_| {
            let node = (rng.random::<u64>() as usize) % nodes;
            match rng.random::<u8>() % 5 {
                0..=2 => {
                    let mut to = (rng.random::<u64>() as usize) % nodes;
                    if to == node {
                        to = (to + 1) % nodes;
                    }
                    Op::Direct {
                        from: node,
                        to,
                        topic: (rng.random::<u64>() as usize) % topics,
                    }
                }
                3 => Op::Disconnect(node),
                _ if rng.random::<bool>() => Op::Reconnect(node),
                _ => Op::Restart(node),
            }
        })
        .collect()
}

fn run(seed: u64, nodes: usize, topics: usize, operations: usize) -> Run {
    let started = Instant::now();
    let mut run = Run::new(seed, nodes);
    for (index, op) in generated_ops(seed, nodes, topics, operations)
        .into_iter()
        .enumerate()
    {
        run.apply(index, op);
    }
    run.metrics.elapsed_ms = started.elapsed().as_millis();
    run
}

fn emit_metrics(run: &Run, soak: bool) {
    eprintln!(
        "reliability-stress seed={} mode={} operations={} direct={} delivered={} reconnects={} restarts={} max_pending={} max_live_nodes={} elapsed_ms={} trace_digest={:016x}",
        run.seed,
        if soak { "soak" } else { "ci" },
        run.metrics.operations,
        run.metrics.direct_messages,
        run.metrics.delivered,
        run.metrics.reconnects,
        run.metrics.restarts,
        run.metrics.max_pending,
        run.metrics.max_live_nodes,
        run.metrics.elapsed_ms,
        run.trace_digest
    );
    if let Ok(path) = env::var("BORU_RELIABILITY_ARTIFACT") {
        let body = format!(
            "{{\n  \"seed\": {},\n  \"mode\": \"{}\",\n  \"operations\": {},\n  \"direct_messages\": {},\n  \"delivered\": {},\n  \"reconnects\": {},\n  \"restarts\": {},\n  \"max_pending\": {},\n  \"max_live_nodes\": {},\n  \"elapsed_ms\": {},\n  \"trace_digest\": \"{:016x}\"\n}}\n",
            run.seed,
            if soak { "soak" } else { "ci" },
            run.metrics.operations,
            run.metrics.direct_messages,
            run.metrics.delivered,
            run.metrics.reconnects,
            run.metrics.restarts,
            run.metrics.max_pending,
            run.metrics.max_live_nodes,
            run.metrics.elapsed_ms,
            run.trace_digest
        );
        let path = Path::new(&path);
        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new("."))).unwrap();
        fs::write(path, body).expect("write reliability metrics artifact");
    }
}

#[test]
fn reliability_stress_ci_is_seed_repeatable_and_bounded() {
    let seed = env::var("BORU_RELIABILITY_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    let first = run(seed, SHORT_NODES, SHORT_TOPICS, SHORT_OPS);
    let second = run(seed, SHORT_NODES, SHORT_TOPICS, SHORT_OPS);
    assert_eq!(
        first.trace_digest, second.trace_digest,
        "same seed must reproduce operation trace"
    );
    assert_eq!(first.metrics.operations, SHORT_OPS);
    assert!(first.metrics.direct_messages > 0);
    assert!(first.metrics.restarts > 0);
    emit_metrics(&first, false);
}

#[test]
#[ignore = "manual long-soak variant; run with BORU_RELIABILITY_SOAK=1"]
fn reliability_stress_long_soak() {
    if env::var_os("BORU_RELIABILITY_SOAK").is_none() {
        return;
    }
    let seed = env::var("BORU_RELIABILITY_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    let result = run(seed, SOAK_NODES, SOAK_TOPICS, SOAK_OPS);
    emit_metrics(&result, true);
}
