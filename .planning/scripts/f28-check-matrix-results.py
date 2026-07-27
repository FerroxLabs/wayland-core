#!/usr/bin/env python3
"""Phase 28 results validator — the gate plans 02, 03 and 04 all run.

This script validates THREE artifacts and the relationships between them:

  * `evidence/28-02/controls.json` — the measured answer to whether the Windows
    sandbox is observable in the certification environment.
  * `evidence/28-02/results.json`  — the per-cell matrix outcomes.
  * `evidence/28-01/matrix.tsv`    — the generated cell set those outcomes must cover.

It enforces, mechanically, the rules the certification contract states in prose:

  1. CONTROL PRECEDENCE.  No sandbox-dimension verdict may be recorded before the
     observability control produced a verdict.  A sandbox cell that cites no control,
     or cites a control recorded after it, is rejected.
  2. ACTIVENESS.  A green on a sandbox-dimension cell requires POSITIVE evidence the
     sandbox was active.  Absence of an observed violation is not evidence of a
     sandbox and is not expressible here: `observed` is a boolean that must be true
     and must carry a non-empty probe AND detail.
  3. SKIP LEGALITY.  Exactly four classes exist.  Each carries its own required
     evidence.  A CRITICAL cell has NO legal skip.  `observation-blocked` requires a
     run-time control reference and rejects every documentary citation — including
     one that reports in the product's favour.
  4. ATTRIBUTION.  Every red is attributed to a carried-red ledger id or filed as an
     explicit new finding with a Phase 28 re-score.
  5. BINARY BINDING.  Each family's run names the binary it exercised by digest, and
     that digest is either bound to the candidate ledger or carries an explicit
     unbound reason.  A family whose binary is neither is void, not approximate.

## Why every mode has a self-test that trips it in BOTH directions

A validator that only ever proves its rejections is indiscriminate; a validator that
only ever proves its acceptances is vacuous.  This program has shipped both diseases.
`--self-test` therefore asserts, for every rejection code, that a bad fixture trips it
AND that a good fixture does NOT.

## A measurement that cannot be taken must never render as 0

Counts in this script are only emitted for sets that were actually enumerated.  Where
a measurement is absent the field is `null` and the checker says so; it never prints a
zero that a reader could mistake for a measured absence.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# ---------------------------------------------------------------------------------
# Vocabulary — mirrors the contract and `e5_matrix.rs`.  A rename in either fails here.
# ---------------------------------------------------------------------------------

SKIP_CLASSES = {
    "platform-inapplicability": ("fact", "observable"),
    "observation-blocked": ("control_ref",),
    "architectural-impossibility": ("impossibility_check",),
    "unresolved-surface": ("phase", "req_disposition"),
}

DIMENSIONS = [
    "sandbox-probes",
    "unicode",
    "long-paths",
    "unc-reparse-symlink",
    "process-cleanup",
    "suspend-resume",
    "offline",
    "disk-full-read-only",
    "hostile-inputs",
]

SANDBOX_DIMENSION = "sandbox-probes"

# Documentary-citation markers.  Mirrors LORE_MARKERS in `e5_matrix.rs` and
# LORE_PATTERNS in `f28-ledger.py`.  Pointing the channel at good news does not make
# it sound, so a favourable intel file matches exactly like an unfavourable one.
LORE_MARKERS = (
    ".md",
    "handoff",
    "intel/",
    "-plan",
    "-summary",
    "requirements",
    "roadmap",
    "lore",
    "as established",
)

CONTROL_REF_RE = re.compile(r"^control:([A-Za-z0-9-]+)@([A-Za-z0-9._-]+):([A-Za-z0-9._-]+)$")

SESSION_TYPES = ("ssh", "scheduled-task")
LEASE_STATES = ("as-found", "cleared", "wedged")

PROBE_REPORTS = ("available", "unavailable")
# What the PRODUCT DID, recorded SEPARATELY from what the probe reported.  Conflating
# these is how a silent-disablement defect gets filed as a tooling quirk, so the two
# fields are disjoint vocabularies and neither can be derived from the other.
PRODUCT_BEHAVIOURS = (
    "executed-sandboxed",       # ran the work, sandbox positively observed active
    "refused-fail-closed",      # declined to run the work at all
    "proceeded-unsandboxed",    # ran the work with no sandbox — the CRITICAL combination
    "indeterminate",            # neither could be established; a red, never a green
)

VERDICTS = ("channel-sound", "channel-broken-and-unclearable", "wedge-clearable", "inconclusive")

DIRECTIONS = ("positive", "negative")
DIRECTIONAL_OUTCOMES = ("observable", "unobservable", "activeness-present", "activeness-absent")

OUTCOMES = ("pass", "red", "skip")


class Fail(Exception):
    """A rejection, carrying the code the SUMMARY and the plan gates quote."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code} {message}")
        self.code = code


def fail(code: str, message: str) -> None:
    raise Fail(code, message)


