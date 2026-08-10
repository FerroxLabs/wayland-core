"""B-4 — build it on the other machine, and clean up if I stop it.

This row needs a genuinely different computer and refuses to pretend otherwise.
`JOBCORPUS_B4_REMOTE` names it (`user@host`, reachable by key-based ssh from the
machine running the corpus).  Without one the row is UNPROVEN — never PASS and
never N/A, because running the "remote" build on the same box defeats the seed
check and proves nothing.  The driver also asks the remote for its hostname and
refuses to grade if it is this machine.

`echo hello` cannot pass.  `build.py` reads a random build seed that exists only
on the build host and stamps its fingerprint into the tarball; the grader
compares that fingerprint, the returned file's sha256 against the one the build
host recorded when it wrote it, and the test count in the build host's own
ledger.  The run id is never taken from the product's summary — it is read out
of the build host's ledger, which build.py wrote.

Two cases:

  build   the whole job runs there and the tarball comes back intact
  cancel  the user changes their mind WHILE the package is being written, and a
          colleague needs that machine thirty seconds later: nothing of ours
          still running, no partial files, lock released
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from _bcommon import (  # noqa: E402
    CORPUS_ROOT,
    FIXTURES,
    UNPROVEN,
    CaseEvidence,
    Check,
    FixtureError,
    ProductHome,
    finish,
    grade,
    new_record,
    unproven_setup,
    verdict_check,
)

ROW_ID = "B-4"
TIER = "B"
TITLE = "work is done on a remote machine"
FIXTURE = "fixtures/b4-remote-build"
KEY = "keys/b-4.key.json"
#: The user asked for a tarball in ./artifacts, nothing else.
DECLARED_SCOPE = ["artifacts/*"]
TEST_COMMAND = None
TIMEOUT = 1500

FIX = os.path.join(FIXTURES, "b4-remote-build")
REMOTE_ENV = "JOBCORPUS_B4_REMOTE"
REMOTE_ROOT_ENV = "JOBCORPUS_B4_REMOTE_ROOT"
CASES = ("build", "cancel")

RUN_ARGS = ["--trust-workspace", "--dangerously-skip-permissions"]

SSH_BASE = ["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ConnectTimeout=20"]

#: Bootstrapped over stdin so nothing has to survive two levels of shell
#: quoting — the remote may be cmd, PowerShell or a POSIX shell.
MKDIRS_PY = """
import os, sys
for p in sys.argv[1:]:
    os.makedirs(p, exist_ok=True)
print("ok")
"""

WHOAMI_PY = """
import json, platform, os
print(json.dumps({"node": platform.node(), "system": platform.system(),
                  "home": os.path.expanduser("~")}))
"""

CLEANUP_PY = """
import shutil, sys
for p in sys.argv[1:]:
    shutil.rmtree(p, ignore_errors=True)
