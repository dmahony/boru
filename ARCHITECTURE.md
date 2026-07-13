# Private-room DHT discovery architecture

This document describes the optional DHT bootstrap path for private Boru Chat
rooms. It is a bootstrap and presence mechanism; it does not replace Boru's
existing gossip protocol.

## Components

```text
+-------------------+       topic + secret       +----------------------+
| TUI / iced_chat   | --------------------------> | PrivateRoomTracker   |
| create / join     |                             | identity derivation  |
+---------+---------+                             +----------+-----------+
          |                                                  |
          | Ticket { topic, bootstrap peers,                | signed,
          |          optional discovery_secret }            | encrypted record
          v                                                  v
+-------------------+       endpoint IDs          +----------------------+
| RoomStore         | <--------------------------- | Mainline DHT          |
| topic, secret,    |                              | distributed-topic-    |
| cached peers      |                              | tracker backend       |
+-------------------+                              +----------------------+
          |
          v
+-------------------------------------------------------------+
| iroh endpoint -> MemoryLookup -> Boru gossip subscribe/join |
+-------------------------------------------------------------+
```

`PrivateRoomTracker` derives a private discovery namespace from the gossip
topic and the shared `DiscoverySecret`. The secret is not derived from the
room name or topic. The DHT backend is injected behind
`TopicDiscoveryBackend`, which permits deterministic in-memory tests without a
public DHT.

## Create flow

```text
create topic
    |
    +-- generate random DiscoverySecret (unless --no-dht)
    |
    +-- derive private namespace
    |
    +-- sign and publish this endpoint's discovery record
    |
    +-- persist topic + secret in RoomStore
    |
    +-- encode Ticket with topic, optional bootstrap peers, and secret
    |
    +-- start continuous publish/discover tracker
```

If publication fails, room creation remains usable through the legacy ticket
bootstrap path. The secret is still retained when DHT discovery was requested;
a later run can publish again.

## Join flow

```text
receive ticket
    |
    +-- decode topic, bootstrap peers, and optional discovery_secret
    |
    +-- seed MemoryLookup with ticket addresses
    |
    +-- if DHT is enabled and secret is present:
    |       derive namespace -> lookup -> validate -> deduplicate endpoint IDs
    |
    +-- merge ticket peers and discovered IDs (do not add self)
    |
    +-- subscribe / subscribe_and_join using the resulting bootstrap set
    |
    +-- continue learning addresses through gossip
```

The DHT currently returns endpoint identities, not complete endpoint
addresses. Ticket addresses therefore remain the initial address source; DHT
discovery alone cannot repair the address-resolution gap when no ticket peer is
reachable. Once a connection succeeds, iroh's normal address lookup and gossip
address propagation can fill that gap.

Legacy tickets with no `discovery_secret` follow the existing path unchanged.

## Continuous flow

After a DHT-enabled room is created or joined, the continuous tracker
periodically publishes the local signed record and looks up current records.
Returned records are decrypted and validated by the tracker/validation path;
stale, malformed, oversized, duplicate, and self records are not used as
peers. Newly discovered endpoint IDs are fed back into the frontend's address
lookup and room state without changing message handling.

Stopping or leaving a room shuts down the tracker and releases its backend.
The DHT is not required for an already-connected gossip mesh to exchange
messages.

## Security model

* **Discovery secret is the capability.** Anyone holding the secret can derive
  the room's private discovery namespace and attempt discovery. Treat it like
  an invitation secret: do not put it in normal logs, diagnostics, screenshots,
  or public issue reports. `DiscoverySecret` deliberately redacts its `Debug`
  and `Display` output.
* **DHT records are protected.** Discovery records use the tracker record format
  and are signed by the advertising endpoint. The private namespace is derived
  from the topic and secret, and the backend's encrypted record pipeline
  prevents observers without the secret from reading the advertised identity.
  Records are still untrusted input and are validated before endpoint IDs are
  returned.
* **This is not message encryption.** Room discovery secrecy does not encrypt
  chat messages and does not authenticate room membership. Boru's existing
  iroh transport and gossip security provide the message transport properties;
  possession of a room topic/ticket remains necessary to join the gossip room.
* **Tickets are bearer invitations.** A ticket containing a discovery secret
  must be shared only with intended participants. A leaked ticket allows
  discovery and room bootstrap attempts; it cannot be revoked by the DHT
  feature itself.

## Privacy model

The DHT namespace is not derived from a display name, and no endpoint address
is added to a DHT-enabled ticket by this feature. An observer who does not hold
the ticket's discovery secret cannot select the private namespace and therefore
cannot use this feature to enumerate room members. The DHT provider can still
observe ordinary network metadata such as packet timing, source IP address,
and participation in the Mainline DHT. Use a suitable network privacy layer if
that metadata matters.

Room membership is therefore **discoverable to ticket holders**, not globally
public. It is not anonymous: a holder can learn the endpoint IDs that publish
valid records, and transport peers can observe connections after joining.

## Compatibility, controls, and limitations

* The feature is optional. `chat` and `iced_chat` retain the legacy ticket
  bootstrap behavior when DHT is unavailable, when a ticket has no secret, or
  when `--no-dht` is supplied. `--dht` is the explicit opt-in spelling for
  deployments/configurations that expose DHT as disabled by default; current
  example builds enable private-room DHT by default and use `--no-dht` to turn
  it off.
* DHT requires UDP reachability and may be blocked or rate-limited by a
  firewall, NAT, corporate network, or ISP. Publication and lookup are slower
  and eventually consistent compared with a directly supplied ticket address.
* Mainline DHT discovery supplies endpoint IDs, not endpoint addresses. A
  reachable ticket bootstrap peer, DNS/Pkarr, mDNS, relay, or another address
  lookup method is still needed to turn an ID into a usable connection in the
  common case.
* DHT discovery does not replace Boru's gossip implementation. Gossip remains
  responsible for room membership and message dissemination.
* Tests use the in-memory backend and must not depend on live public DHT
  infrastructure.
