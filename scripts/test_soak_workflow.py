"""Focused tests for the standard-library workflow engine."""
from __future__ import annotations

import unittest

from soaklib.fixtures import golden_recovery
from soaklib.report import redact
from soaklib.faults import FaultScheduler, FaultSpec, FaultSchedulingError, InvalidFaultTarget
from soaklib.workflow import Workflow, WorkflowContext, WorkflowEngine, action, poll


class WorkflowTests(unittest.TestCase):
    def test_mock_workflow_and_cleanup(self) -> None:
        cleaned = []
        context = WorkflowContext(seed=11)
        context.defer(lambda: cleaned.append(True))
        result = WorkflowEngine(seed=11).run(golden_recovery(), context)
        self.assertEqual(result.outcome, "PASS")
        self.assertEqual(len(result.records), 4)
        self.assertEqual(cleaned, [True])

    def test_poll_timeout_and_assertion_failure(self) -> None:
        result = WorkflowEngine(seed=1).run(Workflow("timeout", (poll("never", lambda _: False, timeout_s=0.001),)))
        self.assertEqual(result.outcome, "FAIL")
        self.assertIn("timed out", result.failure_reason or "")

    def test_seed_replay(self) -> None:
        workflow = Workflow("seed", (action("one", lambda _: None), action("two", lambda _: None)))
        left = WorkflowEngine(seed=99).run(workflow).as_dict()
        right = WorkflowEngine(seed=99).run(workflow).as_dict()
        self.assertEqual([s["correlation_id"] for s in left["steps"]], [s["correlation_id"] for s in right["steps"]])

    def test_cleanup_runs_on_exception(self) -> None:
        state = []
        context = WorkflowContext(seed=2)
        context.defer(lambda: state.append("cleaned"))
        result = WorkflowEngine(seed=2).run(Workflow("error", (action("bad", lambda _: 1 / 0),)), context)
        self.assertEqual(result.outcome, "FAIL")
        self.assertEqual(state, ["cleaned"])

    def test_error_redaction(self) -> None:
        self.assertNotIn("do-not-leak", redact("token=do-not-leak"))
        self.assertEqual(redact({"password": "do-not-leak"})["password"], "<redacted>")

    def test_fault_schedule_is_stable_and_state_aware(self) -> None:
        specs = [FaultSpec("restart", "join_converged"), FaultSpec("offline", "message_queued")]
        left = FaultScheduler(7, "chat", {0, 1}).schedule(specs)
        right = FaultScheduler(7, "chat", {0, 1}).schedule(specs)
        self.assertEqual([fault.as_dict() for fault in left], [fault.as_dict() for fault in right])
        scheduler = FaultScheduler(7, "chat", {0, 1})
        scheduler.schedule(specs)
        records = scheduler.trigger("join_converged", {"ready": True})
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["before"], {"ready": True})
        self.assertEqual(records[0]["after"], {"ready": True})

    def test_fault_target_and_recovery_window_are_validated(self) -> None:
        with self.assertRaises(InvalidFaultTarget):
            FaultScheduler(1, "chat", {0}).schedule([FaultSpec("restart", target=4)])
        with self.assertRaisesRegex(FaultSchedulingError, "positive"):
            FaultScheduler(1, "chat", {0}).schedule([FaultSpec("restart", recovery_window_s=0)])

    def test_trigger_rejects_cleaned_up_node(self) -> None:
        scheduler = FaultScheduler(1, "chat", {0})
        scheduler.schedule([FaultSpec("offline", "transfer_progress")])
        scheduler.active_nodes.clear()
        with self.assertRaises(InvalidFaultTarget):
            scheduler.trigger("transfer_progress")


if __name__ == "__main__":
    unittest.main()
