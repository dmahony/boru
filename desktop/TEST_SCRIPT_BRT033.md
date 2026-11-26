# BRT-033 Manual accessibility verification — owner run

**Status: NOT RUN.** Complete this sheet on the packaged desktop build before release. Record OS, app version, date, tester, and assistive technology versions. Do not mark any check passed without performing it.

## Keyboard-only flow
1. Launch Boru and press `Tab` from the initial focus position. Confirm focus is visible at every step and follows titlebar → primary navigation → active workspace controls in a logical order; reverse with `Shift+Tab`.
2. Use `Enter`/`Space` on Chats, Shared media, and Settings. Confirm the active navigation item is announced/current and focus remains predictable after switching sections.
3. In Rooms, tab through room buttons, open a room, move to the message composer, type a draft and send using the button and `Enter`. Confirm no keyboard trap and focus remains usable.
4. In Shared media, reach file chooser and handle form without a pointer; verify labels, disabled states and transfer progress/cancel controls.
5. Open each dialog gallery variant. Confirm focus moves into the modal, cycles within it, Escape/outside-click dismisses ordinary dialogs, security approval cannot be accidentally dismissed, and focus returns to the trigger.
6. In Settings, operate selects, checkboxes, sliders and color input via keyboard. Confirm visible focus and changes are understandable.

## Screen-reader announcements
1. With NVDA/Firefox or NVDA/Chrome (Windows), VoiceOver/Safari (macOS), or Orca/Firefox (Linux), navigate by landmarks/headings. Confirm one identifiable main landmark, labeled primary and room navigation, and useful heading hierarchy.
2. Switch sections and verify current navigation state is announced. Verify connection status, loading state, errors, unread count and message delivery status are announced without excessive repetition.
3. Open dialogs and verify title/description, modal state, button names, focus containment, and return focus.
4. Inspect media controls and settings widgets for meaningful accessible names; confirm progress values and errors are announced.

## Contrast, zoom and reflow
1. Check dark, light and AMOLED themes. Verify ordinary text ≥4.5:1, large text and meaningful UI boundaries/indicators ≥3:1; include hover, focus, disabled and error states.
2. Repeat at 200% and 400% browser/app zoom. Confirm no clipped text or lost controls; content reflows and remains operable without two-dimensional scrolling except inherently scrollable media/messages.
3. Enable OS high-contrast/forced-colors and increased-contrast settings; confirm focus indicators, selected navigation and control boundaries remain visible.
4. Enable reduced motion; confirm dialogs and transitions do not animate noticeably.

## Summary sheet
| Area | Result (Pass/Fail/Not run) | Notes / defect IDs |
|---|---|---|
| Keyboard-only navigation and focus | NOT RUN | |
| Dialog focus trap / dismissal / return focus | NOT RUN | |
| Screen-reader landmarks and headings | NOT RUN | |
| Screen-reader status, errors, progress and controls | NOT RUN | |
| Contrast: dark / light / AMOLED | NOT RUN | |
| Zoom: 200% / 400% and reflow | NOT RUN | |
| Forced colors / increased contrast | NOT RUN | |
| Reduced motion | NOT RUN | |

Tester: __________  Date: __________  OS/build: __________  Assistive technology: __________
