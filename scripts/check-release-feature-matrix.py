#!/usr/bin/env python3
"""Check that the release workflow matches docs/release-feature-matrix.toml."""
from __future__ import annotations

import argparse
import pathlib
import re
import sys
import tempfile
import unittest
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 fallback
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_MATRIX = ROOT / "docs" / "release-feature-matrix.toml"
DEFAULT_WORKFLOW = ROOT / ".github" / "workflows" / "release.yaml"
TARGET_RE = re.compile(r"^\s*- target:\s*(\S+)\s*$")
FEATURES_RE = re.compile(r"^\s*features:\s*(\S+)\s*$")
VALID_STATUSES = {"supported", "intentionally-disabled", "unsupported", "untested"}


def load_matrix(path: pathlib.Path) -> dict[str, Any]:
    with path.open("rb") as stream:
        data = tomllib.load(stream)
    platforms = data.get("platform", [])
    if not platforms:
        raise ValueError("matrix must contain at least one [[platform]]")
    targets: dict[str, tuple[str, ...]] = {}
    for platform in platforms:
        target = platform.get("target")
        features = platform.get("release_features")
        if not isinstance(target, str) or not isinstance(features, list):
            raise ValueError("each platform needs target and release_features")
        if target in targets:
            raise ValueError(f"duplicate target in matrix: {target}")
        if not all(isinstance(feature, str) and feature for feature in features):
            raise ValueError(f"invalid feature list for {target}")
        implicit = platform.get("implicit_features", [])
        if not all(isinstance(feature, str) and feature for feature in implicit):
            raise ValueError(f"invalid implicit feature list for {target}")
        declared = set(features) | set(implicit)
        feature_info = platform.get("features", {})
        if not declared <= set(feature_info):
            missing = sorted(declared - set(feature_info))
            raise ValueError(f"missing feature metadata for {target}: {missing}")
        for feature, details in feature_info.items():
            status = details.get("status") if isinstance(details, dict) else None
            if status not in VALID_STATUSES:
                raise ValueError(f"invalid status for {target}/{feature}: {status!r}")
            if status == "supported" and feature not in declared:
                raise ValueError(f"supported feature is not enabled for {target}: {feature}")
        targets[target] = tuple(features)
    data["release_targets"] = targets
    return data


def parse_workflow(path: pathlib.Path) -> dict[str, tuple[str, ...]]:
    """Parse the deliberately simple target/features matrix in release.yaml."""
    builds: dict[str, tuple[str, ...]] = {}
    pending_target: str | None = None
    for line in path.read_text(encoding="utf-8").splitlines():
        target_match = TARGET_RE.match(line)
        if target_match:
            pending_target = target_match.group(1)
            continue
        features_match = FEATURES_RE.match(line)
        if features_match and pending_target:
            if pending_target in builds:
                raise ValueError(f"duplicate target in workflow: {pending_target}")
            builds[pending_target] = tuple(
                feature.strip() for feature in features_match.group(1).split(",") if feature.strip()
            )
            pending_target = None
    if pending_target:
        raise ValueError(f"target has no features entry in workflow: {pending_target}")
    if not builds:
        raise ValueError("workflow has no release target/features entries")
    return builds


def check(matrix_path: pathlib.Path = DEFAULT_MATRIX, workflow_path: pathlib.Path = DEFAULT_WORKFLOW) -> list[str]:
    matrix = load_matrix(matrix_path)["release_targets"]
    workflow = parse_workflow(workflow_path)
    errors: list[str] = []
    if set(matrix) != set(workflow):
        errors.append(f"target mismatch: matrix={sorted(matrix)} workflow={sorted(workflow)}")
    for target in sorted(set(matrix) & set(workflow)):
        if matrix[target] != workflow[target]:
            errors.append(
                f"feature mismatch for {target}: matrix={list(matrix[target])} workflow={list(workflow[target])}"
            )
    return errors


class MatrixCheckTests(unittest.TestCase):
    def test_workflow_matches_authoritative_matrix(self) -> None:
        self.assertEqual(check(), [])

    def test_feature_order_and_target_differences_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workflow = pathlib.Path(directory) / "release.yaml"
            workflow.write_text(
                "matrix:\n  include:\n    - target: x86_64-unknown-linux-gnu\n      features: video-playback,gui\n",
                encoding="utf-8",
            )
            errors = check(workflow_path=workflow)
            self.assertTrue(any("feature mismatch" in error for error in errors))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--test", action="store_true", help="run parser tests")
    args = parser.parse_args()
    if args.test:
        return 0 if unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(MatrixCheckTests)).wasSuccessful() else 1
    errors = check()
    if errors:
        for error in errors:
            print(f"release feature matrix check failed: {error}", file=sys.stderr)
        return 1
    print("release feature matrix: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
