#!/usr/bin/env python3
"""Fail the release if a Linux artifact requires a newer glibc than we promise.

WHY THIS EXISTS
---------------
The glibc floor of a shipped binary is a property of the BUILD CONTAINER, not of
the source. Nothing in the repository pins it, so a runner-image bump silently
raises it and the first symptom is a user on a supported distro getting

    ./wayland-core: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found

Measured 2026-07-30: `ubuntu-latest` had moved to 24.04 (glibc 2.39), so the
published Linux binaries could not execute on Ubuntu 22.04 (2.35), Debian 12
(2.36) or RHEL 9 / Rocky 9 / Amazon Linux 2023 (2.34) — i.e. on most deployed
server Linux. This gate turns that from a customer report into a failed release
step.

THE OBVIOUS IMPLEMENTATION IS WRONG
-----------------------------------
The natural one-liner is

    readelf -V bin | grep -o 'GLIBC_[0-9.]*' | sort -u | tail -1

`sort -u` is LEXICOGRAPHIC, so it ranks GLIBC_2.9 above GLIBC_2.39 ("9" > "3")
and reports a 2.39 binary as needing 2.9 — the gate then passes every floor.
Versions here are compared as integer tuples. `--self-test` asserts exactly this
difference, so the self-test cannot pass on the broken implementation.

ABSENCE IS NOT A PASS
---------------------
Finding zero GLIBC_* references is what a wrong path, a stripped/static binary, a
non-ELF file and a broken readelf all produce for free. Each would silently
satisfy any floor. So zero references is a FAILURE, not a pass.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

SYMBOL_RE = re.compile(r"GLIBC_(\d+(?:\.\d+)+)")


def parse_version(text: str) -> tuple[int, ...]:
    """'2.34' -> (2, 34). Compared component-wise as integers, never as text."""
    return tuple(int(part) for part in text.split("."))


def extract_versions(readelf_output: str) -> set[tuple[int, ...]]:
    return {parse_version(m) for m in SYMBOL_RE.findall(readelf_output)}


def format_version(version: tuple[int, ...]) -> str:
    return ".".join(str(part) for part in version)


def read_symbols(binary: str) -> str:
    proc = subprocess.run(
        ["readelf", "--version-info", "--dyn-syms", binary],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise SystemExit(f"::error::readelf failed on {binary}: {proc.stderr.strip()}")
    return proc.stdout


def check(binary: str, max_allowed: str) -> int:
    versions = extract_versions(read_symbols(binary))

    # Anti-vacuity: a glibc-linked binary ALWAYS references GLIBC_2.2.5 (x86_64)
    # or GLIBC_2.17 (aarch64). Zero references means the measurement did not
    # happen, which must never read as compliance.
    if not versions:
        print(
            f"::error::{binary} yielded ZERO GLIBC_* references. A glibc-linked "
            "binary always has some, so this is a broken measurement (wrong path, "
            "non-ELF, or a static build), not a pass."
        )
        return 1

    ceiling = parse_version(max_allowed)
    over = sorted(v for v in versions if v > ceiling)
    actual = max(versions)

    print(f"binary        : {binary}")
    print(f"symbols found : {len(versions)}")
    print(f"declared floor: GLIBC_{max_allowed}")
    print(f"actual max    : GLIBC_{format_version(actual)}")

    if over:
        print(
            f"::error::{binary} requires GLIBC_{format_version(actual)} but the "
            f"declared floor is GLIBC_{max_allowed}. Offending: "
            + ", ".join("GLIBC_" + format_version(v) for v in over)
        )
        return 1

    print(f"OK: within the declared floor GLIBC_{max_allowed}")
    return 0


# --------------------------------------------------------------------------
# Self-test. Three assertions, per the standing rule: a known-positive must
# pass, a known-negative must FAIL, and the obvious-but-wrong implementation
# must answer DIFFERENTLY — without that third one a self-test passes on a
# broken instrument.
# --------------------------------------------------------------------------
SAMPLE = """
  0x0030: Rev: 1  Flags: none  Index: 2  Cnt: 3  Name: libc.so.6
  0x0040:   Name: GLIBC_2.2.5  Flags: none  Index: 2
  0x0050:   Name: GLIBC_2.9    Flags: none  Index: 3
  0x0060:   Name: GLIBC_2.39   Flags: none  Index: 4
"""


def self_test() -> int:
    failures: list[str] = []

    versions = extract_versions(SAMPLE)
    if max(versions) != (2, 39):
        failures.append(f"max should be 2.39, got {max(versions)}")

    # 1. known-negative: 2.39 present must FAIL a 2.34 floor.
    if not [v for v in versions if v > parse_version("2.34")]:
        failures.append("known-negative did not trip: 2.39 must violate a 2.34 floor")

    # 2. known-positive: the same set must PASS a 2.39 floor.
    if [v for v in versions if v > parse_version("2.39")]:
        failures.append("known-positive did not hold: 2.39 must satisfy a 2.39 floor")

    # 3. the broken implementation must disagree. Lexicographic max over the
    #    raw strings picks "2.9", which would wrongly satisfy a 2.34 floor.
    lexicographic_max = max(SYMBOL_RE.findall(SAMPLE))
    if lexicographic_max != "2.9":
        failures.append(
            f"expected the lexicographic bug to pick 2.9, got {lexicographic_max}; "
            "the third assertion is no longer exercising the bug it guards"
        )
    if parse_version(lexicographic_max) > parse_version("2.34"):
        failures.append(
            "lexicographic max wrongly violates the 2.34 floor; this assertion is "
            "supposed to show the BROKEN implementation passing where we fail"
        )

    # 4. absence must not read as compliance.
    if extract_versions("no symbols here"):
        failures.append("empty input should yield no versions")

    if failures:
        for f in failures:
            print(f"SELF-TEST FAILURE: {f}")
        return 1

    print("check_glibc_floor self-test: 4/4 assertions passed")
    print("  known-positive (2.39 set vs 2.39 floor) -> pass")
    print("  known-negative (2.39 set vs 2.34 floor) -> fail, as required")
    print("  lexicographic 'sort -u' bug picks 2.9 and would WRONGLY pass 2.34")
    print("  zero-symbol input yields zero versions (treated as a failure at runtime)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary")
    ap.add_argument(
        "--max-glibc",
        help="highest glibc version the artifact is allowed to require, e.g. 2.34",
    )
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return self_test()
    if not args.binary or not args.max_glibc:
        ap.error("--binary and --max-glibc are required unless --self-test is given")
    return check(args.binary, args.max_glibc)


if __name__ == "__main__":
    sys.exit(main())
