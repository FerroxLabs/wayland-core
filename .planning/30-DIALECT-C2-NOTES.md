# 30-DIALECT-C2 — running notes (append-only, committed continuously)

**Lane** `lane/30-dialect-c2`. **Base** `75babf329235484684ecee3a65973b0c197840c1`.
**Worktree** `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-30-dialect-c2`
(verified via `/usr/bin/git rev-parse --show-toplevel`; NOT the dirty `dev/waylandcore` checkout).

Per LANE-BRIEF §6b-i: this file is committed inside the first 15 minutes and re-committed after
every measurement. There is no partial credit for uncommitted reasoning.

---

## T+0 — orientation, established by reading, not yet by measuring

### What already exists at base (verified by `ls`, not by document claim)

| Artifact | Path | State |
|---|---|---|
| Dialect compiler | `crates/wcore-eval-scenarios/src/dialect.rs` | present, 63074 bytes |
| Discovery meter | `crates/wcore-eval-scenarios/src/dialect_discovery.rs` | present, 19346 bytes |
| Protocol v2 pre-registration | `.planning/phases/30-*/30-DIALECT-PROTOCOL-V2.md` | present |
| Prior lane report | `.planning/phases/30-*/30-DIALECT.md` | present |
| Evidence dirs | `.planning/phases/30-*/evidence/30-{01,02,03,04,dialect}` | present |

**So SR-30-3 is NOT greenfield for this lane.** A prior lane (`lane/30-dialect`, branched
@ `8bcb052b`) already built the compiler, registered protocol v2, and ran a 4/4 panel. My task
brief describes building it; the tree says it is built. **I must not re-build it and must not
assume the prior lane's framing is correct** — the brief explicitly warns that the last lane which
inherited a framing propagated an error into two planning documents.

### The claim I have to independently verify FIRST

`30-DIALECT.md §1` and `30-DIALECT-PROTOCOL-V2.md §0` both assert:

> the frozen F30-03 script emits a tool call named `write_file`; Hermes 30/30, Wayland Core 0/30,
> OpenClaw 0/30 on correctness and recovery; therefore all nine RUN legs are confounded.

**This is inherited framing. Not yet verified by me.** Open questions I must answer from the
30-02 evidence directory and the frozen protocol, not from the summary:

1. Is the tool name in the *frozen* v1 protocol actually `write_file`? (read
   `evidence/30-02/protocol.json`, digest `d18407e0b9…`)
2. Are there exactly nine RUN legs, and is each one actually confounded *by this mechanism*?
   Some legs may be confounded for a different reason, or not confounded at all — e.g. a cost leg
   measuring token spend might be confounded differently from a correctness leg, and the security
   legs are separately `UNPROVEN` for a meter reason (SR-30-1), which is NOT the dialect confound.
3. Does `0/30` for BOTH Wayland and OpenClaw actually follow from dialect, or is OpenClaw's 0/30 a
   different failure that happens to co-occur? Two products failing identically is consistent with
   dialect but does not prove it — a provisioning fault would look the same.

**Q3 is the one most likely to be wrong**, because "both failed, therefore same cause" is exactly
the inference this program keeps getting burned by.

### The blocker the prior lane named, which is what C2 is really for

Protocol v2 status is `REGISTERED, NOT EXECUTED`. Four preconditions, prior lane's status:
corpus for all 3 harnesses (**1 of 3**); `cohort_eligibility` = ELIGIBLE (**NO — COHORT_TOO_SMALL:1**);
translations compiled+verified (**done for Wayland only**); **peers re-provisioned at pins (NOT DONE)**.

Peers are gone from the build host. Pins: Hermes 0.17.0 `dbe734be…`, OpenClaw 2026.6.2 `11a0ad10…`.
Prior lane says Sean's reference checkouts live at `/Users/seandonahoe/dev/resources/` — **unverified
by me; next measurement.**

So the honest shape of this lane is likely: **the instrument is built; the cohort is not.** If I
cannot provision two peers, the position stays unstatable and my deliverable is the precise reason
plus whatever legs a 2-member cohort permits.

### The negative control the brief demands

A deliberately mis-compiled dialect must redden. Must check whether the existing 28 dialect tests
already contain one, or whether the suite only proves the happy path. Per LANE-BRIEF §3.2, I must
read back the executed count and NOT trust exit status; per §3b, `cargo` under `rtk` strips
`0 ignored` / `0 filtered out`, so every cargo invocation goes through `/usr/bin/env cargo`.

### Instrument discipline for this lane

