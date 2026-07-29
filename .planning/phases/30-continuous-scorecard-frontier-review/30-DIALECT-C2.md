---
lane: lane/30-dialect-c2
criterion: "Criterion 2 — frontier position vs Hermes Clawd and OpenClaw"
position-statable: false
legs-retaken: "2 run to protocol-v2 conformance (wayland/correctness, hermes/correctness) and then REFUSED for scoring; 0 published"
legs-still-confounded: "9 of 9 — but for TWO distinct causes, not one, and only 6 were ever the cause the record named"
verdict: "SR-30-3 completed and proven; SR-30-6 half-closed; position NOT statable, and now precisely characterised rather than merely unavailable"
new-finding: "HIGH — the dialect compiler had no consumer; HIGH — a second confound lives in a slot VALUE the compiler structurally cannot reach; MEDIUM — the cost dimension is degenerate by construction and protocol v2 re-registers it as live; MEDIUM — the harness binding trusts an unsigned manifest"
fence-exposure: "NONE — zero bytes changed in crates/wcore-cli/src/{lib,main}.rs vs merge-base 75babf32"
status: complete
---

# 30-DIALECT-C2 — making the parity question answerable

**Lane** `lane/30-dialect-c2`, branched from `75babf329235484684ecee3a65973b0c197840c1`.
**Build/live host** `hetzner-dsm`, worktree `/root/wayland-30c2`. **Graded** 2026-07-29.

| | |
|---|---|
| Execution seam (new) | `crates/wcore-eval-scenarios/src/dialect_exec.rs` |
| Runner wiring (new) | `wayland-scorecard trials run --translation --corpus --discovery-manifest` |
| Anti-pooling + diagnostic guards (new) | `frontier_trials.rs` — 3 new refusals |
| Peer provisioned | **Hermes Agent 0.17.0 @ `dbe734be`**, live, at its pin |
| Cohort gate | **`DIALECT_COHORT=ELIGIBLE members=2`** — first time it has ever returned ELIGIBLE |
| Tests | `--lib dialect` **39 passed, 0 failed, 0 ignored**; `--lib frontier_trials` **12 passed, 0 failed, 0 ignored** (counts read back) |
| Live arms | 9, **185 real trials**, recounted independently on the Mac from the pulled JSONL |
| Evidence | `evidence/30-dialect-c2/live/`, notes `.planning/30-DIALECT-C2-NOTES.md` |
| Provider spend | **$0.00** — every leg loopback; no credential read, written or transmitted |

---

## 0. The answer, in one paragraph

**The position still cannot be stated, and that is now a precise claim rather than a shrug.** The
dialect compiler works — proven, on the real binary, in the product's own words. Wiring it up (it
had never been connected to anything) and running it immediately exposed a **second, independent
confound that the compiler structurally cannot reach**: the canonical script's path argument is
relative, `wayland-core`'s `Write` requires an absolute path, and Hermes does not care either way.
So the registered protocol still measures a path convention rather than two products. With that
confound removed under a clearly-labelled diagnostic, **both harnesses complete the task on every
trial, on both dimensions** — which is materially different from the published `0/30` vs `30/30`,
and which is a diagnostic finding, not a number anyone may cite.

---

## 1. What was actually confounded — the inherited framing was wrong on a third of it

The record (`30-DIALECT.md §1`, `30-DIALECT-PROTOCOL-V2.md §0`, `MILESTONE-RC.md §6`) says: *all
nine RUN legs are confounded because the script spoke one competitor's dialect.* Read from the raw
per-trial records in `evidence/30-02/records/`, not from the summary:

| legs | dimension | what is actually wrong | fixed by dialect compilation? |
|---|---|---|---|
| 01, 02, 06, 07, 11, 12 | correctness, recovery | dialect **and** path convention — two stacked causes | **partially**: cause 1 yes, cause 2 no |
| 04, 09, 14 | cost | **degenerate by construction** — never a dialect problem at all | **no, and v2 does not say so** |

### 1a. The cost legs were never dialect-confounded (MEDIUM)

