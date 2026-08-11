# UI evidence: Tunnels card via card shell (t_5f03f97d)

The Home right-rail Tunnels card now renders through the reusable
`CardShell` component from `examples/iced_chat/card_shell.rs`, matching the
Figure 3 rail treatment used by the sibling Online Peers and Recent Activity
cards (all three were converted in the same shared workspace).

## What changed

- `examples/iced_chat/app.rs` — `view_main_empty_state` Tunnels card:
  - `CardShell::new("Tunnels", tunnel_rows)` with a live count badge
    (`count(tunnel_list.len())`) from `TunnelService::list_tunnels()`.
  - Truthful empty state `"No active tunnels"` via the shell's
    `empty_message` (only rendered when no tunnels exist).
  - Header `View all` action wired to `AppMessage::ShowCreateTunnelDialog` —
    the same tunnel-management route the previous `Manage` button used.
  - Each row is real tunnel state only: lock icon tinted by backend status
    (`tunnel_status_color`), service name (from `shared_tunnels` metadata,
    falling back to the friend name), the local target endpoint
    (`tunnel_target_label`), a status label (`tunnel_status_label`:
    Available / Connecting / Connected / Failed / Disconnected / Expired /
    Revoked), and the existing per-row close action (`CloseTunnel`).
  - Rows are fixed at `CARD_ROW_HEIGHT` (48 px) so the rail rhythm stays
    consistent; the body is a bounded 120 px scrollable via `max_height`.
- `scripts/ui_tunnels_card_evidence.sh` — capture harness (wide, compact,
  zoom, and click-through).
- `docs/ui-redesign/evidence/ui-tunnels-card/` — screenshots below.

No sample data: rows derive from the live `TunnelService`; the empty state is
what a clean launch truthfully shows (matching the UI-10 rail evidence
pattern). A populated tunnel row requires a real friend + created tunnel,
which is not available in an isolated capture run.

## Captures

- `t_5f03f97d_tunnels_1280x800.png` — Home at the wide target size. OCR
  confirms the right rail: `ONLINE PEERS | 0/0 View all`,
  `RECENT ACTIVITY (2)`, and `TUNNELS (0) View all` with
  `No active tunnels`.
- `t_5f03f97d_tunnels_600x1280.png` — compact responsive width (600 px);
  the rail reflows below the hero and the card remains visible.
- `t_5f03f97d_tunnels_zoom_1280x800.png` — zoomed crop of the Tunnels card:
  uppercase `TUNNELS` header, count badge `(0)`, `View all` ghost action, and
  the `No active tunnels` empty state.
- `t_5f03f97d_viewall_before_1280x800.png` / `..._after_1280x800.png` —
  click-through verification: clicking the card's `View all` header action
  opens the Share Tunnel dialog (147,886 px changed in the center region; the
  dialog's `Cancel` button is OCR-visible after the click). The action
  dispatches `ShowCreateTunnelDialog`, the same route the previous `Manage`
  button used.

## Verification

- `cargo check --features gui --bin boru` — PASS.
- `cargo test --features gui --bin boru` — 578 passed, 0 failed.
- `cargo fmt --all -- --check` — the Tunnels card region is clean. Two fmt
  diffs remain in `app.rs` lines ~22029/22052 (Online Peers card) — that
  region is owned by concurrent sibling task t_d4ca2ca4 and was left
  untouched to avoid clobbering in-flight work.
- `git diff --check` — clean for the Tunnels card files.

## How to re-run

```bash
bash scripts/ui_tunnels_card_evidence.sh
```
