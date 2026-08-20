# Local network-location data

Boru's optional network-location resolver is local and fail-soft. It reads
MaxMind-compatible GeoLite2 databases and never calls a hosted geolocation API.
The resolver receives the current `iroh::Endpoint::watch_addr()` value, ignores
relay and custom transports, and filters non-public IP addresses before any
database read. It emits only optional country code, coarse (0.1 degree)
coordinates, and ASN; it never puts the source address in presence metadata.

## Runtime installation

Install and periodically update the MaxMind GeoLite2 City database from
MaxMind, and optionally the GeoLite2 ASN database, using MaxMind's current
download/account process. Configure their local filesystem paths in the
application layer and pass them to `GeoIpResolver::from_paths`. Boru does not
choose a global path, download databases, or ship database bytes. A missing,
unreadable, or malformed file is equivalent to missing location data and does
not prevent startup, connection, or chat.

The City and ASN files may be supplied independently. Results are cached by
public IP address, including negative results, so repeated unchanged endpoint
address notifications do not repeat database work. An address change is
handled by the watcher task and is not performed on the render path.

GeoLite2 data is subject to MaxMind's applicable GeoLite2 End User Licence
Agreement and attribution/update terms. Operators are responsible for
obtaining the data lawfully, retaining required notices, and complying with
those terms when redistributing or updating it.