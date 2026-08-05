# Phase 24 — execution report and Success Criterion grading

Date: 2026-07-26.
Branch: `worktree-wf_b7d743bd-954-4` (repo `/Users/seandonahoe/dev/waylandcore`,
worktree `.claude/worktrees/wf_b7d743bd-954-4`), based on
`2ecdfdf54ff7fda920eec7d068337006e5da4ee4`.
Commits: `a701a8a0`, `80ef6d44`, `b22e3ecc`, plus this report.

## Bottom line

**Phase 24's goal is NOT achieved.** Of four dispatched plans, one ran and
did not reach its own Complete state. Three did not start. Four of the five
Success Criteria have no evidence at all. Nothing here should be read as
partial credit toward closing the phase.

The phase produced three things worth keeping: a working delivery-continuity
spine proved at unit level, one HIGH production defect found by hardware
measurement and fixed, and one cross-audited mechanism decision bound to
that measurement.

## Plan status

| Plan | Wave | Status |
|---|---|---|
| 24-01 Gateway runtime | 1 | **Executed, incomplete.** 3 of 4 tasks. `24-01-SUMMARY.md`. |
| 24-02 Automation plane | 2 | **NOT STARTED.** |
| 24-03 Channels + typed API | 2 | **NOT STARTED.** |
| 24-04 Tri-platform journey | 3 | **NOT STARTED.** Its terminal task is Sean-reserved regardless; nothing before that gate was reached either. |

Why: 24-01 alone is a four-task plan requiring a new crate, a hardware
measurement campaign on two remote hosts, a four-way panel, and a live
multi-platform operator journey. Its wave-1 seam-ownership premise also
collided head-on with this execution's fenced-file list — 24-01 declares it
owns every Phase-24 shared-seam edit, and five of those files are fenced.
One executor did not get through it. That is a scoping fact, reported rather
than absorbed.

## Success Criterion grading — verbatim from ROADMAP.md

> **1. Native service lifecycle, profile isolation, active-turn visibility, drain, restart, upgrade, and rollback work without lost or duplicate delivery.**

**NOT MET, on any platform.**

- *drain* — MET as a mechanism: an ordered, observable state (close
  admission → publish counts → bounded wait → flush → clean-or-forced) with
  falling in-flight counts and named abandonment, tested.
- *without lost or duplicate delivery* — MET at unit level only. 200
  accepted, crash mid-attempt, restart, tally at an **independent sink**:
  200 delivered / 200 unique / 0 duplicates / 0 losses, with provably-settled
  deliveries never retried. This is an in-process sink, not a service
  restarted on a real host.
- *active-turn visibility* — MET as a projection field; never rendered to an
  operator, because no verb exists to render it.
- *native service lifecycle* — **NOT MET.** The argv per family is generated
  and asserted; none of it has been executed against a real registry, and
  the verbs are not on the shipped binary at all.
- *profile isolation* — **NOT MET.** Asserted structurally (one home, one
  lock, distinct homes do not exclude each other); no per-profile child was
  supervised.
- *restart, upgrade, rollback* — **NOT MET.** None was performed on any
  platform. The status projection carries `binary_path`/`binary_version`
  specifically so an upgrade and a rollback would be distinguishable; that
  capability was never exercised.

> **2. Scheduled, event-driven, webhook, polling, and commitment work has bounded history, retry, continuation, and delivery.**

**NOT MET. No evidence.** Plan 24-02 did not run. Note for the record: the
persistent scheduling that Phase 22 Success Criterion 4 explicitly deferred
to Phase 24 is still deferred, and is now overdue against two phases.

> **3. Reference channels prove setup/auth, access, routing, media, native actions, idempotency, reconnect/reload, and health.**

**NOT MET. No evidence.** Plan 24-03 did not run. The idempotency clause has
a durable substrate available (24-01's ledger, with the outbound-key
compatibility decision recorded) but nothing in `wcore-channels` routes
through it.

> **4. Typed authenticated clients recover event gaps and produce useful redacted health/log/support evidence.**

**NOT MET. No evidence.** Plan 24-03 did not run. `wcore-acp` is unchanged;
no support bundle exists.

> **5. Setup-to-recovery journeys pass on macOS, Linux, and Windows.**

**NOT MET. No evidence.** Plan 24-04 did not run. No journey driver, no
receipt schema, no receipt on any platform.

