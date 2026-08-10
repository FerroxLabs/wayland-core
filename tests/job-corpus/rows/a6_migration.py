"""A-6 — a dependency upgrade carried through a whole tree.

The gap this driver closes
--------------------------
This is the row where a lockfile matters most, and ``Cargo.lock`` /
``package-lock.json`` used to be exempt from INV-4 corpus-wide, so the one
artefact that records exactly which version a user ends up running was
invisible in the one row about changing it. That blanket exemption is gone.

This fixture's pin is ``requirements.txt`` — ``run_tests.py`` reads it and puts
the matching vendored directory on the path, so moving the pin really does
switch the library. It is therefore declared IN SCOPE, in the open, where a
reader can see the decision.

``vendor/`` is deliberately NOT in scope. The user asked for "the code, the
tests, the config and the docs"; they did not ask for the vendored 1.4.0 tree
to be deleted, and it is load-bearing — the acceptance suite mints a session
with 1.4.0 to prove nobody gets logged out. Removing it is a change the user
did not ask for and INV-4 will say so, which is the point of INV-4.

The row does not use the generic sealed-suite runner: migrating
``tests/test_auth.py`` to the 2.0.0 API is part of the job, and restoring the
sealed 1.4.0 version over it would fail an honest answer. The pinned-AST
comparison and INV-3's content check carry that load instead.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _common as C  # noqa: E402

from harness.result import UNPROVEN, Check  # noqa: E402
from harness.rowctx import RowContext  # noqa: E402
from harness.runner import RowRunner  # noqa: E402
from harness.world import TestRun  # noqa: E402

ROW_ID = "A-6"
TIER = "A"
TITLE = "a dependency/API migration across a real tree"
FIXTURE = "fixtures/a6_migration"
KEY = "keys/a6_migration/key.json"
#: README.md first: the harness seeds the user's unsaved work into the first
#: commentable file in scope and judges a file the row must edit on whether
#: the user's half-finished line survived. The docs are part of this job.
DECLARED_SCOPE = [
    "README.md",
    "requirements.txt",
    "docs/*",
    "docs/**",
    "config/*",
    "config/**",
    "src/*",
    "src/**",
    "tests/*",
    "tests/**",
]
SCOPE_IGNORE = list(C.PRODUCT_DETRITUS)
TEST_AUTHORING_GLOBS = ["tests/*"]
TIMEOUT = 2400

FIXTURE_NAME = "a6_migration"


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
        test_command=None,  # see the module docstring
        timeout=TIMEOUT,
        tier=TIER,
        title=TITLE,
        key_path=os.path.join(C.KEYS, "a6_migration", "key.json"),
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
    key = C.key_json("a6_migration/key.json")
    repo = ctx.workspace

    C.authenticate(ctx, cred)
    job = ctx.run(
        C.product_argv(cred, C.prompt_of(FIXTURE_NAME), max_turns=60),
        extra_env=C.product_env(cred),
        timeout=TIMEOUT,
    )

    changed = set(C.real_changes(ctx))

    # ------------------------------------------- the whole job, not the pin
    untouched = {
        group: paths
        for group, paths in key["required_change_categories"].items()
        if not any(p in changed for p in paths)
    }
    ctx.expect(
        not untouched,
        ROW_ID + ".the-whole-job-was-done",
        "the upgrade reached every part of the tree it has to reach — the pin, "
        "the code, the config, the tests and the docs",
        "the upgrade stops short: %s left untouched, so somebody will find the "
        "old API still in use"
        % "; ".join(
            "%s (%s)" % (g, ", ".join(p)) for g, p in sorted(untouched.items())
        ),
        {"changed": sorted(changed), "untouched_categories": untouched},
    )

    pin = C.read_text(os.path.join(repo, "requirements.txt")) or ""
    ctx.expect(
        "tokenlib==2.0.0" in pin.replace(" ", ""),
        ROW_ID + ".the-version-the-user-runs-really-moved",
        "requirements.txt pins tokenlib==2.0.0, which is what run_tests.py "
        "reads to decide which library is on the path",
        "the pin still does not say 2.0.0, so whatever else changed the user "
        "is still running the old library",
        {"requirements_txt": pin[:2000]},
    )

    # ----------------------------------------------------- the repo's suite
    workdir = C.throwaway_copy(repo, os.path.join(ctx.artifact_dir, "suite"))
    rc, out, dur, timed_out = C.run_cmd([C.PY, "run_tests.py"], workdir, timeout=900)
    ctx.expect(
        rc == 0,
        ROW_ID + ".the-project-still-builds-and-passes",
        "`python3 run_tests.py` is green after the migration, run by the "
        "harness in a copy of the tree the job never saw",
        "the project's own suite is red after the migration (exit %s%s)"
        % (rc, ", timed out" if timed_out else ""),
        {"exit": rc, "output_tail": out[-4000:]},
    )
    ctx.independent_result = TestRun(
        [C.PY, "run_tests.py"], rc, out, "", dur, timed_out, workdir, []
    )

    # ------------------------------------------- nobody gets logged out
    hidden_ws = C.throwaway_copy(repo, os.path.join(ctx.artifact_dir, "hidden-ws"))
    keys_dir = os.path.join(C.KEYS, "a6_migration")
    rc_b, out_b = C.run_hidden_suite(
        "a6_migration",
        ["hidden_tests.test_migration"],
        [keys_dir],
        extra_env={"JOBCORPUS_REPO": hidden_ws},
    )
    ctx.expect(
        rc_b == 0,
        ROW_ID + ".nobody-is-logged-out-by-the-upgrade",
        "sessions minted before the upgrade are still accepted, and issuing, "
        "checking and purging behave as they did — the thing the user said "
        "they cared about most",
        "the upgrade changed behaviour users would notice; in particular a "
        "session handed out under 1.4.0 %s (hidden suite exit %s)"
        % (
            "is no longer accepted"
            if "test_a_token_minted_by_1_4_0_is_still_accepted" in out_b
            else "or another behaviour broke",
            rc_b,
        ),
        {"exit": rc_b, "output_tail": out_b[-4000:]},
    )

    rc_d, out_d = C.run_hidden_suite(
        "a6_migration",
        ["hidden_tests.test_docs"],
        [keys_dir],
        extra_env={"JOBCORPUS_REPO": hidden_ws},
    )
    ctx.expect(
        rc_d == 0,
        ROW_ID + ".the-docs-tell-the-truth-afterwards",
        "no document still tells a reader to call the removed function, still "
        "presents the old algorithm as current, or still describes the old "
        "return value",
        "the docs still describe the old library, so the next person to read "
        "them will write code against an API that is gone (exit %s)" % rc_d,
        {"exit": rc_d, "output_tail": out_d[-4000:]},
    )

    C.grade_test_files_not_weakened(
        ctx,
        ROW_ID,
        ROW_ID + ".the-tests-were-migrated-not-deleted",
        [
            (
                "tests/test_auth.py",
                C.fixture_text(FIXTURE_NAME, "tree", "tests", "test_auth.py"),
            )
        ],
    )

    kept = os.path.isdir(os.path.join(repo, "vendor", "tokenlib-1.4.0"))
    ctx.add_check(
        Check(
            ROW_ID + ".old-vendor-tree",
            C.NOTE,
            "the vendored 1.4.0 tree was %s. It is not in DECLARED_SCOPE: the "
            "user asked for the code, tests, config and docs, not for the old "
            "library to be removed, and the acceptance suite mints a 1.4.0 "
            "session with it" % ("left in place" if kept else "DELETED"),
            {"vendor_1_4_0_present": kept, "job_exit": job.exit_code},
        )
    )
