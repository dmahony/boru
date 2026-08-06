//! Semantic classification of chat system messages.
//!
//! System messages arrive as plain text through
//! [`ChatCallbacks::push_system`](crate::chat_callbacks::ChatCallbacks::push_system):
//! join/leave notices, renames, command help, tunnel events, whisper activity,
//! file-transfer progress, video guidance, errors, warnings, and generic
//! informational notices. This module is the single data-layer mapping from
//! those message formats to a typed [`SystemEventKind`].
//!
//! # Invariants
//!
//! * **Nothing is silently discarded.** Every input maps to exactly one
//!   variant. Unrecognised text falls back to [`SystemEventKind::Information`];
//!   there is no "ignore" outcome.
//! * **All original content is preserved.** Classification is a pure read-only
//!   function over the message text. It never rewrites, filters, or truncates
//!   the body — callers keep the raw string as the source of truth and use the
//!   kind only to select rendering or grouping treatment.
//! * **Extensible.** Adding a new system-message format is a one-arm change in
//!   [`classify_system_event`] plus one row in the test table. Keep the most
//!   specific formats above the generic keyword buckets so positive domain
//!   signals (e.g. `Download queued …`) win over error/warning keywords.
//!
//! # Known formats → variant
//!
//! Every format produced by the chat core and the Iced frontend today:
//!
//! | Format (template) | Variant | Producer |
//! |---|---|---|
//! | `{name} joined the chat`, `🟢 {name} joined`, `🟢 {name} is online`, `Friend {label} is now ONLINE` | [`Join`](SystemEventKind::Join) | `chat_callbacks::on_neighbor_status_change`, Iced `on_neighbor_up`/`record_presence`/friend status |
//! | `{name} left the chat`, `Friend {label} is now offline` | [`Leave`](SystemEventKind::Leave) | `chat_callbacks::on_neighbor_status_change`, Iced friend status |
//! | `{key} is now known as {name}` | [`Rename`](SystemEventKind::Rename) | `chat_core` profile-name handling |
//! | `{name} shared a file` | [`FileShared`](SystemEventKind::FileShared) | `chat_core` `Message::FileShare` |
//! | `Usage: …`, `Type a message … /help for commands.` | [`CommandHelp`](SystemEventKind::CommandHelp) | Iced slash-command handlers |
//! | `Tunnel request accepted`, `Tunnel request declined`, `Tunnel closed` | [`Tunnel`](SystemEventKind::Tunnel) | Iced tunnel dialogs |
//! | `[Whisper] Connected to {label}`, `[Whisper to {label}] …`, `[Mailbox] …`, `[Offline DM sync: …]` | [`Whisper`](SystemEventKind::Whisper) | Iced whisper/inbox events |
//! | `{label} invited you to …`, `Room invite sent via whisper to …`, `Invite to join this room (boru1): …`, `{label} opened a private chat with you.` | [`Invite`](SystemEventKind::Invite) | Iced room/group invites |
//! | `Added friend: {label}`, `Updated friend: …`, `Removed friend: …`, `No friends tracked yet.`, `Friends ({n}):` | [`Friend`](SystemEventKind::Friend) | Iced friend management |
//! | `Profile image updated.`, `Profile image removed.`, `Saving profile image…` | [`Profile`](SystemEventKind::Profile) | Iced profile screens |
//! | `Mesh degraded: …`, `Mesh offline: …`, `Mesh recovered: …`, `The gossip receiver closed.` | [`Mesh`](SystemEventKind::Mesh) | Iced mesh watchdog, `chat_core` `NetEvent::Closed` |
//! | `Sharing: {name}`, `Download queued for …`, `*{name}* is complete`, `Shared file added: …`, `Shared file removed.` | [`Transfer`](SystemEventKind::Transfer) | Iced transfer handlers |
//! | `Download started — click play again …`, `Download in progress — click play again …`, `Stream ready: …` | [`Video`](SystemEventKind::Video) | Iced inline-video handlers |
//! | `Network error: …`, `Download failed: …`, `Open failed: …`, `Image/File upload failed: …`, `Catalogue fetch failed: …`, `Video verification failed: …`, `Could not delete room history: …`, `Failed to join room: …`, `Mailbox sync failed: …` | [`Error`](SystemEventKind::Error) | core `NetEvent::Error`, Iced failure paths |
//! | `Cannot react/edit/delete a system message`, `No message at index {n}`, `Unknown peer: …`, `Video is not ready to play yet.`, `Shared file is no longer available.`, `… not yet implemented.`, `Group invite not found or expired.`, `Select at least one friend to invite.`, `Inline video playback unavailable: …`, `Rejected invalid contact control message.` | [`Warning`](SystemEventKind::Warning) | Iced command/validation paths |
//! | `Chat joined.`, `Conversation cleared.`, `No known peers to inspect.`, `  {peer}: {status}` (friend-list rows), anything unrecognised | [`Information`](SystemEventKind::Information) | fallback — never discarded |

