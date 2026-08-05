# 23A-01 — Live evidence

Every result below came from driving the **shipped `wayland-core` binary**, not from a unit assertion about an internal guard. Nothing here is a claim about a function that returns `false`.

**Host:** `hetzner-dsm`, Linux, 96 cores. Dedicated per-phase checkout `/root/wayland-p23a` (never the shared `/root/wayland` — six other phases were building concurrently).
**Base SHA:** `2ecdfdf54ff7fda920eec7d068337006e5da4ee4`
**Binary:** `/root/wayland-p23a/target/debug/wayland-core`, built with `WAYLAND_BUILD_SOURCE_SHA` pinned to that SHA.
**Surface used:** `--json-stream` (`crates/wcore-cli/src/main.rs:396`) with `--provider openai --model fixture-chat-v1 --base-url <fixture>`, plus `--trust-workspace` (`main.rs:353`) for one control. The positional `prompt` surface (`main.rs:539`) was **not** used, stated explicitly so a later reader does not assume it was.
**macOS:** not covered here. 23A-04 owns the macOS disposition. Cargo is never run on the Mac in this phase; `cargo fmt --all -- --check` is the only local cargo invocation and it is clean.

---

## L1 — Baseline: the repository's own packaged product gate was already RED

Before this phase changed one line, at the base SHA:

```
cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate \
  --test packaged_driver_gate -- packaged_lifecycle_memory_matrix_has_real_effects_and_quarantine
```

```
thread '...' panicked at crates/wcore-eval-scenarios/tests/packaged_driver_gate.rs:819:17:
generation failed for global=true, project=false, memory=false:
  [AssertionFailed { assertion: "CapabilityHonesty",
    observed: "ProcedureSkillDrafting: advertised ready before required unavailability;
               LegacyAutoSkillDrafting: advertised ready before required unavailability" }]
test result: FAILED. 0 passed; 1 failed
TEST_EXIT=101
```

This is a pre-existing red on the plan branch, not one this phase introduced. It is the product's own live lifecycle gate. **The test is correct and the product was wrong.** No assertion in it was weakened, ignored, re-gated or deleted.

---

## L2 — F23A-01-H1 diagnosed live: the project-level opt-out fails open

### L2.1 The failing observation

Two config files, one binary, capability state read straight off the `--json-stream` protocol surface:

```
home/config.toml            [observability] skills_lifecycle = true
project/.wayland-core.toml  [observability] skills_lifecycle = false
```

```json
{"type":"capability_activation","capability":"procedure_skill_drafting","stage":"ready"}
{"type":"capability_activation","capability":"legacy_auto_skill_drafting","stage":"ready"}
```

Expected `unavailable` (the merge at `config.rs:4171-4173` ANDs the two sources). Observed `ready`.

### L2.2 The project file IS read — disproving the obvious alternative explanation

Feeding the project file malformed TOML makes the binary refuse to start:

```
Error: failed to parse .wayland-core.toml: TOML parse error at line 1, column 6
  |
1 | this is not valid toml {{{
  |      ^
key with no value, expected `=`
```

So the file is loaded and parsed. The value is dropped *after* loading. Both layout forms (`.wayland-core.toml` and `.wayland-core/config.toml`, `config.rs:3119`) behave identically, so this is not a layout-selection artifact either.

### L2.3 The control that isolates the cause

Same project file, same binary, differing only in workspace trust:

| workspace trust | observed `legacy_auto_skill_drafting` |
|---|---|
| untrusted (default) | `ready` |
| after `wayland-core --trust-workspace` | `unavailable` |

That single variable flips the outcome, which puts the cause in `restrict_untrusted_project_config` (`config.rs:4351`) and nowhere else.

### L2.4 After the fix: the full 9-cell live matrix, all green, controls included

Driver: `/root/p23a-live-matrix.sh`. It exits with the **count of mismatched cells**, so it can go red; it is not a script that prints a report and exits 0 regardless.

```
PASS  global-on-project-off  global=true   project=false  trust=untrusted expected=unavailable observed=unavailable
PASS  absent-project-off     global=absent project=false  trust=untrusted expected=unavailable observed=unavailable
PASS  global-off-project-on  global=false  project=true   trust=untrusted expected=unavailable observed=unavailable
PASS  global-on-project-on   global=true   project=true   trust=untrusted expected=ready       observed=ready
PASS  global-off-project-off global=false  project=false  trust=untrusted expected=unavailable observed=unavailable
PASS  global-off-only        global=false  project=absent trust=untrusted expected=unavailable observed=unavailable
PASS  both-absent            global=absent project=absent trust=untrusted expected=ready       observed=ready
PASS  trusted-project-off    global=absent project=false  trust=trusted   expected=unavailable observed=unavailable
PASS  untrusted-cannot-grant global=false  project=true   trust=untrusted expected=unavailable observed=unavailable
CELLS_FAILED=0
```

