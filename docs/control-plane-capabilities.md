# Control-plane capabilities: identifiers and versions

Boru control-plane task 4.1 (PDF Phase 4). This document defines the stable,
namespaced, versioned capability identifiers that peers use to discover which
Boru features a client supports — **without assuming every client speaks the
same protocol**.

Source of truth: `src/control_plane/capabilities.rs` (the `CapabilitySet`
type, `CapabilityId` parser, and `KNOWN_CAPABILITIES` registry) and
`src/control_plane/message.rs` (`CapabilitiesPayload`).

## 1. Why capabilities exist

A capability advertisement answers one question:

> Does the remote peer's build support feature *X* at protocol version *N*?

It does **not** grant access to anything. Being discoverable never makes a
peer a friend, group member, tunnel client, or file recipient; friendship and
permissions are still enforced when a feature is actually invoked (PDF
cross-cutting: *no authorisation by presence*). Capability data is metadata
carried on the control plane; all feature data stays on the feature's own
authenticated data-plane channel.

## 2. Identifier format

A capability id is `feature-vN` — a literal `v` prefix followed by the
version number:

```
files-v2
tunnels-v1
screen-share-v1
```

Rules:

- **Feature names** are stable lowercase names (`files`, `tunnels`, `voice`,
  `video`, `screen-share`, `rich-text`). They may contain `-`, so parsing
  splits at the **last** `-` and requires the tail to be `v` + decimal
  digits.
- **Versions** are integers `>= 1` after the `v` prefix. Version `0` is
  invalid; a bare numeric tail without `v` is not a valid id.
- **No implementation-library details.** Ids name Boru features, never
  crates, codecs, or vendor library versions.
- The wire charset (enforced by the privacy layer) is `[A-Za-z0-9._-]`, at
  most `MAX_CAPABILITIES` ids per advertisement, each at most
  `MAX_CAPABILITY_ID_LEN` bytes.

## 3. Well-known capability registry

| Capability id | Feature | Version | Semantics | Meaning |
|---|---|---|---|---|
| `files-v2` | files | 2 | Enabled locally | File transfer over the private file-access path (signed descriptors + blob transfer). |
| `tunnels-v1` | tunnels | 1 | Enabled locally | Boru secure-tunnel service (private enrolment + forwarding). |
| `voice-v1` | voice | 1 | Enabled locally | Voice calls over the call-control signalling path. |
| `video-v1` | video | 1 | Enabled locally | Video calls (implies voice support). |
| `screen-share-v1` | screen-share | 1 | Enabled locally | Screen sharing over the private authenticated session. |
| `rich-text-v1` | rich-text | 1 | Implemented | Rich-text rendering in chat messages. |

### Semantics of an advertisement

The PDF asks that a capability's meaning be explicit rather than guessed.
Three meanings are distinguished:

| Semantics | Meaning | Example |
|---|---|---|
| **Implemented** | The client's code contains the feature and its protocol version. Weakest claim: exists in the build. | `rich-text-v1` — the renderer exists in this build. |
| **Enabled locally** | Implemented **and** enabled locally (not disabled by settings or feature flags). This is the default meaning of a bare wire capability id. | `files-v2` — the user can send/receive files. |
| **Currently available** | Implemented, enabled, **and** currently usable (device present, not already in a call, etc.). Availability is transient and is **not** inferred from a static advertisement. | (future) `voice-v1` only advertised when an audio device is present. |

A bare wire id means **implemented and enabled locally** unless the registry
says otherwise. "Currently available" is deliberately not part of a static
capability advertisement; it is dynamic state that future phases may carry
separately.

## 4. Version separation

Three distinct versions exist; do not conflate them:

