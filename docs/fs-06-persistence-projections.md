# FS-06 Persistence projections

The dashboard reads durable projections from SQLite; it does not own file bytes,
authorization, descriptors, or transfer execution.

## Schema and migration

Schema version 16 adds:

- `shared_files.version`, defaulting to `1` for existing rows. Local metadata
  upserts increment the version, and catalogue adapters expose it as the
  descriptor version.
- `transfer_activity`, keyed by the lifecycle `event_id` and uniquely constrained
  by `(transfer_id, sequence)`. Replayed events are ignored.

Migration v16 is additive and runs in the existing per-version migration loop.
Column additions use an existence check so a partially-applied legacy migration
can be safely resumed. Existing `group_invites` v14/v15 column migrations now
use the same idempotent helper. No data reset or file-byte copy is performed.

## Activity retention and cleanup

`list_transfer_activity` is bounded to 1,000 rows and returns newest activity
first. Callers should periodically call `prune_transfer_activity` with their
chosen retention cutoff; rows older than that timestamp are deleted. The
projection stores only the allow-listed counters and status fields from the
privacy-safe transfer lifecycle payload. Paths, tokens, descriptors, hashes,
and arbitrary payload keys are discarded.

Deleting a shared offer removes its authorization grants in the same SQLite
transaction. It intentionally does not delete `downloads`: queued or active
transfers remain authoritative in the download state machine and can finish or
transition according to its existing revocation semantics. File objects are
removed only by existing reference-aware cleanup once no attachments, shares,
permissions, collections, or downloads reference them.
