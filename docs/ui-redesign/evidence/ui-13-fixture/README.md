# Figure 4 timeline spec — t_49713d7a

Extraction of the exact target timeline state from Figure 4 of the Boru
Modern UI implementation plan (`Boru_Modern_UI_Hermes_Kanban_Implementation_Plan.pdf`,
page 6, "Target chat screen").

> **Consuming this spec to run the QA fixture?** See
> [`FIXTURE.md`](FIXTURE.md) — `scripts/figure4_fixture.py` injects these
> messages into `chat_history.json` deterministically in an isolated data
> dir (t_6814630e).

## Deliverable

- `figure4-timeline-spec.json` — structured, machine-readable spec consumed by
  the fixture builder (t_6814630e). Contains the ordered message list (13
  entries: 2 date separators, 4 system chips, 7 user bubbles) with each
  message's type, verbatim content, sender (local/remote), relative day and
  local time, delivery state, alignment, and styling hints, plus the full
  styling reference, target visual state (header/sidebar/composer/footer), and
  reproduction notes.
- `target-figure4.png`, `target-figure4-chat.png` — source crops of Figure 4
  used for verification.

## How to consume

1. Read `messages[]` in display order (already store order — do not reorder).
2. Use `type` (`date_separator` / `system_chip` / `text`), `content` verbatim,
   `sender`, `day_offset` (0 = today, 1 = yesterday) and `time` to build the
   injected chat_history entries. Timestamps are local-time strings; compute
   wall-clock offsets relative to "now" so Today/Yesterday resolve on the run
   date.
3. For `system_chip` entries, `classification.kind` + `chip_label` /
   `chip_accent` say what the current `boru_core::system_events::classify_system_event`
   + `presentation::system_event_chip_meta` pipeline will render for that
   verbatim string (INFO/HELP/NAME).
4. `reproduction_notes[]` covers the Figure 4 quirks: the empty "Today" section
   above "Yesterday", the identical rename keys, the delivery indicator, and
   the isolated-data-dir requirement.

## Verification

Extraction was cross-checked at 300 dpi (pdftoppm) with tesseract OCR at
1x-6x magnification plus direct visual inspection; ambiguous glyphs are listed
under `metadata.ambiguities`.

## Automated visual regression

Run the deterministic fixture through the real GUI and compare the 1280x800
capture against the Figure 4-derived baseline:

```bash
scripts/ui13_visual_regression.sh
```

The harness starts Xvfb and the GUI test-action MCP endpoint, so it requires no
manual interaction and is suitable for CI. It uses a tolerant per-channel
pixel comparison (`scripts/compare_screenshot.py`, Pillow only): the default
threshold is 16 channel values and at most 0.5% differing pixels. Override
those limits with `BORU_PIXEL_TOLERANCE` and `BORU_MAX_MISMATCH` when needed.

On success, the capture and comparison metrics are written to
`figure4-current-1280x800.png` and `figure4-comparison.json`. On failure, the
same directory also contains `figure4-diff-1280x800.png`, with changed pixels
highlighted in red, and the script exits non-zero. The committed
`figure4-baseline-1280x800.png` is the captured fixture smoke image from the
same target state; the source Figure 4 crops remain `target-figure4*.png`.
