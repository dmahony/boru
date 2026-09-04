//! Deterministic, rootless network impairment model for media tests.
//!
//! The link advances only when [`ImpairedLink::advance_to`] is called; it never
//! reads the wall clock or sleeps.  This makes traces reproducible and tests
//! fast while still exercising queueing, scheduling, and loss behavior.

use std::cmp::Ordering;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MediaPriority {
    Video,
    Audio,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packet {
    pub id: u64,
    pub size_bytes: usize,
    pub priority: MediaPriority,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(id: u64, size_bytes: usize, priority: MediaPriority) -> Self {
        Self {
            id,
            size_bytes,
            priority,
            payload: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveredPacket {
    pub packet: Packet,
    pub delivered_at_us: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImpairedLinkConfig {
    pub capacity_bps: u64,
    pub base_delay_us: u64,
    pub jitter_us: u64,
    pub loss_percent: u8,
    pub duplicate_percent: u8,
    pub reorder_window: usize,
    pub queue_limit_bytes: usize,
}

impl Default for ImpairedLinkConfig {
    fn default() -> Self {
        Self {
            capacity_bps: 1_000_000,
            base_delay_us: 0,
            jitter_us: 0,
            loss_percent: 0,
            duplicate_percent: 0,
            reorder_window: 0,
            queue_limit_bytes: 64 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinkCounters {
    pub enqueued: u64,
    pub delivered: u64,
    pub dropped_loss: u64,
    pub dropped_queue: u64,
    pub duplicated: u64,
    pub bytes_delivered: u64,
}

#[derive(Clone, Debug)]
struct PendingDelivery {
    packet: Packet,
    at_us: u64,
    order: u64,
}

#[derive(Clone, Debug)]
pub struct ImpairedLink {
    config: ImpairedLinkConfig,
    now_us: u64,
    next_tx_us: u64,
    queue: VecDeque<Packet>,
    queue_bytes: usize,
    deliveries: Vec<PendingDelivery>,
    counters: LinkCounters,
    rng: u64,
    next_order: u64,
}

impl ImpairedLink {
    pub fn new(config: ImpairedLinkConfig, seed: u64) -> Self {
        assert!(config.loss_percent <= 100, "loss_percent must be <= 100");
        assert!(
            config.duplicate_percent <= 100,
            "duplicate_percent must be <= 100"
        );
        Self {
            config,
            now_us: 0,
            next_tx_us: 0,
            queue: VecDeque::new(),
            queue_bytes: 0,
            deliveries: Vec::new(),
            counters: LinkCounters::default(),
            rng: seed | 1,
            next_order: 0,
        }
    }

    pub fn config(&self) -> &ImpairedLinkConfig {
        &self.config
    }
    pub fn now_us(&self) -> u64 {
        self.now_us
    }
    pub fn counters(&self) -> &LinkCounters {
        &self.counters
    }
    pub fn queued_bytes(&self) -> usize {
        self.queue_bytes
    }
    pub fn queued_packets(&self) -> usize {
        self.queue.len()
    }

    /// Change capacity between steps, allowing adaptive-bitrate tests.
    pub fn set_capacity_bps(&mut self, capacity_bps: u64) {
        self.config.capacity_bps = capacity_bps;
    }

    /// Add a packet. Audio evicts queued video when the byte bound is full.
    pub fn send(&mut self, packet: Packet) -> bool {
        self.counters.enqueued += 1;
        if packet.size_bytes > self.config.queue_limit_bytes {
            self.counters.dropped_queue += 1;
            return false;
        }
        while self.queue_bytes + packet.size_bytes > self.config.queue_limit_bytes {
            let Some(index) = self
                .queue
                .iter()
                .position(|p| p.priority == MediaPriority::Video)
            else {
                self.counters.dropped_queue += 1;
                return false;
            };
            let evicted = self.queue.remove(index).expect("queue index exists");
            self.queue_bytes -= evicted.size_bytes;
            self.counters.dropped_queue += 1;
        }
        self.queue_bytes += packet.size_bytes;
        self.queue.push_back(packet);
        true
    }

    /// Alias useful in tests that describe the link as a transport.
    pub fn enqueue(&mut self, packet: Packet) -> bool {
        self.send(packet)
    }

    /// Advance the monotonic simulated clock and transmit everything due.
    pub fn advance_to(&mut self, now_us: u64) {
        assert!(
            now_us >= self.now_us,
            "simulated clock cannot move backwards"
        );
        self.now_us = now_us;
        while !self.queue.is_empty() {
            let start = self.next_tx_us.max(self.now_us.saturating_sub(self.now_us));
            if start > now_us {
                break;
            }
            let packet = self.take_next();
            self.queue_bytes -= packet.size_bytes;
            let duration = if self.config.capacity_bps == 0 {
                u64::MAX / 2
            } else {
                ((packet.size_bytes as u128 * 8 * 1_000_000)
                    .div_ceil(self.config.capacity_bps as u128)) as u64
            };
            let finish = start.saturating_add(duration.max(1));
            if finish > now_us {
                // A packet is not partially transmitted: put it back and wait.
                self.queue.push_front(packet.clone());
                self.queue_bytes += packet.size_bytes;
                break;
            }
            self.next_tx_us = finish;
            if self.percent_hit(self.config.loss_percent) {
                self.counters.dropped_loss += 1;
                continue;
            }
            let at_us = finish
                .saturating_add(self.config.base_delay_us)
                .saturating_add(self.jitter());
            self.schedule(packet.clone(), at_us);
            if self.percent_hit(self.config.duplicate_percent) {
                self.counters.duplicated += 1;
                self.schedule(packet, at_us.saturating_add(1));
            }
        }
    }

    pub fn drain_delivered(&mut self) -> Vec<DeliveredPacket> {
        self.deliveries
            .sort_by(|a, b| a.at_us.cmp(&b.at_us).then(a.order.cmp(&b.order)));
        let split = self
            .deliveries
            .partition_point(|delivery| delivery.at_us <= self.now_us);
        let ready: Vec<_> = self.deliveries.drain(..split).collect();
        ready
            .into_iter()
            .map(|delivery| {
                self.counters.delivered += 1;
                self.counters.bytes_delivered += delivery.packet.size_bytes as u64;
                DeliveredPacket {
                    packet: delivery.packet,
                    delivered_at_us: delivery.at_us,
                }
            })
            .collect()
    }

    fn take_next(&mut self) -> Packet {
        let window = self
            .config
            .reorder_window
            .min(self.queue.len().saturating_sub(1));
        let index = if window == 0 {
            0
        } else {
            (self.next_u64() as usize) % (window + 1)
        };
        self.queue.remove(index).expect("queue is non-empty")
    }

    fn schedule(&mut self, packet: Packet, at_us: u64) {
        self.deliveries.push(PendingDelivery {
            packet,
            at_us,
            order: self.next_order,
        });
        self.next_order += 1;
    }

    fn next_u64(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn percent_hit(&mut self, percent: u8) -> bool {
        percent != 0 && (self.next_u64() % 100) < percent as u64
    }

    fn jitter(&mut self) -> u64 {
        if self.config.jitter_us == 0 {
            return 0;
        }
        self.next_u64() % (self.config.jitter_us.saturating_mul(2).saturating_add(1))
    }
}

impl Ord for PendingDelivery {
    fn cmp(&self, other: &Self) -> Ordering {
        self.at_us
            .cmp(&other.at_us)
            .then(self.order.cmp(&other.order))
    }
}

impl PartialOrd for PendingDelivery {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for PendingDelivery {
    fn eq(&self, other: &Self) -> bool {
        self.at_us == other.at_us && self.order == other.order
    }
}
impl Eq for PendingDelivery {}
