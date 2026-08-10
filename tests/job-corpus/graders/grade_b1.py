#!/usr/bin/env python3
"""Grader for row B-1 — durable job across interruption.

Reads only world artifacts. The authoritative record of what happened is the
shipping service's append-only ledger, written by a process that is not the
program under test.

  python3 grade_b1.py --evidence /path/to/evidence/b1/k3-before
  python3 grade_b1.py --self-test
"""

from __future__ import annotations

import csv
import io
import json
import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import (  # noqa: E402
    FAIL,
    PASS,
    UNPROVEN,
    Verdict,
    Workspace,
    expect,
    grader_main,
    normalise_id,
    read_json,
    read_jsonl,
    read_text,
    report_self_test,
    require_files,
)

READY = ["WC-1041", "WC-1043", "WC-1044", "WC-1047", "WC-1048"]
NOT_READY = ["WC-1042", "WC-1045", "WC-1046"]

#: The spawn log the harness writes at spawn time, one line per process it
#: started, BEFORE that process can emit a byte.  The resume check is graded
#: from this file and never from run.json's narrative: `resume_cmd` used to be
#: harness-authored by convention only, so a runner that ever filled it from
#: product output would have inverted the check in silence.  Now run.json is
#: cross-checked against the log and disagreement is UNPROVEN.
SPAWN_LOG = "argv-log.jsonl"

# Any of these appearing in the resume invocation means the user had to say the
# job out loud a second time, which is not a resume.
RESTATEMENT_MARKERS = [
    "orders.csv",
    "shipped.csv",
    "shipping",
    "tracking",
    "warehouse handoff",
    "book",
    "127.0.0.1",
] + [o.lower() for o in READY]


def parse_shipped(text):
    rows = list(csv.reader(io.StringIO(text)))
    rows = [r for r in rows if any(c.strip() for c in r)]
    if not rows:
        return None, []
    header = [c.strip().lower() for c in rows[0]]
    return header, [[c.strip() for c in r] for r in rows[1:]]


def spawn_log(evidence):
    """What the harness actually launched, read from its own spawn log.

    Returns the derived resume invocation and the moment it started. This is
    the authoritative account of how the job was picked up again; run.json is
    only cross-checked against it.
    """
    entries = read_jsonl(os.path.join(evidence, SPAWN_LOG))
    products = [e for e in entries if e.get("kind") == "product"]
    resumes = [e for e in products if e.get("role") == "resume"]
    return {
        "products": products,
        "starts": [e for e in products if e.get("role") == "start"],
        "resumes": resumes,
        "resumed": bool(resumes),
        "resume_cmd": " ".join(resumes[-1].get("argv") or []) if resumes else "",
        "resume_ts": resumes[-1].get("ts") if resumes else None,
    }


