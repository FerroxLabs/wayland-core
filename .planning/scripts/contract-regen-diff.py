#!/usr/bin/env python3
"""Classify a Desktop-contract corpus regeneration: digest refresh vs wire-shape change.

The corpus fixtures are ONE LINE of JSON each, so `git diff` reports the identical
`1 insertion, 1 deletion` whether the regeneration merely re-stamped a digest or
silently changed a wire field. That is the failure mode this exists to prevent:
a regeneration that absorbs a real shape change is far worse than a red test.

Method: parse both revisions as JSON, flatten to leaf paths, and compare. A
regeneration is DIGEST-ONLY iff every differing/added/removed leaf path ends in a
known digest field. Anything else is a SHAPE CHANGE and must stop the lane.

Usage:
    contract-regen-diff.py <old-rev> <path> [<path> ...]   # old-rev vs working tree
    contract-regen-diff.py --selftest
"""

import hashlib
import json
import subprocess
import sys
from pathlib import Path

GIT = "/usr/bin/git"  # the rtk proxy filters bare `git`; never use bare git here

# Leaf names whose value is a content digest derived from other files. A change
# confined to these is a re-stamp, not a wire change.
DIGEST_LEAVES = {
    "fixture_digest",
    "schema_digest",
    "source_inputs_digest",
}


def flatten(value, prefix=""):
    """Flatten JSON to {dotted-path: scalar}. Arrays index by position."""
    out = {}
    if isinstance(value, dict):
        for k, v in value.items():
            out.update(flatten(v, f"{prefix}.{k}" if prefix else k))
    elif isinstance(value, list):
        for i, v in enumerate(value):
            out.update(flatten(v, f"{prefix}[{i}]"))
    else:
        out[prefix] = value
    return out


def load_lines(raw: bytes):
    """A .jsonl is N JSON docs; a .json here is one. Flatten each, namespaced by line."""
    flat = {}
    for i, line in enumerate(raw.decode().splitlines()):
        if not line.strip():
            continue
        flat.update(flatten(json.loads(line), f"[line{i}]"))
    return flat


def classify(old_raw: bytes, new_raw: bytes):
    """Return (verdict, digest_changes, shape_changes)."""
    old, new = load_lines(old_raw), load_lines(new_raw)
    digest, shape = [], []
    for path in sorted(set(old) | set(new)):
        o, n = old.get(path, "<absent>"), new.get(path, "<absent>")
        if o == n:
            continue
        leaf = path.rsplit(".", 1)[-1]
        (digest if leaf in DIGEST_LEAVES else shape).append((path, o, n))
    verdict = "DIGEST-ONLY" if not shape else "SHAPE CHANGE"
    if not digest and not shape:
        verdict = "IDENTICAL"
    return verdict, digest, shape


def read_at(root: Path, rev, rel):
    return subprocess.run([GIT, "-C", str(root), "show", f"{rev}:{rel}"],
                          check=True, capture_output=True).stdout


