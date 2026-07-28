#!/usr/bin/env python3
"""F-KR-07 lane: independently re-derive the zero-executed-test inventory.

The inherited inventory is trusted for nothing here; it is re-derived and then
compared. Its own generator had this class's disease once (it matched
``#[ignore`` against doc-comment PROSE and reported a non-ignored guard as
ignored), so the parser below never collects a comment line into an attribute
block and anchors every attribute on ``^\\s*#\\[``.

Three flavours, counted separately because each needs a different fix:

  A  every test in the binary carries ``#[ignore]``   -> ``cargo test --test X``
     runs zero and exits 0 printing ``test result: ok``.
  B  the test body returns early behind an env gate   -> prints ``N passed`` for
     zero work, which is strictly worse than a visible ``ignored`` count.
  C  a filter that matches no test name               -> ``cargo test -p X foo``
     exits 0 running nothing, and the command LOOKS deliberately targeted.

A binary with only SOME ignored tests is normal and is NOT counted: the runner
still executes the rest, so it cannot report success on zero work.
"""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]

ATTR = re.compile(r"^\s*#\[")
TEST_ATTR = re.compile(r"^\s*#\[\s*(?:tokio::|async_std::|serial_test::)?test\b|^\s*#\[test\]")
IGNORE_ATTR = re.compile(r"^\s*#\[\s*ignore\b")
FN = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)")
ENV_GATE = re.compile(r"\b(?:var|var_os)\s*\(\s*\"([A-Z0-9_]+)\"")


def classify_file(path):
    """Return (total, ignored, names_ignored, names_live, env_gated_bodies)."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    total = ignored = 0
    names_ignored, names_live = [], []
    block, in_block = [], False
    for line in lines:
        stripped = line.strip()
        # A comment line NEVER joins an attribute block. This is the exact
        # defect that made the first generator report prose as code.
        if stripped.startswith("//") or stripped.startswith("/*") or stripped.startswith("*"):
            continue
        if ATTR.match(line):
            block.append(line)
            in_block = True
            continue
        m = FN.match(line)
        if m:
            if in_block and any(TEST_ATTR.match(b) for b in block):
                total += 1
                if any(IGNORE_ATTR.match(b) for b in block):
                    ignored += 1
                    names_ignored.append(m.group(1))
                else:
                    names_live.append(m.group(1))
            block, in_block = [], False
            continue
        if stripped:
            block, in_block = [], False
    body = path.read_text(encoding="utf-8", errors="replace")
    env_gated = 0
    for m in re.finditer(r"env::(?:var|var_os)\s*\(", body):
        tail = body[m.end() : m.end() + 400]
        if re.search(r"\breturn\b", tail.split("}")[0] if "}" in tail else tail):
            env_gated += 1
    return total, ignored, names_ignored, names_live, env_gated


def test_binaries():
    for crate in sorted((ROOT / "crates").iterdir()):
        tests = crate / "tests"
        if not tests.is_dir():
            continue
        for f in sorted(tests.glob("*.rs")):
            yield crate.name, f


def filtered_invocations():
    """Flavour C sweep: every ``cargo test``/``nextest`` call carrying a filter."""
    hits = []
    globs = ["justfile", "*.just", "scripts/**/*", ".github/workflows/*"]
    seen = set()
    for g in globs:
        for f in ROOT.glob(g):
            if not f.is_file() or f in seen:
                continue
            seen.add(f)
            try:
                text = f.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            for i, line in enumerate(text.splitlines(), 1):
                if "cargo test" not in line and "nextest run" not in line:
                    continue
                hits.append((str(f.relative_to(ROOT)), i, line.strip()))
    return hits


def main():
    flavour_a, some_ignored, flavour_b, clean = [], [], [], []
    for crate, f in test_binaries():
        total, ignored, ni, nl, env_gated = classify_file(f)
        rel = str(f.relative_to(ROOT))
        if total == 0:
            continue
        rec = {
            "crate": crate,
            "file": rel,
            "total": total,
            "ignored": ignored,
            "live": nl,
            "env_gated_bodies": env_gated,
        }
        if ignored == total:
            flavour_a.append(rec)
        elif ignored:
            some_ignored.append(rec)
        else:
            clean.append(rec)
        if env_gated and ignored != total:
            flavour_b.append(rec)

    report = {
        "flavour_a_every_test_ignored": flavour_a,
        "flavour_a_count": len(flavour_a),
        "some_ignored_normal_excluded": [r["file"] for r in some_ignored],
        "some_ignored_count": len(some_ignored),
        "flavour_b_env_gated_candidates": flavour_b,
        "filtered_invocations": filtered_invocations(),
        "clean_count": len(clean),
    }
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
