F03 confirmed defect fix: offline mailbox restart persistence

Date: 2026-09-07
Source SHA before fix: d4d5feafc456edd50974a3a420800e434c8ff4c5
Fix commit: 16e2a66a

Original evidence

- Windows-Windows artifact: /home/dan/iroh-gossip-chat/.worktrees/t_9c71da71/windows-windows-evidence.json
- Original result: direct room join and bidirectional chat PASS; offline_delivery_after_restart FAIL.
- Reproduction: stop peer B, queue a whisper from A, restart B with the same data directory, and inspect mailbox/message telemetry. The queued row was not available for reconnect delivery.
- Root cause: the runtime used MailboxStore::enqueue_outgoing(), but MailboxStore::save() was a no-op. The reconnect path loads mailbox.json, so an offline envelope existed only in memory and disappeared at process exit.

Focused fix

- MailboxStore::save() now expires stale entries and writes an atomic JSON snapshot using the repository atomic_write_json helper.
- The outgoing whisper path persists the envelope before attempting direct QUIC delivery and reports a persistence error instead of silently continuing.
- Incoming acceptance and outgoing acknowledgement persist their updated mailbox state as well, preserving duplicate/retry behavior across a restart.
- Added mailbox::tests::outgoing_envelope_survives_restart, which creates an encrypted envelope, saves it, reloads from a fresh store, and decrypts the retained envelope.

Post-fix verification

- /home/dan/bin/rb test --lib -- mailbox::tests::outgoing_envelope_survives_restart: PASS (1 passed, 0 failed).
- /home/dan/bin/rb test --lib -- mailbox::tests: PASS (19 passed, 0 failed).
- /home/dan/bin/rb check --bin boru --features gui,terminal,voice-calls,video-calls,screen-sharing: PASS (exit 0; existing warnings only).
- git diff --check: PASS before commit.
- git fetch origin && git merge origin/main: already up to date before commit.

Affected matrix status

| Row | Package/startup | Direct chat | Offline restart | Other capabilities | Status |
|---|---|---|---|---|---|
| Linux-Linux | PASS | PASS in latest explicit-bootstrap rerun | Not rerun after fix | Files, progress, calls, screen share, permissions, ticket/name, tunnel not verified | INCOMPLETE |
| Windows-Windows | PASS | PASS | Original FAIL; source fix verified by focused test, packaged rerun pending | Relay unavailable; files, calls, screen share, permissions, ticket/name, tunnel not run | INCOMPLETE |
| Windows-Linux | PASS startup | Blocked by prior cross-platform admission failure | Blocked | All application checks blocked | INCOMPLETE |

The original failing artifacts are preserved in their parent worktrees. A clean packaged Windows-Windows rerun is still required to turn the focused source-level regression test into runtime acceptance; relay and the capability rows explicitly remain unverified rather than being claimed as passes.
