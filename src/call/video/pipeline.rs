//! Live-call video pipeline boundary.
//!
//! The pipeline is intentionally a small ownership shell for future capture,
//! encode, packet, reassembly, and decode stages. It has no path or stream
//! handles and therefore cannot accidentally enter the attachment playback
//! system.

use super::packet::VideoPacket;
use super::reassembly::VideoReassembler;

/// The independent live-call video pipeline state.
#[derive(Debug)]
pub struct LiveVideoPipeline {
    reassembler: VideoReassembler,
    received_packets: u64,
}

impl Default for LiveVideoPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveVideoPipeline {
    /// Create an empty live-call pipeline.
    pub fn new() -> Self {
        Self {
            reassembler: VideoReassembler::new(),
            received_packets: 0,
        }
    }

    /// Accept one live packet for future reassembly and decoding.
    pub fn receive(&mut self, packet: VideoPacket) {
        self.received_packets = self.received_packets.saturating_add(1);
        let _ = self.reassembler.push(packet);
    }

    /// Number of packets accepted by this live pipeline.
    pub const fn received_packets(&self) -> u64 {
        self.received_packets
    }
}
