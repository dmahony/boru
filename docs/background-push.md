# Background push boundary

Boru reports `background_push=unavailable` unless a platform/infrastructure
provider is explicitly configured. Connected activity alerts are independent
and continue to work through `activity_alerts`; a disconnected desktop host
does not imply that push delivery exists.

`background_push` is a broker boundary, not an APNs or FCM client. The core
contains no provider secrets, SDK credentials, callback URLs, or HTTP callback
mechanism. A future mobile/infrastructure milestone may implement a provider
adapter behind `NotificationSink`, but provider authentication and delivery
remain outside this crate.

Registrations are opaque and bound to the authorization grant that created
them. Revocation removes queued hints and prevents retries. Hints are generic
(`NewActivity` plus a count and expiry) and never contain message bodies,
previews, sender IDs, room IDs, filenames, or arbitrary URLs. The recording
adapter bounds the queue, expires stale hints, and limits retries; broker or
platform failures are non-blocking and best effort.

The recording adapter exists for privacy/revocation tests and diagnostics only.
It is not evidence of production push support. UI copy should distinguish
“connected activity alerts” from “background push unavailable”.