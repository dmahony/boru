# Boru — Design System

> **Version:** 1.1 (final)
> **Created:** 2026-07-21
> **Updated:** 2026-07-23
> **Scope:** `examples/iced_chat/` — the `iced` desktop GUI for Boru
> **Audience:** Developers maintaining or extending the Boru GUI

This document specifies every visual token, component, and behaviour in the Boru UI. All values reference the **current codebase** — this is a living document describing the implementation as it stands after the UI redesign (Steps 2–23). Token names, line numbers, and dimensions are verified against the source.

---

## 1. Typography

### Font Family

System font stack — no custom fonts loaded. The stack falls through to the platform default UI font:

```css
/* Conceptual */
font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans",
             Helvetica, Arial, sans-serif;
```

Iced's built-in theme engine uses this stack already; no change needed.

### Type Scale

The current scale uses a minor-second ratio (~1.125) with six steps. These **should remain** — they're used consistently across the app.

| Token    | px  | Current constant | Usage                                                           | File:Line      |
|----------|-----|------------------|-----------------------------------------------------------------|----------------|
| TYPO_XXS | 10  | `TYPO_XXS: f32 = 10.0`  | Fine print, ticket text, instruction text, timestamp labels, size labels | `app.rs:187`   |
| TYPO_XS  | 11  | `TYPO_XS: f32 = 11.0`   | Metadata, identity info, secondary labels, section headers, error detail | `app.rs:186`   |
| TYPO_SM  | 13  | `TYPO_SM: f32 = 13.0`   | Secondary body, preview text, entry labels, button labels (default) | `app.rs:185`   |
| TYPO_MD  | 15  | `TYPO_MD: f32 = 15.0`   | Body text, section headers, primary button labels, settings section titles | `app.rs:184`   |
| TYPO_LG  | 18  | `TYPO_LG: f32 = 18.0`   | Secondary heading (room name, help title), sidebar app name           | `app.rs:183`   |
| TYPO_XL  | 24  | `TYPO_XL: f32 = 24.0`   | Primary heading (settings page title)                                 | `app.rs:182`   |

**Conventions:**
- Button labels default to `TYPO_SM` (13px); primary CTA buttons use `TYPO_MD` (15px).
- Chat message body size is user-configurable (`chat_text_size`, defaults to `TYPO_SM`).
- Section headers in the sidebar use `TYPO_XS` (11px) in muted colour.

### Text Styles

| Usage                | Size      | Colour token          | Example location           |
|----------------------|-----------|-----------------------|----------------------------|
| Chat body (configurable)| variable | `text_local_body` / `text_remote_body` | `app.rs:11286-11290` |
| Chat label (sender)  | TYPO_XS   | `text_local_label` / `text_remote_label` | `app.rs:11270-11284` |
| System messages      | TYPO_SM   | `text_system`         | `app.rs:11240-11251` |
| Timestamp            | TYPO_XXS  | `text_muted`          | `app.rs:11309-11310` |
| Sidebar section title| TYPO_XS   | `text_muted`          | `app.rs:10320`       |
| Sidebar conversation name | TYPO_SM | `text_remote_body` / `Color::WHITE` | `app.rs:10458-10466` |
| Sidebar preview      | TYPO_XS   | `text_muted`          | `app.rs:10475-10479` |
| Sidebar timestamp    | TYPO_XXS  | `text_muted`          | `app.rs:10468-10470` |
| Settings section title | TYPO_MD | — (inherits body)     | `app.rs:11911`       |
| Settings page heading | TYPO_XL  | — (inherits body)     | `app.rs:11768`       |

### Semantic Role Map

The following table maps type-size tokens to their intended semantic role. This ensures consistent choice of size for each kind of content across the UI.

| Role               | Token    | px   | Font weight | Notes                            |
|--------------------|----------|------|-------------|----------------------------------|
| Application/page title | TYPO_XL | 24   | Bold        | Settings title, landing heading  |
| Section heading    | TYPO_LG  | 18   | Medium      | Sidebar section labels, help title, room name |
| Card heading       | TYPO_MD  | 15   | Medium      | Settings card section title      |
| Primary body text  | TYPO_SM  | 13   | Regular     | Chat body (configurable), button labels (default) |
| Secondary text     | TYPO_XS  | 11   | Regular     | Metadata, identity info, section header labels |
| Captions           | TYPO_XXS | 10   | Regular     | Fine print, file size, speed labels |
| Badges             | TYPO_XXS | 10   | Medium      | Unread count pill                 |
| Button labels      | TYPO_SM  | 13   | Medium      | Primary CTAs use TYPO_MD         |
| Secondary labels   | TYPO_XS  | 11   | Regular     | Peer labels, timestamps          |

### Line Height

Iced's `text` widget uses the font's natural line height. No explicit line-height tokens are needed — the platform default is acceptable.

### Icon Sizes

| Icon               | Link/button characters | Equivalent px | Location             |
|--------------------|------------------------|---------------|----------------------|
| Sidebar "+" (add room) | `"＋"` @ `TYPO_MD`  | 15px          | `app.rs:10172`       |
| Sidebar "⚙" (settings)| `"⚙"` @ `TYPO_MD`   | 15px          | `app.rs:10185`       |
| Help button "?"    | `"?"` @ `TYPO_XS`     | 11px          | `app.rs:11453`       |
| Accept "✓"         | `"✓"` @ `TYPO_XS`     | 11px          | `app.rs:10918`       |
| Decline "✗"        | `"✗"` @ `TYPO_XS`     | 11px          | `app.rs:10935`       |

---

## 2. Spacing

### Grid Unit

Base unit: **4px**. All spacing values are multiples or fractions of this base.

| Token    | px | Constant declaration | File:Line |
|----------|----|----------------------|-----------|
| SPACE_2  | 2  | `SPACE_2: f32 = 2.0`  | `app.rs:229` |
| SPACE_4  | 4  | `SPACE_4: f32 = 4.0`  | `app.rs:230` |
| SPACE_6  | 6  | `SPACE_6: f32 = 6.0`  | `app.rs:231` |
| SPACE_8  | 8  | `SPACE_8: f32 = 8.0`  | `app.rs:232` |
| SPACE_10 | 10 | `SPACE_10: f32 = 10.0` | `app.rs:233` |
| SPACE_12 | 12 | `SPACE_12: f32 = 12.0` | `app.rs:234` |
| SPACE_16 | 16 | `SPACE_16: f32 = 16.0` | `app.rs:235` |
| SPACE_24 | 24 | `SPACE_24: f32 = 24.0` | `app.rs:236` |
| SPACE_32 | 32 | `SPACE_32: f32 = 32.0` | (proposed addition — large section gap) |

### Margins & Spacing by Section

