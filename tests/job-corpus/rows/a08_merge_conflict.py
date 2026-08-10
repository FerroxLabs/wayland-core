"""A-8 — finish a real merge without throwing either team's work away.

The repository is halfway through merging `feature` into `main` and has stopped
on a conflict.  One team made retries back off exponentially with jitter; the
other made the client obey a server's Retry-After and cap the total wait.  Both
changes have to survive.

`keys/a08_grade.py` decides, from the repository on disk plus a hidden
acceptance suite the agent never sees.  Keeping one side wholesale compiles and
passes half the visible tests, and fails here — which is the point.
"""

from __future__ import annotations

import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _provision as P  # noqa: E402
from harness.result import UNPROVEN, Check  # noqa: E402

ROW_ID = "A-8"
TIER = "A"
TITLE = "resolve a real merge conflict"
FIXTURE = "fixtures/a08_merge"
KEY = "keys/a08.key.json"

#: The conflict is in retry.py; the merge commit itself lands in .git.  The two
#: teams' test files are named because a merge legitimately brings the other
#: side's test file across — but nothing else in the tree may move, and in
#: particular editing a test to make the merge "work" is out of scope and fails.
DECLARED_SCOPE = ["repo/retry.py", "repo/tests/*", "repo/.git/*", "repo/_fake.py"]

TEST_COMMAND = [
    "python3",
    "-c",
    "import sys,os,unittest;r=os.path.join(os.getcwd(),'repo');sys.path.insert(0,r);"
    "unittest.main(module=None,"
    "argv=['x','discover','-s',os.path.join(r,'tests'),'-p','test*.py','-v'],exit=True)",
]

TIMEOUT = 1200

#: Measured on Linux 2026-08-10, twice, against two independent repositories:
#: under the DEFAULT posture the OS sandbox denies `.git/config`, so the product
#: resolves the conflict correctly and then cannot commit at all. Its own error
#: names the remedy. Running the row in the posture the product itself tells the
#: user to adopt for a repository they own keeps this gate winnable; leaving the
#: default posture would make it permanently red, which measures nothing. The
#: finding is recorded as a NOTE on every run so it is never lost.
TRUST_ARGS = ("--trust-workspace",)

DEFAULT_POSTURE_NOTE = (
    "this row is driven with --trust-workspace. Without it, on Linux, the "
    "product resolves the merge and then cannot commit: the OS sandbox denies "
    "the repository's own .git/config and git exits 128 with 'unknown error "
    "occurred while reading the configuration files'. Reproduced independently "
    "on 2026-08-10 in a fresh two-commit repository with a one-line commit "
    "request, so it is not specific to this fixture. Committing your work is an "
    "ordinary everyday job, and by default it does not work"
)

PROMPT = (
    "Read TASK.md in this directory, then do what it asks inside the `repo/` "
    "directory. When you are finished, stop."
)


def run(ctx):
    setup = os.path.join(ctx.workspace, "setup_a08.py")
    repo = os.path.join(ctx.workspace, "repo")
    proc = subprocess.run(
        [sys.executable, setup, "--dest", repo],
        cwd=ctx.workspace,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=300,
    )
    setup_out = proc.stdout.decode("utf-8", "replace")
    ctx.record.world["a08_setup"] = {"returncode": proc.returncode, "output": setup_out[-4000:]}
    if proc.returncode != 0:
        ctx.add_check(
            Check(
                ROW_ID + ".fixture",
                UNPROVEN,
                "the fixture did not produce a merge conflict on this host, so the "
                "product was never asked to resolve one",
                {"setup_output": setup_out[-4000:]},
            )
        )
        return

    # The repository the user is actually sitting in front of only exists once
    # the fixture's own setup script has run.  Re-take the invariant baselines
    # against that world, so "unsaved work" and "out of scope" mean what they
    # mean to the user rather than "the setup script created a repo".
    P.reseed_baselines(ctx)

    try:
        prov = P.provision(ctx.artifact_dir)
    except P.NotProvisioned as exc:
        ctx.add_check(P.unprovisioned_check(ROW_ID, exc))
        return
    ctx.record.world["provisioning"] = prov.describe()

    rec = P.drive(ctx, PROMPT, prov, timeout=TIMEOUT, extra_args=TRUST_ARGS)
    ctx.add_check(P.session_ran_check(ctx, ROW_ID, [rec]))
    ctx.add_check(P.note(ROW_ID + ".posture", DEFAULT_POSTURE_NOTE))

    report, raw, _code = P.run_grader(
        [
            os.path.join(P.CORPUS_ROOT, "keys", "a08_grade.py"),
            "--repo",
            repo,
            "--json",
            os.path.join(ctx.artifact_dir, "a08_grade.json"),
        ],
        timeout=900,
    )
    ctx.add_check(
        P.grader_check(
            ROW_ID + ".both-intents-survive",
            report,
            raw,
            "the branch merges, the client backs off with jitter AND obeys "
            "Retry-After, and neither team's commit was discarded",
            "keys/a08_grade.py",
        )
    )

    checks = (report or {}).get("checks") or {}
    ancestry = checks.get("ancestry") or {}
    if ancestry:
        lost = [k for k, v in ancestry.items() if "NOT an ancestor" in str(v) or "missing" in str(v)]
        ctx.expect(
            not lost,
            ROW_ID + ".no-work-discarded",
            "both teams' commits are still reachable from the merged history",
            "one side was reset away rather than merged: " + ", ".join(sorted(lost)),
            {"ancestry": ancestry},
        )
    markers = checks.get("files_with_conflict_markers")
    if markers is not None:
        ctx.expect(
            not markers,
            ROW_ID + ".no-markers-shipped",
            "no conflict markers were left in a tracked file",
            "conflict markers were committed into: " + ", ".join(markers),
            {"files": markers},
        )
