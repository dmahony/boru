# UX Audit: Boru (Iced GUI)

**Date:** 2026-07-21  
**Version audited:** commit 02fac77 (latest UI redesign from Step 1–8)  
**Scope:** All view functions in `examples/iced_chat/app.rs`, `download_progress_view.rs`, `file_library.rs`, `main.rs`  
**Method:** Static code review of all rendering logic, no runtime testing.

---

## Executive Summary

Boru's Iced GUI is a functional, privacy-first peer-to-peer messaging application with a clean design system, consistent typography/spacing, proper dark/light mode support, and well-structured screens. The redesign from Steps 1–8 has significantly improved visual polish.

**Overall UX maturity:** Early-stage but solid foundation. The app is usable by a technically-literate user but presents barriers for non-technical and first-time users. Most issues are medium-impact; one high-impact dead button was found.

**Critical score:** 6/10 — functional but has friction points that will cause confusion or frustration.

---

## Persona Walkthroughs

### 1. First-Time User — "Can I start chatting?"

**Flow:**
1. Launch app → secret key generated silently → main window appears.
2. See "BORU" heading + "Private. Peer-to-peer. No central servers." tagline.
3. Status card shows connection health, relay mode, friend count.
4. Four action buttons: "Start Chat", "Add Friend", "Join Ticket", "Browse Files".
5. Recent activity feed (empty on first launch).

**What works:**
- The empty state is welcoming and communicates the app's privacy ethos immediately.
- Action buttons are large and clearly labelled.
- The sidebar is always visible for orientation.

**What doesn't:**
- **No onboarding flow.** A first-time user has zero guidance on what to do first. The four buttons assume domain knowledge.
- **"Start Chat" creates a random room** — but the user has no context for what a "room" is. After creation, the room exists but nobody can find it unless the user shares the ticket or DHT discovery is enabled (which uses networking jargon the user might not understand).
- **"Add Friend"** opens a screen asking for a "Peer public key" (52-character hex string) — this is a hard ask for a non-technical user.
- **The "+" button** in the sidebar header opens a menu with "Add Friend", "Join Ticket", "Scan QR Code" (disabled), "Import Friend" — but the user has no way of knowing what these do without trial-and-error.
- **"Join Ticket"** shows an input labelled "Enter ticket ID" — no explanation of what a ticket is or where to get one.

**30-second test:** A first-time user *cannot* start chatting in under 30 seconds. They'd need to either: (a) figure out / know how to share a room ticket with someone, or (b) have a friend send them a ticket. Both require out-of-band coordination.

---

### 2. Technical User — "Can I find networking info?"

**Flow:**
1. Settings screen (from ⚙ in sidebar or "Settings" in chat header).
2. Network section shows: Peer ID, direct/relayed/neighbor counts, mesh health.
3. Relay section shows mode info.
4. Identity section shows Friend ID with "Copy" button (shows "Copied!" feedback).
5. "Add friend by key…" input accepts public key strings.
6. Friend profile has three-dot menu with "Copy Public Key" option.

**What works:**
- Full public key is displayed with glyph wrapping for narrow windows.
- Copy button with visual feedback (changes to "Copied!").
- Connection statistics (direct/relay/neighbors) are transparent.
- Mesh health indicator (Good/Degraded/Offline with reason).
- "Browse Files" button on discovered peers and friend profiles.

**What doesn't:**
- **No "Export Friend" feature.** The Add menu has "Import Friend" but no corresponding export. A technical user who wants to back up or transfer their contacts has no way to do so.
- **Import/Export path unclear.** "Import Friend" presumably reads a file, but the format is undocumented in the UI.
- **Discovered peers** show a "Chat" and "Browse Files" button on every row — if there are many peers, this creates visual clutter. The buttons are small, but still repeated for every entry.
- **No search bar** in chats or friends sections. With many conversations, the sidebar becomes a scrollable list with no filter.

---

### 3. Non-Technical / Elderly User

**What works:**
- Font size is configurable (XS through XL) in settings.
- Dark/light mode toggle with clear label.
- Buttons use the new design system with min 32px height — adequate touch targets.
- Color contrast is reasonable in both themes.
- Status indicators (●/○) are simple and well-understood.
- Chat composer placeholder says "Type a message…" — clear.
- Help overlay (from "?" button) lists commands in plain language.