| Context                           | Padding / Gap values                | File:Line(s)                   |
|-----------------------------------|-------------------------------------|--------------------------------|
| Sidebar header (Boru title)  | `top:12 right:12 bottom:4 left:12`  | `app.rs:10225-10230`           |
| Sidebar rows (conversation, friend)| `[6, 12]` (vertical, horizontal)   | `app.rs:10481`                 |
| Section headers                   | `top:8 right:12 bottom:4 left:12`   | `app.rs:10321-10327`           |
| Chat panel padding                | `SPACE_16` all around               | `app.rs:11024`                 |
| Chat bubble padding               | `[4, 8]` (vertical, horizontal)     | `app.rs:11294`                 |
| Date separator                    | `[8, 0]` (vertical)                 | `app.rs:11233`                 |
| Composer container padding        | implicit from `row![]` + `container` | `app.rs:11530-11569`           |
| Composer spacing (input ↔ actions) | `SPACE_6`                          | `app.rs:11545`                 |
| Action button group spacing       | `SPACE_2`                          | `app.rs:11532`                 |
| Settings card padding             | `SPACE_24` side + `SPACE_16` vertical| `app.rs:11853`                 |
| Download card padding             | `[12, 16]` (vertical, horizontal)   | `download_progress_view.rs:332`|
| Section card item spacing         | `SPACE_8` between rows              | `app.rs:11833`                 |
| Download rows spacing             | `SPACE_6` internal                  | `download_progress_view.rs:300,327` |
| Friends/peers row spacing         | `SPACE_4` between items             | `app.rs:10797`                 |
| Chat log entries spacing          | `SPACE_4` between rows              | `app.rs:11202`                 |
| Chat panel (header + log + composer)| `SPACE_8` between sections        | `app.rs:11021`                 |
| Chat header sub-row spacing       | `SPACE_4`                          | `app.rs:11126`                 |

---

## 3. Colour Palettes

### 3.1 Light Theme

**Backgrounds**

| Token         | Hex     | RGB              | Usage                       | File:Line |
|---------------|---------|------------------|-----------------------------|-----------|
| bg_primary    | #f0f0f6 | 0.94, 0.94, 0.96 | Main panel background       | `app.rs:353` |
| bg_surface    | #ffffff | 1.0, 1.0, 1.0    | Sidebar, cards, surfaces    | `app.rs:362` |
| bg_input      | #f0f0f4 | 0.94, 0.94, 0.96 | Input field background      | `app.rs:372` |
| bg_hover      | #e6e6f2 | 0.90, 0.90, 0.95 | Row hover state             | `app.rs:381` |
| border_muted  | #d9d9e0 | 0.85, 0.85, 0.88 | Card borders, dividers      | `app.rs:391` |

**Text**

| Token              | Hex     | RGB              | Contrast  | Usage                       | File:Line |
|--------------------|---------|------------------|-----------|-----------------------------|-----------|
| text_primary       | #222    | 0.13, 0.13, 0.13| ≥ 11.5:1   | Body text (remote messages) | `app.rs:328` |
| text_secondary     | #666    | 0.40, 0.40, 0.40| ≥ 5.2:1    | Muted, secondary labels     | `app.rs:283` |
| text_system        | #595959 | 0.35, 0.35, 0.35| ≥ 6.5:1    | System messages, help text  | `app.rs:292` |
| text_local_label   | #007300 | 0.0, 0.45, 0.0 | ≥ 5.8:1    | "You" label in chat         | `app.rs:301` |
| text_local_body    | #005900 | 0.0, 0.35, 0.0 | ≥ 6.5:1    | Self-sent message body      | `app.rs:310` |
| text_remote_label  | #0054A8 | 0.0, 0.33, 0.66| ≥ 5.5:1    | Peer name label             | `app.rs:319` |
| text_remote_body   | #222    | 0.13, 0.13, 0.13| ≥ 11.5:1   | Received message body       | `app.rs:328` |

**Accents**

| Token           | Hex     | RGB              | Usage                               | File:Line |
|-----------------|---------|------------------|-------------------------------------|-----------|
| accent_primary  | #2e70cc | 0.18, 0.44, 0.80 | Buttons, links, selection, focus    | `app.rs:399` |
| accent_green    | #1a8c33 | 0.10, 0.55, 0.20 | Success, online indicator           | `app.rs:408` |
| color_error     | #bf2626 | 0.75, 0.15, 0.15 | Error, destructive actions          | `app.rs:417` |

**Chat bubbles**

| Kind   | Background RGBA          | Effect                         | File:Line |
|--------|--------------------------|--------------------------------|-----------|
| Local  | (0.0, 0.5, 0.0, 0.06)   | Very faint green tint          | `app.rs:340` |
| Remote | (0.1, 0.2, 0.5, 0.05)   | Very faint blue tint           | `app.rs:341` |
| System | None (transparent)       | No bubble — centered text      | `app.rs:334-335` |

### 3.2 Dark Theme

**Backgrounds**

| Token         | Hex     | RGB              | Usage                       | File:Line |
|---------------|---------|------------------|-----------------------------|-----------|
| bg_primary    | #1a1a2e | 0.10, 0.10, 0.18 | Main panel background       | `app.rs:351` |
| bg_surface    | #2a2a3e | 0.16, 0.16, 0.24 | Sidebar, cards, surfaces    | `app.rs:359` |
| bg_input      | #222238 | 0.13, 0.13, 0.22 | Input field background      | `app.rs:370` |
| bg_hover      | #33334d | 0.20, 0.20, 0.30 | Row hover state             | `app.rs:379` |
| border_muted  | #383852 | 0.22, 0.22, 0.32 | Card borders, dividers      | `app.rs:389` |

**Text**

| Token              | Hex     | RGB              | Contrast  | Usage                       | File:Line |
|--------------------|---------|------------------|-----------|-----------------------------|-----------|
| text_primary       | #ccc    | 0.80, 0.80, 0.80| ≥ 5.5:1    | Body text (remote messages) | `app.rs:326` |
| text_secondary     | #999    | 0.60, 0.60, 0.60| ≥ 4.5:1    | Muted, secondary labels     | `app.rs:281` |
| text_system        | #999    | 0.60, 0.60, 0.60| ≥ 4.5:1    | System messages, help text  | `app.rs:289` |
| text_local_label   | #33cc33 | 0.20, 0.80, 0.20| vivid green | "You" label in chat        | `app.rs:299` |
| text_local_body    | #4de64d | 0.30, 0.90, 0.30| bright green| Self-sent message body      | `app.rs:308` |
| text_remote_label  | #66a6ff | 0.40, 0.65, 1.0 | light blue  | Peer name label             | `app.rs:317` |
| text_remote_body   | #ccc    | 0.80, 0.80, 0.80| ≥ 5.5:1    | Received message body       | `app.rs:326` |

**Accents**

