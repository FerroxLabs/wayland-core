#!/usr/bin/env python3
"""Reproducible F11 benchmark and adversarial proof receipts.

The benchmark creates a detached worktree at the frozen pre-F11 base, builds
the same packaged deterministic test in both trees, and alternates paired
executions to reduce ordering bias. The adversarial proof runs the focused F11
tests directly from prebuilt test executables so Cargo startup/build time is not
mistaken for scheduler deadline tolerance.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


BASE_SHA = "a51808821be49f2983529b343294b01d839fb004"
MIN_PAIRS = 20
MEDIAN_REGRESSION_MAX = 0.10
P95_REGRESSION_MAX = 0.15
RUNTIME_BUDGET_MS = 20.0
SCHEDULER_TOLERANCE_MS = 100.0
DEADLINE_SAMPLES = 5
CORPUS_TEST = "packaged_f04_run_is_repeatable_and_content_addressed"
VARIANT_STABILITY_KEYS = (
    "behavior_sha256",
    "fixture_sha256",
    "openai_behavior_sha256",
    "repository_sha256",
)
SEMANTIC_OUTPUT_KEYS = (
    "fixture_sha256",
    "openai_behavior_sha256",
    "repository_sha256",
)


class ProofError(RuntimeError):
    pass


def command(
    argv: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        argv,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if check and result.returncode != 0:
        tail = "\n".join((result.stdout + "\n" + result.stderr).splitlines()[-80:])
        raise ProofError(f"command failed ({result.returncode}): {' '.join(argv)}\n{tail}")
    return result


def git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return command(["git", *args], cwd=repo, check=check)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_tree_sha256(repo: Path) -> str:
    listed = command(
        ["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=repo
    ).stdout
    digest = hashlib.sha256()
    for relative in sorted(path for path in listed.split("\0") if path):
        path = repo / relative
        digest.update(relative.encode("utf-8", "surrogateescape"))
        digest.update(b"\0")
        if path.is_symlink():
            digest.update(b"symlink\0")
            digest.update(os.readlink(path).encode("utf-8", "surrogateescape"))
        elif path.is_file():
            digest.update(b"file\0")
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
        elif not path.exists():
            # A tracked deletion is part of an exact candidate snapshot, not
            # a hashing failure. `git status` in the receipt records the D.
            digest.update(b"missing\0")
        else:
            raise ProofError(f"source entry changed while hashing: {relative}")
        digest.update(b"\0")
    return digest.hexdigest()


def source_identity(repo: Path) -> dict[str, Any]:
    return {
        "head": git(repo, "rev-parse", "HEAD").stdout.strip(),
        "tree_sha256": source_tree_sha256(repo),
        "status": git(repo, "status", "--short", "--untracked-files=all").stdout.splitlines(),
    }


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def require_outside_source(repo: Path, *paths: Path) -> None:
    for path in paths:
        resolved = path.resolve()
        if resolved == repo or repo in resolved.parents:
            raise ProofError(
                f"proof outputs must be outside the measured source tree: {resolved}"
            )


def cargo_build_tests(
    repo: Path,
    target_dir: Path,
    cargo_args: list[str],
) -> list[dict[str, Any]]:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["WAYLAND_BUILD_SOURCE_SHA"] = git(repo, "rev-parse", "HEAD").stdout.strip()
    result = command(
        [
            os.environ.get("WCORE_F11_CARGO", "cargo"),
            "test",
            "--locked",
            *cargo_args,
            "--no-run",
            "--message-format=json-render-diagnostics",
        ],
        cwd=repo,
        env=env,
    )
    artifacts: list[dict[str, Any]] = []
    for line in result.stdout.splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if item.get("reason") == "compiler-artifact" and item.get("executable"):
            artifacts.append(item)
    if not artifacts:
        raise ProofError("Cargo emitted no test executable artifacts")
    return artifacts


def artifact_executable(artifacts: list[dict[str, Any]], target_name: str) -> Path:
    matches = [
        Path(item["executable"])
        for item in artifacts
        if item.get("target", {}).get("name") == target_name
        and item.get("profile", {}).get("test")
    ]
    unique = list(dict.fromkeys(matches))
    if len(unique) != 1:
        raise ProofError(f"expected one test executable for {target_name}, found {unique}")
    return unique[0]


def run_exact_test(
    executable: Path,
    test_name: str,
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
) -> tuple[float, subprocess.CompletedProcess[str]]:
    started = time.perf_counter_ns()
    result = command(
        [str(executable), "--exact", test_name, "--nocapture", "--test-threads=1"],
        cwd=cwd,
        env=env,
        check=False,
    )
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    output = result.stdout + "\n" + result.stderr
    exact_pass = re.search(r"test result: ok\. 1 passed; 0 failed;", output)
    if result.returncode != 0 or exact_pass is None:
        tail = "\n".join(output.splitlines()[-80:])
        raise ProofError(f"focused test did not pass exactly once: {test_name}\n{tail}")
    return elapsed_ms, result


def nearest_rank_p95(samples: list[float]) -> float:
    return sorted(samples)[math.ceil(0.95 * len(samples)) - 1]


def semantic_identity(evidence_dir: Path) -> dict[str, str]:
    path = evidence_dir / "repeatability.json"
    if not path.is_file():
        raise ProofError(f"corpus did not emit {path}")
    value = json.loads(path.read_text())
    if value.get("runs") != 2:
        raise ProofError(f"corpus repeatability receipt did not prove two internal runs: {value}")
    identity = {key: value.get(key) for key in VARIANT_STABILITY_KEYS}
    if not all(isinstance(value, str) and len(value) == 64 for value in identity.values()):
        raise ProofError(f"invalid semantic identity: {identity}")
    return identity  # type: ignore[return-value]


def benchmark_once(
    executable: Path,
    source: Path,
    evidence_dir: Path,
    source_commit: str,
) -> dict[str, Any]:
    if evidence_dir.exists():
        shutil.rmtree(evidence_dir)
    evidence_dir.mkdir(parents=True)
    env = os.environ.copy()
    env["WCORE_F04_EVIDENCE_DIR"] = str(evidence_dir)
    env["WCORE_F04_SOURCE_COMMIT"] = source_commit
    elapsed_ms, _ = run_exact_test(executable, CORPUS_TEST, cwd=source, env=env)
    return {
        "duration_ms": round(elapsed_ms, 3),
        "semantic_identity": semantic_identity(evidence_dir),
        "repeatability_receipt_sha256": sha256_file(evidence_dir / "repeatability.json"),
        "scenario_runs": 2,
    }


def regression(candidate: float, baseline: float) -> float:
    if baseline <= 0:
        raise ProofError("baseline timing must be positive")
    return (candidate / baseline) - 1.0


def benchmark(args: argparse.Namespace) -> int:
    if args.pairs < MIN_PAIRS:
        raise ProofError(f"F11 requires at least {MIN_PAIRS} paired runs")
    repo = args.repo.resolve()
    output = args.output.resolve()
    work_root = args.work_root.resolve()
    require_outside_source(repo, output, work_root)
    work_root.mkdir(parents=True, exist_ok=True)
    baseline = work_root / "baseline-source"
    baseline_target = work_root / "target-baseline"
    candidate_target = work_root / "target-candidate"
    evidence_root = work_root / "evidence"
    receipt: dict[str, Any] = {
        "schema": "wayland.f11.paired-benchmark-receipt",
        "schema_version": 2,
        "frozen_base": BASE_SHA,
        "thresholds": {
            "paired_runs_min": MIN_PAIRS,
            "scenario_completion_rate_min": 1.0,
            "semantic_output_parity_required": True,
            "median_wall_time_regression_max": MEDIAN_REGRESSION_MAX,
            "p95_wall_time_regression_max": P95_REGRESSION_MAX,
        },
        "corpus": {
            "crate": "wcore-cli",
            "test_target": "deterministic_openai_loop",
            "test": CORPUS_TEST,
            "variant_stability_keys": list(VARIANT_STABILITY_KEYS),
            "cross_build_semantic_output_keys": list(SEMANTIC_OUTPUT_KEYS),
            "version_bound_digest": "behavior_sha256",
        },
        "requested_pairs": args.pairs,
        "verdict": "error",
    }
    baseline_created = False
    try:
        candidate_before = source_identity(repo)
        if candidate_before["head"] != BASE_SHA:
            receipt["candidate_note"] = (
                "candidate HEAD differs from the frozen base; exact content remains bound by tree_sha256"
            )
        if baseline.exists():
            raise ProofError(f"baseline worktree path already exists: {baseline}")
        git(repo, "worktree", "add", "--detach", str(baseline), BASE_SHA)
        baseline_created = True
        baseline_identity = source_identity(baseline)
        if baseline_identity["head"] != BASE_SHA or baseline_identity["status"]:
            raise ProofError(f"baseline is not the clean frozen base: {baseline_identity}")

        baseline_artifacts = cargo_build_tests(
            baseline,
            baseline_target,
            ["-p", "wcore-cli", "--test", "deterministic_openai_loop"],
        )
        candidate_artifacts = cargo_build_tests(
            repo,
            candidate_target,
            ["-p", "wcore-cli", "--test", "deterministic_openai_loop"],
        )
        baseline_exe = artifact_executable(baseline_artifacts, "deterministic_openai_loop")
        candidate_exe = artifact_executable(candidate_artifacts, "deterministic_openai_loop")
        receipt["source"] = {
            "baseline": baseline_identity,
            "candidate": candidate_before,
        }
        receipt["artifacts"] = {
            "baseline_test_sha256": sha256_file(baseline_exe),
            "candidate_test_sha256": sha256_file(candidate_exe),
        }

        benchmark_once(
            baseline_exe, baseline, evidence_root / "warmup-baseline", BASE_SHA
        )
        benchmark_once(
            candidate_exe,
            repo,
            evidence_root / "warmup-candidate",
            candidate_before["head"],
        )

        pairs: list[dict[str, Any]] = []
        for index in range(args.pairs):
            order = ("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")
            results: dict[str, Any] = {}
            for variant in order:
                if variant == "baseline":
                    results[variant] = benchmark_once(
                        baseline_exe,
                        baseline,
                        evidence_root / f"pair-{index:02d}-baseline",
                        BASE_SHA,
                    )
                else:
                    results[variant] = benchmark_once(
                        candidate_exe,
                        repo,
                        evidence_root / f"pair-{index:02d}-candidate",
                        candidate_before["head"],
                    )
            pairs.append({"pair": index + 1, "order": list(order), **results})

        candidate_after = source_identity(repo)
        source_stable = candidate_after["tree_sha256"] == candidate_before["tree_sha256"]
        baseline_times = [pair["baseline"]["duration_ms"] for pair in pairs]
        candidate_times = [pair["candidate"]["duration_ms"] for pair in pairs]
        baseline_median = statistics.median(baseline_times)
        candidate_median = statistics.median(candidate_times)
        baseline_p95 = nearest_rank_p95(baseline_times)
        candidate_p95 = nearest_rank_p95(candidate_times)
        median_regression = regression(candidate_median, baseline_median)
        p95_regression = regression(candidate_p95, baseline_p95)
        semantic_parity = all(
            {
                key: pair["baseline"]["semantic_identity"][key]
                for key in SEMANTIC_OUTPUT_KEYS
            }
            == {
                key: pair["candidate"]["semantic_identity"][key]
                for key in SEMANTIC_OUTPUT_KEYS
            }
            for pair in pairs
        )
        variant_digest_stability = all(
            all(
                pair[variant]["semantic_identity"]
                == pairs[0][variant]["semantic_identity"]
                for pair in pairs
            )
            for variant in ("baseline", "candidate")
        )
        completion_rate = len(pairs) / args.pairs
        checks = {
            "source_stable": source_stable,
            "paired_runs_min": len(pairs) >= MIN_PAIRS,
            "scenario_completion_100_percent": completion_rate == 1.0,
            "variant_digest_stability": variant_digest_stability,
            "semantic_output_parity": semantic_parity,
            "median_regression_within_limit": median_regression <= MEDIAN_REGRESSION_MAX,
            "p95_regression_within_limit": p95_regression <= P95_REGRESSION_MAX,
        }
        receipt.update(
            {
                "pairs": pairs,
                "source_after": {"candidate": candidate_after},
                "summary": {
                    "paired_runs": len(pairs),
                    "scenario_runs_per_variant": len(pairs) * 2,
                    "completion_rate": completion_rate,
                    "baseline_median_ms": round(baseline_median, 3),
                    "candidate_median_ms": round(candidate_median, 3),
                    "median_regression": round(median_regression, 6),
                    "baseline_p95_ms": round(baseline_p95, 3),
                    "candidate_p95_ms": round(candidate_p95, 3),
                    "p95_regression": round(p95_regression, 6),
                },
                "checks": checks,
                "verdict": "pass" if all(checks.values()) else "fail",
            }
        )
        atomic_json(output, receipt)
        return 0 if receipt["verdict"] == "pass" else 1
    except Exception as error:
        receipt["error"] = str(error)
        atomic_json(output, receipt)
        raise
    finally:
        if baseline_created and not args.keep_worktree:
            git(repo, "worktree", "remove", "--force", str(baseline), check=False)


ADVERSARIAL_TESTS = (
    (
        "wcore_agent",
        "engine::audit_2026_05_22_tests::budget_cap_terminates_the_run",
        "zero_provider_sends_after_admission_block",
    ),
    (
        "wcore_agent",
        "engine::audit_2026_05_22_tests::unpriced_provider_is_rejected_while_a_usd_cap_is_active",
        "zero_unpriceable_provider_sends_under_usd_cap",
    ),
    (
        "f11_concurrent_reservation_proof",
        "concurrent_provider_reservations_never_oversubscribe",
        "zero_provider_reservation_overshoot",
    ),
    (
        "budget_test",
        "concurrent_process_admission_never_oversubscribes",
        "zero_process_spawning_concurrency_overshoot",
    ),
    (
        "budget_test",
        "concurrent_tool_runtime_admission_never_multiplies_the_cap",
        "zero_runtime_reservation_overshoot",
    ),
    (
        "wcore_agent",
        "orchestration::tests::dispatcher_refuses_process_tool_before_execution_when_cap_is_zero",
        "zero_process_tool_starts_beyond_cap",
    ),
    (
        "wcore_agent",
        "orchestration::tests::concurrent_dispatch_cannot_multiply_remaining_tool_runtime",
        "zero_concurrent_dispatch_runtime_overshoot",
    ),
)
DEADLINE_TEST = "orchestration::tests::dispatcher_preempts_at_remaining_tool_runtime_budget"


def adversarial(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    output = args.output.resolve()
    target_dir = args.target_dir.resolve()
    require_outside_source(repo, output, target_dir)
    source_before = source_identity(repo)
    receipt: dict[str, Any] = {
        "schema": "wayland.f11.adversarial-proof-receipt",
        "schema_version": 1,
        "source": source_before,
        "thresholds": {
            "post_block_provider_sends_max": 0,
            "reservation_overshoot_max": 0,
            "process_spawning_starts_beyond_cap_max": 0,
            "charged_tool_runtime_ms": RUNTIME_BUDGET_MS,
            "scheduler_tolerance_ms": SCHEDULER_TOLERANCE_MS,
            "deadline_observed_process_ms_max": RUNTIME_BUDGET_MS
            + SCHEDULER_TOLERANCE_MS,
        },
        "verdict": "error",
    }
    try:
        agent_artifacts = cargo_build_tests(
            repo,
            target_dir,
            ["-p", "wcore-agent", "--lib", "--test", "budget_test"],
        )
        budget_artifacts = cargo_build_tests(
            repo,
            target_dir,
            ["-p", "wcore-budget", "--lib", "--test", "f11_concurrent_reservation_proof"],
        )
        artifacts = {
            "wcore_agent": artifact_executable(agent_artifacts, "wcore_agent"),
            "budget_test": artifact_executable(agent_artifacts, "budget_test"),
            "wcore_budget": artifact_executable(budget_artifacts, "wcore_budget"),
            "f11_concurrent_reservation_proof": artifact_executable(
                budget_artifacts, "f11_concurrent_reservation_proof"
            ),
        }
        receipt["artifacts"] = {
            name: {"path": str(path), "sha256": sha256_file(path)}
            for name, path in artifacts.items()
        }
        results = []
        for artifact, test_name, claim in ADVERSARIAL_TESTS:
            elapsed_ms, _ = run_exact_test(
                artifacts[artifact], test_name, cwd=repo, env=os.environ.copy()
            )
            results.append(
                {
                    "claim": claim,
                    "test": test_name,
                    "artifact": artifact,
                    "elapsed_ms": round(elapsed_ms, 3),
                    "passed": True,
                }
            )

        deadline_samples = []
        for _ in range(DEADLINE_SAMPLES):
            elapsed_ms, _ = run_exact_test(
                artifacts["wcore_agent"],
                DEADLINE_TEST,
                cwd=repo,
                env=os.environ.copy(),
            )
            deadline_samples.append(round(elapsed_ms, 3))
        deadline_max = max(deadline_samples)
        source_after = source_identity(repo)
        checks = {
            "all_focused_tests_passed": len(results) == len(ADVERSARIAL_TESTS),
            "deadline_samples_complete": len(deadline_samples) == DEADLINE_SAMPLES,
            "runtime_deadline_within_tolerance": deadline_max
            <= RUNTIME_BUDGET_MS + SCHEDULER_TOLERANCE_MS,
            "source_stable": source_after["tree_sha256"] == source_before["tree_sha256"],
        }
        receipt.update(
            {
                "focused_tests": results,
                "deadline_proof": {
                    "test": DEADLINE_TEST,
                    "samples_ms": deadline_samples,
                    "max_observed_process_ms": deadline_max,
                    "measurement_boundary": (
                        "direct prebuilt libtest process wall time; conservatively includes process startup"
                    ),
                },
                "source_after": source_after,
                "checks": checks,
                "verdict": "pass" if all(checks.values()) else "fail",
            }
        )
        atomic_json(output, receipt)
        return 0 if receipt["verdict"] == "pass" else 1
    except Exception as error:
        receipt["error"] = str(error)
        atomic_json(output, receipt)
        raise


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    subparsers = result.add_subparsers(dest="mode", required=True)
    benchmark_parser = subparsers.add_parser("benchmark")
    benchmark_parser.add_argument("--repo", type=Path, default=Path.cwd())
    benchmark_parser.add_argument("--output", type=Path, required=True)
    benchmark_parser.add_argument("--work-root", type=Path, required=True)
    benchmark_parser.add_argument("--pairs", type=int, default=MIN_PAIRS)
    benchmark_parser.add_argument("--keep-worktree", action="store_true")
    benchmark_parser.set_defaults(run=benchmark)
    adversarial_parser = subparsers.add_parser("adversarial")
    adversarial_parser.add_argument("--repo", type=Path, default=Path.cwd())
    adversarial_parser.add_argument("--output", type=Path, required=True)
    adversarial_parser.add_argument("--target-dir", type=Path, required=True)
    adversarial_parser.set_defaults(run=adversarial)
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        return args.run(args)
    except ProofError as error:
        print(f"F11 proof failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
