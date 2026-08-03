# UI-11 evidence: Home quick actions, footer, and composition

The home landing screen now uses four equal quick-action cards wired to the existing
application messages:

- Create Public Room -> `AppMessage::CreateNewRoom`
- Create Group Chat -> `AppMessage::ShowCreateGroupDialog`
- Add Friend -> `AppMessage::OpenFriendRequests`
- Share Files -> `AppMessage::AttachPressed`

The previous six-button grid and duplicate share-files strip were removed. The
quick-action grid is four columns at wide widths, two columns at medium widths,
and one column only in compact mode. Card heights are normalized to keep the row
rhythm stable. A single bottom connection strip reports the live mesh health,
direct peers, relayed peers, and neighbor count. It deliberately does not claim
end-to-end encryption because that state is not exposed by the current home model.

## Final composition (t_24b1cb38)

The home screen is assembled as a responsive grid:

- **Page header** — time-of-day greeting + identity, quiet "Welcome to Boru"
  supporting line, and a compact connection status pill on the right.
- **Main content (two-thirds)** — connection hero card, Mesh Activity card
  (bounded 156 px event viewport), then the four quick-action cards.
- **Activity rail (one-third)** — Online Peers, Recent Activity, and Tunnels
  cards, each rendered through `iced::widget::lazy` with a fine-grained selector
  dependency so one card never rebuilds its siblings.
- **Connection footer** — one truthful, compact status strip spanning the full
  width: mesh health label, direct/relayed counts, and a state-derived
  encryption label ("QUIC encrypted" only while peer connections exist, "Idle"
  otherwise).

Responsive behavior follows plan §4:

- `grid_columns_for(window_width)` (in `examples/iced_chat/quick_actions.rs`)
  returns 4 columns at >= 1040 px, 2 columns at 640–1039 px, and 1 column below
  640 px. Unit tests cover every reference width and a contiguous 320–1920 sweep.
- The rail sits beside the main content at >= 900 px window width and reflows
  below the hero below that; the whole page is a vertical scroll region, so no
  text or control is ever clipped horizontally.
- No horizontal scrollbar appears at any required size (1024x720, 1280x800,
  1440x900, 1920x1080).

## Visual evidence

- `target-figure3.png` — Figure 3 rendered from the implementation-plan PDF page 5.
- `t_24b1cb38_side_by_side_1280x800.png` — cropped Figure 3 target beside the
  final implementation capture.
- `t_24b1cb38_home_1280x800.png` — required reference viewport; complete home
  composition (header, hero, mesh card, four quick-action cards, rail, and
  footer) is visible without vertical clipping.
- `t_24b1cb38_home_1024x720.png` — medium layout; action cards reflow into two
  columns without horizontal overflow (footer is reachable by scrolling the
  page region, as intended for a short viewport).
- `t_24b1cb38_home_1440x900.png` — wide layout; four action columns and 2:1
  main/rail composition with the footer fully visible.
- `t_24b1cb38_home_1920x1080.png` — largest required layout; four cards, rail,
  and footer remain stable.
- `t_d9f6a827_home_1280x800.png` — quick-action card task capture; four full-card
  targets visible in the action row (OCR-verified labels: Create Public Room,
  Create Group Chat, Add Friend, Share Files).

## Interaction smoke test

`scripts/ui11_quick_actions_smoke.sh` launched an isolated GUI instance for each
card and clicked each full-card target. Each flow was dismissed with Escape where
a dialog/file picker opened; all four processes remained alive:

```
public: click handled; process remained alive
group: click handled; process remained alive
friend: click handled; process remained alive
files: click handled; process remained alive
```

## Verification

- `cargo fmt --check` — pass
- `cargo check --features gui --example boru` — pass
- `cargo build --features gui --example boru` — pass
- `cargo test --features gui --example boru` — 598 passed, 0 failed
- `git diff --check` — pass
- Visual captures exercised at 1024x720, 1280x800, 1440x900, and 1920x1080.
