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

# Flavour (d): a FILE-LEVEL inner attribute `#![cfg(...)]`. When the predicate
# is false the binary contains zero tests, so `cargo test --test X` prints
# `running 0 tests` / `test result: ok` and exits 0. It is invisible to the
# attribute scan (there is no `#[ignore]`), to the env-gate scan (there is no
# `env::var`) and to the filter sweep (the command names the binary correctly).
#
# Anchored to column 0: `#![cfg(...)]` is only file-scoped as an inner
# attribute at the top of the file. An indented `#![...]` is inside a module
# and does not blank the binary.
FILE_CFG = re.compile(r"^#!\[cfg\((.+)\)\]")
# A feature gate depends on how cargo was invoked, so it can silently blank a
# binary on a host that could otherwise run it. A platform gate is a property
# of the machine and blanking is correct there — but it still produces an
# affirmative `ok` for zero work on the wrong platform, so both are reported
# and kept in separate buckets.
FEATURE_PRED = re.compile(r"\bfeature\s*=")


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


MOD_DECL = re.compile(r"^\s*(?:pub\s+)?mod\s+([A-Za-z0-9_]+)\s*;")
MOD_PATH_ATTR = re.compile(r'^\s*#\[\s*path\s*=\s*"([^"]+)"\s*\]')


def included_module_files(path, _seen=None):
    """Every source file compiled INTO this test binary via `mod` declarations.

    A test binary is not one file. `crates/wcore-cli/tests/acp_engine_turn.rs`
    declares `#[path = "support/mod.rs"] mod support;`, and that module carries
    8 non-ignored `#[test]`s — so the binary runs 8 tests by default and is NOT
    an all-ignored binary at all.

    Missing this made the previous revision report acp_engine_turn as flavour
    (a). That is a FALSE POSITIVE, and a detector that over-reports is as
    useless as one that under-reports: it trains the reader to skim the list.
    """
    if _seen is None:
        _seen = set()
    path = path.resolve()
    if path in _seen or not path.is_file():
        return []
    _seen.add(path)
    out = [path]
    base = path.parent
    stem_dir = base / path.stem
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    pending_path = None
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("//") or stripped.startswith("/*") or stripped.startswith("*"):
            continue
        pm = MOD_PATH_ATTR.match(line)
        if pm:
            pending_path = pm.group(1)
            continue
        m = MOD_DECL.match(line)
        if m:
            if pending_path:
                cands = [base / pending_path]
            else:
                name = m.group(1)
                cands = [
                    base / f"{name}.rs",
                    base / name / "mod.rs",
                    stem_dir / f"{name}.rs",
                    stem_dir / name / "mod.rs",
                ]
            for c in cands:
                if c.is_file():
                    out.extend(included_module_files(c, _seen))
                    break
            pending_path = None
            continue
        if stripped and not stripped.startswith("#["):
            pending_path = None
    return out


def file_level_cfg(path):
    """Flavour (d) probe: return the file-level ``#![cfg(...)]`` predicate, or None.

    Comment lines are skipped for the same reason ``classify_file`` skips them:
    an earlier generator in this program matched ``#[ignore`` inside its own
    doc-comment prose and reported documentation as code.
    """
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith("//") or stripped.startswith("/*") or stripped.startswith("*"):
            continue
        m = FILE_CFG.match(line)
        if m:
            return m.group(1).split("]")[0].strip()
        # Inner attributes must precede all items. The first non-comment,
        # non-inner-attribute line ends the region where one can appear.
        if not stripped.startswith("#!"):
            return None
    return None


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
    flavour_d_feature, flavour_d_platform = [], []
    for crate, f in test_binaries():
        # Classify the WHOLE binary: the entry file plus every module compiled
        # into it. Flavour (a) is a property of the binary, not of one file.
        total = ignored = env_gated = 0
        ni, nl = [], []
        for unit in included_module_files(f):
            u_total, u_ignored, u_ni, u_nl, u_env = classify_file(unit)
            total += u_total
            ignored += u_ignored
            ni += u_ni
            nl += u_nl
            env_gated += u_env
        rel = str(f.relative_to(ROOT))

        # Flavour (d). Checked BEFORE the `total == 0` short-circuit below:
        # that `continue` is exactly why the previous revision of this script
        # could not see flavour (d) at all. A gated file still has test
        # functions in its text, so `total` is non-zero here — what matters is
        # that the compiler emits none of them when the predicate is false.
        pred = file_level_cfg(f)
        if pred is not None:
            drec = {
                "crate": crate,
                "file": rel,
                "predicate": pred,
                "tests_blanked_when_false": total,
            }
            if FEATURE_PRED.search(pred):
                flavour_d_feature.append(drec)
            else:
                flavour_d_platform.append(drec)

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
        "flavour_d_file_level_feature_gate": flavour_d_feature,
        "flavour_d_feature_count": len(flavour_d_feature),
        "flavour_d_file_level_platform_gate": flavour_d_platform,
        "flavour_d_platform_count": len(flavour_d_platform),
        "filtered_invocations": filtered_invocations(),
        "clean_count": len(clean),
    }
    print(json.dumps(report, indent=2))
    return 0


