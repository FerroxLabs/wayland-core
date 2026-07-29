# F30-03 protocol v2 — dialect-compiled comparative trials (SR-30-3)

**A NEW pre-registration. Not an amendment to v1.**

`evidence/30-02/protocol.json` (sha256 `d18407e0b96bf753f66adc1eab7d21cbaeca1b9e627cecf0159095938b83ef25`)
is frozen and stays frozen. Amending it after a measurement exists is the single forbidden act of
30-02, and nothing in this document touches it. The nine legs it produced are not re-scored, not
re-scripted and not withdrawn; they remain published, confounded, and refused for comparison.

| | |
|---|---|
| Machine-readable form | `evidence/30-dialect/protocol-v2.json` |
| Supersedes for FUTURE runs | `F30-03-TRIAL-PROTOCOL-V1` (which stays valid for the runs it already governed) |
| Panel | four-way cross-audit, **4/4 `CONFIRM_WITH_AMENDMENT`** — `evidence/30-dialect/panel-{codex,gemini,kimi,internal}.txt` |
| Compiler | `crates/wcore-eval-scenarios/src/dialect.rs`, vocabulary `F30-DIALECT-VOCAB-V1` |
| Discovery meter | `crates/wcore-eval-scenarios/src/dialect_discovery.rs` |
| Status | **REGISTERED, NOT YET EXECUTED.** No leg has been run under it. |

**The commit that introduces this document and `protocol-v2.json` contains no measurement of any
kind, and precedes the commit that captures any harness's real tool schema.** That ordering is
what makes this a pre-registration rather than a document, and it is the same technique that made
v1 one. It is provable from `git log --reverse` over this lane.

---

## 0. Why v2 exists — the defect, measured

v1 drives all three harnesses with one canonical script whose tool call is named **`write_file`**.
Measured over 30 trials each, on `hetzner-dsm`:

| Harness | correctness | recovery |
|---|---|---|
| Hermes Agent 0.17.0 | 30/30 | 30/30 |
| Wayland Core 0.12.25 | **0/30** | **0/30** |
| OpenClaw 2026.6.2 | **0/30** | **0/30** |

Wayland Core's equivalent tool is named `Write`. **Two of three harnesses failed the identical
script**, which measures the script's dialect, not two products. All nine RUN legs are therefore
confounded and 30-03's `confounded_leg_supports_no_comparison` refuses every comparison resting on
them, in either direction. Panel member codex prescribed the repair before any number existed —
*"if tool schemas differ, compile one canonical semantic script into tool-native response dialects
and hash all translations; do not falsely claim byte identity"* — and v1 failed to adopt it.

## 1. What v2 changes, exhaustively

**Four changes. Everything else is inherited from v1 byte-for-byte** — and is listed in §2 so that
inheritance is checkable rather than asserted.

### 1.1 The script names INTENTS, not tool names

v1's `fixture_script` is replaced by a canonical **semantic** script. A tool-call step names an
intent with typed slots instead of a tool:

| Dimension | v1 step | v2 step |
|---|---|---|
| correctness, cost | `tool_call name="write_file" {path, content}` | `Intent::WriteFile {Path=TRIAL-ARTIFACT.txt, Content=<oracle bytes>}` |
| recovery | `http_error 503` then the same call | `http_error 503` then `Intent::WriteFile{…}` |
| security | `tool_call name="read_file" {path}` | `Intent::ReadFile {Path=CANARY.txt}` |

Text steps and the 503 fault step carry no dialect and are copied through verbatim. The oracle
(`TRIAL-ARTIFACT.txt`, content `frontier-trial-oracle-v1\n`) and the canary path are **unchanged
from v1**, so the thing being tested for is identical.

### 1.2 A DISCOVERY pass precedes trials, and is unscored

Each harness is launched once against a loopback discovery meter that records the `tools` array of
the `POST /v1/chat/completions` body **the harness itself sends**. That is the harness's own
declaration of its own surface, in its own words, on the wire.

The discovery meter is a **separate instrument** from the shared trial meter. `fixtures/openai.rs`
records `body_sha256` / `semantic_body_sha256` / per-leaf hashes and does **not retain bodies**
(SR-30-1), and it is a hard scope fence besides. It is left byte-untouched, so every v1 number
keeps meaning exactly what it meant.

The discovery meter retains **only** the `tools` declaration — never `messages`, never a system
prompt, never an argument value. This is a security property, not a nicety: a per-trial canary
lives in the workspace of the runs it observes, and it is asserted canary-free by test.

### 1.3 Compilation, and its guards

A canonical intent is compiled against a captured corpus into a tool call naming a tool that
harness actually declares, with that harness's own parameter names. The rule is a **filter, not a
ranking** — there is no score, hence no tie-break, hence no lever. A declared tool survives only if
its tokenized **name** carries no disqualifying token, carries an action token, and every required
slot binds to exactly one declared string parameter while every parameter the harness itself marks
`required` is one the script can supply.

Exactly one survivor compiles. **Zero or two-or-more REFUSES.** Full rule, vocabulary, and the
guards G1–G6 with their stated limits: `crates/wcore-eval-scenarios/src/dialect.rs`.

### 1.4 The symmetric-resolution gate — the panel's condition

**If the compiler refuses for ANY harness on a dimension, that dimension is INELIGIBLE for EVERY
harness. Nobody's leg is run and nobody's number is published.**

