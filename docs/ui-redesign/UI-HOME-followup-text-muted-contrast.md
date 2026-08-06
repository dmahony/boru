# UI-HOME follow-up: text_muted contrast — resolution

**Task:** t_b2ac1e1a (UI-HOME-followup)
**Filed by:** UI-HOME-18 QA (t_266bfba3) and the UI-HOME-19 release gate (t_61a56729)
**Resolved:** 2026-08-06 (UTC)
**Decision:** ✅ **Resolved — token already darkened; documentation trail corrected.**

## 1. What this card asked

> text_muted (#8A978F) measures 3.04:1 contrast on white — fails WCAG AA 4.5:1
> for normal-size text. Used for timestamps/metadata at 12px (TypeRole::Metadata /
> SupportingText). DESIGN_SYSTEM.md documents 2.8:1 as the muted tier; visual
> style intentionally matches the mockup, but the a11y threshold is not met.
> Acceptance: darken text_muted to ≥4.5:1 (e.g. #5F6F66-class) OR restrict muted
> to large/incidental text only, with a regression test pinning the new ratio.
> Update DESIGN_SYSTEM.md if the token changes.

## 2. Finding: the code fix already existed

`TEXT_MUTED` was darkened from `#8A978F` → `#64706A` in commit `04a3a7fe`
("feat(UI-19): WCAG AA contrast improvements — darkened muted/primary/success
tokens", 2026-08-04), which is an ancestor of the UI-HOME-18 QA commit
(`9d34a33c`). The QA checklist measured and reported the *stale*
DESIGN_SYSTEM.md hex (#8A978F / 3.04:1) rather than the live token value —
the checklist's own header claims ratios were "computed this run from the
actual token hex values in design_tokens.rs", but the code at that commit
already contained `#64706A`.

Measured WCAG 2.1 contrast for `#64706A` (light theme), computed from the
actual token:

| Surface            | Hex       | Contrast |
|--------------------|-----------|----------|
| White              | #FFFFFF   | 5.16:1   |
| Canvas             | #F7F9F8   | 4.88:1   |
| Sidebar            | #FCFDFC   | 5.06:1   |
| Input background   | #F0F0F4   | 4.54:1   |
| Primary soft       | #EAF5EE   | 4.62:1   |
| Selected surface   | #EDF7F1   | 4.71:1   |

All six light surfaces pass WCAG AA normal-text 4.5:1. The regression test
`design_tokens::tests::contrast_ratios_pass_wcag_aa` pins muted ≥ 4.5:1 on
white, canvas, sidebar, primary-soft bubble and selected surface — verified
passing this run (`cargo test --example boru --features gui design_tokens`:
24/24 passed).

## 3. Changes made

- `DESIGN_SYSTEM.md` §3.1 text token table: `text_muted()` hex `#8A978F` /
  `≥ 2.8:1` → `#64706A` / `≥ 4.5:1 (measured 4.62–5.16:1 on light surfaces)`.
- `DESIGN_SYSTEM.md` §4.6 offline-dot colour: light `#666` → `#64706A`.
- `DESIGN_SYSTEM.md` §10 legacy colour-function table: `text_muted` row now
  points at `app.rs:674` (the wrapper that delegates to
  `design_tokens::text_muted`) with Light `#64706A`.
- `docs/fs-02-file-sharing-dashboard-spec.md` reused-token table: muted row
  hex `#8A978F` → `#64706A` (line ref updated to current `design_tokens.rs:420`).

No source changes were required: the token and its regression test were
already compliant. This was a documentation/evidence-trail correction.

## 4. No action needed on the alternative branch

The card offered "restrict muted to large/incidental text only" as an
alternative. Not needed — muted text now passes AA normal at 12px, so no
usage restriction is required.
