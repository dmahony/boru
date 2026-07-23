#!/usr/bin/env python3
"""Splash window for Boru Chat. Shows a progress spinner and messages.
Run with: python3 splash.py &
Communicate via stdin: each line becomes a status message, "DONE" exits."""

import tkinter as tk
import sys
import time
import threading

class SplashScreen:
    def __init__(self):
        self.root = tk.Tk()
        self.root.title("Boru Chat")
        self.root.geometry("400x250+%d+%d" % self._center(400, 250))
        self.root.overrideredirect(True)
        self.root.configure(bg="#1a1a2e")
        self.root.attributes('-topmost', True)

        # Title
        self.title_label = tk.Label(
            self.root, text="Boru Chat", font=("sans-serif", 20, "bold"),
            fg="#4a9eff", bg="#1a1a2e"
        )
        self.title_label.pack(pady=(30, 10))

        # Spinner
        self.spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
        self.spinner_idx = 0
        self.spinner_label = tk.Label(
            self.root, text=self.spinner_frames[0],
            font=("monospace", 28), fg="#4a9eff", bg="#1a1a2e"
        )
        self.spinner_label.pack()

        # Messages
        self.messages = []
        self.msg_frame = tk.Frame(self.root, bg="#1a1a2e")
        self.msg_frame.pack(pady=(20, 10), fill=tk.BOTH, expand=True)

        self._running = True
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
            self.root.after(100, self._animate)

    def add_message(self, text):
        self.messages.append(text)
        # Keep only last 5 messages
        if len(self.messages) > 5:
            self.messages.pop(0)
        # Rebuild message labels
        for widget in self.msg_frame.winfo_children():
            widget.destroy()
        for msg in self.messages:
            tk.Label(
                self.msg_frame, text=msg, font=("sans-serif", 10),
                fg="#8892b0", bg="#1a1a2e", anchor="w"
            ).pack(fill=tk.X, padx=20)

    def _read_stdin(self):
        def reader():
            for line in sys.stdin:
                line = line.strip()
                if line == "DONE":
                    self._running = False
                    self.root.after(200, self.root.destroy)
                    return
                if line:
                    self.root.after(0, self.add_message, line)
        t = threading.Thread(target=reader, daemon=True)
        t.start()

    def run(self):
        self.root.mainloop()

if __name__ == "__main__":
    SplashScreen().run()
