# Boru Modern UI Redesign — Evidence Index

This index links the baseline captures, the phase checkpoint evidence, and the
accepted final screenshots for the Boru Modern Home and Chat UI Redesign
(Kanban epic UI-00 … UI-22). All screenshots come from the **running Boru GUI**
captured under Xvfb using the documented screenshot workflow
(`scripts/ui_baseline_screenshots.sh`, `scripts/ui21_final_evidence.sh`, and
the per-phase evidence scripts) — never from image-generation tools or mock
data in production paths.

Screenshot naming convention: `<task-id>_<screen>_<width>x<height>_<state>.png`.

## 1. Baseline (UI-00 / UI-01)

Pre-redesign home and chat screens, used as the regression baseline for
functionality.

| File | Shows |
|---|---|
| `baseline/t_9ec8d24f_home_1280x800_baseline.png` | Home at reference viewport |
| `baseline/t_9ec8d24f_home_1024x720_baseline.png` | Home at medium viewport |
| `baseline/t_9ec8d24f_home_1440x900_baseline.png` | Home at wide viewport |
| `baseline/t_9ec8d24f_chat_1280x800_baseline.png` | Chat at reference viewport |
| `baseline/t_9ec8d24f_chat_1024x720_baseline.png` | Chat at medium viewport |
| `baseline/t_9ec8d24f_chat_1440x900_baseline.png` | Chat at wide viewport |

Supporting baseline records: `baseline-build.log`, `baseline-tests.log`,
`baseline-test-list.log`, `baseline-launch.log`, `baseline-home-1280x800.png`,
`baseline-capture.log`, `ui-00-baseline-evidence.tar.gz`.

Architecture map: `current-ui-map.md` (UI-00 audit).
Screenshot harness description: `README.md` (top of `docs/ui-redesign/`).

## 2. Phase checkpoints

Each phase produced its own evidence folder with a README explaining what was
verified and how to reproduce it. The phase gates are defined in the
implementation plan (PDF, section 5).

| Phase | Folder | What it proves |
|---|---|---|
| 1 — Foundation | `evidence/ui-06-v4/` … `evidence/ui-10/` | Fonts/tokens (UI-02), icon system (UI-03), primitives + shell (UI-04/05), sidebar (UI-06), home hero (UI-08), rail (UI-10) |
| 2 — Home | `evidence/ui-11/` | Home quick actions, footer, composition vs Figure 3 |
| 3 — Sidebar/functional | `evidence/ui-12/` | Chat header/toolbar, functional states |
| 4 — Chat | `evidence/ui-13/`, `ui-13-fixture/`, `ui-14/`, `ui-15/`, `ui-16/` | Timeline (UI-13), bubbles + grouping (UI-14), composer (UI-15), footer + composition (UI-16) vs Figure 4 |
| 5 — Integration | `evidence/ui-17/`, `ui-18/`, `ui-19/` | Real-state integration, responsive/DPI matrix, keyboard/accessibility |
| Supplementary | `evidence/ui-activity/`, `ui-cardshell/`, `ui-event-grouping/`, `ui-online-peers/`, `ui-skeletons/`, `ui-timeline-items/`, `ui-timeline-region/`, `ui-tunnels-card/`, `fs-09/`, `fs-24/` | Component-level and file-sharing dashboard evidence |

Worker report template used across phases: `ui-18-worker-report.md` and the
per-folder `README.md` files.

## 3. Final screenshots (UI-21 — accepted)

Archived under `evidence/final/` by `scripts/ui21_final_evidence.sh`. The
orchestrator accepted the home screen against Figure 3 and the chat screen
against Figure 4.

### 3.1 Side-by-side target comparisons (1280×800)

| File | Shows |
|---|---|
| `final/side_by_side_home_1280x800.png` | Figure 3 target beside final home |
| `final/side_by_side_chat_1280x800.png` | Figure 4 target beside final chat |

### 3.2 Viewport matrix (home and chat)

| Viewport | Home | Chat |
|---|---|---|
| 1024×720 | `final/final_home_1024x720.png` | `final/final_chat_1024x720.png` |
| 1280×800 | `final/final_home_1280x800.png` | `final/final_chat_1280x800.png` |
| 1440×900 | `final/final_home_1440x900.png` | `final/final_chat_1440x900.png` |
| 1920×1080 | `final/final_home_1920x1080.png` | `final/final_chat_1920x1080.png` |

### 3.3 Key-state matrix (1280×800)

| File | State |
|---|---|
| `final/state_ready_1280x800.png` | Ready / healthy |
| `final/state_connecting_1280x800.png` | Connecting |
| `final/state_offline_1280x800.png` | Offline / degraded |
| `final/state_empty_lists_1280x800.png` | Empty lists |
| `final/state_populated_1280x800.png` | Populated lists |
| `final/state_selected_chat_1280x800.png` | Selected chat |
| `final/state_peer_online_1280x800.png` | Peer online |
| `final/state_peer_offline_1280x800.png` | Peer offline |
| `final/state_empty_chat_1280x800.png` | Empty chat |
| `final/state_long_chat_1280x800.png` | Long chat history |
| `final/state_message_ladder_1280x800.png` | Message grouping ladder |
| `final/state_composer_disabled_1280x800.png` | Disabled composer |

## 4. Regression and review records

| File | Content |
|---|---|
| `docs/chat-ui-regression-report.md` | Pre-redesign regression report |
| `docs/ui-redesign/ui-visual-qa-checklist.md` | Visual QA checklist for every capture |
| `docs/ui-redesign/scroll-behavior-investigation.md` | Scroll behaviour investigation |
| `docs/ui-redesign/home-cards-reactivity.md` | Home-rail lazy-reactivity design |

## 4a. File Sharing dashboard evidence (FS epic)

The File Sharing dashboard visual QA (FS-24) and its user/architecture docs:

| File | Content |
|---|---|
| `evidence/fs-24/t_f4f6f34d_file_sharing_1440x900.png` | Dashboard, wide viewport |
| `evidence/fs-24/t_f4f6f34d_file_sharing_1280x800.png` | Dashboard, reference viewport |
| `evidence/fs-24/t_f4f6f34d_file_sharing_1024x720.png` | Dashboard, narrow viewport |
| `evidence/fs-24/FS-24-handoff.md` | FS-24 visual QA handoff + checklist |
| `docs/file-sharing-guide.md` | File Sharing user guide |
| `docs/fs-06-persistence-projections.md` | Projection/subscription/persistence architecture |
| `docs/fs-25-release-note.md` | FS release note + rollback guidance |

## 5. How to regenerate

```sh
cargo build --features gui --example boru

# Baseline screenshots (UI-01 harness)
scripts/ui_baseline_screenshots.sh

# Final regression matrix (UI-21 harness)
scripts/ui21_final_evidence.sh    # writes docs/ui-redesign/evidence/final/
```

The harness uses loopback-only MCP (`--mcp --enable-gui-test-actions`) and
deterministic QA fixtures in temporary data directories; it never touches
production data and never alters release behaviour.
