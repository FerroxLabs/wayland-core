# 0004 — A cross-audit panel must be shown prior decisions on the same surface

Date: 2026-07-30
Status: Proposed

Companion to [ADR 0003](0003-durable-sessions-without-a-secure-store.md), which is the worked
example: the same question was decided in opposite directions five days apart because the second
decision-maker was never shown the first.

This file is the process fix. It is written to be adopted, so it is short and every rule in it has
been executed at least once against the real case.

---

## 1. The failure, stated precisely

On 2026-07-16, `906287e1` chose **refuse** for durable sessions with no secure credential store,
and encoded that choice in a test.

On 2026-07-30, a three-model cross-audit panel voted **2-1 to degrade**. The panel was not shown
the July-16 decision. Measured: zero references to it in any of the panel's materials, with a
known-positive control confirming the search was alive.

Worse, the question put to the panel described refuse as a *"Rejected alternative"*. The incumbent
behaviour — deliberately chosen, with a passing test — was presented to the panel as an option
someone had already discarded.

**Neither lane was careless.** The 2026-07-30 lane ran a four-quadrant live proof, falsified its
own first answer, probed every panel leg alive before counting its vote, conceded a rider to the
dissent and built for it, and listed five things it had *not* proven. It still produced an
oscillation, because the one thing it could not do was know the earlier decision existed.

**So this is a discovery problem, not a diligence problem.** A rule that says "be thorough" fixes
nothing here. The rules below are all mechanical.

---

## 2. What a panel must be shown before its vote counts

A cross-audit vote on a **behaviour posture** — refuse vs degrade, fail-closed vs fail-open,
accept vs reject, strict vs lenient — does not count unless the question itself contains:

1. **Every prior decision on the same surface**, with its verbatim stated reason. Not a link — the
   text, inline. Panel legs are one-shot CLI invocations; they cannot follow references.
2. **The evidence behind each prior decision**, including the test or measurement that encodes it.
3. **The evidence motivating the change**, in equal detail. A panel shown only the prior decision
   will vote to keep it and re-break whatever the change was fixing. In ADR 0003's case, a panel
   shown only §2 votes REFUSE and kills every headless Linux install.
4. **Neutral framing.** Do not label any option "rejected", "proposed", "the fix", or
   "the alternative". Present the options as peers with their evidence attached. The 2-1 split in
   ADR 0003 was taken on a question that pre-labelled one option as already rejected.
5. **The output of the §3 discovery search**, pasted verbatim — including when it returns nothing.
   "I searched and found no prior decision" is a claim a reader can check; silence is not.

If a prior decision is found and the panel still votes to reverse it, that is a fine outcome —
**record the reversal in the ADR as a supersession**, with both reasons. The failure mode is not
reversal. It is reversal by a voter who did not know there was anything to reverse.

---

## 3. Discovery — the hard part

A rule nobody can execute is not a rule. So this section is ordered by how little discipline each
tier requires, and each tier states what it misses.

### Tier 1 — the test suite is the decision index, and it already exists

The July-16 decision left exactly one durable artifact in the codebase: a test. That is typical.
Prose gets stale and is not run; a test is a decision that re-asserts itself on every build.

So the highest-leverage mechanism is not a new index anyone has to maintain. **It is running the
tests that already encode the decisions.** A behaviour change that contradicts a prior decision
then announces itself as a failing test, with no one needing to remember, search, or care.

**Why it did not fire here — measured, and not what the brief assumed.**

The claim I was given was that the merge cadence "runs no tests and no clippy". **That is false.**
The 2026-07-30 lane's recorded gates at `551d9001` were:

```
cargo metadata --locked  0 · fmt --check  0 · check --workspace --all-targets  0
test -p wcore-config --lib                        579 passed / 0 failed / 0 ignored / 0 filtered
test -p wcore-agent (2 targets)                   2 + 3 passed, 0 ignored, 0 filtered
test -p wcore-cli --test json_stream_startup_refusal   6 passed
clippy scoped to all authored code                0, 0, 0
clippy --workspace --all-targets -D warnings      101 — PRE-EXISTING, proven
```

Tests ran. Clippy ran. The real mechanism is finer, and more interesting:

