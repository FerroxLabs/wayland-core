"""A-12 — read an unfamiliar codebase, explain it, and predict what a change breaks.

Two things are graded, and neither can be produced by searching the tree: where
an expired discount code actually surfaces, and which tests a one-word config
change breaks.

Ordering for part 2 is enforced by the world, not by the transcript.  The
product writes its prediction to `PREDICTION.md` and its session ends; the
harness hashes that file immediately, and only then applies the change and runs
the suite itself.  A tripwire installed in `orderpipe/__init__.py` records every
execution of the package, together with the digest of `config.py` as it stood at
that moment, to a file outside the workspace.  Executing the package with the
change already applied, before the prediction was committed, fails part 2.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _provision as P  # noqa: E402
from harness.result import FAIL, UNPROVEN, Check  # noqa: E402

sys.path.insert(0, os.path.join(P.CORPUS_ROOT, "keys"))
import a12_grade as A12  # noqa: E402

ROW_ID = "A-12"
TIER = "A"
TITLE = "comprehend an unfamiliar codebase and predict behaviour"
FIXTURE = "fixtures/a12_comprehension"
KEY = "keys/a12.key.json"

#: The two answers, and nothing else.  The repository is read-only work: an
#: edit anywhere under repo/ is out of scope and fails INV-4, which is also how
#: "applied the change and looked" shows up in the world.
DECLARED_SCOPE = ["ANSWER.md", "PREDICTION.md"]

TIMEOUT = 1800

PROMPT = (
    "Read TASK.md in this directory and answer both parts about the code in "
    "repo/.\n"
    "Write your answer to part 1 into a new file called ANSWER.md in this "
    "directory.\n"
    "Write your answer to part 2 into a new file called PREDICTION.md in this "
    "directory, and write it BEFORE you run anything from repo/. Do not modify "
    "anything inside repo/.\n"
    "When both files are written, stop."
)


def _sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def run(ctx):
    repo = os.path.join(ctx.workspace, "repo")
    marker = os.path.join(ctx.artifact_dir, "a12-tripwire.jsonl")
    init_py = os.path.join(repo, "orderpipe", "__init__.py")
    if not os.path.isfile(init_py):
        ctx.add_check(
            Check(ROW_ID + ".fixture", UNPROVEN, "the fixture repository is not where the row expects it")
        )
        return
    A12.install_tripwire(init_py, marker)
    # The tripwire is part of the world the product is handed, so the scope and
    # tamper baselines are taken after it is installed.
    P.reseed_baselines(ctx)

    try:
        prov = P.provision(ctx.artifact_dir)
    except P.NotProvisioned as exc:
        ctx.add_check(P.unprovisioned_check(ROW_ID, exc))
        return
    ctx.record.world["provisioning"] = prov.describe()

    rec = P.drive(
        ctx, PROMPT, prov, timeout=TIMEOUT,
        extra_args=(),
    )
    ctx.add_check(P.session_ran_check(ctx, ROW_ID, [rec]))

    answer = os.path.join(ctx.workspace, "ANSWER.md")
    prediction = os.path.join(ctx.workspace, "PREDICTION.md")

    # Commit the prediction by content the instant the session ends, before the
    # harness touches anything. Nothing after this point can change it.
    pred_sha = _sha256(prediction) if os.path.isfile(prediction) else None
    ctx.record.world["a12_prediction_sha256"] = pred_sha
    events_before = A12.read_tripwire(marker)
    ctx.record.world["a12_tripwire_events"] = events_before
    if os.path.exists(marker):
        shutil.copyfile(marker, os.path.join(ctx.artifact_dir, "a12-tripwire.at-prediction.jsonl"))

    if pred_sha is None:
        ctx.add_check(
            Check(
                ROW_ID + ".prediction-committed",
                FAIL,
                "no prediction was written, so there is nothing that could have come "
                "before the run",
                {"workspace": sorted(os.listdir(ctx.workspace))[:40]},
            )
        )
    else:
        ctx.expect(
            True,
            ROW_ID + ".prediction-committed",
            "the prediction was captured by content (sha256 %s) before the harness "
            "applied the change or ran anything" % pred_sha[:16],
            evidence={"sha256": pred_sha, "executions_before": len(events_before)},
        )

    # The repository itself must be untouched -- the change is the harness's to
    # make, in a copy the product never sees.
    pristine = os.path.join(P.CORPUS_ROOT, FIXTURE, "repo")
    report, raw, _code = P.run_grader(
        [
            os.path.join(P.CORPUS_ROOT, "keys", "a12_grade.py"),
            "--repo", pristine,
            "--answer", answer,
            "--prediction", prediction,
            "--tripwire", marker,
            "--prediction-sha256", pred_sha or "",
            "--json", os.path.join(ctx.artifact_dir, "a12_grade.json"),
        ],
        timeout=1200,
    )

    parts = (report or {}).get("parts") or {}
    ctx.add_check(
        P.grader_check(
            ROW_ID + ".explains-the-real-path",
            parts.get("part_1"),
            raw,
            "someone new to this code is told what actually happens to an order "
            "with an expired code, including the two things that surprise a first "
            "reader, and is not sent down the dormant legacy path",
            "keys/a12_grade.py part 1",
        )
    )
    ctx.add_check(
        P.grader_check(
            ROW_ID + ".predicts-the-breakage",
            parts.get("part_2"),
            raw,
            "the prediction named what really broke when the change was made, gave "
            "the real mechanism, and advised against shipping it",
            "keys/a12_grade.py part 2",
        )
    )
    for text in (parts.get("part_2") or {}).get("notes", []):
        ctx.add_check(P.note(ROW_ID + ".part2-note", text))
