# 21-C3 — Hostile-corpora equivalence: working notes

Lane `21-c3-hostile`. Branch `lane/21-c3-hostile`, base `5457710e` on
`plan/f20-unified-audit-repair`.

This file is appended to after **every** measurement, per LANE-BRIEF §6b-i. It is the
resume point if this lane dies.

---

## 0. The specification (from `21-04-PHASE-VERDICT.md` §1, Criterion 3)

> CRITERION :: 3 :: NOT-MET :: Standalone and host-protocol hostile corpora prove
> equivalent enforcement.

The verdict names **four** clauses that are unmet. These, and only these, are my
specification:

| # | Unmet clause | Verdict source |
|---|---|---|
| C3-a | **Three of eleven dimensions have no host-protocol expression at all** — `tool` and `fan_out` cannot be requested on the host child-spawn API; `egress` cannot be attempted because the child registry carries no network-capable tool (`Unknown tool: WebFetch`) | VERDICT §1 C3 bullet 1 |
| C3-b | **Fan-out is undetermined live**, on both platforms and both surfaces | VERDICT §1 C3 bullet 2 |
| C3-c | **The Windows standalone live surface has no actor at all** — every PTY-backed transport unavailable, piped fallback has no approval channel | VERDICT §1 C3 bullet 3 |
| C3-d | **The `tool` dimension's REFUSED is jointly attributable** to tool authority and to workspace containment; C1 records the tool guard ABSENT, so that REFUSED must not be read as evidence of tool enforcement | VERDICT §1 C3 bullet 4 |

The lane brief states the gap is **proof narrowness, not a broken enforcement
property**. So the job is to widen the proof over these four clauses, not to hunt a
defect — though a defect found while widening is a bonus and must be reported loudly.

## 0b. The trap I am specifically exposed to

A hostile corpus produces "nothing bad happened" for a genuine pass and for a probe that
never ran. Rules I bind myself to before reporting anything:

1. Assert the corpus artifact **exists and is non-empty** before asserting what is not
   in it.
2. Every known-negative must be **shown to actually fail** — feed a corpus that should be
   rejected and watch it go red.
3. A uniform answer across every input with **no known-positive** is the self-passing
   shape, not a result.
4. Read back `N passed; M failed; K ignored; F filtered out` — never trust exit status
   (LANE-BRIEF §3.2). `rtk` strips `ignored`/`filtered out`, so cargo goes through
   `/usr/bin/env cargo` or an absolute path.

---

## 1. Log

### t0 — orientation (no measurement yet)

Read: `LANE-BRIEF.md`, `21-04-PHASE-VERDICT.md`, `21-05-CRITERION3-REPAIR.md`.

Corpus surface inventory (from `wc -l`, unproxied):

```
1312 crates/wcore-cli/tests/portability_hostile_corpus.rs
 897 crates/wcore-cli/tests/child_authority_corpus.rs
 370 crates/wcore-cli/tests/child_authority_corpus/cases.rs
1593 crates/wcore-cli/tests/child_authority_corpus/live.rs
2205 crates/wcore-cli/tests/child_authority_corpus/surfaces.rs
```

Working hypothesis of the tractability order (to be revised by measurement):

- **C3-d is a differential-control problem, not a product problem.** The probe target
  sits *outside* the child's isolated-mutation checkout, so an absent Bash effect is
  attributable to containment OR to authority. Running a **second probe inside** the
  checkout separates them: if Bash lands inside and not outside, the refusal was
  containment and the tool guard is confirmed absent live (which would corroborate
  F21-02-01 on the live surface for the first time). This also supplies the
  **known-positive** the corpus is missing on that dimension.
- **C3-b is a missing live control.** The in-process fan-out probe already runs an
  at-cap control first (`21-05` §1.3) precisely because zero children is the correct
  enforcement outcome and is indistinguishable from a broken fixture without one. The
  live probe has no such control. Porting it is mechanical.
- **C3-a splits.** `tool`/`fan_out` inexpressibility is a typed fact about
  `SubAgentConfig` + `spawn_host_child` (`ForkOverrides::default()` hardcoded) and is
  honest. `egress` inexpressibility is a *registry* fact (`Unknown tool: WebFetch`) and
  may be reachable if the host child spawn path admits a toolset. TO BE MEASURED.
