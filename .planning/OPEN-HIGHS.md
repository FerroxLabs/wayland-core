---
lane: open-highs
highs-closed: 2
highs-disproved: 0
highs-still-open: 0
regrades: 3
new-finding: 2
fence-exposure: none
status: complete
---

# OPEN-HIGHS — closing the remaining open HIGH findings

Lane `lane/open-highs`, base `75babf32`. Branch `lane/open-highs`.
Running notes: `.planning/OPEN-HIGHS-NOTES.md`. Evidence: `.planning/evidence/open-highs/`.

Two HIGHs taken, both **closed**. Neither was disproved — both were real. Three grades found
resting on superseded or false measurements, one of which is a **live Sean-facing credential
request that the tree's own later measurement falsifies.**

---

## 1. Zombie liveness — **CLOSED**, macOS binary-level proof taken

The gap was named honestly by the prior lane (`ZOMBIE-PROBE.md` §6): Linux and Windows proven in
Rust on real hardware, macOS proven only at kernel-semantics level, in C. It named the one command
that would close it. Two routes had opened since: the LANE-BRIEF §0 **Darwin-behaviour exception**
(added 2026-07-29, permitting exactly `cargo test -p <crate> --test <file>` on the Mac for
Darwin-only behaviour), and the `sean-mac-arm64` runner.

**Took the Darwin exception**, and disclose it here per §0: `cargo test -p wcore-types --test
real_zombie`, macOS 26.3 arm64, uid 501. Single crate, single test file. No workspace build, no
clippy, no release build on the Mac. I qualify because **no permitted build host executes Darwin
code**, so hetzner could not have proven it.

```
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Five tests, not four.** `real_zombie.rs:349` carries a `#[cfg(target_os = "macos")]` ARM D that
Linux and Windows compile out — **it had never executed on any host**. First run anywhere:

```
ARM D reproduced on Darwin: uid=501 pid=1 new_probe=Live
  old_shape(kill(1,0))=alive:false errno=1 (EPERM=1)
independent oracle for pid 62651: ps state=Z
```

So the old `kill(pid,0)==0` shape called **pid 1, launchd, unambiguously running, DEAD** — the
false-clean direction that makes an orphan reaper believe it has nothing to reap. And a real
corpse reads `Dead`, corroborated by an oracle independent of the code under test.

**Negative control, one variable** — `process_liveness.rs:300` (`== SZOMB` → `false`):
`4 passed; 1 failed`, `left: Live right: Dead`, exactly the corpse test. **The positive-direction
tests stayed green**, so the arm is not universal denial. Reverted, `git diff -- crates/` empty,
re-run `5 passed`.

**Two things I checked rather than assumed:**
- *Could this be a pass on a degraded arm?* The macOS arm degrades to `Indeterminate` on ABI
  drift, and the ABI offsets were measured on an **older** macOS than the 26.3 here. Excluded:
  the corpse assertion demands exactly `Dead`, and `Indeterminate` fails it. So the hardcoded
  offsets still hold on 26.3/arm64 — a new datapoint, not a restatement.
- *Do the production sites call the proven helper?* A working probe nothing calls closes nothing.
  All four delegate to `wcore_types::process_liveness` (`pidlock.rs:315`, `cron.rs:1161`,
  `local.rs:386`, `supervisor.rs:484`). The sweep also found **four further production consumers
  the finding never named** (`gateway.rs:404`, `cron.rs:924`, `channel.rs:217/469`,
  `crash_sentinel.rs:100`, `backup/journal.rs:196`) — more closure, not a new gap, but the
  original blast radius was larger than "4 production sites".

Full detail: `.planning/evidence/open-highs/ZOMBIE-MACOS-CLOSURE.md`. **No residual.**

---

## 2. 23A resurrection hazard — **CLOSED** (the guard; promotion stays with the other lane)

### Re-measured, not inherited

