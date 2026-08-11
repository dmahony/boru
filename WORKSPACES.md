# Boru Inline Video Player Redesign — Workspaces

This file records the isolated worktrees provisioned for the PDF plan.
Each workspace is self-contained and should be used from its own directory.
For DEBSRV verification, run `rb` from the workspace root so slot affinity stays stable.

Shared DEBSRV access details:
- Host: 172.16.0.59
- SSH user: dan
- Wrapper: `~/bin/rb`
- Build command: `rb check --bin boru --features gui,video-playback,terminal`

Workspace shell setup:
- Source `workspace.env` from the workspace root.
- `workspace.env` exports the task id, branch, workspace path, and DEBSRV connection values.

| Task | Role | Workspace | Branch | Env file | Notes |
|---|---|---|---|---|---|
| `t_0ba0f2f7` | ops | `/home/dan/iroh-gossip-chat/.worktrees/t_0ba0f2f7` | `wt/t_0ba0f2f7` | `/home/dan/iroh-gossip-chat/.worktrees/t_0ba0f2f7/workspace.env` | Provision isolated workspaces for planned tasks; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_5ce7f106` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_5ce7f106` | `wt/t_5ce7f106` | `/home/dan/iroh-gossip-chat/.worktrees/t_5ce7f106/workspace.env` | BORU-PLAYER-01: Inspect existing video player; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_495928ec` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_495928ec` | `wt/t_495928ec` | `/home/dan/iroh-gossip-chat/.worktrees/t_495928ec/workspace.env` | BORU-PLAYER-02: Core control overlay redesign; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_7035cce2` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_7035cce2` | `wt/t_7035cce2` | `/home/dan/iroh-gossip-chat/.worktrees/t_7035cce2/workspace.env` | BORU-PLAYER-03: Responsive vertical/landscape/square; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_42a92ef0` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_42a92ef0` | `wt/t_42a92ef0` | `/home/dan/iroh-gossip-chat/.worktrees/t_42a92ef0/workspace.env` | BORU-PLAYER-04: Auto-hide + click-to-toggle + centre play overlay; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_c8306f52` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_c8306f52` | `wt/t_c8306f52` | `/home/dan/iroh-gossip-chat/.worktrees/t_c8306f52/workspace.env` | BORU-PLAYER-05: More menu + fullscreen (conditional); rb slot affinity by cwd; no overlap with other active workspaces |
| `t_a549adc2` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_a549adc2` | `wt/t_a549adc2` | `/home/dan/iroh-gossip-chat/.worktrees/t_a549adc2/workspace.env` | BORU-PLAYER-06: Preserve geometry + player container; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_6e00b393` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_6e00b393` | `wt/t_6e00b393` | `/home/dan/iroh-gossip-chat/.worktrees/t_6e00b393/workspace.env` | BORU-PLAYER-07: Keyboard controls + accessibility; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_678b88b5` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_678b88b5` | `wt/t_678b88b5` | `/home/dan/iroh-gossip-chat/.worktrees/t_678b88b5/workspace.env` | BORU-PLAYER-08: Test all video shapes; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_10938ae4` | linux | `/home/dan/iroh-gossip-chat/.worktrees/t_10938ae4` | `wt/t_10938ae4` | `/home/dan/iroh-gossip-chat/.worktrees/t_10938ae4/workspace.env` | BORU-PLAYER-09: Functional regression + screenshots; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_7511ae14` | debsrv | `/home/dan/iroh-gossip-chat/.worktrees/t_7511ae14` | `wt/t_7511ae14` | `/home/dan/iroh-gossip-chat/.worktrees/t_7511ae14/workspace.env` | Execute the build(s) on DEBSRV; rb slot affinity by cwd; no overlap with other active workspaces |
| `t_d5b10adf` | reviewer | `/home/dan/iroh-gossip-chat/.worktrees/t_d5b10adf` | `wt/t_d5b10adf` | `/home/dan/iroh-gossip-chat/.worktrees/t_d5b10adf/workspace.env` | Verify artifacts against the PDF; rb slot affinity by cwd; no overlap with other active workspaces |

## Usage

```bash
cd /home/dan/iroh-gossip-chat/.worktrees/t_5ce7f106
source workspace.env
rb check --bin boru --features gui,video-playback,terminal
```

## Notes

- The worktrees are independent git branches named `wt/<task_id>`.
- The build/review tasks are listed here so downstream workers can locate their dedicated worktrees quickly.
- If a workspace needs a fresh shell, re-source its `workspace.env` before running `rb` or SSH commands.
