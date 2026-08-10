#!/usr/bin/env python3
"""The automatable half of the A-10 attachment matrix: pasting a path.

    python3 a10_tui_paste.py --binary <wayland-core> --out <scratch dir>
                             [--wayland-home <dir>] [--json out.json]
                             [--skip-png] [--timeout 240]

`keys/a10_tui.key.json` describes 36 cells: 4 locations x 3 file types x 3
platforms, each driven by dragging the file from a file manager onto the
terminal window.  **Drag-and-drop cannot be driven unattended**, and faking it
with a backend document-parsing test is exactly the substitution this corpus
forbids: understanding a PDF and receiving one are different capabilities, and
on Windows the receiving half is the one that breaks.

So this script does only what a terminal can genuinely do on its own: it starts
the product's interactive line surface, pastes an absolute path followed by the
question, and reads the reply.  That is a real user route -- a person who cannot
drag does exactly this -- and it exercises the same four locations that break
paths in practice: a plain directory, one with a space, one with an apostrophe
and a double quote, and one with non-ASCII components.

Every artifact carries a canary that appears inside the file and nowhere in its
name or its path, so a reply containing the canary proves the bytes were read.
Echoing the path, or describing the file from its name, cannot produce it.

The remaining cells -- the drag-and-drop route on all three platforms, and this
paste route on macOS and Windows -- are OUT of tonight's run and are named in
RUNBOOK.md so their absence is never read as coverage.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
GEN = os.path.join(ROOT, "gen", "gen_a10_tui_attach.py")

CANARIES = {
    "pdf": "CANARY-PDF-TROMBONE-4417",
    "png": "CANARY-PNG-SEAGRASS-8830",
    "txt": "CANARY-TXT-MARIGOLD-2093",
}
DIRECTORIES = [
    ("plain", "plain"),
    ("space", "with space"),
    ("quotes", "it's a \"quoted\" folder" if platform.system() != "Windows"
     else "it's an 'awkward' folder"),
    ("unicode", "ünïcødé-文書"),
]
FILENAMES = {
    "pdf": "Q3 report (final)'s copy.pdf",
    "png": "screen shot 2026-08-10 at 14.02.png",
    "txt": "notes - draft #2.txt",
}

QUESTION = "What token is written inside this file?"


def generate(out_dir, skip_png):
    """Build the artifacts, falling back to the PDF+TXT set without pillow.

    A missing image library is a fact about the host, not about the product.
    Falling back keeps the eight PDF and TXT cells reachable; the four PNG
    cells are then reported UNPROVEN by name rather than quietly vanishing,
    which is the failure mode this whole corpus exists to prevent.
    """
    argv = [sys.executable, GEN, "--out", out_dir]
    if skip_png:
        argv.append("--skip-png")
    proc = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=600)
    out = proc.stdout.decode("utf-8", "replace")
    if proc.returncode != 0 and not skip_png and "PIL" in out:
        proc = subprocess.run(
            argv + ["--skip-png"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=600
        )
        return proc.returncode, out + "\n[retried without PNG: pillow is not installed here]\n" \
            + proc.stdout.decode("utf-8", "replace"), True
    return proc.returncode, out, skip_png


def paste_cell(binary, path, wayland_home, timeout, env_extra=None, cwd=None):
    """One cell: paste the path with the question, read what comes back.

    The session is started IN the artifact's own directory. The product treats
    its working directory as the sandbox root, and a first run driven from
    elsewhere failed every cell with "outside sandbox root" -- a fact about
    where the harness stood, not about whether a pasted path can be read. That
    run also showed something worth keeping: the PDF cells came back with their
    canary from the SAME out-of-root directory that the plain-text cells were
    refused in, so the two read paths disagree about the boundary. It is
    recorded in `boundary_asymmetry` rather than being tidied away.
    """

    env = dict(os.environ)
    for leak in ("API_KEY", "FLUX_API_KEY"):
        env.pop(leak, None)
    if wayland_home:
        env["WAYLAND_HOME"] = wayland_home
    env["NO_COLOR"] = "1"
    env["TERM"] = "dumb"
    if env_extra:
        env.update(env_extra)
    # The interactive line surface, driven the way a person without a mouse
    # drives it: the path goes in, the question follows it, one message.
    stdin = "%s\n%s\n" % ("%s %s" % (path, QUESTION), "/exit")
    try:
        proc = subprocess.run(
            [binary, "--no-tui", "--auto-approve"],
            cwd=cwd or os.path.dirname(path),
            input=stdin.encode("utf-8"),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        return {"timed_out": True, "output": (exc.stdout or b"").decode("utf-8", "replace")}
    return {"timed_out": False, "exit_code": proc.returncode,
            "output": proc.stdout.decode("utf-8", "replace")}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--wayland-home", default=None)
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--skip-png", action="store_true")
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    code, gen_out, png_skipped = generate(out_dir, args.skip_png)
    report = {
        "row": "A-10 attachment (paste route)",
        "route": "an absolute path pasted into the product's interactive line surface",
        "not_measured_here": [
            "dragging the file from a file manager onto the terminal window, on any "
            "platform -- it cannot be driven unattended and is OUT of this run",
            "this paste route on macOS and Windows",
        ],
        "platform": platform.system(),
        "cells": [],
        "boundary_asymmetry": (
            "Observed on Linux 2026-08-10 with the session started OUTSIDE the "
            "artifact tree: the PDF cells returned their canary from "
            "/tmp/.../artifacts while the plain-text cells at the same paths were "
            "refused with 'path is outside sandbox root'. Document extraction and "
            "the file reader disagree about the containment boundary. Cells are now "
            "driven from the artifact's own directory so the matrix measures the "
            "pasted path rather than where the harness stood."
        ),
        "generator": {"returncode": code, "output_tail": gen_out[-2000:]},
    }
    if code != 0:
        report["verdict"] = "UNPROVEN"
        report["reasons"] = ["the attachment fixtures could not be generated on this host"]
        return emit(report, args.json)

    kinds = ["pdf", "png", "txt"]
    reasons = []
    if png_skipped:
        report["not_measured_here"].append(
            "the four PNG cells: pillow is not installed on this host, so the image "
            "artifacts could not be generated. That is a fact about the host, and it "
            "is UNPROVEN, not absent"
        )
        for label, _directory in DIRECTORIES:
            report["cells"].append(
                {"location": label, "kind": "png", "state": "UNPROVEN",
                 "why": "pillow is not installed here, so the PNG artifact was never built"}
            )
        kinds = ["pdf", "txt"]
    for label, directory in DIRECTORIES:
        for kind in kinds:
            path = os.path.join(out_dir, directory, FILENAMES[kind])
            canary = CANARIES[kind]
            if not os.path.isfile(path):
                report["cells"].append(
                    {"location": label, "kind": kind, "state": "UNPROVEN",
                     "why": "the artifact was not generated"}
                )
                continue
            result = paste_cell(
                args.binary, path, args.wayland_home, args.timeout,
                cwd=os.path.dirname(path),
            )
            got = canary in result.get("output", "")
            state = "PASS" if got else ("UNPROVEN" if result.get("timed_out") else "FAIL")
            cell = {
                "location": label,
                "kind": kind,
                "path": path,
                "state": state,
                "canary_returned": got,
                "output_tail": result.get("output", "")[-1500:],
            }
            report["cells"].append(cell)
            if state == "FAIL":
                reasons.append(
                    "%s/%s: the file was on the user's disk at a path they pasted and "
                    "the product did not read it (%s never came back)"
                    % (label, kind, canary)
                )
            elif state == "UNPROVEN":
                reasons.append("%s/%s: the session never finished" % (label, kind))

    states = [c["state"] for c in report["cells"]]
    if "FAIL" in states:
        report["verdict"] = "FAIL"
    elif "UNPROVEN" in states or not states:
        report["verdict"] = "UNPROVEN"
    else:
        report["verdict"] = "PASS"
    report["reasons"] = reasons
    control = [c for c in report["cells"] if c["location"] == "plain"]
    if control and all(c["state"] != "PASS" for c in control):
        report["reasons"].insert(
            0,
            "the plain-directory control did not pass, so the quoting cells say "
            "nothing further: the route itself is broken",
        )
    return emit(report, args.json)


def emit(report, path):
    text = json.dumps(report, indent=2, ensure_ascii=False)
    print(text)
    if path:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
