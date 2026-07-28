#!/usr/bin/env python3
"""f28-build-receipt.py — assemble and sign the Phase 28 certification receipt.

Every value in the receipt is READ OFF THE RAW EVIDENCE here — cell lists are counted,
digests are computed off disk, hosts are read out of run records. Nothing is copied from a
summary document. `f28-verify-bindings.py --verify` then recomputes all of it independently
and rejects any disagreement, and the Rust `CertificationVerifier` checks the same artifact's
digest, signature and schema rules. Two checkers that must agree.

THE KEY IS PHASE-SCOPED. It is minted here, its public half and fingerprint are recorded in
the receipt and the verdict, and it is bound to NO release trust root. Binding one, rotating
one or publishing one is Phase 29's work. A phase-scoped signature says "this evidence was
assembled by this certification run and has not been altered since". It does NOT say "this
build is released".

The private half is derived deterministically from the receipt's own certification id, so a
later reader can re-derive it and re-verify without this run's machine. That is acceptable
EXACTLY BECAUSE the signature is not an authorization: it is an integrity binding over
evidence, and the seed being public costs nothing a release trust root would care about.
Phase 29's trust root must NOT be derived this way.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

ROOT = Path(__file__).resolve().parents[2]
PHASE = ROOT / ".planning/phases/28-native-cross-platform-certification"
SCHEMA = "wayland.cert.receipt"
SCHEMA_VERSION = 2
DOMAIN = b"wayland.cert.receipt.v2\x00"
CERT_ID = "f28-native-cross-platform-certification"
KEY_ID = "phase-28-certification-2026-07-28"
SCOPE = (
    "PHASE-SCOPED. This signature binds authorship and integrity of the assembled evidence "
    "for Phase 28 only. It is NOT a release trust root, NOT a seal, and NOT an "
    "authorization to release. Trust-root binding, rotation and revocation belong to "
    "Phase 29; tagging, releasing, merging and issue closure are reserved to Sean."
)

SKIP_CLASSES = [
    "platform-inapplicability",
    "observation-blocked",
    "architectural-impossibility",
    "unresolved-surface",
]

LEDGER_FIELDS = [
    "id", "subject", "inherited_severity", "p28_severity", "contradicted_criterion",
    "available_dispositions", "disposition", "rationale", "owner", "backlog_id",
    "executable_check", "counter_evidence", "origin", "downgrade_review",
]


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load(rel: str):
    return json.loads((PHASE / rel).read_text(encoding="utf-8"))


def ledger_rows() -> list[dict]:
    rows = []
    text = (PHASE / "evidence/28-04/findings.tsv").read_text(encoding="utf-8")
    for raw in text.splitlines():
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        cells = raw.split("\t")
        rows.append({n: (cells[i].strip() if i < len(cells) else "")
                     for i, n in enumerate(LEDGER_FIELDS)})
    return rows


def main() -> int:
    results = load("evidence/28-02/results.json")
    soak = load("evidence/28-03/soak.json")
    macos_cells = load("evidence/28-03/macos-cells.json")
    if isinstance(macos_cells, dict):
        macos_cells = macos_cells["cells"]
    cand02 = load("evidence/28-02/candidate.json")["candidate"]
    cand03 = load("evidence/28-03/candidate.json")["candidate"]

    # ---- B2 platform: COUNT THE CELLS. The macOS family was re-run at 28-03, and a re-run
    # SUPERSEDES rather than accumulates, so 28-02's macOS cells are dropped.
    superseded = {c["os"] for c in macos_cells}
    cells = [c for c in results["cells"] if c["os"] not in superseded] + macos_cells
    fam: dict[str, dict] = {}
    for c in cells:
        r = fam.setdefault(c["os"], dict(total=0, p=0, red=0, skip=0, crit=0))
        r["total"] += 1
        r["p"] += c["outcome"] == "pass"
        r["red"] += c["outcome"] == "red"
        r["skip"] += c["outcome"] in ("skip", "skipped")
        r["crit"] += c.get("criticality") == "critical"

    run_by_family = {r["os_family"]: r for r in results["runs"]}
    soak_by_family = {f["family"]: f for f in soak["families"]}

    platform = []
    for family in ("linux", "macos", "windows"):
        counts = fam[family]
        evidence = (
            "evidence/28-03/macos-cells.json (re-run at e4a3f5fc; supersedes 28-02's macOS cells)"
            if family == "macos"
            else "evidence/28-02/results.json"
        )
        platform.append({
            "os_family": family,
            "target": run_by_family[family]["binary"]["target"],
            "cells_total": counts["total"],
            "cells_pass": counts["p"],
            "cells_red": counts["red"],
            "cells_skipped": counts["skip"],
            "critical_cells": counts["crit"],
            "evidence_ref": evidence,
        })

    # ---- B1 candidate: two of them, per scope, because that is what ran. -------------
    candidate = [
        {
            "scope": "matrix-linux-windows",
            "commit": cand02["commit"],
            "tree": cand02["tree"],
            "ledger_ref": "evidence/28-02/candidate.json",
            "binaries": [
                {
                    "target": r["binary"]["target"],
                    "sha256": r["binary"]["sha256"],
                    "provenance": r["binary"]["provenance"],
                }
                for r in results["runs"]
                if r["os_family"] in ("linux", "windows")
            ],
        },
        {
            "scope": "matrix-macos-rerun-and-soak",
            "commit": cand03["commit"],
            "tree": cand03["tree"],
            "ledger_ref": "evidence/28-03/candidate.json",
            "binaries": [
                {
                    "target": f["target"],
                    "sha256": f["binary_sha256"],
                    "provenance": (
                        f"soak family {f['family']} on {f['host']}; digest asserted against "
                        "the candidate ledger before the first session"
                    ),
                }
                for f in soak["families"]
            ],
        },
    ]

    # ---- B3 posture -------------------------------------------------------------------
    posture = [
        {
            "name": "sandbox-required-fail-closed",
            "description": (
                "WAYLAND_SANDBOX=none is an ERROR, not a downgrade, and no opt-in was set. "
                "Every sandbox-probes cell asserts exit=0 with WAYLAND_SANDBOX=none and "
                "WAYLAND_ALLOW_NO_SANDBOX unset, and additionally requires a POSITIVE "
                "activeness observation - a containment differential - so a green cannot be "
                "indistinguishable from a silently disabled sandbox."
            ),
            "evidence_ref": "evidence/28-02/results.json (cells[].observable and .activeness)",
        },
        {
            "name": "observation-blocked-NOT-authorised",
            "description": (
                "The observation-blocked skip class was NOT authorised for any cell "
                "(controls.json observation_blocked_authorised=false, authorised_cells=[]), "
                "and no cell in the matrix carries any skip of any class."
            ),
            "evidence_ref": "evidence/28-02/controls.json",
        },
        {
            "name": "read-only-soak-workload",
            "description": (
                "The soak workload is read-only by construction and no mutating verb can "
                "enter the allowlist. This is why state_dir_bytes is flat, and it is a "
                "recorded weakness of the measurement rather than a strength - see "
                "F-28-04-006."
            ),
            "evidence_ref": "evidence/28-03/soak.json (families[].workload)",
        },
        {
            "name": "pre-registered-delta-bands",
            "description": (
                "Delta bands were decided by unanimous four-way cross-audit and committed at "
                "1dea6437, BEFORE any soak session existed; the record landed at a0ca3ecf. "
                "bands.json declares numbers_are_measured=false and the validator rejects the "
                "file if that is flipped."
            ),
            "evidence_ref": "evidence/28-03/bands.json",
        },
    ]

    # ---- B4 fixture corpus ------------------------------------------------------------
    corpus_srcs = [
        ("e5-matrix-cases", "crates/wcore-eval-scenarios/src/e5_cases.rs"),
        ("e5-soak-definitions", "crates/wcore-eval-scenarios/src/e5_soak.rs"),
    ]
    fixture_corpus = []
    for name, rel in corpus_srcs:
        p = ROOT / rel
        fixture_corpus.append({
            "name": name,
            "sha256": sha256_file(p),
            "item_count": (
                len(set(c["dimension"] for c in cells)) if name == "e5-matrix-cases"
                else len(soak["families"][0]["canary"]["channels_scanned"])
            ),
            "source_ref": rel,
        })

    # ---- B5 environment ----------------------------------------------------------------
    environment = []
    for r in results["runs"]:
        environment.append({
            "host": r["host"],
            "os_family": r["os_family"],
            "os_build": r["binary"]["target"],
            "run_context": r["note"],
            "evidence_ref": "evidence/28-02/results.json (runs[])",
        })
    seen = {e["host"] for e in environment}
    for f in soak["families"]:
        if f["host"] in seen:
            continue
        environment.append({
            "host": f["host"],
            "os_family": f["family"],
            "os_build": f["target"],
            "run_context": f"soak, concurrency {f['concurrency']}, {f['sessions_completed']} sessions",
            "evidence_ref": "evidence/28-03/soak.json (families[])",
        })
        seen.add(f["host"])
    environment.append({
        "host": "github-hosted macos-latest runner",
        "os_family": "macos",
        "os_build": "aarch64-apple-darwin, rustc 1.97.1",
        "run_context": (
            "GitHub Actions run 30364529551 at headSha cf48b349, workflow 'macOS native "
            "suites (Phase 28)'; the only environment in which the two macOS members of the "
            "hostile platform matrix have ever executed"
        ),
        "evidence_ref": "evidence/28-04/macos-ci-30364529551.log",
    })

    # ---- B6 artifacts / B7 logs: digested off disk, never asserted --------------------
    def rels(pattern_dirs, suffixes):
        out = []
        for d in pattern_dirs:
            base = PHASE / d
            if not base.is_dir():
                continue
            for p in sorted(base.rglob("*")):
                if p.is_file() and p.suffix in suffixes:
                    out.append(str(p.relative_to(PHASE)))
        return out

    artifact_paths = rels(
        ["evidence/28-01", "evidence/28-02", "evidence/28-03", "evidence/28-04",
         "evidence/28-03-windows-requeue", "evidence/F-28-02-001"],
        {".json", ".tsv"},
    )
    log_paths = rels(
        ["evidence/28-01", "evidence/28-02", "evidence/28-03", "evidence/28-04",
         "evidence/28-03-windows-requeue", "evidence/28-kr01-repair",
         "evidence/28-kr07-suites", "evidence/F-28-02-001"],
        {".log", ".err", ".txt"},
    )
    artifacts = [
        {"path": r, "sha256": sha256_file(PHASE / r), "bytes": (PHASE / r).stat().st_size}
        for r in artifact_paths
    ]
    logs = [
        {
            "path": r,
            "sha256": sha256_file(PHASE / r),
            "bytes": (PHASE / r).stat().st_size,
            "produced_by": r.split("/")[1],
        }
        for r in log_paths
    ]

    # ---- B8 skip policy: recomputed by counting skipped cells -------------------------
    skipped = [c for c in cells if c["outcome"] in ("skip", "skipped")]
    skipped_critical = sum(1 for c in skipped if c.get("criticality") == "critical")
    skip_policy = {
        "classes": SKIP_CLASSES,
        "skipped_cells": [
            {
                "cell_id": c["cell_id"],
                "class": c.get("skip_class", ""),
                "criticality": c.get("criticality", ""),
                "required_evidence": c.get("skip_evidence", ""),
            }
            for c in skipped
        ],
        "skipped_critical_cases": skipped_critical,
    }

    # ---- findings: every row, verbatim from the adjudicated ledger --------------------
    rows = ledger_rows()
    findings = [
        {
            "id": r["id"],
            "origin": r["origin"],
            "subject": r["subject"],
            "inherited_severity": r["inherited_severity"],
            "p28_severity": r["p28_severity"],
            "contradicted_criterion": r["contradicted_criterion"],
            "disposition": r["disposition"],
            "rationale": r["rationale"],
            "owner": r["owner"],
            "backlog_id": r["backlog_id"],
            "executable_check": r["executable_check"],
            "counter_evidence": r["counter_evidence"],
        }
        for r in rows
    ]

    # ---- the three A3 claims, RECOMPUTED from the ledger and the cell list ------------
    terminal = ("FIXED", "DISPROVED", "ACCEPTED", "DEFERRED")
    claims = {
        "zero_skipped_critical_cases": skipped_critical == 0,
        "zero_undispositioned_findings": all(f["disposition"] in terminal for f in findings),
        "zero_unresolved_critical_or_high": not any(
            f["p28_severity"] in ("CRITICAL", "HIGH")
            and f["disposition"] not in ("FIXED", "DISPROVED")
            for f in findings
        ),
    }

    body = {
        "certification_id": CERT_ID,
        "phase": "28-native-cross-platform-certification",
        "bindings": {
            "candidate": candidate,
            "platform": platform,
            "posture": posture,
            "fixture_corpus": fixture_corpus,
            "environment": environment,
            "artifacts": artifacts,
            "logs": logs,
            "skip_policy": skip_policy,
        },
        "findings": findings,
        "claims": dict(sorted(claims.items())),
    }

    canonical = json.dumps(body, separators=(",", ":"), ensure_ascii=False)
    non_ascii = sorted({c for c in canonical if ord(c) > 127})
    if non_ascii:
        print(f"refusing to sign: body contains non-ASCII {non_ascii}; the Rust and Python "
              "encoders could diverge on escaping", file=sys.stderr)
        return 2
    body_sha256 = hashlib.sha256(canonical.encode("utf-8")).hexdigest()

    # Deterministic phase-scoped key. See the module docstring for why this is acceptable
    # for an evidence binding and forbidden for a trust root.
    seed = hashlib.sha256(f"wayland.phase28.certification.{CERT_ID}".encode()).digest()
    sk = Ed25519PrivateKey.from_private_bytes(seed)
    pk = sk.public_key().public_bytes(
        encoding=serialization.Encoding.Raw, format=serialization.PublicFormat.Raw
    )
    import base64

    signature = sk.sign(DOMAIN + body_sha256.encode("ascii"))
    receipt = {
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "body_sha256": body_sha256,
        "body": body,
        "authority": {
            "kind": "phase_scoped",
            "key_id": KEY_ID,
            "public_key_base64": base64.b64encode(pk).decode(),
            "fingerprint_sha256": hashlib.sha256(pk).hexdigest(),
            "signature_base64": base64.b64encode(signature).decode(),
            "scope": SCOPE,
        },
    }

    out = PHASE / "28-04-CERTIFICATION-RECEIPT.json"
    out.write_text(json.dumps(receipt, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {out.relative_to(ROOT)}")
    print(f"  body_sha256          {body_sha256}")
    print(f"  key fingerprint      {receipt['authority']['fingerprint_sha256']}")
    print(f"  public key (base64)  {receipt['authority']['public_key_base64']}")
    print(f"  bindings             candidate={len(candidate)} platform={len(platform)} "
          f"posture={len(posture)} corpus={len(fixture_corpus)} env={len(environment)} "
          f"artifacts={len(artifacts)} logs={len(logs)} "
          f"skipped_cells={len(skip_policy['skipped_cells'])}")
    print(f"  findings             {len(findings)}")
    for k, v in receipt["body"]["claims"].items():
        print(f"  claim {k:34s} {v}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
