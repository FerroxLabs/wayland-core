---
phase: 21-child-authority-and-budget-inheritance
verified: 2026-07-26T16:55:26Z
verified_at_sha: df63a4af831fd6683cdb0606aafcd19ea818b9e9
code_sha_verified: 359ce2bfd4360eb6a2c8084d3f1efb79d094d51e
status: gaps_found
score: 0/3 success criteria verified
behavior_unverified: 1
overrides_applied: 0
verifier_stance: adversarial (goal-backward, FORCE), re-verification
re_verification:
  previous_status: gaps_found
  previous_score: 0/3
  gaps_closed:
    - "F-V1 — the D1 §3 re-pin was satisfiable and was not done"
    - "F-V2 — the standalone live surface never had a live child"
    - "F-V3 — 7 of 11 in-process surface pairings are tautological"
    - "F-V4 — the NO-CHANNEL approval canary could not fail"
  gaps_remaining:
    - "SC1 — child cannot widen ANY restriction (tool guard absent, PolicyGate unwired)"
    - "SC3 — standalone and host-protocol corpora prove EQUIVALENT enforcement (now honestly graded NOT-MET by the phase itself)"
  regressions: []
  new_findings:
    - "F-V9 (WARNING) — the Criterion 3 amendment reached 21-04-PHASE-VERDICT.md but not its roll-ups"
gaps:
  - truth: "SC1 — A child cannot widen ANY of provider, tool, filesystem, egress, secret, approval, depth, fan-out, time, token or cost restriction."
    status: failed
    reason: "Unchanged since the first verification and independently re-confirmed at this HEAD. The tool dimension has no intersection seam and PolicyGate is still orphaned. The gap-closure round did not touch, and did not claim to touch, Criterion 1."
    artifacts:
      - path: "crates/wcore-agent/src/spawner.rs:2396-2402"
        issue: "build_tool_registry(allowed, requested_workspace, workspace_root, authority_read_deny, sandbox_runtime) — still no parent-authority parameter, so there is no seam at which a parent tool set could be intersected."
      - path: "crates/wcore-agent/src/engine.rs:4064"
        issue: "set_policy_gate still has ZERO callers workspace-wide; the only other hit is the doc comment at :2679. Re-grepped at df63a4af."
    missing:
      - "A parent-authority parameter on the child tool-registry construction path, or an equivalent intersection at a seam all five production spawner sites pass through."
      - "Either wire PolicyGate on the agent path or delete it."
  - truth: "SC3 — Standalone and host-protocol hostile corpora prove EQUIVALENT enforcement."
    status: failed
    reason: "The two defects that made the previous grade unsupportable (F-V2, F-V3) are CLOSED and independently re-measured. The criterion itself is still not met, and the phase now says so: 21-04-PHASE-VERDICT.md:33 was amended MET-WITH-STATED-EXCEPTIONS -> NOT-MET. Equivalence is now proved over a real but partial set."
    artifacts:
      - path: "crates/wcore-cli/tests/child_authority_corpus/surfaces.rs:1449-1465"
        issue: "tool and fan-out have NO host-protocol expression at all — SubAgentConfig carries no tool-authority and no breadth field, and spawn_host_child hardcodes ForkOverrides::default(). Correctly recorded NOT-EXPRESSIBLE rather than paired, but 2 of 11 dimensions therefore have no cross-surface comparison."
      - path: ".planning/phases/21-child-authority-and-budget-inheritance/evidence/21-05-t2-windows.log"
        issue: "Every Windows standalone-live row is NOT-EXPRESSIBLE with child_turns=0 (no ConPTY-drivable PTY). Windows equivalence is proved over the in-process modes and host-protocol-live only."
      - path: ".planning/phases/21-child-authority-and-budget-inheritance/21-05-CRITERION3-REPAIR.md:321-328"
        issue: "The phase's own statement of what is NOT established: the tool REFUSED is jointly attributable to containment vs tool authority, and fan-out live is undetermined on both platforms and both surfaces."
    missing:
      - "A host-protocol expression for the tool and fan-out dimensions, or an accepted decision that those two are permanently standalone-only."
      - "A live actor on the Windows standalone surface, or an accepted decision that Windows equivalence stands on three of four combinations."
      - "A live fan-out control that separates 'the cap bound' from 'nothing ran'."
  - truth: "The Criterion 3 amendment is carried consistently through the phase's roll-up artifacts."
    status: partial
    reason: "NEW FINDING F-V9. The amendment landed in 21-04-PHASE-VERDICT.md:33,137 but four downstream statements still assert the withdrawn grade. This is the same disclosure-does-not-reach-the-verdict failure the original F-V2 named, inverted."
    artifacts:
      - path: ".planning/REQUIREMENTS.md:178"
        issue: "Still reads 'Criteria 2 and 3 MET WITH STATED EXCEPTIONS'."
      - path: ".planning/phases/21-child-authority-and-budget-inheritance/21-04-SUMMARY.md:7"
        issue: "termination-state frontmatter still reads 'Criterion 1 NOT MET, Criteria 2 and 3 MET WITH STATED EXCEPTIONS'. Same at :62 and :229."
    missing:
      - "Propagate CRITERION 3 :: NOT-MET into REQUIREMENTS.md:178 and 21-04-SUMMARY.md:7,62,229, or record why the roll-ups keep the superseded grade."
