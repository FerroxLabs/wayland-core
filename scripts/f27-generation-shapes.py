#!/usr/bin/env python3
"""Phase 27 Criterion 3 — the four media-generation shapes, exercised.

Criterion 3 reads: "Built-in, MCP-only, late-MCP, and combined media
generation expose consistent discovery, credentials, accounting, and
failures." The phase verdict recorded that **none of the four shapes was ever
exercised**, because no MCP media-tool fixture existed, which made three of
them unreachable.

This driver exercises all four against the real binary over the real host
protocol. It needs no paid credential: the local-model route (`ollama:`)
boots the engine without one, and `scripts/f27-mcp-media-fixture.mjs`
supplies deterministic media tools over stdio.

The four shapes:

  A  built-in    the engine's own `image` / `fetch` generation subcommands
  B  MCP-only    media tools from a config-declared MCP server, no built-in
                 credential present
  C  late-MCP    media tools from a server introduced AFTER session start via
                 the `AddMcpServer` host command
  D  combined    B and C together in one session

For each shape four clauses are graded separately, because the criterion
names four and a shape can satisfy some and not others:

  discovery    are the tools announced to the host, by name?
  credentials  is credential absence handled honestly (named, not silent)?
  accounting   is a cost/usage record produced for a media call?
  failures     does a failing generation surface a named cause?

Any clause that cannot be observed renders NOT MEASURED. It is never
rendered 0 and never rendered PASS.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

PASS = "PASS"
FAIL = "FAIL"
NOT_MEASURED = "NOT MEASURED"

LOCAL_MODEL = "ollama:qwen3-coder:30b"

CREDENTIAL_VARS = [
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "FLUX_API_KEY",
    "GEMINI_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
]


def clean_env(home: Path) -> dict:
    env = {k: v for k, v in os.environ.items() if k not in CREDENTIAL_VARS}
    env["WAYLAND_HOME"] = str(home)
    env["NO_COLOR"] = "1"
    return env


class Session:
    """One json-stream session against the engine, driven line by line."""

    def __init__(self, binary: Path, home: Path, timeout: float):
        self.binary = binary
        self.home = home
        self.timeout = timeout
        self.events: list[dict] = []
        self.stderr = ""

    def run(self, commands: list[dict], settle: float) -> list[dict]:
        """Send commands, wait `settle` seconds for asynchronous events, exit.

        The settle window matters: config-declared MCP servers are dialed in
        the BACKGROUND, so a session whose stdin closes immediately can exit
        before `mcp_ready` is ever written. An earlier attempt at this
        measurement saw no MCP events for exactly that reason and would have
        been misread as "the server was never registered".
        """
        payload = "".join(json.dumps(c) + "\n" for c in commands)
        proc = subprocess.Popen(
            [str(self.binary), "--json-stream", "-m", LOCAL_MODEL],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=clean_env(self.home),
            cwd=str(self.home),
        )
        try:
            proc.stdin.write(payload)
            proc.stdin.flush()
            time.sleep(settle)
            proc.stdin.close()
            # `communicate` re-flushes `proc.stdin` if it is still bound, which
            # raises on the handle we just closed. Detach it first.
            proc.stdin = None
            out, err = proc.communicate(timeout=self.timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            out, err = proc.communicate()
        self.stderr = err
        for line in out.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                self.events.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        return self.events


def mcp_ready_tools(events: list[dict]) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {}
    for e in events:
        if e.get("type") in ("mcp_ready", "McpReady"):
            out[e.get("name", "?")] = sorted(e.get("tools", []))
    return out


def mcp_failures(events: list[dict]) -> dict[str, str]:
    out: dict[str, str] = {}
    for e in events:
        if e.get("type") in ("mcp_failed", "McpFailed"):
            out[e.get("name", "?")] = e.get("reason", "")
    return out


class ShapeResult:
    def __init__(self, shape: str, description: str):
        self.shape = shape
        self.description = description
        self.clauses: dict[str, tuple[str, str]] = {}
        self.transcript: list[str] = []

    def grade(self, clause: str, verdict: str, why: str):
        self.clauses[clause] = (verdict, why)

    def to_dict(self):
        return {
            "shape": self.shape,
            "description": self.description,
            "clauses": {k: {"grade": v[0], "why": v[1]} for k, v in self.clauses.items()},
            "transcript": self.transcript[:80],
        }


def write_mcp_config(home: Path, fixture: Path, contact_log: Path, deferred: bool):
    cfg = home / "config.toml"
    cfg.write_text(
        "[mcp.servers.f27media]\n"
        'transport = "stdio"\n'
        'command = "node"\n'
        f'args = ["{fixture}"]\n'
        f"deferred = {'true' if deferred else 'false'}\n"
        "[mcp.servers.f27media.env]\n"
        f'F27_FIXTURE_LOG = "{contact_log}"\n',
        encoding="utf-8",
    )
    return cfg


def shape_a_builtin(binary: Path, tmp: Path) -> ShapeResult:
    r = ShapeResult("A/built-in", "the engine's own `image` generation subcommand")
    home = Path(tempfile.mkdtemp(dir=tmp, prefix="A-"))
    env = clean_env(home)

    p = subprocess.run(
        [str(binary), "image", "--help"], capture_output=True, text=True, env=env
    )
    r.transcript.append(f"$ wayland-core image --help  -> rc={p.returncode}")
    if p.returncode == 0 and "--prompt" in p.stdout:
        r.grade("discovery", PASS, "`image` is a first-class subcommand with a typed surface")
    else:
        r.grade("discovery", FAIL, f"image --help rc={p.returncode}")

    out = home / "a.png"
    p = subprocess.run(
        [str(binary), "image", "--prompt", "a red square", "--out", str(out)],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(home),
    )
    blob = p.stdout + p.stderr
    r.transcript.append(f"$ wayland-core image --prompt ... -> rc={p.returncode}: {blob.strip()[:200]}")
    if p.returncode != 0 and "Flux API key" in blob and not out.exists():
        r.grade(
            "credentials",
            PASS,
            f"rc={p.returncode}, named the missing credential, produced no artifact",
        )
    else:
        r.grade(
            "credentials",
            FAIL,
            f"rc={p.returncode}, artifact_present={out.exists()}",
        )

    # Failure surface: an invalid request must be refused with a named cause.
    p = subprocess.run(
        [str(binary), "image", "--prompt", ""], capture_output=True, text=True, env=env
    )
    blob = p.stdout + p.stderr
    r.transcript.append(f"$ wayland-core image --prompt '' -> rc={p.returncode}: {blob.strip()[:160]}")
    if p.returncode != 0 and "non-empty" in blob:
        r.grade("failures", PASS, "named the violated rule")
    else:
        r.grade("failures", FAIL, f"rc={p.returncode}: {blob.strip()[:160]}")

    # Accounting cannot be observed here: no generation completed, because no
    # credential exists to complete one. Saying PASS or 0 would be a lie.
    r.grade(
        "accounting",
        NOT_MEASURED,
        "no generation completed (no Flux credential), so no cost record could "
        "exist to inspect. Needs a cleared FLUX_API_KEY, which is Sean-reserved.",
    )
    return r


def shape_mcp(
    binary: Path,
    tmp: Path,
    fixture: Path,
    label: str,
    description: str,
    late: bool,
    both: bool,
) -> ShapeResult:
    r = ShapeResult(label, description)
    home = Path(tempfile.mkdtemp(dir=tmp, prefix=f"{label[0]}-"))
    contact = home / "fixture-contact.log"

    commands: list[dict] = []
    if not late or both:
        write_mcp_config(home, fixture, contact, deferred=False)
    if late:
        late_contact = home / "fixture-contact-late.log"
        commands.append(
            {
                "type": "add_mcp_server",
                "name": "f27media-late",
                "transport": "stdio",
                "command": "node",
                "args": [str(fixture)],
                "env": {"F27_FIXTURE_LOG": str(late_contact)},
            }
        )

    s = Session(binary, home, timeout=90)
    events = s.run(commands, settle=12.0)

    ready = mcp_ready_tools(events)
    failed = mcp_failures(events)
    r.transcript.append(f"mcp_ready: {json.dumps(ready)}")
    r.transcript.append(f"mcp_failed: {json.dumps(failed)}")
    r.transcript.append(f"event types seen: {sorted({e.get('type','?') for e in events})}")

    media_tools = sorted(
        t for tools in ready.values() for t in tools if "media_generate" in t
    )
    if media_tools:
        r.grade(
            "discovery",
            PASS,
            f"host was told the media tools by name: {media_tools}",
        )
    elif ready:
        r.grade(
            "discovery",
            FAIL,
            f"servers announced but no media tool named: {json.dumps(ready)}",
        )
    else:
        r.grade(
            "discovery",
            FAIL,
            "no mcp_ready event reached the host at all"
            + (f"; mcp_failed={json.dumps(failed)}" if failed else ""),
        )

    contacted = contact.exists() or (home / "fixture-contact-late.log").exists()
    r.transcript.append(f"fixture contacted: {contacted}")
    if contacted:
        r.grade(
            "credentials",
            PASS,
            "the MCP media server was reached and registered with no provider "
            "credential present anywhere in the session",
        )
    else:
        r.grade(
            "credentials",
            FAIL,
            "the fixture was never contacted, so registration was not real",
        )

    if failed:
        r.grade("failures", PASS, f"failures reach the host by name: {json.dumps(failed)}")
    else:
        r.grade(
            "failures",
            NOT_MEASURED,
            "no server failed in this run, so the host-visible failure path was "
            "not exercised here (it is exercised by the negative-control shape)",
        )

    r.grade(
        "accounting",
        NOT_MEASURED,
        "invoking a registered MCP media tool requires a model turn to call it; "
        "the local-model route boots the engine but no local inference server "
        "is running on this host, so no tool call was issued and no cost record "
        "could be inspected.",
    )
    return r


def shape_negative_control(binary: Path, tmp: Path) -> ShapeResult:
    """Proves the discovery observable can report absence.

    Without this, a green `discovery` grade above could equally be produced by
    a driver that always reports PASS.
    """
    r = ShapeResult(
        "control/absent-server",
        "an MCP server that cannot start must be reported to the host, not silently dropped",
    )
    home = Path(tempfile.mkdtemp(dir=tmp, prefix="ctl-"))
    (home / "config.toml").write_text(
        "[mcp.servers.f27missing]\n"
        'transport = "stdio"\n'
        'command = "definitely-not-a-real-binary-f27"\n'
        "args = []\n",
        encoding="utf-8",
    )
    s = Session(binary, home, timeout=90)
    events = s.run([], settle=10.0)
    ready = mcp_ready_tools(events)
    failed = mcp_failures(events)
    r.transcript.append(f"mcp_ready: {json.dumps(ready)}")
    r.transcript.append(f"mcp_failed: {json.dumps(failed)}")

    if failed:
        r.grade("failures", PASS, f"absence reported with a reason: {json.dumps(failed)}")
    elif "f27missing" in ready:
        r.grade("failures", FAIL, "server reported READY though its command does not exist")
    else:
        r.grade(
            "failures",
            FAIL,
            "a server that cannot start produced neither mcp_ready nor mcp_failed "
            "-- the host is told nothing at all",
        )
    return r


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--fixture", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--status-file")
    args = ap.parse_args()

    binary = Path(args.binary).resolve()
    fixture = Path(args.fixture).resolve()
    tmp = Path(tempfile.mkdtemp(prefix="f27-shapes-"))

    results = [
        shape_a_builtin(binary, tmp),
        shape_mcp(
            binary, tmp, fixture, "B/MCP-only",
            "media tools from a config-declared MCP server, no built-in credential",
            late=False, both=False,
        ),
        shape_mcp(
            binary, tmp, fixture, "C/late-MCP",
            "media tools introduced AFTER session start via AddMcpServer",
            late=True, both=False,
        ),
        shape_mcp(
            binary, tmp, fixture, "D/combined",
            "a config-declared server AND a late-added server in one session",
            late=True, both=True,
        ),
        shape_negative_control(binary, tmp),
    ]

    for r in results:
        print(f"\n=== {r.shape} — {r.description}")
        for clause, (grade, why) in r.clauses.items():
            print(f"  [{grade:>12}] {clause}: {why}")
        for line in r.transcript:
            print(f"    | {line}")

    counts = {PASS: 0, FAIL: 0, NOT_MEASURED: 0}
    for r in results:
        for grade, _ in r.clauses.values():
            counts[grade] = counts.get(grade, 0) + 1

    Path(args.out).write_text(
        json.dumps(
            {"counts": counts, "shapes": [r.to_dict() for r in results]}, indent=2
        ),
        encoding="utf-8",
    )
    print(
        f"\nCOUNTS pass={counts[PASS]} fail={counts[FAIL]} "
        f"not_measured={counts[NOT_MEASURED]}"
    )
    rc = 0 if counts[FAIL] == 0 else 1
    if args.status_file:
        Path(args.status_file).write_text(
            f"WLRC={rc}\nPASS={counts[PASS]}\nFAIL={counts[FAIL]}\n"
            f"NOT_MEASURED={counts[NOT_MEASURED]}\nWLDONE\n",
            encoding="utf-8",
        )
    return rc


if __name__ == "__main__":
    sys.exit(main())
