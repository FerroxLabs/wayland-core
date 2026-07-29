#!/usr/bin/env python3
"""Grade a cargo test run whose log may contain a NESTED cargo's output.

## The defect this repairs, measured in this lane on 2026-07-29

`cargo test -p wcore-cli --lib` at `884bca8c` produced a log containing BOTH:

    test result: FAILED. 0 passed; 1 failed; 0 ignored; ...
    error: test failed, to rerun pass `--lib`
    test result: ok. 1844 passed; 0 failed; 1 ignored; ...

The first two lines are NOT this crate's suite. `wcore-cli`'s own test
`plugin::scaffold::tests::plugin_test_propagates_a_failing_suite`
(`crates/wcore-cli/src/plugin/scaffold.rs:262`) deliberately scaffolds a crate
whose single test panics, runs a NESTED `cargo test` over it, and asserts the
failure surfaces as non-zero. That nested cargo inherits stdout/stderr, so its
output — headers, per-test lines, `test result:`, and a bare `error:` — splices
into the parent's log at whatever point the parent's scheduler ran that test.

My first instrument was `grep -E "^test result:|^error"`. It reports whichever
line it meets with no idea which cargo emitted it: first-match gives a **false
RED** on a green suite, last-match gives a **false GREEN** the moment a real
failure precedes a nested pass. LANE-BRIEF §3.2 self-passing-gate class, and
§6b-ii says repair the instrument in the lane that found it.

## The repair, and why it is a REFUSAL rather than a cleverer parser

My first attempt attributed each `test result:` to the nearest preceding
`Running <target>` header. Its own self-test failed, and the failure was correct:
the nested cargo's header lands in the MIDDLE of the parent's run, so the
parent's own `test result:` is preceded by the CHILD's header. Proximity cannot
separate two interleaved streams. Any parser that claims to is guessing.

So this instrument does not guess. It requires the outer command's exit status
from a file the runner wrote immediately after the command — never read across a
pipe, because `${PIPESTATUS[0]}` returns empty in this environment — and treats
the log strictly as corroboration:

  * the rc file is the VERDICT;
  * the log must show at least one `test result:` block that actually EXECUTED
    tests, because a suite can exit 0 having run zero (`0 of 12`, and
    `test result: ok` with `0 passed` is the shape that proves nothing);
  * every FAILED block is PRINTED, attributed as best the log allows, so a human
    sees which and can recognise a known-nested fixture rather than having the
    tool silently decide for them.

Refusing to grade without an rc file is the point. An instrument that answers an
unanswerable question is worse than one that says it cannot.

Exit: 0 pass, 3 fail, 4 self-test failed, 5 zero tests executed / no result,
      6 cannot grade (no rc file).
"""

import re
import sys

RUNNING = re.compile(r"^\s*Running\s+(?:unittests\s+)?(\S+)\s+\((.+?)\)\s*$")
RESULT = re.compile(
    r"^test result:\s+(ok|FAILED)\.\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored"
)


def parse(lines):
    """Every result block, with the nearest preceding target header as a HINT.

    The hint is explicitly not authoritative — see the module docstring. It is
    printed to help a human, never used to decide.
    """
    hint = None
    out = []
    for line in lines:
        m = RUNNING.match(line)
        if m:
            hint = m.group(2).strip()
            continue
        m = RESULT.match(line)
        if m:
            out.append(
                {
                    "hint": hint or "<no preceding header>",
                    "status": m.group(1),
                    "passed": int(m.group(2)),
                    "failed": int(m.group(3)),
                    "ignored": int(m.group(4)),
                }
            )
    return out


def _old_broken_matcher(lines):
    """The `grep -E '^test result:'` shape, first match. Never used to measure."""
    for line in lines:
        if line.startswith("test result:"):
            return "FAILED" not in line
    return None