print("ok")
"""


def _ssh(remote, args, timeout=300, stdin_program=None, arg_list=()):
    argv = SSH_BASE + [remote]
    if stdin_program is not None:
        argv += ["python3", "-", *[str(a) for a in arg_list]]
        return subprocess.run(argv, input=stdin_program, capture_output=True,
                              text=True, timeout=timeout, check=False)
    argv += list(args)
    return subprocess.run(argv, capture_output=True, text=True, timeout=timeout,
                          check=False, stdin=subprocess.DEVNULL)


def _scp(src, dst, recursive=False, timeout=600):
    argv = ["scp", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-q"]
    if recursive:
        argv.append("-r")
    argv += [src, dst]
    return subprocess.run(argv, capture_output=True, text=True, timeout=timeout, check=False)


def remote_identity(remote):
    res = _ssh(remote, [], stdin_program=WHOAMI_PY)
    if res.returncode != 0:
        raise FixtureError("cannot reach the build host %s: %s"
                           % (remote, (res.stderr or res.stdout).strip()[:300]))
    try:
        return json.loads(res.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        raise FixtureError("the build host answered with %r, not JSON"
                           % res.stdout.strip()[:200])


def collect_remote(remote, remote_dir, checkout, run_id):
    res = _ssh(remote, ["python3", remote_dir + "/collect_remote.py",
                        "--run-id", run_id, "--workspace", checkout], timeout=300)
    if res.returncode != 0:
        raise FixtureError("collect_remote failed on the build host: %s"
                           % (res.stderr or res.stdout).strip()[:400])
    return json.loads(res.stdout)


def run_id_from_ledger(remote_state, prefer_kind="run_start"):
    """The run id, taken from the BUILD HOST's ledger — never from the summary
    the product printed.  A product that says it built something it did not is
    exactly what this row exists to catch."""
    ids = [str(e.get("run_id")) for e in remote_state.get("ledger", [])
           if e.get("kind") == prefer_kind and e.get("run_id")]
    if not ids:
        ids = [str(e.get("run_id")) for e in remote_state.get("ledger", [])
               if e.get("run_id")]
    return ids[-1] if ids else None


class PackagingWatcher(threading.Thread):
    """Cancel the job the moment the build host starts writing the package.

    Cancelling earlier tests nothing (the key scores it UNPROVEN), so the
    window is found by polling the build host's own ledger rather than by
    guessing a delay.
    """

    def __init__(self, remote, remote_dir, checkout, deadline, ev):
        super().__init__(daemon=True)
        self.remote, self.remote_dir, self.checkout = remote, remote_dir, checkout
        self.deadline, self.ev = deadline, ev
        self.proc = None
        self.cancelled_at = None
        self.run_id = None
        self.saw_packaging = False

    def arm(self, proc, _ev):
        self.proc = proc
        self.start()

    def run(self):
        while time.time() < self.deadline and self.proc.poll() is None:
            time.sleep(3)
            try:
                state = collect_remote(self.remote, self.remote_dir, self.checkout, "-")
            except Exception:
                continue
            kinds = [e.get("kind") for e in state.get("ledger", [])]
            if "packaging_started" in kinds:
                self.saw_packaging = True
                self.run_id = run_id_from_ledger(state, "packaging_started")
                self._cancel()
                return

    def _cancel(self):
        """Cancel the way a user does: interrupt the job, then the terminal is
        gone.  The product gets a real chance to tidy up before the tree dies."""
        self.cancelled_at = time.time()
        self.ev.note("cancelling mid-package", run_id=self.run_id)
        try:
            os.killpg(os.getpgid(self.proc.pid), signal.SIGINT)
        except Exception:
            try:
                self.proc.send_signal(signal.SIGINT)
            except Exception:
                pass
        for _ in range(20):  # up to 10 s to wind down politely
            if self.proc.poll() is not None:
                return
            time.sleep(0.5)
        try:
            os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
        except Exception:
            pass


def stage_remote(remote, remote_root, nonce, ev):
    remote_dir = "%s/%s" % (remote_root.rstrip("/"), nonce)
    checkout = remote_dir + "/checkout"
    res = _ssh(remote, [], stdin_program=MKDIRS_PY, arg_list=[remote_dir])
    if res.returncode != 0:
        raise FixtureError("could not create %s on the build host: %s"
                           % (remote_dir, (res.stderr or res.stdout).strip()[:300]))
    ev.note("staged remote directory", remote_dir=remote_dir)

    for name in ("remote_seed.py", "collect_remote.py"):
        r = _scp(os.path.join(FIX, name), "%s:%s/%s" % (remote, remote_dir, name))
        if r.returncode != 0:
            raise FixtureError("could not copy %s to the build host: %s"
                               % (name, r.stderr.strip()[:300]))
    r = _scp(os.path.join(FIX, "seed"), "%s:%s" % (remote, checkout), recursive=True)
    if r.returncode != 0:
        raise FixtureError("could not copy the checkout to the build host: %s"
                           % r.stderr.strip()[:300])

    res = _ssh(remote, ["python3", remote_dir + "/remote_seed.py", "--reset",
                        "--workspace", checkout], timeout=300)
    if res.returncode != 0:
        raise FixtureError("remote_seed failed on the build host: %s"
                           % (res.stderr or res.stdout).strip()[:400])
    return remote_dir, checkout, json.loads(res.stdout)


def local_precheck(path):
    """Prove the build seed is absent here, so the artifact's origin means
    something.  Checks both the corpus operator's home and the throwaway home
    the product was given."""
    candidates = [
        os.path.join(os.path.expanduser("~"), ".jobcorpus-b4", "remote-only-seed.txt"),
        os.path.join(path, ".jobcorpus-b4", "remote-only-seed.txt"),
    ]
    present = [p for p in candidates if os.path.exists(p)]
    return {"seed_present_locally": bool(present), "checked": candidates,
            "present_at": present}


def run_case(binary, artifact_dir, case, remote, remote_root, prompt, timeout):
    evid = os.path.join(artifact_dir, "evidence", case)
    ws = os.path.join(artifact_dir, "ws", case)
    ev = CaseEvidence(evid, case)
    nonce = "%s-%s" % (case, uuid.uuid4().hex[:10])
    remote_dir = checkout = None
    try:
        ident = remote_identity(remote)
        ev.note("build host identified", **ident)
        import platform as _plat
        if ident.get("node", "").strip().lower() == _plat.node().strip().lower():
            raise FixtureError(
                "the 'build host' %s reports the same hostname as this machine "
                "(%s); a local build proves nothing about running elsewhere"
                % (remote, _plat.node()))

        remote_dir, checkout, seed = stage_remote(remote, remote_root, nonce, ev)

        if os.path.isdir(ws):
            shutil.rmtree(ws)
        os.makedirs(os.path.join(ws, "artifacts"))
        for name, body in (("remote-host.txt", remote), ("remote-path.txt", checkout)):
            with open(os.path.join(ws, name), "w", encoding="utf-8") as fh:
                fh.write(body + "\n")

        home = ProductHome(os.path.join(artifact_dir, "home", case), session=True)
        _lend_ssh_identity(home.root)
        with open(os.path.join(evid, "local-precheck.json"), "w", encoding="utf-8") as fh:
            json.dump(local_precheck(home.root), fh, indent=2, sort_keys=True)

        watcher = None
        on_start = None
        if case == "cancel":
            watcher = PackagingWatcher(remote, remote_dir, checkout,
                                       time.time() + timeout, ev)
            on_start = watcher.arm

        result = ev.run_product(binary, [*RUN_ARGS, prompt], "start", cwd=ws,
                                env=home.env(), timeout=timeout, on_start=on_start)

        if case == "cancel":
            # A colleague needs the machine ~30 s later; look then, not sooner.
            time.sleep(15)

        state = collect_remote(remote, remote_dir, checkout, "-")
        run_id = run_id_from_ledger(state)
        if not run_id:
            # No build was ever recorded there. Keep a run id that cannot match
            # so the grader says so, instead of falling into "which run?".
            run_id = "no-build-recorded-" + nonce
        else:
            state = collect_remote(remote, remote_dir, checkout, run_id)
        with open(os.path.join(evid, "remote-after.json"), "w", encoding="utf-8") as fh:
            json.dump(state, fh, indent=2, sort_keys=True)

        ev.snapshot(os.path.join(ws, "artifacts"), "artifacts")
        ev.capture_git(ws)
        ev.write_run_json(
            case=case, run_id=run_id, remote_host=remote,
            remote_node=ident.get("node"), remote_system=ident.get("system"),
            remote_checkout=checkout, seed_fingerprint=seed.get("seed_fingerprint"),
            exit_code=result["exit_code"], timed_out=result["timed_out"],
            cancel_requested_iso=(
                time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(watcher.cancelled_at))
                if watcher and watcher.cancelled_at else None),
            packaging_window_seen=bool(watcher and watcher.saw_packaging),
        )
        return evid
    finally:
        ev.close()
        if remote_dir:
            try:
                _ssh(remote, [], stdin_program=CLEANUP_PY, arg_list=[remote_dir],
                     timeout=180)
            except Exception:
                pass


def _lend_ssh_identity(home_root):
    """The user's own machine has ssh access to the build host; the throwaway
    home the product is given must too, or the row measures the absence of a
    key rather than the product."""
    src = os.path.join(os.path.expanduser("~"), ".ssh")
    dst = os.path.join(home_root, ".ssh")
    if not os.path.isdir(src):
        raise FixtureError(
            "this machine has no ~/.ssh, so it is not a machine that can reach a "
            "build host; the row would measure the missing key, not the product")
    os.makedirs(dst, exist_ok=True)
    os.chmod(dst, 0o700)
    copied = []
    for name in ("id_ed25519", "id_ed25519.pub", "id_rsa", "id_rsa.pub",
                 "known_hosts", "config"):
        s = os.path.join(src, name)
        if os.path.isfile(s):
            d = os.path.join(dst, name)
            shutil.copyfile(s, d)
            os.chmod(d, 0o600)
            copied.append(name)
    if not any(n.startswith("id_") and not n.endswith(".pub") for n in copied):
        raise FixtureError("no ssh private key on this machine to reach the build host")


def main(binary: str, artifact_dir: str):
    rec = new_record(ROW_ID, TIER, TITLE, binary, os.path.join(CORPUS_ROOT, KEY))
    timeout = int(os.environ.get("JOBCORPUS_B4_TIMEOUT", TIMEOUT))
    remote = os.environ.get(REMOTE_ENV, "").strip()
    remote_root = os.environ.get(REMOTE_ROOT_ENV, "").strip() or "jobcorpus-b4"
    rec.world["remote"] = remote or None
    rec.world["remote_root"] = remote_root

    if not remote:
        for case in CASES:
            rec.add_check(Check(
                "%s.%s" % (ROW_ID, case), UNPROVEN,
                "no second machine was made available to this run (%s is unset), so "
                "nothing about doing the work somewhere else was measured. Running "
                "the 'remote' build on this box would defeat the build-seed check "
                "and prove nothing." % REMOTE_ENV))
        return finish(rec, artifact_dir)

    try:
        with open(os.path.join(FIX, "prompt.txt"), "r", encoding="utf-8") as fh:
            prompt = fh.read().strip()
    except OSError as exc:
        return finish(unproven_setup(rec, ROW_ID + ".fixture", exc), artifact_dir)

    verdicts = {}
    for case in CASES:
        try:
            evid = run_case(binary, artifact_dir, case, remote, remote_root, prompt, timeout)
        except Exception as exc:
            verdicts[case] = {"state": "UNPROVEN",
                              "reasons": ["the case could not be staged: %s" % exc],
                              "observed": {}, "notes": []}
            continue
        verdicts[case] = grade("grade_b4.py", evid)

    for case in CASES:
        rec.add_check(verdict_check("%s.%s" % (ROW_ID, case), verdicts[case]))
    rec.world["grader_verdicts"] = verdicts
    return finish(rec, artifact_dir)
