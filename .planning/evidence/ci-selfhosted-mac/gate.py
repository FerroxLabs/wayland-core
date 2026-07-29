#!/usr/bin/env python3
"""Gate for the self-hosted macOS runner job in .github/workflows/ci.yml.

It EXTRACTS the job's `if:` condition and `runs-on` from the workflow and
evaluates them, rather than hardcoding a copy — a hardcoded copy would be a
tautology over the gate's own constants.

The invariant this exists to protect is not "the job is configured nicely". It
is the pair of things that silently destroy a run or a machine:

  A. MUTUAL EXCLUSION. The self-hosted job and the hosted
     `Build (aarch64-apple-darwin)` matrix leg upload the SAME artifact name.
     actions/upload-artifact@v4+ rejects a duplicate artifact name within one
     run (409), so if both are ever scheduled together the run HARD-FAILS.
     This gate proves they are exclusive across the whole input space, by
     evaluating BOTH conditions — mine and the prior lane's — on every case.
  B. FORK-PR SAFETY. This repository is PUBLIC with
     `approval_policy=first_time_contributors`, so a returning contributor's
     fork PR runs unapproved. The self-hosted job must NEVER be reachable from
     `pull_request`, or fork code executes on a personal machine.

Usage:
    python3 gate.py [path/to/ci.yml]        # gate the real file
    python3 gate.py --self-test [path]      # prove the gate can fail
"""
import json
import os
import re
import sys

import yaml

JOB = "build-darwin-selfhosted"
EXPECTED_RUNS_ON = ["self-hosted", "macOS", "ARM64"]
HOSTED_DARWIN_ARM = "aarch64-apple-darwin"

# Reuse the prior lane's extractor/evaluator for the HOSTED condition, so the
# mutual-exclusion check reads the real hosted condition out of the same file
# instead of assuming what it says.
_HERE = os.path.dirname(os.path.abspath(__file__))
_BUDGET = os.path.join(_HERE, os.pardir, "ci-macos-budget", "gate.py")
_budget_ns = {}
with open(_BUDGET) as fh:
    exec(compile(fh.read(), _BUDGET, "exec"), _budget_ns)  # noqa: S102


def haystack(head_msg, commit_msgs):
    return json.dumps(head_msg if head_msg is not None else "") + json.dumps(
        commit_msgs if commit_msgs is not None else []
    )


class Unparsed(Exception):
    """An expression shape this evaluator does not model.

    Raised, never coerced to False. A clause silently evaluating to False would
    make the gate report the SAFE answer for an expression it did not actually
    understand — a self-passing result. Callers must convert this into a gate
    FAILURE, not swallow it.
    """


def eval_clause(clause, ev, ref, hm, cm):
    """Evaluate one atomic GHA clause. Unknown shapes raise, never silently pass."""
    clause = clause.strip()
    ctx = {"github.event_name": ev, "github.ref_name": ref}
    m = re.fullmatch(r"(\S+) == '([^']*)'", clause)
    if m:
        return ctx.get(m.group(1)) == m.group(2)
    m = re.fullmatch(r"(\S+) != '([^']*)'", clause)
    if m:
        return ctx.get(m.group(1)) != m.group(2)
    m = re.fullmatch(r"startsWith\((\S+), '([^']*)'\)", clause)
    if m:
        val = {"github.event_name": ev, "github.ref_name": ref}.get(m.group(1))
        return (val or "").startswith(m.group(2))
    m = re.fullmatch(r"contains\(format\(.+?\), '([^']*)'\)", clause)
    if m:
        # GHA contains() on strings is case-insensitive.
        return m.group(1).lower() in haystack(hm, cm).lower()
    raise Unparsed("gate cannot parse clause: %r" % clause)


