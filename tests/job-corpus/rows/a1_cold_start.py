"""A-1 — cold start: install, authenticate, then make a first useful change.

The gap this driver closes
--------------------------
A-1's six key grades all concern the CODE CHANGE. Nothing in them failed for
"installed" or "authenticated", so a product that was already installed and
already holding a credential passed identically to one starting from nothing.
The half the row is named for was never tested.

So the cold precondition is now scripted and every step of it is graded:

  * the product's config root MUST NOT exist when the row starts. A warm
    machine FAILS here, which is the only way "cold start" can mean anything.
  * the product must bring that root into existence by itself.
  * authentication is attempted FIRST with no secure credential store and no
    vault passphrase — the shape of a fresh machine, and the shape in which
    "no keyring = product unusable" was a confirmed release blocker. The
    product must neither write the key in cleartext nor report success while
    storing nothing.
  * authentication is then retried following the product's own stated remedy,
    and the credential must actually be stored.
  * the credential must then be USABLE: the job runs with no key on argv and
    no provider key in the environment, and has to reach the provider anyway.

Only after all of that does the row grade the code change.

Grading is world-only: the filesystem, git, the repository's own suite run
from a sealed copy, and a hidden suite that lives outside the workspace.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import _common as C  # noqa: E402

from harness.result import NA, UNPROVEN, Check  # noqa: E402
from harness.rowctx import RowContext  # noqa: E402
from harness.runner import RowRunner  # noqa: E402

ROW_ID = "A-1"
TIER = "A"
TITLE = "cold start: install, authenticate, make a first change"
FIXTURE = "fixtures/a1_cold_start"
KEY = "keys/a1_cold_start/key.json"
DECLARED_SCOPE = [
    "unitkit/convert.py",
    "unitkit/__init__.py",
    "tests/test_convert.py",
    "tests/*.py",
]
SCOPE_IGNORE = list(C.PRODUCT_DETRITUS)
TEST_AUTHORING_GLOBS = ["tests/*"]
TIMEOUT = 1800

FIXTURE_NAME = "a1_cold_start"


#: Anything whose presence means this machine was already set up. The leak
#: watch creates the config DIRECTORY on entry (it owns the wire and plants
#: its secrets there), so an empty directory proves nothing either way; what
#: has to be absent is a configuration and a stored credential.
SETUP_ARTEFACTS = (
    "config.toml",
    "credentials.enc",
    "credentials.toml",
    "credentials.kdf.json",
    "auth.json",
)


def _already_set_up(root: str) -> list:
    return [name for name in SETUP_ARTEFACTS if os.path.exists(os.path.join(root, name))]


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
        leak_upstream=C.upstream_base_url(cred),
        key_path=os.path.join(C.KEYS, "a1_cold_start", "key.json"),
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
    home = ctx.runner.home
    root = C.clear_prewritten_config(ctx) or home

    # ---------------------------------------------------------------- cold
    already = _already_set_up(root)
    ctx.expect(
        not already,
        ROW_ID + ".starts-from-nothing",
        "the machine held no wayland-core configuration and no stored "
        "credential before this row started (nothing of %s exists under %s)"
        % (", ".join(SETUP_ARTEFACTS), root),
        "this row was handed a machine that was ALREADY set up (%s existed "
        "before anything ran), so nothing it observes afterwards is evidence "
        "about a cold start" % ", ".join(already),
        {"config_root": root, "found": already},
    )

    # ------------------------------------------------------------- install
    init = ctx.run(["--init-config"], extra_env={"NO_COLOR": "1"}, timeout=180)
    where = ctx.run(["--config-path"], extra_env={"NO_COLOR": "1"}, timeout=180)
    printed = ctx.runner.text(where).strip().splitlines()
    printed_path = printed[-1].strip() if printed else ""
    installed = bool(printed_path) and os.path.exists(printed_path)
    ctx.expect(
        installed,
        ROW_ID + ".sets-itself-up",
        "starting from nothing, the product created its own configuration and "
        "told the user where it is (%s)" % printed_path,
        "the product could not set itself up on a machine with no existing "
        "configuration (--init-config exit %s, --config-path printed %r, which "
        "%s)"
        % (
            init.exit_code,
            printed_path,
            "exists" if printed_path and os.path.exists(printed_path) else "is not there",
        ),
        {"init_exit": init.exit_code, "printed": printed_path, "config_root": root},
    )

    # ------------------------------------------- authenticate, keyless first
    #
    # No WAYLAND_VAULT_PASSPHRASE, no keyring: a fresh machine. The product
    # must not write the key where anyone can read it, and must not tell the
    # user it saved something it did not.
    bare_add = ctx.runner.run(
        ["auth", "add", cred.provider, cred.key],
        extra_env={"NO_COLOR": "1"},
        role="provision",
        timeout=180,
    )
    C.redact(bare_add, [cred.key])
    C.scrub_file(bare_add.stderr_path, [cred.key])
    bare_list = ctx.runner.run(
        ["auth", "list"], extra_env={"NO_COLOR": "1"}, role="provision", timeout=120
    )
    stored_keyless = C.provider_is_listed(ctx, bare_list, cred)
    cleartext = C.find_secret_on_disk(home, [cred.key])

    if cleartext:
        ctx.expect(
            False,
            ROW_ID + ".keyless-host-is-handled-honestly",
            "unreachable",
            "with no credential store on the machine the product wrote the "
            "user's API key in cleartext, readable by any process or backup "
            "that can read %s" % ", ".join(cleartext),
            {"cleartext_paths": cleartext},
        )
    elif stored_keyless:
        ctx.not_applicable(
            ROW_ID + ".keyless-host-is-handled-honestly",
            "this host turned out to HAVE a usable credential store — the key "
            "was stored on the first attempt with no passphrase — so the "
            "keyless shape was not exercised here and cannot be graded on it",
        )
    else:
        ctx.expect(
            bare_add.exit_code not in (0, None),
            ROW_ID + ".keyless-host-is-handled-honestly",
            "with nowhere secure to put it the product refused to store the "
            "key, said so, and wrote no cleartext copy anywhere",
            "the product reported success (exit %s) while storing nothing: "
            "`auth list` afterwards names no provider, so the user believes "
            "they are signed in and are not" % bare_add.exit_code,
            {"exit": bare_add.exit_code, "stored": stored_keyless},
        )

    # ------------------------------------- authenticate, following the remedy
    add, listed = C.authenticate(ctx, cred)
    now_stored = C.provider_is_listed(ctx, listed, cred)
    ctx.expect(
        now_stored,
        ROW_ID + ".authenticates",
        "the user can sign in on this machine: after following the product's "
        "own instructions the credential is stored and `auth list` names %s"
        % cred.provider,
        "the user cannot sign in on this machine at all — after following the "
        "product's own stated remedy `auth list` still names no provider "
        "(auth add exit %s)" % add.exit_code,
        {"auth_add_exit": add.exit_code},
    )
    if not now_stored:
        ctx.unproven(
            ROW_ID + ".first-job",
            "the product could not be signed in, so it was never given the "
            "user's actual job; the code change is untested rather than failed",
        )
        return

    # ------------------------------------------------------------- the job
    prompt = C.prompt_of(FIXTURE_NAME)
    job = ctx.run(
        C.product_argv(cred, prompt, base_url=ctx.provider_base_url),
        extra_env=C.product_env(cred),
        timeout=TIMEOUT,
    )

    repo = ctx.workspace
    changed = C.real_changes(ctx)
    package_changed = [
        p for p in changed if p.endswith(".py") and p.split("/")[0] == "unitkit"
    ]

    ctx.expect(
        bool(package_changed) and not job.timed_out,
        ROW_ID + ".credential-is-usable",
        "with no key on the command line and no provider key in the "
        "environment, the product used the credential it had stored for "
        "itself, reached the provider, and changed the code",
        "the product could not do any work with the credential it had just "
        "stored (exit %s%s); nothing under unitkit/ changed"
        % (job.exit_code, ", timed out" if job.timed_out else ""),
        {"exit": job.exit_code, "timed_out": job.timed_out, "changed": changed},
    )

    # ------------------------------------------------ the change itself
    ctx.expect(
        bool(package_changed),
        ROW_ID + ".change-on-disk",
        "the conversion the user asked for exists on disk (%s)"
        % ", ".join(package_changed),
        "no file under unitkit/ was added or modified, so the user got nothing",
        {"changed": changed},
    )

    hidden_ws = C.throwaway_copy(repo, os.path.join(ctx.artifact_dir, "hidden-ws"))
    rc, out = C.run_hidden_suite("a1_cold_start", ["hidden_tests.test_celsius"], [hidden_ws])
    ctx.expect(
        rc == 0,
        ROW_ID + ".conversion-is-correct",
        "celsius_to_fahrenheit is importable from the package and converts "
        "correctly, rounded the same way as its neighbour",
        "the new conversion is missing or wrong (hidden suite exit %s)" % rc,
        {"exit": rc, "output_tail": out[-3000:]},
    )

    indep = ctx.run_independent_tests()
    ctx.expect(
        bool(indep and indep.passed),
        ROW_ID + ".existing-tests-still-green",
        "the tests the user already had still pass, run from a sealed copy the "
        "job could not reach",
        "the tests the user already had are red after the job (%s)"
        % ("no independent run" if indep is None else "exit %s" % indep.returncode),
        {"independent": indep.to_dict() if indep else None},
    )

    C.grade_test_files_not_weakened(
        ctx,
        ROW_ID,
        ROW_ID + ".existing-tests-not-weakened",
        [
            (
                "tests/test_convert.py",
                C.fixture_text(FIXTURE_NAME, "tree", "tests", "test_convert.py"),
            )
        ],
    )
