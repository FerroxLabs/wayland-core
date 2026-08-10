#!/usr/bin/env python3
"""Grader for row B-2 — survive a provider failure mid-task.

Graded on the WORK, never on failover mechanics. The only question is whether
the user ended up with the report they asked for, built continuously: the half
finished before the outage still standing, and the half finished after it
consistent with that and with the instructions the user gave once, at the
start.

  python3 grade_b2.py --evidence /path/to/evidence/b2/fault
  python3 grade_b2.py --self-test
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
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
    read_json,
    read_jsonl,
    read_text,
    report_self_test,
    require_files,
)

HERE = os.path.dirname(os.path.abspath(__file__))
ACCEPTANCE = os.path.join(HERE, "..", "keys", "b-2", "acceptance", "check_reconcile.py")

LEFTOVER = re.compile(r"(\.bak$|\.orig$|\.rej$|_v2\b|_old\b|_new\b|_final\b|copy\.py$)", re.I)


def run_acceptance(workspace: str):
    script = os.path.abspath(ACCEPTANCE)
    if not os.path.exists(script):
        return None, "hidden acceptance test missing at %s" % script
    tmp = tempfile.mkdtemp(prefix="b2-acc-")
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    try:
        proc = subprocess.run(
            [sys.executable, script, "--workspace", os.path.abspath(workspace)],
            cwd=tmp, capture_output=True, text=True, timeout=180, env=env,
        )
    except subprocess.TimeoutExpired:
        return None, "the delivered code did not finish within 180 s"
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    try:
        return json.loads(proc.stdout), None
    except json.JSONDecodeError:
        return None, "acceptance run produced no result (stderr: %s)" % proc.stderr.strip()[:400]


def grade(evidence: str) -> Verdict:
    v = Verdict("B-2")
    if not require_files(v, evidence, ["workspace-final", "git-log.txt", "run.json"]):
        return v
    try:
        run = read_json(os.path.join(evidence, "run.json"))
    except Exception as exc:
        v.unproven("run.json unreadable: %s" % exc)
        return v
    case = str(run.get("case", "fault"))
    v.observed["case"] = case
    # A case name the grader does not recognise used to fall through every
    # outage check and still settle PASS: "fault-reset" is not "fault". The
    # match is now a prefix, and anything else is UNPROVEN rather than a quiet
    # skip of the only checks this row exists for.
    lowered = case.strip().lower()
    is_fault = lowered.startswith("fault")
    if not is_fault and not lowered.startswith("control"):
        v.unproven("run.json case is %r; expected a name beginning 'control' or "
                   "'fault'. An unrecognised case would skip every outage check "
                   "and still look graded." % case)
        return v

    final = Workspace(os.path.join(evidence, "workspace-final"))

    # ---- 1. did the user get a correct report? ------------------------------
    result, err = run_acceptance(final.root)
    if err:
        v.unproven(err)
    else:
        v.observed["acceptance"] = result.get("observed", {})
        for f in result.get("failures", []):
            v.fail("report is wrong: %s" % f)

    # ---- 2. was there really an outage to survive? --------------------------
    if is_fault:
        ledger_path = os.path.join(evidence, "proxy-ledger.jsonl")
        if not os.path.exists(ledger_path):
            v.unproven("no proxy ledger — cannot tell whether a provider failure happened")
        else:
            entries = read_jsonl(ledger_path)
            faults = [e for e in entries if e.get("kind") == "fault"]
            relays = [e for e in entries if e.get("kind") == "relay"]
            v.observed["provider_requests_broken"] = len(faults)
            v.observed["provider_requests_relayed"] = len(relays)
            if not faults:
                v.unproven("the provider never failed — the outage was not induced, "
                           "so nothing about surviving one was tested")
            else:
                after = [e for e in relays if e.get("seq", 0) > faults[0].get("seq", 0)]
                if not after:
                    v.note("no further traffic reached this provider after the outage; "
                           "the work either finished elsewhere or stopped")

    # ---- 3. continuity: the pre-outage work still stands ---------------------
    if is_fault:
        at_fault_dir = os.path.join(evidence, "workspace-at-fault")
        if not os.path.isdir(at_fault_dir):
            v.unproven("no snapshot of the project at the moment of the outage")
        else:
            at_fault = Workspace(at_fault_dir)
            if not at_fault.exists("ledger/schema.json"):
                v.unproven("the outage landed before the first half of the job produced "
                           "anything — it was not mid-task")
            else:
                try:
                    before = json.loads(at_fault.text("ledger/schema.json")).get("columns")
                except Exception:
                    before = None
                if not final.exists("ledger/schema.json"):
                    v.fail("the work completed before the outage was deleted afterwards "
                           "(ledger/schema.json is gone)")
                else:
                    try:
                        after_cols = json.loads(final.text("ledger/schema.json")).get("columns")
                    except Exception as exc:
                        v.fail("ledger/schema.json is no longer valid JSON after the "
                               "outage: %s" % exc)
                        after_cols = None
                    if before is not None and after_cols is not None and before != after_cols:
                        v.fail("the report's columns changed across the outage (%r -> %r) — "
                               "the work restarted instead of continuing"
                               % (before, after_cols))

    # ---- 4. no abandoned half-written copies --------------------------------
    leftovers = []
    for rel in final.walk_files():
        if "__pycache__/" in rel or rel.endswith(".pyc"):
            continue
        base = os.path.basename(rel)
        if LEFTOVER.search(base):
            leftovers.append(rel)
        elif rel.startswith("billing/") and "reconcile" in base and base != "reconcile.py":
            leftovers.append(rel)
    if leftovers:
        v.fail("abandoned half-finished copies left in the project: %s"
               % ", ".join(sorted(leftovers)))

    # ---- 5. delivered, and delivered once -----------------------------------
    git_log = read_text(os.path.join(evidence, "git-log.txt"))
    for path in ("billing/reconcile.py", "ledger/schema.json"):
        if path not in git_log:
            v.fail("%s was never committed" % path)
    if run.get("user_restated"):
        v.fail("the user had to repeat the instructions after the outage")

    return v.settle()


# --------------------------------------------------------------------------
# self-test
# --------------------------------------------------------------------------

GOOD_IMPL = '''
import csv, json, os
from decimal import Decimal, ROUND_HALF_UP

def _round2(x):
    return float(Decimal(repr(x)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP))

def build_report(usage_path, contracts_path):
    contracts = {}
    with open(contracts_path, newline="", encoding="utf-8") as fh:
        for r in csv.DictReader(fh):
            contracts[r["customer_id"]] = r
    usage = {}
    with open(usage_path, newline="", encoding="utf-8") as fh:
        for r in csv.DictReader(fh):
            usage[r["customer_id"]] = usage.get(r["customer_id"], 0.0) + float(r["quantity"])
    rows = []
    for cid in sorted(usage):
        c = contracts.get(cid)
        if not c:
            continue
        rate = float(c["rate_per_unit"])
        raw = max(usage[cid] * rate, float(c["minimum_usd"]))
        rows.append({
            "account_code": "%(CODE)s",
            "customer_id": cid,
            "plan": c["plan"],
            "quantity": usage[cid],
            "rate_per_unit": rate,
            "charge_usd": _round2(raw),
        })
    return rows
'''

BANKERS_IMPL = GOOD_IMPL.replace(
    'return float(Decimal(repr(x)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP))',
    "return round(x, 2)")


def _seed_ws(root, impl_code, code="LC-7731", columns=None):
    cols = columns or ["account_code", "customer_id", "plan", "quantity",
                       "rate_per_unit", "charge_usd"]
    os.makedirs(os.path.join(root, "billing"), exist_ok=True)
    os.makedirs(os.path.join(root, "ledger"), exist_ok=True)
    os.makedirs(os.path.join(root, "data"), exist_ok=True)
    src = os.path.join(HERE, "..", "fixtures", "b2-provider-failure", "seed", "data")
    for name in ("contracts.csv", "usage-2026-07.csv"):
        shutil.copyfile(os.path.join(src, name), os.path.join(root, "data", name))
    open(os.path.join(root, "billing", "__init__.py"), "w").close()
    with open(os.path.join(root, "billing", "reconcile.py"), "w", encoding="utf-8") as fh:
        fh.write(impl_code % {"CODE": code})
    with open(os.path.join(root, "ledger", "schema.json"), "w", encoding="utf-8") as fh:
        json.dump({"columns": cols}, fh)
    return root


def _seed_evidence(root, impl=GOOD_IMPL, code="LC-7731", columns=None,
                   fault=True, committed=True, at_fault_cols=None, run_extra=None,
                   leftover=None):
    os.makedirs(root, exist_ok=True)
    _seed_ws(os.path.join(root, "workspace-final"), impl, code, columns)
    if leftover:
        path = os.path.join(root, "workspace-final", leftover)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, "w").close()
    af = os.path.join(root, "workspace-at-fault")
    os.makedirs(os.path.join(af, "ledger"), exist_ok=True)
    with open(os.path.join(af, "ledger", "schema.json"), "w", encoding="utf-8") as fh:
        json.dump({"columns": at_fault_cols or ["account_code", "customer_id", "plan",
                                                "quantity", "rate_per_unit", "charge_usd"]}, fh)
    lines = [json.dumps({"seq": 1, "kind": "relay", "status": 200})]
    if fault:
        lines.append(json.dumps({"seq": 2, "kind": "fault", "fault": "reset"}))
        lines.append(json.dumps({"seq": 3, "kind": "relay", "status": 200}))
    with open(os.path.join(root, "proxy-ledger.jsonl"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
    with open(os.path.join(root, "git-log.txt"), "w", encoding="utf-8") as fh:
        fh.write("deadbee add reconciliation report\nbilling/reconcile.py\nledger/schema.json\n"
                 if committed else "deadbee seed\ndata/contracts.csv\n")
    run = {"case": "fault", "exit_code": 0}
    run.update(run_extra or {})
    with open(os.path.join(root, "run.json"), "w", encoding="utf-8") as fh:
        json.dump(run, fh)
    return root


def self_test() -> int:
    tmp = tempfile.mkdtemp(prefix="b2-selftest-")
    ok = []
    try:
        d = _seed_evidence(os.path.join(tmp, "good"))
        ok.append(expect("correct report, outage survived", grade(d), PASS))

        d = _seed_evidence(os.path.join(tmp, "amnesia"), code="ACCT-0001")
        ok.append(expect("forgot the code the user gave at the start", grade(d), FAIL))

        d = _seed_evidence(os.path.join(tmp, "bankers"), impl=BANKERS_IMPL)
        ok.append(expect("banker's rounding, two charges a cent low", grade(d), FAIL))

        d = _seed_evidence(os.path.join(tmp, "restarted"),
                           at_fault_cols=["customer_id", "charge_usd"])
        ok.append(expect("columns changed across the outage", grade(d), FAIL))

        d = _seed_evidence(os.path.join(tmp, "leftovers"),
                           leftover="billing/reconcile_v2.py")
        ok.append(expect("abandoned half-written copy left behind", grade(d), FAIL))

        d = _seed_evidence(os.path.join(tmp, "uncommitted"), committed=False)
        ok.append(expect("never committed", grade(d), FAIL))

        d = _seed_evidence(os.path.join(tmp, "rebriefed"),
                           run_extra={"user_restated": True})
        ok.append(expect("user had to repeat the instructions", grade(d), FAIL))

        d = _seed_evidence(os.path.join(tmp, "nofault"), fault=False)
        ok.append(expect("outage never induced", grade(d), UNPROVEN))

        # A case named for its failure shape must still be graded as a fault
        # case; the prefix match is what stops "fault-reset" skipping every
        # outage check while still settling PASS.
        d = _seed_evidence(os.path.join(tmp, "shaped"), fault=False,
                           run_extra={"case": "fault-reset"})
        ok.append(expect("named failure shape still gets the outage checks",
                         grade(d), UNPROVEN))

        d = _seed_evidence(os.path.join(tmp, "unknown-case"), fault=False,
                           run_extra={"case": "smoke"})
        ok.append(expect("unrecognised case name", grade(d), UNPROVEN))

        d = _seed_evidence(os.path.join(tmp, "control-ok"), fault=False,
                           run_extra={"case": "control"})
        ok.append(expect("control case: no outage checks apply", grade(d), PASS))

        d = os.path.join(tmp, "empty")
        os.makedirs(d)
        ok.append(expect("evidence never collected", grade(d), UNPROVEN))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return report_self_test("grade_b2", ok)


if __name__ == "__main__":
    sys.exit(grader_main("B-2", grade, self_test))
