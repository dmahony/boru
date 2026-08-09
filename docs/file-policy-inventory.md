# File / Path / Permission Policy Inventory (BORU-AUDIT-20)

**Task:** Retire duplicated deprecated profile/file policy logic.
**Date:** 2026-08-10
**Source of truth:** this document + the canonical modules it names.

The goal of BORU-AUDIT-20 is that file validation, path policy and transfer
authorization exist in **one canonical implementation** per rule, so a
security fix has exactly one authoritative location.  This page is the
inventory of every rule, the modules that used to implement it, and where
policy is changed today.

---

## 1. Canonical policy modules (change policy HERE)

| Module | Rule it owns | Public entry points |
|---|---|---|
| `src/file_policy.rs` | Size ceiling + extension allowlist admission ("may this file be shared?") | `file_policy::admission(size, extension, max_file_size, allowed_extensions)` → `FileAdmission { over_limit, extension_blocked }` |
| `src/path_containment.rs` | Path containment + symlink escape | `path_containment::is_path_contained(path, root)`, `path_containment::symlink_is_safe(path, root)`, `path_containment::canonicalize_allow_missing(path)` |
| `src/safe_destination.rs` | Download-destination filename sanitisation (separators, traversal, drive letters, reserved names, dedup) | `safe_destination::safe_destination_path(download_dir, display_name, fallback_stem)`, `safe_destination::prepare_download_destination(...)`, `safe_destination::resolve_destination_with_policy(...)` |
| `src/collection_transfer.rs` | Collection path-component gate (single relative components) | `collection_transfer::validate_path_component(component)`, `collection_transfer::canonicalized_path_to_string(path)` |
| `src/video_playback.rs` | Attachment filename gate for inline video | `video_playback::validate_attachment_filename(name)`, `video_playback::verify_local_attachment(...)` |
| `src/catalogue_model.rs` | Peer-supplied catalogue entry metadata (`display_name`, mime, sizes, hashes) | `catalogue_model::RemoteSharedFile::validate()`, `catalogue_model::RemoteCollection::validate()` |
| `src/file_access_handler.rs` | Transfer authorization at request time (friend grants, expiry, disablement, availability) | `file_access_handler::FileAccessHandler::check_permission(...)` |

---

## 2. Rule-by-rule inventory

### 2.1 Filename / display-name validation

| Rule | Legacy implementation (retired) | Canonical implementation |
|---|---|---|
| Reject traversal / separators / dot refs in peer-controlled names | `UserProfile::validate_received_filename` (deleted — only tests referenced it) | `safe_destination::safe_destination_path` / `sanitise_filename` / `check_traversal` (downloads), `collection_transfer::validate_path_component` (collection components), `catalogue_model::RemoteSharedFile::validate` (catalogue `display_name`), `video_playback::validate_attachment_filename` (inline video) |

### 2.2 Path containment

| Rule | Legacy implementation (retired) | Canonical implementation |
|---|---|---|
| A path must resolve inside its root after symlink resolution | `UserProfile::is_path_contained` (deleted) | `path_containment::is_path_contained` |
| A symlink must not escape its root | `UserProfile::symlink_is_safe` (deleted) | `path_containment::symlink_is_safe` |

### 2.3 Size limit + extension allowlist

| Rule | Legacy implementation (retired) | Canonical implementation |
|---|---|---|
| File size ≤ max; extension in allowlist (empty = all) | `UserProfile::is_file_announce_allowed`, `UserProfile::is_file_allowed`, free fn `check_file_announce_allowed` (deleted) — plus an **inline re-implementation** in `file_indexer::scan_dir_with_profile_checks` | `file_policy::admission` (single gate; `file_indexer` now delegates) |

### 2.4 Transfer authorization / permissions

| Rule | Legacy implementation (retired) | Canonical implementation |
|---|---|---|
| May this peer download this file? | `UserProfile::is_download_allowed` (deleted — trivial getter over `allow_downloads`, unused at runtime) | `file_access_handler::check_permission` (friend grants, expiry, disablement, availability) + `download_initiation::validate_download_request` (catalogue-verified precondition) |

### 2.5 Content-addressed image store (not a duplicate)

`src/image_store.rs` validates identifiers against a **content-hash format**
(stem must be 64 hex chars, extension from a fixed allowlist).  This is not a
peer-facing policy rule — the store never uses peer-supplied filenames as
path components — so it is intentionally left as its own implementation.

---

## 3. What was retired (call-site search result)

| Deleted item | Callers found before deletion | Outcome |
|---|---|---|
| `UserProfile::validate_received_filename` | Tests only | Deleted; coverage moved to conformance matrix |
| `UserProfile::is_file_announce_allowed` | `is_file_allowed`, `check_file_announce_allowed`, tests | Deleted; rule lives in `file_policy::admission` |
| `UserProfile::is_file_allowed` | Tests only | Deleted; disk-reading equivalent is `file_indexer` + `file_policy::admission` |
| `UserProfile::is_path_contained` | `file_indexer` (migrated) + tests | Deleted; rule lives in `path_containment::is_path_contained` |
| `UserProfile::symlink_is_safe` | `file_indexer` (migrated) + tests | Deleted; rule lives in `path_containment::symlink_is_safe` |
| `UserProfile::is_download_allowed` | `chat_core` test | Deleted; use the `allow_downloads` field or `file_access_handler` |
| free fn `check_file_announce_allowed` | Tests only | Deleted |

`file_indexer` now calls `path_containment::symlink_is_safe`,
`path_containment::is_path_contained`, and `file_policy::admission` — no
legacy policy code remains in the shared-folder scan path.

---

## 4. Conformance matrix

`tests/test_policy_conformance.rs` runs the same hostile inputs
(`../escape.txt`, `/etc/passwd`, `sub/file.txt`, `..`, `.`, `C:autoexec.bat`,
`CON`, `NUL`, empty, whitespace, plain names) through every public intake
boundary and asserts the safety property at each one:

- Download-destination boundaries (`safe_destination_path`,
  `prepare_download_destination`, `resolve_destination_with_policy`) never
  produce a path outside the caller's directory.
- `validate_path_component` and `validate_attachment_filename` reject
  traversal / separator / absolute / dot-ref names.
- `path_containment` fails closed for escaping symlinks and outside paths.
- `catalogue_model::RemoteSharedFile::validate` rejects hostile `display_name`s.
- `file_policy::admission` is the single case-insensitive size/extension gate
  (regression: legacy profile method was case-sensitive while the indexer was
  case-insensitive; the canonical rule matches the indexer).

`src/file_indexer.rs` has a unit test (`profile_scan_flags_match_canonical_file_policy`)
that re-runs a profile scan and asserts the emitted `over_limit` /
`extension_blocked` flags exactly match `file_policy::admission` — if an
inline copy ever disagrees, that test fails.

---

## 5. Where to change policy in the future

1. **Size / extension policy** → `src/file_policy.rs::admission`.
2. **Containment / symlink policy** → `src/path_containment.rs`.
3. **Download filename sanitisation** → `src/safe_destination.rs`.
4. **Collection component policy** → `src/collection_transfer.rs`.
5. **Attachment name policy** → `src/video_playback.rs`.
6. **Catalogue metadata policy** → `src/catalogue_model.rs`.
7. **Transfer authorization** → `src/file_access_handler.rs` +
   `src/download_initiation.rs`.

After any change, run the conformance matrix:
`rb test --example boru --features gui,video-playback,terminal -- policy_conformance`
(or `rb test --lib -- file_policy` / `path_containment` / `file_indexer`
for the unit suites).
