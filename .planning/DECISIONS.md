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
| Q-379 / core#379 | the TEARDOWN of the session Q-338c4 creates | **Kill the process GROUP whenever a quarantine `git` run is ABANDONED — owned by the run's SCOPE, never by its branches** | `Child::kill` is not a teardown once the child is a session leader; branch-by-branch is how the gap was made; see D-379 below |
| Q-391 / core#244 c4 | is the Windows local-operator shell expected to confine the VCS content store? | **NO — and say so everywhere the product speaks** | Rewrite #244 c4 to its true scope; keep the standing pin test; do not reopen AppContainer |
| Q-368-honesty / core#368 | fix AppContainer's categorical deny, or declare it? | **DECLARE IT. Do not build the ACL fix.** | `#368` c1-c3 are AppContainer ACL work on the containment boundary. The standing decision is that Windows ships with NO filesystem sandbox, the Job-object default is intended, and AppContainer is never to be chased again — months were lost to it. What was open was only the honesty of the claim. Obliges: `AppContainerBackend::known_limitations` names the defect and `#368`, graded by a test; `#368` c1-c5 stay OPEN and NOT-MET, owned by whoever reverses the standing decision, and no lane may grade them met by weakening the deny arm |
| Q-369-lease / core#369 | quarantine the unrecoverable lease, or declare the wedge? | **DECLARE IT, and make the cause READABLE.** | Same disposition as Q-368-honesty for the same reason: `#369` c1 is lease-recovery surgery inside the AppContainer backend. `#369` c2 is NOT — a bare `bool` that hid a recorded cause for twelve days is a product-honesty defect on a surface every operator reads, so c2 is CLOSED here (`sandbox status` prints the recorded probe cause, human and `--json`). Obliges: c1, c3 and c4 stay OPEN on `#369`; the wedge is declared in `known_limitations` so no future operator loses a fortnight rediscovering it |
| Q-369c4 / core#369 c4 | what to do about the package ACEs already leaked onto a home directory | **TELL THE OPERATOR WHERE TO LOOK; do not auto-revoke.** | An automatic sweep of `S-1-15-2-*` ACEs across a user's home directory is a destructive, unattended, privileged operation whose blast radius is the whole profile, written to repair a defect measured exactly once. The leaked ACE grants a package SID that no longer has a profile, so it is inert until an AppContainer with the same SID is recreated. Obliges: the wedged-lease limitation names the lease directory so an operator can find the recorded intents; `#369` c3 (find what recorded a whole-home grant) stays open and is the thing actually worth fixing — a revocation tool for a leak still being produced is treating the symptom |
| Q-389c2 / core#389 | Windows quarantine console: reach c1's property, or take c2's branch? | **TAKE c2 — LABEL the prompt. c1 is unreachable.** | `#389` measured both remedies foreclosed: reparenting is defeated by `AttachConsole(<pid>)` and a private console by `FreeConsole()` first. Windows has no session-leader equivalent, and the AppContainer route is closed by the decision above. PRODUCT COST, stated: this does NOT stop a determined child re-attaching to the operator's console — it only lets the operator ATTRIBUTE what appears. It also costs one unconditional stderr line per quarantine `git` spawn on Windows, which is noise on a non-interactive host, accepted because a notice that fires only when it guesses a human is watching is a notice that is absent exactly when it is wrong about that. Obliges: `#389` c1 stays OPEN and NOT-MET, and the residual pin measuring the bypass is KEPT, not deleted |

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

**On every ABANDONED run — which is NOT the same set as every failing one.** The first form of
this decision said "on every FAILING exit" and that was wrong in both directions, so it is
restated here rather than left to be discovered.

WRONG IN ONE DIRECTION: `run_hardened` ends with `Err` on a nonzero `git` status, and that exit
deliberately does NOT tear down. `git` ran to completion and said no; both pipes reached EOF, so
nothing it spawned is holding our stdio; and `git`'s own `git-credential-cache--daemon` is in that
group and is shared with the user's other `git` operations. Killing it because a clone failed
would be the same regression as killing it because a clone succeeded.

WRONG IN THE OTHER: the plan counted TWO abandoning exits and there were three. The wall-clock
timeout is the one #379 measured; the drain-grace exit — `git` already exited AND reaped, a
helper's background worker still holding the inherited pipe — is the second; and `try_wait`
returning `Err` was the third, propagated with `?`, abandoning a child that is still RUNNING and
unreaped. That third exit sat in the same function throughout and a teardown written as a line
copied into the two known branches would not have covered it.

**So the teardown is owned by the SCOPE.** `HardenedTree` is armed once, immediately after the
spawn, and its `Drop` tears the session down; every `Err` path — including one nobody has written
yet — inherits that without being told. There is exactly ONE `disarm` site, reached only after
both pipes have hit EOF, and it is the single place this codebase claims a tree is finished rather
than abandoned. Enumerating branches is how #338 opened this hole; the decision is deliberately
not to enumerate them again.

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


