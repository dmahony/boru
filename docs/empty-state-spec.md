# Empty State Specification

> **Version:** 1.0  
> **Scope:** `examples/iced_chat/` — the `iced` desktop GUI for Boru  
> **Purpose:** Implementation-ready shared specification for all empty states.  
> **Audience:** Developers implementing empty-state handling.

---

## 1. Design Principles

1. **Concise.** Empty states are a single short sentence (14–22 words). No multi-paragraph explanations.
2. **No technical terminology.** Never use "mesh", "relay", "DHT", "node", "peer ID", "key material", or similar jargon. Use plain user-facing language.
3. **At most one action per empty state.** Zero actions is preferred. If an action exists, it must be a single inline text button (ghost style) — never a card, never a large CTA.
4. **No large illustrations.** No decorative SVG, emoji art, or oversized graphics. Empty states are text-only (or text + optional inline ghost button).
5. **Shared visual style.** Every empty state uses the same typography size, colour, padding, and alignment (see §3).

---

## 2. Section Copy & Actions

| Section | Empty-state text | Action? | Action label | Rationale |
|---------|------------------|---------|--------------|-----------|
| **Chats** | "No conversations yet. Start a chat with one of your friends." | Optional | "Start Chat" | The main panel landing screen already has a "Start Chat" card (§4 of the landing-screen layout). In the sidebar Chats section, the action is omitted — the empty text alone is sufficient. If a developer chooses to add the action, use a ghost-style button that emits `AppMessage::CreateNewRoom`. |
| **Friends** | "No friends added yet. Add someone using a key or invitation." | Optional | "Add Friend" | The main panel landing screen already has an "Add Friend" card. In the sidebar Friends section, the action is omitted. If a developer chooses to add the action, use a ghost-style button that emits `AppMessage::OpenFriendRequests`. |
| **Discover** | "No peers discovered yet. Peers on your local network will appear here." | No | — | Discovery is automatic via an existing subscription. A manual refresh action was considered and removed per Step 17 requirements to avoid visual noise for no benefit. |
| **Requests** | "No pending requests. New friend requests will appear here." | No | — | The section header already includes a "Manage Requests →" link. Adding an action in the empty state itself would duplicate it. |
| **Recent Activity** | "No recent activity yet. Activity will appear here as friends connect and share files." | No | — | This is a passive informational display in the main panel. There is no meaningful single action (no "Create activity" button). |
| **Friends Online** | "No friends are online right now." | No | — | This is a live status indicator, not an interactive section. The status card's tone already signals "unavailable". No action makes sense here. |

### 2.1 Action button spec (when used)

If an action is added to a sidebar empty state:

| Property | Value |
|----------|-------|
| Style | Ghost/text button (`BUTTON_GHOST` closure at `app.rs:477-494`) |
| Text colour | `text_muted` default, `accent_primary` on hover |
| Font size | `TYPO_XS` (11px) |
| Padding | `[SPACE_4, SPACE_8]` |
| Placement | Inline below the empty-state text, separated by `SPACE_4` |

---

## 3. Shared Visual Rules

All empty states, regardless of section, follow these rules:

### 3.1 Typography

| Property | Value | Token ref |
|----------|-------|-----------|
| Font size | 11px | `TYPO_XS` (`app.rs:186`) |
| Font weight | Regular | — |
| Colour (light) | `#666` | `text_secondary` / `text_muted` |
| Colour (dark) | `#999` | `text_secondary` / `text_muted` |
| Line height | Natural (inherited from font) | — |

Implementation reference: `Self::muted_color(dark_mode)` at `app.rs:11590-11594`.

### 3.2 Spacing

| Context | Padding | Notes |
|---------|---------|-------|
| Sidebar sections (Chats, Friends, Discover, Requests) | `padding([SPACE_4, SPACE_12])` | Vertically 4px, horizontally 12px — matches existing pattern at e.g. `app.rs:12627`. |
| Main panel sections (Recent Activity) | `padding([SPACE_4, 0.0])` | Horizontal padding is inherited from the parent card. |
| Friends Online (status card) | N/A — primary text of a `StatusCard` | Text is the card's `primary` field. Inherits the card's internal layout. |

### 3.3 Alignment

| Context | Alignment |
|---------|-----------|
| Sidebar | Left-aligned within the section `Column`. The parent `Column` uses `spacing(SPACE_2)` between rows. |
| Main panel | Left-aligned within the activity card's `Column`. |

### 3.4 Implementation pattern (Rust / iced)

Every sidebar empty state follows this exact pattern:

```rust
if dep.is_empty {
    section = section.push(
        container(
            text("‹approved copy from §2›")
                .size(TYPO_XS)
                .color(Self::muted_color(dep.dark_mode)),
        )
        .padding([SPACE_4, SPACE_12]),
    );
}
```

Every main-panel empty state follows this pattern:

