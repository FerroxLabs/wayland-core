#!/usr/bin/env python3
"""24-C2 macOS leg — the three things the ledger records as absent.

  * **macOS evidence** — the refusal/accept/publish/fire sequence from
    `24-C2-LIVE-EVIDENCE.md` (measured only on `hetzner-dsm`) re-driven against a
    macOS-built `wayland-core`.
  * **the PTY surface gate** — the same verbs on a REAL pseudo-terminal, with the
    rendered screen kept as the artifact. A pipe measures the non-tty branch.
  * **the kill-mid-fire continuation run** — `cron publish --help` states the
    contract verbatim: *"a process killed between firing a job and clearing the
    event re-fires it on the next tick"*. Nothing had ever killed one. This does,
    with SIGKILL (not SIGTERM — the daemon documents a clean SIGTERM path, so
    SIGTERM would measure the graceful branch), and counts at the on-disk queue
    and history, which outlive the process that was killed.

Both directions everywhere: the continuation arm is paired with a NO-RESTART
control, because "the events were consumed" is free if they were already consumed
before the kill.

  python3 .planning/scripts/macos-legs-m2-cron.py
"""

import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
BIN = ROOT / "target" / "debug" / "wayland-core"
PTY = ROOT / ".planning" / "scripts" / "pty-drive.py"
OUT = ROOT / ".planning" / "evidence" / "macos-legs"
TOPIC = "build.finished"


def wl(home, *args, timeout=60):
    """Run the shipped binary with an isolated WAYLAND_HOME. Returns (rc, out)."""
    env = dict(os.environ)
    env["WAYLAND_HOME"] = str(home)
    env["NO_COLOR"] = "1"
    p = subprocess.run(
        [str(BIN), *args],
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        cwd=str(ROOT),
    )
    return p.returncode, (p.stdout + p.stderr)


def queued_events(home):
    d = pathlib.Path(home) / "cron" / "events"
    return sorted(p.name for p in d.glob("*.json")) if d.is_dir() else []


def history_records(home):
    f = pathlib.Path(home) / "cron" / "history.jsonl"
    if not f.is_file():
        return []
    return [ln for ln in f.read_text(errors="replace").splitlines() if ln.strip()]


def daemon_pid(home):
    f = pathlib.Path(home) / "cron-daemon.pid"
    if not f.is_file():
        return None
    try:
        return int(f.read_text().strip())
    except ValueError:
        return None


def alive(pid):
    if pid is None:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    # `kill -0` succeeds for a zombie too; reject state Z the way baseline 3 does.
    st = subprocess.run(
        ["/bin/ps", "-o", "state=", "-p", str(pid)], capture_output=True, text=True
    ).stdout.strip()
    return bool(st) and not st.startswith("Z")


def kill_daemon(home):
    pid = daemon_pid(home)
    if alive(pid):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        for _ in range(100):
            if not alive(pid):
                break
            time.sleep(0.05)
    return pid


def section(title):
    print()
    print("=" * 78)
    print(title)
    print("=" * 78)


# ── PART A: macOS evidence for the refuse / accept / publish / fire sequence ──


