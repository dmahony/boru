//! Protocol registration: gossip ALPN constants and the protocol message/event
//! type aliases shared by the engine submodules.

use iroh::PublicKey;

use crate::proto;

/// ALPN protocol name
pub const GOSSIP_ALPN: &[u8] = b"/iroh-gossip/1";

/// ALPN for the file access (transfer authorisation) protocol.
///
/// Performs a request-time permission, availability, and integrity check
/// before issuing a short-lived signed download descriptor.  See
/// `docs/protocol-layers.md` for the full protocol specification.
/// This constant is not yet registered on any router — registration is
/// deferred until the file access handler module is built.
pub const FILE_ACCESS_ALPN: &[u8] = b"/boru-file-access/1";

/// Events emitted from the gossip protocol
pub type ProtoEvent = proto::Event<PublicKey>;

/// Short tag for diagnostic logging of a proto event kind.
pub(super) fn event_kind_tag(event: &ProtoEvent) -> &'static str {
    match event {
        ProtoEvent::NeighborUp(_) => "NeighborUp",
        ProtoEvent::NeighborDown(_) => "NeighborDown",
        ProtoEvent::Received(_) => "Received",
        ProtoEvent::MissingMessages { .. } => "MissingMessages",
    }
}
/// Commands for the gossip protocol
pub type ProtoCommand = proto::Command<PublicKey>;

pub(super) type InEvent = proto::InEvent<PublicKey>;
pub(super) type OutEvent = proto::OutEvent<PublicKey>;
pub(super) type Timer = proto::Timer<PublicKey>;
pub(super) type ProtoMessage = proto::Message<PublicKey>;
