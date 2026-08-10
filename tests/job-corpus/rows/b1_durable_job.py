"""B-1 — tonight's warehouse handoff survives being interrupted.

THE product thesis, and the one row whose failure mode is money: the shipping
service bills per booking and never de-duplicates, so an order booked twice is
a customer charged twice, and an order dropped is a parcel that never ships.

What makes this row independent of any instrument the product carries:

  * Correctness is decided by `shipping_service.py`, a separate process that
    shares no code with the product and writes an fsynced, append-only ledger
    OUTSIDE the workspace the job is given.  The count of external effects —
    HTTP bookings that really arrived — is the whole grade.  Nothing the job
    says about its own progress is an input.
  * The interruption is a real process-tree kill fired by that service at an
    exact write boundary, and the sweep walks EVERY boundary: killed after the
    K-th booking is acknowledged, and killed before its reply is written (the
    shipment exists; the job never heard about it), for K = 1 … 5.
  * Both directions are graded.  Duplicated effects FAIL.  A resume that avoids
    duplication by losing the effect FAILS too.

The resume is an ordinary pick-up-again — a new terminal, `--continue`, one
word — and the invocation is recorded in the harness's own spawn log at spawn
time.  The grader derives the resume command from that log, never from a
narrative field, so the check cannot silently invert.
"""

from __future__ import annotations

import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from _bcommon import (  # noqa: E402
    CORPUS_ROOT,
    FIXTURES,
    UNPROVEN,
    CaseEvidence,
    Check,
    FixtureError,
    ProductHome,
    finish,
    free_port,
    grade,
    new_record,
    read_jsonl,
    unproven_setup,
    verdict_check,
    wait_for_port,
)

ROW_ID = "B-1"
TIER = "B"
TITLE = "a long job survives being interrupted"
FIXTURE = "fixtures/b1-durable-job"
KEY = "keys/b-1.key.json"
#: The job is asked for exactly one new file, committed on the current branch.
DECLARED_SCOPE = ["shipped.csv"]
TEST_COMMAND = None
TIMEOUT = 900

FIX = os.path.join(FIXTURES, "b1-durable-job")
READY = ["WC-1041", "WC-1043", "WC-1044", "WC-1047", "WC-1048"]

#: Eleven cases: the undisturbed control, then every write boundary in turn,
#: killed on both sides of the reply.  Reducing this list is visible in the
#: record — every declared case that does not run is named UNPROVEN.
CASES = [("control", None, None)] + [
    ("k%d-%s" % (k, variant), k, variant)
    for k in range(1, len(READY) + 1)
    for variant in ("after", "before")
]

RUN_ARGS = ["--trust-workspace", "--dangerously-skip-permissions"]
#: The user comes back and says one word.  It names no order, no file, no
#: endpoint and no task — re-briefing the job is not resuming it.
RESUME_WORD = "continue"


def _cases_requested() -> list:
    want = os.environ.get("JOBCORPUS_B1_CASES", "").strip()
    if not want:
        return list(CASES)
    names = {n.strip() for n in want.split(",") if n.strip()}
    return [c for c in CASES if c[0] in names]


def _prompt() -> str:
    with open(os.path.join(FIX, "prompt.txt"), "r", encoding="utf-8") as fh:
        return fh.read().strip()


