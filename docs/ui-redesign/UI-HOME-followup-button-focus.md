# UI-HOME follow-up: iced 0.14 button keyboard focus — re-evaluation

**Task:** t_d849e063 (UI-HOME-followup)
**Filed by:** UI-HOME-19 release gate (t_61a56729) from UI-HOME-17/18 QA (t_17a358c8, t_266bfba3)
**Re-evaluated:** 2026-08-06 (UTC)
**Decision:** ✅ **Accepted limitation — documented, no code workaround.** Re-evaluation trigger not met: the project has NOT upgraded (iced 0.14.0 is still the newest published iced release), button focus styling is still unsupported in the framework, and no non-fighting focus-delegation primitive exists in iced 0.14.

## 1. What this card asked

> Re-evaluate when the project upgrades to an iced version that supports button focus styling; if still unsupported, consider focus delegation (e.g. focusable wrappers) or document as accepted limitation. Do NOT attempt a workaround that fights the framework.

The three branches are evaluated in order below.

## 2. Re-evaluation trigger: has the project upgraded?

**No.** The dependency is unchanged and there is no newer iced to move to.

- `Cargo.toml:121` — `iced = { version = "0.14", ... }` (unchanged)
- `Cargo.lock` — `iced 0.14.0` resolved
- crates.io API (`crates.io/api/v1/crates/iced`) — `max_version: 0.14.0`, crate last updated `2025-12-07T20:51:51Z`

iced 0.14.0 is the current latest release on crates.io. The "on upgrade" condition has not occurred, so this re-evaluation confirms the pre-existing limitation against the current dependency state rather than finding a new one.

## 3. Framework check: does iced (any published version) support button focus styling?

**No.** Verified against the actual vendored sources:

- `iced_widget-0.14.2/src/button.rs:471-480` — `pub enum Status` has exactly four variants:
  `Active`, `Hovered`, `Pressed`, `Disabled`. **No `Focused`.**
- `iced_widget-0.14.2/src/button.rs` — zero focus handling: no `on_focus`, no `Focusable` impl, no focus-related operations. Buttons are not part of the widget focus chain at all.
- `iced_widget-0.14.2/src/` — the only widget implementing iced's `Focusable` trait is `text_input.rs`. `Focusable` lives at `iced_core-0.14.0/src/widget/operation/focusable.rs` and is a widget-operation trait, not a general keyboard-navigation affordance.
- iced 0.14's focus features (`focus`, `unfocus`, `is_focused` operations/selectors, CHANGELOG entries #2664/#2804/#2812) operate on widgets that opt into `Focusable` — which is effectively text inputs only.

Conclusion: no published iced version supports button focus styling; 0.14.0 is the newest release and it does not.

## 4. Focus delegation: is a focusable-wrapper primitive viable?

**Not without fighting the framework.** The card suggested "focus delegation (e.g. focusable wrappers)" as an alternative. Evaluated:

- iced 0.14 ships **no generic focusable-wrapper widget** — there is no `focusable()` helper in `iced_widget-0.14.2` helpers or in the `iced` facade. The `Focusable` trait is implemented by `text_input` only.
- Making a button keyboard-focusable would require authoring a custom widget implementing iced's `Widget` trait plus the `Focusable` operation trait (layout, draw, event, overlay, state handling), i.e. re-implementing button semantics inside a bespoke widget — exactly the kind of framework fight the card prohibits.
- Wrapping buttons in invisible `text_input`s (the only focusable widget) is an abuse of the framework and would break the focus chain, screen-reader semantics, and Tab ordering.

Verdict: focus delegation is not a clean option in iced 0.14; it is rejected per the card's own constraint.

## 5. Upstream status (what to watch for)

Both upstream attempts to make buttons focusable are **closed without merging**:

| PR | Title | State |
|----|-------|-------|
| iced-rs/iced#2736 | Allow Button widgets to be focusable | closed, not merged |
| iced-rs/iced#1640 | Add focus operation and style for button | closed, not merged |

So even iced master has no accepted button-focus path. Re-evaluation should be triggered by an actual release note / merged PR adding a `Focused`-style status or a focusable button primitive — not by an arbitrary version bump.

## 6. Accepted limitation and existing mitigations

**Accepted limitation (documented):** home quick-action buttons and row action buttons are mouse/hover-operated, not Tab-reachable, because iced 0.14 buttons cannot take keyboard focus. This is a pre-existing framework limitation, not a regression introduced by the UI-HOME work.

Existing keyboard mitigation already in the app (verified in UI-HOME-06/17/18):

- **Global shortcuts** keep the primary home actions keyboard-reachable without Tab focus: e.g. `Ctrl+N` → Create Room dialog (UI-HOME-06 evidence `t_2577e385_keyboard_ctrln_1600x900.png`).
- **Dialog inputs auto-focus**: on dialog open the first meaningful field gets `focus(...)` (CreateNewRoom → `CREATE_ROOM_NAME_INPUT`, CreateGroup → `CREATE_GROUP_NAME_INPUT`, CreateTunnel → `SHARE_SERVICE_NAME_INPUT`); Tab/Shift+Tab move between inputs (`Shortcut(FocusNext/Previous)` at app.rs:12686-12689, verified in UI-HOME-17/18: name → description Tab order).
- **Focus ring on text inputs**: 2px `color_focus` ring (app.rs:29070-29078); buttons deliberately rely on hover/pressed states since no focused state exists.
- **Hover affordance**: `button::Status::Hovered` is styled; row/quick-action buttons give visual feedback on hover, which is the only interactive affordance iced 0.14 buttons support beyond press.

## 7. Re-evaluation trigger (next time)

Re-open this card when ANY of these is true:

1. `cargo tree | grep iced` shows a major/minor bump (0.15.x+) **and** its changelog/release notes mention button focus (watch for a `Status::Focused` variant or a focusable button primitive).
2. iced-rs/iced#2736 or #1640 (or a successor) is merged upstream and released.
3. The project adopts a different UI toolkit where button focus is first-class.

Until then: keep the accepted-limitation wording in the regression report and UI-HOME-18 report as-is; do not add custom focusable-button widgets.

## 8. Evidence

- `Cargo.toml:121` / `Cargo.lock` — iced 0.14.0 pinned
- crates.io API: max_version 0.14.0 (fetched 2026-08-06)
- iced_widget-0.14.2 `src/button.rs:471-480` — Status enum (no Focused)
- iced_widget-0.14.2 `src/` — only `text_input.rs` implements `Focusable`
- GitHub API: PR #2736, #1640 closed/unmerged (fetched 2026-08-06)
- `docs/chat-ui-regression-report.md` §Known Visual Limitations #2
- `docs/ui-redesign/UI-HOME-18-report.md` §8 item 3, UI-HOME-19 gate §6 item 3 / §7 (follow-up t_d849e063)