def _load(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail("F28R-000", f"artifact does not exist: {path}")
    except json.JSONDecodeError as exc:  # pragma: no cover - exercised by self-test
        fail("F28R-000", f"{path} is not JSON: {exc}")
    raise AssertionError("unreachable")


def _req(obj: dict, key: str, code: str, where: str) -> object:
    if key not in obj:
        fail(code, f"{where}: required field `{key}` is absent")
    value = obj[key]
    if value is None or (isinstance(value, str) and not value.strip()):
        fail(code, f"{where}: required field `{key}` is empty")
    return value


# ---------------------------------------------------------------------------------
# control_ref
# ---------------------------------------------------------------------------------


def check_control_ref(ref: str, where: str) -> None:
    """`control:<id>@<host>:<run-id>`, and NOT a documentary citation.

    Order matters: the lore check runs FIRST, so a reference that is both
    well-shaped and a citation is rejected as a citation.  A path can be made to
    look like a control id; the point of the class is that only a measurement
    counts.
    """
    lowered = ref.lower()
    for marker in LORE_MARKERS:
        if marker in lowered:
            fail(
                "F28R-004",
                f"{where}: observation-blocked evidence `{ref}` cites a document "
                f"(matched `{marker}`); a citation of a handoff, a prior phase or any "
                f"inherited belief is not evidence, including one that reports in the "
                f"product's favour",
            )
    if not CONTROL_REF_RE.match(ref):
        fail(
            "F28R-005",
            f"{where}: observation-blocked evidence `{ref}` is not a control measured "
            f"in the certification environment at run time (control:<id>@<host>:<run-id>); "
            f"absent such a control the cell is a RED, not a skip",
        )


# ---------------------------------------------------------------------------------
# --check-controls
# ---------------------------------------------------------------------------------


def check_controls(doc: dict) -> dict:
    """Validate `controls.json`: six observations, two session types, three lease states.

    The six are required as a COMPLETE cross product, not as a count.  Six rows that
    all name the same session type would satisfy a count and prove nothing, so the
    check is over the set of (session_type, lease_state) pairs.
    """
    if doc.get("schema") != "f28-controls/1":
        fail("F28R-010", f"controls: schema is `{doc.get('schema')}`, expected `f28-controls/1`")

    host = _req(doc, "host", "F28R-011", "controls")
    run_id = _req(doc, "run_id", "F28R-011", "controls")
    verdict = _req(doc, "verdict", "F28R-012", "controls")
    if verdict not in VERDICTS:
        fail(
            "F28R-012",
            f"controls: verdict `{verdict}` is not one of the named outcomes {VERDICTS}",
        )

    ref = str(_req(doc, "control_ref", "F28R-013", "controls"))
    check_control_ref(ref, "controls.control_ref")
    matched = CONTROL_REF_RE.match(ref)
    assert matched is not None
    if matched.group(2) != host or matched.group(3) != str(run_id):
        fail(
            "F28R-013",
            f"controls: control_ref `{ref}` does not bind the host and run it was "
            f"measured on (host={host}, run_id={run_id}); an unbound reference names "
            f"no measurement",
        )

    if not isinstance(doc.get("quiet_run"), bool):
        fail("F28R-014", "controls: `quiet_run` must be recorded as a boolean")
    if doc["quiet_run"] is not True:
        fail(
            "F28R-014",
            "controls: the run was not quiet; the two registered Windows runners are one "
            "physical box and a result observed under concurrent load is not recordable",
        )

    observations = doc.get("observations")
    if not isinstance(observations, list):
        fail("F28R-015", "controls: `observations` must be a list")

    seen: set[tuple[str, str]] = set()
    for index, obs in enumerate(observations):
        where = f"controls.observations[{index}]"
        session = str(_req(obs, "session_type", "F28R-016", where))
        lease = str(_req(obs, "lease_state", "F28R-016", where))
        if session not in SESSION_TYPES:
            fail("F28R-016", f"{where}: session_type `{session}` is not one of {SESSION_TYPES}")
        if lease not in LEASE_STATES:
            fail("F28R-016", f"{where}: lease_state `{lease}` is not one of {LEASE_STATES}")
        if (session, lease) in seen:
            fail("F28R-017", f"{where}: duplicate observation for ({session}, {lease})")
        seen.add((session, lease))

        report = str(_req(obs, "probe_report", "F28R-018", where))
        if report not in PROBE_REPORTS:
            fail("F28R-018", f"{where}: probe_report `{report}` is not one of {PROBE_REPORTS}")
        _req(obs, "probe_evidence", "F28R-018", where)

        # The security half.  What the product DID is a separate field with a
        # disjoint vocabulary, so it cannot be inferred from what the probe said.
        behaviour = str(_req(obs, "product_behaviour", "F28R-019", where))
        if behaviour not in PRODUCT_BEHAVIOURS:
            fail(
                "F28R-019",
                f"{where}: product_behaviour `{behaviour}` is not one of "
                f"{PRODUCT_BEHAVIOURS}; what the product did is recorded separately from "
                f"what the probe reported",
            )
        _req(obs, "product_evidence", "F28R-019", where)

        if report == "available" and behaviour == "proceeded-unsandboxed":
            fail(
                "F28R-020",
                f"{where}: the probe reported the sandbox available while the product "
                f"proceeded unsandboxed; those two cannot both hold and the observation "
                f"is not recordable",
            )

    expected = {(s, l) for s in SESSION_TYPES for l in LEASE_STATES}
    missing = sorted(expected - seen)
    if missing:
        fail(
            "F28R-021",
            f"controls: the (session_type, lease_state) cross product is incomplete; "
            f"missing {missing}. Six observations are required across two session types "
            f"and three lease states, and a count of six is not the same thing",
        )

    authorised = doc.get("observation_blocked_authorised")
    if not isinstance(authorised, bool):
        fail("F28R-022", "controls: `observation_blocked_authorised` must be a boolean")
    if verdict in ("channel-sound", "wedge-clearable") and authorised:
        fail(
            "F28R-023",
            f"controls: verdict `{verdict}` means the channel yields a verdict, so the "
            f"observation-blocked skip class is NOT authorised; it is recorded as "
            f"authorised",
        )
    if verdict == "channel-broken-and-unclearable" and not authorised:
        cells = doc.get("authorised_cells")
        if cells:
            fail(
                "F28R-023",
                "controls: cells are listed as authorised while the class is not authorised",
            )
    if authorised:
        cells = doc.get("authorised_cells")
        if not isinstance(cells, list) or not cells:
            fail(
                "F28R-024",
                "controls: the observation-blocked class is authorised but names no cells; "
                "the class is authorised only for the specific cells the control covers",
            )

    return {
        "observations": len(observations),
        "pairs": len(seen),
        "verdict": verdict,
        "control_ref": ref,
    }


# ---------------------------------------------------------------------------------
# --check-control-directions
# ---------------------------------------------------------------------------------


def check_control_directions(doc: dict) -> dict:
    """Both directional controls must be present AND must behave directionally.

    A positive control that reports unobservable where the channel should be sound
    means the control is measuring something other than what it claims.  A negative
    control that reports observable means the probe reports availability
    unconditionally, in which case every green it ever produced is worthless.  Both
    are fatal to the run here rather than noise recorded in prose.
    """
    controls = doc.get("directional_controls")
    if not isinstance(controls, list) or not controls:
        fail("F28R-030", "controls: `directional_controls` is absent or empty")

    by_direction: dict[str, int] = {"positive": 0, "negative": 0}
    for index, ctl in enumerate(controls):
        where = f"controls.directional_controls[{index}]"
        cid = str(_req(ctl, "id", "F28R-031", where))
        direction = str(_req(ctl, "direction", "F28R-031", where))
        if direction not in DIRECTIONS:
            fail("F28R-031", f"{where}: direction `{direction}` is not one of {DIRECTIONS}")
        expected = str(_req(ctl, "expected", "F28R-032", where))
        actual = str(_req(ctl, "actual", "F28R-032", where))
        for value, name in ((expected, "expected"), (actual, "actual")):
            if value not in DIRECTIONAL_OUTCOMES:
                fail("F28R-032", f"{where}: {name} `{value}` is not one of {DIRECTIONAL_OUTCOMES}")
        _req(ctl, "evidence", "F28R-033", where)

        if not isinstance(ctl.get("passed"), bool):
            fail("F28R-033", f"{where}: `passed` must be a boolean")

        # `passed` is DERIVED here rather than trusted: a run that records
        # `passed: true` beside a mismatched expected/actual is asserting its own
        # conclusion, which is the shape this validator exists to reject.
        if (expected == actual) != ctl["passed"]:
            fail(
                "F28R-034",
                f"{where}: `passed` is {ctl['passed']} but expected=`{expected}` and "
                f"actual=`{actual}`; the control's verdict must follow from its own "
                f"measurement",
            )

        if direction == "negative" and actual in ("observable", "activeness-present"):
            fail(
                "F28R-035",
                f"{where}: negative control `{cid}` reports `{actual}`; the probe reports "
                f"availability unconditionally, so every green it has produced is "
                f"worthless. This is a CRITICAL finding, not a control failure to note",
            )
        if direction == "positive" and actual in ("unobservable", "activeness-absent"):
            fail(
                "F28R-036",
                f"{where}: positive control `{cid}` reports `{actual}` where the channel "
                f"should be sound; the control is measuring something other than what it "
                f"claims and the run is not interpretable",
            )
        by_direction[direction] += 1

    for direction in DIRECTIONS:
        if by_direction[direction] == 0:
            fail(
                "F28R-037",
                f"controls: no {direction} directional control is present; a control "
                f"without both directions cannot distinguish a broken channel from a "
                f"broken product",
            )
    return dict(by_direction)


# ---------------------------------------------------------------------------------
# matrix.tsv
# ---------------------------------------------------------------------------------


def read_matrix(path: Path) -> list[dict]:
    rows: list[dict] = []
    text = path.read_text(encoding="utf-8")
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) != 9:
            fail("F28R-040", f"matrix row has {len(parts)} columns, expected 9: {line[:80]}")
        rows.append(
            {
                "cell_id": parts[0],
                "dimension": parts[1],
                "os": parts[2],
                "surface": parts[3],
                "criticality": parts[4],
                "applicability": parts[5],
                "skip_class": parts[6],
                "skip_evidence": parts[7],
                "activeness": parts[8],
            }
        )
    if not rows:
        fail("F28R-041", f"{path} declares no cells; a matrix that certifies nothing is a red")
    return rows


