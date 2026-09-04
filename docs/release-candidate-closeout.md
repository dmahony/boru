# Release-candidate close-out — Boru 0.231.0

Candidate: `ad0044a7c3a14b05a7928b9d5fdbf23eaef5b33e`

This close-out is intentionally **NOT RELEASE-READY**. The candidate compiles in the shipping GUI feature set and in the supported core-only (`net,metrics`) shape, but the architecture boundary gate found 17 domain modules still importing `use super::*`. The required explicit-import cleanup is tracked by `t_ff8c3af0`; no release promotion should happen until it passes.

The locale checker now validates JSON syntax, English/locale key sets, and interpolation placeholders. `en.json` was corrected to include `gallery.delete`; `fr.json` remains a supported partial locale and falls back to English for 127 keys. This is recorded as a finding rather than silently presenting fallback text as a complete translation.

Verification evidence is machine-readable in `docs/release-candidate-gate.json` and `docs/architecture-refactor/architecture-gate.json`. Heavy Cargo checks were run through `rb` on DEBSRV:

- `rb check --locked --bin boru --features gui,video-playback,terminal`: pass (existing warning debt)
- `rb check --locked --no-default-features --features net,metrics`: pass (5 existing warnings)
- `rb test --locked --lib -- file_transfer_protocol`: pass, but the filter matched zero tests because the tests are not name-qualified by module
- full `rb test --locked --lib`: timed out at the tool limit; it is incomplete evidence, not a pass
- `git diff --check`: pass

Rollback: retain the prior signed release and do not promote this candidate. Re-run the full locked test gate and the architecture gate after `t_ff8c3af0` completes.
