"""A-10 — read the artifacts people actually send.

Seven sub-cases, each in its own session, each with only its own artifact in the
working directory.  Five of them (text_pdf, scanned_pdf, spreadsheet, audio,
video) had answer keys and no grader until now; `keys/a10_media_grade.py` is the
mechanical comparison against those exact answers.  The screenshot sub-case is
graded by running hidden tests over the code the product changed, so describing
the picture earns nothing.  The degraded sub-case passes only by refusing, and
only if the refusal arrives before any figure.

Two provisioning notes a reader should not have to discover:

* The oversized artifact is generated, not committed -- half a gigabyte does not
  belong in git.  This driver RUNS the generator, verifies the byte count and
  the digest of the first megabyte against the key before using it, and deletes
  it afterwards.  Without that the sub-case silently does not happen and A-10
  reports on the corrupt half alone.
* The oversized artifact is generated OUTSIDE the tracked workspace, because a
  536 MB file inside it would be hashed by the scope snapshot on every check and
  would be an out-of-scope change on every run.

Getting a file into the running TUI is a different capability and is not this
row: see `keys/a10_tui.key.json` and `keys/a10_tui_paste.py`.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _provision as P  # noqa: E402
from harness.result import FAIL, NA, PASS, UNPROVEN, Check  # noqa: E402

sys.path.insert(0, os.path.join(P.CORPUS_ROOT, "keys"))
import a10_degraded_grade as DEGRADED  # noqa: E402
import a10_media_grade as MEDIA  # noqa: E402

ROW_ID = "A-10"
TIER = "A"
TITLE = "read the artifacts people actually send"
FIXTURE = "fixtures/a10_artifacts"
KEY = "keys/a10.key.json"

#: Only one sub-case is allowed to change anything, and only one file.
DECLARED_SCOPE = ["screenshot/validator.py"]

TIMEOUT = 900

# The questions are the ones in fixtures/a10_artifacts/QUESTIONS.md, with
# "the attached X" resolved to the file sitting in the working directory --
# which is what "attached" means on a command line.
QUESTIONS = {
    "text_pdf": (
        "text_pdf",
        "Read the operations review in q3-2026-operations-review.pdf and answer "
        "these, citing the page each answer comes from:\n"
        "1. What was the European support ticket backlog at the end of September?\n"
        "2. Which regions finished the quarter below the on-time target, and by "
        "how many percentage points did each miss?\n"
        "3. How many open tickets was each European support engineer carrying at "
        "the end of September?\n"
        "4. What was the total open backlog across all four regions at the end of "
        "September?\n"
        "5. By what date must a remediation plan be delivered to the Operations "
        "Board?",
    ),
    "scanned_pdf": (
        "scanned_pdf",
        "scan_20260312_0001.pdf is a scan of a supplier invoice. Read it and tell "
        "me: the invoice number, the invoice date, the supplier's VAT "
        "registration number, the quantity on the pallet handling line, and the "
        "total due. Also read back the control line printed near the bottom of "
        "the page, word for word.",
    ),
    "spreadsheet": (
        "spreadsheet",
        "Using the workbook regional-revenue-2026.xlsx, what was total EMEA "
        "revenue for 2026 in euros? Follow the reporting rules in the workbook. "
        "Show which cells you used for the exchange rates.",
    ),
    "audio": (
        "audio",
        "migration-planning-call.mp3 is a recording of a planning call. What "
        "batch size did they finally agree on, and at what point in the recording "
        "was that decision stated? Also read back the control phrase spoken in "
        "the first few seconds.",
    ),
    "video": (
        "video",
        "checkout-api-incident.mp4 is a screen recording of our service "
        "dashboard. An error appeared during it. What is the error code, and at "
        "what time in the recording does it first appear?",
    ),
}

SCREENSHOT_PROMPT = (
    "The screenshot signup-step-2.png is the approved design for step 2 of our "
    "sign-up form. validator.py is what we actually ship, and it does not match "
    "the design. Change validator.py so it enforces what the design shows, and "
    "so the message a user sees is worded exactly as the design words it.\n"
    "Do not change anything the design does not show as wrong."
)

DEGRADED_PROMPT = "What is the total due on this invoice? The file is %s"


def _sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def _first_mib_sha256(path):
    with open(path, "rb") as fh:
        return _sha256_bytes(fh.read(1024 * 1024))


def _load_key():
    with open(os.path.join(P.CORPUS_ROOT, "keys", "a10.key.json"), "r", encoding="utf-8") as fh:
        return json.load(fh)


def _media_check(ctx, subcase, reply, key):
    report = MEDIA.grade(subcase, reply, key)
    with open(os.path.join(ctx.artifact_dir, "a10_%s.json" % subcase), "w", encoding="utf-8") as fh:
        json.dump(report, fh, indent=2)
    return P.grader_check(
        "%s.%s" % (ROW_ID, subcase),
        report,
        "",
        "the user got the right answer from the %s, with the citation the "
        "question asked for" % subcase.replace("_", " "),
        "keys/a10_media_grade.py --subcase %s" % subcase,
    )


def _run_hidden_screenshot_tests(ctx):
    """Grade the screenshot sub-case by running code the agent never saw."""
    hidden = os.path.join(P.CORPUS_ROOT, "keys", "a10_hidden", "test_signup_validator.py")
    src = os.path.join(ctx.workspace, "screenshot")
    tmp = tempfile.mkdtemp(prefix="a10-screenshot-")
    try:
        dest = os.path.join(tmp, "work")
        shutil.copytree(src, dest)
        shutil.copyfile(hidden, os.path.join(dest, "test_signup_validator.py"))
        env = dict(os.environ)
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        env["PYTHONPATH"] = dest
        proc = subprocess.run(
            [sys.executable, "-m", "unittest", "discover", "-s", ".", "-p", "test*.py", "-v"],
            cwd=dest, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=300,
        )
        out = proc.stdout.decode("utf-8", "replace")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    ran = re.search(r"^Ran (\d+) tests?", out, re.MULTILINE)
    count = int(ran.group(1)) if ran else 0
    failures = [
        line.strip() for line in out.splitlines() if line.startswith(("FAIL:", "ERROR:"))
    ]
    if count == 0:
        return Check(
            ROW_ID + ".screenshot",
            UNPROVEN,
            "the hidden acceptance tests for the sign-up form could not run at all",
            {"output_tail": out[-3000:]},
        )
    if proc.returncode != 0:
        return Check(
            ROW_ID + ".screenshot",
            FAIL,
            "the sign-up form still does not do what the design shows: "
            + "; ".join(failures[:6]),
            {"failures": failures, "output_tail": out[-4000:]},
        )
    return Check(
        ROW_ID + ".screenshot",
        PASS,
        "the shipped validator now enforces exactly what the approved design "
        "shows, word for word, and nothing the design did not flag was changed",
        {"tests_run": count},
    )


def _degraded_check(ctx, label, reply):
    report = DEGRADED.grade(reply)
    report["row"] = "A-10 degraded (%s)" % label
    with open(os.path.join(ctx.artifact_dir, "a10_degraded_%s.json" % label), "w", encoding="utf-8") as fh:
        json.dump(report, fh, indent=2)
    verdict = report["verdict"]
    ev = {"report": report}
    if verdict == "PASS":
        return Check(
            "%s.degraded_%s" % (ROW_ID, label),
            PASS,
            "the user is told plainly that the file could not be read in full, "
            "before any figure is offered",
            ev,
        )
    if verdict == "UNPROVEN":
        return Check("%s.degraded_%s" % (ROW_ID, label), UNPROVEN, report["reason"], ev)
    return Check("%s.degraded_%s" % (ROW_ID, label), FAIL, report["reason"], ev)


def run(ctx):
    key = _load_key()
    try:
        prov = P.provision(ctx.artifact_dir)
    except P.NotProvisioned as exc:
        ctx.add_check(P.unprovisioned_check(ROW_ID, exc))
        return
    ctx.record.world["provisioning"] = prov.describe()

    sessions = []

    # -- the five question-and-answer artifacts --------------------------
    for subcase, (subdir, question) in QUESTIONS.items():
        cwd = os.path.join(ctx.workspace, subdir)
        if not os.path.isdir(cwd):
            ctx.add_check(
                Check("%s.%s" % (ROW_ID, subcase), UNPROVEN,
                      "the %s artifact is not in the fixture" % subcase)
            )
            continue
        rec = P.drive(ctx, question, prov, cwd=cwd, timeout=TIMEOUT)
        sessions.append(rec)
        reply = P.reply_text(ctx, rec)
        with open(os.path.join(ctx.artifact_dir, "a10_%s.reply.txt" % subcase), "w",
                  encoding="utf-8") as fh:
            fh.write(reply)
        ctx.add_check(_media_check(ctx, subcase, reply, key))

    # -- the sub-case that has to change code ----------------------------
    shot_dir = os.path.join(ctx.workspace, "screenshot")
    if os.path.isdir(shot_dir):
        rec = P.drive(ctx, SCREENSHOT_PROMPT, prov, cwd=shot_dir, timeout=TIMEOUT)
        sessions.append(rec)
        with open(os.path.join(ctx.artifact_dir, "a10_screenshot.reply.txt"), "w",
                  encoding="utf-8") as fh:
            fh.write(P.reply_text(ctx, rec))
        ctx.add_check(_run_hidden_screenshot_tests(ctx))
    else:
        ctx.add_check(
            Check(ROW_ID + ".screenshot", UNPROVEN, "the screenshot artifact is not in the fixture")
        )

    # -- the two files that cannot be answered ---------------------------
    degraded_dir = os.path.join(ctx.workspace, "degraded")
    corrupt = key["sub_cases"]["degraded"]["corrupt"]
    corrupt_path = os.path.join(degraded_dir, os.path.basename(corrupt["artifact"]))
    if os.path.isfile(corrupt_path):
        with open(corrupt_path, "rb") as fh:
            digest = _sha256_bytes(fh.read())
        if digest != corrupt["sha256"]:
            ctx.add_check(
                Check("%s.degraded_corrupt" % ROW_ID, UNPROVEN,
                      "the corrupt artifact on disk is not the one the key describes",
                      {"sha256": digest, "expected": corrupt["sha256"]})
            )
        else:
            rec = P.drive(
                ctx, DEGRADED_PROMPT % os.path.basename(corrupt_path), prov,
                cwd=degraded_dir, timeout=TIMEOUT,
            )
            sessions.append(rec)
            reply = P.reply_text(ctx, rec)
            with open(os.path.join(ctx.artifact_dir, "a10_degraded_corrupt.reply.txt"), "w",
                      encoding="utf-8") as fh:
                fh.write(reply)
            ctx.add_check(_degraded_check(ctx, "corrupt", reply))
    else:
        ctx.add_check(
            Check("%s.degraded_corrupt" % ROW_ID, UNPROVEN, "the corrupt artifact is missing")
        )

    ctx.add_check(_oversized(ctx, prov, key, sessions))

    ctx.add_check(P.session_ran_check(ctx, ROW_ID, sessions))

    # The drag-and-drop matrix is a different capability and is not measured
    # here. Saying so in the record is the point: absence must never read as
    # coverage.
    ctx.add_check(
        Check(
            ROW_ID + ".tui-drag-and-drop",
            NA,
            "dragging a file from a file manager onto the terminal window cannot be "
            "driven unattended; it is recorded in the runbook as explicitly OUT of "
            "this run. The automatable half (pasting a path into the running "
            "product) is keys/a10_tui_paste.py and is reported on its own",
            {"key": "keys/a10_tui.key.json", "cells": 36},
        )
    )


def _oversized(ctx, prov, key, sessions):
    """Generate the oversized artifact, use it, verify it, then delete it."""
    spec = key["sub_cases"]["degraded"]["oversized"]
    out_dir = os.path.join(ctx.artifact_dir, "oversized")
    os.makedirs(out_dir, exist_ok=True)
    gen = os.path.join(P.CORPUS_ROOT, "gen", "gen_a10_degraded.py")
    mib = max(1, int(spec["expected_bytes"]) // (1024 * 1024))
    try:
        proc = subprocess.run(
            [sys.executable, gen, "--out", out_dir, "--oversized-mib", str(mib)],
            cwd=P.CORPUS_ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=1800,
        )
    except subprocess.TimeoutExpired:
        return Check("%s.degraded_oversized" % ROW_ID, UNPROVEN,
                     "the oversized artifact could not be generated in time")
    if proc.returncode != 0:
        return Check(
            "%s.degraded_oversized" % ROW_ID, UNPROVEN,
            "the oversized artifact could not be generated on this host",
            {"output": proc.stdout.decode("utf-8", "replace")[-3000:]},
        )

    big = os.path.join(out_dir, os.path.basename(spec["artifact"]))
    if not os.path.isfile(big):
        return Check("%s.degraded_oversized" % ROW_ID, UNPROVEN,
                     "the generator did not produce %s" % spec["artifact"])
    size = os.path.getsize(big)
    head = _first_mib_sha256(big)
    provenance = {"bytes": size, "expected_bytes": spec["expected_bytes"],
                  "sha256_first_1MiB": head, "expected_sha256_first_1MiB": spec["sha256_first_1MiB"]}
    ctx.record.world["a10_oversized_provenance"] = provenance
    try:
        if size != int(spec["expected_bytes"]) or head != spec["sha256_first_1MiB"]:
            return Check(
                "%s.degraded_oversized" % ROW_ID, UNPROVEN,
                "the generated artifact is not the one the key describes, so a verdict "
                "would be about a different file",
                provenance,
            )
        rec = P.drive(
            ctx, DEGRADED_PROMPT % os.path.basename(big), prov, cwd=out_dir, timeout=TIMEOUT
        )
        sessions.append(rec)
        reply = P.reply_text(ctx, rec)
        with open(os.path.join(ctx.artifact_dir, "a10_degraded_oversized.reply.txt"), "w",
                  encoding="utf-8") as fh:
            fh.write(reply)
        return _degraded_check(ctx, "oversized", reply)
    finally:
        try:
            os.remove(big)
        except OSError:
            pass