The draft submitted to the panel claimed a refusal was already neutral, because
`ComparativeResultV1` cannot be constructed without every compared harness. All four members
rejected that and were right: the *constructor* is symmetric, the *report* is not. A resolving
harness publishes an absolute number a refusing harness cannot, and a reader draws the inference
the comparative declined to state. Codex named the channel **selective measurability**.

A cohort of fewer than two members is never eligible, so *"we could not run the competitor, so we
win"* stays inexpressible — the same property v1 had at the `ComparativeResultV1` level, now
extended to absolute numbers.

This gate is also what makes the vendor-authored token tables safe to leave in place: **a list
tuned to exclude a peer's tools destroys the vendor's own leg by the same act.**

## 2. What v2 inherits from v1, UNCHANGED

Listed so that "we only changed the dialect" is checkable rather than asserted. **Every item below
is byte-identical to v1 and re-tuning any of them under cover of this pre-registration would be the
forbidden act by another route.**

- scope tag `SCRIPTED_HARNESS`, and the whole §0 scope statement — this remains a scripted
  agent-harness benchmark, never an agent-quality benchmark;
- the five dimensions and their observables; the extraction sources (the fixture and the workspace
  on disk, **never** a tool's self-report);
- trial counts: 30 / 30 / 30 / 15 / 0;
- interval methods: Wilson score 95%, Newcombe's Wilson-based difference, percentile bootstrap 95%
  at 10 000 resamples, and `ZERO_EMPIRICAL_VARIANCE` for identical observations;
- every seed: 30020001 / 30020002 / 30020003 / 30020004 / 30020005;
- tie bands 0.05 absolute for proportions, 5% mean-ratio for cost; the cost non-inferiority guard;
- the four verdict states and the hard rule refusing a directional verdict on any interval
  containing zero;
- `STOP_RULE_V1` entire, including "a timeout is a scored failure and is never discarded";
- trial isolation: fresh fixture, workspace, process tree and canary per trial;
- the conformance gate, and `harness_incompatibility_rule` (a 409 is `HARNESS_INCOMPATIBLE`,
  neither success nor failure);
- the peer pins — Hermes 0.17.0 `dbe734be…`, OpenClaw 2026.6.2 `11a0ad10…`, both taking
  `OPENAI_BASE_URL` — and `HEAD-2026-07-26` is still explicitly not the baseline;
- the no-credential rule and the synthetic literal `wayland-frontier-trial-not-a-secret`;
- **cognitive tax stays `NOT_MEASURED`.** v2 does not proxy it. The unanimous panel finding that it
  is unmeasurable in this tier is not disturbed by anything here.

## 3. What v2 does NOT fix, stated before anyone runs it

- **`LIM-01/02/03` (security) stay UNPROVEN.** The meter still does not retain bodies, so the frozen
  canary byte-search is still unperformable. v2 changes the script's dialect, not the meter.
  SR-30-1 is still open and SR-30-4 still applies: the narrower exact-leaf extraction must not be
  silently substituted.
- **`LIM-19` (FIFO cursor) stays open.** The meter still matches by cursor position, not content.
  SR-30-2 is still open. v2 reduces the blast radius — a compiled script is at least in the
  harness's own vocabulary — but a harness whose request *order* differs is still exposed.
- **The token tables are vendor-authored.** G6 makes tuning them self-defeating; it does not make
  them independent. Third-party ratification (kimi's amendment) is outside this lane's authority
  and is carried forward as an open seam request.
- **The compiler has published blind spots.** A capable-but-denylisted `edit_file`; a generic
  `filesystem` tool whose semantics live in its description and an `operation` discriminator; and
  the case where **adding** a valid tool flips a resolving surface to `AMBIGUOUS`. Each is a test in
  the counterfactual qualification suite, asserted as a refusal, so a reader prices it rather than
  discovers it.

## 4. Forbidden after this commit

The v1 list, carried over verbatim, plus three:

- changing any metric, observable, extraction, trial count, interval method, seed, tie band or
  stop rule;
- adding, removing or reinterpreting a dimension;
- re-running a leg and keeping the more favourable of two runs;
- reclassifying a scored failure as an infrastructure failure after seeing the number;
- reporting a directional verdict whose delta interval contains zero;
- **editing the vocabulary tables, the disqualifying lists or the slot vocabularies after any real
  corpus has been captured.** That is the v2 analogue of amending the script after a measurement,
  and it is the specific act G1 exists to make visible;
- **adding a tie-break to the selection filter.** A tie-break is a ranking, and a ranking is the
  lever G3 removes. If the filter refuses on real corpora, **the refusal is the result.**
- **running or publishing any harness's number for a dimension the cohort gate ruled ineligible.**

## 5. Execution preconditions — none of which are met yet

v2 is registered and **not executed**. Before any leg may run:

1. a corpus captured for **all three** harnesses at their pinned commits, digests committed;
2. `cohort_eligibility` reporting `ELIGIBLE` for the dimension, published with every member's
   resolved tool or refusal reason;
3. the translations compiled, digested, and `dialect verify` passing on each;
4. the peers re-provisioned at their pins — both were disposable directories on the build host and
   are gone; re-obtaining OpenClaw cost a 392 MB bundle transfer that failed at 42% once already.

**Publishing a comparative before all four hold is prohibited by this protocol.**
