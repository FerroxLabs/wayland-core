#!/usr/bin/env python3
"""Grade A-12: does the reader understand the code, and can they predict a break?

    python3 a12_grade.py --repo <pristine fixture repo>
                         --answer ANSWER.md --prediction PREDICTION.md
                         [--tripwire <marker file>] [--json out.json]

Two parts, graded separately and reported separately.

**Part 1 -- the execution flow.** Eight waypoints must be named, in order, in an
account of what happens to an order carrying an expired discount code; two
non-obvious facts must be stated; and the dormant legacy engine must not be put
on the live path.  The checker is deliberately not satisfiable by an inventory:
naming every function in the repository in the right order, with no account of
what happens, fails on the prose rule and on both facts.

**Part 2 -- what the change breaks.** Nothing here is taken from the key's
recorded ground truth.  The grader makes the change itself, in a scratch copy,
runs the suite, and compares the prediction with what actually broke.  The
ground truth in the key is then reported alongside as a cross-check: if the two
disagree, the key is stale and this grader says so rather than grading against
a stale number.

**Ordering is enforced structurally, not from the transcript.** The key's
original rule ("the transcript must show the prediction before the run") is
adjudicated from the agent's own account of itself, which is exactly what a
receipt is.  Instead: the harness hashes PREDICTION.md the moment the product's
session ends and passes that digest in, and the harness -- not the product --
performs the graded run afterwards.  A tripwire inside the package records
every execution of `orderpipe`, with the digest of `config.py` as it stood at
that moment, to a file outside the workspace.  If the package was executed with
the change already applied before the prediction was committed, the prediction
was informed by the run and part 2 FAILS.  Residual, stated plainly: an agent
that copies the tree outside its workspace and strips the tripwire from the
copy would not be seen.  That is recorded as a NOTE, not papered over.

Standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "lib"))

from mutation_guard import VacuousMutation, apply_mutation  # noqa: E402

KEY_PATH = os.path.join(HERE, "a12.key.json")
TEST_TIMEOUT_S = 300

CHANGE_OLD = "    def __init__(self, write_through=True, max_entries=512):"
CHANGE_NEW = "    def __init__(self, write_through=False, max_entries=512):"

TRIPWIRE_MARK = "# --- job-corpus A-12 execution tripwire (harness-owned) ---"

TRIPWIRE_SOURCE = '''

{mark}
# This block is the harness's, not the codebase's. It records the fact that
# this package was executed -- and the digest of config.py at that moment --
# to a file outside the working tree, so that "the prediction came before the
# run" is decided by the world rather than by the transcript.
def _job_corpus_tripwire():
    import hashlib as _h, json as _j, os as _o, sys as _s, time as _t
    marker = _o.environ.get("JOB_CORPUS_A12_TRIPWIRE") or {marker!r}
    try:
        cfg = _o.path.join(_o.path.dirname(_o.path.abspath(__file__)), "config.py")
        with open(cfg, "rb") as fh:
            digest = _h.sha256(fh.read()).hexdigest()
    except OSError:
        digest = None
    try:
        _o.makedirs(_o.path.dirname(marker), exist_ok=True)
        with open(marker, "a", encoding="utf-8") as fh:
            fh.write(_j.dumps({{
                "at": _t.time(),
                "argv": _s.argv[:8],
                "cwd": _o.getcwd(),
                "config_sha256": digest,
            }}) + "\\n")
    except OSError:
        pass


_job_corpus_tripwire()
'''


# ---------------------------------------------------------------------------
# part 1 -- the execution flow
# ---------------------------------------------------------------------------

#: (waypoint id, method regex, qualifiers that must appear near a bare mention)
WAYPOINTS = [
    ("pipeline.Pipeline.submit_order", r"submit_order", ()),
    ("pipeline.Pipeline.validate", r"validate", ("pipeline", "stage")),
    ("pricing.resolve_discount", r"resolve_discount", ()),
    ("cache.ResolutionCache.put", r"put", ("cache", "resolutioncache")),
    ("ledger.Ledger.commit", r"commit", ("ledger",)),
    ("cache.ResolutionCache.get_committed", r"get_committed", ()),
    ("pricing.discount_cents", r"discount_cents", ()),
    ("cache.ResolutionCache.flush", r"flush", ("cache", "resolutioncache", "pipeline")),
]

QUALIFIER_WINDOW = 90

_LIST_ITEM = re.compile(r"^\s*(?:[-*+•]|\d+[.)])\s")
_WORD = re.compile(r"[A-Za-z]{2,}")


def _first_qualified(text, method, qualifiers):
    """Index of the first mention of `method` that is really about this symbol."""
    lower = text.lower()
    for m in re.finditer(r"(?<![A-Za-z0-9_])" + method + r"(?![A-Za-z0-9_])", text):
        start = m.start()
        if not qualifiers:
            return start
        # A dotted or slashed form names the owner outright.
        prefix = text[max(0, start - 40) : start]
        if re.search(r"(?:[A-Za-z_][A-Za-z0-9_]*)\s*[.→>/]\s*$", prefix):
            owner = prefix.lower()
            if any(q in owner for q in qualifiers):
                return start
        window = lower[max(0, start - QUALIFIER_WINDOW) : start + QUALIFIER_WINDOW]
        if any(q in window for q in qualifiers):
            return start
    return None


def _line_of(text, index):
    start = text.rfind("\n", 0, index) + 1
    end = text.find("\n", index)
    return text[start : end if end >= 0 else len(text)]


def _prose_words(line):
    """Words that are not part of an identifier -- an inventory has almost none."""
    stripped = re.sub(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+", " ", line)
    stripped = re.sub(r"\b[a-z]+_[a-z_]+\b", " ", stripped)
    return len(_WORD.findall(stripped))


def _sentences(text):
    return [s for s in re.split(r"(?<=[.!?])\s+|\n{2,}", text) if s.strip()]


def grade_part1(answer, key):
    spec = key["part_1_execution_flow"]
    reasons, observed = [], {}

    positions = []
    for name, pattern, qualifiers in WAYPOINTS:
        at = _first_qualified(answer, pattern, qualifiers)
        positions.append((name, at))
    observed["waypoints"] = {n: a for n, a in positions}

    missing = [n for n, a in positions if a is None]
    if missing:
        reasons.append("the account never reaches: " + ", ".join(missing))

    found = [(n, a) for n, a in positions if a is not None]
    out_of_order = []
    for i in range(1, len(found)):
        if found[i][1] < found[i - 1][1]:
            out_of_order.append("%s comes after %s" % (found[i - 1][0], found[i][0]))
    if out_of_order:
        reasons.append("the order of the path is wrong: " + "; ".join(out_of_order[:4]))
    observed["order_as_written"] = [n for n, _a in sorted(found, key=lambda p: p[1])]

    # An inventory is not an account. Each waypoint's own line is inspected: a
    # bare list of names carries almost no prose, and an answer made entirely of
    # such lines has described nothing.
    prose_lines = 0
    inventory_lines = 0
    for name, at in found:
        line = _line_of(answer, at)
        words = _prose_words(line)
        if words >= 8:
            prose_lines += 1
        if _LIST_ITEM.match(line) and words < 8:
            inventory_lines += 1
    observed["waypoint_lines_with_prose"] = prose_lines
    observed["waypoint_lines_that_are_bare_list_items"] = inventory_lines
    if found and prose_lines < 5:
        reasons.append(
            "this is an inventory of names, not an account of what happens: only %d of "
            "the %d waypoints appears in a sentence that says anything about it"
            % (prose_lines, len(found))
        )

    low = answer.lower()
    silent = (
        "expired" in low
        and re.search(r"\b(?:zero|0)\b", low)
        and re.search(r"resolve_discount|pricing", low)
        and re.search(
            r"not an error|no error|does not (?:error|fail|raise|complain)|"
            r"doesn't (?:error|fail|raise|complain)|silently|without complaint|accepts",
            low,
        )
    )
    observed["fact_silent_downgrade"] = bool(silent)
    if not silent:
        reasons.append(
            "the answer never says that pricing accepts the expired code, sets the "
            "discount to zero and does not treat it as an error"
        )

    # Scoped to a single sentence on purpose. Looking for the four words
    # anywhere in the answer matched "rejects an empty order" in the validate
    # paragraph against "FeatureFlags" three paragraphs later, and passed an
    # answer that never mentions the ledger note at all.
    surfaces = "discount_rejected" in low
    if not surfaces:
        for sentence in _sentences(answer):
            s = sentence.lower()
            if not re.search(r"\bcommit\b|\bledger\b", s):
                continue
            if re.search(r"\bnote[ds]?\b|\bnoting\b|annotat", s) and re.search(r"reject", s):
                surfaces = True
                break
    observed["fact_surfaces_at_commit"] = bool(surfaces)
    if not surfaces:
        reasons.append(
            "the answer never says that the rejection surfaces later, as a note "
            "attached to the ledger entry by Ledger.commit"
        )

    live = []
    for sentence in _sentences(answer):
        s = sentence.lower()
        if "legacy" not in s:
            continue
        if re.search(
            r"\bnot\b|\bnever\b|dormant|disabled|switched off|turned off|"
            r"off by default|false|rollback|unless|only if|only when|no order",
            s,
        ):
            continue
        if re.search(
            r"\bis called\b|\bcalls\b|\bruns\b|\bresolves\b|\bapplies\b|\bhandles\b|"
            r"\bthen\b|\bnext\b|\bused\b",
            s,
        ):
            live.append(sentence.strip()[:200])
    observed["legacy_on_live_path"] = live
    if live:
        reasons.append(
            "the dormant legacy engine is put on the path an order actually takes: "
            + live[0]
        )

    return {
        "verdict": "FAIL" if reasons else "PASS",
        "reasons": reasons,
        "observed": observed,
        "pass_condition": spec["pass_condition"],
    }


# ---------------------------------------------------------------------------
# part 2 -- running the change for real
# ---------------------------------------------------------------------------

_RESULT_LINE = re.compile(
    r"^(test_[A-Za-z0-9_]+)\s+\(([^)]*)\)\s*(?:\.\.\.)?\s*(ok|ERROR|FAIL|skipped.*)?\s*$"
)
_FAILURE_HEAD = re.compile(r"^(?:FAIL|ERROR):\s+(test_[A-Za-z0-9_]+)\s+\(([^)]*)\)")


def _class_of(dotted, method):
    parts = [p for p in dotted.split(".") if p]
    if parts and parts[-1] == method:
        parts = parts[:-1]
    return parts[-1] if parts else ""


def run_suite(directory):
    env = dict(os.environ)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    # `tests/` is not a package, so the repository itself has to be on the path
    # for `import orderpipe` to resolve.
    env["PYTHONPATH"] = directory
    try:
        proc = subprocess.run(
            [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test*.py", "-v"],
            cwd=directory,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=TEST_TIMEOUT_S,
        )
    except subprocess.TimeoutExpired:
        return {"ran": 0, "all": set(), "failing": set(), "timed_out": True, "output": ""}
    out = proc.stdout.decode("utf-8", "replace")
    all_tests, failing = set(), set()
    for line in out.splitlines():
        m = _RESULT_LINE.match(line.strip())
        if m:
            method, dotted, status = m.group(1), m.group(2), (m.group(3) or "")
            ident = "%s.%s" % (_class_of(dotted, method), method)
            all_tests.add(ident)
            if status in ("ERROR", "FAIL"):
                failing.add(ident)
            continue
        m = _FAILURE_HEAD.match(line.strip())
        if m:
            method, dotted = m.group(1), m.group(2)
            ident = "%s.%s" % (_class_of(dotted, method), method)
            all_tests.add(ident)
            failing.add(ident)
    ran = re.search(r"^Ran (\d+) tests?", out, re.MULTILINE)
    return {
        "ran": int(ran.group(1)) if ran else 0,
        "all": all_tests,
        "failing": failing,
        "timed_out": False,
        "output": out,
    }


def _staged(repo, mutate=False):
    tmp = tempfile.mkdtemp(prefix="a12-")
    dest = os.path.join(tmp, "repo")
    shutil.copytree(repo, dest)
    if mutate:
        target = os.path.join(dest, "orderpipe", "config.py")
        with open(target, "r", encoding="utf-8") as fh:
            source = fh.read()
        mutated = apply_mutation(source, CHANGE_OLD, CHANGE_NEW, filename=target)
        with open(target, "w", encoding="utf-8") as fh:
            fh.write(mutated)
        # A restored or rewritten file with an older timestamp lets the runtime
        # reuse a stale artifact. Stamp it now.
        os.utime(target, None)
    return tmp, dest


def predicted_tests(prediction, all_tests):
    """What the prediction claims will fail, as concrete test identifiers."""
    by_method = {}
    by_class = {}
    for ident in all_tests:
        cls, method = ident.split(".", 1)
        by_method.setdefault(method, set()).add(ident)
        by_class.setdefault(cls, set()).add(ident)

    claimed = set()
    notes = []
    for token in set(re.findall(r"\btest_[A-Za-z0-9_]+\b", prediction)):
        if token in by_method:
            claimed |= by_method[token]
    return claimed, by_class, notes


def apply_class_claims(prediction, claimed, by_class, actual_failing, notes):
    """A class name counts only if it does not sweep in a surviving test."""
    for cls, members in by_class.items():
        if not re.search(r"\b" + re.escape(cls) + r"\b", prediction):
            continue
        survivors = members - actual_failing
        failing_here = members & actual_failing
        if survivors:
            notes.append(
                "%s is named as a class, but %d of its tests do not fail, so naming the "
                "class claims them too and it is not counted: %s"
                % (cls, len(survivors), ", ".join(sorted(survivors)))
            )
            continue
        claimed |= failing_here
    return claimed


def grade_part2(repo, prediction, key, tripwire_events, prediction_sha256):
    spec = key["part_2_change_impact"]
    scoring = spec["scoring"]
    reasons, notes, observed = [], [], {}

    tmp, base_dir = _staged(repo)
    try:
        baseline = run_suite(base_dir)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    observed["baseline"] = {"ran": baseline["ran"], "failing": sorted(baseline["failing"])}
    if baseline["failing"] or baseline["ran"] == 0:
        return {
            "verdict": "UNPROVEN",
            "reasons": [
                "the fixture suite is not green before the change (%d of %d failing), so "
                "nothing can be attributed to the change"
                % (len(baseline["failing"]), baseline["ran"])
            ],
            "observed": observed,
        }

    try:
        tmp, mut_dir = _staged(repo, mutate=True)
    except VacuousMutation as exc:
        return {
            "verdict": "UNPROVEN",
            "reasons": ["the proposed change could not be proven to alter executable code: %s" % exc],
            "observed": observed,
        }
    try:
        after = run_suite(mut_dir)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    observed["after_the_change"] = {"ran": after["ran"], "failing": sorted(after["failing"])}

    tmp, restored_dir = _staged(repo)
    try:
        restored = run_suite(restored_dir)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    observed["restored"] = {"ran": restored["ran"], "failing": sorted(restored["failing"])}
    if restored["failing"]:
        return {
            "verdict": "UNPROVEN",
            "reasons": ["the suite is not green again with the change reverted, so the "
                        "observed failures cannot be attributed to the change alone"],
            "observed": observed,
        }

    actual = after["failing"]
    if not actual:
        return {
            "verdict": "UNPROVEN",
            "reasons": ["the change broke nothing on this host, so there is no prediction "
                        "to score against"],
            "observed": observed,
        }

    # The key records a ground truth. It is a cross-check, never the grader.
    recorded = {t.split(".", 1)[-1] if t.startswith("test_pipeline.") else t
                for t in spec["ground_truth"]["failing_tests"]}
    recorded = {t.replace("test_pipeline.", "") for t in spec["ground_truth"]["failing_tests"]}
    if recorded != actual:
        notes.append(
            "the key's recorded ground truth (%d tests) differs from what actually "
            "broke here (%d): only what actually broke is graded. Key-only: %s; "
            "observed-only: %s"
            % (len(recorded), len(actual),
               ", ".join(sorted(recorded - actual)) or "-",
               ", ".join(sorted(actual - recorded)) or "-")
        )

    claimed, by_class, class_notes = predicted_tests(prediction, after["all"])
    notes.extend(class_notes)
    claimed = apply_class_claims(prediction, claimed, by_class, actual, notes)

    true_positives = sorted(claimed & actual)
    false_positives = sorted(claimed - actual)
    observed["predicted"] = sorted(claimed)
    observed["true_positives"] = true_positives
    observed["false_positives"] = false_positives
    observed["actually_failing"] = sorted(actual)

    need_tp = int(scoring["true_positives_required"])
    allow_fp = int(scoring["false_positives_allowed"])
    if len(true_positives) < need_tp:
        reasons.append(
            "the prediction names %d of the %d tests that really break; %d were required. "
            "Missed: %s" % (len(true_positives), len(actual), need_tp,
                            ", ".join(sorted(actual - claimed)) or "-")
        )
    if len(false_positives) > allow_fp:
        reasons.append(
            "the prediction claims %d test(s) that do not break (at most %d allowed): %s"
            % (len(false_positives), allow_fp, ", ".join(false_positives))
        )

    low = prediction.lower()
    mechanism = ("pending" in low) and ("get_committed" in low or "durable" in low)
    observed["mechanism_stated"] = bool(mechanism)
    if not mechanism:
        reasons.append(
            "the mechanism given is not the real one: with write_through off the put "
            "lands in the pending buffer and Ledger.commit reads with get_committed, "
            "which never sees pending"
        )
    for wrong in ("eviction", "max_entries", "legacy"):
        if wrong in low and not mechanism:
            reasons.append("the prediction blames %s, which is not what breaks" % wrong)

    ship_no = re.search(
        r"\b(?:do not ship|don't ship|would not ship|wouldn't ship|not ship|no,? "
        r"(?:i )?would not|not without|should not ship|block(?:ing)? (?:the|this) change)\b",
        low,
    )
    observed["ship_recommendation_is_no"] = bool(ship_no)
    if not ship_no:
        reasons.append(
            "the change is not recommended against; it should be 'no', or 'not without "
            "changing commit to tolerate a pending resolution'"
        )

    # Ordering, decided by the world.
    observed["prediction_sha256"] = prediction_sha256
    pristine_config = _sha256_text(_read(os.path.join(repo, "orderpipe", "config.py")))
    observed["pristine_config_sha256"] = pristine_config
    informed = [
        e for e in tripwire_events
        if e.get("config_sha256") and e.get("config_sha256") != pristine_config
    ]
    observed["package_executions_before_prediction"] = len(tripwire_events)
    observed["executions_with_the_change_applied"] = len(informed)
    if informed:
        reasons.append(
            "the package was executed %d time(s) with the change already applied before "
            "the prediction was committed, so the prediction is a report of a run, not a "
            "prediction" % len(informed)
        )
    elif tripwire_events:
        notes.append(
            "the package was executed %d time(s) before the prediction was committed, but "
            "always with config.py as shipped, so nothing in those runs could reveal what "
            "the change breaks" % len(tripwire_events)
        )
    notes.append(
        "residual: the tripwire travels with the package, so a copy taken outside the "
        "workspace still records itself -- but a copy with the tripwire deleted would "
        "not be seen. This is stated, not claimed away."
    )

    return {
        "verdict": "FAIL" if reasons else "PASS",
        "reasons": reasons,
        "notes": notes,
        "observed": observed,
        "pass_condition": spec["pass_condition"],
    }


def _read(path):
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


def _sha256_text(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def install_tripwire(package_init, marker_path):
    """Append the harness's execution recorder to orderpipe/__init__.py."""
    with open(package_init, "r", encoding="utf-8") as fh:
        source = fh.read()
    if TRIPWIRE_MARK in source:
        return False
    with open(package_init, "w", encoding="utf-8") as fh:
        fh.write(source + TRIPWIRE_SOURCE.format(mark=TRIPWIRE_MARK, marker=marker_path))
    os.utime(package_init, None)
    return True


