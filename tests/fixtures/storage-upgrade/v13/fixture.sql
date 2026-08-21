-- Synthetic Boru storage fixture: schema family v13 (historical v0.108.0-era shape).
-- The fixture intentionally stops before v14 so the current opener exercises
-- v14..v26, including additive columns and new projections.
PRAGMA foreign_keys = ON;
CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at_ms INTEGER NOT NULL);
INSERT INTO schema_version SELECT value, 1000 + value FROM (WITH RECURSIVE x(value) AS (SELECT 1 UNION ALL SELECT value+1 FROM x WHERE value < 13) SELECT value FROM x);
CREATE TABLE inbox (msg_id BLOB PRIMARY KEY, conversation_id BLOB NOT NULL, author_user_id BLOB NOT NULL, author_device_id BLOB NOT NULL, created_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL, ciphertext BLOB NOT NULL, signature BLOB NOT NULL, acked_at_ms INTEGER);
CREATE TABLE outbox (msg_id BLOB NOT NULL, recipient_device_id BLOB NOT NULL, status INTEGER NOT NULL, attempts INTEGER NOT NULL, next_attempt_at_ms INTEGER NOT NULL, last_error_code TEXT, last_attempt_at_ms INTEGER, lease_owner TEXT, locked_until_ms INTEGER, expires_at_ms INTEGER, PRIMARY KEY(msg_id, recipient_device_id));
CREATE TABLE downloads (id INTEGER PRIMARY KEY AUTOINCREMENT, content_hash TEXT NOT NULL, remote_peer TEXT NOT NULL, state TEXT NOT NULL, bytes_downloaded INTEGER NOT NULL, total_bytes INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL, last_error TEXT, retry_count INTEGER NOT NULL, next_retry_at_ms INTEGER, temp_path TEXT, destination_path TEXT);
CREATE TABLE shared_files (
 content_hash TEXT NOT NULL, profile_user_id TEXT NOT NULL, metadata_id TEXT NOT NULL,
 display_filename TEXT NOT NULL, description TEXT, offered INTEGER NOT NULL DEFAULT 1,
 created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
 PRIMARY KEY(content_hash, profile_user_id)
);
CREATE TABLE directory_ads (
 topic BLOB NOT NULL, author BLOB NOT NULL, room_name TEXT NOT NULL,
 description TEXT NOT NULL, ticket TEXT NOT NULL, member_count INTEGER NOT NULL,
 last_activity INTEGER NOT NULL, received_at_ms INTEGER NOT NULL,
 PRIMARY KEY(topic, author)
);
CREATE TABLE group_invites (
 invite_id BLOB PRIMARY KEY, group_id BLOB NOT NULL, inviter_public_key BLOB NOT NULL,
 recipient_public_key BLOB NOT NULL, epoch INTEGER NOT NULL, status TEXT NOT NULL,
 created_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL
);
INSERT INTO shared_files VALUES ('1111111111111111111111111111111111111111111111111111111111111111', 'profile-v13', 'file-v13', 'notes.txt', 'synthetic fixture', 1, 1700000000000, 1700000000000);
INSERT INTO directory_ads VALUES (zeroblob(32), zeroblob(32), 'Fixture Room', 'Synthetic room', 'fixture-ticket', 2, 1700000000000, 1700000000000);
INSERT INTO group_invites VALUES (zeroblob(32), zeroblob(32), zeroblob(32), zeroblob(32), 0, 'pending', 1700000000000, 1700001000000);
