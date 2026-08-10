"""Corpus driver.

    python3 -m harness.cli run --binary /path/to/wayland-core \
        --out /tmp/run-2026-08-10 [--row A-2 --row A-3] [--rows-dir rows]
    python3 -m harness.cli summarise --out /tmp/run-2026-08-10
    python3 -m harness.cli selftest

Row contract (deliberately thin, so row authors own their own fixtures):

    rows/a2_issue_to_pr.py
        ROW_ID = "A-2"
        TIER = "A"
        TITLE = "issue/spec -> tested, review-ready change"
        FIXTURE = "fixtures/a2"          # relative to tests/job-corpus/
        DECLARED_SCOPE = ["src/parser.rs", "tests/parser_test.rs"]
        TEST_COMMAND = ["cargo", "test", "-q"]      # optional
        TIMEOUT = 1200                              # optional

        def run(ctx):                    # ctx is a harness.RowContext
            ctx.run(["--print", "..."])
            ctx.expect(..., "A-2.pr-open", "a pull request is open for the change")

A row may instead export `def main(binary, artifact_dir) -> RowRecord` when it
needs a topology RowContext does not model (multiple hosts, a killed process,
an external message).  Either way the driver only ever sees a RowRecord.
"""

from __future__ import annotations

import argparse
import glob
import importlib.util
import json
import os
import sys
import traceback
from typing import Any, Dict, List, Optional

from .result import FAIL, UNPROVEN, Check, RowRecord, summarise
from .rowctx import RowContext
from .world import sha256_file

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS_ROOT = os.path.dirname(HERE)


def _load_module(path: str):
    name = "jobcorpus_row_" + os.path.splitext(os.path.basename(path))[0]
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ImportError("cannot load row module at %s" % path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def discover(rows_dir: str) -> List[str]:
    return sorted(
        p
        for p in glob.glob(os.path.join(rows_dir, "*.py"))
        if not os.path.basename(p).startswith("_")
    )


def run_row(path: str, binary: str, out_dir: str) -> Dict[str, Any]:
    mod = _load_module(path)
    row_id = getattr(mod, "ROW_ID", os.path.splitext(os.path.basename(path))[0])
    artifact_dir = os.path.join(out_dir, row_id.replace("/", "-"))
    os.makedirs(artifact_dir, exist_ok=True)

    if hasattr(mod, "main"):
        record: RowRecord = mod.main(binary, artifact_dir)
        return record.to_dict()

    fixture = getattr(mod, "FIXTURE", None)
    if fixture and not os.path.isabs(fixture):
        fixture = os.path.join(CORPUS_ROOT, fixture)
    ctx_kwargs = dict(
        row_id=row_id,
        binary=binary,
        artifact_dir=artifact_dir,
        fixture=fixture,
        workspace=getattr(mod, "WORKSPACE", None),
        declared_scope=getattr(mod, "DECLARED_SCOPE", ()),
        test_command=getattr(mod, "TEST_COMMAND", None),
        timeout=getattr(mod, "TIMEOUT", 900),
        tier=getattr(mod, "TIER", ""),
        title=getattr(mod, "TITLE", ""),
    )
    with RowContext(**ctx_kwargs) as ctx:
        try:
            mod.run(ctx)
        except Exception:  # a row that crashes is UNPROVEN, never PASS
            ctx.add_check(
                Check(
                    row_id + ".driver",
                    UNPROVEN,
                    "the row driver raised before it could observe the outcome",
                    {"traceback": traceback.format_exc()[-4000:]},
                )
            )
    return ctx.record.to_dict()


def cmd_run(args: argparse.Namespace) -> int:
    binary = os.path.abspath(args.binary)
    if not os.path.isfile(binary):
        print("harness: no binary at %s" % binary, file=sys.stderr)
        return 2
    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    rows_dir = os.path.abspath(args.rows_dir or os.path.join(CORPUS_ROOT, "rows"))

    print("harness: artifact %s" % binary)
    print("harness: sha256   %s" % sha256_file(binary))

    paths = discover(rows_dir) if os.path.isdir(rows_dir) else []
    if args.row:
        wanted = {r.lower() for r in args.row}
        paths = [
            p
            for p in paths
            if os.path.splitext(os.path.basename(p))[0].lower() in wanted
            or (getattr(_load_module(p), "ROW_ID", "") or "").lower() in wanted
        ]
    if not paths:
        print("harness: no rows matched under %s" % rows_dir, file=sys.stderr)
        return 3

    records: List[Dict[str, Any]] = []
    for path in paths:
        name = os.path.basename(path)
        print("harness: running %s" % name, flush=True)
        try:
            rec = run_row(path, binary, out_dir)
        except Exception:
            rec = {
                "schema": "wayland-core/job-corpus/row-record/1",
                "row_id": os.path.splitext(name)[0],
                "verdict": UNPROVEN,
                "artifact": {"path": binary, "sha256": sha256_file(binary)},
                "notes": ["harness crash: " + traceback.format_exc()[-4000:]],
            }
        records.append(rec)
        print("harness:   %s -> %s" % (rec.get("row_id"), rec.get("verdict")), flush=True)

    summary = summarise(records)
    with open(os.path.join(out_dir, "summary.json"), "w", encoding="utf-8") as fh:
        json.dump(summary, fh, indent=2, sort_keys=True)
        fh.write("\n")
    print(json.dumps(summary["counts"], sort_keys=True))
    return 1 if summary["counts"].get(FAIL) else 0


def cmd_summarise(args: argparse.Namespace) -> int:
    out_dir = os.path.abspath(args.out)
    records = []
    for path in sorted(glob.glob(os.path.join(out_dir, "*", "record.json"))):
        with open(path, "r", encoding="utf-8") as fh:
            records.append(json.load(fh))
    summary = summarise(records)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


def cmd_selftest(args: argparse.Namespace) -> int:
    from .selftest import main as selftest_main

    return selftest_main(
        verbose=args.verbose, binary=args.binary, real_stream=args.real_stream
    )


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(prog="harness", description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    r = sub.add_parser("run", help="run corpus rows against a built binary")
    r.add_argument("--binary", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--rows-dir", default=None)
    r.add_argument("--row", action="append", default=[])
    r.set_defaults(func=cmd_run)

    s = sub.add_parser("summarise", help="re-aggregate an existing run directory")
    s.add_argument("--out", required=True)
    s.set_defaults(func=cmd_summarise)

    t = sub.add_parser("selftest", help="positive-control every invariant check")
    t.add_argument("-v", "--verbose", action="store_true")
    t.add_argument("--binary", default=None, help="also drive the real product binary")
    t.add_argument(
        "--real-stream",
        default=None,
        help="a .jsonl of real product output, to ground the claim parser",
    )
    t.set_defaults(func=cmd_selftest)

    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
