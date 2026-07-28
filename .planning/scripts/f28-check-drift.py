#!/usr/bin/env python3
"""f28-check-drift.py — does the certified candidate still describe the code we ship?

**The gap this closes.** `f28-verify-bindings.py` recomputes every binding in a Phase 28
certification receipt against the raw evidence, and it is thorough. But every one of its
checks is *internal*: `bindings.candidate[].commit` is compared against
`evidence/28-0{2,3}/candidate.json`, which is the ledger that commit was resolved FROM.
Nothing in the toolchain ever compares the certified commit to the tree the project is
actually on. Measured: zero git invocations across all six `f28-*.py` scripts.

The consequence is an asymmetry that was measured, not argued:

  * appending ONE comment line to `crates/wcore-eval-scenarios/src/e5_cases.rs` — a file in
    the TEST HARNESS — takes `--verify` to rc=1 with `F28V-CORPUS`, because the corpus digest
    is recomputed against the live working tree;
  * moving the PRODUCT 194 commits forward under `crates/`, including the three commits that
    repaired the very finding whose closure flipped the acceptance gate to passing, leaves
    `--verify` at rc=0.

A 29-byte edit to the ruler fails the gate. 194 commits to the thing being measured do not.
So a certification can be simultaneously valid and stale, and the record had no way to say so.

This script says so. It is deliberately NOT an acceptance gate — staleness is not a defect in
the certification, it is a fact about its scope. Run it wherever a reader might otherwise
assume the passing gate covers the current tree.

Exit codes: 0 = the receipt's candidates are current for the paths checked.
            1 = drift (or divergence) found; every finding names its code.
            2 = usage / missing input.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# Default measurement paths: the PRODUCT. Not `.planning/`, which moves constantly and whose
# movement says nothing about whether the certified binary still resembles what we ship.
DEFAULT_PATHS = ("crates/",)


@dataclass(frozen=True)
class Finding:
    code: str
    where: str
    detail: str

    def __str__(self) -> str:
        return f"  {self.code}  {self.where}: {self.detail}"


def git(*args: str, repo: Path = ROOT) -> tuple[int, str]:
    """Run git and return (rc, stdout). Never routed through a shell, never through rtk."""
    proc = subprocess.run(
        ["/usr/bin/git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        check=False,
    )
    return proc.returncode, proc.stdout.strip()


def candidates_of(receipt: dict) -> list[dict]:
    body = receipt.get("body", receipt)
    return body.get("bindings", {}).get("candidate", [])


def measure(
    receipt: dict, ref: str, paths: tuple[str, ...], repo: Path = ROOT
) -> tuple[list[Finding], list[dict]]:
    """Compare every bound candidate against `ref`. Returns (findings, per-candidate report)."""
    findings: list[Finding] = []
    report: list[dict] = []

    rc, resolved_ref = git("rev-parse", "--verify", f"{ref}^{{commit}}", repo=repo)
    if rc != 0:
        return [Finding("F28D-000", ref, "ref does not resolve to a commit in this repo")], []

    cands = candidates_of(receipt)
    if not cands:
        return [Finding("F28D-004", "bindings.candidate", "receipt binds no candidate at all")], []

    for cand in cands:
        scope = cand.get("scope", "<no scope>")
        commit = cand.get("commit", "")
        row: dict = {"scope": scope, "commit": commit, "ref": resolved_ref}

        rc, _ = git("cat-file", "-e", f"{commit}^{{commit}}", repo=repo)
        if rc != 0:
            findings.append(
                Finding(
                    "F28D-001",
                    f"candidate[{scope}]",
                    f"certified commit {commit} is not present in this repository, so the "
                    "certification's currency CANNOT be assessed here. That is not a pass.",
                )
            )
            row["status"] = "UNASSESSABLE"
            report.append(row)
            continue

        if commit == resolved_ref:
            row.update(status="CURRENT", total=0, scoped=0)
            report.append(row)
            continue

        anc_rc, _ = git("merge-base", "--is-ancestor", commit, resolved_ref, repo=repo)
        if anc_rc != 0:
            findings.append(
                Finding(
                    "F28D-002",
                    f"candidate[{scope}]",
                    f"certified commit {commit[:8]} is NOT an ancestor of {ref} "
                    f"({resolved_ref[:8]}); the certified code DIVERGED rather than merely "
                    "aged, so no commit count describes the gap",
                )
            )
            row["status"] = "DIVERGED"
            report.append(row)
            continue

        _, total = git("rev-list", "--count", f"{commit}..{resolved_ref}", repo=repo)
        _, scoped = git(
            "rev-list", "--count", f"{commit}..{resolved_ref}", "--", *paths, repo=repo
        )
        _, changed = git(
            "diff", "--name-only", commit, resolved_ref, "--", *paths, repo=repo
        )
        files = [f for f in changed.splitlines() if f.strip()]
        row.update(
            status="STALE" if int(scoped or 0) else "CURRENT",
            total=int(total or 0),
            scoped=int(scoped or 0),
            files_changed=len(files),
            files=files[:40],
        )
        report.append(row)

        if int(scoped or 0) > 0:
            findings.append(
                Finding(
                    "F28D-003",
                    f"candidate[{scope}]",
                    f"certified commit {commit[:8]} is {scoped} commit(s) behind {ref} under "
                    f"{','.join(paths)} ({total} overall), across {len(files)} changed file(s). "
                    "The certification is VALID for that commit and STALE for this ref; both "
                    "are true and neither cancels the other.",
                )
            )

    return findings, report


# ---------------------------------------------------------------------------------------
# Self-test. THREE assertions, because two is not enough (LANE-BRIEF §6b-ii).
#   1. known-negative  — a candidate equal to the ref must NOT be reported stale.
#   2. known-positive  — a candidate genuinely behind the ref MUST be reported stale,
#                        with code F28D-003 specifically, not merely a non-zero exit.
#   3. the-old-instrument-missed-it — `f28-verify-bindings.py --verify` returns 0 on the very
#                        receipt this reports stale. Without this third assertion the self-test
#                        would pass just as happily on an instrument that adds nothing.
# ---------------------------------------------------------------------------------------

def self_test() -> list[str]:
    failures: list[str] = []
    receipt_path = (
        ROOT
        / ".planning/phases/28-native-cross-platform-certification"
        / "28-04-CERTIFICATION-RECEIPT-SUPERSEDING-001.json"
    )
    verifier = ROOT / ".planning/scripts/f28-verify-bindings.py"

    # A missing fixture must FAIL, never skip. A self-test that skips its own assertions is
    # the vacuity shape this program has now measured three separate ways.
    if not receipt_path.is_file():
        return [f"fixture absent: {receipt_path} — cannot self-test, and a skip is not a pass"]
    if not verifier.is_file():
        return [f"fixture absent: {verifier} — assertion 3 cannot run, and a skip is not a pass"]

    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    cands = candidates_of(receipt)
    if not cands:
        return ["fixture receipt binds no candidate"]

    _, head = git("rev-parse", "--verify", "HEAD^{commit}")

    # --- 1. known-negative: rebind every candidate to HEAD itself. -----------------------
    neg = json.loads(json.dumps(receipt))
    for c in candidates_of(neg):
        c["commit"] = head
    neg_findings, _ = measure(neg, "HEAD", DEFAULT_PATHS)
    if neg_findings:
        failures.append(
            "assertion 1 (known-negative): a candidate identical to HEAD was reported as "
            f"drifted: {[f.code for f in neg_findings]}"
        )

    # --- 2. known-positive: the real, unmodified receipt. --------------------------------
    pos_findings, pos_report = measure(receipt, "HEAD", DEFAULT_PATHS)
    if not any(f.code == "F28D-003" for f in pos_findings):
        failures.append(
            "assertion 2 (known-positive): the real receipt's candidates were NOT reported "
            f"stale against HEAD; codes seen: {[f.code for f in pos_findings]}"
        )
    if not any(r.get("scoped", 0) > 0 for r in pos_report):
        failures.append("assertion 2 (known-positive): measured zero scoped drift")

    # --- 3. the old instrument missed it. ------------------------------------------------
    proc = subprocess.run(
        [sys.executable, str(verifier), "--verify", str(receipt_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        failures.append(
            "assertion 3 (old-instrument-missed-it): f28-verify-bindings.py --verify returned "
            f"rc={proc.returncode} on the fixture receipt. The asymmetry this script exists to "
            "close is not currently demonstrable, so the self-test cannot claim it. Output:\n"
            + proc.stdout.strip()[-800:]
        )

    return failures


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        description="report how far a Phase 28 certification receipt's bound candidates have "
        "drifted from a given ref"
    )
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--self-test", action="store_true")
    g.add_argument("--receipt", metavar="RECEIPT.json")
    p.add_argument("--ref", default="HEAD", help="ref to measure against (default: HEAD)")
    p.add_argument(
        "--paths",
        nargs="+",
        default=list(DEFAULT_PATHS),
        help="pathspecs that define the PRODUCT (default: crates/)",
    )
    p.add_argument("--json", action="store_true", help="emit the per-candidate report as JSON")
    args = p.parse_args(argv if argv is not None else sys.argv[1:])

    if args.self_test:
        failures = self_test()
        for f in failures:
            print(f"SELF-TEST FAILURE: {f}", file=sys.stderr)
        print(
            f"--self-test: 3 assertions, {len(failures)} failure(s) "
            "(known-negative / known-positive / old-instrument-missed-it)"
        )
        return 1 if failures else 0

    rp = Path(args.receipt)
    if not rp.is_file():
        rp = ROOT / args.receipt
    if not rp.is_file():
        print(f"--receipt: {args.receipt} is not on disk", file=sys.stderr)
        return 2

    receipt = json.loads(rp.read_text(encoding="utf-8"))
    findings, report = measure(receipt, args.ref, tuple(args.paths))

    if args.json:
        print(json.dumps({"ref": args.ref, "paths": args.paths, "candidates": report}, indent=2))
    else:
        for row in report:
            print(
                f"  {row['scope']}: {row.get('status')} commit={row['commit'][:8]} "
                f"scoped_behind={row.get('scoped', '?')} total_behind={row.get('total', '?')} "
                f"files_changed={row.get('files_changed', '?')}"
            )

    if findings:
        print(f"--check-drift: DRIFT ({len(findings)}) against {args.ref}")
        for f in findings:
            print(str(f))
        return 1
    print(f"--check-drift: current against {args.ref} for {','.join(args.paths)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
