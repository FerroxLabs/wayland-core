"""Shared plumbing for the A-7 .. A-12 row drivers.

Three jobs, none of them grading:

1. **Point the product at a provider without leaking a credential.**  Every row
   runs under an isolated ``HOME`` and an isolated ``WAYLAND_HOME``, so the
   product inherits none of the operator's live configuration.  That also means
   it inherits no credential, and a corpus row that cannot reach a provider
   cannot ask the product to do anything.  The operator supplies one, once,
   through the environment; this module writes the throwaway config and hands
   the key to the child process only.  No credential value is ever written to a
   record, a log, or an artifact directory.

2. **Make "no provider" loud instead of silent.**  If nothing is provisioned the
   row records a single UNPROVEN check that NAMES what was missing and does not
   start the product.  It is never a PASS and never a quiet skip.

3. **Run a grader as a subprocess and turn its JSON verdict into Checks.**
   The graders under ``keys/`` already decide PASS / FAIL / UNPROVEN by reading
   the world.  A driver's job is to run the product, then hand the world to the
   grader — not to re-implement its judgement.

Environment the operator sets before a run::

    JOB_CORPUS_ENV_FILE     path to a KEY=VALUE file (e.g. holding
                            ANTHROPIC_API_KEY).  Values are passed to the
                            product and to nothing else.
    JOB_CORPUS_PROVIDER     provider id for the throwaway config  [anthropic]
    JOB_CORPUS_MODEL        model for the throwaway config
    JOB_CORPUS_BASE_URL     optional base_url override
    JOB_CORPUS_MAX_TURNS    turn ceiling written into the throwaway config [80]
    JOB_CORPUS_CONFIG_TOML  a complete config.toml to use verbatim instead of
                            the generated one (takes precedence)

Pure stdlib.  Linux / macOS / Windows.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from typing import Any, Dict, List, Optional, Sequence, Tuple

HARNESS_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if HARNESS_ROOT not in sys.path:
    sys.path.insert(0, HARNESS_ROOT)

from harness.result import FAIL, NA, NOTE, PASS, UNPROVEN, Check  # noqa: E402

CORPUS_ROOT = HARNESS_ROOT

DEFAULT_PROVIDER = "anthropic"
DEFAULT_MODEL = "claude-sonnet-4-5-20250929"

#: Variables that carry a provider credential.  They are forwarded to the
#: product and never recorded.  A bare ``API_KEY`` is deliberately absent: the
#: runner strips it, because a generic API_KEY set for some other service is
#: honoured as a provider credential and is a live exfiltration path.
CREDENTIAL_KEYS = (
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "WAYLAND_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
)

CONFIG_TEMPLATE = """\
# Written by tests/job-corpus/rows/_provision.py for one corpus row.
# Throwaway: this file is created inside the row's artifact directory and is
# deleted with it.  It carries no secret -- the credential reaches the product
# through the environment only.
[default]
provider = "{provider}"
max_turns = {max_turns}
approval_mode = "force"
read_only = false

[providers.{provider}]
provider = "{provider_kind}"
model = "{model}"
{base_url_line}
[memory]
enabled = false

[session]
enabled = true

[observability]
structured_traces = false
"""


class NotProvisioned(Exception):
    """No provider was configured, so the product cannot be asked to work."""


def _read_env_file(path: str) -> Dict[str, str]:
    """Parse a KEY=VALUE file.  Values are never logged or returned by name."""
    out: Dict[str, str] = {}
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("export "):
                line = line[len("export ") :].strip()
            if "=" not in line:
                continue
            key, value = line.split("=", 1)
            key = key.strip()
            value = value.strip().strip('"').strip("'")
            if key:
                out[key] = value
    return out


def credential_env() -> Dict[str, str]:
    """Credential variables from the operator's env file plus the ambient env."""
    found: Dict[str, str] = {}
    env_file = os.environ.get("JOB_CORPUS_ENV_FILE")
    if env_file and os.path.isfile(env_file):
        for key, value in _read_env_file(env_file).items():
            if key in CREDENTIAL_KEYS and value:
                found[key] = value
    for key in CREDENTIAL_KEYS:
        value = os.environ.get(key)
        if value and key not in found:
            found[key] = value
    return found


class Provisioning:
    """Everything a row needs to start the product, and nothing secret in it."""

    def __init__(self, wayland_home: str, extra_env: Dict[str, str], key_names: Sequence[str]):
        self.wayland_home = wayland_home
        self.extra_env = extra_env
        self.key_names = list(key_names)

    def describe(self) -> Dict[str, Any]:
        """Safe to put in a record: names only, never values."""
        return {
            "wayland_home": self.wayland_home,
            "credential_variables_supplied": sorted(self.key_names),
            "provider": os.environ.get("JOB_CORPUS_PROVIDER", DEFAULT_PROVIDER),
            "model": os.environ.get("JOB_CORPUS_MODEL", DEFAULT_MODEL),
        }


