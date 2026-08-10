"""Shared helpers for the B-row graders.

Hard rules obeyed by every grader in this directory:

  * The grader NEVER imports, links to, or reads Wayland Core source. It reads
    only world artifacts: files on disk, append-only ledgers written by
    harness-owned server processes, git output captured as text, and process
    tables captured as text.
  * The graded product's own self-report is never an input. If an evidence file
    was produced by the program under test rather than by a harness-owned
    observer, it may be used for provenance only, never to decide PASS.
  * Five states: PASS / FAIL / UNPROVEN / NA / NOTE. Refusal is never a pass.
    Missing or unreadable evidence is UNPROVEN, never PASS.

Python 3.8+, standard library only, runs on Linux, macOS and Windows.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

PASS = "PASS"
FAIL = "FAIL"
UNPROVEN = "UNPROVEN"
NA = "N/A"


@dataclass
class Verdict:
    row: str
    state: str = ""  # settled to PASS unless a check downgrades it
    reasons: List[str] = field(default_factory=list)
    notes: List[str] = field(default_factory=list)
    observed: Dict[str, Any] = field(default_factory=dict)

    def fail(self, reason: str) -> None:
        self.state = FAIL
        self.reasons.append(reason)

    def unproven(self, reason: str) -> None:
        # FAIL is stickier than UNPROVEN: a proven defect is not erased by a
        # later piece of missing evidence.
        if self.state != FAIL:
            self.state = UNPROVEN
        self.reasons.append(reason)

    def note(self, text: str) -> None:
        self.notes.append(text)

    def settle(self) -> "Verdict":
        if self.state not in (FAIL, UNPROVEN):
            self.state = PASS
        return self

    def to_json(self) -> str:
        return json.dumps(
            {
                "row": self.row,
                "state": self.state,
                "reasons": self.reasons,
                "notes": self.notes,
                "observed": self.observed,
            },
            indent=2,
            sort_keys=True,
        )


def read_jsonl(path: str) -> List[Dict[str, Any]]:
    out: List[Dict[str, Any]] = []
    with open(path, "r", encoding="utf-8") as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError as exc:
                raise ValueError("%s:%d is not JSON: %s" % (path, lineno, exc))
    return out


def read_text(path: str) -> str:
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        return fh.read()


def read_json(path: str) -> Any:
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)


def require_files(v: Verdict, evidence: str, names: List[str]) -> bool:
    """UNPROVEN (never PASS, never FAIL) when the harness failed to collect."""
    missing = [n for n in names if not os.path.exists(os.path.join(evidence, n))]
    if missing:
        v.unproven("evidence not collected: " + ", ".join(sorted(missing)))
        return False
    return True


def kill_tree(pid: int) -> None:
    """Kill a process and every descendant. Used by fixture servers that must
    interrupt the job under test at an exact write boundary."""
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/PID", str(pid), "/T", "/F"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        return
    # POSIX: prefer the process group (runners are required to start the job
    # under test with start_new_session=True), fall back to a pgrep walk.
    try:
        os.killpg(os.getpgid(pid), 9)
        return
    except Exception:
        pass
    kids: List[str] = []
    if shutil.which("pgrep"):
        res = subprocess.run(
            ["pgrep", "-P", str(pid)], capture_output=True, text=True, check=False
        )
        kids = [k for k in res.stdout.split() if k.strip()]
    for kid in kids:
        try:
            kill_tree(int(kid))
        except Exception:
            pass
    try:
        os.kill(pid, 9)
    except Exception:
        pass


def grader_main(row: str, grade_fn, self_test_fn=None) -> int:
    ap = argparse.ArgumentParser(description="Grader for row %s" % row)
    ap.add_argument("--evidence", help="evidence directory for one case")
    ap.add_argument(
        "--self-test",
        action="store_true",
        help="run the grader against synthetic good/bad evidence and exit",
    )
    args = ap.parse_args()
    if args.self_test:
        if self_test_fn is None:
            print("no self-test for %s" % row, file=sys.stderr)
            return 2
        return self_test_fn()
    if not args.evidence:
        ap.error("--evidence is required unless --self-test is given")
    v = grade_fn(args.evidence)
    print(v.to_json())
    return 0 if v.state == PASS else 1


def expect(label: str, got: Verdict, want_state: str) -> bool:
    ok = got.state == want_state
    print(
        "  [%s] %-46s expected %-8s got %-8s %s"
        % ("ok" if ok else "XX", label, want_state, got.state, "" if ok else got.reasons)
    )
    return ok


def report_self_test(name: str, results: List[bool]) -> int:
    bad = results.count(False)
    print("%s self-test: %d/%d checks passed" % (name, len(results) - bad, len(results)))
    return 0 if bad == 0 else 1


def normalise_id(text: str) -> str:
    return text.strip().strip('"').strip("'").upper()


class Workspace:
    """Read-only view over a captured copy of the user's project directory."""

    def __init__(self, root: str) -> None:
        self.root = root

    def exists(self, rel: str) -> bool:
        return os.path.exists(os.path.join(self.root, rel))

    def text(self, rel: str) -> str:
        return read_text(os.path.join(self.root, rel))

    def walk_files(self) -> List[str]:
        out: List[str] = []
        for base, dirs, files in os.walk(self.root):
            dirs[:] = [d for d in dirs if d != ".git"]
            for f in files:
                rel = os.path.relpath(os.path.join(base, f), self.root)
                out.append(rel.replace(os.sep, "/"))
        return sorted(out)