**What doesn't:**
- **"DHT discovery"** in the Create Room dialog is pure jargon. A checkbox labelled "Enable DHT discovery" means nothing to a non-technical user.
- **"Add friend by key…"** expects a hex public key. No QR code scanning (the menu option is disabled). No username system. No phone-book-style integration.
- **The three-dot menu** (⋮) on friend profile uses `\u{22ee}` (⋮ vertical ellipsis) which is not as universally recognised as "…" (horizontal). The menu items "Remove Friend" and "Block Friend" are in red which is good, but destructive actions aren't preceded by a second confirmation step in the sidebar (only in the full friend profile).
- **"Mesh: degraded — reason"** shown on the empty state — the term "mesh" is networking jargon.
- **"Relay: …"** shown on the empty state — "relay" is not common knowledge.
- **"Enable DHT discovery"** checkbox has no help text explaining what DHT does or why a user might want it.

---

### 4. Privacy-Conscious User

**What works:**
- Tagline on empty state: "Private. Peer-to-peer. No central servers." — clear privacy positioning.
- Full public key shown in settings with copy button.
- "Block Friend" and "Remove Friend" options in friend profile.
- "Clear history" in settings with confirm/cancel dialog.
- Chat history can be permanently deleted.
- Secret key is stored with 0o600 permissions on disk.
- Data directory permissions are 0o700.

**What doesn't:**
- **No encryption verification.** For a p2p chat app, there's no UI indicator showing that messages are encrypted, no fingerprint comparison, no "verified" badge.
- **No per-room privacy controls.** A privacy-conscious user might want per-room settings (e.g., "don't announce this room to DHT").
- **No "Report" function.** Only "Block" — if a user is being harassed, there's no mechanism to report them to anyone (though in a fully p2p app this is architecturally tricky, the UI doesn't acknowledge the gap).
- **The peer-to-peer nature is stated but not demonstrated.** A user can't visually confirm that no central server is involved — they have to trust the UI text.

---

## Screen-by-Screen Findings

### A. Empty State / Landing Screen (`view_main_empty_state`)

| Issue | Severity | Description |
|-------|----------|-------------|
| No onboarding | MEDIUM | First launch shows branding and buttons but no guidance. Consider a one-time welcome overlay or tooltip sequence. |
| Jargon on status card | LOW | "Mesh" and "Relay" are displayed on every launch. A casual user doesn't need to see these. Consider hiding them behind an "Advanced" toggle or showing them only in Settings. |

### B. Sidebar (`view_sidebar` and children)

| Issue | Severity | Description |
|-------|----------|-------------|
| No chat search | MEDIUM | With many conversations, the user must scroll through a flat list. No search/filter input. |
| No friend search | MEDIUM | Friends section only sorts alphabetically. No search/filter. |
| Emoji icons in header | LOW | The "+" and "⚙" buttons use text emoji rather than icons. On some platforms the rendering is inconsistent. |
| "+" menu items disabled without explanation | MEDIUM | "Scan QR Code", "Create Group Chat", "Pair Device" are greyed out with no tooltip or reason. The user doesn't know if these aren't implemented or need configuration. |
| Section headers are ALL-CAPS | INFO | Section headers ("CHATS", "FRIENDS", "DISCOVER", "REQUESTS") use all-caps which is fine but slightly less scannable than Title Case. Minor preference. |

### C. Chat Panel (`view_chat_panel`, `view_chat_header`, `view_chat_log`, `view_composer`)

| Issue | Severity | Description |
|-------|----------|-------------|
| Avatar rendering for messages | LOW | Messages without an avatar handle show a bare "?" in TYPO_XL. This looks like an error state rather than a missing-avatar fallback. Consider a coloured initial or person icon instead. |
| Chat "Settings" button goes to app settings | MEDIUM | The header "Settings" button (`OpenSettings`) goes to the global settings screen — not room-specific settings. Users may expect room-level options (mute, leave, share ticket, etc.). |
| No typing indicator | LOW | There's no visual indication when the other party is typing. |
| No delivery receipts shown inline | LOW | Messages have `delivery_state` but I couldn't find a visual indicator (✓, ✓✓, etc.) in the chat log rendering. The state is tracked but not surfaced. |
| Composer help button uses "?" | LOW | The "?" button for help is clear but the ordering places it *after* the send button (right-to-left: attach → send → help). Help is typically on the left or far from the primary action. |

### D. Settings Screen (`view_settings_screen`, `view_settings_screen_cached`)

