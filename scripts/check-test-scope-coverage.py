#!/usr/bin/env python3
"""Refuse a green lane verdict while a crate the lane chose not to run is unrun.

WHY THIS EXISTS -- FerroxLabs/wayland#1256 c3, the GENERAL case.

    c1/c2 closed ONE instance totally: the Desktop contract corpus. A lane
    added 773 lines to `crates/wcore-cli/src/main.rs`, gated with
    `cargo nextest run -p wcore-mcp -p wcore-cli`, ran the pre-flight, got 0
    from both, and reported the tree green. `cargo nextest run -p
    wcore-protocol` was EXIT=100 on that same tree: two corpus tests red on a
    source-hash rebase. Nothing the lane ran could have caught it.

    The corpus fix asks a question about the TREE and is total, so that
    observable no longer depends on the lane's crate list at all. It does not
    close the CLASS. Any test that reads a file by path at test time, in a
    crate the lane did not name, has the same shape -- and so does the plainer
    case the class is mostly made of: the lane simply broke a downstream crate
    it did not think to run.

WHAT THIS GATE ASKS, AND WHY IT IS THAT QUESTION

    Not "did the lane also run -p wcore-protocol?" and not "did the diff touch
    a file some other crate reads?". Both are proxies: they need a correct diff
    base, a matching path spelling, and someone to have remembered. The
    question here is arithmetic and total:

        WHICH WORKSPACE MEMBERS DID THIS LANE'S TEST RUNS NOT COVER?

    The answer is a set. Empty means the lane ran everything and may claim the
    tree. Non-empty means the lane MAY STILL PROCEED -- scoping a run is
    legitimate and often correct -- but it does so with every unrun crate
    printed by name, and the pre-flight banner degrades from PASSED to
    INCOMPLETE. That is the whole of the criterion: a lane cannot report a tree
    GREEN while a crate it chose not to run sits unrun and unmentioned. It can
    report exactly what it did.

    So this is deliberately NOT a hard failure on a scoped run. A gate that
    refused every scoped run would be switched off within a day, and lanes
    scope for good reasons. The lever is disclosure, and it is the reserved
    DEGRADED exit code the #1254 status protocol already defines -- which
    `scripts/preflight.sh` renders distinctly, carries this text through
    verbatim, and refuses to collapse into `ok`.

WHERE THE EVIDENCE COMES FROM

    Receipts written by `tools/remote-proof.py`, which is how every lane in
    this cycle runs cargo. Each receipt is a JSON object carrying the commit it
    built (`source`), the exact `cargo_args`, whether the run completed, and
    the remote exit code. This gate reads the receipts for the CURRENT HEAD and
    derives coverage from what actually ran -- not from a lane's own summary of
    what it ran, which is the thing under audit.

    `--invocation` states a run on the command line instead. It exists for the
    self-test and for a lane whose runner writes no receipt; it is a WEAKER
    instrument and the output says so, because nothing verifies the run
    happened.

EXIT CODES

    0  every workspace member was covered by a completed, passing test run at
       this commit.
    3  DEGRADED (the reserved code `preflight.sh` renders as INCOMPLETE):
       either no test evidence exists for this commit at all, or the runs that
       exist leave members uncovered. Every uncovered member is named.
    1  FAIL: a run names a package that is not a workspace member, or the
       workspace itself cannot be read. A declaration that can quietly stop
       matching is a declaration that excuses everything, so it is refused
       rather than ignored.

WHAT THIS GATE DOES NOT DO, stated rather than discovered later

  * It grades CRATE coverage, not TARGET coverage. `cargo test -p wcore-agent
    --test one_thing` counts wcore-agent as covered even though its lib tests
    and its other integration targets did not run. Closing that needs the
    per-crate target list, which needs cargo metadata, which this gate is
    deliberately without so it can run on the host beside the other pre-flight
    gates. The limit is real and a lane relying on this gate for target-level
    coverage is relying on something it does not provide.
  * It cannot tell a run that passed from a run that was never attempted at a
    commit nobody built. Both are "no evidence", both are DEGRADED, and that is
    the correct conflation: neither is a pass.
  * It says nothing about macOS or Windows. Every receipt it reads is Linux.

USAGE

    python3 scripts/check-test-scope-coverage.py --self-test
    python3 scripts/check-test-scope-coverage.py
    python3 scripts/check-test-scope-coverage.py --invocation "cargo nextest run -p wcore-mcp"
"""

