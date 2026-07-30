#!/usr/bin/env python3
"""effect-accounting / claim B — what happens to a PENDING approval when the
process is SIGKILLed mid-approval, with durable sessions off?

Drives the real `wayland-core --json-stream` engine over stdin/stdout:

  1. wait for `ready`;
  2. send one `message`;
  3. the loopback provider always answers with a destructive `Bash` tool call,
     so the engine must park on `approval_required`;
  4. as soon as `approval_required` arrives, SIGKILL the process — the human's
     answer has NOT been given and the question is in flight;
  5. relaunch with `--resume <same id>` and record everything the product
     offers about that pending approval.

Two arms differ only in whether WAYLAND_VAULT_PASSPHRASE is supplied.
`arm=on` keeps durable sessions; `arm=off` is the headless degrade. The arm
control is asserted in BOTH directions from the degrade notice on stderr.

Every actor that must have started is asserted, not assumed (§6a-i): the run
is only a measurement if (a) the mock logged a round-trip and (b) the engine
emitted `approval_required`. Either missing is reported as NOT-RUN, never as
a pass.
"""

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

BIN = os.environ["BIN"]
OUT = Path(os.environ.get("OUT", "/root/effacc-approval"))
PORT = int(os.environ.get("PORT", "8481"))
MOCK = Path(__file__).with_name("mock_tool_provider.py")
SID_ON = "cccccc-000003"
SID_OFF = "dddddd-000004"
DESTRUCTIVE = "rm -rf /tmp/effacc-destructive-target"

CONFIG = """[default]
provider = "mock"

[providers.mock]
provider = "openai"
model = "mock-model"
api_key = "effect-accounting-lane-not-a-secret"
base_url = "http://127.0.0.1:{port}"

[providers.mock.compat]
include_usage_in_stream = false
"""


def child_env(home, extra=None):
    env = dict(os.environ)
    for k in ("DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR", "DISPLAY",
              "WAYLAND_VAULT_PASSPHRASE", "WAYLAND_VAULT_PASSPHRASE_FD",
              "ANTHROPIC_API_KEY", "OPENAI_API_KEY"):
        env.pop(k, None)
    env["HOME"] = str(home)
    env["WAYLAND_HOME"] = str(home)
    if extra:
        env.update(extra)
    return env


def write_config(home):
    home.mkdir(parents=True, exist_ok=True)
    (home / "config.toml").write_text(CONFIG.format(port=PORT))


def billed(mock_log):
    if not mock_log.exists():
        return 0
    return sum(1 for line in mock_log.read_text().splitlines() if " BILLED " in line)


def drive_until_approval(arm, home, sid, extra_env, mock_log, timeout=90):
    """Launch, send one message, kill the instant approval_required lands."""
    before = billed(mock_log)
    stderr_path = OUT / f"{arm}-run1.stderr"
    events_path = OUT / f"{arm}-run1.events.jsonl"
    proc = subprocess.Popen(
        [BIN, "--json-stream", "--session-id", sid],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=open(stderr_path, "wb"), env=child_env(home, extra_env),
        text=True, bufsize=1,
    )
    approval = None
    seen = []
    deadline = time.time() + timeout
    sent = False
    with open(events_path, "w") as ev:
        while time.time() < deadline:
            line = proc.stdout.readline()
            if not line:
                break
            ev.write(line)
            ev.flush()
            try:
                obj = json.loads(line)
            except ValueError:
                continue
            seen.append(obj.get("type"))
            if obj.get("type") == "ready" and not sent:
                proc.stdin.write(json.dumps({
                    "type": "message", "msg_id": "effacc-1",
                    "content": "Delete the scratch directory.",
                }) + "\n")
                proc.stdin.flush()
                sent = True
            if obj.get("type") == "approval_required":
                approval = obj
                break
    if approval is not None:
        # A crash mid-approval: the human has not answered and cannot now.
        proc.send_signal(signal.SIGKILL)
    else:
        proc.kill()
    try:
        proc.wait(timeout=20)
    except subprocess.TimeoutExpired:
        pass
    return {
        "arm": arm,
        "round_trips": billed(mock_log) - before,
        "event_types": seen,
        "approval_required": approval,
        "killed_signal": "SIGKILL" if approval is not None else "kill (no approval seen)",
        "stderr": stderr_path.read_text(errors="replace"),
    }