use crate::chat_core::ChatKind;

/// Semantic treatment for a chat system message.
///
/// The original message text remains the source of truth; this type only
/// selects the semantic bucket an incoming system message belongs to.
/// Classifying at the data layer lets frontends render join/rename/help
/// entries differently (chip accent, grouping, spacing) without parsing
/// prose themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemEventKind {
    /// A peer joined the chat or came online.
    Join,
    /// A peer left the chat or went offline.
    Leave,
    /// A peer changed their display name.
    Rename,
    /// A remote peer shared a file.
    FileShared,
    /// Command help / usage text (slash-command feedback).
    CommandHelp,
    /// A hard error or failure notice.
    Error,
    /// A soft warning that is not a hard error.
    Warning,
    /// Whisper / mailbox / offline-DM channel activity.
    Whisper,
    /// Room or group invitations (sent or received).
    Invite,
    /// Secure tunnel lifecycle notices.
    Tunnel,
    /// File-transfer / download lifecycle notices.
    Transfer,
    /// Video playback guidance or streaming notices.
    Video,
    /// Friend-list management notices.
    Friend,
    /// Profile-change notices.
    Profile,
    /// Mesh / connection-health notices.
    Mesh,
    /// Generic informational notice (fallback; nothing is discarded).
    Information,
}

/// Classify a system-message body into a semantic [`SystemEventKind`].
///
/// The mapping is deterministic and total: every input — including empty or
/// unrecognised text — maps to exactly one variant, with
/// [`SystemEventKind::Information`] as the fallback, so no incoming system
/// message is ever silently discarded. The original `text` is never modified;
/// callers own the raw body and may render it verbatim.
///
/// # Extension guide
///
/// 1. Add the new variant to [`SystemEventKind`] (or reuse an existing one).
/// 2. Add a match arm below — put unambiguous formats (exact prefixes, marker
///    substrings) before the generic error/warning keyword buckets.
/// 3. Add a row to the `known_formats_map_to_expected_variants` test table.
pub fn classify_system_event(text: &str) -> SystemEventKind {
    let normalized = text.trim().to_ascii_lowercase();

    // 1. Bracketed channel prefixes are unambiguous (whisper / mailbox / offline DM).
    if normalized.starts_with("[whisper")
        || normalized.starts_with("[mailbox")
        || normalized.starts_with("[offline dm")
    {
        return SystemEventKind::Whisper;
    }
    // 2. Rename: "<short-key> is now known as <name>".
    if normalized.contains("is now known as") {
        return SystemEventKind::Rename;
    }
    // 3. File share: "<name> shared a file" (the download card carries the
    //    filename; VIDCARD-12 removed the ": <file>" suffix to avoid
    //    duplicating long filenames in the chat log).
    if normalized.contains("shared a file") {
        return SystemEventKind::FileShared;
    }
    // 4. Peer membership — join / leave.
    if normalized.ends_with(" joined the chat")
        || normalized.contains(" is now online")
        || (normalized.contains('🟢')
            && (normalized.contains("joined") || normalized.contains("is online")))
    {
        return SystemEventKind::Join;
    }
    if normalized.ends_with(" left the chat") || normalized.contains(" is now offline") {
        return SystemEventKind::Leave;
    }
    // 5. Secure tunnel lifecycle.
    if normalized.starts_with("tunnel ") {
        return SystemEventKind::Tunnel;
    }
    // 6. Command help / usage.
    if normalized.starts_with("usage:") || normalized.contains("/help") {
        return SystemEventKind::CommandHelp;
    }
    // 7. Room / group invitations.
    if normalized.contains("invited you to")
        || normalized.contains("opened a private chat with you")
        || normalized.starts_with("room invite sent via whisper")
        || normalized.starts_with("invite to join this room")
    {
        return SystemEventKind::Invite;
    }
    // 8. Friend-list management.
    if normalized.starts_with("added friend:")
        || normalized.starts_with("updated friend:")
        || normalized.starts_with("removed friend:")
        || normalized.starts_with("no friends tracked")
        || normalized.starts_with("friends (")
    {
        return SystemEventKind::Friend;
    }
    // 9. Profile changes.
    if normalized.contains("profile image") {
        return SystemEventKind::Profile;
    }
    // 10. Mesh / connection health.
    if normalized.starts_with("mesh ") || normalized.contains("gossip receiver closed") {
        return SystemEventKind::Mesh;
    }
    // 11. Transfer lifecycle — positive signals first; failures fall through to Error below.
    if normalized.starts_with("download queued")
        || normalized.starts_with("sharing:")
        || normalized.ends_with("is complete")
        || normalized.starts_with("shared file added")
        || normalized.starts_with("shared file removed")
    {
        return SystemEventKind::Transfer;
    }
    // 12. Video-playback guidance (started/in-progress/stream-ready).
    if normalized.starts_with("download started")
        || normalized.starts_with("download in progress")
        || normalized.contains("stream ready")
    {
        return SystemEventKind::Video;
    }
    // 13. Hard errors.
    if normalized.contains("failed")
        || normalized.contains("error")
        || normalized.contains("could not")
        || normalized.contains("invalid")
    {
        return SystemEventKind::Error;
    }
    // 14. Soft warnings.
    if normalized.contains("cannot ")
        || normalized.contains("not ready")
        || normalized.contains("no longer available")
        || normalized.contains("unavailable")
        || normalized.contains("not yet implemented")
        || normalized.contains("unknown peer")
        || normalized.contains("no message at index")
        || normalized.contains("not found")
        || normalized.contains("expired")
        || normalized.contains("no ticket available")
        || normalized.contains("select at least one")
        || normalized.contains("is required.")
        || normalized.contains("rejected")
    {
        return SystemEventKind::Warning;
    }
    // 15. Generic video mentions.
    if normalized.contains("video") {
        return SystemEventKind::Video;
    }
    // 16. Fallback: generic information — never silently discarded.
    SystemEventKind::Information
}