```rust
if self.recent_activity.is_empty() {
    vec![container(
        text("‹approved copy from §2›")
            .size(TYPO_XS)
            .color(text_muted(&theme)),
    )
    .padding([SPACE_4, 0.0])
    .into()]
}
```

No card background (`bg_surface`), no border (`border_muted`), no outline. The empty state is a plain text node inside the section's column flow.

---

## 4. Reusable Empty-State Component Proposal

A helper function should be introduced to eliminate the five nearly-identical inline patterns across Chats, Friends, Discover, and Requests:

```rust
/// Render a standard empty-state text line for sidebar sections.
/// - `text_muted`: primary or muted colour function from the iced theme.
fn sidebar_empty_state<'a, T: 'a>(
    text_muted: Color,
    message: &'a str,
) -> iced::Element<'a, T> {
    use iced::widget::{container, text};

    container(text(message).size(TYPO_XS).color(text_muted))
        .padding([SPACE_4, SPACE_12])
        .into()
}
```

For the main panel (Recent Activity), a parallel helper:

```rust
/// Render a standard empty-state text line for the main panel.
fn panel_empty_state<'a, T: 'a>(
    theme: &iced::Theme,
    message: &'a str,
) -> iced::Element<'a, T> {
    use iced::widget::{container, text};

    container(text(message).size(TYPO_XS).color(text_muted(theme)))
        .padding([SPACE_4, 0.0])
        .into()
}
```

If an action is needed, compose the element inline:

```rust
Column::new()
    .push(container(
        text("Empty text…").size(TYPO_XS).color(muted),
    ).padding([SPACE_4, SPACE_12]))
    .push(
        button(text("Action Label").size(TYPO_XS))
            .on_press(AppMessage::SomeAction)
            .padding([SPACE_4, SPACE_8])
            .style(BUTTON_GHOST),
    )
```

### 4.1 Placement in codebase

Add the helper functions at file scope in `app.rs`, near the existing `muted_color` helper (currently around line 11589), so all empty-state call-sites in the same file can reference them. No separate module or file is needed at this scope.

---

## 5. Accessibility Requirements

| Requirement | Implementation guidance |
|-------------|------------------------|
| **Readable text contrast** | `text_muted` colour must meet WCAG AA for small text (≥ 4.5:1). Light theme `#666` on `#f0f0f6` (bg_primary) = ~5.2:1 ✓. Dark theme `#999` on `#1a1a2e` (bg_primary) = ~4.5:1 ✓. Sidebar surfaces use `bg_surface` (`#ffffff` light / `#2a2a3e` dark): `#666` on white = 5.2:1 ✓; `#999` on `#2a2a3e` = 4.6:1 ✓. |
| **Accessible action labels** | Any action button must have an explicit text label (e.g. "Add Friend"). Do not use icon-only buttons for empty-state actions. The button text itself is the accessible name — no aria-label needed in iced. |
| **Keyboard navigation** | If an action button is present, it must be reachable via Tab key and activatable via Enter/Space. iced's built-in `button` widget satisfies this. |
| **Screen reader text** | Empty-state text is plain `text()` in iced. No special markup needed. Ensure the text string is a complete grammatical sentence so it is read naturally. |
| **Focus indicator** | If the action button is focused (Tab), a visible focus ring must appear. The theme's `accent_primary` focus ring applied by iced's default button style is sufficient. |

---

## 6. Anti-Pattern Checklist

When implementing empty states, verify against this list:

- [ ] **No technical jargon** ("mesh", "relay", "DHT", "peer ID", "key" used alone, "subscription", "discovery service")
- [ ] **No card wrapping** — each empty state sits inside a plain `container` with no `bg_surface` background or `border_muted` border
- [ ] **No large illustrations** — no emoji art, no SVG, no decorative ASCII
- [ ] **No multiple actions** — at most one inline ghost button per empty state, preferably zero
- [ ] **No duplicate actions** — if the action exists elsewhere in the same section (e.g. "Manage Requests →" in Requests, action cards in the landing screen), omit it from the empty state
- [ ] **No bounce/elastic easing** if an animation is introduced — use a subtle fade or no animation
- [ ] **No Inter font** — system font stack only (as specified in DESIGN_SYSTEM.md §1)

---

## 7. Verification Checklist

Before shipping, confirm:

- [ ] Every section (Chats, Friends, Discover, Requests, Recent Activity, Friends Online) has an explicit empty state
- [ ] Every empty state uses `TYPO_XS` + `muted_color` (or `text_muted`)
- [ ] No section uses technical terminology in its empty-state text
- [ ] No section uses a card/illustration wrapper around the empty-state text
- [ ] If an action was added to a sidebar empty state, it uses the ghost button style and `TYPO_XS`
- [ ] All six strings match the approved copy in §2 exactly
- [ ] `cargo build --features gui --example iced_chat` compiles (empty states do not introduce errors)
- [ ] `git diff --check` passes (no trailing whitespace or merge conflicts)
