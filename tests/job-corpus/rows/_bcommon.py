"""Shared machinery for the Tier-B row drivers (B-1 … B-5).

The B rows all have the same shape: stand up one or more harness-owned observer
processes, drive the REAL product binary against a fixture, then hand the
resulting evidence directory to the matching grader in `graders/` and turn its
verdict into Checks on a RowRecord.

Three rules this module exists to enforce, so no individual row can forget:

1. **The product is actually run.**  `CaseEvidence.run_product` is the only way
   a row starts the job, and it refuses to pretend: the binary must exist, the
   argv is recorded at spawn time, and the exit code and wall time are facts.

2. **`run.json` is harness-authored, structurally.**  Every process the harness
   starts is appended to `argv-log.jsonl` *before* it can produce a byte of
   output, and the fields the graders lean on — `resumed`, `resume_cmd`,
   `exit_codes` — are DERIVED from that log by `write_run_json`, which raises
   if a caller tries to supply them.  A future runner cannot quietly fill
   `resume_cmd` from product output without the grader's cross-check
   disagreeing, because the grader reads the spawn log, not the narrative.

3. **The grader is a separate process.**  `grade` shells out to
   `graders/grade_bN.py --evidence …` so the grader keeps reading only world
   artifacts, exactly as it does when an operator runs it by hand.

Pure stdlib, Python 3.8+.
"""

from __future__ import annotations

import glob
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import uuid
from typing import Any, Dict, List, Optional, Sequence

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS_ROOT = os.path.dirname(HERE)
GRADERS = os.path.join(CORPUS_ROOT, "graders")
FIXTURES = os.path.join(CORPUS_ROOT, "fixtures")

sys.path.insert(0, CORPUS_ROOT)

from harness.result import (  # noqa: E402
    FAIL,
    NA,
    NOTE,
    PASS,
    UNPROVEN,
    Check,
    RowRecord,
)
from harness.world import IS_WINDOWS, ProcessTable, kill_tree, sha256_file  # noqa: E402

#: Credentials that must never reach the product: a bare API_KEY is honoured as
#: a provider credential, which both contaminates an "isolated" run and is a
#: live exfiltration path.
SCRUB_ENV = ("API_KEY", "FLUX_API_KEY")

#: Where the operator declares the provider the corpus may spend money through.
#: A path to a TOML fragment containing a [default] table and the matching
#: [providers.*] table.  Deliberately NOT in the repository and never in argv.
PROVIDER_TOML_ENV = "JOBCORPUS_PROVIDER_TOML"


class FixtureError(RuntimeError):
    """The harness could not set the row up.  Always UNPROVEN, never FAIL."""


# ---------------------------------------------------------------------------
# small helpers
# ---------------------------------------------------------------------------

def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def wait_for_port(port: int, timeout: float = 30.0, host: str = "127.0.0.1") -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with socket.create_connection((host, port), timeout=1.0):
                return True
        except OSError:
            time.sleep(0.2)
    return False


