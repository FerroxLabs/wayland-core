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
| Q-1264 / wayland#1264 | is an allowlisted apex admitted on the host match alone for MODEL-chosen URLs too? | **NO — split the grant by request ORIGIN, not by client** | Stamp origin centrally on the request; keep product traffic's unconditional allow; only `WebFetch` is model-directed today |
| Q-1172c3 / wayland#1172 c3 | what happens when the LEARNED served window is one core cannot compact in (the 4,096 slot) | **NARROW ONTO IT UNCONDITIONALLY AND REFUSE THE RUN OUT LOUD** | No `supports_compaction` escape hatch in `narrow_to_served_window`; the refusal carries `minimum_workable_window` and the `num_ctx` remedy; the truncation notice needs the matching third arm |
| Q-1218 / wayland#1218 | clamp `size_output_cap` to the withheld RESERVE, or to the room left in the window in force | **TO THE ROOM IN THE WINDOW IN FORCE** | Never clamp the ask to the reserve at every input - that cuts a 200k Claude turn to the compaction reserve |
| Q-1200 / wayland#1200 | bound the tool-result BUDGET only, or the protected tail too | **BOTH, and record the term that cannot be bounded** | The per-result ingestion cap is not window-derived; the named gap belongs beside the arithmetic, not in a lane report |
| Q-1255 / wayland#1200 residue | fix the unbounded stub term here, or ticket it | **TICKET IT — FerroxLabs/wayland#1255 — and pin the arithmetic in-tree.** SUPERSEDED 2026-08-31: **FOLD IT, and price the cache write** | The pin discharged neither arm of the ticket's own c1, and c2 asks for the opposite polarity of the pin. The fold costs one uncached turn per ~237 dropped results; the leak costs the session |
| Q-369-lease-R / core#369 c1+c3 | REVERSES Q-369-lease for c1 and c3: build the lease-recovery bound and the over-broad-grant refusal, or go on declaring them? | **BUILD BOTH.** | Q-369-lease read `#369` c1 as AppContainer MECHANISM work and declined it under the standing decision. That reading was too wide, and what it cost is measured rather than argued. The standing decision is about CONTAINMENT -- Windows ships no filesystem sandbox, the Job-object default is intended, and AppContainer is never to be pursued as a containment story. `#369` c1 is not a containment property: it is a FAIL-CLOSED AVAILABILITY defect in which ONE unrecoverable file made `is_available()` false forever and the product then refused EVERY sandboxed command for twelve days on a developer machine. Declaring that in `known_limitations` tells an operator why they are wedged; it does not unwedge them. c3 is not containment work either -- it is the PRODUCER of the whole-home package grant, and Q-369c4 declined to build a revocation tool ON THE GROUNDS that the producer was still open, so leaving c3 unbuilt leaves that decision resting on nothing. PRODUCT COST, stated: an operator who runs wayland-core from their home directory with the opt-in AppContainer backend now gets a fail-closed refusal instead of a whole-profile package grant. Obliges: c1 and c3 are graded with red arms on real Windows, both restores blob-verified; Q-368-honesty is UNCHANGED and `#368` c1-c5 stay NOT-MET -- no lane may cite this row to reopen the AppContainer ACL containment fix |
| Q-368-disposition / core#368 + core#410 | core#368 c1-c5 ask for an AppContainer ACL fix that Q-368-honesty forbids building, while `windows-live-acceptance` -- a REQUIRED job -- fails ~1 run in 5 *because* the fix is absent, and core#350 c5 cannot go green until that soak does. Which gives? | **THE STANDING DECISION STANDS; core#368 MOVES TO 0.13.13.** Not a bypass and not a re-argument: c1-c5 name work this release has decided not to do, so as 0.13.12 blockers they are a gate that CANNOT PASS, which is worth exactly as much as one that cannot fail. The issue keeps its ledger, its criteria, its owner and its tracking; it is owed by whoever reverses the standing decision. c6 -- the DISCLOSURE, which is the half that protects the user -- is already `met` and ships in 0.13.12. | Windows ships with NO filesystem sandbox by decision; the Job-object default is intended; AppContainer is never to be chased again. The soak-job contradiction is real and is recorded on core#410 with line-level citations -- it is a CI-gating question, which is where 0.13.13 already holds the CI-flakiness and test-infra work. Deliberately NOT done here: excluding the test by name from PHASE L, because the soak script hard-asserts WAYLAND_SOAK_MIN_TESTS_LIVE_FS_ACL=13 precisely to stop enumeration silently degrading that gate, and I cannot test a Windows-only script change from this host. |

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

