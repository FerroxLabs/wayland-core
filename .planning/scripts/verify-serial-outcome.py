#!/usr/bin/env python3
"""For each test that failed in CI, assert what it did in the serial re-run.

Why this exists: "the crate now passes 100/100" is NOT evidence that the test
which failed in CI passed — it is compatible with that test never having been
executed at all. This repo has measured four distinct flavours of a suite exiting
0 having run zero of the tests you meant. The only honest check is per-test:
find the specific name and read back its status.

Outcome per CI failure:
  PASS_SERIAL  the exact test ran and passed in the serial re-run
  FAIL_SERIAL  the exact test ran and failed again -> NOT an environment artefact
  SKIPPED      the test was skipped (qualified out) -> proves nothing either way
  ABSENT       the test name does not appear at all -> the re-run did not cover it

Usage: verify-serial-outcome.py <ci-failure-list> <serial-log> [<serial-log>...]
       verify-serial-outcome.py --selftest
"""

import re
import sys

LINE = re.compile(
    r"""(?:^|\s)(?:TRY\s+\d+\s+)?
        (?P<status>[A-Z][A-Z0-9]*(?:[+-][A-Z0-9]+)*)
        \s+\[\s*[\d.]+s\]\s*
        (?:\(\s*(?:\d+|─+)\s*/?\s*(?:\d+)?\s*\)\s+)?
        (?P<test>\S.*?)\s*$""",
    re.VERBOSE,
)
NON_FAILURE = {"PASS", "LEAK", "PASS+LK"}


def index(paths):
    """test name -> set of statuses observed."""
    seen = {}
    for p in paths:
        with open(p, errors="replace") as fh:
            for raw in fh:
                m = LINE.search(raw.rstrip("\n"))
                if not m:
                    continue
                seen.setdefault(m.group("test"), set()).add(m.group("status"))
    return seen


def skipped_names(paths):
    """nextest prints skips as `SKIP [ 0.000s] <binary> <test>` or in a list."""
    out = set()
    for p in paths:
        with open(p, errors="replace") as fh:
            for raw in fh:
                if " SKIP " in raw or raw.strip().startswith("SKIP"):
                    m = LINE.search(raw.rstrip("\n"))
                    if m:
                        out.add(m.group("test"))
    return out


def classify(name, seen, skips):
    if name in skips:
        return "SKIPPED"
    if name not in seen:
        return "ABSENT"
    statuses = seen[name]
    if any(s not in NON_FAILURE and s != "SKIP" for s in statuses):
        return "FAIL_SERIAL"
    return "PASS_SERIAL"


# ---------------------------------------------------------------------------
# Self-test: three assertions (LANE-BRIEF 6b-ii). The third is load-bearing --
# the OLD way of judging this (read the crate's total "N passed / 0 failed")
# would have reported the ABSENT test as fine, because a crate can pass 100/100
# while never running the test you care about.
# ---------------------------------------------------------------------------
SERIAL_SAMPLE = """\
        PASS [   0.005s] ( 1/3) wcore-sandbox backends::bwrap::tests::required_live_bwrap_admission
   TRY 2 FAIL [   0.119s] ( 2/3) wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte
        SKIP [   0.000s] ( 3/3) wcore-sandbox backends::macos::tests::only_on_macos
     Summary [   1.000s] 3 tests run: 2 passed, 1 failed, 0 skipped
"""
CI_SAMPLE = [
    "wcore-sandbox backends::bwrap::tests::required_live_bwrap_admission",
    "wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte",
    "wcore-sandbox backends::macos::tests::only_on_macos",
    "wcore-swarm::dispatch_smoke a_test_this_rerun_never_touched",
]


def selftest():
    import tempfile, os
    fd, path = tempfile.mkstemp(suffix=".log")
    with os.fdopen(fd, "w") as fh:
        fh.write(SERIAL_SAMPLE)
    seen = index([path])
    skips = skipped_names([path])
    got = {n: classify(n, seen, skips) for n in CI_SAMPLE}
    os.unlink(path)

    results = []

    a1 = got[CI_SAMPLE[0]] == "PASS_SERIAL"
    results.append(("A1 known-positive: a test that ran and passed is PASS_SERIAL", a1))

    a2 = got[CI_SAMPLE[1]] == "FAIL_SERIAL" and got[CI_SAMPLE[2]] == "SKIPPED"
    results.append(("A2 known-negative: a re-failing test is FAIL_SERIAL and a skipped one is SKIPPED, "
                    "neither is reported as passing", a2))

    # A3: the OLD judgement -- "the crate summary says it passed" -- would call
    # the ABSENT test fine. Prove it: the summary line says 2 passed / 1 failed
    # and contains no mention of the untouched test, yet the old method would
    # have concluded the environment explained it.
    summary_says_pass = "2 passed" in SERIAL_SAMPLE
    old_would_clear_it = summary_says_pass and CI_SAMPLE[3] not in SERIAL_SAMPLE
    a3 = got[CI_SAMPLE[3]] == "ABSENT" and old_would_clear_it
    results.append(("A3 the OLD method (read the crate's N-passed summary) would have cleared a test "
                    "the re-run never executed; this one reports ABSENT", a3))

    for name, ok in results:
        print(f"[{'ok' if ok else 'FAIL'}] {name}")
    passed = sum(1 for _, ok in results if ok)
    print(f"{passed} passed, {len(results) - passed} failed")
    return 0 if passed == len(results) else 1


def main(argv):
    if "--selftest" in argv:
        return selftest()
    if len(argv) < 3:
        print(__doc__)
        return 2
    names = [l.strip() for l in open(argv[1], errors="replace") if l.strip()]
    seen = index(argv[2:])
    skips = skipped_names(argv[2:])
    counts = {}
    for n in names:
        verdict = classify(n, seen, skips)
        counts[verdict] = counts.get(verdict, 0) + 1
        print(f"{verdict}\t{n}")
    print("# " + "  ".join(f"{k}={v}" for k, v in sorted(counts.items())), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
