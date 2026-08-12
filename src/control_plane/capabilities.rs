//! Capability identifiers and versions (PDF Phase 4, Task 4.1).
//!
//! Stable, namespaced, versioned capability identifiers let peers discover
//! which Boru features a client supports **without assuming every client
//! speaks the same protocol**. A capability id names one feature at one
//! version, for example `files-v2`, `tunnels-v1`, or `screen-share-v1`.
//!
//! # Design rules (from the PDF and the BORU-CP chain)
//!
//! * **Namespaced and versioned.** Every id is `feature-vN` with a
//!   stable feature name and an integer version (`>= 1`) carrying a literal
//!   `v` prefix (`files-v2`). Feature names may contain `-`, so parsing
//!   splits at the *last* `-` and requires the tail to be `v` + decimal
//!   digits.
//! * **Tolerant of unknown future values.** [`CapabilitySet`] is a map from
//!   feature name to the set of versions that feature supports. An id this
//!   client does not understand — a future feature, a future id grammar, or
//!   a malformed string — is **preserved, never dropped and never fatal**.
//!   [`CapabilitySet::from_wire`] → [`CapabilitySet::to_wire`] is lossless.
//! * **Separate application protocol version from feature versions.** The
//!   application protocol version
//!   ([`BORU_APP_PROTOCOL_VERSION`](crate::control_plane::message::BORU_APP_PROTOCOL_VERSION))
//!   says which *control-plane semantics* the client speaks; a capability
//!   version says which version of *one feature protocol* it speaks. A peer
//!   can advertise `files-v2` while both sides still run app protocol v1,
//!   and an app-protocol v2 client may choose not to advertise a feature.
//!   Capability sets never read the app version, and app version strings
//!   never imply feature availability.
//! * **Explicit semantics.** [`CapabilitySemantics`] and
//!   [`KNOWN_CAPABILITIES`] document what advertising each capability
//!   means: *implemented*, *enabled locally*, or *currently available*.
//!   A bare wire id is interpreted by the documented contract, not by
//!   guessing from the app version.
//! * **No implementation-library details.** Ids name Boru features
//!   (`files`, `tunnels`, `voice`, …), never crates, codecs, or vendor
//!   library versions.
//!
//! # Wire format
//!
//! The discovery wire carries capabilities as an ordered, deduplicated list
//! of id strings (see
//! [`CapabilitiesPayload`](crate::control_plane::message::CapabilitiesPayload)),
//! validated by the privacy layer (≤ [`MAX_CAPABILITIES`](crate::control_plane::privacy::MAX_CAPABILITIES)
//! ids, each ≤ [`MAX_CAPABILITY_ID_LEN`](crate::control_plane::privacy::MAX_CAPABILITY_ID_LEN)
//! bytes, charset `[A-Za-z0-9._-]`).
//! [`CapabilitySet::from_wire`] / [`CapabilitySet::to_wire`] convert
//! losslessly between that wire list and the typed map used by the rest of
//! the control plane.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

// ── Well-known capability ids ─────────────────────────────────────────────

/// Well-known, namespaced, versioned capability identifiers.
///
/// These string constants are the stable wire values. Adding a feature:
/// choose a stable lowercase feature name, append `-vN` where `N` starts at
/// `1` and increments only on a breaking protocol change for that feature,
/// and add the id to [`KNOWN_CAPABILITIES`] with explicit
/// [`CapabilitySemantics`].
pub mod ids {
    /// File transfer over the private file-access path (blob transfer,
    /// signed descriptors).
    pub const FILES_V2: &str = "files-v2";
    /// The Boru secure-tunnel service (private tunnel enrolment and
    /// forwarding).
    pub const TUNNELS_V1: &str = "tunnels-v1";
    /// Voice calls (call-control signalling plus audio media).
    pub const VOICE_V1: &str = "voice-v1";
    /// Video calls (call-control signalling plus audio/video media).
    pub const VIDEO_V1: &str = "video-v1";
    /// Screen sharing over the private authenticated session.
    pub const SCREEN_SHARE_V1: &str = "screen-share-v1";
    /// Rich-text rendering in chat messages.
    pub const RICH_TEXT_V1: &str = "rich-text-v1";
}