def strip_tripwire(text):
    at = text.find(TRIPWIRE_MARK)
    return text if at < 0 else text[:at].rstrip() + "\n"


def read_tripwire(path):
    events = []
    if not path or not os.path.exists(path):
        return events
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except ValueError:
                continue
    return events


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True, help="a pristine copy of the fixture repo")
    ap.add_argument("--answer", required=True)
    ap.add_argument("--prediction", required=True)
    ap.add_argument("--tripwire", default=None)
    ap.add_argument("--prediction-sha256", default=None)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    with open(KEY_PATH, "r", encoding="utf-8") as fh:
        key = json.load(fh)

    report = {"row": "A-12", "parts": {}}
    if os.path.isfile(args.answer):
        answer = _read(args.answer)
    else:
        answer = ""
    if os.path.isfile(args.prediction):
        prediction = _read(args.prediction)
    else:
        prediction = ""

    if not answer.strip():
        report["parts"]["part_1"] = {
            "verdict": "FAIL",
            "reasons": ["no written account of the execution flow was produced"],
            "observed": {},
        }
    else:
        report["parts"]["part_1"] = grade_part1(answer, key)

    if not prediction.strip():
        report["parts"]["part_2"] = {
            "verdict": "FAIL",
            "reasons": ["no prediction was committed before the harness ran the change"],
            "observed": {},
        }
    else:
        report["parts"]["part_2"] = grade_part2(
            os.path.abspath(args.repo),
            prediction,
            key,
            read_tripwire(args.tripwire),
            args.prediction_sha256,
        )

    verdicts = [p["verdict"] for p in report["parts"].values()]
    report["verdict"] = (
        "FAIL" if "FAIL" in verdicts else ("UNPROVEN" if "UNPROVEN" in verdicts else "PASS")
    )
    report["reasons"] = [
        "part %s: %s" % (name.split("_")[-1], reason)
        for name, part in report["parts"].items()
        for reason in part.get("reasons", [])
    ]
    text = json.dumps(report, indent=2, default=str)
    print(text)
    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
