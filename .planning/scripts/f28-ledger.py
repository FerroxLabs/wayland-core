#!/usr/bin/env python3
"""f28-ledger.py — executable enforcement of the Phase 28 certification contract.

The contract prose lives in
`.planning/phases/28-native-cross-platform-certification/28-01-CERTIFICATION-CONTRACT.md`.
This script is its enforcement. Where the two could drift, `--check-contract` fails closed
by re-extracting the quoted authorities from their ORIGINAL sources (ROADMAP.md,
REQUIREMENTS.md, decision-rationale.txt) and asserting they appear in the contract
verbatim. That makes it a real cross-check rather than the executor grepping a file it
wrote itself.

Every rejection returns a DISTINCT machine-readable failure code, because plan 28-04 gates
on specific codes: a generic failure would let one rule silently stop working while the
gate stayed green.

`--self-test` exercises every code with a fixture that TRIPS it and a fixture that does
NOT. A validator that has only ever been shown valid input is untested, and a rule that
only ever rejects is equally broken — this program's own plan-gate linter shipped the
disease it hunts four separate times by testing one direction only.

Exit codes: 0 = all checks passed. 1 = at least one rejection. 2 = usage / missing input.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

# --------------------------------------------------------------------------------------
# Vocabulary — fixed by the contract. Adding to any of these lists is a contract change.
# --------------------------------------------------------------------------------------

SEVERITIES = ("CRITICAL", "HIGH", "MEDIUM", "LOW")
BLOCKING_SEVERITIES = ("CRITICAL", "HIGH")
DISPOSITIONS = ("FIXED", "DISPROVED", "ACCEPTED", "DEFERRED")
PAPER_DISPOSITIONS = ("ACCEPTED", "DEFERRED")
OPEN = "OPEN"
NONE = "-"

# The four legal skip classes. NO FIFTH CLASS MAY BE ADDED MID-RUN.
SKIP_CLASSES = (
    "platform-inapplicability",
    "observation-blocked",
    "architectural-impossibility",
    "unresolved-surface",
)

# The nine F28-01 dimensions, VERBATIM from the requirement text. Fixed.
DIMENSIONS = (
    "sandbox-probes",
    "unicode",
    "long-paths",
    "unc-reparse-symlink",
    "process-cleanup",
    "suspend-resume",
    "offline",
    "disk-full-read-only",
    "hostile-inputs",
)
SANDBOX_DIMENSION = "sandbox-probes"

OS_FAMILIES = ("linux", "macos", "windows")

MANDATORY_CELLS = (
    "w-sandbox-silent-disable",
    "w-process-cleanup-descendant-tree",
    "w-sandbox-observability-control",
)

# A run-time control reference: control:<id>@<host>:<run-id>.
CONTROL_REF = re.compile(r"^control:[a-z0-9][a-z0-9-]*@[A-Za-z0-9._-]+:[A-Za-z0-9._-]+$")

# Documentary-citation patterns. An `observation-blocked` skip whose evidence matches ANY
# of these is rejected. This is the anti-laundering deny-list and it fires on documents
# that report in the product's FAVOUR too — a laundering channel does not become sound by
# pointing it at good news.
LORE_PATTERNS = (
    ".md",
    "handoff",
    ".planning/intel",
    "intel/",
    "-plan",
    "plan.md",
    "-summary",
    "summary.md",
    "requirements",
    "roadmap",
    "lore",
    "see ",
    "as established",
    "previously",
    "known that",
)


# --------------------------------------------------------------------------------------
# Result plumbing
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Rejection:
    code: str
    where: str
    detail: str

    def __str__(self) -> str:
        return f"{self.code}  {self.where}: {self.detail}"


def _norm(text: str) -> str:
    """Collapse all runs of whitespace so a re-wrapped quote still matches verbatim text.

    Markdown blockquote markers are stripped line-leading, because the contract quotes its
    authorities inside `>` blocks and a wrapped quote would otherwise carry a `>` into the
    middle of a sentence. Paraphrase still fails; only line wrapping, indentation and
    blockquote markers are forgiven.
    """
    lines = [re.sub(r"^\s*>+\s?", "", line) for line in text.splitlines()]
    return re.sub(r"\s+", " ", " ".join(lines)).strip()


# --------------------------------------------------------------------------------------
# Finding-ledger validation  (codes F28L-*)
# --------------------------------------------------------------------------------------

LEDGER_FIELDS = (
    "id",
    "subject",
    "inherited_severity",
    "p28_severity",
    "contradicted_criterion",
    "available_dispositions",
    "disposition",
    "rationale",
)

# Extra fields a fully-dispositioned ledger carries (plan 28-04). Optional here.
LEDGER_OPTIONAL_FIELDS = (
    "owner",
    "backlog_id",
    "executable_check",
    "counter_evidence",
    "origin",
    "downgrade_review",
)


def parse_ledger(text: str) -> list[dict[str, str]]:
    """Parse a `#`-commented TSV into rows keyed by LEDGER_FIELDS (+ optional trailing)."""
    rows: list[dict[str, str]] = []
    names = list(LEDGER_FIELDS) + list(LEDGER_OPTIONAL_FIELDS)
    for lineno, raw in enumerate(text.splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        cells = raw.split("\t")
        row: dict[str, str] = {"_line": str(lineno)}
        for i, name in enumerate(names):
            row[name] = cells[i].strip() if i < len(cells) else ""
        row["_ncells"] = str(len(cells))
        rows.append(row)
    return rows


def validate_ledger(rows: list[dict[str, str]], *, allow_open: bool) -> list[Rejection]:
    """Validate finding rows against the contract's severity and disposition rules.

    `allow_open` is True for the carried-red ledger authored by plan 28-01, which
    deliberately does not dispose findings. Plan 28-04 calls with allow_open=False, at
    which point a missing disposition is the gate Criterion 4 actually is.
    """
    out: list[Rejection] = []
    for row in rows:
        where = f"{row.get('id') or '<no id>'} (line {row['_line']})"

        if int(row["_ncells"]) < len(LEDGER_FIELDS):
            out.append(
                Rejection(
                    "F28L-012",
                    where,
                    f"row has {row['_ncells']} tab-separated fields, "
                    f"expected at least {len(LEDGER_FIELDS)}",
                )
            )
            continue

        sev = row["p28_severity"]
        disp = row["disposition"]
        contradicted = row["contradicted_criterion"]
        avail = [d for d in row["available_dispositions"].split(",") if d]

        # --- A1: a finding may not enter on an inherited severity alone -----------------
        if not sev:
            out.append(
                Rejection(
                    "F28L-006",
                    where,
                    "no Phase 28 re-score (p28_severity empty); amendment A1 forbids a "
                    "finding entering the ledger at an inherited severity",
                )
            )
        elif sev not in SEVERITIES:
            out.append(
                Rejection("F28L-011", where, f"unknown p28_severity {sev!r}")
            )
        if not row["inherited_severity"]:
            out.append(
                Rejection(
                    "F28L-006",
                    where,
                    "inherited_severity absent; A1 requires it be recorded as provenance",
                )
            )

        # --- contradicted_criterion must be explicit, never blank ----------------------
        if contradicted == "":
            out.append(
                Rejection(
                    "F28L-013",
                    where,
                    "contradicted_criterion is empty; write '-' for none so an omission "
                    "cannot read as a none",
                )
            )
        contradicts = contradicted not in ("", NONE)
        if contradicts and contradicted not in ("1", "2", "3", "4"):
            out.append(
                Rejection(
                    "F28L-014",
                    where,
                    f"contradicted_criterion {contradicted!r} is not one of 1..4 or '-'",
                )
            )

        # --- disposition presence: THIS is the gate Criterion 4 actually is -------------
        if not disp:
            out.append(Rejection("F28L-002", where, "no disposition recorded"))
        elif disp == OPEN:
            if not allow_open:
                out.append(
                    Rejection(
                        "F28L-002",
                        where,
                        "disposition is OPEN; acceptance requires a terminal disposition",
                    )
                )
        elif disp not in DISPOSITIONS:
            out.append(Rejection("F28L-010", where, f"unknown disposition {disp!r}"))

        # --- the accept-forbidden rule at CRITICAL/HIGH --------------------------------
        if disp in PAPER_DISPOSITIONS and sev in BLOCKING_SEVERITIES:
            out.append(
                Rejection(
                    "F28L-001",
                    where,
                    f"{disp} is not reachable at {sev}; CRITICAL and HIGH have exactly "
                    "two dispositions, FIXED or DISPROVED",
                )
            )

        # --- A2: a criterion-contradicting finding may not take the paper path ----------
        # Fires on contradicted_criterion REGARDLESS of the severity recorded, so a
        # mis-scored severity cannot reopen the accept path.
        if disp in PAPER_DISPOSITIONS and contradicts:
            out.append(
                Rejection(
                    "F28L-007",
                    where,
                    f"{disp} taken while contradicting Success Criterion {contradicted}; "
                    "amendment A2 closes the accept and defer paths by construction",
                )
            )
        if contradicts and any(d in PAPER_DISPOSITIONS for d in avail):
            out.append(
                Rejection(
                    "F28L-007",
                    where,
                    f"available_dispositions offers {avail} while contradicting Success "
                    f"Criterion {contradicted}; A2 closes the accept and defer paths",
                )
            )
        if sev in BLOCKING_SEVERITIES and any(d in PAPER_DISPOSITIONS for d in avail):
            out.append(
                Rejection(
                    "F28L-001",
                    where,
                    f"available_dispositions offers {avail} at {sev}",
                )
            )

        # --- evidence requirements on the paper path ----------------------------------
        if disp in PAPER_DISPOSITIONS:
            if not row["rationale"]:
                out.append(Rejection("F28L-003", where, f"{disp} with no rationale"))
            if not row.get("owner"):
                out.append(Rejection("F28L-004", where, f"{disp} with no owner"))
            if not row.get("backlog_id"):
                out.append(Rejection("F28L-005", where, f"{disp} with no backlog id"))

        # --- evidence requirements on the repair path ---------------------------------
        if disp == "FIXED" and not row.get("executable_check"):
            out.append(
                Rejection(
                    "F28L-008",
                    where,
                    "FIXED with no executable-check reference; a repair is proved by a "
                    "check, not asserted",
                )
            )
        if disp == "DISPROVED" and not row.get("counter_evidence"):
            out.append(
                Rejection(
                    "F28L-009",
                    where,
                    "DISPROVED with no counter-evidence reference",
                )
            )

    return out


def check_rescoring(rows: list[dict[str, str]]) -> list[Rejection]:
    """Carried-red specific checks, on top of validate_ledger(allow_open=True).

    Proves the A2 crossings the contract names actually carry a closed accept path,
    so a later edit that silently reopens one is caught here rather than at acceptance.
    """
    out = validate_ledger(rows, allow_open=True)
    by_id = {r["id"]: r for r in rows}

    for required in ("KR-01", "KR-05"):
        row = by_id.get(required)
        if row is None:
            out.append(
                Rejection(
                    "F28L-015",
                    required,
                    "the contract names this as an A2 crossing; it is absent from the "
                    "carried-red ledger",
                )
            )
            continue
        if row["contradicted_criterion"] in ("", NONE):
            out.append(
                Rejection(
                    "F28L-015",
                    required,
                    "named by the contract as contradicting a Success Criterion, but its "
                    "contradicted_criterion is none",
                )
            )
        if row["p28_severity"] not in BLOCKING_SEVERITIES:
            out.append(
                Rejection(
                    "F28L-015",
                    required,
                    f"A2 crossing scored {row['p28_severity']!r}; must be CRITICAL or HIGH "
                    "by construction",
                )
            )
    return out


# --------------------------------------------------------------------------------------
# Matrix validation  (codes F28M-*)
# --------------------------------------------------------------------------------------

MATRIX_FIELDS = (
    "cell_id",
    "dimension",
    "os",
    "surface",
    "criticality",
    "applicability",
    "skip_class",
    "skip_evidence",
    "activeness",
)


def parse_matrix(text: str) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for lineno, raw in enumerate(text.splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        cells = raw.split("\t")
        row: dict[str, str] = {"_line": str(lineno), "_ncells": str(len(cells))}
        for i, name in enumerate(MATRIX_FIELDS):
            row[name] = cells[i].strip() if i < len(cells) else ""
        rows.append(row)
    return rows


def validate_matrix(rows: list[dict[str, str]]) -> list[Rejection]:
    out: list[Rejection] = []
    seen_dim_os: set[tuple[str, str]] = set()
    by_id: dict[str, dict[str, str]] = {}

    for row in rows:
        where = f"{row.get('cell_id') or '<no id>'} (line {row['_line']})"
        if int(row["_ncells"]) < len(MATRIX_FIELDS):
            out.append(
                Rejection(
                    "F28M-008",
                    where,
                    f"row has {row['_ncells']} fields, expected {len(MATRIX_FIELDS)}",
                )
            )
            continue
        by_id[row["cell_id"]] = row

        dim, os_family = row["dimension"], row["os"]
        if dim not in DIMENSIONS:
            out.append(
                Rejection(
                    "F28M-009",
                    where,
                    f"dimension {dim!r} is not one of the nine fixed F28-01 dimensions",
                )
            )
        if os_family not in OS_FAMILIES:
            out.append(Rejection("F28M-010", where, f"os {os_family!r} is not an OS family"))
        seen_dim_os.add((dim, os_family))

        critical = row["criticality"] == "critical"
        skipped = row["applicability"] == "skipped"
        klass = row["skip_class"]

        if row["applicability"] not in ("applicable", "skipped"):
            out.append(
                Rejection(
                    "F28M-011",
                    where,
                    f"applicability {row['applicability']!r} must be applicable or skipped",
                )
            )

        if skipped:
            if klass in ("", NONE):
                out.append(
                    Rejection("F28M-001", where, "skip carries no class")
                )
            elif klass not in SKIP_CLASSES:
                out.append(
                    Rejection(
                        "F28M-003",
                        where,
                        f"skip class {klass!r} is not one of the four legal classes; "
                        "no fifth class may be added mid-run",
                    )
                )
            if row["skip_evidence"] in ("", NONE):
                out.append(
                    Rejection(
                        "F28M-012",
                        where,
                        f"skip class {klass!r} carries no evidence",
                    )
                )
            if critical:
                out.append(
                    Rejection(
                        "F28M-002",
                        where,
                        "critical cell carries a skip; a critical cell has NO legal skip "
                        "and a critical cell that cannot be run is a RED",
                    )
                )
        else:
            if klass not in ("", NONE):
                out.append(
                    Rejection(
                        "F28M-013",
                        where,
                        f"applicable cell carries skip class {klass!r}",
                    )
                )

        # Sandbox activeness is required on every sandbox-dimension cell, not only the
        # mandatory one. Absence of an observed violation is not a pass.
        if dim == SANDBOX_DIMENSION:
            if row["activeness"] != "required":
                out.append(
                    Rejection(
                        "F28M-006",
                        where,
                        "sandbox-dimension cell does not carry activeness=required; a "
                        "green here needs POSITIVE evidence the sandbox was active",
                    )
                )
        elif row["activeness"] not in ("n/a", "required"):
            out.append(
                Rejection(
                    "F28M-014",
                    where,
                    f"activeness {row['activeness']!r} must be n/a on a non-sandbox cell",
                )
            )

    # Every dimension on every OS family, or the matrix silently lost a platform.
    for dim in DIMENSIONS:
        for os_family in OS_FAMILIES:
            if (dim, os_family) not in seen_dim_os:
                out.append(
                    Rejection(
                        "F28M-005",
                        f"{dim}/{os_family}",
                        "dimension absent from OS family with no applicability record",
                    )
                )

    for cell_id in MANDATORY_CELLS:
        row = by_id.get(cell_id)
        if row is None:
            out.append(Rejection("F28M-007", cell_id, "mandatory cell absent"))
            continue
        if row["criticality"] != "critical":
            out.append(
                Rejection(
                    "F28M-007", cell_id, "mandatory cell is not marked critical"
                )
            )
        if row["applicability"] == "skipped":
            out.append(Rejection("F28M-007", cell_id, "mandatory cell is skipped"))

    return out


def check_no_uncontrolled_skips(rows: list[dict[str, str]]) -> list[Rejection]:
    """The observation-blocked evidence gate — the highest-value laundering channel.

    Requires a run-time control reference AND rejects any documentary citation, including
    citations of documents that report in the product's favour.
    """
    out: list[Rejection] = []
    for row in rows:
        if row.get("skip_class") != "observation-blocked":
            continue
        where = f"{row.get('cell_id')} (line {row['_line']})"
        evidence = row.get("skip_evidence", "")
        low = evidence.lower()
        hit = next((p for p in LORE_PATTERNS if p in low), None)
        if hit is not None:
            out.append(
                Rejection(
                    "F28M-004",
                    where,
                    f"observation-blocked evidence {evidence!r} cites a document "
                    f"(matched {hit!r}); only a control measured in the certification "
                    "environment at run time is acceptable evidence",
                )
            )
        elif not CONTROL_REF.match(evidence):
            out.append(
                Rejection(
                    "F28M-004",
                    where,
                    f"observation-blocked evidence {evidence!r} is not a run-time control "
                    "reference of the form control:<id>@<host>:<run-id>",
                )
            )
    return out


# --------------------------------------------------------------------------------------
# Contract / source cross-check  (codes F28C-*)
# --------------------------------------------------------------------------------------


def extract_anchors(repo: Path) -> list[tuple[str, str]]:
    """Pull the quoted authorities out of their ORIGINAL sources.

    This is what stops the contract drifting into paraphrase: the strings are read from
    ROADMAP.md, REQUIREMENTS.md and decision-rationale.txt, never from the contract.
    Fails closed (SystemExit 2) when a source or an expected passage is missing, because
    an anchor set that silently shrank would make the check pass vacuously.
    """
    anchors: list[tuple[str, str]] = []

    roadmap = repo / ".planning" / "ROADMAP.md"
    reqs = repo / ".planning" / "REQUIREMENTS.md"
    rationale = (
        repo
        / ".planning"
        / "phases"
        / "28-native-cross-platform-certification"
        / "28-01-decision-evidence"
        / "decision-rationale.txt"
    )
    for path in (roadmap, reqs, rationale):
        if not path.is_file():
            sys.exit(f"F28C-000  anchor source missing: {path}")

    # --- Phase 28's four Success Criteria, from the ROADMAP's Phase 28 section ----------
    rm = roadmap.read_text(encoding="utf-8")
    m = re.search(
        r"^### Phase 28:.*?^\*\*Success Criteria\*\*[^\n]*\n(.*?)(?=^\*\*|^### )",
        rm,
        re.S | re.M,
    )
    if not m:
        sys.exit("F28C-000  could not locate Phase 28 Success Criteria in ROADMAP.md")
    criteria = re.findall(r"^\s*(\d)\.\s+(.+)$", m.group(1), re.M)
    if len(criteria) != 4:
        sys.exit(
            f"F28C-000  expected 4 Phase 28 Success Criteria in ROADMAP.md, found "
            f"{len(criteria)}"
        )
    for num, text in criteria:
        anchors.append((f"ROADMAP Success Criterion {num}", text.strip()))

    # --- The standing severity policy, from the ROADMAP constraints ---------------------
    policy = re.search(
        r"(Findings at CRITICAL or HIGH must be fixed or disproved.*?"
        r"it escalates to Sean\.)",
        rm,
        re.S,
    )
    if not policy:
        sys.exit("F28C-000  could not locate the standing severity policy in ROADMAP.md")
    anchors.append(("standing severity policy", policy.group(1)))

    # --- F28-01 .. F28-04, from REQUIREMENTS.md -----------------------------------------
    rq = reqs.read_text(encoding="utf-8")
    found = re.findall(r"^\s*-\s*\[[ x]\]\s*(\*\*F28-0\d\*\*:\s*.+)$", rq, re.M)
    if len(found) != 4:
        sys.exit(
            f"F28C-000  expected F28-01..F28-04 in REQUIREMENTS.md, found {len(found)}"
        )
    for text in found:
        label = re.match(r"\*\*(F28-0\d)\*\*", text).group(1)
        anchors.append((f"requirement {label}", text.strip()))

    # --- The adopted rule and the three amendments, from decision-rationale.txt ---------
    dr = rationale.read_text(encoding="utf-8")
    rule = re.search(
        r'("Resolved" is implemented as DISPOSITIONED.*?'
        r"the gate must fail closed rather than warn\.)",
        dr,
        re.S,
    )
    if not rule:
        sys.exit("F28C-000  could not locate the adopted acceptance rule in the rationale")
    anchors.append(("adopted acceptance rule", rule.group(1)))

    for label, pattern in (
        ("amendment A1", r"(A1\. SEVERITY IS RE-SCORED.*?requires no launderer\.)"),
        ("amendment A2", r"(A2\. A FINDING THAT CONTRADICTS.*?thing being certified\.)"),
        ("amendment A3", r"(A3\. THE RECEIPT MAY NOT CLAIM.*?REJECT a receipt that asserts either\.)"),
    ):
        mm = re.search(pattern, dr, re.S)
        if not mm:
            sys.exit(f"F28C-000  could not locate {label} in the rationale")
        anchors.append((label, mm.group(1)))

    return anchors


def check_contract_text(contract: str, anchors: list[tuple[str, str]]) -> list[Rejection]:
    """Assert every anchor appears in the contract, modulo line wrapping only."""
    haystack = _norm(contract)
    out: list[Rejection] = []
    for label, text in anchors:
        if _norm(text) not in haystack:
            out.append(
                Rejection(
                    "F28C-001",
                    label,
                    "not quoted verbatim in the contract (a paraphrase does not satisfy "
                    f"this); expected: {_norm(text)[:110]}...",
                )
            )
    return out


# --------------------------------------------------------------------------------------
# Self-test — every code tripped AND not tripped
# --------------------------------------------------------------------------------------

_LEDGER_HEADER = "#" + "\t".join(LEDGER_FIELDS) + "\n"
_MATRIX_HEADER = "#" + "\t".join(MATRIX_FIELDS) + "\n"


def _ledger_row(**over: str) -> str:
    base = {
        "id": "T-1",
        "subject": "a subject",
        "inherited_severity": "known-red/non-gating",
        "p28_severity": "MEDIUM",
        "contradicted_criterion": NONE,
        "available_dispositions": "FIXED,DISPROVED,ACCEPTED,DEFERRED",
        "disposition": "FIXED",
        "rationale": "because",
        "owner": "core",
        "backlog_id": "BL-1",
        "executable_check": "cargo test x",
        "counter_evidence": "log.txt",
        "origin": "matrix",
        "downgrade_review": "",
    }
    base.update(over)
    names = list(LEDGER_FIELDS) + list(LEDGER_OPTIONAL_FIELDS)
    return "\t".join(base[n] for n in names) + "\n"


def _matrix_row(**over: str) -> str:
    base = {
        "cell_id": "c-1",
        "dimension": "unicode",
        "os": "linux",
        "surface": "s-root",
        "criticality": "standard",
        "applicability": "applicable",
        "skip_class": NONE,
        "skip_evidence": NONE,
        "activeness": "n/a",
    }
    base.update(over)
    return "\t".join(base[n] for n in MATRIX_FIELDS) + "\n"


def _full_matrix(extra: str = "", *, drop: tuple[str, str] | None = None) -> str:
    """A structurally complete matrix: nine dimensions on three OS families, plus the
    three mandatory cells. `drop` removes one (dimension, os) pair so the missing-cell
    rejection can be tripped."""
    body = _MATRIX_HEADER
    for dim in DIMENSIONS:
        for os_family in OS_FAMILIES:
            if drop == (dim, os_family):
                continue
            body += _matrix_row(
                cell_id=f"c-{dim}-{os_family}",
                dimension=dim,
                os=os_family,
                activeness="required" if dim == SANDBOX_DIMENSION else "n/a",
            )
    body += _matrix_row(
        cell_id="w-sandbox-silent-disable",
        dimension=SANDBOX_DIMENSION,
        os="windows",
        criticality="critical",
        activeness="required",
    )
    body += _matrix_row(
        cell_id="w-process-cleanup-descendant-tree",
        dimension="process-cleanup",
        os="windows",
        criticality="critical",
    )
    body += _matrix_row(
        cell_id="w-sandbox-observability-control",
        dimension=SANDBOX_DIMENSION,
        os="windows",
        criticality="critical",
        activeness="required",
    )
    return body + extra


def _codes(rejections: list[Rejection]) -> set[str]:
    return {r.code for r in rejections}


def self_test() -> int:
    """Each case names a code, supplies input that MUST trip it, and input that MUST NOT.

    Testing one direction only is how a validator ends up either vacuous or
    indiscriminate. Both are checked for every code.
    """
    failures: list[str] = []

    def case(
        code: str,
        name: str,
        bad: list[Rejection],
        good: list[Rejection],
    ) -> None:
        if code not in _codes(bad):
            failures.append(
                f"{code} [{name}]: the tripping fixture did NOT produce the code "
                f"(got {sorted(_codes(bad)) or 'no rejections'})"
            )
        if code in _codes(good):
            failures.append(
                f"{code} [{name}]: the clean fixture DID produce the code — the rule "
                "over-fires"
            )

    L = lambda text, allow_open=False: validate_ledger(parse_ledger(text), allow_open=allow_open)  # noqa: E731
    M = lambda text: validate_matrix(parse_matrix(text))  # noqa: E731
    S = lambda text: check_no_uncontrolled_skips(parse_matrix(text))  # noqa: E731

    clean_ledger = _LEDGER_HEADER + _ledger_row()

    # -- F28L-001 accept forbidden at CRITICAL/HIGH -------------------------------------
    case(
        "F28L-001",
        "ACCEPTED at HIGH",
        L(
            _LEDGER_HEADER
            + _ledger_row(
                p28_severity="HIGH", disposition="ACCEPTED", available_dispositions="ACCEPTED"
            )
        ),
        L(_LEDGER_HEADER + _ledger_row(p28_severity="HIGH", disposition="FIXED",
                                       available_dispositions="FIXED,DISPROVED")),
    )
    case(
        "F28L-001",
        "DEFERRED at CRITICAL",
        L(
            _LEDGER_HEADER
            + _ledger_row(
                p28_severity="CRITICAL",
                contradicted_criterion="1",
                disposition="DEFERRED",
                available_dispositions="DEFERRED",
            )
        ),
        L(_LEDGER_HEADER + _ledger_row(disposition="ACCEPTED")),  # MEDIUM: legal
    )

    # -- F28L-002 no disposition at all -------------------------------------------------
    case(
        "F28L-002",
        "missing disposition",
        L(_LEDGER_HEADER + _ledger_row(disposition="")),
        L(clean_ledger),
    )
    case(
        "F28L-002",
        "OPEN rejected at acceptance, permitted for the carried-red ledger",
        L(_LEDGER_HEADER + _ledger_row(disposition=OPEN), allow_open=False),
        L(_LEDGER_HEADER + _ledger_row(disposition=OPEN), allow_open=True),
    )

    # -- F28L-003/4/5 paper-path evidence ------------------------------------------------
    for code, field in (
        ("F28L-003", "rationale"),
        ("F28L-004", "owner"),
        ("F28L-005", "backlog_id"),
    ):
        case(
            code,
            f"ACCEPTED missing {field}",
            L(_LEDGER_HEADER + _ledger_row(disposition="ACCEPTED", **{field: ""})),
            L(_LEDGER_HEADER + _ledger_row(disposition="ACCEPTED")),
        )

    # -- F28L-006 A1 re-scoring ----------------------------------------------------------
    case(
        "F28L-006",
        "no Phase 28 re-score",
        L(_LEDGER_HEADER + _ledger_row(p28_severity="")),
        L(clean_ledger),
    )
    case(
        "F28L-006",
        "no inherited provenance",
        L(_LEDGER_HEADER + _ledger_row(inherited_severity="")),
        L(clean_ledger),
    )

    # -- F28L-007 A2 ---------------------------------------------------------------------
    case(
        "F28L-007",
        "ACCEPTED while contradicting a criterion, even at MEDIUM",
        L(
            _LEDGER_HEADER
            + _ledger_row(
                p28_severity="MEDIUM", contradicted_criterion="2", disposition="ACCEPTED"
            )
        ),
        L(_LEDGER_HEADER + _ledger_row(p28_severity="MEDIUM", disposition="ACCEPTED")),
    )
    case(
        "F28L-007",
        "accept path offered while contradicting a criterion",
        L(
            _LEDGER_HEADER
            + _ledger_row(
                p28_severity="HIGH",
                contradicted_criterion="2",
                available_dispositions="FIXED,DISPROVED,ACCEPTED",
                disposition="FIXED",
            )
        ),
        L(
            _LEDGER_HEADER
            + _ledger_row(
                p28_severity="HIGH",
                contradicted_criterion="2",
                available_dispositions="FIXED,DISPROVED",
                disposition="FIXED",
            )
        ),
    )

    # -- F28L-008 / F28L-009 repair-path evidence -----------------------------------------
    case(
        "F28L-008",
        "FIXED with no executable check",
        L(_LEDGER_HEADER + _ledger_row(disposition="FIXED", executable_check="")),
        L(clean_ledger),
    )
    case(
        "F28L-009",
        "DISPROVED with no counter-evidence",
        L(_LEDGER_HEADER + _ledger_row(disposition="DISPROVED", counter_evidence="")),
        L(_LEDGER_HEADER + _ledger_row(disposition="DISPROVED")),
    )

    # -- F28L-010 / 011 / 012 / 013 / 014 vocabulary and shape ----------------------------
    case(
        "F28L-010",
        "unknown disposition",
        L(_LEDGER_HEADER + _ledger_row(disposition="WONTFIX")),
        L(clean_ledger),
    )
    case(
        "F28L-011",
        "unknown severity",
        L(_LEDGER_HEADER + _ledger_row(p28_severity="SEVERE")),
        L(clean_ledger),
    )
    case(
        "F28L-012",
        "short row",
        L(_LEDGER_HEADER + "KR-x\tonly two fields\n"),
        L(clean_ledger),
    )
    case(
        "F28L-013",
        "blank contradicted_criterion",
        L(_LEDGER_HEADER + _ledger_row(contradicted_criterion="")),
        L(clean_ledger),
    )
    case(
        "F28L-014",
        "criterion out of range",
        L(_LEDGER_HEADER + _ledger_row(contradicted_criterion="7")),
        L(_LEDGER_HEADER + _ledger_row(contradicted_criterion="4",
                                       p28_severity="HIGH",
                                       available_dispositions="FIXED,DISPROVED")),
    )

    # -- F28L-015 the named A2 crossings --------------------------------------------------
    a2_rows = (
        _LEDGER_HEADER
        + _ledger_row(
            id="KR-01",
            p28_severity="HIGH",
            contradicted_criterion="2",
            available_dispositions="FIXED,DISPROVED",
            disposition=OPEN,
        )
        + _ledger_row(
            id="KR-05",
            p28_severity="CRITICAL",
            contradicted_criterion="1",
            available_dispositions="FIXED,DISPROVED",
            disposition=OPEN,
        )
    )
    case(
        "F28L-015",
        "a named A2 crossing silently lost its contradicted criterion",
        check_rescoring(
            parse_ledger(
                a2_rows.replace("KR-01\ta subject\tknown-red/non-gating\tHIGH\t2",
                                "KR-01\ta subject\tknown-red/non-gating\tHIGH\t-")
            )
        ),
        check_rescoring(parse_ledger(a2_rows)),
    )
    case(
        "F28L-015",
        "a named A2 crossing is absent entirely",
        check_rescoring(parse_ledger(_LEDGER_HEADER + _ledger_row(id="KR-99"))),
        check_rescoring(parse_ledger(a2_rows)),
    )

    # -- F28M-001 unclassified skip --------------------------------------------------------
    case(
        "F28M-001",
        "skip with no class",
        M(_full_matrix(_matrix_row(cell_id="x", applicability="skipped",
                                   skip_evidence="something"))),
        M(_full_matrix(_matrix_row(cell_id="x", applicability="skipped",
                                   skip_class="platform-inapplicability",
                                   skip_evidence="no UNC on this family"))),
    )

    # -- F28M-002 critical cell skipped ----------------------------------------------------
    case(
        "F28M-002",
        "critical cell carries a skip",
        M(_full_matrix(_matrix_row(cell_id="x", criticality="critical",
                                   applicability="skipped",
                                   skip_class="architectural-impossibility",
                                   skip_evidence="check_x"))),
        M(_full_matrix(_matrix_row(cell_id="x", criticality="critical"))),
    )

    # -- F28M-003 fifth class --------------------------------------------------------------
    case(
        "F28M-003",
        "invented fifth skip class",
        M(_full_matrix(_matrix_row(cell_id="x", applicability="skipped",
                                   skip_class="too-slow", skip_evidence="e"))),
        M(_full_matrix(_matrix_row(cell_id="x", applicability="skipped",
                                   skip_class="unresolved-surface",
                                   skip_evidence="phase=26 req_disposition=open"))),
    )

    # -- F28M-004 observation-blocked without a run-time control ----------------------------
    ob = lambda ev: _full_matrix(  # noqa: E731
        _matrix_row(cell_id="x", applicability="skipped",
                    skip_class="observation-blocked", skip_evidence=ev)
    )
    case(
        "F28M-004",
        "citing a handoff document",
        S(ob("see HANDOFF-2026-07-26-autonomous-execution.md section 5")),
        S(ob("control:ssh-channel@seandesktop:run-4411")),
    )
    case(
        "F28M-004",
        "citing intel that reports in the product's FAVOUR",
        S(ob(".planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md")),
        S(ob("control:appc-observe@seandesktop:30184651330")),
    )
    case(
        "F28M-004",
        "prose with no control reference at all",
        S(ob("the channel was clearly broken")),
        S(ob("control:lease-dir-empty@ferrox-win-msvc:7788")),
    )
    case(
        "F28M-004",
        "other skip classes are not subject to the control requirement",
        S(ob("previously established")),
        S(
            _full_matrix(
                _matrix_row(cell_id="x", applicability="skipped",
                            skip_class="platform-inapplicability",
                            skip_evidence="see docs/platform.md — UNC is Windows-only")
            )
        ),
    )

    # -- F28M-005 dimension missing from an OS family ---------------------------------------
    case(
        "F28M-005",
        "a dimension silently loses a platform",
        M(_full_matrix(drop=("offline", "macos"))),
        M(_full_matrix()),
    )

    # -- F28M-006 sandbox activeness ---------------------------------------------------------
    case(
        "F28M-006",
        "sandbox cell without an activeness requirement",
        M(_full_matrix(_matrix_row(cell_id="x", dimension=SANDBOX_DIMENSION,
                                   os="linux", activeness="n/a"))),
        M(_full_matrix(_matrix_row(cell_id="x", dimension=SANDBOX_DIMENSION,
                                   os="linux", activeness="required"))),
    )

    # -- F28M-007 mandatory cells --------------------------------------------------------------
    for cell in MANDATORY_CELLS:
        text = _full_matrix()
        stripped = "".join(l + "\n" for l in text.splitlines() if not l.startswith(cell + "\t"))
        case("F28M-007", f"mandatory cell {cell} absent", M(stripped), M(text))
    downgraded = _full_matrix().replace(
        "w-sandbox-silent-disable\tsandbox-probes\twindows\ts-root\tcritical",
        "w-sandbox-silent-disable\tsandbox-probes\twindows\ts-root\tstandard",
    )
    case("F28M-007", "mandatory cell downgraded from critical", M(downgraded), M(_full_matrix()))

    # -- F28M-008..014 shape and vocabulary ------------------------------------------------------
    case("F28M-008", "short matrix row", M(_full_matrix("x\ty\n")), M(_full_matrix()))
    case(
        "F28M-009",
        "renamed dimension",
        M(_full_matrix(_matrix_row(cell_id="x", dimension="unicode-handling"))),
        M(_full_matrix()),
    )
    case(
        "F28M-010",
        "unknown OS family",
        M(_full_matrix(_matrix_row(cell_id="x", os="freebsd"))),
        M(_full_matrix()),
    )
    case(
        "F28M-011",
        "unknown applicability",
        M(_full_matrix(_matrix_row(cell_id="x", applicability="maybe"))),
        M(_full_matrix()),
    )
    case(
        "F28M-012",
        "classified skip with no evidence",
        M(_full_matrix(_matrix_row(cell_id="x", applicability="skipped",
                                   skip_class="unresolved-surface"))),
        M(_full_matrix(_matrix_row(cell_id="x", applicability="skipped",
                                   skip_class="unresolved-surface",
                                   skip_evidence="phase=27 req_disposition=deferred"))),
    )
    case(
        "F28M-013",
        "applicable cell carrying a skip class",
        M(_full_matrix(_matrix_row(cell_id="x", skip_class="platform-inapplicability"))),
        M(_full_matrix()),
    )
    case(
        "F28M-014",
        "non-sandbox cell with a bogus activeness value",
        M(_full_matrix(_matrix_row(cell_id="x", activeness="probably"))),
        M(_full_matrix()),
    )

    # -- F28C-001 contract / source cross-check -----------------------------------------------
    anchors = [("criterion X", "Native macOS, Linux, and Windows pass the required matrix.")]
    case(
        "F28C-001",
        "the contract paraphrases an authority instead of quoting it",
        check_contract_text("macOS, Linux and Windows pass the matrix.", anchors),
        check_contract_text(
            "Quoting:\n> Native macOS, Linux, and Windows\n> pass the required matrix.\n",
            anchors,
        ),
    )

    # -- The real carried-red ledger must satisfy its own rules --------------------------------
    print(f"self-test: {len(failures)} failure(s)")
    for f in failures:
        print(f"  FAIL {f}")
    if failures:
        return 1
    print("self-test: every rejection code was tripped by a bad fixture and NOT tripped")
    print("           by a good one (both directions checked for each).")
    return 0


# --------------------------------------------------------------------------------------
# Plan 28-04 acceptance checks (codes F28A-*)
#
# `--check-ledger` above proves a ledger obeys the contract's rules. These four prove the
# things a ledger cannot prove about ITSELF:
#   completeness  — against the upstream artifacts, in the direction that matters;
#   A2            — that the named crossings are still crossings and still closed;
#   downgrades    — that the one re-score direction with an incentive carries a review;
#   backlog ids   — that a cited id is real rather than aspirational.
# --------------------------------------------------------------------------------------

SEVERITY_RANK = {"LOW": 1, "MEDIUM": 2, "HIGH": 3, "CRITICAL": 4}

# The A2 crossings the 28-01 contract names. A ledger that drops one, or reopens its accept
# path, has silently become c4-standing-with-paperwork — which is exactly what the dissent
# warned a later reader to check for.
NAMED_A2_CROSSINGS = ("KR-01", "KR-05")


def _ids_from_json(value, out: set[str]) -> None:
    """Collect every `id` that sits next to a finding-shaped record, at any depth.

    Deliberately structural rather than keyed to one schema: the four upstream artifacts
    have four different shapes, and a collector that only understood one of them would
    report completeness while never having looked at the others.
    """
    if isinstance(value, dict):
        if "id" in value and isinstance(value["id"], str):
            siblings = set(value) & {
                "p28_severity",
                "disposition",
                "contradicted_criterion",
                "inherited_severity",
                "severity",
            }
            if siblings:
                out.add(value["id"])
        for v in value.values():
            _ids_from_json(v, out)
    elif isinstance(value, list):
        for v in value:
            _ids_from_json(v, out)


def upstream_finding_ids(path: Path) -> set[str]:
    text = path.read_text(encoding="utf-8")
    out: set[str] = set()
    if path.suffix == ".json":
        import json

        _ids_from_json(json.loads(text), out)
    else:
        for row in parse_ledger(text):
            if row.get("id"):
                out.add(row["id"])
    return out


def check_completeness(
    rows: list[dict[str, str]], upstream: list[tuple[Path, set[str]]]
) -> list[Rejection]:
    """Upstream -> ledger, and ONLY that direction.

    A finding present in an upstream artifact and absent from the ledger is the failure
    that makes the whole acceptance exercise pointless, so it is validated before anything
    else. The reverse direction is deliberately NOT an error: plan 28-04 raises findings of
    its own, and a ledger richer than its sources is not a defect.
    """
    have = {r.get("id", "") for r in rows}
    out: list[Rejection] = []
    for path, ids in upstream:
        for fid in sorted(ids):
            if fid not in have:
                out.append(
                    Rejection(
                        "F28A-001",
                        fid,
                        f"present in {path.name} and ABSENT from the adjudicated ledger",
                    )
                )
    return out


def check_a2(rows: list[dict[str, str]]) -> list[Rejection]:
    out: list[Rejection] = []
    by_id = {r.get("id", ""): r for r in rows}
    for row in rows:
        where = f"{row.get('id') or '<no id>'} (line {row['_line']})"
        contradicted = row.get("contradicted_criterion", "")
        contradicts = contradicted not in ("", NONE)
        if not contradicts:
            continue
        if row.get("disposition") in PAPER_DISPOSITIONS:
            out.append(
                Rejection(
                    "F28A-002",
                    where,
                    f"{row['disposition']} while contradicting Success Criterion "
                    f"{contradicted}; A2 closes accept and defer by construction",
                )
            )
        if row.get("p28_severity") not in BLOCKING_SEVERITIES:
            out.append(
                Rejection(
                    "F28A-003",
                    where,
                    f"contradicts Criterion {contradicted} but is scored "
                    f"{row.get('p28_severity')!r}; a criterion-contradicting finding is "
                    "CRITICAL or HIGH BY CONSTRUCTION",
                )
            )
        if any(d in PAPER_DISPOSITIONS for d in row.get("available_dispositions", "").split(",")):
            out.append(
                Rejection(
                    "F28A-002",
                    where,
                    "available_dispositions offers a paper path while contradicting "
                    f"Criterion {contradicted}",
                )
            )
    for required in NAMED_A2_CROSSINGS:
        row = by_id.get(required)
        if row is None:
            out.append(
                Rejection(
                    "F28A-004",
                    required,
                    "the 28-01 contract names this as an A2 crossing and it is absent "
                    "from the adjudicated ledger",
                )
            )
            continue
        if row.get("contradicted_criterion") in ("", NONE):
            out.append(
                Rejection(
                    "F28A-004",
                    required,
                    "named by the contract as contradicting a Success Criterion, but its "
                    "contradicted_criterion is none — A2 has been dropped",
                )
            )
    return out


def check_downgrades(rows: list[dict[str, str]]) -> list[Rejection]:
    """A re-score DOWNWARD is the one move that opens a previously closed disposition.

    An upward re-score needs no ceremony, so the ceremony sits exactly where the incentive
    is. A downgrade must cite a recorded independent review; the comparison only runs when
    the inherited label names one of the four bands, because a label like
    `known-red/non-gating` has no rank to compare against and A1 already forbids using it.
    """
    out: list[Rejection] = []
    for row in rows:
        where = f"{row.get('id') or '<no id>'} (line {row['_line']})"
        inherited = row.get("inherited_severity", "").strip().upper()
        inherited_rank = SEVERITY_RANK.get(inherited)
        current_rank = SEVERITY_RANK.get(row.get("p28_severity", "").strip().upper())
        if inherited_rank is None or current_rank is None:
            continue
        if current_rank < inherited_rank and not row.get("downgrade_review", "").strip():
            out.append(
                Rejection(
                    "F28A-005",
                    where,
                    f"re-scored DOWN from {inherited} to {row['p28_severity']} with no "
                    "recorded independent review; a downgrade is the one direction that "
                    "opens a closed disposition",
                )
            )
    return out


def check_backlog_ids(rows: list[dict[str, str]], backlog_text: str) -> list[Rejection]:
    out: list[Rejection] = []
    for row in rows:
        where = f"{row.get('id') or '<no id>'} (line {row['_line']})"
        if row.get("disposition") not in PAPER_DISPOSITIONS:
            continue
        bid = row.get("backlog_id", "").strip()
        if not bid:
            out.append(Rejection("F28A-006", where, "paper disposition with no backlog id"))
            continue
        if bid not in backlog_text:
            out.append(
                Rejection(
                    "F28A-007",
                    where,
                    f"cites backlog id {bid!r}, which does not appear in BACKLOG.md; "
                    "the id must be real rather than aspirational",
                )
            )
        if not row.get("owner", "").strip():
            out.append(Rejection("F28A-008", where, "paper disposition with no named owner"))
    return out


def acceptance_self_test() -> list[str]:
    """Both directions for every F28A code. Returns the list of failures."""
    failures: list[str] = []

    def case(code: str, label: str, bad, good):
        bad_codes = {r.code for r in bad}
        good_codes = {r.code for r in good}
        if code not in bad_codes:
            failures.append(f"{code} ({label}): the BAD fixture did NOT trip it")
        if code in good_codes:
            failures.append(f"{code} ({label}): the GOOD fixture DID trip it")

    def L(**over) -> list[dict[str, str]]:
        base = dict(
            id="F-X",
            subject="s",
            inherited_severity="-",
            p28_severity="MEDIUM",
            contradicted_criterion="-",
            available_dispositions="FIXED,DISPROVED,ACCEPTED,DEFERRED",
            disposition="DEFERRED",
            rationale="r",
            owner="an owner",
            backlog_id="BL-REAL",
            executable_check="",
            counter_evidence="",
            origin="matrix",
            downgrade_review="",
            _line="1",
            _ncells="14",
        )
        base.update(over)
        return [base]

    tmp = Path("/dev/null")
    case(
        "F28A-001",
        "an upstream finding never reaches the ledger",
        check_completeness(L(), [(tmp, {"F-MISSING"})]),
        check_completeness(L(), [(tmp, {"F-X"})]),
    )
    case(
        "F28A-002",
        "a criterion-contradicting finding takes the accept path",
        check_a2(
            L(id="KR-01", contradicted_criterion="2", p28_severity="HIGH",
              available_dispositions="FIXED,DISPROVED", disposition="ACCEPTED")
            + L(id="KR-05", contradicted_criterion="1", p28_severity="CRITICAL",
                available_dispositions="FIXED,DISPROVED", disposition="FIXED")
        ),
        check_a2(
            L(id="KR-01", contradicted_criterion="2", p28_severity="HIGH",
              available_dispositions="FIXED,DISPROVED", disposition="DISPROVED")
            + L(id="KR-05", contradicted_criterion="1", p28_severity="CRITICAL",
                available_dispositions="FIXED,DISPROVED", disposition="FIXED")
        ),
    )
    case(
        "F28A-003",
        "a criterion-contradicting finding scored below HIGH",
        check_a2(
            L(id="KR-01", contradicted_criterion="2", p28_severity="MEDIUM",
              available_dispositions="FIXED,DISPROVED", disposition="FIXED")
            + L(id="KR-05", contradicted_criterion="1", p28_severity="CRITICAL",
                available_dispositions="FIXED,DISPROVED", disposition="FIXED")
        ),
        check_a2(
            L(id="KR-01", contradicted_criterion="2", p28_severity="HIGH",
              available_dispositions="FIXED,DISPROVED", disposition="FIXED")
            + L(id="KR-05", contradicted_criterion="1", p28_severity="CRITICAL",
                available_dispositions="FIXED,DISPROVED", disposition="FIXED")
        ),
    )
    case(
        "F28A-004",
        "a named A2 crossing is missing from the ledger",
        check_a2(L(id="KR-01", contradicted_criterion="2", p28_severity="HIGH",
                   available_dispositions="FIXED,DISPROVED", disposition="FIXED")),
        check_a2(
            L(id="KR-01", contradicted_criterion="2", p28_severity="HIGH",
              available_dispositions="FIXED,DISPROVED", disposition="FIXED")
            + L(id="KR-05", contradicted_criterion="1", p28_severity="CRITICAL",
                available_dispositions="FIXED,DISPROVED", disposition="FIXED")
        ),
    )
    case(
        "F28A-005",
        "a downgrade with no recorded independent review",
        check_downgrades(L(inherited_severity="HIGH", p28_severity="MEDIUM")),
        check_downgrades(
            L(inherited_severity="HIGH", p28_severity="MEDIUM",
              downgrade_review="4-way panel, recorded at ...")
        ),
    )
    # An UPWARD re-score must NOT need a review — the ceremony belongs only where the
    # incentive is, and a rule that fired both ways would just be noise.
    if check_downgrades(L(inherited_severity="LOW", p28_severity="CRITICAL")):
        failures.append("F28A-005: an UPWARD re-score wrongly required a review")
    case(
        "F28A-006",
        "a paper disposition with no backlog id",
        check_backlog_ids(L(backlog_id=""), "BL-REAL"),
        check_backlog_ids(L(), "BL-REAL"),
    )
    case(
        "F28A-007",
        "a backlog id that does not exist in BACKLOG.md",
        check_backlog_ids(L(backlog_id="BL-INVENTED"), "BL-REAL"),
        check_backlog_ids(L(), "BL-REAL"),
    )
    case(
        "F28A-008",
        "a paper disposition with no named owner",
        check_backlog_ids(L(owner=""), "BL-REAL"),
        check_backlog_ids(L(), "BL-REAL"),
    )
    # A FIXED/DISPROVED row needs neither owner nor backlog id, so the backlog rule must
    # not fire on one.
    if check_backlog_ids(L(disposition="FIXED", owner="", backlog_id=""), "BL-REAL"):
        failures.append("F28A-006/7/8: fired on a repair-path row, which carries no backlog id")
    return failures


# --------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------


def _repo_root(start: Path) -> Path:
    for candidate in [start, *start.parents]:
        if (candidate / ".planning").is_dir() and (candidate / ".git").exists():
            return candidate
    return start


def _report(name: str, rejections: list[Rejection]) -> int:
    if rejections:
        print(f"{name}: REJECTED ({len(rejections)})")
        for r in rejections:
            print(f"  {r}")
        return 1
    print(f"{name}: OK")
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--self-test", action="store_true")
    g.add_argument("--check-contract", metavar="CONTRACT.md")
    g.add_argument("--check-rescoring", metavar="known-red.tsv")
    g.add_argument("--check-ledger", metavar="ledger.tsv")
    g.add_argument("--check-matrix", metavar="matrix.tsv")
    g.add_argument("--check-no-uncontrolled-skips", metavar="matrix.tsv")
    # Plan 28-04 acceptance gates.
    g.add_argument("--validate", metavar="findings.tsv")
    g.add_argument("--check-completeness", nargs="+", metavar="FILE")
    g.add_argument("--check-a2", metavar="findings.tsv")
    g.add_argument("--check-downgrades", metavar="findings.tsv")
    g.add_argument("--check-backlog-ids", nargs=2, metavar=("findings.tsv", "BACKLOG.md"))
    ap.add_argument(
        "--allow-open",
        action="store_true",
        help="permit disposition OPEN (carried-red ledger only; never at acceptance)",
    )
    args = ap.parse_args(argv)

    if args.self_test:
        rc = self_test()
        failures = acceptance_self_test()
        print(f"acceptance self-test: {len(failures)} failure(s)")
        for f in failures:
            print(f"  FAIL {f}")
        if failures:
            return 1
        print("acceptance self-test: every F28A code tripped by a bad fixture and NOT by a")
        print("                      good one, and neither an upward re-score nor a repair")
        print("                      row was wrongly caught.")
        return rc

    # -- plan 28-04 acceptance gates ------------------------------------------------------
    if args.check_completeness:
        paths = [Path(p) for p in args.check_completeness]
        for p in paths:
            if not p.is_file():
                print(f"F28C-000  input not found: {p}", file=sys.stderr)
                return 2
        rows = parse_ledger(paths[0].read_text(encoding="utf-8"))
        if not rows:
            print("F28C-000  ledger has no rows", file=sys.stderr)
            return 2
        upstream = [(p, upstream_finding_ids(p)) for p in paths[1:]]
        total = sum(len(ids) for _, ids in upstream)
        if total == 0:
            # A completeness check over an empty upstream set is the tautology this whole
            # phase exists to catch: it would pass having compared nothing.
            print(
                "F28C-000  no finding ids were extracted from any upstream artifact; a "
                "completeness check over an empty set proves nothing",
                file=sys.stderr,
            )
            return 2
        print(
            f"--check-completeness: {len(rows)} ledger row(s) against {total} upstream "
            f"finding id(s) from {len(upstream)} artifact(s)"
        )
        return _report("--check-completeness", check_completeness(rows, upstream))

    if args.check_backlog_ids:
        ledger_path, backlog_path = (Path(p) for p in args.check_backlog_ids)
        for p in (ledger_path, backlog_path):
            if not p.is_file():
                print(f"F28C-000  input not found: {p}", file=sys.stderr)
                return 2
        rows = parse_ledger(ledger_path.read_text(encoding="utf-8"))
        backlog = backlog_path.read_text(encoding="utf-8")
        paper = [r for r in rows if r.get("disposition") in PAPER_DISPOSITIONS]
        print(f"--check-backlog-ids: {len(paper)} accepted/deferred finding(s)")
        return _report("--check-backlog-ids", check_backlog_ids(rows, backlog))

    path = Path(
        args.check_contract
        or args.check_rescoring
        or args.check_ledger
        or args.validate
        or args.check_a2
        or args.check_downgrades
        or args.check_matrix
        or args.check_no_uncontrolled_skips
    )
    if not path.is_file():
        print(f"F28C-000  input not found: {path}", file=sys.stderr)
        return 2
    text = path.read_text(encoding="utf-8")

    if args.check_contract:
        anchors = extract_anchors(_repo_root(path.resolve().parent))
        print(f"--check-contract: {len(anchors)} anchors extracted from their sources")
        return _report("--check-contract", check_contract_text(text, anchors))

    if args.check_rescoring:
        rows = parse_ledger(text)
        if not rows:
            print("F28C-000  carried-red ledger has no rows", file=sys.stderr)
            return 2
        print(f"--check-rescoring: {len(rows)} carried finding(s)")
        return _report("--check-rescoring", check_rescoring(rows))

    if args.check_ledger or args.validate or args.check_a2 or args.check_downgrades:
        rows = parse_ledger(text)
        if not rows:
            print("F28C-000  ledger has no rows", file=sys.stderr)
            return 2
        if args.check_a2:
            print(f"--check-a2: {len(rows)} finding(s)")
            return _report("--check-a2", check_a2(rows))
        if args.check_downgrades:
            print(f"--check-downgrades: {len(rows)} finding(s)")
            return _report("--check-downgrades", check_downgrades(rows))
        name = "--validate" if args.validate else "--check-ledger"
        print(
            f"{name}: {len(rows)} finding(s), allow_open={args.allow_open}"
        )
        return _report(name, validate_ledger(rows, allow_open=args.allow_open))

    rows = parse_matrix(text)
    if not rows:
        print("F28C-000  matrix has no rows", file=sys.stderr)
        return 2

    if args.check_matrix:
        print(f"--check-matrix: {len(rows)} cell(s)")
        return _report("--check-matrix", validate_matrix(rows))

    skips = [r for r in rows if r.get("applicability") == "skipped"]
    print(f"--check-no-uncontrolled-skips: {len(rows)} cell(s), {len(skips)} skip(s)")
    return _report("--check-no-uncontrolled-skips", check_no_uncontrolled_skips(rows))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
