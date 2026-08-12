//! Phase 6 — Optional control-plane extensions (metadata-only).
//!
//! PDF Phase 6 asks for optional extensions that stay **metadata-only** and
//! are authenticated/authorised by each feature's normal private path. This
//! module defines the typed metadata model for all eight extensions:
//!
//! 1. **Group availability hints** — advertise that a known peer is online
//!    and may participate in an existing group. Never broadcast group
//!    membership on the global discovery topic; group reachability is
//!    derived from known local memberships ([`GroupHints`] carries only a
//!    coarse `available` flag, never a group list).
//! 2. **File-transfer readiness** — advertise file-transfer protocol
//!    versions and current ability to receive ([`FileReadiness`]). The
//!    actual file request, consent, metadata, and encrypted bytes stay on
//!    the private file-transfer path.
//! 3. **Tunnel capability** — advertise support for Boru tunnel protocol
//!    versions ([`TunnelCapability`]), NOT open ports, tunnel destinations,
//!    credentials, or traffic. Actual tunnel setup remains authorised and
//!    private.
//! 4. **Voice/video call capability** — advertise supported
//!    signalling/media protocol versions and optionally coarse availability
//!    ([`CallCapability`]). Actual call offers, session keys, sensitive
//!    transport details, and media remain private.
//! 5. **Screen-share capability** — advertise that the client supports
//!    screen sharing and its protocol version ([`ScreenShareCapability`]).
//!    Session negotiation and VNC/video/media data remain on the private
//!    authenticated session.
//! 6. **Multi-device identity support** — extend peer metadata so multiple
//!    devices can advertise under a higher-level identity without pretending
//!    all devices are one network endpoint ([`MultiDeviceIdentity`] defines
//!    a `device_id` per device plus a shared `identity_id` and an
//!    `active_device` selection; the envelope's `sender_node_id` remains the
//!    per-device network endpoint).
//! 7. **LAN/direct-path preference** — use networking-layer path
//!    information to prefer efficient local/direct connectivity where
//!    available ([`PathPreference`]). Raw LAN topology is never broadcast.
//! 8. **Relay preference/health hints** — discovery carries only coarse
//!    compatibility or current reachability hints ([`RelayHealthHint`]).
//!    Authoritative relay/path selection stays in Iroh/networking code, not
//!    application chat logic.
//!
//! # Design rules (from the PDF + the BORU-CP chain)
//!
//! * **Metadata-only by construction.** Every field is a bounded string, a
//!   boolean, or a coarse enum. There is no field that can carry file bytes,
//!   tunnel data, media, session keys, credentials, or LAN topology — and
//!   [`ExtensionsBounds`] caps the variable-size fields so even a malicious
//!   peer cannot smuggle content through the metadata channels.
//! * **Optional sections.** Every extension is an `Option`; a peer
//!   advertises only the sections it supports. An all-`None` payload is the
//!   empty advertisement ([`ExtensionsPayload::is_empty`]).
//! * **Forward compatible.** Fields are `#[serde(default)]`, so an older
//!   client that does not know a future extension section still decodes the
//!   payload; an older client that does not know the *message type* at all
//!   gets [`ControlPlaneDecode::UnknownType`](crate::control_plane::message::ControlPlaneDecode::UnknownType)
//!   and fails closed for that feature without breaking the client.
//! * **No authorisation by presence.** Advertising an extension is metadata,
//!   never a grant: being discoverable does not make a peer a friend, group
//!   member, tunnel client, or file recipient. The private paths still
//!   enforce authorisation.
//! * **Bounded resources.** [`ExtensionsBounds`] caps the number of protocol
//!   version strings and their lengths; the privacy layer rejects
//!   advertisements that exceed them.

use serde::{Deserialize, Serialize};

// ── Bounds (bounded-resources guardrail) ──────────────────────────────────

/// Numeric section tags used by [`ExtensionsViolation`].
pub mod sections {
    /// File-transfer readiness (extension 2).
    pub const FILE: u8 = 0;
    /// Tunnel capability (extension 3).
    pub const TUNNEL: u8 = 1;
    /// Voice/video call capability (extension 4).
    pub const CALL: u8 = 2;
    /// Screen-share capability (extension 5).
    pub const SCREEN_SHARE: u8 = 3;
}

