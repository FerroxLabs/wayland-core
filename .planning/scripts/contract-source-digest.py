#!/usr/bin/env python3
"""Recompute the Desktop contract `source_inputs_digest` outside cargo.

Mirrors `wcore_protocol::contract::generate::source_digest()`, which is
`digest_named_bytes()` over SOURCE_INPUTS:

    sort entries by path; for each: sha256.update(path); update(b"\\x00"); update(bytes)

This exists so the digest can be evaluated at any git revision on a machine that
cannot run cargo (the Mac), and so a claim about *which* commit moved the digest
is a measurement rather than an inference from `git diff`.

Usage:
    contract-source-digest.py [<git-rev>]     # omit rev to read the working tree
    contract-source-digest.py --selftest
"""

import hashlib
import re
import subprocess
import sys
from pathlib import Path

GIT = "/usr/bin/git"  # the rtk proxy silently filters `git log`; never use bare git here


def source_inputs(root: Path):
    spec = (root / "crates/wcore-protocol/src/contract/spec.rs").read_text()
    m = re.search(r"pub const SOURCE_INPUTS: &\[&str\] = &\[(.*?)\];", spec, re.S)
    if not m:
        raise SystemExit("could not locate SOURCE_INPUTS in spec.rs")
    return re.findall(r'"([^"]+)"', m.group(1))


def digest_named_bytes(entries):
    h = hashlib.sha256()
    for path, data in sorted(entries, key=lambda e: e[0]):
        h.update(path.encode())
        h.update(b"\x00")
        h.update(data)
    return "sha256:" + h.hexdigest()


def read_at(root: Path, rev, rel):
    if rev is None:
        return (root / rel).read_bytes()
    return subprocess.run([GIT, "-C", str(root), "show", f"{rev}:{rel}"],
                          check=True, capture_output=True).stdout


def compute(root: Path, rev=None):
    names = source_inputs(root)
    return digest_named_bytes([(n, read_at(root, rev, n)) for n in names]), names


def pinned(root: Path, rev=None):
    import json
    raw = read_at(root, rev, "crates/wcore-protocol/contracts/desktop/v1/manifest.json")
    return json.loads(raw)["source_inputs_digest"]


def selftest():
    """Three assertions (LANE-BRIEF 6b-ii)."""
    results = []

    # A1 known-positive: the documented digest construction reproduces a value
    # computed by hand for a fixed pair of entries.
    h = hashlib.sha256()
    h.update(b"a"); h.update(b"\x00"); h.update(b"AAA")
    h.update(b"b"); h.update(b"\x00"); h.update(b"BBB")
    expect = "sha256:" + h.hexdigest()
    a1 = digest_named_bytes([("b", b"BBB"), ("a", b"AAA")]) == expect
    results.append(("A1 known-positive: digest is order-independent and matches "
                    "the path\\0bytes construction", a1))

    # A2 known-negative: a single changed byte must move the digest.
    a2 = digest_named_bytes([("a", b"AAA")]) != digest_named_bytes([("a", b"AAB")])
    results.append(("A2 known-negative: one changed byte moves the digest", a2))

    # A3 THE LOAD-BEARING ONE. The old way of answering "did a source input
    # change?" was `git diff <rev> HEAD -- <paths>`, which returns EMPTY and rc=0
    # both when nothing changed and when the path list is wrong. Prove the two
    # are indistinguishable to that method, and that this script is not: it reads
    # the paths out of spec.rs and would raise on a missing one.
    old_answer_real = subprocess.run(
        [GIT, "-C", str(Path(__file__).resolve().parents[2]), "diff", "--name-only",
         "HEAD", "HEAD", "--", "crates/wcore-protocol/src/events.rs"],
        capture_output=True, text=True)
    old_answer_typo = subprocess.run(
        [GIT, "-C", str(Path(__file__).resolve().parents[2]), "diff", "--name-only",
         "HEAD", "HEAD", "--", "crates/wcore-protocol/src/eventz.rs"],
        capture_output=True, text=True)
    indistinguishable = (old_answer_real.stdout == old_answer_typo.stdout == ""
                         and old_answer_real.returncode == old_answer_typo.returncode == 0)
    raised = False
    try:
        read_at(Path(__file__).resolve().parents[2], None, "crates/wcore-protocol/src/eventz.rs")
    except Exception:
        raised = True
    a3 = indistinguishable and raised
    results.append(("A3 the OLD method (`git diff -- <paths>`) gives an identical empty/rc=0 "
                    "answer for 'unchanged' and for a mistyped path; this script raises instead", a3))

    for name, ok in results:
        print(f"[{'ok' if ok else 'FAIL'}] {name}")
    p = sum(1 for _, ok in results if ok)
    print(f"{p} passed, {len(results) - p} failed")
    return 0 if p == len(results) else 1


def main(argv):
    if "--selftest" in argv:
        return selftest()
    root = Path(__file__).resolve().parents[2]
    rev = argv[1] if len(argv) > 1 else None
    got, names = compute(root, rev)
    want = pinned(root, rev)
    label = rev or "<working tree>"
    print(f"rev              {label}")
    print(f"source inputs    {len(names)}")
    print(f"computed         {got}")
    print(f"pinned in manifest {want}")
    print(f"MATCH            {got == want}")
    return 0 if got == want else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
