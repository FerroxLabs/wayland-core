#!/usr/bin/env python3
"""Total admission guard for the gates that decide a required status check.

WHY THIS IS NOT A GREP FOR `needs`. wayland#1177 c1's first fix added a job
name to the evidence gate's `if:`; its second fix removed all job names and
asserted only their ABSENCE. A verifier then refuted the second fix with two
conditions that name no job and still switch the gate off:

    if: ${{ hashFiles('junit-reports/**/*.xml') != '' }}   # inert with NO evidence
    if: ${{ success() }}                                   # inert once anything failed

"Does this condition mention `needs`?" is a NEGATIVE test over an OPEN alphabet
of switchable expressions -- necessary, never sufficient. This file asks the
POSITIVE, closed question instead:

    is the condition EXACTLY one of {always(), !cancelled()}?

That is decidable by string equality over a two-element set. `always()` and
`!cancelled()` are the only GitHub Actions expressions that both (a) survive an
earlier step in the same job failing -- a plain expression, `if:`-less step
included, is skipped outright once one has -- and (b) cannot be made false by
any upstream job's result. Every other condition, present or future, named or
unnamed, is rejected without this file having to know what it is.

WHICH STEPS IT APPLIES TO IS DISCOVERED, NOT LISTED. Scripts under
.github/scripts/ declare their own policy with an `ADMISSION:` line; every
script any workflow actually invokes must carry one, so a new gate cannot join
the repository unclassified. Call sites are found by parsing all of
.github/workflows/, so a second caller cannot be missed the way e2e.yml's was.

Emits `PASS <label>` / `FAIL <label>` / `INFO <text>` on stdout for the calling
bash suite to count. Exit status is 1 if any assertion failed.
"""

import glob
import json
import os
import re
import sys

try:
    import yaml
except ImportError:  # pragma: no cover - the caller installs it and re-runs
    print("FAIL PyYAML is available to read the workflows")
    sys.exit(1)

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
WORKFLOWS = sorted(glob.glob(os.path.join(ROOT, ".github", "workflows", "*.yml")))
SCRIPTS = sorted(glob.glob(os.path.join(ROOT, ".github", "scripts", "*.sh")))

# The closed set. Both survive an earlier failed step AND cannot be falsified by
# an upstream job. Nothing else does both, which is why this is an allowlist.
UNCONDITIONAL = frozenset({"always()", "!cancelled()"})

FAILURES = 0


def emit(passed, label, detail=None):
    global FAILURES
    print(("PASS " if passed else "FAIL ") + label)
    if not passed:
        FAILURES += 1
        if detail:
            for line in str(detail).splitlines():
                print("INFO     | " + line)


def info(text):
    print("INFO " + text)


def normalise(condition):
    """`${{ !cancelled() }}` -> `!cancelled()`; None stays None."""
    if condition is None:
        return None
    text = str(condition).strip()
    if text.startswith("${{") and text.endswith("}}"):
        text = text[3:-2]
    return re.sub(r"\s+", "", text)


def admits_unconditionally(condition):
    return normalise(condition) in UNCONDITIONAL


# ── the classifier's polarity, proven before it is trusted ────────────────────
#
# An allowlist read backwards accepts everything and reports zero offenders,
# which is indistinguishable from a clean tree. These two arms pin the polarity
# in the same run: the refutation's own mutations must be REJECTED and the two
# sanctioned forms must be ACCEPTED.
KNOWN_BAD = [
    "${{ hashFiles('junit-reports/**/*.xml') != '' }}",   # verifier M1
    "${{ success() }}",                                   # verifier M2
    "${{ needs.e2e.result != 'cancelled' }}",             # e2e.yml before this lane
    "${{ needs.ci.result != 'skipped' || needs['ci-linux'].result != 'skipped' }}",
    "${{ always() && needs.ci.result != 'skipped' }}",    # a gate ANDed back off
    "${{ failure() }}",
    "${{ github.event_name == 'push' }}",
    None,                                                 # no condition at all
]
KNOWN_GOOD = ["${{ !cancelled() }}", "always()", "${{ always() }}", "!cancelled()"]