Line numbers had drifted (`bootstrap.rs:2145` → `:2173`); the structure held. Layer 1b hydrates
`seed_pairs_for(&candidate_names, "auto_drafter", 1)` where `candidate_names = catalog.visible()`
(`:2147`), `visible()` filters `!disable_model_invocation` (`refs.rs:129`), and auto-drafts are
quarantined at `loader.rs:448`. **Newly found:** Layer 1b is *additionally* gated on
`config.observability.skills_lifecycle` (`:2172`), which the finding does not mention.

Checked for a second production hydration path (the "sole path had three" trap):
`drafter.rs:511` also calls `seed_pairs_for` bypassing `visible()`, but it is inside a `#[test]`.
Production has one path. That test is still evidence *for* the hazard — it proves the retained row
hydrates to a 4-success prior the moment the name reaches the candidate list.

### The guard could only go in one place, and the dependency graph decided it

Concept sweep of `wcore-skills/src/` for `evolved_prompts|PromptStore|prompt_store|auto_drafter`
→ **0 code hits** (known-positive control, same session: `evolved_prompts` across `crates/` →
**22**). So `GovernanceStore::revoke()` is filesystem-only and the DB row outlives revocation.

It cannot be otherwise: `prompt_store.rs:160-162` records that *"`wcore-skills` cannot depend on
`wcore-evolve` (the dep already runs the other way)"*. Making revocation purge the row would
invert an existing edge. **The only place the two can meet is the bridge that already owns both —
agent bootstrap.**

### Built: `drop_revoked_auto_draft_seeds` (`bootstrap.rs`, commit `a3b4d289`)

Filters the Layer 1b **pairs**, not `candidate_names`. Filtering candidates would be simpler and
wrong — it would strip a user's own same-named skill out of Layer 1 and Layer 2 seeding too,
punishing them for a collision with something they deleted. Only the stale prior is dropped.
Fails **open** on an unreadable governance root, matching `is_revoked`'s documented posture
(failing closed would let one bad file disable the learn loop), and logs every drop.

```
GREEN     3 passed; 0 failed; 0 ignored; 2175 filtered out    WLGUARD=0
MUTATED   1 passed; 2 failed; 0 ignored; 2175 filtered out    WLRC=101
CLIPPY    -p wcore-agent --all-targets -D warnings            WLCLIPPY=0
```

`2175 filtered out` is the anti-vacuity read-back: the filter matched 3 real tests, so flavour (c)
is excluded. **Negative control** (`revoked.contains(name)` → `false`, the pre-fix shape, applied
by `sed` — no git ops on the shared store): both drop assertions redden with the revoked seed
visibly surviving (`got [("auto-revoked-one", 4), ("hand-written-keeper", 3)]`), while the
**no-revocation control test stays green**, proving the mutation did not simply break everything.
Reverted and verified with `git diff --quiet` → `WLDIFF=0`, byte-identical to the committed blob.

### Honest scoping of what this guard is worth

**It is inert today** — auto-drafts are quarantined out of `visible()`, so nothing reaches it. It
becomes live the moment governed promotion lifts the quarantine.

**I overstated one variant in my working notes and am correcting it here.** I wrote that the
name-collision variant "needs no promotion at all". That is literally true — `seed_pairs_for`
matches `WHERE skill_name = ?1` with no signature or provenance — but drafts are named
`auto-<signature>` (`drafter.rs:113`), so a collision requires a user to hand-write a skill with
that exact `auto-` prefixed name. **That is improbable, and I should not bank the finding on it.**
The honest statement: the guard's value is defence-in-depth for the promotion moment, plus a cheap
close of an improbable-but-real collision path.

### Coordination — a divergence, reported not raced

`lane/23a-c1-governed` @ `098c2eb9` had committed **only** its NOTES, no code. Its plan claims the
hazard (*"promotion is the path I am building. So closing this is mine"*). **I did not touch
promotion, `ProcedureStatus`, the loader, or the CLI surface — all theirs.**