def provision(
    artifact_dir: str,
    mcp_servers: Optional[Dict[str, Dict[str, Any]]] = None,
    max_turns: int = 80,
) -> Provisioning:
    """Build a throwaway WAYLAND_HOME, or raise NotProvisioned.

    Raising is deliberate: a row that silently ran without a provider would
    report FAIL for the product's supposed incompetence when in fact nobody
    ever asked it anything.
    """
    creds = credential_env()
    override = os.environ.get("JOB_CORPUS_CONFIG_TOML")
    if not creds and not override:
        raise NotProvisioned(
            "no provider credential is available to this run. Set JOB_CORPUS_ENV_FILE "
            "to a KEY=VALUE file holding one of %s, or JOB_CORPUS_CONFIG_TOML to a "
            "complete config.toml. Without one the product is never asked to do the "
            "job, so a FAIL here would be a statement about the harness."
            % ", ".join(CREDENTIAL_KEYS[:3])
        )

    home = os.path.join(os.path.abspath(artifact_dir), "wlhome")
    os.makedirs(home, exist_ok=True)
    config_path = os.path.join(home, "config.toml")

    if override:
        shutil.copyfile(override, config_path)
    else:
        provider = os.environ.get("JOB_CORPUS_PROVIDER", DEFAULT_PROVIDER)
        model = os.environ.get("JOB_CORPUS_MODEL", DEFAULT_MODEL)
        base_url = os.environ.get("JOB_CORPUS_BASE_URL", "")
        # A turn ceiling that bites is a statement about the harness, not the
        # product: A-12 hit 40 while still reading the codebase. Generous by
        # default, and the operator can still pin it.
        max_turns = int(os.environ.get("JOB_CORPUS_MAX_TURNS", max_turns))
        text = CONFIG_TEMPLATE.format(
            provider=provider,
            provider_kind=provider,
            model=model,
            max_turns=max_turns,
            base_url_line=('base_url = "%s"\n' % base_url) if base_url else "",
        )
        if mcp_servers:
            text += "\n" + _mcp_stanzas(mcp_servers)
        with open(config_path, "w", encoding="utf-8") as fh:
            fh.write(text)
    try:
        os.chmod(config_path, 0o600)
    except OSError:
        pass

    extra_env = dict(creds)
    extra_env["WAYLAND_HOME"] = home
    # Deterministic, quiet, and never touching the operator's terminal state.
    extra_env["NO_COLOR"] = "1"
    extra_env["TERM"] = "dumb"
    return Provisioning(home, extra_env, sorted(creds))


