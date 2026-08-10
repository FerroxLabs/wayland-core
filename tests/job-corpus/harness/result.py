"""Five-state result records for the job-corpus harness.

The harness grades the WORLD, never the agent's own receipts.  Every record
therefore has to name the artifact it graded (binary path + sha256), the host
it ran on, the exact commands it issued, and the world-state evidence it
collected.  A record missing the binary sha256 is void and refuses to
serialise.

States
------
PASS      the user-visible outcome was observed in the world
FAIL      it was not (including "the capability is absent" and "it refused")
UNPROVEN  the harness could not obtain reliable evidence either way
N/A       genuinely out of scope; leaves the denominator
NOTE      observed but unscored; never affects a verdict

Pure stdlib, Python 3.8+, Linux / macOS / Windows.
"""

from __future__ import annotations

import json
import os
import platform
import socket
import time
from typing import Any, Dict, Iterable, List, Optional

PASS = "PASS"
FAIL = "FAIL"
UNPROVEN = "UNPROVEN"
NA = "N/A"
NOTE = "NOTE"

SCORED_STATES = (PASS, FAIL, UNPROVEN, NA)
STATES = SCORED_STATES + (NOTE,)


class HarnessError(RuntimeError):
    """Raised when the harness itself is misconfigured (never a row failure)."""


class Check:
    """One graded assertion about the world."""

    __slots__ = ("check_id", "state", "why", "evidence", "kind")

    def __init__(
        self,
        check_id: str,
        state: str,
        why: str,
        evidence: Optional[Dict[str, Any]] = None,
        kind: str = "row",
    ) -> None:
        if state not in STATES:
            raise HarnessError(f"{check_id}: unknown state {state!r}")
        if not why:
            raise HarnessError(f"{check_id}: every check must state its reason")
        self.check_id = check_id
        self.state = state
        self.why = why
        self.evidence = evidence or {}
        self.kind = kind  # "row" | "invariant"

    def to_dict(self) -> Dict[str, Any]:
        return {
            "check_id": self.check_id,
            "kind": self.kind,
            "state": self.state,
            "why": self.why,
            "evidence": self.evidence,
        }

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"<Check {self.check_id} {self.state}: {self.why}>"


def invariant(check_id: str, state: str, why: str, evidence=None) -> Check:
    return Check(check_id, state, why, evidence, kind="invariant")


def roll_up(checks: Iterable[Check]) -> str:
    """Collapse checks into a single row verdict.

    FAIL dominates everything.  A row whose scored checks are all N/A is N/A.
    Any UNPROVEN below that.  Only an all-PASS row passes.  NOTEs are inert.
    """
    scored = [c for c in checks if c.state != NOTE]
    if not scored:
        return UNPROVEN
    states = {c.state for c in scored}
    if FAIL in states:
        return FAIL
    if states == {NA}:
        return NA
    if UNPROVEN in states:
        return UNPROVEN
    return PASS


class CommandRecord:
    """One subprocess the harness issued, with its observable outcome."""

    __slots__ = (
        "argv",
        "cwd",
        "exit_code",
        "timed_out",
        "duration_s",
        "stdout_path",
        "stderr_path",
        "stdout_sha256",
        "stderr_sha256",
        "role",
        "env_scrubbed",
    )

    def __init__(self, **kw: Any) -> None:
        for slot in self.__slots__:
            setattr(self, slot, kw.get(slot))

    def to_dict(self) -> Dict[str, Any]:
        return {s: getattr(self, s) for s in self.__slots__}


class RowRecord:
    """Machine-readable record for one corpus row."""

    def __init__(
        self,
        row_id: str,
        binary_path: Optional[str],
        binary_sha256: Optional[str],
        tier: str = "",
        title: str = "",
    ) -> None:
        self.row_id = row_id
        self.tier = tier
        self.title = title
        self.binary_path = binary_path
        self.binary_sha256 = binary_sha256
        self.host = {
            "hostname": socket.gethostname(),
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        }
        self.started_at = time.time()
        self.ended_at: Optional[float] = None
        self.commands: List[CommandRecord] = []
        self.checks: List[Check] = []
        self.world: Dict[str, Any] = {}
        self.notes: List[str] = []

    # -- accumulation ----------------------------------------------------
    def add_command(self, rec: CommandRecord) -> None:
        self.commands.append(rec)

    def add_check(self, check: Check) -> None:
        self.checks.append(check)

    def add_checks(self, checks: Iterable[Check]) -> None:
        for c in checks:
            self.add_check(c)

    def note(self, text: str) -> None:
        self.notes.append(text)

    # -- verdicts --------------------------------------------------------
    @property
    def invariant_checks(self) -> List[Check]:
        return [c for c in self.checks if c.kind == "invariant"]

    @property
    def row_checks(self) -> List[Check]:
        return [c for c in self.checks if c.kind == "row"]

    def row_verdict(self) -> str:
        return roll_up(self.row_checks)

    def invariant_verdict(self) -> str:
        inv = self.invariant_checks
        if not inv:
            return UNPROVEN
        return roll_up(inv)

    def verdict(self) -> str:
        """A Tier-0 invariant failure fails the row regardless of the row work."""
        rv = self.row_verdict()
        iv = self.invariant_verdict()
        if FAIL in (rv, iv):
            return FAIL
        if rv == NA:
            return NA
        if UNPROVEN in (rv, iv):
            return UNPROVEN
        return PASS

    # -- emission --------------------------------------------------------
    def to_dict(self) -> Dict[str, Any]:
        if not self.binary_sha256:
            raise HarnessError(
                f"row {self.row_id}: refusing to emit a result that cannot name "
                "the artifact it ran (binary_sha256 missing)"
            )
        self.ended_at = self.ended_at or time.time()
        return {
            "schema": "wayland-core/job-corpus/row-record/1",
            "row_id": self.row_id,
            "tier": self.tier,
            "title": self.title,
            "verdict": self.verdict(),
            "row_verdict": self.row_verdict(),
            "invariant_verdict": self.invariant_verdict(),
            "artifact": {"path": self.binary_path, "sha256": self.binary_sha256},
            "host": self.host,
            "started_at": self.started_at,
            "ended_at": self.ended_at,
            "duration_s": round(self.ended_at - self.started_at, 3),
            "commands": [c.to_dict() for c in self.commands],
            "checks": [c.to_dict() for c in self.checks],
            "world": self.world,
            "notes": self.notes,
        }

    def write(self, path: str) -> str:
        data = self.to_dict()
        parent = os.path.dirname(os.path.abspath(path))
        if parent:
            os.makedirs(parent, exist_ok=True)
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump(data, fh, indent=2, sort_keys=True, default=str)
            fh.write("\n")
        os.replace(tmp, path)
        return path


def summarise(records: Iterable[Dict[str, Any]]) -> Dict[str, Any]:
    """Corpus-level tally.  N/A rows leave the denominator, as specified."""
    rows = list(records)
    counts = {s: 0 for s in SCORED_STATES}
    for r in rows:
        counts[r.get("verdict", UNPROVEN)] = counts.get(r.get("verdict", UNPROVEN), 0) + 1
    denominator = counts[PASS] + counts[FAIL] + counts[UNPROVEN]
    return {
        "schema": "wayland-core/job-corpus/summary/1",
        "counts": counts,
        "denominator": denominator,
        "rows": [
            {
                "row_id": r.get("row_id"),
                "tier": r.get("tier"),
                "verdict": r.get("verdict"),
                "artifact_sha256": (r.get("artifact") or {}).get("sha256"),
                "host": (r.get("host") or {}).get("hostname"),
            }
            for r in rows
        ],
    }
