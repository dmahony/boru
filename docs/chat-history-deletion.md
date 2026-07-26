# Chat-history deletion

The chat options **Clear history** confirmation clears the selected room topic
locally. The operation is retry-safe: deleting an already-cleared topic is a
no-op and does not recreate metadata or tombstones.

## Backend contract

`ChatHistoryStore::remove_topic` removes the JSON-backed active history and
`OutboxStore::remove_topic` removes queued gossip sends. The shared room cleanup
operation also removes room/friend metadata and persists those changes.

The SQLite stores are purged in transactions:

- `MessageStore::hard_delete_conversation(topic)` removes inbox, outbox,
  replay bookkeeping, message tombstones, chat message projections, and
  conversation metadata.
- `Storage::delete_chat_history(topic, event_ids)` removes message attachment
  links plus direct-message rows in the catalogue/storage database. It does
  not delete content-addressed file objects, because they can be owned by an
  unrelated chat or by the user's shared-file catalogue.

The topic is the ownership key. No account identity, friend relationship, file
catalogue, contacts, or unrelated topic is removed. A storage transaction
rolls back if any SQL operation fails; the frontend reports the failure instead
of claiming success. The JSON stores use atomic writes after the in-memory
purge.

This is a local operation; it does not send a remote deletion request or erase
copies already persisted by peers.
