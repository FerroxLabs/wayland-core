#!/usr/bin/env python3
"""Mutate the F24-C3-H6 fix out of the product and prove the gates redden.

Three arms, because the finding has two facets and the interesting arm is the one
that fixes only ONE of them:

  M1  "the bug"        — the lease degradation is dropped from the composed
                         health error, so a reload's clear reaches it again.
  M2  "the HALF-FIX"   — H6a repaired (health keeps reporting) but H6b NOT: the
                         reload still starts poll tasks it has no lease for. This
                         is the arm that matters. Every exit-code leg goes GREEN
                         while the gateway silently steals a destructive read, so
                         a driver that only read `channel health`'s exit status —
                         the obvious shape, and the shape I first wrote — would
                         have graded this build as fully fixed.
  M3  "posture only"   — H6b repaired, H6a not. The mirror of M2, included so the
                         two facets are shown to be independently detected rather
                         than one assertion happening to cover both.

Restores every file in a `finally:`, so an interrupted run does not leave the
tree mutated.

Usage:  f24-c3-h6-mutate.py <arm: M1|M2|M3> <out.json>
"""

import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
GATEWAY = ROOT / "crates/wcore-cli/src/gateway.rs"

# Each arm: list of (file, find, replace). `find` must appear exactly once, or the
# harness aborts -- a mutation that silently applied to nothing would make the
# arm report "gates still green" and read as the tests being weak.
ARMS = {
    # Drop the live lease component from the composed error. The reload's clear
    # of the registration component then leaves nothing reporting the dead path.
    "M1": [
        (
            GATEWAY,
            '    if not_polling {\n        parts.push("inbound polling owned by another process".to_string());\n    }\n',
            "",
        )
    ],
    # Keep H6a. Break H6b: reload starts poll tasks regardless of the lease.
    "M2": [
        (
            GATEWAY,
            "let start_policy = if poll_supervisor.is_owner() {",
            "let start_policy = if true {",
        )
    ],
    # Keep H6b. Break H6a the way the original did: clear the whole composed
    # error on a successful reload by emptying the persistent component.
    "M3": [
        (
            GATEWAY,
            "    if let Some(e) = inbound_absent {\n        parts.push(e.clone());\n    }\n    if not_polling {\n        parts.push(\"inbound polling owned by another process\".to_string());\n    }\n",
            "",
        )
    ],
}

SUITES = [
    (
        "gateway-unit (H6a composition)",
        ["cargo", "test", "-p", "wcore-cli", "--lib", "gateway::tests::"],
    ),
]


def run(cmd):
    p = subprocess.run(
        cmd, cwd=ROOT, capture_output=True, text=True,
        env={"PATH": "/root/.cargo/bin:/usr/bin:/bin", "HOME": "/root"},
    )
    return p.returncode, p.stdout + p.stderr


def counts(out):
    """Read the executed counts back rather than trusting exit status.

    LANE-BRIEF 3.2: a suite exits 0 having run zero tests when a filter matches
    no name, so `passed` alone is not evidence -- `filtered out` is reported too.
    """
    for line in out.splitlines():
        if line.startswith("test result:"):
            got = {}
            for field in line.replace("test result:", "").split(";"):
                field = field.strip().rstrip(".")
                parts = field.split(" ", 1)
                if len(parts) == 2 and parts[0].lstrip("-").isdigit():
                    got[parts[1].strip()] = int(parts[0])
                elif field.startswith(("ok", "FAILED")):
                    got["verdict"] = field
            return got
    return {"verdict": "NO test result LINE — suite did not run"}


def main():
    arm = sys.argv[1]
    out_path = pathlib.Path(sys.argv[2])
    if arm not in ARMS:
        sys.exit(f"unknown arm {arm}; choose from {sorted(ARMS)}")

    originals = {}
    result = {"arm": arm, "suites": {}}
    try:
        for path, find, repl in ARMS[arm]:
            text = path.read_text()
            originals.setdefault(path, text)
            n = text.count(find)
            if n != 1:
                sys.exit(
                    f"ABORT: mutation anchor found {n} times in {path.name}, "
                    f"expected exactly 1. The product has moved; fix the anchor "
                    f"rather than reporting a green."
                )
            path.write_text(text.replace(find, repl))
        result["mutation_applied"] = True

        for name, cmd in SUITES:
            rc, out = run(cmd)
            result["suites"][name] = {"rc": rc, **counts(out)}
            failing = [
                ln.strip()
                for ln in out.splitlines()
                if ln.startswith("test ") and " ... FAILED" in ln
            ]
            result["suites"][name]["failed_tests"] = failing
    finally:
        for path, text in originals.items():
            path.write_text(text)
        result["restored"] = True

    out_path.write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