| Token           | Hex     | RGB              | Usage                               | File:Line |
|-----------------|---------|------------------|-------------------------------------|-----------|
| accent_primary  | #4a9eff | 0.29, 0.62, 1.0  | Buttons, links, selection, focus    | `app.rs:397` |
| accent_green    | #3ddc84 | 0.24, 0.86, 0.52 | Success, online indicator           | `app.rs:406` |
| color_error     | #e64040 | 0.90, 0.25, 0.25 | Error, destructive actions          | `app.rs:415` |

**Chat bubbles**

| Kind   | Background RGBA            | Effect                         | File:Line |
|--------|----------------------------|--------------------------------|-----------|
| Local  | (0.15, 0.30, 0.15, 0.40)  | Semi-transparent green tint    | `app.rs:338` |
| Remote | (0.20, 0.20, 0.25, 0.40)  | Semi-transparent grey-blue     | `app.rs:339` |
| System | None (transparent)         | No bubble — centered text      | `app.rs:334-335` |

### 3.3 Semantic Status Colours

| Status   | Light hex | Dark hex  | Used for                                    |
|----------|-----------|-----------|---------------------------------------------|
| Online   | #1a8c33   | #3ddc84   | Green dot ("●") next to peer name           |
| Idle     | #c4a000   | #e6b800   | Amber (currently not explicitly present — recommended addition) |
| Offline  | #999      | #666      | Grey dot ("○") next to peer name            |
| Error    | #bf2626   | #e64040   | Destructive buttons, error messages         |
| Success  | #1a8c33   | #3ddc84   | Confirmation, completed states              |
| Link     | #2e70cc   | #4a9eff   | Peer profile links, clickable labels        |

### 3.4 Notification / Unread Indicators

| Element              | Light                     | Dark                      | File:Line |
|----------------------|---------------------------|---------------------------|-----------|
| Unread count badge   | Inline `" [N]"` appended  | Same pattern              | `app.rs:10434-10436` |
| Selected row bg      | `accent_primary` (blue)   | `accent_primary` (blue)   | `app.rs:10489-10490` |
| Selected row text    | `Color::WHITE`            | `Color::WHITE`            | `app.rs:10463` |

> **Note:** Unread badges currently render as inline text (`" [N]"`) rather than a pill/badge element. The redesign should introduce a dedicated unread badge pill.

### 3.5 Shared Semantic Token Reference

Tokens shared across both themes that are not tied to a specific component:

| Token               | Light Hex | Dark Hex  | Light contrast | Usage                                         |
|---------------------|-----------|-----------|----------------|-----------------------------------------------|
| Elevated surface    | #ffffff   | #2f2f45   | —              | Dropdowns, context menus, tooltips            |
| Selected surface    | #d6e4f8   | #2a3a5a   | —              | Selected row background (non-primary accent)  |
| Warning             | #b8860b   | #e6c200   | ≥ 3:1 (large)  | Warning states, caution borders, transient errors |
| Unread badge bg    | #2e70cc   | #4a9eff   | —              | Unread count pill background                  |
| Keyboard focus      | #4a9eff   | #66b3ff   | —              | Focus ring / keyboard navigation indicator    |

> **Usage guidance:** `Elevated surface` sits above `bg_surface` in the z/hierarchy stack — use it for popovers, context menus, and dropdown panels that float above the main content. `Keyboard focus` is applied as a 2px outline ring on interactive elements during keyboard navigation (Tab/Shift+Tab), not as a hover adornment.

---

## 4. Components

### 4.1 Buttons

#### Primary Button

| Property     | Value                          | File:Line (example) |
|-------------|--------------------------------|---------------------|
| Background  | `accent_primary` (filled)       | `app.rs:11930-11938` |
| Text colour | `Color::WHITE`                  | |
| Border radius| `SPACE_6` (6px)                | |
| Padding     | `[6, 12]` or `[8, 16]`        | `app.rs:11992, 12073` |
| Font size   | `TYPO_SM` or `TYPO_MD`         | |
| Hover state | Theme's default hover (slightly lighter) | |
| Press state | 85% of accent colour           | |

#### Ghost / Text Button

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Background  | None (transparent)             | `app.rs:477-494` |
| Text colour | `#808080` / mute, `accent_primary` on hover | |
| Border      | None                           | |
| Padding     | `[4, 8]` (small) or `[4, 6]`  | |
| Font size   | `TYPO_XS` or `TYPO_SM`        | |

**Implementation:** `BUTTON_GHOST` closure at `app.rs:477-494`. Used for sidebar buttons, "•", "⚙", Chat/Browse Files/Remove buttons in sidebars, and the help/attach buttons in the composer.

#### Outline Button

| Property     | Value                          | File:Line (example) |
|-------------|--------------------------------|---------------------|
| Background  | None                           | `download_progress_view.rs:110-133` |
| Text colour | `accent_primary` on hover; `#808080` default | |
| Border      | `border_muted` 1px, `SPACE_6` radius | |
| Padding     | `[4, 10]`                      | |

Used by download progress action buttons (Download, Pause, Resume, Retry, Open).

#### Danger / Destructive Button

| Property     | Value                          | File:Line (example) |
|-------------|--------------------------------|---------------------|
| Background  | `color_error` (red tint)       | `app.rs:10941-10951` |
| Text colour | `Color::WHITE`                 | |
| Border radius| `SPACE_4` (4px)               | |
| Padding     | `[2, 4]` to `[6, 12]`        | |

Used for "Remove Friend", "Decline Request", "Remove Shared File".

#### Text-only Muted Button

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Background  | None                           | `download_progress_view.rs:136-160` |
| Text colour | `#737373` default, `color_error` on hover | |
| Border      | None                           | |
| Padding     | `[4, 8]`                       | |

Used for "Cancel", "Remove" actions in download cards.

#### Icon-only Buttons (Sidebar)

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Size / padding | `[SPACE_4]` (4px all sides) | `app.rs:10174, 10187` |
| Font size   | `TYPO_MD` (15px)               | |
| Text colour | `text_muted`, `accent_primary` on hover | `app.rs:10176-10182` |
| Background  | None                           | |

### 4.2 Cards / Containers

#### Surface Card

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Background  | `bg_surface`                   | `app.rs:462-472` |
| Border      | `border_muted` 1px             | |
| Border radius| `SPACE_8` (8px)               | |
| Shadow      | None (flat iced card)          | |
| Padding     | Varies by context (see spacing table) | |

Implementation: `container_card` at `app.rs:462-472`.

#### Download Card

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Background  | `bg_surface`                   | `download_progress_view.rs:330-341` |
| Border      | State-coloured 1px (tone)      | |
| Border radius| `SPACE_10` (10px)             | |
| Padding     | `[12, 16]` vertical/horizontal | |

#### Settings Section Card

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Background  | `bg_surface`                   | `app.rs` (inline) |
| Border      | `border_muted` 1px             | |
| Border radius| `SPACE_8` (8px)               | |
| Padding     | `[16, 20]` vertical/horizontal | |

