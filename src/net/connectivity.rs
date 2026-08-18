//! Relay/direct connectivity: the per-connection send/recv loop and the
//! address info (de)serialisation plus transport-path selection.

use std::{collections::BTreeSet, net::SocketAddr};

use iroh::{endpoint::Connection, EndpointAddr, PublicKey, RelayUrl};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, error_span, Instrument};

use super::peer::{ConnOrigin, ConnectionLoopError};
use super::protocol::{InEvent, ProtoMessage};
use super::util::{RecvLoop, SendLoop};
use crate::proto::PeerData;

pub(super) async fn connection_loop(
    from: PublicKey,
    conn: Connection,
    origin: ConnOrigin,
    send_rx: mpsc::Receiver<ProtoMessage>,
    in_event_tx: mpsc::Sender<InEvent>,
    max_message_size: usize,
    queue: Vec<ProtoMessage>,
) -> Result<(), ConnectionLoopError> {
    debug!(?origin, "connection established");

    let mut send_loop = SendLoop::new(conn.clone(), send_rx, max_message_size);
    let mut recv_loop = RecvLoop::new(from, conn, in_event_tx, max_message_size);

    let send_fut = send_loop.run(queue).instrument(error_span!("send"));
    let recv_fut = recv_loop.run().instrument(error_span!("recv"));

    let (send_res, recv_res) = tokio::join!(send_fut, recv_fut);
    send_res?;
    recv_res?;
    Ok(())
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub(super) struct AddrInfo {
    pub(super) relay_url: Option<RelayUrl>,
    pub(super) direct_addresses: BTreeSet<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransportPath {
    Direct,
    Relay,
}

pub(super) fn select_transport(endpoint_addr: &EndpointAddr) -> Option<TransportPath> {
    if endpoint_addr.ip_addrs().next().is_some() {
        Some(TransportPath::Direct)
    } else if endpoint_addr.relay_urls().next().is_some() {
        Some(TransportPath::Relay)
    } else {
        None
    }
}

impl From<EndpointAddr> for AddrInfo {
    fn from(endpoint_addr: EndpointAddr) -> Self {
        Self {
            relay_url: endpoint_addr.relay_urls().next().cloned(),
            direct_addresses: endpoint_addr.ip_addrs().cloned().collect(),
        }
    }
}

pub(super) fn encode_peer_data(info: &AddrInfo) -> PeerData {
    let bytes = postcard::to_stdvec(info).expect("serializing AddrInfo may not fail");
    PeerData::new(bytes)
}

pub(super) fn decode_peer_data(peer_data: &PeerData) -> Result<AddrInfo, postcard::Error> {
    let bytes = peer_data.as_bytes();
    if bytes.is_empty() {
        return Ok(AddrInfo::default());
    }
    let info = postcard::from_bytes(bytes)?;
    Ok(info)
}
