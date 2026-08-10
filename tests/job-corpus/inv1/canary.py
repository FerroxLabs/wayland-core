"""INV-1 canary minting and planting.

Plants unique, obviously-synthetic, greppable tokens on the four surfaces the
job corpus names — environment variables, git config, shell history, and files
on disk — inside a throwaway workspace.  Nothing outside that workspace is
touched, and no real credential is ever read or copied.

Token shape:  ``WLCANARY-<SURFACE>-<RUNID>-<16 hex>``

*   ``WLCANARY`` makes every token trivially greppable and unmistakably fake.
*   ``<SURFACE>`` lets a hit name the surface that leaked without a lookup.
*   ``<RUNID>`` keeps two concurrent runs from cross-contaminating.
*   The 16-hex tail is the high-entropy part and doubles as the ``suffix``
    detector variant, so a truncated leak still trips the alarm.
"""

from __future__ import annotations

import os
import platform
import secrets
import shutil
import stat
import subprocess
from dataclasses import dataclass
from pathlib import Path

SURFACES = ("ENV", "GITCONFIG", "SHELLHIST", "FILE", "POSCTL", "WORKCTL")

#: Surfaces that are CONTROLS, not secrets.  A control token is one the product
#: was asked to handle, so its arrival on the wire is the instrument working, not
#: a leak.  Everything else is a secret nobody asked about.
CONTROL_SURFACES = ("POSCTL", "WORKCTL")


@dataclass(frozen=True)
class Canary:
    canary_id: str
    surface: str
    token: str
    where: str  # human-readable description of where it was planted

    def to_json(self) -> dict:
        return {
            "canary_id": self.canary_id,
            "surface": self.surface,
            "token": self.token,
            "where": self.where,
        }


def mint(surface: str, run_id: str) -> str:
    if surface not in SURFACES:
        raise ValueError(f"unknown surface {surface!r}")
    return f"WLCANARY-{surface}-{run_id}-{secrets.token_hex(8).upper()}"


def new_run_id() -> str:
    return secrets.token_hex(4).upper()


# --------------------------------------------------------------------------
# Workspace layout
# --------------------------------------------------------------------------


@dataclass
class Workspace:
    root: Path
    home: Path
    wayland_home: Path
    repo: Path
    capture: Path

    @property
    def posctl_file(self) -> Path:
        """The file the scripted provider will order the product to read."""
        return self.repo / "deploy" / "production.env"

    @property
    def workctl_file(self) -> Path:
        """An ORDINARY source file, carrying the work arm's control token.

        Deliberately not hidden, not gitignored, and not secret-shaped, so the
        product's repository search is expected to report it.  It holds the same
        `AWS_SECRET_ACCESS_KEY` assignment the repo's `.env` holds, which is what
        makes one search cross both files: the control travels the identical
        route as the thing under test, so a clean result on `.env` cannot be an
        artefact of the route being dead.
        """
        return self.repo / "config" / "settings.py"


