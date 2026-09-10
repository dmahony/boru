//! Presentation helpers for the Home Tunnels panel.
//!
//! The panel is a read-only projection of `TunnelService`; this module must
//! not create networking or persistence paths. In particular, its count is
//! based on live connection observations, not the number of saved definitions.

use boru_core::tunnel::service::TunnelStatus;

/// Count tunnel definitions that currently have a live/ready connection.
///
/// `active_connections` is supplied by `TunnelService::list_tunnels()`. A
/// configured, disconnected, failed, or merely available tunnel therefore
/// does not inflate the Home badge.
pub(crate) fn active_tunnel_count<I>(rows: I) -> usize
where
    I: IntoIterator<Item = (TunnelStatus, usize)>,
{
    rows.into_iter()
        .filter(|(status, connections)| {
            *connections > 0
                && matches!(
                    status,
                    TunnelStatus::Active
                        | TunnelStatus::Connecting
                        | TunnelStatus::Connected
                        | TunnelStatus::Reconnecting
                )
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_uses_live_connections_not_saved_records() {
        assert_eq!(
            active_tunnel_count([
                (TunnelStatus::Active, 1),
                (TunnelStatus::Connected, 2),
                (TunnelStatus::Disconnected, 3),
                (TunnelStatus::Failed, 4),
                (TunnelStatus::Active, 0),
            ]),
            2
        );
    }

    #[test]
    fn reconnecting_connection_is_ready_but_empty_records_are_not() {
        assert_eq!(
            active_tunnel_count([
                (TunnelStatus::Reconnecting, 1),
                (TunnelStatus::Connecting, 0),
                (TunnelStatus::Revoked, 1),
            ]),
            1
        );
    }
}
