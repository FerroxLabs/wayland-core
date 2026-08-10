"""Shared helpers for job-corpus fixture builders.

Every fixture is materialised at run time from a plain ``tree/`` directory into
a real git repository with real history.  Storing the fixture as a tree (rather
than a nested ``.git``) keeps it reviewable in this repository and keeps the
built repo byte-identical on Linux, macOS and Windows.

Stdlib only.  Nothing here imports or depends on any product code.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys

FIXED_DATE = "2026-01-05T09:00:00+00:00"
AUTHOR_NAME = "Job Corpus"
AUTHOR_EMAIL = "corpus@example.invalid"


def _env():
    env = dict(os.environ)
    env.update(
        {
            "GIT_AUTHOR_NAME": AUTHOR_NAME,
            "GIT_AUTHOR_EMAIL": AUTHOR_EMAIL,
            "GIT_COMMITTER_NAME": AUTHOR_NAME,
            "GIT_COMMITTER_EMAIL": AUTHOR_EMAIL,
            "GIT_AUTHOR_DATE": FIXED_DATE,
            "GIT_COMMITTER_DATE": FIXED_DATE,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_SYSTEM": os.devnull,
        }
    )
    return env


def git(repo, *args):
    return subprocess.run(
        ["git", *args],
        cwd=repo,
        env=_env(),
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    ).stdout


def fresh_repo(dest):
    """Create an empty git repo at ``dest``, removing anything already there."""
    dest = os.path.abspath(dest)
    if os.path.exists(dest):
        shutil.rmtree(dest)
    os.makedirs(dest)
    git(dest, "init", "-q", "-b", "main")
    git(dest, "config", "user.name", AUTHOR_NAME)
    git(dest, "config", "user.email", AUTHOR_EMAIL)
    git(dest, "config", "core.autocrlf", "false")
    git(dest, "config", "commit.gpgsign", "false")
    return dest


def copy_tree(src, dest, subdir=""):
    """Copy ``src`` (a fixture ``tree/`` dir) into ``dest`` with LF endings."""
    src = os.path.abspath(src)
    for dirpath, dirnames, filenames in os.walk(src):
        dirnames[:] = [d for d in dirnames if d != "__pycache__"]
        for name in filenames:
            if name.endswith(".pyc"):
                continue
            abs_src = os.path.join(dirpath, name)
            rel = os.path.relpath(abs_src, src)
            abs_dest = os.path.join(dest, subdir, rel)
            os.makedirs(os.path.dirname(abs_dest), exist_ok=True)
            with open(abs_src, "rb") as fh:
                data = fh.read()
            data = data.replace(b"\r\n", b"\n")
            with open(abs_dest, "wb") as fh:
                fh.write(data)


def commit_all(repo, message, tag=None):
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", message)
    if tag:
        git(repo, "tag", tag)
    return git(repo, "rev-parse", "HEAD").strip()


def here(builder_file):
    return os.path.dirname(os.path.abspath(builder_file))


def dest_from_argv(default_name):
    if len(sys.argv) < 2:
        raise SystemExit(
            "usage: python3 build.py <destination-dir>   "
            "(fixture %s)" % default_name
        )
    return sys.argv[1]
