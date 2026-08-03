# UI card shell evidence (t_67cfe73b)

Reusable card shell component built as a dedicated module:
`examples/iced_chat/card_shell.rs` (registered in `examples/iced_chat/main.rs`).

## What the component provides

`CardShell<'a, Message>` — a data-agnostic builder with:

- `title` (required) — rendered uppercase, muted, 12 px per the Figure 3 rail
  look.
- `count: Option<usize>` — optional count badge (primary-soft accent pill) next
  to the title.
- `on_view_all: Option<Message>` — optional "View all" ghost button in the
  header.
- `empty_message: Option<&'a str>` — shown with UI-04 empty-state typography
  (12 px muted, SecondaryText token) when no children are supplied.
- `max_height: f32` — fixed maximum height of the scrollable list body
  (default `DEFAULT_LIST_MAX_HEIGHT` = 180 px). Content beyond this height
  scrolls instead of growing the dashboard.
- `children: Vec<Element<'a, Message>>` — the caller's list rows. The shell
  never fabricates rows or sample data.
- `row_spacing: f32` — vertical spacing between rows (default `SPACE_2`).

Exported token: `CARD_ROW_HEIGHT: f32 = 48.0` — the shared 48 px rail row
height that sibling cards (Online Peers / Recent Activity / Tunnels) use so
all three cards share the same rhythm.

Styling comes entirely from UI-04 tokens: `design_tokens::card_style` (surface
background, 12 px radius, muted border, subtle shadow), `Typography` roles for
header/empty text, and spacing from the design-token scale.

## Captures

- `t_67cfe73b_cardshell_1280x800.png` — the developer gallery opened with
  Ctrl+Shift+G showing the new "Card Shell (Figure 3 rail)" section:
  - Empty state shell: "ONLINE PEERS" header with count badge `(0)` and the
    caller-provided message "No peers are online right now."
  - Populated shell: "ONLINE PEERS" header, count badge `(5)`, "View all"
    action, and 8 demo rows at 48 px inside a 140 px bounded body — a vertical
    scrollbar appears instead of the card growing without bound.
- `t_67cfe73b_cardshell_zoom_1280x800.png` — zoomed crop of the two shells
  side by side; the populated shell's right edge shows the rendered scrollbar
  rail (#DEDEDE) and scroller thumb (#BDBDBD), pixel-verified at
  x≈1226–1236 in the full capture (iced 0.14 default scrollable style).

The gallery is the UI-04 isolation harness for primitives; the component is
rendered there with no production data. The Ctrl+Shift+G shortcut was wired in
`keyboard_shortcuts_subscription` (app.rs) — it was documented in the gallery
header but previously unreachable.

## Verification

- `cargo check --features gui --example boru` — PASS.
- `cargo test --features gui --example boru` — 577 passed, 0 failed (10 new
  card shell unit tests: row-height token = 48 px, default bounded max height,
  count/view-all/empty-message/children storage, empty + populated build
  smoke tests, default row spacing).
- `cargo fmt --check` — PASS for all files touched by this card
  (card_shell.rs, component_gallery.rs, main.rs, and the app.rs shortcut
  hunk). Two pre-existing formatting diffs remain in files owned by other
  in-flight work: `app.rs:7817` (prior UI-10 rail work) and
  `src/diagnostics.rs:6303` (a concurrent SetPeerPresence GUI-test-command
  change) — left untouched to avoid clobbering another worker's edits.
- `git diff --check` — PASS.

## How to re-run

```bash
bash scripts/ui_cardshell_evidence.sh
```