# ---------------------------------------------------------------------------------
# --check-cell-coverage
# ---------------------------------------------------------------------------------

_PROBE_BLOCK_RE = re.compile(r"ProbeSpec\s*\{(.*?)\n    \}", re.DOTALL)
_FIELD_RE = re.compile(r"(\w+)\s*:\s*(.+?),?\s*$", re.MULTILINE)


def parse_probe_specs(source: str) -> list[dict]:
    """Parse the canonical `PROBES` table straight out of `e5_cases.rs`.

    Parsed from the Rust CONST, not from a parallel comment channel: a comment that
    describes the table is a second source of truth and the one that goes stale.
    """
    specs: list[dict] = []
    for block in _PROBE_BLOCK_RE.findall(source):
        spec: dict = {}
        idm = re.search(r'id:\s*"([^"]+)"', block)
        if not idm:
            continue
        spec["id"] = idm.group(1)
        dim = re.search(r"dimension:\s*Dimension::(\w+)", block)
        if not dim:
            fail("F28R-050", f"probe `{spec['id']}` declares no dimension")
        spec["dimension"] = _rust_dimension_id(dim.group(1))
        fams = re.search(r"families:\s*&\[([^\]]*)\]", block)
        if not fams:
            fail("F28R-050", f"probe `{spec['id']}` declares no families")
        spec["families"] = [
            m.lower() for m in re.findall(r"Platform::(\w+)", fams.group(1))
        ]
        cell = re.search(r'cell_id:\s*Some\("([^"]+)"\)', block)
        spec["cell_id"] = cell.group(1) if cell else None
        spec["harness_bound"] = "Harness::HarnessBound" in block
        spec["emits_activeness"] = bool(re.search(r"emits_activeness:\s*true", block))
        specs.append(spec)
    if not specs:
        fail("F28R-051", "no ProbeSpec entries were parsed; the probe table is absent or reshaped")
    return specs


def _rust_dimension_id(variant: str) -> str:
    mapping = {
        "SandboxProbes": "sandbox-probes",
        "Unicode": "unicode",
        "LongPaths": "long-paths",
        "UncReparseSymlink": "unc-reparse-symlink",
        "ProcessCleanup": "process-cleanup",
        "SuspendResume": "suspend-resume",
        "Offline": "offline",
        "DiskFullReadOnly": "disk-full-read-only",
        "HostileInputs": "hostile-inputs",
    }
    if variant not in mapping:
        fail("F28R-052", f"unknown Dimension variant `{variant}`")
    return mapping[variant]


def check_cell_coverage(matrix_path: Path, cases_path: Path) -> dict:
    rows = read_matrix(matrix_path)
    specs = parse_probe_specs(cases_path.read_text(encoding="utf-8"))

    cell_specific = {s["cell_id"]: s for s in specs if s["cell_id"]}
    dimension_specs = [s for s in specs if not s["cell_id"]]

    uncovered: list[str] = []
    multiply_covered: list[str] = []
    for row in rows:
        if row["cell_id"] in cell_specific:
            continue
        claiming = [
            s
            for s in dimension_specs
            if s["dimension"] == row["dimension"] and row["os"] in s["families"]
        ]
        if len(claiming) == 0:
            uncovered.append(row["cell_id"])
        elif len(claiming) > 1:
            multiply_covered.append(row["cell_id"])

    if uncovered:
        fail(
            "F28R-053",
            f"{len(uncovered)} matrix cells have no probe; the first are "
            f"{uncovered[:5]}. A cell with no probe fails the suite rather than being "
            f"reported absent",
        )
    if multiply_covered:
        fail(
            "F28R-054",
            f"{len(multiply_covered)} matrix cells are claimed by more than one probe; "
            f"the first are {multiply_covered[:5]}",
        )

    declared = {s["dimension"] for s in specs}
    missing_dimensions = sorted(set(DIMENSIONS) - declared)
    if missing_dimensions:
        fail(
            "F28R-055",
            f"the probe table omits dimensions {missing_dimensions}; the nine F28-01 "
            f"dimensions are fixed and may not be added to, merged or renamed",
        )

    # Sandbox cells demand an activeness observation, so a sandbox probe that does not
    # emit one cannot cover them.  Checked here rather than at run time, because a
    # probe discovered incapable after the hardware is spent has already cost the run.
    for spec in specs:
        if spec["dimension"] == SANDBOX_DIMENSION and not spec["emits_activeness"]:
            fail(
                "F28R-056",
                f"sandbox probe `{spec['id']}` does not emit an activeness observation; a "
                f"green it produced could not be distinguished from a silently disabled "
                f"sandbox",
            )

    # A harness-bound probe that is the ONLY coverage of a CRITICAL cell on a family
    # where no harness can be built narrows coverage silently.  Fail instead.
    unbuildable = {"macos"}
    for row in rows:
        if row["criticality"] != "critical":
            continue
        spec = cell_specific.get(row["cell_id"])
        if spec is None:
            spec = next(
                s
                for s in dimension_specs
                if s["dimension"] == row["dimension"] and row["os"] in s["families"]
            )
        if spec["harness_bound"] and row["os"] in unbuildable:
            fail(
                "F28R-057",
                f"critical cell `{row['cell_id']}` is covered only by harness-bound probe "
                f"`{spec['id']}` on `{row['os']}`, where no cargo harness can be built; "
                f"that is a silent narrowing of coverage and fails the suite",
            )

    return {"cells": len(rows), "probes": len(specs), "dimensions": len(declared)}


# ---------------------------------------------------------------------------------
# results.json
# ---------------------------------------------------------------------------------


def _runs_index(doc: dict) -> dict[str, dict]:
    runs = doc.get("runs")
    if not isinstance(runs, list) or not runs:
        fail("F28R-060", "results: `runs` is absent or empty")
    index: dict[str, dict] = {}
    for i, run in enumerate(runs):
        rid = str(_req(run, "run_id", "F28R-060", f"results.runs[{i}]"))
        if rid in index:
            fail("F28R-060", f"results: duplicate run_id `{rid}`")
        index[rid] = run
    return index


