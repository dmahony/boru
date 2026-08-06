# PAPIRUS-19 — Tests: Resolver, UI Integration, Fallback

Task: `t_b1d9040d` — the quality gate test suite for the Papirus icon system.
Source spec: `Boru_Papirus_icons.txt` Task 19 (attachment of `t_9d01cfec`).
Parents: PAPIRUS-09 (`t_a341eb4c`, resolver coverage), PAPIRUS-12 (`t_5ce83d41`,
folders), PAPIRUS-17 (`t_dfccb5f0`, perf/packaging), PAPIRUS-18 (`t_cade2c9b`,
import script).

## Outcome

Task 19 is a **tests-only** card: no production resolver / component / view
behaviour was changed (verified: the three touched files are test additions
only — 524 inserted lines, 0 production lines changed).  The suite lands four
groups:

### 1. Resolver tests (`file_type_resolver.rs`)

New PAPIRUS-19 resolver scenarios:

- `task19_unicode_filename_resolves_by_extension` — `résumé.pdf`,
  `фотография.png`, `音乐.flac`, `视频.mp4`, `دليل.docx`, `資料.xlsx`,
  `報告.pptx`, plus an extensionless Unicode name that falls back safely.
- `task19_very_long_filename_resolves_safely` — 256 KiB ASCII name with a
  real extension, 64 KiB compound-archive name, and a 64 KiB extensionless
  name; none panic, none resolve to a missing asset.
- `task19_required_scenarios_all_resolve_to_existing_assets` — the complete
  14-scenario matrix from the spec (MIME only, extension only, agreement,
  conflict, uppercase, compound, missing extension, hidden file, folder,
  unknown type, malformed MIME, path-like malicious, Unicode, very long
  filename); every result is grounded in a bundled SVG.

The spec's 18 required examples (`report.pdf` … `shared-folder`) were already
covered by `task9_required_examples_resolve_to_real_icons` from PAPIRUS-09 and
remain green.

### 2. Fallback tests (`file_type_resolver.rs` + `file_type_icon.rs`)

- `task19_fallback_exact_icon_missing_uses_category_icon` — removes the exact
  `video-mp4` icon from a catalog copy and asserts the resolver falls back to
  the broad `video-x-generic` category icon (priority 7).
- `task19_fallback_category_icon_missing_uses_unknown` — removes both the
  exact and the category icon and asserts the terminal `application-x-generic`
  unknown icon is used (priority 8).
- `task19_missing_bundle_file_renders_embedded_generic_not_broken`
  (`file_type_icon.rs`) — a bundle file missing at runtime must render the
  embedded unknown-generic icon; the returned handle's data equals the
  embedded fallback bytes, proving **no broken-image symbol can appear**.
- `task19_required_examples_render_real_svg_handles_at_every_size`
  (`file_type_icon.rs`) — every required example renders a real bundled SVG
  handle at all five semantic sizes (16/24/32/48/64) for files and folders.

### 3. UI integration tests (`download_progress_view.rs`)

The spec's Task 19 UI list (Chat, Shared by Me, Shared with Me, Downloading,
Downloaded, Peers Downloading from Me, Activity Log, Re-share dialog, Transfer
notification) is verified at the **shared-component level**, which is the
deepest feasible level in this harness:

- Full GUI automation (running `boru` with `--enable-gui-test-actions` and
  driving it over MCP) is impractical in the remote-build environment — there
  is no display, the binary lives only on debsrv, and the test command for
  this card is a filtered `rb test` invocation.
- Every surface above renders its icon through the **same** central entry
  points in `download_progress_view.rs`: `file_type_icon_element`,
  `decorative_file_type_icon_element`, `file_type_icon_element_with_tooltip`,
  and `directory_icon_element`.  These all funnel into
  `file_type_icon_element_impl` → `FileTypeIcon::new` → `resolve_file_icon`,
  and they all share one `FILE_TYPE_ICON_CACHE`.
- `task19_same_file_shows_same_icon_across_all_surfaces` drives the exact
  call signature each surface uses for `report.pdf` and asserts every surface
  resolves to the identical `application-pdf` icon / PDF category / bundled
  SVG path.
- `task19_same_folder_name_shows_folder_icon_on_folder_surfaces` proves a
  folder named `report.pdf` still renders `folder-open` on folder surfaces
  while the same name as a file stays `application-pdf` (no collision).
- `task19_spreadsheet_same_icon_across_surfaces` repeats the consistency
  check for a second type (`budget.xlsx`) so the guarantee is per-type, not a
  single-file coincidence.

### 4. Coverage note (surface → level)

| Spec surface | Verified at | Where |
|---|---|---|
| Chat (file card header, video cards) | shared component | `file_type_icon_element_with_tooltip` call sites: `download_progress_view.rs:705`, `video_file_card.rs:773/984`, `app.rs:29473/29577`; integration test drives the same signature |
| Shared by Me | shared component | `shared_by_me_table.rs:758/768/777` → `decorative_file_type_icon_element` |
| Shared with Me | shared component | `app.rs:32612` → `decorative_file_type_icon_element` |
| Downloading | shared component | `app.rs:34372` → `file_type_icon_element` |
| Downloaded | shared component | `app.rs:34070` → `decorative_file_type_icon_element` |
| Peers Downloading from Me | shared component | `app.rs:33734` → `file_type_icon_element` |
| Activity Log | shared component | `app.rs:33310` → `file_type_icon_element` |
| Re-share dialog | shared component | re-share reuses the chat card / transfer row components (`ReshareFile` → `ExecuteFileSend` → the same `file_type_icon_element_with_tooltip` / `file_type_icon_element` paths, `video_file_card.rs:984`, `app.rs:34935`) |
| Transfer notification | shared component (in-app) / structural (OS) | the in-app transfer rows (Downloading/Downloaded/Peers) carry the icon; the OS notification backend (`notification/backend.rs::RenderedNotification`) is title/body text only with no icon field — there is no icon surface at the OS level to diverge |

No production file-transfer payloads or message types were modified.

## Verification

- `rb check --example boru --features gui,video-playback,terminal` exit 0
  (worktree + canonical repo post-merge).
- Targeted test run (single invocation, filtered — never the full suite):
  `rb test --example boru --features gui,video-playback,terminal -- file_type_resolver file_category file_type_icon download_progress_view`
  → all resolver, fallback, and shared-component UI integration tests pass.
  (Note: cargo 1.97 does not split a single filter argument on commas, so the
  mandated `-- file_type_resolver,file_category` was run as the equivalent
  space-separated filters, extended with the two modules that carry the new
  Task 19 tests.)
- `rb test --lib` unchanged (no `src/` changes).
