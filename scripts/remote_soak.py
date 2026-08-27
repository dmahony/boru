#!/usr/bin/env python3
"""Deploy and run one Boru soak node per SSH host.

The local soak_harness launches several processes on one Linux host. This
controller keeps the same fail-closed evidence shape while placing each node
on a separate SSH host. The manifest contains hostnames and paths only; SSH
keys and credentials remain in the user's SSH configuration.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import shlex
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class Node:
    name: str
    host: str
    binary: pathlib.Path
    remote_binary: str
    mcp_port: int
    display: int
    data_dir: str
    architecture: str
    relay: bool
    bind_port: int

    @classmethod
    def from_dict(cls, value: dict[str, Any]) -> "Node":
        required = ("name", "host", "binary", "remote_binary", "mcp_port", "display", "data_dir", "architecture")
        missing = [key for key in required if key not in value]
        if missing:
            raise ValueError(f"node is missing: {', '.join(missing)}")
        return cls(
            name=str(value["name"]), host=str(value["host"]), binary=pathlib.Path(str(value["binary"])),
            remote_binary=str(value["remote_binary"]), mcp_port=int(value["mcp_port"]),
            display=int(value["display"]), data_dir=str(value["data_dir"]), architecture=str(value["architecture"]),
            relay=bool(value.get("relay", True)),
            bind_port=int(value.get("bind_port", 0)),
        )


def run_command(args: list[str], *, input_text: str | None = None, timeout: float = 30) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, input=input_text, text=True, capture_output=True, timeout=timeout, check=False)


def ssh_options() -> list[str]:
    jump = os.environ.get("BORU_SSH_JUMP")
    return ["-o", f"ProxyJump={jump}"] if jump else []


def ssh(node: Node, command: str, *, timeout: float = 30) -> subprocess.CompletedProcess[str]:
    return run_command(
        ["ssh", *ssh_options(), "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o", "ServerAliveInterval=5",
         "-o", "ServerAliveCountMax=2", node.host, command], timeout=timeout,
    )


def ssh_script(node: Node, script: str, *, timeout: float = 30) -> subprocess.CompletedProcess[str]:
    return run_command(
        ["ssh", *ssh_options(), "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", node.host, "bash", "-s"],
        input_text=script, timeout=timeout,
    )


def scp(node: Node, local: pathlib.Path, remote: str, *, timeout: float = 300) -> subprocess.CompletedProcess[str]:
    base = ["scp", *ssh_options(), "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]
    result = run_command([*base, str(local), f"{node.host}:{remote}"], timeout=timeout)
    if result.returncode == 0:
        return result
    # Some minimal ARM images omit the SFTP subsystem but still support the
    # legacy SCP protocol over the authenticated SSH exec channel.
    result = run_command([*base, "-O", str(local), f"{node.host}:{remote}"], timeout=timeout)
    if result.returncode == 0:
        return result
    # Others have neither SFTP nor a remote scp executable. Stream the file
    # through the authenticated SSH channel; dd is present on the base image.
    data = local.read_bytes()
    streamed = subprocess.run(
        ["ssh", *ssh_options(), "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", node.host,
         f"umask 077; dd of={shlex.quote(remote)} status=none"],
        input=data, capture_output=True, timeout=timeout, check=False,
    )
    return subprocess.CompletedProcess(streamed.args, streamed.returncode,
                                       streamed.stdout.decode(errors="replace"),
                                       streamed.stderr.decode(errors="replace"))


def detail(result: subprocess.CompletedProcess[str]) -> str:
    text = (result.stdout + result.stderr).strip().replace("\n", " ")
    return text[:400]


def check(node: Node) -> dict[str, Any]:
    expected = shlex.quote(node.architecture)
    result = ssh(node, f"arch=$(uname -m); xvfb=$(command -v xvfb-run || true); sha=$(command -v sha256sum || true); printf 'arch=%s xvfb=%s sha256sum=%s\\n' \"$arch\" \"$xvfb\" \"$sha\"; test \"$arch\" = {expected} && test -n \"$xvfb\" && test -n \"$sha\"")
    return {"node": node.name, "host": node.host, "ok": result.returncode == 0,
            "output": detail(result), "expected_architecture": node.architecture}


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def deploy(node: Node) -> dict[str, Any]:
    if not node.binary.is_file() or not os.access(node.binary, os.X_OK):
        return {"node": node.name, "ok": False, "error": f"binary is not executable: {node.binary}"}
    local_hash = sha256(node.binary)
    prep = ssh(node, f"mkdir -p {shlex.quote(os.path.dirname(node.remote_binary) or '.') } {shlex.quote(node.data_dir)}")
    if prep.returncode:
        return {"node": node.name, "ok": False, "error": f"prepare: {detail(prep)}"}
    remote_tmp = node.remote_binary + ".tmp"
    copied = scp(node, node.binary, remote_tmp)
    if copied.returncode:
        return {"node": node.name, "ok": False, "error": f"copy: {detail(copied)}"}
    verify = ssh(node, f"chmod 755 {shlex.quote(remote_tmp)} && sha256sum {shlex.quote(remote_tmp)} && mv -f {shlex.quote(remote_tmp)} {shlex.quote(node.remote_binary)}")
    remote_hash = detail(verify).split()[0] if verify.returncode == 0 and detail(verify) else ""
    return {"node": node.name, "ok": verify.returncode == 0 and remote_hash == local_hash,
            "local_sha256": local_hash, "remote_sha256": remote_hash, "output": detail(verify)}


def launch_script(node: Node) -> str:
    binary = shlex.quote(node.remote_binary)
    data = shlex.quote(node.data_dir)
    pid = shlex.quote(node.data_dir + "/remote-soak.pid")
    log = shlex.quote(node.data_dir + "/remote-soak.log")
    cleanup = cleanup_script(node)
    display = shlex.quote(f":{node.display}")
    # Record the actual Boru PID.  xvfb-run is a shell wrapper and may exit
    # after handing off to the application, which made the old PID file report
    # false process exits while Boru was still serving MCP.
    command = (f"{cleanup} && mkdir -p {data} && rm -f {pid} && "
               f"(nohup setsid bash -c "
               f"'Xvfb {display} -screen 0 1280x720x24 -nolisten tcp & "
               f"xvfb_pid=$!; trap \"kill $xvfb_pid 2>/dev/null || true\" EXIT TERM INT; "
               f"export DISPLAY={display}; exec {binary} --mcp --enable-gui-test-actions "
               f"--mcp-bind 127.0.0.1:{node.mcp_port} --data-dir {data} "
               f"--bind-port {node.bind_port} --no-dht "
               f"{'--no-relay ' if not node.relay else ''}'"
               f">{log} 2>&1 < /dev/null & "
               f"printf '%s\\n' $! >{pid}; printf '%s\\n' $!)")
    return command


def cleanup_script(node: Node) -> str:
    """Stop the exact run process group, even when its PID file is stale."""
    pidfile = node.data_dir + "/remote-soak.pid"
    code = f"""