`dimension_specs.cost.observable` is defined as the sum over *every usage frame **the fixture**
emitted*, explicitly including failed trials. The fixture emits a fixed script. Measured: every
harness reports **2 requests / 20 units** on correctness-shaped runs and **3 / 30** on
recovery-shaped runs, with zero variance — and **Wayland's FAILURE trials report the identical
numbers as Hermes' SUCCESS trials.** The metric cannot distinguish a harness that did the work from
one that did nothing.

A harness can only register a different cost by deviating from the script, and deviation is scored
separately as an `unexpected_request` violation. **So the dimension has resolution only in the
region the protocol treats as invalid.**

Protocol v2 changes the tool *name* in the script, not the number of steps, so a perfect v2 re-take
returns `PRACTICALLY_INDISTINGUISHABLE` for exactly the same non-reason. **v2's §3 "What v2 does NOT
fix" lists security, FIFO, the token tables and cognitive tax — and omits cost**, while §2 inherits
"the cost non-inferiority guard" and the 15 trials by name. A v2 execution run exactly as registered
would publish a meaningless cost comparative under a pre-registration that does not warn the reader.

### 1b. Wayland's published `0/30` on recovery was never a recovery failure

`dimension_specs.recovery` is **conjunctive**: the fault must have been served *and* the workspace
must reach the oracle state. The recovery script is `[503, tool_call, text]` against a FIFO-cursored
meter, so **request count is a direct readout of how far down the script a harness walked**. All
three harnesses made exactly **3 requests / 30 units**: every one of them took the 503, retried,
consumed the tool call and came back for the final turn.

Wayland's recovery machinery worked on all 30 trials. The leg scored zero purely on the artifact
half. **Predicted at T+1 and then tested, not merely re-read** — see §4.

---

## 2. What was built, and why it had to be

### 2a. The compiler had no consumer (HIGH)

Measured on `bin/wayland-scorecard.rs` at `75babf32`, with known-positives alive in the same
invocations:

- `TranslationV1` appears in the CLI exactly twice — the import, and `DialectCommand::Verify`.
- `CompiledStepV1` appears twice — the import, and a **display string** of tool names in `Compile`.
- `TrialsCommand::Run` took `--protocol --invocation --dimension --trials --workspace-root --out`.
  **No translation argument.**
- `drive_leg` obtained its script from `steps_for(protocol, dimension)`, which reads
  `protocol["fixture_script"][dimension]` and deserializes straight to `Vec<OpenAiStep>`.

**There was no code path from a compiled translation to an executed trial.** A translation was
written by `dialect compile` and read back only by `dialect verify`, which re-digests it.

This matters more than the missing peers. Protocol v2 §5 lists four execution preconditions;
**all four could be satisfied and a v2 re-take would still replay v1's `write_file` script
verbatim**, because the only `fixture_script` on disk is v1's. The prior lane's precondition list is
incomplete in the same shape as its limitations list is incomplete on cost.

### 2b. `dialect_exec` — and why identity belongs at execution, not compilation

`dialect.rs` is identity-blind **by type** (G2): `ToolSchemaCorpusV1` has no field naming its
product. That is load-bearing and is not weakened.

Execution has the opposite requirement. Driving harness A with a dialect compiled for harness B *is*
the original F30-03 defect in a new costume, and it is **invisible to digest verification** — such a
translation verifies perfectly against the corpus it came from. So `bind_translation` performs five
checks, of which the load-bearing one is new:

| # | check | catches |
|---|---|---|
| 1 | translation's dimension == the dimension being run | a correctness translation driving a recovery leg |
| 2 | the dimension resolves to a canonical script | a typo'd dimension resolving to nothing |
| 3 | `TranslationV1::verify` (G4) | a translation hand-tuned after compilation |
| 4 | manifest's `corpus_sha256` == the corpus's digest | a manifest paired with a corpus it does not describe |
| 5 | manifest's `tool_label` == the harness being launched | **another harness's dialect** |