def verify_results(doc: dict) -> dict:
    if doc.get("schema") != "f28-results/1":
        fail("F28R-061", f"results: schema is `{doc.get('schema')}`, expected `f28-results/1`")

    candidate = doc.get("candidate")
    if not isinstance(candidate, dict):
        fail("F28R-062", "results: `candidate` is absent")
    commit = str(_req(candidate, "commit", "F28R-062", "results.candidate"))
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        fail(
            "F28R-062",
            f"results.candidate.commit `{commit}` is not a full 40-hex sha; an "
            f"abbreviated sha or a ref names a moving target",
        )
    tree = str(_req(candidate, "tree", "F28R-062", "results.candidate"))
    if not re.fullmatch(r"[0-9a-f]{40}", tree):
        fail("F28R-062", f"results.candidate.tree `{tree}` is not a full 40-hex sha")

    runs = _runs_index(doc)
    cells = doc.get("cells")
    if not isinstance(cells, list) or not cells:
        fail("F28R-063", "results: `cells` is absent or empty")

    seen: set[str] = set()
    tally = {"pass": 0, "red": 0, "skip": 0}
    for i, cell in enumerate(cells):
        where = f"results.cells[{i}]"
        cid = str(_req(cell, "cell_id", "F28R-064", where))
        if cid in seen:
            fail("F28R-064", f"{where}: duplicate cell_id `{cid}`")
        seen.add(cid)

        outcome = str(_req(cell, "outcome", "F28R-065", where))
        if outcome not in OUTCOMES:
            fail("F28R-065", f"{where} `{cid}`: outcome `{outcome}` is not one of {OUTCOMES}")
        tally[outcome] += 1

        dimension = str(_req(cell, "dimension", "F28R-066", where))
        if dimension not in DIMENSIONS:
            fail("F28R-066", f"{where} `{cid}`: dimension `{dimension}` is not an F28-01 dimension")

        criticality = str(_req(cell, "criticality", "F28R-066", where))
        if criticality not in ("critical", "standard"):
            fail("F28R-066", f"{where} `{cid}`: criticality `{criticality}` is not recognised")

        rid = str(_req(cell, "run_id", "F28R-067", where))
        if rid not in runs:
            fail("F28R-067", f"{where} `{cid}`: run_id `{rid}` names no recorded run")

        # A live row is specified by three things and is not accepted without all
        # three: the exact invocation, an observable outcome, and the named platform.
        _req(cell, "invocation", "F28R-068", where)
        _req(cell, "observable", "F28R-068", where)
        os_family = str(_req(cell, "os", "F28R-068", where))
        if os_family != runs[rid].get("os_family"):
            fail(
                "F28R-068",
                f"{where} `{cid}`: os `{os_family}` does not match the os_family of run "
                f"`{rid}` (`{runs[rid].get('os_family')}`)",
            )

        if not isinstance(cell.get("recorded_at"), int):
            fail("F28R-069", f"{where} `{cid}`: `recorded_at` must be an integer sequence")

        if outcome == "red":
            check_attribution(cell, where, cid)
        if outcome == "skip":
            check_skip(cell, where, cid, criticality)

    return {"cells": len(cells), **tally, "runs": len(runs)}


def check_attribution(cell: dict, where: str, cid: str) -> None:
    attribution = cell.get("attribution")
    if not isinstance(attribution, dict):
        fail(
            "F28R-070",
            f"{where} `{cid}`: a red carries no attribution; every red is attributed to a "
            f"carried-red ledger entry or filed as a new finding",
        )
    kind = str(_req(attribution, "kind", "F28R-070", f"{where}.attribution"))
    if kind == "carried":
        ledger_id = str(_req(attribution, "known_red_id", "F28R-071", f"{where}.attribution"))
        if not re.fullmatch(r"KR-\d+", ledger_id):
            fail("F28R-071", f"{where} `{cid}`: `{ledger_id}` is not a carried-red ledger id")
    elif kind == "new-finding":
        finding = attribution.get("finding")
        if not isinstance(finding, dict):
            fail("F28R-072", f"{where} `{cid}`: a new finding carries no record")
        _req(finding, "id", "F28R-072", f"{where}.attribution.finding")
        _req(finding, "subject", "F28R-072", f"{where}.attribution.finding")
        severity = str(_req(finding, "p28_severity", "F28R-073", f"{where}.attribution.finding"))
        if severity not in ("CRITICAL", "HIGH", "MEDIUM", "LOW"):
            fail("F28R-073", f"{where} `{cid}`: p28_severity `{severity}` is not a band")
        # A1: the inherited severity is provenance only.  A finding carrying ONLY an
        # inherited severity is the laundering channel the amendment closes.
        if "inherited_severity" not in finding:
            fail(
                "F28R-074",
                f"{where} `{cid}`: the finding records no `inherited_severity`; it is "
                f"provenance and must be present even when it is `-`",
            )
    else:
        fail("F28R-070", f"{where} `{cid}`: attribution kind `{kind}` is not recognised")


def check_skip(cell: dict, where: str, cid: str, criticality: str) -> None:
    if criticality == "critical":
        fail(
            "F28R-080",
            f"{where} `{cid}`: a CRITICAL cell carries a skip. A critical cell has no legal "
            f"skip under any class, and a critical cell that cannot be run is a RED",
        )
    skip_class = str(_req(cell, "skip_class", "F28R-081", where))
    if skip_class not in SKIP_CLASSES:
        fail(
            "F28R-081",
            f"{where} `{cid}`: skip_class `{skip_class}` is not one of the four contract "
            f"classes {sorted(SKIP_CLASSES)}; no fifth class may be added mid-run",
        )
    evidence = cell.get("skip_evidence")
    if not isinstance(evidence, dict):
        fail("F28R-082", f"{where} `{cid}`: skip carries no evidence object")
    for field in SKIP_CLASSES[skip_class]:
        _req(evidence, field, "F28R-082", f"{where}.skip_evidence ({skip_class})")
    if skip_class == "observation-blocked":
        check_control_ref(str(evidence["control_ref"]), f"{where} `{cid}`")


# ---------------------------------------------------------------------------------
# --check-control-precedence
# ---------------------------------------------------------------------------------


def check_control_precedence(results: dict, controls: dict) -> dict:
    ref = str(_req(controls, "control_ref", "F28R-090", "controls"))
    recorded_at = controls.get("recorded_at")
    if not isinstance(recorded_at, int):
        fail("F28R-090", "controls: `recorded_at` must be an integer sequence")

    checked = 0
    for i, cell in enumerate(results.get("cells", [])):
        if cell.get("dimension") != SANDBOX_DIMENSION:
            continue
        where = f"results.cells[{i}] `{cell.get('cell_id')}`"
        cell_ref = cell.get("control_ref")
        if cell_ref != ref:
            fail(
                "F28R-091",
                f"{where}: a sandbox verdict cites control_ref `{cell_ref}`, which is not "
                f"the control measured for this run (`{ref}`). No sandbox result may rest "
                f"on a control this run did not measure",
            )
        if not isinstance(cell.get("recorded_at"), int):
            fail("F28R-092", f"{where}: `recorded_at` must be an integer sequence")
        if cell["recorded_at"] <= recorded_at:
            fail(
                "F28R-092",
                f"{where}: recorded_at={cell['recorded_at']} does not follow the control "
                f"(recorded_at={recorded_at}); the control completes and its verdict is "
                f"recorded BEFORE any sandbox cell is graded",
            )
        checked += 1

    if checked == 0:
        fail(
            "F28R-093",
            "results: no sandbox-dimension cell was checked for control precedence. A "
            "precedence gate over an empty set passes without proving anything",
        )
    return {"sandbox_cells_checked": checked}


# ---------------------------------------------------------------------------------
# --check-activeness
# ---------------------------------------------------------------------------------


