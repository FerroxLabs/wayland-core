# 30-DIALECT — per-tool dialect compilation (SR-30-3)

**Lane** `lane/30-dialect`, branched from `plan/f20-unified-audit-repair` @ `8bcb052b`.
**Live host** `hetzner-dsm`. **Graded** 2026-07-29.

| | |
|---|---|
| Compiler | `crates/wcore-eval-scenarios/src/dialect.rs`, vocabulary `F30-DIALECT-VOCAB-V1` |
| Discovery meter | `crates/wcore-eval-scenarios/src/dialect_discovery.rs` |
| CLI | `wayland-scorecard dialect {vocabulary,discover,compile,verify,cohort}` |
| Pre-registration | `30-DIALECT-PROTOCOL-V2.md`, `evidence/30-dialect/protocol-v2.json`, sha256 `b23ff64ad427f785cbdd1c393959863ca065d4beb55ed6ab095583be22261947` |
| Panel | 4-way, **4/4 `CONFIRM_WITH_AMENDMENT`** — `evidence/30-dialect/panel-{codex,gemini,kimi,internal}.txt` |
| Live evidence | `evidence/30-dialect/live/LIVE-TRANSCRIPT.md` |
| Tests | `cargo test -p wcore-eval-scenarios --lib dialect` → **28 passed, 0 failed** (executed count read back) |
| **Comparatives re-taken** | **NONE.** Correct: the panel gated a re-take, and the gate is not met. |
| Provider spend | **$0.00** — every leg is loopback; `flux.env` was never opened |

---

## 0. The verdict in one paragraph

**The compiler exists, works on a real harness, and is registered under a new pre-registration —
and it is still not clear to re-take Criterion 2's comparatives.** The four-way panel confirmed the
design with a unanimous amendment, that amendment is implemented and live-proven, and the resulting
gate's honest answer today is `INELIGIBLE`: a cohort of one is not a cohort. That is a complete
outcome, not a partial one, and it is the outcome the lane brief named as complete.

The most useful thing this lane produced is not the compiler. It is the discovery that **three of
the five bias guards I wrote did not prove what I said they proved**, found by a panel I asked to
attack them, and struck from the code rather than footnoted.

## 1. The defect, and why it needed repairing at all

The frozen F30-03 script emits a tool call named **`write_file`**. Measured: Hermes 30/30 on
correctness and recovery; Wayland Core **0/30**; OpenClaw **0/30**. Wayland's equivalent is `Write`.
Two of three harnesses failed the identical script, which measures the script's dialect rather than
two products. All nine RUN legs are confounded and 30-03's `confounded_leg_supports_no_comparison`
mechanically refuses every comparison resting on them.

**Now confirmed on real captured data rather than inferred.** The corpus captured off the live
binary's own wire declares `Bash, Edit, Forge, Glob, Grep, Read, ToolSearch, Write` — and **not
`write_file`**. The frozen script named a tool this harness has never exposed.

## 2. The compiler

A canonical script names **intents** with typed slots, not tool names:

| Dimension | v1 | v2 |
|---|---|---|
| correctness, cost | `tool_call "write_file" {path, content}` | `Intent::WriteFile {Path, Content}` |
| recovery | `503`, then the same call | `503`, then `Intent::WriteFile` |
| security | `tool_call "read_file" {path}` | `Intent::ReadFile {Path}` |

An **unscored discovery pass** captures each harness's own declared `tools` array from the
`POST /v1/chat/completions` body **the harness itself sends**. The translation is therefore derived
from the peer's own bytes, not hand-written by me — which is the whole difference between "I wrote
a mapping per tool" and "each harness told the meter what it exposes and one identical rule read
all three answers".

Discovery uses a **separate meter**. `fixtures/openai.rs` records digests, not bodies (SR-30-1), and
is a hard scope fence; it is left byte-untouched, so every 30-02 number keeps meaning what it meant.
The discovery meter retains only the `tools` declaration — never `messages` — asserted canary-free
by test, because a per-trial canary lives in the workspace of the runs it observes.

**Selection is a filter, not a ranking.** A declared tool survives only if its tokenized *name*
carries no disqualifying token, carries an action token, and every required slot binds to exactly
one declared string parameter while every parameter the harness marks `required` is one the script
can supply. Exactly one survivor compiles; **zero or two-or-more refuses**. There is no score,
therefore no tie-break, therefore no lever.

