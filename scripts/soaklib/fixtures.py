"""Reusable transport-free workflow fixtures for unit tests and local dry runs."""
from __future__ import annotations

from typing import Any

from .workflow import Workflow, WorkflowContext, action, fault, poll, recovery


def golden_recovery(state: dict[str, Any] | None = None) -> Workflow:
    state = state if state is not None else {"action": False, "fault": False, "recovered": False}
    return Workflow("golden-recovery", (
        action("prepare", lambda _: state.__setitem__("action", True), node=0),
        fault("inject_fault", lambda _: state.__setitem__("fault", True), node=0),
        poll("wait_for_recovery", lambda _: state.__setitem__("recovered", state["fault"]) or state["recovered"], node=0),
        recovery("verify_recovery", lambda _: state["recovered"], node=0),
    ))
