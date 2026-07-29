# 21-C3 — Criterion 3: hostile-corpora equivalence, widened

**Lane** `21-c3-hostile` · branch `lane/21-c3-hostile` · base `5457710e` on
`plan/f20-unified-audit-repair` · HEAD at writing `8bf2e663`.

**Scope.** `21-04-PHASE-VERDICT.md` graded Phase 21 NOT ACHIEVED on Criterion 3 alone. That
verdict names exactly four unmet clauses; they are this lane's whole specification. The lane
brief's framing was that the gap is **proof narrowness, not a broken enforcement property** —
widen the proof, and report loudly if a real defect falls out.

Two real defects fell out. One is in the product; one was in this lane's own instrument.

---

## 1. Verdict on Criterion 3

**CRITERION 3 :: STILL NOT MET.** Two of the four clauses close on Linux; two do not.

| Clause (verbatim from the verdict) | Before | After | State |
|---|---|---|---|
| C3-a — three of eleven dimensions have no host-protocol expression | tool / fan-out / egress all NOT-EXPRESSIBLE, and the **egress** reading had no control | unchanged as verdicts, but egress's zero-request reading now carries a **known-positive control** and the recorded cause is refined | **NOT CLOSED** — inexpressibility is a typed fact about `SubAgentConfig`; the record is now non-vacuous |
| C3-b — fan-out is undetermined live, both platforms and both surfaces | NOT-EXPRESSIBLE on both live surfaces, both platforms | **REFUSED** on both live surfaces on Linux and on host-protocol live on Windows, each behind an at-cap control that admitted exactly 5 children | **CLOSED on 3 of 4 platform×surface cells**; the fourth is C3-c |
| C3-c — the Windows standalone live surface has no actor | **declared** as a platform fact | **measured**: the Windows standalone-live at-cap control admitted 0 children in the same run | **CONFIRMED BY MEASUREMENT**, not closed — the surface still has no actor |
| C3-d — the tool REFUSED is jointly attributable to authority and containment | REFUSED, attributed to two mechanisms | **NOT-EXPRESSIBLE on all four cells**, with four measured mechanisms named and neither of the verdict's two among them | **the clause is answered, and the answer is worse than the clause** |

The honest headline is that C3-d did not resolve into "REFUSED, attributed to authority". It
resolved into **"there was never a measurement here at all"**, on every cell, and the corpus now
says so. That is a loss of four decisive-looking verdicts and a gain of the truth — the same
direction `21-05-CRITERION3-REPAIR.md` took, for the same reason.

Criterion 3 asks for EQUIVALENT enforcement proved by both corpora. The tool dimension now
proves nothing on either surface rather than falsely proving the same thing on both. That is
strictly better evidence and strictly not a met criterion.

---

## 2. What the proof covered before, and after

| Platform | Host / path | SHA | Result | Evidence |
|---|---|---|---|---|
| Linux | `hetzner-dsm`, `/root/wayland-21c3` | `fde83e9a` | **29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** in 84.6 s | `evidence/21-c3/21-c3-t1-linux-full.log` |
| Windows | `SeanD@seandesktop`, `D:\lane-21c3` | `eb71644e` | **25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** in 159.8 s | `evidence/21-c3/21-c3-t3-windows-full.log` |

The two SHAs differ by one commit, which adds this document only:
`git diff --stat fde83e9a eb71644e -- crates/` is **empty**, so both platforms ran identical
code. Counts are read back from unproxied `cargo` invocations with `ignored` and `filtered
out` both present, per LANE-BRIEF §3b — never from exit status. The Windows leg additionally
carried `WLRC=0` and `WLDONE` sentinels (§3.2 pattern); both arrived.

Linux was 27 tests before this lane and is 29 now — the two additions are the matcher
self-tests in §6. Windows carries four fewer than Linux for the pre-existing `#[cfg(unix)]`
reason recorded in `21-05-CRITERION3-REPAIR.md` §4.2, unchanged here.

### Verdict deltas against `359ce2bf`

| Dimension / cell | at `359ce2bf` | now | why |
|---|---|---|---|
| fan-out standalone live | NOT-EXPRESSIBLE | **REFUSED** | at-cap live control added |
| fan-out host-protocol live | NOT-EXPRESSIBLE | **REFUSED** | same control |
| tool standalone in-process | REFUSED | **NOT-EXPRESSIBLE** | known-positive arm fails; cause named |
| tool standalone live | REFUSED | **NOT-EXPRESSIBLE** | child's shell never ran |
| tool host-protocol live | REFUSED | **NOT-EXPRESSIBLE** | child's shell never ran |
| egress host-protocol in-process | NOT-EXPRESSIBLE (uncontrolled) | NOT-EXPRESSIBLE (**controlled**) | destination proved alive first |

