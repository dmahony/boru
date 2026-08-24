#!/usr/bin/env python3
"""Boru real-process soak and fault-injection controller.

The controller intentionally uses only the standard library so it can run on a
VM before Boru's optional GUI/test dependencies are installed. It launches
isolated profiles, records compact JSONL events and procfs measurements, and
always attempts process-group cleanup.
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import signal
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import Any

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from soaklib.metrics import proc_metrics
from soaklib.assertions import AssertionEngine, MetricRule
from soaklib.fixtures import golden_recovery
from soaklib.report import build_report, redact, write_evidence
from soaklib.rpc import port_open, rpc
from soaklib.workflow import Workflow, WorkflowContext, WorkflowEngine, action, fault, poll, recovery

SCENARIOS = {"relay-only", "same-lan", "separate-network", "no-dht"}
PROFILES = {
    "developer": {"duration_s": 7200, "nodes": 3, "interval_s": 30},
    "release-candidate": {"duration_s": 28800, "nodes": 6, "interval_s": 60},
}


def build_metadata(cwd: pathlib.Path | None = None) -> dict[str, str]:
    """Capture non-sensitive build identity without requiring a Boru process."""
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=cwd, check=True,
            capture_output=True, text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        commit = "unknown"
    return {"build_version": os.environ.get("BORU_VERSION", "unknown"), "build_commit": commit}


def preflight(args: argparse.Namespace) -> int:
    """Check host prerequisites without launching processes or changing state."""
    checks: list[dict[str, str]] = []
    if args.workflow:
        checks.append({"name": "workflow_fixture", "status": "SKIP",
                       "reason": "fixture mode does not exercise host networking"})
    else:
        checks.append({"name": "binary", "status": "PASS" if args.binary.is_file() and os.access(args.binary, os.X_OK) else "FAIL",
                       "reason": str(args.binary)})
        if not args.no_mcp and not os.environ.get("DISPLAY"):
            checks.append({"name": "display", "status": "SKIP", "reason": "DISPLAY is unset; start Xvfb for MCP/GUI actions"})
        if args.scenario == "separate-network":
            checks.append({"name": "topology", "status": "SKIP",
                           "reason": "controller cannot create a VPN or namespace"})
    checks.append({"name": "procfs", "status": "PASS" if pathlib.Path("/proc").is_dir() else "SKIP",
                   "reason": "Linux procfs metrics"})
    status = "FAIL" if any(check["status"] == "FAIL" for check in checks) else ("SKIP" if any(check["status"] == "SKIP" for check in checks) else "PASS")
    print(json.dumps({"status": status, "checks": checks}, sort_keys=True))
    return 1 if status == "FAIL" else 0


def now() -> float:
    return time.time()


def wait_until(predicate: Any, timeout_s: float, interval_s: float = 0.05) -> bool:
    """Poll a readiness predicate until a monotonic deadline expires."""
    deadline = time.monotonic() + timeout_s
    while True:
        if predicate():
            return True
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        time.sleep(min(interval_s, remaining))


@dataclass
class Node:
    index: int
    data_dir: pathlib.Path
    log_path: pathlib.Path
    mcp_port: int
    process: subprocess.Popen[bytes] | None = None
    restarts: int = 0
    supported: bool = True


@dataclass
class Run:
    root: pathlib.Path
    args: argparse.Namespace
    nodes: list[Node] = field(default_factory=list)
    events: list[dict[str, Any]] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)
    started: float = field(default_factory=now)
    metric_history: dict[int, dict[str, list[Any]]] = field(default_factory=dict)

    @property
    def report_path(self) -> pathlib.Path:
        return self.root / "report.json"

    def event(self, kind: str, **fields: Any) -> None:
        item = {"ts": round(now(), 3), "kind": kind, **redact(fields)}
        self.events.append(item)
        with (self.root / "events.jsonl").open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(item, sort_keys=True) + "\n")

    def fail(self, message: str) -> None:
        self.failures.append(message)
        self.event("failure", message=message)

    def command(self, node: Node) -> list[str]:
        command = [str(self.args.binary), "--data-dir", str(node.data_dir)]
        if self.args.scenario in {"relay-only", "separate-network"}:
            command.append("--no-dht")
        if self.args.scenario in {"same-lan", "no-dht"}:
            command.extend(["--no-dht", "--no-relay"])
        # MCP is optional at runtime, but enables deterministic actions and
        # status capture whenever the selected Boru build supports it.
        if not self.args.no_mcp:
            command += ["--mcp", "--enable-gui-test-actions", "--mcp-bind", f"127.0.0.1:{node.mcp_port}"]
        return command + list(self.args.binary_arg)

    def launch(self, node: Node) -> None:
        node.data_dir.mkdir(parents=True, exist_ok=True)
        log = node.log_path.open("wb")
        env = os.environ.copy()
        env["BORU_DATA_DIR"] = str(node.data_dir)
        env["BORU_CHAT_DATA_DIR"] = str(node.data_dir)
        node.process = subprocess.Popen(
            self.command(node), stdout=log, stderr=subprocess.STDOUT,
            cwd=self.args.cwd or None, env=env, start_new_session=True,
        )
        self.event("node_started", node=node.index, pid=node.process.pid, command=self.command(node))
        ready = wait_until(
            lambda: node.process is not None and node.process.poll() is None
            and (self.args.no_mcp or port_open(node.mcp_port)),
            self.args.readiness_timeout_s,
        )
        if not ready:
            reason = "process exited during startup" if node.process.poll() is not None else "MCP readiness timeout"
            self.fail(f"node {node.index} startup: {reason}")
            if self.args.fail_fast:
                raise RuntimeError(f"node {node.index} failed readiness")
        self.event("node_ready" if ready else "node_not_ready", node=node.index)

    def stop(self, node: Node, reason: str = "cleanup") -> None:
        process = node.process
        if process is None or process.poll() is not None:
            return
        self.event("node_stop", node=node.index, pid=process.pid, reason=reason)
        try:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=3)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                self.fail(f"node {node.index} did not terminate")

    def restart(self, node: Node) -> None:
        self.stop(node, "scheduled_restart")
        node.restarts += 1
        node.process = None
        self.launch(node)
        self.event("fault", node=node.index, fault="restart", restart_count=node.restarts)

    def capture(self) -> None:
        for node in self.nodes:
            if node.process is None:
                continue
            metrics = proc_metrics(node.process.pid, node.data_dir)
            history = self.metric_history.setdefault(node.index, {})
            for metric, value in metrics.items():
                if isinstance(value, (int, float)) or value is None:
                    history.setdefault(metric, []).append(value)
            status: dict[str, Any] = {"reachable": False}
            if not self.args.no_mcp and port_open(node.mcp_port):
                try:
                    status = rpc(node.mcp_port, "boru_get_node_status")
                except (OSError, ValueError, RuntimeError) as exc:
                    status = {"reachable": False, "error": type(exc).__name__}
            self.event("sample", node=node.index, pid=node.process.pid, metrics=metrics, mcp=status)

    def evaluate_assertions(self) -> list[dict[str, Any]]:
        """Evaluate sampled resources once, with caller-supplied limits only."""
        results: list[dict[str, Any]] = []
        for node, observations in self.metric_history.items():
            engine = AssertionEngine()
            for metric, samples in observations.items():
                rule = MetricRule(metric, warmup_samples=self.args.warmup_samples,
                                  max_final=self.args.max_final.get(metric),
                                  max_peak=self.args.max_peak.get(metric),
                                  max_slope=self.args.max_slope.get(metric))
                result = engine.metric(f"resource.{metric}", samples, rule, [node])
                results.append(result.as_dict())
            for result in results[-len(observations):]:
                if result["status"] == "FAIL":
                    self.fail(f"node {node} {result['name']}: {result['failure_code']}")
        return results

    def action(self, name: str, node_index: int | None = None) -> None:
        node = self.nodes[node_index if node_index is not None else 0]
        if name == "restart":
            self.restart(node)
            return
        if name == "burst":
            if self.args.no_mcp:
                self.event("action_skipped", action=name, reason="MCP disabled")
                return
            for i in range(3):
                try:
                    rpc(node.mcp_port, "boru_gui_set_composer", {"text": f"soak-burst-{i}"})
                    rpc(node.mcp_port, "boru_gui_submit_composer")
                except (OSError, ValueError, RuntimeError) as exc:
                    self.fail(f"burst action node {node.index}: {type(exc).__name__}")
            self.event("action", action=name, node=node.index, count=3)
            return
        if name == "offline":
            if node.process and node.process.poll() is None:
                os.killpg(node.process.pid, signal.SIGSTOP)
                self.event("fault", node=node.index, fault="offline", state="stopped")
                time.sleep(min(2.0, self.args.interval_s / 4))
                os.killpg(node.process.pid, signal.SIGCONT)
                self.event("fault", node=node.index, fault="offline", state="resumed")
            return
        self.event("action_skipped", action=name, reason="not available without a room fixture")

    def invariant_check(self, require_live: bool = True, check_exit: bool = True) -> None:
        live = 0
        for node in self.nodes:
            if node.process and node.process.poll() is None:
                live += 1
            if check_exit and node.process and node.process.poll() not in (None, 0) and not self.args.allow_node_exit:
                self.fail(f"node {node.index} exited with {node.process.returncode}")
        if require_live and live == 0:
            self.fail("all nodes exited before the run completed")

    def report(self, status: str) -> None:
        assertions = self.evaluate_assertions()
        assertions.append({"name": "nodes_live_during_run", "status": "FAIL" if self.failures else "PASS"})
        cleanup = {"verified": all(n.process is None or n.process.poll() is not None for n in self.nodes)}
        body = build_report(
            status=status, workflow="process-soak", seed=self.args.seed,
            scenario=self.args.scenario, profile=self.args.profile, nodes=len(self.nodes),
            **build_metadata(self.args.cwd),
            faults=self.args.faults, failures=self.failures, failure_codes=self.failures,
            event_count=len(self.events), run_dir=str(self.root), assertions=assertions,
            cleanup=cleanup, limitations=self.limitations(), events={"file": "events.jsonl", "count": len(self.events)},
            topology={"nodes": len(self.nodes), "scenario": self.args.scenario},
            resources={"samples": sum(1 for event in self.events if event.get("kind") == "sample")},
            started_at_unix_s=self.started, finished_at_unix_s=now(),
        )
        write_evidence(self.root, body)

        self.event("run_finished", status=status, failures=len(self.failures))

    def limitations(self) -> list[str]:
        limits = []
        if self.args.scenario == "separate-network":
            limits.append("controller cannot create a VPN or namespace; use the documented VM/VPN profile")
        limits.append("room/file/call actions require an existing MCP room fixture; unsupported actions are recorded")
        return limits

    def run(self) -> int:
        self.root.mkdir(parents=True, exist_ok=False)
        (self.root / "nodes").mkdir()
        self.event("run_started", scenario=self.args.scenario, profile=self.args.profile, nodes=self.args.nodes)
        for index in range(self.args.nodes):
            node = Node(index, self.root / "nodes" / f"node-{index}", self.root / "nodes" / f"node-{index}.log", self.args.mcp_base + index)
            self.nodes.append(node)
        try:
            for node in self.nodes:
                self.launch(node)
                if self.args.start_stagger_s > 0:
                    deadline = time.monotonic() + self.args.start_stagger_s
                    wait_until(lambda: time.monotonic() >= deadline, self.args.start_stagger_s + 0.1)
            deadline = now() + self.args.duration_s
            action_index = 0
            while now() < deadline:
                self.capture()
                self.invariant_check()
                if self.failures and self.args.fail_fast:
                    break
                if self.args.faults and action_index % max(1, self.args.fault_every) == 0:
                    self.action(self.args.faults[action_index % len(self.args.faults)], action_index % len(self.nodes))
                action_index += 1
                if self.args.duration_s <= 2:
                    break
                time.sleep(min(self.args.interval_s, max(0.05, deadline - now())))
        except (OSError, RuntimeError) as exc:
            self.fail(f"controller error: {type(exc).__name__}: {exc}")
        finally:
            for node in self.nodes:
                self.stop(node)
            self.invariant_check(require_live=False, check_exit=False)
        status = "PASS" if not self.failures else "FAIL"
        self.report(status)
        print(json.dumps({"status": status, "run_dir": str(self.root), "failures": self.failures}, sort_keys=True))
        return 0 if status == "PASS" else 1


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, default=None, help="Boru executable (defaults to target/debug/boru)")
    parser.add_argument("--binary-arg", action="append", default=[], help="extra binary argument; repeatable")
    parser.add_argument("--cwd", type=pathlib.Path)
    parser.add_argument("--profile", choices=sorted(PROFILES), default="developer")
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), default="no-dht")
    parser.add_argument("--nodes", type=int, default=None)
    parser.add_argument("--duration-s", type=float, default=None)
    parser.add_argument("--interval-s", type=float, default=None)
    parser.add_argument("--start-stagger-s", type=float, default=1.0)
    parser.add_argument("--readiness-timeout-s", type=float, default=15.0,
                        help="bounded startup readiness poll deadline")
    parser.add_argument("--mcp-base", type=int, default=19101)
    parser.add_argument("--run-dir", type=pathlib.Path)
    parser.add_argument("--seed", type=int, default=0xB0A75479)
    parser.add_argument("--fault", dest="faults", action="append", choices=["restart", "offline", "burst"], default=[])
    parser.add_argument("--fault-every", type=int, default=4)
    parser.add_argument("--no-mcp", action="store_true")
    parser.add_argument("--allow-node-exit", action="store_true")
    parser.add_argument("--fail-fast", action="store_true")
    parser.add_argument("--self-test", action="store_true", help="validate controller primitives without launching Boru")
    parser.add_argument("--preflight-only", action="store_true", help="check environment and exit without launching Boru")
    parser.add_argument("--workflow", choices=["golden-recovery"], help="run a bounded mock workflow instead of the process soak")
    parser.add_argument("--workflow-timeout-s", type=float, default=10.0)
    parser.add_argument("--repeat", type=int, default=1,
                        help="repeat the workflow with consecutive derived seeds")
    parser.add_argument("--warmup-samples", type=int, default=2)
    parser.add_argument("--max-rss-kb", type=float)
    parser.add_argument("--max-threads", type=float)
    parser.add_argument("--max-fds", type=float)
    parser.add_argument("--max-profile-db-bytes", type=float)
    parser.add_argument("--max-slope", action="append", default=[], metavar="METRIC=VALUE")
    args = parser.parse_args(argv)
    args.max_final = {"rss_kb": args.max_rss_kb, "threads": args.max_threads,
                      "fds": args.max_fds, "profile_db_bytes": args.max_profile_db_bytes}
    args.max_peak = dict(args.max_final)
    slope_limits = args.max_slope
    args.max_slope = {}
    for item in slope_limits:
        try:
            metric, value = item.split("=", 1)
            args.max_slope[metric] = float(value)
        except ValueError:
            parser.error("--max-slope must use METRIC=VALUE")
    profile = PROFILES[args.profile]
    args.nodes = args.nodes or profile["nodes"]
    args.duration_s = profile["duration_s"] if args.duration_s is None else args.duration_s
    args.interval_s = profile["interval_s"] if args.interval_s is None else args.interval_s
    if not 3 <= args.nodes <= 8:
        parser.error("--nodes must be between 3 and 8")
    if args.duration_s <= 0:
        parser.error("--duration-s must be positive")
    if args.readiness_timeout_s <= 0 or args.workflow_timeout_s <= 0:
        parser.error("readiness and workflow timeouts must be positive")
    if args.repeat <= 0:
        parser.error("--repeat must be positive")
    if args.binary is None:
        args.binary = pathlib.Path(__file__).resolve().parents[1] / "target" / "debug" / "boru"
    if not args.self_test and not args.workflow and not args.preflight_only and not args.binary.exists():
        parser.error(f"Boru binary not found: {args.binary}; build it first or pass --binary")
    if args.run_dir is None:
        args.run_dir = pathlib.Path("artifacts") / f"boru-soak-{time.strftime('%Y%m%d-%H%M%S')}-{args.seed:x}"
    return args


def run_workflow(args: argparse.Namespace) -> int:
    """Run the complete bounded workflow contract without raw payloads."""
    args.run_dir.mkdir(parents=True, exist_ok=True)
    results = []
    for attempt in range(args.repeat):
        seed = args.seed + attempt
        workflow = golden_recovery()
        result = WorkflowEngine(seed=seed).run(workflow)
        results.append(result)
        attempt_dir = args.run_dir / f"run-{attempt + 1:02d}-seed-{seed}"
        attempt_dir.mkdir(parents=True, exist_ok=True)
        (attempt_dir / "workflow.json").write_text(
            json.dumps(result.as_dict(), indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        if result.outcome != "PASS":
            break
    result = results[-1]
    status = "PASS" if len(results) == args.repeat and all(r.outcome == "PASS" for r in results) else "FAIL"
    body = build_report(
        status=status, workflow=result.name, seed=args.seed, run_id=result.run_id,
        **build_metadata(args.cwd),
        assertions=[{"name": record.name, "status": record.outcome, "kind": record.kind} for record in result.records],
        failure_codes=[r.failure_reason for r in results if r.failure_reason],
        failures=[r.failure_reason for r in results if r.failure_reason],
        cleanup={"verified": True},
        limitations=["fixture mode; use the real Boru binary for network-backed execution"],
        topology={"nodes": 3, "aliases": {"0": "node-a", "1": "node-b", "2": "node-c"}},
        aliases={"0": "node-a", "1": "node-b", "2": "node-c"},
        events={"file": "workflow.json", "count": sum(len(r.records) for r in results)},
        event_count=sum(len(r.records) for r in results),
        repeat={"requested": args.repeat, "completed": len(results), "seeds": [args.seed + i for i in range(len(results))]},
        run_dir=str(args.run_dir),
    )
    write_evidence(args.run_dir, body)
    (args.run_dir / "workflow.json").write_text(
        json.dumps({"runs": [r.as_dict() for r in results]}, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(body, sort_keys=True))
    return 0 if status == "PASS" else 1


def self_test() -> int:
    with __import__("tempfile").TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        assert redact({"token": "secret", "ok": 1})["token"] == "<redacted>"
        assert not port_open(1)
        assert proc_metrics(os.getpid(), root)["rss_kb"] is not None
        assert {"relay-only", "same-lan", "separate-network", "no-dht"} == SCENARIOS
        state = {"ready": False, "cleaned": False}
        context = WorkflowContext(seed=7)
        context.defer(lambda: state.__setitem__("cleaned", True))
        result = WorkflowEngine(seed=7).run(Workflow("test", (
            action("set", lambda _: state.__setitem__("ready", True)),
            poll("ready", lambda _: state["ready"], timeout_s=1),
        )), context)
        assert result.outcome == "PASS" and state["cleaned"]
        assert WorkflowEngine(seed=7).run(Workflow("test", (poll("never", lambda _: False, timeout_s=0.001),))).outcome == "FAIL"
    print("soak_harness self-test: PASS")
    return 0


if __name__ == "__main__":
    parsed = parse_args(sys.argv[1:])
    raise SystemExit(self_test() if parsed.self_test else preflight(parsed) if parsed.preflight_only else run_workflow(parsed) if parsed.workflow else Run(parsed.run_dir, parsed).run())
