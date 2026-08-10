#!/usr/bin/env python3
"""Prove the A-8 gate is winnable and failable before anyone runs it for real.

Builds the fixture repository four times and resolves the conflict four
different ways:

  correct  -- both intents merged        -> must PASS
  ours     -- keep main wholesale        -> must FAIL
  theirs   -- keep feature wholesale     -> must FAIL
  union    -- stack both hunks           -> must FAIL

A gate that cannot fail is worth exactly as much as one that cannot pass, so
this script is a prerequisite for believing any A-8 result.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SETUP = os.path.join(ROOT, "fixtures", "a08_merge", "setup_a08.py")
TREE = os.path.join(ROOT, "fixtures", "a08_merge", "tree")
CONTROLS = os.path.join(HERE, "a08_controls")
GRADER = os.path.join(HERE, "a08_grade.py")

RESOLUTIONS = {
    "correct": (os.path.join(CONTROLS, "correct.py"), "PASS"),
    "ours": (os.path.join(TREE, "main", "retry.py"), "FAIL"),
    "theirs": (os.path.join(TREE, "feature", "retry.py"), "FAIL"),
    "union": (os.path.join(CONTROLS, "union.py"), "FAIL"),
}


def git(repo, *args):
    env = dict(os.environ)
    env["GIT_AUTHOR_DATE"] = "2026-02-11T09:00:00+00:00"
    env["GIT_COMMITTER_DATE"] = "2026-02-11T09:00:00+00:00"
    env["HOME"] = repo
    proc = subprocess.run(
        ["git", "-C", repo, "-c", "user.name=Job Corpus",
         "-c", "user.email=job-corpus@example.invalid", "-c", "commit.gpgsign=false"]
        + list(args),
        env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def main():
    results = {}
    ok = True
    for name, (source, expected) in RESOLUTIONS.items():
        work = tempfile.mkdtemp(prefix="a08-self-")
        repo = os.path.join(work, "repo")
        try:
            proc = subprocess.run(
                [sys.executable, SETUP, "--dest", repo],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            )
            if proc.returncode != 0:
                results[name] = {
                    "verdict": "SETUP FAILED",
                    "output": proc.stdout.decode("utf-8", "replace"),
                }
                ok = False
                continue
            shutil.copyfile(source, os.path.join(repo, "retry.py"))
            git(repo, "add", "-A")
            git(repo, "commit", "-q", "-m", "Merge feature into main (%s)" % name)

            proc = subprocess.run(
                [sys.executable, GRADER, "--repo", repo],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            )
            report = json.loads(proc.stdout.decode("utf-8", "replace"))
            got = report["verdict"]
            results[name] = {
                "expected": expected,
                "got": got,
                "agrees": got == expected,
                "reasons": report["reasons"],
            }
            if got != expected:
                ok = False
        finally:
            shutil.rmtree(work, ignore_errors=True)

    print(json.dumps(results, indent=2))
    print("\nA-8 gate self-test: %s" % ("OK" if ok else "BROKEN"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
