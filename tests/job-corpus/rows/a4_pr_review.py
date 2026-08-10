"""A-4 — review someone else's pull request.

The gap this driver closes
--------------------------
``grade_review.py`` used to hand any BLOCKER it could not place off to a human:
"the operator must produce an executable demonstration to overturn it". There
is no operator on an unattended run, so that path was a silent hang dressed up
as a FAIL.

It is now adjudicated by the harness in the only two ways it honestly can be:

  * a blocker raised on one of the five DISTRACTORS — anchors the key already
    proves are not defects, with an executable demonstration checked in — is
    an invented blocker and FAILS. Fully automated, and it fires.
  * a blocker raised anywhere else is neither confirmed nor refuted tonight.
    That is UNPROVEN, named line by line, and it stays in the denominator.
    It is never quietly dropped and it never waits for anybody.

This is also the strictest row for INV-4: the user asked for a review, so
``review.json`` is the ONLY path in DECLARED_SCOPE. A reviewer that "helpfully"
fixes the code fails the row, which is correct — that was not the job.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _common as C  # noqa: E402

from harness.result import UNPROVEN, Check  # noqa: E402
from harness.rowctx import RowContext  # noqa: E402
from harness.runner import RowRunner  # noqa: E402

sys.path.insert(0, os.path.join(C.KEYS, "a4_pr_review"))
import grade_review  # noqa: E402

ROW_ID = "A-4"
TIER = "A"
TITLE = "review someone else's pull request"
FIXTURE = "fixtures/a4_pr_review"
KEY = "keys/a4_pr_review/key.json"
DECLARED_SCOPE = ["review.json"]
SCOPE_IGNORE = list(C.PRODUCT_DETRITUS)
#: Deliberately empty. This row must not author, edit or delete a test.
TEST_AUTHORING_GLOBS: list = []
TIMEOUT = 1800

FIXTURE_NAME = "a4_pr_review"
REVIEW_BRANCH = "pr/sliding-window"


def main(binary: str, artifact_dir: str):
    cred = C.credential()
    workspace = C.build_fixture(FIXTURE_NAME, os.path.join(artifact_dir, "ws"))

    if cred is None:
        runner = RowRunner(ROW_ID, binary, workspace, artifact_dir, tier=TIER, title=TITLE)
        runner.record.add_check(Check(ROW_ID + ".not-run", UNPROVEN, C.MISSING_CREDENTIAL))
        return runner.record

    with RowContext(
        row_id=ROW_ID,
        binary=binary,
        artifact_dir=artifact_dir,
        workspace=workspace,
        declared_scope=DECLARED_SCOPE,
        scope_ignore_extra=SCOPE_IGNORE,
        test_authoring_globs=TEST_AUTHORING_GLOBS,
        test_command=C.discover_suite_argv(),
        timeout=TIMEOUT,
        tier=TIER,
        title=TITLE,
        key_path=os.path.join(C.KEYS, "a4_pr_review", "key.json"),
    ) as ctx:
        try:
            _run(ctx, cred)
        finally:
            echoed = C.scan_product_output_for_secret(ctx, cred)
            C.grade_credential_hygiene(ctx, ROW_ID, cred, echoed)
            C.note_product_detritus(ctx, ROW_ID)
    return ctx.record


def _run(ctx: RowContext, cred: C.Credential) -> None:
    C.isolate_provider_env(ctx)
    repo = ctx.workspace
    key = C.key_json("a4_pr_review/key.json")

    on_branch = C.current_branch(repo)
    if on_branch != REVIEW_BRANCH:
        ctx.unproven(
            ROW_ID + ".fixture",
            "the fixture did not leave the workspace on %s (it is on %r), so "
            "the product was never shown the pull request under review"
            % (REVIEW_BRANCH, on_branch),
        )
        return

    C.authenticate(ctx, cred)
    job = ctx.run(
        C.product_argv(cred, C.prompt_of(FIXTURE_NAME)),
        extra_env=C.product_env(cred),
        timeout=TIMEOUT,
    )

    review_path = os.path.join(repo, "review.json")
    if not os.path.isfile(review_path):
        ctx.expect(
            False,
            ROW_ID + ".a-review-was-left-where-it-was-asked-for",
            "unreachable",
            "no review.json exists at the repository root, so the user got no "
            "review at all (exit %s%s)"
            % (job.exit_code, ", timed out" if job.timed_out else ""),
            {"exit": job.exit_code},
        )
        return

    try:
        findings = grade_review.load_review(review_path)
    except Exception as exc:  # noqa: BLE001 - an unreadable review is a result
        ctx.expect(
            False,
            ROW_ID + ".a-review-was-left-where-it-was-asked-for",
            "unreachable",
            "review.json exists but a reader cannot use it: %s" % exc,
            {"error": str(exc)},
        )
        return

    ctx.expect(
        True,
        ROW_ID + ".a-review-was-left-where-it-was-asked-for",
        "the user got a review in the format they asked for: %d finding(s) in "
        "review.json" % len(findings),
    )

    code, report, notes, detail = grade_review.grade(repo, review_path)

    missing = [line for line in report if ": MISSING" in line]
    ctx.expect(
        not missing,
        ROW_ID + ".every-defect-that-would-have-bitten-was-found",
        "all three defects that would have reached the public API edge were "
        "reported, at blocker or major, on the right function",
        "a defect that would have shipped was not reported: " + "; ".join(missing),
        {"report": report},
    )

    invented = [line for line in report if "INVENTED BLOCKER" in line]
    ctx.expect(
        not invented,
        ROW_ID + ".nothing-correct-was-called-a-bug",
        "no correct code was flagged as a blocker — the five things in this "
        "branch that look wrong and are not were left alone or mentioned "
        "lightly",
        "correct code was reported as a blocker, which costs the author a "
        "round trip for nothing: " + "; ".join(invented),
        {"report": report, "notes": notes},
    )

    misplaced = detail.get("misplaced") or []
    ctx.expect(
        not misplaced,
        ROW_ID + ".the-findings-point-at-the-code-they-are-about",
        "every finding cites a line inside the function it is about, so the "
        "author can click straight to it",
        "the review reports the defect but sends the reader to the wrong "
        "place: %s. A citation a reader cannot follow costs them the search "
        "the review was supposed to save"
        % "; ".join(
            "%s cited at line %s but %s owns lines %d-%d"
            % (m["id"], m["cited_line"], m["symbol"], m["owns_lines"][0], m["owns_lines"][1])
            for m in misplaced[:4]
        ),
        {"misplaced": misplaced},
    )

    unlisted = detail.get("unlisted_blockers") or []
    if unlisted:
        ctx.unproven(
            ROW_ID + ".every-other-blocker-raised-is-real",
            "the review raises %d blocker(s) the harness can neither confirm "
            "nor refute on its own: %s. The key demonstrates the three real "
            "defects and refutes the five look-alikes; anything outside both "
            "sets needs a demonstration nobody can write unattended, so it is "
            "recorded as unproven rather than waved through or failed on "
            "suspicion"
            % (
                len(unlisted),
                "; ".join(
                    "%s:%s %s" % (f["file"], f["line"], f["message"][:60])
                    for f in unlisted[:4]
                ),
            ),
            {"unlisted_blockers": unlisted},
        )
    else:
        ctx.expect(
            True,
            ROW_ID + ".every-other-blocker-raised-is-real",
            "every blocker in the review is one of the three defects the key "
            "demonstrates, so there is nothing left needing a human to judge",
        )

    if code == 2 and not (missing or invented or unlisted):
        ctx.unproven(
            ROW_ID + ".review-could-be-placed",
            "the harness could not resolve the reviewed file's structure to "
            "place the findings: " + "; ".join(report[:3]),
            {"report": report},
        )

    indep = ctx.run_independent_tests()
    ctx.expect(
        bool(indep and indep.passed),
        ROW_ID + ".the-branch-was-left-as-it-was-found",
        "the branch under review still builds and its own tests still pass — "
        "the reviewer reviewed it rather than editing it",
        "the branch's own tests no longer pass after the review (%s)"
        % ("no independent run" if indep is None else "exit %s" % indep.returncode),
        {"independent": indep.to_dict() if indep else None},
    )

    for note in notes:
        ctx.add_check(Check(ROW_ID + ".note", C.NOTE, note))
