# Deterministic reliability stress

`tests/reliability_stress.rs` is a bounded, isolated multi-node state-machine
stress suite. It models four CI nodes and three topics by default, generates
bidirectional direct sends from a seeded `ChaCha12Rng`, and injects disconnect,
reconnect, and same-identity restart operations. Offline deliveries are retained
until reconnect; each node's received set is the dedupe boundary.

## CI-safe run

From the repository root (compile/test work runs on DEBSRV):

```sh
BORU_RELIABILITY_SEED=2963756153 scripts/reliability-soak.sh short
```

The test prints a compact metrics line containing the seed, operation counts,
reconnect/restart counts, pending-queue high water mark, elapsed time, and a
trace digest. Failures name the seed and operation index; no message bodies or
addresses are logged. When running the test directly with Cargo, set
`BORU_RELIABILITY_ARTIFACT=artifacts/reliability.json` to write the same bounded
report as JSON; the `rb` wrapper's authoritative output is the metrics line.

## Long manual soak

The long variant uses six nodes, five topics, and 8,000 operations. It is
ignored by normal CI and has a 15-minute timeout in the wrapper:

```sh
BORU_RELIABILITY_SEED=2963756153 scripts/reliability-soak.sh soak
```

Use a different seed for another deterministic run, or rerun the exact seed
from a failure. The runner creates no node processes or `/tmp` state.
