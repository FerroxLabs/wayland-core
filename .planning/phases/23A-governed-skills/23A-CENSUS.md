# 23A-CENSUS — the 16-route quarantine census, measured at HEAD

**Lane:** `lane/23a-census`
**Measured at:** `8bcb052b2aa6b1a9e3f2ed00af935a58c92c1f11` (= `plan/f20-unified-audit-repair` at fetch),
driven from lane commits `159682e9` and `7b5ee047`.
**Machine:** `hetzner-dsm`, worktree `hz/23a-census` at `/root/wayland-23a-census`.
**Predecessor census:** `23A-01-SURFACE-CENSUS.md`, written at `2ecdfdf5` (2026-07-26).

---

## Termination state

**MEASURED — with a coverage limit that the predecessor census did not state.**

Three things were established and one was refuted.

1. **The driver runs at the fixed HEAD, and the two F23A-01-H2 reproducers pass.**
   `481682b0` committed them RED on 2026-07-26; the fix `32a5fc90` landed
   2026-07-27; nothing had run them since. They pass now: `4 passed; 0 failed`.

2. **The `WAYLAND_F23A_SELFTEST` control fires.** It had never been shown to.
   Proven by a two-run differential whose runs DISAGREE, not by inspection.

3. **The fix does not "cover" fifteen of the sixteen routes, and was never
   supposed to.** See §5 — this is the question in the brief and the honest
   answer is that the frame does not apply.

4. **REFUTED: "sixteen routes, zero UNGATED" is not a measurement of sixteen
   routes.** Four are driven end-to-end through the shipped binary. Ten are
   verified by re-resolving their citations at HEAD and are not driven. Two are
   unreachable by construction and there is nothing to drive. The predecessor
   census does not distinguish these and its closing line reads as though it
   does.

No new HIGH. Two instrument defects found and repaired in-lane (§4). Three
citation defects found in the predecessor census, none of which changes a
disposition (§6).

---

## 1. What was run

Built and run on `hetzner-dsm` only; nothing was compiled on the Mac
(`cargo fmt --all -- --check` is the sole Mac exception and it is clean).

```
export PATH=/root/.cargo/bin:$PATH
export WAYLAND_BUILD_SOURCE_SHA=$(git rev-parse HEAD)
cargo build --locked -p wcore-cli --bin wayland-core              # WLRC=0
export WCORE_EVAL_BIN=/root/wayland-23a-census/target/debug/wayland-core
cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate \
  --test f23a_boundary_drive -- --nocapture --test-threads=1
```

Run **by file** (`--test f23a_boundary_drive`), never by a name filter — flavour
(c) of the zero-tests trap is a command that looks targeted and executes
nothing. Every count below is read back from the `N passed` line; the exit
status is recorded but never relied on. Each run wrote `WLRC=<code>` first and
`WLDONE` last to a status file read back by a separate `ssh` call.

`cargo clippy --locked -p wcore-eval-scenarios --features packaged-driver-gate
--tests -- -D warnings` → `WLRC=0`.

The file is `#![cfg(feature = "packaged-driver-gate")]`, so omitting that
feature yields a target containing **zero** tests that exits 0. The executed
count is the only thing that distinguishes a real run from that one.

---

## 2. Per-route results

Grades:

- **LIVE-DRIVEN** — exercised end-to-end through the shipped binary at HEAD,
  with a positive control that must succeed and a substitution control that is
  measured to make the check fail.
- **STATIC@HEAD** — the gate cited by the predecessor census was re-resolved in
  the tree at `8bcb052b` and still exists. Not driven.
- **UNREACHABLE@HEAD** — no production caller or constructor exists at HEAD, so
  there is nothing to drive.

`Citation` is the line **at HEAD**, not the line the predecessor census printed;
where they differ the census's number is given in parentheses.