| Issue | Severity | Description |
|-------|----------|-------------|
| Shared Files section inline | LOW | The shared files list is embedded at the bottom of settings. For users with many files, this will make the settings screen very long. Consider a separate screen. |
| "Clear history" two-click flow | INFO | The confirm/cancel on clear history is good UX for destructive actions. No issue. |
| Text size selector uses buttons with labels | INFO | The XS/SM/MD/LG/XL text size selector is clear and well-implemented with active state highlighting. |
| No import/export settings | LOW | No way to back up or migrate settings between installations. |

### E. Friend Requests (`view_friend_requests`)

| Issue | Severity | Description |
|-------|----------|-------------|
| "Peer public key…" placeholder | MEDIUM | The placeholder says "Peer public key…" which is intimidating. Consider "Friend's key…" or showing an example truncated key. |
| No success feedback | LOW | After sending a friend request, there's no toast or confirmation — the user must check the "Outgoing Requests" section. |
| Accept/Decline buttons use text | LOW | "Accept" and "Decline" are clear, but "Accept" uses the green button style and "Decline" uses the danger style — good. |

### F. Friend Profile (`view_friend_profile`)

| Issue | Severity | Description |
|-------|----------|-------------|
| **"Voice" button is dead (no on_press)** | **HIGH** | The "Voice" button (line 14306–14321) has `padding` and `style` but **no `on_press` handler**. Clicking it does absolutely nothing. This is the most actionable bug found. |
| Three-dot menu "View Profile" is a no-op | LOW | The menu item "View Profile" just closes the menu (toggles it off) — but the user is *already* looking at the profile. This is confusing. |
| Recent messages section is clickable | INFO | The entire recent messages section is wrapped in a button to open the chat — good interaction design. Not obvious from the visual, but functional. |

### G. Peer Profile / Catalogue (`view_peer_profile`, `view_peer_catalogue`)

| Issue | Severity | Description |
|-------|----------|-------------|
| Peer profile always shows "No shared files." | LOW | Even when the peer *does* have shared files, the profile view shows "No shared files." — this seems to be a stub. The full catalogue view is separate. |
| Catalogue table has no visual row striping | LOW | The file table in the catalogue uses `container_surface` for each row but no alternation. Dense data tables benefit from striping. |

### H. Help Overlay (`view_help`)

| Issue | Severity | Description |
|-------|----------|-------------|
| No "Esc to close" visible at all times | LOW | Footer says "Press Esc to close" but this is only visible after scrolling to the bottom of the help panel. |
| Command list is dense | INFO | The command reference is a flat list with no search or grouping expand/collapse. Fine for the current number of commands. |

### I. Download Progress (`download_progress_view.rs`)

| Issue | Severity | Description |
|-------|----------|-------------|
| Row is well-designed | INFO | State badge, progress bar, action buttons (Pause/Resume/Cancel/Retry/Open/Remove), failure reason with recovery hint — all present. This is one of the best-designed UI components. |

### J. Remove/Block Confirmation Dialogs

| Issue | Severity | Description |
|-------|----------|-------------|
| Dialog content helps user understand consequences | INFO | "You will no longer receive messages from them." on block is good. |

---

## Answers to Audit Questions

### Can a user start chatting in under 30 seconds?
**No.** The shortest path requires either: (1) creating a room and sharing the ticket with someone via another channel, or (2) knowing a friend's public key and sending a friend request. Both require pre-existing technical knowledge and out-of-band coordination. No onboarding exists to guide the user through either flow.

### Is every screen self-explanatory?
**Mostly no.** Screens that *are* self-explanatory: the chat panel (type and send), help overlay (command list), friend requests (send/accept/decline). Screens that *aren't*: the empty state (no guidance on first steps), create room dialog (DHT jargon), settings network section (raw counts without context). The app assumes domain knowledge about p2p networking.

### Are advanced networking features hidden appropriately?
**Partially.** The empty state shows "Mesh: healthy" and relay mode on every launch — these are advanced concepts. They should be in Settings only. The DHT discovery checkbox is shown to every user creating a room, which is inappropriate for a non-technical audience.

### Is the interface visually balanced?
**Yes.** The design system (consistent SPACE grid, typography, color tokens, rounded corners, button styles) produces a cohesive, well-proportioned layout. The sidebar (280px) and main panel split is balanced. Cards and sections have consistent padding and spacing.

