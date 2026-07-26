# Changelog

All notable changes to Boru are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The authoritative version source is `Cargo.toml`. For full change history,
see the [git log](https://github.com/dmahony/boru/commits/main/).

## 0.102.0

Performance and transfer improvements from Phase 25, including improved file
indexing, hashing, outbox delivery, image optimization, persistence, and
release build profiles.

## 0.101.0

Latest release. See the git log for the full commit history.

### How changelog entries are added

Changelog entries are updated as part of the version-bump pull request.
Before merging a version bump, run:

```bash
git cliff --prepend CHANGELOG.md --tag v<new-version> --unreleased
```

This groups changes into:

- Added (features)
- Fixed (bug fixes)
- Performance
- Breaking changes

Internal changes (formatting, CI maintenance, dependency bumps) are skipped
unless they are user-facing.
