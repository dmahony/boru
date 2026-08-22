# Opt-in authorized peer profile exchange

`ProfileExchange` is the contact/room-scoped profile path. It is a chat/data
plane message and is never published by `DiscoveryService` or added to the
startup/DHT-backed discovery topic.

## Sender policy

`ProfileExchangePolicy` starts disabled. A caller must both enable publication
and explicitly authorize the recipient (for example, from an accepted contact
or an authorized room roster) before constructing an exchange. The policy is
deliberately independent of friendship, room membership, and file permissions;
those systems remain authoritative for their own decisions.

## Wire and receive gates

`ProfileExchange::sign` signs a canonical, domain-separated tuple containing
the protocol version, sender, intended recipient, nonce, issue/expiry times,
and `PublicUserProfile`. Receive code requires:

* the authenticated gossip source (`from`) to equal the signed sender;
* the local node to equal the signed recipient;
* the version, lifetime, 4 KiB encoded-size bound, profile validation, and
  Ed25519 signature to pass;
* self-originated and replayed nonces to be dropped; and
* the per-sender eight-per-minute guard to admit the update.

The profile payload excludes local file-sharing policy and paths. Accepted
profiles use the existing `on_public_profile_update` callback, whose GUI
implementation validates revision freshness and writes SQLite first; the UI
cache is only a projection. Expiry retains the established stale-display
fallback semantics.

The new `Message::ProfileExchange` variant is appended to preserve postcard
discriminants for existing messages. Existing `PublicProfileUpdate` payloads
remain decodable, including legacy profiles without the trailing revision.