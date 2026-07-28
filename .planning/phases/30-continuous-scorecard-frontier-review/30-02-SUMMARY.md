---
phase: 30-continuous-scorecard-frontier-review
plan: "02"
subsystem: eval-harness
status: complete
termination_state: 2 (partially blocked)
tags: [frontier-review, comparative-trials, pre-registration, confidence-bounds, F30-03]
requires: ["30-01"]
provides:
  - "pre-registered, content-addressed comparative trial protocol (F30-03)"
  - "trial harness in which an unbounded point estimate and a one-sided comparison are unconstructible"
  - "fifteen-leg accounting: 6 RUN, 9 UNPROVEN, each naming a real capture"
affects: ["30-03", "30-04"]
tech-stack:
  added: []
  patterns: ["pre-registration-bound-by-digest", "no-directional-claim-without-separation", "consumer-of-existing-harness"]
key-files:
  created:
    - .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-PROTOCOL.md
    - .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md
    - .planning/phases/30-continuous-scorecard-frontier-review/30-02-decision-evidence/
    - .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/
    - crates/wcore-eval-scenarios/src/frontier_trials.rs
    - crates/wcore-eval-scenarios/tests/frontier_trials_contract.rs
  modified:
    - crates/wcore-eval-scenarios/src/lib.rs
    - crates/wcore-eval-scenarios/bin/wayland-scorecard.rs
decisions:
  - "Shared loopback meter with a declared-but-unrun live tier, 4/4 panel CONFIRM_WITH_AMENDMENT"
  - "FOUR verdict states, not the plan's three: INCONCLUSIVE added so a wide interval is not published as equivalence"
  - "cognitive_tax declared NOT MEASURABLE in this tier rather than proxied - unanimous"
  - "SplitMix64 rather than rand::StdRng, whose stream is not guaranteed stable across releases"
metrics:
  legs_run: 6
  legs_unproven: 9
  contract_tests: 9
  targeted_suite: "470 passed, 0 failed, 5 skipped"
---

# Phase 30 Plan 02: Frontier comparative trials Summary

Pre-registered a content-addressed comparative methodology by four-way cross-audit before any
number existed, built a harness in which a point estimate without bounds and a comparison
missing a peer are both unconstructible, and ran the trials — producing a result that goes
**against** the product whose vendor ran the benchmark, published unaltered.

**Termination state: 2 — partially blocked.** 6 of 15 legs RUN, 9 UNPROVEN with named blockers
and substitution points.

## The panel, and the vote-extraction defects measured on the way

4/4 `CONFIRM_WITH_AMENDMENT` (codex gpt-5.6-sol, gemini-3.1-pro-preview, kimi K3, internal
adversarial). Unanimity is treated as weak evidence and the decision rests on the amendments,
where members genuinely disagreed — trial counts split 30/30/10; only codex raised Newcombe;
only kimi and codex demanded a fourth verdict state.

**Two vote-extraction defects, both caught only by reading the captured output:**

- A poll loop reported codex as voted at 22:39. It had not — `codex exec` **echoes the prompt**,
  which contains the literal `PANEL_POSITION=<CONFIRM|...>`, so `grep -c 'PANEL_POSITION='`
  matched the question. Codex did not return until 22:44. Extraction was tightened to the
  three literal tokens with `tail -1`.
- The converse, measured on the committed artifact: `grep -cE '^PANEL_POSITION=' panel-kimi.txt`
  → **0**; unanchored → **3**. An anchored regex would have silently made this a three-way panel.

## Pre-registration ordering, proved from history

| | |
|---|---|
| protocol.json sha256 | `d18407e0b96bf753f66adc1eab7d21cbaeca1b9e627cecf0159095938b83ef25` |
| pre-registration commit | `a7bd5d87` — contains no measurement of any kind |
| results commit | `abf652af` |
| `git merge-base --is-ancestor` | **PASS**, distinct commits |

## RED before GREEN

