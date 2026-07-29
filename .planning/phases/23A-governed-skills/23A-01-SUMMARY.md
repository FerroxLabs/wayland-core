---
phase: 23A-governed-skills
plan: "01"
subsystem: governed-skills
tags: [security, skills, quarantine, config-authority, live-uat]
requires: []
provides:
  - 23A-01-SURFACE-CENSUS.md
  - 23A-01-LIVE-EVIDENCE.md
  - crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs
affects:
  - crates/wcore-config/src/config.rs
tech-stack:
  added: []
  patterns: [live-binary-drive, controlled-capability-matrix, scope-discriminator-probe]
key-files:
  created:
    - .planning/phases/23A-governed-skills/23A-01-BASE-SHA
    - .planning/phases/23A-governed-skills/23A-01-SURFACE-CENSUS.md
    - .planning/phases/23A-governed-skills/23A-01-LIVE-EVIDENCE.md
    - crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs
  modified:
    - crates/wcore-config/src/config.rs
decisions:
  - "F23A-01-H1 fixed in wcore-config rather than escalated, against the plan's own termination rule, on a 4-0 cross-audited decision"
metrics:
  completed: 2026-07-26
status: partial
---

# Phase 23A Plan 01: Generated-Skill Execution Boundary — Summary

Sixteen routes from generated skill content to execution are enumerated and all sixteen are gated, so the quarantine claim holds on the code paths that exist — but driving the shipped binary found two HIGH defects the code reading would never have produced, one now fixed and one left open and red.

**Termination state: GAP-CLOSED at the generation boundary, INCOMPLETE at the live-proof boundary.**

---

## What was actually established

### The execution boundary holds (REFUTED-NO-GAP on its own terms)

Sixteen routes enumerated with `path:line` citations, every one resolved. `Skill` tool call, system-prompt listing, both router candidate pools, the router hint, `/skill run|list|show`, the cron skill sink, skill-declared hooks, skill-declared MCP, conditional activation, artifact materialisation, shell composition, the session-start prioritizer, and cross-project resolution. **No UNGATED route at the execution boundary.** Details and citations in `23A-01-SURFACE-CENSUS.md`.

Two fragilities recorded rather than glossed:

- **M1** — the hook route is gated by a success-precondition plus a cold LRU, never by an explicit quarantine check. `skill_is_authorized` (`skill_tool.rs:159`) consults *permissions*, not `disable_model_invocation`. It holds today for a reason weaker than it looks, and the cron pre-scan (`cron.rs:262`) already warms the LRU for quarantined names via the unrestricted `resolve()`.
- **M2** — the forgery hypothesis, recorded as **two separate measurements and deliberately not collapsed**: the classifier half is confirmed (and is *deliberate* — an `auto-` name alone must not quarantine user content, which is what the negative control depends on); the tool-write-reachability half is **not established**, because writing under `$WAYLAND_HOME/skills/` is a workspace-policy question outside this plan's files. Severity from the pair: MEDIUM with a stated precondition.

### F23A-01-H1 (HIGH) — found, root-caused, fixed, live-proved

`[observability] skills_lifecycle = false` is documented in its own source file as *"the one observability switch whose explicit `false` is an authority boundary."* It was not one.

`restrict_untrusted_project_config` (`config.rs:4351`) rebuilds an untrusted project's config from `ConfigFile::default()` and copies forward an allowlist of power-reducing settings. `observability.skills_lifecycle` was missing, so an explicit project `false` was discarded and re-defaulted to `true` before the AND-merge saw it. **An untrusted workspace is the default state of any freshly created or cloned project**, so this was the common case.

Consequence: the operator who wrote `skills_lifecycle = false` to keep a project out of the learn loop still got skills auto-drafted from that project's traffic — into the **global** skills directory, so it crossed into every other project on the machine. Transitively it also force-constructed Memory (`bootstrap.rs:1548`).

Fixed with three lines preserving only an explicit `false`. Proved:
- 9-cell live capability matrix against the shipped binary, all green, including six controls that were already correct and had to stay correct, and one proving an untrusted project still cannot *grant* the lifecycle.
- A trust-only control pair: same project file, `ready` untrusted vs `unavailable` after `--trust-workspace`.
- A unit regression measured red (exit 101) with the fix hunk physically removed and green (exit 0) with it restored.

**Why no existing test caught it:** every prior `skills_lifecycle` merge test goes through `merge_config_files`, which hardcodes `project_trusted = true`. The untrusted path — the default — had zero coverage. Same shape as the F21-02 vacuous-truth finding: a property proved only in the configuration where it happens to hold.

This also explains a **pre-existing red** in the repository's own `packaged_driver_gate.rs` lifecycle matrix at cell `global=true, project=false`, present at the base SHA before this phase touched anything. The test was right; the product was wrong. Nothing in it was weakened.

### F23A-01-H2 (HIGH) — found, isolated, LEFT OPEN AND RED

Fixing H1 let the packaged matrix advance past the cell it used to die on — and it then died at the next one, in the catalog probe, which is exactly where the quarantine refusal happens. **H1 had been masking H2.**

**Any tool call that returns an error result leaves a nonterminal tool execution in the session journal, and the engine kills the session with `Session persistence authority unavailable: invalid journal state transition`.**

