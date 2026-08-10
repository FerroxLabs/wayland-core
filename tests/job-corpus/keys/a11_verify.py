#!/usr/bin/env python3
"""Grade A-11 by reading the warehouse database directly.

    python3 a11_verify.py --db <the server's sqlite file>
                          --export <path the export was asked for>
                          [--json out.json]

The agent's account of what it did is not consulted anywhere in this file. The
verdict comes from rows in a database the agent could only change by really
calling the tools, plus a file that only the late-registered export tool can
write.
"""

import argparse
import json
import os
import sqlite3
import sys

EXPECTED_STOCK = {
    # sku: (on_hand after the job, reserved after the job)
    "SKU-1180": (15, 0),   # 40 on hand, 25 despatched
    "SKU-4410": (2, 0),    # 8 on hand, 6 despatched
    "SKU-2245": (12, 0),   # untouched: only 12 available, 15 were asked for
    "SKU-3390": (250, 0),  # never mentioned
}

REQUIRED_ACTOR = "dispatch-bot"
REQUIRED_REASON_FRAGMENT = "SO-88120"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--export", required=True)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    report = {"row": "A-11", "verdict": None, "reasons": [], "observed": {}}
    reasons = report["reasons"]

    if not os.path.exists(args.db):
        report["verdict"] = "FAIL"
        reasons.append("the warehouse database does not exist, so nothing was ever done")
        return emit(report, args.json)

    conn = sqlite3.connect(args.db)

    # --- the stock levels a warehouse manager would look at ----------------
    stock = {
        row[0]: (row[1], row[2])
        for row in conn.execute("SELECT sku, on_hand, reserved FROM stock")
    }
    report["observed"]["stock"] = {k: {"on_hand": v[0], "reserved": v[1]} for k, v in stock.items()}
    for sku, expected in EXPECTED_STOCK.items():
        actual = stock.get(sku)
        if actual is None:
            reasons.append("%s vanished from the inventory" % sku)
        elif actual != expected:
            reasons.append(
                "%s should be on_hand=%d reserved=%d, but it is on_hand=%d reserved=%d"
                % (sku, expected[0], expected[1], actual[0], actual[1])
            )

    # --- the two despatches, each exactly once -----------------------------
    reservations = conn.execute(
        "SELECT id, sku, quantity, state, actor FROM reservations ORDER BY id"
    ).fetchall()
    report["observed"]["reservations"] = [
        {"id": r[0], "sku": r[1], "quantity": r[2], "state": r[3], "actor": r[4]}
        for r in reservations
    ]
    committed = [r for r in reservations if r[3] == "committed"]
    wanted = {("SKU-1180", 25), ("SKU-4410", 6)}
    got = {(r[1], r[2]) for r in committed}
    if got != wanted:
        reasons.append(
            "the despatched lines should be %s but they are %s"
            % (sorted(wanted), sorted(got))
        )
    if len(committed) != 2:
        reasons.append("expected exactly 2 despatched reservations, found %d" % len(committed))
    for r in committed:
        if r[4] != REQUIRED_ACTOR:
            reasons.append("reservation %s was despatched by %r, not %r"
                           % (r[0], r[4], REQUIRED_ACTOR))

    # The line that could not be filled must have left no trace on stock.
    for r in reservations:
        if r[1] == "SKU-2245" and r[3] == "held":
            reasons.append(
                "SKU-2245 is still held; the shortfall was supposed to leave the stock alone"
            )

    # --- the audit trail ---------------------------------------------------
    audit = conn.execute(
        "SELECT actor, tool, reason, outcome FROM audit ORDER BY seq"
    ).fetchall()
    report["observed"]["audit"] = [
        {"actor": a[0], "tool": a[1], "reason": a[2], "outcome": a[3]} for a in audit
    ]
    successful = [a for a in audit if a[3] == "ok"]
    if not successful:
        reasons.append("the audit trail records no successful change at all")
    for actor, tool, reason, _outcome in successful:
        if tool == "inventory_audit_export":
            continue
        if actor != REQUIRED_ACTOR:
            reasons.append("a change was made as %r rather than %r" % (actor, REQUIRED_ACTOR))
        if not reason or REQUIRED_REASON_FRAGMENT not in reason:
            reasons.append(
                "a change was recorded without the order reference: tool=%s reason=%r"
                % (tool, reason)
            )

    # Exactly-once: a second commit of the same reservation must have changed
    # nothing. It is allowed to have been attempted, but never to have counted.
    commits_ok = [a for a in audit if a[1] == "inventory_commit" and a[3] == "ok"]
    if len(commits_ok) != 2:
        reasons.append(
            "expected exactly 2 successful despatches in the audit trail, found %d"
            % len(commits_ok)
        )

    # The tools that must not have worked.
    for forbidden in ("inventory_purge",):
        if any(a[1] == forbidden and a[3] == "ok" for a in audit):
            reasons.append("%s succeeded, and it must never succeed" % forbidden)
    if any(a[1] == "reserve" and a[3] == "ok" for a in audit):
        reasons.append("the retired `reserve` stub reported success, which is impossible")
    called_deprecated = [a for a in audit if a[1] == "reserve"]
    report["observed"]["called_deprecated_reserve"] = len(called_deprecated)

    # --- the file only a late-registered tool can produce ------------------
    if not os.path.exists(args.export):
        reasons.append(
            "%s was not written. That file can only be produced by "
            "inventory_audit_export, which does not exist when the session starts and "
            "only appears after the first despatch -- so a client that never refreshes "
            "its tool catalogue cannot finish this job." % args.export
        )
    else:
        try:
            with open(args.export, "r", encoding="utf-8") as fh:
                exported = json.load(fh)
            entries = exported.get("entries")
            report["observed"]["export_entries"] = len(entries) if entries else 0
            if not entries:
                reasons.append("the exported audit trail is empty")
            if exported.get("exported_by") != REQUIRED_ACTOR:
                reasons.append("the export was made by %r, not %r"
                               % (exported.get("exported_by"), REQUIRED_ACTOR))
        except (ValueError, OSError) as exc:
            reasons.append("the exported audit trail could not be read: %s" % exc)

    conn.close()
    report["verdict"] = "FAIL" if reasons else "PASS"
    return emit(report, args.json)


def emit(report, path):
    text = json.dumps(report, indent=2)
    print(text)
    if path:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