- **Every test that ran was scoped to a crate the author changed** (`wcore-config`, `wcore-agent`),
  plus one integration target the author hand-picked because they knew it was about refusal.
- **The broken test lives in `wcore-cli`, whose source the change never touched.** Nothing in a
  change-scoped test selection reaches it.
- **The only workspace-wide gate was `cargo check --workspace --all-targets`, which compiles
  everything and runs nothing.** A compile-only gate cannot observe a behavioural contradiction —
  the test compiled perfectly.

So the gap is not "no tests". It is **no workspace-wide gate that executes**. A change in crate A
silently invalidated a decision encoded in crate B, and the cadence had no step that could notice.

**Proposal:** a workspace-wide `cargo nextest run` at merge, **diffed against a recorded baseline
of known-red tests**, not against zero.

The baseline is not optional bookkeeping — it is what makes the gate able to *pass*
(LANE-BRIEF §3b-iii). Two things in this repo make an absolute-zero gate permanently red and
therefore worthless: `clippy --workspace --all-targets` already exits **101** on pre-existing
findings, and the workspace suite has documented contention failures under parallel lane load
(~20 `wcore-skills` EMFILE watcher tests; the same suite is 669/669 in isolation). A gate that is
always red teaches everyone to scope down, which is exactly how we got here. A **delta** against a
committed baseline can both pass and fail, which is the only kind of gate worth running.

**What Tier 1 misses:**

- Tests that do not run. There are **250** `#[ignore]` occurrences under `crates/` (measured,
  `/usr/bin/grep -rn`, with both controls). Each is a decision whose guard is switched off.
- **Platform-gated tests.** The test in ADR 0003 is `#[cfg(target_os = "linux")]`. A merge gate
  running only on macOS would never have compiled it, let alone run it. Any single-platform gate
  has a false-negative rate equal to the share of decisions encoded on other platforms.
- Env-gated tests with early returns, and name filters that match nothing — both are documented in
  this repo as suites that exit 0 having run zero tests.
- Decisions never encoded in any test at all. **I cannot quantify this fraction and will not
  guess.** It is the dominant unknown in this proposal.

### Tier 2 — when a test goes red, find its decision before touching it

Triggered, one command, deterministic. **Before changing, re-pointing, `#[ignore]`-ing or deleting
a test that your change turned red:**

```
git log -S "<test function name>" --format='%h %ad %s' --date=short -- <path/to/test file>
```

Executed against the real case:

```
$ git log -S "isolated_profile_without_secure_store_fails_before_turn_or_provider_intent" \
      --format='%h %ad %s' --date=short -- crates/wcore-cli/tests/f14_sigkill_recovery.rs
906287e1 2026-07-16 feat(recovery): seal interrupted turn state
```

One line. The exact commit. Its body carries the reasoning verbatim — *"fail closed instead of
replaying ambiguous effects."* No index, no registry, nothing to maintain, and it works on any
repo with intact history.

Then read that commit body and decide explicitly: **am I reversing this?** If yes, write the
supersession into an ADR before merging. If the body is uninformative, that is itself the signal
that an ADR was owed and never written.

**What Tier 2 misses:**

- **A renamed test.** `-S` keys on the literal string. Mitigation: re-run `-S` on a distinctive
  assertion string from the test body (an error message, a sentinel constant), which survives
  renames better than a function name does. Or `git log --follow -L :<fn>:<file>`.
- **A squashed or rewritten history**, where the reasoning was compressed out of the commit body.
- **An uninformative commit body**, which is common and is the failure this repo's commit-message
  conventions exist to prevent.
- It only fires **if the test ran and went red** — so it inherits every Tier-1 false negative
  above. Tier 2 is a follow-on to Tier 1, not an independent net.

### Tier 3 — write the ADR, so the next search has something high-precision to find

Tiers 1 and 2 are high-recall and low-precision: they surface *a commit*, and you have to
reconstruct the reasoning from it. An ADR is the opposite — low-recall (only exists if someone
wrote it) and high-precision.