## Why Q-1172c3 refuses rather than narrows-and-bricks (the 4,096 decision)
This was the explicit blocker on wayland#1172 c3, and it was parked twice: the ticket's own close
note says "wiring the learned window into the guard today makes a small-window run fail outright,
because the fixed buffers saturate to zero at 4,096". Three options, and the third is taken.

**A. Keep the `supports_compaction` gate (the status quo until this release).** A corroborated
4,096-token served window was deliberately NOT narrowed onto, so core went on sizing against
`UNVERIFIED_CONTEXT_WINDOW` = 32,768 - 8x the window the endpoint had been OBSERVED to serve - and
kept sending ~10.5k-token prompts that Ollama silently truncated. This is the state wayland#1172
refused to let #1150 close, and it fails on the FAIL-OPEN side: the model answers fluently from a
context that no longer holds the system prompt or the task. Declining to narrow did not avoid a
brick, it hid one.

**B. Narrow unconditionally and let the boundaries fall where they fall.** At 4,096 with
`MAX_RESERVE_FRACTION = 0.55` the scaled input ceiling is 2,527 and the autocompact threshold 1,844,
both under `BASELINE_TURN_TOKENS` = 3,118. Every run would terminate at the pre-flight guard before
the user's first turn, with a generic overflow error. Neither degradation rung can recover it -
rung 1 sheds tool RESULTS and rung 2 truncates or drops MESSAGES, while `est(&[]) == overhead` -
because the floor is the system prompt plus the tool schemas, which compaction does not touch.
Louder than A, and correct about the window, but it reads to the operator as a product bug.

**C. TAKEN: narrow unconditionally, AND refuse the endpoint out loud, once, with the number that
fixes it.** The escape hatch is gone from `narrow_to_served_window`, so nothing sizes against a
window the endpoint has been observed not to serve. `unworkable_window_refusal` terminates the run
at the TURN-LOOP TOP, before `run_compaction`, naming `minimum_workable_window` (6,929 at the
default reserves, computed rather than hardcoded) and the `num_ctx` / `OLLAMA_CONTEXT_LENGTH` /
`[compact] context_window` remedy.

The reason C is not B with better copy: B's failure arrives from the pre-flight guard AFTER a prompt
has been assembled, and its message is about token counts. C's arrives before the provider is called
at all - zero requests are sent, so no prompt the endpoint would truncate ever leaves - and its
message is about the endpoint's configuration, which is the only thing the operator can act on.

**What this obliges, and why the two could not be sequenced apart.** The gate's removal and the
refusal had to land together: A alone is the reported defect, B alone is a worse-looking version of
it. It also obliges the truncation notice to grow a third arm, because "Core is now sizing this
session against the {served}-token window" becomes false in the one case C is about - that is
wayland-core#382 c1, and the arm reads "Core cannot size a session against N tokens at all".

## Why Q-1218 clamps to the window, not to the reserve
wayland#1218's title is "the scaled `output_reserve` is decoupled from the `max_tokens` core actually
sends", which reads as an instruction to clamp the ask to the reserve. Doing that literally is a
regression: on a 200,000-token window the scaled reserve is 20,000 tokens, so a Claude turn with a
real 64,000-token output ceiling would be cut to under a third of it on every turn, on every large
model, to fix a defect that only exists below a 49,152-token window.