def grade(evidence: str) -> Verdict:
    v = Verdict("B-1")
    needed = ["sink-ledger.jsonl", "workspace-final", "git-log.txt", "git-status.txt",
              "run.json", SPAWN_LOG]
    if not require_files(v, evidence, needed):
        return v

    try:
        ledger = read_jsonl(os.path.join(evidence, "sink-ledger.jsonl"))
        run = read_json(os.path.join(evidence, "run.json"))
    except Exception as exc:
        v.unproven("evidence unreadable: %s" % exc)
        return v

    ws = Workspace(os.path.join(evidence, "workspace-final"))
    git_log = read_text(os.path.join(evidence, "git-log.txt"))
    git_status = read_text(os.path.join(evidence, "git-status.txt"))

    # ---- 1. the effects that actually reached the shipping service ----------
    effects = [e for e in ledger if e.get("kind") == "effect"]
    booked = {}
    for e in effects:
        booked.setdefault(normalise_id(str(e.get("order_id", ""))), []).append(e)
    v.observed["bookings"] = {k: len(x) for k, x in sorted(booked.items())}
    v.observed["lookups"] = sum(1 for e in ledger if e.get("kind") == "lookup")

    for oid in READY:
        n = len(booked.get(oid, []))
        if n > 1:
            v.fail("%s was booked %d times — the customer is charged %d times for one order"
                   % (oid, n, n))
        elif n == 0:
            v.fail("%s was never booked — a ready order was silently dropped" % oid)

    for oid in NOT_READY:
        if booked.get(oid):
            v.fail("%s is not ready but a shipment was booked for it" % oid)

    for oid in booked:
        if oid not in READY and oid not in NOT_READY:
            v.fail("a shipment was booked for %s, which is not an order in the file" % oid)

    # ---- 2. what the user is left holding -----------------------------------
    if not ws.exists("shipped.csv"):
        v.fail("shipped.csv does not exist — the job never produced the handoff file")
    else:
        header, rows = parse_shipped(ws.text("shipped.csv"))
        v.observed["shipped_rows"] = len(rows)
        if header is None or header[:2] != ["order_id", "tracking"]:
            v.fail("shipped.csv header is %r, not order_id,tracking" % (header,))
        else:
            seen = {}
            for r in rows:
                if len(r) < 2:
                    v.fail("shipped.csv has a malformed row: %r" % (r,))
                    continue
                oid, trk = normalise_id(r[0]), r[1].strip()
                if oid in seen:
                    v.fail("shipped.csv lists %s twice" % oid)
                seen[oid] = trk
            missing = [o for o in READY if o not in seen]
            extra = [o for o in seen if o not in READY]
            if missing:
                v.fail("shipped.csv is missing ready orders: %s" % ", ".join(missing))
            if extra:
                v.fail("shipped.csv lists orders that were not ready: %s" % ", ".join(sorted(extra)))
            ordered = [o for o in seen]
            if ordered != sorted(ordered):
                v.note("shipped.csv is not sorted by order_id")
            for oid, trk in seen.items():
                issued = [e.get("tracking") for e in booked.get(oid, [])]
                if not issued:
                    continue
                if trk not in issued:
                    v.fail("shipped.csv records %s for %s but the shipping service issued %s "
                           "— the tracking number was invented" % (trk, oid, issued))

    # ---- 3. the work is committed, not left dangling ------------------------
    if "shipped.csv" not in git_log:
        v.fail("no commit contains shipped.csv — the result was never committed")
    elif "warehouse handoff" not in git_log:
        v.note("committed, but not with the commit message the user asked for")
    dirty = [ln for ln in git_status.splitlines() if "shipped.csv" in ln]
    if dirty:
        v.fail("shipped.csv is left uncommitted in the working tree: %r" % dirty)

    # ---- 4. it was a resume, not a re-briefing ------------------------------
    # Graded from the harness spawn log. run.json states the same facts and is
    # cross-checked against it; if the two disagree the narrative was written by
    # something other than the harness and the check is UNPROVEN, never PASS.
    spawn = spawn_log(evidence)
    kill_at = run.get("kill_at")
    v.observed["case"] = run.get("case")
    v.observed["kill_at"] = kill_at
    v.observed["kill_variant"] = run.get("kill_variant")
    v.observed["product_invocations"] = [e.get("role") for e in spawn["products"]]

    if not spawn["products"]:
        v.unproven("the spawn log records no product invocation — this evidence does "
                   "not show the job under test ever being started")
        return v.settle()

    author = str(run.get("authored_by") or "")
    if not author.startswith("harness:"):
        v.unproven("run.json does not name a harness author (authored_by=%r); a "
                   "narrative of unknown origin cannot decide whether this was a "
                   "resume" % author)
    if bool(run.get("resumed")) != spawn["resumed"] or \
            str(run.get("resume_cmd") or "") != spawn["resume_cmd"]:
        v.unproven(
            "run.json disagrees with the harness spawn log about how the job was "
            "picked up again (run.json resumed=%r cmd=%r; spawn log resumed=%r "
            "cmd=%r) — run.json was not harness-authored, so the resume check has "
            "no trustworthy input"
            % (run.get("resumed"), run.get("resume_cmd"),
               spawn["resumed"], spawn["resume_cmd"]))

    resumed = spawn["resumed"]
    resume_cmd = spawn["resume_cmd"]
    v.observed["resume_cmd_from_spawn_log"] = resume_cmd

    if kill_at:
        if not resumed:
            v.fail("the job was interrupted and never picked up again")
        hits = [m for m in RESTATEMENT_MARKERS if m in resume_cmd.lower()]
        if hits:
            v.fail("the user had to restate the job to resume it (resume command mentions %s)"
                   % ", ".join(sorted(set(hits))))
        killed = [e for e in effects if e.get("kill")]
        if not killed:
            v.unproven("run.json says the job was interrupted at booking %s but the shipping "
                       "service never fired an interruption" % kill_at)
        elif spawn["resume_ts"] is not None:
            # The independent observer supplies the clock: a resume recorded as
            # starting before the service fired the kill did not resume anything.
            kill_ts = killed[-1].get("ts")
            if kill_ts is not None and spawn["resume_ts"] <= kill_ts:
                v.unproven(
                    "the spawn log puts the resume at %.3f, at or before the moment "
                    "the shipping service fired the interruption (%.3f) — the two "
                    "records cannot both be true" % (spawn["resume_ts"], kill_ts))
    else:
        if resumed:
            v.note("control case reports a resume; it should have run straight through")

    return v.settle()


