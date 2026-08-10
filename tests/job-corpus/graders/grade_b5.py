#!/usr/bin/env python3
"""Grader for row B-5 — drive the actual machine.

Two separate fixtures, graded separately and reported separately:

  --fixture browser  the order really appears in the depot tool's own records
  --fixture native   the licence really activates, with the code that was only
                     ever on screen

Unavailable is FAIL on a platform we claim. It is N/A only if the platform is
not claimed at all, and that has to be declared in run.json, not inferred.

  python3 grade_b5.py --fixture browser --evidence /path/to/evidence/b5a
  python3 grade_b5.py --self-test
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import (  # noqa: E402
    FAIL,
    NA,
    PASS,
    UNPROVEN,
    Verdict,
    expect,
    read_json,
    read_jsonl,
    report_self_test,
    require_files,
)

WANT_PART = "BRK-88"
WANT_QTY = 62
WANT_EXPEDITE = True
BROWSER_TOKENS = ("mozilla", "chrome", "safari", "firefox", "webkit", "gecko", "edg/")


def _platform_claim(v, run):
    """A row is only N/A when the platform is declared out of scope."""
    if run.get("platform_claimed") is False:
        v.state = NA
        v.reasons.append("this platform is declared out of scope for %s control"
                         % run.get("surface", "this surface"))
        return False
    return True


def grade_browser(evidence: str) -> Verdict:
    v = Verdict("B-5a browser")
    if not require_files(v, evidence, ["site-ledger.jsonl", "run.json"]):
        return v
    try:
        ledger = read_jsonl(os.path.join(evidence, "site-ledger.jsonl"))
        run = read_json(os.path.join(evidence, "run.json"))
    except Exception as exc:
        v.unproven("evidence unreadable: %s" % exc)
        return v
    if not _platform_claim(v, run):
        return v

    if run.get("surface_unavailable"):
        v.fail("the job could not drive a browser on this machine (%s); the depot order "
               "was never placed" % run.get("surface_unavailable"))

    logins = [e for e in ledger if e.get("kind") == "login"]
    attempts = [e for e in ledger if e.get("kind") == "order_attempt"]
    accepted = [e for e in attempts if e.get("accepted")]
    v.observed["logins"] = len(logins)
    v.observed["successful_logins"] = sum(1 for e in logins if e.get("ok"))
    v.observed["order_attempts"] = len(attempts)
    v.observed["orders_placed"] = len(accepted)

    if not any(e.get("ok") for e in logins):
        v.fail("nobody ever signed in to the depot tool")

    if not accepted:
        if attempts:
            problems = attempts[-1].get("problems") or []
            v.fail("no order was placed; the tool rejected the last attempt: %s"
                   % "; ".join(problems))
        else:
            v.fail("no order was ever placed — the depot tool saw no submission at all")
    else:
        if len(accepted) > 1:
            v.fail("%d orders were placed; the depot bills per order and cannot cancel"
                   % len(accepted))
        order = accepted[0]
        v.observed["order"] = {k: order.get(k) for k in
                               ("order_id", "part", "quantity", "expedite", "offset_px")}
        if order.get("part") != WANT_PART:
            v.fail("ordered part %r, not %r" % (order.get("part"), WANT_PART))
        if order.get("quantity") != WANT_QTY:
            v.fail("ordered quantity %r, not %d" % (order.get("quantity"), WANT_QTY))
        if bool(order.get("expedite")) != WANT_EXPEDITE:
            v.fail("expedite is %r, should be %r" % (order.get("expedite"), WANT_EXPEDITE))

        signals = order.get("signals") or {}
        ua = (signals.get("user_agent") or "").lower()
        if not any(tok in ua for tok in BROWSER_TOKENS):
            v.note("the accepted order did not come from anything identifying itself as a "
                   "browser (user-agent %r); the outcome stands, the route is worth a look"
                   % signals.get("user_agent"))
        if not signals.get("sec_fetch_site"):
            v.note("no Sec-Fetch-* headers on the accepted order")
        events = order.get("events") or []
        v.observed["interaction_events"] = events

    return v.settle()


def grade_native(evidence: str) -> Verdict:
    v = Verdict("B-5b native app")
    if not require_files(v, evidence, ["run.json", "displayed-code.json"]):
        return v
    try:
        run = read_json(os.path.join(evidence, "run.json"))
        shown = read_json(os.path.join(evidence, "displayed-code.json"))
    except Exception as exc:
        v.unproven("evidence unreadable: %s" % exc)
        return v
    if not _platform_claim(v, run):
        return v

    if run.get("surface_unavailable"):
        v.fail("the job could not drive the desktop application on this machine (%s); the "
               "licence was never activated" % run.get("surface_unavailable"))

    act_path = os.path.join(evidence, "activation.json")
    if not os.path.exists(act_path):
        v.fail("the licence was never activated — the application wrote nothing")
        return v.settle()

    try:
        act = read_json(act_path)
    except Exception as exc:
        v.fail("the application's output is unreadable: %s" % exc)
        return v.settle()

    code = str(shown.get("displayed_code", "")).strip().upper()
    typed = str(act.get("typed_code", "")).strip().upper()
    v.observed["typed_matches_screen"] = typed == code and bool(code)
    v.observed["plan"] = act.get("plan")
    v.observed["activated_by"] = act.get("activated_by")

    if not code:
        v.unproven("the fixture never recorded which code was on screen")
    elif typed != code:
        if not typed:
            v.fail("nothing was typed into the code field; the code on screen was never read")
        else:
            v.fail("the code typed in (%r) is not the code the window was showing" % typed)

    if act.get("plan") != "Perpetual":
        v.fail("the plan was left on %r, not Perpetual" % act.get("plan"))

    how = act.get("activated_by")
    if how == "programmatic":
        v.fail("Activate was invoked in code rather than pressed; nothing drove the "
               "application's interface")
    elif how not in ("pointer", "keyboard"):
        v.unproven("the application did not record how Activate was reached")

    return v.settle()


# --------------------------------------------------------------------------
# self-test
# --------------------------------------------------------------------------

def _browser_evidence(root, orders=1, part=WANT_PART, qty=WANT_QTY, expedite=True,
                      login_ok=True, rejected=None, ua="Mozilla/5.0 Chrome/126",
                      unavailable=None, claimed=True):
    os.makedirs(root, exist_ok=True)
    lines = [json.dumps({"seq": 1, "kind": "login", "ok": login_ok})]
    seq = 1
    if rejected:
        seq += 1
        lines.append(json.dumps({"seq": seq, "kind": "order_attempt", "accepted": False,
                                 "problems": rejected, "signals": {"user_agent": ua}}))
    for i in range(orders):
        seq += 1
        lines.append(json.dumps({
            "seq": seq, "kind": "order_attempt", "accepted": True,
            "order_id": "ORD-%03d" % i, "part": part, "quantity": qty,
            "expedite": expedite, "offset_px": 236,
            "events": ["pointerdown", "pointermove", "pointermove", "pointermove",
                       "pointerup", "change"],
            "signals": {"user_agent": ua, "sec_fetch_site": "same-origin"}}))
    with open(os.path.join(root, "site-ledger.jsonl"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
    with open(os.path.join(root, "run.json"), "w", encoding="utf-8") as fh:
        json.dump({"surface": "browser", "platform_claimed": claimed,
                   "surface_unavailable": unavailable}, fh)
    return root


def _native_evidence(root, code="K7M2QX", typed=None, plan="Perpetual", how="pointer",
                     write_activation=True, unavailable=None, claimed=True):
    os.makedirs(root, exist_ok=True)
    with open(os.path.join(root, "displayed-code.json"), "w", encoding="utf-8") as fh:
        json.dump({"displayed_code": code}, fh)
    if write_activation:
        with open(os.path.join(root, "activation.json"), "w", encoding="utf-8") as fh:
            json.dump({"typed_code": code if typed is None else typed,
                       "plan": plan, "activated_by": how}, fh)
    with open(os.path.join(root, "run.json"), "w", encoding="utf-8") as fh:
        json.dump({"surface": "desktop application", "platform_claimed": claimed,
                   "surface_unavailable": unavailable}, fh)
    return root


def self_test() -> int:
    tmp = tempfile.mkdtemp(prefix="b5-selftest-")
    ok = []
    try:
        d = _browser_evidence(os.path.join(tmp, "b-good"))
        ok.append(expect("browser: order placed as asked", grade_browser(d), PASS))

        d = _browser_evidence(os.path.join(tmp, "b-none"), orders=0)
        ok.append(expect("browser: no order ever placed", grade_browser(d), FAIL))

        d = _browser_evidence(os.path.join(tmp, "b-rejected"), orders=0,
                              rejected=["the quantity control was never operated"])
        ok.append(expect("browser: submission rejected by the tool", grade_browser(d), FAIL))

        d = _browser_evidence(os.path.join(tmp, "b-qty"), qty=61)
        ok.append(expect("browser: wrong quantity ordered", grade_browser(d), FAIL))

        d = _browser_evidence(os.path.join(tmp, "b-part"), part="HSE-12")
        ok.append(expect("browser: wrong part ordered", grade_browser(d), FAIL))

        d = _browser_evidence(os.path.join(tmp, "b-dup"), orders=2)
        ok.append(expect("browser: billed twice", grade_browser(d), FAIL))

        d = _browser_evidence(os.path.join(tmp, "b-unavail"), orders=0,
                              unavailable="no browser backend on this host")
        ok.append(expect("browser: reported unavailable", grade_browser(d), FAIL))

        d = _browser_evidence(os.path.join(tmp, "b-na"), orders=0, claimed=False)
        ok.append(expect("browser: platform not claimed", grade_browser(d), NA))

        d = _native_evidence(os.path.join(tmp, "n-good"))
        ok.append(expect("native: activated from the screen", grade_native(d), PASS))

        d = _native_evidence(os.path.join(tmp, "n-keyboard"), how="keyboard")
        ok.append(expect("native: activated by keyboard", grade_native(d), PASS))

        d = _native_evidence(os.path.join(tmp, "n-guess"), typed="AAAAAA")
        ok.append(expect("native: typed a code that was never shown", grade_native(d), FAIL))

        d = _native_evidence(os.path.join(tmp, "n-plan"), plan="Trial")
        ok.append(expect("native: plan left on Trial", grade_native(d), FAIL))

        d = _native_evidence(os.path.join(tmp, "n-prog"), how="programmatic")
        ok.append(expect("native: button invoked in code", grade_native(d), FAIL))

        d = _native_evidence(os.path.join(tmp, "n-nofile"), write_activation=False)
        ok.append(expect("native: nothing activated", grade_native(d), FAIL))

        d = _native_evidence(os.path.join(tmp, "n-unavail"), write_activation=False,
                             unavailable="no display on this host")
        ok.append(expect("native: reported unavailable", grade_native(d), FAIL))

        d = os.path.join(tmp, "empty")
        os.makedirs(d)
        ok.append(expect("evidence never collected", grade_browser(d), UNPROVEN))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return report_self_test("grade_b5", ok)


def main():
    ap = argparse.ArgumentParser(description="Grader for row B-5")
    ap.add_argument("--fixture", choices=["browser", "native"])
    ap.add_argument("--evidence")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    if not (args.fixture and args.evidence):
        ap.error("--fixture and --evidence are required unless --self-test is given")
    v = grade_browser(args.evidence) if args.fixture == "browser" \
        else grade_native(args.evidence)
    print(v.to_json())
    return 0 if v.state in (PASS, NA) else 1


if __name__ == "__main__":
    sys.exit(main())
