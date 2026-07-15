# Public discovery lease cache

The public lobby uses Mainline DHT by default. The lookup key is
`BLAKE3("boru-chat/public-lobby/v1" || discovery_key)`, a canonical,
domain-separated 32-byte value. It is not derived from display names, account
IDs, addresses, or telemetry identifiers.

## Record contract

The encrypted value contains only the existing signed discovery `Record`: the
lobby topic, minute slot, publisher public key, endpoint public key, and
signature. No nickname, address-book entry, source address, or stable device
identifier is added. The backend attaches a local lease deadline; it is cache
metadata, not plaintext wire data.

- Lease: 600 seconds (10 minutes).
- Refresh: 300 seconds (5 minutes), with existing tracker jitter.
- Expiry: entries with `deadline <= now` are removed before lookup; DHT lookup
  examines only the current and previous minute key slots, so old values are
  naturally unreachable as well.
- Maximum encoded payload: 2048 bytes.
- Maximum records/candidates per lookup: 20. The in-memory cache never retains
  more than 20 records per lobby and returns newest-first.
- Empty and oversized records are rejected with an explicit error. Malformed
  signed records are rejected by the existing decode/signature validation
  pipeline; they are never turned into join candidates.

## Privacy and abuse trade-offs

DHT is decentralized and has no mandatory operator, but DHT participants can
observe lookup/publish timing and the opaque key. The key is intentionally
opaque, but DHT is not an anonymity layer; users requiring network anonymity
should route the transport through their chosen VPN/Tor setup. The ten-minute
lease limits stale presence while tolerating one missed refresh. It also means
brief partitions can hide a live peer until the next refresh.

The record and candidate bounds prevent a malicious response from causing
unbounded allocation, parsing, or gossip joins. A public lobby remains open to
spam and Sybil endpoint keys; signed records prove possession of a key, not
human identity or authorization. Existing join rate limits and public-room
message safety remain the abuse controls rather than adding tracking or
accounts.

## Migration and rollback

The canonical key is versioned by its domain string. During rollout, a caller
must not silently mix old and new key spaces: publish and lookup must use the
same version. A staged migration can read the legacy key for one lease window
while publishing only the canonical key, then remove the compatibility read.
Rollback is safe by reverting the code to the legacy key; no persistent user
state or server-side migration is required. Old records naturally expire.

The in-memory clock injection (`InMemoryDiscoveryBackend::with_clock`) makes
lease expiry and refresh tests deterministic without sleeping or contacting a
network.