| # | Route | Grade | Citation at HEAD | Result |
|---|-------|-------|------------------|--------|
| R1 | `Skill` tool call | **LIVE-DRIVEN** | `skill_tool.rs:187`; gate `wcore-skills/src/refs.rs:290` | Refused, `is_error=true`, no body disclosure, draft body absent from output. Control skill resolved and its nonce reached the model. Substitution makes the check fail (`refused=false`). |
| R2 | System-prompt skill listing | STATIC@HEAD | `context.rs:325`, filter `:327` | Filter present and exact. **Not driven** — see §3. |
| R3 | Router candidate seed pool (bootstrap) | STATIC@HEAD | `bootstrap.rs:2119` (was 2052); consumed at `:2165` | `catalog.visible()` still the sole source of `candidate_names`. |
| R4 | Per-turn router candidate pool | STATIC@HEAD | `engine.rs:8671` (was 8622) | Same `catalog.visible()` iterator. |
| R5 | Per-turn router hint line | STATIC@HEAD | `engine.rs:5350`, gate `:5351` (was 5311/5312) | Independent re-check on `disable_model_invocation` still present, still returns `None`. |
| R6 | `/skill run <name>` | **LIVE-DRIVEN** | `slash/skill.rs:114`, text `:117` | `"this skill is quarantined and cannot be run."` observed in the live info stream. Substitution makes the check fail. |
| R7 | `/skill list` | **LIVE-DRIVEN** | `slash/skill.rs:140`, tag `:148` (was 147) | Draft listed and tagged `(hidden)` **on its own rendered line**. This is the check that was self-passing before this lane — see §4b. |
| R8 | `/skill show <name>` | **LIVE-DRIVEN** | `slash/skill.rs:172`, `:193` (was 173) | `visibility: hidden from model` observed; body not rendered. Substitution makes the check fail. |
| R9 | Cron skill sink body scan | STATIC@HEAD | `cron.rs:374`/`:377`, `bootstrap.rs:3467`/`:3470` (was 262/3260) | Still composes-and-discards; dispatch is still R1. |
| R10 | Skill-declared hooks | STATIC@HEAD | `orchestration/mod.rs:3144` **and `:677`** (census cited only 3086); `skill_tool.rs:434` | Both merge sites are guarded by `!block_is_error`. See §6a — the census enumerated one of two. |
| R11 | Skill-declared MCP servers | UNREACHABLE@HEAD | `wcore-skills/src/mcp.rs:23`, called from `loader.rs:144` | Direction is MCP → skills. No skills → MCP registration path exists. |
| R12 | Conditional path-glob activation | UNREACHABLE@HEAD | `conditional.rs:39`, `:54` | Grep for `ConditionalSkillManager::` excluding test files returns **empty** at HEAD. Nothing constructs it. |
| R13 | Artifact materialisation | STATIC@HEAD | `skill_tool.rs:260` → `artifacts.rs:33` | Downstream of the R1 return; a quarantined name never reaches it. Covered transitively by the R1 live drive. |
| R14 | Shell composition for execution | STATIC@HEAD | `wcore-skills/src/executor.rs:20`, `:65` | Only spawn-leading caller is inside `SkillTool::execute`, downstream of R1. Covered transitively by the R1 live drive. |
| R15 | Session-start prioritizer reordering | STATIC@HEAD | `bootstrap.rs:1908` (was 1815) | Ordering only. **The census's stated input is wrong** — see §6b. |
| R16 | Cross-project sibling resolution | STATIC@HEAD | `refs.rs:290` branch, re-checks `:313`, `:325`, `:334` | Sibling skills load through `load_skills_from_dir`, which runs `is_generated_draft` (`loader.rs:447-449`), so a sibling draft arrives already quarantined. **Plus an additional gate the census omits** — see §6c. |

**Live-driven: 4. Static-verified at HEAD: 10 (2 of them transitively covered by
the R1 drive). Unreachable by construction: 2.**

No route changed disposition. Every gate the predecessor census cited still
exists at HEAD.

---

## 3. The routes that are not driven, and exactly what stops it

