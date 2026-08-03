# t_6fda7f62 — Data-layer event mapping + grouped system-event spacing

Wires `boru_core::system_events::classify_system_event` (16 variants, from
t_85b9dbec) into the timeline chip rendering (components from t_ead7de5f)
and groups consecutive plain system events with tighter vertical spacing
than user messages. Ordering is preserved — the loop renders entries in
store order; grouping only changes the gap between adjacent chips.

## Behavior

- `presentation::system_event_chip_meta` maps every data-layer variant to a
  compact label + restrained accent; nothing is silently discarded.
- `presentation::continues_system_group` decides when two adjacent plain
  system chips (no download attachment) belong to one tight visual group.
- In `view_chat_log`, chips in a continuing system group use `SPACE_2`
  inner spacing (tight), while user messages keep their normal spacing
  (`SPACE_8` label-to-bubble; `SPACE_4` outer column rhythm).

## Evidence

- `t_6fda7f62_grouped_1280x800.png` — conversation opened through the real
  GUI path; the timeline shows consecutive system-event chips from the live
  friend-status path with the new data-layer labels (LEFT "Friend ...
  is now offline", JOIN "Friend ... is now ONLINE") at tight 4-5px gaps.
- `t_6fda7f62_grouped_1024x720.png` — same at the alternate viewport.
- `t_6fda7f62_chips_zoom_1280x800.png` — zoomed crop of the grouped chips.
- `verification.json` — pixel analysis of chip surfaces and gaps: each
  capture has >= 3 chip bands separated by <= 10px (tight grouping).
- OCR notes: the LEFT/JOIN labels only exist in the 16-variant data-layer
  mapping (the old 5-variant classifier never produced them), proving the
  mapping is wired in; chips appear in push order (offline notices from the
  seed, then the online notice from the presence simulation), so ordering
  is unchanged. Live user-bubble captures require a network peer; that
  limitation matches the prior UI-13 reviewer finding, and the normal user
  spacing path (SPACE_8 label-to-bubble) is preserved by the code change
  (only consecutive plain system chips are tightened to SPACE_2).
