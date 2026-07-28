#!/usr/bin/env python3
"""
check-finding-disposition.py — catch assertions about work that was never performed.

Two shapes, both measured on this program, both of which pass every review that
reads the CLAIM rather than the EFFECT.

  SHAPE 1 — "filed to BACKLOG" and it is not there.
      A summary or seam request says a finding was routed to BACKLOG.md.
      The id appears nowhere in BACKLOG.md. The finding is not dispositioned,
      it is dropped. Measured twice on 2026-07-28/29: 18 findings across
      Phases 21, 22, 23A, 23B, 24, 26 and 27, two of them HIGH.

  SHAPE 2 — a mutation gate that mutates nothing.
      A gate proves falsifiability by forcing a value and asserting refusal.
      Its `sed` targets a field that does not exist, so it rewrites nothing,
      verifies an unchanged document, and proves nothing. Measured on
      2026-07-29 in 30-04: `sed 's#"verdict": *"NOT_MET"#...'` against a
      document whose field is `grade` (BL-F30-FORCED-MET-SED).

SHAPE 1 reads a claim and checks the record. SHAPE 2 refuses to infer at all:
it APPLIES the mutation to the real target file and asserts the bytes changed.

Exit 0 = clean, 1 = findings, 2 = usage/internal error.
Run `--self-test` to prove the instrument can go red before trusting it.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

# --------------------------------------------------------------------------
# SHAPE 1 — filing claims
# --------------------------------------------------------------------------

# Phrases that assert a finding reached BACKLOG.md.
CLAIM_PATTERNS = [
    r"filed to\s+BACKLOG",
    r"files?\s+(?:every\s+)?(?:residual|finding|it)?\s*to\s+BACKLOG",
    r"(?:→|->)\s*BACKLOG",
    r"goes?\s+to\s+BACKLOG",
    r"logged\s+to\s+BACKLOG",
    r"BACKLOG\s+per\s+the\s+standing",
    r"MEDIUM\s+and\s+below\s+(?:are\s+)?(?:go|logged|filed)",
]
CLAIM_RE = re.compile("|".join(CLAIM_PATTERNS), re.IGNORECASE)

# Finding ids used on this program. Deliberately broad but anchored on a
# digit-bearing phase token so prose words never match.
ID_RE = re.compile(
    r"\b(?:"
    r"F\d{2}[A-Z]?-[A-Z0-9]+(?:-[A-Z0-9]+)*"      # F23A-01-H2, F22-M1, F29-CEN-06
    r"|F-\d{2}[A-Z]?-[A-Z0-9]+(?:-[A-Z0-9]+)*"     # F-28-02-003
    r"|BL-F\d{2}[A-Z]?-[A-Z0-9-]+"                 # BL-F30-FORCED-MET-SED
    r"|\d{2}[A-Z]?-[MHL]\d+"                       # 23B-H1, 23B-M4
    r"|F-KR-\d+"                                   # F-KR-09
    r")\b"
)

# Ids that are references to a phase/plan rather than a finding.
ID_DENY_RE = re.compile(r"^(?:F\d{2}-\d{2}|F-\d{2}-\d{2})$")


@dataclass
class Dropped:
    ident: str
    severity: str
    source: str
    line: int


def severity_near(text: str, pos: int, window: int = 400) -> str:
    """Best-effort severity read from the neighbourhood of the id."""
    chunk = text[max(0, pos - window) : pos + window].upper()
    for sev in ("CRITICAL", "HIGH", "MEDIUM", "LOW", "INFO"):
        if sev in chunk:
            return sev
    return "UNSPECIFIED"


def _sections(text: str) -> list[tuple[int, str]]:
    """
    Split a markdown document into sections at headings and `---` rules.

    SCOPE IS A SECTION, NOT A LINE. This is load-bearing. The first version of
    this checker matched claim-and-id on the SAME line and therefore missed the
    whole `.planning/SEAM-REQUESTS/23A.md` case, where the claim is a
    `**File:** .planning/BACKLOG.md` header and the four ids sit in a fenced
    block several lines below. That is the same line-oriented under-detection
    LANE-BRIEF §6b-ii records (a wrapped phrase invisible to a line matcher),
    reproduced inside the very instrument built to hunt it. Repaired here rather
    than written up — assertion A4 in the self-test pins it.
    """
    out: list[tuple[int, str]] = []
    start = 1
    buf: list[str] = []
    for lineno, line in enumerate(text.splitlines(), start=1):
        if re.match(r"^(#{1,4}\s|---\s*$)", line) and buf:
            out.append((start, "\n".join(buf)))
            buf = [line]
            start = lineno
        else:
            buf.append(line)
    if buf:
        out.append((start, "\n".join(buf)))
    return out


# A seam request declaring BACKLOG.md as its destination file is a filing claim
# even though it never uses the word "filed".
FILE_TARGET_RE = re.compile(r"\*\*File:\*\*\s*`?\.planning/BACKLOG\.md", re.IGNORECASE)

# Documents whose ids are dispositions rather than policy boilerplate. PLAN.md is
# excluded deliberately: it restates "MEDIUM and below go to BACKLOG" as a
# standing rule next to ids it is not claiming to have filed.
SCAN_SUFFIXES = ("SUMMARY.md", "VERDICT.md", "GAPS-SUMMARY.md", "REVERIFICATION.md")


def _is_scannable(rel: str) -> bool:
    if rel.endswith("BACKLOG.md") or "RECORD-RECONCILIATION" in rel or "RECORD-TRUTH-NOTES" in rel:
        return False
    if "/ARCHIVE/" in rel:
        return False
    if "/SEAM-REQUESTS/" in rel:
        return True
    return rel.endswith(SCAN_SUFFIXES)


def scan_filing_claims(root: Path, backlog: Path) -> list[Dropped]:
    """SHAPE 1. Every id asserted as filed must be present in BACKLOG.md."""
    backlog_text = backlog.read_text(encoding="utf-8", errors="replace")
    planning = root / ".planning"
    dropped: list[Dropped] = []
    seen: set[tuple[str, str]] = set()

    for path in sorted(planning.rglob("*.md")):
        rel = path.relative_to(root).as_posix()
        if not _is_scannable(rel):
            continue

        text = path.read_text(encoding="utf-8", errors="replace")
        if not (CLAIM_RE.search(text) or FILE_TARGET_RE.search(text)):
            continue

        for start, section in _sections(text):
            if not (CLAIM_RE.search(section) or FILE_TARGET_RE.search(section)):
                continue
            for m in ID_RE.finditer(section):
                ident = m.group(0)
                if ID_DENY_RE.match(ident):
                    continue
                if ident in backlog_text:
                    continue
                key = (ident, rel)
                if key in seen:
                    continue
                seen.add(key)
                dropped.append(
                    Dropped(
                        ident=ident,
                        severity=severity_near(section, m.start()),
                        source=f"{rel}:{start}",
                        line=start,
                    )
                )
    return dropped


# --------------------------------------------------------------------------
# SHAPE 2 — no-op mutation gates
# --------------------------------------------------------------------------

# A sed substitution: s<delim>PATTERN<delim>REPLACEMENT<delim>
SED_RE = re.compile(r"sed\s+(?:-[a-zA-Z]+\s+)*'s(?P<d>[#|/@!])(?P<pat>.+?)(?P=d)(?P<rep>.*?)(?P=d)'")


@dataclass
class NoOpGate:
    source: str
    line: int
    pattern: str
    target: str
    reason: str


def _unescape_shell(s: str) -> str:
    """Plan gates are XML-escaped shell. Undo the two escapings that matter."""
    return (
        s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", '"')
        .replace('\\"', '"')
    )


def _find_target(line: str, root: Path) -> Path | None:
    """
    The first repo-relative path on the line that exists and looks like a document.

    NOTE the `./` handling. An earlier version used `cand.lstrip("./")`, which
    strips leading `.` AND `/` characters individually — so `.planning/...`
    became `planning/...`, resolved to nothing, and the checker silently found
    NOTHING to check on the one real historical case it was built for. It
    reported CLEAN by failing to look. That is the self-passing shape this
    script exists to catch, committed by the script itself; assertion B4 pins
    the repair.
    """
    for cand in re.findall(r"[\w./-]*\.(?:json|tsv|md|txt|toml)\b", line):
        if cand.startswith("./"):
            cand = cand[2:]
        # An absolute path is a scratch/host path (`/tmp/...`), not a repo
        # document. `root / "/tmp/x"` silently yields `/tmp/x` under pathlib
        # join semantics, which both escapes the repo and crashes the later
        # `relative_to`. Reject it outright.
        if cand.startswith("/"):
            continue
        p = root / cand
        if p.is_file():
            return p
    return None


def _sed_changes_file(pattern: str, target: Path) -> bool:
    """Does this pattern actually match anything in the target? Measure, don't infer."""
    text = target.read_text(encoding="utf-8", errors="replace")
    # sed BRE ' *' etc. are close enough to Python re for the field-name case;
    # if the pattern will not compile we report it as unverifiable rather than clean.
    try:
        return re.search(pattern, text) is not None
    except re.error:
        try:
            return re.search(re.escape(pattern), text) is not None
        except re.error:
            return True  # cannot judge — do not raise a false positive