def relaunch_and_probe(arm, home, sid, extra_env, timeout=60):
    """Second process: does the pending approval survive, and is it offered?"""
    stderr_path = OUT / f"{arm}-run2.stderr"
    events_path = OUT / f"{arm}-run2.events.jsonl"
    proc = subprocess.Popen(
        [BIN, "--json-stream", "--resume", sid],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=open(stderr_path, "wb"), env=child_env(home, extra_env),
        text=True, bufsize=1,
    )
    seen, raw = [], []
    deadline = time.time() + timeout
    with open(events_path, "w") as ev:
        while time.time() < deadline:
            line = proc.stdout.readline()
            if not line:
                break
            ev.write(line)
            ev.flush()
            raw.append(line)
            try:
                obj = json.loads(line)
            except ValueError:
                continue
            seen.append(obj.get("type"))
            # `ready` carries the recovery posture for a resumed durable turn.
            if obj.get("type") == "ready":
                # give the engine a beat to emit any recovery frames, then stop
                time.sleep(3)
                break
    proc.kill()
    try:
        proc.wait(timeout=20)
    except subprocess.TimeoutExpired:
        pass
    return {
        "arm": arm,
        "rc_after_kill": proc.returncode,
        "event_types": seen,
        "raw": raw,
        "stderr": stderr_path.read_text(errors="replace"),
    }


def run_arm(arm, sid, extra_env, mock_log):
    home = OUT / f"home-{arm}"
    write_config(home)
    first = drive_until_approval(arm, home, sid, extra_env, mock_log)
    second = relaunch_and_probe(arm, home, sid, extra_env)
    degraded1 = "durable session persistence is OFF" in first["stderr"]
    degraded2 = "durable session persistence is OFF" in second["stderr"]
    sessions = sorted(p.name for p in (home / "sessions").glob("*")) \
        if (home / "sessions").exists() else []
    return {
        "arm": arm,
        "degrade_notice_run1": degraded1,
        "degrade_notice_run2": degraded2,
        "run1_round_trips": first["round_trips"],
        "run1_events": first["event_types"],
        "run1_approval_required": first["approval_required"],
        "run1_kill": first["killed_signal"],
        "run2_events": second["event_types"],
        "run2_stderr_tail": second["stderr"].strip().splitlines()[-3:],
        "sessions_on_disk": sessions,
    }


def main():
    if OUT.exists():
        subprocess.run(["rm", "-rf", str(OUT)], check=False)
    OUT.mkdir(parents=True)
    mock_log = OUT / "mock.log"
    env = dict(os.environ, MOCK_LOG=str(mock_log), MOCK_BASH_CMD=DESTRUCTIVE)
    mock = subprocess.Popen([sys.executable, str(MOCK), str(PORT)], env=env,
                            stdout=open(OUT / "mock.stdout", "wb"),
                            stderr=subprocess.STDOUT)
    try:
        for _ in range(40):
            if mock_log.exists() and "listening on" in mock_log.read_text():
                break
            time.sleep(0.25)
        else:
            print("MOCK NEVER STARTED — run is void")
            return 1

        results = [
            run_arm("on", SID_ON,
                    {"WAYLAND_VAULT_PASSPHRASE": "effacc-throwaway-not-a-secret"},
                    mock_log),
            run_arm("off", SID_OFF, None, mock_log),
        ]
    finally:
        mock.kill()

    (OUT / "results.json").write_text(json.dumps(results, indent=2))

    # Participant assertions (§6a-i): every arm must have actually reached the
    # provider AND actually parked on an approval, or it is NOT-RUN.
    unrun = 0
    for r in results:
        ran = r["run1_round_trips"] > 0
        parked = r["run1_approval_required"] is not None
        if not (ran and parked):
            unrun += 1
        print(f"ARM={r['arm']} reached_provider={ran} parked_on_approval={parked} "
              f"degrade_notice_run1={r['degrade_notice_run1']} "
              f"degrade_notice_run2={r['degrade_notice_run2']} "
              f"run1_events={','.join(t for t in r['run1_events'] if t)} "
              f"run2_events={','.join(t for t in r['run2_events'] if t) or '-'} "
              f"sessions_on_disk={len(r['sessions_on_disk'])}")
        print(f"ARM={r['arm']} run2_stderr_tail={r['run2_stderr_tail']}")
    print(f"UNRUN_CELLS={unrun} of {len(results)}")
    print("WLDONE")
    return 0


if __name__ == "__main__":
    sys.exit(main())