def check_activeness(results: dict) -> dict:
    greens = 0
    for i, cell in enumerate(results.get("cells", [])):
        if cell.get("dimension") != SANDBOX_DIMENSION or cell.get("outcome") != "pass":
            continue
        where = f"results.cells[{i}] `{cell.get('cell_id')}`"
        activeness = cell.get("activeness")
        if not isinstance(activeness, dict):
            fail(
                "F28R-100",
                f"{where}: a sandbox green carries no activeness observation. Absence of an "
                f"observed violation is not evidence of a sandbox; this cell is a RED",
            )
        if activeness.get("observed") is not True:
            fail(
                "F28R-100",
                f"{where}: activeness.observed is not true. `NotMeasured` on a sandbox cell "
                f"is a RED, never a green and never a skip",
            )
        _req(activeness, "probe", "F28R-101", f"{where}.activeness")
        _req(activeness, "detail", "F28R-101", f"{where}.activeness")
        greens += 1

    # A measurement that cannot be taken must never render as 0: when there are no
    # sandbox greens at all, say so rather than reporting a checked count of zero as
    # if the rule had been exercised.
    total_sandbox = sum(
        1 for c in results.get("cells", []) if c.get("dimension") == SANDBOX_DIMENSION
    )
    if total_sandbox == 0:
        fail(
            "F28R-102",
            "results: the result set contains no sandbox-dimension cell at all, so the "
            "activeness rule was never exercised. That is not a pass",
        )
    return {"sandbox_greens_checked": greens, "sandbox_cells": total_sandbox}


# ---------------------------------------------------------------------------------
# --check-skips
# ---------------------------------------------------------------------------------


def check_skips(results: dict, matrix_path: Path) -> dict:
    rows = {r["cell_id"]: r for r in read_matrix(matrix_path)}
    skips = 0
    for i, cell in enumerate(results.get("cells", [])):
        if cell.get("outcome") != "skip":
            continue
        where = f"results.cells[{i}] `{cell.get('cell_id')}`"
        row = rows.get(str(cell.get("cell_id")))
        if row is None:
            fail(
                "F28R-110",
                f"{where}: the skipped cell is not in the generated matrix; a result for a "
                f"cell the generator never emitted is a foreign row",
            )
        # Criticality is read from the GENERATED matrix, never from the result row.
        # Reading it from the result is how a critical case becomes non-critical the
        # moment it is inconvenient.
        check_skip(cell, where, str(cell["cell_id"]), row["criticality"])
        skips += 1
    return {"skips_checked": skips}


# ---------------------------------------------------------------------------------
# --check-binary-binding
# ---------------------------------------------------------------------------------


def check_binary_binding(results: dict, candidate: dict) -> dict:
    targets = {t.get("target"): t for t in candidate.get("targets", [])}
    if not targets:
        fail("F28R-120", "candidate: no targets are recorded")

    checked = 0
    unbound: list[str] = []
    for rid, run in _runs_index(results).items():
        where = f"results.runs `{rid}`"
        binary = run.get("binary")
        if not isinstance(binary, dict):
            fail("F28R-121", f"{where}: no binary is recorded; a run names the binary it drove")
        digest = str(_req(binary, "sha256", "F28R-121", where))
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            fail("F28R-121", f"{where}: sha256 `{digest}` is not a 64-hex digest")
        target = str(_req(binary, "target", "F28R-122", where))
        if target not in targets:
            fail(
                "F28R-122",
                f"{where}: target `{target}` is not one of the candidate ledger's targets "
                f"{sorted(t for t in targets if t)}",
            )
        ledger_digest = targets[target].get("digest")
        bound = binary.get("digest_bound_to_ledger")
        if not isinstance(bound, bool):
            fail("F28R-123", f"{where}: `digest_bound_to_ledger` must be a boolean")
        if bound:
            if ledger_digest is None:
                fail(
                    "F28R-124",
                    f"{where}: claims a ledger-bound digest, but the candidate ledger records "
                    f"target `{target}` as unbindable; there is nothing to bind to",
                )
            if str(ledger_digest).lower() != digest:
                fail(
                    "F28R-125",
                    f"{where}: binary digest {digest} does not match the candidate ledger's "
                    f"{ledger_digest} for `{target}`. A family whose binary does not match is "
                    f"not a family that ran the candidate, and its results are void",
                )
        else:
            _req(
                binary,
                "unbound_reason",
                "F28R-126",
                f"{where}: an unbound binary carries a measured reason",
            )
            unbound.append(target)
        checked += 1

    if checked == 0:
        fail("F28R-127", "results: no run was checked for binary binding")
    return {"runs_checked": checked, "unbound_targets": unbound}


# ---------------------------------------------------------------------------------
# --self-test
# ---------------------------------------------------------------------------------


def _good_controls() -> dict:
    obs = []
    for session in SESSION_TYPES:
        for lease in LEASE_STATES:
            unavailable = lease == "wedged"
            obs.append(
                {
                    "id": f"obs-{session}-{lease}",
                    "session_type": session,
                    "lease_state": lease,
                    "probe_report": "unavailable" if unavailable else "available",
                    "probe_evidence": "raw product stderr captured in win-control.log",
                    "product_behaviour": "refused-fail-closed"
                    if unavailable
                    else "executed-sandboxed",
                    "product_evidence": "sentinel file absent / whoami token capture",
                }
            )
    return {
        "schema": "f28-controls/1",
        "host": "seandesktop",
        "run_id": "run-1",
        "recorded_at": 10,
        "quiet_run": True,
        "verdict": "wedge-clearable",
        "observation_blocked_authorised": False,
        "authorised_cells": [],
        "control_ref": "control:appc-observe@seandesktop:run-1",
        "observations": obs,
        "directional_controls": [
            {
                "id": "pos-clean-lease",
                "direction": "positive",
                "expected": "observable",
                "actual": "observable",
                "passed": True,
                "evidence": "clean lease dir; probe reported available",
            },
            {
                "id": "neg-wedged-lease",
                "direction": "negative",
                "expected": "unobservable",
                "actual": "unobservable",
                "passed": True,
                "evidence": "wedged lease; probe reported unavailable",
            },
            {
                "id": "neg-activeness-detector",
                "direction": "negative",
                "expected": "activeness-absent",
                "actual": "activeness-absent",
                "passed": True,
                "evidence": "bare cmd.exe outside the product shows no AppContainer SID",
            },
        ],
    }


def _good_results() -> dict:
    return {
        "schema": "f28-results/1",
        "candidate": {"commit": "a" * 40, "tree": "b" * 40},
        "runs": [
            {
                "run_id": "win-1",
                "host": "seandesktop",
                "os_family": "windows",
                "quiet": True,
                "binary": {
                    "path": r"C:\x\wayland-core.exe",
                    "sha256": "c" * 64,
                    "target": "x86_64-pc-windows-msvc",
                    "digest_bound_to_ledger": False,
                    "unbound_reason": "CI release matrix had produced no artifact at this commit",
                },
            }
        ],
        "cells": [
            {
                "cell_id": "sandbox-probes-windows-acp",
                "dimension": "sandbox-probes",
                "os": "windows",
                "criticality": "critical",
                "outcome": "pass",
                "run_id": "win-1",
                "invocation": "wayland-core acp --help",
                "observable": "exit 0 and an AppContainer SID in the child token",
                "recorded_at": 20,
                "control_ref": "control:appc-observe@seandesktop:run-1",
                "activeness": {
                    "observed": True,
                    "probe": "child-token-sid",
                    "detail": "S-1-15-2-... plus Mandatory Label\\Low",
                },
            },
            {
                "cell_id": "unicode-windows-acp",
                "dimension": "unicode",
                "os": "windows",
                "criticality": "standard",
                "outcome": "skip",
                "run_id": "win-1",
                "invocation": "wayland-core acp --help",
                "observable": "not run",
                "recorded_at": 21,
                "skip_class": "platform-inapplicability",
                "skip_evidence": {"fact": "n/a", "observable": "n/a"},
            },
            {
                "cell_id": "process-cleanup-windows-acp",
                "dimension": "process-cleanup",
                "os": "windows",
                "criticality": "critical",
                "outcome": "red",
                "run_id": "win-1",
                "invocation": "wayland-core acp --help",
                "observable": "a descendant survived",
                "recorded_at": 22,
                "attribution": {"kind": "carried", "known_red_id": "KR-01"},
            },
        ],
    }