/// Stable feature names used by the well-known capability ids.
///
/// These are the *feature* portion of a capability id (before the trailing
/// `-version`). Unknown features are represented by their raw string inside
/// a [`CapabilitySet`]; this module only recognises these.
pub mod features {
    /// File transfer feature.
    pub const FILES: &str = "files";
    /// Secure-tunnel feature.
    pub const TUNNELS: &str = "tunnels";
    /// Voice-call feature.
    pub const VOICE: &str = "voice";
    /// Video-call feature.
    pub const VIDEO: &str = "video";
    /// Screen-share feature.
    pub const SCREEN_SHARE: &str = "screen-share";
    /// Rich-text feature.
    pub const RICH_TEXT: &str = "rich-text";
}

// ── Capability id ─────────────────────────────────────────────────────────

/// Error returned when a string is not a valid `feature-version` id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityParseError {
    /// The id contains no `-` separator.
    MissingSeparator,
    /// The id is empty or the feature portion before the last `-` is empty.
    EmptyFeature,
    /// The portion after the last `-` is not a decimal version number.
    InvalidVersion,
    /// The version parsed but is `0`; versions start at `1`.
    ZeroVersion,
}

impl fmt::Display for CapabilityParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSeparator => write!(f, "capability id has no '-' separator"),
            Self::EmptyFeature => write!(f, "capability id has an empty feature name"),
            Self::InvalidVersion => write!(f, "capability id version is not decimal"),
            Self::ZeroVersion => write!(f, "capability version must be >= 1"),
        }
    }
}

impl std::error::Error for CapabilityParseError {}

/// One namespaced, versioned capability identifier (`feature-version`).
///
/// The canonical form is a lowercase feature name, a `-`, and an integer
/// version `>= 1` (e.g. `files-v2`). Feature names may themselves contain
/// `-` (`screen-share-v1`), so parsing splits at the **last** `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId {
    /// Stable feature name (e.g. `files`).
    pub feature: String,
    /// Feature protocol version this id advertises (e.g. `2`).
    pub version: u16,
}

impl CapabilityId {
    /// Build a capability id from a feature name and version.
    ///
    /// Returns `Err(CapabilityParseError::ZeroVersion)` when `version == 0`.
    pub fn new(feature: impl Into<String>, version: u16) -> Result<Self, CapabilityParseError> {
        let feature = feature.into();
        if feature.is_empty() {
            return Err(CapabilityParseError::EmptyFeature);
        }
        if version == 0 {
            return Err(CapabilityParseError::ZeroVersion);
        }
        Ok(Self { feature, version })
    }

    /// Parse `feature-vN`, splitting at the last `-`.
    ///
    /// The version portion carries a literal `v` prefix (`files-v2` →
    /// feature `files`, version `2`; `screen-share-v1` → feature
    /// `screen-share`, version `1`).
    pub fn parse(id: &str) -> Result<Self, CapabilityParseError> {
        let Some(dash) = id.rfind('-') else {
            return Err(CapabilityParseError::MissingSeparator);
        };
        let feature = &id[..dash];
        if feature.is_empty() {
            return Err(CapabilityParseError::EmptyFeature);
        }
        let version_part = &id[dash + 1..];
        let Some(digits) = version_part.strip_prefix('v') else {
            return Err(CapabilityParseError::InvalidVersion);
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CapabilityParseError::InvalidVersion);
        }
        let version: u16 = digits
            .parse()
            .map_err(|_| CapabilityParseError::InvalidVersion)?;
        if version == 0 {
            return Err(CapabilityParseError::ZeroVersion);
        }
        Ok(Self {
            feature: feature.to_owned(),
            version,
        })
    }

    /// The canonical wire form, `feature-vN` (e.g. `files-v2`).
    pub fn as_str(&self) -> String {
        format!("{}-v{}", self.feature, self.version)
    }

    /// Whether the feature name is one of the well-known
    /// [`features`](mod@features) recognised by this client.
    pub fn is_known_feature(&self) -> bool {
        matches!(
            self.feature.as_str(),
            features::FILES
                | features::TUNNELS
                | features::VOICE
                | features::VIDEO
                | features::SCREEN_SHARE
                | features::RICH_TEXT
        )
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-v{}", self.feature, self.version)
    }
}

