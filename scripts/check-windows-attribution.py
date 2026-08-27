#!/usr/bin/env python3
r"""Fail if a Windows CI conclusion cannot be attributed to a tree.

WHY THIS EXISTS
---------------
FerroxLabs/wayland#1146. The Windows pool is neither one machine nor one
service: `ferrox-win-msvc` and `SEANDESKTOP` are two runner SERVICES on the
same host (ci.yml records both running as `NT AUTHORITY\NetworkService`),
alongside the hosted `windows-latest` pool. The failure set churns between
them on the same tree — ci.yml's own matrix note says exactly that and closes
with "Still open".

The measured instance that filed the issue: `main`'s recorded green had
`CI (windows-latest, hosted)` SKIPPED, and when that leg ran on main's
byte-identical tree it failed three tests on all three tries. Four runs, three
different Windows executors, and not one Windows job said which box served it.

Two things have to hold before a Windows red means anything:

  R1  Every Windows job in the Windows TEST workflows records which executor
      served it, and FAILS CLOSED when it cannot. A job that reports
      anonymously turns every later A/B into a coin flip — which is the state
      #1146 recorded.
  R2  The tests whose failure set churns are not laundered by
      `[profile.ci] retries = 2`. nextest counts a test that failed twice and
      passed once as PASSED, so the run CONCLUSION — the thing release gates,
      dashboards and humans read — cannot tell churn from green. Two of the
      three also carry an in-test `RACE_ATTEMPTS` loop, so nextest retries were
      stacking on top of an internal retry.

Scope note: this covers the workflows that draw a TEST verdict about a tree on
Windows. Release/artifact-build workflows are deliberately out of scope — they
produce binaries, not a pass/fail verdict about a tree.

Known limitation, stated rather than hidden: a job is classified as Windows
from its `runs-on:` value, plus its `strategy:` block when `runs-on` defers to
`matrix.`. A Windows job that selects its runner through some third mechanism
would not be seen. The self-test pins the classifier on all three shapes in use
(`windows-latest`, a `[self-hosted, Windows, X64, msvc]` list, and a
matrix-driven `runs-on`).

Run:  python3 scripts/check-windows-attribution.py
Self-test: python3 scripts/check-windows-attribution.py --self-test
"""

from __future__ import annotations

import os
import re
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

RECORD_STEP_NAME = "Record which Windows executor served this job"

# The step has to do three things, and each is checked, because a step that
# merely echoes into the log satisfies none of the reasons it exists:
#   RUNNER_NAME          — the executor identity itself.
#   exit 1               — fail closed; an anonymous job is worse than a red one.
#   GITHUB_STEP_SUMMARY  — readable from `gh run view` without downloading
#                          logs, so an A/B pair is one command, not two archives.
REQUIRED_IN_STEP = ("RUNNER_NAME", "exit 1", "GITHUB_STEP_SUMMARY")

WINDOWS_TEST_WORKFLOWS = (
    ".github/workflows/ci.yml",
    ".github/workflows/nightly-windows-soak.yml",
    ".github/workflows/windows-flake-ledger.yml",
)

# The three tests #1146 measured failing on all three tries on main's own tree.
QUARANTINED_TESTS = (
    "the_bash_timeout_bounds_the_secret_deny_walk",
    "the_streaming_bash_timeout_bounds_the_secret_deny_walk",
    "matching_assistant_dials_scoped_deferred_server",
)

NEXTEST_CONFIG = ".config/nextest.toml"

JOB_START_RE = re.compile(r"^  ([A-Za-z0-9_.\-]+):\s*$")
TOP_LEVEL_KEY_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_\-]*:")
STEP_NAME_RE = re.compile(r"^      - name:\s*(.+?)\s*$")


def split_jobs(text: str) -> list[tuple[str, list[str]]]:
    """[(job name, its lines)] for every job in a workflow file."""
    lines = text.splitlines()
    jobs: list[tuple[str, list[str]]] = []
    in_jobs = False
    name: str | None = None
    buf: list[str] = []
    for line in lines:
        if not in_jobs:
            if line.rstrip() == "jobs:":
                in_jobs = True
            continue
        # A col-0 key ends the `jobs:` mapping. Comments and blanks do not.
        if TOP_LEVEL_KEY_RE.match(line):
            break
        m = JOB_START_RE.match(line)
        if m:
            if name is not None:
                jobs.append((name, buf))
            name, buf = m.group(1), []
            continue
        if name is not None:
            buf.append(line)
    if name is not None:
        jobs.append((name, buf))
    return jobs


