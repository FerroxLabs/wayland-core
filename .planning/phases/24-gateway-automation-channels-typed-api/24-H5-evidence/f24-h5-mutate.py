#!/usr/bin/env python3
"""F24-C3-H5 mutation harness — proves the new gates can FAIL, and proves
WHICH of them the half-fix would have slipped past.

Run on hetzner, inside the build worktree:

    python3 f24-h5-mutate.py M1
    python3 f24-h5-mutate.py M2
    python3 f24-h5-mutate.py restore     # idempotent safety net

Two mutations, because one is not enough to make the point:

  M1  "the bug"      — `replace` never swaps. Models the pre-fix runtime, where
                       both maps were captured at spawn and unreachable
                       afterwards. Every reload assertion must redden.

  M2  "the half-fix" — `replace` swaps the POLICIES ONLY and keeps the stale
                       postures. This is the trap the finding lane named
                       explicitly: it makes messages ARRIVE, so an
                       arrivals-only test goes green while the channel runs
                       under the wrong permissions. Under M2 the subscriber
                       arrival test is EXPECTED TO PASS and the posture
                       assertions are EXPECTED TO FAIL. That divergence is the
                       whole evidence: it is what proves the acceptance test
                       asserts posture rather than arrival.

Counting discipline: `cargo test` exits 0 while running ZERO tests (measured on
this repo — a filter that matches no test name, and suites that are entirely
`#[ignore]`d). So this harness parses the `test result:` line and reports the
EXECUTED counts; exit status is recorded but never trusted on its own.
"""

import json
import os
import pathlib
import subprocess
import sys

WORKTREE = os.environ.get("F24_H5_WORKTREE", "/root/wayland-24-c3-finish")
SRC = pathlib.Path(WORKTREE) / "crates/wcore-agent/src/channel_policy.rs"

ORIG = """    pub fn replace(&self, snapshot: ChannelPolicySnapshot) -> usize {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let generation = guard.generation + 1;
        *guard = ChannelPolicySnapshot {
            generation,
            ..snapshot
        };
        guard.policies.len()
    }"""

M1 = """    pub fn replace(&self, snapshot: ChannelPolicySnapshot) -> usize {
        // MUTATION M1: the pre-fix runtime. The reload never reaches the maps.
        snapshot.policies.len()
    }"""

M2 = """    pub fn replace(&self, snapshot: ChannelPolicySnapshot) -> usize {
        // MUTATION M2: the HALF-FIX. Policies swap; postures are kept stale.
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let generation = guard.generation + 1;
        let postures = guard.postures.clone();
        *guard = ChannelPolicySnapshot {
            generation,
            postures,
            ..snapshot
        };
        guard.policies.len()
    }"""

SUITES = [
    ("unit-registry", ["--lib", "channel_policy::"]),
    ("unit-subscriber-arrivals", ["--lib", "channel_inbound::tests::a_registry"]),
    ("unit-dispatcher-posture", ["--lib", "channel_dispatch::tests::a_"]),
    ("integration-reload", ["--test", "f24_c3_h5_reload_policies_test"]),
]


def run(label, args):
    cmd = ["cargo", "test", "-p", "wcore-agent", *args, "--no-fail-fast", "--", "--test-threads=1"]
    p = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        cwd=WORKTREE,
        env={"PATH": "/root/.cargo/bin:/usr/bin:/bin", "HOME": "/root"},
    )
    out = p.stdout + p.stderr
    passed = failed = 0
    for line in out.splitlines():
        if line.startswith("test result:"):
            parts = line.replace(";", "").split()
            passed += int(parts[parts.index("passed") - 1])
            failed += int(parts[parts.index("failed") - 1])
    failed_names = sorted(
        {
            ln.split()[1]
            for ln in out.splitlines()
            if ln.startswith("test ") and ln.rstrip().endswith("FAILED")
        }
    )
    return {
        "suite": label,
        "rc": p.returncode,
        "executed": passed + failed,
        "passed": passed,
        "failed": failed,
        "failed_tests": failed_names,
        "output_bytes": len(out),
    }


def apply(body):
    text = SRC.read_text()
    n = text.count(ORIG)
    if n != 1:
        raise SystemExit(f"REFUSING: anchor found {n} times, expected exactly 1")
    SRC.write_text(text.replace(ORIG, body))


def restore():
    text = SRC.read_text()
    for m in (M1, M2):
        if m in text:
            SRC.write_text(text.replace(m, ORIG))
            return "restored"
    if ORIG in text:
        return "already-original"
    raise SystemExit("REFUSING: source is neither mutated nor original")


if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else "restore"
    if which == "restore":
        print(restore())
        sys.exit(0)
    apply({"M1": M1, "M2": M2}[which])
    results = []
    try:
        for label, args in SUITES:
            results.append(run(label, args))
    finally:
        restore()
    print(json.dumps({"mutation": which, "results": results}, indent=2))