import os, pathlib, signal, sys, time
data_dir = {node.data_dir!r}
pidfile = pathlib.Path({pidfile!r})
run_displays = {{{node.display}, {node.display + 1}, {node.display + 2}, {node.display + 3}}}

def matching_groups():
    groups = set()
    for entry in pathlib.Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            pid = int(entry.name)
            argv = (entry / 'cmdline').read_bytes().split(b'\\0')
            argv = [item.decode(errors='replace') for item in argv if item]
            if not argv:
                continue
            command = ' '.join(argv)
            executable = os.path.basename(argv[0])
            is_boru = executable == os.path.basename({node.remote_binary!r}) and data_dir in command
            is_xvfb = executable == 'Xvfb' and any(f':{{display}}' in command for display in run_displays)
            if is_boru or is_xvfb:
                groups.add(os.getpgid(pid))
        except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError):
            continue
    try:
        groups.add(os.getpgid(int(pidfile.read_text().strip())))
    except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError):
        pass
    groups.discard(os.getpgrp())
    return groups

for pgid in matching_groups():
    try:
        os.killpg(pgid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        pass
for _ in range(50):
    if not matching_groups():
        break
    time.sleep(0.1)
for pgid in matching_groups():
    try:
        os.killpg(pgid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass
try:
    pidfile.unlink()
except FileNotFoundError:
    pass
remaining = matching_groups()
if remaining:
    print(f'run process groups remain: {{sorted(remaining)}}', file=sys.stderr)
    raise SystemExit(23)
"""
    return f"python3 -c {shlex.quote(code)}"


def stop_script(node: Node) -> str:
    return cleanup_script(node)


def sample_script(node: Node) -> str:
    pid = shlex.quote(node.data_dir + "/remote-soak.pid")
    return (f"p=$(cat {pid} 2>/dev/null || true); alive=0; rss=; threads=; fds=; listen=0; "
            f"if test -n \"$p\" && kill -0 \"$p\" 2>/dev/null; then alive=1; "
            f"rss=$(awk '/^VmRSS:/{{print $2}}' /proc/$p/status 2>/dev/null || true); "
            f"threads=$(awk '/^Threads:/{{print $2}}' /proc/$p/status 2>/dev/null || true); "
            f"fds=$(find /proc/$p/fd -maxdepth 1 -type l 2>/dev/null | wc -l); "
            f"ss -ltn 2>/dev/null | grep -q ':{node.mcp_port} ' && listen=1; fi; "
            f"printf '{{\"alive\":%s,\"rss_kb\":\"%s\",\"threads\":\"%s\",\"fds\":\"%s\",\"mcp_listening\":%s}}\\n' $alive \"$rss\" \"$threads\" \"$fds\" $listen")


def mcp_call(node: Node, method: str, params: dict[str, Any] | None = None, timeout: float = 5) -> subprocess.CompletedProcess[str]:
    payload = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    code = (
        "import json,socket,sys; "
        f"r={payload!r}; "
        f"s=socket.create_connection(('127.0.0.1',{node.mcp_port}),{timeout}); "
        "s.sendall((json.dumps(r,separators=(',',':'))+'\\n').encode()); "
        "v=json.loads(s.makefile('rb').readline().decode()); print(json.dumps(v)); s.close(); "
        "sys.exit(1 if 'error' in v else 0)"
    )
    return ssh(node, f"python3 -c {shlex.quote(code)}", timeout=timeout + 10)


def wait_for_mcp(node: Node, timeout: float = 120) -> tuple[bool, str]:
    """Require a live Boru PID and successful JSON-RPC before sampling."""
    deadline = time.monotonic() + timeout
    last = "not attempted"
    while time.monotonic() < deadline:
        sample = ssh(node, sample_script(node), timeout=20)
        ping = mcp_call(node, "boru_ping", timeout=5)
        last = detail(sample) or detail(ping)
        if sample.returncode == 0 and ping.returncode == 0:
            try:
                value = json.loads(sample.stdout.strip().splitlines()[-1])
            except (ValueError, IndexError):
                value = {}
            if value.get("alive") and value.get("mcp_listening"):
                return True, last
        time.sleep(2)
    return False, last


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("check", "deploy", "run"))
    parser.add_argument("--manifest", type=pathlib.Path, required=True)
    parser.add_argument("--duration-s", type=float, default=7200)
    parser.add_argument("--interval-s", type=float, default=30)
    parser.add_argument("--fault-every", type=int, default=4)
    parser.add_argument("--run-dir", type=pathlib.Path, required=True)
    args = parser.parse_args(argv)
    try:
        values = json.loads(args.manifest.read_text(encoding="utf-8"))
        nodes = [Node.from_dict(item) for item in values["nodes"]]
    except (OSError, ValueError, json.JSONDecodeError, KeyError) as exc:
        parser.error(f"invalid manifest: {exc}")
    if len(nodes) < 3:
        parser.error("manifest must contain at least three nodes")
    if args.action == "check":
        results = list(map(check, nodes))
        print(json.dumps({"status": "PASS" if all(item["ok"] for item in results) else "FAIL", "nodes": results}, indent=2))
        return 0 if all(item["ok"] for item in results) else 1
    if args.action == "deploy":
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(nodes)) as pool:
            results = list(pool.map(deploy, nodes))
        print(json.dumps({"status": "PASS" if all(item["ok"] for item in results) else "FAIL", "nodes": results}, indent=2))
        return 0 if all(item["ok"] for item in results) else 1

    args.run_dir.mkdir(parents=True, exist_ok=False)
    events: list[dict[str, Any]] = []
    failures: list[str] = []
    started = time.time()
    def event(kind: str, **fields: Any) -> None:
        item = {"ts": round(time.time(), 3), "kind": kind, **fields}
        events.append(item)
        with (args.run_dir / "events.jsonl").open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(item, sort_keys=True) + "\n")
    event("run_started", duration_s=args.duration_s, nodes=[node.name for node in nodes], faults=["restart", "offline", "burst"])
    starts = list(map(lambda node: ssh(node, launch_script(node)), nodes))
    for node, result in zip(nodes, starts):
        event("node_started", node=node.name, ok=result.returncode == 0, detail=detail(result))
        if result.returncode:
            failures.append(f"{node.name}: launch failed")
    if not failures:
        for node in nodes:
            ready, output = wait_for_mcp(node)
            event("node_ready", node=node.name, ok=ready, detail=output)
            if not ready:
                failures.append(f"{node.name}: MCP readiness timeout")
    action_index = 0
    try:
        deadline = time.time() + args.duration_s
        while time.time() < deadline and not failures:
            samples = list(map(lambda node: ssh(node, sample_script(node)), nodes))
            for node, result in zip(nodes, samples):
                event("sample", node=node.name, ok=result.returncode == 0, output=detail(result))
                if result.returncode:
                    failures.append(f"{node.name}: sample failed")
                else:
                    try:
                        sample = json.loads(result.stdout.strip().splitlines()[-1])
                    except (ValueError, IndexError):
                        failures.append(f"{node.name}: invalid sample response")
                    else:
                        if not sample.get("alive"):
                            failures.append(f"{node.name}: remote process exited before sample")
            if action_index % max(1, args.fault_every) == 0:
                node = nodes[action_index % len(nodes)]
                fault = ("restart", "offline", "burst")[action_index % 3]
                if fault == "restart":
                    result = ssh(node, launch_script(node))
                    if result.returncode == 0:
                        ready, output = wait_for_mcp(node)
                        result = subprocess.CompletedProcess(
                            result.args,
                            0 if ready else 1,
                            result.stdout,
                            result.stderr + output,
                        )
                elif fault == "offline":
                    result = ssh(node, f"p=$(cat {shlex.quote(node.data_dir + '/remote-soak.pid')}); kill -STOP $p; sleep 2; kill -CONT $p")
                else:
                    result = mcp_call(node, "boru_gui_set_composer", {"text": f"remote-soak-{action_index}"})
                    if result.returncode == 0:
                        result = mcp_call(node, "boru_gui_submit_composer")
                event("fault", node=node.name, fault=fault, ok=result.returncode == 0, detail=detail(result))
                if result.returncode:
                    failures.append(f"{node.name}: {fault} failed")
            action_index += 1
            time.sleep(min(args.interval_s, max(0.05, deadline - time.time())))
    except KeyboardInterrupt:
        failures.append("interrupted by operator")
    finally:
        stops = list(map(lambda node: ssh(node, stop_script(node)), nodes))
        for node, result in zip(nodes, stops):
            event("node_stopped", node=node.name, ok=result.returncode == 0, detail=detail(result))
            if result.returncode:
                failures.append(f"{node.name}: cleanup failed")
    report = {"schema": "boru-remote-soak-report/v1", "status": "PASS" if not failures else "FAIL",
              "started_at_unix_s": started, "finished_at_unix_s": time.time(), "nodes": [node.name for node in nodes],
              "duration_s": args.duration_s, "failures": failures, "cleanup_verified": not any("cleanup failed" in item for item in failures),
              "limitations": ["remote process harness does not prove room/file/call/screen-share assertions without a prepared MCP fixture",
                              "remote metrics and logs are compact summaries; full logs remain on each host"]}
    (args.run_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, sort_keys=True))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