/// Default maximum number of protocol-version strings per extension section.
pub const DEFAULT_MAX_PROTOCOL_VERSIONS: usize = 16;

/// Default maximum length of one protocol-version string.
pub const DEFAULT_MAX_VERSION_LEN: usize = 32;

/// Default maximum length of a multi-device identity id.
pub const DEFAULT_MAX_IDENTITY_ID_LEN: usize = 64;

/// Default maximum length of a per-device id.
pub const DEFAULT_MAX_DEVICE_ID_LEN: usize = 64;

/// Bounds applied to an [`ExtensionsPayload`] by the privacy layer.
///
/// Mirrors the bounded-resources guardrail for capabilities: a peer cannot
/// grow our memory or smuggle content through the metadata channels beyond
/// these caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionsBounds {
    /// Maximum number of protocol-version strings per section.
    pub max_protocol_versions: usize,
    /// Maximum length of one protocol-version string.
    pub max_version_len: usize,
    /// Maximum length of a multi-device identity id.
    pub max_identity_id_len: usize,
    /// Maximum length of a per-device id.
    pub max_device_id_len: usize,
}

impl Default for ExtensionsBounds {
    fn default() -> Self {
        Self {
            max_protocol_versions: DEFAULT_MAX_PROTOCOL_VERSIONS,
            max_version_len: DEFAULT_MAX_VERSION_LEN,
            max_identity_id_len: DEFAULT_MAX_IDENTITY_ID_LEN,
            max_device_id_len: DEFAULT_MAX_DEVICE_ID_LEN,
        }
    }
}

/// Why an [`ExtensionsPayload`] violates the metadata bounds.
///
/// All fields are numeric so the enum stays `Copy` (it is surfaced through
/// [`AdvertViolation`](crate::control_plane::privacy::AdvertViolation)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionsViolation {
    /// A section advertises more protocol versions than the bound.
    TooManyVersions {
        /// [`sections`](mod@self::sections) tag.
        section: u8,
        /// Number of versions in the advertisement.
        count: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A protocol-version string is longer than the bound.
    VersionTooLong {
        /// [`sections`](mod@self::sections) tag.
        section: u8,
        /// Length of the offending string.
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A protocol-version string is empty or contains characters outside the
    /// metadata charset (`[A-Za-z0-9._-]`).
    VersionInvalid {
        /// [`sections`](mod@self::sections) tag.
        section: u8,
    },
    /// The multi-device identity id is longer than the bound.
    IdentityIdTooLong {
        /// Length of the offending id.
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// The per-device id is longer than the bound.
    DeviceIdTooLong {
        /// Length of the offending id.
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
}

/// Check one protocol-version string: non-empty, bounded length, metadata
/// charset. Returns the section-tagged violation on failure.
fn check_version(
    section: u8,
    version: &str,
    bounds: &ExtensionsBounds,
) -> Result<(), ExtensionsViolation> {
    if version.is_empty() || version.len() > bounds.max_version_len {
        return Err(ExtensionsViolation::VersionTooLong {
            section,
            len: version.len(),
            max: bounds.max_version_len,
        });
    }
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ExtensionsViolation::VersionInvalid { section });
    }
    Ok(())
}

/// Check a version list: count bound then per-string bounds.
fn check_versions(
    section: u8,
    versions: &[String],
    bounds: &ExtensionsBounds,
) -> Result<(), ExtensionsViolation> {
    if versions.len() > bounds.max_protocol_versions {
        return Err(ExtensionsViolation::TooManyVersions {
            section,
            count: versions.len(),
            max: bounds.max_protocol_versions,
        });
    }
    for version in versions {
        check_version(section, version, bounds)?;
    }
    Ok(())
}

// ── Extension 1: group availability hints ────────────────────────────────

/// Extension 1 — group availability hint.
///
/// A coarse flag only: "this peer is online and may participate in an
/// existing group". Group reachability is derived from known local
/// memberships by the advertising client; the discovery topic never carries
/// a group list, group ids, or membership details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupHints {
    /// Whether the peer currently advertises availability to participate in
    /// an existing group.
    #[serde(default)]
    pub available: bool,
}

