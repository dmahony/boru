# 16 — Optional control-plane extensions (BORU-CP-16 / PDF Phase 6)

Status: implemented (2026-08-13) · Build: `net` (service); wire types always available

## What this is

PDF Phase 6 asks for optional extensions on the discovery control plane. They
land as one new control-plane message type (`EXTENSIONS`, tag `4`) carrying a
typed, optional-section metadata payload. Every extension is **metadata-only
by construction**: the payload has no field that can carry file bytes, tunnel
data, media, session keys, credentials, or LAN topology, and the privacy layer
bounds the variable-size fields so even a malicious peer cannot smuggle
content through the metadata channels.

## The eight extensions

| # | Extension | Type | Carries | Never carries |
|---|-----------|------|---------|---------------|
| 1 | Group availability hints | `GroupHints { available }` | coarse "may join an existing group" flag | group ids, membership lists |
| 2 | File-transfer readiness | `FileReadiness { protocol_versions, can_receive }` | protocol versions + coarse can-receive | file bytes, file names, hashes |
| 3 | Tunnel capability | `TunnelCapability { protocol_versions }` | protocol versions | ports, destinations, credentials, traffic |
| 4 | Voice/video call capability | `CallCapability { protocol_versions, availability? }` | protocol versions + coarse availability | call offers, session keys, media |
| 5 | Screen-share capability | `ScreenShareCapability { protocol_versions }` | protocol versions | VNC/video data |
| 6 | Multi-device identity | `MultiDeviceIdentity { identity_id, device_id, active_device }` | higher-level identity + device ids + active selection | pretending all devices are one endpoint; the envelope's `sender_node_id` stays the per-device endpoint |
| 7 | LAN/direct-path preference | `PathPreference` enum | coarse hint (DirectPreferred / RelayPreferred / NoPreference) | raw LAN topology, IPs, addresses |
| 8 | Relay preference/health hints | `RelayHealthHint` enum | coarse hint (Healthy / Degraded / Unknown) | relay state, addresses |

## Wire format

- New `ControlMessageType::Extensions = 4` (stable tag, `from_u8(4)`).
- New `ControlPayload::Extensions(ExtensionsPayload)` variant **appended at the
  end** of the enum so existing postcard variant indices stay stable.
- Every `ExtensionsPayload` field is `#[serde(default)]` and optional; an
  all-`None` payload is the empty advertisement and is never broadcast.
- Backward compatibility: an older client that does not know tag `4` returns
  `ControlPlaneDecode::UnknownType` and fails closed for that feature without
  breaking the client. A client that knows the message type but not a future
  extension section still decodes the payload (forward compatible).

## Privacy layer (src/control_plane/privacy.rs)

- `ControlAdvertPolicy` gains `extensions_bounds: ExtensionsBounds`
  (defaults: 16 protocol versions per section, 32 chars per version,
  64 chars per identity/device id).
- `check()` validates `ExtensionsPayload` via `payload.validate(&bounds)` and
  maps violations to the new `AdvertViolation::Extensions(ExtensionsViolation)`
  variant (numeric, `Copy`).
- `PeerControlState` gains `extensions: Option<ExtensionsPayload>` cached from
  EXTENSIONS envelopes; `record()` refreshes it on a newer sequence and the
  cache dies with presence (TTL).
- New accessor `PeerControlStateStore::extensions_of(node_id)`.

## Discovery service (src/discovery_service.rs)

- `DiscoveryService` holds `local_extensions: Arc<Mutex<ExtensionsPayload>>`
  (default `default_local_extensions()`, built from the well-known capability
  registry) and a separate `extensions_throttle` + `last_announced_extensions`
  on the `ControlAnnounceHandle` (idempotence + rate limit, like capabilities).
- `join()` announces HELLO → CAPABILITIES → EXTENSIONS back-to-back (each with
  its own throttle).
- `presence_refresh_loop` re-announces extensions every
  `extensions_every` ticks (`DEFAULT_EXTENSIONS_REFRESH_EVERY = 3`), so peers
  that join later still learn the payload.
- Public API:
  - `local_extensions() -> ExtensionsPayload`
  - `update_local_extensions(payload) -> AnnounceOutcome` (stores + announces
    on material change; unchanged/empty payloads are idempotent no-ops)
  - `announce_extensions() -> AnnounceOutcome`
  - `peer_extensions(node_id) -> Option<ExtensionsPayload>` (active presence
    only; stale data is never treated as current)
  - Builders `with_extensions_announce_min_interval`, `with_extensions_refresh_every`.

## Guardrails kept

- **Metadata-only by construction** — the typed payload has no content field;
  bounds reject smuggling attempts (test: 4 KiB "file bytes" in a version
  field is rejected).
- **No authorisation by presence** — advertising an extension never makes a
  peer a friend, group member, tunnel client, or file recipient; the private
  paths still enforce authorisation (test: `extensions_advertisement_never_authorises`).
- **No control-plane/chat coupling** — EXTENSIONS rides the existing discovery
  topic; the receive path yields `IncomingOutcome::ControlMessage` and never
  creates a conversation/peer update (test: `extensions_envelope_never_touches_peer_registry`).
- **Deterministic topic ownership** — no new topic subscriptions.
- **Bounded resources** — payload stays tiny on the wire (< 512 B fully
  populated), refreshed on a cadence, throttled per sender.

## Tests

- `src/control_plane/extensions.rs` — model, bounds, no-content-smuggling,
  wire round-trip, envelope round-trip, stable section tags.
- `src/control_plane/message.rs` — Extensions tag stability, sample payloads,
  convenience constructor round-trip, chat-separation.
- `src/control_plane/privacy.rs` — policy bounds, extensions cache + refresh
  + TTL expiry.
- `src/discovery_service.rs` — announce path, idempotence, material-change
  announce, peer cache, TTL expiry, no-authorisation.
- `tests/test_extensions_metadata.rs` — two-node round trip over the real
  discovery topic, wire-level metadata-only proof, peer-registry isolation.
