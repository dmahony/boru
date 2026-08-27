# BORU 0.227.1 packaged Linux release gate

> Final cross-platform synthesis: [`docs/boru-next-05-release-readiness.md`](boru-next-05-release-readiness.md).

Date: 2026-08-27
Source: `0ba8b156acf53733c197d61742ccfe07967d69bd`
Decision: **NOT RELEASE-READY**

This gate built the Linux release artifact on DEBSRV from the required parent
branch, packaged the runtime assets, generated and validated the SPDX SBOM,
verified checksums, smoke-tested the executable, and ran the bounded
real-process soak. No tag, release, signing key, provenance attestation, or
publication was created. The final synthesis also records the focused routing,
room-membership, persistence, and visible-UI audit and its remaining blockers.

## Results

| Gate | Result | Evidence |
|---|---|---|
| DEBSRV release build | PASS | `rb build --offline --release --bin boru --features gui,video-playback`; exit 0; 56,251,128-byte ELF |
| Package assembly | PASS | `boru-linux-x86_64.tar.gz`; binary, Papirus assets, Twemoji assets, and `THIRD_PARTY_NOTICES.md` present |
| Artifact smoke | PASS | `timeout 20s ./boru --help`; exit 0; CLI help rendered |
| SPDX SBOM | PASS | `generate-spdx-sbom.py`; SPDX-2.3 document with 964 resolved Cargo packages |
| Feature/version validators | PASS | `release-validate.py v0.227.1`, `check-release-feature-matrix.py`, Python compilation, and `bash -n` |
| Checksums | PASS | `release-checksums.sh`; strict verification of packaged tarball, SBOM, and metadata |
| Provenance subject validation | PASS (local) | `artifacts/release-gate/provenance.json` records source commit, target/features, and subject SHA-256 values |
| External provenance attestation | NOT RUN | GitHub Actions attestation requires a publication context; no release was published per task scope |
| Real-process bounded soak | PASS | 3 isolated release processes, `no-dht`, 2 seconds, cleanup verified, no failures |
| Soak harness self-test | PASS | `python3 scripts/soak_harness.py --self-test` |
| Parent regression check | PASS | `rb check --test test_discovery_dm_isolation --features gui,video-playback,terminal` |

## Blockers and classification

1. **Environment / required gate not run:** external GitHub artifact provenance
   attestation was not executed because this task must not publish or create a
   release. This is an explicit limitation, not a pass claim.
2. **Baseline:** the release build emitted 340 existing warnings; the build
   still exited successfully. No warning cleanup was included in this gate.
3. The initial `--locked` DEBSRV invocation failed because Cargo requested a
   lockfile update despite the checked-in lockfile. The required release build
   was rerun with `--offline` as Cargo recommended and passed; no repository
   lockfile change resulted.
4. The seeded-peer fixture did not expose conversation deletion, so deletion
   persistence remains an explicitly unvalidated end-to-end scenario.

Because the external attestation gate was not run, this report makes no
release-ready claim.

Machine-readable evidence is in `artifacts/release-gate/evidence.json`; local
artifact files and soak logs are intentionally untracked and contain no user
credentials, keys, tickets, or message bodies.