// ── Extension 2: file-transfer readiness ─────────────────────────────────

/// Extension 2 — file-transfer readiness.
///
/// Protocol versions + coarse "can receive" flag. The actual file request,
/// consent, metadata, and encrypted bytes stay on the private file-transfer
/// path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadiness {
    /// Supported file-transfer protocol versions (e.g. `["v2"]`).
    #[serde(default)]
    pub protocol_versions: Vec<String>,
    /// Whether the peer can currently receive files (coarse availability).
    #[serde(default)]
    pub can_receive: bool,
}

// ── Extension 3: tunnel capability ───────────────────────────────────────

/// Extension 3 — tunnel capability.
///
/// Protocol versions only. Never open ports, tunnel destinations,
/// credentials, or traffic; actual tunnel setup remains authorised and
/// private.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelCapability {
    /// Supported Boru tunnel protocol versions (e.g. `["v1"]`).
    #[serde(default)]
    pub protocol_versions: Vec<String>,
}

// ── Extension 4: voice/video call capability ─────────────────────────────

/// Extension 4 — call capability.
///
/// Supported signalling/media protocol versions plus an optional coarse
/// availability. Actual call offers, session keys, sensitive transport
/// details, and media remain private.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallCapability {
    /// Supported signalling/media protocol versions (e.g. `["v1"]`).
    #[serde(default)]
    pub protocol_versions: Vec<String>,
    /// Optional coarse availability (`None` = not advertised).
    #[serde(default)]
    pub availability: Option<CallAvailability>,
}

/// Coarse call availability (extension 4). Never call content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallAvailability {
    /// The peer is available to take a call.
    Available,
    /// The peer is busy (e.g. already in a call).
    Busy,
    /// Availability is unknown / not advertised.
    Unknown,
}

// ── Extension 5: screen-share capability ─────────────────────────────────

/// Extension 5 — screen-share capability.
///
/// Protocol versions only. Session negotiation and VNC/video/media data
/// remain on the private authenticated session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenShareCapability {
    /// Supported screen-share protocol versions (e.g. `["v1"]`).
    #[serde(default)]
    pub protocol_versions: Vec<String>,
}

// ── Extension 6: multi-device identity ───────────────────────────────────

/// Extension 6 — multi-device identity.
///
/// Multiple devices advertise under a higher-level identity without
/// pretending all devices are one network endpoint: each device keeps its
/// own `sender_node_id` (the envelope's stable network endpoint), while this
/// payload carries the higher-level `identity_id`, a unique `device_id` per
/// device, and an `active_device` selection so peers can prefer the device
/// the user is currently using.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiDeviceIdentity {
    /// Higher-level identity shared by multiple devices of one user.
    pub identity_id: String,
    /// Unique id for this device under the identity.
    pub device_id: String,
    /// Whether this device is the currently active one for the identity.
    #[serde(default)]
    pub active_device: bool,
}

// ── Extension 7: LAN/direct-path preference ──────────────────────────────

/// Extension 7 — LAN/direct-path preference.
///
/// A coarse hint derived from networking-layer path information: prefer
/// efficient local/direct connectivity where available. Never broadcasts raw
/// LAN topology; the underlying transport (Iroh) owns the actual path
/// selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathPreference {
    /// Prefer direct/LAN paths when available.
    DirectPreferred,
    /// Prefer relay paths (e.g. NAT traversal constraints).
    RelayPreferred,
    /// No preference expressed.
    NoPreference,
}

// ── Extension 8: relay preference/health hints ───────────────────────────

/// Extension 8 — relay preference/health hint.
///
/// Coarse compatibility / current reachability hint only. Authoritative
/// relay/path selection stays in Iroh/networking code, not application chat
/// logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayHealthHint {
    /// The peer's relay path is currently healthy.
    Healthy,
    /// The peer's relay path is currently degraded.
    Degraded,
    /// Relay health is unknown / not advertised.
    Unknown,
}