def part_a():
    section("PART A — macOS evidence: refuse, accept, publish, fire")
    home = pathlib.Path(tempfile.mkdtemp(prefix="wl-24c2-macos-"))
    print(f"WAYLAND_HOME={home}")

    for trig in [
        "webhook:/hooks/build",
        "poll:https://status.test/health:300",
    ]:
        rc, out = wl(home, "cron", "add", "--trigger", trig, "--slash", "/brief")
        print(f"\n$ wayland-core cron add --trigger {trig} --slash /brief")
        print(out.rstrip())
        print(f"  EXIT={rc}")
        assert rc != 0, f"A: {trig} must be REFUSED with a non-zero exit, got {rc}"

    rc, out = wl(home, "cron", "list")
    print("\n$ wayland-core cron list   (after both refusals)")
    print(out.rstrip())
    assert rc == 0
    persisted_after_refusal = "(no cron jobs)" in out
    print(f"  NOTHING_PERSISTED={persisted_after_refusal}")
    assert persisted_after_refusal, "A: a refused trigger must not persist a job"

    rc, out = wl(
        home,
        "cron",
        "add",
        "--trigger",
        f"event:{TOPIC}",
        "--channel",
        "team",
        "--text",
        "build is green",
    )
    print(f"\n$ wayland-core cron add --trigger event:{TOPIC} --channel team --text ...")
    print(out.rstrip())
    print(f"  EXIT={rc}")
    assert rc == 0, "A: an event trigger must be accepted"
    job_id = out.strip().split()[-1]

    rc, out = wl(home, "cron", "history", job_id)
    print(f"\n$ wayland-core cron history {job_id}   (BEFORE)")
    print(out.rstrip())
    before = history_records(home)
    print(f"  HISTORY_RECORDS_BEFORE={len(before)}")

    rc, out = wl(home, "cron", "publish", TOPIC)
    print(f"\n$ wayland-core cron publish {TOPIC}")
    print(out.rstrip())
    print(f"  EXIT={rc}")
    assert rc == 0
    q = queued_events(home)
    print(f"  QUEUED_EVENTS_ON_DISK={len(q)} {q}")
    assert len(q) == 1, "A: publish must queue exactly one event on disk"

    rc, out = wl(home, "cron", "daemon")
    print("\n$ wayland-core cron daemon")
    print(out.rstrip())
    pid = daemon_pid(home)
    print(f"  DAEMON_PID={pid} ALIVE={alive(pid)}")
    assert alive(pid), "A: the daemon must actually be running — a dead actor proves nothing"

    fired = False
    for _ in range(600):
        if history_records(home):
            fired = True
            break
        time.sleep(0.1)
    after = history_records(home)
    q2 = queued_events(home)
    rc, out = wl(home, "cron", "history", job_id)
    print(f"\n$ wayland-core cron history {job_id}   (AFTER)")
    print(out.rstrip())
    print(f"  HISTORY_RECORDS_AFTER={len(after)} QUEUE_DRAINED_TO={len(q2)}")
    kill_daemon(home)
    print(
        f"PART-A-SUMMARY: webhook_refused=true poll_refused=true nothing_persisted=true "
        f"event_accepted=true published=1 fired={fired} history_before={len(before)} "
        f"history_after={len(after)} queue_after={len(q2)}"
    )
    assert fired, "A: the published event never produced a fire record"
    assert len(q2) == 0, "A: the drain must consume the queued event"
    shutil.rmtree(home, ignore_errors=True)
    return True


# ── PART B: the PTY surface gate ─────────────────────────────────────────


def pty_run(home, args, screen_name, settle=2.0):
    env = dict(os.environ)
    env["WAYLAND_HOME"] = str(home)
    env.pop("NO_COLOR", None)
    screen = OUT / screen_name
    p = subprocess.run(
        [
            sys.executable,
            str(PTY),
            "--out",
            str(screen),
            "--rows",
            "40",
            "--cols",
            "110",
            "--settle",
            str(settle),
            "--",
            str(BIN),
            *args,
        ],
        env=env,
        capture_output=True,
        text=True,
        cwd=str(ROOT),
        timeout=180,
    )
    rc = None
    for ln in p.stdout.splitlines():
        if ln.startswith("PTY_CHILD_RC="):
            rc = int(ln.split("=", 1)[1])
    return rc, screen.read_text() if screen.is_file() else "", p.stdout + p.stderr


