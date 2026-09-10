#!/usr/bin/env python3
"""Cut wayland#1357 red arms: each is BASE (the repair) plus ONE repair removed.

Usage: mutate1357.py BASE [skip...]. Every mutation must land exactly once, on
code, or the script refuses. SCRATCH ONLY: none of these branches may be merged.
"""
import pathlib
import subprocess
import sys
import time

WT = pathlib.Path.home() / "dev/waylandcore-stabilization-20260905/worktrees/w15-quota1357-red2"
FILE = WT / "crates/wcore-agent/src/session_journal.rs"
BASE = sys.argv[1]


def git(*args):
    for attempt in range(3):
        done = subprocess.run(["/usr/bin/git", "-C", str(WT), *args], capture_output=True, text=True)
        if done.returncode == 0:
            return done.stdout
        print(f"git {args[0]} attempt {attempt + 1} rc={done.returncode}: {done.stderr.strip()}")
        time.sleep(2)
    sys.exit(f"REFUSE: git {' '.join(args)} failed")


def swap(src, old, new):
    count = src.count(old)
    if count != 1:
        sys.exit(f"REFUSE: {old!r} occurs {count} times")
    line = src[: src.index(old)].rsplit("\n", 1)[-1]
    if line.lstrip().startswith("//"):
        sys.exit(f"REFUSE: {old!r} is in a comment")
    return src.replace(old, new)


def registry(src):  # nothing is ever registered live
    return swap(src, "lock_live_temporaries(live).insert(name.clone());",
                "let _ = (live, &name);")


def link_exception(src):  # a live temporary is kept even when it only links the published file
    return swap(src, "            && !is_link_to_published_checkpoint(&metadata, published_checkpoint)\n",
                "            && !(is_link_to_published_checkpoint(&metadata, published_checkpoint) && false)\n")


def stat_gone(src):  # a temporary gone before cleanup's stat is an error again
    return swap(src, "            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,\n", "")


def remove_gone(src):  # a temporary gone before cleanup's removal is an error again
    return swap(src, "                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}\n", "")


def rescan(src):  # an entry vanishing during the quota scan fails the store again
    return swap(src, "                if source.kind() == std::io::ErrorKind::NotFound\n",
                "                if false && source.kind() == std::io::ErrorKind::NotFound\n")


MUTATIONS = [
    ("registry", registry, "no temporary is registered live"),
    ("link", link_exception, "live links to the checkpoint are kept"),
    ("stat", stat_gone, "vanished before stat is an error"),
    ("remove", remove_gone, "vanished before removal is an error"),
    ("rescan", rescan, "no rescan for a vanished entry"),
]

skip = set(sys.argv[2:])
for name, mutate, label in MUTATIONS:
    if name in skip:
        continue
    git("checkout", "-q", "-B", f"w15/quota1357-red-{name}", BASE)
    src = FILE.read_text()
    new = mutate(src)
    if new == src:
        sys.exit(f"REFUSE: {name} produced no change")
    FILE.write_text(new)
    FILE.touch()
    numstat = git("diff", "--numstat").strip()
    if not numstat:
        sys.exit(f"REFUSE: {name} produced no diff")
    subject = f"scratch(1357): DROP ME - red {name}: {label}"
    if len(subject) > 72:
        sys.exit(f"REFUSE: subject {len(subject)} chars: {subject}")
    git("commit", "-q", "-am", subject)
    print(name, git("rev-parse", "--short", "HEAD").strip(), numstat)
print("status:", repr(git("status", "--porcelain")))
