#!/usr/bin/env python3
"""Collect the ORIGINAL eight W12 mutations on one clean Linux source checkout.

Usage: --output FRESH_DIRECTORY_OUTSIDE_CHECKOUT [--source EXPECTED_SHA]
       [--target-dir SHARED_CARGO_TARGET]
Needs initialized vx Rust, cargo-nextest, and ordinary Linux build dependencies.
Each exact selected workload runs once as baseline and once with its original
patch. Reuses one warm target; never runs a whole suite per mutant or retries.
Upload the entire output even on failure. index.json uses the existing strict
stabilization_release_gate.py raw schema. Offline tests are bookkeeping controls,
not mutation proof. Only execution on the final release SHA supplies that proof.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys
import time
import tomllib
import xml.etree.ElementTree as ET


def require(ok, reason):
    if not ok:
        raise RuntimeError(reason)


def git(repo, *args):
    env = os.environ.copy()
    env.pop("GH_TOKEN", None)
    env.pop("GITHUB_TOKEN", None)
    result = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, env=env)
    require(result.returncode == 0, "Git operation failed: " + " ".join(args[:2])
            + "\n" + result.stdout.decode(errors="replace") + result.stderr.decode(errors="replace"))
    return result.stdout.decode()


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def ref(root, path):
    require(path.is_file() and not path.is_symlink() and path.resolve().is_relative_to(root),
            "evidence file missing or outside output")
    return {"path": path.relative_to(root).as_posix(), "sha256": digest(path)}


def clean(repo, source):
    require(git(repo, "rev-parse", "HEAD").strip() == source, "source HEAD changed")
    require(not git(repo, "status", "--porcelain"), "checkout is dirty or contains unowned files")


def command(argv, cwd, env, stdout, stderr):
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=stdout, stderr=stderr,
                               start_new_session=True)
    try:
        return process.wait()
    except BaseException:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        raise


def fixture_environment(output, source, target):
    env = os.environ.copy()
    for key in ("GH_TOKEN", "GITHUB_TOKEN", "OPENAI_API_KEY", "ANTHROPIC_API_KEY",
                "DEEPSEEK_API_KEY", "WAYLAND_VAULT_PASSPHRASE_FD"):
        env.pop(key, None)
    env.update(CARGO_TARGET_DIR=str(target), CARGO_INCREMENTAL="0", NEXTEST_PROFILE="default",
               WAYLAND_BUILD_SOURCE_SHA=source,
               WAYLAND_VAULT_PASSPHRASE="stabilization-fixture-no-real-credentials")
    return env


def validate_mapping(mapping, directory, gate):
    require(mapping.get("schema") == 1, "unsupported original mutation mapping")
    rows = mapping.get("mutants", [])
    require(len(rows) == len(gate.MUTANTS) and {row["id"] for row in rows} == set(gate.MUTANTS),
            "mapping must contain exactly the original eight required IDs")
    for row in rows:
        patch = directory / row["patch"]
        require(patch.resolve().parent == directory.resolve() and digest(patch) == row["patch_sha256"],
                "original mutation patch digest mismatch")
        argv = row["argv"]
        require(argv[:3] == ["cargo", "nextest", "run"] and "--locked" in argv
                and "--no-tests=fail" in argv and "--no-fail-fast" in argv and "-p" in argv
                and ("--test" in argv or "--lib" in argv) and "--workspace" not in argv,
                "mutation must retain scoped original nextest command")
        require(any(argv[i:i + 2] == ["--retries", "0"] for i in range(len(argv)))
                and any(argv[i:i + 2] == ["--test-threads", "1"] for i in range(len(argv))),
                "original mutation execution policy changed")
        require(row.get("expected_cases") and row.get("expected_failures"), "original case/witness mapping absent")
    return rows


def cases(path):
    result = {}
    for case in ET.parse(path).iter("testcase"):
        key = (case.get("classname"), case.get("name"))
        require(all(key) and key not in result, "JUnit case identity missing or duplicated")
        result[key] = case
    require(result, "JUnit contains zero selected tests")
    return result


def qualify(root, row, baseline, mutant, source, gate):
    bp, bc, bn = gate.command_result(root, baseline, source)
    mp, mc, mn = gate.command_result(root, mutant, source)
    require(not bp.get("diff") and bc == 0 and bn["selected"] > 0
            and bn["passed"] == bn["selected"] and bn["retries"] == 0, "fresh baseline did not pass")
    require(bp["argv"] == mp["argv"] == ["vx", "--no-auto-install", *row["argv"]],
            "baseline/mutant command selection changed")
    expected = {(case["classname"], case["name"]) for case in row["expected_cases"]}
    before = cases(root / baseline["junit"]["path"])
    after = cases(root / mutant["junit"]["path"])
    require(set(before) == set(after) == expected, "original selected testcase set changed")
    require(mc == 100 and mn["selected"] == bn["selected"] and mn["skipped"] == 0
            and mn["retries"] == 0, "mutant did not complete a viable failing nextest run")
    failed = {key for key, case in after.items() if case.find("failure") is not None or case.find("error") is not None}
    witnesses = {(case["classname"], case["name"]): case for case in row["expected_failures"]}
    require(failed == set(witnesses), "failure occurred outside the original mutation witness")
    for key, witness in witnesses.items():
        failure = after[key].find("failure")
        require(failure is not None and failure.get("type") == witness["type"],
                "build, timeout, or execution errors cannot count as a caught mutation")
        text = " ".join((failure.text or "").split())
        require(all(" ".join(marker.split()) in text for marker in witness["contains"]),
                "test failed for a different reason than the original mutation assertion")
    require(mp.get("diff") == (root / mutant["patch"]["path"]).read_text(), "tested patch differs from capture")
    return {"baseline": bn, "mutant": mn, "caught": True}


def run_capture(repo, root, row, phase, source, env, junit, execute):
    output = root / row["id"] / phase
    output.mkdir(parents=True, exist_ok=False)
    home = output / "fixture-home"
    home.mkdir(mode=0o700)
    child_env = {**env, "WAYLAND_HOME": str(home)}
    old_digest = digest(junit) if junit.is_file() else None
    if old_digest:
        (output / "prior-junit.xml").write_bytes(junit.read_bytes())
    argv = ["vx", "--no-auto-install", *row["argv"]]
    capture = [sys.executable, str(repo / "scripts/stabilization_release_gate.py"),
               "--action", "capture", "--junit", str(junit), "--output", str(output / "proof.json"), "--", *argv]
    print(f"MUTANT_PHASE {row['id']} {phase}", flush=True)
    started = time.time_ns()
    with (output / "stdout.log").open("wb") as stdout, (output / "stderr.log").open("wb") as stderr:
        code = execute(capture, repo, child_env, stdout, stderr)
    write_json(output / "capture-status.json", {"exit_code": code, "started_ns": started,
                                                "finished_ns": time.time_ns(), "source_sha": source})
    require((output / "proof.json").is_file(), "capture did not produce a proof")
    proof = json.loads((output / "proof.json").read_text())
    if junit.is_file():
        (output / "observed-junit.xml").write_bytes(junit.read_bytes())
    require(proof.get("complete") is True and proof.get("source") == source,
            "capture incomplete: no fresh JUnit or source changed")
    require(junit.is_file() and junit.stat().st_mtime_ns >= started and digest(junit) != old_digest,
            "JUnit is missing or retained from a prior command")
    require(code == proof.get("exit_code"), "capture process and measured test exit disagree")
    (output / "junit.xml").write_bytes(junit.read_bytes())
    return {"id": row["id"], "proof": ref(root, output / "proof.json"),
            "junit": ref(root, output / "junit.xml")}


def collect(repo, output, mapping_dir, expected_source, target, gate, execute=command):
    source = git(repo, "rev-parse", "HEAD").strip()
    require(expected_source is None or expected_source == source, "expected release source differs from HEAD")
    clean(repo, source)
    require(not output.exists() and not output.is_relative_to(repo), "output must be fresh and outside checkout")
    mapping = json.loads((mapping_dir / "mapping.json").read_text())
    rows = validate_mapping(mapping, mapping_dir, gate)
    config = tomllib.loads((repo / ".config/nextest.toml").read_text())
    require(config.get("profile", {}).get("default", {}).get("junit", {}).get("path") == "junit.xml"
            and config.get("store", {}).get("dir", "target/nextest") == "target/nextest",
            "original default-profile JUnit layout is not configured")
    junit = repo / "target/nextest/default/junit.xml"
    output.mkdir(parents=True, mode=0o700)
    env = fixture_environment(output, source, target)
    index = {"source_sha": source, "tests": [], "mutants": [], "native": [], "deferred_risks": [],
             "provenance": {"repository": os.environ.get("GITHUB_REPOSITORY", ""),
                            "run_id": os.environ.get("GITHUB_RUN_ID", ""),
                            "job": os.environ.get("GITHUB_JOB", ""), "source_sha": source},
             "definition_source": mapping.get("definition_source"), "attempts": [], "blockers": []}
    write_json(output / "index.json", index)
    try:
        print("MUTANT_PHASE toolchain-initialization", flush=True)
        with (output / "toolchain.stdout.log").open("wb") as stdout, (output / "toolchain.stderr.log").open("wb") as stderr:
            require(execute(["vx", "cargo", "--version"], repo, env, stdout, stderr) == 0,
                    "vx Cargo initialization failed")
        for row in rows:
            clean(repo, source)
            definition = output / row["id"]
            definition.mkdir()
            (definition / "definition.patch").write_bytes((mapping_dir / row["patch"]).read_bytes())
            write_json(definition / "definition.json", row)
            attempt = {"id": row["id"], "phase": "baseline", "status": "running"}
            index["attempts"].append(attempt)
            write_json(output / "index.json", index)
            baseline = run_capture(repo, output, row, "baseline", source, env, junit, execute)
            bp, bc, bn = gate.command_result(output, baseline, source)
            require(bc == 0 and not bp.get("diff") and bn["selected"] > 0
                    and bn["passed"] == bn["selected"] and bn["retries"] == 0, "baseline failed; no mutation attempted")
            require(set(cases(output / baseline["junit"]["path"])) ==
                    {(case["classname"], case["name"]) for case in row["expected_cases"]},
                    "baseline did not select the original testcase set")
            clean(repo, source)
            patch = mapping_dir / row["patch"]
            paths = [line.split("\t", 2)[2] for line in git(repo, "apply", "--numstat", str(patch)).splitlines()]
            require(paths and all(path.startswith("crates/") and path.endswith(".rs") for path in paths),
                    "mutation includes unexpected file ownership")
            git(repo, "apply", "--check", str(patch))
            git(repo, "apply", str(patch))
            applied = git(repo, "diff", "HEAD", "--")
            exact = output / row["id"] / "tested.patch"
            exact.write_text(applied)
            try:
                require(applied and set(git(repo, "diff", "--name-only", "HEAD", "--").splitlines()) == set(paths),
                        "mutation modified unowned files")
                for path in paths:
                    os.utime(repo / path, None)
                attempt.update(phase="mutant")
                write_json(output / "index.json", index)
                mutant = run_capture(repo, output, row, "mutant", source, env, junit, execute)
                mutant.update(baseline=baseline, patch=ref(output, exact))
                outcome = qualify(output, row, baseline, mutant, source, gate)
            finally:
                require(git(repo, "rev-parse", "HEAD").strip() == source
                        and not git(repo, "diff", "--cached") and git(repo, "diff", "HEAD", "--") == applied,
                        "ownership conflict: leave changed files untouched; do not reset another actor's edits")
                git(repo, "apply", "--reverse", str(patch))
                for path in paths:
                    os.utime(repo / path, None)
            clean(repo, source)
            index["mutants"].append(mutant)
            attempt.update(status="caught", **outcome)
            write_json(output / "index.json", index)
        clean(repo, source)
    except (RuntimeError, OSError, ValueError, KeyError, ET.ParseError, KeyboardInterrupt) as error:
        index["blockers"].append(str(error) or "collector interrupted")
        if index["attempts"] and index["attempts"][-1]["status"] == "running":
            index["attempts"][-1]["status"] = "blocked"
    index["source_clean_after"] = (git(repo, "rev-parse", "HEAD").strip() == source
                                   and not git(repo, "status", "--porcelain"))
    index["complete"] = len(index["mutants"]) == len(gate.MUTANTS) and not index["blockers"] and index["source_clean_after"]
    write_json(output / "index.json", index)
    return index


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source")
    parser.add_argument("--target-dir", type=Path)
    args = parser.parse_args()
    try:
        require(platform.system() == "Linux", "original mutation execution requires the Linux executor")
        import fcntl
        repo = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip()).resolve()
        target = (args.target_dir or Path(os.environ.get("CARGO_TARGET_DIR", repo / "target"))).resolve()
        target.mkdir(parents=True, exist_ok=True)
        with (target / ".stabilization-mutants.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            spec = importlib.util.spec_from_file_location("release_gate", repo / "scripts/stabilization_release_gate.py")
            gate = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(gate)
            previous = signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
            try:
                result = collect(repo, args.output.resolve(), repo / ".github/scripts/stabilization-mutants",
                                 args.source, target, gate)
            finally:
                signal.signal(signal.SIGTERM, previous)
        print(json.dumps({"source_sha": result["source_sha"], "complete": result["complete"],
                          "caught": len(result["mutants"]), "blockers": result["blockers"]}), flush=True)
        return 0 if result["complete"] else 1
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        print(f"mutant collection refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