`914d7946` committed the contract suite alone: `REMOTE_RC=101`,
`error[E0432]: unresolved import wcore_eval_scenarios::frontier_trials`. `cee109f6` added the
module. Contract suite on hetzner: **9 passed, 0 failed, 0 ignored** — executed count read back,
not exit status. Inline suite: 5 passed, 207 filtered out (the filter matched real tests).

## The three structural refusals, each with a pristine control accepted first

| Refusal | Test |
|---|---|
| no unbounded point estimate | `a_measurement_without_an_interval_does_not_deserialize` (also refuses explicit null) |
| no one-sided comparison | `a_comparative_result_with_a_missing_peer_measurement_cannot_be_constructed` (symmetric — dropping Wayland is refused too) |
| no direction on an interval containing zero | `a_directional_verdict_is_refused_when_the_delta_interval_contains_zero`, controlled by `indistinguishable_verifies_when_the_delta_interval_contains_zero` |

**Proof the verifier can fail**, run against the shipped release binary: 1 pass and 4 distinct
refusals — one-byte protocol mutation, dropped leg, UNPROVEN leg with its blocker removed,
invented scope tag rejected at deserialization. Capture: `evidence/30-02/verifier-known-good-bad.txt`.

The bootstrap test carries an **anti-tautology leg**: a different seed must move the bounds, or a
resampler that ignored its seed (or returned min/max) would pass the determinism assertion
trivially.

## The results — and they go against us

| Dimension | Wayland | Hermes 0.17.0 | Verdict |
|---|---|---|---|
| correctness | 0/30, [0.0000, 0.1135] | 30/30, [0.8865, 1.0000] | **PEER_AHEAD** |
| recovery | 0/30, [0.0000, 0.1135] | 30/30, [0.8865, 1.0000] | **PEER_AHEAD** |
| cost | 20.00 units, zero variance | 20.00 units, zero variance | PRACTICALLY_INDISTINGUISHABLE |

**The dominant cause is a defect in my own frozen protocol**, found by running it: the single
canonical script emits a tool call named `write_file`, which Hermes accepts and whose Wayland
equivalent is named `Write`. Panel member codex prescribed per-tool dialect compilation and this
protocol failed to adopt it. Amending a protocol after a measurement exists is the one forbidden
act, so the legs were neither re-scripted nor re-run — **and the result was not suppressed**,
because hiding an unfavourable number is exactly the forgery this plan exists to prevent. The
honest reading is narrow and is stated in §3 of the results: *given a provider emitting a
`write_file` tool call, Hermes completed the task 30/30 and Wayland 0/30.* Whether that is a
Wayland interoperability defect or an artifact of my script **is not settled by this evidence**,
and 30-03 must not position from it as though it were.

**Second finding, also against our own tool.** `wayland-core` refused to start on the headless
host — *"Session persistence authority unavailable: no OS keyring was usable and no encrypted
credentials vault is unlocked"* — and reached a provider only after an `encrypted-file`
credentials config plus a vault passphrase. Neither peer needed an equivalent. Carried as
`workspace_seed_files` **data** so the extra setup our own tool required stays visible.

## The nine UNPROVEN legs

- **security ×3** — the protocol's extraction is a byte search of request bodies; the meter
  records body *digests* and per-leaf hashes, not bodies. An exact-leaf comparison would be a
  strictly **narrower** extraction, so it was **not** silently substituted. Meter gate-checked untouched.
- **cognitive_tax ×3** — unanimous panel finding, frozen before any trial. F30-03 is incomplete
  on one of its five dimensions and this plan says so.
- **openclaw ×5** — the 392,186,966-byte read-only bundle transfer **dropped at 164,766,720 bytes
  (42%)** with `lost connection`; `git clone` refused it (`error: index-pack died`). A resumable
  rsync retry was still running at plan close. **No HEAD snapshot was substituted, no registry
  copy fetched, no other version measured.** OpenClaw contributes zero comparatives; its absence
  makes Wayland's position *less* evidenced, not better.

