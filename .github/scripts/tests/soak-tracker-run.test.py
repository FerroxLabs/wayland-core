#!/usr/bin/env python3
"""Execute the soak-tracker JOB, end to end, off GitHub. FerroxLabs/wayland-core#325 c2.

WHY THIS EXISTS, AND WHY THE UNIT SUITE IS NOT ENOUGH
    `soak-tracker-truth.test.sh` grades the DECISION SCRIPT and greps the
    workflow for the strings that wire it up. Both are necessary and neither
    is the criterion, which reads:

        "A run whose sibling job failed posts a red report instead of closing
         the tracker green."

    Between "the script decided `report`" and "a red report was posted" sit
    four pieces of plumbing that no grep and no unit case touches:

      1. the multi-line `JOB_RESULTS: |` block, whose left-hand names are
         typed by hand next to `${{ needs.<id>.result }}` -- `keyring-blob-size
         =${{ needs.windows-soak.result }}` reads perfectly and is a lie;
      2. `${{ needs.<typo>.result }}`, which GitHub expands to the EMPTY
         STRING rather than failing, and which the decision script must then
         refuse to certify;
      3. `$GITHUB_OUTPUT` -> `steps.decide.outputs.action`, the only channel
         between the script and the two steps that act. The unit suite never
         sets GITHUB_OUTPUT, so that write is completely ungraded there: the
         script could emit nothing at all and every unit case would still pass
         while a real run posted nothing;
      4. the `actions/github-script` bodies themselves -- the code that
         actually creates the comment, and the code that must NOT close.

    This harness runs the real YAML, the real decision script and the real
    github-script bodies against a stubbed Octokit, and asserts on the API
    calls that come out. What it does NOT prove is GitHub's own scheduler
    admitting the job through `if: always()` when a sibling is red; that needs
    one real dispatch, is tracked separately, and is stated in the ledger.

Run: python3 .github/scripts/tests/soak-tracker-run.test.py
"""
import json
import os
import re
import subprocess
import sys
import tempfile

import yaml

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
WF = os.path.join(ROOT, ".github", "workflows", "nightly-windows-soak.yml")
TRACKER = "soak-tracker"

PASS = 0
FAIL = 0


def ok(label):
    global PASS
    PASS += 1
    print("ok   %s" % label)


def bad(label, detail=""):
    global FAIL
    FAIL += 1
    print("FAIL %s" % label)
    if detail:
        for line in str(detail).splitlines():
            print("       | %s" % line)


def want(label, cond, detail=""):
    ok(label) if cond else bad(label, detail)


# ── the workflow, as the runner reads it ─────────────────────────────────────

with open(WF, encoding="utf-8") as fh:
    WORKFLOW = yaml.safe_load(fh)

JOBS = WORKFLOW["jobs"]
JOB = JOBS[TRACKER]
NEEDS = JOB["needs"]
STEPS = JOB["steps"]

EXPR = re.compile(r"\$\{\{\s*needs\.([A-Za-z0-9_-]+)\.result\s*\}\}")
STEP_IF = re.compile(
    r"^\$\{\{\s*steps\.([A-Za-z0-9_-]+)\.outputs\.([A-Za-z0-9_-]+)"
    r"\s*==\s*'([^']*)'\s*\}\}$"
)


def render(value, results):
    """Substitute `${{ needs.X.result }}` the way the runner would.

    An id that is not in `needs:` is a WIRING DEFECT here, deliberately louder
    than GitHub, which silently expands it to the empty string. That silence is
    the failure mode the decision script's `uninterpretable` arm exists for; it
    should never have to fire because a name was mistyped in the YAML.
    """
    missing = [j for j in EXPR.findall(value) if j not in results]
    if missing:
        raise AssertionError(
            "step reads needs.%s.result, which is not in this job's needs: %s. "
            "GitHub expands that to the empty string, so the tracker would be "
            "deciding from a roster with a hole in it." % (missing[0], NEEDS)
        )
    return EXPR.sub(lambda m: results[m.group(1)], value)


def job_results_pairs(value):
    """[(declared name, needs id)] for each line of a `JOB_RESULTS:` block."""
    out = []
    for line in value.strip().splitlines():
        line = line.strip()
        if not line:
            continue
        name, _, rhs = line.partition("=")
        ids = EXPR.findall(rhs)
        out.append((name.strip(), ids[0] if ids else None))
    return out


# ── 1. static wiring the runner would honour ─────────────────────────────────

print("soak-tracker-run: the tracker JOB, executed off GitHub (core#325 c2)")

