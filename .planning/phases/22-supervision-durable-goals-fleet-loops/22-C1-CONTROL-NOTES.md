# 22-C1-CONTROL — running notes (lane/22-c1-goal-control)

**Base:** `0fd17cc0a90b24c32ae887fedf5fd1f23a879a10` (`gh/plan/f20-unified-audit-repair`), SHA
asserted against `git ls-remote gh` before any edit. **Worktree:**
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-22-c1-goal-control`.

Deliberately named `22-C1-CONTROL-*` so nothing here overwrites `lane/22-c1`'s
`22-C1-SUMMARY.md` / `22-C1-NOTES.md` / `22-C1-EVIDENCE/`, which are a different lane's evidence.

---

## T+0 — orchestrator's four measurements, re-verified rather than trusted

| Orchestrator claim | My measurement at `0fd17cc0` | Verdict |
|---|---|---|
| CLI observes AND controls (`goal Open/Task/Run/Status/Effects`) | `crates/wcore-cli/src/goal_cmd.rs` is 1107 lines and exists | HELD (detail pending) |
| TUI observes only, by design (`app.rs:277`) | pending re-read | pending |
| `wcore-protocol/src/commands.rs` has ZERO `Goal` variants | `/usr/bin/grep -c Goal …/commands.rs` → **0**, in the same sweep where `-rn Goal …/src -l` returned **3 files** (`contract/spec.rs`, `events.rs`, `goal.rs`) | **HELD.** Absence measured with a live instrument, not a bare zero (§3b-i) |
| Fixtures `goal_snapshot.json` / `goal_transition.json` exist | pending | pending |

**Instrument-liveness note.** The zero above is only worth anything because the *same* `grep`
invocation family returned a non-zero count for a known-positive. A bare "grep returned nothing"
is self-passing (LANE-BRIEF §3b-i).

## T+0 — the corpus pin is 18, but `ProtocolCommand` has 19 variants

`desktop_contract_corpus.rs:216` — `inventory_is_exactly_eighteen_commands_and_fifty_one_events`
asserts `COMMAND_SPECS.len() == 18`. Counting `ProtocolCommand` by hand gives **19**:

Message, Stop, ToolApprove, ToolDeny, InitHistory, SetMode, SetConfig, ContinueWithBudget,
SessionResync, ResumeTurn, ResolveInterruptedApproval, ResolveUnknownToolEffect,
GetRuntimeDiagnostics, AddMcpServer, RemoveMcpServer, GrantWorkspaceCapability, ApprovalResume,
HostSendMessageResult, Ping.

So **`COMMAND_SPECS` is not 1:1 with the enum** — one variant is enum-only. I must establish
which before I report any new expected count, or the number I hand the orchestrator will be
wrong by one. TO DO.

## T+0 — the prior lane already named my exact task, and named the trap

`22-C1-SUMMARY.md` §6 ¶6 ("WHAT IS DELIBERATELY *NOT* IN THIS REQUEST"):

> NO host->core command. A `goal_resync` pull command must be ANSWERED in the CLI command loop,
> which is `crates/wcore-cli/src/main.rs` — FENCED to another lane. A command variant on the wire
> with no dispatcher is a capability nothing answers, which is the false-advertising class this
> program has already paid for twice.

I am that other lane. Two consequences I am binding myself to now:

1. **A dispatcher is mandatory, not optional.** Adding variants without answering them reproduces
   the defect class the prior lane refused to create. My brief says the same thing in different
   words ("rendering is not control").
2. `crates/wcore-cli/src/main.rs` is a **LANE-BRIEF §6 fenced file** — additive only, minimal, ONE
   contiguous block, no reordering. I must capture `BASE` once and diff against the SHA, never
   against the branch name.

Prior lane's suggested increment was `GoalResync` — a **pull**, i.e. still observation. My brief
asks for the **control** verbs (open/task/run/cancel). Those are different; I need both directions
considered, and I must not mistake shipping a resync for shipping control.

## T+0 — expected-RED, declared in advance

Adding to `COMMAND_SPECS` will fail `desktop_contract_corpus.rs:217` and `:318-321`. That is the
tripwire working. LANE-BRIEF §0 forbids `wcore-contract generate`; the orchestrator regenerates
once over the merged tree. **I will not edit the pinned numbers and will not regenerate.** Note
the corpus test was ALREADY RED at base per `22-C1-SUMMARY.md` §5 (14 passed / 1 failed at
`8bcb052b`) — so I must take a base-side differential before claiming attribution for its state.

## T+1 — OPEN QUESTIONS, RESOLVED

**Q1: which variant is enum-only?** `grant_workspace_capability`. Measured by set-differencing
the enum block against the `"commands/*.json"` fixture paths in `COMMAND_SPECS`, **both
directions** — `comm -13` (specs not in enum) is EMPTY, which is what proves the extractor is not
simply dropping entries. So: `COMMAND_SPECS` = **18**, `ProtocolCommand` = **19**, delta = 1.
Any count I report must say WHICH of the two it is.

**Q2: is the command-loop match exhaustive?** **No — and this is the trap of the whole task.**
`crates/wcore-cli/src/main.rs:5431` ends the mid-turn match with

```rust
_ => { eprintln!("[protocol] Ignoring command during active message processing"); }
```

and the idle loop binds a catch-all `other` at `:4935`. **So new `ProtocolCommand` variants
compile clean and are SILENTLY IGNORED at runtime.** The compiler will not force me to write a
dispatcher. This is exactly the advertised-but-dead class the prior lane refused to create, and it
means "it builds" is worth nothing here — only an end-to-end drive is evidence.

**Q3: does an agent-side Goal mutation surface exist?** **Yes.** `GoalKernel` (`goal/kernel.rs`)
exposes real mutations: `open_goal:75`, `start_iteration:100`, `begin_wait:113`,
`resume_from_wait:129`, `terminate:146`, `terminate_verified:160`, plus reads `goal:314` /
`cursor:319`. `GoalLedger` owns the task side. So control does NOT need a new agent-side engine —
it needs a *binding* from the protocol command loop to `GoalKernel`.

**Q4: can the command loop reach a journal?** `SessionJournal` is `#[derive(Debug, Clone)]`
(`session_journal.rs:209`) over an `Arc<Mutex<JournalWriter>>`, and `AgentEngine` holds
`session_journal: Option<SessionJournal>` at `engine.rs:2460` — but **private, with no accessor**
(`grep '^\s+pub fn [a-z_]*(journal|recovery|goal)'` returns only the five `recovery_*` fns). So an
additive `pub fn` accessor on the engine is required.

## T+1 — the scope divergence I must not paper over

The prior lane's suggested next increment was `GoalResync` — a **pull**. That is still
OBSERVATION. My brief asks for **control** (open / task / run / cancel). These are not the same
deliverable and shipping the resync would not close the criterion.

`goal run` is the hard one: `goal_cmd.rs:278-321` shows it drives the real `FleetDispatcher` with
worker subprocesses, waves, leases and shard timeouts. That is a long-running blocking drive, not
a command-loop reply. Putting it inline in the protocol command loop would block the session.
**Flagging now, deciding later, and I will state the outcome rather than quietly dropping the
verb.**

## T+1 — the dispatcher pattern to mirror

`handle_session_resync` (`main.rs:3682`) is the house pattern, and I will match it rather than
invent one: version check → session-identity check → operate → emit typed event, with every
refusal going out as a typed `*_unavailable` event rather than a silent drop or a log line.
