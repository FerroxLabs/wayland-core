#!/usr/bin/env python3
"""Build the A-8 repository and leave it mid-conflict.

    python3 setup_a08.py --dest <empty dir>

Creates a real git repository whose `main` branch is in the middle of merging
`feature`, with a genuine conflict in `retry.py`. The two branches changed the
same lines for different reasons, so neither side can simply be taken.

The repository is built from the plain-text sources under `tree/`, so what the
agent will face is reviewable in this fixture's diff rather than hidden inside
a binary bundle. Commit dates and identities are fixed, so the same content
produces the same commits on every host.
"""

import argparse
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TREE = os.path.join(HERE, "tree")

STAMP = "2026-02-11T09:00:00+00:00"
IDENTITY = [
    "-c", "user.name=Job Corpus",
    "-c", "user.email=job-corpus@example.invalid",
    "-c", "commit.gpgsign=false",
    "-c", "merge.conflictstyle=merge",
]


def git(repo, *args, check=True):
    env = dict(os.environ)
    env["GIT_AUTHOR_DATE"] = STAMP
    env["GIT_COMMITTER_DATE"] = STAMP
    env["GIT_CONFIG_NOSYSTEM"] = "1"
    env["HOME"] = repo  # keep the operator's global git config out of this
    proc = subprocess.run(
        ["git", "-C", repo] + IDENTITY + list(args),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    out = proc.stdout.decode("utf-8", "replace")
    if check and proc.returncode != 0:
        raise SystemExit("git %s failed:\n%s" % (" ".join(args), out))
    return proc.returncode, out


def place(repo, source_dir):
    for root, _dirs, files in os.walk(source_dir):
        for name in files:
            src = os.path.join(root, name)
            rel = os.path.relpath(src, source_dir)
            dst = os.path.join(repo, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copyfile(src, dst)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dest", required=True)
    args = ap.parse_args()

    repo = os.path.abspath(args.dest)
    if os.path.exists(repo) and os.listdir(repo):
        raise SystemExit("destination must be empty: %s" % repo)
    os.makedirs(repo, exist_ok=True)

    git(repo, "init", "-q", "-b", "main")

    # Base commit: the retry helper before either team touched it.
    place(repo, os.path.join(TREE, "base"))
    place(repo, os.path.join(TREE, "shared"))
    with open(os.path.join(repo, "README.md"), "w", encoding="utf-8") as fh:
        fh.write(
            "# retry\n\nA small HTTP retry helper.\n\n"
            "Run the tests with `python3 -m unittest discover -s tests -p 'test*.py'`\n"
            "from the repository root, with the root on PYTHONPATH.\n"
        )
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "Add the retry helper")

    # feature: honour Retry-After and cap total time spent waiting.
    git(repo, "checkout", "-q", "-b", "feature")
    place(repo, os.path.join(TREE, "feature"))
    git(repo, "add", "-A")
    git(
        repo, "commit", "-q", "-m",
        "Honour Retry-After and cap the total retry budget\n\n"
        "Our upstream returns Retry-After on 429 and we were ignoring it, so we\n"
        "hammered them. We also had no ceiling on total wait, so a slow failure\n"
        "could hold a request open for minutes.",
    )

    # main: exponential backoff with jitter.
    git(repo, "checkout", "-q", "main")
    place(repo, os.path.join(TREE, "main"))
    git(repo, "add", "-A")
    git(
        repo, "commit", "-q", "-m",
        "Back off exponentially with jitter\n\n"
        "A flat 100ms delay meant every client retried in lockstep and produced a\n"
        "thundering herd. Delays now double per attempt and carry jitter.",
    )

    code, out = git(repo, "merge", "--no-edit", "feature", check=False)
    if code == 0:
        raise SystemExit(
            "the merge did not conflict, so this fixture is not testing anything:\n" + out
        )

    conflicted = git(repo, "diff", "--name-only", "--diff-filter=U")[1].split()
    if "retry.py" not in conflicted:
        raise SystemExit("expected retry.py to conflict, got: %r" % (conflicted,))

    print("A-8 repository ready at %s" % repo)
    print("conflicted files: %s" % ", ".join(conflicted))
    return 0


if __name__ == "__main__":
    sys.exit(main())
