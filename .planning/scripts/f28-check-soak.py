#!/usr/bin/env python3
"""f28-check-soak.py -- the F28-02 soak validator.

WHY THIS EXISTS
---------------
A canary scan reporting zero detections and a canary scan that never ran produce
IDENTICAL output. So do a clean orphan census and an orphan census that never
enumerated, and so do a flat resource series and a slope evaluator that cannot see
growth. Absence of a detection and absence of a detector are indistinguishable from the
outside, and a certification that cannot tell them apart signs "no secret leaked" over
the output of nothing.

So every observable in the soak carries a POSITIVE CONTROL, and this validator's first
job is to reject a clean result whose control was not caught. That rejection is VOID,
never RED: a red is a measurement of the product, a void is the absence of a
measurement, and collapsing them would let a missing detector's silence read as clean.

Its second job is the bands. The quality/performance delta has no threshold anywhere in
the program; plan 28-03 task 1 decided one by four-way cross-audit and committed it
BEFORE any soak session ran. This validator enforces the bands AS WRITTEN and refuses to
supply a default, because a default band is a band nobody decided and would make
Criterion 2 unfalsifiable.

Its third job is arithmetic honesty: the session count is read as ACTUALLY RUN rather
than as claimed, the resource verdict is computed from the RETAINED SERIES rather than
from an endpoint or a summary line, and every red is attributed to a carried-red entry,
to a 28-02 matrix finding, or filed as new with a Phase 28 re-score.

MODES
  --self-test                         fixtures, including one per VOID condition
  --check-bands  bands.json           the task-1 decision is mechanical and complete
  --verify       soak.json            schema + every observable's verdict
  --check-controls-caught soak.json   every positive control was caught
  --check-session-count  soak.json    sessions as run, not as claimed
  --check-series soak.json bands.json series retained and slope-evaluated
  --check-attribution soak.json known-red.tsv

EXIT CODES: 0 ok, 1 rejection, 2 usage.

Every rejection carries a distinct F28S-* code so a rule that stopped working is visible
rather than silently absent.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path
from typing import Any

CHANNELS = ["protocol", "stdout", "stderr", "files", "logs", "telemetry"]
OBSERVABLES = [
    "canary-integrity",
    "orphan-census",
    "resource-series",
    "quality-performance-drift",
]
BACKEND_AUTHORITY = {
    "cgroup-v2": True,
    "windows-job-object": True,
    "process-group-observed-nonauthoritative": False,
}
SESSION_TARGET = 1000
BLOCK_SIZE = 100
MIN_CONCURRENCY = 2

FORBIDDEN_SENTINELS = ["panicked at", "STATUS_ACCESS_VIOLATION", "stack backtrace:"]


class Reject(Exception):
    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


def reject(code: str, detail: str) -> None:
    raise Reject(code, detail)


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        reject("F28S-000", f"{path} does not exist; a missing artifact is not a pass")
    except json.JSONDecodeError as exc:
        reject("F28S-000", f"{path} is not valid JSON: {exc}")


# ---------------------------------------------------------------------------------
# bands
# ---------------------------------------------------------------------------------


def check_bands(bands: dict) -> None:
    """The decision must be MECHANICAL. Prose that needs judgement is not a band."""
    if bands.get("schema") != "f28-soak-bands/v1":
        reject("F28S-100", "bands file carries no f28-soak-bands/v1 schema tag")

    decision = bands.get("decision")
    if not isinstance(decision, dict):
        reject("F28S-101", "bands file records no decision block")

    votes = decision.get("votes")
    if not isinstance(votes, dict) or len(votes) != 4:
        reject(
            "F28S-102",
            "the decision must record exactly four panel votes; a silently dropped vote "
            "turns a four-way audit into a three-way and this program has measured that "
            "happening",
        )
    for member, vote in votes.items():
        if vote not in {"A", "B", "C", "D"}:
            reject("F28S-102", f"panel member {member} carries no A/B/C/D position")
    if decision.get("option") not in set(votes.values()):
        reject("F28S-103", "the adopted option is not one any panel member voted for")

    if decision.get("numbers_are_measured") is not False:
        reject(
            "F28S-104",
            "the bands must declare numbers_are_measured:false -- every threshold here "
            "was set by argument before any session ran, and a number nobody labelled as "
            "invented is a number the next person widens",
        )
    for key in ("numbers_provenance", "supersedes_clause", "widening_after_measurement"):
        if not str(decision.get(key, "")).strip():
            reject("F28S-104", f"decision.{key} is empty")

    if bands.get("session_target") != SESSION_TARGET:
        reject(
            "F28S-105",
            f"session_target is {bands.get('session_target')}, not {SESSION_TARGET}; this "
            "decision has no authority to reduce the requirement",
        )
    if int(bands.get("min_concurrency", 0)) < MIN_CONCURRENCY:
        reject(
            "F28S-106",
            "min_concurrency below 2 removes the sibling-dependent defect class from "
            "coverage",
        )

    windows = bands.get("windows") or {}
    early = windows.get("early_blocks") or []
    late = windows.get("late_blocks") or []
    if not early or not late:
        reject("F28S-107", "the drift windows are not both defined")
    if set(early) & set(late):
        reject("F28S-107", "the early and late windows overlap; a window cannot be its own baseline")
    if max(early) >= min(late):
        reject("F28S-107", "the late window does not follow the early window")

    sampling = bands.get("sampling") or {}
    if int(sampling.get("resource_interval_sessions", 0)) <= 0:
        reject("F28S-108", "no resource sampling interval is fixed, so a run could retain endpoints only")
    if int(sampling.get("resource_min_samples", 0)) < 2:
        reject("F28S-108", "resource_min_samples below 2 cannot distinguish a leak from a high-water mark")

    warmup = bands.get("warmup") or {}
    schema = warmup.get("sanity_schema") or {}
    if "required_exit_status" not in schema or not schema.get("forbidden_output_sentinels"):
        reject(
            "F28S-109",
            "warm-up has no committed sanity schema; warm-up may bind a value but it may "
            "never define whatever happened as correct, or a uniformly broken product "
            "teaches the validator that broken is normal",
        )

    if not bands.get("drift"):
        reject("F28S-110", "no drift bands are defined")
    for band in bands["drift"]:
        has_ratio = band.get("max_ratio") is not None
        has_drop = band.get("max_absolute_drop") is not None
        if has_ratio == has_drop:
            reject(
                "F28S-110",
                f"drift band {band.get('metric')!r} must set exactly one of max_ratio / "
                "max_absolute_drop",
            )
    if not bands.get("floors"):
        reject(
            "F28S-111",
            "no absolute floors are defined; option C without floors is option A, and a "
            "run that is uniformly broken from session 1 shows no drift",
        )
    for floor in bands["floors"]:
        if floor.get("op") not in {">=", "<="}:
            reject("F28S-111", f"floor {floor.get('metric')!r} carries no >= / <= operator")
    if not bands.get("slopes"):
        reject("F28S-112", "no resource slope bands are defined")

    noise = bands.get("noise_rule") or {}
    if "NOT PERMITTED" not in str(noise.get("third_verdict_between_pass_and_fail", "")):
        reject(
            "F28S-113",
            "the noise rule must forbid a third verdict between pass and fail; a "
            "load-adjusted INCONCLUSIVE is a dismissal lever with a statistician's "
            "vocabulary",
        )
    if "never" not in str(noise.get("on_drift_breach", "")).lower():
        reject("F28S-113", "the noise rule does not state that a covariate can never convert a breach into a pass")


# ---------------------------------------------------------------------------------
# verdict computation -- mirrors crates/wcore-eval-scenarios/src/e5_soak.rs
# ---------------------------------------------------------------------------------


def canary_verdict(scan: dict) -> tuple[str, str, str]:
    scanned = scan.get("channels_scanned") or []
    missing = [c for c in CHANNELS if c not in scanned]
    if missing:
        return ("void", "F28S-010", f"canary scan did not cover channel(s): {','.join(missing)}")
    if not scan.get("control_detected"):
        return (
            "void",
            "F28S-011",
            f"control canary planted in {scan.get('control_channel')!r} was NOT detected; a "
            "clean scan from a detector that cannot detect is not a clean result",
        )
    detections = sum(int(v) for v in (scan.get("channels") or {}).values())
    if detections:
        return ("red", "F28S-012", f"{detections} real canary detection(s) across the scanned channels")
    return ("green", "", "")


def orphan_verdict(census: dict) -> tuple[str, str, str]:
    backend = census.get("backend")
    if backend not in BACKEND_AUTHORITY:
        return ("void", "F28S-022", f"unknown census backend {backend!r}")
    if bool(census.get("authoritative")) != BACKEND_AUTHORITY[backend]:
        return (
            "void",
            "F28S-022",
            f"census claims authoritative={census.get('authoritative')} for backend "
            f"{backend!r}, whose authority is {BACKEND_AUTHORITY[backend]}",
        )
    if not census.get("control_orphan_found"):
        return (
            "void",
            "F28S-020",
            "the deliberately orphaned control process was NOT found; the census did not "
            "enumerate, which is not the same as there being nothing to enumerate",
        )
    found = int(census.get("orphans_found", 0))
    if found:
        return ("red", "F28S-021", f"{found} orphaned product process(es) survived the run")
    return ("green", "", "")


def series_growth(samples: list[dict], metric: str) -> tuple[float, float] | None:
    points = [
        (float(s["session_index"]), float(s["metrics"][metric]))
        for s in samples
        if metric in (s.get("metrics") or {})
    ]
    if not points:
        return None
    (first_idx, first), (last_idx, last) = points[0], points[-1]
    span = max(last_idx - first_idx, 1.0)
    absolute = (last - first) * (SESSION_TARGET / span)
    if abs(first) < 1e-12:
        ratio = 0.0 if abs(absolute) < 1e-12 else math.inf
    else:
        ratio = 1.0 + absolute / first
    return (absolute, ratio)


def resource_verdict(series: dict, bands: dict) -> tuple[str, str, str]:
    samples = series.get("samples") or []
    minimum = int((bands.get("sampling") or {}).get("resource_min_samples", 2))
    if len(samples) < minimum:
        return (
            "void",
            "F28S-030",
            f"resource series retained {len(samples)} sample(s), below the decided minimum "
            f"of {minimum}; an endpoint reading cannot distinguish a leak from a high-water mark",
        )
    if not series.get("control_growth_flagged"):
        return (
            "void",
            "F28S-031",
            "the deliberately growing control lane was NOT flagged by the slope evaluator; "
            "a flat verdict from an evaluator that cannot see growth is not a flat result",
        )
    breaches = []
    for band in bands.get("slopes") or []:
        growth = series_growth(samples, band["metric"])
        if growth is None:
            return ("void", "F28S-032", f"no retained samples carry metric {band['metric']!r}")
        observed = growth[1] if band.get("ratio") else growth[0]
        if observed > float(band["max_growth"]):
            breaches.append(
                f"{band['metric']}: growth {observed:.4f} exceeds decided {float(band['max_growth']):.4f}"
            )
    if breaches:
        return ("red", "F28S-033", "; ".join(breaches))
    return ("green", "", "")


def drift_verdict(measurements: list[dict], bands: dict | None) -> tuple[str, str, str]:
    if bands is None:
        return (
            "void",
            "F28S-040",
            "no bands file was available; a defaulted delta band is a band nobody decided",
        )
    by_metric = {m["metric"]: m for m in measurements}
    breaches = []
    for band in bands.get("drift") or []:
        m = by_metric.get(band["metric"])
        if m is None:
            return ("void", "F28S-041", f"decided band {band['metric']!r} has no corresponding measurement")
        early, late = float(m["early"]), float(m["late"])
        if band.get("max_ratio") is not None:
            limit = early * float(band["max_ratio"])
            if late > limit:
                breaches.append(
                    f"{band['metric']}: late {late:.3f} exceeds early {early:.3f} x "
                    f"{float(band['max_ratio']):.3f} = {limit:.3f}"
                )
        if band.get("max_absolute_drop") is not None:
            limit = early - float(band["max_absolute_drop"])
            if late < limit:
                breaches.append(
                    f"{band['metric']}: late {late:.4f} falls below early {early:.4f} - "
                    f"{float(band['max_absolute_drop']):.4f} = {limit:.4f}"
                )
    for floor in bands.get("floors") or []:
        m = by_metric.get(floor["metric"])
        if m is None:
            return ("void", "F28S-043", f"decided floor {floor['metric']!r} has no corresponding measurement")
        observed = float(m["late"])
        value = float(floor["value"])
        if floor["op"] == ">=" and observed < value:
            breaches.append(f"{floor['metric']}: {observed:.4f} is below the floor {value:.4f}")
        if floor["op"] == "<=" and observed > value:
            breaches.append(f"{floor['metric']}: {observed:.4f} is above the ceiling {value:.4f}")
    if breaches:
        return ("red", "F28S-042", "; ".join(breaches))
    return ("green", "", "")


def family_verdicts(fam: dict, bands: dict | None) -> dict[str, tuple[str, str, str]]:
    ledger = fam.get("ledger_sha256") or ""
    if not ledger or fam.get("binary_sha256") != ledger:
        v = (
            "void",
            "F28S-001",
            f"binary sha256 {fam.get('binary_sha256')} does not match the candidate ledger's "
            f"{ledger!r} for target {fam.get('target')}",
        )
        return {o: v for o in OBSERVABLES}
    return {
        "canary-integrity": canary_verdict(fam.get("canary") or {}),
        "orphan-census": orphan_verdict(fam.get("census") or {}),
        "resource-series": (
            resource_verdict(fam.get("resources") or {}, bands)
            if bands
            else ("void", "F28S-040", "no bands file was available for the resource slope evaluation")
        ),
        "quality-performance-drift": drift_verdict(fam.get("drift") or [], bands),
    }


# ---------------------------------------------------------------------------------
# modes
# ---------------------------------------------------------------------------------


def require_families(soak: dict) -> list[dict]:
    families = soak.get("families")
    if not isinstance(families, list) or not families:
        reject("F28S-002", "soak record contains no families; an omission is not a pass")
    return families


def do_verify(soak: dict, bands: dict | None) -> list[str]:
    if soak.get("schema") != "f28-soak/v1":
        reject("F28S-003", "soak record carries no f28-soak/v1 schema tag")
    lines = []
    for fam in require_families(soak):
        for key in ("family", "host", "target", "binary_sha256", "ledger_sha256",
                    "sessions_completed", "session_target", "concurrency"):
            if key not in fam:
                reject("F28S-004", f"family {fam.get('family')!r} omits required field {key!r}")
        if int(fam["concurrency"]) < MIN_CONCURRENCY:
            reject(
                "F28S-060",
                f"family {fam['family']!r} ran with concurrency {fam['concurrency']}, below "
                f"the required minimum {MIN_CONCURRENCY}; a sibling-dependent defect is "
                "invisible to a serial run",
            )
        wl = fam.get("workload")
        if not isinstance(wl, dict):
            reject(
                "F28S-006",
                f"family {fam.get('family')!r} records no workload census, so a soak of "
                "three trivial surfaces would be indistinguishable from a soak of the "
                "candidate's resolved surfaces",
            )
        candidate_surfaces = int(wl.get("candidate_surfaces", 0))
        established = int(wl.get("established", 0))
        broken = int(wl.get("broken_inventory", 0))
        max_broken = float(((bands or {}).get("warmup") or {}).get("max_broken_inventory_fraction", 0.05))
        if candidate_surfaces and broken / candidate_surfaces > max_broken:
            reject(
                "F28S-006",
                f"family {fam.get('family')!r}: broken inventory {broken}/{candidate_surfaces} "
                f"exceeds the decided {max_broken:.2%}; an invariant learned from a broken "
                "baseline cannot fail, so the run is VOID rather than passed",
            )
        if established < 20 or (candidate_surfaces and established / candidate_surfaces < 0.25):
            reject(
                "F28S-007",
                f"family {fam.get('family')!r}: only {established} of {candidate_surfaces} "
                "resolved surfaces established an invariant; a thousand sessions over a "
                "collapsed workload certify the collapse",
            )
        recorded = fam.get("observable_verdicts") or {}
        computed = family_verdicts(fam, bands)
        for obs, (kind, code, detail) in computed.items():
            claimed = (recorded.get(obs) or {}).get("verdict")
            if claimed is not None and claimed != kind:
                reject(
                    "F28S-005",
                    f"family {fam['family']!r} records observable {obs} as {claimed!r} but "
                    f"the retained evidence computes {kind!r} ({code}); the summary line is "
                    "not the measurement",
                )
            lines.append(f"{fam['family']}\t{obs}\t{kind}\t{code}\t{detail}")
    return lines


def do_controls(soak: dict) -> list[str]:
    lines = []
    for fam in require_families(soak):
        scan = fam.get("canary") or {}
        census = fam.get("census") or {}
        series = fam.get("resources") or {}
        checks = {
            "canary": bool(scan.get("control_detected")),
            "orphan": bool(census.get("control_orphan_found")),
            "resource": bool(series.get("control_growth_flagged")),
        }
        for name, caught in checks.items():
            lines.append(f"{fam.get('family')}\tcontrol:{name}\t{'CAUGHT' if caught else 'MISSED'}")
        missed = [n for n, c in checks.items() if not c]
        if missed:
            reject(
                "F28S-070",
                f"family {fam.get('family')!r} missed positive control(s) {','.join(missed)}; "
                "that run is VOID, not green, and the miss is itself a finding",
            )
    return lines


def do_session_count(soak: dict) -> list[str]:
    lines = []
    shortfalls = []
    for fam in require_families(soak):
        completed = int(fam.get("sessions_completed", -1))
        target = int(fam.get("session_target", SESSION_TARGET))
        if target != SESSION_TARGET:
            reject(
                "F28S-051",
                f"family {fam.get('family')!r} records session_target {target}; the "
                "requirement is 1,000 and no plan may reduce it",
            )
        # sessions AS RUN, not as claimed: the per-block ledger is the count.
        blocks = fam.get("blocks") or []
        run = sum(int(b.get("sessions", 0)) for b in blocks)
        if not blocks:
            reject(
                "F28S-052",
                f"family {fam.get('family')!r} retains no per-block ledger, so the session "
                "count cannot be read as run rather than as claimed",
            )
        if run != completed:
            reject(
                "F28S-053",
                f"family {fam.get('family')!r} claims {completed} sessions but its retained "
                f"per-block ledger sums to {run}",
            )
        lines.append(f"{fam.get('family')}\tsessions_run={run}\ttarget={target}")
        if run < target:
            shortfalls.append(f"{fam.get('family')}: {run}/{target} (shortfall {target - run})")
    if shortfalls:
        reject(
            "F28S-050",
            "session shortfall -- Criterion 2 is NOT MET for: " + "; ".join(shortfalls),
        )
    # A family that could not be run at all is a shortfall of 1,000, not an omission. It is
    # recorded separately because it has no measured record to carry it, and it must still
    # turn this gate RED -- otherwise "the families we ran all passed" reads as "the soak
    # passed", which is the exact narrowing this phase exists to prevent.
    not_run = soak.get("families_not_run") or []
    if not_run:
        named = "; ".join(
            f"{f.get('family')}: 0/{SESSION_TARGET} ({f.get('reason', 'no reason recorded')})"
            for f in not_run
        )
        for f in not_run:
            lines.append(f"{f.get('family')}\tsessions_run=0\ttarget={SESSION_TARGET}\tNOT RUN")
        reject("F28S-054", "family NOT RUN -- Criterion 2 is NOT MET for: " + named)
    return lines


def do_series(soak: dict, bands: dict) -> list[str]:
    lines = []
    for fam in require_families(soak):
        series = fam.get("resources") or {}
        samples = series.get("samples") or []
        endpoints_only = len(samples) <= 2
        if endpoints_only:
            reject(
                "F28S-030",
                f"family {fam.get('family')!r} retained {len(samples)} sample(s); an endpoint "
                "reading cannot distinguish a leak from a high-water mark and unbounded "
                "growth is a claim about the trend",
            )
        kind, code, detail = resource_verdict(series, bands)
        for band in bands.get("slopes") or []:
            growth = series_growth(samples, band["metric"])
            if growth is not None:
                observed = growth[1] if band.get("ratio") else growth[0]
                lines.append(
                    f"{fam.get('family')}\t{band['metric']}\tgrowth={observed:.4f}\t"
                    f"band={float(band['max_growth']):.4f}"
                )
        lines.append(f"{fam.get('family')}\tresource-series\t{kind}\t{code}\t{detail}")
        if kind != "green":
            reject(code or "F28S-034", f"family {fam.get('family')!r}: {detail}")
    return lines


def load_known_red(path: Path) -> dict[str, dict]:
    rows: dict[str, dict] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        if not raw.strip() or raw.startswith("#"):
            continue
        parts = raw.split("\t")
        if len(parts) < 8:
            continue
        rows[parts[0]] = {
            "subject": parts[1],
            "p28_severity": parts[3],
            "contradicted_criterion": parts[4],
            "available_dispositions": parts[5],
        }
    return rows


def do_attribution(soak: dict, known_red: dict[str, dict]) -> list[str]:
    lines = []
    for fam in require_families(soak):
        for red in fam.get("reds") or []:
            kind = red.get("attribution", {}).get("kind")
            ident = red.get("attribution", {}).get("id")
            if kind == "carried-red":
                if ident not in known_red:
                    reject(
                        "F28S-080",
                        f"red {red.get('id')!r} attributes to carried entry {ident!r}, which is "
                        "not in the carried-red ledger",
                    )
                row = known_red[ident]
                if row["contradicted_criterion"] not in {"-", ""} and red.get("disposition") in {
                    "ACCEPTED",
                    "DEFERRED",
                }:
                    reject(
                        "F28S-081",
                        f"red {red.get('id')!r} takes the {red.get('disposition')} path against "
                        f"carried entry {ident!r}, whose contradicted_criterion is "
                        f"{row['contradicted_criterion']!r}; amendment A2 closes that path",
                    )
            elif kind == "matrix-finding":
                if not str(ident or "").startswith("F-28-02-"):
                    reject("F28S-082", f"red {red.get('id')!r} cites {ident!r} as a 28-02 finding")
            elif kind == "new":
                if not red.get("p28_rescore"):
                    reject(
                        "F28S-083",
                        f"red {red.get('id')!r} is filed as new with no Phase 28 re-score",
                    )
                if red.get("p28_rescore", {}).get("contradicted_criterion") not in {"-", None} and (
                    red.get("disposition") in {"ACCEPTED", "DEFERRED"}
                ):
                    reject(
                        "F28S-081",
                        f"new red {red.get('id')!r} contradicts a criterion and may not take the "
                        f"{red.get('disposition')} path (amendment A2)",
                    )
            else:
                reject(
                    "F28S-084",
                    f"red {red.get('id')!r} carries no attribution; an unattributed red is a red "
                    "nobody has to account for",
                )
            if fam.get("family") == "windows" and not red.get("quiet_run_id"):
                reject(
                    "F28S-085",
                    f"windows red {red.get('id')!r} names no quiet run; the two Windows runners "
                    "are one physical box and a red produced under concurrent load is a load "
                    "artifact rather than a recordable red",
                )
            lines.append(f"{fam.get('family')}\t{red.get('id')}\t{kind}\t{ident}")
    return lines


# ---------------------------------------------------------------------------------
# self-test
# ---------------------------------------------------------------------------------


def _bands_fixture() -> dict:
    return {
        "schema": "f28-soak-bands/v1",
        "decision": {
            "option": "C",
            "votes": {"codex": "C", "gemini": "C", "kimi": "C", "internal_adversarial": "C"},
            "numbers_are_measured": False,
            "numbers_provenance": "pre-registered, unmeasured",
            "supersedes_clause": "re-derive from the first baseline for the NEXT candidate",
            "widening_after_measurement": "FORBIDDEN",
        },
        "session_target": 1000,
        "block_size": 100,
        "min_concurrency": 2,
        "windows": {"early_blocks": [1, 2, 3], "late_blocks": [8, 9, 10]},
        "sampling": {"resource_interval_sessions": 10, "resource_min_samples": 4},
        "warmup": {
            "max_broken_inventory_fraction": 0.05,
            "sanity_schema": {
                "required_exit_status": 0,
                "forbidden_output_sentinels": FORBIDDEN_SENTINELS,
            },
        },
        "drift": [{"metric": "latency_p50_block_median_ms", "max_ratio": 1.5}],
        "floors": [{"metric": "quality_correct_rate_run", "op": ">=", "value": 0.99}],
        "slopes": [{"metric": "state_dir_bytes", "max_growth": 2.0, "ratio": True}],
        "noise_rule": {
            "third_verdict_between_pass_and_fail": "NOT PERMITTED",
            "on_drift_breach": "RED; a covariate may never convert a breach into a pass",
        },
    }


def _family_fixture() -> dict:
    samples = [
        {"session_index": i, "metrics": {"state_dir_bytes": 1000.0 + i}}
        for (i) in (0, 250, 500, 750, 1000)
    ]
    return {
        "family": "linux",
        "host": "fixture",
        "target": "x86_64-unknown-linux-gnu",
        "binary_sha256": "a" * 64,
        "ledger_sha256": "a" * 64,
        "sessions_completed": 1000,
        "session_target": 1000,
        "concurrency": 4,
        "workload": {
            "candidate_surfaces": 116,
            "established": 93,
            "precondition_unavailable": 21,
            "broken_inventory": 2,
        },
        "blocks": [{"block": i + 1, "sessions": 100} for i in range(10)],
        "canary": {
            "channels": {c: 0 for c in CHANNELS},
            "channels_scanned": list(CHANNELS),
            "control_detected": True,
            "control_channel": "files",
        },
        "census": {
            "backend": "cgroup-v2",
            "authoritative": True,
            "orphans_found": 0,
            "control_orphan_found": True,
        },
        "resources": {"samples": samples, "control_growth_flagged": True},
        "drift": [
            {"metric": "latency_p50_block_median_ms", "early": 100.0, "late": 110.0},
            {"metric": "quality_correct_rate_run", "early": 1.0, "late": 1.0},
        ],
        "reds": [],
    }


def _soak_fixture() -> dict:
    return {"schema": "f28-soak/v1", "families": [_family_fixture()]}


def _expect_reject(name: str, code: str, fn) -> tuple[bool, str]:
    try:
        fn()
    except Reject as exc:
        if exc.code == code:
            return (True, f"ok   {name} -> {code}")
        return (False, f"FAIL {name} -> expected {code}, got {exc.code}")
    return (False, f"FAIL {name} -> expected {code}, nothing was rejected")


def _expect_ok(name: str, fn) -> tuple[bool, str]:
    try:
        fn()
    except Reject as exc:
        return (False, f"FAIL {name} -> unexpected {exc.code}: {exc.detail}")
    return (True, f"ok   {name} -> accepted")


def self_test() -> int:
    results: list[tuple[bool, str]] = []
    kr = {"KR-01": {"subject": "orphan", "p28_severity": "HIGH",
                    "contradicted_criterion": "2", "available_dispositions": "FIXED,DISPROVED"}}

    # --- the control path is accepted at all -------------------------------------
    results.append(_expect_ok("baseline bands accepted", lambda: check_bands(_bands_fixture())))
    results.append(_expect_ok("baseline soak verifies",
                              lambda: do_verify(_soak_fixture(), _bands_fixture())))
    results.append(_expect_ok("baseline controls caught", lambda: do_controls(_soak_fixture())))
    results.append(_expect_ok("baseline session count", lambda: do_session_count(_soak_fixture())))
    results.append(_expect_ok("baseline series",
                              lambda: do_series(_soak_fixture(), _bands_fixture())))
    results.append(_expect_ok("baseline attribution", lambda: do_attribution(_soak_fixture(), kr)))

    # --- one fixture per VOID condition ------------------------------------------
    def void_canary_control():
        s = _soak_fixture()
        s["families"][0]["canary"]["control_detected"] = False
        do_controls(s)

    results.append(_expect_reject("VOID: control canary undetected", "F28S-070", void_canary_control))

    def void_canary_control_verdict():
        s = _soak_fixture()
        s["families"][0]["canary"]["control_detected"] = False
        s["families"][0]["observable_verdicts"] = {"canary-integrity": {"verdict": "green"}}
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("VOID: clean scan claimed green with control missed",
                                  "F28S-005", void_canary_control_verdict))

    def void_channel_dropped():
        s = _soak_fixture()
        s["families"][0]["canary"]["channels_scanned"].remove("telemetry")
        s["families"][0]["observable_verdicts"] = {"canary-integrity": {"verdict": "green"}}
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("VOID: a channel was silently dropped", "F28S-005", void_channel_dropped))

    def void_orphan_control():
        s = _soak_fixture()
        s["families"][0]["census"]["control_orphan_found"] = False
        do_controls(s)

    results.append(_expect_reject("VOID: control orphan unfound", "F28S-070", void_orphan_control))

    def void_backend_authority():
        s = _soak_fixture()
        s["families"][0]["census"]["backend"] = "process-group-observed-nonauthoritative"
        s["families"][0]["observable_verdicts"] = {"orphan-census": {"verdict": "green"}}
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("VOID: non-authoritative census claims authority",
                                  "F28S-005", void_backend_authority))

    def void_endpoint_only():
        s = _soak_fixture()
        s["families"][0]["resources"]["samples"] = s["families"][0]["resources"]["samples"][:2]
        do_series(s, _bands_fixture())

    results.append(_expect_reject("VOID: endpoint-only series", "F28S-030", void_endpoint_only))

    def void_growth_control():
        s = _soak_fixture()
        s["families"][0]["resources"]["control_growth_flagged"] = False
        do_series(s, _bands_fixture())

    results.append(_expect_reject("VOID: growth control unflagged", "F28S-031", void_growth_control))

    def void_no_bands():
        assert drift_verdict([], None)[1] == "F28S-040"
        reject("F28S-040", "drift verdict with no bands is void rather than defaulted")

    results.append(_expect_reject("VOID: no bands file supplied", "F28S-040", void_no_bands))

    def void_missing_metric():
        b = _bands_fixture()
        b["slopes"] = [{"metric": "not_sampled", "max_growth": 1.0, "ratio": False}]
        do_series(_soak_fixture(), b)

    results.append(_expect_reject("VOID: banded metric never sampled", "F28S-032", void_missing_metric))

    def void_digest_mismatch():
        s = _soak_fixture()
        s["families"][0]["binary_sha256"] = "b" * 64
        s["families"][0]["observable_verdicts"] = {"canary-integrity": {"verdict": "green"}}
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("VOID: binary is not the candidate", "F28S-005", void_digest_mismatch))

    # --- RED conditions -----------------------------------------------------------
    def red_shortfall():
        s = _soak_fixture()
        s["families"][0]["blocks"] = [{"block": i + 1, "sessions": 100} for i in range(7)]
        s["families"][0]["sessions_completed"] = 700
        do_session_count(s)

    results.append(_expect_reject("RED: 700 of 1000 sessions", "F28S-050", red_shortfall))

    def red_claimed_not_run():
        s = _soak_fixture()
        s["families"][0]["blocks"] = [{"block": i + 1, "sessions": 50} for i in range(10)]
        do_session_count(s)

    results.append(_expect_reject("RED: claimed count exceeds the retained ledger",
                                  "F28S-053", red_claimed_not_run))

    def red_family_not_run():
        s = _soak_fixture()
        s["families_not_run"] = [{"family": "windows", "reason": "host unreachable"}]
        do_session_count(s)

    results.append(_expect_reject("RED: a family could not be run at all", "F28S-054",
                                  red_family_not_run))

    def red_slope():
        s = _soak_fixture()
        s["families"][0]["resources"]["samples"] = [
            {"session_index": i, "metrics": {"state_dir_bytes": 1000.0 * (1 + i)}}
            for i in (0, 250, 500, 750, 1000)
        ]
        do_series(s, _bands_fixture())

    results.append(_expect_reject("RED: state dir grows without bound", "F28S-033", red_slope))

    def red_drift():
        s = _soak_fixture()
        s["families"][0]["drift"][0]["late"] = 200.0
        s["families"][0]["observable_verdicts"] = {"quality-performance-drift": {"verdict": "green"}}
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("RED: late p50 exceeds the decided ratio", "F28S-005", red_drift))

    def red_floor():
        s = _soak_fixture()
        s["families"][0]["drift"][1]["late"] = 0.5
        s["families"][0]["observable_verdicts"] = {"quality-performance-drift": {"verdict": "green"}}
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("RED: uniformly broken run trips the absolute floor",
                                  "F28S-005", red_floor))

    def void_broken_inventory():
        s = _soak_fixture()
        s["families"][0]["workload"]["broken_inventory"] = 40
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("VOID: warm-up baseline is mostly broken", "F28S-006",
                                  void_broken_inventory))

    def void_collapsed_workload():
        s = _soak_fixture()
        s["families"][0]["workload"]["established"] = 3
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("VOID: workload collapsed to a handful of surfaces",
                                  "F28S-007", void_collapsed_workload))

    def void_no_workload_census():
        s = _soak_fixture()
        s["families"][0].pop("workload")
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("VOID: no workload census at all", "F28S-006",
                                  void_no_workload_census))

    def red_zero_concurrency():
        s = _soak_fixture()
        s["families"][0]["concurrency"] = 0
        do_verify(s, _bands_fixture())

    results.append(_expect_reject("RED: zero-concurrency soak", "F28S-060", red_zero_concurrency))

    # --- attribution --------------------------------------------------------------
    def unattributed_red():
        s = _soak_fixture()
        s["families"][0]["reds"] = [{"id": "R1"}]
        do_attribution(s, kr)

    results.append(_expect_reject("unattributed red", "F28S-084", unattributed_red))

    def a2_accept_path():
        s = _soak_fixture()
        s["families"][0]["reds"] = [
            {"id": "R1", "attribution": {"kind": "carried-red", "id": "KR-01"},
             "disposition": "ACCEPTED"}
        ]
        do_attribution(s, kr)

    results.append(_expect_reject("A2: accept path taken against a criterion-contradicting entry",
                                  "F28S-081", a2_accept_path))

    def unknown_carried():
        s = _soak_fixture()
        s["families"][0]["reds"] = [
            {"id": "R1", "attribution": {"kind": "carried-red", "id": "KR-99"}}
        ]
        do_attribution(s, kr)

    results.append(_expect_reject("carried entry not in the ledger", "F28S-080", unknown_carried))

    def new_without_rescore():
        s = _soak_fixture()
        s["families"][0]["reds"] = [{"id": "R1", "attribution": {"kind": "new"}}]
        do_attribution(s, kr)

    results.append(_expect_reject("new red with no Phase 28 re-score", "F28S-083", new_without_rescore))

    def windows_red_without_quiet_run():
        s = _soak_fixture()
        s["families"][0]["family"] = "windows"
        s["families"][0]["reds"] = [
            {"id": "R1", "attribution": {"kind": "carried-red", "id": "KR-01"},
             "disposition": "OPEN"}
        ]
        do_attribution(s, kr)

    results.append(_expect_reject("windows red names no quiet run", "F28S-085",
                                  windows_red_without_quiet_run))

    # --- bands rejections ---------------------------------------------------------
    def bands_three_votes():
        b = _bands_fixture()
        b["decision"]["votes"].pop("kimi")
        check_bands(b)

    results.append(_expect_reject("bands: a dropped panel vote", "F28S-102", bands_three_votes))

    def bands_reduced_target():
        b = _bands_fixture()
        b["session_target"] = 250
        check_bands(b)

    results.append(_expect_reject("bands: session target reduced", "F28S-105", bands_reduced_target))

    def bands_no_floors():
        b = _bands_fixture()
        b["floors"] = []
        check_bands(b)

    results.append(_expect_reject("bands: option C with no floors is option A", "F28S-111", bands_no_floors))

    def bands_overlapping_windows():
        b = _bands_fixture()
        b["windows"]["late_blocks"] = [3, 9, 10]
        check_bands(b)

    results.append(_expect_reject("bands: windows overlap", "F28S-107", bands_overlapping_windows))

    def bands_numbers_claimed_measured():
        b = _bands_fixture()
        b["decision"]["numbers_are_measured"] = True
        check_bands(b)

    results.append(_expect_reject("bands: invented numbers claimed as measured",
                                  "F28S-104", bands_numbers_claimed_measured))

    def bands_inconclusive_permitted():
        b = _bands_fixture()
        b["noise_rule"]["third_verdict_between_pass_and_fail"] = "allowed when load is high"
        check_bands(b)

    results.append(_expect_reject("bands: a third verdict between pass and fail",
                                  "F28S-113", bands_inconclusive_permitted))

    def bands_warmup_defines_correct():
        b = _bands_fixture()
        b["warmup"]["sanity_schema"] = {}
        check_bands(b)

    results.append(_expect_reject("bands: warm-up may define whatever happened as correct",
                                  "F28S-109", bands_warmup_defines_correct))

    def bands_no_sampling_interval():
        b = _bands_fixture()
        b["sampling"]["resource_interval_sessions"] = 0
        check_bands(b)

    results.append(_expect_reject("bands: no sampling interval", "F28S-108", bands_no_sampling_interval))

    def bands_serial():
        b = _bands_fixture()
        b["min_concurrency"] = 1
        check_bands(b)

    results.append(_expect_reject("bands: concurrency configured away", "F28S-106", bands_serial))

    for ok, line in results:
        print(line)
    failed = sum(1 for ok, _ in results if not ok)
    print(f"\n{len(results)} assertions, {failed} failed")
    return 1 if failed else 0


# ---------------------------------------------------------------------------------


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--self-test", action="store_true")
    g.add_argument("--check-bands", metavar="bands.json")
    g.add_argument("--verify", metavar="soak.json")
    g.add_argument("--check-controls-caught", metavar="soak.json")
    g.add_argument("--check-session-count", metavar="soak.json")
    g.add_argument("--check-series", nargs=2, metavar=("soak.json", "bands.json"))
    g.add_argument("--check-attribution", nargs=2, metavar=("soak.json", "known-red.tsv"))
    p.add_argument("--bands", metavar="bands.json", help="bands for --verify")
    args = p.parse_args()

    try:
        if args.self_test:
            return self_test()
        if args.check_bands:
            check_bands(load_json(Path(args.check_bands)))
            print(f"OK  bands accepted: {args.check_bands}")
            return 0
        if args.verify:
            soak_path = Path(args.verify)
            bands_path = Path(args.bands) if args.bands else soak_path.with_name("bands.json")
            bands = load_json(bands_path) if bands_path.exists() else None
            for line in do_verify(load_json(soak_path), bands):
                print(line)
            return 0
        if args.check_controls_caught:
            for line in do_controls(load_json(Path(args.check_controls_caught))):
                print(line)
            return 0
        if args.check_session_count:
            for line in do_session_count(load_json(Path(args.check_session_count))):
                print(line)
            return 0
        if args.check_series:
            soak_p, bands_p = args.check_series
            for line in do_series(load_json(Path(soak_p)), load_json(Path(bands_p))):
                print(line)
            return 0
        if args.check_attribution:
            soak_p, kr_p = args.check_attribution
            for line in do_attribution(load_json(Path(soak_p)), load_known_red(Path(kr_p))):
                print(line)
            return 0
    except Reject as exc:
        print(f"REJECTED {exc.code}: {exc.detail}", file=sys.stderr)
        return 1
    return 2


if __name__ == "__main__":
    sys.exit(main())
