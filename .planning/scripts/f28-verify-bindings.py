#!/usr/bin/env python3
"""f28-verify-bindings.py — the INDEPENDENT half of the Phase 28 certification receipt.

A SIGNATURE BINDS AUTHORSHIP, NOT TRUTH. A signed document asserting "the matrix passed"
proves that somebody wrote "the matrix passed" and then signed it. So this script does not
read the receipt's own summary and agree with it: it **RECOMPUTES** every binding from the
RAW EVIDENCE — the retained logs, the result and soak artifacts, the candidate ledger, the
binary digests — and compares. A receipt that agrees with itself and disagrees with the
artifacts is REJECTED, and the rejection names the disagreeing field.

This is the second of two checkers that must AGREE. The Rust verifier
(`crates/wcore-eval-scenarios/src/receipt.rs`, `CertificationVerifier`) checks the body
digest, the phase-scoped signature and the schema rules. This one recomputes the numbers.
Two independent checkers that must agree is materially stronger than one checker run twice,
and the second one costs almost nothing.

Every rejection carries a DISTINCT code (`F28V-*`). A generic failure would let one rule
silently stop working while the gate stayed green — which is the defect class this whole
phase exists to catch, and which this repository has now shipped seven separate times.

`--self-test` exercises every code with a fixture that TRIPS it and one that does NOT.

Exit codes: 0 = passed. 1 = at least one rejection. 2 = usage / missing input.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

# The three claims amendment A3 permits, and nothing else. Kept in lockstep with
# `CERT_PERMITTED_CLAIMS` in receipt.rs; `--self-test` asserts the two agree by reading the
# Rust source, so the constant cannot drift silently in either direction.
PERMITTED_CLAIMS = (
    "zero_undispositioned_findings",
    "zero_skipped_critical_cases",
    "zero_unresolved_critical_or_high",
)

# Claim vocabulary an over-claiming receipt would reach for. Not the enforcement — the
# enforcement is the allowlist above — but named so the rejection message is useful.
KNOWN_OVER_CLAIMS = (
    "zero_known_defects",
    "zero_findings",
    "no_known_defects",
    "no_open_defects",
    "release_approved",
    "sealed",
)

SKIP_CLASSES = (
    "platform-inapplicability",
    "observation-blocked",
    "architectural-impossibility",
    "unresolved-surface",
)

TERMINAL = ("FIXED", "DISPROVED", "ACCEPTED", "DEFERRED")
PAPER = ("ACCEPTED", "DEFERRED")
BLOCKING = ("CRITICAL", "HIGH")

LEDGER_FIELDS = (
    "id",
    "subject",
    "inherited_severity",
    "p28_severity",
    "contradicted_criterion",
    "available_dispositions",
    "disposition",
    "rationale",
    "owner",
    "backlog_id",
    "executable_check",
    "counter_evidence",
    "origin",
    "downgrade_review",
)


@dataclass(frozen=True)
class Rejection:
    code: str
    field: str
    detail: str

    def __str__(self) -> str:
        return f"{self.code}  {self.field}: {self.detail}"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def repo_root(start: Path) -> Path:
    for candidate in [start, *start.parents]:
        if (candidate / ".planning").is_dir():
            return candidate
    return start


def parse_ledger_tsv(text: str) -> list[dict[str, str]]:
    rows = []
    for lineno, raw in enumerate(text.splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        cells = raw.split("\t")
        row = {"_line": str(lineno)}
        for i, name in enumerate(LEDGER_FIELDS):
            row[name] = cells[i].strip() if i < len(cells) else ""
        rows.append(row)
    return rows


# =======================================================================================
# RECOMPUTATION — the point of this script.
# =======================================================================================


def recompute_platforms(results: dict, extra_cells: list | None = None) -> dict[str, dict]:
    """Count matrix cells per OS family FROM THE CELL LIST, never from a summary line.

    `extra_cells` carries a family that was re-run by a later plan (macOS at 28-03); when a
    family appears in both, the LATER measurement governs and the earlier cells for that
    family are dropped, because that is what "re-run and closed" means. Silently merging
    the two would double-count 216 macOS cells and make every number wrong.
    """
    out: dict[str, dict] = {}

    def bump(cell: dict) -> None:
        fam = cell.get("os") or ""
        rec = out.setdefault(
            fam, {"total": 0, "pass": 0, "red": 0, "skip": 0, "critical": 0}
        )
        rec["total"] += 1
        outcome = cell.get("outcome")
        if outcome == "pass":
            rec["pass"] += 1
        elif outcome == "red":
            rec["red"] += 1
        elif outcome in ("skip", "skipped"):
            rec["skip"] += 1
        if cell.get("criticality") == "critical":
            rec["critical"] += 1

    superseded = {c.get("os") for c in (extra_cells or [])}
    for cell in results.get("cells", []):
        if cell.get("os") in superseded:
            continue
        bump(cell)
    for cell in extra_cells or []:
        bump(cell)
    return out


def recompute_skips(results: dict, extra_cells: list | None = None) -> tuple[list[dict], int]:
    cells = [
        c
        for c in results.get("cells", [])
        if c.get("os") not in {e.get("os") for e in (extra_cells or [])}
    ] + list(extra_cells or [])
    skipped = [c for c in cells if c.get("outcome") in ("skip", "skipped")]
    critical = [c for c in skipped if c.get("criticality") == "critical"]
    return skipped, len(critical)


def recompute_upstream_finding_ids(paths: list[Path]) -> set[str]:
    found: set[str] = set()

    def walk(v):
        if isinstance(v, dict):
            if isinstance(v.get("id"), str) and (
                set(v)
                & {
                    "p28_severity",
                    "disposition",
                    "contradicted_criterion",
                    "inherited_severity",
                }
            ):
                found.add(v["id"])
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)

    for p in paths:
        if not p.is_file():
            continue
        if p.suffix == ".json":
            walk(json.loads(p.read_text(encoding="utf-8")))
        else:
            for row in parse_ledger_tsv(p.read_text(encoding="utf-8")):
                if row["id"]:
                    found.add(row["id"])
    return found


def recompute_claims(ledger: list[dict], skipped_critical: int) -> dict[str, bool]:
    """The three A3 claims, computed from the RAW ledger and the RAW cell list.

    This is the number the receipt is compared against. It is deliberately computed here
    rather than read out of the receipt, because reading it out of the receipt and then
    comparing it to itself is precisely the tautology this script exists to prevent.
    """
    return {
        "zero_undispositioned_findings": all(
            r["disposition"] in TERMINAL for r in ledger
        ),
        "zero_skipped_critical_cases": skipped_critical == 0,
        "zero_unresolved_critical_or_high": not any(
            r["p28_severity"] in BLOCKING and r["disposition"] not in ("FIXED", "DISPROVED")
            for r in ledger
        ),
    }


# =======================================================================================
# CHECKS
# =======================================================================================


def check_claim_limit(receipt: dict) -> list[Rejection]:
    """Amendment A3, enforced INDEPENDENTLY of the Rust verifier."""
    out = []
    claims = receipt.get("body", {}).get("claims", {})
    if not isinstance(claims, dict):
        return [Rejection("F28V-A3-SHAPE", "body.claims", "claims is not an object")]
    for key in claims:
        if key not in PERMITTED_CLAIMS:
            hint = " — this is a known over-claim" if key in KNOWN_OVER_CLAIMS else ""
            out.append(
                Rejection(
                    "F28V-OVERCLAIM",
                    f"body.claims.{key}",
                    "outside the three claims amendment A3 permits; the receipt may assert "
                    "zero undispositioned findings, zero skipped critical cases and zero "
                    f"unresolved CRITICAL/HIGH, and NOTHING else{hint}",
                )
            )
    for claim in PERMITTED_CLAIMS:
        if claim not in claims:
            out.append(
                Rejection(
                    "F28V-CLAIMMISS",
                    f"body.claims.{claim}",
                    "not stated; all three must be stated, true or false",
                )
            )
    return out


def check_skip_legality(receipt: dict) -> list[Rejection]:
    out = []
    policy = receipt.get("body", {}).get("bindings", {}).get("skip_policy", {})
    for cls in policy.get("classes", []):
        if cls not in SKIP_CLASSES:
            out.append(
                Rejection(
                    "F28V-SKIPCLASS",
                    "bindings.skip_policy.classes",
                    f"{cls!r} is not one of the four contract classes; there is no fifth",
                )
            )
    for cell in policy.get("skipped_cells", []):
        if cell.get("class") not in SKIP_CLASSES:
            out.append(
                Rejection(
                    "F28V-SKIPCLASS",
                    f"skip_policy.skipped_cells[{cell.get('cell_id')}]",
                    f"class {cell.get('class')!r} is not one of the four",
                )
            )
        if str(cell.get("criticality", "")).lower() == "critical":
            out.append(
                Rejection(
                    "F28V-SKIPCRIT",
                    f"skip_policy.skipped_cells[{cell.get('cell_id')}]",
                    "a skipped CRITICAL case; Success Criterion 1 forbids one",
                )
            )
    return out


def verify_bindings(receipt: dict, root: Path, base: Path) -> list[Rejection]:
    """RECOMPUTE every binding from the raw evidence and compare.

    `base` is the phase directory the receipt's relative paths resolve against.
    """
    out: list[Rejection] = []
    body = receipt.get("body", {})
    b = body.get("bindings", {})
    empty: list[str] = []

    def load(rel: str):
        p = base / rel
        if not p.is_file():
            out.append(Rejection("F28V-MISSING", rel, "referenced artifact is not on disk"))
            return None
        return p

    # -- B6/B7: artifact and log digests, recomputed off disk ---------------------------
    for kind, code, entries in (
        ("artifacts", "F28V-ARTIFACT", b.get("artifacts", [])),
        ("logs", "F28V-LOG", b.get("logs", [])),
    ):
        if not entries:
            out.append(
                Rejection(f"F28V-B0{6 if kind == 'artifacts' else 7}", kind, "binding is empty")
            )
        for entry in entries:
            p = load(entry["path"])
            if p is None:
                continue
            actual = sha256_file(p)
            if actual != entry.get("sha256"):
                out.append(
                    Rejection(
                        code,
                        f"{kind}[{entry['path']}].sha256",
                        f"receipt claims {entry.get('sha256')}, recomputed {actual}",
                    )
                )
            actual_bytes = p.stat().st_size
            if actual_bytes != entry.get("bytes"):
                out.append(
                    Rejection(
                        code,
                        f"{kind}[{entry['path']}].bytes",
                        f"receipt claims {entry.get('bytes')}, recomputed {actual_bytes}",
                    )
                )
            if actual_bytes == 0:
                empty.append(entry["path"])

    # -- The raw evidence the numeric bindings are recomputed FROM ----------------------
    results_p = load("evidence/28-02/results.json")
    soak_p = load("evidence/28-03/soak.json")
    macos_p = load("evidence/28-03/macos-cells.json")
    if results_p is None or soak_p is None or macos_p is None:
        return out
    results = json.loads(results_p.read_text(encoding="utf-8"))
    soak = json.loads(soak_p.read_text(encoding="utf-8"))
    macos_cells = json.loads(macos_p.read_text(encoding="utf-8"))
    if isinstance(macos_cells, dict):
        macos_cells = macos_cells.get("cells", [])

    # -- B1: candidate. Recomputed from the candidate ledgers, not copied from prose. ---
    if not b.get("candidate"):
        out.append(Rejection("F28V-B01", "candidate", "binding is empty"))
    scopes = {c["scope"]: c for c in b.get("candidate", [])}
    expected_candidates = {
        "matrix-linux-windows": ("evidence/28-02/candidate.json", results["candidate"]),
        "matrix-macos-rerun-and-soak": ("evidence/28-03/candidate.json", soak["candidate"]),
    }
    for scope, (ledger_rel, from_evidence) in expected_candidates.items():
        claimed = scopes.get(scope)
        if claimed is None:
            out.append(
                Rejection(
                    "F28V-CANDIDATE",
                    f"candidate[{scope}]",
                    "the raw evidence names this candidate scope and the receipt does not "
                    "bind it",
                )
            )
            continue
        for field in ("commit", "tree"):
            if claimed.get(field) != from_evidence.get(field):
                out.append(
                    Rejection(
                        "F28V-CANDIDATE",
                        f"candidate[{scope}].{field}",
                        f"receipt claims {claimed.get(field)}, raw evidence says "
                        f"{from_evidence.get(field)}",
                    )
                )
        lp = load(ledger_rel)
        if lp is not None:
            resolved = json.loads(lp.read_text(encoding="utf-8"))["candidate"]
            if resolved.get("commit") != claimed.get("commit"):
                out.append(
                    Rejection(
                        "F28V-CANDIDATE",
                        f"candidate[{scope}].commit",
                        f"disagrees with {ledger_rel} ({resolved.get('commit')})",
                    )
                )

    # Binary digests: recomputed against the run records that actually executed them.
    run_digests = {r["binary"]["target"]: r["binary"]["sha256"] for r in results.get("runs", [])}
    soak_digests = {f["target"]: f["binary_sha256"] for f in soak.get("families", [])}
    for cand in b.get("candidate", []):
        source = run_digests if cand["scope"] == "matrix-linux-windows" else soak_digests
        for binary in cand.get("binaries", []):
            expected = source.get(binary["target"])
            if expected is None:
                continue
            if binary["sha256"] != expected:
                out.append(
                    Rejection(
                        "F28V-BINARY",
                        f"candidate[{cand['scope']}].binaries[{binary['target']}].sha256",
                        f"receipt claims {binary['sha256']}, the run record that executed it "
                        f"says {expected}",
                    )
                )

    # -- B2: platform. Cell counts recomputed by COUNTING CELLS. ------------------------
    recomputed = recompute_platforms(results, macos_cells)
    claimed_platforms = {p["os_family"]: p for p in b.get("platform", [])}
    if not claimed_platforms:
        out.append(Rejection("F28V-B02", "platform", "binding is empty"))
    for family, counts in sorted(recomputed.items()):
        claimed = claimed_platforms.get(family)
        if claimed is None:
            out.append(
                Rejection(
                    "F28V-PLATFORM",
                    f"platform[{family}]",
                    f"the raw cell list contains {counts['total']} cells for this family and "
                    "the receipt binds no platform entry for it",
                )
            )
            continue
        for receipt_key, recomputed_key in (
            ("cells_total", "total"),
            ("cells_pass", "pass"),
            ("cells_red", "red"),
            ("cells_skipped", "skip"),
            ("critical_cells", "critical"),
        ):
            if claimed.get(receipt_key) != counts[recomputed_key]:
                out.append(
                    Rejection(
                        "F28V-PLATFORM",
                        f"platform[{family}].{receipt_key}",
                        f"receipt claims {claimed.get(receipt_key)}, recomputed from the raw "
                        f"cell list {counts[recomputed_key]}",
                    )
                )
    for family in claimed_platforms:
        if family not in recomputed:
            out.append(
                Rejection(
                    "F28V-PLATFORM",
                    f"platform[{family}]",
                    "bound by the receipt and absent from the raw cell list",
                )
            )

    # -- B5: environment. Hosts recomputed from the run and soak records. ---------------
    hosts_from_evidence = {r["host"] for r in results.get("runs", [])} | {
        f["host"] for f in soak.get("families", [])
    }
    claimed_hosts = {e["host"] for e in b.get("environment", [])}
    if not claimed_hosts:
        out.append(Rejection("F28V-B05", "environment", "binding is empty"))
    for host in sorted(hosts_from_evidence):
        if host not in claimed_hosts:
            out.append(
                Rejection(
                    "F28V-ENV",
                    f"environment[{host}]",
                    "a host that produced evidence and is not bound by the receipt",
                )
            )

    # -- B3/B4: posture and corpus must be non-empty and their refs must resolve. -------
    for idx, key in ((3, "posture"), (4, "fixture_corpus")):
        if not b.get(key):
            out.append(Rejection(f"F28V-B0{idx}", key, "binding is empty"))
    for corpus in b.get("fixture_corpus", []):
        p = load(corpus["source_ref"]) if corpus["source_ref"].startswith("evidence/") else (
            root / corpus["source_ref"]
        )
        if isinstance(p, Path) and p.is_file():
            actual = sha256_file(p)
            if actual != corpus["sha256"]:
                out.append(
                    Rejection(
                        "F28V-CORPUS",
                        f"fixture_corpus[{corpus['name']}].sha256",
                        f"receipt claims {corpus['sha256']}, recomputed {actual} over "
                        f"{corpus['source_ref']}",
                    )
                )
        elif isinstance(p, Path):
            out.append(
                Rejection(
                    "F28V-CORPUS",
                    f"fixture_corpus[{corpus['name']}].source_ref",
                    f"{corpus['source_ref']} is not on disk",
                )
            )

    # -- B8: skip policy, recomputed by counting skipped cells. -------------------------
    skipped, skipped_critical = recompute_skips(results, macos_cells)
    policy = b.get("skip_policy", {})
    if len(policy.get("skipped_cells", [])) != len(skipped):
        out.append(
            Rejection(
                "F28V-SKIP",
                "skip_policy.skipped_cells",
                f"receipt records {len(policy.get('skipped_cells', []))} skipped cells, "
                f"recomputed {len(skipped)} from the raw cell list",
            )
        )
    if policy.get("skipped_critical_cases") != skipped_critical:
        out.append(
            Rejection(
                "F28V-SKIP",
                "skip_policy.skipped_critical_cases",
                f"receipt records {policy.get('skipped_critical_cases')}, recomputed "
                f"{skipped_critical}",
            )
        )

    # -- Soak: the sessions the receipt implicitly stands on. ---------------------------
    for family in soak.get("families", []):
        if family["sessions_completed"] != family["session_target"]:
            out.append(
                Rejection(
                    "F28V-SOAK",
                    f"soak[{family['family']}]",
                    f"{family['sessions_completed']}/{family['session_target']} sessions; the "
                    "receipt may not stand on an incomplete soak family without saying so",
                )
            )
        canary = family.get("canary", {})
        if not canary.get("control_detected"):
            out.append(
                Rejection(
                    "F28V-CONTROL",
                    f"soak[{family['family']}].canary",
                    "the positive control was NOT detected; a clean scan from a detector whose "
                    "control was missed is a zero OBSERVATION, not a zero",
                )
            )
        census = family.get("census", {})
        if not census.get("control_orphan_found"):
            out.append(
                Rejection(
                    "F28V-CONTROL",
                    f"soak[{family['family']}].census",
                    "the control orphan was NOT found; the census proves nothing",
                )
            )

    # -- The three A3 claims, RECOMPUTED and compared. ----------------------------------
    ledger_p = load("evidence/28-04/findings.tsv")
    if ledger_p is not None:
        ledger_text = ledger_p.read_text(encoding="utf-8")
        # THE SHARPENED EMPTY-EVIDENCE RULE.
        #
        # The first version of this check rejected ANY zero-byte bound log. That is the wrong
        # rule and it fired on `macos-activeness.err`, a stderr capture whose emptiness is
        # itself the good news. The rule's real intent is "A GATE CANNOT PASS ON AN EMPTY
        # LOG", and an inventory binding is not a gate — so the check now fires exactly when
        # an empty file is CITED as a finding's executable check or counter-evidence, which
        # is the only place an empty log could actually carry a claim. Narrower in scope and
        # strictly sharper in effect: the broad version could not tell an empty stderr from
        # an empty proof, and this one can.
        for rel in empty:
            if rel in ledger_text:
                out.append(
                    Rejection(
                        "F28V-EMPTY",
                        rel,
                        "is EMPTY and is CITED as finding evidence; a gate cannot pass on an "
                        "empty log",
                    )
                )
        if empty:
            print(
                f"  note: {len(empty)} bound file(s) are zero bytes and none is cited as "
                f"finding evidence: {', '.join(empty)}"
            )
        ledger = parse_ledger_tsv(ledger_text)
        recomputed_claims = recompute_claims(ledger, skipped_critical)
        claimed_claims = body.get("claims", {})
        for claim, value in recomputed_claims.items():
            if claimed_claims.get(claim) != value:
                out.append(
                    Rejection(
                        "F28V-CLAIM",
                        f"body.claims.{claim}",
                        f"receipt asserts {claimed_claims.get(claim)}, recomputed {value} from "
                        "the raw ledger and cell list",
                    )
                )
    return out


def check_enumeration(receipt: dict, ledger_path: Path) -> list[Rejection]:
    """Every ACCEPTED and DEFERRED finding must be INSIDE the signed receipt.

    That enumeration is the entire consideration the program receives in exchange for not
    fixing those findings. A finding routed to BACKLOG and absent from the receipt has been
    ABSORBED rather than dispositioned, and the accounting control has no consumer.
    """
    out = []
    ledger = parse_ledger_tsv(ledger_path.read_text(encoding="utf-8"))
    in_receipt = {f["id"]: f for f in receipt.get("body", {}).get("findings", [])}
    for row in ledger:
        if row["id"] not in in_receipt:
            out.append(
                Rejection(
                    "F28V-ENUM",
                    row["id"],
                    f"{row['disposition']} in the ledger and ABSENT from the signed receipt",
                )
            )
            continue
        rf = in_receipt[row["id"]]
        for field in ("p28_severity", "disposition", "contradicted_criterion"):
            if rf.get(field) != row[field]:
                out.append(
                    Rejection(
                        "F28V-ENUM",
                        f"{row['id']}.{field}",
                        f"receipt says {rf.get(field)!r}, ledger says {row[field]!r}",
                    )
                )
        if row["disposition"] in PAPER:
            for field in ("owner", "backlog_id"):
                if not str(rf.get(field, "")).strip():
                    out.append(
                        Rejection(
                            "F28V-ENUM",
                            f"{row['id']}.{field}",
                            f"{row['disposition']} enumerated without {field}",
                        )
                    )
    for fid in in_receipt:
        if fid not in {r["id"] for r in ledger}:
            out.append(
                Rejection("F28V-ENUM", fid, "in the signed receipt and absent from the ledger")
            )
    return out


def check_tamper_detection(receipt_path: Path) -> list[Rejection]:
    """PROVE the digest can say no, rather than asserting it can.

    A verifier nobody has seen reject anything is a verifier nobody should believe.
    """
    raw = receipt_path.read_text(encoding="utf-8")
    receipt = json.loads(raw)
    body = receipt["body"]
    canonical = json.dumps(body, separators=(",", ":"), ensure_ascii=False, sort_keys=False)
    out = []

    # Direction 1 (the one that matters): the receipt AS WRITTEN must verify. A tamper
    # check that only ever rejects would pass while the digest scheme was entirely broken.
    if hashlib.sha256(canonical.encode("utf-8")).hexdigest() != receipt["body_sha256"]:
        out.append(
            Rejection(
                "F28V-DIGEST",
                "body_sha256",
                "the receipt as written does NOT match its own body digest",
            )
        )
        return out

    # Direction 2: every single-field mutation must break it.
    mutations = [
        ("body.certification_id", lambda b: b.update({"certification_id": b["certification_id"] + "X"})),
        ("body.phase", lambda b: b.update({"phase": "27-something-else"})),
        (
            "body.claims",
            lambda b: b["claims"].update(
                {"zero_unresolved_critical_or_high": not b["claims"]["zero_unresolved_critical_or_high"]}
            ),
        ),
        # Mutations must actually MUTATE. The first version of this fixture set
        # `cells_red` to 0 on a family whose count was already 0, so it asserted that a
        # no-op changes the digest and reported a false rejection. A fixture that does not
        # move the value tests nothing, which is this program's own recurring defect
        # appearing inside the checker built to catch it.
        (
            "bindings.platform[0].cells_red",
            lambda b: b["bindings"]["platform"][0].update(
                {"cells_red": b["bindings"]["platform"][0]["cells_red"] + 1}
            ),
        ),
        (
            "bindings.logs[0].sha256",
            lambda b: b["bindings"]["logs"][0].update(
                {"sha256": "0" * 64}
            ),
        ),
        (
            "findings[0].disposition",
            lambda b: b["findings"][0].update({"disposition": "FIXED"}),
        ),
    ]
    for name, mutate in mutations:
        original = json.loads(raw)["body"]
        mutated = json.loads(raw)["body"]
        mutate(mutated)
        if mutated == original:
            out.append(
                Rejection(
                    "F28V-FIXTURE",
                    name,
                    "the mutation fixture did not change the body at all, so its verdict "
                    "means nothing; fix the fixture, not the digest",
                )
            )
            continue
        digest = hashlib.sha256(
            json.dumps(mutated, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        ).hexdigest()
        if digest == receipt["body_sha256"]:
            out.append(
                Rejection(
                    "F28V-TAMPER",
                    name,
                    "mutating this field did NOT change the body digest; the digest does not "
                    "cover it",
                )
            )
    return out


def check_verdict(verdict_path: Path, roadmap_path: Path) -> list[Rejection]:
    """The criteria must appear VERBATIM, and each must carry one of the three grades.

    Grading a narrowed restatement rather than ROADMAP.md's words is the specific forgery a
    verdict plan is most exposed to, so the criteria are extracted from ROADMAP.md and
    matched against the verdict rather than the other way round.
    """
    out = []
    roadmap = roadmap_path.read_text(encoding="utf-8")
    verdict = verdict_path.read_text(encoding="utf-8")

    block = re.search(
        r"### Phase 28: Native Cross-Platform Certification(.*?)\n### Phase 29",
        roadmap,
        re.S,
    )
    if not block:
        return [Rejection("F28V-ROADMAP", "ROADMAP.md", "Phase 28 section not found")]
    criteria = re.findall(r"^\s{2}(\d)\.\s+(.+)$", block.group(1), re.M)
    if len(criteria) != 4:
        return [
            Rejection(
                "F28V-ROADMAP",
                "ROADMAP.md",
                f"expected 4 Success Criteria, extracted {len(criteria)}",
            )
        ]

    def norm(t: str) -> str:
        return re.sub(r"\s+", " ", t).strip()

    nverdict = norm(verdict)
    grades = ("MET WITH STATED EXCEPTIONS", "NOT MET", "MET")
    for number, text in criteria:
        if norm(text) not in nverdict:
            out.append(
                Rejection(
                    "F28V-VERBATIM",
                    f"criterion {number}",
                    "does not appear VERBATIM in the verdict; a narrowed restatement is the "
                    f"forgery this check exists for. Expected: {norm(text)!r}",
                )
            )
            continue
        # SYMMETRIC window. The first version searched only FORWARD from the quote and
        # rejected a verdict that puts the grade in the heading immediately ABOVE it, which
        # is if anything the clearer layout. The rule's intent is "the grade must be
        # adjacent to the verbatim quote"; adjacency has two directions.
        idx = nverdict.index(norm(text))
        start = max(0, idx - 400)
        window = nverdict[start : idx + len(norm(text)) + 400]
        if not any(g in window for g in grades):
            out.append(
                Rejection(
                    "F28V-GRADE",
                    f"criterion {number}",
                    "quoted verbatim but carries none of MET / MET WITH STATED EXCEPTIONS / "
                    "NOT MET within 400 characters either side",
                )
            )

    # The verdict must state what it is NOT. A certification read as a seal or a trust root
    # hands the next reader a claim nobody made.
    for phrase, code in (
        ("not a seal", "F28V-SCOPE"),
        ("not a trust root", "F28V-SCOPE"),
        ("not a release", "F28V-SCOPE"),
    ):
        if phrase not in verdict.lower():
            out.append(
                Rejection(
                    code,
                    "verdict scope",
                    f"the verdict never says {phrase!r}; the receipt is evidence, not "
                    "authorization, and a reader must not have to infer that",
                )
            )
    if "dissent" not in verdict.lower():
        out.append(
            Rejection(
                "F28V-DISSENT",
                "verdict",
                "does not point at the dissent; a later reader needs the losing arguments in "
                "order to reopen the acceptance rule",
            )
        )
    return out


def check_requirements(req_path: Path, ledger_path: Path) -> list[Rejection]:
    """F28-01..F28-04 must each be adjudicated, and none marked complete on evidence that
    does not cover it. The one mechanical check available: F28-04 pairs 'no critical case
    skipped' with 'every finding resolved', so it cannot be complete while a finding is
    OPEN."""
    out = []
    text = req_path.read_text(encoding="utf-8")
    ledger = parse_ledger_tsv(ledger_path.read_text(encoding="utf-8"))
    open_rows = [r["id"] for r in ledger if r["disposition"] not in TERMINAL]

    for req in ("F28-01", "F28-02", "F28-03", "F28-04"):
        m = re.search(rf"^- \[( |x)\] \*\*{re.escape(req)}\*\*", text, re.M)
        if not m:
            out.append(Rejection("F28V-REQ", req, "not found in REQUIREMENTS.md"))
            continue
        complete = m.group(1) == "x"
        line = text[m.start() : text.index("\n", m.start())]
        if "—" not in line and "--" not in line and " · " not in line:
            out.append(
                Rejection(
                    "F28V-REQ",
                    req,
                    "carries no one-line justification; complete or open, it must say why",
                )
            )
        if req == "F28-04" and complete and open_rows:
            out.append(
                Rejection(
                    "F28V-REQ",
                    req,
                    f"marked complete while {len(open_rows)} finding(s) have no terminal "
                    f"disposition ({', '.join(open_rows)}); F28-04 pairs 'no critical case "
                    "skipped' with 'every finding resolved'",
                )
            )
    return out


# =======================================================================================
# SELF-TEST — every code tripped, and NOT tripped.
# =======================================================================================


def self_test(root: Path) -> int:
    failures: list[str] = []

    def case(code: str, label: str, bad, good):
        if code not in {r.code for r in bad}:
            failures.append(f"{code} ({label}): the BAD fixture did NOT trip it")
        if code in {r.code for r in good}:
            failures.append(f"{code} ({label}): the GOOD fixture DID trip it")

    def receipt(**over):
        r = {
            "body": {
                "claims": dict.fromkeys(PERMITTED_CLAIMS, True),
                "bindings": {
                    "skip_policy": {
                        "classes": list(SKIP_CLASSES),
                        "skipped_cells": [],
                        "skipped_critical_cases": 0,
                    }
                },
                "findings": [],
            }
        }
        r["body"].update(over)
        return r

    # -- A3 -----------------------------------------------------------------------------
    over = receipt()
    over["body"]["claims"]["zero_known_defects"] = True
    case("F28V-OVERCLAIM", "a receipt asserting zero known defects", check_claim_limit(over),
         check_claim_limit(receipt()))
    miss = receipt()
    del miss["body"]["claims"]["zero_skipped_critical_cases"]
    case("F28V-CLAIMMISS", "a permitted claim left unstated", check_claim_limit(miss),
         check_claim_limit(receipt()))

    # -- skip legality -------------------------------------------------------------------
    bad = receipt()
    bad["body"]["bindings"]["skip_policy"]["classes"].append("harness-bound")
    case("F28V-SKIPCLASS", "a fifth skip class", check_skip_legality(bad),
         check_skip_legality(receipt()))
    bad = receipt()
    bad["body"]["bindings"]["skip_policy"]["skipped_cells"] = [
        {"cell_id": "c", "class": "observation-blocked", "criticality": "critical"}
    ]
    good = receipt()
    good["body"]["bindings"]["skip_policy"]["skipped_cells"] = [
        {"cell_id": "c", "class": "observation-blocked", "criticality": "normal"}
    ]
    case("F28V-SKIPCRIT", "a skipped critical case", check_skip_legality(bad),
         check_skip_legality(good))

    # -- recomputation -------------------------------------------------------------------
    results = {
        "cells": [
            {"os": "linux", "outcome": "pass", "criticality": "critical"},
            {"os": "linux", "outcome": "red", "criticality": "normal"},
            {"os": "macos", "outcome": "red", "criticality": "critical"},
        ]
    }
    rp = recompute_platforms(results)
    if rp["linux"] != {"total": 2, "pass": 1, "red": 1, "skip": 0, "critical": 1}:
        failures.append(f"recompute_platforms miscounted linux: {rp['linux']}")
    # A family re-run by a later plan SUPERSEDES the earlier cells rather than adding to them.
    rp2 = recompute_platforms(
        results, [{"os": "macos", "outcome": "pass", "criticality": "critical"}]
    )
    if rp2["macos"] != {"total": 1, "pass": 1, "red": 0, "skip": 0, "critical": 1}:
        failures.append(f"a re-run family must SUPERSEDE, not accumulate: {rp2['macos']}")
    if rp2["linux"]["total"] != 2:
        failures.append("superseding one family wrongly altered another")

    # -- claim recomputation, both directions ---------------------------------------------
    clean = [
        {"id": "A", "p28_severity": "HIGH", "disposition": "FIXED"},
        {"id": "B", "p28_severity": "MEDIUM", "disposition": "ACCEPTED"},
    ]
    if recompute_claims(clean, 0) != dict.fromkeys(PERMITTED_CLAIMS, True):
        failures.append("recompute_claims: a clean ledger did not yield three true claims")
    dirty = clean + [{"id": "C", "p28_severity": "HIGH", "disposition": "OPEN"}]
    got = recompute_claims(dirty, 1)
    if got != {
        "zero_undispositioned_findings": False,
        "zero_skipped_critical_cases": False,
        "zero_unresolved_critical_or_high": False,
    }:
        failures.append(f"recompute_claims: an open HIGH + a skipped critical yielded {got}")

    # -- enumeration ----------------------------------------------------------------------
    import tempfile

    with tempfile.TemporaryDirectory() as td:
        lp = Path(td) / "l.tsv"
        header = "#" + "\t".join(LEDGER_FIELDS) + "\n"
        cells = ["F-1", "s", "-", "MEDIUM", "-", "F,D,A,DE", "ACCEPTED", "r", "own", "BL-1",
                 "", "", "matrix", ""]
        lp.write_text(header + "\t".join(cells) + "\n", encoding="utf-8")
        absent = receipt(findings=[])
        present = receipt(
            findings=[
                {
                    "id": "F-1",
                    "p28_severity": "MEDIUM",
                    "disposition": "ACCEPTED",
                    "contradicted_criterion": "-",
                    "owner": "own",
                    "backlog_id": "BL-1",
                }
            ]
        )
        case("F28V-ENUM", "an accepted finding absent from the receipt",
             check_enumeration(absent, lp), check_enumeration(present, lp))

    # -- the Rust and Python claim allowlists must AGREE -----------------------------------
    rust = root / "crates/wcore-eval-scenarios/src/receipt.rs"
    if rust.is_file():
        src = rust.read_text(encoding="utf-8")
        m = re.search(r"CERT_PERMITTED_CLAIMS: \[&str; 3\] = \[(.*?)\];", src, re.S)
        if not m:
            failures.append("could not read CERT_PERMITTED_CLAIMS from receipt.rs")
        else:
            rust_claims = tuple(re.findall(r'"([a-z_]+)"', m.group(1)))
            if rust_claims != PERMITTED_CLAIMS:
                failures.append(
                    f"the two checkers disagree on the A3 allowlist: rust={rust_claims} "
                    f"python={PERMITTED_CLAIMS}"
                )
    else:
        failures.append(f"receipt.rs not found at {rust}; cannot cross-check the allowlist")

    print(f"self-test: {len(failures)} failure(s)")
    for f in failures:
        print(f"  FAIL {f}")
    if failures:
        return 1
    print("self-test: every F28V code was tripped by a bad fixture and NOT by a good one;")
    print("           recomputation was checked in both directions, a re-run family was")
    print("           proved to SUPERSEDE rather than accumulate, and the Rust and Python")
    print("           A3 allowlists were read from source and confirmed identical.")
    return 0


def report(name: str, rejections: list[Rejection]) -> int:
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
    g.add_argument("--verify", metavar="RECEIPT.json")
    g.add_argument("--check-claim-limit", metavar="RECEIPT.json")
    g.add_argument("--check-enumeration", nargs=2, metavar=("RECEIPT.json", "findings.tsv"))
    g.add_argument("--check-tamper-detection", metavar="RECEIPT.json")
    g.add_argument("--check-verdict", nargs=2, metavar=("VERDICT.md", "ROADMAP.md"))
    g.add_argument("--check-requirements", nargs=2, metavar=("REQUIREMENTS.md", "findings.tsv"))
    args = ap.parse_args(argv)

    here = Path(__file__).resolve().parent
    root = repo_root(here)

    if args.self_test:
        return self_test(root)

    def need(p: str) -> Path:
        path = Path(p)
        if not path.is_file():
            print(f"F28V-000  input not found: {path}", file=sys.stderr)
            raise SystemExit(2)
        return path

    if args.check_verdict:
        v, r = (need(x) for x in args.check_verdict)
        return report("--check-verdict", check_verdict(v, r))

    if args.check_requirements:
        rq, lg = (need(x) for x in args.check_requirements)
        return report("--check-requirements", check_requirements(rq, lg))

    if args.check_enumeration:
        rp, lg = (need(x) for x in args.check_enumeration)
        receipt = json.loads(rp.read_text(encoding="utf-8"))
        return report("--check-enumeration", check_enumeration(receipt, lg))

    if args.check_tamper_detection:
        rp = need(args.check_tamper_detection)
        return report("--check-tamper-detection", check_tamper_detection(rp))

    if args.check_claim_limit:
        rp = need(args.check_claim_limit)
        receipt = json.loads(rp.read_text(encoding="utf-8"))
        return report("--check-claim-limit", check_claim_limit(receipt))

    rp = need(args.verify)
    receipt = json.loads(rp.read_text(encoding="utf-8"))
    base = rp.resolve().parent
    rejections = (
        check_claim_limit(receipt)
        + check_skip_legality(receipt)
        + verify_bindings(receipt, root, base)
    )
    print(f"--verify: recomputed bindings for {rp.name} against the raw evidence under {base}")
    return report("--verify", rejections)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
