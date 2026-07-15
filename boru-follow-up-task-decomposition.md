# Boru Chat: Evidence-Driven Follow-up Task Decomposition

This template is for confirmed issues only. Runtime-specific values must be populated from the MCP diagnostic child and independently checked against the board-inspection child before a task is created. Use `unknown`/`not_observed` when evidence is insufficient; never infer a failed stage from missing telemetry.

## Universal task title patterns

- `Fix <symptom> at <first_failed_stage>`
- `Expose/repair <diagnostic or state transition> for <direction>`
- `Add regression coverage for <reproducible scenario> (<first_failed_stage>)`
- `Document/reproduce <concrete workflow gap>`

Keep titles scoped to one independently verifiable outcome. Include direction (`A→B`, `B→A`, or `both`) when asymmetric.

## Required task body

### Problem
One-sentence confirmed defect or concrete observability gap.

### Reproduction
Numbered MCP/GUI steps using the actual workflow. Runtime values require MCP evidence.

### Expected / Actual
State the expected transition and the observed transition, without speculation.

### Diagnostic evidence
Required fields:
- affected client labels and abbreviated node IDs
- direction and safe abbreviated room/peer IDs
- first failed stage and furthest successful stage
- reproducibility: count, conditions, and supported timeout
- relevant event types, sequence range, timestamps, and error codes
- probe ID/message hash abbreviated; never include message contents or secrets
- network, application, GUI-diagnostic, and visible-GUI observations as applicable
- confidence (`high`, `medium`, `low`)
- provenance: MCP diagnostic child evidence reference; board-inspection child duplicate-search result

Every runtime-specific field above is `TBD—MCP evidence required`; the proposed task must not assert a value until the MCP child supplies it. The board-inspection child must confirm no equivalent active or archived task before creation.

### Likely area
Files/modules only when supported by source inspection; otherwise `unknown`.

### Suggested tests
Name the smallest unit, integration, two-client, GUI, or MCP regression test that verifies the task.

### Acceptance criteria
Use the stage-specific template below plus: reproduce the original failure before the fix, verify the target transition in both relevant directions where applicable, and retain existing tests.

## Stage-specific decomposition and acceptance criteria

For each confirmed first failure, create the smallest task matching the earliest broken transition:

| First failed stage | Title pattern | Acceptance criteria template |
|---|---|---|
| `local_room_unavailable` | `Restore shared-room availability for <clients/direction>` | Approved existing room can be opened/joined on both clients; room identity is consistent; no unrelated room is created; subsequent diagnostics can start. MCP and board checks required. |
| `discovery` | `Fix <mechanism> discovery for <direction>` | Expected peer is discovered within supported timeout in the affected direction(s); discovery source and event are recorded; no false claim when only absence is observed. MCP and board checks required. |
| `address_resolution` | `Fix discovered peer address resolution for <direction>` | Observed address is resolved through the supported path; resolution status/error is diagnosable; connection proceeds or reports a deterministic actionable error. MCP and board checks required. |
| `connection` | `Fix peer connection failure after address resolution (<direction>)` | Connection attempt reaches established state within timeout, or returns a stable actionable failure; repeated test has same expected result; no credential/secret leakage. MCP and board checks required. |
| `subscription` | `Fix topic subscription/join transition for <direction>` | Established peer successfully joins the intended subscription; join/leave events are consistent; timeout/error is surfaced. MCP and board checks required. |
| `topic_membership` | `Fix peer topic membership state for <direction>` | Peer appears as a topic member after subscription; membership agrees across network diagnostics; transient churn is reproduced before fixing and absent/handled after. MCP and board checks required. |
| `probe_broadcast` | `Fix diagnostic probe broadcast from <sender>` | Unique probe send returns success and a broadcast result; probe ID/hash/timestamp correlate; failed broadcast has an actionable error; both directions if applicable. MCP and board checks required. |
| `probe_delivery` | `Fix probe delivery from <sender> to <receiver>` | Receiver gets exact correlated probe ID/hash once within supported latency; sender identity and timestamps match; duplicate count is acceptable; test both directions when defect is bidirectional. MCP and board checks required. |
| `gui_action_queue` | `Fix GUI action queueing for <action>` | Semantic action is accepted/queued exactly once; queue status is observable; invalid/unavailable action is rejected clearly; no silent disappearance. MCP and board checks required. |
| `gui_action_handling` | `Fix GUI handling of queued <action>` | Queued action is handled; completion/failure is reported; expected state transition occurs; composer/room action is not duplicated. MCP and board checks required. |
| `network_to_iced_event` | `Route <network event> into Iced handling` | Network receipt produces the expected Iced event exactly once; event type/sequence is diagnosable; no event loss between network and UI event handling. MCP and board checks required. |
| `application_state_update` | `Fix application-state update after <event>` | Iced-handled event updates the intended application state; state contains correlated room/peer/message data; consistency check passes without relying on GUI rendering. MCP and board checks required. |
| `gui_state_update` | `Refresh GUI state after <application event>` | Updated application state appears in GUI diagnostic state within supported timeout; selected room/peer/composer state is correct; snapshots do not remain stale. MCP and board checks required. |
| `message_visibility` | `Show received message in selected conversation` | Correlated message is present in application state and visible when the correct room/conversation is selected; intentional diagnostic hiding/filtering is distinguished from a defect; both local and remote visibility are checked as applicable. MCP and board checks required. |

## Priority guidance

- `Critical`: security issue, data loss, or application unusable; requires direct evidence.
- `High`: consistently broken core discovery, topic membership, or bidirectional messaging.
- `Medium`: partial/asymmetric failure, GUI/application inconsistency, weak error handling, or missing observability that impedes diagnosis.
- `Low`: documentation, non-blocking cleanup, or a test-only/observability improvement with a working user path.

Downgrade unconfirmed or one-off behavior to `unknown/not_observed`, not a defect task. The diagnostic child supplies reproducibility and impact; the board-inspection child checks existing priority/conventions and duplicates.

## Combine versus split

Combine symptoms only when MCP evidence shows the same first failed stage, same root-cause boundary, same affected workflow, and one fix plus one acceptance test can verify all symptoms. Keep directional variants together only when both directions fail identically and the same implementation path is implicated.

Split when first failed stages differ; a later symptom is merely downstream; directions are asymmetric; fixes/tests can land independently; network and GUI/state layers have separate failures; or one symptom is confirmed while another is only `not_observed`. A broad parent task may link related tasks, but each child must have one completion condition.

## Creation gate

Create a follow-up only if the issue is reproducible and confirmed, is a material diagnostic gap, catches a confirmed issue with missing regression coverage, or is a concrete stability/security/documentation blocker. Before creation, attach MCP evidence and the board-inspection duplicate result. Do not create tasks for successful tests, recoverable delays, speculation, vague redesigns, or an equivalent existing task.
