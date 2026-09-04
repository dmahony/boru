#!/usr/bin/env python3
"""Focused tests for version calculation and safe file updates."""
from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SPEC = importlib.util.spec_from_file_location("version", Path(__file__).with_name("version.py"))
assert SPEC and SPEC.loader
version = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(version)


class VersionTests(unittest.TestCase):
    def test_conventional_commit_bump(self) -> None:
        self.assertEqual(version.classify_commit("fix: repair", ""), version.BUMP_PATCH)
        self.assertEqual(version.classify_commit("feat(ui): add panel", ""), version.BUMP_MINOR)
        self.assertEqual(version.classify_commit("feat!: change API", ""), version.BUMP_MAJOR)

    def test_atomic_write_replaces_complete_content(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "state.json"
            version.atomic_write(path, json.dumps({"version": "1.2.3"}) + "\n")
            self.assertEqual(json.loads(path.read_text()), {"version": "1.2.3"})
            self.assertEqual(list(Path(directory).iterdir()), [path])

    def test_parse_version_rejects_incomplete_values(self) -> None:
        with self.assertRaises((IndexError, ValueError)):
            version.parse_version("1.2")


if __name__ == "__main__":
    unittest.main()