def _good_candidate() -> dict:
    return {
        "targets": [
            {"target": "x86_64-pc-windows-msvc", "digest": None, "status": "unbindable"},
            {"target": "aarch64-apple-darwin", "digest": "d" * 64, "status": "bound"},
        ]
    }


_MATRIX_FIXTURE = (
    "# fixture\n"
    "#cell_id\tdimension\tos\tsurface\tcriticality\tapplicability\tskip_class\t"
    "skip_evidence\tactiveness\n"
    "sandbox-probes-windows-acp\tsandbox-probes\twindows\tcmd:acp\tcritical\tapplicable\t-\t-\t"
    "required\n"
    "unicode-windows-acp\tunicode\twindows\tcmd:acp\tstandard\tapplicable\t-\t-\tn/a\n"
    "process-cleanup-windows-acp\tprocess-cleanup\twindows\tcmd:acp\tcritical\tapplicable\t-\t-\t"
    "n/a\n"
)

_RUST_VARIANTS = {
    "sandbox-probes": "SandboxProbes",
    "unicode": "Unicode",
    "long-paths": "LongPaths",
    "unc-reparse-symlink": "UncReparseSymlink",
    "process-cleanup": "ProcessCleanup",
    "suspend-resume": "SuspendResume",
    "offline": "Offline",
    "disk-full-read-only": "DiskFullReadOnly",
    "hostile-inputs": "HostileInputs",
}


def _probe_fixture(
    dimension: str,
    *,
    probe_id: str | None = None,
    families: tuple[str, ...] = ("Linux", "MacOs", "Windows"),
    harness: str = "Harness::BlackBox",
    activeness: bool | None = None,
) -> str:
    """One `ProbeSpec` literal, shaped exactly as `e5_cases.rs` writes them.

    The self-test's fixtures are generated rather than pasted so that a fixture
    cannot silently stop resembling the real table it stands in for.
    """
    if activeness is None:
        activeness = dimension == SANDBOX_DIMENSION
    fams = ", ".join(f"Platform::{f}" for f in families)
    return (
        "    ProbeSpec {\n"
        f'        id: "{probe_id or dimension}",\n'
        f"        dimension: Dimension::{_RUST_VARIANTS[dimension]},\n"
        f"        families: &[{fams}],\n"
        "        cell_id: None,\n"
        f"        harness: {harness},\n"
        f"        emits_activeness: {'true' if activeness else 'false'},\n"
        "    }"
    )


def _cases_fixture(specs: list[str]) -> str:
    body = ",\n".join(specs)
    return f"pub const PROBES: [ProbeSpec; {len(specs)}] = [\n{body},\n];\n"


_CASES_FIXTURE = _cases_fixture([_probe_fixture(d) for d in DIMENSIONS])


def _mutate(doc: dict, path: str, value: object) -> dict:
    import copy

    out = copy.deepcopy(doc)
    node = out
    parts = path.split(".")
    for part in parts[:-1]:
        if part.isdigit():
            node = node[int(part)]
        else:
            node = node[part]
    last = parts[-1]
    if value is _DELETE:
        if last.isdigit():
            del node[int(last)]
        else:
            node.pop(last, None)
    elif last.isdigit():
        node[int(last)] = value
    else:
        node[last] = value
    return out


class _Delete:
    pass


_DELETE = _Delete()