import io
import json
import os
import pathlib
import re
import shlex
import subprocess
import sys

DEGRADED_RC = 3

# A run with any of these covers every member, whatever else is on the line.
WHOLE_WORKSPACE_FLAGS = {"--workspace", "--all"}


def workspace_members(root: pathlib.Path) -> dict:
    """-> {crate name: member path}. Read from Cargo.toml, no cargo needed."""
    manifest = root / "Cargo.toml"
    text = manifest.read_text()
    block = re.search(r"^members\s*=\s*\[(.*?)^\]", text, re.S | re.M)
    if not block:
        raise SystemExit("FAIL: %s has no [workspace] members list" % manifest)
    names = {}
    for rel in re.findall(r'"([^"]+)"', block.group(1)):
        if "*" in rel:
            raise SystemExit(
                "FAIL: workspace member glob %r is not supported by this gate. "
                "Globs make the member set depend on what is on disk, and this "
                "gate must name a fixed set or it cannot report what is "
                "missing from it." % rel
            )
        member = root / rel / "Cargo.toml"
        if not member.exists():
            raise SystemExit("FAIL: workspace member %s has no Cargo.toml" % rel)
        name = re.search(r'^name\s*=\s*"([^"]+)"', member.read_text(), re.M)
        if not name:
            raise SystemExit("FAIL: %s declares no package name" % member)
        names[name.group(1)] = rel
    return names


def is_test_run(args: list) -> bool:
    """Does this cargo invocation RUN TESTS? `check` and `clippy` do not."""
    if not args:
        return False
    if args[0] == "test":
        return True
    return args[0] == "nextest" and len(args) > 1 and args[1] == "run"


def packages_of(args: list):
    """-> (set of -p names, covers_whole_workspace).

    An invocation with neither `-p` nor `--workspace` is cargo's default:
    every member of the workspace. That is the case a lane WANTS, so it must
    not be mistaken for "named nothing, covered nothing".
    """
    named = set()
    whole = False
    i = 0
    while i < len(args):
        a = args[i]
        if a in WHOLE_WORKSPACE_FLAGS:
            whole = True
        elif a in ("-p", "--package"):
            if i + 1 < len(args):
                named.add(args[i + 1])
                i += 1
        elif a.startswith("--package="):
            named.add(a.split("=", 1)[1])
        elif a.startswith("-p") and len(a) > 2 and not a.startswith("--"):
            named.add(a[2:])
        i += 1
    if not named:
        whole = True
    return named, whole


def receipts_for(head: str, evidence_dirs: list):
    """-> (passing runs, failing runs), each [(receipt name, cargo args)].

    A FAILING run buys no coverage. "I ran it and it was red" is not knowledge
    that the crate is green, and folding the two together is the exact
    conflation this gate exists to refuse. It is reported separately rather
    than dropped, because a red run at the commit under audit is something the
    reader of a verdict has to be told about.
    """
    ok, bad = [], []
    for d in evidence_dirs:
        if not d.is_dir():
            continue
        for path in sorted(d.glob("proof-*.json")):
            try:
                r = json.loads(path.read_text())
            except (ValueError, OSError):
                continue
            if r.get("source") != head or not r.get("complete"):
                continue
            if not is_test_run(r.get("cargo_args") or []):
                continue
            (ok if r.get("remote_exit") == 0 else bad).append((path.name, r["cargo_args"]))
    return ok, bad


def evaluate(members: dict, runs: list):
    """-> (uncovered sorted, unknown sorted). `runs` is [(label, args)]."""
    covered = set()
    unknown = set()
    for _label, args in runs:
        named, whole = packages_of(args)
        if whole:
            covered |= set(members)
        for n in named:
            if n in members:
                covered.add(n)
            else:
                unknown.add(n)
    return sorted(set(members) - covered), sorted(unknown)


