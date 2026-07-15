---
target: examples/iced_chat/app.rs
total_score: 22
p0_count: 0
p1_count: 3
p2_count: 3
timestamp: 2026-07-12T00-03-28Z
slug: examples-iced-chat-app-rs
---
# Design Critique: Boru Chat GUI

## Design Health Score

| # | Heuristic | Score | Key Issue |
|---|-----------|-------|-----------|
| 1 | Visibility of System Status | 3 | Message delivery states shown via icons, mesh health visible, but no loading indicator for room joins |
| 2 | Match System / Real World | 3 | Chat conventions followed (bubbles, timestamps), but technical jargon ("TopicId", "RelayMode") leaks into UI |
| 3 | User Control and Freedom | 3 | Back navigation from chat, escape from help overlay, delete-confirm flow. No undo for accidental deletes |
| 4 | Consistency and Standards | 2 | 5 different button styling approaches (inline closures everywhere), inconsistent border radii (0/6/8), no shared component vocabulary |
| 5 | Error Prevention | 2 | Delete confirmation is good, but no protection against sending to wrong room, no draft recovery on crash |
| 6 | Recognition Rather Than Recall | 2 | Emoji-only buttons with no accessible labels; user must memorize what each emoji means |
| 7 | Flexibility and Efficiency | 1 | No keyboard shortcuts for primary actions (send is Enter, that's it), no batch operations, no command palette |
| 8 | Aesthetic and Minimalist Design | 2 | 7 stacked surface containers on chat list with identical visual weight; 32 emoji occurrences as UI; chat list information overload |
| 9 | Error Recovery | 2 | Error messages shown when room joins fail, but no retry guidance or suggestions for what went wrong |
| 10 | Help and Documentation | 2 | Help overlay exists but is a wall of slash commands; no contextual help, no onboarding, no search |
| **Total** | | **22/40** | **Acceptable — significant improvements needed** |

### Cognitive Load Assessment

| Checklist Item | Status | Notes |
|----------------|--------|-------|
| Single focus | FAIL | Chat list has 7 competing sections: identity, ticket, actions, input, rooms, friends, users |
| Chunking | PASS | Content does use section cards and spacing |
| Grouping | PASS | Related items visually grouped within sections |
| Visual hierarchy | FAIL | All sections have identical container_surface styling; nothing is visually prioritized |
| One thing at a time | PASS | Screens are distinct (list vs chat vs settings vs help) |
| Minimal choices | FAIL | Chat list decision points have 5+ options (New Chat, Join, ticket input, Friends, Discovered) |
| Working memory | FAIL | User must remember friend names across multiple screens; no search/filter for rooms |
| Progressive disclosure | FAIL | All chat list content shown at once; no expandable sections |

**Cognitive load verdict: 4 failures — high cognitive load, critical to address**

### Emotional Journey

- **Onboarding**: No first-run experience. User lands on a dense chat list with no guidance on what to do first.
- **Creating/Joining a room**: Reasonably smooth. New Chat and Join buttons are visible, but mixed in with everything else.
- **In a chat**: Chat experience is decent. Bubbles, timestamps, composer with clear send affordance. Green/blue asymmetry works.
- **Settings**: Clean section-card layout with max-width, good.
- **Peak moment**: Sending a message and seeing it appear in a colored bubble. Receiving a message successfully.
- **Valley**: Landing on the chat list for the first time with 7 sections of identical-looking cards and no clear "what now?".

## Anti-Patterns Verdict

**LLM Assessment**: The app largely avoids the most obvious AI slop patterns — no gradient text, no glassmorphism, no side-stripe borders, no numbered section markers, no identical card grids with icon+heading+text. However, the heavy emoji-as-icon usage and section-card-on-card nesting pattern (6+ `container_surface` calls stacked) gives the interface a "built by a solo developer who ships fast" feel rather than a designed one. The 5 inconsistent button styling approaches are the strongest code-level "not designed" signal.

**Deterministic Scan**: CLI detector returned empty (no HTML markup to scan). This is expected for a Rust native GUI.

## What's Working

1. **Dark/light theme with systematic palette**: The color functions (`bg_primary`, `bg_surface`, `bg_input`, `accent_primary`, `accent_green`) form a proper design token system with documented contrast ratios. Both themes are coherent.

2. **Chat bubbles with sent/received asymmetry**: Local messages are right-aligned with green tint, remote messages left-aligned with blue tint. Clear sender attribution without needing to read labels.

3. **Virtualized chat log rendering**: The `ChatLogLayoutCache` with 800px overscan, binary-search indexing, and dirty-flag optimization is a sophisticated implementation that keeps performance smooth even with thousands of messages.

4. **Message timestamps with day separators**: The "— date —" dividers create clear temporal landmarks in long conversations.

5. **Composer progressive disclosure**: Send button switches from ghost (empty) to filled green (text exists) — clear, intentional affordance.

## Priority Issues

### [P1] Flat Visual Hierarchy on Chat List

**What**: 7 sections (identity card, ticket, action buttons, join input, recent chats, friends, discovered users) all wrapped in identical `container_surface` with no visual prioritization. The primary user actions ("New Chat", "Join") sit at the same visual weight as the friend status list and ticket information.

**Why it matters**: New users don't know what to do first. Power users waste time visually scanning a page where everything looks equally important. This creates the cognitive load failure described above.

**Fix**: Establish a clear visual hierarchy:
- Primary actions (New Chat, Join) should be visually distinguished — accent-colored buttons, not wrapped in surface containers
- Group the secondary/utility sections (friends, discovered users) under a collapsible section or at reduced visual weight
- Move identity info and ticket display to a minimized header bar rather than a full card

**Suggested command**: $impeccable layout

### [P1] No Consistent Button System (5 Different Styles)

**What**: Buttons use 5 different styling approaches across the app:
1. Default iced style (New Chat, Join, Chat, ✕ delete)
2. `button::text` style (ticket copy)
3. Inline ghost closures (help ❓, attach 📎)
4. Inline filled closure (send ➤, green fill)
5. Inline surface-closure (← Back)

Three different border radii: 0.0 (ghost), 6.0 (send), 8.0 (back button, composer, bubbles).

**Why it matters**: Every button looks like it was designed independently. Users build a mental model of how buttons look and behave; inconsistent visual vocabulary undermines that trust.

**Fix**: Extract a button style system with 2-3 named variants (primary, secondary, ghost) as reusable functions. Standardize border radius to one value for all buttons.

**Suggested command**: $impeccable extract

### [P1] 32 Emoji Occurrences as UI (Accessibility Fail)

**What**: 32 emoji used as the primary or only representation for interactive elements: ⚙ (gear/settings), ◀ (back), ❓ (help), 📎 (attach), ➤ (send), 👤 (avatar fallback), 🟢/🔴 (online status), 💬 (chat action), ✕ (delete), etc. Plus section headers: 🎨 (appearance), 🔔 (notifications), 🌐 (network), 📡 (relay), 📋 (logs), 💾 (data), ☀/🌙 (theme toggle), 🔊/🔇 (sound).

**Why it matters**: Emoji rendering varies by platform (they look different on Linux vs macOS vs Windows vs Android). Emoji have no accessible name/role semantics — screen readers may read "gear" or may read nothing. Color-dependent emoji (🟢/🔴 for online status) are invisible to colorblind users. This isn't just "cosmetic" — it's a real accessibility failure.

**Fix**: Replace emoji icons with proper icon rendering (SVG icons via iced_image, or Unicode symbols with accessible text labels). Add text labels alongside key icon-only buttons. Use text + icon for status indicators (not emoji alone).

**Suggested command**: $impeccable colorize / $impeccable clarify

### [P2] Dark Mode Contrast Deficiencies

**What**: `text_system` on dark mode (~#808080 on #2a2a3e) yields ~3.8:1 contrast — fails WCAG AA 4.5:1. Dark surface (#2a2a3e) vs background (#1a1a2e) differ by only ~0.1 RGB, making cards barely distinguishable from the page.

**Why it matters**: Users with low vision or working in bright environments will struggle to read system messages and distinguish visual containers in dark mode.

**Fix**: Bump `text_system` on dark to at least ~#999 or darker of the accent hue. Increase surface/background contrast in dark mode by at least 0.05 in luminance.

**Suggested command**: $impeccable audit

### [P2] No Keyboard Shortcuts or Accelerators

**What**: The only keyboard interaction is Enter to send a message. No Ctrl+N for new chat, Ctrl+[, Escape for back navigation, Ctrl+K for command palette, Ctrl+W to close settings, / for slash-command autocomplete.

**Why it matters**: Chat apps are used heavily and repeatedly. Every second saved by keyboard shortcuts compounds. Power users (Alex persona) will abandon an app that requires mouse-only navigation for repeated actions.

**Fix**: Add keyboard shortcuts: Ctrl+N (new chat), Ctrl+Backspace (back), Escape (close settings/help), / (focus composer with slash), Ctrl+K (command palette). Use iced's subscription system for key events.

**Suggested command**: $impeccable delight / $impeccable harden

### [P2] No Onboarding or First-Run Experience

**What**: On first launch with no room history, the user sees: an identity card, empty "No recent chats", empty "No friends", empty "No other users discovered". 4 empty states plus action buttons and a ticket input. No guidance on what to do first.

**Why it matters**: A new user who just installed the app has no idea what "Boru Chat" is or how to start using it. They need a clear first step: "Create a new chat room or join an existing one via ticket."

**Fix**: Add a first-run overlay or simplified empty-state that guides the user to their first action. Combine the 4 empty states into one clear "Get Started" message. Hide/simplify the advanced sections (friends, discovered users) until the user has at least one room.

**Suggested command**: $impeccable onboard

## Persona Red Flags

### Alex (Power User)

- **No keyboard shortcuts**: The only key interaction is Enter to send. No shortcut for New Chat, back navigation, or settings.
- **Slow navigation**: Creating a new chat requires: (1) click "New Chat", (2) wait for room creation, (3) click into the room. No direct room search or type-to-join.
- **No bulk operations**: Cannot batch-delete rooms or clear multiple friend entries.
- **No way to manage rooms**: Room list is scrollable with no search or sort.
- **Will hate the emoji-as-icons**: 5 different emoji in the composer alone is visually noisy.

### Jordan (First-Timer)

- **Landing page overwhelm**: 7 sections of identical surface cards. Where do I click first?
- **Jargon everywhere**: "Identity: ..." "Relay: ..." "TopicId" "Mesh: healthy". None of this means anything to a new user.
- **No clear next step**: The empty states say "No recent chats. Create a new chat or join an existing one." — but "Create a new chat" is a small button buried in a card that looks like everything else.
- **Emoji confusion**: What does ⚙ do? What does ❓ do in the composer? They have no text labels.

### Sam (Accessibility User)

- **Icon-only buttons**: 5+ buttons with emoji/Unicode-only representation. Screen readers may or may not announce them, and the meaning isn't conveyed.
- **Color-only indicators**: 🟢/🔴 for online status — colorblind users cannot distinguish.
- **Emoji rendering**: Cross-platform emoji rendering differences mean the "gear" icon might look completely different on Sam's system compared to the designer's.
- **Keyboard navigation**: No tab-order guarantee on the custom iced components. Focus indicators are unverified.

## Minor Observations

- The help overlay replaces the chat screen completely without a backdrop/dim effect — feels like a page transition, not an overlay (line 4615-4622)
- Section headers in settings use all-caps with emoji ("👤  IDENTITY", "🎨  APPEARANCE") — this is close to the "tiny uppercase eyebrow" anti-pattern, slightly mitigated by the emoji
- Ticket text is rendered at TYPO_XXS (10px) — extremely small for a key sharing mechanism
- The "Discovered Users" section shows 🟢 for all non-friend users regardless of actual online status — misleading
- Bubble max_width (480px, line 4869) is a hardcoded constant that should be extracted to the spacing/token system
- Day-separator uses the `— date —` format with text in `text_system` color — at ~#808080 on dark (#2a2a3e), sub-4.5:1 contrast

## Questions to Consider

- Could the chat list be simplified to show only rooms and a "+" button, with friends/discovered moved to a sidebar or second screen?
- If the app uses custom iced styling everywhere anyway, why not build a proper icon set (SVG via image widget) instead of emoji?
- What would a 3-tier hierarchy look like: primary actions → active content → secondary panels?
- Does the "Discovered Users" section justify its prominent placement, or is it noise until the user has at least 5 friends?
- Should the app collapse to a single-window chat-by-chat view (like Telegram desktop) rather than showing the inbox as the default, when a user already has rooms?
