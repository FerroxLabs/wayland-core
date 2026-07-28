#!/usr/bin/env python3
"""Extract the set of FAILING tests from a cargo-nextest log.

Why this exists (LANE-BRIEF §6b-ii): the obvious matcher for a nextest failure is
`grep 'FAIL ['`. That matcher UNDER-COUNTS, and it did so on the very run this lane
was dispatched to triage. nextest emits a compound status token when a test both
fails and leaks a process: `FL+LK`. `grep 'FAIL ['` misses it silently and returns
rc=0, so the extraction looks successful while dropping tests.

Measured on the ci-linux job 90424728480 of run 30403867920:
    grep 'FAIL ['  ->  66 unique   (summary said 68 failed)
    this extractor ->  68 unique   (matches the summary exactly)

The two it dropped were:
    wcore-exec-backend orphan::tests::the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind
    wcore-exec-backend::fail_closed_matrix the_local_scan_finds_an_orphan_that_no_registry_remembers

Usage:
    extract-nextest-failures.py <logfile> [--expect N]
    extract-nextest-failures.py --selftest
"""

import re
import sys

# A nextest per-test result line, after the CI timestamp prefix is stripped:
#   [TRY n ]STATUS [   1.234s] ( 1234/12820) <binary> <testname>
# STATUS is an uppercase token that may be compound (FL+LK, LEAK-FAIL, PASS+LK).
LINE = re.compile(
    r"""(?:^|\s)(?:TRY\s+\d+\s+)?
        (?P<status>[A-Z][A-Z0-9]*(?:[+-][A-Z0-9]+)*)
        \s+\[\s*[\d.]+s\]\s*
        \(\s*\d+\s*/\s*\d+\s*\)\s+
        (?P<test>\S.*?)\s*$""",
    re.VERBOSE,
)

# Statuses that are NOT failures. Everything else counts as a failure, so a status
# token this program has never seen fails LOUD rather than being silently dropped —
# which is precisely how `grep 'FAIL ['` lost FL+LK.
NON_FAILURE = {"PASS", "SKIP", "SLOW", "LEAK", "PASS+LK", "START", "TRY"}


def extract(text):
    """Return (failures:set[str], statuses:dict[str,int])."""
    failures = set()
    statuses = {}
    for raw in text.splitlines():
        m = LINE.search(raw)
        if not m:
            continue
        status = m.group("status")
        statuses[status] = statuses.get(status, 0) + 1
        if status not in NON_FAILURE:
            failures.add(m.group("test"))
    return failures, statuses


# ---------------------------------------------------------------------------
# Self-test. Three assertions, per LANE-BRIEF §6b-ii:
#   A1 known-positive passes
#   A2 known-negative fails
#   A3 the OLD broken matcher (grep 'FAIL [') would have MISSED it
# A3 is the only one that proves the repair does anything: A1 and A2 both pass
# against the broken matcher too.
# ---------------------------------------------------------------------------
SAMPLE = """\
2026-07-28T23:10:00.0000000Z         PASS [   0.011s] ( 8091/12820) wcore-types happy::tests::a_passing_test
2026-07-28T23:10:00.0000001Z   TRY 3 FAIL [   0.592s] ( 3156/12820) wcore-agent::typed_execution_policy_e2e_test typed_bypass_executes_bash_inside_required_sandbox
2026-07-28T23:10:00.0000002Z   TRY 3 FL+LK [   0.611s] ( 8092/12820) wcore-exec-backend orphan::tests::the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind
2026-07-28T23:10:00.0000003Z         LEAK [   0.400s] ( 8093/12820) wcore-tools leaky::tests::a_leaky_but_passing_test
2026-07-28T23:10:00.0000004Z         SKIP [   0.000s] ( 8094/12820) wcore-mcp skipped::tests::an_ignored_test
"""

OLD_MATCHER = "FAIL ["  # the substring the naive grep looked for


def selftest():
    failures, statuses = extract(SAMPLE)
    results = []

    # A1 known-positive: a plain FAIL and the compound FL+LK are both collected,
    # and nothing that passed/leaked/skipped is.
    a1 = failures == {
        "wcore-agent::typed_execution_policy_e2e_test typed_bypass_executes_bash_inside_required_sandbox",
        "wcore-exec-backend orphan::tests::the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind",
    }
    results.append(("A1 known-positive: both FAIL and FL+LK extracted, PASS/LEAK/SKIP excluded", a1))

    # A2 known-negative: a log with no failing statuses yields an empty set. If the
    # matcher were "anything uppercase", this would wrongly report failures.
    clean = "\n".join(l for l in SAMPLE.splitlines() if " FAIL " not in l and "FL+LK" not in l)
    neg_failures, _ = extract(clean)
    a2 = neg_failures == set()
    results.append(("A2 known-negative: an all-PASS/LEAK/SKIP log yields zero failures", a2))

    # A3 THE LOAD-BEARING ONE: prove the old matcher misses the FL+LK line.
    # If nextest is ever changed so FL+LK renders as "FAIL [", this assertion goes
    # red and this whole workaround should be retired rather than carried forever.
    old_hits = {
        l for l in SAMPLE.splitlines()
        if OLD_MATCHER in l
    }
    fl_lk_line = [l for l in SAMPLE.splitlines() if "FL+LK" in l][0]
    a3 = (fl_lk_line not in old_hits) and (len(failures) == len(old_hits) + 1)
    results.append(
        ("A3 the OLD matcher grep 'FAIL [' MISSES the FL+LK failure "
         f"(old={len(old_hits)}, new={len(failures)})", a3)
    )

    passed = sum(1 for _, ok in results if ok)
    for name, ok in results:
        print(f"[{'ok' if ok else 'FAIL'}] {name}")
    print(f"{passed} passed, {len(results) - passed} failed")
    return 0 if passed == len(results) else 1


def main(argv):
    if "--selftest" in argv:
        return selftest()
    if len(argv) < 2:
        print(__doc__)
        return 2
    with open(argv[1], "r", errors="replace") as fh:
        text = fh.read()
    failures, statuses = extract(text)
    expect = None
    if "--expect" in argv:
        expect = int(argv[argv.index("--expect") + 1])
    for t in sorted(failures):
        print(t)
    print(f"# statuses seen: {statuses}", file=sys.stderr)
    print(f"# unique failing tests: {len(failures)}", file=sys.stderr)
    if expect is not None and len(failures) != expect:
        print(f"# MISMATCH: expected {expect}, extracted {len(failures)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
