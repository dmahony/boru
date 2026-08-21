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
import socket
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import Any

SCENARIOS = {"relay-only", "same-lan", "separate-network", "no-dht"}
PROFILES = {
    "developer": {"duration_s": 7200, "nodes": 3, "interval_s": 30},
    "release-candidate": {"duration_s": 28800, "nodes": 6, "interval_s": 60},
}
SENSITIVE_KEYS = {"secret", "token", "password", "private_key", "ticket", "payload"}


def redact(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: ("<redacted>" if k.lower() in SENSITIVE_KEYS else redact(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    if isinstance(value, str) and len(value) > 128:
        return value[:32] + "…<redacted>"
    return value


def now() -> float:
    return time.time()


def proc_metrics(pid: int, data_dir: pathlib.Path) -> dict[str, Any]:
    """Return metrics available on Linux; missing procfs fields are explicit."""
    result: dict[str, Any] = {"rss_kb": None, "threads": None, "fds": None, "db_bytes": 0}
    status = pathlib.Path(f"/proc/{pid}/status")
    if status.exists():
        for line in status.read_text(errors="replace").splitlines():
            if line.startswith("VmRSS:"):
                result["rss_kb"] = int(line.split()[1])
            elif line.startswith("Threads:"):
                result["threads"] = int(line.split()[1])
    fd_dir = pathlib.Path(f"/proc/{pid}/fd")
    if fd_dir.exists():
        try:
            result["fds"] = len(list(fd_dir.iterdir()))
        except OSError:
            pass
    if data_dir.exists():
        result["db_bytes"] = sum(p.stat().st_size for p in data_dir.rglob("*") if p.is_file())
    return result


def port_open(port: int) -> bool:
    with socket.socket() as sock:
        sock.settimeout(0.2)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def rpc(port: int, method: str, params: dict[str, Any] | None = None, timeout: float = 5) -> dict[str, Any]:
    request = {"jsonrpc": "2.0", "method": method, "params": params or {}, "id": 1}
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as conn:
        conn.settimeout(timeout)
        conn.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        line = conn.makefile("rb").readline()
    if not line:
        raise RuntimeError("empty MCP response")
    value = json.loads(line)
    return value if isinstance(value, dict) else {"result": value}


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
            status: dict[str, Any] = {"reachable": False}
            if not self.args.no_mcp and port_open(node.mcp_port):
                try:
                    status = rpc(node.mcp_port, "boru_get_node_status")
                except (OSError, ValueError, RuntimeError) as exc:
                    status = {"reachable": False, "error": type(exc).__name__}
            self.event("sample", node=node.index, pid=node.process.pid, metrics=metrics, mcp=status)

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
        body = {
            "schema": "boru-soak-report/v1",
            "status": status,
            "started_at_unix_s": self.started,
            "finished_at_unix_s": now(),
            "seed": self.args.seed,
            "scenario": self.args.scenario,
            "profile": self.args.profile,
            "nodes": len(self.nodes),
            "faults": self.args.faults,
            "failures": self.failures,
            "event_count": len(self.events),
            "run_dir": str(self.root),
            "cleanup_verified": all(n.process is None or n.process.poll() is not None for n in self.nodes),
            "limitations": self.limitations(),
        }
        self.report_path.write_text(json.dumps(redact(body), indent=2, sort_keys=True) + "\n", encoding="utf-8")
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
                time.sleep(self.args.start_stagger_s)
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
    parser.add_argument("--mcp-base", type=int, default=19101)
    parser.add_argument("--run-dir", type=pathlib.Path)
    parser.add_argument("--seed", type=int, default=0xB0A75479)
    parser.add_argument("--fault", dest="faults", action="append", choices=["restart", "offline", "burst"], default=[])
    parser.add_argument("--fault-every", type=int, default=4)
    parser.add_argument("--no-mcp", action="store_true")
    parser.add_argument("--allow-node-exit", action="store_true")
    parser.add_argument("--fail-fast", action="store_true")
    parser.add_argument("--self-test", action="store_true", help="validate controller primitives without launching Boru")
    args = parser.parse_args(argv)
    profile = PROFILES[args.profile]
    args.nodes = args.nodes or profile["nodes"]
    args.duration_s = profile["duration_s"] if args.duration_s is None else args.duration_s
    args.interval_s = profile["interval_s"] if args.interval_s is None else args.interval_s
    if not 3 <= args.nodes <= 8:
        parser.error("--nodes must be between 3 and 8")
    if args.duration_s <= 0:
        parser.error("--duration-s must be positive")
    if args.binary is None:
        args.binary = pathlib.Path(__file__).resolve().parents[1] / "target" / "debug" / "boru"
    if not args.self_test and not args.binary.exists():
        parser.error(f"Boru binary not found: {args.binary}; build it first or pass --binary")
    if args.run_dir is None:
        args.run_dir = pathlib.Path("artifacts") / f"boru-soak-{time.strftime('%Y%m%d-%H%M%S')}-{args.seed:x}"
    return args


def self_test() -> int:
    with __import__("tempfile").TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        assert redact({"token": "secret", "ok": 1})["token"] == "<redacted>"
        assert not port_open(1)
        assert proc_metrics(os.getpid(), root)["rss_kb"] is not None
        assert {"relay-only", "same-lan", "separate-network", "no-dht"} == SCENARIOS
    print("soak_harness self-test: PASS")
    return 0


if __name__ == "__main__":
    parsed = parse_args(sys.argv[1:])
    raise SystemExit(self_test() if parsed.self_test else Run(parsed.run_dir, parsed).run())