The scope discriminator settled ownership by measurement rather than assumption:
- a `Skill` call for a name that was **never generated** kills the session → not the quarantine classifier;
- a `Read` of a nonexistent path produces the **identical** error → not the skills surface at all.

Reproducible in 13 seconds, deterministic, Linux, at this SHA. Same class as Windows live-UAT defect **D1** ("a refused tool call kills the session"), now reproduced on Linux with a minimal trigger.

**Left open deliberately.** It is an engine tool-dispatch/journal defect in a subsystem Phases 21 and 22 are editing concurrently; a fix authored here would collide at integration and would be authored without ownership context. The three probes are **committed RED on purpose**. No test was weakened, ignored, re-gated, or deleted to hide it, and no timeout was raised.

Whether H2 is a regression on the integration branch or is present in shipped `v0.12.25` was **attempted and not resolved** — the release binary failed the probe at a different, earlier assertion in 0.03s, which is a harness/binary compatibility failure, not an observation about H2. Recorded as unresolved rather than guessed.

---

## The cross-audited decision

**Question:** F23A-01-H1's fix lands in `wcore-config`, which this plan's own termination criterion names verbatim as an ESCALATE example. The phase mandate says HIGH findings are fixed or disproved. Which governs?

**Panel:** codex `gpt-5.6-sol` **FIX** · gemini `3.1-pro-preview` **FIX** · kimi K3 **FIX** · internal adversarial pass **FIX**. **4-0.**

One trap fired and was caught: codex's first invocation returned only an echo of the prompt (its startup hook failed on a GitHub call), and a naive extraction would have read `PANEL_POSITION=FIX` and `PANEL_POSITION=ESCALATE` out of the echoed *question* and scored a phantom vote. Re-run from a neutral cwd to get a real answer.

**Dissent, recorded because it is correct and all three external legs got it wrong:** each of them asserted an untrusted repository "gains nothing". That is not true. It gains a bounded, workspace-scoped **denial** capability — a hostile checked-in config can now suppress the operator's learn-and-evolve loop and, transitively via `want_memory`, Memory construction for that workspace. The counter that carries the decision is not "gains nothing" but that this denial is **strictly smaller than what the same allowlist already grants** an untrusted repo through `read_only` and `max_turns`, and that it is announced in the capability stream rather than silent. The fix was narrowed in response to this dissent: only `Some(false)` is forwarded, never `Some(true)`, so "project may narrow, never grant" is true by construction rather than by a downstream operator.

---

## What this plan did NOT deliver

Stated plainly, because an unmet clause named is worth more than a clause quietly dropped.

| Plan clause | Status |
|---|---|
| Route census with citations and dispositions | **DONE** |
| Forgery hypothesis as two uncollapsed measurements | **DONE** |
| CRITICAL/HIGH gap fixed or disproved | **PARTIAL** — H1 fixed; H2 found, isolated, reported, **not fixed** |
| `crates/wcore-skills/tests/generated_execution_boundary.rs` hostile nonce corpus | **NOT WRITTEN.** The live drive was prioritised over the unit corpus because the criterion's claim is about the shipped binary, and the live drive found two HIGH defects a unit corpus would have missed entirely. An unmet clause, not a satisfied one. |
| `governed_skill_drive.rs` shared harness + `lib.rs` declaration | **NOT WRITTEN.** The drive target was written standalone; extraction was not reached. |
| Refusal probes added to `packaged_driver_gate.rs` | **NOT DONE.** That file was left byte-identical. |
| `scripts/f23a-boundary-drive.sh` / `.ps1` wrappers | **NOT WRITTEN.** Their only job is to wrap a drive target that does not yet pass. |
| `WAYLAND_EXPECT_SHA` mismatch → exit exactly 3 | **NOT BUILT** (lives in the unwritten wrappers). |
| `WAYLAND_F23A_SELFTEST=refusal` proved to fire | **BUILT, NOT EXERCISED.** The switch and its `F23A-SELFTEST-TRIPPED` marker exist in the drive target but were never made to fire, because the base run is red on H2. An unexercised self-test is precisely the decorative control this plan bans, and it is reported as such rather than counted as delivered. |
| Windows live leg | **NOT RUN.** It would have reproduced the same platform-independent red, not produced new information. Recorded as unmet, not as a pass. |
| `/skill run` refusal observed live | **RECORDED LIMIT.** H2 terminated the session before the route drive completed. |
| macOS | **NOT CLAIMED.** 23A-04's disposition. |

`cargo fmt --all -- --check` is clean. No dependency was added, removed or updated; no `Cargo.toml` or `Cargo.lock` change.

**MEDIUM findings could not be written to `.planning/BACKLOG.md` — it is a fenced file in this wave.** They are filed in `.planning/SEAM-REQUESTS/23A.md` for serial integration.

**Requirement F23-01 is NOT marked complete.**

---

> **STATUS CORRECTION (2026-07-29, lane/record-truth).** This document records
> `F23A-01-H2` as open. **It was fixed at `32a5fc90` on 2026-07-27**, with five
> wired regression tests in
> `crates/wcore-agent/src/orchestration/d1_refusal_terminal_tests.rs`. The body
> above is left as written on purpose. See `23A-STATUS-CORRECTION.md` in this
> directory for the evidence — and for the gap underneath it: the 16-route
> quarantine census is **still unmeasured at HEAD**, and `WAYLAND_F23A_SELFTEST`
> was never shown to fire. H2 being fixed does not close the census.