#### Help/Dialog Surface

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Background  | `bg_surface`                   | `app.rs:11051-11062` |
| Border radius| `SPACE_12` (12px)            | |
| Shadow      | `#0000004D` (30% alpha), offset (0,4), blur 24px | |

### 4.3 Friend List Items

| Element                | Dimension / Style               | File:Line |
|------------------------|---------------------------------|-----------|
| Avatar (fallback)      | 24×24px, circular (12px radius) | `app.rs:10642-10643` |
| Avatar (image)         | 24×24px                         | `app.rs:10620-10623` |
| Online status          | "●" (green) / "○" (grey) char  | `app.rs:10764` |
| Name text              | `TYPO_SM`, `text_remote_body`   | `app.rs:10768-10775` |
| Action buttons         | `TYPO_XS`, ghost style          | `app.rs:10778-10795` |
| Row padding            | `[4, 12]`                       | `app.rs:10799` |
| Row spacing            | `SPACE_4` between items         | `app.rs:10797` |

### 4.4 Chat List Items

| Element                | Dimension / Style               | File:Line |
|------------------------|---------------------------------|-----------|
| Status dot             | "●" or "○" char, inline        | `app.rs:10433` |
| Name + unread          | `TYPO_SM`, selected = `WHITE`, else `text_remote_body` | `app.rs:10458-10467` |
| Unread badge           | `" [N]"` appended to name       | `app.rs:10435` |
| Preview                | `TYPO_XS`, `text_muted`         | `app.rs:10475-10479` |
| Timestamp              | `TYPO_XXS`, `text_muted`        | `app.rs:10468-10470` |
| Row padding            | `[6, 12]`                       | `app.rs:10481` |
| Selected background    | `accent_primary` (blue fill)    | `app.rs:10487-10493` |
| Hover background       | None (dormant feature — button has no hover state) | |
| Border radius on select| `SPACE_4` (4px)                 | `app.rs:10496` |

### 4.5 Notification Badges

**Current state:** Unread counts are shown inline as `" [N]"` in the conversation name text. No dedicated badge element exists. This is a known limitation — see "Remaining Planned UI Work" below.

**Planned for future implementation:**

| Property     | Value proposal                 |
|-------------|--------------------------------|
| Shape       | Rectangular pill with rounded corners |
| Padding     | `[1, 6]` vertical/horizontal    |
| Min width   | 20px                            |
| Background  | `accent_primary` (blue)         |
| Text colour | `Color::WHITE`                  |
| Font size   | `TYPO_XXS` (10px)               |
| Position    | Right of the conversation name, vertically centred |
| Multiple    | Capped at "99+" display         |

### 4.6 Status Indicators

**Current state:** Unicode characters for online/offline, inline with text.

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Online       | "●" (filled circle), green    | `app.rs:10433` |
| Offline      | "○" (hollow circle), grey     | `app.rs:10433` |
| Idle         | Not yet implemented — planned: "◐" amber | |
| Font size    | Inline with name text (inherits `TYPO_SM`) | |
| Colour - Online (light)| `#1a8c33` -> `accent_green` | `app.rs:408` |
| Colour - Online (dark) | `#3ddc84` -> `accent_green` | `app.rs:406` |
| Colour - Offline (lt)  | Same as `text_muted` (`#666`)  | `app.rs:10470` |
| Colour - Offline (dk)  | Same as `text_muted` (`#999`)  | `app.rs:10470` |

**Planned replacement:** Replace Unicode characters with a proper circle widget for better visual consistency. A solid circle of a fixed diameter (8px) with appropriate margin.

### 4.7 Avatars

| Property         | Value                        | File:Line |
|-----------------|------------------------------|-----------|
| Sidebar size    | 24×24px                      | `app.rs:10620-10623` |
| Chat bubble size| 48×48px                      | `app.rs:11325-11327` |
| Fallback        | First char of `fmt_short()`  | `app.rs:10633` |
| Fallback radius | 12px (fully circular)        | `app.rs:10647` |
| Fallback bg     | Derived from peer key bytes  | `app.rs:10627-10630` |
| Fallback text   | `TYPO_XS`, `Color::WHITE`    | `app.rs:10637-10638` |
| Image fit       | `ScaleDown` for chat, raw for sidebar | `app.rs:11323-11324` |

### 4.8 Context Menus

**Current state:** No context menus exist in the codebase. All interactions use explicit buttons. The friend profile "⋮" (three-dot) menu is a manually positioned overlay inside the profile view, not a reusable context-menu component.

**Planned for future implementation:**

| Property     | Value proposal                 |
|-------------|--------------------------------|
| Trigger      | Right-click or long-press on chat items, friend rows |
| Position     | Below and right of the click point |
| Min width    | 160px                           |
| Max width    | 240px                           |
| Item height  | 32px                            |
| Item padding | `[6, 12]`                       |
| Background   | `bg_surface`                    |
| Border       | `border_muted` 1px              |
| Border radius| `SPACE_8` (8px)                 |
| Shadow       | Same as help panel              |
| Item hover   | `bg_hover`                      |
| Separator    | 1px `border_muted` line         |
| Font size    | `TYPO_SM`                       |

### 4.9 Modal / Dialog Windows

