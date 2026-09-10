#!/usr/bin/env python3
"""Retry-flake POPULATION census. FerroxLabs/wayland#1286 c1, clause 2.

WHAT THIS IS, AND WHY IT IS NOT grade-retry-flakes.sh
-----------------------------------------------------
`.github/scripts/grade-retry-flakes.sh` is REACTIVE by construction. It grades
the JUnit of ONE run, in the `report` job of that run, and the only signal that
ever reaches a human is that gate reddening because an UNLISTED test happened to
flake in the run somebody was looking at. Discovery is therefore one member per
CI cycle, and `.config/flaky-allowlist.txt` ends up tracking whichever member
fired rather than the population. That is exactly what wayland#1286 c1 names.

This script inverts the direction. It measures the POPULATION on a schedule:
enumerate every `ci.yml` run in a window, download every `nextest-junit-*`
artifact, deduplicate every XML by sha256, and count `<flakyFailure>` /
`<flakyError>` per test per leg against a denominator of EXECUTIONS. Off that
census it produces the two outputs nothing currently emits:

  * COVERAGE MISSES - every test with incidence > 0 and no live allowlist line,
    reported with its rate BEFORE it happens to redden a run.
  * DEAD ENTRIES    - every allowlist line with zero observations across the
    window, flagged for DELETION at expiry rather than renewal.

It automates the hand pass recorded in `.planning/FLAKE-CENSUS-20260910.md`.
Every trap that pass hit the hard way is encoded below, in the OUTPUT and not
only in a comment, because a comment does not stop a reader misreading a table.

THE FOUR VACUOUS-ZERO TRAPS THIS TOOL MUST NOT REINTRODUCE
----------------------------------------------------------
(1) A ZERO FOR A TEST ON A LEG WHERE THAT TEST NEVER EXECUTED IS A REPORT OF
    ABSENCE, NOT OF HEALTH. `harness_tui_flow`, `f14_sigkill_recovery` and
    `wcore-mcp::transport::stdio::tests` execute ZERO times across all 119
    Windows executions in the 2026-09-10 window, so every "Windows: 0" for them
    is a statement about the test filter and not about Windows. Every cell this
    tool emits therefore carries BOTH numbers - `flaked / executions-containing-
    this-test` - and a cell whose denominator is 0 is printed `VACUOUS (never
    executed)`, never `0`. The same rule is applied to allowlist entries: a dead
    entry whose test never ran is reported as UNOBSERVABLE, not as deletable.

(2) ARTIFACT RETENTION SILENTLY SHRINKS THE DENOMINATOR. An expired artifact is
    indistinguishable from a leg that did not flake: both contribute nothing to
    the numerator, but only one of them should also have contributed to the
    denominator. Past some expiry fraction the census is measuring GitHub's
    retention policy rather than the product's flakiness. This tool counts
    expired and unreadable artifacts, prints that count at the TOP of the
    report, and exits DEGENERATE above --max-expired-fraction (see the constant
    for the justification of the default).

(3) A TEST EXCLUDED FROM THE MAIN STEP HAS A DIFFERENT DENOMINATOR.
    `wcore-tools::walk_parallel_identity_test::redundant_walk_root_is_not_walked_twice`
    has run in an isolated macOS step at `--retries 0 --test-threads 1` since
    2026-09-07T16:20Z, with the main `Run tests` step EXCLUDING the binary, so
    its macOS denominator is 85 and not 104. Nothing special is coded for it:
    per-(test, leg) denominators are derived from PRESENCE IN THE REPORT, which
    handles this case and every future one automatically. The report says so
    out loud, so nobody reads the smaller denominator as a bug in this tool.

(4) INCIDENCE UNDER `retries = 2` IS NOT A `--retries 0` FAILURE RATE, AND A
    CENSUS CAN NEVER PRODUCE ONE. Everything readable from CI history is
    per-execution incidence under the `ci` profile's two retries. Turning that
    into a rate needs a bounded job running the pinned list at `--retries 0`
    with n >= 20 per test per platform. Every rate emitted here is labelled, and
    the Markdown header carries the sentence.

THE DEDUP BUG, WHICH WAS REAL
-----------------------------
A `nextest-junit-*` artifact holds `junit.xml` (the final outer attempt) plus
`outer-attempts/outer-attempt-N.xml`, and when the final attempt also has a
numbered copy the two are BYTE-IDENTICAL. Summing both double-counts the
execution and its flakes. Separately, `ci.yml` uploads the containerized Linux
report TWICE - `nextest-junit-linux-containerized-checkpoint` mid-job and
`nextest-junit-linux-containerized` at the end - and those two artifacts are
byte-identical as well (verified on run 34438211271: both 1272282 bytes,
sha256 314c11cf139b...). Both duplications are removed by hashing every XML and
keeping the first occurrence WITHIN A (run, leg). Dedup is deliberately NOT
global: two different legs, or two different runs, producing byte-identical XML
would be two real executions, and collapsing them would shrink the denominator
in the same fail-open direction trap (2) describes.

`.github/scripts/grade-retry-flakes.sh` still globs `-name "*.xml"` with no
dedup and so still misreports the ATTEMPT COUNT in its message (its per-test
verdict is unaffected). Not fixed here; this file must simply not repeat it.

UNITS, STATED ONCE
------------------
  run       - one `ci.yml` workflow run.
  leg       - one matrix cell: `macos-latest`, `linux-containerized`, `Array`
              (the self-hosted Windows box; `${{ matrix.os }}` renders a YAML
              list as the literal string "Array"), `windows-latest-hosted`.
  EXECUTION - one nextest invocation that produced a JUnit report holding at
              least one `<testcase>`. This is the DENOMINATOR. It is not runs
              and not jobs: the containerized Linux leg is wrapped in an outer
              retry, and each preserved outer attempt is a separate nextest
              invocation with its own report.
  attempt   - one `<flakyFailure>` / `<flakyError>` element, which is exactly
              one `TRY n FAIL` line in the job log. That equivalence was
              verified against job logs in the hand pass, not assumed.

REQUIREMENTS: python3 (stdlib only) and an authenticated `gh` CLI. No pip deps,
because this runs on a hosted runner with nothing provisioned.

NO TOKEN IS EVER READ, PRINTED OR WRITTEN BY THIS SCRIPT. Authentication is
entirely `gh`'s business; nothing here touches GH_TOKEN or a credential file.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import zipfile
from collections import defaultdict
from typing import Any
from xml.etree import ElementTree

# ---------------------------------------------------------------------------
# Constants that encode a judgement. Each one says what it would cost to be
# wrong, because a bare number in a script is a number nobody can review.
# ---------------------------------------------------------------------------

ARTIFACT_PREFIX = "nextest-junit-"

# `ci.yml` uploads the containerized Linux JUnit twice under two artifact names.
# Both are the same leg and (verified) the same bytes; folding the suffix here
# is what lets the (run, leg) dedup below see them as one execution instead of
# two. Kept as an explicit list rather than a regex so that adding a third
# upload point is a deliberate edit and not a silent widening.
LEG_NAME_FOLDS = {
    "linux-containerized-checkpoint": "linux-containerized",
}

# The four legs the hand census measured. Used ONLY to order the table columns;
# a leg that appears in the artifacts and is not on this list is still counted
# and still printed, appended after these. A hardcoded leg list that silently
# DROPPED an unknown leg would be trap (1) applied to a whole platform.
KNOWN_LEG_ORDER = [
    "macos-latest",
    "linux-containerized",
    "Array",
    "windows-latest-hosted",
]

# Trap (2). Above this fraction of expired/unreadable artifacts the census is
# reporting GitHub's 90-day retention rather than the product's flakiness, and
# every "0" it prints is contaminated by artifacts that were never read.
#
# WHY 0.10: the hand pass over 2026-08-31..2026-09-10 downloaded 338 of 338
# artifacts with ZERO expired, because a 10-day window sits far inside
# retention. A weekly cron over a 7-14 day window should therefore see exactly
# 0. Any non-trivial expiry fraction means either the window was widened past
# retention or artifacts are being deleted, and in both cases the denominator is
# quietly wrong. 0.10 is loose enough that one flaky download does not fail the
# job (1 of 338 = 0.3 %) and tight enough that a window reaching back past
# retention - where the expiry fraction climbs to tens of percent - always does.
DEFAULT_MAX_EXPIRED_FRACTION = 0.10

# Positive control (trap: a parser that silently matches nothing reports perfect
# health). The hand pass measured 30 of 104 macos-latest executions carrying at
# least one retry-masked failure, and 47 distinct tests workspace-wide. Against
# a base rate anywhere near that, a census covering a real number of executions
# and finding ZERO flakes is far more likely a broken XML path than a clean
# fortnight.
#
# WHY 60: at a deliberately pessimistic 5 % per-execution incidence - six times
# lower than the measured macOS figure - the chance of observing zero across 60
# executions is 0.95^60 ~= 4.6 %. Below 60 executions zero is unremarkable and
# this fires only a warning; at or above it, zero is treated as degenerate and
# the run fails, because reporting "no flakes" off a parser that matched nothing
# is the single worst output this tool could produce.
POSITIVE_CONTROL_MIN_EXECUTIONS = 60

EXIT_OK = 0
EXIT_DEGENERATE = 2


# ---------------------------------------------------------------------------
# gh plumbing
# ---------------------------------------------------------------------------


def gh_json(path: str) -> Any:
    """One `gh api` call returning parsed JSON.

    Raises on a non-zero exit ON PURPOSE. A silently-swallowed API error shows
    up downstream as an empty run list, and an empty run list reads as "nothing
    flaked" - the failure mode this whole file exists to make impossible. Every
    failure here must be loud.
    """
    cmd = ["gh", "api", "-H", "Accept: application/vnd.github+json", path]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"gh api {path} failed (rc={proc.returncode}): {proc.stderr.strip()[:800]}"
        )
    return json.loads(proc.stdout)


def gh_download_artifact(repo: str, artifact_id: int, dest: str) -> None:
    """Download one artifact zip via `gh api`, which follows the 302 for us."""
    tmp = dest + ".part"
    with open(tmp, "wb") as fh:
        proc = subprocess.run(
            ["gh", "api", f"repos/{repo}/actions/artifacts/{artifact_id}/zip"],
            stdout=fh,
            stderr=subprocess.PIPE,
            text=False,
        )
    if proc.returncode != 0:
        os.unlink(tmp)
        raise RuntimeError(
            f"artifact {artifact_id} download failed (rc={proc.returncode}): "
            + proc.stderr.decode("utf-8", "replace").strip()[:400]
        )
    os.replace(tmp, dest)


# ---------------------------------------------------------------------------
# Run enumeration
# ---------------------------------------------------------------------------


def parse_iso(value: str) -> dt.datetime:
    """Parse an ISO-8601 instant or a bare YYYY-MM-DD date, always as UTC."""
    text = value.strip()
    if re.fullmatch(r"\d{4}-\d{2}-\d{2}", text):
        return dt.datetime.strptime(text, "%Y-%m-%d").replace(tzinfo=dt.timezone.utc)
    return dt.datetime.fromisoformat(text.replace("Z", "+00:00")).astimezone(
        dt.timezone.utc
    )


def list_runs(repo: str, workflow: str, since: dt.datetime, until: dt.datetime,
              max_runs: int) -> tuple[list[dict], dict[str, int]]:
    """Every completed run of `workflow` created within [since, until].

    Pages newest-first and stops as soon as a page's oldest run predates
    `since`; the API returns runs in descending `created_at`, so nothing older
    can appear later. Runs that are not `completed` are EXCLUDED and counted:
    an in-flight run has not uploaded its artifacts yet, so including it would
    add an artifact-less run to the denominator bookkeeping for no reason.
    """
    runs: list[dict] = []
    skipped_incomplete = 0
    page = 1
    while True:
        batch = gh_json(
            f"repos/{repo}/actions/workflows/{workflow}/runs"
            f"?per_page=100&page={page}"
        )
        items = batch.get("workflow_runs", [])
        if not items:
            break
        oldest = None
        for run in items:
            created = parse_iso(run["created_at"])
            oldest = created if oldest is None else min(oldest, created)
            if created > until or created < since:
                continue
            if run.get("status") != "completed":
                skipped_incomplete += 1
                continue
            runs.append(
                {
                    "id": run["id"],
                    "created_at": run["created_at"],
                    "head_branch": run.get("head_branch"),
                    "head_sha": run.get("head_sha"),
                    "conclusion": run.get("conclusion"),
                }
            )
            if max_runs and len(runs) >= max_runs:
                return runs, {"skipped_incomplete": skipped_incomplete}
        if oldest is not None and oldest < since:
            break
        page += 1
        if page > 40:  # 4000 runs; a window that large is a mistake, not a census.
            break
    return runs, {"skipped_incomplete": skipped_incomplete}


# ---------------------------------------------------------------------------
# JUnit parsing
# ---------------------------------------------------------------------------


def parse_junit(blob: bytes) -> tuple[dict[str, int], int]:
    """Return {test_key: flaky_attempt_count} for one report, and its testcase count.

    A test PRESENT in the report but not flaky maps to 0 - that is the whole
    point, and it is what makes the per-(test, leg) denominator fall out of
    presence rather than out of a hardcoded expectation (trap 3).

    `classname::name` is the key, byte-for-byte what nextest prints on its own
    `FLAKY` line and what `.config/flaky-allowlist.txt` stores, so a key from
    here pastes straight into the allowlist or into a `-E 'test(=...)'` filter.
    """
    results: dict[str, int] = {}
    total = 0
    try:
        root = ElementTree.fromstring(blob)
    except ElementTree.ParseError as exc:
        raise ValueError(f"unparseable JUnit XML: {exc}") from exc
    for case in root.iter("testcase"):
        cls = case.get("classname") or ""
        name = case.get("name") or ""
        if not name:
            continue
        key = f"{cls}::{name}" if cls else name
        total += 1
        flaky = 0
        for child in case:
            # `flakyFailure` and `flakyError` only. A plain `<failure>` is a
            # HARD failure - the run is red on its own account and something
            # else already reports it - and counting it here would inflate the
            # flake population with ordinary reds.
            if child.tag in ("flakyFailure", "flakyError"):
                flaky += 1
        results[key] = results.get(key, 0) + flaky
    return results, total


# ---------------------------------------------------------------------------
# Census
# ---------------------------------------------------------------------------


def build_census(args: argparse.Namespace, watch_keys: set[str]) -> dict:
    """Measure the window. `watch_keys` are tests whose PRESENCE must be recorded
    even if they never flaked - i.e. the allowlist keys.

    Without this the DEAD ENTRIES output is vacuous by construction: an entry
    that never fired would have no presence row, and 'no presence row' would be
    reported as 'never ran', which is trap (1) one level up. Presence is
    accumulated for EVERY test the reports contain (a counter per key, ~20k
    keys, cheap) and only these keys plus the flaky population are EMITTED, so
    the JSON stays readable while the dead-entry verdict stays honest.
    """
    since = parse_iso(args.since)
    until = parse_iso(args.until)

    print(f"-- flake census (wayland#1286) ----------------------------------")
    print(f"repo      : {args.repo}")
    print(f"workflow  : {args.workflow}")
    print(f"window    : {since.isoformat()} .. {until.isoformat()}")

    runs, run_stats = list_runs(args.repo, args.workflow, since, until, args.max_runs)
    print(f"runs      : {len(runs)} completed "
          f"({run_stats['skipped_incomplete']} in-flight or queued, excluded)")
    if not runs:
        # Not an error to return - the caller decides. But it IS degenerate, and
        # the degeneracy check below is what turns it into a non-zero exit.
        print("WARNING: no completed runs in the window.")

    cache = args.cache_dir
    os.makedirs(cache, exist_ok=True)

    # Per-leg accumulators.
    executions = defaultdict(int)              # leg -> reports with >=1 testcase
    presence = defaultdict(lambda: defaultdict(int))  # leg -> test -> executions containing it
    flaked = defaultdict(lambda: defaultdict(int))    # leg -> test -> executions where it flaked
    attempts = defaultdict(lambda: defaultdict(int))  # leg -> test -> flaky attempt elements
    exec_with_any_flake = defaultdict(int)     # leg -> executions carrying >=1 flake
    runs_per_leg = defaultdict(set)

    artifacts_seen = 0
    artifacts_downloaded = 0
    artifacts_expired = 0
    artifacts_unreadable = 0
    xml_files_read = 0
    xml_duplicates_dropped = 0
    empty_reports = 0
    runs_without_junit = 0
    unreadable_detail: list[str] = []

    for index, run in enumerate(runs, start=1):
        run_id = run["id"]
        try:
            arts = gh_json(
                f"repos/{args.repo}/actions/runs/{run_id}/artifacts?per_page=100"
            ).get("artifacts", [])
        except RuntimeError as exc:
            # Fail loud, keep going: one 404 must not silently truncate the
            # window, but it must also not abort a two-hour census.
            artifacts_unreadable += 1
            unreadable_detail.append(f"run {run_id}: artifact list failed - {exc}")
            continue

        junit_arts = [a for a in arts if a["name"].startswith(ARTIFACT_PREFIX)]
        if not junit_arts:
            # NOT counted as retention loss. The hand pass downloaded the job
            # logs for these and found 76 of 79 died before nextest ever
            # started (build/setup failure), so they are absent for a reason
            # that has nothing to do with expiry. Reported separately.
            runs_without_junit += 1
            continue

        # sha256 -> leg, scoped to THIS RUN. See the dedup note in the module
        # docstring: dedup is per (run, leg), never global.
        seen_digests: dict[tuple[str, str], bool] = {}

        for art in junit_arts:
            artifacts_seen += 1
            raw_leg = art["name"][len(ARTIFACT_PREFIX):]
            leg = LEG_NAME_FOLDS.get(raw_leg, raw_leg)

            if art.get("expired"):
                # Trap (2) in the flesh: this artifact's executions are lost,
                # and they are lost in the direction that looks like health.
                artifacts_expired += 1
                continue

            path = os.path.join(cache, f"{run_id}-{art['id']}.zip")
            if not os.path.exists(path):
                try:
                    gh_download_artifact(args.repo, art["id"], path)
                except RuntimeError as exc:
                    artifacts_unreadable += 1
                    unreadable_detail.append(f"run {run_id} artifact {art['id']}: {exc}")
                    continue
            artifacts_downloaded += 1

            try:
                zf = zipfile.ZipFile(path)
            except zipfile.BadZipFile as exc:
                artifacts_unreadable += 1
                unreadable_detail.append(f"run {run_id} artifact {art['id']}: {exc}")
                continue

            with zf:
                for entry in sorted(zf.namelist()):
                    if not entry.endswith(".xml"):
                        continue
                    blob = zf.read(entry)
                    digest = hashlib.sha256(blob).hexdigest()
                    key = (leg, digest)
                    if key in seen_digests:
                        # THE DEDUP. junit.xml is a byte-identical copy of the
                        # final outer attempt, and the -checkpoint artifact is a
                        # byte-identical copy of the whole final upload.
                        xml_duplicates_dropped += 1
                        continue
                    seen_digests[key] = True
                    xml_files_read += 1

                    try:
                        per_test, total_cases = parse_junit(blob)
                    except ValueError as exc:
                        artifacts_unreadable += 1
                        unreadable_detail.append(
                            f"run {run_id} artifact {art['id']} {entry}: {exc}"
                        )
                        continue

                    if total_cases == 0:
                        # A report with no `<testcase>` is not an execution. It
                        # is a nextest that died before running anything, and
                        # counting it would inflate every denominator with
                        # executions in which no test COULD have flaked.
                        empty_reports += 1
                        continue

                    executions[leg] += 1
                    runs_per_leg[leg].add(run_id)
                    any_flake = False
                    for test_key, n_flaky in per_test.items():
                        presence[leg][test_key] += 1
                        if n_flaky > 0:
                            flaked[leg][test_key] += 1
                            attempts[leg][test_key] += n_flaky
                            any_flake = True
                    if any_flake:
                        exec_with_any_flake[leg] += 1

        if index % 20 == 0 or index == len(runs):
            print(f"  ... {index}/{len(runs)} runs, {artifacts_downloaded} artifacts, "
                  f"{sum(executions.values())} executions")

    # EVERY known leg is emitted, including one with zero executions. A leg that
    # simply vanished from the columns would be trap (1) at platform scale: the
    # reader sees three columns, concludes the fourth platform is fine, and
    # nothing in the table says it was never measured. `windows-latest-hosted`
    # ran 9 times in the 10-day hand window and 0 times in a 3-day one, so this
    # is the normal case and not a corner.
    legs = list(KNOWN_LEG_ORDER)
    legs += sorted(l for l in executions if l not in KNOWN_LEG_ORDER)

    # Only tests that actually flaked somewhere get a row. Emitting a row per
    # PRESENT test would be ~18,000 rows of zeroes and would bury the finding.
    flaky_tests = sorted({t for leg in flaked for t in flaked[leg]})

    rows = []
    for test in flaky_tests:
        cells = {}
        for leg in legs:
            denom = presence[leg].get(test, 0)
            cells[leg] = {
                "executions_with_test": denom,
                # Trap (1): a denominator of 0 makes the numerator meaningless,
                # and the consumer is told so in the payload rather than being
                # trusted to divide carefully.
                "vacuous": denom == 0,
                "flaked_executions": flaked[leg].get(test, 0),
                "flaky_attempts": attempts[leg].get(test, 0),
                "incidence": (flaked[leg].get(test, 0) / denom) if denom else None,
            }
        rows.append({
            "test": test,
            "total_flaked_executions": sum(flaked[leg].get(test, 0) for leg in legs),
            "total_flaky_attempts": sum(attempts[leg].get(test, 0) for leg in legs),
            "legs": cells,
        })
    rows.sort(key=lambda r: (-r["total_flaked_executions"], -r["total_flaky_attempts"],
                             r["test"]))

    total_artifacts_considered = artifacts_downloaded + artifacts_expired + artifacts_unreadable
    expired_fraction = (
        (artifacts_expired + artifacts_unreadable) / total_artifacts_considered
        if total_artifacts_considered else 0.0
    )

    return {
        "schema": "wayland-flake-census/1",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "repo": args.repo,
        "workflow": args.workflow,
        "window": {"since": since.isoformat(), "until": until.isoformat()},
        # Restated in the payload so a consumer of the JSON alone cannot mistake
        # these numbers for a `--retries 0` rate. Trap (4).
        "measurement_note": (
            "Per-execution incidence under the ci profile's retries = 2. NOT a "
            "--retries 0 failure rate; a census read from CI history can never "
            "produce one. A rate needs a bounded job running the pinned list at "
            "--retries 0 with n >= 20 per test per platform."
        ),
        "denominator_note": (
            "An EXECUTION is one nextest invocation whose JUnit report holds at "
            "least one <testcase>. Per-(test, leg) denominators are derived from "
            "PRESENCE IN THE REPORT, so a test excluded from the main step (e.g. "
            "walk_parallel_identity_test on macOS since 2026-09-07T16:20Z) "
            "correctly gets a smaller denominator than its leg total. That is "
            "the design, not a bug."
        ),
        "runs": {
            "completed_in_window": len(runs),
            "excluded_not_completed": run_stats["skipped_incomplete"],
            "without_junit_artifacts": runs_without_junit,
            "without_junit_note": (
                "Runs that completed and uploaded no nextest-junit-* artifact. "
                "These are NOT retention loss: the 2026-09-10 hand pass pulled "
                "the job logs for 79 such leg-jobs and found 76 died before "
                "nextest started."
            ),
        },
        "artifacts": {
            "seen": artifacts_seen,
            "downloaded": artifacts_downloaded,
            "expired": artifacts_expired,
            "unreadable": artifacts_unreadable,
            "expired_or_unreadable_fraction": round(expired_fraction, 6),
            "retention_note": (
                "An expired artifact is indistinguishable from a leg that did "
                "not flake. Above --max-expired-fraction this census is "
                "measuring retention, not flakiness, and the run is DEGENERATE."
            ),
            "unreadable_detail": unreadable_detail[:50],
        },
        "dedup": {
            "xml_reports_counted": xml_files_read,
            "xml_duplicates_dropped": xml_duplicates_dropped,
            "empty_reports_skipped": empty_reports,
            "note": (
                "Deduplicated by sha256 within each (run, leg). junit.xml is a "
                "byte-identical copy of the final outer attempt, and the "
                "-checkpoint artifact is a byte-identical copy of the final "
                "upload; summing them double-counts."
            ),
        },
        "legs": {
            leg: {
                "executions": executions[leg],
                "runs_contributing": len(runs_per_leg[leg]),
                "executions_with_any_flake": exec_with_any_flake[leg],
                "distinct_flaky_tests": len(flaked[leg]),
            }
            for leg in legs
        },
        "leg_order": legs,
        "tests": rows,
        # Presence-only rows for the allowlist keys that never flaked. This is
        # what lets DEAD ENTRIES distinguish "ran N times and never flaked"
        # (a real deletion candidate) from "never executed in this window"
        # (unobservable, and deleting on that evidence would be the vacuous
        # zero again).
        "watched_presence": {
            key: {leg: presence[leg].get(key, 0) for leg in legs}
            for key in sorted(watch_keys)
        },
    }


# ---------------------------------------------------------------------------
# Allowlist (format and validation deliberately mirror grade-retry-flakes.sh)
# ---------------------------------------------------------------------------


def read_allowlist(path: str, today: str) -> tuple[list[dict], list[str]]:
    """Parse `.config/flaky-allowlist.txt`. Same columns as the reactive gate.

    Format: `<YYYY-MM-DD expiry> <binary-id>::<test-name> <gh#NNNN> <reason...>`.
    Malformed lines are REPORTED and skipped rather than tolerated, for the same
    reason grade-retry-flakes.sh validates them: an allowlist that silently
    accepts a fat-fingered date is an allowlist that fails in the green
    direction.
    """
    entries: list[dict] = []
    problems: list[str] = []
    if not os.path.exists(path):
        problems.append(f"allowlist {path} does not exist")
        return entries, problems
    with open(path, "r", encoding="utf-8") as fh:
        for lineno, raw in enumerate(fh, start=1):
            line = raw.replace("\r", "").strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split(None, 3)
            if len(parts) < 4:
                problems.append(f"{path}:{lineno} has fewer than four fields")
                continue
            expiry, key, issue, reason = parts
            if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", expiry):
                problems.append(f"{path}:{lineno} expiry is not YYYY-MM-DD ({expiry!r})")
                continue
            if "::" not in key:
                problems.append(f"{path}:{lineno} key has no '::' ({key!r})")
                continue
            if not re.fullmatch(r"(gh)?#\d+", issue):
                problems.append(f"{path}:{lineno} names no owning issue ({issue!r})")
                continue
            entries.append({
                "line": lineno,
                "expiry": expiry,
                "key": key,
                "issue": issue,
                "reason": reason,
                # String comparison is correct for zero-padded ISO-8601 and
                # needs no `date` binary, exactly as the reactive gate does it.
                "expired": expiry < today,
            })
    return entries, problems


def grade(census: dict, entries: list[dict]) -> dict:
    """The two outputs nothing currently produces. Computed off the CENSUS."""
    legs = census["leg_order"]
    by_test = {row["test"]: row for row in census["tests"]}
    live_keys = {e["key"] for e in entries if not e["expired"]}
    all_keys = {e["key"] for e in entries}

    # --- COVERAGE MISSES: incidence > 0, no LIVE allowlist line. --------------
    #
    # Graded against LIVE entries, not all entries: an expired line does not
    # allow anything in grade-retry-flakes.sh either, so a test covered only by
    # an expired line is genuinely uncovered and will redden the next run that
    # trips it. Reporting it here is the entire point of running early.
    misses = []
    for row in census["tests"]:
        if row["test"] in live_keys:
            continue
        worst_leg, worst_rate = None, -1.0
        for leg in legs:
            cell = row["legs"][leg]
            if cell["incidence"] is not None and cell["incidence"] > worst_rate:
                worst_leg, worst_rate = leg, cell["incidence"]
        misses.append({
            "test": row["test"],
            "flaked_executions": row["total_flaked_executions"],
            "flaky_attempts": row["total_flaky_attempts"],
            "worst_leg": worst_leg,
            "worst_incidence": worst_rate if worst_rate >= 0 else None,
            "covered_by_expired_entry": row["test"] in all_keys,
            "legs": row["legs"],
        })
    misses.sort(key=lambda m: (-m["flaked_executions"], -m["flaky_attempts"], m["test"]))

    # --- DEAD ENTRIES: zero observations in the window. -----------------------
    #
    # Trap (1) applied to the allowlist. An entry whose test never EXECUTED in
    # the window is UNOBSERVABLE, not dead: deleting it would be deleting an
    # entry the census had no power to exercise. Only an entry whose test ran
    # and never flaked is a deletion candidate, and even that is a
    # recommendation for the expiry date, not a licence to delete today.
    dead = []
    for entry in entries:
        row = by_test.get(entry["key"])
        if row is not None and row["total_flaked_executions"] > 0:
            continue
        # Presence comes from `watched_presence` (recorded for every allowlist
        # key regardless of whether it flaked) and falls back to the flaky row
        # for a key that did flake on some other leg.
        watched = census.get("watched_presence", {}).get(entry["key"])
        executed = 0
        per_leg_presence = {}
        for leg in legs:
            if watched is not None:
                n = watched.get(leg, 0)
            elif row is not None:
                n = row["legs"][leg]["executions_with_test"]
            else:
                n = 0
            per_leg_presence[leg] = n
            executed += n
        # A leg with zero executions in the window measured nothing at all, so
        # a zero presence there is the window's fault and not the filter's.
        # Only legs that DID produce reports can testify to a test's absence.
        absent_legs = [
            leg for leg in legs
            if census["legs"][leg]["executions"] > 0 and per_leg_presence[leg] == 0
        ]
        dead.append({
            "line": entry["line"],
            "key": entry["key"],
            "expiry": entry["expiry"],
            "issue": entry["issue"],
            "expired": entry["expired"],
            "executions_observed": executed,
            "presence_per_leg": per_leg_presence,
            "legs_measured_but_test_absent": absent_legs,
            # THREE verdicts, not two, and the third is the one that matters.
            #
            #   NEVER-EXECUTED    the census had no power to exercise this test
            #                     anywhere. Its zero is absence. NOT deletable.
            #   NOT-EVERYWHERE    the test ran on some measured legs and is
            #                     ABSENT from others. Trap (3): a test excluded
            #                     from a leg's main step stops producing
            #                     evidence on the leg where it used to flake,
            #                     and the entry then looks dead for a reason
            #                     that has nothing to do with the test being
            #                     fixed. Line 68 of the allowlist is exactly
            #                     this: `redundant_walk_root_is_not_walked_twice`
            #                     moved to an isolated `--retries 0` macOS step
            #                     on 2026-09-07T16:20Z whose junit.xml the main
            #                     step overwrites, so macOS presence reads 0
            #                     while the ticket records five reproductions.
            #                     NOT deletable on this evidence.
            #   DELETE-AT-EXPIRY  the test ran on EVERY measured leg, `executed`
            #                     times in total, and never once needed a retry.
            #                     The entry is covering nothing; delete at
            #                     expiry rather than renewing.
            "verdict": (
                "NEVER-EXECUTED (unobservable; NOT a deletion recommendation)"
                if executed == 0 else
                ("NOT-EVERYWHERE (absent from " + ", ".join(absent_legs)
                 + "; check the test filter before deleting)")
                if absent_legs else
                "DELETE-AT-EXPIRY"
            ),
        })
    dead.sort(key=lambda d: d["line"])

    return {"coverage_misses": misses, "dead_entries": dead}


# ---------------------------------------------------------------------------
# Degeneracy
# ---------------------------------------------------------------------------


def degeneracy(census: dict, max_expired_fraction: float) -> list[str]:
    """Reasons this census is not a measurement. Empty list means it is one.

    Deliberately separate from the coverage/dead-entry grading: those two are a
    REPORT and must never fail a build, while these three mean the report itself
    is not evidence of anything and a green from it would be a manufactured
    platform claim.
    """
    reasons = []
    total_exec = sum(l["executions"] for l in census["legs"].values())

    if total_exec == 0:
        reasons.append(
            "ZERO executions with a JUnit report in the whole window. The census "
            "measured nothing; every zero it prints is absence, not health."
        )

    frac = census["artifacts"]["expired_or_unreadable_fraction"]
    if frac > max_expired_fraction:
        reasons.append(
            f"{census['artifacts']['expired'] + census['artifacts']['unreadable']} of "
            f"{census['artifacts']['downloaded'] + census['artifacts']['expired'] + census['artifacts']['unreadable']} "
            f"artifacts were expired or unreadable ({frac:.1%} > "
            f"{max_expired_fraction:.1%}). Past this point the census is measuring "
            "artifact retention rather than flakiness: an expired artifact and a "
            "leg that did not flake are indistinguishable in the output."
        )

    n_flaky = len(census["tests"])
    if n_flaky == 0 and total_exec >= POSITIVE_CONTROL_MIN_EXECUTIONS:
        reasons.append(
            f"POSITIVE CONTROL FAILED: {total_exec} executions were read and ZERO "
            "flakyFailure/flakyError elements were found. The measured base rate "
            "is 30 of 104 macOS executions carrying at least one, so zero across "
            f"{total_exec} executions is far more likely a broken XML path than a "
            "clean window. Investigate the parser before believing this."
        )
    return reasons


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------


def cell_text(cell: dict, leg_executions: int) -> str:
    """One table cell, with trap (1) rendered rather than commented."""
    if leg_executions == 0:
        # The whole LEG produced no JUnit in this window. Different fact from
        # "this test does not run on that leg", and reported differently so a
        # short window cannot be read as a platform being clean.
        return "LEG NOT MEASURED"
    if cell["vacuous"]:
        # NOT "0". The test never executed on this leg, so there was no
        # opportunity for it to flake and no information in the zero.
        return "VACUOUS (never executed)"
    if cell["flaked_executions"] == 0:
        return f"0/{cell['executions_with_test']}"
    return (
        f"**{cell['flaked_executions']}/{cell['executions_with_test']}** "
        f"({cell['flaky_attempts']} att, {cell['incidence']:.1%})"
    )


def render_markdown(census: dict, graded: dict | None, degen: list[str]) -> str:
    legs = census["leg_order"]
    out: list[str] = []
    w = out.append

    w("# Retry-flake census")
    w("")
    w(f"`{census['repo']}` / `{census['workflow']}`, runs created "
      f"`{census['window']['since']}` .. `{census['window']['until']}`. "
      f"Generated {census['generated_at']}.")
    w("")
    w("> **Every rate below is per-execution incidence under the `ci` profile's "
      "`retries = 2`. It is NOT a `--retries 0` failure rate, and no census read "
      "from CI history can produce one.** Turning incidence into a rate needs a "
      "bounded job that runs the pinned list at `--retries 0` with n >= 20 per "
      "test per platform.")
    w("")

    if degen:
        w("## DEGENERATE CENSUS - do not read the numbers below as a measurement")
        w("")
        for reason in degen:
            w(f"* {reason}")
        w("")

    w("## Coverage of the window")
    w("")
    w(f"* completed `{census['workflow']}` runs in window: "
      f"**{census['runs']['completed_in_window']}** "
      f"({census['runs']['excluded_not_completed']} still in flight, excluded)")
    w(f"* runs that uploaded no `nextest-junit-*` artifact: "
      f"**{census['runs']['without_junit_artifacts']}** - "
      f"{census['runs']['without_junit_note']}")
    w(f"* artifacts downloaded: **{census['artifacts']['downloaded']}**")
    w(f"* **artifacts EXPIRED or unreadable: "
      f"{census['artifacts']['expired'] + census['artifacts']['unreadable']} "
      f"({census['artifacts']['expired_or_unreadable_fraction']:.1%})** - "
      f"{census['artifacts']['retention_note']}")
    w(f"* XML reports counted: **{census['dedup']['xml_reports_counted']}**, "
      f"byte-identical duplicates dropped: "
      f"**{census['dedup']['xml_duplicates_dropped']}**, "
      f"empty reports skipped: {census['dedup']['empty_reports_skipped']}")
    w(f"  * {census['dedup']['note']}")
    w("")

    w("## Denominator per leg")
    w("")
    w(census["denominator_note"])
    w("")
    w("| leg | executions with JUnit | runs contributing | executions carrying >=1 retry-masked failure | distinct flaky tests |")
    w("|-----|----------------------:|------------------:|---------------------------------------------:|---------------------:|")
    for leg in legs:
        info = census["legs"][leg]
        w(f"| `{leg}` | {info['executions']} | {info['runs_contributing']} | "
          f"{info['executions_with_any_flake']} | {info['distinct_flaky_tests']} |")
    w("")

    w("## Population")
    w("")
    w(f"**{len(census['tests'])} distinct tests** produced at least one "
      "`<flakyFailure>` / `<flakyError>` in this window. Cells read "
      "`flaked-executions / executions-that-CONTAINED-this-test (attempts, "
      "incidence)`. A cell reading `VACUOUS (never executed)` means the test "
      "never ran on that leg, so its zero is a report of ABSENCE and not of "
      "health - do not read a platform claim out of it.")
    w("")
    header = "| test | allowlist | " + " | ".join(f"`{l}`" for l in legs) + " |"
    w(header)
    w("|------|-----------|" + "|".join(["---"] * len(legs)) + "|")
    if graded is not None:
        miss_keys = {m["test"] for m in graded["coverage_misses"]}
    else:
        miss_keys = set()
    for row in census["tests"]:
        mark = "**MISS**" if row["test"] in miss_keys else ("yes" if graded else "-")
        w(f"| `{row['test']}` | {mark} | "
          + " | ".join(cell_text(row["legs"][l], census["legs"][l]["executions"])
                        for l in legs) + " |")
    w("")

    if graded is None:
        return "\n".join(out) + "\n"

    w("## COVERAGE MISSES - flaking now, with no live allowlist line")
    w("")
    w("These are the tests that will redden `report` the next time they fire, "
      "reported here from a POPULATION measurement rather than from the incident. "
      "This list is the whole point of running on a schedule: it exists before "
      "the run that would have discovered each member one at a time.")
    w("")
    w("**Do not bulk-write these into `.config/flaky-allowlist.txt`.** An entry "
      "means 'this is known to need retries, someone owns it, and the debt has a "
      "date'. N entries added in one commit have no owner and convert N unknown "
      "flakes into N silent retries, which is the harm wayland#1169 built the "
      "gate to stop.")
    w("")
    if not graded["coverage_misses"]:
        w("_None. Every test that flaked in this window has a live allowlist line._")
    else:
        w("| test | flaked executions | attempts | worst leg | incidence there | note |")
        w("|------|------------------:|---------:|-----------|----------------:|------|")
        for m in graded["coverage_misses"]:
            note = "covered only by an EXPIRED entry" if m["covered_by_expired_entry"] else ""
            rate = f"{m['worst_incidence']:.1%}" if m["worst_incidence"] is not None else "n/a"
            w(f"| `{m['test']}` | {m['flaked_executions']} | {m['flaky_attempts']} | "
              f"`{m['worst_leg']}` | {rate} | {note} |")
    w("")

    w("## DEAD ENTRIES - allowlist lines with zero observations in the window")
    w("")
    w("Only `DELETE-AT-EXPIRY` is a deletion recommendation. It means the test "
      "executed on EVERY measured leg, the stated number of times, and never "
      "once needed a retry - the entry covers nothing.")
    w("")
    w("The other two verdicts are the vacuous zero one level up, and acting on "
      "either would delete an entry the census had no power to exercise:")
    w("")
    w("* `NEVER-EXECUTED` - the test did not run anywhere in this window. Widen "
      "the window.")
    w("* `NOT-EVERYWHERE` - the test ran on some legs and is ABSENT from at "
      "least one leg that DID produce reports. A test excluded from a leg's main "
      "step stops producing evidence on the very leg where it used to flake, so "
      "the entry looks dead for a reason that has nothing to do with the test "
      "being fixed. Check the test filter on the named leg first.")
    w("")
    if not graded["dead_entries"]:
        w("_None. Every allowlist entry fired at least once in this window._")
    else:
        w("| allowlist line | test | expiry | issue | executions observed, per leg | verdict |")
        w("|---------------:|------|--------|-------|------------------------------|---------|")
        for d in graded["dead_entries"]:
            exp = f"{d['expiry']}{' (EXPIRED)' if d['expired'] else ''}"
            per_leg = ", ".join(
                f"{leg} {d['presence_per_leg'].get(leg, 0)}"
                for leg in legs if census["legs"][leg]["executions"] > 0
            )
            w(f"| {d['line']} | `{d['key']}` | {exp} | {d['issue']} | "
              f"{d['executions_observed']} ({per_leg}) | {d['verdict']} |")
    w("")
    return "\n".join(out) + "\n"


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    today = dt.datetime.now(dt.timezone.utc)

    ap = argparse.ArgumentParser(
        description="Retry-flake population census and allowlist coverage gate "
                    "(FerroxLabs/wayland#1286).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("--repo", default="FerroxLabs/wayland-core")
    ap.add_argument("--workflow", default="ci.yml")
    ap.add_argument("--since", help="ISO instant or YYYY-MM-DD (UTC)")
    ap.add_argument("--until", help="ISO instant or YYYY-MM-DD (UTC)")
    ap.add_argument("--days", type=int,
                    help="window of the last N days, ending now. Overridden by "
                         "--since/--until if both are given.")
    ap.add_argument("--max-runs", type=int, default=0,
                    help="cap the number of runs enumerated (0 = no cap). For "
                         "bounded proof runs; a capped census is NOT the weekly "
                         "measurement and the JSON records the cap.")
    ap.add_argument("--cache-dir",
                    default=os.environ.get("FLAKE_CENSUS_CACHE", ".flake-census-cache"),
                    help="where artifact zips are cached between runs")
    ap.add_argument("--keep-cache", action="store_true",
                    help="do not delete the artifact cache on success")
    ap.add_argument("--out-json", default="flake-census.json")
    ap.add_argument("--out-md", default="flake-census.md")
    ap.add_argument("--from-census",
                    help="skip the API entirely and grade an existing census JSON")
    ap.add_argument("--gate", action="store_true",
                    help="also emit COVERAGE MISSES and DEAD ENTRIES against the "
                         "allowlist")
    ap.add_argument("--allowlist", default=".config/flaky-allowlist.txt")
    ap.add_argument("--today", default=today.strftime("%Y-%m-%d"),
                    help="YYYY-MM-DD used for allowlist expiry comparison. "
                         "Injectable so a test can exercise both sides of an "
                         "expiry boundary without depending on the calendar.")
    ap.add_argument("--max-expired-fraction", type=float,
                    default=DEFAULT_MAX_EXPIRED_FRACTION)
    ap.add_argument("--fail-on-degenerate", action="store_true",
                    help=f"exit {EXIT_DEGENERATE} if the census is degenerate. "
                         "Coverage misses NEVER fail: they are a report, not a "
                         "merge gate.")
    args = ap.parse_args(argv)

    # Read the allowlist first: its keys are what build_census must record
    # presence for, so that a never-fired entry can be graded honestly.
    entries: list[dict] = []
    allowlist_problems: list[str] = []
    if args.gate:
        entries, allowlist_problems = read_allowlist(args.allowlist, args.today)

    if args.from_census:
        with open(args.from_census, "r", encoding="utf-8") as fh:
            census = json.load(fh)
    else:
        if args.since and args.until:
            pass
        elif args.days:
            args.until = today.isoformat()
            args.since = (today - dt.timedelta(days=args.days)).isoformat()
        elif args.since:
            args.until = today.isoformat()
        else:
            ap.error("give --since/--until or --days")
        census = build_census(args, {e["key"] for e in entries})
        census["max_runs_cap"] = args.max_runs or None

    graded = None
    if args.gate:
        graded = grade(census, entries)
        census["gate"] = graded
        census["allowlist"] = {
            "path": args.allowlist,
            "entries": len(entries),
            "live_entries": sum(1 for e in entries if not e["expired"]),
            "expired_entries": sum(1 for e in entries if e["expired"]),
            "problems": allowlist_problems,
        }

    degen = degeneracy(census, args.max_expired_fraction)
    census["degenerate"] = degen

    with open(args.out_json, "w", encoding="utf-8") as fh:
        json.dump(census, fh, indent=2, sort_keys=False)
        fh.write("\n")
    markdown = render_markdown(census, graded, degen)
    with open(args.out_md, "w", encoding="utf-8") as fh:
        fh.write(markdown)

    print("")
    print(markdown)
    for problem in allowlist_problems:
        print(f"::warning title=Malformed flake allowlist entry::{problem}")
    print(f"census JSON : {args.out_json}")
    print(f"census MD   : {args.out_md}")

    if not args.keep_cache and not args.from_census and os.path.isdir(args.cache_dir):
        shutil.rmtree(args.cache_dir, ignore_errors=True)

    if degen:
        print("")
        for reason in degen:
            print(f"::error title=Degenerate flake census::{reason}")
        if args.fail_on_degenerate:
            return EXIT_DEGENERATE
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
