Boru Home redesign verification — UI-08 / t_0da77b2a

Checkout and integration

- Checkout: /home/dan/iroh-gossip-chat/.worktrees/t_0da77b2a
- Branch: wt/t_0da77b2a
- Parent UI-06 branch wt/t_e33312e9 was merged fast-forward to 1d4e4c22.
- Working tree was clean before this evidence file.
- Compared scope: git diff origin/main..HEAD --stat reports 9 files, all Home redesign files plus the parent audit artifact.
- `git diff --check origin/main..HEAD`: PASS (no whitespace errors).
- `cargo metadata --no-deps --format-version 1`: PASS.

Focused source/test review

- Home presentation helper tests are present in `src/bin/boru/home_visual.rs` (3 tests) and `src/bin/boru/app/home.rs` (4 tests).
- People helper tests are present in `src/bin/boru/app/home_people_activity.rs`, including UTF-8-safe long-name elision.
- Source review confirmed the Recent Activity `View all` action routes to `AppMessage::OpenActivityLog`, friend rows retain separate profile/message actions, and the responsive thresholds remain content-width based.
- No scoped regression was found requiring a code fix.

Commands attempted

- `cargo fmt --all -- --check` — BLOCKED/FAIL: repository-wide pre-existing formatting differences were reported in unrelated files including `benches/compression_bench.rs`, `src/bin/boru/app/calls.rs`, `src/bin/boru/app/chat.rs`, and many tests. No formatter changes were applied.
- `rb check --bin boru --features gui,video-playback,terminal` — NOT RUN: rb rsync failed before Cargo with `No space left on device (28)` on DEBSRV `/home/dan/boru-build/work-0/.git`.
- `rb test --bin boru --features gui,video-playback,terminal -- home_visual home::tests` — NOT RUN: same DEBSRV rsync capacity failure.

Visual matrix

- 1891x1014, 1440x900, 1280x800, 1024x768: NOT TESTED in this worker; no local GUI binary was available and remote build was blocked by DEBSRV capacity.
- 100%, 125%, 150% scale: NOT TESTED.
- Light/dark themes and alternate accents: NOT TESTED at runtime. Source-level theme/accent routing remains covered by existing helper tests, but those tests were not executable in this worker.
- Zero/one/many friends, long IDs/missing avatars, empty/busy activity, and empty/active/loading/error tunnels: NOT TESTED visually.
- Pointer actions and keyboard traversal: NOT TESTED interactively.
- Existing protected hero/logo/sidebar/connection-card regions were not changed by this worker; no visual claim is made without captures.

Environment limitation

DEBSRV `/home/dan` is at 100% usage. The largest build target directories include `work-target-3` (122G), `work-target-2` (62G), `work-target-0` (53G), and `work-target-1` (48G). Cleanup was not performed because these are shared build artifacts and ownership/use could not be established safely.

Result: source review and lightweight metadata/diff checks passed; compile, focused test, and visual matrix evidence remain unavailable until DEBSRV storage is reclaimed and a local GUI executable can be captured.