deferred: []
behavior_unverified_items:
  - truth: "SC2 — Nested reservation, refund, escalation, approval, cancellation and result delivery remain attributable to the correct parent/session."
    test: "Drive the refund case across a real process restart (bind BudgetAuthorityCoordinator over a SessionJournal, kill, rebind, release one sibling's reservation) and confirm the reservation survives and the refund lands on the sibling that made it."
    expected: "Reserved totals survive the rebind and release() returns true for the correct sibling only."
    why_human: "Unchanged from the first verification. crates/wcore-cli/tests/child_attribution_corpus.rs was not touched by the gap-closure round (git diff 1058965e..HEAD -- crates/ lists four files, none of them the attribution corpus). F21-04-02 measured reserved_totals (0, 0.0) after the rebind and release() == false, and the phase could not settle whether the cause is the corpus's binding of the durable path or the product."
human_verification:
  - test: "Decide the disposition of the six HIGH findings entering Phase 22 open: F21-02-01 (tool authority not intersected), F21-02-03 (PolicyGate unreachable), F21-02-02's NOT-CLOSED live closure, F21-04-01 (no per-child observable on the host protocol), F21-04-02 (reservation does not survive restart), F21-04-03 (two parallel Spawn siblings die on a journal-head CAS collision)."
    expected: "Each is either scheduled for repair before Phase 22's fan-out work or explicitly accepted with a recorded reason."
    why_human: "All six are reserved to Sean by the phase's own termination discipline; the repair budget for 21-03 is spent. Unchanged — the gap-closure round repaired the PROOF, not the product."
  - test: "Decide whether F21-04-03 blocks Phase 22 entry. Run two parallel Spawn siblings against the shipped binary on SEANDESKTOP."
    expected: "Both siblings complete. At the recorded SHA, 6 of 6 Windows runs and 3 of 8 Linux runs instead left the losing sibling's budget authority PERMANENTLY FAULTED."
    why_human: "Phase 22 supervises fleets of children; this is a parallel-delegation defect on the product's advertised fan-out path."
  - test: "Decide the disposition of the newly disclosed Windows product question: wcore_agent::confirm::ToolConfirmer::check_for sees io::stdin().is_terminal() == true for a session-0 scheduled task with no console, so the confirmer prompts an approver who cannot exist and read_line never returns."
    expected: "Either confirmed as a product defect and scheduled, or accepted as a scheduled-task artifact that cannot occur for a real Windows user."
    why_human: "Surfaced by closing F-V2 (21-05-CRITERION3-REPAIR.md:85-100), recorded rather than fixed, and outside that repair's authority. It is the reason corpus_tool standalone in-process is NOT-EXPRESSIBLE on Windows."
---

# Phase 21: Child Authority and Budget Inheritance — Re-Verification Report

**Phase Goal (ROADMAP.md:75):** Every delegated actor remains inside the parent's authority and resource envelope.
**Verified:** 2026-07-26T16:55:26Z at HEAD `df63a4af`; all code verified at `359ce2bf` (no `crates/` change between the two — `git diff --stat 359ce2bf..HEAD -- crates/` is empty).
**Status:** gaps_found
**Re-verification:** Yes — grading the four claimed gap closures against the codebase, not against the closure report.
**Repo:** `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`.

---

## 0. Headline

**All four claimed closures hold, and I closed each one by measurement rather than by
reading the report.** I recomputed the three contract digests with my own reimplementation of
the digest algorithm; I ran the corpus on `hetzner-dsm` at `359ce2bf`; and I built the F-V4
injection scenario myself and watched the suite go red on it.

