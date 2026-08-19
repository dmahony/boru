# Voice rooms (v1)

Voice rooms reuse the authenticated call/media transport, but keep room
identity and group state separate from a direct call. `VoiceRoom` is durable
metadata (room id, display name, and the containing room topic); membership is
ephemeral and heartbeat-based. `MembershipView` merges the highest epoch and
expires missing heartbeats, so reconnects and delayed leaves converge without
persisting stale presence.

The v1 input modes are voice activity (with threshold hysteresis) and
push-to-talk. Speaking state is local policy and can be surfaced to the room
presence UI. `VoiceRouter` gives every destination its own bounded queue;
queue overflow increments that peer's drop counter and never blocks or drops
audio for other participants. Per-user mute suppresses delivery to that
destination while retaining the room membership.

`RoomStore::enable_voice_room` binds the durable voice metadata to an existing
chat room. `VoiceRoomSession` then handles only ephemeral membership heartbeats,
speaking/VAD/PTT state, bounded per-peer routing, and `VoiceDiagnostics` (queue
depth, drops, mute suppression, sequence loss, jitter, and bitrate). This split
means leaving or timing out a participant never deletes chat history, while a
slow or offline peer cannot stall other participants. The Iced call surface can
project these snapshots alongside the existing direct-call controls.

V1 is intentionally limited to eight participants per room and a bounded
encoded-frame queue per participant. This is suitable for small groups, not
large events: a future SFU/forwarder is required for substantially larger
rooms. Packet loss, jitter, queue drops, and bitrate remain transport/media
metrics and should be displayed through the existing call diagnostics path.