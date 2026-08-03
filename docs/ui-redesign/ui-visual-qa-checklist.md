# Boru visual QA checklist

Use this checklist for each fixed-size screenshot and for every later UI redesign card.
The screenshot harness uses an isolated temporary data directory, loopback-only MCP,
and a generated local identity. It does not add mock data to normal builds.

## Target regions

- [ ] Shell: window bounds, background, global spacing, light/dark contrast
- [ ] Sidebar: Boru identity, add action, section headers, room rows, selected state, overflow
- [ ] Header: screen/room title, status, toolbar actions, alignment and truncation
- [ ] Cards: borders, radius, padding, hierarchy, empty/loading/error states
- [ ] Message timeline: date dividers, sender labels, bubbles, timestamps, scrolling, attachment bounds
- [ ] Composer: attachment affordance, input, focus state, send affordance, disabled/loading state
- [ ] Footer: bottom spacing, status/help affordances, clipping at the target size

## Required viewport matrix

- [ ] 1280x800
- [ ] 1024x720
- [ ] 1440x900

## Evidence rules

- Baseline files live under `docs/ui-redesign/evidence/baseline/` and are never overwritten by later phases.
- Names follow `<task-id>_<screen>_<width>x<height>_<state>.png`.
- Record visual regressions with the viewport, screen, expected result, actual result, and screenshot filename.