Rows 1–2 are the cells that were failing. Rows 3–8 are controls that were **already correct before the fix** and had to stay correct — a fix that turned lifecycle off for everybody would show up as failures there. Row 9 proves the change stayed one-directional: an untrusted project still cannot *grant* the lifecycle against a global opt-out.

### L2.5 Unit-level red/green, proving the new test discriminates

```
with the 3-line fix present:  test untrusted_project_skills_lifecycle_opt_out_survives_restriction ... ok      exit 0
with the fix hunk removed:    test untrusted_project_skills_lifecycle_opt_out_survives_restriction ... FAILED  exit 101
```

The fix was physically removed on the remote checkout and restored afterward, so the red is measured rather than asserted.

**Why the pre-existing unit suite never caught this:** every prior `skills_lifecycle` merge test goes through `merge_config_files`, which is `#[cfg(test)]` and hardcodes `project_trusted = true` (`config.rs:3890-3892`). The untrusted path — the **default** for a new project — had no coverage at all. That is how a green suite coexisted with a broken product, and it is the same shape as the F21-02 vacuous-truth finding.

---

## L3 — The quarantine itself, driven at the product surface

From the repository's own packaged lifecycle matrix at the base SHA, on the cells it reached, the product does refuse:

- `/skill list` renders the generated draft tagged `(hidden)`.
- `/skill show <name>` reports `visibility: hidden from model`.
- A `Skill` tool call naming the draft returns `is_error`, containing `not found`, and **not** containing the draft body.

So the operator can see what is quarantined while the model cannot invoke it — both halves of the visibility contract, observed at the product surface rather than asserted about a flag.

`/skill run <name>` refuses with `"this skill is quarantined and cannot be run."` (`crates/wcore-agent/src/slash/skill.rs:115-119`). **This route was NOT reached live**, because L4 below terminated the session before the route drive completed. It is a **RECORDED LIMIT**, not a silent drop, and it is named in the summary as an unmet clause.

---

## L4 — F23A-01-H2 (HIGH, OPEN, NOT MINE): any refused tool call kills the session

### The observation

Fixing F23A-01-H1 let the packaged matrix advance past the cell it used to die on, and it then failed at a **later** cell — in the catalog probe, the exact place the quarantine refusal happens:

```
catalog probe failed for global=true, project=true, memory=false:
  RunnerError("engine emitted error: {\"code\":\"engine_error\",
   \"message\":\"Session persistence authority unavailable: invalid journal state
    transition: turn turn-7f10ae64-... has nonterminal tool execution
    tool-execution-39106f70-...\",\"retryable\":false}")
  AssertionFailed { assertion: "Contains(\"CATALOG_OK\")",
    observed: "substring not found in output (0 chars)" }
```

Defect H1 had been **masking** defect H2: the matrix never previously reached the cell where H2 fires.

### The scope discriminator — the part that matters

`crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs` carries three probes. Run at the fixed binary:

```
test generated_draft_is_refused_at_every_route_while_user_content_is_not ... FAILED
test refused_read_tool_call_does_not_kill_the_session ................... FAILED
test refused_skill_tool_call_does_not_kill_the_session .................. FAILED
test result: FAILED. 0 passed; 3 failed; finished in 13.30s
```

- `refused_skill_tool_call_does_not_kill_the_session` uses `f23a-no-such-skill` — a name that **was never generated** and is absent from the catalog entirely. It still kills the session. So this is **not** the quarantine classifier.
- `refused_read_tool_call_does_not_kill_the_session` uses `Read` on `/f23a/definitely/not/a/real/path.txt`. **Identical failure, identical error string.** So it is not the skills surface at all.

```
a refused Read tool call must leave the session usable.
failures=[RunnerError("engine emitted error: {... \"Session persistence authority
  unavailable: invalid journal state transition: turn turn-7667507e-... has
  nonterminal tool execution tool-execution-ca5a1889-...\"}"),
  CostMissing,
  AssertionFailed { assertion: "Contains(\"SESSION_SURVIVED\")",
    observed: "substring not found in output (0 chars)" }]
```

### The finding, stated plainly

