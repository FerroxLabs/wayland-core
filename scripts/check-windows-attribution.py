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
  R3  The aggregate `report` job — the REQUIRED status context, and therefore
      the thing everyone actually reads — states whether Windows was exercised
      at all. On PR #341 `CI (windows-latest, hosted)` was SKIPPED and the
      overall report read as passing: a skipped leg contributes no red, so it
      contributes to a green it never earned. This is checked BOTH ways: the
      wiring (the `report` job depends on the hosted leg and runs the
      annotation step with both leg results, under `if: always()`) and the
      BEHAVIOUR (the annotation script is executed against five fixture states,
      because a script that always said "exercised" would satisfy every grep).

R3 is enforced on LINUX on purpose. A coverage check that itself runs on the
Windows leg would be skipped by the same mechanism that produced the unearned
green, so it could never fire in the case it exists for. It also ANNOTATES
rather than fails: hosted Windows is separately red on main's own byte-identical
tree, and gating main on that would block every merge. The defect being closed
is the honesty of the conclusion, not the redness of the platform.

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
import tempfile

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

RECORD_STEP_NAME = "Record which Windows executor served this job"

# The step has to do three things, and each is checked, because a step that
# merely echoes into the log satisfies none of the reasons it exists:
#   RUNNER_NAME          — the executor identity itself.
#   exit 1               — fail closed; an anonymous job is worse than a red one.
#   GITHUB_STEP_SUMMARY  — readable from `gh run view` without downloading
#                          logs, so an A/B pair is one command, not two archives.
REQUIRED_IN_STEP = ("RUNNER_NAME", "exit 1", "GITHUB_STEP_SUMMARY")

CI_WORKFLOW = ".github/workflows/ci.yml"

# R3. `report` is the required status context; `ci-windows-hosted` is the leg
# that was SKIPPED while it read as passing. The annotation lives in a script so
# it can be executed by this gate rather than only grepped for.
REPORT_JOB = "report"
HOSTED_WINDOWS_JOB = "ci-windows-hosted"
ANNOTATE_SCRIPT = ".github/scripts/annotate-windows-coverage.sh"
ANNOTATE_STEP_NAME = "Annotate Windows coverage (a skipped leg is not a pass)"

# Each token is load-bearing:
#   the script path                    — the annotation is actually run;
#   needs.ci.result                    — the self-hosted matrix leg's result;
#   needs.ci-windows-hosted.result     — the leg the unearned green came from;
#   always()                           — a failing evidence gate earlier in the
#                                        job must not suppress the coverage
#                                        statement, which is exactly when it
#                                        matters most.
REQUIRED_IN_ANNOTATE_STEP = (
    ANNOTATE_SCRIPT,
    "needs.ci.result",
    f"needs.{HOSTED_WINDOWS_JOB}.result",
    "always()",
)

# The two verdicts the annotation may reach. `Windows: not exercised` does not
# contain `Windows: exercised`, so the pair tests in both directions.
EXERCISED_MARKER = "Windows: exercised"
NOT_EXERCISED_MARKER = "Windows: not exercised"