def report(members, runs, weak_source, failing=()):
    uncovered, unknown = evaluate(members, runs)
    if unknown:
        for n in unknown:
            print(
                "FAIL: a test run names package %r, which is not a workspace "
                "member. Either the run is against a different tree or this "
                "gate's member list has rotted; both make its coverage "
                "arithmetic wrong, so it refuses rather than reporting a "
                "number it cannot stand behind." % n
            )
        return 1
    red = ""
    if failing:
        print(
            "RED RUNS at this commit -- these bought no coverage, because a run "
            "that failed is not evidence that its crates are green:"
        )
        for label, args in failing:
            print("      FAILED: %s :: cargo %s" % (label, shlex.join(args)))
        print(
            "      (Deliberate red arms land here too. This gate does not judge "
            "why a run was red; it refuses to count one as knowledge.)"
        )
        red = " and %d recorded run(s) at this commit FAILED" % len(failing)
    if not runs:
        print(
            "DEGRADED: no completed, passing test run is recorded for this "
            "commit, so NOTHING is known about whether any crate in this "
            "workspace is green%s. All %d member(s) are unrun as far as any "
            "evidence goes." % (red, len(members))
        )
        print(
            "This is not a failure and not a pass. Run the tests, or say in "
            "your verdict that you did not."
        )
        return DEGRADED_RC
    for label, args in runs:
        print("  test run: %s :: cargo %s" % (label, shlex.join(args)))
    if weak_source:
        print(
            "NOTE: the run(s) above were STATED on this gate's command line, "
            "not read from a runner receipt. Nothing here verifies they "
            "happened."
        )
    if not uncovered:
        if failing:
            print(
                "DEGRADED: every member was covered by a passing run, but %d "
                "recorded run(s) at this commit FAILED (above). Nothing here "
                "decides whether those were deliberate; the verdict has to say."
                % len(failing)
            )
            return DEGRADED_RC
        print(
            "OK: every one of the %d workspace member(s) was covered by a "
            "completed, passing test run at this commit." % len(members)
        )
        return 0
    print(
        "DEGRADED: %d of %d workspace member(s) were NOT run by anything at "
        "this commit. A lane may scope its test run -- that is often right -- "
        "but it cannot then report the TREE green, because a crate nobody ran "
        "is a crate nobody knows about. Either run these or say, in the "
        "verdict itself, that they were not run:" % (len(uncovered), len(members))
    )
    for name in uncovered:
        print("      unrun: %s  (%s)" % (name, members[name]))
    return DEGRADED_RC


# ── self-test ────────────────────────────────────────────────────────────────

def _members():
    return {"wcore-types": "crates/wcore-types",
            "wcore-protocol": "crates/wcore-protocol",
            "wcore-mcp": "crates/wcore-mcp",
            "wcore-cli": "crates/wcore-cli"}