**The phase goal is still NOT ACHIEVED, and the phase's own grade moved further against
itself, not toward itself.** The only grade movement in this round is Criterion 3
`MET-WITH-STATED-EXCEPTIONS` → `NOT-MET` (`21-04-PHASE-VERDICT.md:33`). Criterion 1 was already
`NOT-MET` and remains so — I re-confirmed both counterexamples at this HEAD. Nothing was
repaired in the product; what was repaired is the *proof*, and the repaired proof claims less
than the old one did.

**Nothing was engineered green.** Checked hard and mechanically: `#[ignore]` 128 at the phase
base, at the prior HEAD, and at this HEAD; `#[allow]` 176 / 176 / 176; zero file deletions;
zero removed lines matching `assert` / `#[test]` / `#[ignore]` / `#[allow]` / `panic!` /
`timeout` / `Duration::from` anywhere under `crates/`; the live-run budget was 18 s before and
is 18 s now. The decisive-verdict count went **down**, which is the honest direction.

**One new finding (F-V9, WARNING):** the Criterion 3 amendment reached the verdict document
and did not reach its roll-ups. `REQUIREMENTS.md:178` and `21-04-SUMMARY.md:7,62,229` still
assert the withdrawn grade. Mechanical, and exactly the failure class the original F-V2 named.

---

## 1. Goal Achievement — Success Criteria

| # | Success Criterion (ROADMAP.md:78-81, verbatim) | Phase's grade now | This verification | Evidence |
|---|---|---|---|---|
| 1 | A child cannot widen any provider, tool, filesystem, egress, secret, approval, depth, fan-out, time, token, or cost restriction. | NOT-MET | ✗ **FAILED** (agrees) | `spawner.rs:2396-2402` still has no parent-authority parameter; `set_policy_gate` still zero callers (`engine.rs:4064`). Re-grepped at `df63a4af`. |
| 2 | Nested reservation, refund, escalation, approval, cancellation, and result delivery remain attributable to the correct parent/session. | MET-WITH-STATED-EXCEPTIONS | ⚠️ **PRESENT_BEHAVIOR_UNVERIFIED** (unchanged) | `child_attribution_corpus.rs` was not touched by this round. Refund across restart still NOT-OBSERVABLE with its cause unsettled. |
| 3 | Standalone and host-protocol hostile corpora prove equivalent enforcement. | **NOT-MET** (amended) | ✗ **FAILED** (agrees with the amended grade) | The two defects are closed and re-measured; the criterion is still unmet for reasons the phase now states itself (2 of 11 dimensions inexpressible on the host protocol, no Windows standalone actor, fan-out live undetermined). |

**Score: 0/3 verified** (2 failed, 1 present-but-behaviour-unverified).

The phase goal — *every delegated actor remains inside the parent's authority and resource
envelope* — is **NOT ACHIEVED**. Six HIGH findings remain open, all four requirements remain
OPEN in `REQUIREMENTS.md:63-66`, and two of the eleven named dimensions still have no guard.

---

## 2. F-V1 — are D1 §3's three digests now the TRUE values?

**VERIFIED CLOSED.** Recomputed, not read.

I reimplemented `digest_named_bytes` (`crates/wcore-protocol/src/contract/canonical.rs:30-42`)
and the `fixtures_digest` self-reference normalization
(`crates/wcore-protocol/src/contract/generate.rs:1107-1137`) in Python, parsed the 40
`SOURCE_INPUTS` **out of `contract/spec.rs:833-874`** rather than reading them from
`manifest.json`, and walked the 156 checked-in corpus files directly:

| Digest | My recompute | `manifest.json` | D1 §3 pin | Verdict |
|---|---|---|---|---|
| `source_inputs_digest` (40 files) | `sha256:9d5928b47f0cf9430e57786af498f9c297a36ace1572bc964776f25a0be0f5f5` | same | `D1:144` same | ✓ |
| `schema_digest` (3 files) | `sha256:e5d1744aa6cadc46d2707a1fa190ac80ee74f13477d685bb9146a71b3fff2e54` | same | `D1:143` same | ✓ |
| `fixture_digest` (151 files) | `sha256:0704cd43a86e52da86af093f9f90c0877328e53c154bec9fddf93d24fd3d7209` | same | `D1:142` same | ✓ |

