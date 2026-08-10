//! Transport boundary for encoded screen frames.

use super::{codec::EncodedFrame, ScreenShareError};

/// Sends encoded frames without coupling screen sharing to gossip or QUIC.
pub trait ScreenTransport: Send {
    /// Send one encoded frame.
    fn send(&mut self, frame: EncodedFrame) -> Result<(), ScreenShareError>;
}
