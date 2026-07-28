#!/usr/bin/env python3
"""Phase 27 Criterion 5 — deterministic packaged smoke.

Runs a fixed corpus of zero-credential probes against a PACKAGED
`wayland-core` binary (an extracted release archive, not a build-tree
binary) and grades each one PASS / FAIL / NOT MEASURED.

Design constraints taken from this program's measured traps:

* **Exit status is read from `subprocess.run(...).returncode`, never from a
  shell pipeline.** `cmd | head` reports `head`'s status; that misread has
  been recorded as a pass on this program before.
* **No probe is graded from an exit status alone.** Every probe asserts on
  captured stdout/stderr bytes and, where the operation is supposed to
  produce or withhold a file, on the filesystem.
* **The corpus is able to fail.** Probe `ollama_hint_is_honest` is expected
  RED at v0.12.25; a corpus in which every probe passes at every commit
  proves nothing, so this one is retained deliberately rather than
  softened. See `--expect-red` below.
* **A probe that cannot be taken renders NOT MEASURED, never PASS and never
  0.** Grading treats NOT MEASURED as its own state.
* **Windows exit status collapses to one bit across ssh.** This script never
  relies on its own process exit status crossing a transport: it writes
  `WLRC=<n>` as the first line and `WLDONE` as the last line of
  `--status-file`, which a separate call reads back.

The binary is run under an isolated `WAYLAND_HOME` and with every provider
credential variable stripped, so it can neither read nor write the operator's
real configuration or credentials, and no secret can reach the output.

Usage:
    python3 f27-packaged-smoke.py --binary /path/to/wayland-core \
        --out result.json --status-file status.txt
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# Credential-bearing variables scrubbed from every probe's environment.
# Stripping these is what makes the corpus deterministic AND what guarantees
# no probe can emit a secret.
CREDENTIAL_VARS = [
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "FLUX_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "TOGETHER_API_KEY",
    "BRAVE_API_KEY",
    "TAVILY_API_KEY",
    "EXA_API_KEY",
    "FIRECRAWL_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "WAYLAND_VAULT_PASSPHRASE",
]

PASS = "PASS"
FAIL = "FAIL"
NOT_MEASURED = "NOT MEASURED"


class Probe:
    """One graded observation."""

    def __init__(self, pid: str, criterion: str, what: str):
        self.id = pid
        self.criterion = criterion
        self.what = what
        self.grade = NOT_MEASURED
        self.why = "not run"
        self.rc: int | None = None
        self.stdout = ""
        self.stderr = ""

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "criterion": self.criterion,
            "what": self.what,
            "grade": self.grade,
            "why": self.why,
            "rc": self.rc,
            # Output is truncated for the record but the grade above was
            # computed against the full capture.
            "stdout_head": self.stdout[:600],
            "stderr_head": self.stderr[:600],
        }


class Runner:
    def __init__(self, binary: Path, home: Path, timeout: int):
        self.binary = binary
        self.home = home
        self.timeout = timeout
        env = {k: v for k, v in os.environ.items() if k not in CREDENTIAL_VARS}
        env["WAYLAND_HOME"] = str(home)
        env["NO_COLOR"] = "1"
        self.env = env

    def run(self, args: list[str], stdin: str | None = None):
        """Run the packaged binary. Returns (rc, stdout, stderr).

        rc is the REAL process exit status from the OS, not a shell's view of
        a pipeline. rc is None only on timeout.
        """
        try:
            p = subprocess.run(
                [str(self.binary)] + args,
                input=stdin,
                capture_output=True,
                text=True,
                timeout=self.timeout,
                env=self.env,
                cwd=str(self.home),
            )
            return p.returncode, p.stdout, p.stderr
        except subprocess.TimeoutExpired as e:
            return None, (e.stdout or b"").decode(errors="replace") if isinstance(
                e.stdout, bytes
            ) else (e.stdout or ""), "TIMEOUT"


def probe_version(r: Runner) -> Probe:
    p = Probe("version_shape", "C5", "packaged binary reports a SemVer identity")
    p.rc, p.stdout, p.stderr = r.run(["--version"])
    if p.rc != 0:
        p.grade, p.why = FAIL, f"--version exited {p.rc}"
        return p
    if re.search(r"wayland-core\s+\d+\.\d+\.\d+", p.stdout):
        p.grade, p.why = PASS, "matched `wayland-core <semver>`"
    else:
        p.grade, p.why = FAIL, "output did not match `wayland-core <semver>`"
    return p


def probe_build_info(r: Runner) -> Probe:
    p = Probe("build_provenance", "C5", "packaged binary names the source it was built from")
    p.rc, p.stdout, p.stderr = r.run(["--build-info"])
    if p.rc != 0:
        p.grade, p.why = FAIL, f"--build-info exited {p.rc}"
        return p
    m = re.search(r"source\s+([0-9a-f]{7,40})", p.stdout)
    if m:
        p.grade, p.why = PASS, f"embedded source SHA {m.group(1)}"
    else:
        p.grade, p.why = FAIL, "no embedded source SHA in --build-info output"
    return p


def probe_image_no_credential(r: Runner) -> Probe:
    """C3 built-in generation shape: refuse loudly, produce nothing.

    This is the silent-acceptance guard. A generation surface that exits 0,
    or that writes a zero-byte / placeholder file when it has no credential,
    is exactly the defect class this program is closing.
    """
    p = Probe(
        "builtin_generation_refuses_without_credential",
        "C3",
        "built-in image generation with no credential refuses loudly and writes no file",
    )
    out = r.home / "smoke-image-out.png"
    if out.exists():
        out.unlink()
    p.rc, p.stdout, p.stderr = r.run(
        ["image", "--prompt", "a red square", "--out", str(out)]
    )
    blob = p.stdout + p.stderr
    if p.rc is None:
        p.grade, p.why = NOT_MEASURED, "timed out"
        return p
    if p.rc == 0:
        p.grade, p.why = FAIL, "exited 0 with no credential (silent acceptance)"
        return p
    if out.exists():
        p.grade, p.why = FAIL, f"wrote {out.name} ({out.stat().st_size} bytes) despite failing"
        return p
    if "Flux API key" not in blob:
        p.grade, p.why = FAIL, "refusal did not name the missing credential"
        return p
    p.grade = PASS
    p.why = f"rc={p.rc}, named the missing credential, wrote no file"
    return p


def probe_image_empty_prompt(r: Runner) -> Probe:
    p = Probe(
        "builtin_generation_validates_input",
        "C3",
        "built-in image generation rejects an empty prompt before any network call",
    )
    p.rc, p.stdout, p.stderr = r.run(["image", "--prompt", "", "--out", "x.png"])
    blob = p.stdout + p.stderr
    if p.rc is None:
        p.grade, p.why = NOT_MEASURED, "timed out"
    elif p.rc == 0:
        p.grade, p.why = FAIL, "accepted an empty prompt"
    elif "non-empty" in blob or "must be" in blob:
        p.grade, p.why = PASS, f"rc={p.rc}, named the validation rule"
    else:
        p.grade, p.why = FAIL, "rejected but did not say why"
    return p


def probe_fetch_no_credential(r: Runner) -> Probe:
    p = Probe(
        "web_fetch_refuses_without_credential",
        "C3",
        "built-in web fetch with no credential refuses loudly",
    )
    p.rc, p.stdout, p.stderr = r.run(["fetch", "https://example.com"])
    blob = p.stdout + p.stderr
    if p.rc is None:
        p.grade, p.why = NOT_MEASURED, "timed out"
    elif p.rc == 0:
        p.grade, p.why = FAIL, "exited 0 with no credential (silent acceptance)"
    elif "Flux API key" in blob:
        p.grade, p.why = PASS, f"rc={p.rc}, named the missing credential"
    else:
        p.grade, p.why = FAIL, "refusal did not name the missing credential"
    return p


def probe_mcp_tools_list(r: Runner) -> Probe:
    """C3 discovery surface, reachable with zero credentials."""
    p = Probe(
        "mcp_registry_discovery",
        "C3",
        "the tool registry enumerates over MCP stdio with no credential",
    )
    req = (
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "f27-smoke", "version": "1"},
                },
            }
        )
        + "\n"
        + json.dumps({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        + "\n"
    )
    p.rc, p.stdout, p.stderr = r.run(["mcp-serve", "--transport", "stdio"], stdin=req)
    if p.rc is None:
        p.grade, p.why = NOT_MEASURED, "timed out"
        return p
    tools = None
    for line in p.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            d = json.loads(line)
        except json.JSONDecodeError:
            continue
        if d.get("id") == 2 and "result" in d:
            tools = sorted(t["name"] for t in d["result"].get("tools", []))
    if tools is None:
        p.grade, p.why = FAIL, "no tools/list response on the stream"
        return p
    p.why = f"{len(tools)} tools: {tools}"
    # Deterministic floor: the always-registered read-only trio.
    if {"Read", "Grep", "Glob"}.issubset(set(tools)):
        p.grade = PASS
    else:
        p.grade, p.why = FAIL, f"read-only trio absent; got {tools}"
    return p


def probe_json_stream_honest_init_failure(r: Runner) -> Probe:
    p = Probe(
        "host_protocol_honest_init_failure",
        "C1",
        "the host protocol emits one structured error and stops when it cannot start",
    )
    p.rc, p.stdout, p.stderr = r.run(["--json-stream"], stdin="")
    if p.rc is None:
        p.grade, p.why = NOT_MEASURED, "timed out"
        return p
    lines = [l for l in p.stdout.splitlines() if l.strip()]
    if p.rc == 0:
        p.grade, p.why = FAIL, "exited 0 with no provider credential"
        return p
    if len(lines) != 1:
        p.grade, p.why = FAIL, f"expected exactly 1 protocol frame, got {len(lines)}"
        return p
    try:
        d = json.loads(lines[0])
    except json.JSONDecodeError:
        p.grade, p.why = FAIL, "frame was not JSON"
        return p
    if d.get("type") == "error" and d.get("error", {}).get("code") == "init_failed":
        p.grade = PASS
        p.why = "one `error`/`init_failed` frame, rc=%s, retryable=%s" % (
            p.rc,
            d.get("error", {}).get("retryable"),
        )
    else:
        p.grade, p.why = FAIL, f"unexpected frame: {lines[0][:200]}"
    return p


def probe_ollama_hint_is_honest(r: Runner) -> Probe:
    """The binary's own remediation text, followed verbatim.

    On a missing credential the engine prints, verbatim:

        To use a LOCAL model with Ollama, select a model id prefixed with
        `ollama:` (e.g. `ollama:qwen3-coder:30b`) -- no API key is needed.

    This probe does exactly that and grades whether the instruction is true.
    It is EXPECTED RED at v0.12.25 and is retained deliberately: a corpus in
    which every probe is green at every commit cannot fail, and therefore
    proves nothing.
    """
    p = Probe(
        "ollama_hint_is_honest",
        "C2",
        "the engine's own no-credential remediation instruction actually works",
    )
    p.rc, p.stdout, p.stderr = r.run(
        ["--json-stream", "-m", "ollama:qwen3-coder:30b"], stdin=""
    )
    blob = p.stdout + p.stderr
    if p.rc is None:
        p.grade, p.why = NOT_MEASURED, "timed out"
        return p
    if "No API key found" in blob or "requires an API key" in blob:
        p.grade = FAIL
        p.why = (
            "followed the engine's own instruction verbatim and it still "
            "failed with MissingApiKey -- the advertised credential-free "
            "path does not exist"
        )
        return p
    p.grade = PASS
    p.why = f"rc={p.rc}; the advertised credential-free path was reachable"
    return p


def probe_home_isolation(r: Runner) -> Probe:
    p = Probe(
        "profile_isolation_holds",
        "C5",
        "WAYLAND_HOME confines config resolution to the isolated profile",
    )
    p.rc, p.stdout, p.stderr = r.run(["--config-path"])
    if p.rc != 0:
        p.grade, p.why = FAIL, f"--config-path exited {p.rc}"
        return p
    resolved = p.stdout.strip()
    try:
        inside = Path(resolved).resolve().is_relative_to(r.home.resolve())
    except (OSError, ValueError):
        inside = False
    if inside:
        p.grade, p.why = PASS, "config path resolved inside the isolated home"
    else:
        p.grade, p.why = FAIL, f"config path escaped the isolated home: {resolved}"
    return p


PROBES = [
    probe_version,
    probe_build_info,
    probe_home_isolation,
    probe_image_no_credential,
    probe_image_empty_prompt,
    probe_fetch_no_credential,
    probe_mcp_tools_list,
    probe_json_stream_honest_init_failure,
    probe_ollama_hint_is_honest,
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--out", required=True, help="JSON result path")
    ap.add_argument("--status-file", help="WLRC/WLDONE status file for ssh transports")
    ap.add_argument("--timeout", type=int, default=90)
    ap.add_argument(
        "--expect-red",
        default="",
        help="comma-separated probe ids permitted to be RED without failing the run",
    )
    args = ap.parse_args()

    binary = Path(args.binary).resolve()
    expect_red = {s for s in args.expect_red.split(",") if s}

    if not binary.exists():
        print(f"FATAL: packaged binary not found at {binary}", file=sys.stderr)
        return 3

    home = Path(tempfile.mkdtemp(prefix="f27-smoke-home-"))
    runner = Runner(binary, home, args.timeout)
    results = []
    try:
        for fn in PROBES:
            pr = fn(runner)
            results.append(pr)
            print(f"[{pr.grade:>12}] {pr.id}: {pr.why}", flush=True)
    finally:
        shutil.rmtree(home, ignore_errors=True)

    npass = sum(1 for p in results if p.grade == PASS)
    nfail = sum(1 for p in results if p.grade == FAIL)
    nnm = sum(1 for p in results if p.grade == NOT_MEASURED)
    unexpected_red = [p.id for p in results if p.grade == FAIL and p.id not in expect_red]

    payload = {
        "binary": str(binary),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "counts": {
            "total": len(results),
            "pass": npass,
            "fail": nfail,
            "not_measured": nnm,
        },
        "expected_red": sorted(expect_red),
        "unexpected_red": unexpected_red,
        "probes": [p.to_dict() for p in results],
    }
    Path(args.out).write_text(json.dumps(payload, indent=2), encoding="utf-8")

    print(
        f"\nCOUNTS total={len(results)} pass={npass} fail={nfail} not_measured={nnm}",
        flush=True,
    )
    print(f"UNEXPECTED_RED={unexpected_red}", flush=True)

    rc = 0 if not unexpected_red else 1
    if args.status_file:
        # WLRC first, WLDONE last -- a truncated write is detectable.
        Path(args.status_file).write_text(
            f"WLRC={rc}\nPASS={npass}\nFAIL={nfail}\nNOT_MEASURED={nnm}\n"
            f"UNEXPECTED_RED={','.join(unexpected_red)}\nWLDONE\n",
            encoding="utf-8",
        )
    return rc


if __name__ == "__main__":
    sys.exit(main())