# ── Falsification ───────────────────────────────────────────────────────────
# A detector nobody falsifies is the defect it hunts. The ancestor of this
# script matched `#[ignore` inside its own doc-comment prose, so it reported
# documentation as code and nobody noticed. `--self-test` writes a
# known-POSITIVE and a known-NEGATIVE fixture and asserts the detector
# SEPARATES them: it must flag the positive and must NOT flag the negative.
# If it flags both, or neither, it is measuring nothing and this exits 1.

SELF_TEST_CASES = [
    # (name, source, expect_flagged, expect_bucket)
    (
        "positive_feature_gate",
        '#![cfg(feature = "otlp")]\n\n#[test]\nfn a() {}\n',
        True,
        "feature",
    ),
    (
        "positive_platform_gate",
        '#![cfg(target_os = "macos")]\n\n#[test]\nfn a() {}\n',
        True,
        "platform",
    ),
    (
        "positive_compound_gate",
        '#![cfg(all(target_os = "linux", feature = "seccomp"))]\n\n#[test]\nfn a() {}\n',
        True,
        "feature",
    ),
    (
        "positive_gate_below_doc_comment",
        '//! A doc comment that mentions #![cfg(feature = "decoy")] in PROSE.\n'
        '#![cfg(feature = "real")]\n\n#[test]\nfn a() {}\n',
        True,
        "feature",
    ),
    (
        "negative_no_gate",
        "#[test]\nfn a() {}\n",
        False,
        None,
    ),
    (
        "negative_prose_only",
        '//! This file DISCUSSES #![cfg(feature = "otlp")] but does not carry it.\n'
        "\n#[test]\nfn a() {}\n",
        False,
        None,
    ),
    (
        "negative_item_level_cfg",
        '#[cfg(feature = "otlp")]\n#[test]\nfn a() {}\n',
        False,
        None,
    ),
    (
        "negative_indented_inner_attr",
        'mod m {\n    #![cfg(feature = "otlp")]\n}\n\n#[test]\nfn a() {}\n',
        False,
        None,
    ),
]


def self_test():
    import tempfile

    failures = 0
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        for name, src, expect_flagged, expect_bucket in SELF_TEST_CASES:
            f = tmp / f"{name}.rs"
            f.write_text(src, encoding="utf-8")
            pred = file_level_cfg(f)
            flagged = pred is not None
            bucket = None
            if flagged:
                bucket = "feature" if FEATURE_PRED.search(pred) else "platform"
            ok = flagged == expect_flagged and bucket == expect_bucket
            if not ok:
                failures += 1
            print(
                f"{'PASS' if ok else 'FAIL'}  {name:34s} "
                f"flagged={flagged!s:5s} bucket={bucket!s:9s} "
                f"expected flagged={expect_flagged!s:5s} bucket={expect_bucket!s}"
            )
    positives = sum(1 for c in SELF_TEST_CASES if c[2])
    negatives = len(SELF_TEST_CASES) - positives
    print(f"\n{positives} known-positive, {negatives} known-negative, {failures} mismatched")
    if failures:
        print("SELF-TEST FAILED: the detector does not separate positives from negatives")
        return 1
    print("SELF-TEST PASSED: detector separates known-positive from known-negative")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv[1:]:
        sys.exit(self_test())
    sys.exit(main())
