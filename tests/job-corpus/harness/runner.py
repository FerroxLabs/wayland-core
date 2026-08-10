"""Black-box runner for the product binary.

Launches the built binary as a subprocess against a prepared fixture
workspace, captures stdout/stderr/exit code, enforces a per-row timeout, and
records the binary's sha256 with every row it grades.

It imports nothing from Wayland Core.  It knows the product only as an
executable file plus argv.

Pure stdlib, Python 3.8+, Linux / macOS / Windows.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import time
import uuid
from typing import Any, Dict, List, Optional, Sequence

from .result import CommandRecord, HarnessError, RowRecord
from .world import IS_WINDOWS, ProcessTable, kill_tree, sha256_bytes, sha256_file

#: Credentials that must never reach the product under test.  A bare API_KEY
#: is honoured as a provider credential, which both contaminates "isolated"
#: runs and is a live exfiltration path, so the runner strips it by default
#: rather than trusting each row to remember.
SCRUBBED_ENV = ("API_KEY", "FLUX_API_KEY")


class RunnerError(HarnessError):
    pass


class RowRunner:
    """Owns one row's execution: workspace, artifact dir, subprocesses, record."""

    def __init__(
        self,
        row_id: str,
        binary: str,
        workspace: str,
        artifact_dir: str,
        timeout: int = 900,
        tier: str = "",
        title: str = "",
        env: Optional[Dict[str, str]] = None,
        scrub_env: Sequence[str] = SCRUBBED_ENV,
    ) -> None:
        binary = os.path.abspath(binary)
        if not os.path.isfile(binary):
            raise RunnerError(
                "binary not found at %r; a result that cannot name the artifact "
                "it ran is void" % binary
            )
        if not os.access(binary, os.X_OK) and not IS_WINDOWS:
            raise RunnerError("binary at %r is not executable" % binary)

        self.row_id = row_id
        self.binary = binary
        self.binary_sha256 = sha256_file(binary)
        self.workspace = os.path.abspath(workspace)
        self.artifact_dir = os.path.abspath(artifact_dir)
        os.makedirs(self.artifact_dir, exist_ok=True)
        self.timeout = timeout
        self.nonce = "jcnonce-%s-%s" % (row_id.replace("/", "-"), uuid.uuid4().hex[:12])
        self.base_env = dict(os.environ if env is None else env)
        for key in scrub_env:
            self.base_env.pop(key, None)
        self.scrubbed = list(scrub_env)
        # Rows are graded by what the user got, so the product is pointed at a
        # throwaway HOME: a row must not inherit a developer's live config.
        self.home = os.path.join(self.artifact_dir, "home")
        os.makedirs(self.home, exist_ok=True)

        self.record = RowRecord(row_id, self.binary, self.binary_sha256, tier=tier, title=title)
        self.record.world["nonce"] = self.nonce
        self.record.world["workspace"] = self.workspace
        self.record.world["artifact_dir"] = self.artifact_dir
        self._pre_procs = {e.pid for e in ProcessTable.take().entries}
        self.launched: List[Dict[str, int]] = []
        self._seq = 0

    # -- environment -----------------------------------------------------
    def env_for(self, extra: Optional[Dict[str, str]] = None, isolate_home: bool = True) -> Dict[str, str]:
        env = dict(self.base_env)
        env["JOB_CORPUS_ROW"] = self.row_id
        env["JOB_CORPUS_NONCE"] = self.nonce
        if isolate_home:
            env["HOME"] = self.home
            env["USERPROFILE"] = self.home
            env["XDG_CONFIG_HOME"] = os.path.join(self.home, ".config")
            env["XDG_DATA_HOME"] = os.path.join(self.home, ".local", "share")
            env["XDG_CACHE_HOME"] = os.path.join(self.home, ".cache")
            for d in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
                os.makedirs(env[d], exist_ok=True)
        if extra:
            env.update(extra)
        for key in self.scrubbed:
            env.pop(key, None)
        return env

    # -- execution -------------------------------------------------------
    def run(
        self,
        args: Sequence[str],
        stdin: Optional[str] = None,
        timeout: Optional[int] = None,
        cwd: Optional[str] = None,
        extra_env: Optional[Dict[str, str]] = None,
        role: str = "product",
        binary: Optional[str] = None,
        isolate_home: bool = True,
    ) -> CommandRecord:
        """Run the product (or, with `binary=`, a harness-owned helper)."""
        exe = binary or self.binary
        argv = [exe, *args]
        cwd = os.path.abspath(cwd or self.workspace)
        env = self.env_for(extra_env, isolate_home=isolate_home)
        limit = self.timeout if timeout is None else timeout

        self._seq += 1
        stem = os.path.join(self.artifact_dir, "cmd-%02d" % self._seq)
        out_path, err_path = stem + ".stdout", stem + ".stderr"

        popen_kwargs: Dict[str, Any] = {}
        if IS_WINDOWS:
            popen_kwargs["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        else:
            popen_kwargs["start_new_session"] = True

        started = time.time()
        timed_out = False
        with open(out_path, "wb") as fout, open(err_path, "wb") as ferr:
            proc = subprocess.Popen(
                argv,
                cwd=cwd,
                env=env,
                stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                stdout=fout,
                stderr=ferr,
                **popen_kwargs,
            )
            pgid = proc.pid if not IS_WINDOWS else -1
            self.launched.append({"pid": proc.pid, "pgid": pgid})
            try:
                proc.communicate(
                    input=stdin.encode("utf-8") if stdin is not None else None, timeout=limit
                )
            except subprocess.TimeoutExpired:
                timed_out = True
                kill_tree(proc.pid, pgid if pgid > 0 else None)
                try:
                    proc.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    pass
        duration = time.time() - started

        rec = CommandRecord(
            argv=argv,
            cwd=cwd,
            exit_code=proc.returncode,
            timed_out=timed_out,
            duration_s=round(duration, 3),
            stdout_path=out_path,
            stderr_path=err_path,
            stdout_sha256=sha256_file(out_path),
            stderr_sha256=sha256_file(err_path),
            role=role,
            env_scrubbed=list(self.scrubbed),
        )
        self.record.add_command(rec)
        return rec

    # -- captured output -------------------------------------------------
    @staticmethod
    def text(rec: CommandRecord) -> str:
        parts = []
        for p in (rec.stdout_path, rec.stderr_path):
            if p and os.path.exists(p):
                with open(p, "rb") as fh:
                    parts.append(fh.read().decode("utf-8", "replace"))
        return "\n".join(parts)

    def all_output(self, role: Optional[str] = "product") -> str:
        return "\n".join(
            self.text(c) for c in self.record.commands if role is None or c.role == role
        )

    # -- teardown --------------------------------------------------------
    def survivors(self) -> List[Dict[str, Any]]:
        """Processes belonging to this row that are still alive."""
        table = ProcessTable.take()
        found: Dict[int, Any] = {}
        for handle in self.launched:
            for e in table.descendants(
                root_pid=handle["pid"],
                pgid=handle["pgid"] if handle["pgid"] > 0 else None,
                exclude_pids=self._pre_procs,
            ):
                found[e.pid] = e
        for e in table.descendants(nonce=self.nonce, exclude_pids=self._pre_procs):
            found[e.pid] = e
        return [found[p].to_dict() for p in sorted(found)]

    def reap(self) -> List[Dict[str, Any]]:
        """Record survivors, then kill them so they cannot poison the next row."""
        left = self.survivors()
        for handle in self.launched:
            kill_tree(handle["pid"], handle["pgid"] if handle["pgid"] > 0 else None)
        for entry in left:
            try:
                kill_tree(int(entry["pid"]))
            except (KeyError, ValueError, OSError):
                pass
        return left


def prepare_workspace(fixture_dir: str, dest: str) -> str:
    """Copy a fixture into a throwaway workspace and return its path."""
    dest = os.path.abspath(dest)
    if os.path.isdir(dest):
        shutil.rmtree(dest)
    shutil.copytree(fixture_dir, dest, symlinks=True)
    return dest


def scratch_dir(prefix: str = "jobcorpus-") -> str:
    return tempfile.mkdtemp(prefix=prefix)
