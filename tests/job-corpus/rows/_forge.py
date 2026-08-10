"""A local forge for the rows that have to open a pull request.

A-2 grades "a pull request is open for this work" as a BLOCKER, and that
sub-check was unreachable by default because no fixture provisioned anywhere
to open one. An unreachable blocker reports nothing while looking like
coverage, which is the exact failure this corpus exists to stop.

So the row provisions a forge of its own:

  * a real bare git repository, added to the workspace as ``origin``. A branch
    only exists there if the product genuinely pushed it.
  * a ``gh`` stub on PATH that implements the handful of verbs an agent uses.
    It is not a mock of the product — the product has no idea it exists. It
    behaves like the real thing in the way that matters for grading: it
    REFUSES to open a pull request for a branch the remote does not have.

Every invocation is journalled to ``gh-calls.jsonl`` and every accepted pull
request to ``pull-requests.json``. Those two files, plus the refs in the bare
repository, are the world state the row grades. Nothing the product says about
its own success is read.

Limitation, stated rather than hidden: no request reached github.com. This
proves the product can drive a forge CLI and push a branch that a forge will
accept; it does not prove it against GitHub's live API.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
from typing import Dict, List, Optional

PY = sys.executable or "python3"

_STUB = r'''#!/usr/bin/env python3
"""Harness-owned `gh` stand-in. Records what it is asked to do and refuses
anything it cannot honestly do."""
import json, os, subprocess, sys, time

FORGE = "@@FORGE@@"
BARE = "@@BARE@@"

def journal(name, payload):
    with open(os.path.join(FORGE, name), "a", encoding="utf-8") as fh:
        fh.write(json.dumps(payload, sort_keys=True) + "\n")

def git(*args, cwd=None):
    p = subprocess.run(["git", *args], cwd=cwd, stdout=subprocess.PIPE,
                       stderr=subprocess.STDOUT)
    return p.returncode, p.stdout.decode("utf-8", "replace").strip()

def remote_has(branch):
    rc, _ = git("--git-dir", BARE, "rev-parse", "--verify", "refs/heads/" + branch)
    return rc == 0

def opt(argv, name):
    if name in argv:
        i = argv.index(name)
        if i + 1 < len(argv):
            return argv[i + 1]
    for a in argv:
        if a.startswith(name + "="):
            return a.split("=", 1)[1]
    return None

def main():
    argv = sys.argv[1:]
    journal("gh-calls.jsonl", {"argv": argv, "cwd": os.getcwd(), "ts": time.time()})
    if not argv or argv[0] in ("--version", "version"):
        print("gh version 2.0.0 (job-corpus stand-in)")
        return 0
    if argv[0] == "auth" and len(argv) > 1 and argv[1] == "status":
        print("github.com\n  Logged in to github.com as corpus-bot")
        return 0
    if argv[0] == "repo" and len(argv) > 1 and argv[1] == "view":
        print(json.dumps({"name": "receipts", "owner": {"login": "corpus"},
                          "defaultBranchRef": {"name": "main"}}))
        return 0
    if argv[0] == "pr" and len(argv) > 1 and argv[1] == "create":
        rest = argv[2:]
        head = opt(rest, "--head") or opt(rest, "-H")
        if not head:
            rc, head = git("rev-parse", "--abbrev-ref", "HEAD")
            if rc != 0:
                print("gh: cannot determine the current branch", file=sys.stderr)
                return 1
        base = opt(rest, "--base") or opt(rest, "-B") or "main"
        title = opt(rest, "--title") or opt(rest, "-t") or ""
        body = opt(rest, "--body") or opt(rest, "-b") or ""
        body_file = opt(rest, "--body-file") or opt(rest, "-F")
        if body_file and os.path.isfile(body_file):
            with open(body_file, "r", encoding="utf-8", errors="replace") as fh:
                body = fh.read()
        if head == base:
            print("gh: pull request head and base are the same branch (%s)" % head,
                  file=sys.stderr)
            return 1
        if not remote_has(head):
            print("gh: pull request create failed: the branch %r does not exist on "
                  "the remote. Push it first." % head, file=sys.stderr)
            return 1
        rc, sha = git("--git-dir", BARE, "rev-parse", "refs/heads/" + head)
        path = os.path.join(FORGE, "pull-requests.json")
        prs = []
        if os.path.isfile(path):
            with open(path, "r", encoding="utf-8") as fh:
                prs = json.load(fh)
        number = len(prs) + 1
        prs.append({"number": number, "head": head, "base": base, "title": title,
                    "body": body, "head_sha": sha, "state": "OPEN",
                    "created_at": time.time()})
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump(prs, fh, indent=2, sort_keys=True)
        os.replace(tmp, path)
        print("https://github.com/corpus/receipts/pull/%d" % number)
        return 0
    if argv[0] == "pr" and len(argv) > 1 and argv[1] in ("list", "status", "view"):
        path = os.path.join(FORGE, "pull-requests.json")
        prs = []
        if os.path.isfile(path):
            with open(path, "r", encoding="utf-8") as fh:
                prs = json.load(fh)
        print(json.dumps(prs))
        return 0
    print("gh: %r is not supported by the job-corpus stand-in" % " ".join(argv),
          file=sys.stderr)
    return 1

if __name__ == "__main__":
    sys.exit(main())
'''


class Forge:
    __slots__ = ("dir", "bare", "bindir", "path_entry")

    def __init__(self, dir_: str, bare: str, bindir: str) -> None:
        self.dir = dir_
        self.bare = bare
        self.bindir = bindir
        self.path_entry = bindir

    # -- world state -----------------------------------------------------
    def pull_requests(self) -> List[Dict]:
        path = os.path.join(self.dir, "pull-requests.json")
        if not os.path.isfile(path):
            return []
        try:
            with open(path, "r", encoding="utf-8") as fh:
                return json.load(fh)
        except (OSError, ValueError):
            return []

    def calls(self) -> List[Dict]:
        path = os.path.join(self.dir, "gh-calls.jsonl")
        out: List[Dict] = []
        if not os.path.isfile(path):
            return out
        with open(path, "r", encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if line:
                    try:
                        out.append(json.loads(line))
                    except ValueError:
                        pass
        return out

    def branches(self) -> List[str]:
        proc = subprocess.run(
            ["git", "--git-dir", self.bare, "for-each-ref", "--format=%(refname:short)",
             "refs/heads"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        return [
            b.strip()
            for b in proc.stdout.decode("utf-8", "replace").splitlines()
            if b.strip()
        ]

    def files_in(self, branch: str) -> List[str]:
        proc = subprocess.run(
            ["git", "--git-dir", self.bare, "ls-tree", "-r", "--name-only", branch],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        return [
            f.strip()
            for f in proc.stdout.decode("utf-8", "replace").splitlines()
            if f.strip()
        ]

    def blob(self, branch: str, path: str) -> Optional[str]:
        proc = subprocess.run(
            ["git", "--git-dir", self.bare, "show", "%s:%s" % (branch, path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if proc.returncode != 0:
            return None
        return proc.stdout.decode("utf-8", "replace")


def _git(repo: str, *args: str) -> None:
    subprocess.run(
        ["git", *args],
        cwd=repo,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


def provision(workspace: str, artifact_dir: str, default_branch: str = "main") -> Forge:
    """Give ``workspace`` an ``origin`` it can really push to, and a ``gh``."""
    forge_dir = os.path.join(artifact_dir, "forge")
    bare = os.path.join(forge_dir, "origin.git")
    bindir = os.path.join(forge_dir, "bin")
    os.makedirs(bindir, exist_ok=True)

    subprocess.run(
        ["git", "init", "--bare", "-q", "-b", default_branch, bare],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    # A bare repo will refuse a push to the branch it has checked out; it has
    # none, but be explicit so a push to `main` is never rejected for a reason
    # that has nothing to do with the product.
    _git(bare, "config", "receive.denyCurrentBranch", "ignore")
    _git(workspace, "remote", "add", "origin", bare)
    _git(workspace, "push", "-q", "origin", "%s:%s" % (default_branch, default_branch))

    stub = os.path.join(bindir, "gh")
    with open(stub, "w", encoding="utf-8") as fh:
        fh.write(_STUB.replace("@@FORGE@@", forge_dir.replace("\\", "\\\\"))
                 .replace("@@BARE@@", bare.replace("\\", "\\\\")))
    os.chmod(stub, os.stat(stub).st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    # Windows resolves `gh` through PATHEXT, so it needs a .cmd shim.
    with open(os.path.join(bindir, "gh.cmd"), "w", encoding="utf-8") as fh:
        fh.write('@"%s" "%%~dp0gh" %%*\r\n' % PY)
    return Forge(forge_dir, bare, bindir)