want("the tracker job is admitted whatever the siblings did",
     "always()" in str(JOB.get("if", "")), JOB.get("if"))
want("the tracker depends on every job it grades",
     set(NEEDS) >= {"windows-soak", "keyring-blob-size", "windows-live-acceptance"},
     NEEDS)
want("the tracker may write issues",
     JOB.get("permissions", {}).get("issues") == "write", JOB.get("permissions"))

# THE CROSS-WIRING GUARD. Every `<name>=${{ needs.<id>.result }}` pair must
# name the job it reads. A mismatch produces a roster that is complete, well
# formed, parseable -- and wrong about which job failed, which is core#325
# with the labels swapped.
blocks = 0
for step in STEPS:
    for key, value in (step.get("env") or {}).items():
        if key != "JOB_RESULTS":
            continue
        blocks += 1
        for declared, read in job_results_pairs(value):
            want("JOB_RESULTS in step %r reports %s from needs.%s"
                 % (step.get("name", "?"), declared, read),
                 declared == read and read in NEEDS,
                 "declared %r but reads %r" % (declared, read))
want("every step that acts carries its own JOB_RESULTS block", blocks == 3, blocks)

decide_steps = [s for s in STEPS if s.get("id") == "decide"]
want("exactly one step produces the decision", len(decide_steps) == 1, len(decide_steps))
DECIDE = decide_steps[0]


# ── 2. run the job ───────────────────────────────────────────────────────────

STUB_HEAD = """
const calls = [];
const github = { rest: { issues: {
  listForRepo: async (a) => { calls.push({ call: 'listForRepo', args: a }); return { data: %s }; },
  createComment: async (a) => { calls.push({ call: 'createComment', args: a }); return { data: {} }; },
  create:        async (a) => { calls.push({ call: 'create', args: a }); return { data: {} }; },
  update:        async (a) => { calls.push({ call: 'update', args: a }); return { data: {} }; },
}}};
const context = {
  serverUrl: 'https://github.com',
  repo: { owner: 'FerroxLabs', repo: 'wayland-core' },
  runId: 33053333326,
};
const core = {
  info:      (m) => calls.push({ call: 'info', args: m }),
  setFailed: (m) => calls.push({ call: 'setFailed', args: m }),
};
(async () => {
%s
})().then(
  () => { console.log('@@CALLS@@' + JSON.stringify(calls)); },
  (e) => { console.log('@@CALLS@@' + JSON.stringify(calls.concat([{ call: 'threw', args: String(e) }]))); },
);
"""


def run_job(results, existing_issues):
    """-> (outputs, [step name], [api calls]) for one synthetic run."""
    tmp = tempfile.mkdtemp(prefix="soak-tracker-run-")
    gh_out = os.path.join(tmp, "github_output")
    open(gh_out, "w").close()

    env = dict(os.environ)
    env["GITHUB_OUTPUT"] = gh_out
    for key, value in (DECIDE.get("env") or {}).items():
        env[key] = render(str(value), results)

    proc = subprocess.run(
        ["bash", "-c", DECIDE["run"]],
        cwd=ROOT, env=env, capture_output=True, text=True,
    )

    outputs = {}
    for line in open(gh_out, encoding="utf-8"):
        if "=" in line:
            k, _, v = line.partition("=")
            outputs[k.strip()] = v.strip()

    ran, calls = [], []
    for step in STEPS:
        if step is DECIDE or "if" not in step:
            continue
        m = STEP_IF.match(str(step["if"]).strip())
        if not m:
            raise AssertionError(
                "step %r is gated on %r, which this harness cannot evaluate. "
                "Teach it the new form rather than letting the step go "
                "silently ungraded." % (step.get("name"), step["if"])
            )
        step_id, output, literal = m.groups()
        if step_id != "decide":
            raise AssertionError("step %r reads outputs of %r, not the decision"
                                 % (step.get("name"), step_id))
        if outputs.get(output) != literal:
            continue
        ran.append(step.get("name"))

        body = step["with"]["script"]
        js = os.path.join(tmp, "step.js")
        with open(js, "w", encoding="utf-8") as fh:
            fh.write(STUB_HEAD % (json.dumps(existing_issues), body))
        senv = dict(os.environ)
        for key, value in (step.get("env") or {}).items():
            senv[key] = render(str(value), results)
        node = subprocess.run(["node", js], capture_output=True, text=True, env=senv)
        marker = [l for l in node.stdout.splitlines() if l.startswith("@@CALLS@@")]
        if not marker:
            raise AssertionError("the github-script body produced no result: %s\n%s"
                                 % (node.stdout, node.stderr))
        calls.extend(json.loads(marker[-1][len("@@CALLS@@"):]))

    return outputs, ran, calls, proc


