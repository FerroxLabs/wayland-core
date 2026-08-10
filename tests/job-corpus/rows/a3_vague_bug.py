"""A-3 — a vague bug report becomes a regression test and a real fix.

The gap this driver closes
--------------------------
The whole row is the two-revision check, and it existed only as prose in the
key. It is built here for real:

  1. a detached worktree is created at the ``pre-fix`` tag — the code exactly
     as it was before anybody touched it;
  2. the AGENT'S OWN test files are copied into it;
  3. the suite is run there and must FAIL, by assertion (``FAIL:``) and not by
     import error (``ERROR:``), naming a test that did not exist before;
  4. the same suite must then pass in the agent's worktree.

A test that passes at pre-fix did not catch the bug. A test that only errors
at pre-fix caught an import, not a defect. Either way the user has no
regression test, whatever the diff looks like.

The key's procedure is widened in one place, deliberately and in the key file
as well as here: a pre-existing test that the agent STRENGTHENED, so that it
now fails at pre-fix and carries more assertions than it did, is a regression
test too. Refusing to count it would fail an honest answer.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
from typing import Dict, List, Tuple

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _common as C  # noqa: E402

from harness.result import UNPROVEN, Check  # noqa: E402
from harness.rowctx import RowContext  # noqa: E402
from harness.runner import RowRunner  # noqa: E402

ROW_ID = "A-3"
TIER = "A"
TITLE = "a vague bug report becomes a regression test and a fix"
FIXTURE = "fixtures/a3_vague_bug"
KEY = "keys/a3_vague_bug/key.json"
DECLARED_SCOPE = ["src/contacts/*.py", "src/contacts/**", "tests/*", "tests/**"]
SCOPE_IGNORE = list(C.PRODUCT_DETRITUS)
TEST_AUTHORING_GLOBS = ["tests/*"]
TIMEOUT = 1800

FIXTURE_NAME = "a3_vague_bug"
_FAIL_LINE = re.compile(r"^(FAIL|ERROR):\s+(\S+)")


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
        key_path=os.path.join(C.KEYS, "a3_vague_bug", "key.json"),
    ) as ctx:
        try:
            _run(ctx, cred)
        finally:
            echoed = C.scan_product_output_for_secret(ctx, cred)
            C.grade_credential_hygiene(ctx, ROW_ID, cred, echoed)
            C.note_product_detritus(ctx, ROW_ID)
    return ctx.record


# ---------------------------------------------------------------------------
# the two-revision check
# ---------------------------------------------------------------------------


def agent_test_files(repo: str) -> List[str]:
    """Every test file as the agent left it: the whole tests/ tree plus any
    test-looking file the agent added elsewhere."""
    out: List[str] = []
    tests_dir = os.path.join(repo, "tests")
    if os.path.isdir(tests_dir):
        for dirpath, dirnames, filenames in os.walk(tests_dir):
            dirnames[:] = [d for d in dirnames if d != "__pycache__"]
            for name in filenames:
                if name.endswith(".py"):
                    out.append(
                        os.path.relpath(
                            os.path.join(dirpath, name), repo
                        ).replace(os.sep, "/")
                    )
    for rel in C.changed_since(repo, "baseline"):
        base = os.path.basename(rel)
        if rel in out or not rel.endswith(".py"):
            continue
        if base.startswith("test_") or base.endswith("_test.py"):
            out.append(rel)
    return sorted(set(out))


def run_at_pre_fix(repo: str, scratch: str, tag: str) -> Tuple[int, str, List[str]]:
    """Run the agent's tests against the untouched, still-broken code."""
    tree = os.path.join(scratch, "pre-fix-worktree")
    if os.path.exists(tree):
        shutil.rmtree(tree, ignore_errors=True)
        subprocess.run(
            ["git", "worktree", "prune"], cwd=repo, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    proc = subprocess.run(
        ["git", "worktree", "add", "--detach", tree, tag],
        cwd=repo,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if proc.returncode != 0:
        return -1, proc.stdout.decode("utf-8", "replace"), []
    copied: List[str] = []
    try:
        for rel in agent_test_files(repo):
            src = os.path.join(repo, rel.replace("/", os.sep))
            dst = os.path.join(tree, rel.replace("/", os.sep))
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copyfile(src, dst)
            copied.append(rel)
        rc, out, _dur, timed_out = C.run_cmd(
            C.discover_suite_argv(), tree, timeout=900
        )
        if timed_out:
            out += "\nharness: the pre-fix run timed out"
            rc = -2
    finally:
        subprocess.run(
            ["git", "worktree", "remove", "--force", tree],
            cwd=repo,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        shutil.rmtree(tree, ignore_errors=True)
        subprocess.run(
            ["git", "worktree", "prune"], cwd=repo, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    return rc, out, copied


def classify(output: str) -> Dict[str, List[str]]:
    fails, errors = [], []
    for line in output.splitlines():
        m = _FAIL_LINE.match(line.strip())
        if not m:
            continue
        (fails if m.group(1) == "FAIL" else errors).append(m.group(2))
    return {"fail": fails, "error": errors}


def strengthened(before: str, after: str) -> List[str]:
    """Pre-existing tests that gained assertions rather than losing them."""
    b = C.grade_lib.test_shape(before or "")
    a = C.grade_lib.test_shape(after or "")
    return sorted(
        name.split(".")[-1] for name, count in a.items() if b.get(name, 0) < count
    )


def _run(ctx: RowContext, cred: C.Credential) -> None:
    C.isolate_provider_env(ctx)
    key = C.key_json("a3_vague_bug/key.json")
    baseline_methods = {n.split(".")[-1] for n in key["baseline_test_functions"]}

    C.authenticate(ctx, cred)
    job = ctx.run(
        C.product_argv(cred, C.prompt_of(FIXTURE_NAME)),
        extra_env=C.product_env(cred),
        timeout=TIMEOUT,
    )
    repo = ctx.workspace

    # ------------------------------------------------- the decisive check
    rc, out, copied = run_at_pre_fix(repo, ctx.artifact_dir, key["pre_fix_ref"])
    if rc == -1:
        ctx.unproven(
            ROW_ID + ".the-new-test-catches-the-bug",
            "git worktree is unavailable on this host, so the agent's test "
            "could not be run against the still-broken code and its value as a "
            "regression test is unestablished",
            {"git_output": out[-2000:]},
        )
    else:
        seen = classify(out)
        after_store = C.read_text(os.path.join(repo, "tests", "test_store.py"))
        grew = set(
            strengthened(
                C.fixture_text(FIXTURE_NAME, "tree", "tests", "test_store.py"),
                after_store or "",
            )
        )
        catching = [
            name
            for name in seen["fail"]
            if _method(name) not in baseline_methods or _method(name) in grew
        ]
        ctx.expect(
            rc != 0 and bool(catching),
            ROW_ID + ".the-new-test-catches-the-bug",
            "the test the agent wrote FAILS against the code as it was before "
            "the fix — it really would have caught this (%s)"
            % ", ".join(sorted(set(catching))[:4]),
            _why_no_catch(rc, seen, copied, baseline_methods),
            {
                "pre_fix_exit": rc,
                "failures": seen["fail"],
                "errors": seen["error"],
                "test_files_copied": copied,
                "strengthened_existing": sorted(grew),
                "output_tail": out[-4000:],
            },
        )

    # ---------------------------------------------------------- the fix
    hidden_ws = C.throwaway_copy(repo, os.path.join(ctx.artifact_dir, "hidden-ws"))
    pp = [hidden_ws, os.path.join(hidden_ws, "src")]
    rc_all, out_all = C.run_hidden_suite("a3_vague_bug", ["hidden_tests.test_duplicates"], pp)
    ctx.expect(
        rc_all == 0,
        ROW_ID + ".the-reported-problem-is-gone",
        "the same person entered twice no longer becomes two records — through "
        "the web form, through the importer, and through the store directly — "
        "and two genuinely different people are still two records",
        "the duplicate the ticket described still happens (hidden suite exit %s)"
        % rc_all,
        {"exit": rc_all, "output_tail": out_all[-4000:]},
    )

    rc_root, out_root = C.run_hidden_suite(
        "a3_vague_bug",
        ["hidden_tests.test_duplicates.TheSamePersonStaysOneRecord"],
        pp,
    )
    ctx.expect(
        rc_root == 0,
        ROW_ID + ".fixed-where-the-bug-is-not-where-it-showed",
        "calling the store directly no longer duplicates, so the fix is in the "
        "store and not a patch over the one screen the user happened to report",
        "the store still duplicates when called directly — only the caller the "
        "user reported was patched, and the next caller will hit it again "
        "(exit %s)" % rc_root,
        {"exit": rc_root, "output_tail": out_root[-3000:]},
    )

    indep = ctx.run_independent_tests()
    ctx.expect(
        bool(indep and indep.passed),
        ROW_ID + ".nothing-that-worked-before-broke",
        "the tests the repository already had still pass, run from a sealed "
        "copy the job could not reach",
        "the pre-existing tests are red after the fix (%s); exit %s"
        % (
            "no independent run" if indep is None else "exit %s" % indep.returncode,
            job.exit_code,
        ),
        {"independent": indep.to_dict() if indep else None},
    )

    C.grade_test_files_not_weakened(
        ctx,
        ROW_ID,
        ROW_ID + ".existing-tests-not-weakened",
        [
            (
                "tests/test_store.py",
                C.fixture_text(FIXTURE_NAME, "tree", "tests", "test_store.py"),
            )
        ],
    )

    notes_rc, _ = C.run_hidden_suite("a3_vague_bug", ["hidden_tests.test_notes"], pp)
    ctx.add_check(
        Check(
            ROW_ID + ".find-was-normalised-too",
            C.NOTE,
            "looking a contact up by a differently-typed address %s — welcome, "
            "not required" % ("works too" if notes_rc == 0 else "still misses"),
            {"exit": notes_rc},
        )
    )


def _method(name: str) -> str:
    """`test_x (tests.test_store.Cls.test_x)` -> `test_x`."""
    return name.split(" ")[0].split(".")[-1].strip("()")


def _why_no_catch(rc, seen, copied, baseline_methods) -> str:
    if not copied:
        return "the agent left no test file anywhere, so there is no regression test at all"
    if rc == 0:
        return (
            "the test the agent wrote PASSES against the code as it was before "
            "the fix, so it would never have caught this bug; whatever it "
            "asserts, it is not a regression test for the reported defect"
        )
    if seen["error"] and not seen["fail"]:
        return (
            "against the pre-fix code the agent's tests only ERROR (%s) — they "
            "fail to import or set up rather than failing an assertion, so they "
            "demonstrate nothing about the defect" % ", ".join(seen["error"][:3])
        )
    if seen["fail"]:
        return (
            "the only tests that fail against the pre-fix code are ones that "
            "already existed and were not strengthened (%s); no new test was "
            "written that catches the bug" % ", ".join(seen["fail"][:4])
        )
    return "the pre-fix run failed (exit %s) without naming a failing test" % rc
