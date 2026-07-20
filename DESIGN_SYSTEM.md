# Boru Chat — Design System

> **Version:** 1.0  
> **Created:** 2026-07-21  
> **Scope:** `examples/iced_chat/` — the `iced` desktop GUI for boru-chat  
> **Audience:** Developers implementing the UI redesign (Steps 2–9)

This document specifies every visual token, component, and behaviour in the Boru Chat UI. All values reference the existing codebase and propose a unified system to replace the current ad-hoc styling.

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

### Margins & Spacing by Section

| Context                           | Padding / Gap values                | File:Line(s)                   |
|-----------------------------------|-------------------------------------|--------------------------------|
| Sidebar header (Boru Chat title)  | `top:12 right:12 bottom:4 left:12`  | `app.rs:10225-10230`           |
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

> **Note:** Unread badges currently render as inline text (`" [3]"`) rather than a pill/badge element. The redesign should introduce a dedicated unread badge pill.

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

**Current state:** Unread counts are shown inline as `" [N]"` in the conversation name text. No dedicated badge element exists.

**Proposed spec for redesign:**

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

| Property     | Value                          | File:Line |
|-------------|--------------------------------|-----------|
| Online       | "●" (filled circle), green    | `app.rs:10433` |
| Offline      | "○" (hollow circle), grey     | `app.rs:10433` |
| Idle         | Not yet implemented — proposal: "◐" amber | |
| Font size    | Inline with name text (inherits `TYPO_SM`) | |
| Colour - Online (light)| `#1a8c33` -> `accent_green` | `app.rs:408` |
| Colour - Online (dark) | `#3ddc84` -> `accent_green` | `app.rs:406` |
| Colour - Offline (lt)  | Same as `text_muted` (`#666`)  | `app.rs:10470` |
| Colour - Offline (dk)  | Same as `text_muted` (`#999`)  | `app.rs:10470` |

**Proposal:** Replace Unicode characters with a proper circle widget for better visual consistency. A solid circle of a fixed diameter (8px) with appropriate margin.

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

**Current state:** No context menus exist in the codebase. All interactions use explicit buttons.

**Proposed spec for redesign:**

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
│  │  │ Boru Chat    ＋  ⚙  │ │  │ Header        │ │ │
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

| Token           | Value  | Usage                                  | File:Line |
|-----------------|--------|----------------------------------------|-----------|
| `SPACE_4`       | 4px    | Small: sidebar selected row, small buttons | `app.rs:10496` |
| `SPACE_6`       | 6px    | Buttons (primary, secondary), settings buttons | `download_progress_view.rs:128` |
| `SPACE_8`       | 8px    | Cards, chat bubble, composer container | `app.rs:468, 11298, 11560` |
| `SPACE_10`      | 10px   | State badge pill, download card        | `download_progress_view.rs:96, 338` |
| `SPACE_12`      | 12px   | Avatars, dialogs, help panel           | `app.rs:10647, 11054` |

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

### Button States

| State    | Primary                               | Ghost / Text                     |
|----------|---------------------------------------|----------------------------------|
| Default  | Filled `accent_primary`, white text   | Muted text, no background        |
| Hovered  | Slightly lighter primary fill         | `accent_primary` text            |
| Pressed  | 85% brightness of accent              | 85% brightness of accent         |
| Disabled | N/A (no disabled buttons in current UI)| N/A                             |

### Row / Item States

| State    | Sidebar conversations                 | Sidebar friends / peers           |
|----------|---------------------------------------|-----------------------------------|
| Default  | Transparent background                | Transparent background            |
| Selected | `accent_primary` fill, white text     | N/A                               |
| Hovered  | None (button borderless, clickable area) | None (button borderless)         |

### Text Input States

The composer input (`text_input`) uses iced's built-in theme styling with a custom container border (see composer section). The settings input uses iced defaults.

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

## 11. Recommended Design Token Changes

### 11.1 Adopt a central token module

Currently colours, spacing, and typography are defined as `pub(crate)` constants and free functions in `app.rs`. For the redesign, consider extracting them into a dedicated `theme.rs` module.

### 11.2 Add missing tokens

| Token                    | Reason                                          |
|--------------------------|-------------------------------------------------|
| `TYPO_XXXS` (8px)       | For very dense data (download speeds, file sizes) |
| `SPACE_20` (20px)       | Gap between major sidebar sections              |
| Text `WARNING` (amber)  | For transient failure states (temporary)        |
| `ANIMATION_DURATION`    | Standard transition speed for hover/selection   |

### 11.3 Standardise button API

Currently button styles are defined inline in multiple places. A single `ButtonKind` enum (`Primary`, `Secondary`, `Ghost`, `Danger`, `TextOnly`) with a unified `button_style` function would reduce duplication.

### 11.4 Replace unicode status dots with vector circles

The `"●"` and `"○"` characters for online/offline status are not aligned consistently across platforms. Replace them with a coloured circle widget (e.g. 8×8px container with `Border { radius: 50% }`).

### 11.5 Introduce real unread badge pills

Replace inline `" [N]"` text with a rendered pill container (see section 4.5).

---

## 12. Implementation Order (for downstream Steps 2–9)

1. **Step 2** — Extract theme tokens into `theme.rs`, add missing tokens
2. **Step 3** — Standardise button style helpers
3. **Step 4** — Implement notification badge pills and status dot widgets
4. **Step 5** — Refine sidebar spacing and layout
5. **Step 6** — Polish chat panel (bubbles, timestamps, avatar layout)
6. **Step 7** — Add context menus
7. **Step 8** — Finalise modal/dialog consistency
8. **Step 9** — UX audit (assigned to `linux` profile)