OPEN_TRACKER = [{
    "number": 319,
    "title": "[nightly-windows-soak] FAIL - 2026-08-27",
}]

GREEN = {"windows-soak": "success", "keyring-blob-size": "success",
         "windows-live-acceptance": "success"}
SIBLING_RED = dict(GREEN, **{"windows-live-acceptance": "failure"})


def kinds(calls):
    return [c["call"] for c in calls]


# ── THE CRITERION, run rather than modelled ──────────────────────────────────
#
# Run 33053333326, replayed: the reporting job green, the self-hosted
# live-acceptance job red. That run posted the word GREEN and closed #319.
try:
    outputs, ran, calls, proc = run_job(SIBLING_RED, OPEN_TRACKER)
except AssertionError as exc:
    bad("a run whose sibling failed reaches a decision", exc)
    outputs, ran, calls = {}, [], []

want("a sibling failure decides `report`, from the real YAML env block",
     outputs.get("action") == "report", outputs)
want("the decision reaches the steps through $GITHUB_OUTPUT",
     set(outputs) >= {"action", "reason"},
     "the decision script wrote %r to GITHUB_OUTPUT; a step gated on "
     "steps.decide.outputs.action cannot fire on that" % outputs)
want("the close step does not run on a red sibling",
     not any("Close" in (n or "") for n in ran), ran)
want("the report step runs on a red sibling",
     any("Report" in (n or "") for n in ran), ran)
want("a red report is POSTED, not merely decided",
     "createComment" in kinds(calls), kinds(calls))
want("nothing closes the tracker on a red run",
     not any(c["call"] == "update" and c["args"].get("state") == "closed"
             for c in calls),
     kinds(calls))
comment = next((c for c in calls if c["call"] == "createComment"), None)
want("the posted report names the job that actually failed",
     comment is not None
     and "windows-live-acceptance" in comment["args"]["body"]
     and "windows-live-acceptance=failure" in comment["args"]["body"],
     comment["args"]["body"] if comment else "no comment posted")
want("the report lands on the tracker issue, not a new one",
     comment is not None and comment["args"]["issue_number"] == 319,
     comment["args"] if comment else None)

# Same red run with no tracker open yet: three consecutive reds produced ZERO
# issues under the old wiring, so "posts a red report" has to include opening
# one.
outputs, ran, calls, _ = run_job(SIBLING_RED, [])
created = next((c for c in calls if c["call"] == "create"), None)
want("with no tracker open, the red run OPENS one",
     created is not None
     and created["args"]["title"].startswith("[nightly-windows-soak] FAIL"),
     kinds(calls))
want("the opened issue keeps the label narrowing the reporter is bound by",
     created is not None
     and sorted(created["args"]["labels"]) == ["test-debt", "windows-soak"],
     created["args"].get("labels") if created else None)

# ── THE NEGATIVE CONTROL ─────────────────────────────────────────────────────
#
# A guard that never closes anything would pass every assertion above. A
# genuinely all-green run must still close the tracker.
outputs, ran, calls, _ = run_job(GREEN, OPEN_TRACKER)
want("an all-green run decides `close`", outputs.get("action") == "close", outputs)
want("an all-green run runs the close step and not the report step",
     any("Close" in (n or "") for n in ran)
     and not any("Report" in (n or "") for n in ran), ran)
want("an all-green run actually closes the tracker",
     any(c["call"] == "update" and c["args"].get("state") == "closed"
         for c in calls), kinds(calls))
want("an all-green run opens nothing", "create" not in kinds(calls), kinds(calls))

# ── the roster holes the plumbing can produce ────────────────────────────────
outputs, ran, calls, _ = run_job(
    dict(GREEN, **{"keyring-blob-size": ""}), OPEN_TRACKER)
want("an empty result -- a mistyped needs id -- closes nothing and reports nothing",
     outputs.get("action") == "none" and ran == [], (outputs, ran))

outputs, ran, calls, _ = run_job(
    dict(GREEN, **{"windows-live-acceptance": "skipped"}), OPEN_TRACKER)
want("a skipped sibling closes nothing", outputs.get("action") == "none", outputs)

print()
print("soak-tracker-run: %d passed, %d failed" % (PASS, FAIL))
sys.exit(1 if FAIL else 0)
