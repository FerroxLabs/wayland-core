# DECISIONS — wayland-core backlog sweep
# Taken 2026-08-29. Every one of these was previously parked in MASTER-PLAN.md §8 as
# "outstanding maintainer decision". Parked decisions decay into partials, so they are
# taken here. Each records the choice, the reason, and what it obliges a lane to do.

| ID | Decision | TAKEN | Obliges |
|---|---|---|---|
| SECRET | `LEDGER_ISSUES_TOKEN` | **CREATED 2026-08-29** (maintainer) | Nothing further — the follow-on recommendation was REFUTED, see D-SECRET-2 |
| Q1 / core#335 | @-refs escaping the workspace root | **A — leave, pin with a test** | Test must be phrased over ESCAPING paths, not absolute ones |
| Q5 / core#238 | bare-`NUL` guard | **BUILD THE NARROW GUARD** | Bare `NUL` only. Record the Win11-26200 measurement on the issue |
| Q2 / core#340 D | refuse indirect runners (`sh -c "npx …"`)? | **NO — and say so in the doc** | Also fix the reachability understatement |
| Q3 / core#340 B | how the fail-open notice reaches a user | **A typed protocol frame, not a log level** | Ship doc-honesty now; frame lands with Q4 |
| Q4 / core#314 D-2 | typed protocol refusals | **YES — contract minor bump, with Desktop** | Open on FerroxLabs/wayland#1099 |
| Q6 / core#253 | the umbrella | **Keep open + unscheduled; split the Telegram defect out now** | Do NOT ship slice 2's breaking migration |
| Q7 | Windows merge freeze for Lane W | **YES — a declared window, opened after Lane 0.1 is read** | Serializes the single Windows box |
| Q-113 | core#113 | **CLOSE AS REFUTED**, recording deny-by-default as the decision | Maintainer performs the close |
| Q-366d6 | core#366 d6: does an unscoped sweep RECLAIM? | **NO — report only, and print the command** | The operator surface names each leftover and the `docker rm -f` to run; nothing is destroyed automatically |
| Q-338c4 | core#338 credential surface | **Deny `/dev/tty` via `setsid`, in the SAME change as layer 1** | Layer 1 alone makes the test green while `credential.helper` stays open |

## D-SECRET-2 — REFUTED 2026-08-29. Do not build this.
MASTER-PLAN.md §8 recommended splitting `reached` per tracker "to close the one hole in the
fail-closed claim". **There is no hole. The guard already exists and is stronger than the proposal.**

`check-criteria-ledger.py` `load_trackers()` loops per repo and, for each one:

    every = gh_issues(repo, None, "all")
    if not every:
        raise TrackerError("%s: the tracker query reached ZERO issues in any state. ..." % repo)

That fires PER TRACKER, on the all-states query, before any summing. And it is not swallowed —
`main()` catches `TrackerError` and does `return 2` with "The gate refuses to degrade to a structural
check silently." A token that reaches `wayland` but returns an empty-but-successful list for
`wayland-core` therefore FAILS THE RELEASE, naming the repo.

The summed `reached == 0` at the later line is a redundant second backstop, not the primary guard.
The §8 recommendation was written from reading `release.yml` and the summed check alone, without
reading `load_trackers`. Building the "fix" would have added a duplicate guard and, worse, would have
been recorded as closing a hole that was never open.

## Why Q3 is not a log-level change (the trap it avoids)
`main.rs:1372-1380` — the TUI branch is file-only: NOTHING may reach stdio, not even an error.
`main.rs:1104-1108` — the json-stream consumer does not read stderr.
`main.rs:1381-1390` — only headless/REPL tees to stderr at ERROR.
The TUI is the PRIMARY MCP-launch surface. So `warn!`->`error!` fixes 1 of 3 shipped modes while an
`assert level == ERROR` test goes green for all three — certifying a NEW false claim.
Compounding it: the pattern being copied (`wayland-ijfw/src/mcp.rs:434-467`) wraps a SYNC fn and
asserts `levels.len() == 1`; the target `refuse_if_malware` (`malware_gate.rs:155`) is ASYNC under a
real HTTP backend, and `with_default` is THREAD-LOCAL, so under `flavor = "multi_thread"` it captures
nothing. A test built that way is vacuous.
See the standing note: a `warn!` never reaches the user when RUST_LOG is unset.

## Why Q-366d6 is "report only"
core#365's submit-path reclaim is safe for one specific reason: it holds the exact task id it is
about to use, and no other process may hold that id concurrently, so a surface wearing that name
is provably a dead predecessor of the run about to start. **An unscoped sweep holds no claim over
anything it finds.** It matched on the presence of a label, not on an identity it owns. On a
shared daemon — which is the normal case, `docker` on the build host is shared with other people —
a labelled surface may be another tenant's, or a LIVE task's in a different process whose
registry this one cannot read. Reclaiming on that evidence destroys running work to tidy a list.

The `unclaimed` flag does NOT license removal either. It means "no entry in the registry THIS
process can read carries that nonce", which is exactly as blind to another process's live task as
the scan it replaces. Inheriting core#365's reclaim would be inheriting a guard whose premise
(I own this id) does not hold here.

So: the sweep reports, marks the unclaimed ones, exits non-zero so it is scriptable as a gate,
and PRINTS the removal command rather than running it. That leaves the destroy decision with the
one party that can see the whole host.

## Why Q2 is "no"
"Detect a shell and refuse" is a game of spellings — `/bin/sh`, `env sh`, `busybox sh`, a wrapper
script — that will not hold, and a half-fix buys FALSE COVERAGE, which is worse than a documented gap.
But the issue UNDERSTATES reachability and that part must be corrected: not only a hand-edited
`config.toml` but `ProtocolCommand::AddMcpServer` (`commands.rs:388`) lets the desktop host inject an
arbitrary command+args at runtime, validated only for LENGTH (`main.rs:3546-3583`).

## Why Q5 is "build it"
Bare `NUL` makes Write/Edit DISCARD THE BYTES WHILE REPORTING SUCCESS — silent data loss with a false
success claim. The textbook reserved-name list is REFUTED BY MEASUREMENT: on Win11 26200 only bare
`NUL` is still a device, so the textbook guard would refuse `aux.txt`, `COM1`, `NUL.txt`, `con.json` —
real addressable user files. The narrow guard has already been written and unit-tested green ONCE and
discarded. Record the measurement on the issue so it is not refiled a third time.

## Why Q1 is "A"
The security half is already closed by the #323 union: `@~/.ssh/id_ecdsa`, `@~/.aws/credentials`,
`@/root/.git-credentials` are refused on the absolute path directly. Option B (upward `.gitignore`
discovery) does not exist today. Option C removes a real capability users experience as a regression.
The pin must be written over ESCAPING paths — `@../../secrets/foo.txt` escapes identically, so a test
phrased as "refuse absolute paths" would not close it.

## Why Q6 splits rather than schedules
The core#253 umbrella requests ABSENT behaviour across 8 sub-designs and a 12-line acceptance matrix;
nothing in it describes broken behaviour. Its slice 2 carries a BREAKING migration
(`SHAPE_FIELDS` 13->14, `ADMISSION_SHAPE_VERSION` admission-v2->v3) that invalidates every
`acknowledge_open_admission` token an operator has written — every open-admission channel refuses to
start until re-acknowledged. The buried Telegram defect needs none of that. Ship the defect, park the
feature request, and be explicit that it is parked as a feature.
