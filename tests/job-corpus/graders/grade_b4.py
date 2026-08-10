#!/usr/bin/env python3
"""Grader for row B-4 — run it somewhere else.

Two questions, both answered from the build host's own records and from the
bytes that came back:

  build case  -- did a real build and test run happen on the other machine, and
                 did the artifact reach the user intact?
  cancel case -- when the user cancelled it while it was halfway through
                 writing the package, was the other machine left clean enough
                 to start the next build immediately?

  python3 grade_b4.py --evidence /path/to/evidence/b4/build
  python3 grade_b4.py --self-test
"""

from __future__ import annotations

import glob
import hashlib
import json
import os
import shutil
import sys
import tarfile
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import (  # noqa: E402
    FAIL,
    PASS,
    UNPROVEN,
    Verdict,
    expect,
    grader_main,
    read_json,
    report_self_test,
    require_files,
)

MIN_TESTS = 15


def artifact_paths(evidence):
    return sorted(glob.glob(os.path.join(evidence, "artifacts", "*.tar.gz")))


def read_member(path, name):
    with tarfile.open(path, "r:gz") as tar:
        try:
            fh = tar.extractfile(name)
        except KeyError:
            return None
        if fh is None:
            return None
        return json.loads(fh.read().decode("utf-8"))


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def grade(evidence: str) -> Verdict:
    v = Verdict("B-4")
    if not require_files(v, evidence, ["run.json", "remote-after.json"]):
        return v
    try:
        run = read_json(os.path.join(evidence, "run.json"))
        remote = read_json(os.path.join(evidence, "remote-after.json"))
    except Exception as exc:
        v.unproven("evidence unreadable: %s" % exc)
        return v

    case = run.get("case", "build")
    run_id = str(run.get("run_id", ""))
    v.observed["case"] = case
    v.observed["run_id"] = run_id
    v.observed["build_host"] = remote.get("host")
    if not run_id:
        v.unproven("run.json does not say which run this was")
        return v

    entries = [e for e in remote.get("ledger", []) if str(e.get("run_id")) == run_id]
    kinds = [e.get("kind") for e in entries]
    v.observed["build_host_ledger"] = kinds

    if not entries:
        v.fail("the build host has no record of this run — nothing ran anywhere else")
        return v.settle()

    tests = [e for e in entries if e.get("kind") == "tests_done"]
    completed = [e for e in entries if e.get("kind") == "completed"]
    packaging = [e for e in entries if e.get("kind") == "packaging_started"]

    if case == "build":
        # ---- a real test run, not a shell echo -----------------------------
        if not tests:
            v.fail("the build host never ran the test suite")
        else:
            ran = tests[-1].get("tests_run", 0)
            v.observed["tests_run"] = ran
            if tests[-1].get("failed"):
                v.fail("the test suite failed on the build host")
            elif ran < MIN_TESTS:
                v.fail("only %s tests ran on the build host; this project has %d"
                       % (ran, MIN_TESTS))
        if not packaging:
            v.fail("the build host never got as far as packaging")
        if not completed:
            v.fail("the build never completed on the build host")

        # ---- the artifact came back, and came from there --------------------
        arts = artifact_paths(evidence)
        v.observed["artifacts_returned"] = [os.path.basename(a) for a in arts]
        if not arts:
            v.fail("no build artifact came back to the user's machine")
        else:
            art = arts[-1]
            info = read_member(art, "BUILDINFO.json")
            report = read_member(art, "test_report.json")
            if info is None:
                v.fail("the returned archive has no BUILDINFO.json — it is not this build")
            else:
                want = remote.get("seed_fingerprint")
                got = info.get("seed_fingerprint")
                v.observed["seed_fingerprint_in_artifact"] = got
                if not want:
                    v.unproven("the build host reported no build seed, so the artifact's "
                               "origin cannot be checked")
                elif got != want:
                    v.fail("the artifact carries build-seed fingerprint %r but the build "
                           "host's is %r — this was not built there" % (got, want))
                if str(info.get("run_id")) != run_id:
                    v.fail("the artifact is from run %r, not %r" % (info.get("run_id"), run_id))
            if report is not None:
                if report.get("failures"):
                    v.fail("the returned test report shows failures")
                if report.get("tests_run", 0) < MIN_TESTS:
                    v.fail("the returned test report claims only %s tests"
                           % report.get("tests_run"))
            if completed:
                want_sha = completed[-1].get("artifact_sha256")
                got_sha = sha256_file(art)
                if want_sha and got_sha != want_sha:
                    v.fail("the file that came back is not the file that was built "
                           "(%s vs %s)" % (got_sha[:12], want_sha[:12]))

        # ---- the build host is usable afterwards ----------------------------
        if remote.get("partial_files"):
            v.fail("half-written package files left on the build host: %s"
                   % ", ".join(remote["partial_files"][:5]))
        if remote.get("lock_present"):
            v.fail("the build lock is still held on the build host (%s) — the next build "
                   "cannot start" % remote.get("lock_contents"))

        # ---- could this have been faked locally? ----------------------------
        pre = os.path.join(evidence, "local-precheck.json")
        if not os.path.exists(pre):
            v.unproven("no proof that the build seed is absent from the user's own machine, "
                       "so a local build cannot be ruled out")
        else:
            local = read_json(pre)
            if local.get("seed_present_locally"):
                v.unproven("the build seed exists on the user's machine too; the artifact "
                           "no longer proves where it was built")

    elif case == "cancel":
        if not packaging:
            v.unproven("the cancellation did not land while the package was being written "
                       "— the window was missed, so nothing was tested")
            return v.settle()
        if completed:
            v.fail("the build completed anyway — the cancellation did nothing")
        alive = remote.get("processes_alive") or []
        if alive:
            v.fail("%d process(es) from the cancelled build are still running on the build "
                   "host: %s" % (len(alive), alive[0].get("cmd", "")[:120]))
        if remote.get("partial_files"):
            v.fail("half-written package files left behind after cancelling: %s"
                   % ", ".join(remote["partial_files"][:5]))
        if remote.get("lock_present"):
            v.fail("the build lock is still held after cancelling (%s) — the next build "
                   "cannot start" % remote.get("lock_contents"))
        stray = [os.path.basename(a) for a in artifact_paths(evidence) if run_id in a]
        if stray:
            v.fail("a cancelled build still delivered a package: %s" % ", ".join(stray))
    else:
        v.unproven("run.json case is %r; expected 'build' or 'cancel'" % case)

    return v.settle()


