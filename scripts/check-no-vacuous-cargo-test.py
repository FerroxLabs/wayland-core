#!/usr/bin/env python3
"""Fail if any *executable* `cargo test` invocation can report green having run zero tests.

WHY THIS EXISTS
---------------
`cargo nextest` fails closed on an empty run (exit 4, `NO_TESTS_RUN`).
**`cargo test` has no such option and no such default — none at all.**
Measured on this checkout (hetzner-dsm, cargo 1.96.0, nextest 0.9.137):

    $ cargo test -p wcore-eval-scenarios --test packaged_driver_gate
    running 0 tests
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
    rc=0                                    <-- a release-integrity gate, green, having done nothing

    $ cargo nextest run -p wcore-eval-scenarios --test packaged_driver_gate
    Summary [0.000s] 0 tests run: 0 passed, 0 skipped
    error: no tests to run
    rc=4

There are ~488 integration-test binaries reachable by a bare `cargo test` in
this workspace, of which 44 carry a file-level `#![cfg(...)]` (17 feature-gated,
27 platform-gated) and 23 have every case `#[ignore]`d. Any of those compiles or
collects to EMPTY under the wrong feature/platform/filter and prints
`test result: ok`. An instrument that cannot fail is not evidence.

THE RULE
--------
An executable `cargo test` line is a violation unless one of:

  * it passes `--no-run` (compiles only; it never executes, so it cannot be
    vacuously green);
  * it is annotated `vacuity-checked: <reason>` on the line itself or within
    the preceding 10 lines, meaning the caller reads the EXECUTED COUNT back
    explicitly rather than trusting exit status.

`cargo nextest run` is never a violation. NOTE: nextest's fail-closed behaviour
is a CLI option (`--no-tests=fail`) and a version default, NOT a config key — a
`no-tests = "fail"` key under `[profile.default]` was measured to be silently
ignored ("ignoring unknown configuration key"). The release-integrity gates
therefore pass `--no-tests=fail` explicitly, and `.config/nextest.toml` pins a
`nextest-version.required` floor. See that file's header for the measurement.

FALSE POSITIVES ARE HANDLED BY DECLARATION, NOT SUPPRESSION
-----------------------------------------------------------
A `#![cfg(windows)]` binary is *legitimately* empty on Linux. The answer is not
a blanket suppression: it is to scope the invocation so the emptiness is
declared (a `[unix]` / `[windows]` justfile recipe, or a platform-conditional CI
job), which the justfile already does for `f01-packaged-driver-gate`. The
`vacuity-checked:` marker exists for the other legitimate case — a caller that
already asserts `running N test` / `N passed` itself, as
`.github/workflows/macos-native-suites.yml` and `scripts/f24-c3-tests.sh` do.

Run:  python3 scripts/check-no-vacuous-cargo-test.py
Self-test: python3 scripts/check-no-vacuous-cargo-test.py --self-test
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

MARKER = "vacuity-checked:"
MARKER_WINDOW = 10

# `cargo test`, `vx cargo test`, `cargo +nightly test`, `$DOCKER_RUN img cargo test`.
# Deliberately NOT matching `cargo nextest run`: `\btest\b` cannot match inside
# the word "nextest", and the explicit (?!next) guard documents the intent.
CARGO_TEST = re.compile(r"\bcargo\s+(?:\+\S+\s+)?(?!next)test\b")

COMMENT_PREFIXES = ("#", "//", "*", "///", "//!", "<!--")

# Backtick-quoted prose, e.g. a YAML step name mentioning `cargo test`.
BACKTICKED = re.compile(r"`[^`]*`")

# A YAML step label: `- name: ...` / `name: ...`. Never an executed command.
YAML_NAME = re.compile(r"^-?\s*name:\s")


def scan_text(text: str) -> list[tuple[int, str]]:
    """Return [(lineno, line)] for every executable, unguarded `cargo test`."""
    lines = text.splitlines()
    violations: list[tuple[int, str]] = []
    for i, raw in enumerate(lines):
        line = raw.strip()
        # Strip backtick-quoted spans BEFORE matching. A YAML step name such as
        # `- name: No vacuous \`cargo test\` invocations` is prose, not an
        # invocation — this guard false-fired on its own CI wiring step on first
        # run. Markdown-style backticks are how this repo quotes commands in
        # prose everywhere, so this is the general fix, not a special case.
        probe = BACKTICKED.sub("", line)
        if not CARGO_TEST.search(probe):
            continue
        # Prose, doc comments, YAML/justfile/shell comments: not executable.
        if line.startswith(COMMENT_PREFIXES):
            continue
        # A YAML step name is a label, never a command.
        if YAML_NAME.match(line):
            continue
        # `--no-run` builds the binary and stops. It cannot report a vacuous pass.
        if "--no-run" in line:
            continue
        # Explicitly declared as reading its own executed count back.
        window = lines[max(0, i - MARKER_WINDOW) : i + 1]
        if any(MARKER in w for w in window):
            continue
        violations.append((i + 1, line))
    return violations


def targets(root: Path) -> list[Path]:
    """Files that can actually EXECUTE a test run. Source/docs are prose only."""
    candidates: list[Path] = [root / "justfile", root / "Justfile"]
    candidates += sorted((root / ".github" / "workflows").glob("*.yml"))
    candidates += sorted((root / ".github" / "workflows").glob("*.yaml"))
    for pat in ("*.sh", "*.bash", "*.ps1", "*.mjs", "*.js"):
        candidates += sorted((root / "scripts").glob(pat))

    # Dedupe by (device, inode), NOT by path string. On a case-insensitive
    # filesystem (macOS default) `justfile` and `Justfile` are ONE file that
    # resolves to two distinct-looking paths, which double-reported every
    # justfile violation on the first run of this guard.
    here = Path(__file__).resolve()
    here_id = (here.stat().st_dev, here.stat().st_ino)
    seen: set[tuple[int, int]] = {here_id}
    out: list[Path] = []
    for p in candidates:
        if not p.is_file():
            continue
        st = p.stat()
        key = (st.st_dev, st.st_ino)
        if key in seen:
            continue
        seen.add(key)
        out.append(p)
    return out


def run(root: Path) -> int:
    total = 0
    for path in targets(root):
        try:
            text = path.read_text(errors="ignore")
        except OSError:
            continue
        for lineno, line in scan_text(text):
            rel = path.relative_to(root)
            print(f"FAIL {rel}:{lineno}: bare `cargo test` can exit 0 having run zero tests")
            print(f"     {line}")
            total += 1
    if total:
        print()
        print(f"GATE: FAILED — {total} unguarded `cargo test` invocation(s).")
        print("Fix by ONE of:")
        print("  1. use `cargo nextest run --no-tests=fail` (preferred);")
        print("  2. add `--no-run` if you only meant to compile;")
        print("  3. read the executed count back yourself and annotate the line")
        print(f"     `{MARKER} <how you assert N>` within {MARKER_WINDOW} lines above it.")
        return 1
    print("GATE: PASSED — every executable `cargo test` is nextest, --no-run, or count-checked.")
    return 0


# ---------------------------------------------------------------------------
# Self-test. Three assertions, per LANE-BRIEF §6b-ii: a known-positive passes,
# a known-negative fails, AND the old/naive shape would have missed it.
# ---------------------------------------------------------------------------

POSITIVE = """\
harness:
    vx cargo build --release -p wcore-cli
    vx cargo test -p wcore-cli --test harness_cli_surface
