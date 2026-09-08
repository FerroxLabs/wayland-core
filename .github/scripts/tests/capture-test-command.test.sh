#!/usr/bin/env bash
# Focused runner controls; no Rust build or provider access.
set -euo pipefail
cd "$(dirname "$0")/../../.."
python3 - <<'PYTHON'
import json
import os
import signal
import subprocess
import tempfile
import time
from pathlib import Path

root = Path.cwd()
wrapper = root / ".github/scripts/run-tests-with-attempt-evidence.sh"
capture = root / ".github/scripts/capture-test-command.sh"
with tempfile.TemporaryDirectory() as temp:
    home = Path(temp)
    def run(name, code, expected, stale=False):
        directory = home / name
        directory.mkdir()
        junit = directory / "junit.xml"
        if stale:
            junit.write_text("STALE REPORT")
        env = {**os.environ, "ATTEMPT_DIR": str(directory / "attempts"),
               "JUNIT_PATH": str(junit), "EVIDENCE_SOURCE_SHA": "fixture-source"}
        result = subprocess.run(["bash", str(wrapper), "bash", "-c", code],
                                env=env, capture_output=True)
        assert result.returncode == expected, (name, result.returncode, result.stderr)
        attempt = directory / "attempts/attempt-1"
        metadata = json.loads((attempt / "metadata.json").read_text())
        assert metadata["source_sha"] == "fixture-source"
        assert metadata["command"] == ["bash", "-c", code]
        assert metadata["cwd"] == str(root)
        assert metadata["finished_at"] >= metadata["started_at"]
        assert (directory / "attempts/.attempt").read_text().strip() == "1"
        return directory, attempt, metadata

    _, attempt, metadata = run("failure", 'echo running 1 test; echo full-panic-body >&2; echo "<testsuites/>" > "$JUNIT_PATH"; exit 100', 100)
    assert metadata["exit_code"] == 100
    assert (attempt / "stderr.log").read_text() == "full-panic-body\n"
    assert (attempt / "stdout.log").read_text() == "running 1 test\n"
    assert (attempt.parent / "outer-attempt-1.xml").exists()
    assert (attempt / "junit.snapshot").read_bytes() == (attempt.parent / "outer-attempt-1.xml").read_bytes()
    print("PASS actual test failure runs once and retains full streams and receipt")

    for code in (75, 42, 143):
        directory, attempt, metadata = run(f"no-junit-{code}", f"exit {code}", code, stale=True)
        assert not (directory / "junit.xml").exists()
        assert metadata["exit_code"] == code
        assert (attempt.parent / "final-status.txt").read_text().strip() == "failure"
        assert not list(attempt.parent.glob("outer-attempt-*.xml"))
    print("PASS infrastructure, unknown, and signal-style failures never retry or reuse stale JUnit")

    _, attempt, metadata = run("no-report-success", "true", 2, stale=True)
    assert metadata["exit_code"] == 0
    assert (attempt / "runner-exit-code.txt").read_text().strip() == "2"
    print("PASS zero command exit without fresh JUnit is an evidence failure")

    directory, attempt, metadata = run("success", 'echo "<testsuites/>" > "$JUNIT_PATH"; echo success', 0)
    assert (attempt.parent / "final-status.txt").read_text().strip() == "success"
    snapshot = (attempt / "junit.snapshot").read_bytes()
    (directory / "junit.xml").write_text("REPLACED BY LATER SUITE")
    assert snapshot == b"<testsuites/>\n"
    assert (attempt / "junit.snapshot").read_bytes() == snapshot
    print("PASS successful nextest command remains successful and keeps an immutable JUnit snapshot")

    directory = home / "shared"
    env = {**os.environ, "CAPTURE_DIR": str(directory)}
    result = subprocess.run(["bash", str(capture), "bash", "-c", "echo shared; echo panic >&2; exit 101"], env=env, capture_output=True)
    assert result.returncode == 101
    assert (directory / "stderr.log").read_text() == "panic\n"
    result = subprocess.run(["bash", str(capture), "true"], env=env, capture_output=True)
    assert result.returncode == 2
    assert (directory / "stderr.log").read_text() == "panic\n"
    print("PASS shared-process capture preserves failure and refuses stale destination reuse")

    directory = home / "signal"
    env["CAPTURE_DIR"] = str(directory)
    process = subprocess.Popen(["bash", str(capture), "bash", "-c", "echo ready; exec sleep 30"], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert process.stdout.readline() == b"ready\n"
    process.send_signal(signal.SIGTERM)
    process.communicate(timeout=5)
    assert process.returncode == 143
    metadata = json.loads((directory / "metadata.json").read_text())
    assert metadata["signal"] == signal.SIGTERM and metadata["exit_code"] == 143
    print("PASS received signal terminates the command and remains in metadata")

    directory = home / "missing-command"
    env["CAPTURE_DIR"] = str(directory)
    result = subprocess.run(["bash", str(capture), str(home / "does-not-exist")], env=env, capture_output=True)
    assert result.returncode == 127
    assert json.loads((directory / "metadata.json").read_text())["state"] == "launch_failed"
    print("PASS launch failure preserves status and raw diagnostic without fake pass")
PYTHON
