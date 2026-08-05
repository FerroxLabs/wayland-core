#!/usr/bin/env python3
"""Census of process-global mutation and iteration-order dependence in tests.

Classifies each site as TEST or PROD. TEST sites are the flake surface:
under `cargo test` all tests in a crate share one process, so a
`std::env::set_var` in one test is visible to every other test in that binary.
"""
import os
import re
import sys
from collections import defaultdict

ROOT = sys.argv[1] if len(sys.argv) > 1 else "crates"

ENV_MUT = re.compile(r"\b(?:env::)?(set_var|remove_var)\s*\(")
SET_CWD = re.compile(r"\bset_current_dir\s*\(")
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
MOD_LINE = re.compile(r"^\s*(pub\s+)?mod\s+\w+")
FN_TEST_ATTR = re.compile(r"^\s*#\[(test|tokio::test|rstest)")


def test_line_ranges(path, lines):
    """Return set of line numbers (1-based) that live inside test code."""
    # Whole-file test: anything under a tests/ directory, or benches/examples.
    parts = path.replace("\\", "/").split("/")
    if "tests" in parts or "benches" in parts:
        return "ALL"

    inside = set()
    i = 0
    n = len(lines)
    while i < n:
        if CFG_TEST.match(lines[i]):
            # find the `mod` on the next non-attribute line
            j = i + 1
            while j < n and lines[j].lstrip().startswith("#["):
                j += 1
            if j < n and MOD_LINE.match(lines[j]):
                # brace-match from the opening brace
                depth = 0
                started = False
                k = j
                while k < n:
                    depth += lines[k].count("{") - lines[k].count("}")
                    if "{" in lines[k]:
                        started = True
                    if started and depth <= 0:
                        break
                    k += 1
                for m in range(i, min(k + 1, n)):
                    inside.add(m + 1)
                i = k + 1
                continue
        i += 1
    return inside


def main():
    env_sites = []
    cwd_sites = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d != "target"]
        for fn in filenames:
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn)
            try:
                with open(path, encoding="utf-8", errors="replace") as fh:
                    lines = fh.read().splitlines()
            except OSError:
                continue
            tr = test_line_ranges(path, lines)
            for idx, line in enumerate(lines, 1):
                if line.lstrip().startswith("//"):
                    continue
                kind = None
                if ENV_MUT.search(line):
                    kind = "env"
                elif SET_CWD.search(line):
                    kind = "cwd"
                if kind is None:
                    continue
                is_test = tr == "ALL" or idx in tr
                rec = (path, idx, "TEST" if is_test else "PROD", line.strip())
                (env_sites if kind == "env" else cwd_sites).append(rec)

    for label, sites in (("ENV MUTATION", env_sites), ("set_current_dir", cwd_sites)):
        t = [s for s in sites if s[2] == "TEST"]
        p = [s for s in sites if s[2] == "PROD"]
        print(f"\n===== {label}: {len(sites)} total  TEST={len(t)}  PROD={len(p)} =====")
        by_file = defaultdict(int)
        for s in t:
            by_file[s[0]] += 1
        print(f"--- TEST sites by file ({len(by_file)} files) ---")
        for f, c in sorted(by_file.items(), key=lambda kv: -kv[1]):
            print(f"{c:4d}  {f}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