Live, against the real corpus: `Write` selected for the write intents, `Read` for the read intent,
and the other six declared tools excluded — without the compiler being told anything about any of
them.

## 3. The bias guard — including the parts of it that failed

This is the section to read adversarially, because it is the section where a vendor-run benchmark
would lie.

| Guard | What it proves | What it does **not** prove |
|---|---|---|
| **G1** vocabulary committed before any corpus was captured; no token is a product name (asserted mechanically, `product_tokens_found=0`) | the tables were not fitted to captured data | **independence from my prior knowledge.** `Write` is not a product name, so a rule keyed to it passes G1 cleanly |
| **G2** identity-blind *by type* — the corpus struct has no field naming its product, the compiler takes nothing else; permutation test asserts invariance | determinism, and absence of label leakage | **fairness.** Any pure function passes it, including the maximally biased rule *"select the tool named exactly `Write`"* |
| **G3** filter, not ranking | there is no tie-break to bias | that the gate membership is itself neutral |
| **G4** both digests recomputed by `verify()` | a hand-tuned translation is detected | anything about whether the translation is the *fairest* one |
| **G5** no byte-identity claimed; all digests published | — | — |
| **G6** symmetric-resolution gate *(added by the panel)* | selective measurability costs the vendor what it costs a peer | that the token tables are independent |

**An earlier draft of this design asserted the three "does not prove" cells as if they were proven.
The panel struck all three and they are struck in the code comment too**, not merely noted here —
LANE-BRIEF §6b-ii is explicit that a written-up instrument defect is a defect you have agreed to
keep.

### G6 — what the panel added, and why it costs us

I claimed a refusal was neutral, because `ComparativeResultV1` cannot be built without every
compared harness. **All four members rejected that, and they were right.** The *constructor* is
symmetric; the *report* is not. If Wayland resolves and a peer refuses, Wayland publishes an
absolute number the peer cannot, and a reader draws the inference the comparative declined to
state. Codex named the channel **selective measurability**.

So: **a refusal by ANY harness makes that dimension ineligible for EVERY harness, ours included.**
A cohort under two members is never eligible, so *"we could not run the competitor, so we win"*
stays inexpressible for absolute numbers, not just for comparatives.

This is also what makes the vendor-authored token lists safe to leave in place: **a list tuned to
exclude a peer's tools destroys our own leg by the same act.**

Proven live, and the load-bearing row is the one that costs us:

```
GATE 2  members=2  refused_by=peer   →  INELIGIBLE, rc=1
        member=wayland resolved=Write refusal=-      ← resolved cleanly, still unpublishable
        member=peer    resolved=-     refusal=DIALECT_NO_CANDIDATE
```

### The blind spots, published rather than patched

The counterfactual qualification suite exists to **publish** what the filter gets wrong, not to
show it gets nothing wrong. Each of these is an asserted refusal in a test:

- a **capable-but-denylisted** tool is refused — `edit_file` creates missing files on many
  harnesses; the filter cannot know that and does not guess;
- a **generic tool whose semantics live in its description** (`filesystem` + an `operation`
  discriminator) is refused;
- **adding a valid tool flips a resolving surface to `AMBIGUOUS`** — so a richer tool surface faces
  more collision opportunities. Codex: *"adding a valid tool can destroy eligibility."* Measured
  true. Patching it would require a tie-break, and a tie-break is the lever G3 removes, so it is
  recorded as a limitation instead.

### What is NOT closed

**The token tables are vendor-authored.** G6 makes tuning them self-defeating; it does not make
them independent. Kimi's amendment — third-party ratification of the vocabulary against an external
corpus — is the real fix and is outside this lane's authority. **Carried forward as a seam
request.**

## 4. The panel

Four-way, **4/4 `CONFIRM_WITH_AMENDMENT`**, and the three external members converged independently
on the same amendment:

| Member | Top amendment |
|---|---|
| codex `gpt-5.6-sol` | product-blind counterfactual qualification suite; **suppress all harnesses' scores for any leg with an asymmetric refusal** |
| gemini `3.1-pro-preview` | publish a pre-benchmark all-harness dry-run; a ruleset refusing a peer before trials begin is asymmetric and must be rejected |
| kimi K3 | vocabulary ratified by a disinterested third party; published all-harness survivor/refusal dry-run before any scored leg |
| internal (adversarial) | argued **REJECT**; lost on the merits, landed four hits, all adopted |