| Version | Constant | Meaning |
|---|---|---|
| Control-plane envelope version | `CONTROL_PLANE_PROTOCOL_VERSION` (u8) | Header layout of the discovery envelope. Bump = incompatible header; older clients fail closed. |
| Application protocol version | `BORU_APP_PROTOCOL_VERSION` (u8) | Semantics of the control-plane application (presence, capability rules). Advertised in HELLO. |
| Capability versions | per-id, e.g. `files-v2` | Version of **one feature protocol**. Independent of both envelope and app versions. |

Consequences:

- A peer can advertise `files-v2` while both sides still speak app protocol
  v1.
- An app-protocol v2 client may choose not to advertise a feature.
- **Feature availability is never inferred from app version strings.**
  `CapabilitySet` never reads `BORU_APP_PROTOCOL_VERSION`; the app version
  implies nothing about individual features.

## 5. Forward compatibility

`CapabilitySet` is a map from feature name to the set of versions that
feature supports, plus a raw bucket for ids that do not parse:

- **Unknown future features** (`hologram-v3`) are parsed into the map and
  preserved losslessly.
- **Unknown future id grammars** (`files-v2.1-beta`) are kept verbatim in the
  raw bucket.
- `from_wire` → `to_wire` is a total, lossless round-trip: nothing is dropped
  and nothing crashes.

This is the mechanism behind the acceptance criterion *older clients ignore
capabilities they do not understand*: an older client that has never seen
`voice-v1` still carries it in its set, still parses the `files-v2` it does
understand, and never breaks because of the unknown entries.

## 6. Version coexistence during migration

Because a feature maps to a *set* of versions, two versions of the same
feature can coexist while the fleet migrates:

```rust
let mut set = CapabilitySet::new();
set.insert_id("files-v1"); // old peer still on v1
set.insert_id("files-v2"); // new peer on v2
assert_eq!(set.versions_of("files"), Some(&BTreeSet::from([1, 2])));
```

The negotiation primitive is `compatible_version(local, remote, feature)`:
it returns the highest version both sides support, or `None` when they share
no version (fail closed — no shared version means no initiation).

## 7. Wire usage

The discovery wire carries capabilities as an ordered, deduplicated list of
id strings in `CapabilitiesPayload` (message type `CAPABILITIES`):

```rust
let payload = CapabilitiesPayload::from_set(&local_set);
// payload.capabilities == ["files-v2", "tunnels-v1", ...]
let remote_set = payload.to_set(); // lossless, unknown ids preserved
```

Exchanging capabilities through discovery (sending them in HELLO, caching
per-peer state, expiry) is control-plane task 4.2 and is implemented
separately; gating feature initiation on negotiated support is task 4.3.

## 8. Adding a new capability

1. Pick a stable lowercase feature name (never a crate or library name).
2. Start versions at `1`; bump the version only on a breaking protocol
   change for that feature.
3. Add the id constant to `capabilities::ids` and the feature constant to
   `capabilities::features`.
4. Add a `CapabilityDescriptor` to `KNOWN_CAPABILITIES` with explicit
   semantics and a description.
5. Update this registry table.
6. Keep the `CapabilitySet` behaviour: unknown ids stay lossless.

## 9. Test matrix

| Requirement | Test |
|---|---|
| Older clients ignore unknown capabilities | `test_unknown_capabilities_are_ignored_not_fatal` — future feature + future grammar preserved, known feature unaffected, round-trip lossless |
| Two versions coexist during migration | `test_two_versions_coexist` — `files-v1` + `files-v2` in one set, union, dedup, wire keeps both |
| Availability not inferred from app version strings | `test_availability_not_inferred_from_app_version` — identical app versions with different capability sets; newer app without `files` is not file-capable |
| Ids parse and validate | `test_known_ids_parse`, `test_multi_dash_feature_parses_at_last_dash`, `test_parse_rejects_malformed` |
| Negotiation primitive | `test_compatible_version` — highest shared version, fail-closed on disjoint/absent/unknown |
| Registry well-formed | `test_registry_is_well_formed` — unique, sorted, parseable, explicit semantics |
| Wire payload conversion | `test_payload_roundtrip_preserves_unknown` |