## Peers, read-only

Hermes clone resolved to `dbe734beff0caf5e8ee2acbe4277db7f6cf84a21`, `PIN_MATCH=YES`,
`version = "0.17.0"` at the pin. **Both reference checkouts gate-checked clean and unmoved**;
only `git bundle create` (read-only) touched them.

**Correction to the plan's own premise, measured at the pin:** the plan asserts Hermes uses
`HERMES_BASE_URL`. It does not — 0 hits across `agent/`, `hermes_cli/`, `run_agent.py`. The real
override is `OPENAI_BASE_URL` (`agent/auxiliary_client.py:1946`, `hermes_cli/auth.py:191`), plus a
`~/.hermes/config.yaml` naming the custom provider, without which it made **zero** fixture requests.

## Deviations, all recorded and all strengthening

1. **Four verdict states, not three** (panel; the plan said three). Both added states are
   non-directional, so the hard zero rule is preserved and strengthened.
2. **SplitMix64 rather than `rand::StdRng`** — StdRng's stream is not guaranteed stable across
   `rand` releases, so seeding it would not deliver the reproducibility the protocol promises.
3. **Own hetzner worktree `/root/wayland-30-02`** rather than the plan's `cd /root/wayland` —
   five lanes share that checkout and `git checkout --detach` there would yank another lane's tree.
4. **Inactivity timeout observes fixture requests only**, not stdout/stderr or workspace mutation.
   Strictly more aggressive, so it can only produce *more* timeouts and cannot flatter any tool.
5. **`base_url_suffix` and `workspace_seed_files`** added as invocation **values**, so per-tool
   differences stay visible in the results instead of hiding in the driver.

## Gates

All Task 1/2/3 local gates PASS: four votes present (unanchored), content address recomputed,
five dimensions mechanical, both peer pins present and one re-resolved, module landed, no
`Option<IntervalV1>`, ≥4 `deny_unknown_fields`, six named tests present, fifteen well-formed legs
= fifteen total, every leg's capture ≥3 lines, protocol commit ancestor of results commit,
results carry the current protocol digest, scope stated, peer checkouts clean.

`cargo fmt --all -- --check` clean. **Authoritative gate:** `wayland-scorecard trials verify`
against the committed protocol and committed results on hetzner → `TRIALS_VERIFY=OK legs=15 run=6
unproven=9 comparatives=3`, rc=0. **`cargo nextest run -p wcore-eval-scenarios --no-fail-fast`:
470 passed, 0 failed, 5 skipped.**

**Clippy is RED, and it is not mine.** 4 errors, all four in Phase 24's `journey.rs`
(683/695/707/717), zero in this lane's four files. Pre-existing; another lane owns that file. Not
fixed, not silenced, not attributed here.

Fence intact: `git diff <merge-base> --name-only` over the meter, the concurrent lanes' files, the
shared CLI files and the lockfile → **0**. This lane changed exactly four source files.

Disk: 750G before provisioning, 742G after. Peer clone and trial workspaces live under `/tmp`.

## Seam request for the orchestrator

`crates/wcore-eval-scenarios/src/fixtures/openai.rs` needs (a) request-body retention under a
redaction policy or leaf-hash exposure, so canary non-exfiltration becomes measurable, and
(b) content-routed rather than FIFO-cursored matching, so a peer whose request order differs is
not penalised. Both change the shared meter and belong to a release-coordinated change.

## The bias this plan does not close

All four panel members named it independently: the fixture is FIFO-cursored and is the dialect
Wayland's own contract tests were built against. The protocol bounds it — a 409 is
`HARNESS_INCOMPATIBLE`, neither success nor failure — but **no trial triggered that state, so the
bound was never exercised.** The bias remains open.

## Nothing was claimed

No claim, no positioning, no requirement marked complete. F30-03 records evidence only; closure
is 30-04's.
