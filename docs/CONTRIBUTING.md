# Contributing to Boru

## Commit message conventions

Boru uses **conventional commits** for version management.

### Format

```
<type>(<optional scope>): <description>
```

Types that trigger a version bump:

| Type       | Version bump | Description                     |
|------------|-------------|---------------------------------|
| `feat`     | minor       | A new feature                   |
| `fix`      | patch       | A bug fix                       |
| `perf`     | patch       | Performance improvement         |
| `refactor` | patch       | Code restructuring              |
| `revert`   | patch       | Reverting a previous change     |

Types that do **not** trigger a version bump:

| Type     | Description                           |
|----------|---------------------------------------|
| `docs`   | Documentation only                    |
| `style`  | Formatting, missing semicolons, etc.  |
| `test`   | Adding or fixing tests                |
| `ci`     | CI configuration or scripts           |
| `chore`  | Build process, dependencies, tooling  |
| `build`  | Changes affecting the build system    |

### Breaking changes

Add a `!` before the `:` to mark a breaking change:

```
feat!: replace the local storage format
```

or with a scope:

```
feat(storage)!: replace the local storage format
```

You can also use `BREAKING CHANGE:` in the commit body:

```
feat(storage): replace storage format

BREAKING CHANGE: Existing databases require migration.
```

Breaking changes below `1.0.0` result in a **minor** version bump.

### Useful scopes (optional)

```
chat
notifications
delivery
contacts
network
storage
ui
android
desktop
security
```

### Examples

```
fix(chat): prevent duplicate messages
feat(notifications): add local notifications
docs: update installation instructions
test(delivery): add retry tests
refactor(contacts): simplify contact storage
perf(network): reduce gossip message overhead
ci: update workflow configuration
chore: update development dependencies
```

### Version bumps below 1.0.0

Because Boru is below `1.0.0`:

| Change      | Bump       |
|-------------|------------|
| `fix:`      | `0.N.0 → 0.N.1` (patch)    |
| `feat:`     | `0.N.0 → 0.N+1.0` (minor)  |
| breaking    | `0.N.0 → 0.N+1.0` (minor)  |

After reaching `1.0.0`:

| Change      | Bump       |
|-------------|------------|
| `fix:`      | `1.N.0 → 1.N.1` (patch)    |
| `feat:`     | `1.N.0 → 1.N+1.0` (minor)  |
| breaking    | `1.N.0 → 2.0.0` (major)    |

## Pull request titles

Since the project uses **squash merging**, the pull request title is the
authoritative versioning input.

PR titles must follow the conventional commit format:

```
<type>(<optional scope>): <description>
```

The PR title is validated by CI (`commit.yaml`).

### How to fix an invalid PR title

If CI rejects your PR title, edit it through the GitHub UI (not by pushing
more commits). Use the correct type prefix from the table above.

## Pull request checklist

Before opening a PR, confirm:

- [ ] `cargo fmt --check`, `cargo check`, `cargo clippy`, and the relevant
      tests pass.
- [ ] If the change alters **architecture, protocol, or persistence
      behavior** (module layout, schema/migration, wire format, storage
      source of truth, security invariants), the corresponding docs are
      updated **in the same change** — `ARCHITECTURE.md`,
      `docs/message-storage-design.md`, `docs/security-model.md`,
      `docs/protocol-layers.md`, or the affected feature doc.
- [ ] If the SQLite schema version (`CURRENT_SCHEMA_VERSION` in
      `src/storage.rs`) changed, `docs/message-storage-design.md` and any
      other schema references were updated; the
      `docs_reference_current_schema_version` test enforces this.
- [ ] Exact, volatile metrics (file/line counts, byte sizes) are not added
      to long-lived docs — use qualitative descriptions or generated output.
- [ ] If this PR touches **screen sharing** (the `screen-sharing` cargo
      feature or `src/screen_share/`): **no RustDesk (AGPL-3.0) code was
      copied** — no copied source, no line-for-line translations or
      mechanical ports, no copied comments/tests/constants, and no GPL/AGPL
      dependency added to the compiled graph. The licence gate
      (`./scripts/check-licenses.sh`, CI `cargo_deny` job) must pass; adding
      a copyleft dependency requires a reviewed
      `[[licenses.exceptions]]` entry in `deny.toml`. Every implementation
      decision cites an independent source (platform API docs, official
      specifications, permissively licensed libraries) in the PR
      description. See `docs/screenshare-rustdesk-reference-policy.md` and
      `THIRD_PARTY_NOTICES.md`.

## What to commit

Agent-generated kanban artifacts (task audits, reports, closeouts, evidence
screenshots/logs under `docs/**/evidence/`, task-token docs like
`CONN-*`/`EPIC-*`/`UI-HOME-*`/`PAPIRUS-*`/`KLIPY-*`/`FS-*`, `WORKSPACES.md`,
`*.log`, `*.patch`, helper scripts, `report.html`) are **not** committed to
this repository. They stay in the working tree/kanban workspace and are
gitignored (see the "Agent-generated kanban artifacts" block in `.gitignore`).

Commit only the actual deliverable: source, config, tests, and real project
documentation under `docs/`. If a piece of agent documentation genuinely
belongs in the repo, it must be a polished project doc under `docs/` — not a
task artifact at the repo root.

## Squash merging

Boru uses squash merging to keep the git history linear.

### Required GitHub settings

These must be configured in the repository settings:

1. Open the repository **Settings**.
2. Open **General**.
3. Find **Pull Requests**.
4. Enable **Allow squash merging**.
5. Set **Default commit message** to **Pull request title** (so the
   squash commit uses the PR title, which is the authoritative versioning
   input).

## Version management

Boru uses a `.version-state.json` file to track which commit the current
version corresponds to. The authoritative application version is in
`Cargo.toml` (`[package] version`).

### Version calculation

`scripts/version.py` inspects conventional commits since the last recorded
version change and determines the next semantic version.

### Initialise version state

After cloning or when adding the system to an existing repo:

```bash
python scripts/version.py initialise
```

This records the current version and commit without incrementing.

### Check proposed version

```bash
python scripts/version.py check
```

Reports the proposed version without modifying files.

### Apply a version locally

```bash
python scripts/version.py apply --dry-run   # preview
python scripts/version.py apply             # update Cargo.toml and state
```

After applying, review the diff and commit:

```bash
git diff
git add Cargo.toml .version-state.json
git commit -m "chore: bump Boru version to X.Y.Z"
```

### GitHub Actions workflows

| Workflow | Trigger | Effect |
|----------|---------|--------|
| `version-check.yml` | PRs to main, manual | Read-only check, shows proposed version in workflow summary |
| `apply-version.yml` | Manual (`workflow_dispatch`) | Creates a version-bump branch and PR for review |
| `commit.yaml` | PRs to main | Validates PR title format |

### What the workflows do NOT do

- No GitHub Releases are automatically created.
- No Git tags are automatically created.
- No packages, crates, binaries, or installers are published.
- No automatic commits are pushed to the default branch.

## Development setup

See `docs/build-release.md` for build and release instructions.
