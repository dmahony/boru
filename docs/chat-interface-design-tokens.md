# Boru Chat — Design Tokens

Specification for the centralised visual token system in `design_tokens.rs` and `fonts.rs`.

## Palette (Light)

### Backgrounds

| Token | Hex | Role |
|-------|-----|------|
| `APP_BACKGROUND` | `#F4F6F4` | Main window background (primary canvas) |
| `SIDEBAR_BG` | `#FFFFFF` | Left sidebar background |
| `PANEL_BG` | `#F4F6F4` | Main panel / content area background |
| `ELEVATED_BG` | `#FFFFFF` | Elevated cards, dialogs, popovers |
| `SURFACE_SECONDARY` | `#EEF1EE` | Secondary surfaces — grouped controls, quiet sections |
| `SURFACE_HOVER` | `#E7EBE8` | Hover state for rows and interactive surfaces |
| `SELECTED_SURFACE` | `#E0EDFA` | Selected navigation row |

### Message Bubbles

| Token | Hex | Role |
|-------|-----|------|
| `OUTGOING_MSG_BG` | `rgba(0, 128, 0, 0.06)` | Local (self) message bubble background |
| `INCOMING_MSG_BG` | `rgba(25, 50, 128, 0.05)` | Remote message bubble background |

### Text

| Token | Hex | Contrast | Role |
|-------|-----|----------|------|
| `TEXT` | `#202522` | 11.5:1 | Primary body text |
| `TEXT_SECONDARY` | `#6B746E` | 5.5:1 | Secondary labels, previews |
| `TEXT_MUTED` | `#8A928D` | 3.5:1 | Muted hints, timestamps, placeholders |
| `TEXT_LOCAL_LABEL` | `#007300` | 5.8:1 | Local message sender label |
| `TEXT_LOCAL_BODY` | `#005900` | 6.5:1 | Local message body |
| `TEXT_REMOTE_LABEL` | `#0054A8` | 5.5:1 | Remote message sender label |
| `TEXT_REMOTE_BODY` | `#222222` | 11.5:1 | Remote message body |

### Accents & Status

| Token | Hex | Role |
|-------|-----|------|
| `PRIMARY` | `#2F6B4F` | Primary accent (interactive controls, active state) |
| `PRIMARY_HOVER` | `#285B44` | Primary hover |
| `PRIMARY_PRESSED` | `#214C39` | Primary pressed |
| `ONLINE` | `#28A45D` | Online / success indicator |
| `WARNING` | `#B3730D` | Warning / reconnecting / degraded state |
| `DESTRUCTIVE` | `#B64141` | Error / destructive action colour |

### Borders & Shadows

| Token | Value | Role |
|-------|-------|------|
| `BORDER` | `#DDE2DE` | Panel and card borders |
| `BORDER_SELECTED` | `#2F6B4F` | Selected-item border (primary accent) |
| `FOCUS_WIDTH` | `2.0` | Focus ring thickness |
| `SHADOW_CARD` | `rgba(0,0,0,0.08) 0 1 3` | Subtle card shadow |
| `SHADOW_DIALOG` | `rgba(0,0,0,0.20) 0 4 12` | Dialog / popover shadow |
| `SHADOW_ELEVATED` | `rgba(0,0,0,0.30) 0 4 24` | Elevated modal shadow |

### Input Fields

| Token | Hex | Role |
|-------|-----|------|
| `INPUT_BG` | `#F0F0F4` | Text input / search field background |

## Palette (Dark)

| Light Token | Dark Equivalent | Role |
|-------------|-----------------|------|
| `APP_BACKGROUND` | `#1A1A2E` | Window background |
| `SIDEBAR_BG` | `#29293D` | Sidebar background |
| `ELEVATED_BG` | `#2A2A3D` | Card / dialog background |
| `SURFACE_SECONDARY` | `#222238` | Secondary surface |
| `SURFACE_HOVER` | `#33334D` | Hover surface |
| `SELECTED_SURFACE` | `#293A56` | Selected row |
| `TEXT` | `#CCCCCC` | Primary text |
| `TEXT_SECONDARY` / `TEXT_MUTED` | `#999999` | Secondary text |
| `PRIMARY` | `#4A9EFF` | Primary accent |
| `ONLINE` | `#3DDC85` | Online indicator |
| `WARNING` | `#F2A626` | Warning indicator |
| `INPUT_BG` | `#222238` | Input background |

## Typography

Font families loaded at startup (see `fonts.rs`):

