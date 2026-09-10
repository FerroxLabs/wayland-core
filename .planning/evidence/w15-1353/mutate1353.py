#!/usr/bin/env python3
"""Cut wayland#1353 red-arm branches: each is a8fd1ac9c plus ONE mutation.

Every mutation must land exactly once, on code (not a comment), or the script
refuses. SCRATCH ONLY: none of these branches may be merged.
"""
import pathlib
import re
import subprocess
import sys
import time

WT = pathlib.Path.home() / "dev/waylandcore-stabilization-20260905/worktrees/w15-quota1353-red"
FILE = WT / "crates/wcore-agent/src/session_journal.rs"
BASE = "a8fd1ac9c"


def git(*args):
    # The worktree shares refs and objects with lanes running remote-proof
    # bundles, so a transient lock can fail one call: retry, and show stderr.
    for attempt in range(3):
        done = subprocess.run(["/usr/bin/git", "-C", str(WT), *args],
                              capture_output=True, text=True)
        if done.returncode == 0:
            return done.stdout
        print(f"git {args[0]} attempt {attempt + 1} rc={done.returncode}: {done.stderr.strip()}")
        time.sleep(2)
    sys.exit(f"REFUSE: git {' '.join(args)} failed")


SKIP = set(sys.argv[1:])


def replace_once(src, old, new):
    count = src.count(old)
    if count != 1:
        sys.exit(f"REFUSE: {old!r} occurs {count} times")
    line = src[: src.index(old)].rsplit("\n", 1)[-1]
    if line.lstrip().startswith("//"):
        sys.exit(f"REFUSE: {old!r} is in a comment")
    return src.replace(old, new)


def m1(src):  # repair removed: stores the scan missed are never counted
    return re.subn(r"let unseen = ledger[^;]*;", "let unseen = 0_u64;", src, count=1)


def m2(src):  # reservation leaks: Drop never ends it
    return replace_once(src, "if self.reserved == 0 {", "if self.reserved == 0 || self.reserved != 0 {"), 1


def m3(src):  # ledger shared by every session in the process
    return replace_once(
        src,
        "checkpoint_quota: Arc::default(),",
        "checkpoint_quota: {\n                static SHARED: std::sync::OnceLock<Arc<Mutex<CheckpointQuota>>> =\n"
        "                    std::sync::OnceLock::new();\n                Arc::clone(SHARED.get_or_init(Arc::default))\n            },",
    ), 1


def m4(src):  # released counted absolutely instead of since this store's scan
    return replace_once(src, "wrapping_sub(self.released_before_scan)", "wrapping_sub(0)"), 1


def m5(src):  # crash-left temporaries not counted by the quota scan
    start = src.index("fn checkpoint_directory_bytes(directory: &Path)")
    anchor = "let path = entry.path();"
    at = src.index(anchor, start) + len(anchor)
    return src[:at] + "\n        if path.to_string_lossy().ends_with(\".tmp\") {\n            continue;\n        }" + src[at:], 1


def m6(src):  # one process-wide lock held from before the scan through the write
    anchor = "let mut admission = CheckpointAdmission::before_scan(&self.checkpoint_quota);"
    return replace_once(
        src, anchor,
        "static SERIAL: Mutex<()> = Mutex::new(());\n        let _serial = SERIAL.lock().unwrap();\n        " + anchor,
    ), 1


MUTATIONS = [
    ("m1", m1, "repair removed, unseen stores never counted"),
    ("m2", m2, "reservation never released"),
    ("m3", m3, "ledger shared across sessions"),
    ("m4", m4, "released bytes counted absolutely"),
    ("m5", m5, "crash-left temporaries not counted"),
    ("m6", m6, "global lock across scan and write"),
]

for name, mutate, label in MUTATIONS:
    if name in SKIP:
        continue
    git("checkout", "-q", "-B", f"w15/quota1353-red-{name}", BASE)
    src = FILE.read_text()
    new, count = mutate(src)
    if count != 1 or new == src:
        sys.exit(f"REFUSE: {name} did not land ({count})")
    FILE.write_text(new)
    FILE.touch()
    numstat = git("diff", "--numstat")
    if not numstat.strip():
        sys.exit(f"REFUSE: {name} produced no diff")
    git("commit", "-q", "-am", f"scratch(1353): DROP ME - red arm {name}: {label}")
    print(name, git("rev-parse", "--short", "HEAD").strip(), numstat.strip())
git("checkout", "-q", "w15/quota1353-red-m1")
print("status:", repr(git("status", "--porcelain")))
