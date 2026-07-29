#!/usr/bin/env python3
"""Splash window for Boru Chat. Shows spinner + status messages.
Acts as a runtime watchdog: detects hangs (heartbeat timeout)
and crashes (pipe close).

Usage: python3 splash.py [--log LOGFILE] [--version VERSION_STRING]
Stdin: lines become status messages.
  "hb"        → heartbeat tick (resets timeout)
  "DONE"      → close splash
  other text  → shown as status messages
If --log is given, the log file is tailed and lines shown as messages.
If --version is given, that string is displayed as the version label."""

import tkinter as tk
import sys
import os
import time
import threading

FADE_COLORS = [
    "#ffffff", "#d9d9d9", "#b3b3b3",
    "#8c8c8c", "#666666", "#404040",
]
BG = "#000000"
ACCENT = "#ffffff"
SUBTLE = "#ffffff"
WARN_FG = "#f2a626"  # amber for not-responding
CRASH_FG = "#e64040"  # red for crash


class SplashScreen:
    def __init__(self, logfile=None, version="v0.0.0"):
        self.root = tk.Tk()
        self.root.title("Boru")
        self.root.geometry("440x360+%d+%d" % self._center(440, 360))
        self.root.overrideredirect(True)
        self.root.configure(bg=BG)
        self.root.attributes('-topmost', True)

        outer = tk.Frame(self.root, bg="#1e1e3a", padx=2, pady=2)
        outer.pack(fill=tk.BOTH, expand=True)
        inner = tk.Frame(outer, bg=BG)
        inner.pack(fill=tk.BOTH, expand=True)

        tk.Label(inner, text="BORU", font=("sans-serif", 28, "bold"),
                 fg=ACCENT, bg=BG).pack(pady=(20, 2))
        tk.Label(inner, text=version, font=("sans-serif", 9),
                 fg=SUBTLE, bg=BG).pack()

        self.spinner_frames = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"]
        self.spinner_idx = 0
        self.spinner_label = tk.Label(
            inner, text=self.spinner_frames[0],
            font=("monospace", 14), fg=ACCENT, bg=BG)
        self.spinner_label.pack(pady=(12, 8))

        # Status indicator label (crash, not-responding, etc.)
        self.status_label = tk.Label(
            inner, text="", font=("sans-serif", 10, "bold"),
            fg=WARN_FG, bg=BG)
        self.status_label.pack(pady=(0, 4))

        self.msg_frame = tk.Frame(inner, bg=BG)
        self.msg_frame.pack(pady=(0, 16), padx=24, fill=tk.BOTH, expand=True)

        self.messages = []
        self._running = True
        self._anim_start = time.time()

        # ── Heartbeat watchdog ───────────────────────────────────────
        self._last_heartbeat = time.time()
        self._hb_timeout = 6.0       # seconds before "not responding"
        self._hb_warned = False       # avoid repeat warn flicker
        self._pipe_closed = False     # stdin EOF = crash
        # ──────────────────────────────────────────────────────────────

        self._animate()
        self._read_stdin()
        if logfile:
            self._tail_log(logfile)
        self._watchdog_tick()

    def _center(self, w, h):
        sw = self.root.winfo_screenwidth()
        sh = self.root.winfo_screenheight()
        return ((sw - w) // 2, (sh - h) // 2)

    def _animate(self):
        if self._running:
            self.spinner_idx = (self.spinner_idx + 1) % len(self.spinner_frames)
            self.spinner_label.config(text=self.spinner_frames[self.spinner_idx])
            for widget in self.msg_frame.winfo_children():
                age = getattr(widget, '_msg_age', 0)
                color = FADE_COLORS[min(age, len(FADE_COLORS) - 1)]
                widget.config(fg=color)
                widget._msg_age = min(age + 1, len(FADE_COLORS) - 1)
            self.root.after(250, self._animate)

    def _watchdog_tick(self):
        """Check heartbeat every 500ms for hang detection."""
        if not self._running:
            return
        if self._pipe_closed:
            # Process is gone — show crash, close shortly
            self.status_label.config(text="⚠ Process exited unexpectedly", fg=CRASH_FG)
            self._running = False
            self.root.after(2000, self.root.destroy)
            return
        elapsed = time.time() - self._last_heartbeat
        if elapsed > self._hb_timeout and not self._hb_warned:
            self.status_label.config(
                text="⚠ Not responding — may be hung", fg=WARN_FG)
            self._hb_warned = True
        elif elapsed <= self._hb_timeout and self._hb_warned:
            # Recovered
            self.status_label.config(text="")
            self._hb_warned = False
        self.root.after(500, self._watchdog_tick)

    def add_message(self, text):
        # Skip empty lines
        text = text.strip()
        if not text:
            return
        # Strip common ANSI / tracing prefixes for cleaner display
        import re
        text = re.sub(r'\x1b\[[0-9;]*m', '', text)  # ANSI colors
        text = re.sub(r'^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z\s+', '', text)  # timestamp
        text = re.sub(r'^\s*(INFO|WARN|ERROR|DEBUG|TRACE)\s+', '', text)  # log level
        if len(text) > 60:
            text = text[:57] + "…"

        self.messages.append(text)
        while len(self.messages) > 7:
            self.messages.pop(0)

        for widget in self.msg_frame.winfo_children():
            widget.destroy()

        for i, msg in enumerate(reversed(self.messages)):
            lbl = tk.Label(
                self.msg_frame, text=msg,
                font=("monospace", 8),
                fg=FADE_COLORS[min(len(self.messages) - 1 - i, len(FADE_COLORS) - 1)],
                bg=BG, anchor="w", justify="left")
            lbl._msg_age = len(self.messages) - 1 - i
            lbl.pack(fill=tk.X, pady=1)

    def _read_stdin(self):
        def reader():
            for line in sys.stdin:
                line = line.strip()
                if line == "DONE":
                    self._running = False
                    self.root.after(300, self.root.destroy)
                    return
                if line == "hb":
                    self._last_heartbeat = time.time()
                    continue
                if line:
                    self.root.after(0, self.add_message, line)
            # EOF on stdin — process exited or crashed
            self._pipe_closed = True
        t = threading.Thread(target=reader, daemon=True)
        t.start()

    def _tail_log(self, logfile):
        def tail():
            # Wait for the log file to appear
            for _ in range(50):
                if os.path.exists(logfile):
                    break
                time.sleep(0.2)
            if not os.path.exists(logfile):
                return
            with open(logfile, 'r') as f:
                f.seek(0, os.SEEK_END)
                while self._running:
                    line = f.readline()
                    if line:
                        self.root.after(0, self.add_message, line.strip())
                    else:
                        time.sleep(0.1)
        t = threading.Thread(target=tail, daemon=True)
        t.start()

    def run(self):
        self.root.mainloop()


if __name__ == "__main__":
    logfile = None
    version = "v0.0.0"
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--log" and i + 1 < len(args):
            logfile = args[i + 1]
            i += 2
        elif args[i] == "--version" and i + 1 < len(args):
            version = args[i + 1]
            i += 2
        else:
            i += 1
    SplashScreen(logfile=logfile, version=version).run()