### Verdict deltas on Windows

| Dimension / cell | at `359ce2bf` | now | why |
|---|---|---|---|
| fan-out host-protocol live | NOT-EXPRESSIBLE | **REFUSED** | at-cap control admitted 5 children from 7 served requests |
| fan-out standalone live | NOT-EXPRESSIBLE (declared) | NOT-EXPRESSIBLE (**measured**) | at-cap control admitted **0** children — C3-c, now a measurement |
| tool, all cells | NOT-EXPRESSIBLE / REFUSED | NOT-EXPRESSIBLE with named cause | arm-granted hit the 45 s bound (the session-0 confirmer hang recorded as defect (d) in `21-05` §1.3); host-protocol live: the child's shell never ran |

Every other cell is unchanged on both platforms.

**C3-c, restated precisely.** The verdict declared the Windows standalone live surface actorless
as a platform fact. The at-cap control now *measures* it inside the same run that would
otherwise have taken a verdict from it: a batch of 5 — a batch the gate must admit — admitted
zero children. That converts a declaration into evidence and, importantly, makes the honest
NOT-EXPRESSIBLE on that cell **self-justifying from the row itself** rather than from a footnote
elsewhere. The clause is not closed: the surface still has no actor.

### New controls and observables

1. **Tool-authority differential** (`surfaces.rs`). Two arms of the production Delegate path,
   identical in workspace, script, child config, requested toolset and approval posture (all
   probe tools allow-listed in both, so the shipped confirmer is held constant), with the
   sandbox resolved in both by the production `SandboxRegistry::required_for_session`. The only
   variable is `parent_tool_authority`. Arm 1 is the **known-positive**: if a child of a parent
   that HOLDS the tool cannot use it, no verdict is taken from arm 2.
2. **Live shell known-positive** (`live.rs`). The delegated child's Bash prints a marker before
   attempting anything refusable; the marker returns in the child's `tool_result` and is read
   from the routed mock's served request bodies. Previously the live tool row read one bit — is
   the file on disk — and asserted the refusal was "attributable to what that child was given",
   which it had no way to establish.
3. **Live containment discriminator.** The same command writes to a relative path inside the
   child's own workspace, where containment has nothing to bind, and reports it separately.
4. **At-cap live fan-out control.** The in-process probe has had one since `359ce2bf`; the live
   probe did not. Fan-out is the only dimension whose CORRECT enforcement outcome is zero
   children, so a bound cap and a dead fixture read identically without it.
5. **Egress destination control.** One GET through the sanctioned `EgressClient` proves the
   loopback destination alive before the child spawns; its own request is subtracted so the
   control cannot manufacture the `received > 0` branch.
6. **The child's own `tool_result`, live.** The child engine writes to a `NullSink`, so its
   results never reach the parent transcript. They are now pulled out of the conversation the
   child sends back to its own endpoint. This is what named both causes in §3.

---

## 3. FINDING (HIGH, NEW, PRODUCT) — a delegated mutating child cannot run any shell command on Linux

`spawner.rs:1817` gives every `RequestedChildWorkspace::IsolatedMutation` child

```rust
authority_read_deny: vec![parent.as_ref().clone(), git_common_dir]
```

and `git_common_dir` is `<parent_workspace>/.git` — an **overlapping pair**. `bwrap.rs:295`
renders each *directory* deny as `--ro-bind <empty mask> <denied path>`. The first masks the
parent workspace as an empty READ-ONLY directory; the second then needs a mount point at
`.git` inside that read-only mask, and bubblewrap aborts before the shell starts.

Reproduced standalone on `hetzner-dsm`, control first:

```
=== CONTROL (known-positive): only the parent denied ===
SHELL_RAN
rc=0
=== DEFECT: parent AND parent/.git denied, the spawner.rs:1817 pair ===
bwrap: Can't mkdir /tmp/tmp.Fp0XY5mMGd/workspace/.git: Read-only file system
rc=1
```

And through the **shipped binary**, live, standalone headless-PTY — the child's own
`tool_result`, read off the wire:

```
Exit code: 1
STDOUT:

STDERR:
bwrap: Can't mkdir /tmp/.tmpZFK6oo/workspace/.git: Read-only file system
```

**Severity HIGH.** The advertised delegated-mutation path cannot execute a shell at all on
Linux, and it fails as an *absence of effect* — which is indistinguishable from enforcement
working. That is precisely why three plans recorded it as a refusal.

