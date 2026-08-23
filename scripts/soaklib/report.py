"""Redacted, JSON-serializable workflow/report values."""
from __future__ import annotations

import re
from typing import Any

SENSITIVE_KEYS = {"secret", "token", "password", "private_key", "ticket", "payload"}


def redact(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: ("<redacted>" if k.lower() in SENSITIVE_KEYS else redact(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    if isinstance(value, tuple):
        return [redact(v) for v in value]
    if isinstance(value, str):
        value = re.sub(r"(?i)(secret|token|password|private_key|ticket|payload)\s*[:=]\s*[^,; ]+", r"\1=<redacted>", value)
        if len(value) > 128:
            return value[:32] + "…<redacted>"
    return value