Checks 3 and 5 are independent and neither subsumes the other. **The design is: the compiler must
not see identity; the executor must.**

### 2c. Three new refusals that make a bad number unrepresentable

- **`MixedDialectProvenance`** — a leg mixing frozen-script trials with dialect-compiled ones is
  refused. Averaging a confounded arm with an unconfounded one is how the confound comes back.
  Enforced by an `Option<TrialDialectV1>` that is `None` on exactly the v1 records.
- **`MixedTranslationDigests`** — every trial in a leg must be driven by the identical translation,
  or the proportion measures which mapping each trial drew.
- **`DiagnosticTrialsCannotBeScored`** — a trial run under a deliberate instrument deviation is
  stamped, and one stamped trial disqualifies the whole leg. **A diagnostic is therefore structurally
  incapable of becoming a published number, even when its result is flattering** — which is the only
  case anybody would be tempted to fold it in. The test asserts the same trials score 1.0 unstamped,
  so the refusal is caused by the stamp and not by a guard that refuses everything.

---

## 3. Proving the compiler is not itself the confound

### 3a. The 2×2 — both factors necessary, neither sufficient

Same protocol file (`sha256 d18407e0b9…`, byte-identical to the frozen v1 document), same oracle,
same binary, same invocation. **The only thing that varies is the script.**

| | frozen v1 script (`write_file`) | compiled dialect (`Write`) |
|---|---|---|
| **relative path** (as registered) | ARM-A **0/30** | ARM-B **0/30** |
| absolutized path (diagnostic) | ARM-D **0/10** | ARM-C **30/30** |

This is a controlled isolation, not an inference from co-occurrence — the failure mode this program
keeps getting caught by. **ARM-C is the proof the lane brief asked for**: a compiled dialect driven
end-to-end against a real harness produces the oracle artifact at the correct content digest on 30
of 30 trials. The compiler makes the harness do the real work; it does not merely fail more politely.

### 3b. Negative control 1 — static, and refused for the RIGHT reason

A peer-flavour translation with an honest `hermes` manifest, launched against the `wayland`
invocation:

```
wayland-scorecard: DIALECT_EXEC_HARNESS_MISMATCH corpus_declared_by=hermes launching=wayland
                   — refusing to drive a harness with another harness's dialect        rc=1
```

**Instrument repaired in-lane, per LANE-BRIEF §6b-ii.** The first attempt at this control scraped
the corpus digest out of the compiler's stdout with `sed` and captured a leading newline, so the run
refused with `CORPUS_MANIFEST_MISMATCH` — a refusal for the **wrong reason**, which would have
passed as a green negative control while proving nothing about the harness binding. Repaired at
source (the generator now reads `corpus_sha256` from the translation JSON and asserts
`len==64 && sha.strip()==sha`), then re-run. **The refusal is asserted by error identity, not by
exit status.**

### 3c. Negative control 2 — dynamic, and it reddens

A guard refusing is not enough; the brief asks for a mis-compiled dialect whose *measurement* goes
red. The manifest was **forged** to claim `tool_label: "wayland"` while carrying the peer's corpus,
so the binder accepts and Wayland is actually driven with `write_file`:

```
NC-B  script=dialect_compiled+DIAGNOSTIC_ABSOLUTIZED  driven_tools=write_file  trials=10 success=0
```

**0/10 at the exact configuration where the correct dialect scores 30/30.** Same binary, same
protocol, same oracle, same path handling — the compiled tool name is the only difference.

**And it was not a synthetic stand-in.** Real Hermes' compiled correctness translation digests to
`dc33a24446f4888a4713727ccec88aa913a160eacad50ae18d679d786d02825a` — byte-identical to the one NC-B
used. The negative control drove Wayland with **the actual Hermes dialect**.

---

## 4. The second confound, and the symmetry test

### 4a. FINDING C (HIGH) — it is a slot VALUE, and the compiler cannot reach it

ARM-B is 0/30 *with* the correct dialect. From Wayland's own session transcript:

