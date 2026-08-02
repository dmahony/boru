# Boru Secure Tunnels

## Overview

Secure Tunnels let a Boru user share a **local TCP service** (for example a
development web server on `127.0.0.1:3000`, an SSH agent, or a local
dashboard) with **one selected friend** through an encrypted peer-to-peer
tunnel. The friend reaches the service through a loopback address on their
own machine — there is no port forwarding, no public inbound port, and no
third-party relay that can read the traffic.

```
Your machine                                Friend's machine

Local TCP app  ──►  Boru Tunnel  ──►  encrypted Iroh/QUIC  ──►  Boru Tunnel  ──►  Local TCP app
(127.0.0.1:3000)    (owner side)         stream (TLS 1.3)       (receiver)     (127.0.0.1:auto)
```

Boru Secure Tunnels are implemented natively inside Boru using the existing
Iroh networking infrastructure — the same endpoint, identity, friends store,
address resolution, relay support, and NAT traversal used by chat, file
sharing, and discovery. There is no second networking stack and no
dumbpipe-style standalone relay.

This document describes what Secure Tunnels are, how they work, their
security and threat model, direct vs relay connections, how to share and
connect to a service, expiration, revocation, and current limitations.

- Protocol: [`/boru-tunnel/1`](#protocol) (ALPN) over the shared Iroh endpoint
- Transport: Iroh/QUIC with TLS 1.3, mutual Ed25519 authentication
- Access control: recipient-bound, expiring, signed capability tokens

---

## What Secure Tunnels are

A Secure Tunnel maps one **owner-chosen loopback TCP target** onto an
authenticated, encrypted, bidirectional Iroh QUIC stream between two Boru
peers.

Key properties:

- **One local service, one selected friend.** A tunnel is created by the
  owner for exactly one allowed peer. No other peer can connect, even if they
  learn the tunnel identifier, because the capability is bound to the
  recipient's endpoint identity.
- **Loopback-only targets.** The owner chooses the local target (for example
  `127.0.0.1:3000`) when the tunnel is created. The remote peer can never
  select a destination — the request names only an existing tunnel, never an
  arbitrary host or port. This is the SSRF-protection invariant of the
  design.
- **Encrypted in transit.** All tunnel traffic is encrypted by Iroh/QUIC
  (TLS 1.3) end to end. An attacker on the network path cannot read or modify
  the forwarded bytes.
- **Ephemeral by design.** Tunnel definitions live in the `TunnelService`
  process memory. Creating a tunnel does not write a capability or target to
  SQLite; a restart removes all active definitions. See
  [Expiration](#expiration) and
  [docs/secure-tunnels-design.md](secure-tunnels-design.md#persistence-decision-phase-12).
- **Deliberate network-level grant.** Sharing a local service deliberately
  grants the selected peer **network-level access to that specific service**.
  While the tunnel is active, the friend can connect to that service exactly
  as if they were on the local machine. The grant is to the service, not to
  the whole machine, and it is time-limited and revocable.

---

## How they work

### Architecture

```
┌────────────────────────────┐            ┌────────────────────────────┐
│ Owner (sharer)             │            │ Receiver (friend)          │
│                            │            │                            │
│ TunnelService              │            │ LocalTunnelListener        │
│  • tunnel definition       │            │  • binds 127.0.0.1:<auto>  │
│  • target (loopback)       │            │  • forwards local TCP      │
│  • allowed peer            │            │    to the QUIC stream      │
│  • expiry / status         │            │                            │
│                            │            │                            │
│ /boru-tunnel/1 handler     │            │ open_tunnel()              │
│  • verify capability       │  ◄─QUIC──► │  • dial owner endpoint     │
│  • forward to local target │  TLS 1.3   │  • send TunnelRequest::Open│
└────────────────────────────┘            └────────────────────────────┘
```

The tunnel uses the **shared Boru Iroh endpoint**: one process, one
`iroh::Endpoint`, one identity, one protocol router. The `/boru-tunnel/1`
handler is registered on that router alongside the existing gossip, inbox,
whisper, backfill, catalogue, and file-access ALPNs. No extra endpoint is
created.

### Protocol steps

1. **Owner creates a tunnel.** The owner calls
   `TunnelService::create_tunnel_for_duration` with a random `TunnelId`, the
   owner's public key, a loopback `TunnelTarget`, the allowed peer's public
   key, and a duration. The service rejects non-loopback targets, invalid
   expiries, and duplicate IDs. (GUI: the "Share local service" dialog, see
   [How to share a service](#how-to-share-a-service).)

2. **Owner signs a capability.** `TunnelCapability::sign` produces an
   Ed25519-signed, recipient-bound token containing the tunnel ID, owner and
   allowed-peer endpoint IDs, `created_at_ms` / `expires_at_ms`, and a random
   nonce. The signature covers every field except the signature itself.

3. **Owner sends the offer.** The owner sends a `TunnelOffer` — the tunnel
   ID, the signed capability, a human-readable service name, an HTTP flag,
   the owner's current endpoint address, and an expiry — over the existing
   signed contact control channel (whisper) as a
   `ContactAction::TunnelOffer`. The offer names only an existing tunnel,
   never an arbitrary destination.

4. **Receiver verifies and connects.** The receiver verifies the signed
   contact message and stores the offer. When the user connects, the
   receiver's `LocalTunnelListener` binds a **loopback** address
   (`127.0.0.1`, auto-selected port), dials the owner's endpoint with the
   `/boru-tunnel/1` ALPN, opens a QUIC bidirectional stream, and sends
   `TunnelRequest::Open { protocol_version, tunnel_id, capability }`.

5. **Owner validates and forwards.** The `/boru-tunnel/1` handler on the
   owner side checks the protocol version, applies a per-peer connection
   attempt rate limit, looks up the tunnel, verifies the capability against
   the authenticated connection (owner identity, recipient identity, tunnel
   ID, expiry, tunnel active), enforces connection limits, connects to the
   local loopback target, and replies `TunnelResponse::Accepted`. Then both
   sides forward bytes bidirectionally until either side ends.

6. **Forwarding.** Each local TCP connection maps to one QUIC bidirectional
   stream. The two directions are copied concurrently with explicit
   cancellation: EOF in one direction half-closes that direction while the
   other drains; an error or cancellation stops both. The owner side applies
   an idle timeout (5 minutes) and connection/handshake timeouts (10 s / 5 s).

### Limits and timeouts

| Constant | Value | Meaning |
|---|---|---|
| `MAX_ACTIVE_SHARED_TUNNELS` | 32 | Max owner-configured tunnels per service |
| `MAX_ACTIVE_RECEIVED_TUNNELS` | 32 | Max simultaneously received tunnel streams |
| `DEFAULT_MAX_CONNECTIONS_PER_TUNNEL` | 16 | Default simultaneous connections per tunnel |
| `MAX_CONNECTION_ATTEMPTS_PER_INTERVAL` | 8 | Max new tunnel streams from one peer per window |
| `CONNECTION_ATTEMPT_INTERVAL` | 60 s | Rate-limit window |
| `TUNNEL_CONNECTION_TIMEOUT` | 10 s | Bounds endpoint/local-service connection setup |
| `TUNNEL_HANDSHAKE_TIMEOUT` | 5 s | Bounds each tunnel handshake read |
| `TUNNEL_IDLE_TIMEOUT` | 5 min | Max time a tunnel may remain completely idle |
| `MAX_HANDSHAKE_SIZE` | 64 KiB | Bounds handshake frames |

---

## Security model

### Transport encryption

All tunnel traffic is **encrypted in transit by Iroh/QUIC using TLS 1.3**.
Every connection is mutually authenticated: each peer proves its Ed25519
identity during the QUIC handshake. The forwarded application bytes, the
tunnel handshake frames, and the capability exchange are all carried over
this encrypted channel. An attacker on the network path cannot read, modify,
or inject tunnel traffic between the two peers.

### Identity and authorisation

- **Mutual authentication.** Only the two peers with the matching Ed25519
  keys can establish the QUIC connection. The tunnel is bound to the
  authenticated peer identity, never to a bare address.
- **Recipient-bound capabilities.** A `TunnelCapability` names exactly one
  allowed peer (`allowed_peer_endpoint_id`). The owner's handler verifies
  that the authenticated requesting peer matches the capability before any
  stream is forwarded. A capability stolen from the wire (or from an offer
  message) cannot be used by anyone other than the named recipient.
- **Signed capabilities.** The capability is signed by the owner's Ed25519
  key over every field (version, tunnel ID, owner, recipient, timestamps,
  nonce). Any tampering — including changing the expiry, tunnel ID, or
  recipient — invalidates the signature.
- **No destination selection by the remote peer.** The tunnel request carries
  only a tunnel ID and a capability; the local target is chosen by the owner
  at creation time and enforced loopback-only by `TunnelService`. A remote
  peer can never ask for an arbitrary host or port. This is verified by the
  test `remote_payload_cannot_select_a_different_target`.
- **Deliberate network-level grant.** Sharing a local service deliberately
  grants the selected peer **network-level access to that specific service**
  — the same access the owner's local apps have. The grant is to the named
  service only, is bounded by the tunnel's expiry and connection limits, and
  can be revoked. It is not a grant to the whole machine or to other services.

### Validation order

The owner-side handler rejects a stream if any of the following fails (in a
deliberately non-revealing order):

1. Protocol version mismatch → `ProtocolMismatch`
2. Per-peer connection attempt rate limit → `Busy`
3. Unknown tunnel ID → `UnknownTunnel`
4. Capability verification:
   - Bad/malformed signature → `InvalidCapability`
   - Owner mismatch → `InvalidCapability`
   - Recipient mismatch → `InvalidCapability`
   - Tunnel ID mismatch → `InvalidCapability`
   - Capability not yet valid → `InvalidCapability`
   - Expired → `Expired`
   - Unsupported capability version → `InvalidCapability`
   - Tunnel revoked / inactive → `InvalidCapability`
5. Connection limit (per-tunnel or global) → `Busy`
6. Local target unavailable → `TargetUnavailable`

Failure responses are structured and generic; they do not disclose local
implementation details.

### Capability material handling

Capability tokens and serialized handshakes are never written to logs.
Diagnostics may expose non-secret metadata (redacted tunnel ID prefix,
lifecycle state, expiry state, connection counts) but never the capability
token or stream contents.

---

## Threat model

### Assumed adversary capabilities

- **Network observer**: can see that two Boru peers are exchanging QUIC
  packets, observe packet timing and sizes, and see the Iroh relay URLs and
  direct addresses used. Cannot read or modify tunnel payloads (TLS 1.3).
- **Non-recipient Boru peer**: has a valid Iroh identity but is not the
  named recipient. Can attempt to connect to `/boru-tunnel/1` and present
  any capability it can obtain. It will be rejected because the capability is
  bound to a different recipient.
- **Recipient peer**: has the intended access while the tunnel is active, and
  can (as with any service access) behave badly toward the service itself.
- **Relay operator**: can observe that two peers are relaying through it, and
  packet timing/sizes. Cannot read plaintext traffic (QUIC/TLS 1.3 end to
  end). This is the same trust model as the rest of Boru.
- **Malicious local process on either host**: outside the application's
  threat model; a compromised host can read memory and files regardless of
  tunnel design.

### Protections

| Threat | Protection |
|---|---|
| Eavesdropping / tampering on the wire | Iroh/QUIC TLS 1.3, mutual auth, encrypted streams |
| A different peer tries to use a shared tunnel | Recipient-bound capability; recipient mismatch rejected |
| Capability replay / theft | Signature covers all fields; bound to tunnel + recipient; expires; random nonce |
| Remote peer selects an arbitrary destination | Request carries only tunnel ID + capability; target is owner-configured loopback; `NonLoopbackTarget` at creation |
| Expired / revoked tunnel still used | Expiry checked on every connect; revoked tunnels rejected (`TunnelInactive`) |
| Connection flooding / resource exhaustion | Per-tunnel and global connection limits; per-peer attempt rate limit; handshake size bound; timeouts (connect, handshake, idle) |
| Capability material leaked in logs | Capabilities and stream contents are never logged |
| Tunnel definitions survive restart / stale state | Ephemeral in-memory service; restart clears all tunnels |

### What this model does not claim

- A tunnel does not protect the local service from the recipient's behaviour
  — the recipient is deliberately granted network-level access to that
  service.
- Tunnel encryption does not protect a compromised local host.
- The tunnel does not anonymise traffic: relay operators and network
  observers can still see that two peers communicate and how much traffic
  flows.
- The offer's service name and HTTP flag are display metadata chosen by the
  owner; they are authenticated by the signed contact message but are not a
  guarantee about the service's actual protocol.

---

## Direct vs relay connections

Boru Secure Tunnels reuse the endpoint's existing connection machinery —
there is no second connection mechanism.

- **Direct connection**: when the two peers can reach each other directly
  (same LAN, or successful NAT traversal), the QUIC connection carries the
  tunnel traffic peer-to-peer. No relay is involved in the data path.
- **Relay connection**: when direct connection is not possible (both peers
  behind restrictive NAT, no port forwarding), the connection is established
  through the configured Iroh relay, which forwards encrypted packets between
  the peers. The relay never sees plaintext.
- **Address hints**: the `TunnelOffer` carries the owner's current endpoint
  address (relay URLs and any published direct addresses) so the receiver can
  dial it. The receiver's `LocalTunnelListener` records whether the route it
  used was `relay` or `direct` based on the owner's advertised addresses, or
  `unknown` when no address hints are available.
- **Reporting**: when reliable path information is available, tunnel UI can
  report whether a connection is direct or relayed. When it is not
  (no reliable path data), it reports connected/unknown rather than guessing.

Both modes use the identical `/boru-tunnel/1` protocol and capability
validation; the route only changes how the QUIC packets travel.

---

## How to share a service

Sharing is done from the friend profile (the feature is available for any
confirmed friend).

1. Open the friend's profile (click the "…" menu next to their name in the
   FRIENDS sidebar section, or open the friend profile screen).
2. Choose **"Share local service"** from the friend profile menu.
3. In the "Share Local Service" dialog:
   - **Service name** — an optional human-readable label (defaults to
     "Development Server").
   - **Local port** — the TCP port of the service running on your machine
     (for example `3000`). Only `127.0.0.1` is used as the target address.
   - **Share with** — the selected friend (fixed to the friend whose profile
     you are on).
   - **Expires after** — one of: 10 minutes, 30 minutes, 1 hour, 8 hours,
     or until Boru exits.
4. Review the warning: *"`<friend>` will be able to connect to this local
   service while the tunnel is active."* This is the deliberate network-level
   grant described throughout this document.
5. Click **Share**. A tunnel is created, a capability is signed, and the
   offer is sent to the friend over the signed contact channel.

The service does **not** need to be publicly reachable — it just needs to be
listening on a loopback port on your machine.

---

## How to connect to a service

When a friend shares a service with you, Boru delivers the signed offer over
the contact channel.

1. You receive the shared-service offer from your friend, showing the
   service name, the sharer, and the expiry.
2. Choose **Connect**. Boru binds a **loopback listener** on your machine
   (`127.0.0.1` with an automatically selected port), dials your friend's
   endpoint, and presents the signed capability.
3. The friend's Boru validates the capability and connects to their local
   service. Your local listener now forwards to the shared service over the
   encrypted tunnel.
4. Point your local application at the displayed loopback address — for
   example open `http://127.0.0.1:43827` in a browser for an HTTP service, or
   `ssh -p <port> 127.0.0.1` for an SSH service.

While connected, your app talks to the shared service exactly as if it were
running on your own machine. Disconnect when you are done; the owner can also
revoke the tunnel at any time.

> **Note on the GUI surface:** the backend transport, capability validation,
> local listeners, and the "Share local service" dialog are implemented and
> tested. The received-offer/connect/disconnect GUI flow is being completed
> as part of the active tunnel management work; the protocol and service APIs
> described here are the stable interface the UI builds on.

---

## Expiration

Every tunnel and capability has an expiry.

- **Duration choices**: 10 minutes, 30 minutes, 1 hour, 8 hours, or until
  Boru exits (`UntilExit`). The chosen duration is converted to an absolute
  `expires_at_ms` timestamp at creation.
- **Capability expiry**: the signed capability contains
  `created_at_ms`/`expires_at_ms`; the owner rejects a capability that is
  before `created_at_ms` (`NotYetValid`) or after `expires_at_ms`
  (`Expired`).
- **Tunnel expiry**: `TunnelService` also checks the tunnel's own
  `expires_at_ms` when connecting (`connect_tunnel`) and when acquiring a
  connection slot (`try_acquire_connection`); an expired tunnel rejects new
  connections.
- **In-flight streams**: an already-open stream is not killed the instant the
  clock passes expiry; it is bounded by the idle timeout and by revocation.
  Expiration prevents **new** connections; revocation is the mechanism for
  cutting existing access (see below).
- **UI**: the share dialog shows how long the shared service remains
  available.

---

## Revocation

The owner can revoke a tunnel at any time:

- **`revoke_tunnel`** removes the tunnel definition and marks it revoked.
  Existing streams continue until they close naturally; no new connections
  are accepted.
- **`revoke_tunnel_with_termination(terminate_existing = true)`** additionally
  cancels existing streams immediately via the tunnel's cancellation token,
  cutting off an active peer right away.

After revocation:

- The tunnel no longer appears in the service's active list.
- New connection attempts are rejected: the handler no longer finds the
  tunnel (`UnknownTunnel`) or the capability verification fails because the
  tunnel is no longer active (`TunnelInactive` → `InvalidCapability`).
- A revoked tunnel is gone for the life of the process; recreating it is a
  new share with a new capability.

Revocation is the owner's kill switch: sharing a service grants network-level
access to that service, and revocation takes that access away.

---

## Current limitations

- **Loopback TCP only.** v1 supports local TCP targets on loopback addresses
  (`127.0.0.1`). Non-loopback and wildcard targets are rejected at creation.
  Unix socket targets and non-loopback local listeners are future work (see
  [docs/secure-tunnels-design.md](secure-tunnels-design.md#future-use-cases-phase-23)).
- **Ephemeral definitions.** Tunnels exist for the life of the Boru process.
  They are not persisted to SQLite and do not survive restart. This is a
  deliberate design decision, not an oversight — capabilities are
  authorisation secrets and are not written to durable storage without a
  recovery/access-control design. See
  [docs/secure-tunnels-design.md](secure-tunnels-design.md#persistence-decision-phase-12).
- **One recipient per tunnel.** Each tunnel authorises exactly one peer. To
  share with several friends, create several tunnels (up to
  `MAX_ACTIVE_SHARED_TUNNELS` = 32).
- **Bounded concurrency.** At most 32 simultaneously received tunnel streams
  service-wide, 16 simultaneous connections per tunnel by default, and 8 new
  tunnel streams per peer per 60-second window. These limits are deliberate
  abuse protection; they can be tuned via the service constants.
- **Byte-stream protocol.** `/boru-tunnel/1` is a reliable byte-stream
  transport. It is not designed for realtime media (voice/video) or UDP
  game traffic; those are explicitly out of scope for v1 (see the design
  doc's future-use-cases boundary).
- **No CLI command.** Tunnel control is exposed through the in-process
  service API and the GUI; there is no standalone `boru tunnel share/connect`
  CLI command (see the design doc's Phase 21 note).
- **GUI surface.** The "Share local service" dialog is implemented. The
  received-offer/connect/disconnect and active-tunnel-management GUI flows
  are being completed on top of the stable protocol and service APIs.
- **Relay visibility.** When a relay is used, the relay operator can observe
  that two peers communicate and how much traffic flows (encrypted), the same
  as all other Boru traffic.
- **Service-level trust.** A tunnel grants the recipient network-level access
  to the shared service; the owner must treat the service as exposed to that
  friend for the tunnel's lifetime. Use short expiries and revocation for
  anything sensitive.

---

## Related documentation

- [`secure-tunnels-design.md`](secure-tunnels-design.md) — integration design,
  dumbpipe concept review, persistence decision, future use cases, invariants
- [`security-model.md`](security-model.md) — cryptographic and storage
  security properties across Boru
- [`privacy-model.md`](privacy-model.md) — what metadata peers see, path
  protection, authorization privacy
- [`networking-audit.md`](networking-audit.md) — endpoint, ALPN, and
  connection architecture
