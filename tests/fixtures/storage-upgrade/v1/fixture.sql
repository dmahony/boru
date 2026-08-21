-- Synthetic Boru storage fixture: schema family v1 (historical v0.103.0 shape).
-- No secrets, private keys, host paths, or real user data.
PRAGMA foreign_keys = ON;
CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at_ms INTEGER NOT NULL);
INSERT INTO schema_version VALUES (1, 1000);
CREATE TABLE inbox (
    msg_id BLOB PRIMARY KEY, conversation_id BLOB NOT NULL,
    author_user_id BLOB NOT NULL, author_device_id BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL,
    ciphertext BLOB NOT NULL, signature BLOB NOT NULL, acked_at_ms INTEGER
);
CREATE TABLE outbox (
    msg_id BLOB NOT NULL, recipient_device_id BLOB NOT NULL,
    status INTEGER NOT NULL, attempts INTEGER NOT NULL,
    next_attempt_at_ms INTEGER NOT NULL, last_error_code TEXT,
    last_attempt_at_ms INTEGER, PRIMARY KEY (msg_id, recipient_device_id)
);
CREATE TABLE contacts (
    user_id BLOB NOT NULL, device_id BLOB NOT NULL, endpoint_addr BLOB,
    identity_key BLOB NOT NULL, last_seen_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL, PRIMARY KEY (user_id, device_id)
);
CREATE TABLE sync_cursor (
    peer_device_id BLOB PRIMARY KEY, last_seen_msg_clock BLOB,
    last_sync_at_ms INTEGER NOT NULL
);
INSERT INTO inbox VALUES (
    X'1111111111111111111111111111111111111111111111111111111111111111',
    X'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    X'2222222222222222222222222222222222222222222222222222222222222222',
    X'2222222222222222222222222222222222222222222222222222222222222222',
    1700000000000, 1700001000000, X'6c65676163792d6d6573736167652d7631',
    zeroblob(64), NULL
);
INSERT INTO contacts VALUES (
    X'2222222222222222222222222222222222222222222222222222222222222222',
    X'3333333333333333333333333333333333333333333333333333333333333333',
    NULL, X'4444444444444444444444444444444444444444444444444444444444444444',
    1700000000000, 1700001000000
);
