#!/usr/bin/env python3
"""Classify a test's status on the Windows leg -- repaired instrument (LANE-BRIEF 6b-ii).

THE DEFECT THIS REPAIRS, found in my own reasoning during this lane.
I asked "is this test in win81.txt?" and read absence as "it PASSED on Windows".
That is wrong for any platform-gated test: a `#[cfg(target_os = "linux")]` test is
absent from the Windows failure list because it DOES NOT EXIST there, not because
it passed. I used that shape to argue the bwrap tests had native coverage
elsewhere, which would have overstated the case for skipping them.

A failure list alone cannot distinguish "ran and passed" from "never ran". The
repaired classifier consults the cfg gate as a second, independent oracle and
returns a third state -- NOT_PRESENT -- instead of silently folding it into PASSED.

Self-test: `python3 classify-windows-status.py --selftest`. Three assertions; the
third is the only one that proves the repair does anything, because A1 and A2 pass
against the OLD broken classifier too.
"""
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
WIN81 = REPO / ".planning/evidence/red-68/win81.txt"

FAILED = "FAILED_ON_WINDOWS"
PASSED = "PASSED_ON_WINDOWS"
ABSENT = "NOT_PRESENT_ON_WINDOWS"


def in_failure_list(name: str) -> bool:
    text = WIN81.read_text(encoding="utf-8", errors="replace")
    return name in text


def windows_gate(name: str):
    """Second oracle: does this test compile on Windows at all?

    Returns (compiles_on_windows: bool|None, gate: str). None = could not tell,
    which is reported as such rather than assumed either way.
    """
    hit = subprocess.run(
        ["/usr/bin/grep", "-rl", f"fn {name}", str(REPO / "crates")],
        capture_output=True, text=True,
    )
    files = [f for f in hit.stdout.split("\n") if f.strip()]
    if not files:
        return None, "SOURCE-NOT-FOUND"
    path = Path(files[0])
    src = path.read_text(encoding="utf-8", errors="replace")
    idx = src.find(f"fn {name}")
    if idx < 0:
        return None, "FN-NOT-LOCATED"
    before = src[:idx]
    gates = re.findall(r'cfg\(\s*(?:all\(\s*)?(?:test\s*,\s*)?'
                       r'target_os\s*=\s*"([a-z]+)"|cfg\(\s*unix\s*\)|cfg\(\s*windows\s*\)',
                       before)
    tail = re.findall(r'cfg\([^)]*\)', before)
    gate = tail[-1] if tail else "NONE"
    if 'target_os = "linux"' in gate or 'target_os="linux"' in gate:
        return False, gate
    if re.search(r'cfg\(\s*unix\s*\)', gate):
        return False, gate
    return True, gate


def classify(name: str) -> str:
    if in_failure_list(name):
        return FAILED
    compiles, _gate = windows_gate(name)
    if compiles is False:
        return ABSENT
    if compiles is None:
        return "UNKNOWN"
    return PASSED


def classify_old(name: str) -> str:
    """The shape I actually used before repairing it."""
    return FAILED if in_failure_list(name) else PASSED


def selftest() -> int:
    # Chosen from real repo data, verified by hand before being pinned here.
    known_failing = "typed_bypass_executes_bash_inside_required_sandbox"   # in win81
    cross_platform_pass = "dispatches_4_noop_workers_in_parallel"          # no cfg gate, absent
    linux_only = "required_live_bwrap_admission"                           # cfg(target_os="linux")

    results = []

    a1 = classify(known_failing) == FAILED
    results.append(("A1_known_positive_listed_test_reads_FAILED", a1))

    a2 = classify(cross_platform_pass) == PASSED
    results.append(("A2_known_negative_ungated_absent_test_reads_PASSED", a2))

    # A3, the discriminator: a linux-gated test must read NOT_PRESENT, AND the
    # old shape must have gotten it wrong. Both halves are required -- without
    # the second half this assertion passes on the broken classifier too.
    new_says = classify(linux_only)
    old_says = classify_old(linux_only)
    a3 = (new_says == ABSENT) and (old_says == PASSED)
    results.append(("A3_linux_gated_test_is_NOT_PRESENT_and_old_shape_said_PASSED", a3))

    npass = sum(1 for _, ok in results if ok)
    for label, ok in results:
        print(f"SELFTEST {label}: {'PASS' if ok else 'FAIL'}")
    print(f"SELFTEST A3 detail: repaired={new_says}  old={old_says}  "
          f"(the old shape would have counted a test that cannot compile on "
          f"Windows as evidence of Windows coverage)")
    print(f"SELFTEST SUMMARY: {npass} passed, {len(results) - npass} failed")
    return 0 if npass == len(results) else 1


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    for arg in sys.argv[1:]:
        print(f"{arg}\t{classify(arg)}")
