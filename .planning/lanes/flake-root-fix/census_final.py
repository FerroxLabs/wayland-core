#!/usr/bin/env python3
"""Definitive census, using the REPAIRED brace scanner (see rsutil.py).

Reports, per TEST BINARY (the real contention unit -- `cargo test` gives each
crate's --lib and each tests/*.rs its own process):

  1. UNPROTECTED mutators: a #[test] fn mutating process globals with no
     #[serial]. One of these defeats an entire serial group.
  2. REGIME SPLITS: one variable mutated from >1 independent serial regime
     within one binary. serial_test groups are independent locks -- a bare
     #[serial] does NOT exclude #[serial(named_group)].

Only variables that are actually CONTENDED (mutated by >1 test in the same
binary, or mutated by one test while another in that binary reads it) can flake.
"""
import os
import re
import sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rsutil import fn_body_range, strip_literals  # noqa: E402

ROOT = sys.argv[1] if len(sys.argv) > 1 else "crates"

FN = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
SER = re.compile(r"#\[(?:serial_test::)?(?:file_)?serial(?:\(\s*([A-Za-z_][A-Za-z_0-9]*)\s*\))?\]")
MUT = re.compile(r"\b(?:env::)?(?:set_var|remove_var)\s*\(\s*([^,)]+)")
CWD = re.compile(r"\bset_current_dir\s*\(")
STRLIT = re.compile(r'^"([A-Z_0-9]+)"$')
READ = re.compile(r'\benv::var(?:_os)?\s*\(\s*"([A-Z_0-9]+)"')


def binary_of(path):
    parts = path.replace("\\", "/").split("/")
    if "tests" in parts:
        i = parts.index("tests")
        return f"{parts[i-1]}::tests/{parts[-1]}"
    if "src" in parts:
        i = parts.index("src")
        return f"{parts[i-1]}::lib"
    return path


def collect():
    """binary -> list of (file, line, fn, regime, vars_mutated, vars_read)"""
    out = defaultdict(list)
    for dp, dn, fns in os.walk(ROOT):
        dn[:] = [d for d in dn if d != "target"]
        for f in sorted(fns):
            if not f.endswith(".rs"):
                continue
            p = os.path.join(dp, f)
            lines = open(p, encoding="utf-8", errors="replace").read().splitlines()
            alias = {}
            st = {}
            code = [strip_literals(l, st) for l in lines]
            for ln in lines:
                m = re.match(r'\s*let\s+(\w+)(?::\s*&str)?\s*=\s*"([A-Z_0-9]+)"\s*;', ln)
                if m:
                    alias[m.group(1)] = m.group(2)
            for i, cl in enumerate(code):
                m = FN.match(cl)
                if not m:
                    continue
                attrs, j = [], i - 1
                while j >= 0 and code[j].lstrip().startswith("#["):
                    attrs.append(lines[j])
                    j -= 1
                blob = " ".join(attrs)
                if "#[test" not in blob and "tokio::test" not in blob:
                    continue
                end = fn_body_range(lines, i)
                body = "\n".join(lines[i:end + 1])
                muts = set()
                for raw in MUT.findall(body):
                    raw = raw.strip()
                    lit = STRLIT.match(raw)
                    v = lit.group(1) if lit else alias.get(raw)
                    if v:
                        muts.add(v)
                if CWD.search(body):
                    muts.add("<CWD>")
                reads = set(READ.findall(body))
                if not muts:
                    continue
                sm = SER.search(blob)
                regime = ("serial:" + (sm.group(1) or "<default>")) if sm else "UNPROTECTED"
                out[binary_of(p)].append((p, i + 1, m.group(1), regime, muts, reads))
    return out


def main():
    data = collect()
    unprot, splits = [], []
    for binary, recs in data.items():
        # which vars are contended IN THIS BINARY (touched by >1 test)?
        touch = defaultdict(set)
        for p, ln, fn, reg, muts, reads in recs:
            for v in muts:
                touch[v].add(fn)
        regimes = defaultdict(set)
        for p, ln, fn, reg, muts, reads in recs:
            for v in muts:
                regimes[v].add(reg)
        for p, ln, fn, reg, muts, reads in recs:
            contended = {v for v in muts if len(touch[v]) > 1}
            if reg == "UNPROTECTED" and contended:
                unprot.append((binary, p, ln, fn, sorted(contended)))
        for v, rs in regimes.items():
            if len(rs) > 1 and len(touch[v]) > 1:
                splits.append((binary, v, sorted(rs), sorted(touch[v])))

    print("=" * 74)
    print(f"UNPROTECTED mutators of a CONTENDED variable: {len(unprot)}")
    print("=" * 74)
    for b, p, ln, fn, vs in sorted(unprot):
        print(f"{b}\n    {p}:{ln} fn {fn}\n      contends on: {', '.join(vs)}")

    print("\n" + "=" * 74)
    print(f"REGIME SPLITS (one var, >1 independent lock, same binary): {len(splits)}")
    print("=" * 74)
    for b, v, rs, fns in sorted(splits):
        print(f"{b}\n    {v}: {', '.join(rs)}")
        for fn in fns:
            print(f"        - {fn}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
