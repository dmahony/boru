# Figure 4 QA timeline fixture — t_6814630e

Deterministic fixture that injects the Figure 4 target timeline (from
`figure4-timeline-spec.json`, extracted by t_49713d7a) into the Boru chat
timeline data store (`chat_history.json`) in deterministic store order.

**QA / test only.** The fixture writes exclusively into the data directory
you pass it and refuses to touch directories that already contain app data
unless `--force` is given. It never touches production data paths.

## Files

| File | Purpose |
|---|---|
| `scripts/figure4_fixture.py` | The fixture module (inject / cleanup / validate / selfcheck) |
| `figure4-timeline-spec.json` | Input spec: 13 ordered entries (Today/Yesterday separators, 4 system chips, 7 user bubbles) |
| `target-figure4.png`, `target-figure4-chat.png` | Source crops of Figure 4 used for extraction verification |

## Invocation

### 1. Inject the timeline

```bash
python3 scripts/figure4_fixture.py inject <data_dir>
```

Creates (or reuses) `<data_dir>` and writes:

- `secret_key.txt` — fixed deterministic identity (seed `bytes(range(32))`)
- `friends.json` — remote peer `28d7ee8656` (online, Direct conversation)
- `conversations.json` — the Direct conversation row (sidebar CHATS entry)
- `chat_history.json` — the Figure 4 timeline (schema v1)

The default output contains **12 entries**: one *today-anchor* system entry
(`Conversation started.` at 09:05 today) plus the spec's 11 real content
entries (4 system chips + 7 user bubbles) all dated **Yesterday**. The
anchor reproduces the figure's empty "Today" divider sitting above the
"Yesterday" divider (see `reproduction_notes[1]` in the spec). Capture with
the timeline scrolled so the anchor sits just above the viewport top.

Options:

| Flag | Effect |
|---|---|
| `--no-today-anchor` | Omit the anchor → 11 entries; the timeline starts at the "Yesterday" divider + chips (acceptable close match per spec) |
| `--spec PATH` | Use a different spec JSON (default: the extraction output) |
| `--now-ms MS` | Pin the clock (epoch ms) for byte-identical reruns |
| `--force` | Overwrite existing fixture files in `<data_dir>` |

### 2. Remove the injected data

```bash
python3 scripts/figure4_fixture.py cleanup <data_dir>
```

Removes exactly the files the fixture wrote, then removes the directory
itself if it became empty.

### 3. Validate an injected directory

```bash
python3 scripts/figure4_fixture.py validate <data_dir>
```

Checks schema version, entry order/content against the spec, contiguous
`event_id` 1..N, and a single topic. Anchor-aware (accepts both layouts).

### 4. Self-check (determinism + cleanup proof)

```bash
python3 scripts/figure4_fixture.py selfcheck
```

Injects twice with the same pinned clock into two temp dirs and asserts
byte-identical outputs, verifies spec order/content, then verifies cleanup
removes every injected file. Exit code 0 = all pass.

## Module API

```python
from figure4_fixture import inject, cleanup, load_spec

summary = inject("/tmp/boru-qa-data")          # returns paths + counts
removed = cleanup("/tmp/boru-qa-data")          # returns removed paths
spec = load_spec()                              # parsed spec dict
```

## Determinism guarantees

- Fixed identity seed, fixed remote key (`28d7ee8656` + padding, 64-hex),
  fixed message order/content/times — no randomness anywhere.
- Timestamps are computed relative to `now` so Today/Yesterday resolve on
  the run date; `--now-ms` pins the clock for byte-identical reruns.
- Same-minute entries are distinguished by a deterministic seconds offset
  (`id * 3 % 60`), so bubbles/chips never share a timestamp slot.

## GUI smoke test (how the timeline was verified)

The fixture was verified end-to-end by launching the real GUI headless
(`target/debug/boru`) under Xvfb against an injected data dir,
opening the Direct conversation through the MCP test-action path
(`boru_gui_open_conversation` + `boru_gui_set_peer_presence`), capturing a
1280x800 screenshot, and OCR-verifying the rendered timeline:

- Centered system chips with data-layer labels: `INFO e164bd892c joined`,
  `NAME 28d7ee8656 is now known as 28d7ee8656`, help chip.
- All 7 user bubbles in spec order: incoming left (`Hi e164bd892c!`,
  `Just testing the new Boru chat UI.`, `Absolutely. Boru keeps it private
  and secure.`), outgoing right (`Hey 28d7ee8656`, `Looks great! …`,
  `End-to-end encrypted as always.`, `Great work!`) with `Read` delivery
  indicators.
- Header (peer name + Online + E2EE cue), composer, and the "Today" date
  divider rendered.

The screenshot harness for t_693bab20 should use `scripts/figure4_fixture.py
inject <data_dir>` as the deterministic fixture step.
