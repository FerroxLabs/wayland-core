# 21-C3-EQUIVALENCE — Criterion 3, the differential made able to run

**Lane** `21-c3-equivalence` · branch `lane/21-c3-equivalence` · base
`5be910561f688c75d39492e7b982d6e100772a64` on `plan/f20-unified-audit-repair`
(asserted against `git ls-remote gh`) · **HEAD `9c7677b6`**.

Criterion: *"Standalone and host-protocol hostile corpora prove equivalent enforcement."*

**Verdict: NOT MET, and materially closer.** The tool dimension — the only dimension that had
**zero** running equivalence pairs — now has a decisive, differential, failure-capable pair in
process on both surfaces. Two cells remain non-decisive on tool, and five dimensions remain
non-decisive live. Details in §6.

---

## 1. Which of the orchestrator's measurements held

Everything below re-measured at base on `hetzner-dsm`, unproxied cargo, not inherited.

| Brief claim | Held? |
|---|---|
| "zero ALLOWED outcomes at HEAD" | **HELD.** Zero ALLOWED on any dimension, surface or mode, before and after. |
| "`spawn_host_child` hardcodes `ForkOverrides::default()`" | **HELD, one frame downstream.** Not at `spawn_host_child` (`spawner.rs:1110`) but at `spawn_one_with_origin` (`spawner.rs:1867-1873`), which it called. |
| "the host child-spawn request type carries `[name, prompt, max_turns, max_tokens, system_prompt, provider, model, temperature]`" | **HELD.** `SubAgentConfig`, `wcore-types/src/spawner.rs:518-539`. |
| "tool and fan-out cannot be *requested* over the protocol" | **HELD for tool. FALSE for fan-out in-process, and the live cells were already closed.** |
| **"Fan-out is reported undetermined live on both surfaces (0 provider requests by a delegated child)"** | **FALSE at HEAD.** Measured `corpus_fan_out :: standalone :: live :: REFUSED` and `:: host-protocol :: live :: REFUSED`. Lane `21-c3-hostile` closed both with an at-cap control before my base. |
| the 11×4 table quoted from `21-REVERIFICATION.md:170-184` | **8 of 44 cells had moved.** See §2. |
| **"the host child-spawn request type in `wcore-protocol` … a schema change ⇒ fenced seam, Desktop must re-pin"** (`CRITERIA-GAP-LEDGER.md`) | **FALSE.** See §5 — this is the load-bearing correction. |

## 2. The measured 11×4 at base, against the brief's table

Extracted with `grep -oE '^COMBINATION :: corpus_[a-z_]+ :: linux :: [a-z-]+ :: [a-z-]+ :: [A-Z-]+'`
from an unproxied `--nocapture` run.

| Dimension | SA in-proc | HP in-proc | SA live | HP live | vs brief |
|---|---|---|---|---|---|
| tool | **NOT-EXPR** | NOT-EXPR | **REFUSED** | **NOT-EXPR** | 3 of 4 differ |
| depth | REFUSED | REFUSED | **REFUSED** | **REFUSED** | 2 differ |
| fan-out | REFUSED | NOT-EXPR | **REFUSED** | **REFUSED** | 2 differ |
| provider | NO-CHANNEL | **REFUSED** | NO-CHANNEL | NO-CHANNEL | 1 differs |
| filesystem / egress / secret / time / token / cost / approval | — | — | — | — | held |

## 3. This was a PROOF gap, not an enforcement hole — established, not assumed

The brief asked me to establish which before building. It is a proof gap, and the evidence is
structural rather than statistical:

- **`build_tool_registry` has exactly ONE production call site** — `spawner.rs:1186`, via
  `child_tool_registry`. Every other hit in `crates/` is a comment or a test. Search stated so
  it can be re-run: `/usr/bin/grep -rn "build_tool_registry" crates/ --include="*.rs"`.
- Its intersection has **no skip arm**: `let permitted = permitted && parent_tool_authority.contains(*name);`
  (`spawner.rs:2756`), with `parent_tool_authority` a required parameter, not an `Option`.