**Any tool call that returns an error result leaves a nonterminal tool execution in the session journal, and the engine then terminates the session with `Session persistence authority unavailable`.** Reproducible in 13 seconds, deterministic, on Linux, at this SHA.

The rejecting check is `crates/wcore-agent/src/session_journal/reducer.rs:969-980`, which refuses to close a turn holding a tool in `Prepared | Running | Unknown`. The terminal append that should have moved it to `Failed` is `crates/wcore-agent/src/orchestration/mod.rs:2290` (`lease.fail(...)`), which is on the `r.is_error` branch and *looks* correct on inspection — so the leak is somewhere between `PreparedToolLease::start` (`orchestration/mod.rs:2012`) and that append, and was not isolated within this plan's budget.

This is the same class as **Windows live-UAT defect D1** ("a refused tool call kills the session") recorded in `.planning/phases/20A-native-windows-macos-uat/20A-LIVE-WINDOWS-UAT.md`, now reproduced on **Linux** with a minimal deterministic trigger.

### Why it is left OPEN rather than fixed here

It is not a skills defect and its fix is in the engine's tool-dispatch/journal path, a subsystem that Phases 21 and 22 are actively editing in concurrent worktrees at this moment. A fix authored from this phase would collide at integration and would be authored without the ownership context. **Reported, not engineered around; no test was weakened to hide it.** The three probes are committed RED on purpose.

### Attribution status — attempted, INCONCLUSIVE

Whether H2 is a regression introduced on the integration branch, or is present in the shipped `v0.12.25` release, was **attempted and not resolved**.

A `wayland-core` binary was built at release SHA `61b79c4f90f71fe2cf243affa7620b3c9b607f14` (`chore(main): release 0.12.25`) in a separate checkout `/root/wayland-p23a-rel` — the first attempt failed on `--locked` lockfile drift, the second succeeded without it. The `refused_read` probe was then re-run with `WCORE_EVAL_BIN` pointed at that release binary:

```
test refused_read_tool_call_does_not_kill_the_session ... FAILED
...panicked at f23a_boundary_drive.rs:287: the Read tool call reached the product
test result: FAILED. 0 passed; 1 failed; finished in 0.03s
```

It failed at a **different and earlier** assertion — the Read tool call never reached the product at all — and the whole run took 0.03s, which is far too fast for a real session. That is a harness/binary compatibility failure between the base-SHA test harness and the older release binary, **not** an observation about H2 either way.

So: **not determined.** Stated as unresolved rather than guessed, because "the shipped release is broken" and "a concurrent phase broke the integration branch" call for completely different escalations and the evidence distinguishes neither.

---

## L5 — Platform coverage actually achieved

| Leg | Status |
|---|---|
| Linux (`hetzner-dsm`) — capability cascade, 9 cells + controls | **DONE, green** |
| Linux — config unit regression, red without fix / green with | **DONE** |
| Linux — packaged lifecycle matrix | **advances past the H1 cell, then RED on H2** |
| Linux — boundary drive, 3 probes | **RED on H2, deliberately committed red** |
| Windows (`SeanD@seandesktop`) | **NOT RUN.** Not attempted, because the boundary drive it would run is red on Linux for a reason that is platform-independent; running it would have produced a second copy of the same red, not new information. Recorded as an unmet clause, not as a pass. |
| macOS | **NOT RUN.** 23A-04's disposition; not claimed. |
| `WAYLAND_EXPECT_SHA` mismatch → exit 3 | **NOT BUILT.** The `.sh`/`.ps1` wrappers were not written, because their only job is to wrap a drive target that does not yet pass. Recorded as an unmet clause. |
| `WAYLAND_F23A_SELFTEST=refusal` | **BUILT, NOT EXERCISED.** The switch and its `F23A-SELFTEST-TRIPPED: refusal` marker are implemented in the drive target, but cannot be meaningfully exercised while the base run is red on H2. Recorded as an unmet clause — an unexercised self-test is exactly the decorative control this plan bans, and it is reported as such rather than counted. |

---

> **STATUS CORRECTION (2026-07-29, lane/record-truth).** This document records
> `F23A-01-H2` as open. **It was fixed at `32a5fc90` on 2026-07-27**, with five
> wired regression tests in
> `crates/wcore-agent/src/orchestration/d1_refusal_terminal_tests.rs`. The body
> above is left as written on purpose. See `23A-STATUS-CORRECTION.md` in this
> directory for the evidence — and for the gap underneath it: the 16-route
> quarantine census is **still unmeasured at HEAD**, and `WAYLAND_F23A_SELFTEST`
> was never shown to fire. H2 being fixed does not close the census.
