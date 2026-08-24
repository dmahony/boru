"""Focused tests for the standard-library workflow engine."""
from __future__ import annotations

import unittest

from soaklib.fixtures import golden_recovery
from soaklib.report import build_report, redact, render_evidence
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

    def test_nested_redaction_and_oversized_content(self) -> None:
        value = {"outer": [{"private_key": "secret-value", "payload": "body"}], "text": "x" * 200}
        safe = redact(value)
        self.assertEqual(safe["outer"][0]["private_key"], "<redacted>")
        self.assertEqual(safe["outer"][0]["payload"], "<redacted>")
        self.assertNotIn("x" * 200, str(safe))

    def test_v2_schema_and_summary_are_redacted(self) -> None:
        report = build_report(
            status="SKIP", workflow="fixture", seed=7,
            assertions=[{"name": "ticket", "status": "unsupported", "ticket": "never-show"}],
            failures=["token=never-show"],
        )
        self.assertEqual(report["schema"], "boru-soak-report/v2")
        self.assertEqual(report["status"], "SKIP")
        summary = render_evidence(report)
        self.assertNotIn("never-show", summary)
        self.assertIn("SKIP", summary)


if __name__ == "__main__":
    unittest.main()