def part_b():
    section("PART B — the PTY surface gate (real controlling terminal)")
    home = pathlib.Path(tempfile.mkdtemp(prefix="wl-24c2-pty-"))
    print(f"WAYLAND_HOME={home}")

    # Instrument liveness FIRST: prove the harness really gives the child a tty,
    # and that the same probe reports 0 through a pipe. Without this pair every
    # screen below could be the non-tty branch.
    rc, scr, _ = pty_run(home, [], "pty-DISCARD.txt", settle=0.2)
    probe = subprocess.run(
        [
            sys.executable,
            str(PTY),
            "--out",
            str(OUT / "pty-instrument-control.txt"),
            "--rows",
            "10",
            "--cols",
            "60",
            "--settle",
            "0.3",
            "--",
            "/bin/sh",
            "-c",
            "if [ -t 1 ]; then echo ISATTY=1; else echo ISATTY=0; fi; /bin/stty size",
        ],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
        timeout=60,
    )
    ctrl = (OUT / "pty-instrument-control.txt").read_text()
    piped = subprocess.run(
        ["/bin/sh", "-c", "if [ -t 1 ]; then echo ISATTY=1; else echo ISATTY=0; fi"],
        capture_output=True,
        text=True,
    ).stdout.strip()
    print("instrument: under the PTY harness ->")
    print("   " + ctrl.strip().replace("\n", "\n   "))
    print(f"instrument: through a pipe        -> {piped}")
    assert "ISATTY=1" in ctrl, "PTY harness does not give the child a tty"
    assert piped == "ISATTY=0", "the negative control is not negative"
    (OUT / "pty-DISCARD.txt").unlink(missing_ok=True)

    steps = [
        (["cron", "add", "--trigger", "webhook:/hooks/build", "--slash", "/brief"],
         "pty-cron-refuse-webhook.txt", 1),
        (["cron", "add", "--trigger", f"event:{TOPIC}", "--slash", "/brief"],
         "pty-cron-add-event.txt", 0),
        (["cron", "list"], "pty-cron-list.txt", 0),
    ]
    ok = True
    for args, name, want_rc in steps:
        rc, scr, drv = pty_run(home, args, name)
        print(f"\n--- PTY: wayland-core {' '.join(args)}")
        print(drv.strip())
        print(f"    expected rc={want_rc} got rc={rc}")
        print("    ── rendered screen ──")
        for ln in scr.rstrip().splitlines():
            print("    | " + ln)
        if rc != want_rc:
            ok = False
    print(f"\nPART-B-SUMMARY: pty_isatty=1 pipe_isatty=0 screens={len(steps)} rc_all_expected={ok}")
    shutil.rmtree(home, ignore_errors=True)
    return ok


# ── PART C: kill-mid-fire continuation, with a no-restart control ────────


def stage_events(home, n):
    """Add one event job and publish `n` events with NO consumer running."""
    rc, out = wl(
        home, "cron", "add", "--trigger", f"event:{TOPIC}", "--channel", "team",
        "--text", "kill-mid-fire",
    )
    assert rc == 0, out
    job_id = out.strip().split()[-1]
    for _ in range(n):
        rc, out = wl(home, "cron", "publish", TOPIC)
        assert rc == 0, out
    return job_id