| arm | tool called | `tool_result` |
|---|---|---|
| A (frozen v1) | `write_file {path, content}` | `Unknown tool: write_file` |
| B (compiled) | `Write {file_path, content}` | `Refused to write TRIAL-ARTIFACT.txt: path must be absolute` |

**The failure moved from "I do not have that tool" to "I have that tool and I decline this
argument".** That is the dialect confound being removed, in the product's own words, and it is not
the only confound.

The compiler translates tool **names** and parameter **names**. It does not translate parameter
**values**, and the second confound lives entirely in the value. It cannot be fixed by extending
the compiler: selection reads the tokenized tool name and the declared JSON Schema, and *"this
parameter must be absolute"* is in **neither** — it is in the tool's prose description and its
runtime behaviour, and the compiler deliberately never reads descriptions (that exclusion is what
makes `qual_a_generic_tool_with_semantics_in_its_description_is_refused` a published blind spot).

**Compiling names does not compile conventions.**

Nobody could have found this earlier, and that is the mechanism rather than a criticism: with no
wiring, no compiled translation had ever been executed. `30-DIALECT.md` says *"Live, against the
real corpus: `Write` selected"* — true, and **selection is not execution.** The second confound was
masked by the first.

### 4b. The symmetry test — the remedy is not a tune

The obvious attack: *"you absolutized the path because it makes your product pass."* So it was run
on the peer.

| harness | compiled dialect, **relative** path | compiled dialect, **absolutized** path |
|---|---|---|
| Hermes 0.17.0 | **30/30** | **30/30** |
| Wayland Core | **0/30** | **30/30** |

**Absolutizing is neutral for the peer and decisive for us.** A change that flattered Wayland
selectively would have to *hurt* the peer; this one does not touch it. The uncomfortable corollary
is the honest half: **v1's relative path was itself the biased choice**, silently requiring a path
convention that exactly one harness honours and never stating it.

### 4c. Finding A confirmed by prediction

§1b predicted from v1's request counts that Wayland's recovery worked. Tested under compiled
dialects with the path confound removed: **wayland recovery 15/15, hermes recovery 15/15.**

---

## 5. The position — stated as precisely as the evidence allows

### 5a. What CAN now be computed, and why it is still not published

With Hermes provisioned and the cohort ELIGIBLE, a protocol-v2-conformant correctness comparative is
computable for the first time: **Wayland 0/30, Hermes 30/30 → `PEER_AHEAD`.**

**I am not publishing that as the frontier position, and the reason is not that it is unflattering.**
It is confounded by Finding C, which was established from Wayland's own transcript **before Hermes
was provisioned at all**, and whose remedy was then shown neutral for the peer by a falsifiable test
that could have gone the other way. Had Hermes failed under absolutization, the remedy would have
been exposed as a tune and that would be in this document instead. A wrong number that favours the
competitor is still a wrong number.

### 5b. The best current estimate of the truth, clearly labelled

Under the diagnostic instrument, with both confounds removed and applied identically to both
harnesses:

| dimension | Wayland Core | Hermes 0.17.0 |
|---|---|---|
| correctness | 30/30 | 30/30 |
| recovery | 15/15 | 15/15 |

**This is a diagnostic, not a scored comparative, and the code refuses to fold it into one.** Within
the `SCRIPTED_HARNESS` scope it supports one narrow statement: *on a scripted loopback harness with
the model held constant, once the script stops embedding one harness's tool names and one harness's
path convention, Wayland Core and Hermes Agent 0.17.0 both complete the scripted write task and both
recover from an injected 503, on every trial run.* It says nothing about model quality, real-world
task success, dollar cost, or OpenClaw.

**That is not parity and must not be reported as parity.** It is one dimension pair, two harnesses,
one scripted tier, n=30 and n=15.

### 5c. Legs re-taken: 0 published. Legs still confounded: 9 of 9.

