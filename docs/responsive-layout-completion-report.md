# Boru responsive-layout completion report

This report is the BORU-RESP-15 Definition-of-Done gate for
`boru-responsive-layout-agent-plan.pdf`. It records the evidence available in
this checkout after BORU-RESP-01..14.

## Gate result

**PASS for the responsive-layout scope.** The implementation has one shared
`LayoutConfig` / `ResponsiveLayout` model, responsive boundary tests pass, and
the documented eight-viewport QA matrix reports no known P1/P2 responsive
defect. The full all-targets check also exposed one unrelated pre-existing test
fixture compile error; it is recorded below and was not changed by this gate.

## Definition-of-Done evidence

| DoD clause | Evidence | Result |
|---|---|---|
| One coherent extension of `LayoutConfig` / `ResponsiveLayout` | `src/bin/boru/layout.rs` owns the typed structural model and central tier resolution; `layout_merge.rs` applies overrides; screen modules consume the shared model. | PASS |
| Home behaviour preserved unless deliberately improved | `LayoutConfig::default()` and the documented baseline in `boru-layout.example.toml` are asserted equal in `layout_regression::matrix_parse_complete_config`; Home defaults, section order, content sizing and max width are covered by layout tests. Runtime Home captures are recorded in `docs/responsive-layout-qa.md`. | PASS |
| Major screens have narrow/desktop/ultra-wide behaviour | Shared layout values are wired through Home, Sidebar, Chat, Files, Tunnels, Discover, Settings, calls/screen-share and dialogs. The screen-specific source paths and runtime/automated evidence are listed in the QA matrix's Screen coverage table. | PASS (runtime capture where deterministic; automated/code-review evidence for seeded-state screens) |
| Short-height windows handled intentionally | Height-aware rules and dialog body caps are covered by the 1024x720, 1280x720 and 1366x768 fixtures; QA records scrolling/flow instead of clipped lower content. | PASS |
| TOML structural overrides work through live reload | `src/bin/boru/layout_watcher.rs` watches `boru-layout.toml`, parses on a background thread, and sends validated reload messages through the Iced update loop. `layout_config.rs`, `layout_merge.rs`, and `layout_regression.rs` cover parsing, merge, semantic validation, malformed input, missing files, and last-known-good behavior. | PASS |
| No duplicated breakpoint framework | Width and height tier resolution is centralized in `ResponsiveLayout`; `docs/responsive-layout-qa.md` explicitly records no second breakpoint framework or new view-local viewport threshold. | PASS |
| Usable at 1024x720 and high DPI | Home was captured at 1024x720; short-height tests pass. BORU-RESP-13 records logical-equivalent 100/125/150/175/200% scaling checks and the Xvfb limitation that physical compositor scaling was unavailable. | PASS with documented environment limitation |
| Ultra-wide max-width/centering | Home uses configured max content width and the 1440/1920/2560/3840 matrix records constrained content rather than indefinite stretching. Chat and media layout paths use configured width/contain safeguards. | PASS |
| Automated boundary tests and documented QA matrix committed | `src/bin/boru/layout_regression.rs` contains the responsive acceptance matrix; `docs/responsive-layout-regression.md` defines the eight fixtures and boundary intent; `docs/responsive-layout-qa.md` records viewport/screen results and limitations. | PASS |

## Delivery trace (PDF tasks 1–14)

The chain delivered the following areas without introducing a second layout
system:

- T1–T2: canonical typed layout model and responsive tier API in
  `src/bin/boru/layout.rs`.
- T3: Home and Sidebar structural layout/configuration wiring.
- T4: Chat width, composer, details-panel and media-flow responsiveness.
- T5: Files table/card/component placement and available-width safeguards.
- T6–T8: Tunnels, Discover, Settings, calls/screen-share and shared dialog
  sizing/scroll behavior.
- T9: height-aware short-window behavior.
- T10: expanded `boru-layout.toml` schema and example coverage.
- T11: remaining structural values routed through layout configuration.
- T12: regression matrix and boundary tests.
- T13: DPI/scaling validation record.
- T14: full-app viewport QA record in `docs/responsive-layout-qa.md`.

Relevant source/test anchors are `layout.rs`, `layout_config.rs`,
`layout_merge.rs`, `layout_watcher.rs`, `layout_regression.rs`,
`app/home.rs`, `app/sidebar.rs`, `app/chat.rs`, `app/files.rs`,
`app/tunnels.rs`, `app/discover.rs`, `app/settings.rs`, `app/dialogs.rs`,
and the calls/screen-share modules.

## Verification executed for this gate

Environment: DEBSRV (`172.16.0.59`), 32G free on `/` before verification; no
cleanup was required.

- `rb test --bin boru --features gui,video-playback,terminal -- layout_regression`
  — **12 passed, 0 failed** (1,523 filtered).
- `rb check --all-targets --features gui,video-playback,terminal` — **failed in
  an unrelated existing integration test compile path**:
  `tests/test_discovery_dm_isolation.rs:215,219` calls
  `DiscoveryService::join` with four arguments while the current API requires a
  fifth `SecretKey` argument. No responsive code was involved or changed.
- `cargo fmt --all -- --check` — **failed on pre-existing repository-wide
  formatting drift** across unrelated Rust files. `git diff --check` passes.
- `git fetch origin && git merge origin/main` — already up to date before the
  report change.

The upstream T14 QA record additionally reports successful desktop-target
verification with:

- `rb check --bin boru --features gui,video-playback,terminal` — **passed in this gate run** (warnings only).
- `rb build --bin boru --features gui,video-playback,terminal` — **passed in the upstream T14 verification** recorded in `docs/responsive-layout-qa.md`.

## Known limitations and out-of-scope items

The isolated Xvfb run had no peers, room tickets, file catalogue, tunnel or
media session. Therefore Chat, Files, Discover, Calls/screen-share and
populated dialog states have automated/source evidence rather than populated
runtime screenshots. The QA document calls this out explicitly and identifies
seeded-peer desktop capture as a useful follow-up, not a discovered P1/P2
responsive defect. Physical OS compositor scaling was unavailable in Xvfb;
logical-equivalent viewport and scaling safeguards are documented instead.

The unrelated `DiscoveryService::join` integration-test fixture mismatch and
repository-wide formatting drift are not part of responsive-layout scope and
were not modified.