def selftest():
    """Three assertions (LANE-BRIEF 6b-ii); A3 is the load-bearing one."""
    results = []

    base = {"type": "ready", "contract": {
        "source_inputs_digest": "sha256:" + "a" * 64,
        "fixture_digest": "sha256:" + "b" * 64,
        "major": 1, "minor": 8},
        "capabilities": {"mcp": True}}

    # A1 known-positive: a pure digest re-stamp classifies DIGEST-ONLY.
    restamped = json.loads(json.dumps(base))
    restamped["contract"]["source_inputs_digest"] = "sha256:" + "c" * 64
    v1, d1, s1 = classify(json.dumps(base).encode(), json.dumps(restamped).encode())
    a1 = v1 == "DIGEST-ONLY" and len(d1) == 1 and not s1
    results.append(("A1 known-positive: a digest-only re-stamp classifies DIGEST-ONLY", a1))

    # A2 known-negative: a real wire change must classify SHAPE CHANGE, and must
    # do so even when a digest ALSO moved (the realistic case -- a shape change
    # always drags the digests with it, so it must not be masked by them).
    shaped = json.loads(json.dumps(base))
    shaped["contract"]["source_inputs_digest"] = "sha256:" + "c" * 64
    shaped["contract"]["minor"] = 9          # a wire-visible field moved
    shaped["capabilities"]["new_flag"] = True  # and a field appeared
    v2, d2, s2 = classify(json.dumps(base).encode(), json.dumps(shaped).encode())
    a2 = (v2 == "SHAPE CHANGE"
          and {p for p, _, _ in s2} == {"[line0].contract.minor",
                                        "[line0].capabilities.new_flag"}
          and len(d2) == 1)
    results.append(("A2 known-negative: a wire change classifies SHAPE CHANGE even when "
                    "a digest moved in the same edit", a2))

    # A3 THE LOAD-BEARING ONE. The instrument this replaces is `git diff` line
    # counting. Prove the OLD method CANNOT tell A1 from A2: because these
    # fixtures are single-line JSON, both edits produce byte-different content
    # that git reports identically as one line replaced. If the old method could
    # distinguish them, this script would be redundant -- so this assertion is
    # what proves the repair does anything at all.
    def numstat(a: bytes, b: bytes):
        import tempfile, os
        d = tempfile.mkdtemp()
        subprocess.run([GIT, "-C", d, "init", "-q"], check=True)
        f = Path(d) / "fixture.jsonl"
        f.write_bytes(a + b"\n")
        subprocess.run([GIT, "-C", d, "add", "fixture.jsonl"], check=True)
        subprocess.run([GIT, "-C", d, "-c", "user.email=t@t", "-c", "user.name=t",
                        "commit", "-qm", "base"], check=True)
        f.write_bytes(b + b"\n")
        return subprocess.run([GIT, "-C", d, "diff", "--numstat"],
                              capture_output=True, text=True).stdout.strip()

    old_on_restamp = numstat(json.dumps(base).encode(), json.dumps(restamped).encode())
    old_on_shape = numstat(json.dumps(base).encode(), json.dumps(shaped).encode())
    a3 = (old_on_restamp == old_on_shape == "1\t1\tfixture.jsonl"
          and v1 != v2)
    results.append(("A3 the OLD method (`git diff --numstat`) reports an IDENTICAL "
                    f"'{old_on_restamp}' for both, i.e. would have missed the shape "
                    "change; this script separates them", a3))

    for name, ok in results:
        print(f"[{'ok' if ok else 'FAIL'}] {name}")
    p = sum(1 for _, ok in results if ok)
    print(f"{p} passed, {len(results) - p} failed")
    return 0 if p == len(results) else 1


def main(argv):
    if "--selftest" in argv:
        return selftest()
    if len(argv) < 3:
        print(__doc__)
        return 2
    root = Path(__file__).resolve().parents[2]
    rev, paths = argv[1], argv[2:]
    overall = "DIGEST-ONLY"
    for rel in paths:
        old = read_at(root, rev, rel)
        new = (root / rel).read_bytes()
        verdict, digest, shape = classify(old, new)
        print(f"\n=== {rel}")
        print(f"    bytes {len(old)} -> {len(new)}")
        print(f"    sha256 {hashlib.sha256(old).hexdigest()[:16]} -> "
              f"{hashlib.sha256(new).hexdigest()[:16]}")
        print(f"    VERDICT {verdict}  ({len(digest)} digest leaves, "
              f"{len(shape)} shape leaves)")
        for path, o, n in digest:
            print(f"      digest {path}\n         {o}\n      -> {n}")
        for path, o, n in shape:
            print(f"      SHAPE! {path}\n         {o!r}\n      -> {n!r}")
        if shape:
            overall = "SHAPE CHANGE"
    print(f"\nOVERALL {overall}")
    return 0 if overall == "DIGEST-ONLY" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