def key_block(job_lines: list[str], key: str) -> str:
    """The value of a 4-space-indented job key, including its continuation."""
    out: list[str] = []
    collecting = False
    head = f"    {key}:"
    for line in job_lines:
        if not collecting:
            if line.startswith(head):
                collecting = True
                out.append(line)
            continue
        if not line.strip():
            out.append(line)
            continue
        indent = len(line) - len(line.lstrip())
        if indent <= 4:
            break
        out.append(line)
    return "\n".join(out)


def is_windows_job(job_lines: list[str]) -> bool:
    runs_on = key_block(job_lines, "runs-on")
    if not runs_on:
        return False
    if "windows" in runs_on.lower():
        return True
    if "matrix." in runs_on:
        return "windows" in key_block(job_lines, "strategy").lower()
    return False


def step_body(job_lines: list[str], step_name: str) -> str | None:
    """The lines of the named step, or None when the job has no such step."""
    out: list[str] = []
    collecting = False
    for line in job_lines:
        m = STEP_NAME_RE.match(line)
        if m:
            if collecting:
                break
            if m.group(1).strip("'\"") == step_name:
                collecting = True
                out.append(line)
            continue
        if collecting:
            # Any other list item at step indent ends this step.
            if line.startswith("      - "):
                break
            out.append(line)
    return "\n".join(out) if collecting else None


def check_workflow(path: str, text: str) -> list[str]:
    """R1 violations in one workflow file, plus the Windows jobs it saw."""
    violations: list[str] = []
    for name, job_lines in split_jobs(text):
        if not is_windows_job(job_lines):
            continue
        body = step_body(job_lines, RECORD_STEP_NAME)
        if body is None:
            violations.append(
                f"{path}: job `{name}` can run on Windows but has no "
                f"`{RECORD_STEP_NAME}` step — a Windows verdict from it cannot "
                f"be attributed to an executor (gh#1146)"
            )
            continue
        missing = [tok for tok in REQUIRED_IN_STEP if tok not in body]
        if missing:
            violations.append(
                f"{path}: job `{name}` records its executor but the step is "
                f"missing {missing} — it must name RUNNER_NAME, publish it to "
                f"GITHUB_STEP_SUMMARY, and `exit 1` when it is unset (gh#1146)"
            )
    return violations


def windows_jobs(text: str) -> list[str]:
    return [n for n, lines in split_jobs(text) if is_windows_job(lines)]


def check_nextest(text: str) -> list[str]:
    """R2: every quarantined test must sit under an override with retries = 0."""
    blocks: list[str] = []
    cur: list[str] | None = None
    for line in text.splitlines():
        if line.startswith("["):
            if cur is not None:
                blocks.append("\n".join(cur))
            cur = [line] if line.startswith("[[profile.ci.overrides]]") else None
            continue
        if cur is not None:
            cur.append(line)
    if cur is not None:
        blocks.append("\n".join(cur))

    quarantined = [b for b in blocks if re.search(r"^\s*retries\s*=\s*0\s*$", b, re.M)]
    covered = {t for t in QUARANTINED_TESTS for b in quarantined if t in b}
    missing = [t for t in QUARANTINED_TESTS if t not in covered]
    if missing:
        return [
            f"{NEXTEST_CONFIG}: no `[[profile.ci.overrides]]` with `retries = 0` "
            f"covers {missing} — `[profile.ci] retries = 2` launders their churn "
            f"into a green run conclusion (gh#1146)"
        ]
    return []


def scan(root: str) -> int:
    violations: list[str] = []
    seen = 0
    for rel in WINDOWS_TEST_WORKFLOWS:
        path = os.path.join(root, rel)
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        jobs = windows_jobs(text)
        seen += len(jobs)
        print(f"  {rel}: {len(jobs)} Windows job(s): {', '.join(jobs) or '-'}")
        violations += check_workflow(rel, text)

    # A scan that finds no Windows jobs at all found nothing to check: that is a
    # broken parser, not a clean tree.
    if seen == 0:
        violations.append(
            "no Windows jobs found in any scanned workflow — the classifier is "
            "broken, not the tree"
        )

    with open(os.path.join(root, NEXTEST_CONFIG), encoding="utf-8") as fh:
        violations += check_nextest(fh.read())

    if violations:
        print()
        for v in violations:
            print(f"VIOLATION: {v}")
        print(f"\nFAIL: {len(violations)} Windows-attribution violation(s)")
        return 1
    print(
        f"OK: {seen} Windows job(s) record their executor; "
        f"{len(QUARANTINED_TESTS)} churning test(s) at retries=0"
    )
    return 0


