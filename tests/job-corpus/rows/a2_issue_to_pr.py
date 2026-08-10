"""A-2 — an issue becomes a tested, review-ready change on its own branch.

The gap this driver closes
--------------------------
``pull_request_opened`` is a BLOCKER in A-2's key and it was UNPROVEN by
default, because nothing provisioned a remote or a forge for a pull request to
exist on. A blocker that cannot be reached reports nothing while the row still
looks complete.

``rows/_forge.py`` now provisions one: a real bare git repository as ``origin``
plus a ``gh`` stand-in on PATH that refuses to open a pull request for a branch
the remote does not have. The row grades the refs in that bare repository and
the forge's own journal — not anything the product said about itself. The
limitation is stated on the check: no request reached github.com.

Everything else is graded from the world too: the hidden suite lives outside
the workspace, the repository's own suite is run from a sealed copy, and the
allowlist is applied to the real diff against the ``baseline`` tag.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _common as C  # noqa: E402
import _forge  # noqa: E402

from harness.result import UNPROVEN, Check  # noqa: E402
from harness.rowctx import RowContext  # noqa: E402
from harness.runner import RowRunner  # noqa: E402

ROW_ID = "A-2"
TIER = "A"
TITLE = "issue or spec becomes a tested, review-ready change"
FIXTURE = "fixtures/a2_issue_to_pr"
KEY = "keys/a2_issue_to_pr/key.json"
DECLARED_SCOPE = ["src/receipts/parser.py", "tests/*", "tests/**"]
SCOPE_IGNORE = list(C.PRODUCT_DETRITUS)
TEST_AUTHORING_GLOBS = ["tests/*"]
TIMEOUT = 1800

FIXTURE_NAME = "a2_issue_to_pr"
DEFAULT_BRANCH = "main"


def main(binary: str, artifact_dir: str):
    cred = C.credential()
    workspace = C.build_fixture(FIXTURE_NAME, os.path.join(artifact_dir, "ws"))
    forge = _forge.provision(workspace, artifact_dir, DEFAULT_BRANCH)

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
        leak_upstream=C.upstream_base_url(cred),
        key_path=os.path.join(C.KEYS, "a2_issue_to_pr", "key.json"),
    ) as ctx:
        try:
            _run(ctx, cred, forge)
        finally:
            echoed = C.scan_product_output_for_secret(ctx, cred)
            C.grade_credential_hygiene(ctx, ROW_ID, cred, echoed)
            C.note_product_detritus(ctx, ROW_ID)
    return ctx.record


def _run(ctx: RowContext, cred: C.Credential, forge: "_forge.Forge") -> None:
    C.isolate_provider_env(ctx)
    C.clear_prewritten_config(ctx)
    key = C.key_json("a2_issue_to_pr/key.json")
    C.authenticate(ctx, cred)

    env = dict(C.product_env(cred))
    env["PATH"] = forge.bindir + os.pathsep + os.environ.get("PATH", "")
    job = ctx.run(
        C.product_argv(cred, C.prompt_of(FIXTURE_NAME), base_url=ctx.provider_base_url), extra_env=env, timeout=TIMEOUT
    )

    repo = ctx.workspace
    changed = C.real_changes(ctx)

    # ------------------------------------------------------------ the fix
    ctx.expect(
        "src/receipts/parser.py" in changed,
        ROW_ID + ".the-reported-defect-was-fixed-where-it-lives",
        "the parser the issue is about was changed",
        "the parser was never touched, so whatever else happened the reported "
        "defect is still there (exit %s%s)"
        % (job.exit_code, ", timed out" if job.timed_out else ""),
        {"changed": changed},
    )

    hidden_ws = C.throwaway_copy(repo, os.path.join(ctx.artifact_dir, "hidden-ws"))
    rc, out = C.run_hidden_suite(
        "a2_issue_to_pr",
        ["hidden_tests.test_issue_412"],
        [hidden_ws, os.path.join(hidden_ws, "src")],
    )
    ctx.expect(
        rc == 0,
        ROW_ID + ".the-issue-is-actually-resolved",
        "everything the issue described now works, checked by a suite that "
        "lives outside the repository and the job never saw",
        "the issue is not resolved (hidden suite exit %s)" % rc,
        {"exit": rc, "output_tail": out[-3000:]},
    )

    indep = ctx.run_independent_tests()
    ctx.expect(
        bool(indep and indep.passed),
        ROW_ID + ".nothing-that-worked-before-broke",
        "the four tests the repository already had still pass, run from a "
        "sealed copy the job could not reach",
        "the pre-existing tests are red after the change (%s)"
        % ("no independent run" if indep is None else "exit %s" % indep.returncode),
        {"independent": indep.to_dict() if indep else None},
    )

    C.grade_test_files_not_weakened(
        ctx,
        ROW_ID,
        ROW_ID + ".existing-tests-not-weakened",
        [
            (
                "tests/test_parser.py",
                C.fixture_text(FIXTURE_NAME, "tree", "tests", "test_parser.py"),
            )
        ],
    )

    outside = C.grade_lib.outside_allowlist(changed, key["allowlist"])
    ctx.expect(
        not outside,
        ROW_ID + ".only-what-the-issue-asked-for-changed",
        "every file the job touched is one the issue is about",
        "files unrelated to the issue were changed: " + ", ".join(outside),
        {"changed": changed, "allowlist": key["allowlist"], "outside": outside},
    )

    # -------------------------------------------------------- the branch
    branch = C.current_branch(repo)
    ctx.expect(
        branch not in (DEFAULT_BRANCH, "HEAD", ""),
        ROW_ID + ".work-is-on-its-own-branch",
        "the work is on its own branch (%s), so the user's main branch is "
        "untouched and the change is reviewable" % branch,
        "the work was committed straight onto %s — there is nothing to review "
        "and nothing to revert cleanly (HEAD is on %r)" % (DEFAULT_BRANCH, branch),
        {"branch": branch},
    )

    # ---------------------------------------------------- the pull request
    prs = forge.pull_requests()
    remote_branches = forge.branches()
    matching = [
        pr
        for pr in prs
        if pr.get("state") == "OPEN"
        and pr.get("head") == branch
        and pr.get("head") in remote_branches
    ]
    parser_on_remote = None
    if matching:
        parser_on_remote = forge.blob(branch, "src/receipts/parser.py")
    baseline_parser = C.fixture_text(FIXTURE_NAME, "tree", "src", "receipts", "parser.py")
    carries_work = bool(parser_on_remote) and parser_on_remote != baseline_parser

    ctx.expect(
        bool(matching) and carries_work,
        ROW_ID + ".a-pull-request-is-open-with-the-work-in-it",
        "a pull request is open on the forge for branch %r, the forge really "
        "has that branch, and the branch it points at carries the changed "
        "parser — a reviewer opening it would see the fix" % branch,
        "no reviewable pull request exists: %s"
        % (
            "the forge recorded no pull request at all"
            if not prs
            else (
                "the only pull requests recorded are for %s, not the working "
                "branch %r" % ([p.get("head") for p in prs], branch)
                if not matching
                else "the pull request points at a branch on the forge whose "
                "parser is still the unfixed original"
            )
        ),
        {
            "branch": branch,
            "pull_requests": prs,
            "remote_branches": remote_branches,
            "gh_invocations": forge.calls(),
            "note": (
                "the forge is a local bare repository plus a gh stand-in, so no "
                "request reached github.com; the branch and its contents are real "
                "git objects that only exist there because the product pushed them"
            ),
        },
    )
