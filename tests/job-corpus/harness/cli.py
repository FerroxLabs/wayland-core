"""Corpus driver.

    python3 -m harness.cli run --binary /path/to/wayland-core \
        --out /tmp/run-2026-08-10 [--row A-2 --row A-3] [--rows-dir rows]
    python3 -m harness.cli summarise --out /tmp/run-2026-08-10
    python3 -m harness.cli selftest

Row contract (deliberately thin, so row authors own their own fixtures):

    rows/a2_issue_to_pr.py
        ROW_ID = "A-2"                   # must be on the roster in result.py
        TIER = "A"
        TITLE = "issue/spec -> tested, review-ready change"
        FIXTURE = "fixtures/a2"          # relative to tests/job-corpus/
        KEY = "keys/a2_issue_to_pr/key.json"   # the rubric, pinned by sha256
        DECLARED_SCOPE = ["src/parser.rs", "tests/parser_test.rs"]
        TEST_COMMAND = ["cargo", "test", "-q"]      # optional
        TIMEOUT = 1200                              # optional
        SCOPE_IGNORE = []                # optional, per-row, EMPTY by default

        def run(ctx):                    # ctx is a harness.RowContext
            ctx.run(["--print", "..."])
            ctx.expect(..., "A-2.pr-open", "a pull request is open for the change")

A row may instead export `def main(binary, artifact_dir) -> RowRecord` when it
needs a topology RowContext does not model (multiple hosts, a killed process,
an external message).  Either way the driver only ever sees a RowRecord.

Two declarations are MANDATORY and are validated before anything runs, so a
row that forgot them fails loudly at load rather than quietly at grade time:

  KEY              the rubric file.  Its sha256 is pinned into the record, so
                   a result always names the exact bytes that graded it.
  DECLARED_SCOPE   what the user asked to be changed.  A row that legitimately
                   changes nothing sets DECLARED_SCOPE = [] and states
                   SCOPE_NOT_APPLICABLE = "why" — and then ANY change fails
                   INV-4, which is the point.

THE PROVIDER ENDPOINT.  RowContext puts a harness-owned recording endpoint
between the product and its provider and writes a config pointing at it into
the row's isolated HOME, so by default every row is already in INV-1's view and
already feeding INV-5's meter.  A row that writes its own provider config MUST
point it at `ctx.provider_base_url`; a row that needs real model behaviour sets
`leak_upstream=` (or exports JOB_CORPUS_UPSTREAM_BASE_URL) and the endpoint
relays verbatim while still recording.  A row that does neither takes itself out
of observation, and INV-1 reports that as UNPROVEN by name rather than passing
quietly.  `ctx.scenario(...)` scripts the endpoint's answers for rows that want
a deterministic model.
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

from .result import (
    GATE_ROSTER,
    ROSTER_KINDS,
    UNPROVEN,
    Check,
    HarnessError,
    RowRecord,
    exit_code_for,
    summarise,
)
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


_SENTINEL = object()


def validate_row_module(mod: Any, path: str) -> Dict[str, Any]:
    """Reject a row that cannot be graded, BEFORE anything is executed.

    A missing scope declaration used to become a silent UNPROVEN on INV-4 at
    grade time — which is how INV-4 came to be UNPROVEN on 100% of rows while
    the run still looked like it had run.  It is now a load-time error.
    """
    row_id = str(getattr(mod, "ROW_ID", "") or "").strip()
    if not row_id:
        raise HarnessError("%s: the row declares no ROW_ID" % path)
    if row_id.upper() not in ROSTER_KINDS:
        raise HarnessError(
            "%s: ROW_ID %r is not on the 22-gate roster in harness/result.py; "
            "add it to the roster or fix the id" % (path, row_id)
        )

    key = getattr(mod, "KEY", None)
    if not key:
        raise HarnessError(
            "%s (%s): the row names no KEY. A result that cannot name the rubric "
            "that graded it proves nothing." % (path, row_id)
        )
    key_path = key if os.path.isabs(key) else os.path.join(CORPUS_ROOT, key)
    if not os.path.isfile(key_path):
        raise HarnessError("%s (%s): KEY %s does not exist" % (path, row_id, key_path))

    scope = getattr(mod, "DECLARED_SCOPE", _SENTINEL)
    exempt = getattr(mod, "SCOPE_NOT_APPLICABLE", None)
    if scope is _SENTINEL:
        raise HarnessError(
            "%s (%s): the row declares no DECLARED_SCOPE. Declare the paths the "
            "user asked to be changed, or set DECLARED_SCOPE = [] together with "
            "SCOPE_NOT_APPLICABLE = '<why this row changes nothing>'."
            % (path, row_id)
        )
    scope = list(scope)
    if not scope and not exempt:
        raise HarnessError(
            "%s (%s): DECLARED_SCOPE is empty and SCOPE_NOT_APPLICABLE is unset. "
            "An undefined scope makes 'unrelated' undefinable and INV-4 vacuous."
            % (path, row_id)
        )
    return {
        "row_id": row_id,
        "key_path": key_path,
        "key_sha256": sha256_file(key_path),
        "declared_scope": scope,
        "scope_not_applicable": exempt,
    }


def run_row(path: str, binary: str, out_dir: str) -> Dict[str, Any]:
    mod = _load_module(path)
    spec = validate_row_module(mod, path)
    row_id = spec["row_id"]
    artifact_dir = os.path.join(out_dir, row_id.replace("/", "-"))
    os.makedirs(artifact_dir, exist_ok=True)

    if hasattr(mod, "main"):
        record: RowRecord = mod.main(binary, artifact_dir)
        # A hand-rolled row still has to name its rubric.
        record.key_path = record.key_path or spec["key_path"]
        record.key_sha256 = record.key_sha256 or spec["key_sha256"]
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
        declared_scope=spec["declared_scope"],
        scope_not_applicable=spec["scope_not_applicable"],
        scope_ignore_extra=getattr(mod, "SCOPE_IGNORE", ()),
        test_authoring_globs=getattr(mod, "TEST_AUTHORING_GLOBS", ()),
        key_path=spec["key_path"],
        key_sha256=spec["key_sha256"],
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

    if not os.path.isdir(rows_dir):
        print(
            "harness: there is no rows directory at %s — the corpus has no drivers, "
            "so it would grade nothing" % rows_dir,
            file=sys.stderr,
        )
        return 3

    paths = discover(rows_dir)
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

    # PRE-FLIGHT.  A row that cannot be graded is a harness misconfiguration,
    # not a product result, so it stops the run before the product is started
    # instead of arriving later as one more UNPROVEN nobody reads.
    problems: List[str] = []
    for path in paths:
        try:
            validate_row_module(_load_module(path), path)
        except HarnessError as exc:
            problems.append(str(exc))
        except Exception:
            problems.append("%s: the row module would not import:\n%s" % (path, traceback.format_exc()[-1500:]))
    if problems:
        print("harness: %d row(s) cannot be graded as written:" % len(problems), file=sys.stderr)
        for p in problems:
            print("  - " + p, file=sys.stderr)
        return 2

    records: List[Dict[str, Any]] = []
    for path in paths:
        name = os.path.basename(path)
        print("harness: running %s" % name, flush=True)
        try:
            rec = run_row(path, binary, out_dir)
        except Exception:
            rec = {
                "schema": "wayland-core/job-corpus/row-record/2",
                "row_id": _row_id_of(path),
                "verdict": UNPROVEN,
                "artifact": {"path": binary, "sha256": sha256_file(binary)},
                "key": {"path": None, "sha256": None},
                "notes": ["harness crash: " + traceback.format_exc()[-4000:]],
            }
        records.append(rec)
        print("harness:   %s -> %s" % (rec.get("row_id"), rec.get("verdict")), flush=True)

    summary = summarise(records)
    with open(os.path.join(out_dir, "summary.json"), "w", encoding="utf-8") as fh:
        json.dump(summary, fh, indent=2, sort_keys=True)
        fh.write("\n")
    _report(summary)
    return exit_code_for(summary)


def _row_id_of(path: str) -> str:
    """Best-effort row id for a module that could not be loaded or validated."""
    try:
        mod = _load_module(path)
        rid = str(getattr(mod, "ROW_ID", "") or "").strip()
        if rid:
            return rid
    except Exception:
        pass
    return os.path.splitext(os.path.basename(path))[0]


def _report(summary: Dict[str, Any]) -> None:
    """Print the roster, not just the tally.

    A three-row run and a twenty-two-row run must not look alike, so the
    unreached gates are printed by name every time.
    """
    print(json.dumps(summary["counts"], sort_keys=True))
    print(
        "harness: %s — %s" % (summary["run_disposition"], summary["coverage"]),
        flush=True,
    )
    for entry in summary["roster"]:
        print("harness:   %-6s %-14s %s" % (entry["gate"], entry["state"], entry["what_the_user_gets"]))
    never = summary.get("gates_never_reached") or []
    if never:
        print(
            "harness: %d gate(s) were NEVER REACHED and measured nothing: %s"
            % (len(never), ", ".join(never)),
            file=sys.stderr,
        )
    if summary.get("unknown_gates"):
        print(
            "harness: record(s) for rows that are not on the roster: %s"
            % ", ".join(summary["unknown_gates"]),
            file=sys.stderr,
        )


def cmd_summarise(args: argparse.Namespace) -> int:
    out_dir = os.path.abspath(args.out)
    records = []
    for path in sorted(glob.glob(os.path.join(out_dir, "*", "record.json"))):
        with open(path, "r", encoding="utf-8") as fh:
            records.append(json.load(fh))
    summary = summarise(records)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return exit_code_for(summary)


def cmd_roster(args: argparse.Namespace) -> int:
    """Print the declared roster.  Nothing may run that is not on it."""
    for gate, kind, why in GATE_ROSTER:
        print("%-6s %-10s %s" % (gate, kind, why))
    print("\n%d gates declared" % len(GATE_ROSTER))
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

    g = sub.add_parser("roster", help="print the 22 declared gates")
    g.set_defaults(func=cmd_roster)

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