Not fixed here: this lane's remit is the proof, and a fix touches the sandbox manifest
contract. Routed as-is with a two-line reproduction.

---

## 4. FINDING (HIGH) — every `corpus_tool` REFUSED in the Phase 21 record came from a tool call that never executed

The verdict records the tool REFUSED as *"jointly attributable to tool authority and to
workspace containment"*. Measured causes at `fde83e9a`, one per cell:

| Cell | What the child's own tool call returned | Mechanism |
|---|---|---|
| standalone in-process (as shipped at `359ce2bf`) | `Tool execution denied by user` | the shipped **CONFIRMER** — `Config::default()` resolves to `ApprovalPolicy::Prompt`, `child_config` hands the child that posture unchanged (audit H-7 / M-9), and `check_for` denies unconditionally when stdin is not a terminal |
| standalone in-process (sandbox resolved as production does) | `bwrap: Can't mkdir …/.git` | §3 |
| standalone live | `Exit code: 1 … bwrap: Can't mkdir …/workspace/.git` | §3 |
| host-protocol live | `Tool execution denied by user` | the **CONFIRMER** |
| host-protocol in-process | — | correctly NOT-EXPRESSIBLE; `SubAgentConfig` has no tool field |

**Four distinct mechanisms; neither of the two the verdict names is among them.** All four
produce the same "no probe file on disk" reading the old probe keyed on.

A second, smaller finding in the same family: the standalone in-process tool probe recorded
`obtained no Bash effect — nothing the read-only parent does not hold`, but `AgentSpawner::new`
seeds `ParentToolAuthority::unrestricted()` (`spawner.rs:983`) and `parent_fixture` never
narrowed it. **There was no read-only parent in the run**, so even a working guard could not
have been exercised. The differential now narrows it explicitly.

Worth stating plainly, because it cuts the other way: **the merged tree HAS the tool guard the
verdict records as ABSENT.** `spawner.rs:2718` intersects the child registry against
`parent_tool_authority`, with a second dispatch-time layer via `set_policy_gate`, and
`tests/spawner_authority_enumeration.rs` fails if a new production construction site appears
undeclared. Another lane repaired F21-02-01 / F21-02-03 after the verdict was written. The
denied arm's evidence shows that guard firing — `Denied by policy: no matching grant for
actor+resource+action` — while the granted arm gets past it. The differential cannot yet turn
that into a verdict, for the two reasons in §6.

---

## 5. Known-negatives demonstrated

Per the lane brief: every gate must be shown to fail. Full transcript:
`evidence/21-c3/21-c3-t2-known-negatives.log`.

| Gate | Injection | Result |
|---|---|---|
| at-cap fan-out live control | `FAN_OUT_CAP` = 99, so the control is itself over-cap | both live rows flip **REFUSED → NOT-EXPRESSIBLE :: the breadth seam admitted no child even at the cap** ✅ |
| tool differential, arm-liveness gate | (unstaged) a non-hex per-arm session id made `create_for_run` reject the bind | **NOT-EXPRESSIBLE :: an arm of the tool differential produced no child** ✅ |
| tool differential, known-positive gate | (unstaged) fail-closed sandbox, then §3 | **NOT-EXPRESSIBLE :: the mutating tool did not work even for a parent that holds it** ✅ |
| live shell known-positive | shell exits before printing | **caught the matcher instead** — see §6 |
| shell-marker matcher | command-text-only body | permanent test `the_shell_marker_matcher_is_not_satisfied_by_the_command_text`, three assertions ✅ |

The two unstaged failures are worth more than the staged one: neither was arranged, and under
the old probe both would have been recorded as `REFUSED :: obtained no Bash effect`.

---

## 6. FINDING (in this lane's own instrument, REPAIRED — LANE-BRIEF §6b-ii)

The first draft of the live shell known-positive was **self-passing**. The marker was written
literally into the child's Bash command; the command text travels inside the child's `ToolUse`
block, which is in the SAME served request bodies the matcher searched. `child_shell_ran` was
therefore true whether or not any shell ran.

It reported, across two commits, `the delegated child's SHELL RAN … ATTRIBUTED TO WORKSPACE
CONTAINMENT, NOT TOOL AUTHORITY` on both live surfaces. **That reading was false.** It was
caught because the injection that should have flipped it changed nothing.

Repaired in this lane rather than written up and carried: the shell now concatenates the marker
halves at runtime (`printf %s%s A B` on Unix, `echo A^B` under `cmd` — both verified against
their real shells before use), so only the shell's stdout can produce the joined string. Two
permanent tests pin it, including the **third assertion §6b-ii requires**: that the OLD matcher
WOULD have matched the command-text-only body, so the self-test cannot pass on an instrument
that never had the defect.

Ruled out as an alternative explanation for the post-repair flip: the split construction itself,
verified under bubblewrap on the same host — `bwrap … /bin/sh -c "printf %s%s CORPUSSHELL
RAN7d21"` → `CORPUSSHELLRAN7d21`, rc 0.