| legs | status after this lane | what it needs |
|---|---|---|
| correctness ×3, recovery ×3 | **dialect confound REMOVED and proven; path confound OPEN** | protocol **v3**: one canonical slot-value change, already shown peer-neutral |
| cost ×3 | **degenerate by construction** — v2 does not repair it and does not admit so | a cost observable that can vary between conforming harnesses, or honest reclassification to `NOT_MEASURED` |
| security ×3 | UNPROVEN, unchanged | SR-30-1 (meter must retain bodies) |
| cognitive_tax ×3 | `NOT_MEASURED`, unchanged and correct | out of tier |

---

## 6. Honest grading

- **SR-30-3: COMPLETED by this lane.** The prior lane built the compiler and graded it DELIVERED on
  unit tests and a panel. It was not executable. It is now, and its first real run produced a
  finding four rounds of unit tests and a 4/4 expert panel did not.
- **SR-30-6: HALF-CLOSED.** Hermes provisioned at its pin and discovered live. OpenClaw not
  attempted — its 392 MB bundle plus 243 s build is a unit of work on its own, and with Finding C
  open it would not have changed any verdict here.
- **Criterion 2: STILL NOT MET.** The published `0/30` stands as published. No replacement number
  exists and none may be cited from this document.
- **What genuinely changed.** The blocker moved from *"the instrument does not exist"* (before the
  prior lane) through *"the instrument exists but has never been connected"* (its true state at
  base) to **"the instrument works, the cohort is real, and one named slot value stands between here
  and a valid comparative."** That last one is a day's work with a pre-registration, not a
  multi-month goal.

---

## 7. Seam requests

### SR-30-7 — protocol v3: the canonical script must not embed a path convention
**What is needed.** Re-register the canonical script with a workspace-absolute `Path` slot, applied
identically to every harness. Proven semantics-preserving for the oracle (which reads
`workspace.join(target_path)`) and proven peer-neutral (§4b). **Must be a new pre-registration**;
changing the slot value under cover of v2 would be the forbidden amendment-after-measurement.
**What breaks if omitted.** Every correctness and recovery comparative stays confounded, and a v2
run executed as registered will publish `PEER_AHEAD` for a path-convention reason.

### SR-30-8 — the cost dimension must be repaired or reclassified
**What is needed.** Either an observable that can vary between script-conforming harnesses, or an
honest reclassification of cost to `NOT_MEASURED` alongside cognitive tax. **What breaks if
omitted.** v2 publishes a comparative whose zero variance is a property of the fixture, presented as
a property of the products.

### SR-30-9 — bind the discovery manifest to the pass that produced it
**What is needed.** A signature or a digest chain from the observed wire bytes. Today
`bind_translation` check 5 trusts an unsigned manifest — this lane's own dynamic negative control
depends on forging one. **It prevents an accident, not an adversary.** MEDIUM.

### SR-30-10 (carried, not mine) — promote `wcore-egress` to a normal dependency
`src/judge.rs:138` makes a real outbound HTTPS call under
`#[allow(clippy::disallowed_methods)]`, justified in `Cargo.toml:86-89` by avoiding an internal-crate
edge. That is a live egress-boundary bypass in non-test code. Promoting `wcore-egress` from
dev-dependency to dependency removes both remaining allows in the crate with no cycle risk.
**Flagged, deliberately not done** — a crate-graph change did not belong in the commit that unblocks CI.

---

## 8. Fences, and what this lane did NOT do

- **Fence exposure: NONE.** `git diff <merge-base 75babf32> -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is **empty**. All 1,011 changed lines are inside
  `crates/wcore-eval-scenarios/`.
- Did **not** merge, open a PR, tag, publish, close an issue, or run `wcore-contract generate`.
- Did **not** touch `.github/workflows/*`.
- Did **not** touch `src/fixtures/openai.rs` — the shared meter and hard scope fence — so every
  30-02 number keeps meaning what it meant.
- Did **not** amend `evidence/30-02/protocol.json` (verified on the host: still
  `d18407e0b96bf753f66adc1eab7d21cbaeca1b9e627cecf0159095938b83ef25`), re-score any v1 leg, or
  withdraw any published number.
- Did **not** provision OpenClaw, weaken or ignore any test, or use any credential.