wrongly_accepted = [c for c in KNOWN_BAD if admits_unconditionally(c)]
emit(
    not wrongly_accepted,
    "the admission classifier rejects every condition that can go inert",
    "wrongly accepted: " + repr(wrongly_accepted),
)
wrongly_rejected = [c for c in KNOWN_GOOD if not admits_unconditionally(c)]
emit(
    not wrongly_rejected,
    "the admission classifier accepts the two status-check forms (anti-vacuity)",
    "wrongly rejected: " + repr(wrongly_rejected),
)

# ── parse the corpus ─────────────────────────────────────────────────────────
parsed = {}
broken = []
for path in WORKFLOWS:
    try:
        parsed[path] = yaml.safe_load(open(path, encoding="utf-8")) or {}
    except yaml.YAMLError as exc:
        broken.append("%s: %s" % (os.path.basename(path), exc))
emit(not broken, "every workflow file parses as YAML", "\n".join(broken))
emit(
    len(parsed) >= 10,
    "the workflow corpus is the whole directory (control: >=10 files)",
    "parsed %d" % len(parsed),
)
info("workflow files parsed: %d" % len(parsed))


def steps_of(doc):
    for job_id, job in (doc.get("jobs") or {}).items():
        if not isinstance(job, dict):
            continue
        for step in job.get("steps") or []:
            if isinstance(step, dict):
                yield job_id, job, step


def step_text(step):
    """Everything a step could invoke a script from: `run:` and any `with:`
    value (nick-fields/retry passes the command through `with: command:`, which
    a `run:`-only scan misses -- and that is exactly where this repository runs
    the test wrapper)."""
    parts = [str(step.get("run") or "")]
    for value in (step.get("with") or {}).values():
        parts.append(str(value))
    return "\n".join(parts)


# ── every invoked script declares an admission policy ────────────────────────
POLICY_RE = re.compile(r"^#\s*ADMISSION:\s*(unconditional|caller-decides)\b", re.M)

policies = {}
for path in SCRIPTS:
    found = POLICY_RE.findall(open(path, encoding="utf-8").read())
    policies[os.path.basename(path)] = found

call_sites = {}
for path, doc in parsed.items():
    for job_id, job, step in steps_of(doc):
        text = step_text(step)
        for name in policies:
            if name in text:
                call_sites.setdefault(name, []).append(
                    (os.path.basename(path), job_id, step.get("name"), step.get("if"))
                )

invoked = sorted(call_sites)
info("scripts in .github/scripts/: %d; invoked by a workflow: %d"
     % (len(policies), len(invoked)))

unclassified = [n for n in invoked if len(policies[n]) != 1]
emit(
    not unclassified,
    "every script a workflow invokes declares exactly one ADMISSION policy",
    "unclassified or duplicated: " + repr({n: policies[n] for n in unclassified}),
)

unconditional_scripts = [n for n in invoked if policies[n] == ["unconditional"]]
emit(
    len(unconditional_scripts) >= 2,
    "at least two invoked scripts are declared unconditional (anti-vacuity)",
    "declared unconditional: " + repr(unconditional_scripts),
)

graded = [(n, site) for n in unconditional_scripts for site in call_sites[n]]
emit(
    len(graded) >= 3,
    "the sweep found call sites to grade (anti-vacuity)",
    "graded call sites: %d" % len(graded),
)
info("unconditional call sites graded: %d" % len(graded))

offenders = []
for name, (wf, job_id, step_name, condition) in graded:
    if not admits_unconditionally(condition):
        offenders.append(
            "%s / %s / %r runs %s under if: %r" % (wf, job_id, step_name, name, condition)
        )
emit(
    not offenders,
    "every caller of an unconditional gate is admitted by always() or !cancelled()",
    "\n".join(offenders),
)