- `/usr/bin/git`, `/usr/bin/grep` for anything load-bearing. (`/usr/bin/cat` does NOT exist on this
  Mac — measured, exit 127. Use the Read tool.)
- Any absence claim needs a known-positive in the same invocation + the query stated (§3b-i).
- Fence: `crates/wcore-cli/src/{lib,main}.rs` — diff against captured `BASE`, never branch name.

## T+0 — status

Nothing measured yet. Nothing built yet. Next: verify the confound claim against 30-02 evidence.

---

## T+1 — MEASUREMENT 1: the inherited "all nine legs are confounded" framing is WRONG in part

Read from `evidence/30-02/{legs.tsv,protocol.json,records/*.jsonl}` — the raw records, not the
summary. Distributions computed with python3 over all 9 record files (n read back per file).

### 1a. What the raw records actually say

| record | n | outcome | fixture_requests | token_units | violations |
|---|---|---|---|---|---|
| hermes-correctness | 30 | SUCCESS 30 | 2 | 20 | none |
| hermes-recovery | 30 | SUCCESS 30 | **3** | **30** | none |
| hermes-cost | 15 | SUCCESS 15 | 2 | 20 | none |
| wayland-correctness | 30 | FAILURE 30 | 2 | 20 | none |
| wayland-recovery | 30 | FAILURE 30 | **3** | **30** | none |
| wayland-cost | 15 | FAILURE 15 | 2 | 20 | none |
| openclaw-correctness | 30 | FAILURE 30 | 2 | 20 | none |
| openclaw-recovery | 30 | FAILURE 30 | **3** | **30** | none |
| openclaw-cost | 15 | FAILURE 15 | 2 | 20 | none |

### 1b. FINDING A — Wayland DID recover from the injected 503. The record proves it.

`dimension_specs.recovery` requires TWO things: the fault was actually served, AND the workspace
reaches the oracle state. The recovery script is `[http_error 503, tool_call, text]` and the meter
is FIFO-cursored, so **request count is a direct readout of how far down the script the harness
walked**. All three harnesses made **exactly 3 requests / 30 token units** — i.e. every one of them
took the 503, retried, consumed the tool-call turn, and came back for the final text turn.

So Wayland Core's published **0/30 on recovery is not a recovery failure at all.** The retry-after-
fault half of the definition succeeded 30/30; the leg scores zero solely on the artifact half, which
is downstream of a tool name Wayland does not expose. Same for OpenClaw.

This is favourable to Wayland and it is still only worth what it is worth: it demonstrates HTTP-level
fault retry, which is a proper subset of the dimension as pre-registered. **It does not license
"Wayland recovers 30/30."** The pre-registered observable is conjunctive and it was not met.

### 1c. FINDING B — the three cost legs are NOT dialect-confounded, and v2 does not repair them

`dimension_specs.cost.observable` = `synthetic_token_units_per_attempted_trial`, defined as the sum
over **every usage frame the fixture emitted**, explicitly *including trials that ultimately failed*.
The fixture emits a fixed script. Therefore:

- cost is **invariant for any harness that follows the script** — measured: 20/20/20 on correctness-
  shaped runs and 30/30/30 on recovery-shaped runs, across three harnesses, with zero variance;
- Wayland's FAILURE trials and Hermes' SUCCESS trials report the **identical** 2 requests / 20 units.
  The metric cannot distinguish a harness that did the work from one that did nothing;
- the only way a harness can register a different cost is by deviating from the script — and
  deviation is separately scored as an `unexpected_request` violation, not as higher cost.

**So the dimension has resolution only in the region the protocol treats as invalid.**