| Family | Weights | Scope |
|--------|---------|-------|
| Source Sans 3 | 400 Regular, 500 Medium, 600 Semibold, 700 Bold | Primary interface (85-90% of UI) |
| Raleway | 800 ExtraBold | Boru wordmark / branding only |
| JetBrains Mono | 400 Regular | Technical values (peer IDs, diagnostics) |

### Type-scale tokens

| Token | Size (px) | Role |
|-------|-----------|------|
| `XXS` | 10 | Fine print |
| `XS` | 11 | Timestamps, metadata, delivery state |
| `SM` | 13 | Body text, conversation previews, supporting text |
| `MD` | 15 | Contact names, button labels, section headers |
| `LG` | 18 | Secondary headings, section titles |
| `XL` | 24 | Primary headings, page titles, logo |

### Semantic typography roles (`Typography` enum in `fonts.rs`)

| Role | Font | Weight | Size (px) | Usage |
|------|------|--------|-----------|-------|
| `DisplayLarge` | Source Sans 3 | Bold | 24 | Primary heading |
| `DisplayMedium` | Source Sans 3 | Semibold | 18 | Secondary heading |
| `PageTitle` | Source Sans 3 | Bold | 24 | Page/screen title |
| `SectionHeading` | Source Sans 3 | Semibold | 18 | Section header |
| `ContactName` | Source Sans 3 | Semibold | 15 | Contact / conversation name |
| `ChatMessage` | Source Sans 3 | Regular | 13 | Message body |
| `ConversationPreview` | Source Sans 3 | Regular | 13 | Inbox preview |
| `ConversationPreviewUnread` | Source Sans 3 | Semibold | 13 | Unread inbox preview |
| `ButtonLabel` | Source Sans 3 | Medium | 15 | Button label |
| `NavigationLabel` | Source Sans 3 | Medium | 15 | Nav item label |
| `FormLabel` | Source Sans 3 | Medium | 15 | Form / setting label |
| `SupportingText` | Source Sans 3 | Regular | 13 | Secondary / helper text |
| `Timestamp` | Source Sans 3 | Regular | 11 | Message timestamp |
| `DeliveryState` | Source Sans 3 | Regular | 11 | Delivery status label |
| `SystemMessage` | Source Sans 3 | Medium | 13 | System event text |
| `TechnicalValue` | JetBrains Mono | Regular | 11 | Peer IDs, diagnostics |
| `BoruWordmark` | Raleway | ExtraBold | 24 | Brand logo |

## Spacing

4 px base unit scale:

| Token | Value (px) | Usage |
|-------|------------|-------|
| `SPACE_2` | 2 | Fine adjustments, tight inline gaps |
| `SPACE_4` | 4 | Minimum spacing, icon-to-text |
| `SPACE_6` | 6 | Tight padding |
| `SPACE_8` | 8 | Standard compact spacing |
| `SPACE_10` | 10 | Subtle inset |
| `SPACE_12` | 12 | Comfortable spacing |
| `SPACE_16` | 16 | Section spacing |
| `SPACE_24` | 24 | Large section spacing |
| `SPACE_32` | 32 | Page-level padding |

## Corner Radii

| Token | Value (px) | Usage |
|-------|------------|-------|
| `RADIUS_SM` | 8 | Buttons, cards, inputs |
| `RADIUS_MD` | 10 | Message bubbles |
| `RADIUS_LG` | 12 | Dialogs, modals |
| `RADIUS_XL` | 14 | Large elevated panels |

## Control Heights

| Token | Value (px) | Usage |
|-------|------------|-------|
| `CONTROL_HEIGHT` | 40 | Standard control height |
| `CONTROL_HEIGHT_COMPACT` | 36 | Compact controls (sidebar, inline) |

## Layout

| Token | Value (px) | Usage |
|-------|------------|-------|
| `SIDEBAR_WIDTH` | 300 | Left sidebar width |
| `AVATAR_SM` | 36 | Small avatar (sidebar rows, message bubbles) |
| `AVATAR_MD` | 48 | Medium avatar (identity block, profile) |
| `AVATAR_LG` | 64 | Large avatar (profile pages) |
| `MESSAGE_MAX_WIDTH` | 480 | Maximum message bubble width |
| `IMAGE_PREVIEW_MAX_WIDTH` | 360 | Inline image preview max width |
| `IMAGE_PREVIEW_MAX_HEIGHT` | 400 | Inline image preview max height |
