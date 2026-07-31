#!/usr/bin/env python3
"""27-C1 macOS leg — the PTY drive the ledger records as never taken.

`CRITERIA-GAP-LEDGER.md` (row 27-C1): *"The TUI half was never exercised (no PTY
drive) and macOS has no artifact."* The JSON-stream half has been driven; the
terminal half never had been, on any platform.

This starts the real TUI on a real pseudo-terminal, points it at the recording
mock provider, and makes the mock answer the first turn with a `transcribe_audio`
tool call — so the consolidated media-intake chokepoint runs inside a terminal
session and whatever it decides is rendered on the screen a user would see.

Two arms, identical except for the DIRECTORY the fixture sits in:

  * `home`   — `$HOME/.wl-m1-pty/good.wav`, whose path components are all real
               directories on macOS;
  * `tmpdir` — the same bytes under the platform's own `$TMPDIR`, which on macOS
               is always beneath `/var/folders/...` and therefore behind the
               OS-provided `/var -> private/var` symlink.

That pair is the discrimination. If both arms produced the same screen, the
directory would not be the variable and nothing here would be a finding.

  python3 .planning/scripts/macos-legs-m1-pty.py
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
MOCK = ROOT / ".planning" / "scripts" / "f27-mock-provider.py"
OUT = ROOT / ".planning" / "evidence" / "macos-legs"
PORT = 18942
WAV = b"RIFF$\x08\x00\x00WAVEfmt " + b"\x00" * 16


def write_config(home: pathlib.Path):
    (home / "config.toml").write_text(
        "[default]\n"
        'provider = "anthropic"\n'
        'model = "claude-sonnet-4-20250514"\n'
        "\n"
        "[providers.anthropic]\n"
        'api_key = "f27-fixture-credential"\n'
        f'base_url = "http://127.0.0.1:{PORT}"\n'
        "\n"
        "[session]\n"
        "enabled = false\n"
    )


def start_mock(capture: pathlib.Path, audio_path: str):
    log = open(OUT / "m1-pty-mock.stdout", "w")
    p = subprocess.Popen(
        [
            sys.executable,
            str(MOCK),
            str(PORT),
            str(capture),
            "transcribe_audio",
            json.dumps({"audio_path": audio_path}),
        ],
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    for _ in range(100):
        try:
            if "F27-MOCK-READY" in (OUT / "m1-pty-mock.stdout").read_text():
                return p
        except OSError:
            pass
        time.sleep(0.1)
    p.kill()
    raise RuntimeError("mock provider never became ready")


def drive(label: str, fixture: pathlib.Path):
    home = pathlib.Path(tempfile.mkdtemp(prefix=f"wl-m1-pty-{label}-"))
    write_config(home)
    capture = OUT / f"m1-pty-{label}-wire.jsonl"
    mock = start_mock(capture, str(fixture))
    screen = OUT / f"pty-tui-intake-{label}.txt"
    env = dict(os.environ)
    env["HOME"] = str(home)
    env["WAYLAND_HOME"] = str(home)
    env["GROQ_API_KEY"] = "f27-fixture-not-a-real-key"
    env.pop("NO_COLOR", None)
    try:
        p = subprocess.run(
            [
                sys.executable,
                str(PTY),
                "--out",
                str(screen),
                "--raw-out",
                str(OUT / f"pty-tui-intake-{label}.raw"),
                "--rows",
                "44",
                "--cols",
                "120",
                "--send",
                "transcribe it",
                "--wait",
                "1.5",
                "--send",
                "\\r",
                "--wait",
                "12",
                "--settle",
                "4",
                "--",
                str(BIN),
                "--provider",
                "anthropic",
            ],
            env=env,
            capture_output=True,
            text=True,
            cwd=str(ROOT),
            timeout=180,
        )
    finally:
        mock.send_signal(signal.SIGTERM)
        mock.wait(timeout=10)
    reqs = 0
    if capture.is_file():
        reqs = len([l for l in capture.read_text().splitlines() if l.strip()])
    txt = screen.read_text() if screen.is_file() else ""
    shutil.rmtree(home, ignore_errors=True)
    return p.stdout + p.stderr, reqs, txt


def main():
    print("### 27-C1 macOS leg — PTY drive of the TUI media-intake surface")
    print(f"uname: {subprocess.run(['uname','-a'],capture_output=True,text=True).stdout.strip()}")
    print(f"HEAD:  {subprocess.run(['/usr/bin/git','rev-parse','HEAD'],cwd=str(ROOT),capture_output=True,text=True).stdout.strip()}")
    print(f"binary: {BIN} ({subprocess.run([str(BIN),'--version'],capture_output=True,text=True).stdout.strip()})")
    print(f"TMPDIR={os.environ.get('TMPDIR')}")
    print()

    real_home = pathlib.Path(os.path.expanduser("~")) / ".wl-m1-pty"
    real_home.mkdir(exist_ok=True)
    f_home = real_home / "good.wav"
    f_home.write_bytes(WAV)

    tmpd = pathlib.Path(tempfile.mkdtemp(prefix="wl-m1-pty-fixture-"))
    f_tmp = tmpd / "good.wav"
    f_tmp.write_bytes(WAV)

    print(f"fixture(home)   = {f_home}")
    print(f"fixture(tmpdir) = {f_tmp}")
    print()

    seen = {}
    for label, fixture in (("home", f_home), ("tmpdir", f_tmp)):
        drv, reqs, txt = drive(label, fixture)
        print("=" * 78)
        print(f"ARM {label} — audio_path={fixture}")
        print("=" * 78)
        print(drv.strip())
        print(f"MOCK_REQUESTS_CAPTURED={reqs}   "
              "(0 would mean the TUI never reached the provider and the arm is void)")
        print("── rendered TUI screen ──")
        for ln in txt.rstrip().splitlines():
            print("| " + ln)
        print()
        seen[label] = txt

    for p in (f_home, f_tmp):
        try:
            p.unlink()
        except OSError:
            pass
    shutil.rmtree(tmpd, ignore_errors=True)
    try:
        real_home.rmdir()
    except OSError:
        pass

    marker = "Not a directory"
    home_hit = marker in seen.get("home", "")
    tmp_hit = marker in seen.get("tmpdir", "")
    print(
        f"M1-PTY-SUMMARY: screens=2 home_shows_ENOTDIR={home_hit} "
        f"tmpdir_shows_ENOTDIR={tmp_hit} "
        f"arms_differ={seen.get('home') != seen.get('tmpdir')}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