def self_test(tmp: Path) -> int:
    """Every rejection code tripped by a bad fixture; every good fixture accepted."""
    passed = 0
    failed: list[str] = []

    def expect_ok(name: str, fn) -> None:
        nonlocal passed
        try:
            fn()
            passed += 1
        except Fail as exc:
            failed.append(f"GOOD FIXTURE REJECTED [{name}]: {exc}")

    def expect_code(name: str, code: str, fn) -> None:
        nonlocal passed
        try:
            fn()
        except Fail as exc:
            if exc.code == code:
                passed += 1
            else:
                failed.append(f"[{name}] expected {code}, got {exc.code}: {exc}")
            return
        failed.append(f"[{name}] expected {code}, but the fixture was ACCEPTED")

    good_c = _good_controls()
    good_r = _good_results()
    good_cand = _good_candidate()

    matrix = tmp / "matrix.tsv"
    matrix.write_text(_MATRIX_FIXTURE, encoding="utf-8")
    cases = tmp / "e5_cases.rs"
    cases.write_text(_CASES_FIXTURE, encoding="utf-8")

    # ---- controls, both directions -------------------------------------------------
    expect_ok("controls/good", lambda: check_controls(good_c))
    expect_code(
        "controls/schema", "F28R-010", lambda: check_controls(_mutate(good_c, "schema", "nope"))
    )
    expect_code(
        "controls/verdict",
        "F28R-012",
        lambda: check_controls(_mutate(good_c, "verdict", "probably fine")),
    )
    expect_code(
        "controls/ref-is-a-document",
        "F28R-004",
        lambda: check_controls(
            _mutate(good_c, "control_ref", ".planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md")
        ),
    )
    expect_code(
        "controls/ref-favourable-document",
        "F28R-004",
        lambda: check_controls(
            _mutate(
                good_c,
                "control_ref",
                ".planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md",
            )
        ),
    )
    expect_code(
        "controls/ref-not-a-control",
        "F28R-005",
        lambda: check_controls(_mutate(good_c, "control_ref", "the channel was clearly broken")),
    )
    expect_code(
        "controls/ref-unbound-host",
        "F28R-013",
        lambda: check_controls(
            _mutate(good_c, "control_ref", "control:appc-observe@otherbox:run-1")
        ),
    )
    expect_code(
        "controls/not-quiet", "F28R-014", lambda: check_controls(_mutate(good_c, "quiet_run", False))
    )
    expect_code(
        "controls/five-observations",
        "F28R-021",
        lambda: check_controls(_mutate(good_c, "observations.5", _DELETE)),
    )
    # Six rows that are not the cross product: a COUNT of six proves nothing.
    six_but_not_crossed = _mutate(good_c, "observations.5.lease_state", "as-found")
    expect_code(
        "controls/six-but-not-crossed",
        "F28R-017",
        lambda: check_controls(six_but_not_crossed),
    )
    expect_code(
        "controls/behaviour-vocabulary",
        "F28R-019",
        lambda: check_controls(_mutate(good_c, "observations.0.product_behaviour", "fine")),
    )
    expect_code(
        "controls/behaviour-missing",
        "F28R-019",
        lambda: check_controls(_mutate(good_c, "observations.0.product_behaviour", _DELETE)),
    )
    expect_code(
        "controls/impossible-combination",
        "F28R-020",
        lambda: check_controls(
            _mutate(good_c, "observations.0.product_behaviour", "proceeded-unsandboxed")
        ),
    )
    expect_code(
        "controls/sound-but-class-authorised",
        "F28R-023",
        lambda: check_controls(_mutate(good_c, "observation_blocked_authorised", True)),
    )
    authorised_no_cells = _mutate(good_c, "verdict", "channel-broken-and-unclearable")
    authorised_no_cells = _mutate(authorised_no_cells, "observation_blocked_authorised", True)
    expect_code(
        "controls/authorised-names-no-cells",
        "F28R-024",
        lambda: check_controls(authorised_no_cells),
    )

    # ---- directional controls ------------------------------------------------------
    expect_ok("directions/good", lambda: check_control_directions(good_c))
    expect_code(
        "directions/absent",
        "F28R-030",
        lambda: check_control_directions(_mutate(good_c, "directional_controls", [])),
    )
    expect_code(
        "directions/passed-does-not-follow",
        "F28R-034",
        lambda: check_control_directions(
            _mutate(good_c, "directional_controls.1.actual", "observable")
        ),
    )
    neg_reports_observable = _mutate(good_c, "directional_controls.1.actual", "observable")
    neg_reports_observable = _mutate(neg_reports_observable, "directional_controls.1.passed", False)
    expect_code(
        "directions/negative-reports-observable",
        "F28R-035",
        lambda: check_control_directions(neg_reports_observable),
    )
    pos_reports_unobservable = _mutate(good_c, "directional_controls.0.actual", "unobservable")
    pos_reports_unobservable = _mutate(
        pos_reports_unobservable, "directional_controls.0.passed", False
    )
    expect_code(
        "directions/positive-reports-unobservable",
        "F28R-036",
        lambda: check_control_directions(pos_reports_unobservable),
    )
    only_positive = _mutate(good_c, "directional_controls.2", _DELETE)
    only_positive = _mutate(only_positive, "directional_controls.1", _DELETE)
    expect_code(
        "directions/no-negative", "F28R-037", lambda: check_control_directions(only_positive)
    )

    # ---- results -------------------------------------------------------------------
    expect_ok("results/good", lambda: verify_results(good_r))
    expect_code(
        "results/abbreviated-sha",
        "F28R-062",
        lambda: verify_results(_mutate(good_r, "candidate.commit", "a1b2c3d")),
    )
    expect_code(
        "results/unknown-outcome",
        "F28R-065",
        lambda: verify_results(_mutate(good_r, "cells.0.outcome", "probably-ok")),
    )
    expect_code(
        "results/no-invocation",
        "F28R-068",
        lambda: verify_results(_mutate(good_r, "cells.0.invocation", _DELETE)),
    )
    expect_code(
        "results/os-mismatch",
        "F28R-068",
        lambda: verify_results(_mutate(good_r, "cells.0.os", "linux")),
    )
    expect_code(
        "results/foreign-run",
        "F28R-067",
        lambda: verify_results(_mutate(good_r, "cells.0.run_id", "mac-9")),
    )
    expect_code(
        "results/unattributed-red",
        "F28R-070",
        lambda: verify_results(_mutate(good_r, "cells.2.attribution", _DELETE)),
    )
    expect_code(
        "results/new-finding-without-rescore",
        "F28R-073",
        lambda: verify_results(
            _mutate(
                good_r,
                "cells.2.attribution",
                {
                    "kind": "new-finding",
                    "finding": {
                        "id": "F-28-02-9",
                        "subject": "x",
                        "p28_severity": "whatever",
                        "inherited_severity": "-",
                    },
                },
            )
        ),
    )
    expect_code(
        "results/new-finding-without-provenance",
        "F28R-074",
        lambda: verify_results(
            _mutate(
                good_r,
                "cells.2.attribution",
                {
                    "kind": "new-finding",
                    "finding": {"id": "F-28-02-9", "subject": "x", "p28_severity": "HIGH"},
                },
            )
        ),
    )
    expect_code(
        "results/critical-cell-skipped",
        "F28R-080",
        lambda: verify_results(
            _mutate(
                _mutate(
                    _mutate(good_r, "cells.2.outcome", "skip"),
                    "cells.2.skip_class",
                    "observation-blocked",
                ),
                "cells.2.skip_evidence",
                {"control_ref": "control:x@y:z"},
            )
        ),
    )
    fifth_class = _mutate(good_r, "cells.1.skip_class", "harness-bound")
    expect_code("results/fifth-skip-class", "F28R-081", lambda: verify_results(fifth_class))
    expect_code(
        "results/skip-without-evidence",
        "F28R-082",
        lambda: verify_results(_mutate(good_r, "cells.1.skip_evidence", {"fact": "x"})),
    )
    ob_skip = _mutate(good_r, "cells.1.skip_class", "observation-blocked")
    ob_doc = _mutate(
        ob_skip,
        "cells.1.skip_evidence",
        {"control_ref": ".planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md"},
    )
    expect_code("results/ob-skip-cites-document", "F28R-004", lambda: verify_results(ob_doc))
    ob_ok = _mutate(
        ob_skip, "cells.1.skip_evidence", {"control_ref": "control:appc@seandesktop:run-1"}
    )
    expect_ok("results/ob-skip-with-control", lambda: verify_results(ob_ok))

    # ---- control precedence --------------------------------------------------------
    expect_ok("precedence/good", lambda: check_control_precedence(good_r, good_c))
    expect_code(
        "precedence/cell-predates-control",
        "F28R-092",
        lambda: check_control_precedence(_mutate(good_r, "cells.0.recorded_at", 5), good_c),
    )
    expect_code(
        "precedence/foreign-control",
        "F28R-091",
        lambda: check_control_precedence(
            _mutate(good_r, "cells.0.control_ref", "control:other@host:run-2"), good_c
        ),
    )
    expect_code(
        "precedence/no-control-ref",
        "F28R-091",
        lambda: check_control_precedence(_mutate(good_r, "cells.0.control_ref", _DELETE), good_c),
    )
    no_sandbox = _mutate(good_r, "cells.0", _DELETE)
    expect_code(
        "precedence/vacuous-over-empty-set",
        "F28R-093",
        lambda: check_control_precedence(no_sandbox, good_c),
    )

    # ---- activeness ----------------------------------------------------------------
    expect_ok("activeness/good", lambda: check_activeness(good_r))
    expect_code(
        "activeness/green-without-observation",
        "F28R-100",
        lambda: check_activeness(_mutate(good_r, "cells.0.activeness", _DELETE)),
    )
    expect_code(
        "activeness/not-measured-is-a-red",
        "F28R-100",
        lambda: check_activeness(
            _mutate(good_r, "cells.0.activeness", {"observed": False, "reason": "probe unavailable"})
        ),
    )
    expect_code(
        "activeness/empty-detail",
        "F28R-101",
        lambda: check_activeness(
            _mutate(good_r, "cells.0.activeness", {"observed": True, "probe": "p", "detail": "  "})
        ),
    )
    expect_code(
        "activeness/no-sandbox-cell-at-all",
        "F28R-102",
        lambda: check_activeness(no_sandbox),
    )

    # ---- skips against the generated matrix ----------------------------------------
    expect_ok("skips/good", lambda: check_skips(good_r, matrix))
    expect_code(
        "skips/foreign-cell",
        "F28R-110",
        lambda: check_skips(_mutate(good_r, "cells.1.cell_id", "unicode-windows-nosuch"), matrix),
    )
    # Criticality is read from the MATRIX, so relabelling the result row cannot make a
    # critical cell skippable.
    critical_in_matrix = _mutate(good_r, "cells.2.outcome", "skip")
    critical_in_matrix = _mutate(critical_in_matrix, "cells.2.criticality", "standard")
    critical_in_matrix = _mutate(
        critical_in_matrix, "cells.2.skip_class", "platform-inapplicability"
    )
    critical_in_matrix = _mutate(
        critical_in_matrix, "cells.2.skip_evidence", {"fact": "f", "observable": "o"}
    )
    expect_code(
        "skips/relabelled-criticality",
        "F28R-080",
        lambda: check_skips(critical_in_matrix, matrix),
    )

    # ---- binary binding ------------------------------------------------------------
    expect_ok("binding/good", lambda: check_binary_binding(good_r, good_cand))
    expect_code(
        "binding/unbound-without-reason",
        "F28R-126",
        lambda: check_binary_binding(
            _mutate(good_r, "runs.0.binary.unbound_reason", _DELETE), good_cand
        ),
    )
    expect_code(
        "binding/unknown-target",
        "F28R-122",
        lambda: check_binary_binding(
            _mutate(good_r, "runs.0.binary.target", "sparc-unknown-none"), good_cand
        ),
    )
    claims_bound = _mutate(good_r, "runs.0.binary.digest_bound_to_ledger", True)
    expect_code(
        "binding/claims-bound-to-unbindable",
        "F28R-124",
        lambda: check_binary_binding(claims_bound, good_cand),
    )
    mac_run = _mutate(good_r, "runs.0.binary.target", "aarch64-apple-darwin")
    mac_run = _mutate(mac_run, "runs.0.binary.digest_bound_to_ledger", True)
    expect_code(
        "binding/digest-mismatch",
        "F28R-125",
        lambda: check_binary_binding(mac_run, good_cand),
    )
    mac_ok = _mutate(mac_run, "runs.0.binary.sha256", "d" * 64)
    expect_ok("binding/ledger-bound-match", lambda: check_binary_binding(mac_ok, good_cand))

    # ---- cell coverage -------------------------------------------------------------
    expect_ok("coverage/good", lambda: check_cell_coverage(matrix, cases))

    def write_cases(name: str, specs: list[str]) -> Path:
        path = tmp / name
        path.write_text(_cases_fixture(specs), encoding="utf-8")
        return path

    # A dimension the matrix uses, dropped from the table: the cell is uncovered.
    dropped = write_cases(
        "cases-missing.rs", [_probe_fixture(d) for d in DIMENSIONS if d != "process-cleanup"]
    )
    expect_code("coverage/uncovered-cell", "F28R-053", lambda: check_cell_coverage(matrix, dropped))
    # A dimension whose family list is narrowed off the family the matrix needs.
    narrowed = write_cases(
        "cases-narrow.rs",
        [
            _probe_fixture(d, families=("Linux",)) if d == "unicode" else _probe_fixture(d)
            for d in DIMENSIONS
        ],
    )
    expect_code(
        "coverage/family-narrowed", "F28R-053", lambda: check_cell_coverage(matrix, narrowed)
    )
    # The nine are fixed.  `offline` is absent from this small fixture matrix, so
    # dropping its probe leaves every cell covered — and F28R-055 must still fire,
    # because the requirement is that all nine exist, not that the current matrix
    # happens to exercise them.
    renamed = write_cases(
        "cases-renamed.rs", [_probe_fixture(d) for d in DIMENSIONS if d != "offline"]
    )
    expect_code(
        "coverage/dimension-dropped-but-uncrossed",
        "F28R-055",
        lambda: check_cell_coverage(matrix, renamed),
    )
    # A sandbox probe that emits no activeness could not distinguish its own green
    # from a silently disabled sandbox.
    no_activeness = write_cases(
        "cases-no-activeness.rs",
        [
            _probe_fixture(d, activeness=False) if d == SANDBOX_DIMENSION else _probe_fixture(d)
            for d in DIMENSIONS
        ],
    )
    expect_code(
        "coverage/sandbox-probe-without-activeness",
        "F28R-056",
        lambda: check_cell_coverage(matrix, no_activeness),
    )
    # Two probes claiming the same (dimension, family) — ambiguous coverage.
    dup = write_cases(
        "cases-dup.rs",
        [_probe_fixture(d) for d in DIMENSIONS] + [_probe_fixture("unicode", probe_id="unicode-2")],
    )
    expect_code("coverage/double-claimed", "F28R-054", lambda: check_cell_coverage(matrix, dup))
    # A harness-bound probe as the ONLY coverage of a critical cell on macOS, where
    # no cargo harness can be built, narrows coverage silently.
    macro_matrix = tmp / "matrix-macos.tsv"
    macro_matrix.write_text(
        _MATRIX_FIXTURE
        + "sandbox-probes-macos-acp\tsandbox-probes\tmacos\tcmd:acp\tcritical\tapplicable\t-\t-\t"
        "required\n",
        encoding="utf-8",
    )
    harness_bound = write_cases(
        "cases-harness-bound.rs",
        [
            _probe_fixture(d, harness='Harness::HarnessBound { reason: "needs cargo" }')
            if d == SANDBOX_DIMENSION
            else _probe_fixture(d)
            for d in DIMENSIONS
        ],
    )
    expect_code(
        "coverage/harness-bound-critical-on-unbuildable-family",
        "F28R-057",
        lambda: check_cell_coverage(macro_matrix, harness_bound),
    )
    expect_ok(
        "coverage/black-box-critical-on-macos",
        lambda: check_cell_coverage(macro_matrix, cases),
    )

    print(f"self-test: {passed} assertions passed, {len(failed)} failed")
    for line in failed:
        print(f"  FAIL {line}")
    return 0 if not failed else 1


