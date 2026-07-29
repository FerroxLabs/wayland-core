#!/usr/bin/env python3
"""Per-TEST-BINARY serial-regime split for process-env mutators.

Unit of contention is the TEST BINARY, not the workspace: `cargo test` gives
each crate's `--lib` and each `tests/*.rs` its OWN process. Tests in different
binaries cannot race, so a cross-crate "split" is not a defect.

Within one binary, serial_test's groups are INDEPENDENT locks:
  #[serial]                  -> group "" (default)
  #[serial(wayland_home_env)] -> group "wayland_home_env"
These two do NOT exclude each other. So two tests mutating the same variable
from different groups still run concurrently -- as does any test with no
#[serial] at all.

Reports, per binary, the variables mutated from more than one regime.
"""
import os
import re
import sys
from collections import defaultdict

ROOT = sys.argv[1] if len(sys.argv) > 1 else "crates"

FN = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
SER = re.compile(r"#\[(?:serial_test::)?(?:file_)?serial(?:\(\s*([A-Za-z_][A-Za-z_0-9]*)\s*\))?\]")
MUT = re.compile(r"\b(?:env::)?(?:set_var|remove_var)\s*\(\s*([^,)]+)")
STRLIT = re.compile(r'^"([A-Z_0-9]+)"$')


def binary_of(path):
    """Map a source file to the test BINARY it compiles into."""
    parts = path.replace("\\", "/").split("/")
    # crates/<crate>/tests/<file>.rs  -> its own integration binary
    if "tests" in parts:
        i = parts.index("tests")
        crate = parts[i - 1]
        return f"{crate}::tests/{parts[-1]}"
    # crates/<crate>/src/**           -> the crate's --lib unittest binary
    if "src" in parts:
        i = parts.index("src")
        return f"{parts[i-1]}::lib"
    return path


def main():
    # binary -> var -> set of regimes
    table = defaultdict(lambda: defaultdict(set))
    for dp, dn, fns in os.walk(ROOT):
        dn[:] = [d for d in dn if d != "target"]
        for f in fns:
            if not f.endswith(".rs"):
                continue
            p = os.path.join(dp, f)
            lines = open(p, encoding="utf-8", errors="replace").read().splitlines()
            # local string aliases: let k = "WAYLAND_HOME";
            alias = {}
            for ln in lines:
                m = re.match(r'\s*let\s+(\w+)(?::\s*&str)?\s*=\s*"([A-Z_0-9]+)"\s*;', ln)
                if m:
                    alias[m.group(1)] = m.group(2)
            for i, ln in enumerate(lines):
                m = FN.match(ln)
                if not m:
                    continue
                attrs, j = [], i - 1
                while j >= 0:
                    s = lines[j].lstrip()
                    if s.startswith("#["):
                        attrs.append(lines[j]); j -= 1; continue
                    if s.startswith("//") or s == "":
                        break
                    break
                blob = " ".join(attrs)
                if "#[test" not in blob and "tokio::test" not in blob:
                    continue
                d, st, k = 0, False, i
                while k < len(lines):
                    d += lines[k].count("{") - lines[k].count("}")
                    if "{" in lines[k]:
                        st = True
                    if st and d <= 0:
                        break
                    k += 1
                body = "\n".join(lines[i:k + 1])
                sm = SER.search(blob)
                regime = ("serial:" + (sm.group(1) or "<default>")) if sm else "UNPROTECTED"
                for raw in MUT.findall(body):
                    raw = raw.strip()
                    lit = STRLIT.match(raw)
                    var = lit.group(1) if lit else alias.get(raw)
                    if not var:
                        continue
                    table[binary_of(p)][var].add(regime)

    bad = []
    for binary, vars_ in table.items():
        for var, regimes in vars_.items():
            if len(regimes) > 1:
                bad.append((binary, var, sorted(regimes)))
    print(f"Variables mutated from MORE THAN ONE serial regime within a single "
          f"test binary: {len(bad)}\n")
    for binary, var, regimes in sorted(bad):
        print(f"{binary}\n    {var}: {', '.join(regimes)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