def verdict(blocks, rc):
    """(exit_code, message). `rc is None` means the caller supplied no rc file."""
    if rc is None:
        return 6, ("CANNOT GRADE: no exit-status file. This log contains interleaved "
                   "output from more than one cargo; the verdict is not derivable "
                   "from it. Re-run capturing `$?` to a file.")
    executed = sum(b["passed"] + b["failed"] for b in blocks)
    if not blocks:
        return 5, "NO RESULT: the log contains no `test result:` line at all."
    if executed == 0:
        return 5, ("ZERO TESTS EXECUTED: every result block reports 0 passed and 0 "
                   "failed. Exit status and the word `ok` are both satisfied while "
                   "nothing was measured.")
    if rc != 0:
        return 3, f"FAILED: the outer command exited {rc}."
    return 0, f"PASS: outer command exited 0; {executed} tests executed across {len(blocks)} block(s)."


def self_test():
    """Three assertions. Only the third proves the repair changed anything."""
    # Shaped like the REAL log: the nested cargo's header and FAILED land in the
    # MIDDLE of the parent's run, so the parent's own `ok` is preceded by the
    # child's header. This is the case that broke the proximity parser.
    log = [
        "     Running unittests src/lib.rs (target/debug/deps/wcore_cli-7116a58a)",
        "test tui::protocol_bridge::tests::a_goal_snapshot_becomes_visible_state ... ok",
        "     Running unittests src/lib.rs (target/debug/deps/failing_fixture-451498ab)",
        "test always_fails ... FAILED",
        "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
        "error: test failed, to rerun pass `--lib`",
        "test result: ok. 1844 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out",
    ]
    results = []
    blocks = parse(log)

    # 1. KNOWN-POSITIVE: with the outer rc of 0 this GREEN run grades PASS, and
    #    the executed count is real (1845 across both blocks), not zero.
    code, msg = verdict(blocks, 0)
    a1 = code == 0 and "1845 tests executed" in msg
    results.append(("1 known-positive: outer rc=0 grades PASS with a real executed count", a1))

    # 2. KNOWN-NEGATIVE: a genuinely failing outer run must fail, and a run that
    #    executed nothing must NOT pass just because it exited 0.
    fails = verdict(blocks, 101)[0] == 3
    zero = verdict([{"hint": "x", "status": "ok", "passed": 0, "failed": 0, "ignored": 12}], 0)[0] == 5
    refuses = verdict(blocks, None)[0] == 6
    a2 = fails and zero and refuses
    results.append(
        ("2 known-negative: rc!=0 fails, zero-executed fails, and a missing rc file is refused", a2))

    # 3. THE OLD SHAPE WOULD HAVE MISSED IT. First-match grep meets the NESTED
    #    fixture's FAILED and calls this green run red. Two opposite verdicts on
    #    byte-identical input is the proof the repair does something.
    old = _old_broken_matcher(log)
    a3 = old is False and code == 0
    results.append(
        ("3 old-shape-misses: first-match grep calls this GREEN log RED; this instrument does not", a3))

    for label, ok in results:
        print(f"  [{'PASS' if ok else 'FAIL'}] assertion {label}")
    return all(ok for _, ok in results)


def main():
    if "--self-test" in sys.argv:
        print("instrument self-test:")
        ok = self_test()
        print("SELF-TEST:", "PASS" if ok else "FAIL")
        return 0 if ok else 4

    path = sys.argv[1]
    rc = None
    if "--rc-file" in sys.argv:
        rc_path = sys.argv[sys.argv.index("--rc-file") + 1]
        try:
            rc = int(open(rc_path, encoding="utf-8").read().strip())
        except (OSError, ValueError) as error:
            print(f"exit-status file unreadable ({error}) — treating as ungradeable")

    lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
    blocks = parse(lines)

    print(f"log: {path}  ({len(lines)} lines)  outer rc: {rc}")
    for block in blocks:
        mark = "FAIL" if block["status"] == "FAILED" else "ok  "
        print(f"  [{mark}] {block['passed']:>5} passed / {block['failed']} failed / "
              f"{block['ignored']} ignored   nearest-header-hint: {block['hint']}")
    failed = [b for b in blocks if b["status"] == "FAILED"]
    if failed:
        print(f"  NOTE: {len(failed)} FAILED block(s) present. If the outer rc is 0 these came "
              f"from a nested cargo the suite deliberately ran; confirm each against the source "
              f"before dismissing it.")

    code, message = verdict(blocks, rc)
    print(f"VERDICT: {message}")
    return code


if __name__ == "__main__":
    sys.exit(main())
