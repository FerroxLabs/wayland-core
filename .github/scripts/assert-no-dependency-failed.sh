#!/usr/bin/env bash
# A REQUIRED CHECK MUST GRADE WHAT IT AGGREGATES -- ALL OF IT, NAMING NONE OF IT.
#
# ADMISSION: unconditional -- every workflow step that runs this script must be
# admitted by a status-check function (`always()` / `!cancelled()`), because a
# gate a sibling job can switch off grades nothing on exactly the runs where
# something went wrong. Enforced by
# .github/scripts/tests/report-gate-wiring.test.sh.
#
# WHY THIS IS A SCRIPT AND NOT SIX LINES OF YAML. The body it replaces was a
# hand-written enumeration inside ci.yml's `report` job:
#
#     needs:   [ci, ci-windows-hosted, ci-linux, eval-gate-linux, build, all-features-check]
#     check ci                 "${{ needs.ci.result }}"
#     check ci-windows-hosted  "${{ needs.ci-windows-hosted.result }}"
#     ... four more, by hand
#
# Two lists of six names with nothing keeping them in sync. Deleting a single
# `check` line left the required check silently passing over that job with
# every self-test on the branch still green -- measured, and the reason this
# file exists. The same class one level up cost more: `ci-linux` was missing
# from `needs:` itself until 2026-08-30, and `report` went GREEN over a RED
# `ci-linux` (run 33262552890).
#
# The set graded is now the set depended on, BY CONSTRUCTION: the caller passes
# ${{ toJSON(needs) }} and this script iterates it. There is no second list to
# drift, so "the aggregate silently skipped a dependency" is not a bug that can
# be reintroduced by editing this file or the workflow.
#
# A SKIP MUST BE EXPLAINED, NOT ASSUMED (wayland#1291 c2). Until 2026-09-04 this
# gate treated EVERY `skipped` as OK. That is right for the macOS/Windows legs,
# which are rationed by design -- and it made every other skip invisible with
# them. GitHub skips a job for exactly two reasons: its own job-level `if:`
# evaluated false, or something in its `needs:` did not succeed. A job that
# declares NEITHER has no reason to skip at all, so if it does, the workflow
# changed under the gate and the required check must not pass over it. Four of
# `report`'s six dependencies are in that class -- they carry no `if:` and no
# `needs:` -- and each of them was, until this change, one silent edit away from
# being skipped and graded `ok`.
#
# The permission is therefore READ FROM THE WORKFLOW rather than listed here: a
# dependency may conclude `skipped` only if its own definition in WORKFLOW_FILE
# declares a condition under which it may not run -- and, for a `needs:`
# cascade, only if something in that `needs:` actually did not succeed on this
# run. A declaration is corroborated, never taken on trust: "it declares
# something, so it is fine" is the same reasoning that made every skip
# invisible, one level down. That keeps the c1 property intact -- there is still
# no second list to drift, because the only list is the workflow's own job
# definitions.
#
# Inputs (env):
#   NEEDS_JSON     the caller's ${{ toJSON(needs) }} -- a JSON object keyed by job id
#   WORKFLOW_FILE  repo-relative path to the workflow that DEFINES those jobs,
#                  read to decide whether a skip was declared. REQUIRED even on
#                  runs where nothing skipped: a gate that only notices it cannot
#                  answer the question on the day the answer matters has failed
#                  open for every run before it.
#
# Exit 0 when every dependency concluded `success`, or `skipped` with a declared
# reason. Exit 1 otherwise -- including when NEEDS_JSON or WORKFLOW_FILE is
# absent, unreadable, unparseable or empty, because a gate that cannot see its
# dependencies must not pass over them.
set -euo pipefail

if [ -z "${NEEDS_JSON:-}" ]; then
  echo "::error title=report::NEEDS_JSON is empty. This gate must be given \${{ toJSON(needs) }}; without it it grades nothing."
  exit 1
fi

if [ -z "${WORKFLOW_FILE:-}" ]; then
  echo "::error title=report::WORKFLOW_FILE is empty. Without the workflow that defines these jobs this gate cannot tell an intended skip from an accidental one, so it fails closed."
  exit 1
fi

python3 - <<'PY'
import json
import os
import sys

try:
    import yaml
except ImportError:
    print(
        "::error title=report::PyYAML is not available, so this gate cannot read"
        " the workflow and cannot tell an intended skip from an accidental one."
        " Failing closed."
    )
    sys.exit(1)

raw = os.environ["NEEDS_JSON"]
try:
    needs = json.loads(raw)
except json.JSONDecodeError as exc:
    print(f"::error title=report::NEEDS_JSON is not JSON ({exc}). The gate cannot grade what it cannot read.")
    sys.exit(1)

