"""Focused tests for the standard-library workflow engine."""
from __future__ import annotations

import unittest
import pathlib
import tempfile

from soaklib.assertions import AssertionEngine, MetricRule, evaluate_metric
from soaklib.fixtures import golden_recovery
from soaklib.report import build_report, redact, render_evidence
from soaklib.metrics import proc_metrics
from soaklib.workflow import Workflow, WorkflowContext, WorkflowEngine, action, poll
from soak_harness import parse_args, run_workflow, wait_until


class WorkflowTests(unittest.TestCase):
    def test_failing_metric_is_deterministic_and_diagnostic(self) -> None:
        rule = MetricRule("rss_kb", warmup_samples=2, max_peak=120)
        left = evaluate_metric("rss", [100, 101, 125], rule, nodes=[2]).as_dict()
        right = evaluate_metric("rss", [100, 101, 125], rule, nodes=[2]).as_dict()
        self.assertEqual(left["status"], "FAIL")
        self.assertEqual(left["failure_code"], "peak_limit")
        self.assertEqual(left["last_observed"], right["last_observed"])
        self.assertEqual(left["details"]["threshold"], 120)

    def test_warmup_median_tolerates_normal_noise(self) -> None:
        result = evaluate_metric("fds", [100, 102, 99, 101, 103],
                                 MetricRule("fds", warmup_samples=3, max_delta=10))
        self.assertEqual(result.status, "PASS")

    def test_unsupported_metric_is_explicit_skip(self) -> None:
        result = evaluate_metric("gpu", [None, None], MetricRule("gpu"), nodes=[1])
        self.assertEqual(result.status, "SKIP")
        self.assertEqual(result.failure_code, "unsupported_metric")
        self.assertEqual(result.as_dict()["nodes"], [1])

    def test_orphan_guard_reports_fail_and_skip(self) -> None:
        engine = AssertionEngine(clock=lambda: 10.0)
        self.assertEqual(engine.orphan_children("children", [44], [0]).failure_code,
                         "orphan_children")
        self.assertEqual(engine.orphan_children("unsupported", None).status, "SKIP")

    def test_proc_metrics_exposes_resource_fields(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            db = pathlib.Path(tmp) / "profile.db"
            db.write_bytes(b"123")
            metrics = proc_metrics(__import__("os").getpid(), pathlib.Path(tmp))
            self.assertIsNotNone(metrics["rss_kb"])
            self.assertEqual(metrics["profile_db_bytes"], 3)
            self.assertIn("orphan_children", metrics)

    def test_mock_workflow_and_cleanup(self) -> None:
        cleaned = []
        context = WorkflowContext(seed=11)
        context.defer(lambda: cleaned.append(True))
        result = WorkflowEngine(seed=11).run(golden_recovery(), context)
        self.assertEqual(result.outcome, "PASS")
        self.assertEqual(len(result.records), 15)
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

    def test_repeated_developer_workflow_records_every_seed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = parse_args([
                "--workflow", "golden-recovery", "--repeat", "10", "--seed", "100",
                "--run-dir", str(pathlib.Path(tmp) / "smoke"),
            ])
            self.assertEqual(run_workflow(args), 0)
            report = __import__("json").loads((args.run_dir / "report.json").read_text())
            self.assertEqual(report["repeat"], {"requested": 10, "completed": 10,
                                                  "seeds": list(range(100, 110))})
            workflow = __import__("json").loads((args.run_dir / "workflow.json").read_text())
            self.assertEqual(len(workflow["runs"]), 10)

    def test_wait_until_uses_a_bounded_poll(self) -> None:
        calls = []
        self.assertTrue(wait_until(lambda: calls.append(True) or len(calls) >= 2, 0.2, 0.001))
        self.assertGreaterEqual(len(calls), 2)

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