// ── The payload ──────────────────────────────────────────────────────────

/// The Phase 6 optional extensions payload (metadata only).
///
/// Every section is optional; an all-`None` payload is the empty
/// advertisement. The privacy layer applies [`ExtensionsBounds`] before the
/// payload is cached or acted on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionsPayload {
    /// Extension 1 — group availability hint.
    #[serde(default)]
    pub group: Option<GroupHints>,
    /// Extension 2 — file-transfer readiness.
    #[serde(default)]
    pub file: Option<FileReadiness>,
    /// Extension 3 — tunnel capability.
    #[serde(default)]
    pub tunnel: Option<TunnelCapability>,
    /// Extension 4 — voice/video call capability.
    #[serde(default)]
    pub call: Option<CallCapability>,
    /// Extension 5 — screen-share capability.
    #[serde(default)]
    pub screen_share: Option<ScreenShareCapability>,
    /// Extension 6 — multi-device identity.
    #[serde(default)]
    pub identity: Option<MultiDeviceIdentity>,
    /// Extension 7 — LAN/direct-path preference.
    #[serde(default)]
    pub path_preference: Option<PathPreference>,
    /// Extension 8 — relay preference/health hint.
    #[serde(default)]
    pub relay_health: Option<RelayHealthHint>,
}

impl ExtensionsPayload {
    /// Whether every extension section is absent — the empty advertisement.
    pub fn is_empty(&self) -> bool {
        self.group.is_none()
            && self.file.is_none()
            && self.tunnel.is_none()
            && self.call.is_none()
            && self.screen_share.is_none()
            && self.identity.is_none()
            && self.path_preference.is_none()
            && self.relay_health.is_none()
    }

    /// Validate this payload against `bounds`.
    ///
    /// Returns `Ok(())` for a bounded, metadata-only advertisement;
    /// `Err(violation)` with the specific bound exceeded. Never panics.
    pub fn validate(&self, bounds: &ExtensionsBounds) -> Result<(), ExtensionsViolation> {
        if let Some(file) = &self.file {
            check_versions(sections::FILE, &file.protocol_versions, bounds)?;
        }
        if let Some(tunnel) = &self.tunnel {
            check_versions(sections::TUNNEL, &tunnel.protocol_versions, bounds)?;
        }
        if let Some(call) = &self.call {
            check_versions(sections::CALL, &call.protocol_versions, bounds)?;
        }
        if let Some(screen_share) = &self.screen_share {
            check_versions(
                sections::SCREEN_SHARE,
                &screen_share.protocol_versions,
                bounds,
            )?;
        }
        if let Some(identity) = &self.identity {
            if identity.identity_id.is_empty()
                || identity.identity_id.len() > bounds.max_identity_id_len
            {
                return Err(ExtensionsViolation::IdentityIdTooLong {
                    len: identity.identity_id.len(),
                    max: bounds.max_identity_id_len,
                });
            }
            if identity.device_id.is_empty() || identity.device_id.len() > bounds.max_device_id_len
            {
                return Err(ExtensionsViolation::DeviceIdTooLong {
                    len: identity.device_id.len(),
                    max: bounds.max_device_id_len,
                });
            }
        }
        Ok(())
    }
}

// ── Default local advertisement ──────────────────────────────────────────

