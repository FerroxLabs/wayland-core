#!/usr/bin/env python3
"""Phase 27 Criterion 3 — the cells `f27-generation-shapes.py` left NOT MEASURED.

The prior lane exercised all four generation shapes and closed `discovery` and
`credentials`. Seven cells stayed NOT MEASURED for two named reasons, both of
which this driver removes:

  * `accounting`, shapes A/B/C/D — shape A needed a cleared `FLUX_API_KEY`;
    shapes B/C/D registered the MCP media tools but **invoking** one needs a
    model turn, and no inference server ran on the measurement host.
  * `failures`, shapes B/C/D — no server failed inside those runs, so each
    shape borrowed the negative control's result rather than producing its own.

Both are answered by the same credential: `flux-router` is an OpenAI-compatible
**inference** provider as well as the image vendor, so it supplies the model
turn that issues the tool call. `media_generate_locked` in the fixture is the
in-shape failure.

WHAT "ACCOUNTING" MEANS HERE, precisely, because the word is doing real work:
the criterion asks whether a media generation produces a **cost record**. Three
distinct things are therefore graded separately and never conflated:

  turn_cost   did the session emit a cost record for the model turn at all?
              (proves the accounting channel exists and is live)
  media_cost  is any cost attributable to the MEDIA CALL itself?
  tool_called was the media tool actually invoked? (without this, an
              `accounting` grade of any kind would be vacuous)

A shape where the tool was never called renders NOT MEASURED for accounting.
It is never rendered 0 and never rendered PASS.

NO CREDENTIAL VALUE IS WRITTEN, PRINTED OR LOGGED. The key is read from the
environment, injected into the child process environment only, and every
captured string is passed through `redact()` before it reaches stdout or the
JSON record.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

PASS = "PASS"
FAIL = "FAIL"
NOT_MEASURED = "NOT MEASURED"

# The routing alias used for the model turn. Deliberately the cheapest arm.
# NOTE (measured 2026-07-29): `flux-fast` is a REASONING model. A 16-token
# budget returns HTTP 200 with empty content and every token spent as
# `reasoning_tokens`. Budget generously or starvation reads as a defect.
#
# NOTE (measured 2026-07-29): the `provider:model` prefix form that works for
# `ollama:` does NOT work here. `-m flux-router:flux-fast` with a populated
# `[providers.flux-router]` block boots into provider `anthropic` and dies with
# `init_failed: No API key found ... (API_KEY, ANTHROPIC_API_KEY, or
# OPENAI_API_KEY)` -- naming neither Flux nor the key that was present. The
# working form is an explicit provider: `-p flux-router -m flux-fast`, plus a
# `[default] provider` block.
TURN_PROVIDER = "flux-router"
TURN_MODEL = "flux-fast"

_KEY = os.environ.get("FLUX_API_KEY", "")


def redact(s: str) -> str:
    """Strip the credential from anything about to be emitted.

    Belt and braces: the literal key, and any long bearer-shaped token.
    """
    if not s:
        return s
    if _KEY:
        s = s.replace(_KEY, "<REDACTED-KEY>")
    s = re.sub(r"(?i)(bearer\s+)[A-Za-z0-9_\-\.]{16,}", r"\1<REDACTED-KEY>", s)
    return s


def child_env(home: Path) -> dict:
    env = dict(os.environ)
    env["WAYLAND_HOME"] = str(home)
    env["NO_COLOR"] = "1"
    # Measured 2026-07-29 on headless `hetzner-dsm`: without this the FIRST
    # turn dies with `engine_error: Session persistence authority unavailable:
    # ... no OS keyring was usable and no encrypted credentials vault is
    # unlocked`, and `stream_end` reports `finish_reason: error` with zero
    # tokens. The engine boots, MCP servers connect and `mcp_ready` fires, so
    # every discovery observable looks healthy while no turn can ever run.
    # This is a throwaway passphrase for a throwaway home, not a credential.
    env["WAYLAND_VAULT_PASSPHRASE"] = "f27-credentialled-throwaway"
    return env


class Session:
    """One json-stream session against the engine, driven line by line."""

    def __init__(self, binary: Path, home: Path, timeout: float):
        self.binary = binary
        self.home = home
        self.timeout = timeout
        self.events: list[dict] = []
        self.stderr = ""
        self.raw_lines: list[str] = []

    def run(self, commands: list[dict], settle: float) -> list[dict]:
        payload = "".join(json.dumps(c) + "\n" for c in commands)
        proc = subprocess.Popen(
            [
                str(self.binary),
                "--json-stream",
                "-p",
                TURN_PROVIDER,
                "-m",
                TURN_MODEL,
                "--assistant",
                "f27-shapes",
                "--force",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=child_env(self.home),
            cwd=str(self.home),
        )
        try:
            proc.stdin.write(payload)
            proc.stdin.flush()
            time.sleep(settle)
            proc.stdin.close()
            proc.stdin = None
            out, err = proc.communicate(timeout=self.timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            out, err = proc.communicate()
        self.stderr = redact(err)
        for line in out.splitlines():
            line = line.strip()
            if not line:
                continue
            self.raw_lines.append(redact(line))
            try:
                self.events.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        return self.events


def write_config(home: Path, fixture: Path, contact_log: Path, with_mcp: bool) -> Path:
    """Config with the flux-router provider, and optionally the MCP fixture.

    The provider block is what supplies the model turn. `api_key` is read from
    the environment at write time and IS written into this throwaway config
    inside a `mktemp` home that is never collected as evidence -- the evidence
    capture only ever sees `redact()`ed strings.
    """
    lines = [
        "[default]\n",
        f'provider = "{TURN_PROVIDER}"\n',
        f'model = "{TURN_MODEL}"\n',
        "max_tokens = 2000\n",
        "[providers.flux-router]\n",
        f'api_key = "{_KEY}"\n',
        'base_url = "https://api.fluxrouter.ai/v1"\n',
    ]
    if with_mcp:
        lines += [
            "[mcp.servers.f27media]\n",
            'transport = "stdio"\n',
            'command = "node"\n',
            f'args = ["{fixture}"]\n',
            "deferred = false\n",
            "[mcp.servers.f27media.env]\n",
            f'F27_FIXTURE_LOG = "{contact_log}"\n',
        ]
    cfg = home / "config.toml"
    cfg.write_text("".join(lines), encoding="utf-8")
    return cfg


def collect(events: list[dict], types: set[str]) -> list[dict]:
    return [e for e in events if e.get("type") in types]


def find_cost_records(events: list[dict]) -> list[dict]:
    """Every event carrying a cost-shaped field, wherever it sits.

    Searched recursively rather than by a fixed key path, because the protocol
    puts per-turn cost inside `TraceEvent.trace.cost_usd` and session cost in a
    separate `session_cost` variant. A fixed path would miss one of them and
    report absence -- the under-detection class this program keeps hitting.
    """
    hits = []

    def walk(node, path):
        if isinstance(node, dict):
            for k, v in node.items():
                if isinstance(v, (int, float)) and (
                    "cost" in k.lower() or k.lower() in ("usd",)
                ):
                    hits.append({"event_type": path[0], "path": ".".join(path[1:] + [k]), "value": v})
                walk(v, path + [k])
        elif isinstance(node, list):
            for i, v in enumerate(node):
                walk(v, path + [str(i)])

    for e in events:
        walk(e, [e.get("type", "?")])
    return hits


def tool_calls(events: list[dict]) -> list[str]:
    """Tool names the engine actually invoked, from any tool-shaped event."""
    names = []
    for e in events:
        t = (e.get("type") or "").lower()
        if "tool" not in t:
            continue
        for key in ("name", "tool", "tool_name"):
            v = e.get(key)
            if isinstance(v, str) and v:
                names.append(f"{e.get('type')}:{v}")
                break
    return names


class ShapeResult:
    def __init__(self, shape: str, description: str):
        self.shape = shape
        self.description = description
        self.clauses: dict[str, tuple[str, str]] = {}
        self.transcript: list[str] = []

    def grade(self, clause: str, verdict: str, why: str):
        self.clauses[clause] = (verdict, redact(why))

    def note(self, s: str):
        self.transcript.append(redact(s))

    def to_dict(self):
        return {
            "shape": self.shape,
            "description": self.description,
            "clauses": {k: {"grade": v[0], "why": v[1]} for k, v in self.clauses.items()},
            "transcript": self.transcript[:120],
        }


def drive_shape(
    binary: Path,
    tmp: Path,
    fixture: Path,
    label: str,
    description: str,
    late: bool,
    both: bool,
    settle: float,
) -> ShapeResult:
    r = ShapeResult(label, description)
    home = Path(tempfile.mkdtemp(dir=tmp, prefix=f"{label[0]}c-"))
    contact = home / "fixture-contact.log"
    late_contact = home / "fixture-contact-late.log"

    write_config(home, fixture, contact, with_mcp=(not late) or both)

    commands: list[dict] = []
    if late:
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
    # Two turns: one that must SUCCEED, one that must FAIL the way a
    # paid-but-uncleared arm fails. The second is this shape's OWN failure
    # observation -- it does not borrow the negative control's.
    commands.append(
        {
            "type": "message",
            "msg_id": "m1",
            "content": (
                "Call the media_generate_image tool exactly once with the prompt "
                "'a plain red square'. Do not explain, just make the tool call."
            ),
            "files": [],
        }
    )
    commands.append(
        {
            "type": "message",
            "msg_id": "m2",
            "content": (
                "Now call the media_generate_locked tool exactly once with the prompt "
                "'a plain blue square'. Do not explain, just make the tool call."
            ),
            "files": [],
        }
    )

    s = Session(binary, home, timeout=240)
    events = s.run(commands, settle=settle)

    types_seen = sorted({e.get("type", "?") for e in events})
    r.note(f"event types seen ({len(events)} events): {types_seen}")
    r.note(f"stderr (first 400): {s.stderr[:400]}")

    calls = tool_calls(events)
    r.note(f"tool-shaped events: {calls[:40]}")
    contacted = contact.exists() or late_contact.exists()
    r.note(f"fixture process contacted: {contacted}")

    fixture_log = ""
    for p in (contact, late_contact):
        if p.exists():
            fixture_log += p.read_text(encoding="utf-8", errors="replace")
    r.note(f"fixture log ({len(fixture_log)} bytes): {fixture_log[:600]}")

    called_image = "media_generate_image" in fixture_log or any(
        "media_generate_image" in c for c in calls
    )
    called_locked = "media_generate_locked" in fixture_log or any(
        "media_generate_locked" in c for c in calls
    )

    costs = find_cost_records(events)
    r.note(f"cost-shaped fields found: {json.dumps(costs[:20])}")

    # ---- accounting -------------------------------------------------------
    if not called_image and not called_locked:
        r.grade(
            "accounting",
            NOT_MEASURED,
            "no media tool was invoked in this run, so there was no media call "
            "for a cost record to describe. Grading PASS or FAIL here would be "
            "vacuous.",
        )
    else:
        turn_costs = [c for c in costs if c["value"] is not None]
        media_attributed = [
            c for c in turn_costs if "media" in c["path"].lower() or "tool" in c["path"].lower()
        ]
        if media_attributed:
            r.grade(
                "accounting",
                PASS,
                f"a cost record attributable to the media call was emitted: "
                f"{json.dumps(media_attributed[:5])}",
            )
        elif turn_costs:
            r.grade(
                "accounting",
                FAIL,
                f"the media tool WAS invoked and the session emitted "
                f"{len(turn_costs)} cost-shaped field(s), but every one is "
                f"token/turn-shaped with no attribution to the media call: "
                f"{json.dumps(turn_costs[:5])}",
            )
        else:
            r.grade(
                "accounting",
                FAIL,
                "the media tool WAS invoked and the session emitted NO "
                "cost-shaped field of any kind.",
            )

    # ---- failures ---------------------------------------------------------
    if called_locked:
        # Did the named cause reach the transcript?
        blob = "\n".join(s.raw_lines)
        named = "locked" in blob.lower() or "premium" in blob.lower() or "paid" in blob.lower()
        if named:
            r.grade(
                "failures",
                PASS,
                "this shape invoked the failing media tool itself and a named "
                "cause reached the host transcript.",
            )
        else:
            r.grade(
                "failures",
                FAIL,
                "the failing media tool was invoked but no named cause reached "
                "the host transcript.",
            )
    else:
        r.grade(
            "failures",
            NOT_MEASURED,
            "the failing media tool was not invoked in this run, so this "
            "shape's own failure path was not exercised.",
        )

    r.grade(
        "tool_invocation",
        PASS if (called_image or called_locked) else FAIL,
        f"media_generate_image called={called_image}, media_generate_locked called={called_locked}",
    )
    return r


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True, type=Path)
    ap.add_argument("--fixture", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--status", required=True, type=Path)
    ap.add_argument("--settle", type=float, default=60.0)
    args = ap.parse_args()

    if not _KEY:
        print("FLUX_API_KEY is not set in the environment", file=sys.stderr)
        return 2

    tmp = Path(tempfile.mkdtemp(prefix="f27cred-"))
    results = []
    for label, desc, late, both in (
        ("B/MCP-only", "media tools from a config-declared MCP server", False, False),
        ("C/late-MCP", "media tools introduced after session start via AddMcpServer", True, False),
        ("D/combined", "config-declared and late servers together", True, True),
    ):
        r = drive_shape(args.binary, tmp, args.fixture, label, desc, late, both, args.settle)
        results.append(r)
        print(f"--- {label}")
        for clause, (grade, why) in r.clauses.items():
            print(f"    {clause:16s} {grade}")

    record = {
        "driver": "f27-generation-shapes-credentialled.py",
        "turn_model": TURN_MODEL,
        "shapes": [r.to_dict() for r in results],
    }
    args.out.write_text(redact(json.dumps(record, indent=2)), encoding="utf-8")

    counts = {PASS: 0, FAIL: 0, NOT_MEASURED: 0}
    for r in results:
        for grade, _ in r.clauses.values():
            counts[grade] = counts.get(grade, 0) + 1

    # Status file first, completion marker last (LANE-BRIEF §6b-ii): the
    # caller reads this back rather than trusting an exit status.
    args.status.write_text(
        f"WLRC=0\nPASS={counts[PASS]}\nFAIL={counts[FAIL]}\n"
        f"NOT_MEASURED={counts[NOT_MEASURED]}\nWLDONE\n",
        encoding="utf-8",
    )
    print(f"PASS={counts[PASS]} FAIL={counts[FAIL]} NOT_MEASURED={counts[NOT_MEASURED]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
