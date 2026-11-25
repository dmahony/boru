#!/usr/bin/env python3
"""BRT-028 WebDriver harness.

Room bootstrap uses the reviewer-approved SQLite fixture in the disposable
Tauri profile because the desktop UI has no create/join affordance. The approved
fallback harness seeds `kv_store.conversations` with the exact BRT-028 room
fixture after the first launch creates `boru.db`; assertions then exercise the
real Tauri/WebKit UI and Iced peer traffic. Profile isolation is via
`XDG_DATA_HOME`/`XDG_CONFIG_HOME` (there is no Tauri `--profile-dir` option):
restart with the same environment rooted at `--profile-dir`. Run under one
Xvfb display (for example ``xvfb-run -a python3 scripts/e2e/brt028_driver.py``).
"""
from __future__ import annotations

import argparse
import json
import os
import socket
import tempfile
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import traceback
from datetime import datetime, timezone
from typing import Callable, Any


PRODUCTION_PATHS = (
    Path.home() / ".local/share/computer.iroh.boru.desktop",
    Path.home() / ".config/computer.iroh.boru.desktop",
    Path.home() / ".local/share/boru",
    Path.home() / ".config/boru",
)
POLL_INTERVAL = 0.2
STARTUP_TIMEOUT = 60.0
DRIVER_URL = "http://127.0.0.1:4444"


def wait_until(predicate: Callable[[], Any], timeout: float, description: str) -> Any:
    """Poll a predicate and return its truthy value, or raise with last observation."""
    deadline = time.monotonic() + timeout
    last: Any = None
    while True:
        last = predicate()
        if last:
            return last
        if time.monotonic() >= deadline:
            raise TimeoutError(f"Timed out waiting for {description}; last observed value: {last!r}")
        time.sleep(POLL_INTERVAL)