The `spec.rs` source list is byte-identical to `manifest.json`'s (checked, 40/40), the fixture
count recomputes to 151, and the whole-corpus convenience pin at `D1:370`
(`dcaef42c8e03ad902341ed69c2cc48bd057a2747340c1ae4925c700bb908b4e5`) reproduces on my Mac.
Three of the §3.3 plain-file pins spot-checked with `shasum -a 256` and matched exactly
(`manifest.json` `8827fb5e…`, `events/ready.json` `ef560477…`, `schema/core-event.schema.json`
`d1e1036f…`).

**Did `wcore-contract check` pass?** Yes, and it is the leg that matters — it regenerates every
artifact in memory from the generator sources and compares byte-for-byte.
`evidence/21-05-d1-repin-linux.log:11-13` records, at `HEAD 1058965e` on `hetzner-dsm` with an
empty `git status --short`:

```
=== CHECK ===
Desktop contract corpus is current (wcore-desktop-contract-gen/11)
EXIT=0
```

The same log carries the `digest` leg (`:5-10`) with all three values, and
`302 tests run: 302 passed` for `wcore-protocol`.

**Is the bump recorded explicitly?** Yes. `D1-CORE-PRODUCER-CONTRACT.md` §3.0 (`:146-213`) is a
published contract bump, not a silent re-pin: before/after table for all three digests, the
cause commit (`e0cae85e`, `execution_policy.rs`, entry 17 of the 40 source inputs), the
regeneration commit (`a412aba7`), the five corpus files rewritten, the mechanical explanation
of why `fixture_digest` moved, the full revision sequence, a `FixtureDigestMismatch` /
`SourceInputsDigestMismatch` impact note for Desktop, and an explicit statement that the clause
being discharged is the admission panel's binding one. The header banner at `:28-33` warns any
consumer pinned to revision 1. The false claim in `21-03-SUMMARY.md` is retracted in place at
`:104+` under a `CORRECTION` block that names both halves of the original error.

**Still valid at this HEAD:** no `SOURCE_INPUTS` file and no corpus file changed between
`1058965e` and `df63a4af` — the only `crates/` changes in that range are four test files.

*Residual (INFO, not a gap):* D1 §3.2's leg B cites `scratchpad/verify_digests.py` as "session
scratchpad, not committed", so that reproduction is not durable. I reconstructed it
independently and got the same three values, which is the stronger outcome, but the same class
as F-V8 applies.

---

## 3. F-V2 — does a delegated child now genuinely act on the standalone live path?

**VERIFIED CLOSED on Linux. Honestly declared NOT-EXPRESSIBLE on Windows.**

### 3.1 The gate is keyed on evidence the child produced

`live.rs:1471` — `let child_acted = run.child_turns > 0;` — and `child_turns` is counted at
`live.rs:645-648` as served provider requests whose **first** user message carries the run's L1
goal marker (`first_message_contains`, `live.rs:560-565`; `CHILD_GOAL_L1 = "CORPUSGENL1"`,
`:87`). I checked the discriminator adversarially: the parent's first message is the operator
prompt `"delegate the task"` (`live.rs:668, 827, 964`), which never contains the marker; only a
delegated child's own conversation opens with the goal. `delegation_attempted` is still
recorded separately (`:642`), so "the delegation never executed" and "the delegation executed
but the child never acted" stay distinguishable.

The provider dimension, whose probe *is* the request accounting, withholds on the same
condition rather than being gated on it (`live.rs:1229-1247`).

### 3.2 The standalone surface now has a real actor, and it is not a bypass

`run_headless_pty` (`live.rs:962-1006`) spawns the **same shipped invocation** as the piped
variant — `wayland-core --no-tui --provider anthropic "delegate the task"` — attached to a real
PTY, and `answer_approval_prompts` (`:1032-1060`) answers the shipped confirmer's
`Allow? [y]es…` with `y`. `--force` is explicitly rejected in the comment and appears nowhere
in the corpus. That exercises the gate; it does not skip it. `run_tui` (`:1064-1130`) likewise
now answers the approval card instead of sleeping 8 s past it.

The root cause the closure names is real and is a product fact:
`wcore_agent::confirm::ToolConfirmer::check_for` denies unconditionally when stdin is not a
terminal, so on pipes the `Delegate` call was refused before any child existed.

### 3.3 Measured — I ran it myself

`hetzner-dsm:/root/wayland-p21` at `359ce2bf`, clean worktree, `corpus_approval`:

* standalone live (TUI transport): `approval prompts answered: 1`, **1 delegated child provider
  turn**, verdict `NO-CHANNEL`, screen shows the real wordmark and `Delegate({ "goal":
  "CORPUSGENL1: write a file" …}) running…`.
