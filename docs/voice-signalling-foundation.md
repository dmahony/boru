# Voice signalling foundation (VOC-01..02)

## Scope

Boru call setup uses an authenticated, reliable Iroh call-control connection. Chat remains on its existing gossip topics; media frames are not encoded as chat messages and are not sent over gossip. Once a call is accepted, the existing call actor owns the control stream and QUIC datagram media path.

The shared session state machine in `src/call/session.rs` is the protocol-independent boundary for call lifecycle decisions. It is deliberately small and transport-neutral so the actor, UI, and future reconnect coordinator can share the same rules without sharing media buffers.

## Signalling contract

`Offer -> Accept` establishes a call. `Decline` and `Leave` are terminal and idempotent. `Reconnect` temporarily moves an active call to `Reconnecting`; a subsequent `Accept` restores `Active` without creating a second session. Duplicate messages are harmless, and messages from a peer other than the authenticated Iroh connection's remote identity are rejected before state is inspected or changed.

The call-control wire protocol remains `CALL_ALPN` plus length-prefixed postcard frames. `CallControl` carries capability negotiation, safe reject/hangup reasons, and media-state updates. The session abstraction is not a second transport or a gossip protocol; it centralizes lifecycle/authentication semantics for that existing control path.

## Timeouts and diagnostics

Offers and reconnects are bounded by the caller's deadline. `expire(now)` transitions an overdue non-terminal session to `Ended` with `Timeout`. Diagnostics are local-only counters (accepted, declined, leaves, reconnects, unauthorized messages, and timeouts); they never include media payloads, addresses, or credentials.

## Authentication boundary

The Iroh connection is authenticated by Iroh. The session additionally pins the expected peer identity and rejects any signal whose sender does not equal `Connection::remote_id()`. Authorization remains a separate policy gate in `CallHandle`/`CallProtocol`; capability advertisements never grant authorization.