- **Both surfaces converge on it.** `spawn_fork` → `spawn_durable(..., Delegate)`
  (`spawner.rs:2598`); `spawn_host_child` → `spawn_one_with_origin` → `spawn_durable(...,
  ForkOverrides::default(), Host)` (`spawner.rs:1867`). Same seam, same intersection.

So the host surface was never *less enforced*. It was **unable to ask**, and an unasked question
returns a WITHHELD verdict. `Outcome::is_decisive` (`surfaces.rs:135-137`) is
`Refused | Allowed | NoChannel`, and `assert_surface_equivalence`
(`child_authority_corpus.rs:341-343`) `continue`s when either side is non-decisive — so every
NOT-EXPRESSIBLE cell silently **skipped** the equivalence assertion.

Applying that to the base table, **`tool` was the only dimension with no running equivalence pair
in either mode.** That is a sharper and more actionable statement of the unmet criterion than
either the brief or the ledger makes.

## 4. What landed

**Production (`crates/wcore-agent/src/spawner.rs`, additive only):**

- `AgentSpawner::spawn_host_child_with_overrides(config, overrides)` — carries the caller's
  `ForkOverrides` to the same `spawn_durable` seam `spawn_fork` uses, with `ChildOrigin::Host`.
- `HostChildController::spawn_child_with_authority(config, overrides)` — the host-facing wrapper.
- `spawn_host_child` now delegates with `ForkOverrides::default()`, so **every existing caller is
  byte-identical**. No signature changed, no struct gained a field, no wire shape moved.

It grants nothing: `allowed_tools` can only SELECT from what the parent holds (intersected at
`build_tool_registry`), and `budget` can only LOWER a cap.

**Corpus (`crates/wcore-cli/tests/child_authority_corpus/surfaces.rs`):**

- `ChildSpawnSeam { Delegate, HostChild }` threaded through `tool_arm` / `tool_arm_inner`, so the
  identical two-arm differential runs on both surfaces with the spawn seam as the only variable.
- `HostProtocolInProcess`'s tool dimension drives that differential instead of recording
  NOT-EXPRESSIBLE.
- `ToolArm::obtained_any_mutating_tool()` — the known-positive gate now accepts **either**
  mutating observable. See §4.1; this is the change that actually unblocked the dimension.

### 4.1 FINDING (in the corpus, REPAIRED) — a working differential was sitting unread

The gate was keyed on `granted.obtained_mutating_tool` alone: a `Write` sentinel written and read
back. That leg **cannot ever succeed** — `Write`/`Read` demand an absolute path and a delegated
child's checkout is allocated at `<session>/delegated-workspaces/checkouts/<worker_id>`, unknown
to a scripted corpus (21-C3-04, still open). It returns `path must be absolute` in every run.

Meanwhile, in the **same output**, the arms already read:

- ARM-GRANTED: `shell executed: true`
- ARM-DENIED: `shell executed: false`, returning `Denied by policy: no matching grant for
  actor+resource+action`

`Bash` and `Write` are the same class — both outside `SHARED_READ_ONLY_CHILD_TOOLS`, both
admitted by exactly one predicate. Either executing is a positive observation. The gate was
reading past a live differential and reporting NOT-EXPRESSIBLE.

**This only became true recently.** While 21-C3-01 was open the shell died in bubblewrap before
running, so *both* observables were dead and NOT-EXPRESSIBLE was the correct answer. The
`f21-bwrap-overlap` lane's renderer fix unmasked the `Bash` leg; nothing re-read the gate
afterwards. Two now-stale rationales in the probe's own doc comments are corrected in the same
commit.

## 5. CONTRACT: no seam request, no regeneration, no re-pin — the prediction was wrong

The orchestrator instructed me to expect `desktop_contract_corpus` to go RED on its count pins
(23 commands / 52 events, `CONTRACT_MINOR = 10`) and to report new counts for re-pinning.

**It does not go red, and there is nothing to re-pin.** Measured at HEAD `9c7677b6`:

```
cargo test -p wcore-protocol --test desktop_contract_corpus
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.60s
```

Why, established before I wrote any code:

