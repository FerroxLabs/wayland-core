#!/usr/bin/env python3
"""BL-F26-SC3-O1-ROLLBACK — does a ROLLED-BACK home get its live SQLite
database back intact?

Sibling of `sqlite-backup-consistency-proof.py`, which asks the same question of
the ARCHIVE side. This one asks it of the UNDO STORE.

`backup restore --replace` captures the prior home into the journal's undo store
before it clears the target. That capture is `std::fs::copy` per file, walked
with `read_dir` — each member of a WAL trio read at a different instant. It is
byte-for-byte the shape the archive lane proved corrupting. If the restore is
then interrupted, `backup recover` copies that capture back, and the user's home
comes back holding a database that was never in a consistent state.

## What is actually driven

The REAL product binary, killed with an uncatchable SIGKILL while three stock
Python `sqlite3` writers commit into a live WAL `memory.db`:

  1. prefill `home/memory.db`, WAL confirmed;
  2. start N writers, wait for their START markers;
  3. launch `backup restore <archive> --home home --replace --pace-ms P`;
  4. poll the journal record until it reports `preserved: true` — the moment the
     undo capture has FINISHED, so the kill lands after the capture and inside
     the payload write, which is what an interrupted replace looks like;
  5. SIGKILL and REAP the restore;
  6. stop the writers, then `backup recover --home home`;
  7. ask the recovered `home/memory.db` two questions.

The two questions are the archive proof's, unchanged:

  * `PRAGMA integrity_check` — is it structurally sound?
  * is every row COMMITTED BEFORE the restore was launched still present?

Rows committed *during* the capture window are not demanded: a snapshot is
entitled to miss them. Only rows already durable before `backup restore` was
launched are required, which is the conservative direction.

## Why the kill is triggered on the record and not on a timer

A blind delay can land before `preserve_target` finishes, and a record with
`preserved: false` means the target was never touched, so recovery correctly
does nothing — the run would then report a clean home having tested no rollback
at all. That is the self-passing shape LANE-BRIEF §6a-i describes. Polling the
product's own record for `preserved: true` makes the trigger a fact about the
program rather than a guess about its speed, and the run ABORTS if it never
observes it.

## Anti-vacuity guards — every one of these ABORTS rather than passing

  G1  every writer wrote a START marker, which it writes only after its FIRST
      COMMIT SUCCEEDED (process launch is not enough);
  G2  the journal mode is read back from each writer's own marker, never
      inferred from the PRAGMA we sent (LANE-BRIEF §3b-ii);
  G3  `preserved: true` was OBSERVED — the capture really ran;
  G4  every writer's committed high-water mark INCREASED across the capture
      window; a writer that was present but blocked is a dead instrument;
  G5  `backup recover` reported `recovered_operations: 1` — a rollback really
      happened. If it reports 0 there was nothing to test and the run is void;
  G6  the required row set is non-empty.

## The control arm

`--arm sequenced` runs the SAME writers over the SAME rows and waits for every
one to EXIT before the restore starts, so the undo capture is taken of a home at
rest. Concurrency is then the only variable between the arms, and a PASS there
proves this harness is CAPABLE of reporting a pass — without which a FAIL in the
concurrent arm is equally consistent with a harness that always fails.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
WRITER = HERE / "sqlite-backup-writer.py"
JOURNAL_DIR = ".wayland-backup-journal"


class HarnessError(RuntimeError):
    """The instrument failed. Never silently degraded into a data point."""


def run(cmd: list[str]) -> tuple[int, str, str]:
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)  # noqa: S603
    return p.returncode, p.stdout, p.stderr


def prefill(db: Path, mb: int) -> None:
    """Grow `db` to ~`mb` MiB in WAL mode, checkpointing as it goes."""
    conn = sqlite3.connect(str(db), isolation_level=None)
    mode = conn.execute("PRAGMA journal_mode=WAL").fetchone()[0]
    if str(mode).lower() != "wal":
        raise SystemExit(f"prefill: journal_mode={mode}, not WAL — wrong arm")
    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute("CREATE TABLE IF NOT EXISTS ballast (i INTEGER PRIMARY KEY, b BLOB)")
    conn.execute(
        "CREATE TABLE IF NOT EXISTS rows_committed ("
        " wid TEXT NOT NULL, n INTEGER NOT NULL, blob TEXT NOT NULL,"
        " PRIMARY KEY (wid, n))"
    )
    payload = os.urandom(4096)
    target = mb * 1024 * 1024
    i = 0
    while db.stat().st_size < target:
        conn.execute("BEGIN")
        conn.executemany(
            "INSERT INTO ballast (i, b) VALUES (?, ?)",
            [(i + k, payload) for k in range(2000)],
        )
        conn.execute("COMMIT")
        i += 2000
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.close()


def read_progress(markers: Path, wids: list[str]) -> dict[str, int]:
    """Every writer's committed high-water mark.

    An unreadable or empty marker RAISES rather than becoming a plausible zero —
    the instrument defect the archive lane measured and repaired.
    """
    out: dict[str, int] = {}
    for w in wids:
        p = markers / f"PROGRESS-{w}"
        try:
            raw = p.read_text().strip()
        except OSError as exc:
            raise HarnessError(f"progress marker unreadable for {w}: {exc}") from exc
        if not raw:
            raise HarnessError(f"progress marker EMPTY for {w} (non-atomic publish?)")
        try:
            out[w] = int(raw)
        except ValueError as exc:
            raise HarnessError(f"progress marker for {w} not an int: {raw!r}") from exc
    return out


def account_rows(need: dict[str, int], have: dict[str, int]) -> tuple[int, int, dict[str, int]]:
    """Row accounting that cannot cancel a loss against an impossibility.

    `rows_committed` is `PRIMARY KEY (wid, n)` and every writer commits
    `n = 1..need`, so `COUNT(*) WHERE n <= need` can never exceed `need`. A
    CORRUPT database can nonetheless report that it does: measured on
    `hetzner-dsm`, run `base-c3` returned `have = need + 107` for one writer
    off a database failing `integrity_check` with 101 problem lines.

    The first version of this summed the SIGNED differences, so that +107
    would have cancelled 107 genuinely missing rows from another writer and
    reported `MISSING-COMMITTED-ROWS: 0`. Repaired rather than written up
    (LANE-BRIEF §6b-ii); `sqlite-restore-rollback-selftest.py` holds the
    three-assertion self-test, including the one that proves the OLD version
    would have missed it.

    Returns `(missing, surplus, per_writer_missing)`. A non-zero SURPLUS is
    itself a corruption finding, never rounded away.
    """
    missing = 0
    surplus = 0
    detail: dict[str, int] = {}
    for w, n in need.items():
        got = have.get(w, 0)
        detail[w] = max(0, n - got)
        missing += max(0, n - got)
        surplus += max(0, got - n)
    return missing, surplus, detail


def account_rows_SIGNED(need: dict[str, int], have: dict[str, int]) -> int:
    """The BROKEN accounting this harness shipped with, retained as evidence.

    The self-test asserts this one reports a clean 0 for a case that has really
    lost rows, and that `account_rows` does not. Without that third assertion
    the self-test would pass on the broken instrument too. Never called by the
    driver.
    """
    return sum(n - have.get(w, 0) for w, n in need.items())


def read_records(home: Path) -> list[dict]:
    """Every open journal record in `home`, parsed.

    A record being written is read through `atomic_write`, so a partial parse is
    a transient we retry rather than a fact; an unparseable file is skipped for
    this poll only.
    """
    root = home / JOURNAL_DIR
    if not root.is_dir():
        return []
    out = []
    for p in sorted(root.iterdir()):
        if not p.is_file() or p.suffix != ".json":
            continue
        try:
            out.append(json.loads(p.read_text()))
        except (OSError, json.JSONDecodeError):
            continue
    return out


def build_donor_archive(binary: Path, work: Path) -> Path:
    """A small, quiescent archive to restore FROM.

    Its content is deliberately unlike the live home so a completed restore is
    distinguishable from a rolled-back one by inspection.
    """
    donor = work / "donor"
    donor.mkdir(parents=True)
    (donor / "config.toml").write_text('default_profile = "donor"\n')
    for i in range(24):
        (donor / f"donor-payload-{i:02d}.txt").write_text(f"donor {i}\n" * 64)
    archive = work / "donor.tar.gz"
    rc, out, err = run(
        [str(binary), "backup", "create", "--home", str(donor), "--out", str(archive)]
    )
    if rc != 0:
        raise HarnessError(f"donor archive create failed rc={rc}: {err.strip()}")
    return archive


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True, help="path to the wayland-core binary")
    ap.add_argument("--workdir", required=True)
    ap.add_argument("--arm", choices=["concurrent", "sequenced"], default="concurrent")
    ap.add_argument("--writers", type=int, default=3)
    ap.add_argument("--prefill-mb", type=int, default=300)
    ap.add_argument("--duration", type=float, default=180.0)
    ap.add_argument(
        "--pace-ms",
        type=int,
        default=250,
        help="widen the payload-write window so the kill lands mid-flight",
    )
    ap.add_argument("--label", default="run")
    args = ap.parse_args()

    binary = Path(args.bin).resolve()
    if not binary.is_file():
        print(f"ABORT: binary not found: {binary}")
        return 2

    work = Path(args.workdir).resolve()
    if work.exists():
        shutil.rmtree(work)
    home = work / "home"
    markers = work / "markers"
    home.mkdir(parents=True)
    markers.mkdir(parents=True)

    archive = build_donor_archive(binary, work)
    print(f"[{args.label}] donor archive: {archive.name}")

    (home / "config.toml").write_text('default_profile = "live"\n')
    db = home / "memory.db"

    print(f"[{args.label}] prefilling memory.db to ~{args.prefill_mb} MiB ...")
    t0 = time.time()
    prefill(db, args.prefill_mb)
    print(
        f"[{args.label}] prefill done in {time.time() - t0:.1f}s, "
        f"memory.db = {db.stat().st_size / 1048576:.1f} MiB"
    )

    wids = [f"w{i}" for i in range(args.writers)]
    writer_duration = 10.0 if args.arm == "sequenced" else args.duration
    procs = []
    for w in wids:
        procs.append(
            subprocess.Popen(  # noqa: S603
                [sys.executable, str(WRITER), str(db), w, str(markers), str(writer_duration)],
                stdout=subprocess.DEVNULL,
                stderr=open(work / f"writer-{w}.err", "w"),
            )
        )

    def stop_writers() -> None:
        for p in procs:
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
        for p in procs:
            try:
                p.wait(timeout=60)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait(timeout=30)

    # --- G1: every writer reached a START marker ---------------------------
    started: set[str] = set()
    for i in range(60):
        started = {w for w in wids if (markers / f"START-{w}").exists()}
        print(
            f"[{args.label}] waiting for writers: {len(started)}/{len(wids)} "
            f"started (iter {i}, {time.time():.0f})"
        )
        if len(started) == len(wids):
            break
        time.sleep(1)
    if len(started) != len(wids):
        stop_writers()
        print(f"WRITERS-STARTED: {len(started)}/{len(wids)}")
        print("VERDICT: ABORTED-WRITERS-DID-NOT-START")
        return 4

    # --- G2: the arm is the arm we believe it is ---------------------------
    modes = {w: (markers / f"JOURNALMODE-{w}").read_text().strip() for w in wids}
    print(f"[{args.label}] journal modes reported by writers: {modes}")
    if any(m.lower() != "wal" for m in modes.values()):
        stop_writers()
        print("VERDICT: ABORTED-NOT-WAL")
        return 5

    if args.arm == "sequenced":
        for p in procs:
            p.wait(timeout=300)
        alive = [i for i, p in enumerate(procs) if p.poll() is None]
        if alive:
            print(f"VERDICT: ABORTED-WRITERS-STILL-ALIVE-IN-SEQUENCED-ARM {alive}")
            return 9
        print(f"[{args.label}] all {len(procs)} writers exited; home is at rest")

    pre = read_progress(markers, wids)
    print(f"[{args.label}] pre-restore committed high-water marks: {pre}")

    # --- launch the replace, directly (never via a shell wrapper) ----------
    # LANE-BRIEF §6b-ii: a backgrounded shell FUNCTION makes $! the wrapper's
    # pid, and the kill then hits the wrapper while the product runs to
    # completion. Popen gives us the product's own pid.
    t_op = time.time()
    proc = subprocess.Popen(  # noqa: S603
        [
            str(binary),
            "backup",
            "restore",
            str(archive),
            "--home",
            str(home),
            "--replace",
            "--accept-missing-secrets",
            "--pace-ms",
            str(args.pace_ms),
        ],
        stdout=open(work / "restore.out", "w"),
        stderr=open(work / "restore.err", "w"),
    )
    print(f"[{args.label}] restore --replace launched pid={proc.pid}")

    # --- G3: wait for the product to declare the capture finished ----------
    preserved_seen = False
    op_id = None
    deadline = time.time() + 600
    it = 0
    while time.time() < deadline:
        it += 1
        recs = read_records(home)
        for r in recs:
            if r.get("pid") == proc.pid and r.get("preserved"):
                preserved_seen = True
                op_id = r.get("op_id")
                break
        if preserved_seen:
            break
        if proc.poll() is not None:
            break
        if it % 20 == 0:
            print(
                f"[{args.label}] waiting for preserved=true "
                f"(iter {it}, {time.time() - t_op:.1f}s, records={len(recs)})"
            )
        time.sleep(0.05)

    capture_secs = time.time() - t_op
    print(f"[{args.label}] preserved={preserved_seen} after {capture_secs:.2f}s op_id={op_id}")
    if not preserved_seen:
        rc = proc.poll()
        stop_writers()
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=60)
        print(f"RESTORE-EXITED-EARLY-RC: {rc}")
        print("VERDICT: ABORTED-CAPTURE-NEVER-COMPLETED")
        return 11

    post = read_progress(markers, wids)
    print(f"[{args.label}] post-capture committed high-water marks: {post}")

    # --- G4: the writers were live ACROSS the capture window ---------------
    if args.arm == "concurrent":
        stalled = [w for w in wids if post[w] <= pre[w]]
        if stalled:
            print(f"WRITERS-STALLED-ACROSS-CAPTURE: {stalled}")
            print("VERDICT: ABORTED-NO-CONCURRENCY-DURING-CAPTURE")
            proc.kill()
            proc.wait(timeout=60)
            stop_writers()
            return 7
        print(
            f"[{args.label}] commits during capture window: "
            + ", ".join(f"{w}=+{post[w] - pre[w]}" for w in wids)
        )

    # --- kill the restore mid-flight, and REAP it --------------------------
    # Reaping matters: `journal::recover` acts only on records whose owner is
    # DEAD, and `kill(pid, 0)` on an unreaped ZOMBIE still reports alive. An
    # unreaped child would make recovery skip the record and the run would
    # report a clean home having rolled back nothing.
    if proc.poll() is None:
        proc.send_signal(signal.SIGKILL)
    killed_rc = proc.wait(timeout=120)
    print(f"[{args.label}] restore killed, reaped rc={killed_rc}")

    stop_writers()

    recs = read_records(home)
    open_recs = [r for r in recs if r.get("open")]
    print(f"[{args.label}] open journal records after kill: {len(open_recs)}")

    # --- G5: a rollback actually happened ----------------------------------
    rc, out, err = run([str(binary), "backup", "recover", "--home", str(home)])
    print(f"[{args.label}] backup recover rc={rc}")
    print(out.strip())
    if rc != 0:
        print(err.strip())
        print("VERDICT: ABORTED-RECOVER-FAILED")
        return 12
    recovered = -1
    for line in out.splitlines():
        if line.startswith("recovered_operations:"):
            recovered = int(line.split(":", 1)[1].strip())
    if recovered != 1:
        print(f"RECOVERED-OPERATIONS: {recovered}")
        print("VERDICT: ABORTED-NO-ROLLBACK-HAPPENED")
        return 13

    entries = sorted(p.name for p in home.iterdir())
    print(f"[{args.label}] home entries after rollback: {entries}")
    donor_left = [e for e in entries if e.startswith("donor-payload-")]
    if donor_left:
        print(f"ROLLBACK-LEFT-DONOR-FILES: {donor_left}")
        print("VERDICT: FAIL-ROLLBACK-INCOMPLETE")
        return 14

    rdb = home / "memory.db"
    if not rdb.is_file():
        print("VERDICT: FAIL-NO-DATABASE-AFTER-ROLLBACK")
        return 1

    # --- the two questions --------------------------------------------------
    integrity = "unreadable"
    integrity_lines = -1
    missing_total = 0
    surplus_total = 0
    missing_detail: dict[str, int] = {}
    try:
        rconn = sqlite3.connect(f"file:{rdb}?mode=ro", uri=True)
        rows = rconn.execute("PRAGMA integrity_check").fetchall()
        joined = "\n".join(str(r[0]) for r in rows)
        integrity_lines = len([ln for ln in joined.splitlines() if ln.strip()])
        integrity = "ok" if joined.strip() == "ok" else joined.splitlines()[0]
        have: dict[str, int] = {}
        for w in wids:
            have[w] = rconn.execute(
                "SELECT COUNT(*) FROM rows_committed WHERE wid = ? AND n <= ?",
                (w, pre[w]),
            ).fetchone()[0]
        missing_total, surplus_total, missing_detail = account_rows(pre, have)
        rconn.close()
    except sqlite3.DatabaseError as exc:
        integrity = f"DatabaseError: {exc}"
        missing_total = sum(pre.values())
        missing_detail = dict(pre)

    print("---- RESULT ----")
    print(f"ARM: {args.arm}")
    print(f"WRITERS: {len(wids)}")
    print(f"CAPTURE-SECONDS: {capture_secs:.2f}")
    print(f"PRE-RESTORE-COMMITTED: {sum(pre.values())}")
    print(f"RECOVERED-OPERATIONS: {recovered}")
    print(f"ROLLED-BACK-SIDECARS: {[e for e in entries if e.startswith('memory.db-')]}")
    print(f"INTEGRITY-CHECK: {integrity}")
    print(f"INTEGRITY-PROBLEM-LINES: {integrity_lines if integrity != 'ok' else 0}")
    print(f"MISSING-COMMITTED-ROWS: {missing_total}")
    print(f"MISSING-BY-WRITER: {missing_detail}")
    # A count that EXCEEDS what the schema can hold is impossible on a sound
    # database, so it is reported as its own finding rather than folded into
    # the missing total where it would cancel a real loss.
    print(f"IMPOSSIBLE-SURPLUS-ROWS: {surplus_total}")
    ok = integrity == "ok" and missing_total == 0 and surplus_total == 0
    # --- G6: a pass that demanded nothing is not a pass --------------------
    if ok and sum(pre.values()) == 0:
        print("VERDICT: ABORTED-VACUOUS-NOTHING-WAS-DEMANDED")
        return 10
    print(f"VERDICT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
