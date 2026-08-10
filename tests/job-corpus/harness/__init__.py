"""Job-corpus harness core.

Drives the built product binary as a BLACK BOX and grades the world, not the
product's own account of the world.  Imports, links to and depends on nothing
from Wayland Core: if a row needed a product internal to be graded, the row is
written wrong.

Public surface
--------------
    RowContext            per-row driver; seeds and grades Tier 0 automatically
    RowRunner             the raw subprocess layer (binary sha256, timeouts)
    FsSnapshot, GitState, IndependentTests, ProcessTable   world-state grader
    Meter, HarnessLedger, Claims                           INV-5 metering
    Check, RowRecord, PASS/FAIL/UNPROVEN/NA/NOTE           five-state results
"""

from .invariants import (
    DirtyWorktreeSeed,
    HonestyCheck,
    ScopeCheck,
    TestFileMetrics,
    TestWeakeningCheck,
)
from .meter import Claims, HarnessLedger, Meter
from .result import (
    FAIL,
    NA,
    NOTE,
    PASS,
    UNPROVEN,
    Check,
    CommandRecord,
    HarnessError,
    RowRecord,
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