def saved_session_ids(home):
    """Session ids from the product's own on-disk index — a file, not a
    sentence it printed.  The harness needs the id to follow the product's
    recovery instructions; it must not read it out of prose."""
    path = os.path.join(home.root, "sessions", "index.json")
    try:
        with open(path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return []
    return [s.get("id") for s in data.get("sessions", []) if s.get("id")]


def resume_ladder(ev, binary, ws, env, home, timeout):
    """Pick the job up again, doing exactly what the product tells the user to do.

    An ordinary user types `--continue` and one word.  If the product refuses
    and names a remedy, the user does that and tries again.  What the user
    CANNOT do is adjudicate whether a shipment booking landed — that is the
    knowledge the job was interrupted holding — so the ladder never resolves a
    reconcile item on the product's behalf.  Every step's exit code is recorded.
    """
    steps = []
    first = ev.run_product(binary, ["--continue", *RUN_ARGS, RESUME_WORD], "resume",
                           cwd=ws, env=env, timeout=timeout)
    steps.append({"step": "continue", "exit_code": first["exit_code"]})
    if first["exit_code"] == 0 and not first["timed_out"]:
        return steps

    for sid in saved_session_ids(home)[:1]:
        listed = ev.run_product(binary, ["session", "reconcile", sid], "recover",
                                cwd=ws, env=env, timeout=180)
        steps.append({"step": "session reconcile", "session": sid,
                      "exit_code": listed["exit_code"]})
        cancelled = ev.run_product(binary, ["session", "cancel", sid], "recover",
                                   cwd=ws, env=env, timeout=180)
        steps.append({"step": "session cancel", "session": sid,
                      "exit_code": cancelled["exit_code"]})
        again = ev.run_product(binary, ["--continue", *RUN_ARGS, RESUME_WORD], "resume",
                               cwd=ws, env=env, timeout=timeout)
        steps.append({"step": "continue (2)", "exit_code": again["exit_code"]})
    return steps


def run_case(binary, artifact_dir, case, kill_at, variant, prompt, timeout):
    """Drive one case end to end and return (evidence_dir, notes)."""
    evid = os.path.join(artifact_dir, "evidence", case)
    ws = os.path.join(artifact_dir, "ws", case)
    ev = CaseEvidence(evid, case)
    try:
        port = free_port()
        ledger = os.path.join(evid, "sink-ledger.jsonl")
        pid_file = os.path.join(evid, "job.pid")

        service = [
            sys.executable, os.path.join(FIX, "shipping_service.py"),
            "--port", str(port), "--ledger", ledger,
        ]
        if kill_at is not None:
            service += ["--kill-at", str(kill_at), "--kill-variant", variant,
                        "--pid-file", pid_file]
        ev.start_helper(service, "shipping-service")
        if not wait_for_port(port, timeout=30):
            raise FixtureError("the shipping service never came up on port %d" % port)

        seeded = ev.run_helper(
            [sys.executable, os.path.join(FIX, "seed_workspace.py"),
             "--dest", ws, "--endpoint", "http://127.0.0.1:%d" % port],
            "seed-workspace", timeout=180,
        )
        if seeded.returncode != 0:
            raise FixtureError("seed_workspace failed: %s" % seeded.stderr.strip()[:400])

        home = ProductHome(os.path.join(artifact_dir, "home", case), session=True)
        env = home.env()

        first = ev.run_product(binary, [*RUN_ARGS, prompt], "start", cwd=ws, env=env,
                               timeout=timeout, pid_file=pid_file)

        recovery = []
        if kill_at is not None:
            # The disconnect: the terminal the job was started from is gone and
            # never reused.  Confirm the tree really died before picking it up.
            time.sleep(5)
            ev.capture_processes(needle=os.path.join(artifact_dir, "ws", case))
            recovery = resume_ladder(ev, binary, ws, env, home, timeout)

        ev.snapshot(ws, "workspace-final")
        ev.capture_git(ws)
        survivors = ev.surviving_children()
        ev.write_run_json(
            case=case, kill_at=kill_at, kill_variant=variant,
            endpoint="http://127.0.0.1:%d" % port,
            first_exit=first["exit_code"], first_timed_out=first["timed_out"],
            surviving_processes=survivors,
            recovery_steps=recovery,
        )
        return evid
    finally:
        ev.close()


def main(binary: str, artifact_dir: str):
    rec = new_record(ROW_ID, TIER, TITLE, binary, os.path.join(CORPUS_ROOT, KEY))
    timeout = int(os.environ.get("JOBCORPUS_B1_TIMEOUT", TIMEOUT))
    wanted = _cases_requested()
    rec.world["cases_declared"] = [c[0] for c in CASES]
    rec.world["cases_requested"] = [c[0] for c in wanted]

    try:
        prompt = _prompt()
    except OSError as exc:
        return finish(unproven_setup(rec, ROW_ID + ".fixture", exc), artifact_dir)

    verdicts = {}
    ran = []
    for case, kill_at, variant in wanted:
        try:
            evid = run_case(binary, artifact_dir, case, kill_at, variant, prompt, timeout)
        except Exception as exc:  # a case that could not be staged is UNPROVEN
            verdicts[case] = {"state": "UNPROVEN",
                              "reasons": ["the case could not be staged: %s" % exc],
                              "observed": {}, "notes": []}
            continue
        verdicts[case] = grade("grade_b1.py", evid)
        ran.append(case)
        ledger = read_jsonl(os.path.join(evid, "sink-ledger.jsonl"))
        rec.world.setdefault("effects_per_case", {})[case] = {
            "bookings": sum(1 for e in ledger if e.get("kind") == "effect"),
            "lookups": sum(1 for e in ledger if e.get("kind") == "lookup"),
        }

    rec.world["cases_run"] = ran

    # The control proves the job is achievable at all.  Without it the kill
    # cases are UNPROVEN, not FAIL: a broken fixture is not a product defect.
    control = verdicts.get("control")
    control_ok = bool(control) and control.get("state") == "PASS"
    if control is not None:
        rec.add_check(verdict_check(ROW_ID + ".control", control))
    else:
        rec.add_check(Check(
            ROW_ID + ".control", UNPROVEN,
            "the undisturbed control was not run, so nothing establishes that this "
            "job is achievable at all and no interrupted case can be read as a "
            "product defect"))

    for case, _kill, _variant in CASES:
        if case == "control":
            continue
        check_id = "%s.%s" % (ROW_ID, case)
        if case not in verdicts:
            rec.add_check(Check(
                check_id, UNPROVEN,
                "write boundary %s was never exercised in this run; absence of a "
                "result here is not evidence that the boundary is safe" % case))
            continue
        v = verdicts[case]
        force = None
        if v.get("state") == "FAIL" and not control_ok:
            force = UNPROVEN
        check = verdict_check(check_id, v, force_state=force)
        if force:
            check.why = ("the undisturbed control did not pass, so this interrupted "
                         "case cannot be charged to the product: " + check.why)
        rec.add_check(check)

    rec.world["grader_verdicts"] = verdicts
    return finish(rec, artifact_dir)
