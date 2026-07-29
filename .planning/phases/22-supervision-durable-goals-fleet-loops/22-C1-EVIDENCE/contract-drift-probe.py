#!/usr/bin/env python3
"""Measure Desktop producer-contract drift WITHOUT running `wcore-contract generate`.

Lanes are forbidden to regenerate the corpus, but a lane that edits `wcore-protocol`
still needs to know, factually, whether it drifted the contract and by how much. This
reimplements the two digests that `contract/generate.rs` computes, from the same inputs,
and diffs them against the recorded `manifest.json`.

Why this is not a self-passing gate: it reproduces `schema_digest` from on-disk bytes and
asserts it EQUALS the value the Rust generator recorded. If the reimplementation of
`digest_named_bytes` were wrong, that assertion fails. It is therefore an instrument
whose own correctness is proved by a value it did not choose.

Run:  python3 contract-drift-probe.py [--self-test]
Exit: 0 = no drift, 3 = drift, 4 = instrument self-test failed.
"""

import hashlib
import json
import os
import re
import subprocess
import sys

CORPUS = "crates/wcore-protocol/contracts/desktop/v1"
SPEC = "crates/wcore-protocol/src/contract/spec.rs"


def digest_named_bytes(entries):
    """Port of contract/canonical.rs::digest_named_bytes.

    The `path + NUL + bytes` framing is load-bearing, not decoration: without the
    separator the boundary between a path and its content is ambiguous, so moving a
    byte across that boundary produces the SAME digest. assertion 3 proves it.
    """
    h = hashlib.sha256()
    for path, data in sorted(entries, key=lambda e: e[0]):
        h.update(path.encode())
        h.update(b"\0")
        h.update(data)
    return "sha256:" + h.hexdigest()


def _naive_digest_named_bytes(entries):
    """The shape this instrument would plausibly have been written as: no NUL framing.

    Kept ONLY so assertion 3 can demonstrate what it misses. Never used to measure.
    """
    h = hashlib.sha256()
    for path, data in sorted(entries, key=lambda e: e[0]):
        h.update(path.encode())
        h.update(data)
    return "sha256:" + h.hexdigest()


def source_inputs(root):
    spec = open(os.path.join(root, SPEC), encoding="utf-8").read()
    match = re.search(r"pub const SOURCE_INPUTS: &\[&str\] = &\[(.*?)\n\];", spec, re.S)
    if match is None:
        raise SystemExit("could not parse SOURCE_INPUTS out of spec.rs")
    return re.findall(r'"([^"]+)"', match.group(1))


def schema_entries(root):
    entries = []
    base = os.path.join(root, CORPUS)
    for dirpath, _, files in os.walk(os.path.join(base, "schema")):
        for name in files:
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, base).replace("\\", "/")
            entries.append((rel, open(full, "rb").read()))
    return entries


def measure(root):
    manifest = json.load(open(os.path.join(root, CORPUS, "manifest.json"), encoding="utf-8"))

    schema_computed = digest_named_bytes(schema_entries(root))

    paths = source_inputs(root)
    missing = [p for p in paths if not os.path.exists(os.path.join(root, p))]
    src_entries = [
        (p, open(os.path.join(root, p), "rb").read()) for p in paths if p not in missing
    ]
    src_computed = digest_named_bytes(src_entries)

    return {
        "source_inputs_count": len(paths),
        "source_inputs_missing": missing,
        "schema_digest_computed": schema_computed,
        "schema_digest_recorded": manifest["schema_digest"],
        "schema_digest_match": schema_computed == manifest["schema_digest"],
        "source_inputs_digest_computed": src_computed,
        "source_inputs_digest_recorded": manifest["source_inputs_digest"],
        "source_inputs_digest_match": src_computed == manifest["source_inputs_digest"],
        "event_spec_count_recorded": len(manifest["events"]),
        "command_spec_count_recorded": len(manifest["commands"]),
    }


def drifted_source_inputs(root):
    """Which SOURCE_INPUTS changed since the corpus was last regenerated."""
    last = subprocess.run(
        ["git", "-C", root, "log", "-1", "--format=%H", "--",
         os.path.join(CORPUS, "manifest.json")],
        capture_output=True, text=True, check=True).stdout.strip()
    if not last:
        return None, []
    names = subprocess.run(
        ["git", "-C", root, "diff", "--name-only", last, "HEAD", "--"] + source_inputs(root),
        capture_output=True, text=True, check=True).stdout.split()
    return last, names


def self_test(root):
    """Three assertions. The third is the only one that proves the repair does anything."""
    results = []

    # 1. KNOWN-POSITIVE: the reimplementation reproduces a digest the Rust generator
    #    wrote. If the port were wrong this fails, so the instrument cannot silently
    #    measure with a broken hash.
    manifest = json.load(open(os.path.join(root, CORPUS, "manifest.json"), encoding="utf-8"))
    entries = schema_entries(root)
    a1 = digest_named_bytes(entries) == manifest["schema_digest"]
    results.append(("1 known-positive: schema_digest reproduces the generator's value", a1))

    # 2. KNOWN-NEGATIVE: a single flipped byte in a schema file MUST change the digest.
    #    Without this the instrument could be a constant function and assertion 1 would
    #    still pass by luck.
    mutated = [(entries[0][0], entries[0][1] + b" ")] + entries[1:]
    a2 = digest_named_bytes(mutated) != manifest["schema_digest"]
    results.append(("2 known-negative: a one-byte edit changes the digest", a2))

    # 3. THE OLD/NAIVE SHAPE WOULD HAVE MISSED IT: drop the NUL framing and a byte moved
    #    across the path/content boundary becomes invisible. Two genuinely different
    #    corpora collide under the naive digest while the real one separates them.
    left = [("events/ready.json", b"X")]
    right = [("events/ready.jsonX", b"")]
    naive_collides = _naive_digest_named_bytes(left) == _naive_digest_named_bytes(right)
    framed_separates = digest_named_bytes(left) != digest_named_bytes(right)
    a3 = naive_collides and framed_separates
    results.append(
        ("3 old-shape-misses: unframed digest collides on a path/content byte move; framed does not", a3))

    for label, ok in results:
        print(f"  [{'PASS' if ok else 'FAIL'}] assertion {label}")
    return all(ok for _, ok in results)


def main():
    root = subprocess.run(["git", "rev-parse", "--show-toplevel"],
                          capture_output=True, text=True, check=True).stdout.strip()

    if "--self-test" in sys.argv:
        print(f"instrument self-test (repo {root}):")
        ok = self_test(root)
        print("SELF-TEST:", "PASS" if ok else "FAIL")
        return 0 if ok else 4

    m = measure(root)
    print(json.dumps(m, indent=2))
    last, names = drifted_source_inputs(root)
    print(f"\nlast corpus regeneration: {last}")
    print(f"SOURCE_INPUTS drifted since then: {len(names)} of {m['source_inputs_count']}")
    for n in names:
        print("  ", n)

    if m["schema_digest_match"] and m["source_inputs_digest_match"]:
        print("\nVERDICT: no corpus drift.")
        return 0
    print("\nVERDICT: CORPUS DRIFT — a `wcore-contract generate` is owed. "
          "This lane does not run it (LANE-BRIEF §0).")
    return 3


if __name__ == "__main__":
    sys.exit(main())
