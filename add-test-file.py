#!/usr/bin/env python3
"""Add a test file to the shared catalogue on VM54."""
import hashlib
import json
import os
import sqlite3
import struct
import time
import sys

# Create a test file
test_content = b"Hello from VM54! This is a test file for the file sharing feature."
hash_hex = hashlib.blake3(test_content).hexdigest()
size = len(test_content)
filename = "test-share.txt"

# Compute metadata_id (matches Rust SharedFile::new)
meta_hasher = hashlib.blake3()
meta_hasher.update(filename.encode())
meta_hasher.update(struct.pack("<Q", size))
now_ts = int(time.time())
meta_hasher.update(struct.pack("<Q", now_ts))
metadata_id = meta_hasher.hexdigest()

# Read room.json to get profile_user_id
room_path = "/tmp/boru-live-54/room.json"
profile_user_id = "default"
if os.path.exists(room_path):
    with open(room_path) as f:
        room_data = json.load(f)
    profile_user_id = room_data.get("nickname", "default")
    print(f"profile_user_id from room.json: {profile_user_id}")

# Connect to the database
db_path = "/tmp/boru-live-54/boru.db"
print(f"Connecting to: {db_path}")
conn = sqlite3.connect(db_path)

# Check existing tables
cursor = conn.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
tables = [r[0] for r in cursor.fetchall()]
print(f"Tables: {tables}")

# Verify the schema has file_objects and shared_files
if "file_objects" not in tables:
    print("ERROR: file_objects table not found!")
    sys.exit(1)
if "shared_files" not in tables:
    print("ERROR: shared_files table not found!")
    sys.exit(1)

# Insert file object
mime = "text/plain"
now_ms = int(time.time() * 1000)
conn.execute(
    "INSERT OR IGNORE INTO file_objects (content_hash, size, mime_type, filename, created_at_ms, data) VALUES (?, ?, ?, ?, ?, ?)",
    (hash_hex, size, mime, filename, now_ms, test_content)
)
print(f"Inserted file_object (hash={hash_hex[:16]}...)")

# Insert shared_file
conn.execute(
    """INSERT INTO shared_files
       (content_hash, profile_user_id, metadata_id, display_filename, description, offered, created_at_ms, updated_at_ms)
       VALUES (?, ?, ?, ?, ?, 1, ?, ?)
       ON CONFLICT(content_hash, profile_user_id) DO UPDATE SET
           metadata_id=excluded.metadata_id,
           display_filename=excluded.display_filename,
           offered=excluded.offered,
           updated_at_ms=excluded.updated_at_ms""",
    (hash_hex, profile_user_id, metadata_id, filename, None, now_ms, now_ms)
)
print(f"Inserted shared_file (profile={profile_user_id})")

conn.commit()

# Verify
cursor = conn.execute("SELECT content_hash, display_filename, offered FROM shared_files WHERE content_hash = ?", (hash_hex,))
row = cursor.fetchone()
print(f"Verification: {row}")

conn.close()
print(f"\nDone! Content hash: {hash_hex}")