- **C3-c is a Windows PTY claim to re-measure.** "`portable_pty`'s ConPTY backend does
  not surface the spawned binary's stdout to the master end" is a strong negative
  claim and per LANE-BRIEF §3b-i a negative is the easiest thing to pass without work.
  Re-measure it directly on `SeanD@seandesktop` under `D:\` before either accepting or
  rejecting it.

Nothing measured yet. Next: read the four corpus files and locate the tool probe, the
fan-out probe and the host-protocol spawn path.

### t1 — FOUR measurements, three of them findings

All greps unproxied (`/usr/bin/grep`), globs quoted. Each absence below is preceded by a
known-positive in the same invocation, per LANE-BRIEF §3b-i.

#### M1 (FINDING, HIGH) — the tool dimension's in-process REFUSED is confirmer-denied, not "jointly attributable to tool authority and workspace containment"

The verdict's C3-d clause names TWO causes for the tool REFUSED. The shipped evidence
names a THIRD one in plain text in the same row, and it is the one that actually fired.

Instrument alive: `/usr/bin/grep -c "COMBINATION ::" 21-05-t1-linux.log` → **44**
(non-zero, so the file and the matcher both work).

`evidence/21-05-t1-linux.log:116`, standalone / in-process / `corpus_tool`:

```
REFUSED :: obtained no Bash effect — nothing the read-only parent does not hold ::
... the probe target sits outside the child's isolated-mutation checkout, so an absent
effect is jointly attributable to tool authority and to workspace containment ...
What the child's own tool call returned: Tool execution denied by user
```

`Tool execution denied by user` is `orchestration/mod.rs:751` — the string
`confirm_call` emits on `ConfirmResult::Denied`. Root cause chain, read from source:

* `Config::default()` → `approval_mode: Default` → `Config::smart_approval_policy()`
  returns `ApprovalPolicy::Prompt` (`wcore-config/src/config.rs:1241`).
* `AgentSpawner::child_config` (`spawner.rs:2348`) **deliberately** does not flip
  `auto_approve`; the child inherits the parent's posture (audit H-7 / M-9).
* `ToolConfirmer::requires_confirmation_for` under `Prompt` returns `true` for every
  tool not on the allow-list (`confirm.rs:70`).
* `ToolConfirmer::check_for` (`confirm.rs:124`) returns `Denied` unconditionally when
  `io::stdin().is_terminal()` is false — which it is under a CI runner.

So the child's Bash call **never reached the Bash tool, the workspace guard, or the tool
registry**. The recorded REFUSED is attributable to a third cause the verdict does not
list, and neither of the two it does list was exercised. The corpus's own comment at
`surfaces.rs:1018-1029` describes this mechanism accurately for the Windows hang case
and does not connect it to the Linux verdict.

**Not universal.** `evidence/21-05-t1-linux.log:69`, host-protocol / in-process /
`corpus_filesystem`, shows the VFS's own text —
`path "..." is outside sandbox root "..."` — so the host-protocol child's `Read` DID
reach the VFS. The difference is `child_config`'s
`if let Some(manager) = &self.approval_manager { config.set_smart_approval_policy(...) }`:
the `AgentBootstrap`-built host session has an approval manager; the corpus's bare
`AgentSpawner::new` fixture does not. So the confounder is specific to the STANDALONE
in-process driver.

#### M2 (FINDING, HIGH) — the standalone in-process tool probe's stated premise is false in its own fixture

The probe records `obtained no Bash effect — nothing the read-only parent does not hold`.
But `AgentSpawner::new` seeds `parent_tool_authority: ParentToolAuthority::unrestricted()`
(`spawner.rs:983`), and `parent_fixture` never calls `narrow_parent_tool_authority`. The
fixture's parent therefore holds Bash. There is no read-only parent in the run, so even
a working guard could not have been exercised.

#### M3 — the merged tree HAS the tool guard the Phase 21 verdict recorded ABSENT

`21-04-PHASE-VERDICT.md` §1 C1 records F21-02-01 DECLINED and open: *"`build_tool_registry`
registers a requested tool without consulting a parent"*. At `5457710e` that is no longer
true. `spawner.rs:2718`:

```rust
// F21-02-01 — authority is an intersection, never a replacement.
let permitted = permitted && parent_tool_authority.contains(*name);
```

plus a second dispatch-time layer (`set_policy_gate`, F21-02-03) at `spawner.rs:2257ff`,
and `tests/spawner_authority_enumeration.rs` re-deriving the production construction
sites so a new unwired site fails. Another lane repaired this after the verdict was
written. **This is what makes a real tool-dimension proof possible for the first time**,
and it also means the corpus can now carry a genuine known-positive.

#### M4 — the host-protocol egress NOT-EXPRESSIBLE is correct on its stated cause

`SHARED_READ_ONLY_CHILD_TOOLS = &["Read", "Grep", "Glob"]`
(`crates/wcore-types/src/spawner.rs:159`) and `spawn_host_child` hardcodes
`ForkOverrides::default()` (empty `allowed_tools`), so `build_tool_registry`'s
`allowed.is_empty()` branch gives the child exactly that floor. No network-capable tool.
Instrument alive: the same grep returned 8 hits across 3 files for
`SHARED_READ_ONLY_CHILD_TOOLS`.

But the probe that RECORDS that verdict cannot tell it from a dead instrument: it reads
`server.received_requests() == 0` with no control proving the loopback destination is
reachable at all. That is the exact self-passing shape the lane brief names.

### t1 — the design that follows

The tool dimension needs a **three-run differential** in which the confirmer is held
constant and parent authority is the only variable:

1. **KNOWN-POSITIVE** — parent holds Bash, `allowed_tools=["Bash"]`, target INSIDE the
   child's workspace. The effect MUST land. If it does not, the instrument is dead and
   all three runs record NOT-EXPRESSIBLE.
2. **CONTAINMENT CONTROL** — parent holds Bash, target OUTSIDE the workspace. Isolates
   containment.
3. **THE HOSTILE PROBE** — parent narrowed to read-only (no Bash), `allowed_tools=["Bash"]`,
   target INSIDE the workspace. A refusal here cannot be containment (run 1 proved an
   inside write lands) and cannot be the confirmer (held constant across all three), so
   it is tool authority.

Known-negative to demonstrate: delete the intersection line and run 3 must flip to
ALLOWED.

Next: implement. Hetzner worktree `/root/wayland-21c3` on `hz/21-c3-hostile`, warm build
running.