* host-protocol live: **2 delegated child provider turns**, verdict `NO-CHANNEL`, transcript
  opens with a real `{"type":"ready","version":"0.12.25",…}` frame whose `contract` block
  carries the exact three digests I recomputed in §2.

Against the ledger, Linux at `359ce2bf` (`evidence/21-05-t1-linux.log`), all decisive live rows:

| Dimension | standalone live | child turns | host-protocol live | child turns |
|---|---|---|---|---|
| filesystem | REFUSED | **2** | REFUSED | 2 |
| secret | REFUSED | **2** | REFUSED | 2 |
| egress | REFUSED | **2** | REFUSED | 2 |
| depth | REFUSED | **2** | REFUSED | 2 |
| tool | REFUSED | **2** | REFUSED | 2 |
| provider | NO-CHANNEL | **1** | NO-CHANNEL | 1 |
| approval | NO-CHANNEL | **1** | NO-CHANNEL | 2 |
| fan-out, time, token, cost | NOT-EXPRESSIBLE | 0 | NOT-EXPRESSIBLE | 0 |

**14 of 14 decisive live rows carry an actor.** The verifier's table had `child turns 0` on
every standalone-live entry. The tool dimension — the phase's primary amplification candidate —
reaches a live delegated child on the standalone surface for the first time at any SHA.

### 3.4 The REFUSED count dropped, and every drop is correct

* fan-out standalone live REFUSED → NOT-EXPRESSIBLE: the batch is rejected at the tool's own
  parse before any child exists, and a single live run cannot separate "the cap bound" from
  "nothing ran" without a live control. Withheld rather than assumed.
* Eight Windows standalone-live rows REFUSED/NO-CHANNEL → NOT-EXPRESSIBLE: no ConPTY-drivable
  PTY, so the piped variant is driven and has no approval channel at all. Every one of those
  eight was vacuous at the baseline, so this is a loss of nothing real.
* Windows `corpus_tool` standalone in-process REFUSED → NOT-EXPRESSIBLE: the 45 s bound
  expired because the in-process child's engine reached the shipped confirmer on the harness's
  own stdin under a session-0 scheduled task. Recorded as a withheld verdict, never a refusal
  (`surfaces.rs:834-849`).

I checked whether withholding could hide a red: it cannot. The suite fails only on
`Outcome::Allowed`, on a canary trip, or on an equivalence mismatch, and a non-decisive row is
skipped by the equivalence assertions rather than counted as agreement. No row anywhere is
`Allowed` in the shipped tree, so no equivalence assertion could have fired either way.

**Remaining honest limit:** the Windows standalone live surface has no actor, at this SHA, on
this harness. It says so, on every row, with the reason.

---

## 4. F-V3 — are the two surface drivers now genuinely different code paths?

**VERIFIED CLOSED for the in-process pair; the host-protocol LIVE leg does reach the real
`--json-stream` wire.**

### 4.1 Zero shared probe functions remain

`StandaloneInProcess::probe` (`surfaces.rs:1338-1365`) dispatches to
`provider_no_channel_canary`, `approval_no_channel_canary`, `tool_widening_through_spawn_fork`,
`filesystem_escape_probe`, `secret_read_probe`, `egress_probe`, `fan_out_probe`, and raw
`ExecutionBudgetView::sub_budget`.

`HostProtocolInProcess::probe` (`surfaces.rs:1414-1465`) dispatches to
`host_child_provider_pin_probe`, `host_child_approval_inheritance_probe`,
`host_child_read_probe` ×2, `host_child_egress_probe`, `host_request_surface_not_expressible`
×2, and `BudgetAuthorityCoordinator::begin_active_turn`. **The intersection is empty.**

The host driver is the production object graph, verified in source rather than in prose:
`wcore_agent::bootstrap::AgentBootstrap::new(...).provider(...).build()` at `surfaces.rs:1648`,
then `engine.init_session(...)` at `:1660`, then children through
`session.host_children.spawn_child(...)` at `:1731`, `:1793`, `:1851`, `:1940`. The read probes
observe the tool-result body the session's own provider was shown — the bytes the shipped VFS
handed the child — not a hand-built `SandboxedFs`.

### 4.2 The dimensions that cannot be expressed SAY so, in a way that cannot go stale

