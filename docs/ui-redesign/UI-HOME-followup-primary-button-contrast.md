# UI-HOME follow-up: primary-button white label contrast — resolution

**Task:** t_80938852 (UI-HOME-followup)
**Filed by:** UI-HOME-18 QA (t_266bfba3) and the UI-HOME-19 release gate (t_61a56729)
**Resolved:** 2026-08-06 (UTC)
**Decision:** ✅ **Resolved — token already darkened; documentation trail corrected.**

## 1. What this card asked

> White 14px semibold label on primary #188C50 = 4.28:1 — passes AA
> large-text (3:1) but misses AA normal-text 4.5:1. Affects primary buttons
> (ButtonLabel role) across the home screen and dialogs.
> Acceptance: darken primary (e.g. #147643-class) so white label ≥4.5:1, OR
> bump button labels to ≥18px semibold (large-text threshold), with a
> regression test pinning the ratio.

## 2. Finding: the code fix already existed

`PRIMARY` was darkened from `#188C50` → `#187F50` in commit `04a3a7fe`
("feat(UI-19): WCAG AA contrast improvements — darkened muted/primary/success
tokens", 2026-08-04), which is an ancestor of the UI-HOME-18 QA commit
(`9d34a33c`). The QA checklist measured and reported the *stale*
DESIGN_SYSTEM.md hex (#188C50 / 4.28:1) rather than the live token value —
the checklist's own header claims ratios were "computed this run from the
actual token hex values in design_tokens.rs", but the code at that commit
already contained `#187F50`.

Measured WCAG 2.1 contrast for `#187F50` (light theme), computed from the
actual token:

| Surface            | Hex       | White label on primary | Primary as ink on surface |
|--------------------|-----------|------------------------|---------------------------|
| White              | #FFFFFF   | 5.01:1                 | 5.01:1                    |
| Canvas             | #F7F9F8   | 5.01:1                 | 4.74:1                    |
| Sidebar            | #FCFDFC   | 5.01:1                 | 4.91:1                    |
| Input background   | #F0F0F4   | 5.01:1                 | 4.41:1                    |
| Primary soft       | #EAF5EE   | 5.01:1                 | 4.49:1                    |
| Selected surface   | #EDF7F1   | 5.01:1                 | 4.58:1                    |

The white `ButtonLabel` on the primary button background is 5.01:1 — above
the 4.5:1 AA normal-text threshold (the 14px semibold `ButtonLabel` is
normal-size text, so 4.5:1 is the applicable bar). The alternative branch
(bumping labels to ≥18px large-text) is unnecessary: the darkened token
passes the stricter normal-text requirement.

The regression test `design_tokens::tests::contrast_ratios_pass_wcag_aa`
pins `primary_on_white ≥ 4.5:1` — verified passing this run
(`cargo test --bin boru --features gui design_tokens`: 24/24 passed).

## 3. Changes made

- `DESIGN_SYSTEM.md` §3.1 accent token table: `primary()` hex `#188C50` →
  `#187F50` (with AA note).
- `docs/fs-02-file-sharing-dashboard-spec.md` reused-token table: primary
  accent hex `#188C50 (light)` → `#187F50 (light)` (line ref updated to
  current `design_tokens.rs:429`).
- `docs/chat-interface-design-tokens.md`: primary spec hex `#188C50` →
  `#187F50`.
- `UI-HOME-18-report.md` §8 remaining-difference item 2: marked resolved,
  pointing at this record.
- `docs/ui-redesign/evidence/t_266bfba3/accessibility_checklist.md`: primary
  contrast row annotated with the live-token footnote; follow-up ticket 2
  marked resolved.

No source changes were required: the token and its regression test were
already compliant. This was a documentation/evidence-trail correction.

## 4. Related observation (out of scope, flagged for reviewer)

Dark-theme `primary()` is the blue accent `Color::from_rgb(0.29, 0.62, 1.0)`
≈ #4A9EFF; white `ButtonLabel` on it measures 2.75:1 — below even the 3:1
large-text AA bar. Dark theme was explicitly out of scope for UI-HOME-18 QA
(light theme is default; UI-HOME-18-report §8 item 4). Recommend a separate
follow-up if dark-mode button contrast is in scope for the next pass.
