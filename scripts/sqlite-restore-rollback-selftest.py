#!/usr/bin/env python3
"""Self-test for `sqlite-restore-rollback-proof.py`'s row accounting.

LANE-BRIEF §6b-ii requires three assertions, not two, when an instrument
defect is repaired:

  A1  known-positive PASSES  — an intact result is reported clean;
  A2  known-negative FAILS   — a result that really lost rows is reported lost;
  A3  the OLD broken matcher would have MISSED it — without this, the self-test
      passes on the broken instrument too and proves the repair does nothing.

## The defect being guarded

`rows_committed` is `PRIMARY KEY (wid, n)` and each writer commits `n = 1..need`,
so `COUNT(*) WHERE n <= need` cannot exceed `need`. A CORRUPT database can
report that it does — measured on `hetzner-dsm`, base run `base-c3` returned
`have = need + 107` for writer `w2` off a database failing `integrity_check`
with 101 problem lines.

The original accounting summed SIGNED differences, so such a surplus silently
cancels a genuine loss from another writer.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent

spec = importlib.util.spec_from_file_location(
    "rollback_proof", HERE / "sqlite-restore-rollback-proof.py"
)
mod = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(mod)

account_rows = mod.account_rows
account_rows_SIGNED = mod.account_rows_SIGNED


def main() -> int:
    failures: list[str] = []

    # --- A1: known-positive -------------------------------------------------
    need = {"w0": 1000, "w1": 1000, "w2": 1000}
    intact = {"w0": 1000, "w1": 1000, "w2": 1000}
    missing, surplus, detail = account_rows(need, intact)
    print(f"A1 intact: missing={missing} surplus={surplus} detail={detail}")
    if missing != 0 or surplus != 0:
        failures.append(f"A1: an intact result was reported dirty ({missing}/{surplus})")

    # --- A2: known-negative -------------------------------------------------
    # 50 rows genuinely lost by w0.
    lost = {"w0": 950, "w1": 1000, "w2": 1000}
    missing, surplus, detail = account_rows(need, lost)
    print(f"A2 lost-50: missing={missing} surplus={surplus} detail={detail}")
    if missing != 50:
        failures.append(f"A2: 50 lost rows were reported as {missing}")

    # --- A3: the OLD matcher would have missed it ---------------------------
    # The cancelling case, taken from the real shape base-c3 produced: one
    # writer loses 107 rows, another reports 107 MORE than the schema permits.
    cancelling = {"w0": 893, "w1": 1107, "w2": 1000}
    old = account_rows_SIGNED(need, cancelling)
    missing, surplus, detail = account_rows(need, cancelling)
    print(
        f"A3 cancelling: OLD signed-sum={old} "
        f"| NEW missing={missing} surplus={surplus} detail={detail}"
    )
    if old != 0:
        failures.append(
            f"A3 is not exercising the defect: the old matcher reported {old}, "
            "so this case would not have slipped through"
        )
    if missing != 107:
        failures.append(f"A3: the repaired matcher reported missing={missing}, expected 107")
    if surplus != 107:
        failures.append(f"A3: the repaired matcher reported surplus={surplus}, expected 107")

    # A3 restated as the verdict each version would have PRODUCED, which is the
    # thing that actually mattered: with `integrity_check` reading "ok", the old
    # verdict is a clean PASS on a database that lost 107 committed rows.
    old_verdict = "PASS" if old == 0 else "FAIL"
    new_verdict = "PASS" if (missing == 0 and surplus == 0) else "FAIL"
    print(f"A3 verdicts on the same data: OLD={old_verdict} NEW={new_verdict}")
    if not (old_verdict == "PASS" and new_verdict == "FAIL"):
        failures.append(
            f"A3: expected OLD=PASS NEW=FAIL, got OLD={old_verdict} NEW={new_verdict}"
        )

    print("---- SELFTEST ----")
    for f in failures:
        print(f"FAILED: {f}")
    print(f"ASSERTIONS: 3  FAILURES: {len(failures)}")
    print(f"VERDICT: {'PASS' if not failures else 'FAIL'}")
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