Now the consequence that matters for this lane: **protocol v2 does not touch this.** v2 changes the
tool *name* in the script (`write_file` → `Intent::WriteFile` → each harness's own tool). It does not
change the number of script steps. After a perfect dialect compilation, Wayland executes `Write`,
succeeds on correctness — and still reports 20 units. **The cost comparative comes back
`PRACTICALLY_INDISTINGUISHABLE` for exactly the same non-reason.**

### 1d. The correction to the inherited framing, stated as a table

Inherited (30-DIALECT.md §1, 30-DIALECT-PROTOCOL-V2.md §0, and MILESTONE-RC §6):
*"all nine RUN legs are confounded [because the script spoke one competitor's dialect]"*.

Measured — nine RUN legs, **two** dispositions, not one:

| legs | dimension | disposition | repaired by v2? |
|---|---|---|---|
| 01,02,06,07,11,12 (6) | correctness, recovery | **dialect-confounded** — inherited framing correct | **YES** |
| 04,09,14 (3) | cost | **degenerate by construction** — observable is defined on the fixture's own scripted emissions | **NO** |

### 1e. Why this is a defect in the v2 pre-registration, not just a wording nit

Absence claim, with its query stated and a known-positive in the same invocation
(LANE-BRIEF §3b-i), run with `/usr/bin/grep`:

- known-positive: `grep -c -i cost` → `30-DIALECT.md:6`, `30-DIALECT-PROTOCOL-V2.md:3` — instrument alive.
- known-positive control: `grep -n -E 'stays (UNPROVEN|open)|NOT_MEASURED|does NOT fix'` → 4 hits,
  correctly locating §3 "What v2 does NOT fix" and the LIM-19 / cognitive-tax entries.
- the query: every occurrence of `cost` in both documents, read individually (9 hits total).
  **Not one of them names cost as a dimension v2 fails to repair.** Six of the nine are the English
  word "costs"/"cost" in prose about transfer size or the G6 gate.

Meanwhile `30-DIALECT-PROTOCOL-V2.md` §2 inherits, byte-for-byte and by name, *"trial counts
30/30/30/15/0"* and *"the cost non-inferiority guard"* — i.e. **v2 re-registers cost as a live,
repairable dimension.** Its §3 "What v2 does NOT fix" lists security, FIFO, the vendor-authored
token tables, the compiler blind spots and cognitive tax. Cost is absent from that list.

**So a v2 execution, run exactly as registered, would publish a cost comparative that means nothing,
under a pre-registration that does not warn the reader.** That is the same class of defect as v1's
dialect bug: an instrument limitation the protocol does not price. Recording it now, before any leg
is run, is the only moment at which recording it is not an amendment-after-measurement.

### 1f. Peer availability — SR-30-6 may be closable after all

`/usr/bin/git cat-file -t` against Sean's Mac reference checkouts:

| peer | checkout | pin | present? |
|---|---|---|---|
| Hermes | `/Users/seandonahoe/dev/resources/hermes-agent` @ `d59b79fa` | `dbe734be` | **YES — `commit`** |
| OpenClaw | `/Users/seandonahoe/dev/resources/openclaw` @ `3659c85e` | `11a0ad10` | **YES — `commit`** |

Both working trees clean (0 porcelain lines). So the pins are obtainable without Sean and without a
network fetch from a vendor. The prior lane's blocker was that the *build host* copies were deleted,
not that the pins were lost.

## T+1 — status

Confound analysis done and it corrects the inherited framing on 3 of 9 legs. Next: (a) audit the
compiler's own test suite for a real negative control, (b) attempt peer provisioning at the pins.

---

## T+2 — MEASUREMENT 2: **the compiler is not connected to the trial runner.** HIGH.

This is the finding that decides the lane. Established by reading `bin/wayland-scorecard.rs`, with
known-positives alive in the same invocations per LANE-BRIEF §3b-i.

### 2a. The measurement

**Known-positive 1** — the runner's script source exists and is reached:
`/usr/bin/grep -n 'steps_for' bin/wayland-scorecard.rs` → `914:fn steps_for(`, `936: let steps =
steps_for(protocol, dimension)?;`. Instrument alive.

**Known-positive 2** — `TranslationV1` is a real symbol with real uses:
`/usr/bin/grep -rn 'TranslationV1' bin/ src/ | wc -l` → **9**. Non-zero. Instrument alive.

**The query** — every occurrence of `TranslationV1` and `CompiledStepV1` *in the CLI binary*:

```
bin/wayland-scorecard.rs:27   use ... CompiledStepV1, ToolSchemaCorpusV1, TranslationV1, ...
bin/wayland-scorecard.rs:635      CompiledStepV1::ToolCall(call) => Some(call.tool_name.as_str())
bin/wayland-scorecard.rs:659      let translation: TranslationV1 = serde_json::from_slice(...)
```

Line 635 is inside `DialectCommand::Compile` and only builds a **display string** of resolved tool
names for the `DIALECT_COMPILE=OK … tool_names=…` line. Line 659 is inside `DialectCommand::Verify`.

**So a compiled translation is written by `dialect compile` and read back by exactly one consumer:
`dialect verify`, which re-digests it. Nothing executes it.**

Corroborating, from the runner side:

- `TrialsCommand::Run` (line 244) takes `--protocol --invocation --dimension --trials
  --workspace-root --out`. **There is no `--translation`, no `--corpus`, no `--dialect` argument.**
- `drive_leg` (line 929) obtains its script from `steps_for(protocol, dimension)` (line 936), and
  `steps_for` (line 914) reads `protocol["fixture_script"][dimension]` and deserializes it straight
  into `Vec<OpenAiStep>`.

**There is no code path from a `TranslationV1` to an executed trial.**

### 2b. Why this is worse than the missing peers

`30-DIALECT-PROTOCOL-V2.md` §5 lists four execution preconditions: corpus for all three harnesses,
`cohort_eligibility` ELIGIBLE, translations compiled and verified, peers re-provisioned at their
pins. **All four can be satisfied and protocol v2 still cannot be executed**, because `trials run`
physically cannot consume a translation. It would re-read `protocol.fixture_script` — and the only
`fixture_script` that exists is **v1's, the one that says `write_file`.**

Run today with a perfect three-harness cohort, the v2 re-take would reproduce v1's numbers exactly.

So the prior lane's own precondition list is incomplete, in the same shape as its "what v2 does not
fix" list is incomplete on cost (T+1). Both omissions point the same way: **the compiler was built
to the water's edge and graded on its unit tests rather than on its ability to drive a trial.**

### 2c. What the 27 existing tests do and do not prove

`/usr/bin/grep -c '#[test]'` → `dialect.rs` **21**, `dialect_discovery.rs` **6**. `#[ignore]` count
**0** in both, so the suite is not vacuous by the §3.2 flavour-(a) mechanism.

Read the names: `g1_*` (vocabulary), `g2_*` (permutation invariance), `g3_*` (refusal, no ranking),
`g4_verify_accepts_a_real_translation_and_rejects_a_tuned_one`, `qual_*` (published blind spots),
`tokenizer_*`, `cohort_*`, plus discovery-parser tests.

**Every one of them is a unit test on the selection filter or the discovery parser.** Not one of
them starts a harness, and not one of them asserts that two harnesses driven by their own compiled
translations perform the *same work*. So the suite proves the compiler's *choice* is unbiased; it
proves nothing about whether a compiled script drives a real harness at all.

That is exactly the hole the lane brief names: *"prove the compiler does not itself become the
confound."* It currently is not proven, and cannot be, because the compiler's output is never run.

### 2d. Consequence for this lane

The order of work is now forced:

1. **Wire the translation into the runner** — `trials run --translation`, `CompiledStepV1` →
   `OpenAiStep`. Without it nothing else in this lane is executable. (Not a fenced file:
   `bin/wayland-scorecard.rs` is neither `wcore-cli/src/lib.rs` nor `main.rs`.)
2. **Equivalence test + negative control** — semantically identical trial, two dialects, identical
   work; and a deliberately mis-compiled dialect that must redden.
3. **Provision peers at the pins** (both pins confirmed present on the Mac, T+1f) and re-take
   whatever the instrument makes valid.

## T+2 — status

Two structural findings recorded (cost degeneracy, compiler not wired). Next: build the wiring.

---

## T+3 — MEASUREMENT 3: the compiler works, and it uncovered a SECOND confound underneath

Live on `hetzner-dsm`, worktree `/root/wayland-30c2` @ `9a2e2554`. Real `wayland-core` binary
(330,464,344 bytes), real loopback meter, 30 trials per arm, **same protocol file, same oracle,
same binary, same invocation — the ONLY difference between the arms is the script source.**

Protocol file digest read back on the host: `sha256 d18407e0b96bf753f66adc1eab7d21cbaeca1b9e627cecf0159095938b83ef25`
— byte-identical to the frozen v1 protocol, so both arms ran against the genuinely frozen document.

Live discovery against this lane's own build reproduced the prior lane's corpus digest exactly:
`10c85fbd29ba89bdd08539c63d73744efb5616f584ed84c7f96e4c4a9e8f1323`, 8 declared tools
`Bash,Edit,Forge,Glob,Grep,Read,ToolSearch,Write`. Compile selected `Write`,
`translation_sha256 3ad52c367219ff4278abe86d66401be1983a0b145b83ddc657c1e463a778b4dd`,
`dialect verify` OK.

### 3a. The A/B, as printed by the binary

```
ARM-A  script=frozen_script_v1   driven_tools=-      trials=30 success=0
ARM-B  script=dialect_compiled   driven_tools=Write  trials=30 success=0
```

**Arm B is 0/30 too. The compiled dialect did not fix the number.**

### 3b. But the two arms failed for COMPLETELY DIFFERENT reasons — read from the harness's own
### session transcript, not from its exit status

| arm | tool called | tool_result | |
|---|---|---|---|
| A (v1 frozen) | `write_file` `{path, content}` | `Unknown tool: write_file` | the dialect confound |
| B (v2 compiled) | `Write` `{file_path, content}` | `Refused to write TRIAL-ARTIFACT.txt: path must be absolute: "TRIAL-ARTIFACT.txt"` | **something else entirely** |

Elapsed corroborates: 502 ms/trial in arm A (bounced immediately on an unknown tool) versus
2514 ms/trial in arm B (accepted the call, ran the tool, refused inside it).

**So the compiler did exactly what it was built to do, and it is provable in the product's own
words: the failure moved from "I do not have that tool" to "I have that tool and I declined this
argument".** That is the confound being removed. It is just not the only confound.

### 3c. FINDING C (HIGH) — the second confound is a SLOT VALUE, and the compiler structurally
### cannot fix it

`wayland-core`'s `Write` requires an **absolute** path. The canonical script's `Path` slot is the
literal `TRIAL-ARTIFACT.txt`, which is **relative**. Hermes accepted a relative path and wrote the
file 30/30; Wayland refuses relative paths outright.

The dialect compiler translates **tool names** and **parameter names**. It does not translate
**parameter values** — and the second confound lives entirely in the value.

**It cannot be fixed by extending the compiler, and the reason is structural.** Selection reads the
tokenized tool *name* and the declared JSON Schema. "This parameter must be absolute" appears in
**neither**: it is in the tool's prose description and in its runtime behaviour. The compiler
deliberately never reads descriptions (that exclusion is what makes `qual_a_generic_tool_with_
semantics_in_its_description_is_refused` a published blind spot). So a path convention is invisible
to the instrument by design.

**Compiling names does not compile conventions.** That sentence is the finding.

### 3d. Why nobody could have seen this before now

The prior lane could not have found it, and this is not a criticism — it is the mechanism. Its
compiler was never wired to the runner (T+2), so **no compiled translation had ever been executed
against a harness.** `30-DIALECT.md` says *"Live, against the real corpus: `Write` selected for the
write intents"* — true, and selection is not execution. The second confound was masked by the first
and could only appear once the first was removed.

That is also the strongest argument that the wiring was worth building: **the instrument's first
real run immediately produced a finding that four rounds of unit tests and a 4/4 expert panel did
not.**

### 3e. What I must NOT do

Absolutizing the path **for Wayland only** would be hand-tuning an arm, i.e. the forbidden act by
another route. Any change to the slot value must apply to the canonical script for **every** arm,
must be re-registered before any leg is scored, and must be shown semantics-preserving for the
oracle check. That is a protocol v3 question, not a repair I may make mid-measurement.

## T+3 — status

Compiler proven to work and proven insufficient. Next: a labelled DIAGNOSTIC (not a scored leg)
absolutizing the path for BOTH arms, to prove the diagnosis rather than assert it.

---

## T+4 — MEASUREMENT 4: the 2x2 factorial, and both negative controls

All on `hetzner-dsm`, real `wayland-core`, real loopback meter, frozen v1 protocol file
(`d18407e0b9…`) in every cell. Records pulled to `evidence/30-dialect-c2/live/` and **recounted
independently on the Mac from the pulled JSONL**, not taken from the runner's own summary line.

### 4a. The factorial — two factors, neither sufficient alone

| | frozen v1 script (`write_file`) | compiled dialect (`Write`) |
|---|---|---|
| **relative path** (as registered) | **ARM-A 0/30** | **ARM-B 0/30** |
| absolutized path (diagnostic) | **ARM-D 0/10** | **ARM-C 30/30** |

Independent recount: `arm-a` 30 FAILURE, `arm-b` 30 FAILURE, `arm-c` **30 SUCCESS**,
`arm-d` 10 FAILURE. Every arm's `driven_tools` and `diagnostic` stamp read back from the records.

**Both factors are necessary and neither is sufficient.** That is a controlled isolation, not an
inference from co-occurrence — the failure mode this program keeps getting caught by.

- The dialect compiler is **necessary**: A and D, which lack it, fail at every path setting.
- The dialect compiler is **not sufficient**: B has it and still fails.
- The path convention is a **genuine second, independent confound**: B → C changes nothing but the
  path and moves 0/30 to 30/30.

**ARM-C is also the proof the lane brief asked for.** A compiled dialect, driven end-to-end against
a real harness, produces the oracle artifact with the correct content digest on 30 of 30 trials.
The compiler makes the harness do the actual work; it does not merely fail more politely.

### 4b. Negative control 1 (static) — the guard fires, and for the RIGHT reason

Peer-flavour snake_case corpus, compiled cleanly (`translation_sha256 dc33a244…`, tool `write_file`),
honest manifest saying `hermes`, launched against the `wayland` invocation:

```
wayland-scorecard: DIALECT_EXEC_HARNESS_MISMATCH corpus_declared_by=hermes launching=wayland
                   — refusing to drive a harness with another harness's dialect     rc=1
```

**Instrument repair, per LANE-BRIEF §6b-ii.** My first attempt at this control extracted the corpus
digest by `sed`-scraping the compiler's stdout and captured a leading newline, so the run refused
with `CORPUS_MANIFEST_MISMATCH` — a refusal for the **wrong reason**, which would have passed as a
green negative control while proving nothing about the harness binding. Repaired at source: the
manifest generator now reads `corpus_sha256` out of the translation JSON the compiler wrote and
asserts `len==64 and sha.strip()==sha` before use. Re-run then produced the intended
`HARNESS_MISMATCH`. The refusal is now asserted **by error identity, not by exit status.**

### 4c. Negative control 2 (dynamic) — a mis-compiled dialect REDDENS

The guard refusing is not enough; the brief asks for a mis-compiled dialect whose *measurement*
goes red. So the manifest was **forged** to claim `tool_label: "wayland"` while carrying the peer's
snake_case corpus. The binder accepts it, and wayland is driven with `write_file`:

```
NC-B  script=dialect_compiled+DIAGNOSTIC_ABSOLUTIZED  driven_tools=write_file  trials=10 success=0
```

**0/10, at the exact configuration where the correct dialect scores 30/30.** Same binary, same
protocol, same oracle, same path handling — the compiled tool name is the only difference. A
compiler that silently degraded one arm would show up here, and it does.

### 4d. FINDING D (MEDIUM) — the harness binding is defeated by a forged manifest

NC-B works *because* the manifest can be forged. So check 5 is only as trustworthy as the manifest's
provenance, and a hand-written manifest defeats it. Published rather than patched: the fix is to
bind the manifest to the discovery pass that produced it (a signature, or a digest chain from the
observed wire bytes), which is a larger change than this lane should make mid-measurement.

Stated plainly: **`bind_translation` prevents an accident, not an adversary.**

## T+4 — status

Compiler proven necessary, proven insufficient, proven not to be a confound itself, and both
negative controls red. Next: can any leg actually be re-taken?

---

## T+5 — PRIORITY INTERRUPT handled: B1 egress chokepoint, CI red on Windows

`crates/wcore-eval-scenarios/src/dialect_discovery.rs:449` used a raw `reqwest::Client::new()`,
which `clippy::disallowed_methods` rejects because it bypasses the `wcore_egress::EgressClient`
egress boundary. Present at base `75babf32`; arrived with already-merged dialect work.

**Fixed, not suppressed.** Routed through `wcore_egress::EgressClient::tool()`, matching the
existing loopback precedent in this crate (`tests/packaged_driver_gate.rs:104`). Shipped as its own
commit `983ca230` ahead of the lane's other work so it can merge without waiting.

**No new dependency edge.** `wcore-egress` is already a `[dev-dependencies]` entry of this crate and
the call sits in a `#[cfg(test)]` module, so the rationale recorded in `Cargo.toml:86-89` for
`judge.rs`'s scoped allow — *"would add an internal-crate edge this crate avoids"* — does not apply
to this call site.

### 5a. Gate verified, and proven able to fail

| run | commit | result |
|---|---|---|
| `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` | `983ca230` (fix) | **0 error/warning lines** |
| same command, **falsification control** | `01f83b8f` (parent) | **rc=101**, `error: use of a disallowed method reqwest::Client::new` |
| `cargo clippy --workspace --all-targets -- -D warnings` (the CI gate verbatim) | `32bf61a9` | **WSRC=0**, `disallowed` count **0**, 440 `Checking` lines |
| `cargo fmt --all -- --check` (Mac, permitted) | `32bf61a9` | clean |

The falsification control is the point: a clean clippy proves nothing unless the same command is
shown to fail on the same file one commit earlier. It does.

The single `warning:` line in the workspace log is a third-party future-incompat notice for
`imap-proto v0.10.2` (line 456). It is pre-existing, not in my files, and does not fail the gate.

### 5b. Sibling sweep — two found, both already suppressed, NEITHER touched

Query, unproxied, with a known-positive in the same invocation (13 `reqwest` hits in the crate, so
the instrument is alive):
`/usr/bin/grep -rn -E 'reqwest::(blocking::)?Client(Builder)?::(new|builder)|reqwest::ClientBuilder' src/ bin/ tests/`
plus a second query for `^\s*use reqwest::` to catch a bare `Client::` that the fully-qualified
pattern would miss (**0 hits**, so no import-shortened construction exists).

| site | status | disposition |
|---|---|---|
| `src/judge.rs:138` | scoped `#[allow(clippy::disallowed_methods)]`, rationale in `Cargo.toml:86-89` | **left alone — reported, argued below** |
| `tests/openai_fixture_contract.rs:10` | scoped allow | **left alone — not mine, not red** |

**Why I did not remove them.** `judge.rs` is **not** test code: the D9 LLM-as-judge grader makes a
real outbound HTTPS call to a provider, which is precisely what B1 exists to police, and I think
that allow is weaker than it looks. But fixing it requires promoting `wcore-egress` from a
dev-dependency to a normal dependency — a change to the crate dependency graph, in a repository
with five other lanes live. That is an architectural change, not a lint fix, and bundling it into
the commit that unblocks CI would have delayed the merge the coordinator asked for.

**Recommendation, offered rather than taken:** promote `wcore-egress` to a normal dependency of
`wcore-eval-scenarios` and delete both allows. There is no cycle risk — `wcore-egress` sits below
this crate and this crate already depends on `wcore-protocol`, `wcore-config` and `wcore-types`.
The `tests/` one is free either way. **I am flagging it, not doing it.**

---

## T+6 — MEASUREMENT 5: a REAL two-member cohort, and the symmetry test

**SR-30-6 is closed.** Hermes Agent **0.17.0 at pin `dbe734be`** provisioned on `hetzner-dsm` from a
`git archive` of Sean's reference checkout (59,749,790 bytes — the prior lane's 392 MB OpenClaw
bundle failure does not apply to a tree archive), installed into a venv with
`env -u OPENAI_API_KEY -u ANTHROPIC_API_KEY -u GITHUB_TOKEN`. `hermes --version` →
`Hermes Agent v0.17.0 (2026.6.19)`.

