"""Reusable transport-free workflow fixtures for unit tests and local dry runs."""
from __future__ import annotations

from typing import Any

from .workflow import Workflow, WorkflowContext, action, fault, poll, recovery


def golden_recovery(state: dict[str, Any] | None = None) -> Workflow:
    """Describe the complete bounded A/B/C golden-recovery sequence.

    Callbacks are deliberately supplied by the real-process runner.  The
    default fixture records the contract and is useful for deterministic
    orchestration tests without pretending to have delivered network data.
    """
    state = state if state is not None else {"ready": False, "offline": False, "transfer": False}

    def mark(key: str):
        return lambda _: state.__setitem__(key, True)

    def recovered(_: Any) -> bool:
        return state["offline"]

    def transfer_recovered(_: Any) -> bool:
        return state["transfer"]

    return Workflow("golden-recovery", (
        action("create_room_A", mark("ready"), node=0),
        action("join_room_B", mark("ready"), node=1),
        action("join_room_C", mark("ready"), node=2),
        poll("membership_convergence_ABC", lambda _: state["ready"], node=0),
        action("message_A_to_B", mark("ready"), node=0),
        action("message_B_to_A", mark("ready"), node=1),
        action("message_C_to_room", mark("ready"), node=2),
        fault("take_B_offline", mark("offline"), node=1),
        action("send_around_interruption", mark("offline"), node=0),
        recovery("recover_B_without_duplicates", recovered, node=1),
        action("start_A_to_B_transfer", mark("transfer"), node=0),
        fault("interrupt_B_transfer", mark("transfer"), node=1),
        recovery("resume_exact_hash_and_size", transfer_recovered, node=1),
        action("C_leave_and_rejoin", mark("ready"), node=2),
        recovery("post_rejoin_message_and_membership", lambda _: state["ready"], node=2),
    ))