impl FromStr for CapabilityId {
    type Err = CapabilityParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ── Capability set ────────────────────────────────────────────────────────

/// A lossless set/map of capability identifiers.
///
/// Internally a map from feature name to the set of versions that feature
/// supports (`files -> {1, 2}`), so **two versions of the same feature can
/// coexist during migration**. Ids this client does not understand (future
/// features, future id grammars, malformed strings) are preserved in a raw
/// bucket rather than dropped, so [`from_wire`](Self::from_wire) /
/// [`to_wire`](Self::to_wire) round-trip is total.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySet {
    versions: BTreeMap<String, BTreeSet<u16>>,
    raw: BTreeSet<String>,
}

impl CapabilitySet {
    /// An empty capability set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a parsed capability id.
    pub fn insert(&mut self, id: CapabilityId) {
        self.versions
            .entry(id.feature)
            .or_default()
            .insert(id.version);
    }

    /// Insert a raw wire id, parsing it if possible.
    ///
    /// Values that parse as `feature-version` land in the version map;
    /// anything else is preserved verbatim in the raw bucket. Never fails.
    pub fn insert_id(&mut self, id: &str) {
        match CapabilityId::parse(id) {
            Ok(parsed) => self.insert(parsed),
            Err(_) => {
                self.raw.insert(id.to_owned());
            }
        }
    }

    /// Whether the set advertises any version of `feature`.
    pub fn has_feature(&self, feature: &str) -> bool {
        self.versions.contains_key(feature)
    }

    /// The versions advertised for `feature`, if any.
    pub fn versions_of(&self, feature: &str) -> Option<&BTreeSet<u16>> {
        self.versions.get(feature)
    }

    /// Whether the set contains exactly `id` (feature and version).
    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.versions
            .get(&id.feature)
            .is_some_and(|vs| vs.contains(&id.version))
    }

    /// Number of distinct parsed ids (raw bucket excluded).
    pub fn len(&self) -> usize {
        self.versions.values().map(BTreeSet::len).sum()
    }

    /// Whether the set advertises no parsed capabilities.
    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    /// Iterate over the parsed capability ids, sorted by (feature, version).
    pub fn parsed(&self) -> impl Iterator<Item = CapabilityId> + '_ {
        self.versions.iter().flat_map(|(feature, versions)| {
            versions.iter().map(move |version| CapabilityId {
                feature: feature.clone(),
                version: *version,
            })
        })
    }

    /// The raw, unparsed ids (future/malformed values) this set preserves.
    pub fn raw_ids(&self) -> &BTreeSet<String> {
        &self.raw
    }

    /// Merge `other` into `self` (union of versions and raw ids).
    pub fn union_with(&mut self, other: &CapabilitySet) {
        for (feature, versions) in &other.versions {
            self.versions
                .entry(feature.clone())
                .or_default()
                .extend(versions.iter().copied());
        }
        self.raw.extend(other.raw.iter().cloned());
    }

    /// Build a set from the wire form (a list of id strings).
    ///
    /// Lossless: every input string is either parsed into the version map
    /// or preserved in the raw bucket. Duplicates collapse.
    pub fn from_wire(ids: impl IntoIterator<Item = String>) -> Self {
        let mut set = Self::new();
        for id in ids {
            set.insert_id(&id);
        }
        set
    }

    /// The wire form: a sorted, deduplicated list of id strings.
    ///
    /// Parsed ids first (`feature-version`, sorted), then raw ids (sorted).
    /// [`from_wire`](Self::from_wire) applied to this output returns an
    /// equal set.
    pub fn to_wire(&self) -> Vec<String> {
        let mut out: Vec<String> = self.parsed().map(|id| id.as_str()).collect();
        out.extend(self.raw.iter().cloned());
        out.sort();
        out.dedup();
        out
    }
}