"""

NEGATIVE = """\
      - name: No vacuous `cargo test` invocations
# This comment mentions `cargo test` in prose and must NOT fire.
#   run: cargo test -p wcore-foo --test bar
harness:
    vx cargo nextest run -p wcore-cli --test harness_cli_surface
    cargo test -p wcore-agent --test multi_day_journey_test --no-run --message-format=json
    # vacuity-checked: rc via PIPESTATUS[0] and `N passed` grepped from the log below
    cargo test -p wcore-channel-sms 2>&1 | tee -a "$OUT" | grep -E '^test result:'
"""

# The literal line this guard was built to catch, as it stood at 8420ee94.
REAL_PREFIX_DEFECT = """\
      - name: F01 packaged wayland-eval driver gate
        run: vx cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate --test packaged_driver_gate
"""


def self_test() -> int:
    failures = []

    # A1 — KNOWN-POSITIVE: a real bare `cargo test` invocation is detected.
    v = scan_text(POSITIVE)
    if len(v) != 1:
        failures.append(f"A1 known-positive: expected 1 violation, got {len(v)}: {v}")
    else:
        print(f"A1 PASS  known-positive detected at line {v[0][0]}")

    # A2 — KNOWN-NEGATIVE: nextest / --no-run / marker-annotated / prose all pass.
    v = scan_text(NEGATIVE)
    if v:
        failures.append(f"A2 known-negative: expected 0 violations, got {v}")
    else:
        print("A2 PASS  known-negative clean (nextest + --no-run + marker + prose)")

    # A3a — THE OLD SHAPE WOULD HAVE MISSED IT: there was no guard at all, so
    # the real defect shipped. Prove this guard fires on the exact line.
    v = scan_text(REAL_PREFIX_DEFECT)
    if len(v) != 1:
        failures.append(f"A3a: guard does not fire on the real pre-fix ci.yml line: {v}")
    else:
        print("A3a PASS the real pre-fix ci.yml:266 line IS caught (it shipped unguarded)")

    # A4 — REGRESSION: this guard false-fired on its own CI step name, a YAML
    # label containing `cargo test` in backticks. Both defences are asserted.
    for label, fixture in (
        ("yaml step name", "      - name: No vacuous `cargo test` invocations\n"),
        ("backticked prose", '        run: echo "see `cargo test` docs"\n'),
    ):
        v = scan_text(fixture)
        if v:
            failures.append(f"A4 {label}: expected 0 violations, got {v}")
        else:
            print(f"A4 PASS  {label} does not false-fire (real defect, found on first run)")

    # A3b — a NAIVE substring matcher is not an acceptable substitute: it
    # false-fires on the clean fixture. This proves the filtering does real
    # work rather than the guard passing trivially.
    naive = [ln for ln in NEGATIVE.splitlines() if "cargo test" in ln]
    if len(naive) < 2:
        failures.append(f"A3b: naive matcher should over-report on the clean fixture, got {naive}")
    else:
        print(f"A3b PASS naive substring matcher false-fires {len(naive)}x on the clean fixture")

    if failures:
        print()
        for f in failures:
            print("SELF-TEST FAIL:", f)
        return 1
    print("\nSELF-TEST: PASSED (6 assertions)")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(run(Path(__file__).resolve().parent.parent))