Provider config was NOT guessed: `--provider openai` fails with `Unknown provider 'openai'` at this
pin, and the working shape (`custom_providers: [{name, base_url, model, api_key}]`) was read out of
`hermes_cli/config.py:4400 get_compatible_custom_providers`.

Live discovery: **18 declared tools**, `corpus_sha256 13c077fc…`, names include `write_file`,
`read_file`, `patch`, `terminal`, `execute_code`.

```
DIALECT_COHORT=ELIGIBLE dimension=correctness members=2 all_resolved=true
  member=wayland declared_tools=8  resolved=Write       corpus_sha256=10c85fbd…
  member=hermes  declared_tools=18 resolved=write_file  corpus_sha256=13c077fc…
```

**That gate has never returned ELIGIBLE before.** The prior lane reported `COHORT_TOO_SMALL:1`.

**Incidental validation of the negative control.** Hermes' compiled correctness translation digests
to `dc33a24446f4888a4713727ccec88aa913a160eacad50ae18d679d786d02825a` — **byte-identical** to the
synthetic peer translation NC-B used. So NC-B did not drive Wayland with an approximation of a peer
dialect; it drove it with **the real Hermes dialect**, and got 0/10.

### 6a. Every arm, recounted independently on the Mac from the pulled JSONL

| arm | tool | dim | n | success | driven tool | diagnostic |
|---|---|---|---|---|---|---|
| arm-a | wayland | correctness | 30 | **0** | FROZEN_V1 (`write_file`) | no |
| arm-b | wayland | correctness | 30 | **0** | `Write` | no |
| hermes-rel | hermes | correctness | 30 | **30** | `write_file` | no |
| arm-c | wayland | correctness | 30 | **30** | `Write` | yes |
| arm-d | wayland | correctness | 10 | **0** | FROZEN_V1 | yes |
| hermes-abs | hermes | correctness | 30 | **30** | `write_file` | yes |
| nc-b | wayland | correctness | 10 | **0** | `write_file` (real Hermes dialect) | yes |
| wayland-recovery-abs | wayland | recovery | 15 | **15** | `Write` | yes |
| hermes-recovery-abs | hermes | recovery | 15 | **15** | `write_file` | yes |