#### Create Room Dialog

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Width        | 320px fixed                    | `app.rs:10136` |
| Height       | `Shrink` (content-dependent)   | `app.rs:10137` |
| Padding      | 24px                           | `app.rs:10138` |
| Background   | `rgba(0.15, 0.15, 0.15, 0.95)` (#262626F2) | `app.rs:10140-10142` |
| Border radius| 12px                           | `app.rs:10144` |
| Backdrop     | Full-screen, semi-transparent fill via `stack![]` | `app.rs:10151-10157` |

#### Help Panel (Overlay)

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Max width    | 480px                          | `app.rs:11049` |
| Max height   | 600px                          | `app.rs:11050` |
| Background   | `bg_surface`                   | `app.rs:11052` |
| Border radius| `SPACE_12` (12px)              | `app.rs:11054` |
| Shadow       | `#0000004D` (30% alpha), offset (0,4), blur 24px | `app.rs:11057-11061` |
| Backdrop     | Full-screen, dark/light tint: `rgba(0,0,0,0.55)` / `rgba(0,0,0,0.35)` | `app.rs:11039-11041` |

#### Image Preview

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Image fit   | `Contain` (preserve aspect ratio within panel) | `app.rs:11589` |
| Image width | `FillPortion(1)`               | `app.rs:11590` |
| Image height| `FillPortion(1)`               | `app.rs:11591` |
| Back button | "← Back", `TYPO_MD`, padding `[6, 12]` | `app.rs:11580-11583` |

---

## 5. Layout Structure

```
┌─────────────────────────────────────────────────────┐
│  ┌────────── 280px ──────────┬───────── Fill ──────┐ │
│  │  Sidebar (bg_surface)     │  Main Panel         │ │
│  │                           │  (bg_primary)       │ │
│  │  ┌─ Header ─────────────┐ │  ┌─ Chat Panel ──┐ │ │
│  │  │ Boru    ＋  ⚙  │ │  │ Header        │ │ │
│  │  └──────────────────────┘ │  │               │ │ │
│  │  ┌─ Identity Row ───────┐ │  │ Chat Log      │ │ │
│  │  │ Label | Relay mode   │ │  │ (scrollable)  │ │ │
│  │  └──────────────────────┘ │  │               │ │ │
│  │  ┌──────────────────────┐ │  ├───────────────┤ │ │
│  │  │ Join by ticket       │ │  │ Composer      │ │ │
│  │  │ [input] [Join]       │ │  │ [input] ⋯     │ │ │
│  │  └──────────────────────┘ │  └───────────────┘ │ │
│  │  ┌─ Chats ─────────────┐ │  └───────────────────┘ │
│  │  │ Conversations       │ │                        │
│  │  │ ● Name        12:34 │ │                        │
│  │  │   Preview text…     │ │                        │
│  │  └──────────────────────┘ │                        │
│  │  ┌─ Online Peers ──────┐ │                        │
│  │  │ ● abc123  [Chat]   │ │                        │
│  │  │ ○ def456  [Chat]   │ │                        │
│  │  └──────────────────────┘ │                        │
│  │  ┌─ Friends ───────────┐ │                        │
│  │  │ [Add friend by key] │ │                        │
│  │  │ ● Alice [Chat] [..]│ │                        │
│  │  └──────────────────────┘ │                        │
│  │  ┌─ Friend Requests ───┐ │                        │
│  │  │ Reqs (2)  [Manage]  │ │                        │
│  │  │ Bob  [✓][✗]        │ │                        │
│  │  └──────────────────────┘ │                        │
│  └───────────────────────────┴────────────────────────┘
└─────────────────────────────────────────────────────┘
```

- **Sidebar fixed width:** 280px (`app.rs:10076`)
- **Settings content max width:** 520px (`app.rs:11887`)
- **Chat bubble max width:** 480px (`app.rs:11317`)
- **Window default size:** iced default — typically 1280×720 on modern desktops. The app scales to fill.

---

## 6. Overlays & Z-Index

| Layer               | Z context              | Notes                       |
|---------------------|------------------------|-----------------------------|
| Base content        | 0 (bottom)             | Sidebar + main panel        |
| Help/dialog backdrop| Stack above base       | Semi-transparent fill       |
| Help/dialog panel   | Stack above backdrop   | Centred panel               |
| Create Room dialog  | Stack above base       | Same pattern as help        |

Iced renders `stack![]` children in order, with later children on top. The backdrop is a full-size button that catches click-to-dismiss.

---

## 7. Border Radii

Radii are organised by the semantic surface they belong to, not by arbitrary px values. Each category maps to one or more `SPACE_N` tokens.

| Category           | px       | Token       | Usage                                              |
|--------------------|----------|-------------|----------------------------------------------------|
| Small controls     | 4px      | `SPACE_4`   | Sidebar selected row, small action buttons, toggle |
| List rows          | 4px      | `SPACE_4`   | Conversation rows, friend rows (selected state)   |
| Buttons (standard) | 6px      | `SPACE_6`   | Primary/secondary/outline buttons, state badge     |
| Cards / containers | 8px      | `SPACE_8`   | Surface cards, chat bubbles, composer container, settings cards |
| State pill / download card | 10px | `SPACE_10` | State badge pill, download progress card          |
| Dialogs / avatars  | 12px     | `SPACE_12`  | Modal dialogs, help overlay, avatar circles        |

> **Guideline:** Never round a surface more than 12px. Never set a corner radius to 0px for interactive elements — the minimum is 4px for the smallest controls.

---

## 8. Shadows

Used only in the help panel overlay currently:

| Property      | Value                   | File:Line |
|---------------|------------------------|-----------|
| Color         | `#0000004D` (30% black) | `app.rs:11058` |
| Offset        | (0, 4)                  | `app.rs:11059` |
| Blur radius   | 24px                    | `app.rs:11060` |

Proposed for dialogs and context menus in the redesign: same shadow values.

---

## 9. Interactive States

All interactive elements in the UI respond to the same seven states. This section defines the visual treatment for each state across the three main interactive patterns.

| # | State            | Description                                             |
|---|------------------|---------------------------------------------------------|
| 1 | **Normal**       | Default resting state, no interaction                   |
| 2 | **Hover**        | Pointer (mouse cursor) is over the element              |
| 3 | **Pressed**      | Pointer button is held down on the element              |
| 4 | **Selected**     | Element is the currently active / chosen item           |
| 5 | **Keyboard focused** | Element has keyboard focus (Tab/Shift+Tab)         |
| 6 | **Disabled**     | Element is visible but not interactive                  |
| 7 | **Error**        | Element contains or relates to a validation failure     |

### 9.1 Button States

| State             | Primary                                  | Ghost / Text                            | Danger                                  |
|-------------------|------------------------------------------|-----------------------------------------|-----------------------------------------|
| Normal            | Filled `accent_primary`, white text      | Muted text, transparent background      | Filled `color_error`, white text        |
| Hover             | ↑ brightness 15% (lighter)              | `accent_primary` text colour            | ↑ brightness 15%                        |
| Pressed           | ↓ brightness 15% (darker)               | ↓ brightness 15% of `accent_primary`    | ↓ brightness 15%                        |
| Keyboard focused  | 2px solid `keyboard focus` ring         | 2px solid `keyboard focus` ring         | 2px solid `keyboard focus` ring         |
| Selected          | N/A (momentary action)                  | N/A                                     | N/A                                     |
| Disabled          | `bg_surface` bg, 40% opacity text       | 40% opacity text, no hover change       | 40% opacity bg, muted text              |
| Error             | Same as Normal (form buttons) or Danger | Red-tinted text (`color_error`)         | Same as Danger                          |

> **Focus ring:** Apply via `container` with `Border { width: 2px, color: keyboard_focus, radius: <button_radius> }` on the outermost button wrapper. Only visible during keyboard navigation.

### 9.2 Row / Item States

| State             | Sidebar conversations                   | Sidebar friends / peers                 | Settings list rows                      |
|-------------------|------------------------------------------|-----------------------------------------|-----------------------------------------|
| Normal            | Transparent background, body text       | Transparent background, body text       | Transparent background                  |
| Hover             | `bg_hover` background (currently dormant — recommended) | `bg_hover` background                  | `bg_hover` background                   |
| Pressed           | `bg_hover` → brief flash                | `bg_hover` → brief flash                | `bg_hover` → brief flash                |
| Selected          | `accent_primary` fill, white text, `SPACE_4` radius | N/A (handled via chat button) | N/A (handled individually)              |
| Keyboard focused  | 2px `keyboard focus` inset ring         | 2px `keyboard focus` inset ring         | 2px `keyboard focus` ring               |
| Disabled          | 40% opacity text, no interaction         | 40% opacity text, no interaction        | 40% opacity text                        |
| Error             | Red left border (see error state section)| Red tinted text                          | Red text or border                      |

### 9.3 Text Input States

| State             | Composer input                          | Settings / join-ticket input            |
|-------------------|------------------------------------------|-----------------------------------------|
| Normal            | `bg_input` fill, `border_muted` border  | Iced default theme                      |
| Hover             | Slightly lighter background             | Iced default theme                      |
| Focused           | `accent_primary` border, 1px            | Iced default theme                      |
| Keyboard focused  | 2px `keyboard focus` outer ring         | 2px `keyboard focus` outer ring         |
| Disabled          | `bg_primary` fill, 40% text opacity     | Iced default greyed                     |
| Error             | `color_error` border, tinted bg         | `color_error` border                    |
| Pressed           | N/A (text input, not pressable)         | N/A                                     |
| Selected          | N/A (text selection is system-level)    | N/A                                     |

---

## 10. Existing Code References

### Colour Functions (`app.rs:277-419`)

| Function              | Line  | Returns                                  |
|-----------------------|-------|------------------------------------------|
| `text_muted`          | 279   | Dark: `#999`, Light: `#666`             |
| `text_system`         | 288   | Dark: `#999`, Light: `#595959`          |
| `text_local_label`    | 297   | Dark: bright green, Light: `#007300`    |
| `text_local_body`     | 306   | Dark: bright green, Light: `#005900`    |
| `text_remote_label`   | 315   | Dark: light blue, Light: `#0054A8`      |
| `text_remote_body`    | 324   | Dark: `#ccc`, Light: `#222`            |
| `bubble_bg`           | 333   | Returns `Option<Background>`            |
| `bg_primary`          | 349   | Dark: `#1a1a2e`, Light: `#f0f0f6`       |
| `bg_surface`          | 358   | Dark: `#2a2a3e`, Light: `#fff`          |
| `bg_input`            | 368   | Dark: `#222238`, Light: `#f0f0f4`       |
| `bg_hover`            | 377   | Dark: `#33334d`, Light: `#e6e6f2`       |
| `border_muted`        | 386   | Dark: `#383852`, Light: `#d9d9e0`       |
| `accent_primary`      | 395   | Dark: `#4a9eff`, Light: `#2e70cc`       |
| `accent_green`        | 404   | Dark: `#3ddc84`, Light: `#1a8c33`       |
| `color_error`         | 413   | Dark: `#e64040`, Light: `#bf2626`       |

### Container Styles (`app.rs:422-472`)

| Function              | Line  | Style                                     |
|-----------------------|-------|-------------------------------------------|
| `container_primary`   | 423   | `bg_primary` background                   |
| `container_surface`   | 431   | `bg_surface` background                   |
| `container_hover`     | 439   | `bg_hover` background                     |
| `container_card`      | 461   | `bg_surface` + `border_muted` 1px + `SPACE_8` radius |

### Button Styles (`app.rs:475-494`, `download_progress_view.rs:105-160`)

| Helper                      | Line(s)       | Usage                              |
|-----------------------------|---------------|------------------------------------|
| `BUTTON_GHOST`              | `app.rs:477`  | Ghost button (sidebar icons)       |
| `action_button`             | `dpv.rs:105`  | Outline button (downloads)         |
| `text_button`               | `dpv.rs:136`  | Text-only muted button             |

---

## 11. Design Token Status

### 11.1 Adopt a central token module

**Status:** *Not implemented.* Colours, spacing, and typography remain defined as `pub(crate)` constants and free functions in `app.rs`. Extracting them into a dedicated `theme.rs` module is still recommended for maintainability.

### 11.2 Missing tokens

| Token                    | Reason                                          | Status |
|--------------------------|-------------------------------------------------|--------|
| `TYPO_XXXS` (8px)       | For very dense data (download speeds, file sizes) | Not added |
| `SPACE_20` (20px)       | Gap between major sidebar sections              | Not added — sections use existing spacing tokens |
| Text `WARNING` (amber)  | For transient failure states (temporary)        | Not added |
| `ANIMATION_DURATION`    | Standard transition speed for hover/selection   | Not added — Iced has no animation API |

### 11.3 Standardise button API

**Status:** *Partially implemented.* Button styles are still defined inline in multiple places (`BUTTON_GHOST`, `BUTTON_ICON`, `BUTTON_PRIMARY`, `BUTTON_OUTLINE`, `BUTTON_GHOST_BG`, `DANGER_BUTTON`). A single `ButtonKind` enum with a unified `button_style` function would reduce duplication but has not been merged.

### 11.4 Replace unicode status dots with vector circles

**Status:** *Not implemented.* The `"●"` and `"○"` characters remain in use.

### 11.5 Introduce real unread badge pills

**Status:** *Not implemented.* Inline `" [N]"` text remains in use.

---

## 12. Implementation History

The UI redesign was completed in the following steps (see Kanban tasks for details):

1. **Step 2** — Extract theme tokens into `theme.rs`, add missing tokens *(NOT completed — tokens remain in app.rs)*
2. **Step 3** — Standardise button style helpers *(NOT completed — button helpers remain ad-hoc)*
3. **Step 4** — Implement notification badge pills and status dot widgets *(NOT completed)*
4. **Step 5** — Refine sidebar spacing and layout *(COMPLETED — collapsible sections, identity row, 280px fixed width)*
5. **Step 6** — Polish chat panel (bubbles, timestamps, avatar layout) *(COMPLETED)*
6. **Step 7** — Add context menus *(NOT completed)*
7. **Step 8** — Finalise modal/dialog consistency *(COMPLETED)*
8. **Step 9** — UX audit *(COMPLETED — see UX_AUDIT.md)*
9. **Step 10** — Landing screen redesign *(COMPLETED — status card, actions, recent activity)*
10. **Step 11** — Sidebar section collapsible headers *(COMPLETED)*
11. **Step 12** — Redesigned friend profile *(COMPLETED — rename inline, status, three-dot menu)*
12. **Step 13** — Redesigned recent activity *(COMPLETED — structured rows with relative time)*
13. **Step 14** — Friends-online panel *(COMPLETED — online status in CHATS and FRIENDS sections)*
14. **Step 15** — Dashboard file-drop area *(NOT implemented — Iced lacks native drag-and-drop)*
15. **Step 16** — Discovered peers section *(COMPLETED — Chat + Browse Files buttons per peer)*
16. **Step 17** — Settings screen refinements *(COMPLETED — identity, network, appearance sections)*
17. **Step 18** — Image preview screen *(COMPLETED)*
18. **Step 19** — Accessibility and keyboard support audit *(COMPLETED — focus rings, keyboard shortcuts)*
19. **Step 20** — Chats section in sidebar *(COMPLETED — online sort, unread counts, previews)*
20. **Step 21** — Requests section in sidebar *(COMPLETED — accept/decline, Manage button)*
21. **Step 22** — Friends section in sidebar *(COMPLETED — add by key input, alphabetically sorted)*
22. **Step 23** — Documentation update *(COMPLETED — this document and README)*

---

## 13. Accessibility Requirements

These requirements apply to every UI surface. No feature should ship without meeting these standards.

### 13.1 Colour and Contrast

| Requirement              | Target                     | How to verify                               |
|--------------------------|----------------------------|---------------------------------------------|
| Text contrast (normal)   | ≥ 4.5:1 against background | WCAG AA — use `color_contrast` tool        |
| Text contrast (large)    | ≥ 3:1 against background   | 18px+ bold or 24px+ regular                 |
| Non-text contrast        | ≥ 3:1 (borders, icons)     | Controls and visual indicators              |
| No colour-only status    | Every status has an icon, label, or text indicator in addition to colour | Review each status indicator for colour-only encoding |

> **Dark theme minimum:** All text tokens in the dark palette must achieve at least 4.5:1 against `bg_primary` (`#1a1a2e`) and `bg_surface` (`#2a2a3e`). Current dark theme `text_secondary` and `text_system` at `#999` achieve ~5.5:1 against `#2a2a3e` — this is AA-compliant but should be treated as the floor, not a comfortable margin.

### 13.2 Focus Indicators

| Requirement                       | Target                               |
|-----------------------------------|--------------------------------------|
| Visible focus ring                | 2px solid `keyboard_focus` outline   |
| Focus order                       | Matches visual reading order (LTR)   |
| No focus removal                  | Never suppress focus outlines without providing an equivalent visible indicator |
| Keyboard-operable                | Every interactive element reachable via Tab/Shift+Tab |
| Tab stops                         | Visible focus ring on every button, input, link, and interactive row |

### 13.3 Touch / Click Targets

| Requirement                       | Minimum size        | Notes                        |
|-----------------------------------|---------------------|------------------------------|
| Interactive element size          | 32×32 logical px    | Icon-only buttons            |
| Button / input height             | 32 logical px       | Standard controls            |
| Spacing between tappable targets  | 4px minimum         | Prevent fat-finger errors    |

> **Iced-specific note:** Icon-only buttons (e.g. `"⚙"`, `"＋"`, `"?"`) must have explicit `width` and `height` set to at least 32px, with the icon centred inside. Without explicit dimensions, iced collapses these to the text extent.

### 13.4 Typography and Scaling

| Requirement                       | Target                                     |
|-----------------------------------|--------------------------------------------|
| Minimum body text size            | 13px (`TYPO_SM`) for primary content      |
| Secondary text minimum            | 11px (`TYPO_XS`) — never smaller          |
| Caption / badge minimum           | 10px (`TYPO_XXS`) — never smaller         |
| Configurable text size            | Chat body size must be user-adjustable    |
| Display scaling                   | Respect OS-level UI scaling (iced handles this natively) |

> **Rule:** Never use `TYPO_XXS` (10px) for interactive labels or tappable content. Reserve it for passive read-only data (timestamps, file sizes, metadata).

### 13.5 Error States (Accessibility)

| Requirement                       | Target                                     |
|-----------------------------------|--------------------------------------------|
| Error identification              | Every error must have a text message       |
| Error colour redundancy           | Error text must include an icon or label prefix (e.g. `!"...`) in addition to red colour |
| Recovery guidance                 | Every error message must tell the user what to do next |
| Transient errors                  | Toast messages must persist long enough to be read (minimum 4 seconds) |

### 13.6 Screen Reader / Assistive Technology

| Requirement                       | Target                                     |
|-----------------------------------|--------------------------------------------|
| Semantic labels                   | Every icon-only button must have an accessible label (Iced: `.tooltip()` or `aria_label` equivalent) |
| Status announcements              | Online/offline transitions, message receipts, and errors should be reachable via assistive tech |
| Empty states                      | Empty lists must communicate "no items" with a text message, not just a blank area |

### 13.7 Colour Palette Compliance Notes

- **Light theme text OK:** `text_secondary` (`#666`, 5.2:1 on white), `text_system` (`#595959`, 6.5:1) — both pass WCAG AA.
- **Light theme green text:** `text_local_label` (`#007300`, 5.8:1) passes AA at normal size. `text_local_body` (`#005900`, 6.5:1) is comfortable.
- **Dark theme green text:** `text_local_label` (`#33cc33`) and `text_local_body` (`#4de64d`) on `#1a1a2e` — these are vivid but should be verified with a contrast tool as the exact ratios depend on monitor calibration.
- **Warning colour:** `#b8860b` (light) / `#e6c200` (dark) — these are for large indicators and borders, not primary text. For warning text, use `text_system` or `text_secondary`.
- **Online indicator:** The online dot uses `accent_green` colour plus a filled-circle icon shape — never colour-only. The companion label text (e.g. "Online") or presence of the dot vs. absence communicates state redundantly.

---

## 14. Landing Screen (Empty State)

The landing screen (`view_main_empty_state`, app.rs:12501) is shown when no conversation is selected (screen = `ChatList`).

### Layout

```
┌─────────────── MAX WIDTH 480px ───────────────┐
│                                                 │
│                BORU (TYPO_XL, accent_primary)    │
│          Private. Peer-to-peer. No              │
│                   central servers.              │
│                                                 │
│  ┌── Status Card (container_card) ──────────┐  │
│  │  ● Online                                │  │
│  │  ● Mesh: healthy                         │  │
│  │  ● Relay: connected via <relay>          │  │
│  │  ● Friends Online: 2 / 5                 │  │
│  └──────────────────────────────────────────┘  │
│                                                 │
│  ┌─────┐ ┌──────────┐ ┌───────┐ ┌──────────┐  │
│  │Start │ │Add Friend││ Join  │ │ Browse   │  │
│  │Chat  │ │          ││ Ticket│ │ Files    │  │
│  └─────┘ └──────────┘ └───────┘ └──────────┘  │
│                                                 │
│  ┌── Activity Card (container_card) ─────────┐  │
│  │  Recent Activity                           │  │
│  │  • Alice came online          1m ago       │  │
│  │  • Blue Falcon shared file   5m ago        │  │
│  │  (scrollable, max 200px)                   │  │
│  └────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────┘
```

### Components

| Section | Implementation | Notes |
|---------|---------------|-------|
| **Branding** | `text("BORU").size(TYPO_XL).color(accent_primary)` | |
| **Tagline** | `text("Private. Peer-to-peer. No central servers.")` | `TYPO_SM`, `text_muted` |
| **Status card** | `container(…).style(container_card)` | 4 status rows with Unicode dots |
| **Quick actions** | 2×2 grid of outline buttons | Each `width(Length::Fill)` |
| **Recent activity** | `scrollable` within `container_card` | Max 20 events, capped at 200px height |

### Status Indicators

| Row | Icon | Content | Dynamic |
|-----|------|---------|---------|
| 1 | ● green | "Online" | Static |
| 2 | ● blue | "Mesh: healthy / degraded / offline" | Dynamic — `MeshHealth` |
| 3 | ● blue | Relayed/disconnected relay mode | Dynamic — `relay_mode()` |
| 4 | ● green/grey | "Friends Online: N / M" or "No friends are online" | Dynamic |

---

## 15. Sidebar Structure

The sidebar (`view_sidebar`, app.rs:11482) is a fixed-width (280px) left panel with a branded header, identity row, and four collapsible sections.

### Layout

```
┌──── Sidebar (280px, bg_surface) ────────────┐
│                                               │
│  ┌─ Header ────────────────────────────┐     │
│  │ Boru (TYPO_LG)            ＋   ⚙   │     │
│  └─────────────────────────────────────┘     │
│  ┌─ Identity Row ──────────────────────┐     │
│  │ avatar  label: display_label        │     │
│  │         relay_mode                  │     │
│  └─────────────────────────────────────┘     │
│  ┌─ CHATS (N) ─── [▼] ────────────────┐     │
│  │ ● Peer_1            preview  12:34  │     │
│  │ ○ Peer_2            preview   5h    │     │
│  └─────────────────────────────────────┘     │
│  ┌─ FRIENDS (N) ── [▼] ───────────────┐     │
│  │ [Add friend by key…]                │     │
│  │ ● Alice                          …  │     │
│  │ ○ Bob                             …  │     │
│  └─────────────────────────────────────┘     │
│  ┌─ DISCOVER (N) ─ [▼] ───────────────┐     │
│  │ ● Peer  [Chat] [Browse Files]      │     │
│  └─────────────────────────────────────┘     │
│  ┌─ REQUESTS (N) ─ [▼] ───────────────┐     │
│  │ [Manage Requests]                   │     │
│  │ Alice                        ✓  ✗  │     │
│  └─────────────────────────────────────┘     │
│                                               │
│  (scrollable)                                 │
└───────────────────────────────────────────────┘
```

### Section Behaviour

| Section | Collapse | Content Source | Sort Order |
|---------|----------|---------------|------------|
| CHATS | Yes (toggle) | `conversation_store` | Online first, then recency, then name |
| FRIENDS | Yes (toggle) | `friends` (JSON) | Alphabetical by display name |
| DISCOVER | Yes (toggle) | `discovered_peers` | As-received from mDNS/DHT |
| REQUESTS | Yes (toggle) | `friend_request_store` | Alphabetical by requester name |

### Section Header

Each collapsible section header is rendered by `sidebar_collapsible_section_header()`:

| Property | Value |
|----------|-------|
| Label | ALL-CAPS section name + count badge |
| Expand/collapse | ▲ (expanded) or ▼ (collapsed) indicator |
| Toggle | Click on header row toggles collapse state |
| Data | `sidebar_section_collapsed[0..3]` boolean array |

### Conversation Row

Rendered by `view_sidebar_conversation_row()` (app.rs:11814):

| Element | Spec |
|---------|------|
| Avatar | 24×24px, circular, image or fallback initial |
| Status dot | "●" green (online) or "○" grey (offline) |
| Name | `TYPO_SM`, selected=WHITE, else `text_remote_body` |
| Unread badge | Inline `" [N]"` appended to name |
| Preview | `TYPO_XS`, `text_muted`, one line |
| Timestamp | `TYPO_XXS`, `text_muted`, relative time |
| Row padding | `[6, 12]` vertical/horizontal |
| Selected bg | `accent_primary` fill with `SPACE_4` radius |
| Hover bg | `bg_hover` (dormant — clicked via full-width button) |

---

## 16. Friend Profile View

The redesigned friend profile (`view_friend_profile`, app.rs:14670) displays friend details with inline rename and action buttons.

### Layout

| Section | Content |
|---------|---------|
| **Header** | Display name (or inline rename input + ✓/✕), "⋮" menu button, "✕" close button |
| **Status** | "● Online" or "○ Offline" with additional info ("Connected locally." when direct connection exists) |
| **Actions** | "Chat" (primary), "Browse Files" (outline), "Remove Friend" (danger), "Block Friend" (danger) |
| **Key info** | Peer public key (52-char hex, copyable), "Copy" button with "Copied!" feedback |
| **Recent Messages** | Last 3 messages in the conversation (clickable to open chat) |
| **Three-dot menu** | "Rename", "Message", "Copy Public Key", "Remove Friend", "Block Friend" (toggle), "Cancel" |

### Inline Rename

When the "⋮" menu "Rename" option is selected, the name element switches to a `text_input` with ✓ confirm and ✕ cancel buttons. The new name is stored as the friend's `label` in the `FriendsStore`, which takes top priority in the display-name resolution chain.

---

## 17. Peer Names (from `peer_names.rs`)

Peer names are generated deterministically from the peer's 32-byte Ed25519 public key (see `src/peer_names.rs`).

### Algorithm

1. First 4 bytes → u32 → adjective index into 110+ curated adjectives
2. Next 4 bytes + adjective component → noun index into 140+ curated nouns
3. Output: `"<Adjective> <Noun>"` (e.g. "Blue Falcon", "Quiet Harbour")

### Display Priority

```
1. Friend label (user-assigned nickname)
2. Remote profile display name (from ProfileUpdate gossip)
3. Last announced name (from friend record metadata)
4. Session / device name
5. Generated friendly name ("Blue Falcon")
6. Truncated peer ID ("dfab…961f") — secondary text only
```

### Truncated Key

`fmt_truncated()` produces: `"dfab…961f"` (first 4 hex chars + ellipsis + last 4 hex chars).

---

## 18. Remaining Planned Work

The following items from the original design spec have NOT been implemented and remain as future work:

1. **Dedicated `theme.rs` module** — tokens still live in `app.rs`
2. **Standardised button API** — `ButtonKind` enum not merged
3. **Unread badge pills** — inline `" [N]"` remains
4. **Vector status dots** — Unicode ●/○ characters remain
5. **Context menus** — no right-click menus anywhere
6. **Dashboard file-drop area** — Iced v0.14 lacks native drag-and-drop
7. **Toast notifications** — no transient notification system
8. **Sidebar search/filter** — no text input for filtering chats or friends
9. **Onboarding overlay** — no first-launch tutorial
10. **Room-level settings** — "Settings" button opens global settings
11. **"Voice" button** — dead button on friend profile, no action handler
12. **Export Friend** — no counterpart to "Import Friend"
13. **Delivery status indicators** — delivery_state tracked but not surfaced in chat log
14. **`SPACE_20` gap token** — not added
15. **`TYPO_XXXS` token** — not added
16. **Animation duration token** — not applicable (Iced has no animation API)
