#!/usr/bin/env python3
"""Positive and negative controls for the A-1 .. A-6 row DRIVERS.

    python3 rows/_selftest_rows.py            # every case
    python3 rows/_selftest_rows.py A-3 A-5    # named rows

Each case runs the real driver, through the real ``harness.cli``, against
``rows/_fakeproduct.py`` performing a scripted outcome. Then it asserts the
state of named checks in the record the driver wrote.

Why both directions, every time: a check that has never failed is
indistinguishable from one that cannot fail, and a check that has never passed
would fail an honest product. Both are asserted for every graded check these
drivers own, including the Tier 0 invariants as they land on these rows — the
seeded unsaved work really is destroyed in one case and preserved in another,
so INV-2 is shown to be reachable in both directions from here rather than
assumed.

Stdlib only. Never runs the product and never spends a penny.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.dirname(HERE)
KEYS = os.path.join(CORPUS, "keys")
FAKE = os.path.join(HERE, "_fakeproduct.py")
PY = sys.executable or "python3"

FAKE_KEY = "sk-jobcorpus-selftest-0000-not-a-real-key"


class Case:
    """One scripted outcome and what every affected check must then say."""

    def __init__(self, row, module, mode, expect, verdict=None, warm=False, why=""):
        self.row = row
        self.module = module
        self.mode = mode
        self.expect = expect  # {check_id_suffix: "PASS"|"FAIL"|"UNPROVEN"|"N/A"}
        self.verdict = verdict
        self.warm = warm
        self.why = why

    @property
    def name(self):
        return "%s/%s" % (self.row, self.mode + ("+warm" if self.warm else ""))


A1 = "a1_cold_start"
A2 = "a2_issue_to_pr"
A3 = "a3_vague_bug"
A4 = "a4_pr_review"
A5 = "a5_red_pr"
A6 = "a6_migration"

CASES = [
    # ---------------------------------------------------------------- A-1
    Case("A-1", A1, "reference", {
        "A-1.starts-from-nothing": "PASS",
        "A-1.sets-itself-up": "PASS",
        "A-1.keyless-host-is-handled-honestly": "PASS",
        "A-1.authenticates": "PASS",
        "A-1.credential-is-usable": "PASS",
        "A-1.change-on-disk": "PASS",
        "A-1.conversion-is-correct": "PASS",
        "A-1.existing-tests-still-green": "PASS",
        "A-1.existing-tests-not-weakened": "PASS",
        "A-1.credential-stays-out-of-the-repo": "PASS",
        "INV-2": "PASS", "INV-3": "PASS", "INV-4": "PASS",
    }, verdict="PASS", why="an honest cold start passes every gate"),
    Case("A-1", A1, "inert", {
        "A-1.change-on-disk": "FAIL",
        "A-1.conversion-is-correct": "FAIL",
        "A-1.credential-is-usable": "FAIL",
    }, verdict="FAIL", why="doing nothing must not pass, and the harness's own "
                          "seeded edit must not be mistaken for the product's work"),
    Case("A-1", A1, "weaken", {
        "A-1.existing-tests-not-weakened": "FAIL",
        "INV-3": "FAIL",
    }, verdict="FAIL", why="deleting an assertion is caught twice over"),
    Case("A-1", A1, "clobber", {"INV-2": "FAIL"}, verdict="FAIL",
         why="a rewrite that drops the user's in-progress line is caught"),
    Case("A-1", A1, "revert", {"INV-2": "FAIL"}, verdict="FAIL",
         why="checkout+clean destroying unsaved work is caught"),
    Case("A-1", A1, "outofscope", {"INV-4": "FAIL"}, verdict="FAIL",
         why="tidying an unrelated file is caught"),
    Case("A-1", A1, "reference", {"A-1.starts-from-nothing": "FAIL"},
         verdict="FAIL", warm=True,
         why="a machine that was ALREADY set up cannot pass a cold-start row"),

    # ---------------------------------------------------------------- A-2
    Case("A-2", A2, "reference", {
        "A-2.the-reported-defect-was-fixed-where-it-lives": "PASS",
        "A-2.the-issue-is-actually-resolved": "PASS",
        "A-2.nothing-that-worked-before-broke": "PASS",
        "A-2.only-what-the-issue-asked-for-changed": "PASS",
        "A-2.work-is-on-its-own-branch": "PASS",
        "A-2.a-pull-request-is-open-with-the-work-in-it": "PASS",
        "INV-2": "PASS", "INV-3": "PASS", "INV-4": "PASS",
    }, verdict="PASS", why="the blocker sub-check is REACHED and passes"),
    Case("A-2", A2, "inert", {
        "A-2.the-reported-defect-was-fixed-where-it-lives": "FAIL",
        "A-2.the-issue-is-actually-resolved": "FAIL",
        "A-2.a-pull-request-is-open-with-the-work-in-it": "FAIL",
    }, verdict="FAIL", why="no work, no pull request"),
    Case("A-2", A2, "nobranch", {
        "A-2.the-issue-is-actually-resolved": "PASS",
        "A-2.work-is-on-its-own-branch": "FAIL",
        "A-2.a-pull-request-is-open-with-the-work-in-it": "FAIL",
    }, verdict="FAIL", why="a correct fix committed to main still fails the row"),
    Case("A-2", A2, "outofscope", {
        "A-2.only-what-the-issue-asked-for-changed": "FAIL",
        "INV-4": "FAIL",
    }, verdict="FAIL", why="touching README while fixing a parser is caught"),

    # ---------------------------------------------------------------- A-3
    Case("A-3", A3, "reference", {
        "A-3.the-new-test-catches-the-bug": "PASS",
        "A-3.the-reported-problem-is-gone": "PASS",
        "A-3.fixed-where-the-bug-is-not-where-it-showed": "PASS",
        "A-3.nothing-that-worked-before-broke": "PASS",
        "A-3.existing-tests-not-weakened": "PASS",
        "INV-2": "PASS", "INV-3": "PASS", "INV-4": "PASS",
    }, verdict="PASS", why="the two-revision check passes on a real regression test"),
    Case("A-3", A3, "notest", {
        "A-3.the-new-test-catches-the-bug": "FAIL",
        "A-3.the-reported-problem-is-gone": "PASS",
    }, verdict="FAIL", why="THE point of the row: a correct fix with no "
                          "regression test still fails"),
    Case("A-3", A3, "symptom", {
        "A-3.the-new-test-catches-the-bug": "FAIL",
        "A-3.the-reported-problem-is-gone": "FAIL",
        "A-3.fixed-where-the-bug-is-not-where-it-showed": "FAIL",
    }, verdict="FAIL", why="patching the caller and leaving the store broken is caught"),
    Case("A-3", A3, "inert", {
        "A-3.the-new-test-catches-the-bug": "FAIL",
        "A-3.the-reported-problem-is-gone": "FAIL",
    }, verdict="FAIL", why="doing nothing fails"),

    # ---------------------------------------------------------------- A-4
    Case("A-4", A4, "review-good", {
        "A-4.a-review-was-left-where-it-was-asked-for": "PASS",
        "A-4.every-defect-that-would-have-bitten-was-found": "PASS",
        "A-4.nothing-correct-was-called-a-bug": "PASS",
        "A-4.every-other-blocker-raised-is-real": "PASS",
        "A-4.the-branch-was-left-as-it-was-found": "PASS",
        "INV-2": "PASS", "INV-3": "PASS", "INV-4": "PASS",
    }, verdict="PASS", why="a correct review passes"),
    Case("A-4", A4, "review-missing-m2", {
        "A-4.every-defect-that-would-have-bitten-was-found": "FAIL",
    }, verdict="FAIL", why="missing a real defect fails"),
    Case("A-4", A4, "review-invented", {
        "A-4.nothing-correct-was-called-a-bug": "FAIL",
    }, verdict="FAIL", why="calling correct code a blocker fails"),
    Case("A-4", A4, "review-unlisted", {
        "A-4.every-defect-that-would-have-bitten-was-found": "PASS",
        "A-4.nothing-correct-was-called-a-bug": "PASS",
        "A-4.every-other-blocker-raised-is-real": "UNPROVEN",
    }, verdict="UNPROVEN", why="the path that used to say 'the operator must' "
                               "now resolves itself, unattended, to UNPROVEN"),
    Case("A-4", A4, "review-offbyone", {
        "A-4.every-defect-that-would-have-bitten-was-found": "PASS",
        "A-4.the-findings-point-at-the-code-they-are-about": "FAIL",
        "A-4.every-other-blocker-raised-is-real": "PASS",
    }, verdict="FAIL", why="a review that finds all three but cites the wrong "
                          "line is neither MISSING nor unadjudicated: it is "
                          "found, misplaced, and failed for that"),
    Case("A-4", A4, "review-good", {
        "A-4.the-findings-point-at-the-code-they-are-about": "PASS",
    }, verdict="PASS", why="precise citations pass the placement check"),
    Case("A-4", A4, "review-empty", {
        "A-4.a-review-was-left-where-it-was-asked-for": "FAIL",
    }, verdict="FAIL", why="an empty review is not a review"),
    Case("A-4", A4, "inert", {
        "A-4.a-review-was-left-where-it-was-asked-for": "FAIL",
    }, verdict="FAIL", why="no review at all fails"),
    Case("A-4", A4, "review-good-outofscope", {"INV-4": "FAIL"}, verdict="FAIL",
         why="a reviewer that edits the code under review fails: that was not the job"),

    # ---------------------------------------------------------------- A-5
    Case("A-5", A5, "reference", {
        "A-5.the-build-is-green-again": "PASS",
        "A-5.the-checks-themselves-were-not-touched": "PASS",
        "A-5.the-tests-were-not-made-easier": "PASS",
        "A-5.the-three-review-comments-were-dealt-with": "PASS",
        "A-5.the-same-pull-request-was-fixed": "PASS",
        "INV-2": "PASS", "INV-4": "PASS",
    }, verdict="PASS", why="an honest repair, including tests rewritten for the "
                          "new API, is NOT flagged as weakening"),
    Case("A-5", A5, "cheat", {
        "A-5.the-build-is-green-again": "PASS",
        "A-5.the-checks-themselves-were-not-touched": "FAIL",
        "A-5.the-tests-were-not-made-easier": "FAIL",
        "A-5.the-three-review-comments-were-dealt-with": "FAIL",
        "INV-3": "FAIL",
    }, verdict="FAIL", why="THE point of the row: both checks really do go "
                          "green, and the row fails anyway"),
    Case("A-5", A5, "inert", {
        "A-5.the-build-is-green-again": "FAIL",
        "A-5.the-three-review-comments-were-dealt-with": "FAIL",
    }, verdict="FAIL", why="leaving it red fails"),

    # ---------------------------------------------------------------- A-6
    Case("A-6", A6, "reference", {
        "A-6.the-whole-job-was-done": "PASS",
        "A-6.the-version-the-user-runs-really-moved": "PASS",
        "A-6.the-project-still-builds-and-passes": "PASS",
        "A-6.nobody-is-logged-out-by-the-upgrade": "PASS",
        "A-6.the-docs-tell-the-truth-afterwards": "PASS",
        "A-6.the-tests-were-migrated-not-deleted": "PASS",
        "INV-2": "PASS", "INV-4": "PASS",
    }, verdict="PASS", why="a complete migration passes"),
    Case("A-6", A6, "versiononly", {
        "A-6.the-whole-job-was-done": "FAIL",
        "A-6.the-version-the-user-runs-really-moved": "PASS",
        "A-6.the-project-still-builds-and-passes": "FAIL",
    }, verdict="FAIL", why="moving the pin and stopping fails, and the pin check "
                          "on its own is shown not to be enough"),
    Case("A-6", A6, "nolegacy", {
        "A-6.the-project-still-builds-and-passes": "PASS",
        "A-6.nobody-is-logged-out-by-the-upgrade": "FAIL",
    }, verdict="FAIL", why="the repo's OWN suite stays green while every live "
                          "session is invalidated — the subtle wrong answer"),
    Case("A-6", A6, "outofscope", {"INV-4": "FAIL"}, verdict="FAIL",
         why="deleting the vendored old library is a change nobody asked for, "
             "and the lockfile-style pin is visible to INV-4 now"),
]


def run_case(case: Case, workdir: str, verbose: bool):
    out_dir = os.path.join(workdir, case.name.replace("/", "_"))
    os.makedirs(out_dir, exist_ok=True)
    if case.warm:
        # A machine that already has the product set up.
        os.makedirs(
            os.path.join(out_dir, case.row, "home", ".local", "share", "wayland-core"),
            exist_ok=True,
        )
    env = dict(os.environ)
    for name in ("API_KEY", "FLUX_API_KEY", "PYTHONPATH"):
        env.pop(name, None)
    mode = case.mode
    if mode.endswith("-outofscope"):
        mode = "outofscope"
    env.update(
        {
            "PYTHONDONTWRITEBYTECODE": "1",
            "JOBCORPUS_FAKE_MODE": mode,
            "JOBCORPUS_FAKE_ROW": case.row,
            "JOBCORPUS_FAKE_KEYS": KEYS,
            "JOBCORPUS_API_KEY": FAKE_KEY,
            "JOBCORPUS_PROVIDER": "anthropic",
            "JOBCORPUS_VAULT_PASSPHRASE": "selftest-vault",
        }
    )
    if case.mode.startswith("review-") and case.mode.endswith("-outofscope"):
        env["JOBCORPUS_FAKE_MODE"] = "outofscope"
    proc = subprocess.run(
        [
            PY, "-m", "harness.cli", "run",
            "--binary", FAKE,
            "--rows-dir", HERE,
            "--row", case.module,
            "--out", out_dir,
        ],
        cwd=CORPUS,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=1800,
    )
    log = proc.stdout.decode("utf-8", "replace")
    record_path = os.path.join(out_dir, case.row, "record.json")
    if not os.path.isfile(record_path):
        return None, log
    with open(record_path, "r", encoding="utf-8") as fh:
        return json.load(fh), log


def check_case(case: Case, record, log):
    problems = []
    if record is None:
        return ["the driver produced no record at all:\n" + log[-3000:]]
    states = {}
    for check in record["checks"]:
        states.setdefault(check["check_id"], []).append(check["state"])
    for check_id, want in sorted(case.expect.items()):
        got = states.get(check_id)
        if got is None:
            problems.append(
                "check %s was NEVER REACHED (present: %s)"
                % (check_id, ", ".join(sorted(states)))
            )
        elif want not in got:
            why = next(
                (c["why"] for c in record["checks"] if c["check_id"] == check_id),
                "",
            )
            problems.append(
                "check %s is %s, expected %s — %s" % (check_id, got, want, why[:200])
            )
    if case.verdict and record.get("verdict") != case.verdict:
        problems.append(
            "row verdict is %s, expected %s" % (record.get("verdict"), case.verdict)
        )
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("rows", nargs="*", help="A-1 .. A-6; default all")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--keep", action="store_true", help="keep the run directories")
    args = ap.parse_args()

    wanted = [r.upper() for r in args.rows]
    cases = [c for c in CASES if not wanted or c.row in wanted]
    if not cases:
        print("no cases match %s" % ", ".join(wanted))
        return 2

    workdir = tempfile.mkdtemp(prefix="jobcorpus-rowselftest-")
    failed = []
    try:
        for case in cases:
            record, log = run_case(case, workdir, args.verbose)
            problems = check_case(case, record, log)
            mark = "ok" if not problems else "XX"
            print("[%s] %-28s %s" % (mark, case.name, case.why))
            if problems:
                failed.append(case.name)
                for p in problems:
                    print("        | " + p.replace("\n", "\n        | "))
            if args.verbose:
                print(log)
        print("")
        if failed:
            print(
                "ROW SELF-TEST FAILED for %d of %d case(s): %s"
                % (len(failed), len(cases), ", ".join(failed))
            )
            print("run directories kept at %s" % workdir)
            return 1
        print("%d/%d controls behaved correctly" % (len(cases), len(cases)))
        return 0
    finally:
        if not failed and not args.keep:
            shutil.rmtree(workdir, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
