//! Reassembly boundary for live-call video packets.
//!
//! Fragment reassembly is out of scope for this skeleton. The type below
//! records the ownership boundary so a future implementation cannot reuse the
//! attachment downloader's file assembly state by accident.

use super::packet::VideoPacket;

/// Result of attempting to assemble live video packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassemblyResult {
    /// No complete access unit is available yet.
    Pending,
    /// A complete encoded access unit is ready for the live decoder.
    Complete(Vec<u8>),
}

/// Per-call live packet reassembler placeholder.
#[derive(Debug, Default)]
pub struct VideoReassembler {
    packet_count: usize,
}

impl VideoReassembler {
    /// Create an empty live-media reassembler.
    pub const fn new() -> Self {
        Self { packet_count: 0 }
    }

    /// Observe one live packet without forwarding it to attachment playback.
    pub fn push(&mut self, _packet: VideoPacket) -> ReassemblyResult {
        self.packet_count = self.packet_count.saturating_add(1);
        ReassemblyResult::Pending
    }

    /// Number of live packets observed by this instance.
    pub const fn packet_count(&self) -> usize {
        self.packet_count
    }
}
