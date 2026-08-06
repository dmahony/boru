# UI-HOME-followup evidence (task t_8834836b)

Follow-up from UI-HOME-14 (t_5c7a2325) and the UI-HOME-19 release gate
(t_61a56729): remove tracked-but-dead modules and adopt TypeRole in the two
remaining views that still called `source_sans` directly.

## Changes (commit 851198ad, pushed to origin/main)

1. **Removed orphaned modules** (`git rm`): `examples/iced_chat/dashboard.rs`,
   `examples/iced_chat/file_library.rs`, `examples/iced_chat/invitation_qr.rs`.
   All three were tracked but had **no `mod` declaration** in main.rs and **zero
   references** anywhere in examples/, src/, tests, or scripts (verified by
   grep for `dashboard::`, `file_library::`, `invitation_qr::` and for the
   modules' exported symbols). Uncompiled dead code since the UI redesign
   (last touched by commit 2702f0cc / 39dfdf98, months ago).

2. **TypeRole adoption**:
   - `connection_details.rs`: `source_sans(...)` / `jetbrains_mono(...)` calls
     replaced with `type_role_text(TypeRole::...)` /
     `TypeRole::TechnicalValue.font()` (SupportingText for status messages,
     ButtonLabel for labels/buttons, PageTitle for dialog title, Metadata for
     announcement). Dropped now-unused `Weight`, `text`, `TYPO_*` imports.
   - `download_progress_view.rs`: same adoption for state badge (Metadata),
     action/text buttons (ButtonLabel), filename (ButtonLabel), metadata rows
     (Metadata), failure block (BodyEmphasised title + Metadata + TechnicalValue
     diagnostics), pct label (BodyEmphasised), bytes label (Metadata). Dropped
     unused `Weight` import.

3. **Cargo.lock**: sync boru-core 0.117.1 (the version-bump commit e9f24bd4
   updated Cargo.toml only; build regenerated the lockfile).

## Verification (fresh, at HEAD 851198ad)

- `cargo build --example boru --features gui` → **BUILD_EXIT=0** (Finished dev
  profile; only pre-existing warnings remain, none in the two migrated files).
- `cargo test --example boru --features gui` → **896 passed / 0 failed /
  0 ignored** (64.67s) — identical to the UI-HOME-19 gate baseline.

## Files

- `gate_build_test.log` — combined build (exit 0) + test (896/0) output.
