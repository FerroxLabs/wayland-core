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
| Q4 / core#314 D-2 | typed protocol refusals | **YES — contract minor bump, with Desktop** | Core work, tracked as core#314 c5. Corrected 2026-08-29: FerroxLabs/wayland#1099 is CLOSED and is a different subject |
| Q6 / core#253 | the umbrella | **Keep open + unscheduled; split the Telegram defect out now** | Do NOT ship slice 2's breaking migration |
| Q7 | Windows merge freeze for Lane W | **YES — a declared window, opened after Lane 0.1 is read** | Serializes the single Windows box |
| Q-113 | core#113 | **CLOSE AS REFUTED**, recording deny-by-default as the decision | Record posted on #113 2026-08-29; the close is queued on FerroxLabs/wayland#1229, with wayland-core#364 filed independently for the same act -- both open, maintainer to dedupe |
| Q-338c4 | core#338 credential surface | **Deny `/dev/tty` via `setsid`, in the SAME change as layer 1** | Layer 1 alone makes the test green while `credential.helper` stays open |
| Q-379 / core#379 | the TEARDOWN of the session Q-338c4 creates | **Kill the process GROUP, on every FAILING exit of a quarantine `git` run and on no other** | `Child::kill` is not a teardown once the child is a session leader; see D-379 below |
| Q-391 / core#244 c4 | is the Windows local-operator shell expected to confine the VCS content store? | **NO — and say so everywhere the product speaks** | Rewrite #244 c4 to its true scope; keep the standing pin test; do not reopen AppContainer |

## D-379 — the teardown Q-338c4 owed and did not record

MASTER-PLAN.md:202 obliged "Layers 1+2 as ONE change, teardown decided in the same change".
Layers 1+2 landed; the teardown did not, and Q-338c4 above did not mention it. This is that
decision, written where it should have been written, one row up.

**What was wrong.** `harden_against_credential_prompt` puts every quarantine `git` child in a
NEW SESSION. The abort paths in `run_git` did `child.kill(); child.wait()` — one pid. Every
helper `git` spawned (credential, askpass, transport) inherited the new group, so no group
signal reached them, and with no controlling terminal no hangup would either. The hardening
therefore made those helpers strictly LESS reachable than before it: previously they shared our
group and our terminal.

**Taken: kill the group.** `terminate_hardened_tree` sends `SIGKILL` to `-pgid`, where the pgid
is the child's pid because `setsid` made it both session and group leader. Safe after the direct
child has been reaped, because a pid is not recycled while it is still in use as a process-group
id, so the signal reaches our surviving members or nothing at all.

**On every FAILING exit, and only those.** Two exits abandon a tree, not one: the wall-clock
timeout, and the drain-grace exit where `git` has already exited and been reaped and a helper's
background worker still holds the inherited pipe. The second is the one the plan did not name and
is graded separately. The SUCCESSFUL exit deliberately does not tear down: `git`'s own
`git-credential-cache--daemon` is in that group, it is shared with the user's other `git`
operations, and killing it after an install that worked would be a regression. A drained pipe is
also evidence that no descendant is holding our stdio.

**Not claimed.** A descendant that calls `setsid`/`setpgid` for itself leaves the group and no
group signal can reach it; that is a sandbox's job, not a teardown's. On Windows the hardening
creates no session and no group — `DETACHED_PROCESS` is a creation-time console decision — so
there is nothing for a group signal to address there. Windows did not regress (it had no group
teardown to lose) but it has no teardown either; that gap is stated in the code and on the ledger
rather than covered by the wording of this row.

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

## Why Q-391 is "no"
core#244 c4 was graded on the unqualified sentence "a Bash subprocess cannot read the store".
MEASURED FALSE on the Windows shipping default for the ordinary interactive user, twice and
independently: on real Windows 10.0.26200.9168 (`ROOT_STORE_LEAKED=true`, `NESTED_STORE_LEAKED=true`,
`RECURSIVE_LEAKED=true`) and on Linux against the same `WindowsJobObjectBackend`, which compiles and
spawns on every target (`LOCAL_OPERATOR_STORE_READ: Exit code: 0 / STDOUT: ROOT-OBJECT-BYTES-244`).

The gate is `shell_requires_os_read_deny()` = `secret_read_deny_required && !local_operator_principal`.
On a backend that cannot enforce read-deny, a NON-local principal loses the shell entirely
(fail-closed, reproduced first) and the LOCAL OPERATOR keeps an unconfined one.

**Taken as "no", for a reason that is already settled elsewhere and is only being written down here:**
delivering the confinement needs a Windows FILESYSTEM sandbox, and the standing ruling is that Windows
gets none — AppContainer is closed and must not be reopened (see core#254's history and the #389 note
carrying the same bar). The job-object default is intended. Removing the local-operator exemption
instead is not an option either: it was added because refusing left every fresh Windows clone with no
shell at all, and the product's own printed remedy `--trust-workspace` hands back the identical
uncontained shell, so the refusal bought nothing and cost the whole tool.

What IS in scope, and is delivered by the change that records this: the product stops claiming more
than it delivers. #244 c4's text now states the scope at which the property holds; the
`is_vcs_content_store_static` doc no longer says `BashTool`'s subprocess is confined full stop; and
`bash_vcs_store_local_operator_gap.rs` asserts the gap IS there on every platform the build host runs,
so the day it closes, someone re-grades instead of quietly agreeing. Scope of the "no": Linux (bwrap)
and macOS (sandbox-exec) both enforce read-deny at their shipping default, so the exemption is inert
there, and every non-local principal on Windows is still refused.
