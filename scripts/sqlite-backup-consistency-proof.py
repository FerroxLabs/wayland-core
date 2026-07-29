#!/usr/bin/env python3
"""F26-SC3-O1 — does `wayland-core backup` round-trip a LIVE SQLite database?

Runs the REAL product binary against a real home while stock-Python SQLite
writers commit into `memory.db`, then restores the archive and asks two
questions of the restored database:

  1. `PRAGMA integrity_check` — is it structurally sound?
  2. is every row that was COMMITTED BEFORE the archive started still there?

Question 2 is the load-bearing one, and it is deliberately conservative: rows
committed *during* the archive window are excluded from the required set,
because a snapshot taken at the start of the run is entitled to miss them. Only
rows that were durable before `backup create` was even launched are demanded.

## Why python and not sh

LANE-BRIEF section 3.2: a pipe steals exit status, and `$?` after a shell
pipeline is a well-known self-passing gate. Every subprocess here is run with
an explicit `check` and its return code is read back as an integer.

## The three arms

* `--arm concurrent` — writers live across the archive. Known-negative.
* `--arm sequenced`  — the SAME writers commit the SAME rows and are then
  STOPPED before the archive starts. This is the control that proves the
  harness is CAPABLE of reporting a pass; without it a FAIL in the concurrent
  arm is equally consistent with a harness that always fails. It isolates
  concurrency as the only variable — a `--arm quiescent` control with no
  writers at all would also have made the row-survival question vacuous,
  leaving only `integrity_check` doing any work.
* `--arm concurrent` again, after the fix. Positive.

## Anti-vacuity guards (LANE-BRIEF section 6a-i)

* every writer must write a START marker, which it does only after its FIRST
  COMMIT SUCCEEDS — process launch is not enough;
* every writer must have COMMITTED MORE ROWS during the archive window than
  before it. A writer that was alive but blocked is a dead instrument too, and
  the run ABORTS rather than reporting a pass;
* the journal mode SQLite actually reported is read back from the writer's own
  marker, not inferred from the PRAGMA we sent.
"""

from __future__ import annotations

import argparse
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


def run(cmd: list[str], *, cwd: Path | None = None) -> tuple[int, str, str]:
    p = subprocess.run(
        cmd, cwd=cwd, capture_output=True, text=True, timeout=1800  # noqa: S603
    )
    return p.returncode, p.stdout, p.stderr


def prefill(db: Path, mb: int) -> None:
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


class HarnessError(RuntimeError):
    """The instrument failed. Never silently degraded into a data point."""