def wait_for_file(path: str, timeout: float = 60.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if os.path.exists(path):
            return True
        time.sleep(0.25)
    return False


def read_jsonl(path: str) -> List[Dict[str, Any]]:
    out: List[Dict[str, Any]] = []
    if not os.path.exists(path):
        return out
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except ValueError:
                continue
    return out


def copy_tree(src: str, dest: str) -> str:
    if os.path.isdir(dest):
        shutil.rmtree(dest)
    shutil.copytree(src, dest, symlinks=True)
    return dest


def git_text(ws: str, *args: str) -> str:
    try:
        res = subprocess.run(
            ["git", "-C", ws, *args], capture_output=True, text=True, timeout=120, check=False
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return "git %s failed: %s\n" % (" ".join(args), exc)
    return res.stdout


# ---------------------------------------------------------------------------
# the product's isolated home
# ---------------------------------------------------------------------------

#: Appended after the operator's provider fragment.  It must not re-open a
#: table the fragment already declared — a duplicated [default] is a TOML error,
#: which would look like a product failure and is not one.
_BASE_TOML = """
[session]
enabled = {session}

[memory]
enabled = false
"""


def provider_fragment() -> str:
    """The operator-declared provider block, or raise.

    Kept out of the repository and out of argv on purpose.  A row that cannot
    reach a provider is UNPROVEN — the product was never asked to do anything.
    """
    path = os.environ.get(PROVIDER_TOML_ENV)
    if not path:
        raise FixtureError(
            "no provider is configured for this corpus run: set %s to a TOML "
            "fragment declaring [default] provider/model and the matching "
            "[providers.<name>] block. Without it the product cannot be asked "
            "to do anything, so the row measures nothing." % PROVIDER_TOML_ENV
        )
    if not os.path.isfile(path):
        raise FixtureError("%s points at %s, which does not exist" % (PROVIDER_TOML_ENV, path))
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


def provider_base_url() -> Optional[str]:
    """The upstream base_url the operator declared, for rows that proxy it."""
    frag = provider_fragment()
    m = re.search(r'^\s*base_url\s*=\s*"([^"]+)"', frag, re.M)
    return m.group(1) if m else None


class ProductHome:
    """A throwaway WAYLAND_HOME: the row never inherits a developer's config."""

    def __init__(self, root: str, session: bool = True, extra_toml: str = "",
                 base_url: Optional[str] = None) -> None:
        self.root = os.path.abspath(root)
        os.makedirs(self.root, exist_ok=True)
        try:
            os.chmod(self.root, 0o700)
        except OSError:
            pass
        fragment = provider_fragment().rstrip()
        if base_url:
            # Point the product at a harness-owned proxy instead of the
            # provider.  Rewriting the existing key rather than appending one
            # keeps the TOML valid; a duplicated key would look like a product
            # failure and is not one.
            fragment, n = re.subn(r'^\s*base_url\s*=\s*"[^"]*"',
                                  'base_url = "%s"' % base_url, fragment, flags=re.M)
            if not n:
                raise FixtureError(
                    "the provider fragment declares no base_url, so it cannot be "
                    "pointed at the harness proxy this row needs")
        body = (
            fragment
            + "\n"
            + _BASE_TOML.format(session="true" if session else "false")
            + (extra_toml or "")
        )
        self.config_path = os.path.join(self.root, "config.toml")
        fd = os.open(self.config_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(body)
        # A headless host has no OS keyring.  The product's own documented
        # remedy is a vault passphrase, so give it one rather than grading the
        # product in a mode it tells you not to run in.
        self.passphrase = uuid.uuid4().hex + uuid.uuid4().hex
        self.passphrase_path = os.path.join(self.root, ".vault-passphrase")
        fd = os.open(self.passphrase_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(self.passphrase)

    def env(self, extra: Optional[Dict[str, str]] = None) -> Dict[str, str]:
        env = dict(os.environ)
        for key in SCRUB_ENV:
            env.pop(key, None)
        env["WAYLAND_HOME"] = self.root
        env["HOME"] = self.root
        env["USERPROFILE"] = self.root
        env["WAYLAND_VAULT_PASSPHRASE"] = self.passphrase
        env["NO_COLOR"] = "1"
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        if extra:
            env.update(extra)
        for key in SCRUB_ENV:
            env.pop(key, None)
        return env


# ---------------------------------------------------------------------------
# one graded case: its evidence directory, its spawn log, its run.json
# ---------------------------------------------------------------------------

#: run.json fields the graders lean on that a caller may NOT hand-write.
DERIVED_FIELDS = ("resumed", "resume_cmd", "exit_codes", "argv_log", "authored_by",
                  "wall_seconds", "started_iso")


class CaseEvidence:
    """The evidence directory for one case, and the only way to start a process.

    Every spawn is appended to `argv-log.jsonl` and fsynced BEFORE the child can
    write anything, so the log is a harness artifact by construction rather than
    by convention.  `write_run_json` derives the authorship-critical fields from
    that log and refuses to take them from the caller.
    """

    def __init__(self, root: str, case: str) -> None:
        self.root = os.path.abspath(root)
        os.makedirs(self.root, exist_ok=True)
        self.case = case
        self.argv_log = os.path.join(self.root, "argv-log.jsonl")
        self.started = time.time()
        self._seq = 0
        self._children: List[subprocess.Popen] = []
        self._helpers: List[subprocess.Popen] = []
        self._pre_pids = {e.pid for e in ProcessTable.take().entries}
        self._record_seq = 0
        # Stamp the log's own origin, so an empty log and a missing log differ.
        self._append({
            "kind": "case_open", "case": case, "pid": os.getpid(),
            "harness": "tests/job-corpus/rows/_bcommon.py",
        })

    # -- the spawn log ---------------------------------------------------
    def _append(self, entry: Dict[str, Any]) -> Dict[str, Any]:
        self._record_seq += 1
        entry = dict(entry)
        entry["seq"] = self._record_seq
        entry["ts"] = time.time()
        with open(self.argv_log, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(entry, sort_keys=True) + "\n")
            fh.flush()
            os.fsync(fh.fileno())
        return entry

    def note(self, text: str, **kw: Any) -> None:
        self._append(dict(kw, kind="note", text=text))

    # -- helper processes (observers, servers, reply bots) ----------------
    def start_helper(
        self,
        argv: Sequence[str],
        role: str,
        cwd: Optional[str] = None,
        env: Optional[Dict[str, str]] = None,
        log_name: Optional[str] = None,
    ) -> subprocess.Popen:
        stem = os.path.join(self.root, "logs", log_name or role)
        os.makedirs(os.path.dirname(stem), exist_ok=True)
        out = open(stem + ".out", "ab")
        err = open(stem + ".err", "ab")
        kw: Dict[str, Any] = {}
        if IS_WINDOWS:
            kw["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        else:
            kw["start_new_session"] = True
        proc = subprocess.Popen(
            list(argv), cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            stdout=out, stderr=err, **kw
        )
        self._append({"kind": "spawn", "role": role, "argv": list(argv),
                      "pid": proc.pid, "cwd": cwd or os.getcwd()})
        self._helpers.append(proc)
        return proc

    def run_helper(
        self,
        argv: Sequence[str],
        role: str,
        cwd: Optional[str] = None,
        env: Optional[Dict[str, str]] = None,
        timeout: int = 300,
        input_text: Optional[str] = None,
        redact_input: bool = False,
    ) -> subprocess.CompletedProcess:
        self._append({"kind": "exec", "role": role, "argv": list(argv),
                      "cwd": cwd or os.getcwd(),
                      "stdin": "<redacted>" if redact_input else None})
        return subprocess.run(
            list(argv), cwd=cwd, env=env, capture_output=True, text=True,
            timeout=timeout, check=False, input=input_text,
            stdin=None if input_text is not None else subprocess.DEVNULL,
        )

    # -- the product under test -------------------------------------------
    def run_product(
        self,
        binary: str,
        args: Sequence[str],
        role: str,
        cwd: str,
        env: Dict[str, str],
        timeout: int = 900,
        pid_file: Optional[str] = None,
        on_start=None,
    ) -> Dict[str, Any]:
        """Start the REAL product binary and wait for it (or its killer).

        `role` is what the grader keys on: "start" for the first invocation,
        "resume" for a pick-up-again invocation.  It is recorded at spawn time.
        """
        binary = os.path.abspath(binary)
        if not os.path.isfile(binary):
            raise FixtureError("no product binary at %s" % binary)
        self._seq += 1
        stem = os.path.join(self.root, "product-%s-%02d" % (role, self._seq))
        argv = [binary, *args]

        # The pid the fixture may kill has to be on disk BEFORE the product can
        # make its first request, so the boundary sweep is not a race.  `sh -c`
        # writes its own pid and then execs the product, which keeps the pid and
        # the process-group id.
        spawn_argv = list(argv)
        if pid_file and not IS_WINDOWS:
            spawn_argv = [
                "/bin/sh", "-c",
                'printf %s "$$" > "$1" || exit 97; shift; exec "$@"',
                "_", pid_file, *argv,
            ]

        kw: Dict[str, Any] = {}
        if IS_WINDOWS:
            kw["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        else:
            kw["start_new_session"] = True

        started = time.time()
        with open(stem + ".stdout", "wb") as fout, open(stem + ".stderr", "wb") as ferr:
            proc = subprocess.Popen(
                spawn_argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                stdout=fout, stderr=ferr, **kw
            )
            entry = self._append({
                "kind": "product", "role": role, "argv": argv, "spawn_argv": spawn_argv,
                "pid": proc.pid, "cwd": cwd, "pid_file": pid_file,
                "binary_sha256": sha256_file(binary),
            })
            if IS_WINDOWS and pid_file:
                with open(pid_file, "w", encoding="utf-8") as fh:
                    fh.write(str(proc.pid))
            self._children.append(proc)
            if on_start is not None:
                # Rows that have to act WHILE the job is running (watch a remote
                # ledger, cancel mid-side-effect) get the live process here.
                on_start(proc, self)
            timed_out = False
            try:
                proc.communicate(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                kill_tree(proc.pid, proc.pid if not IS_WINDOWS else None)
                try:
                    proc.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    pass
        result = {
            "role": role, "argv": argv, "exit_code": proc.returncode,
            "timed_out": timed_out, "seconds": round(time.time() - started, 2),
            "stdout": stem + ".stdout", "stderr": stem + ".stderr",
            "spawn_seq": entry["seq"],
        }
        self._append(dict(result, kind="product_exit"))
        return result

    def product_text(self, role: Optional[str] = None) -> str:
        parts = []
        for path in sorted(glob.glob(os.path.join(self.root, "product-*.std*"))):
            if role and ("-%s-" % role) not in os.path.basename(path):
                continue
            with open(path, "rb") as fh:
                parts.append(fh.read().decode("utf-8", "replace"))
        return "\n".join(parts)

    # -- world snapshots ---------------------------------------------------
    def snapshot(self, src: str, name: str) -> Optional[str]:
        if not os.path.isdir(src):
            return None
        return copy_tree(src, os.path.join(self.root, name))

    def capture_git(self, ws: str, log_format: str = "%H %s") -> None:
        with open(os.path.join(self.root, "git-log.txt"), "w", encoding="utf-8") as fh:
            fh.write(git_text(ws, "log", "--format=" + log_format, "--name-only"))
        with open(os.path.join(self.root, "git-status.txt"), "w", encoding="utf-8") as fh:
            fh.write(git_text(ws, "status", "--porcelain"))

    def capture_processes(self, name: str = "procs-after-kill.txt",
                          needle: Optional[str] = None) -> List[str]:
        rows = []
        for entry in ProcessTable.take().entries:
            line = "%s %s" % (entry.pid, entry.cmdline)
            if needle is None or needle in entry.cmdline:
                rows.append(line)
        with open(os.path.join(self.root, name), "w", encoding="utf-8") as fh:
            fh.write("\n".join(rows) + "\n")
        return rows

    def surviving_children(self) -> List[Dict[str, Any]]:
        table = ProcessTable.take()
        found: Dict[int, Any] = {}
        for proc in self._children:
            for e in table.descendants(root_pid=proc.pid, exclude_pids=self._pre_pids):
                found[e.pid] = e
        return [found[p].to_dict() for p in sorted(found)]

    # -- run.json, derived ---------------------------------------------------
    def write_run_json(self, **fields: Any) -> Dict[str, Any]:
        """Write run.json with the authorship-critical fields DERIVED.

        `resumed`, `resume_cmd` and `exit_codes` come from the spawn log this
        object wrote, never from a caller and never from product output.  A
        caller that tries to pass one gets a FixtureError, so the inversion the
        vacuity review warned about cannot be introduced by accident later.
        """
        clash = sorted(set(fields) & set(DERIVED_FIELDS))
        if clash:
            raise FixtureError(
                "run.json fields %s are derived from the harness spawn log and may "
                "not be supplied by a row: they are the only thing standing between "
                "the resume check and a value copied out of product output"
                % ", ".join(clash)
            )
        entries = read_jsonl(self.argv_log)
        products = [e for e in entries if e.get("kind") == "product"]
        exits = [e for e in entries if e.get("kind") == "product_exit"]
        resumes = [e for e in products if e.get("role") == "resume"]
        payload = dict(fields)
        payload.update({
            "case": fields.get("case", self.case),
            "authored_by": "harness:tests/job-corpus/rows/_bcommon.py",
            "argv_log": "argv-log.jsonl",
            "resumed": bool(resumes),
            "resume_cmd": " ".join(resumes[-1]["argv"]) if resumes else "",
            "exit_codes": [e.get("exit_code") for e in exits],
            "wall_seconds": round(time.time() - self.started, 2),
            "started_iso": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(self.started)),
        })
        path = os.path.join(self.root, "run.json")
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(payload, fh, indent=2, sort_keys=True)
            fh.write("\n")
        return payload

    # -- teardown ------------------------------------------------------------
    def close(self) -> None:
        for proc in self._helpers + self._children:
            try:
                kill_tree(proc.pid, proc.pid if not IS_WINDOWS else None)
            except Exception:
                pass
        for proc in self._helpers + self._children:
            try:
                proc.wait(timeout=10)
            except Exception:
                pass


# ---------------------------------------------------------------------------
# grading
# ---------------------------------------------------------------------------

def grade(grader: str, evidence: str, extra: Sequence[str] = ()) -> Dict[str, Any]:
    """Run a grader as its own process and return its verdict as a dict."""
    script = os.path.join(GRADERS, grader)
    if not os.path.isfile(script):
        return {"row": grader, "state": UNPROVEN,
                "reasons": ["no grader at %s" % script], "notes": [], "observed": {}}
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    for key in SCRUB_ENV:
        env.pop(key, None)
    try:
        res = subprocess.run(
            [sys.executable, script, "--evidence", evidence, *extra],
            capture_output=True, text=True, timeout=900, check=False, env=env,
        )
    except subprocess.TimeoutExpired:
        return {"row": grader, "state": UNPROVEN,
                "reasons": ["the grader did not finish within 900 s"],
                "notes": [], "observed": {}}
    try:
        return json.loads(res.stdout)
    except ValueError:
        return {"row": grader, "state": UNPROVEN,
                "reasons": ["the grader produced no verdict (exit %s): %s"
                            % (res.returncode, (res.stderr or res.stdout)[-600:])],
                "notes": [], "observed": {}}


_STATE_MAP = {"PASS": PASS, "FAIL": FAIL, "UNPROVEN": UNPROVEN, "N/A": NA, "NOTE": NOTE}


def verdict_check(check_id: str, verdict: Dict[str, Any],
                  force_state: Optional[str] = None) -> Check:
    state = _STATE_MAP.get(str(verdict.get("state", "")).upper(), UNPROVEN)
    if force_state:
        state = force_state
    reasons = verdict.get("reasons") or []
    why = "; ".join(reasons) if reasons else "graded from world artifacts: %s" % state
    # A grader NOTE can be the most important thing in the verdict — "the order
    # was placed, but not by anything identifying itself as a browser" is a
    # PASS with a caveat, and a caveat buried in an evidence blob is a caveat
    # nobody reads.
    notes = verdict.get("notes") or []
    if notes:
        why += " [NOTE: " + "; ".join(notes) + "]"
    return Check(check_id, state, why[:4000],
                 {"observed": verdict.get("observed"), "notes": verdict.get("notes"),
                  "grader_state": verdict.get("state")})


def new_record(row_id: str, tier: str, title: str, binary: str,
               key_path: str) -> RowRecord:
    binary = os.path.abspath(binary)
    rec = RowRecord(
        row_id, binary, sha256_file(binary), tier=tier, title=title,
        key_path=os.path.abspath(key_path),
        key_sha256=sha256_file(key_path) if os.path.isfile(key_path) else None,
    )
    return rec


def finish(rec: RowRecord, artifact_dir: str) -> RowRecord:
    """Write record.json for a hand-rolled row.

    `harness.cli` only persists a record for rows driven through RowContext, so
    a `main()` row that skips this leaves nothing for `harness.cli summarise`
    to re-aggregate and its gate silently disappears from a later summary.
    """
    rec.write(os.path.join(artifact_dir, "record.json"))
    return rec


def unproven_setup(rec: RowRecord, check_id: str, exc: BaseException) -> RowRecord:
    rec.add_check(Check(check_id, UNPROVEN,
                        "the fixture could not be stood up, so the product was never "
                        "asked to do the job: %s" % exc))
    return rec