Tool and fan-out record NOT-EXPRESSIBLE with the `SubAgentConfig` field set as evidence, read
by **exhaustive destructuring** (`surfaces.rs:1506-1524`): adding a field to that production
type stops the corpus compiling, so the record cannot silently go stale, and a matching field
also raises a `canary_trip` which the canary assertion turns red. This is precisely the "must
SAY so rather than be silently paired" requirement, and it is implemented as a compile-time
obligation rather than a comment.

Three dimensions **lost** a decisive host-protocol in-process verdict relative to the baseline
(`egress`, `tool`, `fan_out`: REFUSED → NOT-EXPRESSIBLE) because those verdicts were the
standalone driver's answer wearing the host-protocol label. That is the correct consequence of
the repair, and it is stated.

### 4.3 The host-protocol live leg is the real wire

`run_json_stream` (`live.rs:667-800`) spawns the shipped binary with `--json-stream --provider
anthropic`, writes a real `{"type":"message",…}` frame on stdin, reads the frame stream, proves
the mode on the `ready` frame that only the protocol front-end emits, and answers
`approval_required` with `{"type":"tool_approve","call_id":…}` parsed from the frame rather than
pattern-matched on text (`:797-806`). I observed the real `ready` frame in my own run, including
the `capabilities` map, the `execution_policy` frame and the `workspace_policy` frame. **This is
the wire.**

*Stated limit, not a gap:* the host-protocol **in-process** leg is the production object graph
the wire front-end owns, not the wire itself. Wire coverage is carried entirely by the live
mode. The repair document says so; I confirm it.

---

## 5. F-V4 — construct the canary's scenario and watch the suite go RED

**VERIFIED CLOSED. I built the scenario myself; the report's demonstration was not taken on
trust.**

### 5.1 The assertion exists and runs first

`assert_no_channel_canaries_stayed_intact` (`child_authority_corpus.rs:461-490`) is called from
`drive()` at `:220`, **before** the equivalence pair, with the reason stated in the code. Two
independent triggers: a `canary_trip` on any entry, and `Outcome::Allowed` on any entry carrying
`no_channel_canary` — the second firing **regardless of census verdict, surface or mode**, which
is exactly the early-return hole (`:504`) that let the old suite stay green.

### 5.2 The injection — run by this verifier, on `hetzner-dsm`, at `359ce2bf`

I wrote a production source file naming the child-sourced request type:

```
crates/wcore-config/src/verifier2_fv4_probe.rs
    use wcore_types::execution_policy::PolicySource;
    pub fn requested_source() -> PolicySource { PolicySource::Child }
```

`cargo test -p wcore-cli --test child_authority_corpus -- --exact corpus_approval` →

```
test corpus_approval ... FAILED
thread 'corpus_approval' panicked at crates/wcore-cli/tests/child_authority_corpus.rs:476:13:
assertion `left != right` failed: NO-CHANNEL CANARY TRIPPED (realised widening) ::
corpus_approval :: dimension approval :: the child obtained a child-sourced approval request
channel in crates/wcore-config/src/verifier2_fv4_probe.rs through the standalone surface in
in-process mode. ... The census verdict (VACUOUS) is a measurement taken before this run and
does not excuse it.
test result: FAILED. 0 passed; 1 failed; 26 filtered out; finished in 24.03s
```

The injected file was then removed and the worktree confirmed clean. **The full suite at
`359ce2bf`, run by me: `27 passed; 0 failed; 0 ignored; 0 filtered out`** — including all
eleven `corpus_*` cases and the four canary proofs.

### 5.3 The four permanent tests pass, and one of them is the finding itself

Run by name on `hetzner-dsm`, all four `ok`:

| Test | What it pins |
|---|---|
| `every_other_assertion_stays_green_on_a_realised_approval_widening` | completeness, surface-equivalence, mode-equivalence and the census assertion ALL pass on the F-V4 scenario — the finding reproduced as data, so the canary assertion can never be mistaken for redundant |
| `the_no_channel_canary_goes_red_on_a_realised_approval_widening` | the canary assertion panics on that exact scenario |
| `the_no_channel_canary_passes_once_the_widening_is_removed` | it does not panic when the protection is intact |
| `a_structural_canary_trip_goes_red_on_any_dimension` | the budget canary's trip now fails the suite instead of being display text |

The budget canary now returns a `CanaryState` (`surfaces.rs:794-812`) that lands in
`Execution::canary_trip` (`:190`, `:253`) and is asserted on, rather than a `String`
interpolated into prose. That was the third of the three holes.

