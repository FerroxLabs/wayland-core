#!/usr/bin/env python3
"""Run unchanged tag ledger gates with a committed, two-file metadata overlay."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile


ALLOWED = frozenset((
    ".planning/ledger/wayland-1116.md",
    ".planning/ledger/wayland-core-443.md",
))
CHECKERS = ("check-criteria-ledger.py", "check-release-readiness.py")


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args])


def commit(root, sha):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("release metadata requires a full lowercase commit SHA")
    resolved = git(root, "rev-parse", "--verify", sha + "^{commit}").decode().strip()
    if resolved != sha:
        raise ValueError("SHA must identify a commit, not a tag object")
    return resolved


def validate(root, tag, source_sha, metadata_sha):
    commit(root, source_sha)
    commit(root, metadata_sha)
    git(root, "check-ref-format", "refs/tags/" + tag)
    actual = git(root, "rev-parse", "--verify", "refs/tags/" + tag + "^{commit}").decode().strip()
    if actual != source_sha or git(root, "rev-parse", "HEAD").decode().strip() != source_sha:
        raise ValueError("stale tag or checkout: immutable source SHA does not match")
    parents = git(root, "show", "-s", "--format=%P", metadata_sha).decode().split()
    if parents != [source_sha]:
        raise ValueError("metadata must be one commit directly on the immutable tag source")
    changed = set(git(root, "diff", "--no-renames", "--name-only", "-z",
                      source_sha, metadata_sha, "--").decode().rstrip("\0").split("\0"))
    if changed != ALLOWED:
        raise ValueError("metadata tree must change exactly the two approved ledger paths")
    blobs = {}
    for path in sorted(ALLOWED):
        entry = git(root, "ls-tree", metadata_sha, "--", path).decode().strip()
        if not entry.startswith("100644 blob "):
            raise ValueError("approved ledger must exist as a regular non-executable file: " + path)
        blobs[path] = git(root, "show", metadata_sha + ":" + path)
    if git(root, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("source checkout must be clean before metadata admission")
    return blobs


def run(root, tag, source_sha, metadata_sha, receipt):
    root = Path(root).resolve()
    receipt = Path(receipt).resolve()
    # Do not allow a stale receipt to survive a failed re-run.
    receipt.unlink(missing_ok=True)
    blobs = validate(root, tag, source_sha, metadata_sha)
    report = {
        "schema_version": 1, "tag": tag, "source_sha": source_sha,
        "metadata_sha": metadata_sha, "status": "blocked", "checks": [],
        "ledger_sha256": {path: hashlib.sha256(data).hexdigest()
                          for path, data in blobs.items()},
    }
    receipt.parent.mkdir(parents=True, exist_ok=True)
    try:
        with tempfile.TemporaryDirectory(prefix="release-metadata-") as temporary:
            stage = Path(temporary) / "tag"
            git(root, "worktree", "add", "--detach", str(stage), source_sha)
            try:
                for path, data in blobs.items():
                    (stage / path).write_bytes(data)
                for checker in CHECKERS:
                    for args in (("--self-test",), ()):
                        command = [sys.executable, "-B", "scripts/" + checker, *args]
                        result = subprocess.run(command, cwd=stage, check=False)
                        report["checks"].append({"command": command[2:], "exit_code": result.returncode})
                        if result.returncode:
                            raise RuntimeError("tag checker failed: " + checker)
            finally:
                # Only this task-owned worktree is removed; the build checkout never changes.
                git(root, "worktree", "remove", "--force", str(stage))
        if git(root, "status", "--porcelain", "--untracked-files=all"):
            raise RuntimeError("source checkout changed during metadata admission")
        if git(root, "rev-parse", "HEAD").decode().strip() != source_sha:
            raise RuntimeError("source checkout identity changed during metadata admission")
        report["status"] = "passed"
    finally:
        receipt.write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--metadata-sha", required=True)
    parser.add_argument("--receipt", required=True)
    args = parser.parse_args()
    run(args.source, args.tag, args.source_sha, args.metadata_sha, args.receipt)


if __name__ == "__main__":
    main()