### 6b. THE SYMMETRY TEST — the remedy is not a Wayland tune

The obvious attack on Finding C's remedy is: *"you absolutized the path because it makes your
product pass."* So it was run on the peer too.

| harness | compiled dialect, **relative** path (as registered) | compiled dialect, **absolutized** path |
|---|---|---|
| Hermes 0.17.0 | **30/30** | **30/30** |
| Wayland Core | **0/30** | **30/30** |

**Absolutizing is neutral for the peer and decisive for us.** Hermes is indifferent; it passes
either way. That is what a de-biasing change looks like, and it is the opposite of a tune: the
change that would flatter Wayland selectively would have to *hurt* the peer, and this one does not
touch it.

The corollary is the uncomfortable half, and it is the honest reading: **v1's relative path was
itself the biased choice.** It silently required a path convention that exactly one of the two
harnesses honours, and that requirement was never stated in the protocol.

### 6c. FINDING A confirmed by prediction, not by re-reading

T+1b predicted from the v1 request counts that Wayland's recovery machinery worked and only the
artifact half of the conjunctive observable failed. Tested: **wayland recovery 15/15, hermes
recovery 15/15** under compiled dialects with the path confound removed. The prediction held.

## T+6 — status

Cohort ELIGIBLE at 2 members, symmetry proven, both dimensions exercised. Writing the deliverable.

