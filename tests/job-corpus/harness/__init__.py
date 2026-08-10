"""Job-corpus harness core.

Drives the built product binary as a BLACK BOX and grades the world, not the
product's own account of the world.  Imports, links to and depends on nothing
from Wayland Core: if a row needed a product internal to be graded, the row is
written wrong.

Public surface
--------------
    GATE_ROSTER           the 22 declared gates; a run states all of them
    RowContext            per-row driver; seeds and grades Tier 0 automatically
    RowRunner             the raw subprocess layer (binary sha256, timeouts)
    FsSnapshot, GitState, IndependentTests, ProcessTable   world-state grader
    Meter, HarnessLedger, Claims                           INV-5 metering
    Check, RowRecord, PASS/FAIL/UNPROVEN/NA/NOTE           five-state results
"""

from .invariants import (
    DEFAULT_SCOPE_IGNORE,
    DirtyWorktreeSeed,
    HonestyCheck,
    ScopeCheck,
    TestFileMetrics,
    TestWeakeningCheck,
    sealed_tests_check,
)
from .meter import Claims, HarnessLedger, Meter
from .result import (
    FAIL,
    GATE_ROSTER,
    GREEN,
    INCOMPLETE,
    NA,
    NOTE,
    PASS,
    RED,
    ROSTER_GATES,
    UNPROVEN,
    Check,
    CommandRecord,
    HarnessError,
    RowRecord,
    exit_code_for,
    gate_report,
    invariant,
    roll_up,
    summarise,
)
from .rowctx import DEFAULT_SEAL_GLOBS, RowContext
from .runner import RowRunner, prepare_workspace, scratch_dir
from .world import (
    FsSnapshot,
    GitState,
    IndependentTests,
    ProcessTable,
    TestRun,
    kill_tree,
    sha256_bytes,
    sha256_file,
)

__all__ = [
    "sealed_tests_check",
    "gate_report",
    "exit_code_for",
    "ROSTER_GATES",
    "RED",
    "INCOMPLETE",
    "GREEN",
    "GATE_ROSTER",
    "DEFAULT_SCOPE_IGNORE",
    "Check",
    "Claims",
    "CommandRecord",
    "DEFAULT_SEAL_GLOBS",
    "DirtyWorktreeSeed",
    "FAIL",
    "FsSnapshot",
    "GitState",
    "HarnessError",
    "HarnessLedger",
    "HonestyCheck",
    "IndependentTests",
    "Meter",
    "NA",
    "NOTE",
    "PASS",
    "ProcessTable",
    "RowContext",
    "RowRecord",
    "RowRunner",
    "ScopeCheck",
    "TestFileMetrics",
    "TestRun",
    "TestWeakeningCheck",
    "UNPROVEN",
    "invariant",
    "kill_tree",
    "prepare_workspace",
    "roll_up",
    "scratch_dir",
    "sha256_bytes",
    "sha256_file",
    "summarise",
]