def self_test() -> int:
    ok = True
    cases = [
        # label, runs, want_uncovered, want_unknown
        ("no runs at all -> everything unrun",
         [], ["wcore-cli", "wcore-mcp", "wcore-protocol", "wcore-types"], []),
        # THE DEFECT, verbatim: lane/f13-w2-mcp-transports' own invocation.
        ("the #1256 lane's own scoped run leaves wcore-protocol unrun",
         [("x", ["nextest", "run", "-p", "wcore-mcp", "-p", "wcore-cli"])],
         ["wcore-protocol", "wcore-types"], []),
        # CONTROL 1: the same shape with nothing scoped is cargo's default and
        # must be TOTAL, or this gate degrades every honest full run and gets
        # switched off.
        ("an unscoped run covers the whole workspace",
         [("x", ["nextest", "run"])], [], []),
        ("--workspace covers the whole workspace even beside a -p",
         [("x", ["test", "--workspace", "-p", "wcore-mcp"])], [], []),
        # CONTROL 2: coverage accumulates across runs, so a lane that scopes
        # into several invocations is not punished for the shape of its
        # command line.
        ("coverage is the union over several scoped runs",
         [("a", ["test", "-p", "wcore-mcp", "-p", "wcore-cli"]),
          ("b", ["nextest", "run", "-p", "wcore-protocol"]),
          ("c", ["test", "--package=wcore-types"])], [], []),
        ("the -pNAME spelling counts",
         [("a", ["test", "-pwcore-mcp", "-p", "wcore-cli"]),
          ("b", ["test", "-p", "wcore-protocol", "-p", "wcore-types"])], [], []),
        # A package nobody in this workspace declares cannot be silently
        # counted as coverage.
        ("a package that is not a member is refused, not counted",
         [("x", ["nextest", "run", "-p", "not-a-member"])],
         ["wcore-cli", "wcore-mcp", "wcore-protocol", "wcore-types"],
         ["not-a-member"]),
    ]
    for label, runs, want_unc, want_unk in cases:
        unc, unk = evaluate(_members(), runs)
        good = (unc == want_unc and unk == want_unk)
        ok &= good
        print("  %-62s %s" % (label[:62], "ok" if good else
                              "SELF-TEST FAILED got %r %r" % (unc, unk)))

    # The verb filter, both directions. A `check` or `clippy` receipt is not a
    # test run, and counting one would let a lane claim coverage from a build.
    verb_cases = [(["test", "-p", "x"], True), (["nextest", "run"], True),
                  (["check", "--workspace", "--all-targets"], False),
                  (["clippy", "--workspace"], False), (["nextest", "list"], False),
                  ([], False)]
    for args, want in verb_cases:
        got = is_test_run(args)
        ok &= (got == want)
        print("  %-62s %s" % ("verb: cargo %s -> %s" % (" ".join(args) or "<empty>",
                                                        "test run" if want else "not a test run"),
                              "ok" if got == want else "SELF-TEST FAILED"))

    # And the exit codes the whole protocol turns on, through `report` itself
    # rather than a reimplementation of the rule.
    rc_cases = [("no evidence -> DEGRADED", [], DEGRADED_RC),
                ("scoped run -> DEGRADED",
                 [("x", ["nextest", "run", "-p", "wcore-mcp"])], DEGRADED_RC),
                ("full run -> 0", [("x", ["nextest", "run"])], 0),
                ("non-member -> FAIL",
                 [("x", ["test", "-p", "nope"])], 1)]
    for label, runs, want in rc_cases:
        buf = io.StringIO()
        real, sys.stdout = sys.stdout, buf
        try:
            got = report(_members(), runs, weak_source=False)
        finally:
            sys.stdout = real
        ok &= (got == want)
        print("  %-62s want rc %-2s got rc %-2s  %s"
              % (label[:62], want, got, "ok" if got == want else "SELF-TEST FAILED"))

    # A red run must not be counted as coverage, and must not be silent. Both
    # arms, because a gate that ignored red runs and a gate that failed on them
    # are each wrong in a different direction -- deliberate red arms are how
    # this repo grades its own guards, so a FAIL here would punish the practice
    # the gate is written to support.
    red_cases = [
        ("a full run that FAILED buys no coverage -> DEGRADED",
         [], [("x", ["nextest", "run"])], DEGRADED_RC),
        ("full coverage BESIDE a red run is still not a pass -> DEGRADED",
         [("a", ["nextest", "run"])], [("b", ["test", "-p", "wcore-mcp"])], DEGRADED_RC),
        ("full coverage and no red run -> 0",
         [("a", ["nextest", "run"])], [], 0),
    ]
    for label, runs, failing, want in red_cases:
        buf = io.StringIO()
        real, sys.stdout = sys.stdout, buf
        try:
            got = report(_members(), runs, weak_source=False, failing=failing)
        finally:
            sys.stdout = real
        named = "FAILED:" in buf.getvalue()
        good = (got == want) and (bool(failing) == named)
        ok &= good
        print("  %-62s want rc %-2s got rc %-2s  %s"
              % (label[:62], want, got, "ok" if good else "SELF-TEST FAILED"))

    print("self-test: %s" % ("both directions proven" if ok else "BROKEN"))
    return 0 if ok else 1



def main() -> int:
    argv = sys.argv[1:]
    if "--self-test" in argv:
        return self_test()

    root = pathlib.Path(
        subprocess.run(["git", "rev-parse", "--show-toplevel"],
                       capture_output=True, text=True, check=True).stdout.strip())
    members = workspace_members(root)

    stated = []
    i = 0
    while i < len(argv):
        if argv[i] == "--invocation" and i + 1 < len(argv):
            words = shlex.split(argv[i + 1])
            if words and words[0] == "cargo":
                words = words[1:]
            stated.append(("--invocation", words))
            i += 1
        i += 1

    if stated:
        runs = [r for r in stated if is_test_run(r[1])]
        return report(members, runs, weak_source=True)

    head = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True,
                          text=True, check=True).stdout.strip()
    dirs = []
    env = os.environ.get("WCORE_PROOF_EVIDENCE_DIR")
    if env:
        dirs.append(pathlib.Path(env))
    # tools/remote-proof.py writes into <stabilization root>/evidence, which is
    # the parent of the `worktrees/` directory this lane lives in.
    dirs.append(root.parent.parent / "evidence")
    dirs.append(root / "evidence")
    print("commit under audit: %s" % head)
    print("receipt dir(s): %s" % ", ".join(str(d) for d in dirs))
    passing, failing = receipts_for(head, dirs)
    return report(members, passing, weak_source=False, failing=failing)


if __name__ == "__main__":
    sys.exit(main())