# ── self-test ──────────────────────────────────────────────────────────────

_GOOD_STEP = """      - name: Record which Windows executor served this job
        shell: bash
        run: |
          if [ -z "${RUNNER_NAME:-}" ]; then
            echo "::error::unset"
            exit 1
          fi
          echo "- runner_name: ${RUNNER_NAME}" >> "$GITHUB_STEP_SUMMARY"
"""

_WF = """name: x

on:
  push:

jobs:
  hosted:
    name: hosted leg
    runs-on: windows-latest
    steps:
{hosted_step}      - uses: actions/checkout@v4

  selfhosted:
    runs-on:
      - self-hosted
      - Windows
      - X64
      - msvc
    steps:
{self_step}      - run: echo hi

  matrixed:
    runs-on: ${{{{ matrix.os }}}}
    strategy:
      matrix:
        os: ["macos-latest", ["self-hosted", "Windows", "X64", "msvc"]]
    steps:
{matrix_step}      - run: echo hi

  linux:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi

  darwin:
    runs-on: [self-hosted, macOS, ARM64]
    steps:
      - run: echo hi
"""


def _wf(hosted: str = _GOOD_STEP, self_: str = _GOOD_STEP, matrixed: str = _GOOD_STEP) -> str:
    return _WF.format(hosted_step=hosted, self_step=self_, matrix_step=matrixed)


def self_test() -> int:
    failures: list[str] = []

    def check(label: str, cond: bool) -> None:
        if not cond:
            failures.append(label)

    # Classifier: three Windows shapes in, Linux and macOS out.
    jobs = windows_jobs(_wf())
    check(f"classifier picked {jobs}", jobs == ["hosted", "selfhosted", "matrixed"])

    # Silent when every Windows job records.
    check("clean fixture must not fire", check_workflow("f.yml", _wf()) == [])

    # Fires once per Windows job that does not record — including the
    # matrix-driven one, which is the shape the `ci` job uses.
    for label, kwargs in (
        ("hosted", {"hosted": ""}),
        ("selfhosted", {"self_": ""}),
        ("matrixed", {"matrixed": ""}),
    ):
        v = check_workflow("f.yml", _wf(**kwargs))
        check(f"missing step in {label} must fire", len(v) == 1 and label in v[0])

    # A step that records but cannot fail closed is a violation too.
    half = _GOOD_STEP.replace("            exit 1\n", "")
    v = check_workflow("f.yml", _wf(hosted=half))
    check("fail-open step must fire", len(v) == 1 and "exit 1" in v[0])
    log_only = _GOOD_STEP.replace('>> "$GITHUB_STEP_SUMMARY"', "")
    v = check_workflow("f.yml", _wf(hosted=log_only))
    check("log-only step must fire", len(v) == 1 and "GITHUB_STEP_SUMMARY" in v[0])

    # R2 both directions.
    names = " + ".join(f"test(={t})" for t in QUARANTINED_TESTS)
    good = f"[[profile.ci.overrides]]\nfilter = '{names}'\nretries = 0\n"
    check("covered nextest config must not fire", check_nextest(good) == [])
    check(
        "retries=2 must fire",
        len(check_nextest(good.replace("retries = 0", "retries = 2"))) == 1,
    )
    partial = f"[[profile.ci.overrides]]\nfilter = 'test(={QUARANTINED_TESTS[0]})'\nretries = 0\n"
    v = check_nextest(partial)
    check(
        "partial coverage must fire and name the gap",
        len(v) == 1 and QUARANTINED_TESTS[1] in v[0] and QUARANTINED_TESTS[0] not in v[0],
    )
    # retries = 0 in some OTHER override must not be read as covering them.
    other = "[[profile.ci.overrides]]\nfilter = 'binary(=pipeline_test)'\nretries = 0\n"
    check("unrelated override must not cover", len(check_nextest(other)) == 1)

    if failures:
        for f in failures:
            print(f"SELF-TEST FAIL: {f}")
        return 1
    print("self-test OK: classifier, R1 (both directions), R2 (both directions)")
    return 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    root = REPO_ROOT
    for i, a in enumerate(argv):
        if a == "--root":
            root = argv[i + 1]
    return scan(root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