Both halves — the qualification suite and the symmetric-resolution gate — are implemented, tested
and live-proven. Kimi's third-party ratification is not, and cannot be from inside this lane.

**Capture hygiene, since the brief warns these silently drop votes.** Every capture was
byte-counted and eyeballed. Codex's *first* run cast **no vote**: it treated the prompt as an
agentic task, explored the repository, and echoed the question — whose text contains all three
verdict strings, so a naive "take the last match" grep would have scored it **REJECT**. It was
re-run from a neutral directory with an explicit no-tools instruction and then voted. Kimi
indents its verdict, so the extraction is unanchored. `rtk` was also observed rewriting `grep`
output in this environment, so every extraction used `/usr/bin/grep`.

## 5. What is now re-takeable — and what is not

**Re-takeable in principle, blocked in practice.** Protocol v2 is `REGISTERED, NOT EXECUTED`. Its
four execution preconditions, and their honest status:

| Precondition | Status |
|---|---|
| corpus captured for **all three** harnesses at their pins | **1 of 3.** Wayland captured live; both peers not provisioned |
| `cohort_eligibility` reporting `ELIGIBLE` | **NO** — today it reports `COHORT_TOO_SMALL:1`, rc=1 |
| translations compiled, digested, `dialect verify` passing | **DONE for Wayland**, all four dimensions |
| peers re-provisioned at their pins | **NOT DONE** — the 30-02 install directories were disposable and are gone from the build host (`find /root /srv /opt` for `pyproject.toml`/`openclaw.mjs`: 0 hits) |

The peers are re-obtainable — Sean's reference checkouts are on the Mac at
`/Users/seandonahoe/dev/resources/` — but re-provisioning OpenClaw cost a 392 MB bundle transfer
that failed at 42% once already, plus a 243 s build. That is a lane's worth of work on its own and
it is not this lane's.

**What genuinely changed:** before this lane, Criterion 2 could not be re-taken *at all*, because
any re-run would have reproduced the same confound. It now has a registered protocol, a working
compiler proven against a real harness, and a gate that will refuse to let it be run dishonestly.
The blocker moved from *"there is no instrument"* to *"the cohort is incomplete"*, which is a
provisioning problem rather than a methodological one.

**Still unfixed by v2, and stated before anyone runs it:** the security legs stay UNPROVEN (the
meter still retains digests, not bodies — SR-30-1 open, SR-30-4 still forbids the narrower
substitute); the meter is still FIFO-cursored, not content-routed (SR-30-2 open); cognitive tax
stays `NOT_MEASURED`.

## 6. Honest grading

- **SR-30-3: DELIVERED.** Compiler, discovery, digests, CLI, protocol v2, panel. Codex's original
  prescription is adopted literally, including *"do not falsely claim byte identity"*.
- **Criterion 2: STILL NOT MET, and this lane does not change that grade.** No comparative was
  re-taken and none may be until the cohort is complete. Anyone reading this document as *"the
  confound is fixed, so Wayland's 0/30 doesn't count"* is reading it wrong: **the 0/30 stands as
  published**, and no replacement number exists.
- **The lane brief's stated complete outcome — a working compiler plus a registered amendment —
  is met.**

## 7. New seam requests

### SR-30-5 — third-party ratification of the dialect vocabulary
**Raised by:** panel member kimi, echoed by codex.
**What is needed.** The action / disqualifying / slot token tables in `dialect.rs` are authored by
the vendor whose product they will help measure. G6 makes tuning them self-defeating but not
independent. They need ratification by a disinterested party against a corpus of external harness
schemas the vendor did not choose.
**What breaks if omitted.** The strongest available claim stays *"tuning the tables costs us our
own leg"* rather than *"the tables are neutral"*. That is a real bound and it is not neutrality.

### SR-30-6 — peer re-provisioning at the pins, as its own unit of work
**What is needed.** Hermes `dbe734be…` and OpenClaw `11a0ad10…` re-provisioned on the build host
from their own lockfiles, and a discovery pass run against each.
**What breaks if omitted.** Protocol v2 stays `REGISTERED, NOT EXECUTED` and Criterion 2 stays
un-re-takeable in practice, however good the instrument is.
