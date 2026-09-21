# Disposable companion protocol probe

The probe is an independent client. It generates its own Iroh key and stores only non-secret registration metadata in `BORU_PROBE_STATE` (default: the system temporary directory). It never accepts invitation material on the command line.

Build on DEBSRV:

    rb build --features examples --example companion_probe

Run with a protected invitation file:

    chmod 600 invitation.txt
    BORU_PROBE_STATE=/tmp/boru-probe-a.json rb run --features examples --example companion_probe -- --invitation-file invitation.txt

Or pipe the invitation through protected stdin:

    cat invitation.txt | BORU_PROBE_STATE=/tmp/boru-probe-b.json rb run --features examples --example companion_probe

The probe exercises the genuine `/boru/companion/1` handler in order: `Hello`, `PairBegin`, `PairStatus`, and authenticated `HostStatus`. Run a second process with a different state path to exercise a competing claimant; only the first endpoint claim is accepted. `--disconnect-after` closes immediately after the exchange, and rerunning with the same state path exercises reconnect/restart persistence. `--delay-ms N` inserts a controlled response delay.

Local approval remains a desktop-side action. The probe reports `PairStatus=pending` until the desktop approves the claim; after approval, rerun it with the persisted registration metadata and the host status request should authenticate. Revocation should produce `ApprovalRequired`/`StaleGrant` and close the tracked host session.

The invitation file and state files should be removed after a smoke run. Never place invitation material in shell history, process arguments, or logs.

## DEBSRV verification evidence

Executed from the worktree with `/home/dan/bin/rb`:

    rb check --features examples --example companion_probe
    rb test --features examples companion_protocol::tests --lib
    rb run --features examples --example companion_probe -- --help

Results: the example check passed; all 6 companion protocol tests passed; and the probe help command exited successfully. The build emitted only pre-existing repository warnings (unused imports/private interface/dead code).