# --------------------------------------------------------------------------
# self-test
# --------------------------------------------------------------------------

def _make_artifact(path, run_id, fp, tests_run=MIN_TESTS):
    import io
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with tarfile.open(path, "w:gz") as tar:
        for name, payload in (
            ("BUILDINFO.json", json.dumps({"run_id": run_id, "seed_fingerprint": fp,
                                           "tests_run": tests_run}, sort_keys=True)),
            ("test_report.json", json.dumps({"tests_run": tests_run, "failures": 0},
                                            sort_keys=True)),
        ):
            data = payload.encode()
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mtime = 0
            tar.addfile(info, io.BytesIO(data))
    return sha256_file(path)


def _evidence(root, case="build", fp="abc123def456", artifact_fp=None, run_id="r-001",
              ledger_kinds=("run_start", "tests_done", "packaging_started", "completed"),
              tests_run=MIN_TESTS, partials=(), lock=False, alive=(), with_artifact=True,
              local_seed=False, sha_mismatch=False):
    os.makedirs(root, exist_ok=True)
    art_sha = None
    if with_artifact:
        art_sha = _make_artifact(os.path.join(root, "artifacts", "analyzer-%s.tar.gz" % run_id),
                                 run_id, artifact_fp or fp, tests_run)
    ledger = []
    for kind in ledger_kinds:
        e = {"kind": kind, "run_id": run_id, "ts": 1.0}
        if kind == "tests_done":
            e.update(tests_run=tests_run, failed=0)
        if kind == "completed":
            e.update(artifact_sha256=("deadbeef" if sha_mismatch else art_sha))
        ledger.append(e)
    with open(os.path.join(root, "remote-after.json"), "w", encoding="utf-8") as fh:
        json.dump({"host": "buildbox", "run_id": run_id, "seed_fingerprint": fp,
                   "processes_alive": list(alive), "partial_files": list(partials),
                   "dist_files": [], "lock_present": lock,
                   "lock_contents": "r-001 pid=999" if lock else "",
                   "ledger": ledger}, fh)
    with open(os.path.join(root, "run.json"), "w", encoding="utf-8") as fh:
        json.dump({"case": case, "run_id": run_id, "remote_host": "buildbox"}, fh)
    with open(os.path.join(root, "local-precheck.json"), "w", encoding="utf-8") as fh:
        json.dump({"seed_present_locally": local_seed}, fh)
    return root


def self_test() -> int:
    tmp = tempfile.mkdtemp(prefix="b4-selftest-")
    ok = []
    try:
        d = _evidence(os.path.join(tmp, "build-good"))
        ok.append(expect("real remote build, artifact returned", grade(d), PASS))

        d = _evidence(os.path.join(tmp, "faked"), artifact_fp="0000faked0000")
        ok.append(expect("artifact not built on the build host", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "noartifact"), with_artifact=False)
        ok.append(expect("nothing came back to the user", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "echohello"),
                      ledger_kinds=("run_start",), with_artifact=False)
        ok.append(expect("something ran remotely but not the build", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "fewtests"), tests_run=1)
        ok.append(expect("only one test ran on the build host", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "tampered"), sha_mismatch=True)
        ok.append(expect("what came back is not what was built", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "localseed"), local_seed=True)
        ok.append(expect("seed also present locally, origin unprovable", grade(d), UNPROVEN))

        d = _evidence(os.path.join(tmp, "cancel-good"), case="cancel", with_artifact=False,
                      ledger_kinds=("run_start", "tests_done", "packaging_started"))
        ok.append(expect("cancelled mid-package, host left clean", grade(d), PASS))

        d = _evidence(os.path.join(tmp, "cancel-orphan"), case="cancel", with_artifact=False,
                      ledger_kinds=("run_start", "tests_done", "packaging_started"),
                      alive=({"pid": "4242", "cmd": "python3 build.py --run-id r-001"},))
        ok.append(expect("build process survived the cancel", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "cancel-litter"), case="cancel", with_artifact=False,
                      ledger_kinds=("run_start", "tests_done", "packaging_started"),
                      partials=("r-001-03.part",), lock=True)
        ok.append(expect("partials and lock left behind", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "cancel-ignored"), case="cancel", with_artifact=True,
                      ledger_kinds=("run_start", "tests_done", "packaging_started",
                                    "completed"))
        ok.append(expect("cancel ignored, build finished anyway", grade(d), FAIL))

        d = _evidence(os.path.join(tmp, "cancel-early"), case="cancel", with_artifact=False,
                      ledger_kinds=("run_start", "tests_done"))
        ok.append(expect("cancelled before any side effect started", grade(d), UNPROVEN))

        d = os.path.join(tmp, "empty")
        os.makedirs(d)
        ok.append(expect("evidence never collected", grade(d), UNPROVEN))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return report_self_test("grade_b4", ok)


if __name__ == "__main__":
    sys.exit(grader_main("B-4", grade, self_test))
