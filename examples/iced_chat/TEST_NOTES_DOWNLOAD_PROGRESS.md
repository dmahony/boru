# Attachment download progress — test notes

## Location

All tests are in `examples/iced_chat/app.rs`, module `#[cfg(test)] mod tests`, under the
`// ── Download progress lifecycle tests ──` section (starting ~line 9830).

## What is covered

### Unit tests (via `TestDownloadManager`)

| Test | What it verifies |
|---|---|
| `download_lifecycle_started_progress_completed` | Full happy path: Started → Progress (50%, 100%) → Completed. Checks state, action label ("Open"), and that `active_download_transfer_id` is cleared. |
| `download_lifecycle_started_progress_failed` | Started → Progress → Failed. Checks error message propagates to `status_label()` and action shows "Retry". |
| `download_lifecycle_started_cancelled` | Started → Cancelled. Checks action shows "Retry" and status shows "Cancelled". |
| `download_stale_progress_after_completion_ignored` | After Completed, a stray Progress event arrives. Documents known limitation: the progress handler **overwrites** terminal state because it does not check whether the current state is terminal before applying `DownloadState::Active`. |
| `download_transfer_id_anchoring_survives_entry_reorder` | TransferId anchoring: after a new text entry is inserted before the download entry (shifting index from 0→1), Progress for the original TransferId still finds the correct row at index 1. |
| `download_anchoring_falls_back_to_index_when_no_transfer_id` | The `Started` arm uses `current_download_entry_index(None)` → `download_entry_index` fallback when no TransferId is set on the entry yet. |
| `download_multiple_attachments_update_correct_row` | Two concurrent download entries. Progress for TransferId A reaches entry A, progress for TransferId B reaches entry B. |
| `download_unknown_total_shows_size_unknown` | When `total: None`, status shows "size unknown" and `progress_fraction()` returns `None`. |
| `download_image_lifecycle_uses_image_kind` | TransferKind::Image variants are **not matched** by `handle_download_progress` — they fall through to `_ => {}`. This documents that image download progress is not tracked by the same handler. |
| `download_zero_total_edge_case` | Zero total (total: Some(0)) prevents division by zero: `progress_fraction()` returns `None`. |
| `download_estimated_height_fits_each_state` | Verified height constants per state: Ready=84, Active+total=112, Active+unknown=104, Completed=92, Failed=104, Cancelled=84. |

### Pre-existing test

| Test | What it verifies |
|---|---|
| `download_attachment_state_helpers_cover_all_states` (line 8279) | `action_label()`, `status_label()`, and `progress_fraction()` return correct values for every `DownloadState` variant. |

## Known limitations

1. **Stale progress overwrites terminal state.** The `handle_download_progress` handler unconditionally sets `download.state = DownloadState::Active { bytes, total }` in the `Progress` arm, even if the entry already has a `Completed` state. Since the entry retains its `transfer_id`, stale Progress events continue to match the row and overwrite its state. A fix would add a guard like `if !matches!(download.state, DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled)` before applying Active.

2. **Started for non-File kinds is silently ignored.** The `Started` arm only matches `TransferKind::File`. `TransferKind::Image` and any future kinds fall through to `_ => {}` with no effect. If image download progress tracking is desired, dedicated match arms must be added.

3. **Multiple concurrent downloads rely on correct `download_entry_index`.** The `Started` arm uses `current_download_entry_index(None)` which returns `self.download_entry_index`. If this index is stale (e.g., a text entry was inserted ahead of the download entry), a new Started event will target the wrong row. The `Progress`/`Completed`/`Failed`/`Cancelled` arms are safe because they match by `TransferId`.

4. **Tests are synchronous unit tests, not integration tests.** `TestDownloadManager` replicates the `handle_download_progress` and `current_download_entry_index` logic in a standalone struct. The real `IcedChat::handle_download_progress` is almost identical, but any divergence between the two would be undetected. To fully verify the real method, an integration test against a live `IcedChat` instance would be needed.

## Manual verification steps

To exercise the real download progress flow end-to-end:

1. Launch two instances of `cargo run --example iced_chat --features gui`
2. Peer A shares a file via `/download <path>` in the chat
3. Peer B receives a system message with a "Download" button
4. Click "Download" on Peer B's side
5. Observe: the button label changes to "Downloading" and a progress bar appears
6. The status label updates incrementally as bytes arrive
7. On completion, the button changes to "Open" and the status shows "Saved"
8. Try closing and reopening the chat view during the download — verify the progress bar and status resume correctly (TransferId anchoring)
9. Try cancelling a download mid-transfer and verify the "Retry" button appears
10. Try failing a download (disconnect peers mid-transfer) and verify "Retry" + error message
