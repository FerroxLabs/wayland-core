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

  It is published with `os.replace`, NOT by truncating in place. Measured
  2026-07-29 on this harness's very first run: an in-place `open(path, "w")`
  leaves the file ZERO BYTES for the window between truncate and write, and the
  driver read exactly that window — reporting `w1: 2820 -> 0` and ABORTing with
  `ABORTED-NO-CONCURRENCY-DURING-ARCHIVE` for a writer that was in fact
  committing at full speed. The guard was right to fire; the instrument was
  wrong. Repaired here rather than written up, per LANE-BRIEF section 6b-ii,
  and `sqlite-backup-harness-selftest.py` holds the three-assertion self-test
  that proves the repair does something.
* `<markers>/JOURNALMODE-<wid>` records the mode SQLite actually reported, so
  the driver asserts the arm it believes it is running (LANE-BRIEF section 3b-ii).
"""

import os
import sqlite3
import sys
import time


def publish_progress(path: str, n: int) -> None:
    """Publish `n` so a concurrent reader can NEVER observe a partial value.

    `open(path, "w")` truncates first, so the file is zero bytes until the
    write lands. A reader in that window sees an empty file. `os.replace` is
    atomic on POSIX and on Windows, so the reader sees either the old value or
    the new one — never nothing. Exported so the self-test can drive it.
    """
    tmp = f"{path}.tmp"
    with open(tmp, "w") as fh:
        fh.write(str(n))
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp, path)


def publish_progress_TRUNCATING(path: str, n: int) -> None:
    """The BROKEN publisher this harness shipped with, retained as evidence.

    The self-test asserts that this one CAN be caught mid-truncation and the
    repaired one cannot. Without that third assertion the self-test would pass
    on the broken instrument too, which is the failure mode LANE-BRIEF section
    6b-ii names. Never called by the writer.
    """
    with open(path, "w") as fh:
        time.sleep(0.002)  # the truncate/write window, widened to be observable
        fh.write(str(n))
        fh.flush()
        os.fsync(fh.fileno())


def main() -> int:
    db_path, wid, markers, duration_s = (
        sys.argv[1],
        sys.argv[2],
        sys.argv[3],
        float(sys.argv[4]),
    )
    # Optional 5th argument: the journal mode to demand. Defaults to WAL, which
    # is what every existing caller means. `truncate` is the mode
    # `wcore_config::sqlite_journal` selects on a NETWORK FILESYSTEM, where WAL's
    # shared-memory wal-index is unavailable — so it is the arm that exercises
    # the `-journal` sidecar rather than the `-wal`/`-shm` pair.
    want = (sys.argv[5] if len(sys.argv) > 5 else "wal").lower()

    conn = sqlite3.connect(db_path, timeout=30.0, isolation_level=None)
    mode = conn.execute(f"PRAGMA journal_mode={want}").fetchone()[0]
    with open(os.path.join(markers, f"JOURNALMODE-{wid}"), "w") as fh:
        fh.write(str(mode))
    if str(mode).lower() != want:
        # Do NOT write START. A writer in the wrong mode is not the experiment.
        sys.stderr.write(
            f"writer {wid}: journal_mode={mode}, wanted {want}, refusing to run\n"
        )
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

        publish_progress(progress_path, n)

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