/// The default local extensions advertisement (BORU-CP-16).
///
/// Built from the well-known capability registry
/// ([`KNOWN_CAPABILITIES`](crate::control_plane::capabilities::KNOWN_CAPABILITIES)):
/// for every capability this build implements, the matching Phase 6
/// extension section is advertised with the capability's version. Sections
/// with no build support (group hints, multi-device identity, path
/// preference, relay health) are left `None` — the app populates them via
/// [`DiscoveryService::update_local_extensions`](crate::discovery_service::DiscoveryService::update_local_extensions)
/// when it has the local state to derive them (e.g. group reachability from
/// known local memberships).
pub fn default_local_extensions() -> ExtensionsPayload {
    use crate::control_plane::capabilities::{features, ids, known_descriptor};

    let mut payload = ExtensionsPayload::default();

    if let Some(descriptor) = known_descriptor(ids::FILES_V2) {
        let version = descriptor.id.rsplit('-').next().unwrap_or("v1").to_string();
        payload.file = Some(FileReadiness {
            protocol_versions: vec![version],
            can_receive: true,
        });
    }
    if let Some(descriptor) = known_descriptor(ids::TUNNELS_V1) {
        let version = descriptor.id.rsplit('-').next().unwrap_or("v1").to_string();
        payload.tunnel = Some(TunnelCapability {
            protocol_versions: vec![version],
        });
    }
    // Voice and video share the call section: advertise the union of the
    // supported signalling/media versions.
    let mut call_versions: Vec<String> = Vec::new();
    for id in [ids::VOICE_V1, ids::VIDEO_V1] {
        if let Some(descriptor) = known_descriptor(id) {
            if let Some(version) = descriptor.id.rsplit('-').next() {
                let version = version.to_string();
                if !call_versions.contains(&version) {
                    call_versions.push(version);
                }
            }
        }
    }
    if !call_versions.is_empty() {
        payload.call = Some(CallCapability {
            protocol_versions: call_versions,
            availability: None,
        });
    }
    if let Some(descriptor) = known_descriptor(ids::SCREEN_SHARE_V1) {
        let version = descriptor.id.rsplit('-').next().unwrap_or("v1").to_string();
        payload.screen_share = Some(ScreenShareCapability {
            protocol_versions: vec![version],
        });
    }

    // `features` is used here to keep the feature-name list in sync at
    // compile time (group/identity/path/relay are app-derived sections, not
    // capability-backed).
    let _ = (
        features::FILES,
        features::TUNNELS,
        features::VOICE,
        features::VIDEO,
        features::SCREEN_SHARE,
        features::RICH_TEXT,
    );

    payload
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::capabilities::ids;

    fn key(byte: u8) -> crate::control_plane::message::ControlEnvelope {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        let sk = iroh_base::SecretKey::from_bytes(&seed);
        crate::control_plane::message::ControlEnvelope::extensions(
            sk.public(),
            1,
            1_700_000_000,
            ExtensionsPayload::default(),
        )
    }

    /// The default local advertisement covers every capability-backed
    /// extension section and leaves the app-derived sections unset.
    #[test]
    fn default_local_extensions_covers_capability_backed_sections() {
        let payload = default_local_extensions();
        assert!(!payload.is_empty());
        assert!(
            payload.file.is_some(),
            "files-v2 is implemented in this build"
        );
        assert!(payload.tunnel.is_some(), "tunnels-v1 is implemented");
        assert!(payload.call.is_some(), "voice/video are implemented");
        assert!(
            payload.screen_share.is_some(),
            "screen-share-v1 is implemented"
        );
        // App-derived sections start unset: no group list, no device ids, no
        // path/relay topology ever advertised by default.
        assert!(payload.group.is_none());
        assert!(payload.identity.is_none());
        assert!(payload.path_preference.is_none());
        assert!(payload.relay_health.is_none());
    }

    /// Empty payload validates and is empty; a fully populated payload
    /// validates under default bounds.
    #[test]
    fn payload_validate_accepts_empty_and_full() {
        let bounds = ExtensionsBounds::default();
        assert!(ExtensionsPayload::default().validate(&bounds).is_ok());
        assert!(ExtensionsPayload::default().is_empty());

        let full = ExtensionsPayload {
            group: Some(GroupHints { available: true }),
            file: Some(FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: true,
            }),
            tunnel: Some(TunnelCapability {
                protocol_versions: vec!["v1".into()],
            }),
            call: Some(CallCapability {
                protocol_versions: vec!["v1".into()],
                availability: Some(CallAvailability::Available),
            }),
            screen_share: Some(ScreenShareCapability {
                protocol_versions: vec!["v1".into()],
            }),
            identity: Some(MultiDeviceIdentity {
                identity_id: "user-alice".into(),
                device_id: "dev-phone".into(),
                active_device: true,
            }),
            path_preference: Some(PathPreference::DirectPreferred),
            relay_health: Some(RelayHealthHint::Healthy),
        };
        assert!(!full.is_empty());
        assert!(full.validate(&bounds).is_ok());
    }

    /// Bounds: too many versions, too-long version, bad charset, and
    /// too-long identity/device ids are all rejected.
    #[test]
    fn payload_validate_rejects_bound_violations() {
        let bounds = ExtensionsBounds {
            max_protocol_versions: 2,
            max_version_len: 8,
            max_identity_id_len: 16,
            max_device_id_len: 16,
        };

        let too_many = ExtensionsPayload {
            file: Some(FileReadiness {
                protocol_versions: vec!["v1".into(), "v2".into(), "v3".into()],
                can_receive: true,
            }),
            ..Default::default()
        };
        assert!(matches!(
            too_many.validate(&bounds),
            Err(ExtensionsViolation::TooManyVersions {
                section: 0,
                count: 3,
                max: 2
            })
        ));

        let too_long = ExtensionsPayload {
            tunnel: Some(TunnelCapability {
                protocol_versions: vec!["v-12345678".into()],
            }),
            ..Default::default()
        };
        assert!(matches!(
            too_long.validate(&bounds),
            Err(ExtensionsViolation::VersionTooLong { section: 1, .. })
        ));

        let invalid = ExtensionsPayload {
            call: Some(CallCapability {
                protocol_versions: vec!["v1 bad".into()],
                availability: None,
            }),
            ..Default::default()
        };
        assert!(matches!(
            invalid.validate(&bounds),
            Err(ExtensionsViolation::VersionInvalid { section: 2 })
        ));

        let bad_identity = ExtensionsPayload {
            identity: Some(MultiDeviceIdentity {
                identity_id: "x".repeat(17),
                device_id: "dev".into(),
                active_device: true,
            }),
            ..Default::default()
        };
        assert!(matches!(
            bad_identity.validate(&bounds),
            Err(ExtensionsViolation::IdentityIdTooLong { .. })
        ));

        let bad_device = ExtensionsPayload {
            identity: Some(MultiDeviceIdentity {
                identity_id: "user".into(),
                device_id: "y".repeat(17),
                active_device: true,
            }),
            ..Default::default()
        };
        assert!(matches!(
            bad_device.validate(&bounds),
            Err(ExtensionsViolation::DeviceIdTooLong { .. })
        ));
    }

    /// A malicious peer cannot smuggle content through the metadata
    /// channels: a huge "file bytes" string in a version field is rejected
    /// by the bound, and a fully populated payload stays tiny on the wire.
    #[test]
    fn metadata_only_no_content_smuggling() {
        let bounds = ExtensionsBounds::default();

        // Attempt to smuggle 4 KiB of "file bytes" through a version field.
        let smuggled = ExtensionsPayload {
            file: Some(FileReadiness {
                protocol_versions: vec!["x".repeat(4096)],
                can_receive: true,
            }),
            ..Default::default()
        };
        assert!(matches!(
            smuggled.validate(&bounds),
            Err(ExtensionsViolation::VersionTooLong { .. })
        ));

        // Even a fully populated, valid payload encodes far below the
        // control-plane payload cap (4 KiB) — no room for data-plane content.
        let full = ExtensionsPayload {
            group: Some(GroupHints { available: true }),
            file: Some(FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: true,
            }),
            tunnel: Some(TunnelCapability {
                protocol_versions: vec!["v1".into()],
            }),
            call: Some(CallCapability {
                protocol_versions: vec!["v1".into()],
                availability: Some(CallAvailability::Available),
            }),
            screen_share: Some(ScreenShareCapability {
                protocol_versions: vec!["v1".into()],
            }),
            identity: Some(MultiDeviceIdentity {
                identity_id: "user-alice".into(),
                device_id: "dev-phone".into(),
                active_device: true,
            }),
            path_preference: Some(PathPreference::DirectPreferred),
            relay_health: Some(RelayHealthHint::Healthy),
        };
        let encoded = postcard::to_stdvec(&full).expect("encode");
        assert!(
            encoded.len() < 512,
            "fully populated extensions payload must stay tiny, got {} bytes",
            encoded.len()
        );
        // The wire contains none of the data-plane markers we must never
        // broadcast (no file bytes marker, no session key shape, no port,
        // no IP/topology).
        let text = String::from_utf8_lossy(&encoded).to_lowercase();
        for marker in ["0.0.0.0", ":8080", "session_key", "password", "file_bytes"] {
            assert!(!text.contains(marker), "wire must not contain {marker:?}");
        }
    }

    /// Wire round-trip: a payload with every section survives postcard
    /// encode/decode identically (used by the envelope round-trip too).
    #[test]
    fn payload_wire_roundtrip() {
        let full = ExtensionsPayload {
            group: Some(GroupHints { available: true }),
            file: Some(FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: true,
            }),
            tunnel: Some(TunnelCapability {
                protocol_versions: vec!["v1".into()],
            }),
            call: Some(CallCapability {
                protocol_versions: vec!["v1".into()],
                availability: Some(CallAvailability::Busy),
            }),
            screen_share: Some(ScreenShareCapability {
                protocol_versions: vec!["v1".into()],
            }),
            identity: Some(MultiDeviceIdentity {
                identity_id: "user-alice".into(),
                device_id: "dev-phone".into(),
                active_device: true,
            }),
            path_preference: Some(PathPreference::RelayPreferred),
            relay_health: Some(RelayHealthHint::Degraded),
        };
        let encoded = postcard::to_stdvec(&full).unwrap();
        let decoded: ExtensionsPayload = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, full);
    }

    /// The envelope convenience constructor produces an Extensions envelope
    /// whose payload round-trips through the strict decoder.
    #[test]
    fn extensions_envelope_roundtrip() {
        use crate::control_plane::message::{ControlEnvelope, ControlPlaneDecode};
        let envelope = key(0xAB);
        let payload = ExtensionsPayload {
            file: Some(FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: false,
            }),
            ..Default::default()
        };
        let envelope =
            ControlEnvelope::extensions(envelope.sender_node_id, 7, 1_700_000_000, payload.clone());
        let bytes = envelope.encode();
        match ControlEnvelope::decode(&bytes).expect("decode") {
            ControlPlaneDecode::Message(decoded) => {
                assert_eq!(decoded, envelope);
                assert_eq!(
                    decoded.message_type,
                    ControlEnvelope::extensions(
                        envelope.sender_node_id,
                        0,
                        0,
                        ExtensionsPayload::default()
                    )
                    .message_type
                );
                let crate::control_plane::message::ControlPayload::Extensions(decoded_payload) =
                    &decoded.payload
                else {
                    panic!("expected Extensions payload");
                };
                assert_eq!(decoded_payload, &payload);
            }
            other => panic!("expected Message, got {other:?}"),
        }
        // The wire is a control-plane envelope (magic BC), never chat.
        assert!(bytes.starts_with(&crate::control_plane::message::CONTROL_PLANE_MAGIC));
        assert!(
            postcard::from_bytes::<crate::discovery_message::DiscoveryMessage>(&bytes).is_err()
        );
    }

    /// Section tags are stable wire constants.
    #[test]
    fn section_tags_are_stable() {
        assert_eq!(sections::FILE, 0);
        assert_eq!(sections::TUNNEL, 1);
        assert_eq!(sections::CALL, 2);
        assert_eq!(sections::SCREEN_SHARE, 3);
    }

    /// The known capability ids used by the default builder still parse
    /// (compile-time registry sanity).
    #[test]
    fn known_ids_present() {
        for id in [
            ids::FILES_V2,
            ids::TUNNELS_V1,
            ids::VOICE_V1,
            ids::VIDEO_V1,
            ids::SCREEN_SHARE_V1,
        ] {
            assert!(
                crate::control_plane::capabilities::CapabilityId::parse(id).is_ok(),
                "{id} must parse"
            );
        }
    }
}
