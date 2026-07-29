#!/usr/bin/env python3
"""One concurrent SQLite writer for the backup-consistency proof (F26-SC3-O1).

Deliberately stock: it uses Python's bundled `sqlite3`, not any wayland-core
code. A reproduction whose writer is our own library could be accused of
arranging the failure; a stock third-party client cannot.

Contract with the driver:

* `<markers>/START-<wid>` is written only AFTER the connection is open, WAL is
  confirmed active, and the FIRST row has committed. "Started" therefore means
  "genuinely writing", not "process launched" — LANE-BRIEF section 6a-i.
* `<markers>/PROGRESS-<wid>` holds the highest `n` this writer has COMMITTED.
  It is written after the commit returns, so it always under-reports; the
  driver's "must survive" set is conservative in the right direction.
* `<markers>/JOURNALMODE-<wid>` records the mode SQLite actually reported, so
  the driver asserts the arm it believes it is running (LANE-BRIEF section 3b-ii).
"""

import os
import sqlite3
import sys
import time


def main() -> int:
    db_path, wid, markers, duration_s = (
        sys.argv[1],
        sys.argv[2],
        sys.argv[3],
        float(sys.argv[4]),
    )

    conn = sqlite3.connect(db_path, timeout=30.0, isolation_level=None)
    mode = conn.execute("PRAGMA journal_mode=WAL").fetchone()[0]
    with open(os.path.join(markers, f"JOURNALMODE-{wid}"), "w") as fh:
        fh.write(str(mode))
    if str(mode).lower() != "wal":
        # Do NOT write START. A writer that is not in WAL is not the experiment.
        sys.stderr.write(f"writer {wid}: journal_mode={mode}, refusing to run\n")
        return 3

    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute(
        "CREATE TABLE IF NOT EXISTS rows_committed ("
        " wid TEXT NOT NULL, n INTEGER NOT NULL, blob TEXT NOT NULL,"
        " PRIMARY KEY (wid, n))"
    )

    progress_path = os.path.join(markers, f"PROGRESS-{wid}")
    filler = "x" * 400

    def commit_row(n: int) -> None:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(
            "INSERT INTO rows_committed (wid, n, blob) VALUES (?, ?, ?)",
            (wid, n, filler),
        )
        conn.execute("COMMIT")

    n = 0
    deadline = time.time() + duration_s
    while True:
        n += 1
        try:
            commit_row(n)
        except sqlite3.OperationalError as exc:
            # Contention is expected; a genuine failure is not silently ignored.
            sys.stderr.write(f"writer {wid}: n={n}: {exc}\n")
            n -= 1
            time.sleep(0.001)
            if time.time() >= deadline:
                break
            continue

        with open(progress_path, "w") as fh:
            fh.write(str(n))
            fh.flush()
            os.fsync(fh.fileno())

        if n == 1:
            with open(os.path.join(markers, f"START-{wid}"), "w") as fh:
                fh.write(f"pid={os.getpid()}")
                fh.flush()
                os.fsync(fh.fileno())

        if time.time() >= deadline:
            break

    conn.close()
    sys.stderr.write(f"writer {wid}: committed {n} rows\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