### Are there any unnecessary clicks?
**Several:**
- To read a friend's shared files from their profile, the user clicks "Browse" in the Shared Files section, which opens a separate full-screen view. Could be integrated.
- The "+" menu adds an extra click to Add Friend (open menu → click Add Friend) vs. a direct button.
- Settings navigation: "← Back" is at the bottom of the settings page, requiring scroll. A top-left back button would be more ergonomic.

### Are error messages clear and helpful?
**Mostly.** Friend request errors show in red text below the relevant section. The download failure view is excellent — it shows failure title, stability label (Temporary/Terminal), message, recovery action, and diagnostics. **Room for improvement:** generic connection failures don't always have user-friendly text.

### Is the dark mode comfortable to read?
**Yes.** The dark mode uses proper color values (not just inverted light mode). Contrast ratios appear adequate. The text colors (`text_remote_body`, `text_system`, `text_muted`) are distinct and legible against both dark and light backgrounds.

### Are interactive elements large enough?
**Mostly yes.** The button design system enforces minimum 32px height. However:
- The sidebar conversation row buttons have 0 padding (width: Fill, padding: 0) — the clickable area is the full row width, so this is fine.
- The friend request accept/decline buttons in the sidebar are small (`[SPACE_2, SPACE_4]`) — these could benefit from larger touch targets.

---

## Recommendations (Prioritised)

### HIGH (must fix)

1. **Add `on_press` to the "Voice" button** in `view_friend_profile` (app.rs:14306). Currently it's a dead button with no action. Either implement voice calling/show a "coming soon" toast, or remove the button entirely.

2. **Create a one-time onboarding overlay** for first-launch users. A simple 3-step card explaining: (1) "This app connects you directly to other users — no servers involved", (2) "To chat, share your Friend ID or create a room and share the ticket", (3) "Your Friend ID is in Settings. Keep your secret key safe." Dismissable with "Got it".

### MEDIUM

3. **Replace "DHT discovery" with plain language.** "Allow others to find this room" or "Public room (anyone can join)". Move the raw networking terms behind an "Advanced" expander.

4. **Add search/filter to sidebar sections.** A text input at the top of the Chats and Friends sections would dramatically improve usability for users with many conversations.

5. **Move mesh/relay status from empty state to Settings.** The landing screen should show only essential status (online/offline, friend count). Network diagnostics belong in the Network settings section.

6. **Hide or explain disabled "+" menu items.** "Scan QR Code", "Create Group Chat", and "Pair Device" should either be removed, or display a small text note like "Coming soon".

7. **Add room-level settings.** The "Settings" button in the chat header currently opens global settings. Add room-specific options: mute notifications, share ticket, leave room, export chat history.

8. **Improve the "Add friend by key" flow.** Validate the key format in real-time (show a green checkmark or red error). Show a truncated example of what a key looks like. Consider adding a "Scan QR Code" option that actually works.

9. **Add "Export Friend" to match "Import Friend".** Technical users will want to back up their friend list. Export as a file (same format as import).

### LOW

10. **Replace "?" avatar fallback with initials.** When no avatar handle is available, show a coloured circle with the user's initial (like the sidebar does) instead of a bare "?" that looks like an error.

11. **Add toast notifications for transient actions.** Friend request sent, friend added, file downloaded — these should produce a temporary toast at screen top rather than requiring the user to navigate to a different screen to confirm.

12. **Fix the "Voice" button gap** by either removing it or adding a disabled state with a tooltip ("Voice calls not yet available").

13. **Add delivery status indicators** in the chat log. The data model already tracks `delivery_state` — surface it with a subtle ✓/✓✓ icon or clock icon.

14. **Add row striping to the peer catalogue file table** for easier scanning of dense file lists.

15. **Consider adding "Esc" handling note** more prominently in UI (perhaps a small persistent hint in the composer area when a dialog is open).

---

## Conclusion

Boru has a solid visual foundation — the design system is consistent, the color palette is well-chosen, and the overall layout is clean. The app is already usable by technically-literate users who understand p2p networking concepts.

The **single highest-impact issue** is the dead "Voice" button, which will cause immediate confusion when clicked. Beyond that, the main UX gap is the **lack of onboarding** and the **exposure of networking jargon** to first-time users. The core chat experience (send/receive messages, share files, add friends) works well once the user understands the mental model.

Fixing the HIGH issue (Voice button) and tackling the MEDIUM issues (onboarding, jargon reduction, search, room-level settings) would bring the UX to a good baseline for wider release.