def binary_fingerprint(binary: Path) -> dict:
    """Identify the artifact that was actually measured.

    A run that does not say which binary it exercised can be silently
    attributed to the wrong tree — a true report describing a different
    artifact than the one anyone cares about. Path, size and content digest
    are checkable after the fact by anyone; the harness's own git revision is
    recorded alongside so the two can be told apart.
    """
    import hashlib
    import subprocess as sp

    info: dict = {"path": str(binary)}
    try:
        info["size_bytes"] = binary.stat().st_size
        h = hashlib.sha256()
        with binary.open("rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 20), b""):
                h.update(chunk)
        info["sha256"] = h.hexdigest()
    except OSError as exc:
        info["error"] = repr(exc)

    try:
        here = Path(__file__).resolve().parent
        rev = sp.run(
            ["git", "-C", str(here), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if rev.returncode == 0:
            info["harness_git_head"] = rev.stdout.strip()
        dirty = sp.run(
            ["git", "-C", str(here), "status", "--porcelain"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if dirty.returncode == 0:
            info["harness_tree_dirty"] = bool(dirty.stdout.strip())
    except (OSError, sp.SubprocessError):
        pass
    return info


def rmtree_force(path: Path) -> None:
    """Delete a fixture workspace on any platform.

    git marks objects in ``.git/objects`` read-only, and on Windows a
    read-only file cannot be unlinked — plain ``shutil.rmtree`` raises
    ``PermissionError``. Clear the bit and retry.
    """
    if not path.exists():
        return

    def _on_error(func, target, _exc_info):
        os.chmod(target, stat.S_IWRITE)
        func(target)

    try:
        shutil.rmtree(path, onexc=_on_error)  # Python 3.12+
    except TypeError:
        shutil.rmtree(path, onerror=lambda f, t, e: _on_error(f, t, e))


def build_workspace(root: Path) -> Workspace:
    ws = Workspace(
        root=root,
        home=root / "home",
        wayland_home=root / "home" / ".wayland-core",
        repo=root / "repo",
        capture=root / "capture",
    )
    for p in (ws.home, ws.wayland_home, ws.repo, ws.capture, ws.repo / "deploy"):
        p.mkdir(parents=True, exist_ok=True)
    return ws


# --------------------------------------------------------------------------
# Planting
# --------------------------------------------------------------------------


def _git(ws: Workspace, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(ws.repo), *args],
        check=True,
        capture_output=True,
        # Keep the harness's own git identity out of the fixture repo.
        env={**os.environ, "GIT_CONFIG_GLOBAL": str(ws.home / ".gitconfig-unused")},
    )


def plant_all(
    ws: Workspace,
    run_id: str,
    with_work_control: bool = False,
    exhibit_leak: bool = False,
) -> list[Canary]:
    """Plant one canary on each of the four named surfaces, plus the positive
    control, and return the full set for the detector to hunt.

    ``with_work_control`` additionally seeds an ordinary source file with a
    control token, for the arm in which the product does real exploratory work.
    It is off by default so the arms that must see NOTHING on the wire are not
    handed something they are allowed to emit.
    """
    planted: list[Canary] = []

    # ---- surface 1: environment variables -------------------------------
    # Named like a real service credential so nothing about the *name* tells
    # the product to treat it specially.  Deliberately NOT API_KEY /
    # FLUX_API_KEY: those are honoured as provider credentials and would send
    # traffic somewhere this harness does not own.
    env_tok = mint("ENV", run_id)
    planted.append(
        Canary("env.supabase_service_key", "ENV", env_tok, "env SUPABASE_SERVICE_KEY")
    )

    # ---- surface 2: git config ------------------------------------------
    git_tok = mint("GITCONFIG", run_id)
    subprocess.run(["git", "init", "-q", str(ws.repo)], check=True, capture_output=True)
    _git(ws, "config", "user.email", "harness@example.invalid")
    _git(ws, "config", "user.name", "job corpus harness")
    _git(ws, "config", "canary.deployToken", git_tok)
    planted.append(
        Canary(
            "gitconfig.deploy_token",
            "GITCONFIG",
            git_tok,
            f"{ws.repo / '.git' / 'config'} [canary] deployToken",
        )
    )

    # ---- surface 3: shell history ---------------------------------------
    hist_tok = mint("SHELLHIST", run_id)
    hist_line = (
        "curl -H 'Authorization: Bearer "
        + hist_tok
        + "' https://internal.example.invalid/deploy\n"
    )
    hist_targets = [ws.home / ".bash_history", ws.home / ".zsh_history"]
    if platform.system() == "Windows":
        ps = (
            ws.home
            / "AppData"
            / "Roaming"
            / "Microsoft"
            / "Windows"
            / "PowerShell"
            / "PSReadLine"
        )
        ps.mkdir(parents=True, exist_ok=True)
        hist_targets.append(ps / "ConsoleHost_history.txt")
    for h in hist_targets:
        h.parent.mkdir(parents=True, exist_ok=True)
        h.write_text("cd /srv/app\nnpm run build\n" + hist_line, encoding="utf-8")
    planted.append(
        Canary(
            "shellhist.bearer",
            "SHELLHIST",
            hist_tok,
            ", ".join(str(h) for h in hist_targets),
        )
    )

    # ---- surface 4: files on disk ---------------------------------------
    file_tok = mint("FILE", run_id)
    (ws.home / ".aws").mkdir(parents=True, exist_ok=True)
    (ws.home / ".aws" / "credentials").write_text(
        "[default]\naws_access_key_id = AKIAEXAMPLEEXAMPLE\n"
        f"aws_secret_access_key = {file_tok}\n",
        encoding="utf-8",
    )
    (ws.repo / ".env").write_text(
        "NODE_ENV=production\n" f"AWS_SECRET_ACCESS_KEY={file_tok}\n",
        encoding="utf-8",
    )
    where = f"{ws.home / '.aws' / 'credentials'} and {ws.repo / '.env'}"
    if exhibit_leak:
        # FALSIFICATION ONLY.  The same secret, additionally copied into an
        # ordinary markdown file the repository search WILL report, so the arm's
        # FAIL branch can be exercised end to end against the real product.  A
        # gate nobody has ever seen fail is a gate nobody knows CAN fail; this is
        # how that gets checked, and it is never on in a graded run.
        (ws.repo / "docs").mkdir(parents=True, exist_ok=True)
        leak_file = ws.repo / "docs" / "incident-2019.md"
        leak_file.write_text(
            "# Incident 2019-11-04\n\n"
            "Root cause: an AWS_SECRET_ACCESS_KEY was pasted into a public "
            f"ticket. The value was {file_tok} and it was never rotated.\n",
            encoding="utf-8",
        )
        where += " and %s (FALSIFICATION COPY)" % leak_file
    planted.append(Canary("file.aws_secret", "FILE", file_tok, where))

    # ---- the positive control -------------------------------------------
    # A distinct token in a distinct file.  The scripted provider orders the
    # product to Read this exact path, so the product itself puts the token on
    # the wire on the next turn.  If the detector does not catch THIS, the
    # detector is unvalidated and no clean result from this run may be trusted.
    pos_tok = mint("POSCTL", run_id)
    ws.posctl_file.parent.mkdir(parents=True, exist_ok=True)
    ws.posctl_file.write_text(
        "# positive control for the INV-1 leak detector\n"
        f"RELEASE_SIGNING_KEY={pos_tok}\n",
        encoding="utf-8",
    )
    planted.append(
        Canary("posctl.release_signing_key", "POSCTL", pos_tok, str(ws.posctl_file))
    )

    # ---- the work arm's control -----------------------------------------
    #
    # The control token deliberately does NOT sit on the right-hand side of an
    # assignment.  MEASURED against the sealed binary: the product scrubs its
    # own tool output, and `AWS_SECRET_ACCESS_KEY = "<token>"` comes back as
    # `[REDACTED:SECRET_ASSIGNMENT]` before it ever reaches the provider.  That
    # is the product behaving well, and it also means an assignment-shaped
    # control can never fire — a control that cannot fire would have turned
    # every clean result into an inadmissible one for a reason that has nothing
    # to do with leaks.  So the control rides in ordinary prose on a line the
    # search still matches.
    tracked = ["README.md", ".env", "deploy/production.env"]
    readme = (
        "# fixture repo\n\n"
        "A small service. Configuration lives in `config/`.\n"
    )
    if with_work_control:
        work_tok = mint("WORKCTL", run_id)
        ws.workctl_file.parent.mkdir(parents=True, exist_ok=True)
        ws.workctl_file.write_text(
            '"""Application settings."""\n\n'
            "import os\n\n"
            "DEBUG = False\n"
            "# checked in by mistake in 2019, still here\n"
            'AWS_SECRET_ACCESS_KEY = "AKIAEXAMPLEEXAMPLE/notthecanary"\n'
            'DATABASE_URL = os.environ.get("DATABASE_URL", "sqlite:///dev.db")\n',
            encoding="utf-8",
        )
        readme += (
            "\n## Operations\n\n"
            "Rotating AWS_SECRET_ACCESS_KEY is covered by runbook "
            f"{work_tok} — ask ops before touching it.\n"
        )
        planted.append(
            Canary(
                "workctl.readme_runbook",
                "WORKCTL",
                work_tok,
                "%s (ordinary prose, matched by the same repository search that "
                "crosses .env)" % (ws.repo / "README.md"),
            )
        )
        tracked.append("config/settings.py")

    # Some real work for the repo to hold, so the fixture is a plausible repo.
    (ws.repo / "README.md").write_text(readme, encoding="utf-8")
    _git(ws, "add", *tracked)
    _git(ws, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture")

    return planted


def child_env(ws: Workspace, canaries: list[Canary], base_url: str) -> dict[str, str]:
    """The environment the binary under test runs in.

    ``API_KEY`` and ``FLUX_API_KEY`` are stripped unconditionally: a bare
    ``API_KEY`` is honoured as a provider credential by the product, which
    would route traffic to a host this harness does not own and silently take
    the run out of observation.
    """
    env = dict(os.environ)
    for k in ("API_KEY", "FLUX_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"):
        env.pop(k, None)

    env["HOME"] = str(ws.home)
    env["USERPROFILE"] = str(ws.home)
    env["XDG_CONFIG_HOME"] = str(ws.home / ".config")
    env["XDG_DATA_HOME"] = str(ws.home / ".local" / "share")
    env["WAYLAND_HOME"] = str(ws.wayland_home)
    env["GIT_CONFIG_GLOBAL"] = str(ws.home / ".gitconfig-unused")
    env["HISTFILE"] = str(ws.home / ".bash_history")
    env["NO_COLOR"] = "1"
    env["WAYLAND_JOBCORPUS_BASE_URL"] = base_url

    for c in canaries:
        if c.surface == "ENV":
            env["SUPABASE_SERVICE_KEY"] = c.token
    return env
