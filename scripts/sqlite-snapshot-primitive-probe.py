#!/usr/bin/env python3
"""Which SQLite snapshot primitive is safe for THIS schema? Measured, not voted.

The F26-SC3-O1 cross-audit panel split 2-1 on the design, and the split was over
FACTS rather than preference, so the tie is broken by measurement.

`wcore-memory`'s schema v1 declares:

    CREATE TABLE episodes (id TEXT PRIMARY KEY, ...)          -- IMPLICIT rowid
    CREATE VIRTUAL TABLE episodes_fts USING fts5(
        summary, atomic_facts, content='episodes', content_rowid='rowid')

That is an external-content FTS5 index keyed on an IMPLICIT rowid, and SQLite
documents VACUUM as free to renumber implicit rowids
(https://sqlite.org/lang_vacuum.html#how_vacuum_works). If it does, the restored
database passes `integrity_check` and every full-text search silently returns the
WRONG ROWS — a quieter defect than the torn read being fixed.

This probe reproduces that exact schema and asks two primitives the same
question:

  P1  VACUUM INTO       — does it renumber rowids and desync the FTS index?
  P2  sqlite3_backup    — (python exposes it as Connection.backup) same question.

It deliberately includes a KNOWN-POSITIVE: it first proves the FTS index is
genuinely live on the source (a search returns the right row). Without that, a
"search returned nothing" result in either arm would be indistinguishable from
an index that never worked.
"""

from __future__ import annotations

import os
import sqlite3
import tempfile
from pathlib import Path

SCHEMA = """
CREATE TABLE episodes (
    id TEXT PRIMARY KEY,
    summary TEXT NOT NULL,
    atomic_facts TEXT NOT NULL DEFAULT '[]'
);
CREATE VIRTUAL TABLE episodes_fts USING fts5(
    summary, atomic_facts, content='episodes', content_rowid='rowid');
CREATE TRIGGER episodes_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episodes_fts (rowid, summary, atomic_facts)
    VALUES (new.rowid, new.summary, new.atomic_facts);
END;
"""


def build_source(path: Path) -> tuple[int, str]:
    conn = sqlite3.connect(str(path), isolation_level=None)
    conn.executescript(SCHEMA)
    for i in range(1, 2001):
        conn.execute(
            "INSERT INTO episodes (id, summary) VALUES (?, ?)",
            (f"ep-{i:05d}", f"episode number {i} about topic{i % 7}"),
        )
    # Deletions leave rowid gaps. Gaps are what VACUUM compacts away.
    conn.execute("DELETE FROM episodes WHERE rowid % 3 != 0")
    conn.execute("INSERT INTO episodes (id, summary) VALUES ('ep-needle', 'zqxjkv unique needle token')")
    needle_rowid = conn.execute(
        "SELECT rowid FROM episodes WHERE id = 'ep-needle'"
    ).fetchone()[0]
    conn.commit()
    conn.close()
    return needle_rowid, "ep-needle"


def fts_lookup(path: Path) -> tuple[str | None, int]:
    """Search the FTS index for the needle; return the id it resolves to."""
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        row = conn.execute(
            "SELECT e.id FROM episodes_fts f JOIN episodes e ON e.rowid = f.rowid"
            " WHERE episodes_fts MATCH 'zqxjkv'"
        ).fetchone()
        max_rowid = conn.execute("SELECT MAX(rowid) FROM episodes").fetchone()[0]
        return (row[0] if row else None), max_rowid
    finally:
        conn.close()


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="wl-sqlite-primitive-probe-"))
    src = tmp / "memory.db"
    needle_rowid, needle_id = build_source(src)

    # --- KNOWN-POSITIVE: the index works on the source ----------------------
    found, src_max = fts_lookup(src)
    print(f"SOURCE-NEEDLE-ROWID: {needle_rowid}")
    print(f"SOURCE-MAX-ROWID: {src_max}")
    print(f"SOURCE-FTS-RESOLVES-TO: {found}")
    if found != needle_id:
        print("VERDICT: ABORTED-INSTRUMENT-DEAD (the FTS index never worked on the source)")
        return 2

    results: dict[str, dict[str, object]] = {}

    # --- P1: VACUUM INTO ----------------------------------------------------
    vac = tmp / "vacuumed.db"
    conn = sqlite3.connect(str(src))
    try:
        conn.execute("VACUUM INTO ?", (str(vac),))
        v_found, v_max = fts_lookup(vac)
        v_integrity = sqlite3.connect(str(vac)).execute(
            "PRAGMA integrity_check"
        ).fetchone()[0]
        results["VACUUM-INTO"] = {
            "max_rowid": v_max,
            "fts_resolves_to": v_found,
            "integrity": v_integrity,
        }
    except sqlite3.Error as exc:
        results["VACUUM-INTO"] = {"error": f"{type(exc).__name__}: {exc}"}
    finally:
        conn.close()

    # --- P2: sqlite3_backup (page-level) ------------------------------------
    bak = tmp / "backed-up.db"
    sconn = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
    dconn = sqlite3.connect(str(bak))
    try:
        sconn.backup(dconn)  # one call; copies every page
        dconn.close()
        b_found, b_max = fts_lookup(bak)
        b_integrity = sqlite3.connect(str(bak)).execute(
            "PRAGMA integrity_check"
        ).fetchone()[0]
        results["SQLITE3-BACKUP"] = {
            "max_rowid": b_max,
            "fts_resolves_to": b_found,
            "integrity": b_integrity,
        }
    except sqlite3.Error as exc:
        results["SQLITE3-BACKUP"] = {"error": f"{type(exc).__name__}: {exc}"}
    finally:
        sconn.close()

    print("---- PRIMITIVE PROBE ----")
    print(f"SQLITE-VERSION: {sqlite3.sqlite_version}")
    verdicts = []
    for name, r in results.items():
        print(f"{name}: {r}")
        ok = (
            r.get("integrity") == "ok"
            and r.get("fts_resolves_to") == needle_id
            and r.get("max_rowid") == src_max
        )
        verdicts.append((name, ok))
        print(f"{name}-PRESERVES-ROWID-IDENTITY: {ok}")
    print("VERDICT: " + ", ".join(f"{n}={'SAFE' if ok else 'UNSAFE'}" for n, ok in verdicts))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