/// Classify an entry's body if it is a system entry; `None` otherwise.
///
/// Convenience wrapper for callers that hold a [`ChatEntry`](crate::chat_core::ChatEntry)
/// and only need the semantic kind when it is a system notice.
pub fn classify_entry(entry: &crate::chat_core::ChatEntry) -> Option<SystemEventKind> {
    match entry.kind {
        ChatKind::System => Some(classify_system_event(&entry.body)),
        ChatKind::Local | ChatKind::Remote => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every format produced by the chat core and the Iced frontend, verbatim
    /// from the call sites, mapped to its expected semantic variant.
    #[test]
    fn known_formats_map_to_expected_variants() {
        let cases: &[(&str, SystemEventKind)] = &[
            // ── Join ────────────────────────────────────────────────────────
            ("alice joined the chat", SystemEventKind::Join),
            ("🟢 bob joined", SystemEventKind::Join),
            ("🟢 bob is online", SystemEventKind::Join),
            ("Friend alice is now ONLINE", SystemEventKind::Join),
            // ── Leave ───────────────────────────────────────────────────────
            ("alice left the chat", SystemEventKind::Leave),
            ("Friend alice is now offline", SystemEventKind::Leave),
            // ── Rename ──────────────────────────────────────────────────────
            ("ab12cd34 is now known as Alice", SystemEventKind::Rename),
            // ── FileShared ──────────────────────────────────────────────────
            ("Alice shared a file", SystemEventKind::FileShared),
            // Backward-compatible with older persisted lines that included
            // the filename after a colon.
            ("Alice shared a file: report.pdf", SystemEventKind::FileShared),
            // ── CommandHelp ─────────────────────────────────────────────────
            ("Usage: /react <msg_index> <emoji>", SystemEventKind::CommandHelp),
            ("Usage: /edit <msg_index> <new_text>", SystemEventKind::CommandHelp),
            ("Usage: /delete <msg_index>", SystemEventKind::CommandHelp),
            ("Usage: /whisper <peer-key|friend-alias> <message>", SystemEventKind::CommandHelp),
            (
                "Type a message and press Enter to send.  /help for commands.",
                SystemEventKind::CommandHelp,
            ),
            // ── Tunnel ─────────────────────────────────────────────────────
            ("Tunnel request accepted", SystemEventKind::Tunnel),
            ("Tunnel request declined", SystemEventKind::Tunnel),
            ("Tunnel closed", SystemEventKind::Tunnel),
            // ── Whisper ─────────────────────────────────────────────────────
            ("[Whisper] Connected to alice", SystemEventKind::Whisper),
            ("[Whisper] Disconnected from alice", SystemEventKind::Whisper),
            ("[Whisper to alice] hello there", SystemEventKind::Whisper),
            (
                "[Mailbox] Failed to accept envelope from alice: boom",
                SystemEventKind::Whisper,
            ),
            (
                "[Offline DM sync: received 2 messages from alice]",
                SystemEventKind::Whisper,
            ),
            // ── Invite ─────────────────────────────────────────────────────
            ("alice invited you to room abc123", SystemEventKind::Invite),
            (
                "alice invited you to group \"Team\" (see REQUESTS section to accept)",
                SystemEventKind::Invite,
            ),
            ("alice opened a private chat with you.", SystemEventKind::Invite),
            (
                "Room invite sent via whisper to ab12cd34",
                SystemEventKind::Invite,
            ),
            (
                "Invite to join this room (boru1): boru1:abc…",
                SystemEventKind::Invite,
            ),
            // ── Friend ─────────────────────────────────────────────────────
            ("Added friend: alice", SystemEventKind::Friend),
            ("Updated friend: alice", SystemEventKind::Friend),
            ("Removed friend: alice", SystemEventKind::Friend),
            ("No friends tracked yet.", SystemEventKind::Friend),
            ("Friends (3):", SystemEventKind::Friend),
            // ── Profile ─────────────────────────────────────────────────────
            ("Profile image updated.", SystemEventKind::Profile),
            ("Profile image removed.", SystemEventKind::Profile),
            ("Saving profile image…", SystemEventKind::Profile),
            // ── Mesh ────────────────────────────────────────────────────────
            ("Mesh degraded: No peers in the mesh", SystemEventKind::Mesh),
            ("Mesh offline: Not connected to any room", SystemEventKind::Mesh),
            ("Mesh recovered: all peers active.", SystemEventKind::Mesh),
            ("Mesh recovered: endpoint back online.", SystemEventKind::Mesh),
            ("The gossip receiver closed.", SystemEventKind::Mesh),
            // ── Transfer ────────────────────────────────────────────────────
            ("Sharing: photo.jpg", SystemEventKind::Transfer),
            ("Download queued for *alice* (id=42)", SystemEventKind::Transfer),
            ("*photo.jpg* is complete", SystemEventKind::Transfer),
            (
                "Shared file added: report.pdf (1234 bytes)",
                SystemEventKind::Transfer,
            ),
            ("Shared file removed.", SystemEventKind::Transfer),
            // ── Video ───────────────────────────────────────────────────────
            (
                "Download started — click play again when the progress bar reaches 100%.",
                SystemEventKind::Video,
            ),
            (
                "Download in progress — click play again when complete.",
                SystemEventKind::Video,
            ),
            (
                "Stream ready: http://127.0.0.1:9999/video\nPaste this URL into VLC or your browser to watch.",
                SystemEventKind::Video,
            ),
            // ── Error ───────────────────────────────────────────────────────
            ("Network error: connection refused", SystemEventKind::Error),
            ("Download failed: timeout", SystemEventKind::Error),
            ("Open failed: permission denied", SystemEventKind::Error),
            ("Image upload failed: too big", SystemEventKind::Error),
            ("File upload failed: disk full", SystemEventKind::Error),
            ("Catalogue fetch failed: 404", SystemEventKind::Error),
            ("Video verification failed: hash mismatch", SystemEventKind::Error),
            ("Could not delete room history: locked", SystemEventKind::Error),
            ("Failed to join room: invalid ticket", SystemEventKind::Error),
            ("Mailbox sync failed: peer offline", SystemEventKind::Error),
            // ── Warning ─────────────────────────────────────────────────────
            ("Cannot react to a system message", SystemEventKind::Warning),
            ("Cannot edit a system message", SystemEventKind::Warning),
            ("Cannot delete a system message", SystemEventKind::Warning),
            ("No message at index 5", SystemEventKind::Warning),
            (
                "Unknown peer: xyz. Use a public key or friend alias.",
                SystemEventKind::Warning,
            ),
            ("Group name is required.", SystemEventKind::Warning),
            ("Select at least one friend to invite.", SystemEventKind::Warning),
            ("Shared file is no longer available.", SystemEventKind::Warning),
            ("Video is not ready to play yet.", SystemEventKind::Warning),
            ("Cannot download video: unknown size.", SystemEventKind::Warning),
            (
                "Video cannot be played because its content identity is missing.",
                SystemEventKind::Warning,
            ),
            (
                "Pause requested — transfer suspension not yet implemented.",
                SystemEventKind::Warning,
            ),
            (
                "Resume requested — transfer resumption not yet implemented.",
                SystemEventKind::Warning,
            ),
            ("Group invite not found or expired.", SystemEventKind::Warning),
            ("Group not found. Cannot send invite.", SystemEventKind::Warning),
            (
                "Cannot accept group invite: no ticket available. Ask the sender to re-invite.",
                SystemEventKind::Warning,
            ),
            (
                "Inline video playback unavailable: missing GStreamer.",
                SystemEventKind::Warning,
            ),
            // ── Information (fallback) ──────────────────────────────────────
            ("Chat joined.", SystemEventKind::Information),
            ("Conversation cleared.", SystemEventKind::Information),
            ("No known peers to inspect.", SystemEventKind::Information),
            ("  ab12cd34: Online", SystemEventKind::Information),
            (
                "Direct file transfer is disabled; use the authorised file catalogue.",
                SystemEventKind::Information,
            ),
        ];

        for (text, expected) in cases {
            assert_eq!(
                classify_system_event(text),
                *expected,
                "unexpected variant for {text:?}"
            );
        }
    }

    /// The mapping is total: unrecognised text still maps to a concrete
    /// variant (Information) instead of being dropped.
    #[test]
    fn unrecognised_text_falls_back_to_information() {
        for text in [
            "",
            "   ",
            "hello world",
            "some completely unknown message",
            "👍",
            "a".repeat(10_000).as_str(),
        ] {
            assert_eq!(
                classify_system_event(text),
                SystemEventKind::Information,
                "expected Information fallback for {text:?}"
            );
        }
    }

    /// Classification is case-insensitive and tolerant of surrounding whitespace.
    #[test]
    fn classification_normalizes_case_and_whitespace() {
        assert_eq!(
            classify_system_event("  FRIEND ALICE IS NOW ONLINE  "),
            SystemEventKind::Join
        );
        assert_eq!(
            classify_system_event("Alice LEFT the chat"),
            SystemEventKind::Leave
        );
        assert_eq!(
            classify_system_event("Usage: /REACT <msg_index> <emoji>"),
            SystemEventKind::CommandHelp
        );
    }

    /// Domain-specific positive formats win over the generic error/warning
    /// keyword buckets (precedence guard).
    #[test]
    fn specific_formats_take_precedence_over_keyword_buckets() {
        // "failed" is an Error keyword, but the bracket prefix is the stronger signal.
        assert_eq!(
            classify_system_event("[Mailbox] Failed to accept envelope from alice: boom"),
            SystemEventKind::Whisper
        );
        // "Download queued" is a Transfer lifecycle notice, not an error.
        assert_eq!(
            classify_system_event("Download queued for *alice* (id=42)"),
            SystemEventKind::Transfer
        );
        // "Mesh offline" is a Mesh notice even though it mentions offline.
        assert_eq!(
            classify_system_event("Mesh offline: Not connected to any room"),
            SystemEventKind::Mesh
        );
    }

    /// `classify_entry` only classifies system entries; local/remote stay `None`.
    #[test]
    fn classify_entry_respects_chat_kind() {
        let system = crate::chat_core::ChatEntry::system("alice joined the chat");
        assert_eq!(
            classify_entry(&system),
            Some(SystemEventKind::Join),
            "system entry must classify"
        );
        let local = crate::chat_core::ChatEntry::local("me", "hello");
        assert_eq!(
            classify_entry(&local),
            None,
            "local entry has no system kind"
        );
        let remote = crate::chat_core::ChatEntry::remote("alice", "hi");
        assert_eq!(
            classify_entry(&remote),
            None,
            "remote entry has no system kind"
        );
    }
}