WINDOWS_TEST_WORKFLOWS = (
    CI_WORKFLOW,
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


def check_report_coverage(path: str, text: str) -> list[str]:
    """R3 wiring: `report` must be able to tell a SKIPPED Windows leg from a pass.

    Two things have to be true and neither implies the other. The job has to
    DEPEND on the hosted Windows leg — without that, `needs.ci-windows-hosted`
    does not resolve and the skip is invisible — and it has to RUN the
    annotation with both leg results. Deleting either one restores the exact
    state #1146 measured: a green `report` covering no Windows test.
    """
    jobs = dict(split_jobs(text))
    if REPORT_JOB not in jobs:
        return [
            f"{path}: no `{REPORT_JOB}` job — the required status context that "
            f"has to state whether Windows was exercised is gone (gh#1146)"
        ]
    job = jobs[REPORT_JOB]
    violations: list[str] = []

    if HOSTED_WINDOWS_JOB not in key_block(job, "needs"):
        violations.append(
            f"{path}: job `{REPORT_JOB}` does not `needs:` `{HOSTED_WINDOWS_JOB}`, "
            f"so that leg being SKIPPED is invisible to it and goes on quietly "
            f"contributing to a green it never earned (gh#1146)"
        )

    body = step_body(job, ANNOTATE_STEP_NAME)
    if body is None:
        violations.append(
            f"{path}: job `{REPORT_JOB}` has no `{ANNOTATE_STEP_NAME}` step — its "
            f"conclusion cannot distinguish a Windows leg that PASSED from one "
            f"that was never run (gh#1146)"
        )
        return violations

    missing = [tok for tok in REQUIRED_IN_ANNOTATE_STEP if tok not in body]
    if missing:
        violations.append(
            f"{path}: job `{REPORT_JOB}`'s `{ANNOTATE_STEP_NAME}` step is missing "
            f"{missing} — it must run `{ANNOTATE_SCRIPT}`, be handed BOTH Windows "
            f"leg results, and carry `if: always()` so a failing evidence gate "
            f"cannot suppress the coverage statement (gh#1146)"
        )
    return violations


# A junit that certifies something, and the one nextest writes for a filterset
# that matched nothing — the same tests=0 file .github/scripts/assert-test-evidence.sh
# documents. A Windows artifact holding zero test cases is not Windows coverage.
_JUNIT_ONE = (
    '<?xml version="1.0" encoding="UTF-8"?>\n<testsuites name="nextest-run" tests="1">\n'
    '<testsuite name="s" tests="1"><testcase name="t" classname="c" time="0.1"/></testsuite>\n'
    "</testsuites>\n"
)
_JUNIT_EMPTY = (
    '<?xml version="1.0" encoding="UTF-8"?>\n'
    '<testsuites name="nextest-run" tests="0" failures="0" errors="0" time="0.000">\n</testsuites>\n'
)

# (label, {artifact dir: junit body} or None, needs.ci-windows-hosted.result,
#  must say exercised).  `None` means the evidence directory does not exist at
#  all — `download-artifact` runs under `continue-on-error` and leaves no
#  directory when nothing matched, which is the shape a run with no uploads
#  actually has.
#
# Case 2 is the defect verbatim: macOS and Linux reported, Windows skipped, and
# the aggregate read as a pass. Every negative case is paired with a positive
# one over the same code, so a script that ALWAYS said "not exercised" fails
# this too — a gate that cannot pass is as worthless as one that cannot fail.
_ANNOTATE_CASES = (
    ("no evidence directory at all, hosted leg skipped", None, "skipped", False),
    ("no Windows report at all, hosted leg skipped", {}, "skipped", False),
    (
        "only macOS + Linux reported, hosted leg skipped",
        {"nextest-junit-macos-latest": _JUNIT_ONE, "nextest-junit-linux-containerized": _JUNIT_ONE},
        "skipped",
        False,
    ),
    ("Windows junit present but declares zero tests", {"nextest-junit-Array": _JUNIT_EMPTY}, "skipped", False),
    ("self-hosted Windows leg reported tests", {"nextest-junit-Array": _JUNIT_ONE}, "skipped", True),
    (
        "hosted Windows leg reported tests",
        {"nextest-junit-windows-latest-hosted": _JUNIT_ONE},
        "success",
        True,
    ),
)


def check_annotate_behaviour(script: str) -> list[str]:
    """R3 behaviour: EXECUTE the annotation and pin what it says, both ways.

    The wiring check above is a set of greps, and a script that hardcoded
    "Windows: exercised" would satisfy every one of them. So the script itself
    is run against the fixture states in `_ANNOTATE_CASES`, and its verdict —
    read from stdout AND from the step summary it writes — is compared to what
    that state means. It must also always exit 0: this step annotates the
    conclusion, it does not gate main.
    """
    if not os.path.exists(script):
        return [
            f"{ANNOTATE_SCRIPT}: missing — the `{REPORT_JOB}` job's Windows "
            f"coverage annotation has no implementation (gh#1146)"
        ]

    violations: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, (label, dirs, hosted_result, want_exercised) in enumerate(_ANNOTATE_CASES):
            evidence = os.path.join(tmp, f"case{i}")
            if dirs is not None:
                os.makedirs(evidence, exist_ok=True)
            for name, body in (dirs or {}).items():
                os.makedirs(os.path.join(evidence, name), exist_ok=True)
                with open(os.path.join(evidence, name, "junit.xml"), "w", encoding="utf-8") as fh:
                    fh.write(body)
            summary = os.path.join(tmp, f"summary{i}.md")
            env = dict(os.environ)
            env.update(
                {
                    "EVIDENCE_DIR": evidence,
                    "SELF_HOSTED_RESULT": "success",
                    "HOSTED_RESULT": hosted_result,
                    "GITHUB_STEP_SUMMARY": summary,
                }
            )
            proc = subprocess.run(["bash", script], capture_output=True, text=True, env=env)
            said = proc.stdout + proc.stderr
            if os.path.exists(summary):
                with open(summary, encoding="utf-8") as fh:
                    said += fh.read()
            if proc.returncode != 0:
                violations.append(
                    f"{ANNOTATE_SCRIPT}: `{label}` exited {proc.returncode} — this "
                    f"step annotates the conclusion and must never fail the "
                    f"`{REPORT_JOB}` job (gh#1146)"
                )
            no = NOT_EXERCISED_MARKER in said
            yes = EXERCISED_MARKER in said
            want, got = (
                (EXERCISED_MARKER, NOT_EXERCISED_MARKER) if want_exercised else (NOT_EXERCISED_MARKER, EXERCISED_MARKER)
            )
            # NAMING THE SKIP IS THE FIX. "Windows: exercised" over a run
            # where one of the two legs never ran is the PR #341 shape one
            # notch quieter, so a skipped leg has to be called out whichever
            # verdict the run reaches.
            if hosted_result == "skipped" and "SKIPPED" not in said:
                violations.append(
                    f"{ANNOTATE_SCRIPT}: `{label}` never says the hosted Windows "
                    f"leg was SKIPPED — an omitted leg reads as coverage it did "
                    f"not provide (gh#1146)"
                )
            if (yes and not no) != want_exercised:
                violations.append(
                    f"{ANNOTATE_SCRIPT}: `{label}` must report `{want}` and not "
                    f"`{got}` — it said exercised={yes} not_exercised={no}, so the "
                    f"`{REPORT_JOB}` conclusion does not distinguish a skipped "
                    f"Windows leg from a passing one (gh#1146)"
                )
    return violations


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
    ci_text = ""
    for rel in WINDOWS_TEST_WORKFLOWS:
        path = os.path.join(root, rel)
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        if rel == CI_WORKFLOW:
            ci_text = text
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

    # R3 — wiring, then behaviour. Both, because neither implies the other.
    violations += check_report_coverage(CI_WORKFLOW, ci_text)
    violations += check_annotate_behaviour(os.path.join(root, ANNOTATE_SCRIPT))

    if violations:
        print()
        for v in violations:
            print(f"VIOLATION: {v}")
        print(f"\nFAIL: {len(violations)} Windows-attribution violation(s)")
        return 1
    print(
        f"OK: {seen} Windows job(s) record their executor; "
        f"{len(QUARANTINED_TESTS)} churning test(s) at retries=0; "
        f"`{REPORT_JOB}` states Windows coverage over "
        f"{len(_ANNOTATE_CASES)} fixture state(s)"
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


# ── R3 fixtures ────────────────────────────────────────────────────────────

_GOOD_ANNOTATE = (
    "      - name: " + ANNOTATE_STEP_NAME + "\n"
    "        if: always()\n"
    "        env:\n"
    "          EVIDENCE_DIR: junit-reports\n"
    "          SELF_HOSTED_RESULT: ${{ needs.ci.result }}\n"
    "          HOSTED_RESULT: ${{ needs.ci-windows-hosted.result }}\n"
    "        run: bash " + ANNOTATE_SCRIPT + "\n"
)

_REPORT_WF = """name: x

on:
  push:

jobs:
  report:
    name: report
{needs}    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
{annotate}      - name: Assert test evidence exists (a skipped test step is not a pass)
        run: bash .github/scripts/assert-test-evidence.sh
"""


def _report_wf(
    annotate: str = _GOOD_ANNOTATE,
    needs: str = "    needs: [ci, ci-windows-hosted]\n",
) -> str:
    return _REPORT_WF.format(annotate=annotate, needs=needs)


# Stubs that satisfy every grep in check_report_coverage while saying one thing
# unconditionally — the shape a wiring-only gate cannot see.
_STUB_ALWAYS = """#!/usr/bin/env bash
echo "CI (windows-latest, hosted) : SKIPPED"
echo "{marker}"
echo "{marker}" >> "$GITHUB_STEP_SUMMARY"
exit 0
"""


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

    # ── R3 wiring, both directions ────────────────────────────────────────
    check("clean report fixture must not fire", check_report_coverage("f.yml", _report_wf()) == [])
    v = check_report_coverage("f.yml", _report_wf(needs="    needs: ci\n"))
    check(
        "report without the hosted Windows leg in needs must fire",
        len(v) == 1 and HOSTED_WINDOWS_JOB in v[0],
    )
    v = check_report_coverage("f.yml", _report_wf(annotate=""))
    check(
        "report without the annotation step must fire",
        len(v) == 1 and ANNOTATE_STEP_NAME in v[0],
    )
    # A step that runs the script but is never told the hosted leg's result
    # cannot say a skipped leg was skipped.
    blind = _GOOD_ANNOTATE.replace(
        "          HOSTED_RESULT: ${{ needs.ci-windows-hosted.result }}\n", ""
    )
    v = check_report_coverage("f.yml", _report_wf(annotate=blind))
    check(
        "annotation blind to the hosted leg result must fire",
        len(v) == 1 and f"needs.{HOSTED_WINDOWS_JOB}.result" in v[0],
    )
    # ...and one an earlier failing gate can suppress is not a guarantee.
    suppressible = _GOOD_ANNOTATE.replace("        if: always()\n", "")
    v = check_report_coverage("f.yml", _report_wf(annotate=suppressible))
    check(
        "annotation without always() must fire",
        len(v) == 1 and "always()" in v[0],
    )

    # ── R3 behaviour, both directions ─────────────────────────────────────
    # The repo's own script over all five states is the positive control; the
    # two stubs are the mutation arms a grep-only gate would have accepted.
    check(
        "the repo annotation script must satisfy every fixture state",
        check_annotate_behaviour(os.path.join(REPO_ROOT, ANNOTATE_SCRIPT)) == [],
    )
    negatives = sum(1 for c in _ANNOTATE_CASES if not c[3])
    positives = len(_ANNOTATE_CASES) - negatives
    with tempfile.TemporaryDirectory() as tmp:
        for marker, want, label in (
            (EXERCISED_MARKER, negatives, "always-exercised"),
            (NOT_EXERCISED_MARKER, positives, "always-not-exercised"),
        ):
            stub = os.path.join(tmp, f"{label}.sh")
            with open(stub, "w", encoding="utf-8") as fh:
                fh.write(_STUB_ALWAYS.format(marker=marker))
            v = check_annotate_behaviour(stub)
            check(f"{label} stub must fire on {want} state(s)", len(v) == want)
        # A script that fails the job is a gate, not an annotation: #1146 is
        # explicit that main must not be blocked on a known-red Windows test.
        failing = os.path.join(tmp, "failing.sh")
        with open(failing, "w", encoding="utf-8") as fh:
            fh.write(_STUB_ALWAYS.format(marker=NOT_EXERCISED_MARKER).replace("exit 0", "exit 1"))
        v = check_annotate_behaviour(failing)
        check(
            "a script that fails the report job must fire",
            any("must never fail" in x for x in v),
        )
    check(
        "a missing annotation script must fire",
        len(check_annotate_behaviour(os.path.join(REPO_ROOT, "no-such-script.sh"))) == 1,
    )

    if failures:
        for f in failures:
            print(f"SELF-TEST FAIL: {f}")
        return 1
    print(
        "self-test OK: classifier, R1 (both directions), R2 (both directions), "
        "R3 wiring + behaviour (both directions)"
    )
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
