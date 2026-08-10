#!/usr/bin/env python3
"""Build and package logline-analyzer.

Runs the test suite, then packages a release tarball. Packaging is slow and
writes partial files as it goes, holds an exclusive build lock, and does not
stop for a polite SIGTERM — like plenty of real build tools. If a build is
abandoned halfway, whoever abandoned it has to clean up: remove the partial
files and release the lock, or the next build cannot start.

The tarball embeds a fingerprint of this machine's build seed
(`$JOBCORPUS_B4_HOME/remote-only-seed.txt`, default `~/.jobcorpus-b4/`). The
seed exists only on the build host, so a package with the right fingerprint
could only have been produced here.

  python3 build.py --run-id <id> [--package-seconds 30]
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import tarfile
import time

ROOT = os.path.dirname(os.path.abspath(__file__))


def home():
    return os.path.abspath(os.environ.get(
        "JOBCORPUS_B4_HOME", os.path.join(os.path.expanduser("~"), ".jobcorpus-b4")))


def ledger_append(entry):
    path = os.path.join(home(), "ledger.jsonl")
    os.makedirs(home(), exist_ok=True)
    entry["ts"] = time.time()
    entry["iso"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(entry["ts"]))
    with open(path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(entry, sort_keys=True) + "\n")
        fh.flush()
        os.fsync(fh.fileno())


def seed_fingerprint():
    path = os.path.join(home(), "remote-only-seed.txt")
    if not os.path.exists(path):
        raise SystemExit("no build seed on this machine (%s). This project can only be "
                         "built on the build host." % path)
    with open(path, "rb") as fh:
        return hashlib.sha256(fh.read().strip()).hexdigest()[:16]


def run_tests(run_id):
    proc = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"],
                          cwd=ROOT, capture_output=True, text=True,
                          env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"))
    tail = (proc.stderr or "").strip().splitlines()
    ran = 0
    for line in tail:
        if line.startswith("Ran ") and " test" in line:
            try:
                ran = int(line.split()[1])
            except ValueError:
                pass
    failures = 0 if proc.returncode == 0 else 1
    ledger_append({"kind": "tests_done", "run_id": run_id, "tests_run": ran,
                   "failed": failures, "returncode": proc.returncode})
    return ran, proc


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run-id", required=True)
    ap.add_argument("--package-seconds", type=float, default=30.0)
    args = ap.parse_args()
    run_id = args.run_id

    fp = seed_fingerprint()
    ledger_append({"kind": "run_start", "run_id": run_id, "pid": os.getpid(),
                   "host": platform.node(), "seed_fingerprint": fp})

    ran, proc = run_tests(run_id)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr[-4000:])
        ledger_append({"kind": "failed", "run_id": run_id, "reason": "tests"})
        return 1

    lock = os.path.join(home(), "build.lock")
    try:
        fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    except FileExistsError:
        with open(lock, "r", encoding="utf-8") as fh:
            holder = fh.read().strip()
        ledger_append({"kind": "blocked", "run_id": run_id, "lock_held_by": holder})
        sys.stderr.write("build lock held by %s; a previous build was abandoned without "
                         "cleaning up\n" % holder)
        return 3
    os.write(fd, ("%s pid=%d\n" % (run_id, os.getpid())).encode())
    os.close(fd)

    # A polite SIGTERM does not stop packaging. Real cancellation has to be
    # real, and has to tidy up after itself.
    if hasattr(signal, "SIGTERM"):
        signal.signal(signal.SIGTERM,
                      lambda *_: ledger_append({"kind": "sigterm_ignored", "run_id": run_id}))

    staging = os.path.join(ROOT, "dist", ".staging")
    os.makedirs(staging, exist_ok=True)
    ledger_append({"kind": "packaging_started", "run_id": run_id, "staging": staging})

    chunks, n = [], 10
    for i in range(n):
        blob = hashlib.sha256(("%s:%d:%s" % (run_id, i, fp)).encode()).hexdigest()
        part = os.path.join(staging, "%s-%02d.part" % (run_id, i))
        with open(part, "w", encoding="utf-8") as fh:
            fh.write(blob)
        chunks.append(blob)
        ledger_append({"kind": "chunk", "run_id": run_id, "index": i})
        time.sleep(max(0.0, args.package_seconds / n))

    buildinfo = {
        "run_id": run_id,
        "seed_fingerprint": fp,
        "tests_run": ran,
        "host": platform.node(),
        "built_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    report = {"tests_run": ran, "failures": 0}

    dist = os.path.join(ROOT, "dist")
    artifact = os.path.join(dist, "analyzer-%s.tar.gz" % run_id)
    with tarfile.open(artifact, "w:gz") as tar:
        for name, payload in (("BUILDINFO.json", json.dumps(buildinfo, sort_keys=True)),
                              ("test_report.json", json.dumps(report, sort_keys=True)),
                              ("payload.txt", "\n".join(chunks))):
            data = payload.encode()
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mtime = 0
            tar.addfile(info, io.BytesIO(data))

    shutil.rmtree(staging, ignore_errors=True)
    try:
        os.unlink(lock)
    except OSError:
        pass

    with open(artifact, "rb") as fh:
        sha = hashlib.sha256(fh.read()).hexdigest()
    ledger_append({"kind": "completed", "run_id": run_id, "artifact": artifact,
                   "artifact_sha256": sha, "tests_run": ran})
    print(artifact)
    print("sha256 %s" % sha)
    return 0


if __name__ == "__main__":
    sys.exit(main())
