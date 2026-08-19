# Screen sharing and real-time media session integration

Status: implemented in `call::session` and the Iced calls domain.

## Working capability

- Voice and native screen sharing now project into one `RealtimeMediaSession`.
- `MediaTrack::Voice` and `MediaTrack::Screen` have independent lifecycle state;
  stopping or reconnecting screen sharing does not stop an active voice track.
- The screen-share host path enters `Starting`, `Active`, `Reconnecting`, and
  `Stopped` through the shared projection. The existing screen-share protocol,
  capture backends, encoder, viewer, consent, source picker, and reconnect
  worker remain unchanged.
- The existing direct-chat actions remain the control surface: Start Screen
  Share, Stop Sharing, source selection, watch/accept/decline, and fullscreen
  viewer controls. Voice mute/camera/hang-up remain owned by the call actor.
- `RealtimeMediaSession::presence()` is a compact projection suitable for a
  sharing presence card: session active, voice state, and screen state. It is
  deliberately metadata-only and contains no media or transport state.

## Deliberate boundaries and remaining gaps

- The voice actor and screen-share protocol still use separate authenticated
  wire paths. This avoids rewriting two working transports and preserves their
  independent failure handling; the shared session is an application lifecycle
  boundary, not a multiplexing protocol.
- A screen-share invitation has its existing consent prompt and source picker;
  it does not implicitly start voice. A voice call does not implicitly publish
  the screen. This is required for independent track permissions.
- Native capture remains platform-dependent: Linux Wayland portal/PipeWire and
  X11 fallback are implemented, Windows is implemented but requires Windows
  hardware verification, and macOS remains a stub. Window capture and system
  audio are separate follow-up capabilities.
- Real two-peer LAN/relay coexistence and reconnect tests require the manual
  platform/network matrix in `docs/screenshare-test-matrix.md`; unit tests cover
  the shared lifecycle and existing screen-share state machines.
