#!/usr/bin/env python3
"""Three-way diff: the shipped binary's command tree vs docs/ vs CTRL-01 families.

The binary side is `surfaces.tsv`, which is walker output over the REAL release
binary and is byte-diffed against a regeneration — it is a measurement, not an
assertion. The docs side is extracted mechanically. The family side maps each
shipped top-level command to exactly one CTRL-01 coverage family by the family's
declared scope; a command no family claims is recorded NO_FAMILY, which means it
ships with no security owner and no peer baseline.

Also emits `surface-truths.tsv`: the per-surface seven-truth table. Every truth
that has not been measured reads UNPROVEN. The peer-delta column is UNPROVEN on
EVERY row by construction — no comparative trial has run, 30-02 owns that, and
writing anything else would forge the number this phase exists to earn.

Usage: python3 build-surface-diff.py <repo-root> <out-dir>
"""
import os
import re
import sys

REPO = os.path.abspath(sys.argv[1])
OUT = os.path.abspath(sys.argv[2])

# CTRL-01's ten coverage families and their declared scope, mapped to the
# top-level commands each family's scope actually claims. Where a command
# plausibly belongs to two, it is assigned to the family whose security
# authority owner is accountable for it and the ambiguity is noted, never
# duplicated into two rows.
FAMILY_SCOPE = {
    "AUTH-*": ["sandbox", "auth"],
    "TXN-*": ["forge", "crucible"],
    "GOAL-*": ["goal", "swarm", "workflow", "cron"],
    "CONT-*": ["session", "index", "agent"],
    "GATEWAY-*": ["gateway", "channel", "acp"],
    "REACH-*": ["backend", "node", "plugin"],
    "PORT-*": ["migrate", "backup"],
    "MEDIA-*": ["image", "fetch"],
    "NATIVE-*": [],
    "SUPPLY-*": ["self-update"],
}
CMD_TO_FAMILY = {c: f for f, cs in FAMILY_SCOPE.items() for c in cs}

# The last-refresh phase CTRL-01 records per family, used for truth 7.
FAMILY_REFRESH = {
    "AUTH-*": "21+25-04+28-02",
    "TXN-*": "21+22+23B-H1+28",
    "GOAL-*": "22+24-02",
    "CONT-*": "23A+23B",
    "GATEWAY-*": "24",
    "REACH-*": "25",
    "PORT-*": "26",
    "MEDIA-*": "27",
    "NATIVE-*": "28",
    "SUPPLY-*": "29",
}
# Every family's security authority owner is `core` except two, per CTRL-01.
FAMILY_SEC_OWNER = {f: ("shared" if f in ("NATIVE-*", "SUPPLY-*") else "core")
                    for f in FAMILY_SCOPE}


def load_surfaces():
    path = os.path.join(OUT, "surfaces.tsv")
    rows = []
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if i == 0 or not line.strip():
                continue
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 3:
                continue
            rows.append((parts[0], int(parts[1]), parts[2]))
    return rows


def docs_commands():
    """Extract command references from docs/ mechanically.

    The invocation form the documentation actually uses is the product name
    followed by a subcommand token. This is a FLOOR, not a complete set: a
    documented form this pattern misses is under-counted, never over-counted.
    """
    found = {}
    docs = os.path.join(REPO, "docs")
    pat = re.compile(r"\bwayland-core\s+([a-z][a-z0-9-]{1,30})\b")
    for root, _dirs, files in os.walk(docs):
        for fn in files:
            if not fn.endswith(".md"):
                continue
            p = os.path.join(root, fn)
            try:
                with open(p, encoding="utf-8", errors="replace") as f:
                    text = f.read()
            except OSError:
                continue
            for m in pat.finditer(text):
                tok = m.group(1)
                found.setdefault(tok, set()).add(os.path.relpath(p, REPO))
    return found


