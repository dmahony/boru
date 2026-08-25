#!/usr/bin/env python3
"""Run fixture-backed MCP assertions against already-running Boru nodes.

The RC controller owns process lifecycle. This companion owns application
assertions and fails closed when a fixture or MCP capability is unavailable.
The manifest contains SSH host aliases and MCP ports only.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import pathlib
import re
import shlex
import time
from typing import Any

from remote_soak import Node, mcp_call, ssh, ssh_script

LOBBY_ROOM = "9021bd1ed0932e4fb1dfd5477ebee17916eb431c892d44f16287962329eaf303"
RELAY_URL = "https://boru.chat:8443/"


def call(node: Node, method: str, params: dict[str, Any] | None = None, timeout: float = 15) -> dict[str, Any]:
    result = mcp_call(node, method, params, timeout=timeout)
    text = (result.stdout + result.stderr).strip().splitlines()
    if not text:
        return {"transport_ok": False, "error": "empty response", "returncode": result.returncode}
    try:
        value = json.loads(text[-1])
    except json.JSONDecodeError:
        return {"transport_ok": False, "error": text[-1], "returncode": result.returncode}
    return {"transport_ok": result.returncode == 0, "response": value, "returncode": result.returncode}


def result_value(value: dict[str, Any]) -> dict[str, Any]:
    response = value.get("response", {})
    return response.get("result", {}) if isinstance(response, dict) else {}


def remote_ip(node: Node) -> str:
    result = ssh(node, "hostname -I | tr ' ' '\\n' | grep -E '^[0-9]+\\.' | head -1", timeout=20)
    if result.returncode or not result.stdout.strip():
        raise RuntimeError(f"{node.name}: cannot determine remote IPv4 address")
    return result.stdout.strip()


def bootstrap_room(nodes: list[Node], room_id: str) -> dict[str, Any]:
    if any(node.bind_port == 0 for node in nodes):
        raise RuntimeError("bootstrap requires non-zero bind_port for every node")
    statuses = [call(node, "boru_get_node_status", timeout=25) for node in nodes]
    node_ids = [result_value(status).get("node_id") for status in statuses]
    if any(not node_id for node_id in node_ids):
        raise RuntimeError("bootstrap could not resolve every node identity")
    addresses = [remote_ip(node) for node in nodes]
    action_ids: list[str] = []
    for index, node in enumerate(nodes):
        peers = [
            {
                "node_id": node_ids[peer_index],
                "addr": f"{addresses[peer_index]}:{nodes[peer_index].bind_port}",
                "relay_url": RELAY_URL,
            }
            for peer_index in range(len(nodes))
            if peer_index != index
        ]
        queued = call(
            node,
            "boru_gui_bootstrap_room",
            {"room_id": room_id, "bootstrap_peers": peers},
            timeout=25,
        )
        action_id = result_value(queued).get("action_id")
        if not queued.get("transport_ok") or not action_id:
            raise RuntimeError(f"{node.name}: bootstrap request failed: {queued}")
        action_ids.append(action_id)
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        states = []
        for node, action_id in zip(nodes, action_ids):
            status = call(node, "boru_gui_get_action_status", {"action_id": action_id}, timeout=25)
            states.append(result_value(status).get("status", {}))
        if all(state.get("state") == "completed" for state in states):
            return {"node_ids": node_ids, "addresses": addresses, "actions": states}
        if any(state.get("state") == "rejected" for state in states):
            raise RuntimeError(f"room bootstrap rejected: {states}")
        time.sleep(1)
    raise RuntimeError("room bootstrap timed out")


def create_remote_fixture_file(node: Node, path: str) -> tuple[int, str]:
    payload = b"boru-rc-file-fixture-v1\n" + bytes(range(256)) * 16
    encoded = base64.b64encode(payload).decode("ascii")
    script = f"set -eu\nmkdir -p {shlex.quote(str(pathlib.PurePosixPath(path).parent))}\nprintf '%s' {shlex.quote(encoded)} | base64 -d > {shlex.quote(path)}\n"
    result = ssh_script(node, script, timeout=30)
    if result.returncode:
        raise RuntimeError(f"{node.name}: fixture file creation failed: {result.stderr.strip()}")
    return len(payload), hashlib.sha256(payload).hexdigest()


def registered_file(node: Node, filename: str) -> tuple[str, int]:
    result = call(node, "boru_get_local_shared_files", timeout=25)
    files = result_value(result).get("files", [])
    file_info = next((item for item in files if item.get("display_name") == filename), None)
    if not result.get("transport_ok") or not file_info:
        raise RuntimeError(f"{node.name}: shared file is not registered")
    return str(file_info["content_hash"]), int(file_info["size_bytes"])


def wait_for_action(node: Node, action_id: str, timeout: float = 90) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status = call(node, "boru_gui_get_action_status", {"action_id": action_id}, timeout=25)
        value = result_value(status).get("status", {})
        if value.get("state") in {"completed", "rejected"}:
            return value
        time.sleep(1)
    raise RuntimeError(f"{node.name}: GUI action {action_id} timed out")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=pathlib.Path, required=True)
    parser.add_argument("--room-id", default=LOBBY_ROOM)
    parser.add_argument("--message", default="rc-fixture-message")
    parser.add_argument(
        "--prejoined",
        action="store_true",
        help="use a room joined through an explicit bootstrap ticket",
    )
    parser.add_argument(
        "--bootstrap-room",
        action="store_true",
        help="create/join the room through explicit manifest bind ports and peer addresses",
    )
    parser.add_argument(
        "--file-source",
        default="/home/dan/boru-test/rc-fixture.bin",
        help="remote source path for the deterministic file-transfer assertion",
    )
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    values = json.loads(args.manifest.read_text(encoding="utf-8"))
    nodes = [Node.from_dict(item) for item in values["nodes"]]
    report: dict[str, Any] = {
        "schema": "boru-rc-fixture-report/v1",
        "started_at_unix_s": time.time(),
        "room_id": args.room_id,
        "nodes": {},
        "assertions": {
            "room_membership": "NOT_VALIDATED",
            "message_delivery": "NOT_VALIDATED",
            "file_transfer": "NOT_VALIDATED",
            "call_lifecycle": "NOT_AVAILABLE",
            "screen_share_lifecycle": "NOT_AVAILABLE",
        },
        "failures": [],
    }
    bootstrap_info: dict[str, Any] = {}
    if args.bootstrap_room:
        try:
            bootstrap_info = bootstrap_room(nodes, args.room_id)
            report["bootstrap"] = bootstrap_info
        except Exception as error:
            report["failures"].append(f"room bootstrap failed: {error}")
    node_ids: dict[str, str] = {}
    room_membership_ok = True
    for node in nodes:
        entry: dict[str, Any] = {}
        status = call(node, "boru_get_node_status")
        entry["node_status"] = status
        response = status.get("response", {})
        node_id = response.get("result", {}).get("node_id") if isinstance(response, dict) else None
        if node_id:
            node_ids[node.name] = node_id
        if args.bootstrap_room and bootstrap_info:
            joined = {
                "transport_ok": True,
                "response": {"result": {"joined": True, "room_id": args.room_id}},
                "returncode": 0,
            }
        elif args.prejoined:
            joined = {
                "transport_ok": True,
                "response": {"result": {"joined": True, "room_id": args.room_id}},
                "returncode": 0,
            }
        else:
            joined = call(node, "boru_join_lobby_room", {"timeout_ms": 60_000}, timeout=75)
        entry["join_lobby"] = joined
        room = call(node, "boru_get_room_status", {"room_id": args.room_id})
        entry["room_status"] = room
        if not joined.get("transport_ok") or not room.get("transport_ok"):
            report["failures"].append(f"{node.name}: room fixture unavailable")
            room_membership_ok = False
        else:
            pass
        report["nodes"][node.name] = entry
    if room_membership_ok and len(node_ids) == len(nodes):
        report["assertions"]["room_membership"] = "PASS"
    if len(node_ids) >= 2 and report["assertions"]["room_membership"] == "PASS":
        names = list(node_ids)
        for index, node in enumerate(nodes):
            expected = node_ids[names[(index + 1) % len(names)]]
            result = call(node, "boru_run_gui_message_test", {
                "room_id": args.room_id,
                "message_text": f"{args.message}-{node.name}",
                "expected_peer_id": expected,
                "timeout_ms": 60_000,
            }, timeout=75)
            report["nodes"][node.name]["message_test"] = result
            if not result.get("transport_ok"):
                report["failures"].append(f"{node.name}: message assertion failed")
        if not report["failures"]:
            report["assertions"]["message_delivery"] = "PASS"
    else:
        report["failures"].append("message assertion skipped because room membership is unavailable")
    if report["assertions"]["room_membership"] == "PASS" and len(nodes) >= 2:
        source, destination = nodes[0], nodes[1]
        try:
            expected_size, expected_sha256 = create_remote_fixture_file(source, args.file_source)
            share = call(source, "boru_gui_test_share_file", {"path": args.file_source}, timeout=25)
            share_action = result_value(share).get("action_id")
            if not share.get("transport_ok") or not share_action:
                raise RuntimeError(f"share request failed: {share}")
            share_state = wait_for_action(source, share_action)
            if share_state.get("state") != "completed":
                raise RuntimeError(f"share action rejected: {share_state}")
            source_peer = node_ids[source.name]
            content_hash, registered_size = registered_file(source, pathlib.PurePosixPath(args.file_source).name)
            if registered_size != expected_size:
                raise RuntimeError(f"registered file size mismatch: {registered_size} != {expected_size}")
            grant = call(source, "boru_grant_file_read_access", {
                "content_hash": content_hash,
                "grantee_id": node_ids[destination.name],
            }, timeout=25)
            if not grant.get("transport_ok"):
                raise RuntimeError(f"file read grant failed: {grant}")
            catalogue: dict[str, Any] = {}
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                catalogue = call(destination, "boru_browse_peer_catalogue", {"peer_id": source_peer}, timeout=35)
                files = result_value(catalogue).get("files", [])
                if any(file.get("size_bytes") == expected_size for file in files):
                    break
                time.sleep(2)
            files = result_value(catalogue).get("files", [])
            file_meta = next((file for file in files if file.get("size_bytes") == expected_size), None)
            if not file_meta:
                raise RuntimeError(f"shared fixture file not present in catalogue: {catalogue}")
            download = call(destination, "boru_download_file", {
                "content_hash": file_meta["content_hash"],
                "peer_id": source_peer,
                "known_size": expected_size,
            }, timeout=35)
            download_id = result_value(download).get("download_id")
            if not download.get("transport_ok"):
                error_data = download.get("response", {}).get("error", {}).get("data", "")
                match = re.search(r"already exists \(id=(\d+), state=complete\)", error_data)
                if match:
                    download_id = int(match.group(1))
            if download_id is None:
                raise RuntimeError(f"download request failed: {download}")
            download_state: dict[str, Any] = {}
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                status = call(destination, "boru_get_download_status", {"download_id": download_id}, timeout=25)
                download_state = result_value(status)
                if download_state.get("state") in {"complete", "failed"}:
                    break
                time.sleep(2)
            if download_state.get("state") != "complete":
                raise RuntimeError(f"download did not complete: {download_state}")
            display_name = file_meta.get("display_name") or pathlib.PurePosixPath(args.file_source).name
            destination_path = pathlib.PurePosixPath(destination.data_dir) / display_name
            verified = ssh(destination, f"sha256sum {shlex.quote(str(destination_path))} && stat -c '%s' {shlex.quote(str(destination_path))}", timeout=25)
            fields = verified.stdout.split()
            if verified.returncode or len(fields) < 3 or fields[0] != expected_sha256 or int(fields[-1]) != expected_size:
                raise RuntimeError(f"downloaded file integrity mismatch: {verified.stdout.strip()} {verified.stderr.strip()}")
            report["file_transfer"] = {
                "source": source.name,
                "destination": destination.name,
                "size_bytes": expected_size,
                "sha256": expected_sha256,
                "content_hash": file_meta["content_hash"],
                "download_id": download_id,
            }
            report["assertions"]["file_transfer"] = "PASS"
        except Exception as error:
            report["failures"].append(f"file transfer assertion failed: {error}")
    report["finished_at_unix_s"] = time.time()
    report["status"] = "PASS" if not report["failures"] else "NOT_VALIDATED"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"status": report["status"], "output": str(args.output), "failures": report["failures"]}, sort_keys=True))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
