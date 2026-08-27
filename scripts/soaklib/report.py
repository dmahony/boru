"""Versioned soak evidence, central redaction, and Markdown rendering."""
from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

SCHEMA = "boru-soak-report/v2"
SENSITIVE_KEYS = {"secret", "token", "password", "private_key", "ticket", "payload"}
MAX_STRING = 128
MAX_BYTES = 256


def _sensitive(key: Any) -> bool:
    normalized = str(key).lower().replace("-", "_")
    return normalized in SENSITIVE_KEYS or any(part in SENSITIVE_KEYS for part in normalized.split("_"))


def redact(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: ("<redacted>" if _sensitive(k) else redact(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    if isinstance(value, tuple):
        return [redact(v) for v in value]
    if isinstance(value, (bytes, bytearray)):
        return "<redacted:bytes>" if len(value) > MAX_BYTES else value.hex()
    if isinstance(value, str):
        value = re.sub(r"(?i)(secret|token|password|private[_-]?key|ticket|payload)\s*[:=]\s*[^,; ]+", r"\1=<redacted>", value)
        if len(value) > MAX_STRING:
            return value[:32] + "…<redacted>"
    return value


def build_report(*, status: str, workflow: str, seed: int, run_id: str | None = None,
                 **fields: Any) -> dict[str, Any]:
    """Build the stable v2 envelope while retaining useful legacy aliases."""
    if status not in {"PASS", "FAIL", "SKIP", "unsupported"}:
        raise ValueError(f"invalid report status: {status}")
    report: dict[str, Any] = {
        "schema": SCHEMA, "status": status, "workflow": workflow, "seed": seed,
        "run_id": run_id, "build": {"version": fields.pop("build_version", None), "commit": fields.pop("build_commit", None)},
        "topology": fields.pop("topology", {}), "aliases": fields.pop("aliases", {}),
        "assertions": fields.pop("assertions", []), "faults": fields.pop("faults", []),
        "messages": fields.pop("messages", {}), "transfers": fields.pop("transfers", {}),
        "resources": fields.pop("resources", {}), "cleanup": fields.pop("cleanup", {}),
        "limitations": fields.pop("limitations", []), "failure_codes": fields.pop("failure_codes", []),
        "events": fields.pop("events", []),
        # Compatibility fields used by the original soak reader.
        "scenario": fields.pop("scenario", None), "profile": fields.pop("profile", None),
        "nodes": fields.pop("nodes", None), "event_count": fields.pop("event_count", None),
        "failures": fields.pop("failures", []), "run_dir": fields.pop("run_dir", None),
        **fields,
    }
    return redact(report)


def render_evidence(report: dict[str, Any]) -> str:
    """Render only redacted, concise facts (never raw events or message bodies)."""
    report = redact(report)
    status = report.get("status", "unsupported")
    lines = [f"# Boru soak evidence", "", f"- Status: **{status}**", f"- Workflow: `{report.get('workflow', 'unknown')}`", f"- Schema: `{report.get('schema', SCHEMA)}`", f"- Seed: `{report.get('seed')}`"]
    if report.get("scenario"):
        lines.append(f"- Scenario: `{report['scenario']}`")
    assertions = report.get("assertions") or []
    if assertions:
        lines += ["", "## Assertions"]
        for assertion in assertions:
            if isinstance(assertion, dict):
                lines.append(f"- {assertion.get('name', 'assertion')}: **{assertion.get('status', assertion.get('outcome', 'unsupported'))}**")
    failures = report.get("failures") or report.get("failure_codes") or []
    if failures:
        lines += ["", "## Failures"]
        lines.extend(f"- `{item}`" for item in failures)
    limitations = report.get("limitations") or []
    if limitations:
        lines += ["", "## Limitations"]
        lines.extend(f"- {item}" for item in limitations)
    return "\n".join(lines) + "\n"


def write_evidence(root: Path, report: dict[str, Any]) -> None:
    root.mkdir(parents=True, exist_ok=True)
    (root / "report.json").write_text(json.dumps(redact(report), indent=2, sort_keys=True) + "\n", encoding="utf-8")
    (root / "evidence.md").write_text(render_evidence(report), encoding="utf-8")
