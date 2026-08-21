# Peer profile freshness

Public profiles exchanged over Boru gossip carry a publisher `revision`. A peer
accepts a payload only when its revision is greater than the persisted revision
for that authenticated public key; duplicates and older/replayed payloads are
ignored deterministically. Legacy payloads without a revision decode as
revision `0`.

Received profiles are persisted in SQLite and considered fresh for one hour
from the last accepted update. The periodic GUI maintenance tick marks an
expired cache entry stale and expires its SQLite row. The last known display
name remains available as a fallback, while bio/avatar/shared-file details are
not rendered until a newer profile arrives. A newer accepted profile updates
SQLite, the in-memory cache, and the friends/profile UI revision so profile
screens and actions re-render.

Local profile saves increment the local revision and rebroadcast the privacy-
safe public payload. The payload contains only display name, bio, avatar
reference, and explicitly announceable shared-file metadata; local file-sharing
policy and filesystem paths are never sent.