**But its named mechanism is a different store from the graded finding's, and prior measurement
found its store inert.** Its chain is `Procedure` row → promotion materialises directory →
`seed_from_prioritizer` (Layer 2). That is store **(c)** in `23A-C1-NOTES.md` §6.3, which that same
file measured as inert: *"No path materialises a `Procedure` into an on-disk skill or executes
one."* Its notes never mention `evolved_prompts`, `seed_pairs_for` or Layer 1b — store **(b)**,
the one §6.3 measured as gated shut *only* by the quarantine their work lifts.

This is a **recurrence of a recorded error**. `CROSS-AUDIT.md` Q4 already records that 2/3 of the
panel ranked the `Procedure` row top risk and that measurement showed *"the panel names the wrong
database"*. The concurrent lane has independently re-selected the wrong database.

Their guards (refuse revoked name at promotion, loader enforcement) are directory-level and would
cover their variant. They do not cover a name-keyed DB row. **The two halves are complementary,
not duplicative — both are needed.**

---

## 3. Grades resting on a superseded or false instrument — three found

### 3a. NEW FINDING — MILESTONE-RC's credential ask is falsified by the tree's own later measurement

`MILESTONE-RC.md` §5 item 6 asks Sean for *"A REAL PROVIDER CREDENTIAL for the durability
harness"*, calling `23B-H1` *"currently unmeasurable by anyone"* and **"the single highest-value
credential you can supply."** That is a live, Sean-reserved ask in the document the handoff calls
"the plan — read it first".

Dated by `git log -S`, unproxied:

| when | what |
|---|---|
| 2026-07-29 **03:15:48** | `2e064325` writes "unmeasurable by anyone" into `MILESTONE-RC.md` |
| 2026-07-29 **06:42:43** | `1d482250` — *"reach achieved — 34 live runs, 69 tool events"* |
| 2026-07-29 **07:32:05** | `94831e12` — *"92 reach-proven runs, 0 reproductions"* |

The measurement **used a real provider key**, piped Mac→hetzner over ssh stdin
(`23B-H1-MEASURE-NOTES.md:77-79`). Reach markers are non-zero across ~95 runs
(`F23_H1_REACH=1..12`), with only 2 empty. `BACKLOG.md` already carries the corrected disposition
(MEDIUM, non-reproducing). `MILESTONE-RC.md` was edited again at 09:02 (`d0fcf1f0`) but only to
correct the Desktop digest pair, so §5.6 was never revisited.

**So the credential was obtained, the defect was measured, and the ask is stale by ~3.5 hours.**
Per HANDOFF §4 six "blocked on Sean" claims have already been falsified; this is a seventh, and it
is the one currently labelled highest-value. **Recommend striking `MILESTONE-RC.md` §5 item 6.**

Note the inversion, which is the actual answer to the brief's question: `BL-23B-H1`'s **MEDIUM
grade rests on a live instrument** (`f23-h1-repro-live.sh`, which emits `F23_H1_REACH=` so a
non-reaching run can never again be counted as a pass). It is the **HIGH claim** that rests on the
dead one — the old harness pointed at `127.0.0.1:1` with a placeholder key.

### 3b. NEW FINDING — two Phase 29 MEDIUMs were closed by their own owner and never struck

