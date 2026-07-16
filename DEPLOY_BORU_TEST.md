# Boru Test Deployment Trigger

## Overview

Every push to `main` on `github.com/dmahony/iroh-gossip-chat` automatically triggers
`deploy-boru-test.sh` on the self-hosted runner `dell7070-boru-test` (this machine).

## Files

| File | Purpose |
|------|---------|
| `.github/workflows/boru-test-deploy.yaml` | GitHub Actions workflow definition |
| `deploy-boru-test.sh` | Build and smoke-test script |
| `start-testing.sh` | Manual build/deploy/start entrypoint |
| `scripts/boru-test-instance.sh` | Remote lifecycle supervisor for one VM instance |
| `.gitignore` | Ignores `.deploy-logs/` directory |
| `DEPLOY_BORU_TEST.md` | This file |

## Trigger

- **Event:** `push` to `main`
- **Runner labels:** `self-hosted`, `linux`, `X64`, `boru-test`
- **Workflow:** `Boru Test Deploy` (run #1 completed successfully)

## Prerequisites

1. Rust toolchain installed on the self-hosted runner (`cargo`, `rustc` on PATH)
2. Self-hosted runner registered with repo and running as systemd service
3. Runner must have network access to GitHub and cargo registry

## What the Script Does

1. Verifies it is at the repo root (`Cargo.toml` present)
2. Records `rustc` and `cargo` versions
3. Builds `lan_test` example (debug)
4. Builds `iced_chat` example with `--features gui` (debug)
5. Runs `cargo test --lib` (library tests only, 4 threads)
6. Writes timestamped logs to `.deploy-logs/`

## VM lifecycle and isolation

Both launchers copy `scripts/boru-test-instance.sh` to `~/boru-test` and use it
as a supervisor. Each run gets a unique directory under
`~/boru-test/runs/<timestamp>-<launcher-pid>/node-{54,55}`; existing
`~/boru-chat-data-*` directories are not touched. The supervisor records the
single process-group leader in `~/boru-test/instance.pid`, keeps `xvfb-run` in
the foreground so it can reap Xvfb, and binds MCP to the VM loopback address.
Starting again is idempotent: the recorded process group is stopped before a
new one is created. Displays :98 and :99 (and reserved recovery range :98-:127)
are dedicated to these tests; stale Xvfb processes in that range are removed,
not arbitrary user X servers.

## Safe stop and verification

```bash
ssh dan@172.16.0.54 '~/boru-test/boru-test-instance.sh stop'
ssh dan@172.16.0.55 '~/boru-test/boru-test-instance.sh stop'
ssh dan@172.16.0.54 '~/boru-test/boru-test-instance.sh status 8765'
ssh dan@172.16.0.55 '~/boru-test/boru-test-instance.sh status 8766'
```

For an identity check without exposing key contents, hash the key file on the
VM (if present):

```bash
ssh dan@172.16.0.54 'sha256sum ~/boru-test/runs/*/node-54/secret-key 2>/dev/null || true'
ssh dan@172.16.0.55 'sha256sum ~/boru-test/runs/*/node-55/secret-key 2>/dev/null || true'
```

Only delete confirmed temporary run directories, for example
`~/boru-test/runs/<id>`, after stopping the instance. Do not remove the legacy
`~/boru-chat-data-*` directories unless their owner explicitly confirms they
are disposable test data.

## Logs and Monitoring

- Local logs: `/home/dan/actions-runner/_work/iroh-gossip-chat/iroh-gossip-chat/.deploy-logs/`
- GitHub Actions UI: https://github.com/dmahony/iroh-gossip-chat/actions
- Runner service: `systemctl status actions.runner.dmahony-iroh-gossip-chat.dell7070-boru-test`

## Observed Result

- First push (commit `994ceee7`) triggered run #1
- Runner picked up the job at 00:39:17 UTC
- Build and test completed at 00:46:50 UTC
- GitHub API reported: `status: completed`, `conclusion: success`

## Failure Handling

- If `deploy-boru-test.sh` exits non-zero, the workflow step fails
- GitHub Actions marks the run as failed and shows red in the UI
- On failure, the workflow uploads `.deploy-logs/` as an artifact

## Security Notes

- No credentials are hard-coded in the workflow or script
- The runner uses the GitHub-provided short-lived token for checkout
- `.deploy-logs/` is `.gitignore`d so logs never leak into the repo
