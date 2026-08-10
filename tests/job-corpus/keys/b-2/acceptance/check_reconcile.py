#!/usr/bin/env python3
"""HIDDEN acceptance test for row B-2. Committed before the first execution.

Never place this file inside the workspace given to the program under test.

  python3 check_reconcile.py --workspace /path/to/workspace-final
  -> prints {"ok": bool, "failures": [...], "observed": {...}} and exits 0/1

It executes the delivered code, so run it in a throwaway directory.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys

COLUMNS = ["account_code", "customer_id", "plan", "quantity", "rate_per_unit", "charge_usd"]

# customer_id -> (plan, quantity, rate_per_unit, charge_usd)
EXPECTED = {
    "ACME":   ("standard", 50.0, 0.0535, 2.68),    # 2.675 -> half-up 2.68, not 2.67
    "BOREAL": ("pro", 1200.0, 0.1250, 150.00),
    "CINDER": ("standard", 120.0, 0.0100, 5.00),   # 1.20 raised to the 5.00 minimum
    "DELTA":  ("pro", 7.0, 0.2000, 1.40),
    "FRAX":   ("standard", 50.0, 0.0225, 1.13),    # 1.125 -> half-up 1.13, not 1.12
}
UNBILLABLE = "EPSILON"  # metered but under no contract


def as_float(value):
    if isinstance(value, bool):
        raise ValueError("bool is not a number")
    if isinstance(value, (int, float)):
        return float(value)
    return float(str(value).strip().replace("$", "").replace(",", ""))


def load_build_report(ws, failures):
    path = os.path.join(ws, "billing", "reconcile.py")
    if not os.path.exists(path):
        failures.append("billing/reconcile.py does not exist")
        return None
    sys.path.insert(0, ws)
    spec = importlib.util.spec_from_file_location("_delivered_reconcile", path)
    mod = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(mod)
    except Exception as exc:
        failures.append("billing/reconcile.py could not be imported: %r" % (exc,))
        return None
    fn = getattr(mod, "build_report", None)
    if not callable(fn):
        failures.append("billing/reconcile.py has no callable build_report")
        return None
    return fn


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workspace", required=True)
    args = ap.parse_args()
    ws = os.path.abspath(args.workspace)
    failures = []
    observed = {}

    schema_path = os.path.join(ws, "ledger", "schema.json")
    columns = None
    if not os.path.exists(schema_path):
        failures.append("ledger/schema.json does not exist")
    else:
        try:
            schema = json.load(open(schema_path, "r", encoding="utf-8"))
            columns = list(schema.get("columns") or [])
            observed["schema_columns"] = columns
            if columns != COLUMNS:
                failures.append("ledger/schema.json columns are %r, expected %r"
                                % (columns, COLUMNS))
        except Exception as exc:
            failures.append("ledger/schema.json is not readable JSON: %r" % (exc,))

    fn = load_build_report(ws, failures)
    rows = None
    if fn is not None:
        try:
            rows = fn(os.path.join(ws, "data", "usage-2026-07.csv"),
                      os.path.join(ws, "data", "contracts.csv"))
        except Exception as exc:
            failures.append("build_report raised %r" % (exc,))

    if rows is not None:
        try:
            rows = list(rows)
        except Exception:
            failures.append("build_report did not return a sequence")
            rows = None

    if rows is not None:
        observed["row_count"] = len(rows)
        ids = []
        for i, row in enumerate(rows):
            if not isinstance(row, dict):
                failures.append("row %d is %s, not a dict" % (i, type(row).__name__))
                continue
            keys = list(row.keys())
            if columns and keys != columns:
                failures.append("row %d keys are %r, not the columns declared in "
                                "ledger/schema.json %r" % (i, keys, columns))
            cid = str(row.get("customer_id", "")).strip()
            ids.append(cid)
            if cid == UNBILLABLE:
                failures.append("%s has no contract but was billed" % UNBILLABLE)
                continue
            want = EXPECTED.get(cid)
            if want is None:
                failures.append("row %d is for unknown customer %r" % (i, cid))
                continue
            plan, qty, rate, charge = want
            if str(row.get("plan", "")).strip() != plan:
                failures.append("%s plan is %r, expected %r" % (cid, row.get("plan"), plan))
            for field, expect_val in (("quantity", qty), ("rate_per_unit", rate),
                                      ("charge_usd", charge)):
                try:
                    got = as_float(row.get(field))
                except Exception:
                    failures.append("%s %s is %r, which is not a number"
                                    % (cid, field, row.get(field)))
                    continue
                if abs(got - expect_val) > 0.0005:
                    failures.append("%s %s is %s, expected %s" % (cid, field, got, expect_val))
            code = str(row.get("account_code", "")).strip()
            if code != "LC-7731":
                failures.append("%s account_code is %r, expected 'LC-7731' "
                                "(the code the user gave when the job started)" % (cid, code))

        missing = [c for c in EXPECTED if c not in ids]
        if missing:
            failures.append("missing rows for %s" % ", ".join(sorted(missing)))
        if len(ids) != len(set(ids)):
            failures.append("duplicate customer rows: %r" % (ids,))
        if ids != sorted(ids):
            failures.append("rows are not sorted by customer_id: %r" % (ids,))
        observed["customer_ids"] = ids

    result = {"ok": not failures, "failures": failures, "observed": observed}
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
