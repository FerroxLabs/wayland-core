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

WHAT R2 ENFORCES, EXACTLY
-------------------------
The default run is a text scan with no toolchain, so it can only check the
CONFIG. It requires each churning test to appear as the literal operand of a
`test(...)` predicate, spelled as nextest names it, in an override that sets
`retries = 0`, with `#` comments stripped first. That is deliberately narrower
than it sounds, and the narrowness is the fix: the first version of this gate
searched the block for the name as a SUBSTRING, which accepted a
module-path-less spelling that selects zero tests and accepted the names
sitting in a comment inside an unrelated override. Both are pinned in the
self-test now.

`--with-nextest` is the oracle the text scan cannot be: it asks cargo which
tests each filterset actually selects, so it catches a pinned name that has
been renamed or moved, and an EARLIER override that sets `retries` for the same
test and therefore shadows the quarantine. It needs a built workspace, so it is
not part of the CI text gate — run `just check-windows-attribution-live` after
touching any of the three tests or any `[[profile.ci.overrides]]` above the
quarantine block.

Run:  python3 scripts/check-windows-attribution.py
Self-test: python3 scripts/check-windows-attribution.py --self-test
Live (needs cargo + a built workspace):
      python3 scripts/check-windows-attribution.py --with-nextest
"""

from __future__ import annotations

import os
import re
import subprocess
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

# The three tests #1146 measured failing on all three tries on main's own tree,
# spelled the way NEXTEST names them. The spelling IS the check: the two bash
# ones are unit tests inside the `wcore-tools` lib binary, so their nextest name
# carries the module path, and `test(=the_bash_timeout_bounds_the_secret_deny_walk)`
# without it is a filterset that selects ZERO tests — an override built on it
# leaves them at `[profile.ci] retries = 2` and says nothing.
#
# Resolved against this tree with `--with-nextest` (`cargo nextest list
# --profile ci -E <the override filter>` returns exactly these three).
QUARANTINED_TESTS = (
    "bash::tests::the_bash_timeout_bounds_the_secret_deny_walk",
    "bash::tests::the_streaming_bash_timeout_bounds_the_secret_deny_walk",
    "matching_assistant_dials_scoped_deferred_server",
)

NEXTEST_CONFIG = ".config/nextest.toml"

JOB_START_RE = re.compile(r"^  ([A-Za-z0-9_.\-]+):\s*$")
TOP_LEVEL_KEY_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_\-]*:")
STEP_NAME_RE = re.compile(r"^      - name:\s*(.+?)\s*$")

FILTER_RE = re.compile(r"""^\s*filter\s*=\s*(?P<q>['"])(?P<expr>.*)(?P=q)\s*$""", re.M)
RETRIES_RE = re.compile(r"^\s*retries\s*=\s*(?P<n>\d+)\s*$", re.M)
# The operand of a literal `test(...)` / `test(=...)` predicate. A regex
# operand (`test(/re/)`) is deliberately not accepted as coverage: this gate
# compares operands to exact names and cannot evaluate a regex.
TEST_LITERAL_RE = re.compile(r"test\(\s*=?\s*([^)/]+?)\s*\)")


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


def strip_toml_comments(text: str) -> str:
    """`#` comments removed, quoted strings preserved.

    A test name written in a comment is documentation, not coverage. Stripping
    first is what stops a `# the_bash_timeout_...` line inside an unrelated
    override from reading as a quarantine.
    """
    out: list[str] = []
    for line in text.splitlines():
        quote: str | None = None
        cut: int | None = None
        for i, ch in enumerate(line):
            if quote is not None:
                if ch == quote:
                    quote = None
            elif ch in "'\"":
                quote = ch
            elif ch == "#":
                cut = i
                break
        out.append(line if cut is None else line[:cut])
    return "\n".join(out)


def ci_overrides(text: str) -> list[dict[str, object]]:
    """`[[profile.ci.overrides]]` blocks IN FILE ORDER, comments removed.

    Order matters: nextest applies the FIRST override that both matches a test
    and specifies a setting, so a preceding block that sets `retries` shadows a
    later quarantine. Only `--with-nextest` can resolve that (it needs cargo to
    say which tests a filterset selects); the static check uses the order only
    to report the blocks it read.
    """
    raw: list[list[str]] = []
    cur: list[str] | None = None
    for line in strip_toml_comments(text).splitlines():
        if line.startswith("["):
            if cur is not None:
                raw.append(cur)
            cur = [] if line.startswith("[[profile.ci.overrides]]") else None
            continue
        if cur is not None:
            cur.append(line)
    if cur is not None:
        raw.append(cur)

    out: list[dict[str, object]] = []
    for block in raw:
        body = "\n".join(block)
        fm = FILTER_RE.search(body)
        rm = RETRIES_RE.search(body)
        out.append(
            {
                "filter": fm.group("expr") if fm else None,
                "retries": int(rm.group("n")) if rm else None,
            }
        )
    return out


def check_nextest(text: str) -> list[str]:
    """R2: every quarantined test must be SELECTED by an override at retries = 0.

    Presence of the name in the block is NOT coverage, and treating it as such
    is how the first version of this gate passed both of these:

      * the names present only as `#` comments inside an unrelated
        `binary(=pipeline_test)` override — comments are stripped now;
      * `test(=the_bash_timeout_bounds_the_secret_deny_walk)`, the module-path-less
        spelling, which is a valid filterset that selects ZERO tests and leaves
        the quarantine vacuous — operands are compared to the exact nextest name
        now, and the module-path case is called out by name in the message.

    What this still cannot do without cargo: prove the pinned names exist, and
    prove no EARLIER override sets `retries` for them first. That is
    `--with-nextest`.
    """
    covered: set[str] = set()
    for ov in ci_overrides(text):
        expr = ov["filter"]
        if ov["retries"] != 0 or not isinstance(expr, str):
            continue
        covered |= set(TEST_LITERAL_RE.findall(expr))
    missing = [t for t in QUARANTINED_TESTS if t not in covered]
    if not missing:
        return []
    shorts = sorted({t.rsplit("::", 1)[-1] for t in missing} & covered)
    hint = (
        f" — {shorts} appears without its module path, which is a filterset "
        f"that selects ZERO tests"
        if shorts
        else ""
    )
    return [
        f"{NEXTEST_CONFIG}: no `[[profile.ci.overrides]]` with `retries = 0` "
        f"selects {missing}{hint} — `[profile.ci] retries = 2` launders their "
        f"churn into a green run conclusion (gh#1146)"
    ]


def nextest_select(root: str, expr: str) -> set[str]:
    """The test names `cargo nextest list -E <expr>` selects under profile `ci`.

    nextest writes the build log to stderr and the selection to stdout, one
    `<binary-id> <test name>` per line, so stdout parses without filtering.
    """
    proc = subprocess.run(
        ["cargo", "nextest", "list", "--profile", "ci", "-E", expr],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"`cargo nextest list -E '{expr}'` exited {proc.returncode}:\n"
            f"{proc.stderr[-2000:]}"
        )
    return {ln.split(" ", 1)[1] for ln in proc.stdout.splitlines() if " " in ln}


def check_nextest_live(root: str) -> list[str]:
    """R2 with cargo as the oracle: the tests are real AND land at retries = 0.

    Two properties the text scan cannot reach:
      1. each pinned name selects a test at all (a rename or a module move
         leaves the filterset syntactically fine and semantically empty);
      2. the first override that specifies `retries` for that test specifies 0
         — nextest resolves each setting from the first matching override, so
         an earlier block would shadow the quarantine silently.
    """
    pinned = " + ".join(f"test(={t})" for t in QUARANTINED_TESTS)
    selected = nextest_select(root, pinned)
    stale = [t for t in QUARANTINED_TESTS if t not in selected]
    if stale:
        # No point resolving overrides against names that select nothing.
        return [
            f"{NEXTEST_CONFIG}: `test(={t})` selects no test in this workspace — "
            f"the pinned nextest name is stale (gh#1146)"
            for t in stale
        ]

    with open(os.path.join(root, NEXTEST_CONFIG), encoding="utf-8") as fh:
        overrides = ci_overrides(fh.read())
    decided: dict[str, int] = {}
    for ov in overrides:
        expr, retries = ov["filter"], ov["retries"]
        if not isinstance(expr, str) or not isinstance(retries, int):
            continue
        for t in nextest_select(root, f"({expr}) & ({pinned})"):
            decided.setdefault(t, retries)

    violations = []
    for t in QUARANTINED_TESTS:
        got = decided.get(t)
        if got != 0:
            got_txt = "no override at all" if got is None else f"retries = {got}"
            violations.append(
                f"{NEXTEST_CONFIG}: `{t}` resolves to {got_txt} under profile "
                f"`ci` — the first override that sets `retries` for it wins, and "
                f"it is not the quarantine (gh#1146)"
            )
    return violations


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

    # ── the two decoys the substring scan this replaced accepted ───────────
    # DECOY A: the module-path-less spelling. Syntactically valid, selects zero
    # tests, so the quarantine is vacuous and the tests stay at retries = 2.
    shorts = " + ".join(f"test(={t.rsplit('::', 1)[-1]})" for t in QUARANTINED_TESTS)
    v = check_nextest(f"[[profile.ci.overrides]]\nfilter = '{shorts}'\nretries = 0\n")
    check(
        "module-path-less spelling must fire and say why",
        len(v) == 1
        and "ZERO tests" in v[0]
        and all(t in v[0] for t in QUARANTINED_TESTS if "::" in t),
    )

    # DECOY B: the names present only as comments inside an unrelated override.
    commented = (
        "[[profile.ci.overrides]]\nfilter = 'binary(=pipeline_test)'\n"
        + "".join(f"# {t}\n" for t in QUARANTINED_TESTS)
        + "retries = 0\n"
    )
    check("names in comments must not cover", len(check_nextest(commented)) == 1)

    # A trailing comment must not be able to smuggle an operand in either,
    # while a `#` inside the quoted filter must survive.
    trailing = (
        f"[[profile.ci.overrides]]\nfilter = 'binary(=x)'  # test(={QUARANTINED_TESTS[0]})\n"
        "retries = 0\n"
    )
    check("trailing comment must not cover", len(check_nextest(trailing)) == 1)
    check(
        "quoted # must survive stripping",
        strip_toml_comments("filter = 'test(=a#b)'") == "filter = 'test(=a#b)'",
    )

    if failures:
        for f in failures:
            print(f"SELF-TEST FAIL: {f}")
        return 1
    print("self-test OK: classifier, R1 (both directions), R2 (both directions)")
    return 0


def live_scan(root: str) -> int:
    """`--with-nextest`: resolve the filtersets with cargo instead of trusting
    the spelling. Needs a built workspace, so it is NOT part of the text-only
    gate that runs in CI — `just check-windows-attribution-live` is the way to
    run it, and it is what must be re-run whenever one of the three tests is
    renamed or moved."""
    try:
        violations = check_nextest_live(root)
    except RuntimeError as exc:
        print(f"VIOLATION: {exc}")
        print("\nFAIL: 1 Windows-attribution violation(s)")
        return 1
    if violations:
        print()
        for v in violations:
            print(f"VIOLATION: {v}")
        print(f"\nFAIL: {len(violations)} Windows-attribution violation(s)")
        return 1
    print(
        f"OK (live): cargo nextest resolves all {len(QUARANTINED_TESTS)} "
        f"churning test(s) to retries=0 under profile `ci`"
    )
    return 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    root = REPO_ROOT
    for i, a in enumerate(argv):
        if a == "--root":
            root = argv[i + 1]
    if "--with-nextest" in argv:
        return live_scan(root)
    return scan(root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
