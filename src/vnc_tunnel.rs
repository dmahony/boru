//! Guardrails for the experimental VNC-over-Boru-Tunnel prototype.
//!
//! VNC credentials and the VNC wire protocol remain outside Boru. Boru only
//! forwards an already-running TCP service, and this module makes the
//! localhost-only policy explicit and testable.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Experimental service label used in tunnel offers and the GUI.
pub const SERVICE_NAME: &str = "VNC desktop (experimental)";
/// The only source address accepted by the prototype.
pub const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Configuration for a localhost VNC server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VncTunnelConfig {
    /// Address where the host VNC server is listening.
    pub source: SocketAddr,
    /// Optional loopback port requested by the viewer.
    pub preferred_viewer_port: Option<u16>,
}

impl VncTunnelConfig {
    /// Validate a VNC source before it is registered with `TunnelService`.
    pub fn validate(self) -> Result<Self, VncTunnelError> {
        if self.source.ip() != LOOPBACK {
            return Err(VncTunnelError::SourceMustBeLoopback);
        }
        if self.source.port() == 0 {
            return Err(VncTunnelError::SourcePortRequired);
        }
        if self.preferred_viewer_port == Some(0) {
            return Err(VncTunnelError::InvalidViewerPort);
        }
        Ok(self)
    }
}

/// A safe VNC prototype configuration error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VncTunnelError {
    /// The source address is not exactly IPv4 loopback.
    SourceMustBeLoopback,
    /// The source port is zero.
    SourcePortRequired,
    /// An explicitly supplied viewer port is zero.
    InvalidViewerPort,
}

impl std::fmt::Display for VncTunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SourceMustBeLoopback => {
                "the VNC server must listen on 127.0.0.1; LAN/WAN addresses are refused"
            }
            Self::SourcePortRequired => "the VNC source port must be between 1 and 65535",
            Self::InvalidViewerPort => "the viewer port must be non-zero or omitted",
        })
    }
}

impl std::error::Error for VncTunnelError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_ipv4_loopback() {
        let config = VncTunnelConfig {
            source: "127.0.0.1:5900".parse().unwrap(),
            preferred_viewer_port: None,
        };
        assert!(config.validate().is_ok());
        for address in ["0.0.0.0:5900", "192.168.1.2:5900", "[::1]:5900"] {
            let config = VncTunnelConfig {
                source: address.parse().unwrap(),
                preferred_viewer_port: None,
            };
            assert_eq!(config.validate(), Err(VncTunnelError::SourceMustBeLoopback));
        }
    }

    #[test]
    fn rejects_zero_ports() {
        assert_eq!(
            (VncTunnelConfig {
                source: "127.0.0.1:0".parse().unwrap(),
                preferred_viewer_port: None
            })
            .validate(),
            Err(VncTunnelError::SourcePortRequired)
        );
        assert_eq!(
            (VncTunnelConfig {
                source: "127.0.0.1:5900".parse().unwrap(),
                preferred_viewer_port: Some(0)
            })
            .validate(),
            Err(VncTunnelError::InvalidViewerPort)
        );
    }
}