`F29-CEN-13` (*"Zero occurrences of `revoke|revocation|crl|blocklist|denylist` in
`self_update.rs`"*) — **re-measured, the stated count is 2, not zero.** Known-positive control in
the same invocation: `tag_name` → 2 hits, so the instrument is alive.

`F29-CEN-12` (*"Zero occurrences of `expires|expiry|timestamp|…`"*) — now **8** in
`self_update.rs` and **14** in `update_trust.rs`.

Both are owned by "29-03", and `crates/wcore-cli/src/update_trust.rs` landed
**2026-07-28 08:56** (`e5952311`, *"an ordered, fail-closed update decision in the shipped
updater"*) — about 10.5 hours after the findings were written (2026-07-27 22:22, `34866767`). It
provides `check_only_report`, `decide_update`, `FreezeState`, `DEFAULT_MAX_MANIFEST_AGE_SECS`, and
its own doc comment says *"persisted freeze protection and revocation enforcement"*.

**The findings were correct when written and were closed by their named owner.** They are not a
dead-instrument case; they are the findings-leak class running in reverse — closed work still
presenting as open, inflating the remaining-work picture. **Recommend striking both.**

### 3c. REGRADE — `F23A-C1-M1` is graded MEDIUM in one place and HIGH in another

`SEAM-REQUESTS/23A-C1.md` grades it `MEDIUM`; `HANDOFF-2026-07-29.md` §3 lists it as an open
**HIGH**. The MEDIUM rests on "can never fire today", which is true only while the quarantine
holds — and lifting that quarantine is work actively in flight in a parallel lane. I treated it at
HIGH and closed it. **Recommend the HIGH grade stand** until promotion ships with its half.

---

## 4. What I did NOT do — stated plainly

- **Did not touch promotion, `ProcedureStatus`, the loader, or the skills CLI surface.** All owned
  by `lane/23a-c1-governed`. I built only the Layer 1b guard and reported the divergence.
- **Did not dispatch a CI job to `sean-mac-arm64`.** I read its state (`online`, `busy:false`) but
  the Darwin exception made the direct run cheaper and more direct, and dispatching would have
  needed a workflow I am fenced from touching. The runner remains unreconfigured and unstopped.
- **Did not touch `.github/workflows/*`**, `crates/wcore-cli/src/lib.rs` or
  `crates/wcore-cli/src/main.rs`. Verified against the **merge-base SHA**, not the branch name:
  `git diff 75babf32 --stat -- <fenced paths>` → empty.
- **Did not weaken, `#[ignore]`, re-gate, delete or re-time any test.** Test count went up by 3.
- **Did not strike the stale BACKLOG / MILESTONE-RC entries myself.** They are cross-lane planning
  documents and edits there collide; recommendations recorded here instead.
- **Did not fix the pre-existing `wcore-agent --lib` failures** (see §5). Not this lane's subject.
- Did not merge, open a PR, tag, publish, close an issue, or run `wcore-contract generate`.

---

## 5. One red I am reporting as red

`cargo test -p wcore-agent --lib` is **failing, and it fails at base too**:

| commit | result |
|---|---|
| lane `a3b4d289` | `2164 passed; 11 failed; 3 ignored; 0 filtered out` |
| base `75babf32` | `2152 passed; 20 failed; 3 ignored; 0 filtered out` |

Nine of my 11 appear in base's 20; base carries 11 my run did not. The sets **overlap heavily and
differ in both directions, and base is strictly worse** — the signature of the load-dependent
flake class LANE-BRIEF §6 already documents. Nearly all are `session journal writer lease is
already held`, plus injected-crash tests. My change is a pure filter over router seed pairs and
holds no journal lease.

**I did not assume that** — I ran the identical suite at base in a separate hetzner worktree as a
one-variable control, which is the only reason I can say it rather than argue it.

**Residual, stated rather than hidden:** the two runs were taken at different times, so background
load from the other live lanes was not held constant. A same-instant A/B would be stronger. What
is established is that this suite is **already red at base** and that my change does not make it
redder.

---

## 6. For the orchestrator to serialize

- **One production file:** `crates/wcore-agent/src/bootstrap.rs`, **additive only** (+182, −0).
  One call-site line inside the existing Layer 1b block, two new free functions, one test module.
  No new dependencies — `wcore-agent` already depends on both `wcore-skills` and `wcore-evolve`.
- **Fence exposure: none.** Verified against `75babf32`.
- **Complementary to `lane/23a-c1-governed`, not competing** — different store, different layer.
  If both land, the DB-row half and the directory half are both covered. **If only theirs lands,
  the `evolved_prompts` row is still unguarded.**
- **Two planning-document edits recommended but not made** (§3a, §3b): strike `MILESTONE-RC.md`
  §5 item 6, and strike `F29-CEN-12` / `F29-CEN-13` from `BACKLOG.md`.

Hetzner worktrees `/root/wayland-open-highs` and `/root/wayland-oh-base` removed with their
`target/` dirs.