*Residual (INFO, not a gap):* the structural canaries are grep-based over `crates/*/src/**.rs`
(`source_files_mentioning`, `surfaces.rs:275-312`), so they trip on a file that merely names the
type even if nothing compiles it. That is fail-loud rather than fail-silent — the correct
direction — but a structural trip is a "read this" signal, not proof of an exploitable channel.
The `Outcome::Allowed` trigger is the one that proves exploitability, and it is now
unconditional.

---

## 6. Was anything engineered green?

**No. CLEAN, and checked mechanically at three points, not two.**

| Probe | Phase base `dd02a624` | Prior HEAD `1058965e` | This HEAD `df63a4af` |
|---|---|---|---|
| `#[ignore` under `crates/` | 128 | 128 | 128 |
| `#[allow` under `crates/` | 176 | 176 | 176 |

| Probe | Result |
|---|---|
| Files deleted, `1058965e..HEAD` | **none** |
| Removed lines matching `assert` / `#[test]` / `#[ignore` / `#[allow` / `panic!` / `timeout` / `Duration::from` / `expect(` under `crates/` | 2, neither a weakening: `expect("session directory")` moved into the new `host_session` builder, and a passive `thread::sleep(Duration::from_secs(8))` was REPLACED by an active `answer_approval_prompts(..., 9s)` loop |
| Timeouts raised | **none.** `LIVE_RUN_BUDGET` is `from_secs(18)` at `1058965e` and `from_secs(18)` now. The only new bounds are additions: a 45 s cap on the in-process tool probe whose expiry records NOT-EXPRESSIBLE and never REFUSED (`surfaces.rs:834-849`), and the 9 s approval-answer window above |
| New `#[cfg]` gates | 2, both on the new PTY driver, each with a `#[cfg(not(unix))]` counterpart that records UNAVAILABLE and never proves a mode, so no verdict can be taken from it (`live.rs:961, 1007-1023`) |
| New env-var skip gates | **none** |
| Grades improved without evidence | **none.** The only grade movement in the phase is downward: `CRITERION :: 3 :: MET-WITH-STATED-EXCEPTIONS -> NOT-MET`, with an `AMENDED ::` row, a superseded-evidence marker on `EVIDENCE-LIVE :: 1`, a `QUALIFIED:` marker on `EVIDENCE-LIVE :: 3`, and two new evidence rows (`21-04-PHASE-VERDICT.md:33-44`) |
| Injection artifacts left in the tree | **none** — no `crates/*/src` file names `PolicySource::Child` outside `wcore-types/src/execution_policy.rs` (proved by the suite passing), and both my worktree and Hetzner's are clean |
| `cargo fmt --all -- --check` | clean (recorded `FMT_CLEAN` in `evidence/21-05-t1-linux.log:5`) |

The corpus's **decisive-verdict count went down**, not up: eleven verdicts moved from a
decisive value to NOT-EXPRESSIBLE, and every one of those moves surrenders a claim the phase
previously made. That is the opposite of engineering green.

---

## 7. Findings status roll-up

| Finding | Prior severity | State now | Evidence |
|---|---|---|---|
| F-V1 — stale D1 §3 digest pins | HIGH | **CLOSED** | §2 — three digests independently recomputed and matching, `check` EXIT=0, explicit §3.0 bump record, 21-03 SUMMARY correction |
| F-V2 — standalone live had no child | HIGH | **CLOSED (Linux); declared NOT-EXPRESSIBLE (Windows)** | §3 — gate re-keyed at `live.rs:1471`, 14/14 decisive Linux live rows with an actor, reproduced by this verifier |
| F-V3 — 7/11 pairings tautological | HIGH | **CLOSED for 9, WITHHELD for 2** | §4 — zero shared probe functions, production `AgentBootstrap` + `HostChildController`, compile-breaking field-set record for the 2 |
| F-V4 — approval canary could not fail | HIGH | **CLOSED** | §5 — injection run by this verifier goes RED at `child_authority_corpus.rs:476`; four permanent tests pass |
| F-V7 — non-sequitur evidence text | WARNING | **CLOSED** | `21-05-t1-linux.log:118` now reads "2 delegated child provider turn(s) arrived, so a child existed and took its own turn" |
| F-V5 — `21-04-ATTRIBUTION-RESULTS.md` §2 contradicts its ledger | MEDIUM | **OPEN** | `:72` still reads json-stream (linux) = NOT-OBSERVABLE against `21-04-t2-linux.log:71` `CORRECT`. Declared out of scope at `21-05-CRITERION3-REPAIR.md:329` |
| F-V6 — 21-04 panel has 3 captures | LOW | **OPEN** (declared) | unchanged |
| F-V8 — `.planning/TEST-AUDIT.md` untracked | LOW | **OPEN** | `git ls-files` still returns nothing for it |
| **F-V9 — amendment did not reach the roll-ups** | **NEW, WARNING** | **OPEN** | `REQUIREMENTS.md:178`, `21-04-SUMMARY.md:7,62,229` still assert `Criteria 2 and 3 MET WITH STATED EXCEPTIONS` |