---

## T+7 — final verification, and a self-inflicted false alarm worth recording

### 7a. Final suite at deliverable HEAD `317807fb`

```
cargo test -p wcore-eval-scenarios --lib
test result: ok. 263 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.62s
FTRC=0
```

Executed count read back per LANE-BRIEF §3.2. `0 filtered out` confirms no flavour-(c) empty filter;
2 ignored out of 265 is the normal partial-ignore case, not the all-ignored pathology.

### 7b. The false alarm — I nearly reported a hang that was mine

For ~20 minutes `--lib` appeared wedged on
`process_tree::linux::tests::private_materialization_preserves_ali…`, a test in a file this lane
never touched. **The tempting move was to report it as a pre-existing flake and move on.**

`ps -eo pid,etime` showed **two** copies of the same test binary running, at 36:30 and 16:19: an
earlier foreground run had been backgrounded on timeout and never died, and I had launched a second
alongside it. `process_tree` tests contend. A clean single run then finished the same 263 tests in
**0.62 s**.

So the hang was **self-inflicted concurrency, not a product defect and not a flake**. Recording it
because the near-miss is the interesting part: "cluster of failures in one crate under load — re-run
in isolation before reporting a regression" (LANE-BRIEF §6) applies to a *hang* exactly as it applies
to a failure, and I was one step from filing a false report against another lane's file.