**Partial credit that must NOT be mistaken for this criterion:** real work
was done ON Windows hardware — five probe transcripts from `SEANDESKTOP`,
each with a verdict its own script emitted. That is a MEASUREMENT campaign
that found and fixed a defect. It is not a setup-to-recovery journey, and
the distinction is exactly the one Phase 20A got wrong.

## What is genuinely broken, and what is genuinely fixed

**Fixed, with executable evidence.** `crates/wcore-cli/src/cron.rs`'s
non-Unix spawn branch set no process creation flags. Measured: a child
spawned through that exact path wrote **1 of 600 heartbeats** and was killed
when its session ended; with `DETACHED_PROCESS |
CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB` the same probe wrote
**600 of 600** and exited normally. Severity HIGH — every
`wayland-core cron daemon` ever started over a remote session on Windows
died the moment that session returned, and nothing reported it.

**Still broken / unverified, in priority order.**

1. ~~**The Windows arms of the new code have never been compiled.**~~
   **CLOSED (`8b582851`).** `wcore-gateway` was compiled, linted and tested
   on real Windows: 42 tests green including the 8 hostile pidlock cases
   against actual `LockFileEx` mandatory locking, clippy `-D warnings`
   clean. It found one genuine Windows-only red — a `/opt/...` literal
   passed to `is_absolute()`, which is false on Windows — now fixed with a
   per-family assertion plus the drive-relative trap that was missing.
   **Residual:** the `wcore-cli` `cron.rs` `creation_flags` CALL SITE is
   still unbuilt on Windows (the crate was only built on Linux, where that
   block is inert). The flag values are pinned by a test that does run on
   Windows and the identical set was measured working in the probe, but the
   call site itself is unverified. That remains the integrator's first
   check.
2. **Criterion 5's recovery clause is unmeasured for the authorized Windows
   mechanism.** Task Scheduler's restart-on-failure is capped and delayed,
   and is genuinely weaker than an SCM recovery policy. The panel asserted
   equivalence; it is not equivalent. Whether the platform brings the
   runtime back after a hard kill is unknown.
3. **`win-service-scm` was excluded on a probe artifact, not a property.**
   `sc start` returned 1053 because a sixty-line console probe cannot answer
   the service control handshake. If a handshake-capable binary is ever
   built, that decision deserves reopening on the merits.

## Process observations worth more than the code

**A unanimous four-way panel agreed on evidence that did not show what they
said it showed.** All three external members read the `schtasks` transcript
as proof of a "session-independent parent". Its observation window sat
INSIDE the spawning session, so it measured "the task runs", not "the
process outlives the session" — the property the option actually needed.
None of the three noticed. The conclusion survived only because a fifth
probe was written afterwards to take the missing measurement. **Weight panel
captures by what each MEASURED, not by the tally.**

**Two probe verdict rules were caught scoring the wrong thing**, in opposite
directions: one reported `dies` for a child that had SURVIVED and finished
(frozen heartbeat at the terminal beat); one reported `survives` off a
successful registration when survival had never been observed. Both were
corrected in the artifacts and both corrections are recorded. The second
correction removed an option from the choosable set — it made the decision
harder, which is how you can tell it was honest.

## Environment defect the orchestrator must know about

The worktree supplied for Phase 24 was cut from
`/Users/seandonahoe/dev/waylandcore` at release commit `61b79c4f`, which has
no `.planning/` directory. The live program is in a different clone,
`/Users/seandonahoe/dev/waylandcore-ferrox` on
`plan/f20-unified-audit-repair`, which was being actively committed to
during this session. Resolved by fetching that branch into the (clean,
unstarted) worktree and resetting onto `2ecdfdf5`.

**Consequence for integration:** Phase-24 commits are on branch
`worktree-wf_b7d743bd-954-4` **in the `waylandcore` repo, not in
`waylandcore-ferrox`.** A push from that worktree to the ferrox repo is
refused by a secret-scanning ratchet on pre-existing history (commit
`d06a6051`), so transport must be by `git bundle` or `git fetch` between the
two clones. If the other six phases were dispatched the same way, they are
in the same position.

## Reserved to Sean

- Merging, pushing, PR, tag, release, issue closure — none performed.
- 24-04's terminal acceptance task — never reached.
- **An interactive logon on `SEANDESKTOP`** when a Windows gateway install is
  eventually exercised: the authorized scheduled-task registration reports
  `Logon Mode: Interactive only`. Narrow and named; not a decision gate.
