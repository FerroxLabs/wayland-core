#!/usr/bin/env bash
# ADMISSION: caller-decides -- executes the caller-selected test/build command;
# aggregate evidence gates independently require its result.
# Capture one command, without retries or output filtering. CAPTURE_DIR must be
# a fresh directory; EVIDENCE_SOURCE_SHA defaults to git HEAD. Arguments are
# recorded verbatim: pass only CI commands, never credentials in argv. Environment
# variables are not dumped. stdin is closed (these are noninteractive tests).
set -euo pipefail
exec python3 - "$@" <<'PYTHON'
import datetime
import json
import os
import signal
import subprocess
import sys
import threading
from pathlib import Path

if len(sys.argv) < 2 or not os.environ.get("CAPTURE_DIR"):
    sys.exit("usage: CAPTURE_DIR=<new directory> capture-test-command.sh <command> [args...]")
directory = Path(os.environ["CAPTURE_DIR"])
try:
    directory.mkdir(parents=True, exist_ok=False)
except OSError as error:
    print(f"SETUP FAILURE, no test ran: {error}", file=sys.stderr)
    sys.exit(2)
source = os.environ.get("EVIDENCE_SOURCE_SHA") or subprocess.run(
    ["git", "rev-parse", "HEAD"], capture_output=True, text=True
).stdout.strip()
metadata = {
    "schema_version": 1, "source_sha": source, "command": sys.argv[1:],
    "cwd": os.getcwd(), "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "state": "starting", "exit_code": None, "signal": None,
}
metadata_path = directory / "metadata.json"
def save():
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
save()
errors = []
child = None
received_signal = None

def forward(signum, _frame):
    global received_signal
    received_signal = signum
    if child is not None and child.poll() is None:
        try:
            if os.name == "posix":
                os.killpg(child.pid, signum)
            else:
                child.send_signal(signum)
        except ProcessLookupError:
            pass

for signum in (signal.SIGTERM, signal.SIGINT):
    signal.signal(signum, forward)

def copy(stream, path, console):
    try:
        with path.open("wb") as raw:
            while True:
                data = stream.read1(65536)
                if not data:
                    break
                raw.write(data)
                raw.flush()
                try:
                    console.buffer.write(data)
                    console.buffer.flush()
                except BrokenPipeError:
                    pass  # A closed log viewer must not discard the raw artifact.
    except Exception as error:
        errors.append(str(error))

try:
    child = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             start_new_session=(os.name == "posix"))
    metadata["state"] = "running"
    metadata["pid"] = child.pid
    save()
    threads = [threading.Thread(target=copy, args=(stream, directory / name, console))
               for stream, name, console in [(child.stdout, "stdout.log", sys.stdout),
                                              (child.stderr, "stderr.log", sys.stderr)]]
    for thread in threads:
        thread.start()
    if received_signal is not None:
        forward(received_signal, None)
    result = child.wait()
    for thread in threads:
        thread.join()
    status = 128 - result if result < 0 else result
    metadata["signal"] = -result if result < 0 else received_signal
    if received_signal is not None:
        status = 128 + received_signal
    metadata["state"] = "finished"
except OSError as error:
    status = 127 if isinstance(error, FileNotFoundError) else 126
    metadata["state"] = "launch_failed"
    metadata["error"] = str(error)
    (directory / "stdout.log").touch()
    (directory / "stderr.log").write_text(str(error) + "\n")
    print(error, file=sys.stderr)
if errors:
    metadata["capture_errors"] = errors
    if status == 0:
        status = 2
metadata["exit_code"] = status
metadata["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
save()
sys.exit(status)
PYTHON