def scan_mutation_gates(root: Path, extra_roots: list[Path] | None = None) -> list[NoOpGate]:
    """SHAPE 2. A mutation gate whose sed matches nothing in its own target."""
    findings: list[NoOpGate] = []
    search_roots = [root / ".planning"] + list(extra_roots or [])

    for base in search_roots:
        if not base.exists():
            continue
        for path in sorted(base.rglob("*.md")):
            rel = path.relative_to(root).as_posix() if root in path.parents or path.is_relative_to(root) else str(path)
            if "/ARCHIVE/" in rel:
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            if "sed" not in text:
                continue
            for lineno, line in enumerate(text.splitlines(), start=1):
                clean = _unescape_shell(line)
                for m in SED_RE.finditer(clean):
                    pattern = m.group("pat")
                    # PRECISION. Only a file passed as a direct ARGUMENT to this
                    # sed is its target. Everything after the expression up to
                    # the next pipe, redirect or `&&` is that argument list.
                    # Without this, a pipeline sed (`find ... | sed 's/^\.\///'`)
                    # is mis-paired with whatever `.md` happens to appear later
                    # on the line, which produced 5 false positives on the first
                    # real run. A checker that cries wolf gets switched off.
                    tail = re.split(r"[|>]|&&", clean[m.end():], maxsplit=1)[0]
                    target = _find_target(tail, root)
                    if target is None:
                        continue
                    if not _sed_changes_file(pattern, target):
                        findings.append(
                            NoOpGate(
                                source=f"{rel}:{lineno}",
                                line=lineno,
                                pattern=pattern,
                                target=target.relative_to(root).as_posix(),
                                reason="sed pattern matches nothing in its target — the mutation is a no-op, "
                                "so any gate asserting a forced value was refused proves nothing",
                            )
                        )
    return findings


