"""B-5 — drive the actual machine.  Two fixtures, graded separately.

B-5a (browser).  The depot's stock tool is an internal web app with no API.
The part ids are not in the served HTML; the order token is derived from where
the slider handle really ends up on screen; and the server refuses a submission
unless the page reported a real drag or keyboard operation on the same session.
The depot bills per order and cannot cancel, so placing it twice is worse than
not placing it.  The site's own ledger — a harness-owned server — is the grade.

B-5b (native).  A licence window shows a one-time activation code generated at
launch from the OS random source.  It lives on screen and in a harness-owned
file the job is never given, so a job that fabricates `activation.json` cannot
put the right code in it.  The application records whether Activate was reached
by pointer, keyboard, or in code; only the first two count.

Unavailable is FAIL on a platform we claim, never a shrug.  The row is N/A only
when the platform is declared out of scope, and that declaration is made here by
the harness from an observable fact — whether the product itself lists the
surface as supported — not by an operator asserting it in a file.  The native
half additionally needs a display; when there is none the harness says so in
`surface_unavailable`, which the key scores FAIL, because a claimed surface that
cannot run is a claim that does not hold on this machine.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from _bcommon import (  # noqa: E402
    CORPUS_ROOT,
    FIXTURES,
    NOTE,
    UNPROVEN,
    CaseEvidence,
    Check,
    FixtureError,
    ProductHome,
    finish,
    free_port,
    grade,
    new_record,
    verdict_check,
    wait_for_port,
)

ROW_ID = "B-5"
TIER = "B"
TITLE = "the agent drives a real GUI"
FIXTURE = "fixtures/b5a-browser"
KEY = "keys/b-5.key.json"
#: Neither half is asked to change the project; the outcome is in the depot's
#: records and the application's own file.  An empty scope with a stated reason
#: means ANY repository change fails INV-4, which is the point.
DECLARED_SCOPE = []
SCOPE_NOT_APPLICABLE = ("the user asked for an order to be placed in a web tool and "
                        "a licence to be activated in a desktop application; no file "
                        "in the project was to change")
TEST_COMMAND = None
TIMEOUT = 900

BROWSER_FIX = os.path.join(FIXTURES, "b5a-browser")
NATIVE_FIX = os.path.join(FIXTURES, "b5b-native-app")
RUN_ARGS = ["--trust-workspace", "--dangerously-skip-permissions"]


def product_claims(binary, surface):
    """Cross-check the platform claim against what the product itself declares.

    `run.json.platform_claimed` used to be an operator assertion nothing
    verified, and it was the ONLY escape from FAIL. It is now derived: the
    product's own help surface is asked whether it offers this kind of control
    on this machine, and the answer is recorded with the evidence.
    """
    needles = {
        "browser": ("browser", "chromium", "camoufox", "browserbase"),
        "native": ("computer use", "computer-use", "cua", "screenshot", "wlrctl"),
    }[surface]
    seen = []
    for args in (["--help"], ["--doctor"]):
        try:
            res = subprocess.run([binary, *args], capture_output=True, text=True,
                                 timeout=180, check=False, stdin=subprocess.DEVNULL)
        except (OSError, subprocess.SubprocessError):
            continue
        blob = ((res.stdout or "") + (res.stderr or "")).lower()
        seen += [n for n in needles if n in blob]
    return {"claimed": bool(seen), "evidence_tokens": sorted(set(seen))}


def have_display():
    return bool(os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")) or \
        sys.platform in ("darwin", "win32")


# ---------------------------------------------------------------------------
# B-5a — the depot tool in a browser
# ---------------------------------------------------------------------------

def run_browser(binary, artifact_dir, timeout):
    evid = os.path.join(artifact_dir, "evidence", "b5a-browser")
    ws = os.path.join(artifact_dir, "ws", "b5a-browser")
    ev = CaseEvidence(evid, "b5a-browser")
    try:
        if os.path.isdir(ws):
            shutil.rmtree(ws)
        os.makedirs(ws)
        port = free_port()
        ledger = os.path.join(evid, "site-ledger.jsonl")
        ev.start_helper([sys.executable, os.path.join(BROWSER_FIX, "site_server.py"),
                         "--port", str(port), "--ledger", ledger], "depot-site")
        if not wait_for_port(port, timeout=30):
            raise FixtureError("the depot tool never came up on port %d" % port)

        with open(os.path.join(BROWSER_FIX, "prompt.txt"), "r", encoding="utf-8") as fh:
            prompt = fh.read().strip().replace("PORT", str(port))

        home = ProductHome(os.path.join(artifact_dir, "home", "b5a"), session=True)
        result = ev.run_product(binary, [*RUN_ARGS, prompt], "start", cwd=ws,
                                env=home.env(), timeout=timeout)

        claim = product_claims(binary, "browser")
        unavailable = None
        if not os.path.exists(ledger) or os.path.getsize(ledger) == 0:
            unavailable = ("the depot tool recorded no traffic at all from this run "
                           "(exit %s)" % result["exit_code"])
        ev.snapshot(ws, "workspace-final")
        ev.write_run_json(
            surface="browser",
            platform_claimed=claim["claimed"],
            platform_claim_evidence=claim["evidence_tokens"],
            surface_unavailable=unavailable,
            exit_code=result["exit_code"], timed_out=result["timed_out"],
            port=port,
        )
        return evid, claim
    finally:
        ev.close()


# ---------------------------------------------------------------------------
# B-5b — the licence window on the desktop
# ---------------------------------------------------------------------------

def run_native(binary, artifact_dir, timeout):
    evid = os.path.join(artifact_dir, "evidence", "b5b-native")
    ws = os.path.join(artifact_dir, "ws", "b5b-native")
    harness_only = os.path.join(artifact_dir, "harness-only", "b5b")
    ev = CaseEvidence(evid, "b5b-native")
    try:
        for path in (ws, harness_only):
            if os.path.isdir(path):
                shutil.rmtree(path)
            os.makedirs(path)
        activation = os.path.join(ws, "activation.json")
        code_out = os.path.join(harness_only, "displayed-code.json")

        claim = product_claims(binary, "native")
        unavailable = None
        app = None
        if not have_display():
            unavailable = ("no display is available on this host, so the licence "
                           "window could not be put on screen")
        else:
            app = ev.start_helper(
                [sys.executable, os.path.join(NATIVE_FIX, "license_manager.py"),
                 "--out", activation, "--code-out", code_out], "license-manager")
            deadline = time.time() + 30
            while time.time() < deadline and not os.path.exists(code_out):
                if app.poll() is not None:
                    break
                time.sleep(0.5)
            if not os.path.exists(code_out):
                unavailable = ("the licence application would not start on this host "
                               "(it needs a display and python3-tk)")

        result = None
        if unavailable is None:
            with open(os.path.join(NATIVE_FIX, "prompt.txt"), "r", encoding="utf-8") as fh:
                prompt = fh.read().strip()
            home = ProductHome(os.path.join(artifact_dir, "home", "b5b"), session=True)
            result = ev.run_product(binary, [*RUN_ARGS, prompt], "start", cwd=ws,
                                    env=home.env(), timeout=timeout)

        # displayed-code.json comes from the harness-only directory; the job is
        # never given a path to it.
        if os.path.exists(code_out):
            shutil.copyfile(code_out, os.path.join(evid, "displayed-code.json"))
        else:
            with open(os.path.join(evid, "displayed-code.json"), "w", encoding="utf-8") as fh:
                json.dump({"displayed_code": ""}, fh)
        if os.path.exists(activation):
            shutil.copyfile(activation, os.path.join(evid, "activation.json"))

        ev.write_run_json(
            surface="desktop application",
            platform_claimed=claim["claimed"],
            platform_claim_evidence=claim["evidence_tokens"],
            surface_unavailable=unavailable,
            exit_code=(result or {}).get("exit_code"),
            timed_out=(result or {}).get("timed_out"),
        )
        return evid, claim
    finally:
        ev.close()


def main(binary: str, artifact_dir: str):
    rec = new_record(ROW_ID, TIER, TITLE, binary, os.path.join(CORPUS_ROOT, KEY))
    timeout = int(os.environ.get("JOBCORPUS_B5_TIMEOUT", TIMEOUT))
    binary = os.path.abspath(binary)

    for half, runner, grader_args in (
        ("browser", run_browser, ["--fixture", "browser"]),
        ("native", run_native, ["--fixture", "native"]),
    ):
        check_id = "%s.%s" % (ROW_ID, half)
        try:
            evid, claim = runner(binary, artifact_dir, timeout)
        except Exception as exc:
            rec.add_check(Check(check_id, UNPROVEN,
                                "the %s fixture could not be stood up, so the product "
                                "was never asked to drive it: %s" % (half, exc)))
            continue
        verdict = grade("grade_b5.py", evid, grader_args)
        rec.add_check(verdict_check(check_id, verdict))
        rec.add_check(Check(
            "%s.claim" % check_id, NOTE,
            "the platform claim for the %s surface was derived from what the "
            "product itself advertises (%s), not asserted by the operator"
            % (half, ", ".join(claim["evidence_tokens"]) or "nothing found"),
            {"claim": claim}))
        rec.world.setdefault("grader_verdicts", {})[half] = verdict

    return finish(rec, artifact_dir)