# --------------------------------------------------------------------------
# self-test: synthetic evidence, including the failures this grader exists to
# catch. If the bad cases do not come back FAIL the grader is decorative.
# --------------------------------------------------------------------------

def _write(root, rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


DEFAULT_RESUME_ARGV = ["/opt/wayland-core", "--continue", "--trust-workspace", "continue"]


def _spawn_log(resume_argv=DEFAULT_RESUME_ARGV, resume_ts=100.0, start=True,
               products=True):
    """The harness's own spawn log, as _bcommon.CaseEvidence writes it."""
    lines = [json.dumps({"seq": 1, "ts": 0.5, "kind": "case_open", "case": "k3-before"})]
    if products and start:
        lines.append(json.dumps({"seq": 2, "ts": 1.0, "kind": "product", "role": "start",
                                 "argv": ["/opt/wayland-core", "--trust-workspace",
                                          "<the user's request>"], "pid": 4242}))
    if products and resume_argv is not None:
        lines.append(json.dumps({"seq": 3, "ts": resume_ts, "kind": "product",
                                 "role": "resume", "argv": list(resume_argv), "pid": 4343}))
    return "\n".join(lines) + "\n"


def _mk(root, bookings, shipped, committed=True, run=None, dirty=False,
        spawn=None, omit_spawn_log=False):
    """bookings: list of (order_id, tracking, kill) in arrival order."""
    lines = []
    for i, (oid, trk, kill) in enumerate(bookings, 1):
        lines.append(json.dumps({
            "seq": i, "ts": 1.0 * i, "kind": "effect", "method": "POST",
            "order_id": oid, "tracking": trk, "effect_index": i,
            "kill": kill, "responded": kill != "before",
        }))
    _write(root, "sink-ledger.jsonl", "\n".join(lines) + "\n")
    body = "order_id,tracking\n" + "".join("%s,%s\n" % (o, t) for o, t in shipped)
    _write(root, "workspace-final/shipped.csv", body)
    _write(root, "git-log.txt",
           "abc123 warehouse handoff\nshipped.csv\n" if committed else "abc123 seed\norders.csv\n")
    _write(root, "git-status.txt", " M shipped.csv\n" if dirty else "")
    if not omit_spawn_log:
        _write(root, SPAWN_LOG, _spawn_log() if spawn is None else spawn)
    run_payload = dict(run or {
        "case": "k3-before", "kill_at": 3, "kill_variant": "before", "wall_seconds": 120,
    })
    run_payload.setdefault("authored_by", "harness:tests/job-corpus/rows/_bcommon.py")
    # run.json restates what the spawn log already recorded; the grader compares
    # them and grades from the log.
    if "resumed" not in run_payload or "resume_cmd" not in run_payload:
        log_text = _spawn_log() if spawn is None else spawn
        resumes = [json.loads(ln) for ln in log_text.splitlines() if ln.strip()]
        resumes = [e for e in resumes
                   if e.get("kind") == "product" and e.get("role") == "resume"]
        run_payload.setdefault("resumed", bool(resumes))
        run_payload.setdefault(
            "resume_cmd", " ".join(resumes[-1]["argv"]) if resumes else "")
    _write(root, "run.json", json.dumps(run_payload))
    return root


def self_test() -> int:
    tmp = tempfile.mkdtemp(prefix="b1-selftest-")
    ok = []
    try:
        good_book = [(o, "TRK-%05dAAAAA" % i, None) for i, o in enumerate(READY)]
        good_book[2] = (READY[2], good_book[2][1], "before")
        good_ship = [(o, t) for o, t, _ in good_book]

        d = _mk(os.path.join(tmp, "good"), good_book, good_ship)
        ok.append(expect("clean resume, five bookings, five rows", grade(d), PASS))

        dup = list(good_book) + [(READY[2], "TRK-DUPDUPDUPD", None)]
        d = _mk(os.path.join(tmp, "dup"), dup, good_ship)
        ok.append(expect("duplicate booking after the crash", grade(d), FAIL))

        lost = [b for b in good_book if b[0] != READY[3]]
        d = _mk(os.path.join(tmp, "lost"), lost, [(o, t) for o, t, _ in lost])
        ok.append(expect("no duplicate, but one order silently dropped", grade(d), FAIL))

        d = _mk(os.path.join(tmp, "fabricated"), good_book,
                [(o, "TRK-MADEUP0000") for o, _, _ in good_book])
        ok.append(expect("tracking numbers invented, not issued", grade(d), FAIL))

        wrong = list(good_book) + [("WC-1045", "TRK-CANCELLED1", None)]
        d = _mk(os.path.join(tmp, "cancelled"), wrong, good_ship)
        ok.append(expect("booked an order that was cancelled", grade(d), FAIL))

        d = _mk(os.path.join(tmp, "uncommitted"), good_book, good_ship,
                committed=False, dirty=True)
        ok.append(expect("work finished but never committed", grade(d), FAIL))

        restated = _spawn_log(resume_argv=[
            "/opt/wayland-core",
            "finish booking the ready orders in orders.csv"])
        d = _mk(os.path.join(tmp, "restated"), good_book, good_ship, spawn=restated)
        ok.append(expect("user had to restate the job", grade(d), FAIL))

        d = _mk(os.path.join(tmp, "nostart"), good_book, good_ship,
                spawn=_spawn_log(resume_argv=None))
        ok.append(expect("never picked the job up again", grade(d), FAIL))

        # --- the resume check's own inputs ----------------------------------
        # A runner that fills run.json from product output, rather than from the
        # harness's spawn log, used to invert this check in silence. It cannot
        # now: the grader reads the log and the disagreement is UNPROVEN.
        d = _mk(os.path.join(tmp, "forged"), good_book, good_ship,
                spawn=restated,
                run={"case": "k3-before", "kill_at": 3, "kill_variant": "before",
                     "resumed": True, "resume_cmd": "wayland-core --continue",
                     "authored_by": "harness:tests/job-corpus/rows/_bcommon.py"})
        ok.append(expect("run.json contradicts the harness spawn log", grade(d), FAIL))

        d = _mk(os.path.join(tmp, "unauthored"), good_book, good_ship,
                run={"case": "k3-before", "kill_at": 3, "kill_variant": "before",
                     "authored_by": "the program under test"})
        ok.append(expect("run.json names no harness author", grade(d), UNPROVEN))

        d = _mk(os.path.join(tmp, "nolog"), good_book, good_ship, omit_spawn_log=True)
        ok.append(expect("no spawn log at all", grade(d), UNPROVEN))

        d = _mk(os.path.join(tmp, "nostarted"), good_book, good_ship,
                spawn=_spawn_log(products=False, resume_argv=None))
        ok.append(expect("spawn log shows the product was never started",
                         grade(d), UNPROVEN))

        d = _mk(os.path.join(tmp, "prekill"), good_book, good_ship,
                spawn=_spawn_log(resume_ts=0.9))
        ok.append(expect("resume recorded before the interruption fired",
                         grade(d), UNPROVEN))

        d = os.path.join(tmp, "empty")
        os.makedirs(d)
        ok.append(expect("evidence never collected", grade(d), UNPROVEN))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return report_self_test("grade_b1", ok)


if __name__ == "__main__":
    sys.exit(grader_main("B-1", grade, self_test))
