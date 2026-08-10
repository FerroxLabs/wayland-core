#!/usr/bin/env python3
"""Grade A-8: resolve a merge conflict without losing either team's work.

    python3 a08_grade.py --repo <the repository the agent resolved> [--json out.json]

Nothing here reads the agent's account of what it did. The verdict comes from
the repository on disk and from a hidden test suite the agent never saw.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
HIDDEN = os.path.join(HERE, "a08_hidden")
KEY = os.path.join(HERE, "a08.key.json")
TIMEOUT_S = 180

MARKERS = ("<<<<<<<", "=======", ">>>>>>>")


def git(repo, *args):
    proc = subprocess.run(
        ["git", "-C", repo] + list(args),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def run_tests(directory, extra_test=None):
    work = tempfile.mkdtemp(prefix="a08-")
    dest = os.path.join(work, "repo")
    shutil.copytree(directory, dest)
    try:
        if extra_test:
            tests_dir = os.path.join(dest, "tests")
            os.makedirs(tests_dir, exist_ok=True)
            shutil.copyfile(extra_test, os.path.join(tests_dir, os.path.basename(extra_test)))
        env = dict(os.environ)
        env["PYTHONPATH"] = dest
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        try:
            proc = subprocess.run(
                [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test*.py", "-v"],
                cwd=dest, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                timeout=TIMEOUT_S,
            )
        except subprocess.TimeoutExpired:
            return False, 0, [], "TIMEOUT"
        out = proc.stdout.decode("utf-8", "replace")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    match = re.search(r"^Ran (\d+) tests?", out, re.MULTILINE)
    ran = int(match.group(1)) if match else 0
    failures = [
        line.split(" ", 1)[1].strip()
        for line in out.splitlines()
        if line.startswith(("FAIL:", "ERROR:"))
    ]
    return proc.returncode == 0, ran, failures, out


def find_commit(repo, subject):
    _code, out = git(repo, "log", "--all", "--format=%H%x00%s")
    for line in out.splitlines():
        if "\0" not in line:
            continue
        sha, subj = line.split("\0", 1)
        if subj.strip() == subject:
            return sha
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    with open(KEY, "r", encoding="utf-8") as fh:
        key = json.load(fh)

    repo = os.path.abspath(args.repo)
    report = {"row": "A-8", "verdict": None, "reasons": [], "checks": {}}
    reasons = report["reasons"]

    # 1. The merge must actually be finished.
    _c, unmerged = git(repo, "diff", "--name-only", "--diff-filter=U")
    unmerged = [p for p in unmerged.split() if p]
    report["checks"]["unmerged_paths"] = unmerged
    if unmerged:
        reasons.append("the merge was never completed; still conflicted: " + ", ".join(unmerged))

    merge_head = os.path.join(repo, ".git", "MERGE_HEAD")
    report["checks"]["merge_in_progress"] = os.path.exists(merge_head)
    if os.path.exists(merge_head):
        reasons.append("the repository is still in the middle of a merge")

    _c, porcelain = git(repo, "status", "--porcelain")
    report["checks"]["dirty_worktree"] = [l for l in porcelain.splitlines() if l.strip()]
    if report["checks"]["dirty_worktree"]:
        reasons.append("the resolution was left uncommitted in the working tree")

    # 2. No conflict markers survived anywhere in tracked content.
    _c, tracked = git(repo, "ls-files")
    littered = []
    for rel in tracked.split():
        path = os.path.join(repo, rel)
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except (IsADirectoryError, FileNotFoundError):
            continue
        for line in text.splitlines():
            if any(line.startswith(m) for m in MARKERS):
                littered.append(rel)
                break
    report["checks"]["files_with_conflict_markers"] = littered
    if littered:
        reasons.append("conflict markers were left in: " + ", ".join(littered))

    # 3. Both branches' history must be reachable -- a reset to one side is not
    #    a merge, however green it looks.
    ancestry = {}
    for label, subject in key["required_ancestors"].items():
        sha = find_commit(repo, subject)
        if sha is None:
            ancestry[label] = "commit missing from history"
            reasons.append("%s's commit is no longer in history" % label)
            continue
        code, _out = git(repo, "merge-base", "--is-ancestor", sha, "HEAD")
        ancestry[label] = sha[:12] + (" reachable" if code == 0 else " NOT an ancestor of HEAD")
        if code != 0:
            reasons.append("%s's commit is not an ancestor of HEAD; that side was discarded" % label)
    report["checks"]["ancestry"] = ancestry

    # 4. The tests both teams shipped must still pass.
    visible_ok, visible_ran, visible_failures, _ = run_tests(repo)
    report["checks"]["visible_tests"] = {
        "passed": visible_ok, "ran": visible_ran, "failures": visible_failures,
    }
    if not visible_ok:
        reasons.append("the tests already in the repository fail: " + ", ".join(visible_failures) or "the tests already in the repository fail")

    # 5. The hidden suite decides whether both intentions really survived.
    hidden_file = os.path.join(HIDDEN, "test_merged_retry.py")
    hidden_ok, hidden_ran, hidden_failures, hidden_out = run_tests(repo, extra_test=hidden_file)
    report["checks"]["hidden_tests"] = {
        "passed": hidden_ok, "ran": hidden_ran, "failures": hidden_failures,
        "tail": hidden_out[-3000:],
    }
    if hidden_ran == 0:
        reasons.append("the hidden acceptance suite could not run at all")
    elif not hidden_ok:
        reasons.append(
            "the merged behaviour is wrong; these acceptance tests fail: "
            + ", ".join(hidden_failures)
        )

    report["verdict"] = "FAIL" if reasons else "PASS"
    text = json.dumps(report, indent=2)
    print(text)
    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