- `SubAgentConfig` and `ForkOverrides` live in **`wcore-types`**, not `wcore-protocol`.
  `/usr/bin/grep -rn "SubAgentConfig\|ForkOverrides" crates/wcore-protocol/` returns **0 hits**
  (instrument alive: the same pattern over `crates/` returns 20+ files).
- **`ProtocolCommand` has no child-spawn variant at all.** Its 24 variants are Message, Stop,
  ToolApprove, ToolDeny, InitHistory, SetMode, SetConfig, ContinueWithBudget, SessionResync,
  ResumeTurn, ResolveInterruptedApproval, ResolveUnknownToolEffect, GetRuntimeDiagnostics,
  GoalOpen, GoalDeclareTask, GoalAdvance, GoalCancel, GoalResync, AddMcpServer, RemoveMcpServer,
  GrantWorkspaceCapability, ApprovalResume, HostSendMessageResult, Ping.
- The pins read `COMMAND_SPECS.len()` / `EVENT_SPECS.len()`, which enumerate wire commands and
  events. My change adds neither. `SubAgentConfig` has no `Serialize` derive.

**Counts are unchanged: 23 commands, 52 events, `CONTRACT_MINOR = 10`.** The "host-protocol
surface" in this corpus is the production `AgentBootstrap` object graph plus
`HostChildController`, not a wire command — so the ledger's framing of this criterion as needing
a fenced Desktop seam is wrong, and the cost estimate that rests on it is too high.

**I did not run `wcore-contract generate`.**

## 6. The differential result, dimension by dimension

`cargo test -p wcore-cli --test child_authority_corpus`, `hetzner-dsm`, HEAD `9c7677b6`:

```
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 24.30s
```

`WLRC=0` + `WLDONE` present. All counts read back from unproxied cargo with `ignored` and
`filtered out` both present, per LANE-BRIEF §3b.

| Dimension | in-process pair | live pair | change |
|---|---|---|---|
| **tool** | **RUNS — REFUSED / REFUSED** | skipped (HP NOT-EXPR) | **was skipped in BOTH modes** |
| filesystem | runs — REFUSED / REFUSED | runs — REFUSED / REFUSED | — |
| secret | runs — REFUSED / REFUSED | runs — REFUSED / REFUSED | — |
| depth | runs — REFUSED / REFUSED | runs — REFUSED / REFUSED | — |
| fan-out | skipped (HP NOT-EXPR) | runs — REFUSED / REFUSED | — |
| egress | skipped (HP NOT-EXPR) | runs — REFUSED / REFUSED | — |
| provider | runs — NO-CHANNEL / REFUSED (mechanism difference) | runs — NO-CHANNEL / NO-CHANNEL | — |
| approval | runs — NO-CHANNEL / REFUSED (mechanism difference) | runs — NO-CHANNEL / NO-CHANNEL | — |
| time / token / cost | runs — REFUSED / REFUSED | skipped (both NOT-EXPR) | — |

**Zero ALLOWED on any dimension, any surface, any mode.** Two
`SURFACE-MECHANISM-DIFFERENCE` rows (provider, approval — both in-process, neither widened),
printed rather than asserted on, unchanged from base.

**Net: running equivalence pairs went from 15 to 16 of 22.** The one added is the one that
matters most, because tool was the only dimension at zero.

## 7. Three-assertion self-test on the new gate (LANE-BRIEF §6b-ii)

The injection simulates a **real one-surface bypass**: the host arm skips
`narrow_parent_tool_authority` while standalone still applies it. Applied on hetzner, never
committed, reverted with `git checkout -- <one path>` and the tree re-verified green.

**(1) Known-positive passes** — clean tree, HEAD `9c7677b6`:
`29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`, tool REFUSED / REFUSED in process.

**(2) Known-negative genuinely fails** — `WLRC=101`,
`test result: FAILED. 28 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out`, verbatim:

```
thread 'corpus_tool' (2232823) panicked at crates/wcore-cli/tests/child_authority_corpus.rs:344:9:
assertion `left == right` failed: SURFACE-EQUIVALENCE FAILURE :: corpus_tool :: dimension tool
:: mode in-process :: standalone REFUSED (obtained: no mutating tool — nothing the read-only
parent does not hold) against host-protocol ALLOWED (obtained: a mutating tool the read-only
parent does not itself hold). One surface enforces and the other does not, so the weaker path is
a bypass of the stronger.
  left: false
 right: true
```

