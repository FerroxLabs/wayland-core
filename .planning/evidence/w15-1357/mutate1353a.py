#!/usr/bin/env python3
"""Cut wayland#1353 review red arms: each is a5124fd43 plus ONE ordering mutation.

Every mutation must land exactly once, on code, or the script refuses.
SCRATCH ONLY: none of these branches may be merged.
"""
import pathlib
import subprocess
import sys
import time

WT = pathlib.Path.home() / "dev/waylandcore-stabilization-20260905/worktrees/w15-quota1357-red"
FILE = WT / "crates/wcore-agent/src/session_journal.rs"
BASE = sys.argv[1]

SNAPSHOT = "        let mut admission = CheckpointAdmission::before_scan(&self.checkpoint_quota);\n"
SCAN = "        let session_bytes = scan_checkpoint_quota(directory)?;\n"
ADMIT = "        admission.admit(session_bytes, contents.len() as u64)?;\n"
LINK = "            match std::fs::hard_link(&temporary, &path) {\n"
RELEASE = "        drop(admission);\n"


def git(*args):
    for attempt in range(3):
        done = subprocess.run(["/usr/bin/git", "-C", str(WT), *args], capture_output=True, text=True)
        if done.returncode == 0:
            return done.stdout
        print(f"git {args[0]} attempt {attempt + 1} rc={done.returncode}: {done.stderr.strip()}")
        time.sleep(2)
    sys.exit(f"REFUSE: git {' '.join(args)} failed")


def once(src, needle):
    if src.count(needle) != 1:
        sys.exit(f"REFUSE: {needle!r} occurs {src.count(needle)} times")


def snapshot_after_scan(src):  # (a)
    once(src, SNAPSHOT + SCAN)
    return src.replace(SNAPSHOT + SCAN, SCAN + SNAPSHOT)


def release_before_link(src):  # (b) literal: after the write and sync, before hard_link
    once(src, RELEASE)
    once(src, LINK)
    src = src.replace(RELEASE, "")
    return src.replace(LINK, "            drop(admission);\n" + LINK)


def release_before_write(src):  # (b') right after admission
    once(src, RELEASE)
    once(src, ADMIT)
    src = src.replace(RELEASE, "")
    return src.replace(ADMIT, ADMIT + "        drop(admission);\n")


MUTATIONS = [
    ("a", snapshot_after_scan, "snapshot taken after the scan"),
    ("b", release_before_link, "reservation ends before hard_link"),
    ("b2", release_before_write, "release before the write"),
]

skip = set(sys.argv[2:])
for name, mutate, label in MUTATIONS:
    if name in skip:
        continue
    git("checkout", "-q", "-B", f"w15/quota1353-review-red-{name}", BASE)
    src = FILE.read_text()
    new = mutate(src)
    if new == src:
        sys.exit(f"REFUSE: {name} produced no change")
    FILE.write_text(new)
    FILE.touch()
    numstat = git("diff", "--numstat").strip()
    if not numstat:
        sys.exit(f"REFUSE: {name} produced no diff")
    subject = f"scratch(1353): DROP ME - review red {name}: {label}"
    if len(subject) > 72:
        sys.exit(f"REFUSE: subject {len(subject)} chars")
    git("commit", "-q", "-am", subject)
    print(name, git("rev-parse", "--short", "HEAD").strip(), numstat)
print("status:", repr(git("status", "--porcelain")))