The highest-value undriven routes are R2 and R5 — the two that put content into
a **model-visible payload**, which is the census's own stated boundary. Driving
them means asserting that the draft's name and body are absent from the request
bodies the binary actually sent, with the user-authored control's name present
as the positive control.

**That is not reachable with the harness as it stands, for a specific reason.**
`OpenAiFixtureScript` records `FixtureRequestRecord`
(`crates/wcore-eval-scenarios/src/fixtures/openai.rs:311-321`), which retains
`body_sha256`, `semantic_body_sha256` and per-leaf hashes — and nothing else.
The comment at `:317-318` states the intent: *"without retaining or printing
request content."* Hashes cannot be searched for a nonce.

This is a deliberate design property of a fixture shared by many other targets,
so changing it is a seam decision and not a census lane's call. Recorded as a
limit with its cause, per LANE-BRIEF §3.2 ("a route the harness cannot reach is
a RECORDED LIMIT, never a silent drop"). **What it would take:** an opt-in
retention mode on the fixture (off by default, enabled per-scenario), returning
request bodies to the caller. Roughly one shared file, plus one assertion pair
in the drive target.

R9 (cron sink), R10 (hooks) and R16 (cross-project) each need environment the
drive target does not construct today: a scheduled cron entry, a skill whose
execution *succeeds* so the hook merge is even attempted, and a
`cross_project_root` with a sibling carrying `memory.db`. R10 in particular
cannot be driven at a quarantined draft at all — the draft's `execute` always
returns `is_error: true` via R1, which is the very condition that makes the
merge unreachable. **A route that is unreachable by construction cannot be
live-driven; asserting it live would be theatre.** Both facts are recorded
rather than dressed up.

---

## 4. The selftest differential, and the two instrument defects it exposed

### 4a. The differential

Four runs. Runs **A/B** are the instrument as it stood; **E/F** are the repaired
instrument. Counts read back from `N passed`.

| Run | Commit | `WAYLAND_F23A_SELFTEST` | Executed count | `WLRC` |
|-----|--------|-------------------------|----------------|--------|
| A | `dedd13d7` | unset | `3 passed; 0 failed; 0 ignored; 0 filtered out` | 0 |
| B | `dedd13d7` | `refusal` | `2 passed; 1 failed; 0 ignored; 0 filtered out` | 101 |
| E | `7b5ee047` | unset | `4 passed; 0 failed; 0 ignored; 0 filtered out` | 0 |
| F | `7b5ee047` | `refusal` | `3 passed; 1 failed; 0 ignored; 0 filtered out` | 101 |

**The runs DISAGREE in both pairs. The control fires.** Run B was the first time
`F23A-SELFTEST-TRIPPED: refusal` has ever been printed; 23A-01-SUMMARY line 111
recorded the switch as *"BUILT, NOT EXERCISED … an unexercised self-test is
precisely the decorative control this plan bans."* That clause is now closed.

The 3 → 4 count change is the self-test added in §4b. The baseline stays green
under a **strictly stronger** R7 matcher — nothing was weakened to reach it.

### 4b. Defect 1 — the differential was worth exactly one assertion

`trip_selftest` returns `!`, and it was called inline at the first route check
that failed. So run B panicked at `/skill run` and **never evaluated the other
three checks at all**. The differential proved the R6 assertion had teeth and
said nothing whatever about R7, R8 or R1.

Repaired (`159682e9`): all four checks are evaluated into a `route_checks` array
before any is acted on, each outcome is printed, and the target fails once at
the end naming any check that is *not* discriminating. Run F:

```
F23A-SELFTEST-ROUTE: R6 /skill run    refused=false
F23A-SELFTEST-ROUTE: R7 /skill list   refused=false
F23A-SELFTEST-ROUTE: R8 /skill show   refused=false
F23A-SELFTEST-ROUTE: R1 Skill tool    refused=false
... checks that still reported a refusal, and are therefore not discriminating: []
```

All four discriminate. The differential is now worth four assertions.

### 4c. Defect 2 — the `/skill list` check was a self-passing gate

The R7 matcher was:

```rust
let listed_hidden = info.contains(&probed_name) && info.contains("(hidden)");
```

Two **unbound** substring searches over the whole joined info stream. The
quarantined `auto-*` draft is always in the catalog and always carries the tag,
so the second conjunct is true for *every* input. The check could not
distinguish a hidden skill from a visible one.

Repaired to bind the tag to the name on one rendered line — `runtime_list`
emits `"  - {name} (hidden) [src={src}]"` (`slash/skill.rs:156-164`) — with a
three-assertion self-test (`list_tags_hidden_matcher_selftest`) whose third
assertion is executable, not prose: *the old matcher reports `true` for the
visible control*. Without that third assertion the self-test passes on the
broken instrument too.

**And the vacuity is measured on the live bytes, not argued.** `7b5ee047` also
prints the pre-repair matcher's verdict on the same live info stream the
repaired check reads. Run F:

```
F23A-SELFTEST-ROUTE:  R7 /skill list refused=false     <- repaired: discriminating
F23A-SELFTEST-LEGACY: R7 /skill list old_matcher=true  <- pre-repair: vacuous
```

The pre-repair check reported *"the quarantined draft is tagged hidden"* while
being handed a user-authored, model-**visible** skill. That is a self-passing
gate of exactly the class LANE-BRIEF §3.2 enumerates, sitting inside the
instrument built to hunt it — another instance of an instrument carrying the
defect class it hunts. Repaired in-lane per §6b-ii rather than written up and
left, because the recorded precedent is that a documented-but-unrepaired
instrument defect recurs in the next lane.

---

## 5. Does the fix cover all sixteen routes?

**The frame does not apply to fifteen of them, and saying so is the honest
answer rather than manufacturing per-route coverage.**

`32a5fc90` is a **journal-terminality** fix on the tool-dispatch path: an
`is_error` result from an opaque tool was left in the journal's `Unknown`
state, which is nonterminal, so the turn could never be committed and the
process exited 1. Its diff touches neither the quarantine classifier
(`loader.rs:463`) nor `disable_model_invocation` nor any of the sixteen route
gates. Verified directly: `git show 32a5fc90 -- crates/wcore-agent/src/orchestration/mod.rs`
grepped for `block_is_error` and `merge_skill_hooks` returns **empty**, so
R10's gate condition is untouched by it.

So:

- **R1 is the one route where the fix's property is load-bearing**, and it is
  measured: a refused `Skill` tool call now leaves the session usable
  (`refused_skill_tool_call_does_not_kill_the_session`, passing at HEAD). Before
  the fix, the security control was itself a denial of service — the refusal
  killed the session, which also destroyed the operator's ability to observe
  what had been refused.
- **R2–R16 never depended on the fix.** They are visibility filters and
  resolution re-checks. Asking whether the fix "covers" them is a category
  error.
- **What the fix actually unblocked is the ability to run the census at all.**
  23A-01 could not exercise its own self-test because the base run was red on
  H2. With H2 closed, the base run is green and the differential became
  possible. That is the whole causal chain, and it is why "the HIGH is fixed"
  and "the routes are measured" are two different statements — which is
  precisely what the record-reconciliation lane declined to conflate, and it
  was right.

**No route is left uncovered *by the fix* in a way that matters. Twelve routes
are left un-driven *by the census*, which is a different and real gap, graded
in §2 and explained in §3.**

---

## 6. Defects in the predecessor census document

None of these changes a disposition. All three are recorded because a census
whose citations are wrong on re-read carries less authority than its closing
line claims, and ten of its sixteen rows rest on citation alone.

**6a — R10 enumerates one of two call sites.** The census cites
`orchestration/mod.rs:3086` as *the* place `maybe_merge_skill_hooks` is guarded.
At HEAD there are two: `:3144` and `:677`. The second is not new — `git log -L`
attributes it to `906287e1` (2026-07-16), ten days *before* the census's own
base SHA. Both are guarded identically by `!block_is_error`, so the gate holds;
the enumeration was incomplete. The census's §3 M1 fragility argument ("two
accidental conditions, no explicit check") applies to both sites and is
therefore slightly stronger than written, not weaker.

**6b — R15 cites the wrong input.** The census states the prioritizer consumes
`iter_names()` (`refs.rs:171`). At HEAD, `bootstrap.rs:1906` builds its input as
`skill_refs.iter().map(|r| r.name.clone())` — the raw ref vector, not
`catalog.iter_names()`. Both are unrestricted, so the disposition
(ordering-only) is unchanged. Worth noting because the comment at
`bootstrap.rs:1902-1903` says the reorder *"flows through the system_prompt
below"* — which is what makes R2's filter (`context.rs:327`) load-bearing rather
than redundant.

**6c — R16 omits a gate, in the safe direction.** Beyond the
`disable_model_invocation` re-checks the census cites, `resolve_cross_project`
refuses any sibling skill that is not prompt-only
(`refs.rs:381`, `skill_is_prompt_only`), logging *"ignored executable
sibling-project skill without independent workspace trust"*. The route is more
gated than the census claims.

**6d — every line number outside `slash/skill.rs`, `skill_tool.rs`,
`context.rs`, `executor.rs` and `mcp.rs` has drifted**, several by hundreds of
lines (R4 by 49, R5 by 39, R15 by 93, R9 by 112 and 207). A census citing bare
`path:line` at a SHA it does not pin per-row degrades silently. Recommend future
censuses cite a symbol plus a line, so re-resolution is mechanical.

---

## 7. Plan-versus-tree divergences the orchestrator should know about

These are not defects in the product. They matter because **`23A-04-PLAN.md`
line 150 lists two files in its `<read_first>` that have never existed**, so an
executor picking up 23A-04 will fail its first instruction.

| Artifact | State at HEAD |
|----------|---------------|
| `crates/wcore-eval-scenarios/src/governed_skill_drive.rs` | **Never committed.** `git log --all` on the path is empty. 23A-01-SUMMARY line 107 records it NOT WRITTEN; 23A-02 and 23A-04 both still `read_first` it. |
| `scripts/f23a-boundary-drive.sh` / `.ps1` | **Never committed.** `git log --all` empty; no `f23a-*` file exists in `scripts/`. 23A-01-SUMMARY line 109 records them NOT WRITTEN. |
| `WAYLAND_EXPECT_SHA` mismatch → exit exactly 3 | **Still not built.** It lives only in the unwritten wrappers. The SHA pin in this lane was done by `git reset --hard` to a named commit in a dedicated worktree and read back with `git rev-parse`, which is weaker than an in-band assertion. |
| Refusal probes in `packaged_driver_gate.rs` | **Still not added.** That file remains byte-identical. |

23A-01-SUMMARY was scrupulously honest about all of these. Nothing here
contradicts it; it is restated because two downstream plans still point at the
missing files.

---

## 8. Cross-audit

Question put to the panel: whether the twelve un-driven routes should be graded
a NEW HIGH (unproven live) or static-verified with a recorded coverage limit.

| Panellist | Position |
|-----------|----------|
| `codex exec -m gpt-5.6-sol` | `PANEL_POSITION=B` — gates re-resolved at HEAD; absent dynamic proof is not itself a vulnerability. |
| `gemini -m gemini-3.1-pro-preview` | `PANEL_POSITION=B` — the four live routes validate the gate mechanism at runtime; the rest is a coverage boundary. |
| `kimi` | `PANEL_POSITION=B` — grading them HIGH would conflate "verified statically, not yet exercised" with "unmitigated exposure". |

**Internal adversarial pass, arguing FOR A:** Phase 20A reached CI-green on two
platforms by exactly this kind of reasoning and a later live pass found three
HIGH defects in that same build; §3.1 exists because reading code is not
measuring behaviour. Worse, the base rate of "looks gated on paper" inside *this
very artifact* is not low — three of sixteen rows were mis-cited on re-read
(§6), and the single route where a live check existed and was examined closely
(R7) turned out to be self-passing.

**Disposition: B, unanimously, and the adversarial pass is adopted as a separate
correctly-scoped finding rather than dismissed.** A HIGH under LANE-BRIEF §5
"must be fixed, or disproved with executable evidence" — and there is no defect
to fix here, because no bypass was produced; the only available action *is* to
drive the route, which is the coverage limit already recorded. What the
adversarial pass does establish is that the predecessor census's confidence
claim is overstated, which is §6 and the REFUTED item in the termination state.
That is a MEDIUM about a document, not a HIGH about the product, and it goes to
BACKLOG non-blocking.

---

## 9. What I did NOT do

- **Did not drive R2, R5, R9, R10, R16.** Cause and remedy in §3; not silently dropped.
- **Did not modify `crates/wcore-eval-scenarios/src/fixtures/openai.rs`** to retain
  request bodies. It is shared across many targets and its non-retention is a
  stated design property — a seam decision, not a census lane's call.
- **Did not go near `crates/wcore-skills/` or the promotion path** (owned by
  `lane/23a-c1`), `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`,
  or `.planning/BACKLOG.md`. My only source change is one test file:
  `crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs`.
- **Did not write the missing wrappers or the `governed_skill_drive.rs` harness.**
  Out of this lane's scope; recorded in §7 for whoever takes 23A-04.
- **Did not run a Windows or macOS leg.** Linux only. macOS remains 23A-04's
  disposition.
- **Did not run a full-workspace build**, so I claim nothing about crates I did
  not touch, and I am not reporting any failure cluster as mine.
- **Did not merge, open a PR, tag, or close an issue.**
- Used `git reset --hard` on hetzner **only** on my own `hz/23a-census` branch in
  my own dedicated worktree, to pin each measurement to a named commit. No other
  lane's ref or working tree was touched. Disclosed because LANE-BRIEF §0 names
  the command.

---

## 9b. Gate linter

`python3 .planning/scripts/lint-plan-gates.py .planning/phases/23A-governed-skills/`
→ *4 plans, 93 gates examined: 11 HIGH, 9 other*. **Zero findings cite anything
this lane authored** (5 in `23A-01-PLAN.md`, 3 in `23A-02`, 1 in `23A-03`, 2 in
`23A-04`); the linter's own note records that 4 of 4 plans have already executed,
so its pre-execution rules fire by construction. Reported for completeness, not
claimed as mine and not fixed — those plans are not this lane's to edit.

---

## 10. Evidence

All under `.planning/phases/23A-governed-skills/evidence/23A-census/`. Byte
counts were taken on the remote and re-checked after transfer; every pair
matched.

| File | Bytes | What |
|------|-------|------|
| `23A-CENSUS-NOTES.md` | — | Append-only working notes, committed from T+0 per §6b-i |
| `run-A-baseline.log` | 6022 | Pre-repair, no selftest — `3 passed; 0 failed` |
| `run-B-selftest-refusal.log` | 1061 | Pre-repair, selftest — `2 passed; 1 failed`, tripped at check 1 of 4 |
| `run-C-baseline-repaired.log` | 633 | Post-repair, no selftest — `4 passed; 0 failed` |
| `run-D-selftest-repaired.log` | 1628 | Post-repair, selftest — all four route outcomes reported |
| `run-E-baseline-final.log` | 633 | Final, no selftest — `4 passed; 0 failed` |
| `run-F-selftest-final.log` | 1752 | Final, selftest — includes the live legacy-matcher measurement |
| `run-status-sentinels.txt` | 201 | `WLRC=` / `WLDONE` status files read back by a separate `ssh` call |