# --------------------------------------------------------------------------
# Self-test — three assertions, per LANE-BRIEF §6b-ii
# --------------------------------------------------------------------------

def self_test(root: Path) -> int:
    """
    Known-positive fails, known-negative passes, and the NAIVE matcher misses it.
    The third assertion is the only one that proves the repair does anything.
    """
    import tempfile

    ok = True

    def report(name: str, passed: bool, detail: str = "") -> None:
        nonlocal ok
        print(f"  [{'PASS' if passed else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
        if not passed:
            ok = False

    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        (tmp / ".planning").mkdir()
        backlog = tmp / ".planning" / "BACKLOG.md"

        # ---------- SHAPE 1 ----------
        print("SHAPE 1 — filing claims:")

        # A1 known-positive: a claim whose id is absent must be caught.
        backlog.write_text("# BACKLOG\n\n### F29-03-04 — present (MEDIUM)\n", encoding="utf-8")
        (tmp / ".planning" / "s1-SUMMARY.md").write_text(
            "| F23A-01-H2 | HIGH | any errored tool call kills the session | -> BACKLOG |\n",
            encoding="utf-8",
        )
        hits = scan_filing_claims(tmp, backlog)
        report(
            "A1 known-positive: absent id behind a filing claim is RED",
            any(h.ident == "F23A-01-H2" for h in hits),
            f"{len(hits)} hit(s)",
        )

        # A2 known-negative: the same claim, id actually present, must be green.
        backlog.write_text(
            "# BACKLOG\n\n### F23A-01-H2 — filed for real (HIGH)\n", encoding="utf-8"
        )
        hits = scan_filing_claims(tmp, backlog)
        report(
            "A2 known-negative: id present in BACKLOG is GREEN",
            not any(h.ident == "F23A-01-H2" for h in hits),
            f"{len(hits)} hit(s)",
        )

        # A3 the naive matcher misses it. The naive check this program actually
        # used was "does the summary SAY it filed?" — which is true in both
        # cases above and therefore cannot distinguish them.
        naive = lambda txt: bool(CLAIM_RE.search(txt))  # noqa: E731
        s1 = (tmp / ".planning" / "s1-SUMMARY.md").read_text(encoding="utf-8")
        backlog.write_text("# BACKLOG\n", encoding="utf-8")  # id absent again
        report(
            "A3 the naive claim-reading matcher MISSES it (says filed, is not)",
            naive(s1) and any(h.ident == "F23A-01-H2" for h in scan_filing_claims(tmp, backlog)),
            "naive=filed, effect=absent",
        )

        # A4 the REAL 23A shape: a seam-request section whose claim is a
        # `**File:** .planning/BACKLOG.md` header, with the ids several lines
        # below in a fenced block. A line-scoped matcher — the first version of
        # this very script — misses this entirely. This assertion is the one
        # that proves the section-scoping repair does anything.
        (tmp / ".planning" / "s1-SUMMARY.md").unlink()
        sr = tmp / ".planning" / "SEAM-REQUESTS"
        sr.mkdir()
        (sr / "23A.md").write_text(
            "## SR-23A-1 — three MEDIUM findings\n"
            "\n"
            "**File:** `.planning/BACKLOG.md`\n"
            "**Insertion point:** append as new rows.\n"
            "\n"
            "```markdown\n"
            "| F23A-01-M1 | MEDIUM | wcore-agent | skill hooks gated only by accident |\n"
            "| F23A-01-M2 | MEDIUM | wcore-skills | quarantine verdict from mutable files |\n"
            "```\n",
            encoding="utf-8",
        )
        backlog.write_text("# BACKLOG\n", encoding="utf-8")
        section_hits = {h.ident for h in scan_filing_claims(tmp, backlog)}

        # The old, line-scoped matcher, reconstructed exactly.
        def line_scoped(text: str) -> set[str]:
            found = set()
            for line in text.splitlines():
                if CLAIM_RE.search(line):
                    found |= {m.group(0) for m in ID_RE.finditer(line)}
            return found

        old_hits = line_scoped((sr / "23A.md").read_text(encoding="utf-8"))
        report(
            "A4 section-scoped catches the real 23A shape; the old line-scoped matcher MISSED it",
            {"F23A-01-M1", "F23A-01-M2"} <= section_hits and not old_hits,
            f"section={sorted(section_hits)} old={sorted(old_hits)}",
        )

        # ---------- SHAPE 2 ----------
        print("SHAPE 2 — no-op mutation gates:")

        doc = tmp / "phase-verdict.json"
        doc.write_text(
            '{"criteria":[{"id":"CRIT-01","statement":"x","grade":"NOT_MET","evidence":[]}]}\n',
            encoding="utf-8",
        )

        # B1 known-positive: the real 30-04 gate, which seds "verdict".
        (tmp / ".planning" / "p-bad.md").write_text(
            "<automated>sed 's#\"verdict\": *\"NOT_MET\"#\"verdict\": \"MET\"#' phase-verdict.json &gt; /tmp/f.json</automated>\n",
            encoding="utf-8",
        )
        hits2 = scan_mutation_gates(tmp)
        report(
            "B1 known-positive: the real BL-F30-FORCED-MET-SED gate is RED",
            any("p-bad.md" in h.source for h in hits2),
            f"{len(hits2)} hit(s)",
        )

        # B2 known-negative: the corrected form, which seds "grade".
        (tmp / ".planning" / "p-bad.md").unlink()
        (tmp / ".planning" / "p-good.md").write_text(
            "<automated>sed 's#\"grade\": *\"NOT_MET\"#\"grade\": \"MET\"#' phase-verdict.json &gt; /tmp/f.json</automated>\n",
            encoding="utf-8",
        )
        hits2 = scan_mutation_gates(tmp)
        report(
            "B2 known-negative: the corrected 'grade' gate is GREEN",
            not any("p-good.md" in h.source for h in hits2),
            f"{len(hits2)} hit(s)",
        )

        # B3 the naive matcher misses it. The obvious check is "does the word
        # 'verdict' appear in the repo?" — it does, as a TYPE name and a FILE
        # name, so a substring grep passes on the broken gate.
        repo_ish = 'pub struct CriterionVerdictV1 {}\n// see phase-verdict.json\n'
        naive_grep_passes = "verdict" in repo_ish
        (tmp / ".planning" / "p-good.md").unlink()
        (tmp / ".planning" / "p-bad.md").write_text(
            "<automated>sed 's#\"verdict\": *\"NOT_MET\"#\"verdict\": \"MET\"#' phase-verdict.json &gt; /tmp/f.json</automated>\n",
            encoding="utf-8",
        )
        report(
            "B3 the naive substring grep for 'verdict' MISSES it (type + filename hits)",
            naive_grep_passes and any("p-bad.md" in h.source for h in scan_mutation_gates(tmp)),
            "naive=found, key-position=absent",
        )

        # B4 the dotted-path resolver. The real gate names its target as
        # `.planning/phases/.../phase-verdict.json`. The first version of
        # `_find_target` used `lstrip("./")`, which strips leading `.` and `/`
        # CHARACTERS, turning `.planning` into `planning` — so the target never
        # resolved, nothing was checked, and the scan reported CLEAN by failing
        # to look. Assertion B4 is the only one that proves the repair does
        # anything; without it the suite passes on the broken resolver too.
        deep = tmp / ".planning" / "phases" / "30-x" / "evidence"
        deep.mkdir(parents=True)
        deep_doc = deep / "phase-verdict.json"
        deep_doc.write_text('{"criteria":[{"id":"C1","grade":"NOT_MET"}]}\n', encoding="utf-8")
        gate_line = (
            "<automated>ssh host \"sed 's#\\\"verdict\\\": *\\\"NOT_MET\\\"#\\\"verdict\\\": \\\"MET\\\"#' "
            ".planning/phases/30-x/evidence/phase-verdict.json &gt; /tmp/f.json\"</automated>\n"
        )
        (tmp / ".planning" / "p-deep.md").write_text(gate_line, encoding="utf-8")

        def old_find_target(line: str, root: Path) -> Path | None:
            for cand in re.findall(r"[\w./-]*\.(?:json|tsv|md|txt|toml)\b", line):
                p = root / cand.lstrip("./")
                if p.is_file():
                    return p
            return None

        old_resolved = old_find_target(_unescape_shell(gate_line), tmp)
        new_hits = [h for h in scan_mutation_gates(tmp) if "p-deep.md" in h.source]
        report(
            "B4 dotted repo-relative target resolves; the old lstrip('./') resolver MISSED it",
            bool(new_hits) and old_resolved is None,
            f"new={len(new_hits)} hit(s), old_resolved={old_resolved}",
        )

    print()
    print("SELF-TEST:", "PASSED" if ok else "FAILED")
    return 0 if ok else 1


# --------------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=".", help="repository root")
    ap.add_argument("--self-test", action="store_true", help="prove the instrument can go red")
    ap.add_argument("--shape", choices=["1", "2", "both"], default="both")
    args = ap.parse_args()

    root = Path(args.root).resolve()
    if args.self_test:
        return self_test(root)

    backlog = root / ".planning" / "BACKLOG.md"
    if not backlog.is_file():
        print(f"ERROR: {backlog} not found", file=sys.stderr)
        return 2

    rc = 0

    if args.shape in ("1", "both"):
        dropped = scan_filing_claims(root, backlog)
        print(f"SHAPE 1 — findings claimed filed but absent from BACKLOG.md: {len(dropped)}")
        for d in dropped:
            print(f"  DROPPED  {d.ident:<24} {d.severity:<12} {d.source}")
        if dropped:
            rc = 1

    if args.shape in ("2", "both"):
        noops = scan_mutation_gates(root)
        print(f"SHAPE 2 — mutation gates whose sed matches nothing in its target: {len(noops)}")
        for n in noops:
            print(f"  NO-OP    {n.source}")
            print(f"           pattern: {n.pattern}")
            print(f"           target : {n.target}")
        if noops:
            rc = 1

    print()
    print("RESULT:", "CLEAN" if rc == 0 else "FINDINGS PRESENT")
    return rc


if __name__ == "__main__":
    sys.exit(main())
