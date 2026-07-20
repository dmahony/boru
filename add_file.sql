INSERT OR IGNORE INTO file_objects (content_hash, size, mime_type, filename, created_at_ms, data)
VALUES ('7fc651d1639815218f1162f56136c1b88cc3e63cdf454760b1a8759434dc0e98', 66, 'text/plain', 'test-share.txt', 1784555952671, X'48656c6c6f2066726f6d20564d3534212054686973206973206120746573742066696c6520666f72207468652066696c652073686172696e6720666561747572652e');

INSERT INTO shared_files (content_hash, profile_user_id, metadata_id, display_filename, description, offered, created_at_ms, updated_at_ms)
VALUES ('7fc651d1639815218f1162f56136c1b88cc3e63cdf454760b1a8759434dc0e98', 'default', '19750bf7072d9524bff2eebde22076eb1e453c6221965b584f512e962bee8b70', 'test-share.txt', NULL, 1, 1784555952671, 1784555952671)
ON CONFLICT(content_hash, profile_user_id) DO UPDATE SET metadata_id=excluded.metadata_id, display_filename=excluded.display_filename, offered=excluded.offered, updated_at_ms=excluded.updated_at_ms;

SELECT content_hash, display_filename, offered FROM shared_files WHERE content_hash='7fc651d1639815218f1162f56136c1b88cc3e63cdf454760b1a8759434dc0e98';
