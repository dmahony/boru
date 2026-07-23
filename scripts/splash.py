#!/usr/bin/env python3
"""Splash window for Boru Chat. Shows spinner + startup messages.
Usage: python3 splash.py [--log LOGFILE]
Stdin: lines become status messages, "DONE" exits.
If --log is given, the log file is tailed and lines shown as messages."""

import tkinter as tk
import sys
import os
import time
import threading

FADE_COLORS = [
    "#e2e8f0", "#c8d0dc", "#aeb8c8",
    "#94a0b4", "#7a88a0", "#64748b",
]
BG = "#0f0f1a"
ACCENT = "#4a9eff"
SUBTLE = "#64748b"


class SplashScreen:
    def __init__(self, logfile=None):
        self.root = tk.Tk()
        self.root.title("Boru Chat")
        self.root.geometry("440x340+%d+%d" % self._center(440, 340))
        self.root.overrideredirect(True)
        self.root.configure(bg=BG)
        self.root.attributes('-topmost', True)

        outer = tk.Frame(self.root, bg="#1e1e3a", padx=2, pady=2)
        outer.pack(fill=tk.BOTH, expand=True)
        inner = tk.Frame(outer, bg=BG)
        inner.pack(fill=tk.BOTH, expand=True)

        tk.Label(inner, text="Boru Chat", font=("sans-serif", 22, "bold"),
                 fg=ACCENT, bg=BG).pack(pady=(20, 2))
        tk.Label(inner, text="v0.101.1", font=("sans-serif", 9),
                 fg=SUBTLE, bg=BG).pack()

        self.spinner_frames = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"]
        self.spinner_idx = 0
        self.spinner_label = tk.Label(
            inner, text=self.spinner_frames[0],
            font=("monospace", 14), fg=ACCENT, bg=BG)
        self.spinner_label.pack(pady=(12, 8))

        self.msg_frame = tk.Frame(inner, bg=BG)
        self.msg_frame.pack(pady=(0, 16), padx=24, fill=tk.BOTH, expand=True)

        self.messages = []
        self._running = True
        self._anim_start = time.time()
        self._animate()
        self._read_stdin()
        if logfile:
            self._tail_log(logfile)

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

    def add_message(self, text):
        # Skip empty lines, strip ANSI escapes, truncate
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
                if line:
                    self.root.after(0, self.add_message, line)
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
    args = sys.argv[1:]
    if len(args) >= 2 and args[0] == "--log":
        logfile = args[1]
    SplashScreen(logfile=logfile).run()