/// Highest version both sides support for `feature`, or `None` if they
/// share no version (or one side does not advertise the feature at all).
///
/// This is the pure negotiation primitive later used to gate feature
/// initiation (PDF Task 4.3); it deliberately fails closed — no common
/// version means no version to initiate.
pub fn compatible_version(
    local: &CapabilitySet,
    remote: &CapabilitySet,
    feature: &str,
) -> Option<u16> {
    let local_versions = local.versions_of(feature)?;
    let remote_versions = remote.versions_of(feature)?;
    local_versions
        .iter()
        .filter(|v| remote_versions.contains(v))
        .max()
        .copied()
}

// ── Semantics ─────────────────────────────────────────────────────────────

/// What advertising a capability means.
///
/// The PDF (Task 4.1) asks for explicit semantics instead of leaving a bare
/// id ambiguous. [`KNOWN_CAPABILITIES`] pins one of these per well-known id;
/// where the three meanings could differ, the documentation explains which
/// one the wire id carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySemantics {
    /// The client's code implements the feature and its protocol version.
    /// This is the weakest claim: the feature exists in the build.
    Implemented,
    /// The feature is implemented **and** enabled locally (not disabled by
    /// settings or feature flags). This is the default meaning of a bare
    /// wire capability id.
    EnabledLocally,
    /// The feature is implemented, enabled, **and** currently available
    /// (e.g. an audio device is present, the user is not already in a
    /// call). Availability is transient and is not inferred from a static
    /// capability advertisement.
    CurrentlyAvailable,
}

/// A well-known capability and its documented semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    /// The stable wire id (e.g. `files-v2`).
    pub id: &'static str,
    /// What advertising this id means.
    pub semantics: CapabilitySemantics,
    /// Human-readable description of the feature and version.
    pub description: &'static str,
}

/// Registry of well-known capabilities.
///
/// This is the contract for what a wire id means. Ids not present here are
/// still carried losslessly by [`CapabilitySet`] (forward compatibility);
/// they are simply not interpreted by this client.
pub const KNOWN_CAPABILITIES: &[CapabilityDescriptor] = &[
    CapabilityDescriptor {
        id: ids::FILES_V2,
        semantics: CapabilitySemantics::EnabledLocally,
        description:
            "File transfer over the private file-access path (signed descriptors + blob transfer).",
    },
    CapabilityDescriptor {
        id: ids::TUNNELS_V1,
        semantics: CapabilitySemantics::EnabledLocally,
        description: "Boru secure-tunnel service (private enrolment + forwarding).",
    },
    CapabilityDescriptor {
        id: ids::VOICE_V1,
        semantics: CapabilitySemantics::EnabledLocally,
        description: "Voice calls over the call-control signalling path.",
    },
    CapabilityDescriptor {
        id: ids::VIDEO_V1,
        semantics: CapabilitySemantics::EnabledLocally,
        description: "Video calls (implies voice support).",
    },
    CapabilityDescriptor {
        id: ids::SCREEN_SHARE_V1,
        semantics: CapabilitySemantics::EnabledLocally,
        description: "Screen sharing over the private authenticated session.",
    },
    CapabilityDescriptor {
        id: ids::RICH_TEXT_V1,
        semantics: CapabilitySemantics::Implemented,
        description: "Rich-text rendering in chat messages.",
    },
];

