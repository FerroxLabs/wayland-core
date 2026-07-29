#!/usr/bin/env python3
"""Attribute each env mutation to its enclosing #[test] fn and report whether
that fn is protected by #[serial].

A #[serial] test only serializes against OTHER #[serial] tests. A test that
mutates process env WITHOUT #[serial] runs concurrently with everything else in
its binary, so it is the collision SOURCE. Those are the flake surface.
"""
import os
import re
import sys
from collections import defaultdict

ROOT = sys.argv[1] if len(sys.argv) > 1 else "crates"

ENV_MUT = re.compile(r"\b(?:env::)?(set_var|remove_var)\s*\(")
SET_CWD = re.compile(r"\bset_current_dir\s*\(")
FN_DEF = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
ATTR = re.compile(r"^\s*#\[")
TEST_ATTR = re.compile(r"#\[(?:\w+::)?(?:test|tokio::test|rstest)\b|#\[test\]")
SERIAL_ATTR = re.compile(r"#\[serial\b|#\[serial_test::serial\b|#\[file_serial\b")


def scan(path):
    """Yield (fn_name, is_test, has_serial, line_no, kind) for env mutations."""
    with open(path, encoding="utf-8", errors="replace") as fh:
        lines = fh.read().splitlines()

    # Build a map line -> (fn_name, is_test, has_serial) by walking fn defs and
    # the contiguous attribute block immediately above each.
    owner = {}
    n = len(lines)
    for i, line in enumerate(lines):
        m = FN_DEF.match(line)
        if not m:
            continue
        # collect attribute block above
        attrs = []
        j = i - 1
        while j >= 0 and (ATTR.match(lines[j]) or lines[j].strip().startswith("//")
                          or lines[j].strip() == ""):
            if ATTR.match(lines[j]):
                attrs.append(lines[j])
            elif lines[j].strip() == "":
                break
            j -= 1
        blob = " ".join(attrs)
        is_test = bool(TEST_ATTR.search(blob))
        has_serial = bool(SERIAL_ATTR.search(blob))
        # body extent by brace matching
        depth = 0
        started = False
        k = i
        while k < n:
            depth += lines[k].count("{") - lines[k].count("}")
            if "{" in lines[k]:
                started = True
            if started and depth <= 0:
                break
            k += 1
        for ln in range(i, min(k + 1, n)):
            owner[ln + 1] = (m.group(1), is_test, has_serial)

    for idx, line in enumerate(lines, 1):
        s = line.lstrip()
        if s.startswith("//"):
            continue
        kind = "env" if ENV_MUT.search(line) else ("cwd" if SET_CWD.search(line) else None)
        if kind is None:
            continue
        fn, is_test, has_serial = owner.get(idx, ("<toplevel/helper>", False, False))
        yield (fn, is_test, has_serial, idx, kind)


def main():
    unprotected = []      # test fn mutating env with NO #[serial]
    protected = 0
    helper = defaultdict(list)   # mutations inside non-test helpers

    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d != "target"]
        for fn_ in sorted(filenames):
            if not fn_.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn_)
            try:
                for fname, is_test, has_serial, ln, kind in scan(path):
                    if is_test and not has_serial:
                        unprotected.append((path, ln, fname, kind))
                    elif is_test:
                        protected += 1
                    else:
                        helper[path].append((ln, fname, kind))
            except OSError:
                continue

    print(f"UNPROTECTED test fns mutating process globals: {len(unprotected)} sites")
    print(f"PROTECTED (#[serial]) mutation sites          : {protected}")
    hcount = sum(len(v) for v in helper.values())
    print(f"Mutations in NON-test fns (helpers/prod)      : {hcount}")

    byfile = defaultdict(list)
    for p, ln, fname, kind in unprotected:
        byfile[p].append((ln, fname, kind))
    print(f"\n=== UNPROTECTED, by file ({len(byfile)} files) ===")
    for p, items in sorted(byfile.items(), key=lambda kv: -len(kv[1])):
        print(f"\n{p}  ({len(items)} sites)")
        seen = set()
        for ln, fname, kind in items:
            if fname in seen:
                continue
            seen.add(fname)
            print(f"    {p}:{ln}  fn {fname}  [{kind}]")
    return 0


if __name__ == "__main__":
    sys.exit(main())