def eval_if(cond, ev, ref, hm, cm):
    """Evaluate the job `if:` — a top-level && chain that may contain one !(a || b)."""
    parts, depth, buf = [], 0, ""
    i = 0
    while i < len(cond):
        ch = cond[i]
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if depth == 0 and cond[i:i + 2] == "&&":
            parts.append(buf)
            buf = ""
            i += 2
            continue
        buf += ch
        i += 1
    parts.append(buf)

    result = True
    for part in parts:
        part = part.strip()
        neg = False
        if part.startswith("!"):
            neg = True
            part = part[1:].strip()
            assert part.startswith("(") and part.endswith(")"), "negation must wrap a group"
            part = part[1:-1]
            sub = [p for p in re.split(r"\|\|(?![^(]*\))", part)]
            # split on top-level || only
            sub, depth, buf = [], 0, ""
            j = 0
            while j < len(part):
                c = part[j]
                if c == "(":
                    depth += 1
                elif c == ")":
                    depth -= 1
                if depth == 0 and part[j:j + 2] == "||":
                    sub.append(buf)
                    buf = ""
                    j += 2
                    continue
                buf += c
                j += 1
            sub.append(buf)
            val = any(eval_clause(s, ev, ref, hm, cm) for s in sub)
        else:
            val = eval_clause(part, ev, ref, hm, cm)
        result = result and (not val if neg else val)
    return result


# name, event, ref, head_msg, commit_msgs, expect_selfhosted_runs
CASES = [
    ("lane push, no token",          "push", "lane/x", "docs: notes",      ["docs: notes"],      True),
    ("lane push, [ci-darwin]",       "push", "lane/x", "[ci-darwin] need", ["[ci-darwin] need"], False),
    ("lane push, [ci-macos] alias",  "push", "lane/x", "feat [ci-macos]",  ["feat [ci-macos]"],  False),
    ("lane push, UPPERCASE token",   "push", "lane/x", "[CI-DARWIN] x",    ["[CI-DARWIN] x"],    False),
    ("lane push, token not on tip",  "push", "lane/x", "fixup",            ["[ci-darwin] e", "fixup"], False),
    ("lane push, hostile message",   "push", "lane/x", '"; rm -rf /; #',   ['"; rm -rf /; #'],   True),
    ("main push",                    "push", "main",   "release",          ["release"],          False),
    ("integration push",             "push", "plan/f20-unified-audit-repair", "merge", ["merge"], False),
    ("PULL_REQUEST (fork safety)",   "pull_request", "main", None, None,                          False),
    # DEFENCE IN DEPTH, and the case that makes INVARIANT B load-bearing.
    # On real GHA a pull_request's `github.ref_name` is "<PR#>/merge", so
    # startsWith(ref_name,'lane/') already excludes PRs incidentally. That
    # incidental exclusion is NOT the guard we rely on: the `event_name ==
    # 'push'` clause is. Without a pull_request case carrying a lane-shaped ref,
    # deleting the push-only clause changes NOTHING the gate can see — measured:
    # that mutation arm reported GATE_FAILURES=0 before this case existed.
    ("PULL_REQUEST w/ lane-shaped ref", "pull_request", "lane/x", "docs",  ["docs"],             False),
    ("lane-ish decoy 'lanes/x'",     "push", "lanes/x", "docs",            ["docs"],             False),
]


def fatal_on_arch_mismatch(step_body):
    """True iff the uname-mismatch branch itself terminates the job.

    Deliberately NOT `"exit 1" in step_body`. That was the first version and it
    is too coarse: this step contains a SECOND `exit 1` for the RUNNER_ARCH
    check, so gutting the uname branch left the coarse test satisfied and the
    mutation arm passed (measured: GATE_FAILURES=0). Scope the search to the
    branch under test.
    """
    lines = step_body.splitlines()
    start = None
    for i, ln in enumerate(lines):
        if "UNAME_M" in ln and "!=" in ln and ln.strip().startswith("if "):
            start = i
            break
    if start is None:
        return False
    for ln in lines[start + 1:]:
        if ln.strip() == "fi":
            return False
        if re.search(r"\bexit\s+[1-9]", ln):
            return True
    return False


