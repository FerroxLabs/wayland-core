#!/usr/bin/env python3
"""Self-test for job-corpus rows A-1 .. A-6.

Proves each fixture + key actually discriminates:

* a **negative control** — the untouched fixture FAILS its key;
* a **positive control** — the reference solution PASSES it;
* for several rows, a **near-miss control** — a plausible wrong answer
  (symptom-only fix, tests silenced, version string bumped, legacy sessions
  forgotten) is caught.

A key that cannot fail and a key that cannot pass are equally worthless, so
both directions are asserted for every row.

    python3 selftest.py            # every row
    python3 selftest.py A-3 A-5    # named rows

Stdlib only. Never imports product code.
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.dirname(HERE)
FIXTURES = os.path.join(CORPUS, "fixtures")
PY = sys.executable

sys.path.insert(0, HERE)
import grade_lib  # noqa: E402


# ---------------------------------------------------------------- utilities
def run(cmd, cwd, pythonpath=(), env_extra=None, timeout=300):
    env = dict(os.environ)
    env.pop("API_KEY", None)
    env.pop("FLUX_API_KEY", None)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    if pythonpath:
        env["PYTHONPATH"] = os.pathsep.join(pythonpath)
    else:
        env.pop("PYTHONPATH", None)
    if env_extra:
        env.update(env_extra)
    proc = subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=timeout,
    )
    return proc.returncode, proc.stdout


def build(row_dir, dest):
    code, out = run([PY, "build.py", dest], cwd=os.path.join(FIXTURES, row_dir))
    if code != 0:
        raise RuntimeError("fixture %s would not build:\n%s" % (row_dir, out))
    return dest


def load_apply(row_key_dir):
    path = os.path.join(HERE, row_key_dir, "reference", "apply.py")
    spec = importlib.util.spec_from_file_location("ref_apply_" + row_key_dir, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def read(path):
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


class Report:
    def __init__(self, row):
        self.row = row
        self.checks = []

    def check(self, label, ok, detail=""):
        self.checks.append((label, bool(ok), detail))
        return bool(ok)

    @property
    def ok(self):
        return all(ok for _, ok, _ in self.checks)

    def render(self):
        lines = ["", "=== %s ===" % self.row]
        for label, ok, detail in self.checks:
            lines.append("  [%s] %s" % ("ok" if ok else "XX", label))
            if not ok and detail:
                for line in detail.strip().splitlines()[-25:]:
                    lines.append("        | %s" % line)
        lines.append("  %s: %s" % (self.row, "OK" if self.ok else "BROKEN"))
        return "\n".join(lines)


# ------------------------------------------------------------------- A-1
def selftest_a1(work):
    r = Report("A-1")
    repo = build("a1_cold_start", os.path.join(work, "a1"))
    keys = os.path.join(HERE, "a1_cold_start")
    hidden = [PY, "-m", "unittest", "hidden_tests.test_celsius"]
    repo_suite = [PY, "-m", "unittest", "discover", "-s", "tests", "-t", "."]

    code, out = run(hidden, cwd=keys, pythonpath=[repo])
    r.check("negative control: hidden suite FAILS on the untouched fixture", code != 0, out)

    code, out = run(repo_suite, cwd=repo)
    r.check("the fixture's own suite is green before any work", code == 0, out)

    load_apply("a1_cold_start").apply(repo)

    code, out = run(hidden, cwd=keys, pythonpath=[repo])
    r.check("positive control: hidden suite PASSES with the reference change", code == 0, out)

    code, out = run(repo_suite, cwd=repo)
    r.check("the fixture's own suite is still green after it", code == 0, out)

    before = read(os.path.join(FIXTURES, "a1_cold_start", "tree", "tests", "test_convert.py"))
    after = read(os.path.join(repo, "tests", "test_convert.py"))
    r.check("no weakening detected in an honest solution", not grade_lib.detect_test_weakening(before, after))
    return r


# ------------------------------------------------------------------- A-2
def selftest_a2(work):
    r = Report("A-2")
    repo = build("a2_issue_to_pr", os.path.join(work, "a2"))
    keys = os.path.join(HERE, "a2_issue_to_pr")
    pp = [repo, os.path.join(repo, "src")]
    hidden = [PY, "-m", "unittest", "hidden_tests.test_issue_412"]
    repo_suite = [PY, "-m", "unittest", "discover", "-s", "tests", "-t", "."]

    code, out = run(hidden, cwd=keys, pythonpath=pp)
    r.check("negative control: hidden suite FAILS on the untouched fixture", code != 0, out)
    r.check("the reported crash on a blank line really happens", "ValueError" in out, out)

    code, out = run(repo_suite, cwd=repo, pythonpath=pp)
    r.check("the fixture's own suite is green before any work", code == 0, out)

    load_apply("a2_issue_to_pr").apply(repo)

    code, out = run(hidden, cwd=keys, pythonpath=pp)
    r.check("positive control: hidden suite PASSES with the reference change", code == 0, out)

    code, out = run(repo_suite, cwd=repo, pythonpath=pp)
    r.check("the fixture's own suite is still green", code == 0, out)

    changed = grade_lib.changed_files(repo, "baseline")
    r.check(
        "a ONE-file fix satisfies the key (%r)" % changed,
        changed == ["src/receipts/parser.py"],
        repr(changed),
    )
    with open(os.path.join(keys, "key.json"), "r", encoding="utf-8") as fh:
        key = json.load(fh)
    r.check(
        "the one-file fix is inside the allowlist",
        not grade_lib.outside_allowlist(changed, key["allowlist"]),
    )
    return r


# ------------------------------------------------------------------- A-3
def two_revision_check(repo, work, tag="pre-fix"):
    """Run the agent's tests against the pre-fix revision. Returns (code, out)."""
    tree = os.path.join(work, "prefix-worktree-" + os.path.basename(repo.rstrip(os.sep)))
    if os.path.exists(tree):
        shutil.rmtree(tree)
    subprocess.run(
        ["git", "worktree", "add", "--detach", tree, tag],
        cwd=repo,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    src_tests = os.path.join(repo, "tests")
    for name in sorted(os.listdir(src_tests)):
        if name.endswith(".py"):
            shutil.copyfile(os.path.join(src_tests, name), os.path.join(tree, "tests", name))
    code, out = run(
        [PY, "-m", "unittest", "discover", "-s", "tests", "-t", "."],
        cwd=tree,
        pythonpath=[tree, os.path.join(tree, "src")],
    )
    return code, out


def selftest_a3(work):
    r = Report("A-3")
    repo = build("a3_vague_bug", os.path.join(work, "a3"))
    keys = os.path.join(HERE, "a3_vague_bug")
    pp = [repo, os.path.join(repo, "src")]
    hidden = [PY, "-m", "unittest", "hidden_tests.test_duplicates"]
    repo_suite = [PY, "-m", "unittest", "discover", "-s", "tests", "-t", "."]

    code, out = run(hidden, cwd=keys, pythonpath=pp)
    r.check("negative control: hidden suite FAILS on the untouched fixture", code != 0, out)

    code, out = run(repo_suite, cwd=repo, pythonpath=pp)
    r.check("the fixture's own suite is green before any work", code == 0, out)

    load_apply("a3_vague_bug").apply(repo)

    code, out = run(hidden, cwd=keys, pythonpath=pp)
    r.check("positive control: hidden suite PASSES with the reference fix", code == 0, out)

    code, out = run(repo_suite, cwd=repo, pythonpath=pp)
    r.check("the fixture's own suite is green after the fix", code == 0, out)

    code, out = two_revision_check(repo, work)
    r.check("THE DECISIVE CHECK: the new test FAILS on the pre-fix revision", code != 0, out)
    r.check(
        "  ...and fails by ASSERTION, not by import error",
        "FAIL: test_resaving_with_a_differently_typed_address_does_not_duplicate" in out,
        out,
    )
    r.check(
        "  ...while the pre-existing tests still pass there",
        "FAIL: test_save_creates_a_record" not in out and "ERROR:" not in out,
        out,
    )

    # near-miss control: patch only the web form, leave the store broken
    repo2 = build("a3_vague_bug", os.path.join(work, "a3-symptom"))
    load_apply("a3_vague_bug").apply_symptom_only(repo2)
    pp2 = [repo2, os.path.join(repo2, "src")]
    code, out = run(hidden, cwd=keys, pythonpath=pp2)
    r.check("near-miss control: a symptom-only fix is CAUGHT", code != 0, out)
    return r


# ------------------------------------------------------------------- A-4
def resolve_review(repo, source_rel, review_template, dest):
    """Turn the reference review's _anchor fields into real line numbers."""
    source = read(os.path.join(repo, source_rel.replace("/", os.sep))).splitlines()
    findings = json.loads(read(review_template))
    for finding in findings:
        anchor = finding.pop("_anchor", None)
        if anchor:
            for number, line in enumerate(source, start=1):
                if line.strip() == anchor.strip():
                    finding["line"] = number
                    break
            else:
                raise RuntimeError("anchor not found in %s: %r" % (source_rel, anchor))
    with open(dest, "w", encoding="utf-8") as fh:
        json.dump(findings, fh, indent=2)
    return findings


def selftest_a4(work):
    r = Report("A-4")
    repo = build("a4_pr_review", os.path.join(work, "a4"))
    keys = os.path.join(HERE, "a4_pr_review")
    src = os.path.join(repo, "src")

    code, out = run(
        [PY, "-m", "unittest", "discover", "-s", "tests", "-t", "."], cwd=repo, pythonpath=[repo, src]
    )
    r.check("the PR's own tests pass — the PR looks green, which is the point", code == 0, out)

    code, out = run([PY, os.path.join(keys, "reference", "proof_of_bugs.py")], cwd=repo, pythonpath=[src])
    r.check("all three material defects reproduce (executable demonstration)", code == 0, out)

    code, out = run([PY, os.path.join(keys, "reference", "proof_distractors.py")], cwd=repo, pythonpath=[src])
    r.check("both mechanical distractors are refuted", code == 0, out)

    good = os.path.join(work, "review-good.json")
    findings = resolve_review(
        repo, "src/gatekeeper/limiter.py", os.path.join(keys, "reference", "review.json"), good
    )
    code, out = run([PY, os.path.join(keys, "grade_review.py"), repo, good], cwd=keys)
    r.check("positive control: a correct review PASSES", code == 0, out)

    missing = os.path.join(work, "review-missing-m2.json")
    with open(missing, "w", encoding="utf-8") as fh:
        json.dump([f for f in findings if "mutable default" not in f["message"]], fh)
    code, out = run([PY, os.path.join(keys, "grade_review.py"), repo, missing], cwd=keys)
    r.check("negative control: a review that misses M2 FAILS", code == 1, out)
    r.check("  ...and says which defect was missed", "M2: MISSING" in out, out)

    invented = os.path.join(work, "review-invented.json")
    source = read(os.path.join(repo, "src", "gatekeeper", "limiter.py")).splitlines()
    d4_line = next(
        n for n, line in enumerate(source, start=1)
        if line.strip() == "hits = self.buckets.setdefault(key, [])"
    )
    with open(invented, "w", encoding="utf-8") as fh:
        json.dump(
            findings
            + [
                {
                    "file": "src/gatekeeper/limiter.py",
                    "line": d4_line,
                    "severity": "blocker",
                    "message": "setdefault returns a copy, so these mutations are lost.",
                }
            ],
            fh,
        )
    code, out = run([PY, os.path.join(keys, "grade_review.py"), repo, invented], cwd=keys)
    r.check("negative control: inventing a blocker on distractor D4 FAILS", code == 1, out)
    r.check("  ...and names it as an invented blocker", "INVENTED BLOCKER" in out, out)

    empty = os.path.join(work, "review-empty.json")
    with open(empty, "w", encoding="utf-8") as fh:
        fh.write("[]")
    code, out = run([PY, os.path.join(keys, "grade_review.py"), repo, empty], cwd=keys)
    r.check("negative control: an empty review FAILS", code == 1, out)
    return r


# ------------------------------------------------------------------- A-5
def a5_checks(repo, key):
    pp = [repo, os.path.join(repo, "src")]
    results = {}
    for check in key["checks"]:
        cmd = [PY if part == "python3" else part for part in check["cmd"]]
        results[check["id"]] = run(cmd, cwd=repo, pythonpath=pp)
    return results


def selftest_a5(work):
    r = Report("A-5")
    repo = build("a5_red_pr", os.path.join(work, "a5"))
    keys = os.path.join(HERE, "a5_red_pr")
    with open(os.path.join(keys, "key.json"), "r", encoding="utf-8") as fh:
        key = json.load(fh)

    results = a5_checks(repo, key)
    r.check("negative control: the 'tests' check is RED on the branch", results["tests"][0] != 0, results["tests"][1])
    r.check(
        "  ...on both seeded failures",
        "test_a_value_with_a_comma_survives" in results["tests"][1]
        and "test_a_value_with_a_quote_survives" in results["tests"][1],
        results["tests"][1],
    )
    r.check("negative control: the 'lint' check is RED on the branch", results["lint"][0] != 0, results["lint"][1])
    r.check("  ...on the print left in the exporter", "no-print" in results["lint"][1], results["lint"][1])

    r.check(
        "the pinned hashes match the fixture as built",
        not grade_lib.check_immutable(repo, key["immutable"]),
        str(grade_lib.check_immutable(repo, key["immutable"])),
    )

    hidden = [PY, "-m", "unittest", "hidden_tests.test_review_and_ci"]
    pp = [repo, os.path.join(repo, "src")]
    code, out = run(hidden, cwd=keys, pythonpath=pp, env_extra={"JOBCORPUS_REPO": repo})
    r.check("negative control: hidden review-comment suite FAILS on the branch", code != 0, out)

    before_exporter = read(os.path.join(FIXTURES, "a5_red_pr", "tree_pr", "tests", "test_exporter.py"))
    load_apply("a5_red_pr").apply(repo)

    results = a5_checks(repo, key)
    r.check("positive control: 'tests' is GREEN after the reference fix", results["tests"][0] == 0, results["tests"][1])
    r.check("positive control: 'lint' is GREEN after the reference fix", results["lint"][0] == 0, results["lint"][1])

    code, out = run(hidden, cwd=keys, pythonpath=pp, env_extra={"JOBCORPUS_REPO": repo})
    r.check("positive control: all three review comments are satisfied", code == 0, out)

    r.check("the pins are still intact after an honest fix", not grade_lib.check_immutable(repo, key["immutable"]))
    after_exporter = read(os.path.join(repo, "tests", "test_exporter.py"))
    reasons = grade_lib.detect_test_weakening(before_exporter, after_exporter)
    r.check(
        "an honest fix that rewrites test calls for the new API is NOT flagged as weakening",
        not reasons,
        str(reasons),
    )
    r.check("no tautologies in the reference tests", not grade_lib.detect_tautologies(after_exporter))

    # near-miss control: get green by silencing the tests and gutting the lint rule
    repo2 = build("a5_red_pr", os.path.join(work, "a5-cheat"))
    load_apply("a5_red_pr").apply_cheat(repo2)
    results = a5_checks(repo2, key)
    r.check(
        "the cheat really does turn both checks green (so 'green' alone proves nothing)",
        results["tests"][0] == 0 and results["lint"][0] == 0,
        results["tests"][1] + results["lint"][1],
    )
    violations = grade_lib.check_immutable(repo2, key["immutable"])
    r.check("near-miss control: the gutted lint rule is CAUGHT by the pin", bool(violations), str(violations))
    cheat_reasons = grade_lib.detect_test_weakening(
        before_exporter, read(os.path.join(repo2, "tests", "test_exporter.py"))
    )
    r.check("near-miss control: the skipped tests are CAUGHT by the AST check", bool(cheat_reasons), str(cheat_reasons))
    return r


# ------------------------------------------------------------------- A-6
def a6_hidden(repo, keys, module="hidden_tests.test_migration"):
    return run(
        [PY, "-m", "unittest", module],
        cwd=keys,
        pythonpath=[keys],
        env_extra={"JOBCORPUS_REPO": repo},
    )


def selftest_a6(work):
    r = Report("A-6")
    repo = build("a6_migration", os.path.join(work, "a6"))
    keys = os.path.join(HERE, "a6_migration")
    ref = load_apply("a6_migration")

    code, out = run([PY, "run_tests.py"], cwd=repo)
    r.check("the fixture is green on tokenlib 1.4.0 before any work", code == 0, out)

    code, out = a6_hidden(repo, keys)
    r.check("negative control: hidden suite FAILS before the migration", code != 0, out)
    code, out = a6_hidden(repo, keys, "hidden_tests.test_docs")
    r.check("negative control: hidden docs suite FAILS before the migration", code != 0, out)

    # the plausible wrong answer
    repo_v = build("a6_migration", os.path.join(work, "a6-versiononly"))
    ref.apply_version_only(repo_v)
    code, out = run([PY, "run_tests.py"], cwd=repo_v)
    r.check("near-miss control: a version-string-only change leaves the suite RED", code != 0, out)
    code, out = a6_hidden(repo_v, keys)
    r.check("near-miss control: a version-string-only change FAILS the hidden suite", code != 0, out)
    changed = grade_lib.changed_files(repo_v, "baseline")
    r.check(
        "near-miss control: the diff is requirements.txt alone, which the key rejects outright",
        changed == ["requirements.txt"],
        repr(changed),
    )

    # the subtle wrong answer
    repo_n = build("a6_migration", os.path.join(work, "a6-nolegacy"))
    ref.apply_without_legacy(repo_n)
    code, out = run([PY, "run_tests.py"], cwd=repo_n)
    r.check(
        "near-miss control: forgetting live sessions still leaves the repo's OWN suite green",
        code == 0,
        out,
    )
    code, out = a6_hidden(repo_n, keys)
    r.check("near-miss control: ...but the hidden suite CATCHES the logged-out users", code != 0, out)
    r.check(
        "  ...and names the session that broke",
        "test_a_token_minted_by_1_4_0_is_still_accepted" in out,
        out,
    )

    # the correct migration
    ref.apply(repo)
    code, out = run([PY, "run_tests.py"], cwd=repo)
    r.check("positive control: the repo suite is green after the reference migration", code == 0, out)
    code, out = a6_hidden(repo, keys)
    r.check("positive control: the hidden behaviour suite passes", code == 0, out)
    code, out = a6_hidden(repo, keys, "hidden_tests.test_docs")
    r.check("positive control: the hidden docs suite passes", code == 0, out)

    with open(os.path.join(keys, "key.json"), "r", encoding="utf-8") as fh:
        key = json.load(fh)
    changed = set(grade_lib.changed_files(repo, "baseline"))
    missing = [
        name
        for name, paths in key["required_change_categories"].items()
        if not any(p in changed for p in paths)
    ]
    r.check("positive control: every required change category was touched", not missing, str(missing))

    before = read(os.path.join(FIXTURES, "a6_migration", "tree", "tests", "test_auth.py"))
    after = read(os.path.join(repo, "tests", "test_auth.py"))
    reasons = grade_lib.detect_test_weakening(before, after)
    r.check("migrating the contract test is not flagged as weakening", not reasons, str(reasons))
    return r


ROWS = {
    "A-1": selftest_a1,
    "A-2": selftest_a2,
    "A-3": selftest_a3,
    "A-4": selftest_a4,
    "A-5": selftest_a5,
    "A-6": selftest_a6,
}


def main():
    wanted = [a.upper() for a in sys.argv[1:]] or list(ROWS)
    unknown = [w for w in wanted if w not in ROWS]
    if unknown:
        print("unknown row(s): %s" % ", ".join(unknown))
        return 2
    work = tempfile.mkdtemp(prefix="jobcorpus-selftest-")
    reports = []
    try:
        for row in wanted:
            try:
                reports.append(ROWS[row](work))
            except Exception as exc:  # noqa: BLE001 - a broken fixture is a result
                broken = Report(row)
                broken.check("self-test ran to completion", False, "%s: %s" % (type(exc).__name__, exc))
                reports.append(broken)
        for report in reports:
            print(report.render())
        failed = [r.row for r in reports if not r.ok]
        print("")
        if failed:
            print("SELF-TEST FAILED for: %s" % ", ".join(failed))
            return 1
        print("SELF-TEST OK for: %s" % ", ".join(r.row for r in reports))
        return 0
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