## Egress: split the allowlist grant by traffic origin (FerroxLabs/wayland#1264)

**Decision: SPLIT.** Provider/LLM traffic to an allowlisted host keeps its unconditional `Allow`.
Tool-driven traffic to the SAME host is shape-checked, and a data-bearing tool request is DENIED when
the session has no approval surface to ask.

### What was measured

`classify()` receives method + URL only (headers and body are never read) and returned `Allow` on the
host match, BEFORE the method check and before the path/query check. `get_carries_data` had exactly
one call site, on the non-allowlisted branch, so for any of the 38 hosts on the shipped
`WELL_KNOWN_DOMAINS` default the shape check was UNREACHABLE. `WebFetch` is GET-only but takes an
unrestricted URL, so the query string is model-chosen. Net: in an unattended run a `WebFetch` of
`https://<allowlisted-apex>/?leak=<secret>` was admitted with no approval in any mode.

Filed from source reading. It has since been issued: `crates/wcore-agent/tests/egress_tool_origin_test.rs`
drives the real `HttpFetchBackend` against the real `AgentEgressPolicy`.

### Why not the two obvious alternatives

Both were put to independent external review and both were refuted.

* **A per-client policy** — give tool clients a stricter `EgressPolicy`. The boundary would then
  depend on WHO CONSTRUCTED THE CLIENT, so every code path able to build a client becomes a bypass
  factory. There is one policy; only the request is labelled.
* **Excluding `-` from the data-bearing token run** to cut false positives. `-` is in the alphabet of
  every base64url secret, so dropping it blinds the check to exactly the payload it exists to see.

Narrowing the allowlist was never available: it would deny the agent's own LLM POSTs, and
`classify::tests::post_to_allowlisted_host_is_allowed` + `policy::tests::allowlisted_post_is_allowed`
pin that as intended. Both now carry a comment naming this decision.

### The shape taken

Request ORIGIN is stamped centrally and the one policy is keyed on it.

1. `wcore_egress::EgressOrigin` (`Provider` / `Tool`) travels in the `x-wayland-egress-origin` header,
   because `reqwest::Request::extensions` is `pub(crate)` in 0.12 and a header is the only per-request
   channel a caller can write and a policy can read. `EgressRequestBuilder::send` REMOVES it after the
   policy has read it and before dispatch, at the one seam every outbound request passes through, so
   it never goes on the wire.
2. `build_ssrf_safe_tool_client` stamps `Tool` once for the whole tool surface (WebFetch + the github
   / gitlab / linear / notion backends), so the label's completeness is a property of that constructor
   and not of whoever adds the next tool. An ABSENT marker reads as `Provider`: the opposite default
   would refuse the agent's own traffic on every unmarked path.
3. `classify` gains an origin parameter and a fourth verdict, `ToolData`, returned only for a
   tool-originated, data-bearing request to an ALLOWLISTED host.

### Why `ToolData` is not `Ask`

`egress/policy.rs`'s `resolve_ask` FAILS OPEN when no consent doorbell is wired — `return
EgressDecision::Allow` — and that is deliberate and correct: nothing sensitive leaves on a data-less
read to a new host. A shape check resolved through it would therefore classify the leak correctly and
allow it anyway. Theatre.

Blanket-denying every `Ask` in an unattended session is not the fix either: it breaks legitimate
unattended traffic, which is a wrong-refusal shipped in the name of a hardening. So the deny is scoped
to `ToolData`, a verdict provider traffic can never reach. With an approval surface present the
operator gets a prompt and the tool keeps working; an "always" answer allows that request only,
because the host was already allowlisted and the question asked was about the payload.

### Redirects

`reqwest` follows a redirect INSIDE `Client::execute`, so every hop after the first never reached the
egress chokepoint — the shape check would have been correct on the request and absent on the hop that
carried the payload. `ssrf_safe_redirect_policy` cannot close this: its per-hop callback is
synchronous and the egress policy is async over shared state, so it can answer an SSRF question and
not an egress one. `HttpFetchBackend` now follows redirects itself, one hop at a time, re-issuing each
through `EgressRequestBuilder::send`. Same 10-hop bound, same `is_safe_url` floor per hop; what
changed is where each hop is CHECKED.

### What an operator sees

The refusal text names the host and says plainly that being on the egress allow list permits the agent
to REACH a host and does not permit a tool to choose what to send to it. The reasoning is here, and
`classify`'s own doc comment points at it.

### Known bound, stated rather than implied

This is a cooperative label. A tool that built its own `EgressClient` without the stamp would be
classified as provider-origin. That is strictly better than the per-client alternative — where the
same tool would simply get the weaker POLICY — but it is not a hard boundary, and calling it one would
be the overclaim this ticket was split out of #1195 to avoid.