def gate(text, verbose=True):
    fails = []
    doc = yaml.safe_load(text)
    jobs = doc.get("jobs", {})

    if JOB not in jobs:
        fails.append("job %r is absent" % JOB)
        print("GATE_FAILURES=%d" % len(fails))
        for f in fails:
            print("  FAIL", f)
        return fails
    j = jobs[JOB]

    # --- runner targeting -------------------------------------------------
    if j.get("runs-on") != EXPECTED_RUNS_ON:
        fails.append("runs-on is %r, expected %r" % (j.get("runs-on"), EXPECTED_RUNS_ON))

    # --- machine protection ----------------------------------------------
    conc = j.get("concurrency") or {}
    if not conc.get("group"):
        fails.append("no job-level concurrency group -> lane bursts pile up on one serial runner")
    if conc.get("cancel-in-progress") is not True:
        fails.append("concurrency.cancel-in-progress is not true -> superseded builds are not coalesced")
    if "${{ github.ref }}" not in str(conc.get("group", "")):
        fails.append("concurrency group is not per-ref -> lanes would cancel each other")
    if not isinstance(j.get("timeout-minutes"), int):
        fails.append("no integer timeout-minutes -> a runaway build can own the owner's machine")

    # --- the Rosetta trap must be checked, and must be FATAL --------------
    steps = j.get("steps", [])
    arch_steps = [s for s in steps if "uname -m" in str(s.get("run", ""))]
    if not arch_steps:
        fails.append("no step measures `uname -m` -> ARM64 label is trusted, not verified")
    else:
        body = str(arch_steps[0].get("run", ""))
        if not fatal_on_arch_mismatch(body):
            fails.append("the uname-mismatch branch does not exit non-zero -> a Rosetta/x64 "
                         "runner would still build and upload a mislabelled arm64 artifact")
        if steps.index(arch_steps[0]) != 0:
            fails.append("arch assertion is not the FIRST step -> repo code is checked out before the check")

    # --- artifact integrity check must not be tautological ----------------
    verify = [s for s in steps if "lipo -archs" in str(s.get("run", ""))]
    if not verify:
        fails.append("no `lipo -archs` verification of the produced binary")
    else:
        body = str(verify[0].get("run", ""))
        if re.search(r"file\s+\"?\$", body) and "file -b" not in body:
            fails.append("uses `file <path>` without -b -> matches the aarch64 in the PATH (tautology)")

    # --- artifact name must stay the documented one -----------------------
    ups = [s for s in steps if "upload-artifact" in str(s.get("uses", ""))]
    if not ups:
        fails.append("job uploads no artifact -> the lane still cannot obtain a binary")
    elif ups[0].get("with", {}).get("name") != "wayland-core-%s" % HOSTED_DARWIN_ARM:
        fails.append("artifact name is %r, breaks the documented `gh run download` route"
                     % ups[0].get("with", {}).get("name"))

    # --- truth table + the two cross-cutting invariants -------------------
    cond = j.get("if")
    if not cond:
        fails.append("job has no `if:` -> it would run on pull_request from forks")
        cond = "github.event_name == 'never'"

    hosted_conds, hosted_literals = _budget_ns["extract"](text)
    hosted_cond = hosted_conds[0] if hosted_conds else None

    for name, ev, ref, hm, cm, exp in CASES:
        # An expression this evaluator cannot model is a FAILURE, not a crash and
        # not a False. Crashing aborted the remaining self-test arms when this
        # was first written; coercing to False would have reported the safe
        # answer for an expression the gate never understood.
        try:
            got = eval_if(cond, ev, ref, hm, cm)
        except Unparsed as exc:
            fails.append("case %r: %s" % (name, exc))
            continue
        if got != exp:
            fails.append("case %r: self-hosted runs=%s expected=%s" % (name, got, exp))
            continue

        # INVARIANT A — mutual exclusion with the hosted arm64 darwin leg.
        if hosted_cond is not None:
            hosted_darwin = _budget_ns["evaluate"](hosted_cond, ev, ref, hm, cm)
            hosted_builds_arm = hosted_darwin and any(
                HOSTED_DARWIN_ARM in full for full, _ in hosted_literals)
            if got and hosted_builds_arm:
                fails.append("case %r: BOTH self-hosted and hosted arm64 darwin scheduled "
                             "-> duplicate artifact name -> run hard-fails (409)" % name)

        # INVARIANT B — fork-PR safety.
        if ev == "pull_request" and got:
            fails.append("case %r: self-hosted job reachable from pull_request on a PUBLIC repo" % name)

        if verbose:
            print("  ok   %-30s self-hosted=%s" % (name, got))

    print("GATE_FAILURES=%d" % len(fails))
    for f in fails:
        print("  FAIL", f)
    return fails


