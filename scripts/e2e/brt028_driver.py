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

Run the harness on the host that owns the binaries: the `--tauri-bin` /
`--peer-bin` defaults (`/home/dan/boru-build/brt028-target/debug/...`) are DEBSRV
paths, and DEBSRV has `tauri-driver`, `WebKitWebDriver`, `xvfb-run`, and
Selenium. A host without those binaries fails fast with
`binary does not exist: ...` from the argument check.

Group ordering contract (reconciled from the group-5 and group-6 cards, which
conflict as written):

* group 5 sends the peer reply while BRT-028 Room is NOT selected, so the reply
  is stored unread; it asserts the `1 unread` badge, then opens the room and
  asserts the reply body/room/non-local sender (its own display acceptance),
  then deselects again and has the peer send the group-6 inbound body so the
  restart starts from exactly one unread message.
* group 6 restarts the app against the same `XDG_DATA_HOME`/`XDG_CONFIG_HOME`,
  asserts `1 unread`, opens the room, and asserts restored history plus the
  previously captured `.delivery` state.

Deselection is a webview reload (`Driver`/`deselect_room`): the room
subscription lives in the Rust runtime's persistent room-sender registry, so a
reload keeps gossip flowing while `mark_read` (fired only from the room-open
path, `desktop/src/main.tsx:105`) stops clearing the count.
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
import sqlite3
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
# Shared BRT-028 room fixture (identical topic the SQLite seed below writes).
ROOM_NAME = "BRT-028 Room"
SHARED_TOPIC = "9021bd1ed0932e4fb1dfd5477ebee17916eb431c892d44f16287962329eaf303"
STARTUP_TIMEOUT = 60.0
DRIVER_URL = "http://127.0.0.1:4444"
# Transport flags for the CLI/Iced peer. Configuration A (BRT-028 step 1) wants
# relay defaults, i.e. NO transport-disabling flags; configuration B wants
# `--no-relay --no-dht`. Selected via --peer-relay-mode.
PEER_RELAY_ARGS: tuple[str, ...] = ("--no-relay", "--no-dht")


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
        self.peer_port = peer_port
        # Remember the real profile root: group-5 reads the desktop's
        # message_store.db from it, and --profile-dir is not required to live
        # inside --artifacts-dir.
        self.profile = profile
        env = os.environ.copy()
        env["XDG_DATA_HOME"] = str(profile / "tauri" / "data")
        env["XDG_CONFIG_HOME"] = str(profile / "tauri" / "config")
        Path(env["XDG_DATA_HOME"]).mkdir(parents=True, exist_ok=True)
        Path(env["XDG_CONFIG_HOME"]).mkdir(parents=True, exist_ok=True)
        # Desktop tracing is opt-in via BORU_DESKTOP_LOG (desktop/src-tauri/src/lib.rs)
        # and defaults to a `warn` filter. Set both so the inbound accept/reject
        # diagnostics (BRT-028 observability requirement) are captured; tauri-driver
        # passes its environment through to the spawned desktop binary.
        env["BORU_DESKTOP_LOG"] = str(self.artifacts / "desktop.log")
        env.setdefault("RUST_LOG", "info,boru=info,boru_core=info")
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
        peer_argv = [str(peer_bin), "--data-dir", str(peer_data), "--mcp", "--mcp-bind",
                     f"127.0.0.1:{peer_port}", "--enable-gui-test-actions",
                     *PEER_RELAY_ARGS, "--keep-running"]
        self.peer = self.spawn(peer_argv, env=env, stdout=peer_out,
                               stderr=subprocess.STDOUT, name="Iced/CLI peer")
        started = time.monotonic()
        try:
            wait_until(lambda: self.peer.poll() is None and self.peer_trace_path.exists() and
                       "MCP diagnostic server listening on" in self.peer_trace_path.read_text(
                           errors="replace"), STARTUP_TIMEOUT,
                       "Iced peer log: MCP diagnostic server listening on")
        except TimeoutError as exc:
            raise RuntimeError(self.diagnostic("Iced/CLI", "MCP diagnostic server listening on", started)) from exc
        # The peer's GUI OpenRoom command rejects topics absent from its room
        # history (`src/bin/boru/app.rs:9512`), so the same approved room fixture
        # is seeded into the peer catalog and the peer is relaunched to load it.
        # Without this the peer never joins the shared topic, the desktop's send
        # has no mesh member, and its delivery state stays `queued` forever.
        peer_db = peer_data / "boru.db"
        wait_until(lambda: peer_db.is_file(), STARTUP_TIMEOUT,
                   "Iced peer profile DB creation (boru.db)")
        self.stop_peer()
        self.seed_conversation(peer_db)
        self.peer = self.spawn(peer_argv, env=env, stdout=peer_out,
                               stderr=subprocess.STDOUT, name="Iced/CLI peer (seeded)")
        started = time.monotonic()
        try:
            wait_until(lambda: self.peer.poll() is None and self._tcp_open("127.0.0.1", peer_port),
                       STARTUP_TIMEOUT, "Iced peer MCP port after catalog seed")
        except TimeoutError as exc:
            raise RuntimeError(self.diagnostic("Iced/CLI", "MCP port after catalog seed", started)) from exc
        self.web = self.open_webdriver(tauri_bin)

    def stop_peer(self) -> None:
        """Terminate the Iced peer's process group and wait for it to exit."""
        peer, self.peer = self.peer, None
        if peer is None or peer.poll() is not None:
            return
        try:
            os.killpg(peer.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        try:
            peer.wait(timeout=10)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(peer.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            peer.wait()

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


def desktop_node_id(profile: Path) -> str:
    """Derive the Tauri app's node id from its persisted ed25519 secret key.

    The desktop profile stores the raw 32-byte secret as hex in
    `secret_key.txt`; the node id is the matching ed25519 public key.
    """
    key_file = profile / "tauri" / "data" / "computer.iroh.boru.desktop" / "secret_key.txt"
    try:
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    except ImportError as exc:
        raise RuntimeError("cryptography is required to derive the desktop node id") from exc
    seed = bytes.fromhex(key_file.read_text().strip())
    public = Ed25519PrivateKey.from_private_bytes(seed).public_key()
    return public.public_bytes(serialization.Encoding.Raw,
                               serialization.PublicFormat.Raw).hex()


def desktop_udp_ports() -> list[int]:
    """Return the running boru-desktop endpoint's candidate UDP (QUIC) ports.

    The app binds several UDP sockets (mDNS 5353, DHT, and the iroh endpoint), so
    every candidate is offered to `BootstrapRoom`; gossip dials whichever address
    answers. mDNS is skipped because it is not the endpoint.
    """
    listing = subprocess.run(["ss", "-lunp"], capture_output=True, text=True).stdout
    ports: list[int] = []
    for line in listing.splitlines():
        if "boru-desktop" not in line:
            continue
        fields = line.split()
        if len(fields) <= 3:
            continue
        # ss -lunp columns: State Recv-Q Send-Q Local:Port Peer:Port [Process]
        local = fields[3]
        try:
            port = int(local.rsplit(":", 1)[1])
        except ValueError:
            continue
        if port != 5353 and port not in ports:
            ports.append(port)
    if not ports:
        raise RuntimeError("no UDP listen socket found for boru-desktop")
    return ports


def bootstrap_peer_room(driver: Driver, profile: Path, timeout: float = 60.0) -> None:
    """Join the peer to the shared room via explicit desktop endpoint addresses.

    The Iced GUI `OpenRoom` action only accepts rooms already known to the app
    (`src/bin/boru/app.rs:12973-12990`) and the startup lobby auto-join was
    removed (BORU-DISC-12), so without an explicit bootstrap the peer never
    joins the room topic. `BootstrapRoom` exists for exactly this case.
    """
    node_id = desktop_node_id(profile)
    ports = wait_until(desktop_udp_ports, 30.0, "boru-desktop UDP endpoint ports")
    peers = [{"node_id": node_id, "addr": f"127.0.0.1:{port}"} for port in ports]
    peer_mcp(driver, "boru_gui_bootstrap_room",
             {"room_id": SHARED_TOPIC, "bootstrap_peers": peers}, timeout=15.0)

    def room_open() -> bool:
        # `boru_get_room_status` grades gossip-join diagnostics, which stay
        # unavailable until traffic flows; the GUI snapshot is the correct
        # evidence that the peer command surface has the room selected (and so
        # the composer commands have a target).
        snapshot = peer_mcp(driver, "boru_get_gui_snapshot")
        return isinstance(snapshot, dict) and snapshot.get("active_room") == SHARED_TOPIC
    wait_until(room_open, timeout,
               f"Iced peer opened shared room {SHARED_TOPIC} (bootstrap {[p for p in ports]})")


def assertion_1(driver: Driver, profile: Path, tauri_bin: Path, peer_bin: Path,
                peer_port: int) -> None:
    def start_and_wait() -> None:
        driver.start_infrastructure(profile, tauri_bin, peer_bin, peer_port)
        wait_until(lambda: bool(driver.web and driver.peer and driver.peer.poll() is None),
                   STARTUP_TIMEOUT, "Tauri WebDriver session + live Iced peer")
        # Put the peer in the shared room before any send: a room topic with no
        # second mesh member leaves the desktop's outgoing message queued
        # forever, so groups 3-4 could never observe a sent/delivered transition.
        bootstrap_peer_room(driver, profile)
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

def peer_mcp(driver: Driver, method: str, params: dict[str, Any] | None = None,
             timeout: float = 5.0) -> Any:
    """Call one MCP JSON-RPC method on the Iced peer’s loopback diagnostic server.

    ``timeout`` bounds the socket read: long-running tools such as
    ``boru_join_lobby_room`` block server-side until join evidence arrives, so
    they need a timeout above the tool’s own ``timeout_ms``.

    The peer’s GUI-test surface enforces 10 actions/sec. Repeated harness
    actions (composer + submit + room sends in one flow) can trip it, which is
    a throttle rather than a failure, so rate-limit errors are retried with a
    short backoff. Every other error is propagated unchanged.
    """
    request = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    attempts = 6
    for attempt in range(attempts):
        with socket.create_connection(("127.0.0.1", driver.peer_port), timeout=timeout) as conn:
            conn.sendall((json.dumps(request) + "\n").encode())
            response = b""
            while not response.endswith(b"\n"):
                chunk = conn.recv(65536)
                if not chunk:
                    break
                response += chunk
        if not response:
            raise RuntimeError(f"Iced peer MCP {method} returned no response")
        decoded = json.loads(response)
        if "error" not in decoded:
            return decoded.get("result")
        error = decoded["error"]
        detail = str(error)
        if "Rate limit exceeded" in detail and attempt < attempts - 1:
            time.sleep(0.25)
            continue
        raise RuntimeError(f"Iced peer MCP {method} failed: {error}")


def unread_badge(driver: Driver, room_name: str = ROOM_NAME) -> str | None:
    """Return the room entry's `aria-label` unread badge text, or None when absent."""
    return driver.web.execute_script(
        "const room=Array.from(document.querySelectorAll('.room-list .room'))"
        ".find(e=>e.innerText.includes(arguments[0])); "
        "return room?.querySelector('b[aria-label$=\" unread\"]')"
        "?.getAttribute('aria-label') || null", room_name)


def deselect_room(driver: Driver) -> None:
    """Return the UI to the room list without stopping the Tauri backend.

    A webview reload resets the React selection; the Rust runtime keeps the room
    subscription from the earlier `open_room`, so gossip still arrives while
    `mark_read` (room-open only) no longer clears the unread count.
    """
    driver.web.find_element("css selector", "button[aria-label='Close room']").click()
    wait_until(lambda: driver.web.execute_script(
        "return !!document.querySelector('.room-placeholder')"), 30.0,
        "room deselected (.room-placeholder) after close-room click")
    wait_until(lambda: ROOM_NAME in driver.web.execute_script(
        "return document.querySelector('.room-list')?.innerText || ''"), 30.0,
        f"room list still lists {ROOM_NAME!r} after close-room click")

    wait_until(lambda: driver.web.execute_script("return document.querySelector('.status')?.innerText || ''").startswith("Connected · revision "), 30.0, "snapshot subscription remains connected after room close")


def open_room(driver: Driver) -> None:
    driver.web.find_element("css selector", ".room-list .room").click()
    wait_until(lambda: driver.web.execute_script(
        "return document.querySelector('.room-chat header h2')?.innerText || ''") == ROOM_NAME,
        20.0, f"opened room header {ROOM_NAME!r} via .room-list .room")


def peer_send_room_message(driver: Driver, body: str) -> None:
    """Send `body` from the Iced peer through the approved loopback MCP GUI path."""
    # The room is known to the peer after the group-1 bootstrap, so the GUI
    # OpenRoom action validates; it also makes the room the active conversation
    # that the composer commands target.
    peer_mcp(driver, "boru_gui_open_room", {"room_id": SHARED_TOPIC})
    peer_mcp(driver, "boru_gui_set_composer", {"text": body})
    peer_mcp(driver, "boru_gui_submit_composer")


def assertion_5_peer_reply(driver: Driver, sent_marker: str) -> str:
    """Group 5: receive a peer reply unread, then assert it renders in the room.

    Returns the inbound body deliberately left unread for the group-6 restart.
    """
    reply = f"{sent_marker}-reply"
    deselect_room(driver)
    peer_send_room_message(driver, reply)
    revision_before_reply = driver.web.execute_async_script("const done=arguments[0]; window.__TAURI_INTERNALS__.invoke('get_app_snapshot').then(x=>done(x.cursor)).catch(e=>done(String(e)))")
    try:
        badge = wait_until(lambda: unread_badge(driver) or False, 60.0,
                           f"unread badge for {ROOM_NAME!r} after peer reply {reply!r}")
    except TimeoutError:
        try:
            browser_logs = driver.web.get_log("browser")
        except Exception as exc:
            browser_logs = [{"unavailable": str(exc)}]
        evidence = {"revision_before_reply": revision_before_reply,
                    "revision_after_timeout": driver.web.execute_script("return document.querySelector('.status')?.innerText || ''"),
                    "room_list_html": driver.web.execute_script("return document.querySelector('.room-list')?.innerHTML || ''"),
                    "placeholder_present": driver.web.execute_script("return !!document.querySelector('.room-placeholder')"),
                    "direct_snapshot_unread": driver.web.execute_async_script("const done=arguments[0]; window.__TAURI_INTERNALS__.invoke('get_app_snapshot').then(x=>done(x.snapshot.unread)).catch(e=>done(String(e)))"),
                    "browser_logs": browser_logs}
        (driver.artifacts / "group5-live-update.json").write_text(json.dumps(evidence, indent=2))
        raise
    if badge != "1 unread":
        raise AssertionError(
            f"unread badge: selector=.room-list .room b[aria-label$=\" unread\"], expected='1 unread', observed={badge!r}")
    revision_after_reply = driver.web.execute_async_script("const done=arguments[0]; window.__TAURI_INTERNALS__.invoke('get_app_snapshot').then(x=>done(x.cursor)).catch(e=>done(String(e)))")
    snapshot_unread = driver.web.execute_async_script("const done=arguments[0]; window.__TAURI_INTERNALS__.invoke('get_app_snapshot').then(x=>done(x.snapshot.unread)).catch(e=>done(String(e)))")
    profile_root = getattr(driver, "profile", None) or (driver.artifacts / "profile")
    conversation_db = profile_root / "tauri/data/computer.iroh.boru.desktop/message_store.db"
    with sqlite3.connect(conversation_db) as connection:
        row = connection.execute("SELECT hex(conversation_id), unread_count, last_message_preview FROM conversation_meta WHERE last_message_preview = ?", (reply,)).fetchone()
    evidence = {"cursor_before_reply": revision_before_reply, "cursor_after_reply": revision_after_reply,
                "snapshot_unread": snapshot_unread, "rendered_badge": badge,
                "room_list_html": driver.web.execute_script("return document.querySelector('.room-list')?.innerHTML || ''"),
                "conversation_meta": row}
    (driver.artifacts / "group5-live-update.json").write_text(json.dumps(evidence, indent=2))

    # Group-5 display acceptance: the reply must render in the open room with the
    # right room context and a non-local sender.
    open_room(driver)
    def observe_reply() -> dict[str, str] | bool:
        return driver.web.execute_script(
            "const ms=Array.from(document.querySelectorAll('.message')); "
            "const m=ms.find(e=>e.innerText.includes(arguments[0])); "
            "return m ? {text:m.innerText, room:document.querySelector('.room-chat header h2')?.innerText||'', "
            "sender:m.querySelector('small')?.innerText.split(' · ')[0]||''} : false", reply)
    observed = wait_until(observe_reply, 45.0,
                          f"reply body={reply!r} rendered in {ROOM_NAME!r} from a non-local sender")
    if not isinstance(observed, dict) or reply not in observed["text"] \
            or observed["room"] != ROOM_NAME or not observed["sender"] or observed["sender"] == "You":
        raise AssertionError(
            f"expected body={reply!r}, room={ROOM_NAME!r}, sender=non-local Iced peer; observed={observed!r}")

    wait_until(lambda: unread_badge(driver) is None, 30.0,
               f"unread badge cleared by mark_read after opening {ROOM_NAME!r}")

    # Re-arm exactly one unread message for the group-6 restart assertion.
    inbound = f"{sent_marker}-restart"
    deselect_room(driver)
    peer_send_room_message(driver, inbound)
    badge = wait_until(lambda: unread_badge(driver) or False, 60.0,
                       f"unread badge for {ROOM_NAME!r} after group-6 inbound body {inbound!r}")
    if badge != "1 unread":
        raise AssertionError(
            f"pre-restart unread badge: expected='1 unread', observed={badge!r} for body={inbound!r}")
    return inbound


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
    global STARTUP_TIMEOUT, PEER_RELAY_ARGS
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts-dir", type=Path, default=Path("/tmp/brt028-artifacts"))
    parser.add_argument("--profile-dir", type=Path, default=Path("/tmp/brt028-profile"))
    parser.add_argument("--tauri-bin", type=Path, default=Path("/home/dan/boru-build/brt028-target/debug/boru-desktop"))
    parser.add_argument("--peer-bin", type=Path, default=Path("/home/dan/boru-build/brt028-target/debug/boru"))
    parser.add_argument("--peer-port", type=int, default=43567)
    parser.add_argument("--startup-timeout", type=float, default=STARTUP_TIMEOUT)
    parser.add_argument("--peer-relay-mode", choices=("no-relay", "default"), default="no-relay",
                        help="Transport config for the CLI/Iced peer: 'default' = relay+DHT enabled "
                             "(BRT-028 configuration A), 'no-relay' = --no-relay --no-dht "
                             "(configuration B).")
    args = parser.parse_args()
    STARTUP_TIMEOUT = args.startup_timeout
    PEER_RELAY_ARGS = () if args.peer_relay_mode == "default" else ("--no-relay", "--no-dht")
    print(f"Peer relay mode: {args.peer_relay_mode} (peer argv transport flags: {PEER_RELAY_ARGS or 'none'})")
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
        # BRT-028 pair-probe check (b): record the room-header participant count
        # the desktop UI actually renders (`<header><small>N participants`).
        header_participants = driver.web.execute_script(
            "return document.querySelector('.room-chat header small')?.innerText || null")
        (driver.artifacts / "participants.json").write_text(json.dumps(
            {"room": ROOM_NAME, "header_small": header_participants,
             "room_list": driver.web.execute_script(
                 "return document.querySelector('.room-list')?.innerText || null")}, indent=2))
        print(f"Room header participants: {header_participants!r}", flush=True)
        driver.assertion("group-4-delivery-state", 4, lambda: assertion_4_delivery(driver, marker, before_state))
        reply_marker = f"BRT028-REPLY-{int(time.time())}"
        # Capture the outgoing ack state before group 5 reloads the webview;
        # group 6 asserts the identical value after the restart.
        prior_ack = driver.web.execute_script("const m=Array.from(document.querySelectorAll('.message')); const e=m.find(x=>x.innerText.includes(arguments[0])); return e?.querySelector('.delivery')?.innerText || null", marker)
        inbound: dict[str, str] = {}
        def group_5() -> None:
            inbound["body"] = assertion_5_peer_reply(driver, reply_marker)
        driver.assertion("group-5-peer-reply", 5, group_5)
        driver.assertion("group-6-restart-persistence", 6,
                         lambda: assertion_6(driver, args.tauri_bin.resolve(), inbound["body"], marker, prior_ack))
        print("Fallback fixture: seeded the approved BRT-028 Room conversation in the disposable SQLite DB after initial schema creation; restart reused the same XDG_DATA_HOME/XDG_CONFIG_HOME selected by --profile-dir.")
        print("Harness gap: the peer reply is sent through the loopback MCP GUI-test path (no physical keyboard input), and room deselection is a webview reload because the desktop UI exposes no deselect affordance.")
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
