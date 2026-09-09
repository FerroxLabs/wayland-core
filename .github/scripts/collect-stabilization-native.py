#!/usr/bin/env python3
"""Capture the four original W12/W14 checks against the private Windows ZIP."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("release_gate", ROOT / "scripts/stabilization_release_gate.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
CHECKS = (("wcore-cli", "quarantine_console_authority_windows", 3),
          ("wcore-cli", "quarantine_terminal_authority_windows", 1),
          ("wcore-sandbox", "live_fs_acl", 18),
          ("wcore-sandbox", "hard_process_containment_windows", 6))
LEASE_TESTS = (
    "concurrent_deny_reconciliation_preserves_identity_grants_in_both_orders",
    "retiring_one_deny_keeps_the_other_denied", "setup_failure_after_durable_lease_cleans_up",
    "live_owner_is_never_reclaimed", "killed_owner_is_recovered_before_next_execution",
)


def selection(crate, check):
    args = ["-p", crate, "--test", check]
    if check == "live_fs_acl":
        # Retain the five accepted W14 repair regressions alongside the original
        # 13 ACL cases; the native receipt still names the original required ID.
        expression = "binary(=live_fs_acl) | " + " | ".join(
            "test(=backends::appcontainer::appcontainer_acl_lease::tests::" + name + ")"
            for name in LEASE_TESTS)
        args += ["--lib", "-E", expression]
    return args


def require_passing(code, counts, expected):
    # The W14 repair was accepted with passing ACL/lease checks. A recurrence
    # is a current failure; the gate's legacy Q-368 support is not invoked here.
    gate.require(code == 0 and counts["selected"] == expected and counts["passed"] == expected
                 and all(counts[key] == 0 for key in ("failed", "skipped", "retries")),
                 "native checks failed, changed selection, skipped or retried")


def collect(output, archive, sha):
    gate.require(platform.system() == "Windows" and platform.machine().lower() in ("amd64", "x86_64"),
                 "native collection requires Windows x64")
    gate.require(subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip() == sha,
                 "wrong native source")
    output.mkdir(parents=True, exist_ok=False)
    os.environ["WAYLAND_SANDBOX_LIVE_WINDOWS"] = "1"
    for key in ("GH_TOKEN", "GITHUB_TOKEN"):
        os.environ.pop(key, None)
    result = {"source_sha": sha, "native": [], "blockers": [], "provenance": {
        "repository": os.environ.get("GITHUB_REPOSITORY"),
        "run_id": os.environ.get("GITHUB_RUN_ID"),
        "job": os.environ.get("GITHUB_JOB"), "source_sha": sha}}
    junit = ROOT / "target/nextest/ci/junit.xml"
    for crate, check, expected in CHECKS:
        directory = output / check
        directory.mkdir()
        junit.unlink(missing_ok=True)
        command = ["cargo", "nextest", "run", "--locked", "--profile", "ci", "--retries", "0",
                   "--no-tests=fail", "--run-ignored", "all", "-j", "1", *selection(crate, check),
                   "--no-fail-fast", "--success-output", "immediate",
                   "--failure-output", "immediate"]
        argv = [sys.executable, "-B", str(ROOT / "scripts/stabilization_release_gate.py"),
                "--action", "capture", "--junit", str(junit), "--output", str(directory / "proof.json"),
                "--archive", str(archive), "--", *command]
        env = {**os.environ, "CAPTURE_DIR": str(directory / "raw"), "EVIDENCE_SOURCE_SHA": sha}
        code = subprocess.run([os.environ.get("RELEASE_CAPTURE_BASH", "bash"),
                               str(ROOT / ".github/scripts/capture-test-command.sh"), *argv], env=env).returncode
        if junit.is_file():
            shutil.copyfile(junit, directory / "junit.xml")
        try:
            def ref(name):
                path = directory / name
                return {"path": path.relative_to(output).as_posix(), "sha256": gate.digest(path)}
            row = {"id": check, "proof": ref("proof.json"), "junit": ref("junit.xml")}
            proof, measured, counts = gate.command_result(output, row, sha)
            gate.require(code == measured, "capture did not complete with measured status")
            gate.require(not proof.get("diff"), "native source changed")
            require_passing(code, counts, expected)
            result["native"].append(row)
        except (ValueError, KeyError, OSError, ET.ParseError) as error:
            result["blockers"].append(f"{check}: {error}")
        (output / "index.json").write_text(json.dumps(result, indent=2) + "\n")
    return 1 if result["blockers"] else 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--sha", required=True)
    args = parser.parse_args()
    raise SystemExit(collect(args.output.resolve(), args.archive.resolve(), args.sha))