def naive_matcher(text):
    """The gate a hurried author would have written: 'is the runner wired up?'

    Required by LANE-BRIEF 6b-ii: the self-test must prove the repair does
    something, by showing the OLD/naive matcher passes where the real gate fails.
    This one greps for the labels, which is exactly the shape that feels
    sufficient and is not.
    """
    return ("self-hosted" in text) and ("ARM64" in text) and (JOB in text)


def self_test(text):
    print("=== ARM 0: unmutated workflow (MUST pass) ===")
    base = gate(text)
    assert not base, "gate failed on the unmutated workflow"
    assert naive_matcher(text), "naive matcher should also pass here"

    arms = [
        ("drop the [ci-darwin] mutual exclusion (artifact 409)",
         lambda t: t.replace(
             "      && !(contains(format('{0}{1}', toJSON(github.event.head_commit.message), toJSON(github.event.commits.*.message)), '[ci-darwin]')\n"
             "      || contains(format('{0}{1}', toJSON(github.event.head_commit.message), toJSON(github.event.commits.*.message)), '[ci-macos]'))\n",
             "")),
        ("allow pull_request (fork code on a personal Mac)",
         lambda t: t.replace("      github.event_name == 'push'\n",
                             "      github.event_name != 'nope'\n")),
        ("retarget to the hosted pool",
         lambda t: t.replace("    runs-on: [self-hosted, macOS, ARM64]\n",
                             "    runs-on: macos-latest\n")),
        ("drop concurrency coalescing (pile-up on the owner's laptop)",
         lambda t: t.replace("      cancel-in-progress: true\n    # Hard ceiling",
                             "      cancel-in-progress: false\n    # Hard ceiling")),
        ("make the Rosetta arch assertion non-fatal",
         lambda t: t.replace(
             "            echo \"::error title=Runner arch mismatch::Runner is labelled ARM64 but uname -m reports '${UNAME_M}'. Refusing to build — this is the Rosetta/x64-package trap. Do NOT relabel; reinstall the arm64 runner package.\"\n            exit 1\n",
             "            echo \"warning: arch mismatch\"\n")),
    ]
    ok = True
    for label, mutate in arms:
        mutated = mutate(text)
        assert mutated != text, "mutation %r did not change the file" % label
        print("\n=== ARM: %s (MUST fail) ===" % label)
        # One arm must never be able to abort the remaining arms. The first
        # version of this loop let an evaluator exception propagate, and arms
        # 3-5 silently never ran while the suite still printed output.
        try:
            f = gate(mutated, verbose=False)
        except Exception as exc:  # noqa: BLE001 - an arm that explodes still counts as detected
            print("  gate raised on this arm (counts as detected): %r" % (exc,))
            f = ["gate raised: %r" % (exc,)]
        if not f:
            print("  !! GATE DID NOT DETECT THIS MUTATION — gate is self-passing")
            ok = False
        # THE THIRD ASSERTION: prove the repair does something. A naive
        # "is the runner wired up?" matcher passes on every one of these.
        # Only claim the comparison when the real gate ACTUALLY caught it —
        # printing "real gate caught what it would have missed" next to a
        # GATE_FAILURES=0 arm is the instrument lying about itself.
        elif naive_matcher(mutated):
            print("  (naive matcher PASSES this mutation — real gate caught what it would have missed)")
        else:
            print("  !! naive matcher also caught it; this arm does not demonstrate the improvement")
            ok = False
    print("\nSELF_TEST=%s" % ("PASS" if ok else "FAIL"))
    return ok


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if a != "--self-test"]
    path = args[0] if args else ".github/workflows/ci.yml"
    text = open(path).read()
    if "--self-test" in sys.argv[1:]:
        sys.exit(0 if self_test(text) else 1)
    sys.exit(1 if gate(text) else 0)
