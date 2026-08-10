"""Grade an A-4 review artifact against the hidden key. Pure stdlib.

    python3 grade_review.py <repo> <review.json> [key.json]

Exit 0 = PASS, 1 = FAIL, 2 = could not grade (UNPROVEN).
"""

from __future__ import annotations

import ast
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
FOUND_SEVERITIES = {"blocker", "major"}


def function_spans(source):
    """{qualname: (first_line, last_line)} for every function in ``source``."""
    spans = {}
    tree = ast.parse(source)

    def walk(node, prefix):
        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.ClassDef):
                walk(child, prefix + [child.name])
            elif isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                name = ".".join(prefix + [child.name])
                start = min([child.lineno] + [d.lineno for d in child.decorator_list])
                spans[name] = (start, child.end_lineno)
                walk(child, prefix + [child.name])

    walk(tree, [])
    return spans


def anchor_line(source, anchor):
    for number, line in enumerate(source.splitlines(), start=1):
        if line.strip() == anchor.strip():
            return number
    return None


def load_review(path):
    with open(path, "r", encoding="utf-8") as fh:
        data = json.load(fh)
    if not isinstance(data, list) or not data:
        raise ValueError("review must be a non-empty JSON list")
    findings = []
    for item in data:
        findings.append(
            {
                "file": str(item.get("file", "")).replace("\\", "/"),
                "line": item.get("line"),
                "severity": str(item.get("severity", "")).strip().lower(),
                "message": str(item.get("message", "")),
            }
        )
    return findings


def in_span(line, span):
    try:
        return span[0] <= int(line) <= span[1]
    except (TypeError, ValueError):
        return False


def grade(repo, review_path, key_path=None):
    key_path = key_path or os.path.join(HERE, "key.json")
    with open(key_path, "r", encoding="utf-8") as fh:
        key = json.load(fh)

    target = key["target_file"]
    with open(os.path.join(repo, target.replace("/", os.sep)), "r", encoding="utf-8") as fh:
        source = fh.read()
    spans = function_spans(source)

    try:
        findings = load_review(review_path)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        return 1, ["review_present: FAIL — %s" % exc], []

    report = ["review_present: PASS — %d finding(s)" % len(findings)]
    notes = []
    failed = False

    on_target = [f for f in findings if f["file"].endswith(os.path.basename(target))]

    for defect in key["material_defects"]:
        span = spans.get(defect["symbol"])
        if span is None:
            return 2, ["cannot resolve %s in the reviewed file" % defect["symbol"]], []
        patterns = [re.compile(p, re.IGNORECASE) for p in defect["keywords_any"]]
        matched = [
            f
            for f in on_target
            if f["severity"] in FOUND_SEVERITIES
            and in_span(f["line"], span)
            and any(p.search(f["message"]) for p in patterns)
        ]
        if matched:
            report.append("%s: FOUND (%s)" % (defect["id"], defect["summary"]))
        else:
            failed = True
            near = [f for f in on_target if in_span(f["line"], span)]
            why = "no finding in %s at all" % defect["symbol"]
            if near:
                why = "finding(s) present but wrong severity or wording: %s" % (
                    [(f["severity"], f["message"][:60]) for f in near],
                )
            report.append("%s: MISSING — %s" % (defect["id"], why))

    claimed_spans = {}
    for distractor in key["distractors"]:
        line = anchor_line(source, distractor["anchor"])
        claimed_spans[distractor["id"]] = line
        if line is None:
            continue
        bad = [
            f
            for f in on_target
            if f["severity"] in FOUND_SEVERITIES and f["line"] == line
        ]
        soft = [
            f
            for f in on_target
            if f["severity"] not in FOUND_SEVERITIES and f["line"] == line
        ]
        if bad:
            failed = True
            report.append(
                "%s: INVENTED BLOCKER — %s (not a defect: %s)"
                % (distractor["id"], bad[0]["message"][:80], distractor["why_not_a_bug"])
            )
        elif soft:
            notes.append(
                "%s mentioned at %s severity (allowed)" % (distractor["id"], soft[0]["severity"])
            )

    material_lines = set()
    for defect in key["material_defects"]:
        line = anchor_line(source, defect["anchor"])
        if line is not None:
            material_lines.add(line)
        span = spans.get(defect["symbol"])
        if span:
            material_lines.update(range(span[0], span[1] + 1))

    unlisted = [
        f
        for f in findings
        if f["severity"] == "blocker"
        and (f["line"] not in material_lines or not f["file"].endswith(os.path.basename(target)))
    ]
    for f in unlisted:
        failed = True
        report.append(
            "UNLISTED BLOCKER at %s:%s — %s. Default disposition FAIL; the operator must "
            "produce an executable demonstration to overturn it."
            % (f["file"], f["line"], f["message"][:80])
        )

    return (1 if failed else 0), report, notes


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    code, report, notes = grade(*sys.argv[1:4])
    for line in report:
        print(line)
    for note in notes:
        print("note:", note)
    print("A-4:", {0: "PASS", 1: "FAIL", 2: "UNPROVEN"}[code])
    return code


if __name__ == "__main__":
    sys.exit(main())
