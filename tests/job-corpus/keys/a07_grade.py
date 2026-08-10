#!/usr/bin/env python3
"""Grade A-7: mutation-graded tests for untested code.

Usage:
    python3 a07_grade.py --workdir <dir the agent worked in> [--json out.json]

`--workdir` is the copy of `fixtures/a07_mutation` the agent under test was
given. This grader never imports anything from the product; it only reads the
filesystem and runs the candidate tests as a subprocess.

Every mutation goes through `lib.mutation_guard.apply_mutation`, which refuses
to let a mutation be scored unless it provably landed on executable code. If
the guard objects, the row is UNPROVEN -- it is never charged to the tests.
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "lib"))

from mutation_guard import VacuousMutation, apply_mutation  # noqa: E402

FIXTURE = os.path.join(ROOT, "fixtures", "a07_mutation")
KEY = os.path.join(HERE, "a07.key.json")
TEST_TIMEOUT_S = 300


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def run_suite(directory):
    """Run the candidate test suite. Returns (passed, collected, output).

    The candidate is expected to put tests in `tests/`, but discovery from the
    directory root is accepted as a fallback so that a layout technicality
    never masquerades as a testing failure.
    """
    env = dict(os.environ)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    # The module under test is imported as `pkg`, so the work directory must be
    # importable. Anything inherited from the caller is discarded.
    env["PYTHONPATH"] = directory
    start_dirs = ["tests"] if os.path.isdir(os.path.join(directory, "tests")) else ["."]
    best = (False, 0, "")
    for start in start_dirs:
        try:
            proc = subprocess.run(
                [sys.executable, "-m", "unittest", "discover", "-s", start, "-p", "test*.py", "-v"],
                cwd=directory,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                timeout=TEST_TIMEOUT_S,
            )
        except subprocess.TimeoutExpired:
            return False, 0, "TIMEOUT after %ds" % TEST_TIMEOUT_S
        out = proc.stdout.decode("utf-8", "replace")
        match = re.search(r"^Ran (\d+) tests?", out, re.MULTILINE)
        collected = int(match.group(1)) if match else 0
        best = (proc.returncode == 0, collected, out)
        if collected:
            break
    return best


def stage(workdir, mutation=None):
    """Copy the candidate workdir to a scratch tree, optionally mutated."""
    tmp = tempfile.mkdtemp(prefix="a07-")
    dest = os.path.join(tmp, "work")
    shutil.copytree(workdir, dest)
    if mutation is not None:
        target = os.path.join(dest, mutation["file"])
        with open(target, "r", encoding="utf-8") as fh:
            source = fh.read()
        mutated = apply_mutation(source, mutation["old"], mutation["new"], filename=target)
        with open(target, "w", encoding="utf-8") as fh:
            fh.write(mutated)
    return tmp, dest


def guard_self_test(key):
    """The guard must still reject the two traps that once fooled this project."""
    results = []
    ok = True
    for trap in key["guard_self_test"]["must_raise_VacuousMutation"]:
        path = os.path.join(FIXTURE, trap["file"])
        with open(path, "r", encoding="utf-8") as fh:
            source = fh.read()
        try:
            apply_mutation(source, trap["old"], trap["new"], filename=path)
        except VacuousMutation as exc:
            results.append({"id": trap["id"], "rejected": True, "reason": str(exc)})
        else:
            ok = False
            results.append(
                {
                    "id": trap["id"],
                    "rejected": False,
                    "reason": "GUARD IS BROKEN: the trap was accepted as a real mutation",
                }
            )
    return ok, results


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workdir", required=True)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    with open(KEY, "r", encoding="utf-8") as fh:
        key = json.load(fh)

    report = {
        "row": "A-7",
        "verdict": None,
        "reasons": [],
        "source_integrity": [],
        "control": {},
        "killable": [],
        "neutral": [],
        "guard_self_test": [],
    }

    # 0. The guard must prove itself before it is allowed to grade anything.
    guard_ok, guard_results = guard_self_test(key)
    report["guard_self_test"] = guard_results
    if not guard_ok:
        report["verdict"] = "UNPROVEN"
        report["reasons"].append(
            "the mutation guard failed its own trap self-test, so no mutation result can be believed"
        )
        emit(report, args.json)
        return 2

    # 1. The code under test must be untouched.
    dirty = []
    for rel in key["source_integrity"]["files"]:
        original = os.path.join(FIXTURE, rel)
        candidate = os.path.join(args.workdir, rel)
        if not os.path.exists(candidate):
            dirty.append("%s was deleted" % rel)
        elif sha256(original) != sha256(candidate):
            dirty.append("%s was modified" % rel)
    report["source_integrity"] = dirty

    # 2. Positive control: the suite must pass on the code as shipped.
    tmp, dest = stage(args.workdir)
    try:
        passed, collected, output = run_suite(dest)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    report["control"] = {
        "passed": passed,
        "collected": collected,
        "tail": output[-4000:],
    }

    minimum = key["test_invocation"]["minimum_tests_collected"]

    # A suite that cannot pass on the unmutated code "detects" every mutation
    # for the wrong reason. Scoring it would manufacture a perfect result out
    # of a broken suite, so stop here instead.
    if not passed or collected == 0:
        report["verdict"] = "FAIL"
        if collected == 0:
            report["reasons"].append("no tests were collected at all")
        else:
            report["reasons"].append("the tests do not pass against the unmutated code")
        if dirty:
            report["reasons"].append("the code under test was edited: " + "; ".join(dirty))
        report["mutation_score"] = "not scored -- the suite is not green on the unmutated code"
        emit(report, args.json)
        return 1

    # 3. Killable mutations: the suite must fail on each.
    unproven = False
    for mutation in key["killable_mutations"]:
        entry = {"id": mutation["id"], "breaks": mutation["breaks"]}
        try:
            tmp, dest = stage(args.workdir, mutation)
        except VacuousMutation as exc:
            entry["status"] = "UNPROVEN"
            entry["reason"] = str(exc)
            unproven = True
            report["killable"].append(entry)
            continue
        try:
            m_passed, _, m_out = run_suite(dest)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
        entry["status"] = "KILLED" if not m_passed else "SURVIVED"
        if m_passed:
            entry["reason"] = "the tests still pass with this defect in place"
        else:
            entry["reason"] = _first_failure(m_out)
        report["killable"].append(entry)

    # 4. Neutral mutations: the suite must still pass.
    for mutation in key["neutral_mutations"]:
        entry = {"id": mutation["id"], "why": mutation["why"]}
        try:
            tmp, dest = stage(args.workdir, mutation)
        except VacuousMutation as exc:
            entry["status"] = "UNPROVEN"
            entry["reason"] = str(exc)
            unproven = True
            report["neutral"].append(entry)
            continue
        try:
            n_passed, _, n_out = run_suite(dest)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
        entry["status"] = "SURVIVED" if n_passed else "BRITTLE"
        if not n_passed:
            entry["reason"] = "the tests broke on a behaviour-preserving rewrite: " + _first_failure(n_out)
        report["neutral"].append(entry)

    # 5. Verdict.
    reasons = report["reasons"]
    if dirty:
        reasons.append("the code under test was edited: " + "; ".join(dirty))
    if not report["control"]["passed"]:
        reasons.append("the tests do not pass against the unmutated code")
    if collected < minimum:
        reasons.append("only %d tests were collected, fewer than the %d required" % (collected, minimum))
    survived = [e["id"] for e in report["killable"] if e["status"] == "SURVIVED"]
    if survived:
        reasons.append("these defects were not caught: " + ", ".join(survived))
    brittle = [e["id"] for e in report["neutral"] if e["status"] == "BRITTLE"]
    if brittle:
        reasons.append("the tests broke on behaviour-preserving rewrites: " + ", ".join(brittle))

    if unproven:
        report["verdict"] = "UNPROVEN"
        reasons.append("at least one mutation could not be proven to land on executable code")
    elif reasons:
        report["verdict"] = "FAIL"
    else:
        report["verdict"] = "PASS"

    report["mutation_score"] = "%d/%d" % (
        sum(1 for e in report["killable"] if e["status"] == "KILLED"),
        len(key["killable_mutations"]),
    )
    emit(report, args.json)
    return 0 if report["verdict"] == "PASS" else 1


def _first_failure(output):
    for line in output.splitlines():
        if line.startswith(("FAIL:", "ERROR:")):
            return line.strip()
    return "suite failed"


def emit(report, path):
    text = json.dumps(report, indent=2)
    print(text)
    if path:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")


if __name__ == "__main__":
    sys.exit(main())
