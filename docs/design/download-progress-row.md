# Download Progress Row — Design Specification

## Purpose

A stateless card widget that renders a single download row within the
chat / file-transfer view.  Used inside `iced::widget::lazy` for efficient
re-rendering of the download list.

## Data model (`DownloadAttachment`)

| Field                | Type                 | Notes                                   |
|----------------------|----------------------|-----------------------------------------|
| `kind`               | `TransferKind`       | Inbound / Outbound                      |
| `name`               | `String`             | Display filename                        |
| `ticket`             | `String`             | Serialised blob ticket                  |
| `transfer_id`        | `Option<TransferId>` | Assigned when transfer begins           |
| `state`              | `DownloadState`      | See state machine below                 |
| `source_peer`        | `String`             | Sender display name or short public key |
| `speed_bytes_per_sec`| `Option<u64>`        | Transfer speed, emitted periodically    |

## State machine (`DownloadState`)

```
Ready ──→ Active ──→ Completed
  │          │
  │          ├──→ Paused ──→ Active  (resume)
  │          │       │
  │          │       └──→ Ready    (reset)
  │          │
  │          └──→ Failed ──→ Ready  (retry)
  │
  └──→ Cancelled ──→ Ready  (retry)
```

Each state determines which visual elements are shown and which action
buttons are available (see below).

## Visual layout (5 rows, top→bottom)

```
┌─────────────────────────────────────────────────────┐
│ [Pending]  project_report.pdf           4.2 MiB     │  ← Row 1: state badge + filename + size
│ From: 12AB34CD56                       2.1 MiB/s    │  ← Row 2: source peer + speed (Active only)
│ ████████████████░░░░░░░░░  62%                     │  ← Row 3: progress bar (Active only)
│ [Pause] [Cancel]                                     │  ← Row 4: action buttons
│ ─────────────────────────────────                    │  ← Row 5: failure reason (Failed only)
└─────────────────────────────────────────────────────┘
```

### Row 1 — Header
- **State badge** — coloured pill (`[Pending]`, `[Downloading]`, `[Paused]`,
  `[Complete]`, `[Failed]`, `[Cancelled]`).  Background colour matches state
  semantics (blue=active, green=complete, red=failed, grey=cancelled).
- **Filename** — state-coloured text, grows to fill available width
  (truncated with ellipsis on overflow).
- **Size** — human-readable (e.g. `4.2 MiB`, `1.3 GiB`).  For Active state
  with unknown total, shows `X received`.

### Row 2 — Source & speed (Active state only)
- **Source** — `From: <peer>` label in muted text.
- **Speed** — transfer rate (e.g. `2.1 MiB/s`) in state colour.

### Row 3 — Progress bar (Active state only)
- **Bar** — Iced `progress_bar` widget, `6px` girth, fills width.
  Coloured using `accent_primary` (active track) over `border_muted` (track).
  Percentage label at right.
- **Verifying (Paused)** — bar dimmed using `border_muted` tones.

### Row 4 — Action buttons
Context-sensitive, rendered left→right:

| State       | Buttons                                |
|-------------|----------------------------------------|
| `Ready`     | [Download]                             |
| `Active`    | [Pause] [Cancel]                       |
| `Paused`    | [Resume] [Cancel]                      |
| `Completed` | [Open]                                 |
| `Failed`    | [Retry] [Cancel]                       |
| `Cancelled` | [Retry] [Remove]                       |

- **Primary action** — ghost/outline button with accent colour border.
- **Destructive action** — bare text button, system-red on hover.

### Row 5 — Failure reason (Failed state only)
- Top-bordered section with `Failed:` label in error colour + error message
  in muted text.  Visible only when `DownloadState::Failed { error }` and
  error string is non-empty.

## Card container
- Surface background (`bg_surface`), 1px state-colour border, rounded
  corners (10px).
- Inner padding: 12px top/bottom, 16px left/right.
- Column spacing: 6px between rows.

## Design tokens used (from `app.rs`)

| Token              | Usage                           |
|--------------------|---------------------------------|
| `accent_primary`   | Active badge, progress bar fill |
| `accent_green`     | Completed badge                 |
| `color_error`      | Failed badge, error text        |
| `bg_surface`       | Card background                 |
| `border_muted`     | Track background, button border |
| `text_system`      | Secondary/labels                |
| `TYPO_SM`          | Filename (14px)                 |
| `TYPO_XS`          | Source/speed (12px)             |
| `TYPO_XXS`         | Size, percentage, badge (10px)  |
| `SPACE_2`–`SPACE_16`| Spacing constants              |

## File structure

```
examples/iced_chat/
  download_progress_view.rs   ← widget implementation (this file)
  app.rs                       ← DownloadAttachment, DownloadState definitions
                                  and update handlers for all download actions
```

## Integration

Called from `app.rs::view_download_attachment_content()` which wraps it in
`iced::widget::lazy` for efficient cache-based re-rendering.

```rust
// app.rs line 3036
iced::widget::lazy(dependency, |(entry_index, attachment, dark_mode)| {
    Self::view_download_attachment_content(*entry_index, attachment, *dark_mode)
})
```

## Action messages

| Message                          | Trigger                  |
|----------------------------------|--------------------------|
| `ExecuteDownloadAt(usize)`       | Download / Retry buttons |
| `PauseDownloadAt(usize)`         | Pause button             |
| `ResumeDownloadAt(usize)`        | Resume button            |
| `CancelDownloadAt(usize)`        | Cancel / Remove buttons  |
| `OpenDownloadedFile(String)`     | Open button (Completed)  |
