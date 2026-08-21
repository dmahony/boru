# Peer profile discovery decision

Status: intentionally not implemented in the global discovery topic.

## Decision

Peer profiles remain contact/room scoped and continue to use the existing
`PublicProfileUpdate` chat message path. Boru's internal discovery service is
not a safe publication path for profile content in its current form.

This is a deliberate privacy boundary: the discovery topic is joined at
startup, bootstrapped through discovery and DHT paths, and is intended to make
nodes reachable. A profile advertisement sent there would be visible to every
node that reaches that mesh, rather than only to a selected contact or shared
room. The current discovery service has no profile-specific recipient
authorization or opt-in publication state.

## Code basis

* `src/discovery_service.rs` starts and drains one internal discovery topic;
  its `peer_updates` and control-plane events are networking infrastructure and
  explicitly do not create conversations or grant authorization.
* `src/bin/boru/main.rs` starts that service at application startup and feeds
  peers from it into the Discover sidebar and reconnect scheduler. The service
  is also bootstrapped from the global DHT path, so it is not a contact-only
  channel.
* `src/control_plane/message.rs` defines metadata-oriented control envelopes
  with a bounded 4096-byte payload, but the envelope's authentication proves
  the publishing node only. It does not authorize a profile recipient.
* `src/control_plane/privacy.rs` applies sender attribution, deduplication,
  rate limits, and presence TTLs; those controls prevent abuse but do not turn
  global discovery into a private profile exchange.
* `src/user_profile.rs` deliberately separates `PublicUserProfile` from local
  file-sharing and policy fields. The existing profile payload is therefore
  suitable for authenticated contact/room gossip, but publication on the
  global discovery topic would still disclose the public fields too broadly.

## Required follow-up before implementation

A future implementation must add a separate, opt-in profile exchange path,
not simply add a `ProfileAdvertisement` control-plane variant. It needs:

1. explicit local publication opt-in, defaulting to disabled;
2. recipient/contact or room authorization (discovery presence alone is not
   authorization);
3. authenticated sender and intended-recipient binding, with replay/expiry
   handling;
4. the existing `PublicUserProfile::validate` bounds plus profile-specific
   rate and size limits;
5. self-advertisement filtering and no effect on friendship, room membership,
   file-download permissions, or other authorization decisions;
6. a storage/UI event path that keeps SQLite authoritative for received
   profiles.

Until those pieces exist, adding profile publication to the global discovery
mesh would violate the task's privacy requirement. The narrowly scoped
follow-up card tracks that design/implementation work.
