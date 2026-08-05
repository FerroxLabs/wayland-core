#!/usr/bin/env python3
"""Self-test for the F26-SC3-O1 proof harness itself.

LANE-BRIEF section 6b-ii: when you find a defect in your own instrument you
repair it IN THE SAME LANE, and the repair gets a self-test with **three**
assertions — known-positive passes, known-negative fails, and *the old broken
instrument would have missed it*. The third is the only one that proves the
repair does anything, because the first two pass on the broken version too.

The defect being guarded here is real and was measured on this harness's first
run: the writer published its committed high-water mark with `open(path, "w")`,
which truncates before it writes. The driver read the file inside that window,
got zero bytes, coerced it to `0`, and reported that a writer committing ~29,000
rows during the archive had stalled — ABORTing a run that was working.
"""

from __future__ import annotations

import importlib.util
import shutil
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


writer = _load("wl_writer", HERE / "sqlite-backup-writer.py")
proof = _load("wl_proof", HERE / "sqlite-backup-consistency-proof.py")


def _hammer(publish, path: Path, iterations: int) -> int:
    """Publish 1..iterations while a reader hammers the file. Returns empties."""
    empties = 0
    stop = threading.Event()

    def reader() -> None:
        nonlocal empties
        while not stop.is_set():
            try:
                if path.exists() and path.read_text().strip() == "":
                    empties += 1
            except OSError:
                pass

    t = threading.Thread(target=reader, daemon=True)
    t.start()
    try:
        for n in range(1, iterations + 1):
            publish(str(path), n)
            time.sleep(0.001)
    finally:
        stop.set()
        t.join(timeout=5)
    return empties


def main() -> int:
    failures: list[str] = []
    tmp = Path(tempfile.mkdtemp(prefix="wl-sqlite-harness-selftest-"))
    try:
        # --- assertion 3 (run first: it proves the instrument can see the bug)
        broken = tmp / "PROGRESS-broken"
        broken_empties = _hammer(writer.publish_progress_TRUNCATING, broken, 200)
        print(f"A3 old-broken-publisher empty reads observed: {broken_empties}")
        if broken_empties == 0:
            failures.append(
                "A3 FAILED: the OLD broken publisher produced no observable empty "
                "read, so this self-test cannot distinguish the repair from a no-op"
            )

        # --- assertion 1: known-positive — repaired publisher is never partial
        fixed = tmp / "PROGRESS-fixed"
        fixed_empties = _hammer(writer.publish_progress, fixed, 200)
        print(f"A1 repaired-publisher empty reads observed: {fixed_empties}")
        if fixed_empties != 0:
            failures.append(
                f"A1 FAILED: repaired publisher still exposed {fixed_empties} "
                "partial reads"
            )
        if fixed.read_text().strip() != "200":
            failures.append("A1 FAILED: repaired publisher lost the final value")

        # --- assertion 2: known-negative — the driver REFUSES a bad marker
        markers = tmp / "markers"
        markers.mkdir()
        (markers / "PROGRESS-w0").write_text("")
        try:
            proof.read_progress(markers, ["w0"])
            failures.append(
                "A2 FAILED: read_progress accepted an EMPTY marker instead of "
                "raising — an instrument fault would become a fake data point"
            )
        except proof.HarnessError as exc:
            print(f"A2 driver correctly refused an empty marker: {exc}")

        (markers / "PROGRESS-w0").write_text("41")
        got = proof.read_progress(markers, ["w0"])
        if got != {"w0": 41}:
            failures.append(f"A2 FAILED: a VALID marker was misread as {got}")
        else:
            print("A2 driver correctly read a valid marker: {'w0': 41}")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("---- SELFTEST ----")
    for f in failures:
        print(f)
    print(f"ASSERTIONS: 3")
    print(f"SELFTEST-VERDICT: {'PASS' if not failures else 'FAIL'}")
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
