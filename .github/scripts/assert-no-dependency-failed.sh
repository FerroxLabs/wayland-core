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
# Inputs (env):
#   NEEDS_JSON  the caller's ${{ toJSON(needs) }} -- a JSON object keyed by job id
#
# Exit 0 when every dependency concluded `success` or `skipped`, 1 otherwise --
# including when NEEDS_JSON is absent, unparseable or empty, because a gate
# that cannot see its dependencies must not pass over them.
set -euo pipefail

if [ -z "${NEEDS_JSON:-}" ]; then
  echo "::error title=report::NEEDS_JSON is empty. This gate must be given \${{ toJSON(needs) }}; without it it grades nothing."
  exit 1
fi

python3 - <<'PY'
import json
import os
import sys

raw = os.environ["NEEDS_JSON"]
try:
    needs = json.loads(raw)
except json.JSONDecodeError as exc:
    print(f"::error title=report::NEEDS_JSON is not JSON ({exc}). The gate cannot grade what it cannot read.")
    sys.exit(1)

if not isinstance(needs, dict) or not needs:
    print("::error title=report::this check depends on NO job, so passing it certifies nothing.")
    sys.exit(1)

# SKIPPED IS DELIBERATELY NOT A FAILURE: the macOS/Windows legs are rationed by
# design and a skipped leg contributes no red. Anything OTHER than success or
# skipped fails closed, including a result this script does not recognise -- a
# conclusion string GitHub adds later must red the aggregate, not slip through
# an `else` branch that assumed it was fine.
failed = False
for job in sorted(needs):
    entry = needs[job] if isinstance(needs[job], dict) else {}
    result = entry.get("result", "<absent>")
    if result in ("success", "skipped"):
        print(f"  ok   {job:<24} {result}")
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
