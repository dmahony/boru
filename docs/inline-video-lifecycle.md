# Inline video lifecycle policy

Boru's inline player uses the chat log's existing virtualized window as its
viewport signal. The window includes 800 px of overscan, so an active card is
considered nearby while it is visible or close enough to be rendered during
normal scrolling. No decoder is created for a card merely because it enters
this window; playback is still started only by the card's explicit Play action.

When the active card leaves the overscanned window, playback is paused
immediately. Audio-only background playback is not supported. The player is
kept warm for 10 seconds to absorb rapid scrolls, then dropped so GStreamer
resources are released. The attachment path is never modified or deleted.

Before dropping the player, Boru records its current position in lightweight
UI state. Pressing Play again creates a decoder only on demand and seeks to
that position after loading. Explicit close, room switching, room deletion,
and application teardown use the existing stop path, which pauses and drops
the player without retaining a resume entry.

The existing virtualized chat renderer continues to build only its overscanned
range, so chats with many video attachments do not allocate players until a
user activates one.