The injected host ARM-DENIED reports `shell executed: true` — the un-narrowed host child really
did obtain `Bash`.

**(3) The old shape would have missed it** — the assertion that proves the repair does anything.
Same injection left in place, gate reverted to `obtained_mutating_tool` only:

```
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 24.29s
COMBINATION :: corpus_tool :: linux :: host-protocol :: in-process :: NOT-EXPRESSIBLE
```

**The suite goes GREEN while the host surface is bypassing parent tool authority**, because a
NOT-EXPRESSIBLE cell makes `assert_surface_equivalence` skip. That is precisely the vacuity the
criterion's word *equivalent* is about, demonstrated rather than argued.

## 8. Fan-out: already determinable, and I did not need to make it so

The brief asked me to determine fan-out if my work made it determinable. **It was already
determined before I started** — `REFUSED` on both live surfaces at base, closed by
`21-c3-hostile`'s at-cap control. Live, both surfaces run the real binary and breadth is
requested through the `Spawn` tool, so the seam is genuinely shared.

The remaining fan-out gap is the **host-protocol in-process** cell only, and I deliberately did
NOT close it. The breadth cap lives in `SpawnTool::execute` (`spawn_tool.rs:178-193`) — the tool
layer — and `spawn_parallel_with_extras_origin` applies **no cap at all**. Closing that cell
requires giving `HostChildController` a batch entry point, which is **adding attack surface in
order to test it** — the exact trap the phase names. The live pair already proves the property.
Named, not fixed.

## 9. What I did NOT do

- **Windows is unmeasured for this criterion by me.** I did not run `child_authority_corpus` on
  `SeanD@seandesktop`. I make no claim about it in either direction; the brief already records
  that nobody has measured it at this SHA.
- **The `tool` live cells were not closed.** HP live remains NOT-EXPRESSIBLE (*"the delegated
  child's shell never ran"*, `Tool execution denied by user` — the 21-C3-03 confirmer). SA live
  remains REFUSED but *attributed to workspace containment, not tool authority*. My change is
  in-process only.
- **21-C3-04 was not fixed** — `Write`/`Read` still cannot be targeted at a child's checkout. I
  routed around it by using the `Bash` observable rather than closing it.
- **No batch host entry point** (§8), and no new `ProtocolCommand`. I did not manufacture a wire
  change to make the orchestrator's RED prediction come true.
- **No contract regeneration, no PR, no merge to integration, no tag, no issue action.**
- **The shared fence was not touched.** `git diff $BASE HEAD -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is empty, with `BASE=5be91056` captured once at the start.
- Nothing was weakened: no `#[ignore]`, no `#[allow]`, no test deleted, no timeout raised. The
  45 s per-arm bound is unchanged and is now applied to two arms per surface exactly as before.

## 10. Routed to the phase

| # | Severity | Item |
|---|---|---|
| 21-C3E-01 | **HIGH (process)** | `CRITERIA-GAP-LEDGER.md`'s 21-C3 row prescribes a `wcore-protocol` schema change and a fenced Desktop seam. Both are wrong: the type is in `wcore-types` and no protocol command reaches it. The row's cost ("2–3 lane-sessions + one fenced protocol seam") should be corrected before anyone budgets against it. |
| 21-C3E-02 | MEDIUM | The corpus known-positive gate under-detected for the whole interval between the `f21-bwrap-overlap` fix and this lane, reporting NOT-EXPRESSIBLE on a dimension that was measurable. Repaired here. Generalises: **a gate written around a defect must be re-read when that defect is fixed** — nothing in the process prompts it. |
| 21-C3E-03 | MEDIUM | `21-REVERIFICATION.md:170-184`'s table is stale in 8 of 44 cells; it is quoted as current by the ledger and by lane briefs. |
| 21-C3E-04 | MEDIUM | Host-protocol in-process fan-out and egress remain NOT-EXPRESSIBLE; closing fan-out requires new host batch surface (§8). |
| 21-C3E-05 | — | Windows unmeasured for this criterion (§9), unchanged. |