What the ticket actually measures is an OVERFLOW - "total ask 13,245 on an 8,192 slot" is
ceiling 5,053 + ask 8,192 - so the property is `admitted input + asked output <= window`, and
`room(window_in_force) = window - est - WINDOW_BUFFER` delivers exactly that at every input, not
only the worst one. It is also IDENTITY wherever the window in force is the catalogued window, which
is every registry model absent a #1172 narrowing, so no large-window sizing moves. Pinned by
`a_window_in_force_that_is_the_catalogued_one_changes_no_sizing`, which would fail under the
literal reading.

## Why Q-1200 bounds both terms and names the one it cannot
wayland#1200's worst case is `total_budget_bytes + keep_recent x max_result_size` = 120,000 +
4 x 50,000 = 320,000 bytes, about 80,000 tokens on a 32,768-token window. Bounding only the budget
leaves the protected tail dominating - 200,000 bytes of it - so the ticket would be half-closed with
a number that still does not fit. Capping the tail by COUNT instead of by BYTES was rejected: it
drops the tail to one result on any window under ~100,000 even when the results are small, and a
stubbed working set is how the re-read loop wayland#1172 reports begins.

The term that cannot be bounded here is named rather than hidden: the NEWEST tool result is
protected unconditionally, and its size is the per-result ingestion cap
(`wcore_tools::Tool::max_result_size` = 50,000 chars), which is not window-derived. So the budget is
sized against `admissible - max(admissible/2, MAX_TOOL_RESULT_BYTES)` - room is left for that one
result rather than pretending it is not there. Below a ceiling of about 12,500 tokens that single
result exceeds the window on its own and no arithmetic in `wcore-config` can change it; that window
is unworkable by Q-1172c3's test and is refused by the turn loop, which is where it belongs.

## Why Q-1255 (the THIRD term) is ticketed rather than fixed in this lane

Q-1200 above bounds two terms and names one it cannot bound. Answering this lane's refutation
turned up a **third** term that neither the ticket's arithmetic nor that decision mentions, because
until the pass was driven and measured rather than predicted, nobody had looked at what it leaves
behind:

```
carried = protected_tail + dropped_results x stub_len          stub_len = 130 bytes
```

`bound_accumulated_tool_results` replaces each over-budget result with a stub and never re-mutates
one. Measured on a 32,768-token window at HEAD: 52,470 bytes at 20 tool calls, 62,870 at 100,
114,870 at 500, **309,870 = 77,467 tokens at 2,000** — 2.36x the whole window. The window's own
ceiling (80,832 bytes) is crossed at about **238 tool calls**. That is wayland#1150's reported
symptom, reached by a longer session rather than by a bigger budget, and 2,000 tool calls is an
ordinary agent session.

**It is not fixed here, and the reason is a real trade rather than a schedule.** The residue exists
*because* the pass is monotone: an already-stubbed body must never change bytes again, or the
provider's cached prefix is invalidated on every turn — which is the entire discipline
wayland#1150 c6 and wayland#559 were built on. The only fix that changes the O(n) is to collapse
runs of adjacent stubs, and that means re-mutating a stubbed body. A plausible shape is to collapse
at epoch boundaries only, reusing the `epoch_results` quantization the pass already has, so the
prefix is rewritten once per epoch instead of once per turn — but the cost of that rewrite has not
been measured, and improvising it inside a lane answering a refutation is how the first #1200 fix
came to be graded against a predictor instead of against the pass.

Shortening `bounded_result_stub` is explicitly **not** a fix: it moves the constant, not the
order of growth.

