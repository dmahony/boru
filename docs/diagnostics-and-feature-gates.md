# Diagnostics and independent feature gates

Boru lifecycle diagnostics are metadata only. They may record bounded event
names, sequence numbers, short opaque identifiers, counters, durations, and
coarse error labels. They must not contain message bodies, file contents,
clipboard data, credentials, raw IP addresses, or full command/payload JSON.

The diagnostics recording boundary applies the redaction helpers in
`boru_core::diagnostics` to address lists, connection endpoints, error text,
and GUI command payloads. Callers should use `redact_payload` for any new
arbitrary input rather than placing that input in an event variant.

## Runtime gates

`FeatureGates` provides independent switches. Defaults are enabled to preserve
existing behavior when no configuration is present:

```toml
map = true
screen_share_transport = true
presence = true
file_transfer = true
```

For development and diagnostics, the same switches can be overridden without
coupling them to one another through environment variables:

| Gate | Environment variable |
|---|---|
| Map | `BORU_FEATURE_MAP` |
| Screen-share transport | `BORU_FEATURE_SCREEN_SHARE_TRANSPORT` |
| Presence | `BORU_FEATURE_PRESENCE` |
| File transfer | `BORU_FEATURE_FILE_TRANSFER` |

Accepted values are `1`/`true`/`on` and `0`/`false`/`off`. Missing or unknown
values retain the enabled default. A disabled gate must skip only its optional
work and leave the existing chat, reconnect, and persistence fallback paths
unchanged.
