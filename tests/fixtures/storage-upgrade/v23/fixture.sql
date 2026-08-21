-- Synthetic Boru storage fixture: schema family v23.
-- v24..v26 are intentionally pending to exercise interrupted/restart recovery.
PRAGMA foreign_keys = ON;
CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at_ms INTEGER NOT NULL);
INSERT INTO schema_version SELECT value, 1000 + value FROM (WITH RECURSIVE x(value) AS (SELECT 1 UNION ALL SELECT value+1 FROM x WHERE value < 23) SELECT value FROM x);
CREATE TABLE inbox (msg_id BLOB PRIMARY KEY, conversation_id BLOB NOT NULL, author_user_id BLOB NOT NULL, author_device_id BLOB NOT NULL, created_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL, ciphertext BLOB NOT NULL, signature BLOB NOT NULL, acked_at_ms INTEGER);
CREATE TABLE outbox (msg_id BLOB NOT NULL, recipient_device_id BLOB NOT NULL, status INTEGER NOT NULL, attempts INTEGER NOT NULL, next_attempt_at_ms INTEGER NOT NULL, last_error_code TEXT, last_attempt_at_ms INTEGER, lease_owner TEXT, locked_until_ms INTEGER, expires_at_ms INTEGER, PRIMARY KEY(msg_id, recipient_device_id));
CREATE TABLE downloads (id INTEGER PRIMARY KEY AUTOINCREMENT, content_hash TEXT NOT NULL, remote_peer TEXT NOT NULL, state TEXT NOT NULL, bytes_downloaded INTEGER NOT NULL, total_bytes INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL, last_error TEXT, retry_count INTEGER NOT NULL, next_retry_at_ms INTEGER, temp_path TEXT, destination_path TEXT);
CREATE TABLE shared_files (content_hash TEXT NOT NULL, profile_user_id TEXT NOT NULL, metadata_id TEXT NOT NULL, display_filename TEXT NOT NULL, description TEXT, offered INTEGER NOT NULL DEFAULT 1, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL, version INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(content_hash, profile_user_id));
CREATE TABLE chat_messages (
 id INTEGER PRIMARY KEY AUTOINCREMENT, msg_hash BLOB NOT NULL,
 topic BLOB NOT NULL, sender BLOB NOT NULL, timestamp_ms INTEGER NOT NULL,
 signed_bytes BLOB NOT NULL, thread_root_id BLOB, reply_to_message_id BLOB,
 deleted INTEGER NOT NULL DEFAULT 0, UNIQUE(msg_hash)
);
CREATE INDEX idx_chat_messages_topic ON chat_messages(topic, timestamp_ms);
CREATE INDEX idx_chat_messages_thread_root ON chat_messages(topic, thread_root_id, timestamp_ms, id);
CREATE INDEX idx_chat_messages_reply_target ON chat_messages(reply_to_message_id);
CREATE TABLE thread_state (
 topic BLOB NOT NULL, thread_root_id BLOB NOT NULL,
 followed INTEGER NOT NULL DEFAULT 0, unread_replies INTEGER NOT NULL DEFAULT 0,
 read_at_ms INTEGER, PRIMARY KEY(topic, thread_root_id)
);
INSERT INTO chat_messages(msg_hash, topic, sender, timestamp_ms, signed_bytes, deleted)
VALUES (zeroblob(32), zeroblob(32), zeroblob(32), 1700000000000, X'73696e7468657469632d7632332d6d657373616765', 0);
INSERT INTO thread_state(topic, thread_root_id, followed, unread_replies, read_at_ms)
VALUES (zeroblob(32), zeroblob(32), 1, 2, 1700000000000);
