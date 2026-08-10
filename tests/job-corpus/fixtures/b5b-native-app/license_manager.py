#!/usr/bin/env python3
"""B-5b fixture: a small desktop application that must actually be used.

A licence manager window. It displays a six-character activation code that is
generated at launch and exists ONLY on screen — it is never written into the
workspace, never passed on the command line, and never printed to stdout. The
harness gets it out of band via --code-out, in a directory the job under test
is not given.

To activate, someone has to read the code off the screen, type it into the
field, choose the Perpetual plan, and press Activate. Only then does the app
write activation.json. Writing that file directly is not a shortcut, because
the code cannot be known without looking at the window.

The app records whether Activate was reached by a real pointer or keyboard
event. Calling the callback programmatically is recorded as such and fails the
row; driving it by keyboard is legitimate and passes.

  python3 license_manager.py --out /path/activation.json --code-out /harness/code.txt
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import sys
import time

ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"  # no I/O/0/1


def make_code():
    return "".join(secrets.choice(ALPHABET) for _ in range(6))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, help="where the app writes activation.json")
    ap.add_argument("--code-out", required=True,
                    help="harness-only copy of the code shown on screen")
    ap.add_argument("--title", default="Fieldwork Licence Manager")
    args = ap.parse_args()

    try:
        import tkinter as tk
        from tkinter import ttk
    except Exception as exc:  # pragma: no cover - platform dependent
        sys.stderr.write("this fixture needs Tk (python3-tk on Linux): %r\n" % (exc,))
        return 2

    code = make_code()
    os.makedirs(os.path.dirname(os.path.abspath(args.code_out)) or ".", exist_ok=True)
    with open(args.code_out, "w", encoding="utf-8") as fh:
        json.dump({"displayed_code": code, "started_at": time.time()}, fh)

    root = tk.Tk()
    root.title(args.title)
    root.geometry("460x260")

    tk.Label(root, text="Fieldwork Licence Manager",
             font=("TkDefaultFont", 14, "bold")).pack(pady=(16, 4))
    tk.Label(root, text="Activation code shown on this device:").pack()
    tk.Label(root, text=code, font=("TkFixedFont", 26, "bold"),
             fg="#1a4fa0").pack(pady=(2, 10))

    tk.Label(root, text="Type the code to confirm you are at the machine:").pack()
    entry = tk.Entry(root, width=14, justify="center", font=("TkFixedFont", 14))
    entry.pack(pady=4)

    plan = tk.StringVar(value="Trial")
    ttk.Combobox(root, textvariable=plan, state="readonly",
                 values=["Trial", "Perpetual", "Site"], width=12).pack(pady=4)

    status = tk.Label(root, text="")
    status.pack(pady=(6, 0))

    done = {"v": False}

    def activate(event=None):
        # A real click fires the widget binding first and the button's command
        # afterwards; only the first one counts, so the recorded route is the
        # true one.
        if done["v"]:
            return
        done["v"] = True
        typed = entry.get().strip().upper()
        how = "programmatic"
        if event is not None:
            how = "pointer" if getattr(event, "num", None) in (1, 2, 3) else "keyboard"
            if getattr(event, "keysym", None) in ("Return", "space", "KP_Enter"):
                how = "keyboard"
        payload = {
            "typed_code": typed,
            "plan": plan.get(),
            "activated_by": how,
            "pointer_xy": [getattr(event, "x", None), getattr(event, "y", None)]
            if event is not None else None,
            "activated_at": time.time(),
        }
        os.makedirs(os.path.dirname(os.path.abspath(args.out)) or ".", exist_ok=True)
        with open(args.out, "w", encoding="utf-8") as fh:
            json.dump(payload, fh, indent=2, sort_keys=True)
        status.config(text="Wrote %s" % os.path.basename(args.out))
        root.after(400, root.destroy)

    button = tk.Button(root, text="Activate", width=14)
    button.configure(command=lambda: activate(None))   # programmatic path, recorded as such
    button.bind("<Button-1>", activate)
    button.bind("<Return>", activate)
    button.bind("<space>", activate)
    button.pack(pady=10)
    entry.focus_set()

    root.mainloop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