So: filed as FerroxLabs/wayland#1255 with the measurements and the trade written out, and the
arithmetic pinned in-tree by `the_carried_payload_grows_by_one_stub_per_dropped_result` so the term
cannot be rediscovered as a surprise. The corresponding false gloss — that carried bytes "stop
growing with the session" — has been removed from wayland#1150 c4's ledger note, and the
`+ 20_000` slack that hid the term (the real difference between a 20-call and a 100-call session is
10,400 bytes) has been replaced by an equality on it.

### SUPERSEDED 2026-08-31 — Q-1255 is FOLDED, and the cache write is priced

The decision above was to ticket the third term and pin its arithmetic. Executing wayland#1255 in
lane `f13-ctx-1255` showed that the pin discharges **neither** arm of the ticket's own c1, so the
ticket could not have been closed on it:

* c1's second arm is *"the prompt-cache cost of **bounding** them is measured and the tradeoff
  recorded as a decision"*. The section above records the tradeoff and then says, in its own words,
  that *"the cost of that rewrite has not been measured"*. Recorded, not measured — the arm is open.
* c2 asks for the **opposite polarity** of the pin: the same call,
  `bound_accumulated_tool_results(.., Some(32_768))`, at a session past the crossing, asserting the
  carried payload **fits**. No statement *about* the pass can satisfy that, however well measured.
  Closing c1 on the pin would have left c2 permanently unmeetable while the ticket read as answered.

So the behaviour changed, along the shape this section itself proposed: **collapse at a boundary,
not per turn.** `fold_bounded_tool_rounds` elides whole already-stubbed tool ROUNDS — `ToolUse`
together with its `ToolResult`, matching ids, all results already stubbed, nothing carrying text or
thinking — down to a single aggregate, and only when the stub residue exceeds `total_budget_bytes`.

**The cache cost, now measured rather than deferred.** A fold rewrites history at the front, so the
provider's cached prefix is invalidated whole: one uncached turn. What buys it back is frequency.
The residue grows one stub per dropped result and the fold fires at `total_budget_bytes`, so the
interval is `30,832 / 130 = 237` dropped results on a 32,768-token window and `120,000 / 130 = 923`
on the flat unknown-window constants. Driving the real pass turn-by-turn for 800 turns on a
32,768-token window: **3 folds — one prefix rewrite per 267 turns.** Below the threshold the fold
does nothing at all and the pass is byte-identical to before, which is why every pre-existing
prompt-cache test (`the_ceiling_is_byte_stable_on_a_second_pass`,
`the_ceiling_advances_in_epoch_sized_batches`,
`a_large_window_leaves_the_pass_byte_identical_to_the_unknown_window_arm`, and the four
`compact_tool_call_args` monotonicity tests) passes unedited.

The trade, stated as the two numbers it is: **1 uncached turn in 237**, against a session that
otherwise carries 309,870 bytes = 77,467 tokens at 2,000 tool calls on a window whose pre-flight
guard admits 20,208 and aborts the run. After the fold the same session carries 50,295 bytes =
12,573 tokens, and 20,000 tool calls carry 50,296.

**The wrong-refusal side, weighed above the leak.** `bounded_result_stub` ends *"re-run the tool if
you still need them"* and says it on an `Edit` result as readily as on a `Read` — one standing
invitation to re-execute a mutating tool per dropped result, 1,999 of them at 2,000 calls. The
aggregate that replaces a run of them says the opposite (*"Do NOT re-run a tool that changed state;
inspect the current state instead"*) and names how many of the elided calls were state-changing, a
running count carried across later folds. The exposure falls from `n-1` invitations to none outside
the protected tail. What is genuinely lost is the tool names and arguments of the elided rounds,
whose results were already stubs; that is the price the aggregate's counts partially repay, and it
is why the fold refuses to touch any message carrying model reasoning.

Shortening `bounded_result_stub` remains **not** a fix, for the reason given above: it moves the
constant, not the order of growth. So does any scheme leaving `k > 0` bytes per call. The count of
carried bodies, not their size, is what had to stop growing.