# ---------------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--check-controls", metavar="CONTROLS")
    parser.add_argument("--check-control-directions", metavar="CONTROLS")
    parser.add_argument("--verify", metavar="RESULTS")
    parser.add_argument("--check-control-precedence", nargs=2, metavar=("RESULTS", "CONTROLS"))
    parser.add_argument("--check-activeness", metavar="RESULTS")
    parser.add_argument("--check-skips", nargs=2, metavar=("RESULTS", "MATRIX"))
    parser.add_argument("--check-binary-binding", nargs=2, metavar=("RESULTS", "CANDIDATE"))
    parser.add_argument("--check-cell-coverage", nargs=2, metavar=("MATRIX", "CASES"))
    args = parser.parse_args(argv)

    try:
        if args.self_test:
            import tempfile

            with tempfile.TemporaryDirectory() as td:
                return self_test(Path(td))
        if args.check_controls:
            print(json.dumps(check_controls(_load(Path(args.check_controls))), sort_keys=True))
            return 0
        if args.check_control_directions:
            doc = _load(Path(args.check_control_directions))
            print(json.dumps(check_control_directions(doc), sort_keys=True))
            return 0
        if args.verify:
            print(json.dumps(verify_results(_load(Path(args.verify))), sort_keys=True))
            return 0
        if args.check_control_precedence:
            results, controls = (Path(p) for p in args.check_control_precedence)
            print(
                json.dumps(
                    check_control_precedence(_load(results), _load(controls)), sort_keys=True
                )
            )
            return 0
        if args.check_activeness:
            print(json.dumps(check_activeness(_load(Path(args.check_activeness))), sort_keys=True))
            return 0
        if args.check_skips:
            results, matrix = (Path(p) for p in args.check_skips)
            print(json.dumps(check_skips(_load(results), matrix), sort_keys=True))
            return 0
        if args.check_binary_binding:
            results, candidate = (Path(p) for p in args.check_binary_binding)
            print(
                json.dumps(check_binary_binding(_load(results), _load(candidate)), sort_keys=True)
            )
            return 0
        if args.check_cell_coverage:
            matrix, cases = (Path(p) for p in args.check_cell_coverage)
            print(json.dumps(check_cell_coverage(matrix, cases), sort_keys=True))
            return 0
    except Fail as exc:
        print(f"REJECTED: {exc}", file=sys.stderr)
        return 1

    parser.print_help()
    return 2


if __name__ == "__main__":
    sys.exit(main())
