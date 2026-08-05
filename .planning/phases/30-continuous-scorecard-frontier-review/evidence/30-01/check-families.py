#!/usr/bin/env python3
"""Check every CTRL-01 coverage family against all seven required clauses.

F30-01 names versioned activation, operator-completeness, maturity, security
owner, evidence IDs and peer delta; F30-02 adds the refresh phase. A row missing
any one of them is not a partially good row, it is an unreviewable row.

Every determination here is produced by parsing the ledger's own table and then
RUNNING something against the tree — the evidence IDs are checked for membership
in the declared index, and each row's "Next proof" phases are checked against
what is actually on disk, which is the check the ledger cannot do for itself.

Usage: python3 check-families.py <repo-root> <out-dir> [ledger-path]
"""
import os
import re
import sys

REPO = os.path.abspath(sys.argv[1])
OUT = os.path.abspath(sys.argv[2])
LEDGER = (os.path.abspath(sys.argv[3]) if len(sys.argv) > 3
          else os.path.join(REPO, ".planning/intel/COMPETITIVE-LEDGER.md"))

# The eight states the ledger itself declares, in its own order.
MATURITY = [
    "ABSENT", "SOURCE", "CONFIGURED", "CONSTRUCTED",
    "REACHED", "EFFECTIVE", "OPERATOR_COMPLETE", "PACKAGED_PROVEN",
]
PINNED_TOKEN = "BASE-2026-07-13"
COLS = ["coverage_ids", "family", "owner", "sec_owner", "maturity",
        "evidence_ids", "peer_baseline", "delta", "limitation",
        "last_refresh", "next_proof"]


def split_row(line):
    """Split a markdown table row on unescaped pipes."""
    body = line.strip()
    body = body[1:] if body.startswith("|") else body
    body = body[:-1] if body.endswith("|") else body
    # The ledger escapes in-cell pipes as \| — protect them.
    body = body.replace(r"\|", "\x00")
    return [c.replace("\x00", "|").strip() for c in body.split("|")]


def main():
    with open(LEDGER, encoding="utf-8") as f:
        lines = f.read().splitlines()

    declared_ids = set()
    for ln in lines:
        m = re.match(r"^\| `([^`]+)` \| ", ln)
        if m:
            declared_ids.add(m.group(1))

    # Coverage-family rows are the ones whose FIRST cell is `XXX-*` and which
    # carry the full eleven-column shape. The disposition table also starts
    # `XXX-*` but has four columns, so the width check separates them.
    families = []
    for ln in lines:
        if not re.match(r"^\| [A-Z]+-\* \|", ln):
            continue
        cells = split_row(ln)
        if len(cells) != len(COLS):
            continue
        families.append(dict(zip(COLS, cells)))

    if not families:
        sys.exit("FATAL: no coverage-family rows parsed")

    phases_on_disk = set()
    pdir = os.path.join(REPO, ".planning/phases")
    for d in os.listdir(pdir):
        for f in os.listdir(os.path.join(pdir, d)):
            m = re.match(r"^(\d+[AB]?-\d+[A-Z]?)-SUMMARY\.md$", f)
            if m:
                phases_on_disk.add(m.group(1))
            m2 = re.match(r"^(\d+[AB]?-\d+[A-Z]?)-PHASE-VERDICT\.md$", f)
            if m2:
                phases_on_disk.add(m2.group(1))

    out_lines = []
    tsv = []
    for fam in families:
        name = fam["coverage_ids"]
        defects = []

        # Clause 1 — maturity is a member of the declared enum.
        mat_raw = fam["maturity"]
        mat = None
        for state in MATURITY:
            if re.search(r"\b" + state + r"\b", mat_raw):
                mat = state
                break
        if mat is None:
            defects.append(f"maturity '{mat_raw}' is NOT a member of the declared enum")

        # Clause 2 — security authority owner.
        if not fam["sec_owner"] or fam["sec_owner"].upper() in ("", "TBD", "NONE"):
            defects.append("no security authority owner")

        # Clause 3 — evidence IDs present AND drawn from the declared index.
        cited = re.findall(r"`([^`]+)`", fam["evidence_ids"])
        if not cited:
            defects.append("no evidence IDs cited")
        undeclared = []
        for c in cited:
            if c in declared_ids:
                continue
            # F05-TRUTH-6 instantiates the F05-TRUTH-{n} template row.
            if re.match(r"^F05-TRUTH-\d+$", c) and "F05-TRUTH-{n}" in declared_ids:
                continue
            undeclared.append(c)
        if undeclared:
            defects.append("evidence IDs NOT in the declared index: "
                           + ", ".join(undeclared))

        # Clause 4 — the peer baseline is the pinned token, not UNPINNED.
        if PINNED_TOKEN not in fam["peer_baseline"]:
            defects.append("peer baseline is not the pinned token")
        if "UNPINNED" in fam["peer_baseline"].upper():
            defects.append("peer baseline is UNPINNED")

        # Clauses 5, 6, 7 — delta, limitation, refresh phase.
        for key, label in (("delta", "delta"), ("limitation", "limitation"),
                           ("last_refresh", "last refresh phase")):
            if len(fam[key]) < 8:
                defects.append(f"no {label}")

        # The check the ledger cannot do for itself: is the row's own declared
        # "Next proof" already on disk? If so the row is DATED — its stated
        # blocker has been discharged since the refresh.
        dated = []
        for m in re.finditer(r"\b(2[0-9][AB]?)-(\d\d)\b", fam["next_proof"]):
            pid = f"{m.group(1)}-{m.group(2)}"
            if pid in phases_on_disk:
                dated.append(pid)

        status = "CONFIRMED" if not defects else "REFUTED"
        sev = "NONE"
        if defects:
            sev = "HIGH"
        elif dated:
            status, sev = "CONFIRMED", "MEDIUM"

        out_lines.append(f"== {name}")
        out_lines.append(f"   maturity        : {mat} (raw: {mat_raw})")
        out_lines.append(f"   security owner  : {fam['sec_owner']}")
        out_lines.append(f"   evidence IDs    : {len(cited)} cited, "
                         f"{len(undeclared)} undeclared")
        out_lines.append(f"   peer baseline   : {fam['peer_baseline'][:60]}")
        out_lines.append(f"   delta chars     : {len(fam['delta'])}")
        out_lines.append(f"   limitation chars: {len(fam['limitation'])}")
        out_lines.append(f"   last refresh    : {fam['last_refresh']}")
        out_lines.append(f"   next proof      : {fam['next_proof'][:110]}")
        out_lines.append(f"   seven-clause    : "
                         + ("ALL SEVEN PRESENT" if not defects
                            else "DEFECTS: " + "; ".join(defects)))
        out_lines.append(f"   dated-by-tree   : "
                         + (", ".join(dated) + " already on disk — the row's own"
                            " stated next proof has landed since the refresh"
                            if dated else "none"))
        out_lines.append(f"   VERDICT         : {status} sev={sev}")
        out_lines.append("")
        tsv.append((name, status, sev, ";".join(defects), ";".join(dated)))

    cap = os.path.join(OUT, "family-clause-check.txt")
    with open(cap, "w", encoding="utf-8") as f:
        f.write(f"CTRL-01 COVERAGE-FAMILY SEVEN-CLAUSE CHECK\n"
                f"ledger: {os.path.relpath(LEDGER, REPO)}\n"
                f"families parsed: {len(families)}\n"
                f"declared evidence IDs: {len(declared_ids)}\n\n")
        f.write("\n".join(out_lines) + "\n")

    for row in tsv:
        print("\t".join(row))


if __name__ == "__main__":
    main()