def read_progress(markers: Path, wids: list[str]) -> dict[str, int]:
    """Read every writer's committed high-water mark.

    An unreadable or empty marker RAISES. The first version of this function
    coerced it to 0, and that turned an instrument fault into the reading
    `w1: 2820 -> 0`, i.e. a fabricated "this writer stalled". A harness that
    can silently substitute a plausible number for a failed read is exactly the
    self-passing-gate class, pointed the other way.
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
            raise HarnessError(f"progress marker for {w} is not an int: {raw!r}") from exc
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True, help="path to the wayland-core binary")
    ap.add_argument("--workdir", required=True)
    ap.add_argument(
        "--arm",
        choices=["concurrent", "sequenced", "quiescent"],
        default="concurrent",
    )
    ap.add_argument("--writers", type=int, default=3)
    ap.add_argument("--prefill-mb", type=int, default=300)
    ap.add_argument("--duration", type=float, default=180.0)
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
    restored = work / "restored"
    archive = work / "backup.tar.gz"
    home.mkdir(parents=True)
    markers.mkdir(parents=True)

    (home / "config.toml").write_text('default_profile = "main"\n')
    db = home / "memory.db"

    print(f"[{args.label}] prefilling memory.db to ~{args.prefill_mb} MiB ...")
    t0 = time.time()
    prefill(db, args.prefill_mb)
    print(
        f"[{args.label}] prefill done in {time.time() - t0:.1f}s, "
        f"memory.db = {db.stat().st_size / 1048576:.1f} MiB"
    )

    wids = [f"w{i}" for i in range(args.writers)] if args.arm != "quiescent" else []
    # `sequenced` runs the writers for a bounded burst and stops them BEFORE the
    # archive; `concurrent` lets them run across it.
    writer_duration = 10.0 if args.arm == "sequenced" else args.duration
    procs = []
    for w in wids:
        procs.append(
            subprocess.Popen(  # noqa: S603
                [
                    sys.executable,
                    str(WRITER),
                    str(db),
                    w,
                    str(markers),
                    str(writer_duration),
                ],
                stdout=subprocess.DEVNULL,
                stderr=open(work / f"writer-{w}.err", "w"),
            )
        )

    # --- anti-vacuity guard 1: every writer reached a START marker -----------
    if wids:
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
            for p in procs:
                p.send_signal(signal.SIGTERM)
            print(f"WRITERS-STARTED: {len(started)}/{len(wids)}")
            print("VERDICT: ABORTED-WRITERS-DID-NOT-START")
            return 4
        modes = {
            w: (markers / f"JOURNALMODE-{w}").read_text().strip() for w in wids
        }
        print(f"[{args.label}] journal modes reported by writers: {modes}")
        if any(m.lower() != "wal" for m in modes.values()):
            print("VERDICT: ABORTED-NOT-WAL")
            return 5

    if args.arm == "sequenced":
        # The control: let the burst finish and every writer EXIT, so the home
        # is genuinely at rest when the archive starts. Same code, same rows.
        for p in procs:
            p.wait(timeout=300)
        alive = [i for i, p in enumerate(procs) if p.poll() is None]
        if alive:
            print(f"VERDICT: ABORTED-WRITERS-STILL-ALIVE-IN-SEQUENCED-ARM {alive}")
            return 9
        print(f"[{args.label}] all {len(procs)} writers exited; home is at rest")

    pre = read_progress(markers, wids)
    print(f"[{args.label}] pre-archive committed high-water marks: {pre}")

    t_arch = time.time()
    rc, out, err = run(
        [str(binary), "backup", "create", "--home", str(home), "--out", str(archive)]
    )
    arch_secs = time.time() - t_arch
    print(f"[{args.label}] backup create rc={rc} in {arch_secs:.2f}s")
    print(out.strip())
    if rc != 0:
        print(err.strip())
        print("VERDICT: ABORTED-CREATE-FAILED")
        return 6

    post = read_progress(markers, wids)
    print(f"[{args.label}] post-archive committed high-water marks: {post}")

    # --- anti-vacuity guard 2: every writer was ALIVE ACROSS the window -----
    if wids and args.arm == "concurrent":
        stalled = [w for w in wids if post[w] <= pre[w]]
        if stalled:
            print(f"WRITERS-STALLED-ACROSS-ARCHIVE: {stalled}")
            print("VERDICT: ABORTED-NO-CONCURRENCY-DURING-ARCHIVE")
            for p in procs:
                p.send_signal(signal.SIGTERM)
            return 7
        print(
            f"[{args.label}] commits during archive window: "
            + ", ".join(f"{w}=+{post[w] - pre[w]}" for w in wids)
        )

    for p in procs:
        p.send_signal(signal.SIGTERM)
    for p in procs:
        try:
            p.wait(timeout=60)
        except subprocess.TimeoutExpired:
            p.kill()

    rc, out, err = run(
        [
            str(binary),
            "backup",
            "restore",
            str(archive),
            "--home",
            str(restored),
            "--accept-missing-secrets",
        ]
    )
    print(f"[{args.label}] backup restore rc={rc}")
    print(out.strip())
    if rc != 0:
        print(err.strip())
        print("VERDICT: ABORTED-RESTORE-FAILED")
        return 8

    rdb = restored / "memory.db"
    entries = sorted(p.name for p in restored.iterdir())
    print(f"[{args.label}] restored entries: {entries}")
    if not rdb.is_file():
        print("VERDICT: FAIL-NO-DATABASE-RESTORED")
        return 1

    # --- the two questions --------------------------------------------------
    integrity = "unreadable"
    integrity_lines = -1
    missing_total = 0
    missing_detail: dict[str, int] = {}
    try:
        rconn = sqlite3.connect(f"file:{rdb}?mode=ro", uri=True)
        rows = rconn.execute("PRAGMA integrity_check").fetchall()
        # integrity_check returns one row per problem, capped at 100 by SQLite,
        # and a single row reading exactly "ok" when the database is sound.
        # Reported as a COUNT plus the first line: the raw dump is thousands of
        # lines and buries the verdict.
        joined = "\n".join(str(r[0]) for r in rows)
        integrity_lines = len([ln for ln in joined.splitlines() if ln.strip()])
        integrity = "ok" if joined.strip() == "ok" else joined.splitlines()[0]
        for w in wids:
            need = pre[w]
            if need <= 0:
                missing_detail[w] = 0
                continue
            have = rconn.execute(
                "SELECT COUNT(*) FROM rows_committed WHERE wid = ? AND n <= ?",
                (w, need),
            ).fetchone()[0]
            missing_detail[w] = need - have
            missing_total += need - have
        rconn.close()
    except sqlite3.DatabaseError as exc:
        integrity = f"DatabaseError: {exc}"
        missing_total = sum(pre.values())
        missing_detail = dict(pre)

    print("---- RESULT ----")
    print(f"ARM: {args.arm}")
    print(f"WRITERS: {len(wids)}")
    print(f"ARCHIVE-SECONDS: {arch_secs:.2f}")
    print(f"PRE-ARCHIVE-COMMITTED: {sum(pre.values())}")
    print(f"RESTORED-SIDECARS: {[e for e in entries if e.startswith('memory.db-')]}")
    print(f"INTEGRITY-CHECK: {integrity}")
    print(f"INTEGRITY-PROBLEM-LINES: {integrity_lines if integrity != 'ok' else 0}")
    print(f"MISSING-COMMITTED-ROWS: {missing_total}")
    print(f"MISSING-BY-WRITER: {missing_detail}")
    ok = integrity == "ok" and missing_total == 0
    # A pass in an arm that demanded nothing is not a pass. The row-survival
    # question is only meaningful if there were rows to demand.
    if ok and args.arm != "quiescent" and sum(pre.values()) == 0:
        print("VERDICT: ABORTED-VACUOUS-NOTHING-WAS-DEMANDED")
        return 10
    print(f"VERDICT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
