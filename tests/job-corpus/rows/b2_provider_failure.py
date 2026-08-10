"""B-2 — the month-end report, and the provider dies halfway through.

Graded on CONTINUITY OF THE WORK, never on failover mechanics.  The user said,
once, which account code to stamp and how to round.  What has to survive the
outage is that instruction and the half of the job already on disk — a run that
comes back having forgotten the account code, or that quietly restarts with
different columns, has survived nothing.

The outage is real and is induced on the wire: `provider_proxy.py` relays to the
operator's actual provider until `ledger/schema.json` appears in the project,
then breaks the connection for six requests before healing.  Timing the fault
off the WORK rather than off a stopwatch is what makes it land mid-task in the
same place every time.  The proxy records method, status and timing only — never
a body, never a credential header.

Three artefacts decide the grade and the product wrote none of them: the
proxy's ledger, the harness's snapshot of the project at the instant the outage
began, and the hidden acceptance test in `keys/b-2/` executing the delivered
code against data the prompt never showed.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import threading
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
    copy_tree,
    finish,
    free_port,
    grade,
    new_record,
    provider_base_url,
    unproven_setup,
    verdict_check,
    wait_for_port,
)

ROW_ID = "B-2"
TIER = "B"
TITLE = "a provider failure does not become the user's problem"
FIXTURE = "fixtures/b2-provider-failure"
KEY = "keys/b-2.key.json"
DECLARED_SCOPE = ["ledger/schema.json", "billing/reconcile.py"]
TEST_COMMAND = None
TIMEOUT = 1200

FIX = os.path.join(FIXTURES, "b2-provider-failure")
TRIGGER_REL = os.path.join("ledger", "schema.json")

#: `control` proves the job is achievable and gradeable at all.  Every other
#: case is the same job with a different failure shape on the wire.
CASES = [
    ("control", None),
    ("fault-reset", "reset"),
    ("fault-503", "http503"),
    ("fault-timeout", "timeout"),
]
DEFAULT_CASES = ("control", "fault-reset")

RUN_ARGS = ["--trust-workspace", "--dangerously-skip-permissions"]


def _cases_requested():
    want = os.environ.get("JOBCORPUS_B2_CASES", "").strip()
    names = {n.strip() for n in want.split(",") if n.strip()} or set(DEFAULT_CASES)
    return [c for c in CASES if c[0] in names]


def _prompt():
    with open(os.path.join(FIX, "prompt.txt"), "r", encoding="utf-8") as fh:
        return fh.read().strip()


class FaultSnapshot(threading.Thread):
    """Copy the whole project the instant the first half of the job lands.

    Without this snapshot there is nothing to compare the finished report
    against, and 'the work continued' is unfalsifiable.
    """

    def __init__(self, ws, trigger, dest, deadline):
        super().__init__(daemon=True)
        self.ws, self.trigger, self.dest, self.deadline = ws, trigger, dest, deadline
        self.taken_at = None

    def run(self):
        while time.time() < self.deadline:
            if os.path.exists(self.trigger):
                for attempt in range(3):
                    try:
                        copy_tree(self.ws, self.dest)
                        with open(os.path.join(self.dest, TRIGGER_REL), "r",
                                  encoding="utf-8") as fh:
                            json.load(fh)
                        self.taken_at = time.time()
                        return
                    except Exception:
                        time.sleep(0.4)
                self.taken_at = time.time()
                return
            time.sleep(0.2)


def run_case(binary, artifact_dir, case, fault, prompt, timeout, upstream):
    evid = os.path.join(artifact_dir, "evidence", case)
    ws = os.path.join(artifact_dir, "ws", case)
    ev = CaseEvidence(evid, case)
    watcher = None
    try:
        if os.path.isdir(ws):
            shutil.rmtree(ws)
        seeded = ev.run_helper(
            [sys.executable, os.path.join(FIX, "seed_workspace.py"), "--dest", ws],
            "seed-workspace", timeout=180)
        if seeded.returncode != 0:
            raise FixtureError("seed_workspace failed: %s" % seeded.stderr.strip()[:400])

        port = free_port()
        proxy = [
            sys.executable, os.path.join(FIX, "provider_proxy.py"),
            "--port", str(port), "--upstream", upstream,
            "--ledger", os.path.join(evid, "proxy-ledger.jsonl"),
        ]
        if fault:
            proxy += ["--trigger-path", os.path.join(ws, TRIGGER_REL),
                      "--fault", fault, "--fault-requests", "6"]
            if fault == "timeout":
                proxy += ["--hang-seconds", "90"]
        ev.start_helper(proxy, "provider-proxy")
        if not wait_for_port(port, timeout=30):
            raise FixtureError("the provider proxy never came up on port %d" % port)

        if fault:
            watcher = FaultSnapshot(ws, os.path.join(ws, TRIGGER_REL),
                                    os.path.join(evid, "workspace-at-fault"),
                                    time.time() + timeout)
            watcher.start()

        home = ProductHome(os.path.join(artifact_dir, "home", case), session=True,
                           base_url="http://127.0.0.1:%d" % port)
        result = ev.run_product(binary, [*RUN_ARGS, prompt], "start", cwd=ws,
                                env=home.env(), timeout=timeout)

        ev.snapshot(ws, "workspace-final")
        ev.capture_git(ws)
        ev.write_run_json(
            case="control" if not fault else "fault",
            fault_shape=fault or "none",
            case_id=case,
            proxy_port=port,
            exit_code=result["exit_code"],
            timed_out=result["timed_out"],
            # The job was briefed once. Nothing in this driver ever speaks to it
            # again, so this is a fact about the harness, not a claim about the
            # product.
            user_restated=False,
            fault_snapshot_taken=bool(watcher and watcher.taken_at),
        )
        return evid
    finally:
        ev.close()


def main(binary: str, artifact_dir: str):
    rec = new_record(ROW_ID, TIER, TITLE, binary, os.path.join(CORPUS_ROOT, KEY))
    timeout = int(os.environ.get("JOBCORPUS_B2_TIMEOUT", TIMEOUT))
    wanted = _cases_requested()
    rec.world["cases_declared"] = [c[0] for c in CASES]
    rec.world["cases_requested"] = [c[0] for c in wanted]

    try:
        prompt = _prompt()
        upstream = provider_base_url()
        if not upstream:
            raise FixtureError(
                "the operator's provider fragment declares no base_url, so there "
                "is no upstream for the fault proxy to break")
    except Exception as exc:
        return finish(unproven_setup(rec, ROW_ID + ".fixture", exc), artifact_dir)

    verdicts = {}
    for case, fault in wanted:
        try:
            evid = run_case(binary, artifact_dir, case, fault, prompt, timeout, upstream)
        except Exception as exc:
            verdicts[case] = {"state": "UNPROVEN",
                              "reasons": ["the case could not be staged: %s" % exc],
                              "observed": {}, "notes": []}
            continue
        verdicts[case] = grade("grade_b2.py", evid)

    control = verdicts.get("control")
    control_ok = bool(control) and control.get("state") == "PASS"
    if control is not None:
        rec.add_check(verdict_check(ROW_ID + ".control", control))
    else:
        rec.add_check(Check(
            ROW_ID + ".control", UNPROVEN,
            "the undisturbed control was not run, so nothing establishes that this "
            "report is achievable at all and no outage case can be charged to the "
            "product"))

    for case, _fault in CASES:
        if case == "control":
            continue
        check_id = "%s.%s" % (ROW_ID, case)
        if case not in verdicts:
            if case in DEFAULT_CASES:
                rec.add_check(Check(check_id, UNPROVEN,
                                    "failure shape %s was never induced in this run" % case))
            else:
                rec.add_check(Check(
                    check_id, UNPROVEN,
                    "failure shape %s is declared by the fixture and was not run "
                    "tonight; its absence is not evidence that it is survivable"
                    % case))
            continue
        force = UNPROVEN if (verdicts[case].get("state") == "FAIL" and not control_ok) else None
        check = verdict_check(check_id, verdicts[case], force_state=force)
        if force:
            check.why = ("the undisturbed control did not pass, so this outage case "
                         "cannot be charged to the product: " + check.why)
        rec.add_check(check)

    rec.world["grader_verdicts"] = verdicts
    return finish(rec, artifact_dir)
