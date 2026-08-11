# EPIC-FONTS / Boru typography migration — decomposition report

Task: t_15c72313 — implement Boru_FONTS.txt as kanban steps
Board: iroh-gossip-chat
Spec: /home/dan/.hermes/kanban/boards/iroh-gossip-chat/attachments/t_15c72313/Boru_FONTS

## Outcome

The Boru_FONTS.txt spec (17 tasks + acceptance criteria) was decomposed into an
18-card kanban graph, completed by run 4071 of this task. All cards are
assigned to the `orchestrator` profile with separate git worktree workspaces
(so multiple cards run concurrently without stomping the shared canonical
tree), force-load the `iroh-gossip-chat-workflows` skill (which mandates
debsrv/`rb` remote builds), and every implementation card body carries the
canonical `rb check --bin boru --features gui,video-playback,terminal`
build instruction so compilation happens on debsrv (172.16.0.59), never the
local 6-core i5.

## Card graph (surviving set, 18 cards)

- EPIC-FONTS (t_8e4fc95b): final close-out + report; parents FONTS-13/15/17
- FONTS-01 (t_8dfc72dd): Audit existing typography — RUNNING (re-dispatched
  after orphan-worker cleanup; worker pid 1705435)
- FONTS-02 (t_b187bb90): Bundle Archivo SemiCondensed Bold — todo
- FONTS-03 (t_7db937fe): Bundle IBM Plex Sans — todo
- FONTS-04 (t_73b8a214): Update semantic typography system (TypeRole) — todo
- FONTS-05 (t_5635b8dd): Home screen typography — todo
- FONTS-06 (t_7a4a1694): Sidebar typography — todo
- FONTS-07 (t_c5c89e02): Quick-action cards typography — todo
- FONTS-08 (t_a43d7e21): Keep Figtree for chat messages — todo
- FONTS-09 (t_7ede6d76): Keep JetBrains Mono for technical values — todo
- FONTS-10 (t_c2259c6a): File sharing screens typography — todo
- FONTS-11 (t_bfdadec3): Creation dialogs typography — todo
- FONTS-12 (t_a237676c): Remove old Source Sans 3 and Manrope — todo
- FONTS-13 (t_f8bea160): Font loading and packaging verification — todo
- FONTS-14 (t_2d2204a7): Fallback fonts — todo
- FONTS-15 (t_ae0dc03d): Recheck layout after font metrics change — todo
- FONTS-16 (t_c4a492fd): Typography sizes baseline — todo
- FONTS-17 (t_fd89da28): Visual QA — todo

Dependency links: 01 → 02 → 03 → 04; 04 fans out to 05..14,16; those converge
on 12; 12 → 15,17; 13/15/17 → EPIC-FONTS.

## Duplicate-worker incident (run 4070 vs 4071)

This task was originally spawned as run 4070. That run was blocked→unblocked
while its process stayed alive (block does not kill the worker PID), so the
dispatcher spawned a replacement worker (run 4071). Both workers decomposed
the spec independently. Run 4071 completed first: it created the 18-card set
above and archived the duplicate 18-card set created by run 4070 (all 18
archived — t_4ded971e .. t_47bd8fee). During the overlap, run 4070's process
killed the replacement's in-flight FONTS-01 worker; the card was reclaimed and
re-dispatched (now running, pid 1705435). Final board state is coherent: one
canonical 18-card set, no duplicates, no dead-worker running tasks.

## Repo state

- Canonical repo /home/dan/iroh-gossip-chat: clean (only unrelated untracked
  docs/file-type-icons/PAPIRUS-22-evidence/), HEAD 590bd110.
- No source changes made by the decomposition itself (analysis/routing only).