**Rule:** a change that argues for a **behaviour posture** in its commit body — refuse vs degrade,
fail-closed vs fail-open, strict vs lenient — owes an ADR in `docs/decisions/`. If you find
yourself writing a paragraph justifying *which way the product should fail*, that paragraph
belongs in `docs/decisions/`, not only in a commit body where the next person will not find it.

Note that `906287e1` would have qualified under this rule and did not write one. **That is the
honest measure of Tier 3's false-negative rate: the one case we know about, it would have
missed** — unless the rule is enforced by something other than memory. Candidate enforcement: the
Tier-1 workspace gate also fails when a commit body matches a posture-word pattern and no file
under `docs/decisions/` changed. That is cheap and mechanical, and it is the only version of
Tier 3 I would claim actually works. I have not built it.

### Honest summary of the false-negative rate

| tier | catches | requires | misses |
|---|---|---|---|
| 1 — run the workspace suite | any decision encoded in a *running* test, automatically | infrastructure, a red baseline, CI time | 250 `#[ignore]`s, platform-gated tests, env-gated returns, untested decisions |
| 2 — `git log -S` on a reddened test | the introducing commit, exactly | one command, at a moment you are already stopped | renames, squashes, thin commit bodies; inherits all of Tier 1's misses |
| 3 — write the ADR | the reasoning, in full, findable by title | discipline, or a mechanical check nobody has built | everything nobody wrote down — including the case that produced this ADR |

**Nothing here catches a decision that was never encoded in a test and never written down.** That
class is invisible to every mechanism I can propose, and I do not know how large it is. Anyone
adopting this should know that up front rather than discover it later.

---

## 4. `.planning/LANE-BRIEF.md` §7.4 — a required deliverable the harness forbids

**Two lanes hit this on 2026-07-30 and both correctly flagged it rather than routing around it.**

§7.4 requires:

> 4. Write `<PLAN-ID>-SUMMARY.md` next to the PLAN file: what landed, what the gates showed, the
>    live evidence, deviations with reasons, anything still open, and an honest verdict on whether
>    the plan's criteria were met.

The lane harness hard-blocks writing summary/report/findings files. So §7.4 instructs lanes to do
something they are mechanically prevented from doing. A rule that cannot be complied with trains
lanes to treat the brief as advisory, which is expensive here — the rest of the brief is load-bearing.

The content is still wanted. Only the **file** is blocked, and only under names the harness treats
as report-shaped. The two mechanisms that do work are the NOTES file §6b-i already mandates (which
is committed, survives lane death, and is where the measurements accumulate anyway) and the final
report message.

**Proposed replacement for §7.4:**

> 4. **Record the plan's outcome in your committed NOTES file** — `<PHASE>-NOTES.md` in your
>    evidence directory, the same file §6b-i requires you to have been appending to since minute
>    15. Add a final `# OUTCOME` section: what landed, what the gates showed with real numbers,
>    the live evidence, deviations with reasons, anything still open, and an honest verdict on
>    whether the plan's criteria were met.
>
>    **Do not create a separate `SUMMARY.md`.** The harness blocks report-shaped files, and a
>    required deliverable that cannot be written is worse than none — it teaches lanes that this
>    brief is advisory. The NOTES file is strictly better anyway: it is already committed, it
>    already survives lane death, and it holds the measurements in the order they were taken.
>
>    Your §8 report message is a summary *of* that section, not a substitute for it. A report
>    message is not committed and does not survive the session.

Precedent for the shape: `.planning/evidence/fix-headless-keyring/NOTES.md` already ends with a
`# OUTCOME` section carrying exactly this content, and it is the most useful artifact that lane
produced.

---

## 5. Adoption checklist

- [ ] `LANE-BRIEF.md` §7.4 replaced with the wording in §4 above.
- [ ] `LANE-BRIEF.md` §4 (cross-audit panel) gains the five preconditions from §2 above.
- [ ] Workspace `cargo nextest run` added at merge, **as a delta against a committed
      known-red baseline**, with the baseline file checked in and dated.
- [ ] Tier-2 command (§3) added to the brief wherever it tells a lane what to do about a red test.
- [ ] Decide whether to build the mechanical Tier-3 check, or accept that Tier 3 is
      discipline-only and record that choice.