class Driver:
    def __init__(self, artifacts: Path):
        self.artifacts = artifacts
        self.shots = artifacts / "shots"
        self.shots.mkdir(parents=True, exist_ok=True)
        self.step_log = (artifacts / "steps.jsonl").open("a", encoding="utf-8")
        self.processes: list[subprocess.Popen] = []
        self.xvfb: subprocess.Popen | None = None
        self.tauri_driver: subprocess.Popen | None = None
        self.peer: subprocess.Popen | None = None
        self.peer_log_path: Path | None = None
        self.peer_trace_path: Path | None = None
        self.started_at = time.monotonic()
        self.web: Any = None

    def spawn(self, argv: list[str], *, env: dict[str, str] | None = None,
              stdout: Any = subprocess.PIPE, stderr: Any = subprocess.STDOUT,
              name: str) -> subprocess.Popen:
        proc = subprocess.Popen(argv, env=env, stdout=stdout, stderr=stderr,
                                start_new_session=True, text=True, bufsize=1)
        proc._boru_name = name
        self.processes.append(proc)
        return proc

    @staticmethod
    def _drain(proc: subprocess.Popen | None) -> str:
        if proc is None or proc.stdout is None:
            return ""
        # Drivers and app logs are also persisted to files; avoid blocking on
        # pipes here, as readiness diagnostics must not deadlock the process.
        return ""

    def diagnostic(self, peer: str, signal: str, started: float) -> str:
        elapsed = time.monotonic() - started
        blocks = [f"Readiness timeout: peer={peer}, signal={signal}, elapsed={elapsed:.1f}s"]
        for name, proc, log_path in (
            ("tauri-driver", self.tauri_driver, self.artifacts / "tauri-driver.log"),
            ("peer", self.peer, self.peer_log_path),
        ):
            output = self._drain(proc)
            if log_path and log_path.exists():
                output += log_path.read_text(errors="replace")[-12000:]
            blocks.append(f"--- {name} stdout/stderr ---\\n{output or '<empty>'}")
        if self.peer_trace_path and self.peer_trace_path.exists():
            blocks.append("--- peer readiness trace ---\\n" +
                          self.peer_trace_path.read_text(errors="replace")[-12000:])
        return "\\n".join(blocks)

    def start_infrastructure(self, profile: Path, tauri_bin: Path, peer_bin: Path,
                             peer_port: int) -> None:
        env = os.environ.copy()
        env["XDG_DATA_HOME"] = str(profile / "tauri" / "data")
        env["XDG_CONFIG_HOME"] = str(profile / "tauri" / "config")
        Path(env["XDG_DATA_HOME"]).mkdir(parents=True, exist_ok=True)
        Path(env["XDG_CONFIG_HOME"]).mkdir(parents=True, exist_ok=True)
        driver_log = self.artifacts / "tauri-driver.log"
        driver_out = driver_log.open("w", encoding="utf-8")
        self.tauri_driver = self.spawn(
            [shutil.which("tauri-driver") or "tauri-driver", "--native-driver",
             "/usr/bin/WebKitWebDriver", "--port", "4444"],
            env=env, stdout=driver_out, stderr=subprocess.STDOUT, name="tauri-driver")
        started = time.monotonic()
        try:
            wait_until(lambda: self.tauri_driver.poll() is None and self._tcp_open("127.0.0.1", 4444),
                       STARTUP_TIMEOUT, "tauri-driver TCP 127.0.0.1:4444")
        except TimeoutError as exc:
            raise RuntimeError(self.diagnostic("tauri-driver", "TCP 127.0.0.1:4444", started)) from exc
        # Bootstrap the profile schema using a real app session, then close it
        # before seeding and starting the peer.
        web = self.open_webdriver(tauri_bin)
        started = time.monotonic()
        db = Path(env["XDG_DATA_HOME"]) / "computer.iroh.boru.desktop" / "boru.db"
        try:
            wait_until(lambda: db if db.is_file() else False, STARTUP_TIMEOUT,
                       "Tauri profile DB creation (boru.db)")
        except TimeoutError as exc:
            self.save_webdriver_logs(web)
            raise RuntimeError(self.diagnostic("tauri", f"profile DB {db}", started)) from exc
        finally:
            web.quit()
        self.seed_conversation(db)
        # Approved fallback: seed the fixture in this disposable SQLite profile.
        # The user-visible assertions below still use the real Tauri UI.
        # The approved fixture seed is performed by the dependent seeding step
        # after this schema-bootstrap session has exited.
        peer_data = profile / "peer" / "data"
        peer_data.mkdir(parents=True, exist_ok=True)
        self.peer_log_path = self.artifacts / "peer.stdout-stderr.log"
        self.peer_trace_path = peer_data / "logs" / "boru.log"
        peer_out = self.peer_log_path.open("w", encoding="utf-8")
        self.peer = self.spawn(
            [str(peer_bin), "--data-dir", str(peer_data), "--mcp", "--mcp-bind",
             f"127.0.0.1:{peer_port}", "--enable-gui-test-actions", "--no-relay",
             "--no-dht", "--keep-running"], env=env, stdout=peer_out,
            stderr=subprocess.STDOUT, name="Iced/CLI peer")
        started = time.monotonic()
        try:
            wait_until(lambda: self.peer.poll() is None and self.peer_trace_path.exists() and
                       "MCP diagnostic server listening on" in self.peer_trace_path.read_text(
                           errors="replace"), STARTUP_TIMEOUT,
                       "Iced peer log: MCP diagnostic server listening on")
        except TimeoutError as exc:
            raise RuntimeError(self.diagnostic("Iced/CLI", "MCP diagnostic server listening on", started)) from exc
        self.web = self.open_webdriver(tauri_bin)

    @staticmethod
    def _tcp_open(host: str, port: int) -> bool:
        try:
            with socket.create_connection((host, port), timeout=0.2):
                return True
        except OSError:
            return False

    def open_webdriver(self, tauri_bin: Path) -> Any:
        try:
            from selenium import webdriver
            from selenium.webdriver.chrome.options import Options
        except ImportError as exc:
            raise RuntimeError("Selenium is required on the execution host (DEBSRV)") from exc
        options = Options()
        options.set_capability("browserName", "wry")
        options.set_capability("tauri:options", {"application": str(tauri_bin)})
        options.set_capability("timeouts", {"implicit": 0, "pageLoad": 30000, "script": 30000})
        return webdriver.Remote(command_executor=DRIVER_URL, options=options)

    @staticmethod
    def seed_conversation(db: Path) -> None:
        import sqlite3
        conversation = {"schema_version": 1, "conversations": [{
            "topic": [144, 33, 189, 30, 208, 147, 46, 79, 177, 223, 213, 71,
                      126, 190, 225, 121, 22, 235, 67, 28, 137, 45, 68, 241,
                      98, 135, 150, 35, 41, 234, 243, 3],
            "name": "BRT-028 Room", "kind": "Group", "created_at_unix_ms": 0,
            "last_seen_at_unix_ms": 0, "unread_count": 0, "archived": False,
            "visibility": "Private"}]}
        with sqlite3.connect(db) as connection:
            connection.execute("INSERT INTO kv_store(key,value,updated_at_ms) VALUES(?,?,0) "
                               "ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                               ("conversations", json.dumps(conversation, separators=(",", ":"))))

    def save_webdriver_logs(self, web: Any) -> None:
        try:
            logs = web.get_log("browser")
        except Exception as exc:
            logs = [{"error": str(exc)}]
        (self.artifacts / "webdriver-browser.log").write_text(json.dumps(logs, indent=2))

    def assertion(self, step_id: str, group: int, fn: Callable[[], Any]) -> None:
        started = time.monotonic()
        started_at = datetime.now(timezone.utc).isoformat()
        status, message = "pass", ""
        try:
            fn()
        except BaseException as exc:
            status, message = "fail", f"{type(exc).__name__}: {exc}"
            raise
        finally:
            screenshot_path = ""
            screenshot_error = None
            try:
                screenshot_path = self.screenshot(step_id, status)
            except Exception as exc:
                screenshot_error = exc
                if status == "pass":
                    status, message = "fail", f"Screenshot failed: {exc}"
                else:
                    message += f"; screenshot failed: {exc}"
            record = {"step_id": step_id, "group": group, "status": status,
                      "started_at": started_at, "duration_ms": round((time.monotonic()-started)*1000),
                      "message": message, "screenshot_path": screenshot_path}
            self.step_log.write(json.dumps(record, separators=(",", ":")) + "\\n")
            self.step_log.flush()
            if screenshot_error is not None and status == "pass":
                raise RuntimeError(f"Could not capture assertion screenshot: {screenshot_error}") from screenshot_error


    def screenshot(self, step_id: str, status: str) -> str:
        path = self.shots / f"{step_id}-{'pass' if status == 'pass' else 'fail'}.png"
        capture = shutil.which("import")
        if capture:
            subprocess.run([capture, "-window", "root", str(path)], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        elif shutil.which("scrot"):
            subprocess.run([shutil.which("scrot"), str(path)], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        else:
            raise RuntimeError("Screenshot requires ImageMagick 'import' or scrot")
        return str(path)

    def teardown(self) -> None:
        # Start in new sessions; signal the process group so descendants die too.
        for proc in [*self.processes, *([self.xvfb] if self.xvfb else [])]:
            if proc is None or proc.poll() is not None:
                continue
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                continue
        for proc in [*self.processes, *([self.xvfb] if self.xvfb else [])]:
            if proc is None:
                continue
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                proc.wait()
        self.step_log.close()


def assertion_1(driver: Driver, profile: Path, tauri_bin: Path, peer_bin: Path,
                peer_port: int) -> None:
    def start_and_wait() -> None:
        driver.start_infrastructure(profile, tauri_bin, peer_bin, peer_port)
        wait_until(lambda: bool(driver.web and driver.peer and driver.peer.poll() is None),
                   STARTUP_TIMEOUT, "Tauri WebDriver session + live Iced peer")
    driver.assertion("group-1-readiness", 1, start_and_wait)


def assertion_2_room_list(driver: Driver) -> None:
    expected_name = "BRT-028 Room"
    selector = ".room-list .room"
    def observe() -> list[str]:
        return driver.web.execute_script("return Array.from(document.querySelectorAll(arguments[0])).map(e=>e.innerText.trim())", selector)
    rooms = wait_until(lambda: (values if expected_name in (values := observe()) else False),
                       30.0, f"room selector {selector} contains {expected_name!r}")
    if expected_name not in rooms:
        raise AssertionError(f"selector={selector}; expected={expected_name!r}, observed={rooms!r}")


def assertion_3_open_and_send(driver: Driver, text: str) -> None:
    expected_name = "BRT-028 Room"
    selector = ".room-list .room"
    driver.web.find_element("css selector", selector).click()
    wait_until(lambda: driver.web.execute_script("return document.querySelector('.room-chat header h2')?.innerText || ''") == expected_name,
               20.0, f"opened room header {expected_name!r} via {selector}")
    input_selector = '.composer input[aria-label="Message"]'
    send_selector = '.composer button'
    driver.web.find_element("css selector", input_selector).send_keys(text)
    wait_until(lambda: driver.web.execute_script("return document.querySelector(arguments[0])?.disabled === false", send_selector),
               10.0, f"send button enabled ({send_selector})")
    driver.web.find_element("css selector", send_selector).click()
    wait_until(lambda: text in driver.web.execute_script("return document.querySelector('.messages')?.innerText || ''"),
               30.0, f"message body {text!r} visible in .messages")


def assertion_4_delivery(driver: Driver, text: str, before: str | None) -> str:
    selector = ".message"
    def state() -> str | None:
        return driver.web.execute_script("const a=Array.from(document.querySelectorAll(arguments[0])); const m=a.find(e=>e.innerText.includes(arguments[1])); return m?.querySelector('.delivery')?.innerText || null", selector, text)
    observed = wait_until(lambda: (value if value in {"sent", "delivered"} else False) if (value := state()) else False,
                          45.0, f"delivery state for {text!r} in {selector} transitions from {before!r} to sent/delivered")
    if observed not in {"sent", "delivered"} or observed == before:
        raise AssertionError(f"selector={selector} .delivery; before={before!r}, expected changed state sent/delivered, observed={observed!r}")
    return observed

def assertion_5() -> None: raise NotImplementedError("BRT-028 assertion 5 is implemented by downstream worker")


def assertion_6(driver: Driver, tauri_bin: Path, inbound_text: str,
                acknowledged_text: str, prior_ack_state: str) -> None:
    """Restart the same XDG-isolated app and verify persisted UI state."""
    if not prior_ack_state:
        raise AssertionError("Cannot assert restored acknowledgement: no prior .delivery state was captured")
    if not inbound_text or not acknowledged_text:
        raise AssertionError("Cannot assert restored state without unique inbound and acknowledged message bodies")

    # Read the unread marker before navigation: opening the room marks it read.
    driver.web.quit()
    driver.web = None
    driver.web = driver.open_webdriver(tauri_bin)
    unread_selector = '.room-list .room b[aria-label$=" unread"]'
    badge = wait_until(
        lambda: (v if v else False) if (v := driver.web.execute_script(
            "const room=Array.from(document.querySelectorAll('.room-list .room')).find(e=>e.innerText.includes('BRT-028 Room')); "
            "return room?.querySelector('b[aria-label$=\\\" unread\\\"]')?.getAttribute('aria-label') || null")) else False,
        30.0, f"restored unread badge {unread_selector} for BRT-028 Room")
    expected_badge = "1 unread"
    if badge != expected_badge:
        raise AssertionError(f"restored unread mismatch: selector={unread_selector}, expected={expected_badge!r}, observed={badge!r}")

    room_selector = ".room-list .room"
    driver.web.find_element("css selector", room_selector).click()
    wait_until(lambda: driver.web.execute_script(
        "return document.querySelector('.room-chat header h2')?.innerText || ''") == "BRT-028 Room",
        20.0, "reopened BRT-028 Room after restart")
    history_selector = ".messages"
    history = wait_until(lambda: (v if inbound_text in v else False) if (v := driver.web.execute_script(
        "return document.querySelector(arguments[0])?.innerText || ''", history_selector)) else False,
        30.0, f"restored message history contains exact inbound body {inbound_text!r}")
    if inbound_text not in history:
        raise AssertionError(f"history mismatch: selector={history_selector}, expected body={inbound_text!r}, observed={history!r}")

    message_selector = ".message"
    state = wait_until(lambda: (v if v else False) if (v := driver.web.execute_script(
        "const m=Array.from(document.querySelectorAll(arguments[0])).find(e=>e.innerText.includes(arguments[1])); "
        "return m?.querySelector('.delivery')?.innerText || null", message_selector, acknowledged_text)) else False,
        20.0, f"restored prior acknowledgement {prior_ack_state!r} for {acknowledged_text!r}")
    if state != prior_ack_state:
        raise AssertionError(f"acknowledgement mismatch: selector={message_selector} .delivery, expected={prior_ack_state!r}, observed={state!r}")



def main() -> int:
    global STARTUP_TIMEOUT
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts-dir", type=Path, default=Path("/tmp/brt028-artifacts"))
    parser.add_argument("--profile-dir", type=Path, default=Path("/tmp/brt028-profile"))
    parser.add_argument("--tauri-bin", type=Path, default=Path("/home/dan/boru-build/brt028-target/debug/boru-desktop"))
    parser.add_argument("--peer-bin", type=Path, default=Path("/home/dan/boru-build/brt028-target/debug/boru"))
    parser.add_argument("--peer-port", type=int, default=43567)
    parser.add_argument("--startup-timeout", type=float, default=STARTUP_TIMEOUT)
    args = parser.parse_args()
    STARTUP_TIMEOUT = args.startup_timeout
    profile = args.profile_dir.expanduser().resolve()
    if any(profile == p.expanduser().resolve() or p.expanduser().resolve() in profile.parents
           for p in PRODUCTION_PATHS):
        parser.error(f"refusing production profile path: {profile}")
    artifacts = args.artifacts_dir.expanduser().resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    profile.mkdir(parents=True, exist_ok=True)
    for binary in (args.tauri_bin, args.peer_bin):
        if not binary.is_file():
            parser.error(f"binary does not exist: {binary}")
    driver = Driver(artifacts)
    failed = False
    try:
        assertion_1(driver, profile, args.tauri_bin.resolve(), args.peer_bin.resolve(), args.peer_port)
        driver.assertion("group-2-room-list", 2, lambda: assertion_2_room_list(driver))
        marker = f"BRT028-{int(time.time())}"
        before_state = driver.web.execute_script("const a=Array.from(document.querySelectorAll('.message')); const m=a.find(e=>e.innerText.includes(arguments[0])); return m?.querySelector('.delivery')?.innerText || null", marker)
        driver.assertion("group-3-open-send", 3, lambda: assertion_3_open_and_send(driver, marker))
        driver.assertion("group-4-delivery-state", 4, lambda: assertion_4_delivery(driver, marker, before_state))
        reply_marker = f"BRT028-REPLY-{int(time.time())}"
        # Group 5 must send this reply while the room is not selected so it
        # becomes unread; the outgoing marker is used for the ack-state check.
        prior_ack = driver.web.execute_script("const m=Array.from(document.querySelectorAll('.message')); const e=m.find(x=>x.innerText.includes(arguments[0])); return e?.querySelector('.delivery')?.innerText || null", marker)
        driver.assertion("group-6-restart-persistence", 6,
                         lambda: assertion_6(driver, args.tauri_bin.resolve(), reply_marker, marker, prior_ack))
        print("Fallback fixture: seeded the approved BRT-028 Room conversation in the disposable SQLite DB after initial schema creation; restart reused the same XDG_DATA_HOME/XDG_CONFIG_HOME selected by --profile-dir.")
    except KeyboardInterrupt:
        failed = True
        print("Interrupted", file=sys.stderr)
    except BaseException:
        failed = True
        traceback.print_exc()
        # Group 1 failures still require a fail screenshot and durable step record.
        if not (artifacts / "steps.jsonl").exists() or not (driver.shots / "group-1-readiness-fail.png").exists():
            try:
                driver.assertion("group-1-readiness", 1, lambda: (_ for _ in ()).throw(
                    RuntimeError("Group 1 startup/readiness failed; see stderr diagnostics")))
            except RuntimeError:
                pass
    finally:
        if driver.web is not None:
            try:
                driver.web.quit()
            except Exception:
                pass
        driver.teardown()
    print(f"Artifacts: {artifacts}", file=sys.stderr if failed else sys.stdout)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