if not isinstance(needs, dict) or not needs:
    print("::error title=report::this check depends on NO job, so passing it certifies nothing.")
    sys.exit(1)

wf_path = os.environ["WORKFLOW_FILE"]
try:
    with open(wf_path, encoding="utf-8") as handle:
        workflow = yaml.safe_load(handle)
except OSError as exc:
    print(
        f"::error title=report::cannot read WORKFLOW_FILE {wf_path!r} ({exc})."
        " A gate that cannot see the workflow cannot grade a skip."
    )
    sys.exit(1)
except yaml.YAMLError as exc:
    print(
        f"::error title=report::WORKFLOW_FILE {wf_path!r} does not parse as YAML"
        f" ({exc}). Failing closed."
    )
    sys.exit(1)

defs = workflow.get("jobs") if isinstance(workflow, dict) else None
if not isinstance(defs, dict) or not defs:
    print(
        f"::error title=report::WORKFLOW_FILE {wf_path!r} declares no jobs, so"
        " every skip below it would be unexplainable. Failing closed."
    )
    sys.exit(1)


def declared_skip_reason(job_id, results):
    """Whether this job was ALLOWED to be skipped, read from its own definition.

    GitHub skips a job for exactly two reasons -- its job-level `if:` was false,
    or something in its `needs:` did not succeed -- so those are the only two
    declarations that can excuse a skip. Returns a (verdict, detail) pair:

      "undefined"       not defined in WORKFLOW_FILE at all: the gate is reading
                        the wrong file, or the job was renamed out from under it
      "allowed"         a declaration excuses it, and `detail` names which
      "undeclared"      defined, but declares no reason it may not run
      "uncorroborated"  declares a `needs:` it could cascade from, but nothing
                        in that `needs:` actually failed or skipped on this run
    """
    spec = defs.get(job_id)
    if not isinstance(spec, dict):
        return ("undefined", None)
    if spec.get("if") is not None:
        return ("allowed", "job-level `if:`")
    upstream = spec.get("needs")
    if upstream:
        if isinstance(upstream, str):
            upstream = [upstream]
        # A CASCADE IS CORROBORATED, NOT TRUSTED. `needs:` says this job CAN
        # cascade, never that it did, and "it declares something, so it is
        # fine" is the exact reasoning that made every skip invisible in the
        # first place -- one level down. An upstream that is not among the
        # jobs this check grades cannot corroborate anything either: its
        # failure is invisible here, so it fails closed rather than getting
        # the benefit of the doubt.
        culprits = sorted(u for u in upstream
                          if u in results and results[u] != "success")
        if culprits:
            return ("allowed", "cascade from `needs:` " + ", ".join(culprits))
        return ("uncorroborated", ", ".join(sorted(upstream)))
    return ("undeclared", None)


# ANYTHING OTHER than success or an EXPLAINED skip fails closed, including a
# result this script does not recognise -- a conclusion string GitHub adds later
# must red the aggregate, not slip through an `else` branch that assumed it was
# fine.
results = {
    job: (needs[job] if isinstance(needs[job], dict) else {}).get("result", "<absent>")
    for job in needs
}

failed = False
for job in sorted(needs):
    result = results[job]
    if result == "success":
        print(f"  ok   {job:<24} {result}")
    elif result == "skipped":
        verdict, detail = declared_skip_reason(job, results)
        if verdict == "allowed":
            print(f"  ok   {job:<24} {result} (declared: {detail})")
        elif verdict == "undefined":
            print(
                f"::error title=report::dependency {job!r} concluded 'skipped'"
                f" and is not defined in {wf_path!r} -- the gate cannot tell an"
                " intended skip from an accidental one. Failing closed."
            )
            failed = True
        elif verdict == "uncorroborated":
            print(
                f"::error title=report::dependency {job!r} concluded 'skipped'"
                f" and could only cascade from {detail} -- but nothing there"
                " failed or skipped on this run, so the skip is unexplained and"
                " its cause is not among the jobs this check grades."
                " Failing closed."
            )
            failed = True
        else:
            print(
                f"::error title=report::dependency {job!r} concluded 'skipped'"
                " but its definition declares no condition under which it may"
                " not run -- no job-level `if:` and no `needs:`. A job with no"
                " declared reason to skip did not run, so the workflow changed"
                " under this gate. Failing closed."
            )
            failed = True
    elif result in ("failure", "cancelled"):
        print(
            f"::error title=report::dependency {job!r} concluded {result!r}"
            " -- the required check cannot pass over it."
        )
        failed = True
    else:
        print(
            f"::error title=report::dependency {job!r} reported an"
            f" uninterpretable result {result!r} -- failing closed."
        )
        failed = True

if failed:
    sys.exit(1)
print(f"OK: no dependency failed or was cancelled ({len(needs)} graded, none named in this file).")
PY
