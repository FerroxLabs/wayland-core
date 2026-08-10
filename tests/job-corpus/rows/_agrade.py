"""Shared plumbing for the A-7 .. A-11 drivers.

These four rows differ from A-1 .. A-6 in one respect only: their rubric is
already implemented as a standalone grader under ``keys/`` (``a07_grade.py``,
``a08_grade.py``, ``a09_probe.py``, ``a11_verify.py``).  The row driver's job is
therefore to build the fixture, DRIVE THE PRODUCT AGAINST IT, and then hand the
world the product left behind to that grader — never to re-implement the rubric
in a second place where the two could disagree.

Every grader here speaks the same interface: argv, a JSON report on stdout,
exit 0 for PASS.  ``apply_verdict`` turns that report into graded checks, and
maps an unreadable or missing report to UNPROVEN rather than to a FAIL the
product did not earn.

Pure stdlib.  Never imports product code.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from typing import Any, Dict, List, Optional, Sequence, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS_ROOT = os.path.dirname(HERE)

if CORPUS_ROOT not in sys.path:
    sys.path.insert(0, CORPUS_ROOT)

from harness.result import FAIL, NOTE, PASS, UNPROVEN, Check  # noqa: E402

PY = sys.executable or "python3"

#: git invoked with the operator's own configuration held at arm's length, so a
#: fixture repository is the same repository on every host.
GIT_IDENTITY = (
    "-c", "user.name=Job Corpus",
    "-c", "user.email=job-corpus@example.invalid",
    "-c", "commit.gpgsign=false",
)


def _git_env() -> Dict[str, str]:
    env = dict(os.environ)
    env.pop("API_KEY", None)
    env.pop("FLUX_API_KEY", None)
    env["GIT_CONFIG_NOSYSTEM"] = "1"
    env["GIT_CONFIG_GLOBAL"] = os.devnull
    env["GIT_AUTHOR_DATE"] = "2026-02-11T09:00:00+00:00"
    env["GIT_COMMITTER_DATE"] = "2026-02-11T09:00:00+00:00"
    return env


def git(repo: str, *args: str) -> Tuple[int, str]:
    proc = subprocess.run(
        ["git", "-C", repo, *GIT_IDENTITY, *args],
        env=_git_env(),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=300,
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def is_tracked(repo: str, rel: str) -> bool:
    return git(repo, "ls-files", "--error-unmatch", "--", rel)[0] == 0


def copy_fixture_repo(fixture_dir: str, dest: str, include: Sequence[str] = ()) -> str:
    """Copy a plain fixture directory into ``dest`` and make it a real git repo.

    A-7, A-9 and A-11 ship their fixture as ordinary files rather than as a
    ``build.py``.  They still need a repository: INV-2's "a careless
    `git checkout -- .` destroyed the user's unsaved work" shape does not exist
    outside one, and grading a row whose invariant cannot fire would be grading
    less than the row claims.
    """
    import shutil

    if os.path.isdir(dest):
        shutil.rmtree(dest, ignore_errors=True)
    os.makedirs(dest, exist_ok=True)
    names = list(include) if include else sorted(os.listdir(fixture_dir))
    for name in names:
        src = os.path.join(fixture_dir, name)
        dst = os.path.join(dest, name)
        if os.path.isdir(src):
            shutil.copytree(src, dst, ignore=shutil.ignore_patterns("__pycache__"))
        elif os.path.isfile(src):
            shutil.copyfile(src, dst)
    rc, out = git(dest, "init", "-q", "-b", "main")
    if rc != 0:
        raise RuntimeError("could not git init the fixture workspace:\n" + out)
    git(dest, "add", "-A")
    rc, out = git(dest, "commit", "-q", "-m", "Fixture as handed to the agent")
    if rc != 0:
        raise RuntimeError("could not commit the fixture workspace:\n" + out)
    return dest


def read_text(path: str) -> Optional[str]:
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return None


def task_prompt(fixture_dir: str, filename: str = "TASK.md") -> str:
    text = read_text(os.path.join(fixture_dir, filename))
    if not text:
        raise RuntimeError("fixture %s has no %s to hand the agent" % (fixture_dir, filename))
    return text.strip()


def run_grader(
    argv: Sequence[str],
    cwd: str,
    json_path: str,
    timeout: int = 1800,
) -> Tuple[Optional[int], Optional[Dict[str, Any]], str]:
    """Run one of the keys/ graders.  Returns (exit code, report, raw output)."""
    env = dict(os.environ)
    env.pop("API_KEY", None)
    env.pop("FLUX_API_KEY", None)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env.pop("PYTHONPATH", None)
    try:
        proc = subprocess.run(
            list(argv),
            cwd=cwd,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
        rc: Optional[int] = proc.returncode
        raw = proc.stdout.decode("utf-8", "replace")
    except subprocess.TimeoutExpired as exc:
        rc = None
        raw = (exc.stdout or b"").decode("utf-8", "replace") + "\nharness: the grader timed out"
    except OSError as exc:
        return None, None, "harness: could not launch the grader %r: %s" % (list(argv), exc)

    report = None
    if os.path.isfile(json_path):
        try:
            with open(json_path, "r", encoding="utf-8") as fh:
                report = json.load(fh)
        except (ValueError, OSError):
            report = None
    if report is None:
        # The graders print the same JSON they write, so a missing --json file
        # is recoverable rather than fatal.
        start = raw.find("{")
        if start >= 0:
            try:
                report = json.loads(raw[start:])
            except ValueError:
                report = None
    return rc, report, raw


def apply_verdict(
    ctx: Any,
    check_id: str,
    pass_why: str,
    report: Optional[Dict[str, Any]],
    raw: str,
    unreadable_why: str,
) -> str:
    """Turn a grader report into one graded check.  Returns the state applied."""
    if not isinstance(report, dict) or not report.get("verdict"):
        ctx.add_check(
            Check(check_id, UNPROVEN, unreadable_why, {"grader_output_tail": raw[-4000:]})
        )
        return UNPROVEN
    verdict = str(report["verdict"]).upper()
    reasons: List[str] = [str(r) for r in (report.get("reasons") or [])]
    evidence = {"grader_report": report, "grader_output_tail": raw[-4000:]}
    if verdict == "PASS":
        ctx.add_check(Check(check_id, PASS, pass_why, evidence))
        return PASS
    if verdict == "FAIL":
        ctx.add_check(
            Check(
                check_id,
                FAIL,
                "; ".join(reasons[:8]) or "the grader returned FAIL without naming a reason",
                evidence,
            )
        )
        return FAIL
    ctx.add_check(
        Check(
            check_id,
            UNPROVEN,
            "; ".join(reasons[:8]) or "the grader could not reach a verdict",
            evidence,
        )
    )
    return UNPROVEN


def set_independent(ctx: Any, argv: Sequence[str], ok: bool, output: str, workdir: str) -> None:
    """Record the grader's own run as this row's independent verification.

    INV-5.completion asks whether "all tests pass" was a true statement. These
    rows have no ``TEST_COMMAND`` of their own because the rubric already runs
    the suite (or drives the service) itself — so that run is what the product's
    account has to agree with. Without this the invariant would be UNPROVEN on
    every one of them, and an invariant that is UNPROVEN everywhere has not been
    shown to work at all.
    """
    from harness.world import TestRun

    ctx.independent_result = TestRun(
        list(argv), 0 if ok else 1, output, "", 0.0, False, workdir, []
    )


def note(ctx: Any, check_id: str, why: str, evidence: Optional[Dict[str, Any]] = None) -> None:
    ctx.add_check(Check(check_id, NOTE, why, evidence or {}))


def port_is_free(port: int) -> bool:
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            sock.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True