# ── a gate's PREREQUISITES are admitted on the same terms as the gate ────────
#
# Admitting the gate is not enough. MEASURED on run 33320774111: ci.yml's
# `report` job ran its evidence gate (it carries `!cancelled()`) and the gate
# died with `bash: .github/scripts/assert-test-evidence.sh: No such file or
# directory`, exit 127 -- because `No dependency failed` had failed, and the
# CHECKOUT two steps below it carried no condition and was therefore skipped
# along with the artifact download. A required check that reports "No such
# file" where it means "a dependency failed" is a red naming the wrong cause,
# and the gate graded nothing either way.
#
# The rule is scoped by the JOB's own admission, which is what makes it total
# rather than a list of step names: a job that itself runs unconditionally is a
# job that exists to speak when its dependencies did not, so every step of it up
# to and including its last gate must survive an earlier failure too. Steps
# AFTER the last gate are unconstrained -- `Publish test report` is guarded on
# `hashFiles(...)` on purpose, and by then the gate has already graded.
prereq_offenders = []
prereq_jobs = 0
for path, doc in parsed.items():
    for job_id, job in (doc.get("jobs") or {}).items():
        if not isinstance(job, dict) or not admits_unconditionally(job.get("if")):
            continue
        steps = [s for s in (job.get("steps") or []) if isinstance(s, dict)]
        last_gate = -1
        for i, step in enumerate(steps):
            text = step_text(step)
            if any(n in text for n in unconditional_scripts):
                last_gate = i
        if last_gate < 0:
            continue
        prereq_jobs += 1
        for step in steps[:last_gate + 1]:
            if not admits_unconditionally(step.get("if")):
                prereq_offenders.append(
                    "%s / %s / %r is admitted by %r"
                    % (os.path.basename(path), job_id, step.get("name") or step.get("uses"),
                       step.get("if"))
                )
emit(
    not prereq_offenders,
    "in an unconditional job, every step up to its last gate is unconditional too",
    "\n".join(prereq_offenders),
)
emit(
    prereq_jobs >= 2,
    "the prerequisite rule found the aggregate jobs to apply to (anti-vacuity)",
    "unconditional jobs containing a gate: %d" % prereq_jobs,
)
info("aggregate jobs graded for prerequisites: %d" % prereq_jobs)

# ── the aggregate check grades every job it depends on, and names none ───────
CI = os.path.join(ROOT, ".github", "workflows", "ci.yml")
ci_doc = parsed.get(CI, {})
ci_jobs = ci_doc.get("jobs") or {}
report = ci_jobs.get("report") or {}
needs = report.get("needs") or []
if isinstance(needs, str):
    needs = [needs]

declared = re.findall(r"^\s*#\s*not-aggregated:\s*(\S+)", open(CI, encoding="utf-8").read(), re.M)
roster = [j for j in ci_jobs if j != "report"]
unaccounted = sorted(set(roster) - set(needs) - set(declared))
emit(
    not unaccounted,
    "every ci.yml job is aggregated by `report` or declared not-aggregated",
    "unaccounted: " + repr(unaccounted),
)
stale = sorted(set(declared) - set(roster))
emit(
    not stale,
    "no not-aggregated declaration names a job that no longer exists",
    "stale declarations: " + repr(stale),
)
emit(
    len(roster) >= 5 and len(needs) >= 5,
    "the ci.yml roster and `report.needs` were both read (anti-vacuity)",
    "roster=%d needs=%d" % (len(roster), len(needs)),
)
info("ci.yml jobs=%d aggregated=%d declared-not-aggregated=%d"
     % (len(roster), len(needs), len(declared)))

# A `run:` block inside the aggregate job that names a job is an enumeration,
# and an enumeration beside `needs:` is what drifted. `env:` diagnostics are
# excluded ON PURPOSE and stated as such: a message cannot make a gate inert.
NAMES_A_JOB = re.compile(r"needs(\.|\[)")
enumerating = []
for step in report.get("steps") or []:
    if isinstance(step, dict) and NAMES_A_JOB.search(str(step.get("run") or "")):
        enumerating.append(repr(step.get("name")))
emit(
    not enumerating,
    "no `run:` block in the report job enumerates a dependency",
    "enumerating steps: " + ", ".join(enumerating),
)
# ...and the polarity of THAT check, on a synthetic step, so a regex that has
# stopped matching cannot report a clean tree.
emit(
    bool(NAMES_A_JOB.search('check ci "${{ needs.ci.result }}"'))
    and not NAMES_A_JOB.search("bash .github/scripts/assert-no-dependency-failed.sh"),
    "the enumeration detector still matches a known enumeration (anti-vacuity)",
)

aggregate = None
for step in report.get("steps") or []:
    if isinstance(step, dict) and "assert-no-dependency-failed.sh" in str(step.get("run") or ""):
        aggregate = step
emit(
    aggregate is not None
    and "toJSON(needs)" in json.dumps(aggregate.get("env") or {}),
    "the aggregate gate is fed the WHOLE `needs` object, not a list of names",
    repr(aggregate),
)

sys.exit(1 if FAILURES else 0)
