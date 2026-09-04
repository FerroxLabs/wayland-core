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
WF_DIR = os.path.join(ROOT, ".github", "workflows")
# BOTH extensions, and the count is checked against the directory listing below:
# a `*.yml` glob silently excludes a `.yaml` file, and an excluded workflow is a
# workflow this sweep certifies without reading.
WORKFLOWS = sorted(glob.glob(os.path.join(WF_DIR, "*.yml"))
                   + glob.glob(os.path.join(WF_DIR, "*.yaml")))
ON_DISK = sorted(f for f in os.listdir(WF_DIR)
                 if os.path.isfile(os.path.join(WF_DIR, f)))
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
missed = sorted(set(ON_DISK) - set(os.path.basename(p) for p in WORKFLOWS))
emit(
    not missed,
    "no file in .github/workflows/ is outside the sweep",
    "not swept: " + repr(missed),
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

# ── the aggregate is told which workflow defines the jobs it grades ──────────
#
# wayland#1291 c2. The gate decides whether a `skipped` dependency was ALLOWED
# to skip by reading that job's own definition, so it needs the file that holds
# it. Getting this wrong is silent in the worst direction: point it at another
# workflow and every dependency becomes "not defined here", which fails closed
# and is loud -- but OMIT it and the gate cannot grade a skip at all. It is
# required unconditionally for that reason, and asserted here rather than
# trusted, because the env key is one deletion away from being gone with every
# self-test still green. Discovered from the call sites, not listed: a second
# caller gets the same rule for free.
wf_offenders = []
wf_callers = 0
for path, doc in parsed.items():
    for job_id, job in (doc.get("jobs") or {}).items():
        if not isinstance(job, dict):
            continue
        for step in job.get("steps") or []:
            if not isinstance(step, dict):
                continue
            if "assert-no-dependency-failed.sh" not in str(step.get("run") or ""):
                continue
            wf_callers += 1
            env = step.get("env") or {}
            declared = str(env.get("WORKFLOW_FILE") or "")
            want = os.path.join(".github", "workflows", os.path.basename(path))
            if declared != want:
                wf_offenders.append(
                    "%s / %s / %r passes WORKFLOW_FILE=%r, wants %r"
                    % (os.path.basename(path), job_id,
                       step.get("name") or step.get("uses"), declared, want)
                )
emit(
    not wf_offenders,
    "the aggregate gate is told which workflow defines its dependencies",
    "\n".join(wf_offenders),
)
emit(
    wf_callers >= 1,
    "the WORKFLOW_FILE rule found a caller to grade (anti-vacuity)",
    "callers seen: %d" % wf_callers,
)
info("aggregate-gate call sites graded for WORKFLOW_FILE: %d" % wf_callers)

# ── a step that runs a REPO script must come AFTER the checkout ──────────────
#
# MEASURED, PR #417 run 33542986300: the aggregate gate was rewritten from an
# inline `run:` block -- which needs no working tree -- into an invocation of
# `.github/scripts/assert-no-dependency-failed.sh`, and left in its original
# position ABOVE `actions/checkout`. Every dependency had succeeded, and the
# required `report` check went RED with
#   bash: .github/scripts/assert-no-dependency-failed.sh: No such file or directory
# A required check reporting a missing file when it means nothing failed is the
# same "red naming the wrong cause" that `!cancelled()` on the prerequisites
# exists to prevent, one step over -- and EVERY assertion in this file passed
# both before and after that mistake, which is why this one exists.
for jobs_file, doc in parsed.items():
    for job_id, job in (doc.get("jobs") or {}).items():
        if not isinstance(job, dict):
            continue
        seen_checkout = False
        offenders = []
        for step in job.get("steps") or []:
            if not isinstance(step, dict):
                continue
            if "actions/checkout" in str(step.get("uses") or ""):
                seen_checkout = True
                continue
            run = str(step.get("run") or "")
            # only repo-relative script invocations; `python3 -c`, inline bash and
            # absolute paths are not working-tree dependent in the same way.
            if ".github/scripts/" in run and not seen_checkout:
                offenders.append("%s / %s / %r runs a repo script before any checkout"
                                 % (os.path.basename(jobs_file), job_id,
                                    step.get("name") or step.get("uses")))
        if offenders:
            emit(False,
                 "a step that runs a repo script comes after the checkout",
                 "\n".join(offenders))
            break
    else:
        continue
    break
else:
    emit(True, "a step that runs a repo script comes after the checkout", "")

# ANTI-VACUITY for the rule above: it must actually have inspected steps that
# invoke repo scripts, or a workflow that stopped using them would pass it
# silently.
_script_steps = sum(
    1
    for doc in parsed.values()
    for job in (doc.get("jobs") or {}).values() if isinstance(job, dict)
    for step in (job.get("steps") or []) if isinstance(step, dict)
    and ".github/scripts/" in str(step.get("run") or "")
)
emit(_script_steps >= 3,
     "the checkout-ordering rule found repo-script steps to grade (anti-vacuity)",
     "repo-script steps seen: %d" % _script_steps)
info("repo-script steps graded for checkout ordering: %d" % _script_steps)

# -- RECONCILIATION with core#412 c2 / scripts/check-ci-step-suppression.py ---
#
# core#414 c2. Two gates now hold rules about the SAME fact -- which `if:` on a
# step keeps that step alive after an EARLIER step in the same job has failed --
# and they do not agree. Neither named the other, so the first person to satisfy
# one found out about the other by reddening it. The agreement is written here,
# and pointed back at from there, so both can be read together.
#
#   THIS FILE (core#414, core#405). In a job whose own `if:` normalises to
#   `always()` or `!cancelled()` AND which invokes a script declaring
#   `ADMISSION: unconditional`, every step up to and including the last such
#   gate must carry a condition that normalises EXACTLY to `always()` or
#   `!cancelled()`. Equality over a two-element set. Nothing may be ANDed on,
#   because `always() && X` is a gate X can switch back off -- KNOWN_BAD above
#   carries that exact mutation.
#
#   scripts/check-ci-step-suppression.py (core#412 c2/c3). Every step of
#   ci.yml`s `ci-linux` job must carry a condition CONTAINING `!cancelled()` or
#   `always()`, unless it is named in that file`s SUPPRESSIBLE map with the
#   reason its failure makes the later steps unmeasurable. SUBSTRING, not
#   equality -- it deliberately admits `!cancelled() && steps.ci_image.outcome
#   == 'success'`, because a step that runs INSIDE the CI image cannot report
#   anything once the image build has failed.
#
# WHICH ADMISSION FORMS SATISFY WHICH RULE:
#
#   `${{ !cancelled() }}` and `${{ always() }}`      -> BOTH. The only two that
#       do, and therefore the only two safe to write in a job both rules reach.
#   `${{ !cancelled() && steps.X.outcome == 'success' }}`
#                                                    -> core#412 ONLY. Accepted
#       there by substring; REJECTED here by equality.
#   no `if:` at all; `success()`; `failure()`; `needs.X.result != '...'`;
#   `hashFiles(...) != ' '`                        -> NEITHER.
#   (nothing satisfies THIS file only: the set this file accepts is a strict
#    SUBSET of the set core#412`s check accepts, so satisfying this one always
#    satisfies that one. The stricter rule is the one to write to.)
#
# WHY THEY HAVE NOT COLLIDED YET -- stated so nobody relies on it. On this tree
# the two rules grade DISJOINT jobs: this file`s prerequisite rule reaches
# `ci.yml/report` and `e2e.yml/e2e_report` (the only jobs with an unconditional
# job-level `if:` that also run a gate script), core#412`s reaches
# `ci.yml/ci-linux`, which has no job-level `if:` at all. A job that is both --
# an unconditional aggregate job that also runs steps inside an image -- must
# use the intersection, i.e. the bare form, and so gives up the image guard.
#
# THE ESCAPE HATCHES DIFFER, also deliberately. core#412 has SUPPRESSIBLE: name
# the step, give the reason. This file has NO hatch for a step at or before the
# last gate, because a required status check that reports on behalf of its
# dependencies has no step it may legitimately skip before it has graded.
#
# The marker below is split so that this assertion cannot be satisfied by its
# own source text: deleting the block above must red it.
_RECON_MARKER = "RECON" + "CILIATION with core#412 c2"
_SUPPRESSION_GATE = os.path.join(ROOT, "scripts", "check-ci-step-suppression.py")
_recon = []
if _RECON_MARKER not in open(os.path.abspath(__file__), encoding="utf-8").read():
    _recon.append("this file no longer carries the reconciliation block")
if not os.path.isfile(_SUPPRESSION_GATE):
    _recon.append("scripts/check-ci-step-suppression.py is gone; the rule it "
                  "reconciles with cannot be read beside this one")
elif "gate-admission.py" not in open(_SUPPRESSION_GATE, encoding="utf-8").read():
    _recon.append("scripts/check-ci-step-suppression.py no longer points back "
                  "at this file, so only one half of the pair can be found")
emit(
    not _recon,
    "the two admission rules record their reconciliation and name each other",
    "\n".join(_recon),
)


sys.exit(1 if FAILURES else 0)