def part_c():
    section("PART C — kill-mid-fire continuation (SIGKILL) + no-restart control")
    results = {}

    for arm in ("restart", "no-restart-control"):
        home = pathlib.Path(tempfile.mkdtemp(prefix=f"wl-24c2-{arm}-"))
        print(f"\n--- ARM {arm}: WAYLAND_HOME={home}")
        job_id = stage_events(home, 6)
        q0 = queued_events(home)
        print(f"    staged: job={job_id} queued_events={len(q0)} history={len(history_records(home))}")
        assert len(q0) == 6, f"staging failed: {len(q0)} events queued"

        rc, out = wl(home, "cron", "daemon")
        pid = daemon_pid(home)
        print(f"    daemon started pid={pid} alive={alive(pid)}")
        assert alive(pid), "the daemon never started — nothing below would be a measurement"

        # Kill the instant the FIRST fire record lands: that is "between firing a
        # job and clearing the event".
        killed_at = None
        deadline = time.time() + 60
        while time.time() < deadline:
            if history_records(home):
                killed_at = len(history_records(home))
                os.kill(pid, signal.SIGKILL)
                break
            time.sleep(0.01)
        for _ in range(200):
            if not alive(pid):
                break
            time.sleep(0.05)
        time.sleep(0.5)
        h1, q1 = len(history_records(home)), len(queued_events(home))
        print(
            f"    SIGKILL sent at history={killed_at}; daemon_alive_after={alive(pid)} "
            f"history_after_kill={h1} queued_after_kill={q1}"
        )
        if not alive(pid) and killed_at is None:
            print("    NOT MEASURED: no fire record ever appeared, so nothing was killed mid-fire")
            results[arm] = None
            shutil.rmtree(home, ignore_errors=True)
            continue

        if arm == "restart":
            rc, out = wl(home, "cron", "daemon")
            pid2 = daemon_pid(home)
            print(f"    daemon RESTARTED pid={pid2} alive={alive(pid2)}")
            deadline = time.time() + 90
            while time.time() < deadline and queued_events(home):
                time.sleep(0.1)
            kill_daemon(home)
        else:
            print("    (control) daemon deliberately NOT restarted")
            time.sleep(3.0)

        h2, q2 = len(history_records(home)), len(queued_events(home))
        print(f"    FINAL history={h2} queued={q2}")
        results[arm] = {
            "killed_at_history": killed_at,
            "history_after_kill": h1,
            "queued_after_kill": q1,
            "history_final": h2,
            "queued_final": q2,
            "published": 6,
        }
        shutil.rmtree(home, ignore_errors=True)

    print()
    print("PART-C-SUMMARY: " + json.dumps(results, sort_keys=True))
    r, c = results.get("restart"), results.get("no-restart-control")
    if not r or not c:
        print("PART-C-VERDICT: NOT MEASURED (an arm never reached the kill point)")
        return None
    # The measurement is only meaningful if the kill really landed with work
    # still outstanding — LANE-BRIEF: take the comparison while the two sides can
    # still disagree.
    if r["queued_after_kill"] == 0:
        print(
            "PART-C-VERDICT: NOT MEASURED — the daemon drained the whole queue before "
            "the kill landed, so nothing was outstanding and continuation is untested"
        )
        return None
    # The property under test is CONTINUATION — that work outstanding when the
    # process was hard-killed is still picked up afterwards. It is deliberately
    # NOT "the queue reached zero": the drain is rate-bounded by design, so a
    # zero-queue criterion would measure the rate limiter, not continuation, and
    # would go red for a completely correct product. What must be true is that
    # the restarted daemon made progress the control arm did not.
    continued = (
        r["history_final"] > r["history_after_kill"]
        and r["queued_final"] < r["queued_after_kill"]
    )
    control_held = (
        c["history_final"] == c["history_after_kill"]
        and c["queued_final"] == c["queued_after_kill"]
    )
    # No event may be lost or double-counted: fired + still-queued must equal
    # published, on both arms.
    conserved = all(
        x["history_final"] + x["queued_final"] == x["published"] for x in (r, c)
    )
    print(
        f"PART-C-VERDICT: continuation_after_kill={continued} "
        f"no_restart_control_made_no_progress={control_held} "
        f"events_conserved(fired+queued==published)={conserved} "
        f"discrimination={'PASS' if continued and control_held and conserved else 'FAIL'}"
    )
    return continued and control_held and conserved


def main():
    print("### 24-C2 macOS leg")
    print(f"host:  {subprocess.run(['hostname'], capture_output=True, text=True).stdout.strip()}")
    print(f"uname: {subprocess.run(['uname', '-a'], capture_output=True, text=True).stdout.strip()}")
    print(f"HEAD:  {subprocess.run(['/usr/bin/git', 'rev-parse', 'HEAD'], cwd=str(ROOT), capture_output=True, text=True).stdout.strip()}")
    print(f"binary: {BIN}")
    v = subprocess.run([str(BIN), "--version"], capture_output=True, text=True).stdout.strip()
    print(f"version: {v}")
    print(f"date:  {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}")

    only = sys.argv[1].lower() if len(sys.argv) > 1 else None
    a = b = c = None
    for name, fn in (("a", part_a), ("b", part_b), ("c", part_c)):
        if only and only != name:
            continue
        try:
            v = fn()
        except AssertionError as exc:
            print(f"PART {name.upper()} FAILED: {exc}")
            v = False
        if name == "a":
            a = v
        elif name == "b":
            b = v
        else:
            c = v

    print()
    print(f"M2-RESULT: macos_evidence={a} pty_surface_gate={b} kill_mid_fire={c}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
