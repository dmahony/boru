#!/usr/bin/env python3
"""Splash window for Boru Chat. Shows a progress spinner and messages.
Run with: python3 splash.py &
Communicate via stdin: each line becomes a status message, "DONE" exits."""

import tkinter as tk
import sys
import time
import threading

FADE_COLORS = [
    "#e2e8f0",  # newest - bright
    "#c8d0dc",  # slightly faded
    "#aeb8c8",  # more faded
    "#94a0b4",  # getting dimmer
    "#7a88a0",  # quite dim
    "#64748b",  # very dim
]

BG = "#0f0f1a"
ACCENT = "#4a9eff"
SUBTLE = "#64748b"


class SplashScreen:
    def __init__(self):
        self.root = tk.Tk()
        self.root.title("Boru Chat")
        self.root.geometry("420x320+%d+%d" % self._center(420, 320))
        self.root.overrideredirect(True)
        self.root.configure(bg=BG)
        self.root.attributes('-topmost', True)

        # Outer frame with subtle border
        outer = tk.Frame(self.root, bg="#1e1e3a", padx=2, pady=2)
        outer.pack(fill=tk.BOTH, expand=True)

        inner = tk.Frame(outer, bg=BG)
        inner.pack(fill=tk.BOTH, expand=True)

        # Title
        tk.Label(
            inner, text="Boru Chat", font=("sans-serif", 22, "bold"),
            fg=ACCENT, bg=BG
        ).pack(pady=(25, 5))

        # Version
        tk.Label(
            inner, text="v0.101.1", font=("sans-serif", 9),
            fg=SUBTLE, bg=BG
        ).pack()

        # Spinner
        self.spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
        self.spinner_idx = 0
        self.spinner_label = tk.Label(
            inner, text=self.spinner_frames[0],
            font=("monospace", 14), fg=ACCENT, bg=BG
        )
        self.spinner_label.pack(pady=(15, 10))

        # Message area
        self.msg_frame = tk.Frame(inner, bg=BG)
        self.msg_frame.pack(pady=(0, 20), padx=30, fill=tk.BOTH, expand=True)

        self.messages = []
        self._running = True
        self._anim_start = time.time()
        self._animate()
        self._read_stdin()

    def _center(self, w, h):
        sw = self.root.winfo_screenwidth()
        sh = self.root.winfo_screenheight()
        return ((sw - w) // 2, (sh - h) // 2)

    def _animate(self):
        if self._running:
            self.spinner_idx = (self.spinner_idx + 1) % len(self.spinner_frames)
            self.spinner_label.config(text=self.spinner_frames[self.spinner_idx])

            # Fade message colors based on age
            for widget in self.msg_frame.winfo_children():
                age = getattr(widget, '_msg_age', 0)
                color = FADE_COLORS[min(age, len(FADE_COLORS) - 1)]
                widget.config(fg=color)
                widget._msg_age = min(age + 1, len(FADE_COLORS) - 1)

            self.root.after(250, self._animate)

    def add_message(self, text):
        # Truncate to reasonable length
        if len(text) > 50:
            text = text[:47] + "…"

        self.messages.append(text)
        # Keep only last 6 messages
        while len(self.messages) > 6:
            self.messages.pop(0)

        # Rebuild message labels
        for widget in self.msg_frame.winfo_children():
            widget.destroy()

        for i, msg in enumerate(reversed(self.messages)):
            lbl = tk.Label(
                self.msg_frame, text=msg,
                font=("monospace", 9),
                fg=FADE_COLORS[min(len(self.messages) - 1 - i, len(FADE_COLORS) - 1)],
                bg=BG, anchor="w", justify="left"
            )
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

    def run(self):
        self.root.mainloop()


if __name__ == "__main__":
    SplashScreen().run()