/// Look up a well-known descriptor by its wire id.
pub fn known_descriptor(id: &str) -> Option<&'static CapabilityDescriptor> {
    KNOWN_CAPABILITIES.iter().find(|d| d.id == id)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> CapabilitySet {
        CapabilitySet::from_wire(ids.iter().map(|s| s.to_string()))
    }

    /// Well-known ids parse to the expected feature/version pairs.
    #[test]
    fn test_known_ids_parse() {
        let cases = [
            (ids::FILES_V2, features::FILES, 2u16),
            (ids::TUNNELS_V1, features::TUNNELS, 1),
            (ids::VOICE_V1, features::VOICE, 1),
            (ids::VIDEO_V1, features::VIDEO, 1),
            (ids::SCREEN_SHARE_V1, features::SCREEN_SHARE, 1),
            (ids::RICH_TEXT_V1, features::RICH_TEXT, 1),
        ];
        for (id, feature, version) in cases {
            let parsed = CapabilityId::parse(id).expect("well-known id must parse");
            assert_eq!(parsed.feature, feature);
            assert_eq!(parsed.version, version);
            assert_eq!(parsed.as_str(), id);
            assert!(parsed.is_known_feature());
        }
    }

    /// Feature names containing '-' parse at the LAST separator.
    #[test]
    fn test_multi_dash_feature_parses_at_last_dash() {
        let id = CapabilityId::parse("screen-share-v1").expect("parse");
        assert_eq!(id.feature, "screen-share");
        assert_eq!(id.version, 1);
    }

    /// Malformed ids are rejected by `parse` with the right error.
    #[test]
    fn test_parse_rejects_malformed() {
        assert_eq!(
            CapabilityId::parse("files"),
            Err(CapabilityParseError::MissingSeparator)
        );
        assert_eq!(
            CapabilityId::parse("-v1"),
            Err(CapabilityParseError::EmptyFeature)
        );
        assert_eq!(
            CapabilityId::parse("files-"),
            Err(CapabilityParseError::InvalidVersion)
        );
        assert_eq!(
            CapabilityId::parse("files-v"),
            Err(CapabilityParseError::InvalidVersion)
        );
        assert_eq!(
            CapabilityId::parse("files-v0"),
            Err(CapabilityParseError::ZeroVersion)
        );
        assert_eq!(
            CapabilityId::parse("files-v99999"),
            Err(CapabilityParseError::InvalidVersion)
        );
        assert_eq!(
            CapabilityId::new("files", 0),
            Err(CapabilityParseError::ZeroVersion)
        );
        assert_eq!(
            CapabilityId::new("", 1),
            Err(CapabilityParseError::EmptyFeature)
        );
    }

    /// Older clients ignore capabilities they do not understand: an
    /// unknown future id is preserved losslessly and never affects the
    /// parsed capabilities the client *does* understand.
    #[test]
    fn test_unknown_capabilities_are_ignored_not_fatal() {
        // A future client advertises files-v2 plus two ids this client has
        // never seen: a future feature and a future id grammar.
        let wire = vec![
            "files-v2".to_string(),
            "hologram-v3".to_string(),
            "files-v2.1-beta".to_string(),
        ];
        let set = CapabilitySet::from_wire(wire.clone());

        // The known feature is readable and unaffected by the unknowns.
        assert_eq!(
            set.versions_of(features::FILES),
            Some(&BTreeSet::from([2u16]))
        );
        assert!(set.contains(&CapabilityId::parse(ids::FILES_V2).unwrap()));
        // The unknown feature is NOT interpreted by this client: no known
        // descriptor, no well-known feature name — it is carried verbatim.
        let hologram = CapabilityId::parse("hologram-v3").expect("parse");
        assert!(!hologram.is_known_feature());
        assert!(known_descriptor("hologram-v3").is_none());
        assert_eq!(known_descriptor(ids::FILES_V2).unwrap().id, ids::FILES_V2);

        // The unknowns are preserved, not dropped.
        assert!(set.has_feature("hologram"));
        assert_eq!(set.versions_of("hologram"), Some(&BTreeSet::from([3u16])));
        assert!(set.raw_ids().contains("files-v2.1-beta"));

        // Wire round-trip is lossless (ordering may differ, content not).
        let round = CapabilitySet::from_wire(set.to_wire());
        assert_eq!(round, set);
    }

    /// Two versions of the same feature can coexist during migration.
    #[test]
    fn test_two_versions_coexist() {
        let mut set = CapabilitySet::new();
        set.insert_id("files-v1");
        set.insert_id("files-v2");
        set.insert_id("files-v2"); // duplicate collapses

        assert_eq!(
            set.versions_of(features::FILES),
            Some(&BTreeSet::from([1u16, 2u16]))
        );
        assert_eq!(set.len(), 2);

        // Wire form keeps both.
        let wire = set.to_wire();
        assert_eq!(wire, vec!["files-v1".to_string(), "files-v2".to_string()]);

        // Union preserves both across sets.
        let mut other = CapabilitySet::new();
        other.insert_id("files-v2");
        other.insert_id("tunnels-v1");
        set.union_with(&other);
        assert_eq!(
            set.versions_of(features::FILES),
            Some(&BTreeSet::from([1u16, 2u16]))
        );
        assert!(set.has_feature(features::TUNNELS));
    }

    /// `compatible_version` picks the highest shared version and fails
    /// closed when there is no shared version.
    #[test]
    fn test_compatible_version() {
        let old = set(&["files-v1", "tunnels-v1"]);
        let new = set(&["files-v1", "files-v2", "tunnels-v1"]);

        // Both support v1 and v2 -> pick highest shared (2).
        let both_v2 = set(&["files-v1", "files-v2", "tunnels-v1"]);
        assert_eq!(
            compatible_version(&both_v2, &both_v2, features::FILES),
            Some(2)
        );

        // A peer that only has v1 negotiates v1.
        let only_v1 = set(&["files-v1"]);
        assert_eq!(
            compatible_version(&only_v1, &both_v2, features::FILES),
            Some(1)
        );
        assert_eq!(
            compatible_version(&both_v2, &only_v1, features::FILES),
            Some(1)
        );

        // Disjoint versions -> no compatible version (fail closed).
        let v3_only = set(&["files-v3"]);
        assert_eq!(compatible_version(&old, &v3_only, features::FILES), None);

        // Feature absent on one side -> None.
        assert_eq!(compatible_version(&old, &set(&[]), features::FILES), None);
        assert_eq!(compatible_version(&old, &new, features::VOICE), None);
    }

    /// Feature availability is NOT inferred from app version strings: the
    /// capability set is a standalone object; the app protocol version
    /// never implies or removes capabilities.
    #[test]
    fn test_availability_not_inferred_from_app_version() {
        // Two peers, same app protocol version 1. One advertises files-v2,
        // the other does not. Capability set is the only source of truth.
        let advertiser = set(&["files-v2", "tunnels-v1"]);
        let non_advertiser = set(&["tunnels-v1"]);

        assert!(advertiser.has_feature(features::FILES));
        assert!(!non_advertiser.has_feature(features::FILES));
        // Identical app versions do not equal identical capabilities.
        assert_ne!(advertiser, non_advertiser);

        // A hypothetical newer app version that does NOT advertise files
        // must not be treated as file-capable — availability comes from the
        // set, not the version number.
        let new_app_no_files = set(&["tunnels-v1"]);
        assert!(!new_app_no_files.has_feature(features::FILES));

        // The well-known registry is independent of the app protocol
        // version constant; nothing here consults it.
        let _ = crate::control_plane::message::BORU_APP_PROTOCOL_VERSION;
    }

    /// Wire conversion helpers on the existing payload are lossless,
    /// including unknown values.
    #[test]
    fn test_payload_roundtrip_preserves_unknown() {
        let set = set(&["files-v2", "hologram-v3", "files-v2.1-beta", "tunnels-v1"]);
        let payload = crate::control_plane::message::CapabilitiesPayload::from_set(&set);
        let decoded = payload.to_set();
        assert_eq!(decoded, set);
    }

    /// The registry is well-formed: unique ids, each parses, each has
    /// explicit semantics and a non-empty description.
    #[test]
    fn test_registry_is_well_formed() {
        assert!(!KNOWN_CAPABILITIES.is_empty());
        let mut seen = std::collections::BTreeSet::new();
        for d in KNOWN_CAPABILITIES {
            assert!(seen.insert(d.id), "duplicate registry id {}", d.id);
            let parsed = CapabilityId::parse(d.id).expect("registry id must parse");
            assert!(parsed.is_known_feature());
            assert!(parsed.version >= 1);
            assert!(!d.description.is_empty());
        }
        // Every known id is resolvable through the lookup helper.
        for d in KNOWN_CAPABILITIES {
            assert!(known_descriptor(d.id).is_some());
        }
    }

    /// A set that advertises nothing is empty on the wire too.
    #[test]
    fn test_empty_set() {
        let set = CapabilitySet::new();
        assert!(set.is_empty());
        assert_eq!(set.to_wire(), Vec::<String>::new());
        assert_eq!(compatible_version(&set, &set, features::FILES), None);
    }
}
