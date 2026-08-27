#!/usr/bin/env python3
"""Re-measure a named set of tests with nextest retries DISABLED.

Why this exists
---------------
`.config/nextest.toml` sets `retries = 2` on the `ci` profile. nextest reports a
test that failed twice and passed on the third attempt as `FLAKY 3/3` and counts
it in the PASSED total. The run summary line then reads, truthfully and
uselessly, `13346 passed (6 flaky) ... 33 failed`.

Two things are wrong with reading that as "6 tests are flaky":

1. `FLAKY 3/3` is not evidence of nondeterminism. It is evidence of two
   consecutive failures. A test that needs attempt 3 every time is a test that
   fails every time, wearing a costume.
2. nextest prints NO output for a retried-then-passed attempt. Measured on CI
   run 30699019736 (Windows leg, job 91372229704): the log contains 6 `FLAKY`
   lines, 66 `TRY 3 FAIL` lines, and ZERO `TRY 1` / `TRY 2` lines. Every
   intermediate failure's diagnostic was discarded.

So the only way to grade these is to run them again with `--retries 0` and count.

What it measures
----------------
Three conditions, because "flaky" usually means "load-dependent" and the two
must be told apart:

  isolated   one nextest invocation per test, nothing else running
  batch      all named tests in one nextest invocation (mild self-concurrency)
  loaded     same as `batch`, with `--load` busy CPU hogs running alongside

Every invocation passes `--retries 0` and `--no-fail-fast`. A run is graded
PASS only if nextest reports exactly one test run and zero failures — a
zero-test invocation (a filter that matched nothing, a binary that compiled to
empty under this platform's cfg) is graded NOTRUN, never PASS.

Usage
-----
    python3 scripts/flake-ledger.py --runs 10 --out ledger.json
    python3 scripts/flake-ledger.py --runs 10 --conditions isolated
    python3 scripts/flake-ledger.py --runs 20 --conditions isolated \
        --tests 'wcore-tools::wcore-tools::bash::tests::the_bash_timeout_bounds_the_secret_deny_walk'

`DEFAULT_TARGETS` below is a DEFAULT, not a ceiling. `--tests` / `--targets-file`
point the same instrument at any other set — which is what gh#1146 needed: the
red arm it asks for (same tree, same box, N runs, retries off) could not be run
against the three tests it names, because they were not in the hardcoded table
and there was no way to add them. A triple that does not resolve grades NOTRUN
through the existing presence check, never PASS.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import platform
import re
import shlex
import subprocess
import sys
import time
from dataclasses import dataclass, field

# The six tests CI reported as `FLAKY` on the Windows leg.
#   run 30698794888 (job 91366049013): 2 flaky
#   run 30699019736 (job 91372229704): 6 flaky
# `crate::binary` is the nextest binary id; `test` is the exact test name.
DEFAULT_TARGETS: list[tuple[str, str, str]] = [
    (
        "wcore-cli",
        "deterministic_openai_loop",
        "packaged_core_recovers_after_two_503_responses",
    ),
    (
        "wcore-swarm",
        "dispatch_smoke",
        "required_live_windows_public_dispatch_refuses_bash_worker_and_preserves_parent_and_sibling_state",
    ),
    (
        "wcore-swarm",
        "dispatch_smoke",
        "malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic",
    ),
    (
        "wcore-swarm",
        "dispatch_smoke",
        "public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state",
    ),
    ("wcore-swarm", "dispatch_smoke", "dispatches_4_noop_workers_in_parallel"),
    (
        "wcore-swarm",
        "worker_runtime_limits",
        "multi_worker_output_exhaustion_fails_without_retaining_buffers",
    ),
]

# `Summary [   1.234s] 1 test run: 1 passed, 0 skipped`
SUMMARY_RE = re.compile(
    r"Summary \[\s*[\d.]+s\]\s+(\d+)\s+tests? run:\s+(\d+)\s+passed"
    r"(?:.*?(\d+)\s+failed)?"
)

# nextest colours its output whenever it believes the sink can take it, and on
# the hosted Windows runner it did so even through a pipe. The escape sequences
# land INSIDE the summary line — `Summary\x1b[0m [ 6.7s] \x1b[1m1\x1b[0m test
# run` — so an un-stripped regex matches nothing and every run is graded ERROR.
# Measured on run 30727452188: all 180 repetitions came back ERROR with the
# tests themselves plainly reporting `1 test run: 1 passed` or `1 failed`.
# `--color never` is passed as well; this is the belt to that's braces, because
# the failure mode of getting this wrong is a ledger that grades real results
# unreadable.
ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")


# `crates/wcore-swarm/tests/common/mod.rs` lets these tests SKIP when no sandbox
# backend can host delegated dispatch, and a skip returns early — so the test
# PASSES. nextest captures a passing test's output, so the skip is invisible in
# the run that matters; the module therefore appends a record to this file
# inside the cargo target dir. Reading it back is the only way this harness can
# tell "ran and passed" from "declined to run and reported a pass".
#
# This matters most under exactly the condition being measured: the Windows
# AppContainer backend's availability probe is a real spawn, and a spawn probe
# that fails under load resolves the registry to `fail_closed`, which
# `admit_delegated_backend` rejects. Load can therefore convert these tests into
# vacuous greens rather than failures, and a ledger that could not see that
# would report the wrong verdict in the safest-looking direction.
SKIP_LEDGER_NAME = "swarm-delegated-skips.txt"


@dataclass
class Outcome:
    verdict: str  # PASS | FAIL | SKIPPED | NOTRUN | ERROR
    seconds: float
    rc: int
    detail: str = ""


@dataclass
class Row:
    key: str
    package: str
    binary: str
    test: str
    results: dict[str, list[Outcome]] = field(default_factory=dict)


def filterset(package: str, binary: str, test: str) -> str:
    return f"package({package}) and binary({binary}) and test(={test})"


def parse_target_spec(spec: str) -> tuple[str, str, str]:
    """`PKG::BINARY::TEST` -> the triple `DEFAULT_TARGETS` holds.

    Split on the FIRST TWO separators only: a unit test inside a lib carries
    its module path in the test name (`bash::tests::the_...`) and the lib's own
    binary id is the package name, so the honest spelling of one is
    `wcore-tools::wcore-tools::bash::tests::the_...` — five components, three
    fields.

    Refuses a two-component spec instead of guessing a binary. `package(x) and
    binary(y)` with the wrong `y` selects nothing, and a filter that selects
    nothing is graded NOTRUN, which reads like a platform-absent test rather
    than like a typo.
    """
    parts = spec.split("::", 2)
    if len(parts) != 3 or not all(p.strip() for p in parts):
        raise ValueError(
            f"target must be PKG::BINARY::TEST with three non-empty parts, got {spec!r}"
        )
    p, b, t = (part.strip() for part in parts)
    return p, b, t


def parse_targets(tests: list[str], targets_file: str) -> list[tuple[str, str, str]]:
    """Every triple named on the command line, in order, deduplicated.

    Empty return means "the caller supplied none", which is what selects
    `DEFAULT_TARGETS`.
    """
    specs: list[str] = []
    for chunk in tests:
        specs += chunk.replace("\n", ",").split(",")
    if targets_file:
        with open(targets_file, encoding="utf-8") as fh:
            for line in fh:
                line = line.split("#", 1)[0]
                specs.append(line)
    out: list[tuple[str, str, str]] = []
    for spec in specs:
        if not spec.strip():
            continue
        triple = parse_target_spec(spec)
        if triple not in out:
            out.append(triple)
    return out


def parse_summary(text: str) -> tuple[int, int, int] | None:
    m = SUMMARY_RE.search(text)
    if not m:
        return None
    run = int(m.group(1))
    passed = int(m.group(2))
    failed = int(m.group(3) or 0)
    return run, passed, failed


def list_count(root: str, profile: str, expr: str) -> int:
    """How many tests this filter actually selects on THIS platform.

    A target can be absent here and present elsewhere — the Windows-only
    `required_live_windows_*` case is exactly that. Asking nextest instead of
    assuming is what keeps an absent test graded NOTRUN rather than silently
    shrinking the batch's expected count into an ERROR.
    """
    proc = subprocess.run(
        ["cargo", "nextest", "list", "--profile", profile, "-E", expr],
        cwd=root,
        capture_output=True,
        text=True,
        errors="replace",
    )
    if proc.returncode != 0:
        return -1
    # `nextest list` writes ONE flat `binary-id test-name` line per test to
    # stdout (verified on 0.9.x: `wcore-swarm::dispatch_smoke <name>`); every
    # compile message goes to stderr. So a non-empty stdout line is a test.
    return sum(1 for line in proc.stdout.splitlines() if line.strip())


def skip_ledger_path(root: str) -> str:
    return os.path.join(
        os.environ.get("CARGO_TARGET_DIR") or os.path.join(root, "target"),
        SKIP_LEDGER_NAME,
    )


def read_skips(root: str) -> str:
    try:
        with open(skip_ledger_path(root), errors="replace") as fh:
            return fh.read()
    except OSError:
        return ""


def run_nextest(
    root: str, profile: str, expr: str, expected: int, timeout: int
) -> tuple[Outcome, str]:
    # Clear the skip ledger so what it holds afterwards belongs to THIS run.
    try:
        os.remove(skip_ledger_path(root))
    except OSError:
        pass
    cmd = [
        "cargo",
        "nextest",
        "run",
        "--profile",
        profile,
        "--retries",
        "0",
        "--no-fail-fast",
        "--no-tests",
        "fail",
        "--color",
        "never",
        "-E",
        expr,
    ]
    started = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            cwd=root,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return (
            Outcome("ERROR", time.monotonic() - started, -1, f"timeout after {timeout}s"),
            "",
        )
    elapsed = time.monotonic() - started
    blob = ANSI_RE.sub("", proc.stdout + proc.stderr)
    summary = parse_summary(blob)
    if summary is None:
        # No summary line at all: nextest never got to running tests (compile
        # error, filter error, runner error). Never grade this as a pass.
        tail = "\n".join(blob.strip().splitlines()[-15:])
        return Outcome("ERROR", elapsed, proc.returncode, tail), blob
    run, passed, failed = summary
    if run == 0:
        return Outcome("NOTRUN", elapsed, proc.returncode, "0 tests run"), blob
    if run != expected:
        return (
            Outcome(
                "ERROR",
                elapsed,
                proc.returncode,
                f"expected {expected} tests, nextest ran {run}",
            ),
            blob,
        )
    skips = read_skips(root)
    if failed == 0 and passed == expected:
        if skips.strip():
            # It "passed" by declining to run. That is not a pass.
            return (
                Outcome(
                    "SKIPPED",
                    elapsed,
                    proc.returncode,
                    " | ".join(skips.split("\n"))[:400],
                ),
                blob + "\n=== delegated skip ledger ===\n" + skips,
            )
        return Outcome("PASS", elapsed, proc.returncode), blob
    # The per-test result line carries a `(n/m)` progress counter between the
    # duration and the binary id — `FAIL [ 0.583s] (1/1) wcore-swarm::x name` —
    # so anything matching on it must tolerate that field. The whole line is
    # kept, which is what lets the batch pass attribute a failure to a row by
    # searching for the test's name inside it.
    fails = [
        line.strip()
        for line in blob.splitlines()
        if line.strip().startswith(("FAIL [", "TRY", "SIGSEGV", "TIMEOUT [", "ABORT ["))
    ]
    return Outcome("FAIL", elapsed, proc.returncode, "; ".join(fails[:6])), blob


class Load:
    """N busy-spin child processes, to reproduce suite-level CPU contention.

    Each worker carries its OWN deadline and exits on its own. `__exit__` kills
    them in the normal path, but a `SIGKILL`/crash of this process skips
    `__exit__` entirely, and an orphaned busy-spinner pins a core forever. This
    harness runs on a machine somebody else owns, so the lifetime bound lives
    inside the child where nothing can skip it.
    """

    def __init__(self, workers: int, lifetime_s: int) -> None:
        self.workers = workers
        self.lifetime_s = lifetime_s
        self.procs: list[subprocess.Popen] = []

    def __enter__(self) -> "Load":
        spin = (
            "import time;"
            f"end=time.monotonic()+{self.lifetime_s};"
            "\nwhile time.monotonic()<end: pass"
        )
        for _ in range(self.workers):
            self.procs.append(
                subprocess.Popen(
                    [sys.executable, "-c", spin],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
            )
        return self

    def __exit__(self, *_exc: object) -> None:
        for p in self.procs:
            p.kill()
        for p in self.procs:
            p.wait()


def self_test() -> int:
    """Prove the target parser before anyone measures with it.

    A misparsed triple does not crash: it selects zero tests and grades NOTRUN,
    which is indistinguishable from a platform-absent test. So the parser is
    checked directly, in both directions.
    """
    import tempfile

    failures: list[str] = []

    def check(label: str, cond: bool) -> None:
        if not cond:
            failures.append(label)

    check("plain triple", parse_target_spec("a::b::c") == ("a", "b", "c"))
    check(
        "module path stays in the test field",
        parse_target_spec("wcore-tools::wcore-tools::bash::tests::x")
        == ("wcore-tools", "wcore-tools", "bash::tests::x"),
    )
    for bad in ("a::b", "", "   ", "a::b::", "::b::c", "a::::c"):
        try:
            parse_target_spec(bad)
        except ValueError:
            pass
        else:
            failures.append(f"{bad!r} must be refused")

    check(
        "comma separated, order preserved",
        parse_targets(["a::b::c, d::e::f"], "") == [("a", "b", "c"), ("d", "e", "f")],
    )
    check("repeatable", parse_targets(["a::b::c", "d::e::f"], "") == [("a", "b", "c"), ("d", "e", "f")])
    check("deduplicated", parse_targets(["a::b::c", "a::b::c"], "") == [("a", "b", "c")])
    check("empty selection falls back", parse_targets([], "") == [])

    with tempfile.TemporaryDirectory() as td:
        path = os.path.join(td, "targets.txt")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("# a comment\n\na::b::c   # trailing\n\nd::e::f\n")
        check("targets-file", parse_targets([], path) == [("a", "b", "c"), ("d", "e", "f")])

    # The spelling is the whole point: this exact string is what nextest is
    # asked for, and a wrong one selects nothing and grades NOTRUN.
    check(
        "filterset spelling",
        filterset("wcore-tools", "wcore-tools", "bash::tests::x")
        == "package(wcore-tools) and binary(wcore-tools) and test(=bash::tests::x)",
    )

    if failures:
        for f in failures:
            print(f"SELF-TEST FAIL: {f}")
        return 1
    print("self-test OK: target parser, both directions")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=10)
    ap.add_argument("--profile", default="ci")
    ap.add_argument("--root", default=".")
    ap.add_argument("--timeout", type=int, default=900)
    ap.add_argument(
        "--conditions",
        default="isolated,batch,loaded",
        help="comma-separated subset of isolated,batch,loaded",
    )
    ap.add_argument("--load", type=int, default=0, help="busy workers for `loaded` (0 = num-cpus)")
    ap.add_argument("--out", default="flake-ledger.json")
    ap.add_argument("--logdir", default="")
    ap.add_argument(
        "--tests",
        action="append",
        default=[],
        metavar="PKG::BINARY::TEST",
        help="target triple(s); repeatable, or comma/newline separated. "
        "Replaces DEFAULT_TARGETS when given.",
    )
    ap.add_argument(
        "--targets-file",
        default="",
        help="file of PKG::BINARY::TEST lines; '#' comments and blanks ignored",
    )
    ap.add_argument("--self-test", action="store_true", help="check the parser and exit")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    conditions = [c.strip() for c in args.conditions.split(",") if c.strip()]
    load_workers = args.load or (os.cpu_count() or 4)
    try:
        selected = parse_targets(args.tests, args.targets_file)
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    source = "--tests/--targets-file" if selected else "DEFAULT_TARGETS"
    targets = selected or DEFAULT_TARGETS
    rows = [Row(f"{p}::{b} {t}", p, b, t) for (p, b, t) in targets]
    if args.logdir:
        os.makedirs(args.logdir, exist_ok=True)

    print(f"host={platform.node()} os={platform.system()} {platform.release()}")
    print(f"cpus={os.cpu_count()} profile={args.profile} runs={args.runs} retries=0")
    print(f"targets={len(targets)} source={source}")
    print(f"conditions={conditions} load_workers={load_workers}")
    print("", flush=True)

    def record(row: Row, cond: str, out: Outcome, blob: str, i: int) -> None:
        row.results.setdefault(cond, []).append(out)
        print(
            f"  [{cond}] {row.test[:64]:<64} run {i + 1:>2}/{args.runs}"
            f"  {out.verdict:<6} {out.seconds:6.1f}s"
            + (f"  {out.detail[:160]}" if out.detail else ""),
            flush=True,
        )
        if args.logdir and out.verdict != "PASS" and blob:
            name = f"{cond}-{row.test[:60]}-{i + 1}.log"
            with open(os.path.join(args.logdir, name), "w", errors="replace") as fh:
                fh.write(blob)

    # Which targets exist on THIS platform. A target that does not exist is
    # recorded as NOTRUN in every condition and excluded from the batch, so an
    # absent test can never render as a pass and can never turn the batch's
    # own count into a spurious ERROR.
    present: list[Row] = []
    for row in rows:
        n = list_count(args.root, args.profile, filterset(row.package, row.binary, row.test))
        if n == 1:
            present.append(row)
        else:
            reason = "filter selected 0 tests on this platform" if n == 0 else f"nextest list failed (n={n})"
            print(f"  ABSENT  {row.test[:70]:<70} {reason}")
            for cond in conditions:
                row.results[cond] = [Outcome("NOTRUN", 0.0, 0, reason)] * args.runs
    print(f"present targets: {len(present)}/{len(rows)}\n", flush=True)

    if "isolated" in conditions:
        print("== condition: isolated ==")
        for row in present:
            for i in range(args.runs):
                out, blob = run_nextest(
                    args.root,
                    args.profile,
                    filterset(row.package, row.binary, row.test),
                    1,
                    args.timeout,
                )
                record(row, "isolated", out, blob, i)

    batch_expr = " or ".join(
        f"({filterset(r.package, r.binary, r.test)})" for r in present
    )

    def batch_pass(cond: str, loaded: bool = False) -> None:
        print(f"== condition: {cond} ==")
        if not present:
            return
        for i in range(args.runs):
            # A fresh, short-lived Load per repetition: even an abandoned set of
            # spinners expires within one repetition's budget.
            ctx = (
                Load(load_workers, args.timeout + 60)
                if loaded
                else contextlib.nullcontext()
            )
            with ctx:
                out, blob = run_nextest(
                    args.root, args.profile, batch_expr, len(present), args.timeout
                )
            # A batch invocation grades the SET. Attribute per test from the log.
            for row in present:
                if out.verdict in ("ERROR", "NOTRUN"):
                    per = Outcome(out.verdict, out.seconds, out.rc, out.detail)
                elif out.verdict == "SKIPPED":
                    per = Outcome(
                        "SKIPPED" if row.test in out.detail else "PASS",
                        out.seconds,
                        out.rc,
                        out.detail,
                    )
                elif row.test in out.detail:
                    per = Outcome("FAIL", out.seconds, out.rc, out.detail)
                elif out.verdict == "FAIL":
                    per = Outcome("PASS", out.seconds, out.rc, "(other test in batch failed)")
                else:
                    per = Outcome("PASS", out.seconds, out.rc)
                record(row, cond, per, blob if per.verdict != "PASS" else "", i)

    if "batch" in conditions:
        batch_pass("batch")
    if "loaded" in conditions:
        batch_pass("loaded", loaded=True)

    print()
    print("| test | condition | runs | passes | failures | verdict |")
    print("|---|---|---:|---:|---:|---|")
    payload = []
    for row in rows:
        entry = {
            "package": row.package,
            "binary": row.binary,
            "test": row.test,
            "conditions": {},
        }
        for cond, outs in row.results.items():
            n = len(outs)
            p = sum(1 for o in outs if o.verdict == "PASS")
            f = sum(1 for o in outs if o.verdict == "FAIL")
            s = sum(1 for o in outs if o.verdict == "SKIPPED")
            other = n - p - f - s
            if other == n and n:
                verdict = outs[0].verdict  # NOTRUN / ERROR throughout
            elif s and f == 0 and p == 0:
                verdict = "SKIPPED (no delegated backend)"
            elif s:
                # A skip is not a pass and it is not a failure either; it is a
                # measurement that did not happen, and it must never be summed
                # into either column silently.
                verdict = f"MIXED ({p} pass / {f} fail / {s} skipped)"
            elif f == 0:
                verdict = "GREEN"
            elif p == 0:
                verdict = "CONSISTENTLY FAILING"
            else:
                verdict = "FLAKY"
            entry["conditions"][cond] = {
                "runs": n,
                "passes": p,
                "failures": f,
                "skipped": s,
                "other": other,
                "verdict": verdict,
                "details": [o.detail for o in outs if o.detail][:4],
            }
            print(f"| {row.test} | {cond} | {n} | {p} | {f} | {verdict} |")
        payload.append(entry)

    with open(args.out, "w") as fh:
        json.dump(
            {
                "host": platform.node(),
                "os": platform.system(),
                "release": platform.release(),
                "cpus": os.cpu_count(),
                "profile": args.profile,
                "runs": args.runs,
                "retries": 0,
                "load_workers": load_workers,
                "rows": payload,
            },
            fh,
            indent=2,
        )
    print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