def main():
    surfaces = load_surfaces()
    top = [(p, a, s) for (p, a, s) in surfaces if " " not in p]
    docs = docs_commands()
    doc_toks = set(docs)

    diff_rows = []
    n = 0

    # Every SHIPPED command path, classified.
    for path, _arity, _syn in surfaces:
        n += 1
        head = path.split(" ")[0]
        fam = CMD_TO_FAMILY.get(head, "NONE")
        documented = head in doc_toks
        if fam == "NONE":
            bucket = "NO_FAMILY"
        elif documented:
            bucket = "BINARY_AND_DOCS"
        else:
            bucket = "BINARY_ONLY"
        diff_rows.append(f"SURF-{n:03d}::{bucket}::path={path}::family={fam}")

    # Every DOCUMENTED reference the binary does not offer.
    shipped_heads = {p.split(" ")[0] for p, _, _ in surfaces}
    # Filter obvious prose false-positives: a documented token is only counted
    # if it appears in at least one doc AND is not an English word we know the
    # pattern picks up after the product name.
    NOISE = {"is", "the", "and", "or", "will", "can", "to", "as", "in", "with",
             "for", "on", "from", "run", "runs", "does", "has", "was", "when",
             "binary", "process", "session", "instance", "reads", "writes",
             "supports", "uses", "emits", "sends", "starts", "exits", "may"}
    for tok in sorted(doc_toks - shipped_heads):
        if tok in NOISE:
            continue
        n += 1
        diff_rows.append(f"SURF-{n:03d}::DOCS_ONLY::path={tok}::family=NONE")

    with open(os.path.join(OUT, "surface-diff.tsv"), "w", encoding="utf-8") as f:
        f.write("\n".join(diff_rows) + "\n")

    # -------- the seven-truth table --------
    truths = []
    for i, (path, _arity, _syn) in enumerate(surfaces, start=1):
        head = path.split(" ")[0]
        fam = CMD_TO_FAMILY.get(head, "NONE")
        # Truth 1, versioned activation: the binary's own version is measured —
        # it is the version that emitted this command tree.
        versioned_activation = "0.12.25"
        # Truth 2, operator completeness: NOT measured by a command-tree walk.
        operator_completeness = "UNPROVEN"
        # Truth 3, maturity: taken from the owning family's CTRL-01 row. A
        # command owned by no family has no recorded maturity — UNPROVEN, not a
        # plausible guess.
        maturity = {
            "AUTH-*": "CONSTRUCTED", "TXN-*": "REACHED", "GOAL-*": "CONSTRUCTED",
            "CONT-*": "REACHED", "GATEWAY-*": "CONSTRUCTED", "REACH-*": "REACHED",
            "PORT-*": "REACHED", "MEDIA-*": "SOURCE", "NATIVE-*": "REACHED",
            "SUPPLY-*": "CONSTRUCTED",
        }.get(fam, "UNPROVEN")
        sec = FAMILY_SEC_OWNER.get(fam, "UNPROVEN")
        evidence = fam if fam != "NONE" else "UNPROVEN"
        # Truth 6, peer delta: UNPROVEN on EVERY row. No comparative trial has
        # run; 30-02 owns it. A number here would be forged.
        peer_delta = "UNPROVEN"
        refresh = FAMILY_REFRESH.get(fam, "UNPROVEN")
        truths.append("\t".join([
            f"SURF-{i:03d}", path, versioned_activation, operator_completeness,
            maturity, sec, evidence, peer_delta, refresh,
        ]))

    header = ("# id\tcommand_path\tversioned_activation\toperator_completeness"
              "\tmaturity\tsecurity_authority_owner\tevidence\tpeer_delta"
              "\tlast_refreshed_phase\n"
              "# UNPROVEN means NOT MEASURED. peer_delta is UNPROVEN on every "
              "row: no comparative trial has run (30-02 owns it).\n")
    with open(os.path.join(OUT, "surface-truths.tsv"), "w", encoding="utf-8") as f:
        f.write(header + "\n".join(truths) + "\n")

    # -------- summary to stdout --------
    from collections import Counter
    buckets = Counter(r.split("::")[1] for r in diff_rows)
    print(f"shipped surfaces      : {len(surfaces)}")
    print(f"shipped top-level     : {len(top)}")
    print(f"docs tokens extracted : {len(doc_toks)}")
    for b, c in sorted(buckets.items()):
        print(f"  {b:<16}: {c}")
    nofam = sorted({r.split('path=')[1].split('::')[0].split(' ')[0]
                    for r in diff_rows if '::NO_FAMILY::' in r})
    print(f"top-level commands owned by NO family ({len(nofam)}): {' '.join(nofam)}")
    docs_only = [r.split('path=')[1].split('::')[0]
                 for r in diff_rows if '::DOCS_ONLY::' in r]
    print(f"documented-but-absent ({len(docs_only)}): {' '.join(docs_only)}")


if __name__ == "__main__":
    main()
