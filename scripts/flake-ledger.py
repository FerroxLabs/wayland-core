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
"""

from __future__ import annotations

import argparse
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


@dataclass
class Outcome:
    verdict: str  # PASS | FAIL | NOTRUN | ERROR
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
    return sum(
        1
        for line in proc.stdout.splitlines()
        if line.startswith("    ") and line.strip() and not line.strip().endswith(":")
    )


def run_nextest(
    root: str, profile: str, expr: str, expected: int, timeout: int
) -> tuple[Outcome, str]:
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
    blob = proc.stdout + proc.stderr
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
    if failed == 0 and passed == expected:
        return Outcome("PASS", elapsed, proc.returncode), blob
    fails = [
        line.strip()
        for line in blob.splitlines()
        if line.strip().startswith(("FAIL [", "TRY", "SIGSEGV", "TIMEOUT ["))
    ]
    return Outcome("FAIL", elapsed, proc.returncode, "; ".join(fails[:6])), blob


class Load:
    """N busy-spin child processes, to reproduce suite-level CPU contention."""

    def __init__(self, workers: int) -> None:
        self.workers = workers
        self.procs: list[subprocess.Popen] = []

    def __enter__(self) -> "Load":
        for _ in range(self.workers):
            self.procs.append(
                subprocess.Popen(
                    [sys.executable, "-c", "while True: pass"],
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
    args = ap.parse_args()

    conditions = [c.strip() for c in args.conditions.split(",") if c.strip()]
    load_workers = args.load or (os.cpu_count() or 4)
    rows = [Row(f"{p}::{b} {t}", p, b, t) for (p, b, t) in DEFAULT_TARGETS]
    if args.logdir:
        os.makedirs(args.logdir, exist_ok=True)

    print(f"host={platform.node()} os={platform.system()} {platform.release()}")
    print(f"cpus={os.cpu_count()} profile={args.profile} runs={args.runs} retries=0")
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

    def batch_pass(cond: str) -> None:
        print(f"== condition: {cond} ==")
        if not present:
            return
        for i in range(args.runs):
            out, blob = run_nextest(
                args.root, args.profile, batch_expr, len(present), args.timeout
            )
            # A batch invocation grades the SET. Attribute per test from the log.
            for row in present:
                if out.verdict in ("ERROR", "NOTRUN"):
                    per = Outcome(out.verdict, out.seconds, out.rc, out.detail)
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
        with Load(load_workers):
            batch_pass("loaded")

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
            other = n - p - f
            if other == n and n:
                verdict = outs[0].verdict  # NOTRUN / ERROR throughout
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