---

## 7. What this lane did NOT do

- **The §3 sandbox defect was not reproduced on Windows.** It is a bubblewrap defect and
  bubblewrap is Linux-only; the Windows tool cells fail for the two other reasons in §4. Whether
  the AppContainer backend has an analogous overlapping-deny problem is **not measured** and must
  not be inferred either way.
- **A measurement error of my own, worth recording.** For roughly 35 minutes I polled
  `Get-Process cargo,rustc` on `SeanDesktop` and read a non-zero count as "my build is
  progressing". It was not: my `Start-Process` launch had failed silently (0-byte logs, no
  target directory), and every process I was counting belonged to the two live CI
  `Runner.Worker` jobs. The count was a real number attached to the wrong subject — the same
  shape as a self-passing gate. It was caught by checking the artifact (`Test-Path
  D:\lane-21c3-target` → `False`) rather than the proxy. The build was then re-run synchronously
  and completed in 3m10s.
- **No production file under `crates/*/src` was modified.** Every change is in
  `crates/wcore-cli/tests/child_authority_corpus/`.
- **Nothing was weakened, checked mechanically** against `BASE=5457710e` with unproxied `git`:
  - `#[ignore]` under `crates/` **225 → 225**; `#[allow` **187 → 187**.
  - `git diff BASE HEAD -- crates/ | grep '^-' | grep -E 'assert|#\[test\]|#\[ignore|#\[allow|panic!|timeout|Duration::from'`
    returns **2 lines**, and both are accounted for: a comment line, and
    `match run_bounded(Duration::from_secs(45), …)` which was **re-sited, not removed** — the
    matching addition `run_bounded(Duration::from_secs(45), …)` is inside `tool_arm`, so each
    arm keeps exactly the budget the single run had. No `Duration::from_secs` above 45 was
    added anywhere. A widened budget dressed as a refactor is still a widened budget.
  - Instrument-alive control for that measurement: the same diff adds **17** lines matching
    `assert|#[test]`, so the matcher is not returning zero because it is broken.
- **The shared fence was not touched.** `git diff BASE HEAD -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is empty. The whole lane is 2 files, +944 / −92, both under
  `crates/wcore-cli/tests/child_authority_corpus/`.
- **The §3 defect was not fixed**, and no seal, merge, PR, tag or issue action was taken.
- The tool dimension still has **no decisive verdict** on any cell. Two corpus-side limits stand
  and are recorded in the probe's own withholding text: the shell leg is blocked by §3, and the
  file leg cannot be targeted because `Write`/`Read` require an absolute path while a delegated
  child's isolated checkout is allocated at `<session>/delegated-workspaces/checkouts/<worker_id>`,
  which a scripted corpus cannot know before the child launches. Closing that needs either a fix
  for §3 or a way for a probe to learn the checkout root — both out of this lane's scope.

---

## 8. Routed to the phase

| # | Severity | Item |
|---|---|---|
| 21-C3-01 | **HIGH** | Overlapping `fs_read_deny` entries abort bubblewrap; a delegated isolated-mutation child cannot run any shell command on Linux (`spawner.rs:1817` × `bwrap.rs:295`). Reproduced with a control. |
| 21-C3-02 | **HIGH** | Every `corpus_tool` REFUSED in the Phase 21 record came from a tool call that never executed, via four mechanisms, none of them the two the verdict names. |
| 21-C3-03 | MEDIUM | `ToolConfirmer` denies every delegated child's tool call under a non-TTY parent at the default `Prompt` posture. Correct fail-closed behaviour; it silently vacates any in-process child-authority test that does not allow-list its probe tool. |
| 21-C3-04 | MEDIUM | The tool dimension is unmeasurable by a scripted corpus until a probe can learn the child's isolated checkout root (§7). |
| 21-C3-05 | MEDIUM | On Windows the tool differential's granted arm expires on the 45 s bound — the session-0 confirmer prompts on a console nobody is attached to (`21-05` §1.3 defect (d), still open). The dimension is therefore unmeasurable on Windows for a third reason. |
| 21-C3-06 | — | Whether the Windows AppContainer backend has an overlapping-deny analogue of 21-C3-01 is **not measured**; bubblewrap is Linux-only. |