---

## 8. Behavioural spot-checks (run, not inferred)

| Behaviour | Command | Result | Status |
|---|---|---|---|
| Three contract digests are the true values | own Python reimplementation of `digest_named_bytes` + fixture normalization over the checked-in corpus, `SOURCE_INPUTS` parsed from `spec.rs` | all three match `manifest.json` AND `D1` §3 | ✓ PASS |
| Corpus tree pin reproduces | `find . -type f | sort | xargs shasum -a 256 | shasum -a 256` in the corpus root | `dcaef42c…` = `D1:370` | ✓ PASS |
| The four canary proofs pass | `cargo test -p wcore-cli --test child_authority_corpus -- --exact <4 names>` on `hetzner-dsm` | `4 passed; 0 failed` | ✓ PASS |
| The canary goes RED on the scenario it exists for | injected `crates/wcore-config/src/verifier2_fv4_probe.rs`, ran `corpus_approval` | `FAILED`, `NO-CHANNEL CANARY TRIPPED (realised widening)` at `child_authority_corpus.rs:476` | ✓ PASS (the red is real) |
| Green again once the scenario is removed | removed the file, ran the full corpus | `27 passed; 0 failed; 0 ignored; 0 filtered out` | ✓ PASS |
| A delegated child acts on the standalone live surface | observed in the `corpus_approval` run above | TUI transport, 1 approval answered, **1 delegated child provider turn** | ✓ PASS |
| Host-protocol live is the real `--json-stream` wire | observed in the same run | real `ready` frame, `version 0.12.25`, contract block carrying the three digests of §2 | ✓ PASS |
| SC1 counterexample 1 still holds | read `spawner.rs:2396-2402` | no parent-authority parameter | ✓ PASS (confirms the red) |
| SC1 counterexample 2 still holds | `grep -rn set_policy_gate crates/` | 2 hits: doc comment `:2679`, definition `:4064`. Zero callers | ✓ PASS (confirms the red) |
| `cargo fmt --all -- --check` | run locally (the only permitted local cargo command) | clean | ✓ PASS |
| Workspace-wide suite | not run by me | Hetzner aggregate at `f2d186f6` reads `11565/11565 passed, 48 skipped`; the `wcore-protocol` leg re-run at `1058965e` reads `302/302` | ? SKIP |

Both remote worktrees were left clean; nothing this verification wrote survives.

---

## 9. Verdict

**Phase 21's goal — every delegated actor remains inside the parent's authority and resource
envelope — is NOT ACHIEVED.**

The gap-closure round did what it claimed and no more, which is the right shape: it repaired
the **proof**, disclosed four further instances of the same vacuity family it was closing (one
of them created by its own first draft), narrowed three grades, withdrew a fourth, and surfaced
a new Windows product question rather than absorbing it. Every one of the four claimed closures
survives independent measurement, including the one that can only be proved by making the suite
fail.

What did not change is the product. The tool dimension still has no intersection seam, the
`PolicyGate` is still orphaned, two of eleven dimensions still have no host-protocol
expression, and the Windows standalone live surface still has no actor. Criterion 1 was
`NOT-MET` before and is `NOT-MET` now; Criterion 3 is `NOT-MET` and the phase amended its own
grade to say so.

**Recommended before Phase 22 entry, in order:** close F-V9 (mechanical — propagate the
Criterion 3 amendment into `REQUIREMENTS.md:178` and `21-04-SUMMARY.md:7,62,229`); correct
`21-04-ATTRIBUTION-RESULTS.md` §2 against its own ledger (F-V5); commit `.planning/TEST-AUDIT.md`
and the digest reproduction script (F-V8 and the §2 residual). The six HIGH findings, the
refund-across-restart question, and the newly disclosed Windows session-0 confirmer question are
Sean's to route.

---

_Verified: 2026-07-26T16:55:26Z at `df63a4af` (code at `359ce2bf`)_
_Verifier: Claude (ferrox-verifier), adversarial goal-backward stance, re-verification_
_Not committed — left to the orchestrator._