Second-order note: the `pkill -f "wcore_eval_scenarios-<hash>"` I used to clear the duplicates
**matched its own command line** and killed the shell, and the run launched immediately after died of
SIGTERM (signal 15) — which `cargo` surfaced as `error: test failed`, i.e. an infrastructure kill
wearing a test-failure costume. Re-run under `setsid` with a non-self-matching pattern: clean.

### 7c. Fence, verified against the merge-base SHA (never the branch name)

```
BASE=75babf329235484684ecee3a65973b0c197840c1
git diff $BASE -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs   →  0 lines
git diff $BASE -- crates/wcore-eval-scenarios/src/fixtures/openai.rs         →  0 lines
git diff $BASE -- crates/wcore-eval-scenarios/src/dialect_exec.rs            →  557 lines  (known-positive)
```

The known-positive is what makes the two zeros measurements rather than a dead command.

Frozen v1 protocol digest re-verified identical on host and in repo:
`d18407e0b96bf753f66adc1eab7d21cbaeca1b9e627cecf0159095938b83ef25`.

## LANE COMPLETE

Deliverable: `.planning/phases/30-continuous-scorecard-frontier-review/30-DIALECT-C2.md`.

### 7d. Host cleanup, and what was deliberately RETAINED

Removed: `/root/wayland-30c2` worktree and its `target/`, every trial workspace, the Hermes source
tree and its archive. `df -h /root` 697G → **709G free**. Worktree list: 28 remain (known-positive),
`30c2` count **0**.

**Retained on purpose at `/root/c2` (207 MB):** the Hermes 0.17.0 venv — verified still runnable
(`Hermes Agent v0.17.0 (2026.6.19)`) — plus every invocation, corpus, manifest and translation. A
follow-up lane executing SR-30-7 needs a provisioned peer, and re-provisioning it is the exact cost
SR-30-6 was raised about. **Do not delete `/root/c2` when reclaiming space; it is the only
provisioned peer on the host.**