def _toml_value(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, (list, tuple)):
        return "[" + ", ".join(_toml_value(v) for v in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join("%s = %s" % (k, _toml_value(v)) for k, v in value.items()) + " }"
    return json.dumps(str(value))


def _mcp_stanzas(servers: Dict[str, Dict[str, Any]]) -> str:
    lines: List[str] = []
    for name, cfg in servers.items():
        lines.append("[mcp.servers.%s]" % name)
        for key, value in cfg.items():
            lines.append("%s = %s" % (key, _toml_value(value)))
        lines.append("")
    return "\n".join(lines)


def reseed_baselines(ctx: Any) -> None:
    """Re-take the Tier-0 baselines after a fixture materialises itself.

    Two rows hand the product a world that only exists once the fixture's own
    setup script has run (A-8 builds a three-branch repository mid-merge).  The
    baselines RowContext takes on entry then describe the world *before* the
    fixture existed, which would make "out of scope" mean "the setup script
    created a repository" and would seal no test files at all.

    Re-seeding does not weaken anything: the seeded unsaved work stays exactly
    where it was planted, and every invariant is still graded on exit.  It moves
    the reference point to the world the user is actually sitting in front of.
    """
    if ctx._indep is not None:
        ctx._indep.seal(ctx.workspace)
    ctx._weak.seed()
    ctx.fs_before = ctx._scope.seed()
    ctx.record.world["baselines_reseeded_after_fixture_setup"] = True


def unprovisioned_check(row_id: str, exc: Exception) -> Check:
    return Check(
        row_id + ".provisioning",
        UNPROVEN,
        "the product was never started: %s" % exc,
        {"remedy": "set JOB_CORPUS_ENV_FILE or JOB_CORPUS_CONFIG_TOML before the run"},
    )


# ---------------------------------------------------------------------------
# Driving the product
# ---------------------------------------------------------------------------


def drive(
    ctx: Any,
    prompt: str,
    prov: Provisioning,
    cwd: Optional[str] = None,
    timeout: Optional[int] = None,
    extra_args: Sequence[str] = (),
) -> Any:
    """One headless product session against the fixture.

    ``--auto-approve`` stands in for the human who would otherwise press "yes"
    at each tool prompt.  It skips confirmation only; the OS sandbox stays on.
    Without it an unattended run blocks forever at the first tool call, which
    would measure the harness rather than the product.
    """
    args = ["--auto-approve", *extra_args, prompt]
    return ctx.run(args, cwd=cwd, timeout=timeout, extra_env=dict(prov.extra_env))


def reply_text(ctx: Any, rec: Any) -> str:
    """Everything the user would have seen from one session."""
    return ctx.runner.text(rec)


# ---------------------------------------------------------------------------
# Running a key grader and importing its verdict
# ---------------------------------------------------------------------------


def run_grader(
    argv: Sequence[str],
    timeout: int = 900,
    cwd: Optional[str] = None,
) -> Tuple[Optional[Dict[str, Any]], str, int]:
    """Run a grader script; return (parsed json | None, raw output, exit code)."""
    env = dict(os.environ)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    try:
        proc = subprocess.run(
            [sys.executable, *argv],
            cwd=cwd or CORPUS_ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return None, "grader timed out after %ds" % timeout, -1
    raw = proc.stdout.decode("utf-8", "replace")
    parsed: Optional[Dict[str, Any]] = None
    # The graders print one JSON document; anything before it is noise.
    start = raw.find("{")
    if start >= 0:
        try:
            parsed = json.loads(raw[start:])
        except ValueError:
            parsed = None
    return parsed, raw, proc.returncode


def grader_check(
    check_id: str,
    report: Optional[Dict[str, Any]],
    raw: str,
    pass_why: str,
    grader_name: str,
) -> Check:
    """Turn a grader's own verdict into a Check without re-judging it."""
    if report is None:
        return Check(
            check_id,
            UNPROVEN,
            "%s produced no readable verdict, so nothing about this job was "
            "established either way" % grader_name,
            {"grader_output_tail": raw[-4000:]},
        )
    verdict = str(report.get("verdict", "")).upper()
    reasons = report.get("reasons") or []
    if isinstance(reasons, str):
        reasons = [reasons]
    evidence = {"grader": grader_name, "report": report}
    if verdict == "PASS":
        return Check(check_id, PASS, pass_why, evidence)
    if verdict == "FAIL":
        return Check(
            check_id,
            FAIL,
            "; ".join(str(r) for r in reasons[:6]) or "%s returned FAIL" % grader_name,
            evidence,
        )
    if verdict == "UNPROVEN":
        return Check(
            check_id,
            UNPROVEN,
            "; ".join(str(r) for r in reasons[:6]) or "%s could not decide" % grader_name,
            evidence,
        )
    if verdict == "N/A":
        return Check(check_id, NA, "; ".join(str(r) for r in reasons[:4]) or "out of scope", evidence)
    return Check(
        check_id,
        UNPROVEN,
        "%s returned an unrecognised verdict %r" % (grader_name, verdict),
        evidence,
    )


def note(check_id: str, why: str, evidence: Optional[Dict[str, Any]] = None) -> Check:
    return Check(check_id, NOTE, why, evidence)


def session_ran_check(ctx: Any, row_id: str, records: Sequence[Any]) -> Check:
    """The product really was started and really did something.

    A driver that prepares a fixture and never runs the product is worthless,
    so every row states, from the command records the runner kept, that the
    binary was executed and did not simply die.
    """
    if not records:
        return Check(
            row_id + ".product-ran",
            UNPROVEN,
            "the harness never started the product for this row",
        )
    timed_out = [r for r in records if r.timed_out]
    crashed = [r for r in records if r.exit_code not in (0, None)]
    ev = {
        "sessions": len(records),
        "exit_codes": [r.exit_code for r in records],
        "timed_out": [r.timed_out for r in records],
        "durations_s": [r.duration_s for r in records],
    }
    if timed_out:
        return Check(
            row_id + ".product-ran",
            FAIL,
            "%d of %d sessions never finished within the time the user would wait"
            % (len(timed_out), len(records)),
            ev,
        )
    if crashed:
        return Check(
            row_id + ".product-ran",
            FAIL,
            "%d of %d sessions ended in an error exit (%s)"
            % (len(crashed), len(records), ", ".join(str(r.exit_code) for r in crashed[:5])),
            ev,
        )
    return Check(
        row_id + ".product-ran",
        PASS,
        "the product ran %d session(s) against the fixture and each one finished"
        % len(records),
        ev,
    )
