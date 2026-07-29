# LANE BRIEF — Wayland Core frontier execution (read fully before acting)

You are ONE LANE of five running concurrently. You own exactly one phase. Execute its
remaining plans **in wave order, strictly serially**, to completion.

---

## 0. Hard boundaries

- **NEVER touch `/Users/seandonahoe/dev/waylandcore`.** It is a different, heavily-dirty
  checkout. Your first action is to verify `git rev-parse --show-toplevel` and abort if it
  resolves there.
- **NEVER run cargo on the Mac.** The exceptions are `cargo fmt --all -- --check`, and the
  **narrow Darwin-behaviour exception** below. All compilation, tests and clippy run on hetzner
  (see §2).

  **Darwin-behaviour exception, added 2026-07-29.** The rule exists because full builds on the Mac
  are slow and were causing real problems — not because Darwin behaviour is uninteresting. But **no
  permitted host runs macOS**, so a behaviour that only Darwin exhibits was unprovable, and a lane
  correctly named that as a **rule-imposed gap rather than papering over it**. So:

  A **single-crate, single-test** run IS permitted on the Mac when the thing under test is
  **platform behaviour Darwin alone can demonstrate** — `cargo test -p <crate> --test <file>`, never
  a workspace build, never clippy, never a release build. **Say in your summary that you used it and
  why.** If you find yourself wanting it for anything that hetzner could have proven, you do not
  qualify.

  This was granted after a lane measured macOS liveness semantics **in C** to work around the rule,
  and found the old check wrong in *both* directions there — a result worth having in Rust.
- **Never print, echo, or transmit a secret value.** If a plan needs a real credential and
  none is configured, report it as a blocker in your SUMMARY — do NOT embed one.

  **Sanctioned exception, added 2026-07-29 because the rule as written forbade something
  necessary.** A fix that needs a real provider cannot be live-proven on the Mac, because the
  Mac cannot build. So a credential MAY reach hetzner, under all of these:
  - **injected on stdin only** — never in `argv`, never written to disk, never into a log,
    an evidence capture, a commit or a summary;
  - **swept for afterwards** — grep every changed file and every capture against the live value
    and report the hit count (expect `0`);
  - **disclosed in your SUMMARY**, naming the machine and the method.

  A lane did exactly this, flagged it against the old wording rather than burying it, and was
  right to. **Flagging a rule that blocks necessary work is the behaviour to want** — silently
  routing around it is not, and neither is abandoning the proof.
- **Reserved to Sean — never do these:** merging to `main`, opening a PR, tagging, releasing,
  closing a GitHub issue, deleting a retained evidence ref, supplying real credentials.
  Committing and pushing **your own lane branch** is expected and fine.
- **Do NOT run `wcore-contract generate`.** Regenerating Desktop wire-contract fixtures is a
  release-coordination action. If a plan needs a contract change, write a fenced seam request
  into your SUMMARY instead.
- **Never `git add -A`.** Stage only paths your plan declares. Never `git checkout`,
  `git reset`, `git stash`, or `git rebase` — other lanes share the repository object store.
- No `Co-Authored-By` trailers.

## 1. Your workspace (Mac side)

Create your own worktree. Do NOT use the Agent tool's `isolation: worktree` — it forks from
the wrong repo.

```bash
git -C /Users/seandonahoe/dev/waylandcore-ferrox worktree add -b lane/<PHASE> \
  /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-<PHASE> \
  plan/f20-unified-audit-repair
cd /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-<PHASE>
git rev-parse --show-toplevel   # MUST print the lane path above
```

Commit to `lane/<PHASE>` only. **Do not merge into `plan/f20-unified-audit-repair`** — the
orchestrator merges lanes serially. Push your branch with `git push gh lane/<PHASE>` (remote
is `gh`, not `origin`).

**Merging integration INTO your branch is required and fine. Pushing the result to integration
is not.** These are one keystroke apart and a lane conflated them on 2026-07-29: it merged
integration forward correctly, then pushed to `plan/f20-unified-audit-repair` instead of to its
own ref. The push also dragged in a *second, unrelated* lane's in-flight branch that happened to
be an ancestor — including a fix that had not yet been proven at the time.

Nothing broke, because that fix was proven an hour later. **That is luck, not a process.** The
orchestrator merges serially so that each lane's work is verified against a tree it has actually
been tested on; a lane that self-merges removes that check silently and no one can tell from the
log which merges were reviewed.

Concretely: `git push gh HEAD:lane/<your-lane>`, never `git push gh HEAD:plan/...`. If you find
yourself typing the integration branch name in a push command, stop.

## 2. Your workspace (hetzner — where code actually builds)

```bash
ssh hetzner-dsm
git -C /root/wayland worktree add -b hz/<PHASE> /root/wayland-<PHASE> <your-commit>
export PATH=/root/.cargo/bin:$PATH      # a bare `cargo` exits 127 — that is PATH, not a build failure
```

- `cargo` is NOT on a non-login shell's PATH. Use `/root/.cargo/bin/cargo`.
- **Build targeted: `cargo test -p <crate>`, never a bare full-workspace build.** Five lanes
  running full builds is what filled the disk and took sshd down for hours previously.
- A connection timeout is **load to back off from**, not a dead host. Sleep and retry; do not
  escalate concurrency.
- Check `df -h /root` before a big run. If free space drops under ~150G, prune your own
  `target/` first and say so.
- When your phase is done, remove your hetzner worktree and its `target/`.

## 3. The three standing rules (AGENTS.md §11 — established by measurement)

1. **Live testing ranks at least as high as green code.** A phase is not done because the
   suite is green. Exercise the real `wayland-core` binary and, where the plan touches it,
   the real TUI. Phase 20A reached CI-green on Windows and macOS without anyone launching the
   binary; a later live pass found three HIGH defects in that same build.
2. **Gates must be able to fail.** Before you trust any gate you write or run, ask whether it
   could fail. Known self-passing classes, all found in this repo:
   - a pipe steals exit status (`cmd | grep -v X` reports grep's status, not cmd's);
   - `git status --porcelain` and `git diff --stat` exit 0 unconditionally;
   - grepping an evidence file that the executor itself writes is a tautology;
   - the gate was already green at base, so it proves nothing was done;
   - `git diff --name-only` is blind to untracked files;
   - **a suite whose tests are all `#[ignore]` exits 0 printing `test result: ok` having run
     ZERO tests** — `cargo test --test live_fs_acl` did exactly this on 0 of 12. Assert the
     executed count (`N passed` where N is the number you expect); never trust exit status.
     **Three flavours of this are now measured, and only the first is visible from the
     attribute list:** (a) every test `#[ignore]`d — 15 binaries in this repo, distinct from 12
     with *some* ignored, which is normal; (b) an **env-gated early `return`** —
     `live_integrity.rs` printed `5 passed` for zero work; (c) a **filter that matches no test
     name** — `cargo test -p wcore-cli migrate` exited 0 having run 0 tests. Flavour (c) is the
     easiest to write by accident and the hardest to notice, because the command *looks*
     targeted. Always read the `N passed` count back;
   - **an exit status that crossed ssh to Windows is only one bit.** Every non-zero collapses
     to **1** (measured: 2, 3, 7, 100, 255 all arrive as 1), so `rc == 1` passes for every
     failure mode you meant to distinguish. `$LASTEXITCODE` is fine *inside* PowerShell — it
     dies at the session boundary. Stdout sentinels are not enough either: CLIXML progress
     records splice into the stream and a status line can vanish while its marker survives.
     **Verified pattern:** remote writes `WLRC=<code>` first and `WLDONE` last to a status
     file; a *separate* ssh call reads it back; exit status is ignored entirely. Grade three
     states — no marker = incomplete, marker without status = **UNREADABLE**, both = true
     code. Verified 7/7 over 0/1/2/3/7/100/255. Poll for `WLDONE`, not for process absence.
     Brace variables in sentinels (`"WLRC=${rc}"`): `"$LASTEXITCODE:TAG"` renders **empty**,
     because PowerShell reads `$VAR:` as namespace notation.
     Full evidence: `.planning/phases/28-*/evidence/28-kr01-repair/F-WR-06-EXIT-STATUS-PATTERN.md`.
   Run the linter over anything you author: `python3 .planning/scripts/lint-plan-gates.py <dir>`.
3. **Decide, do not park.** A `checkpoint:decision` or `checkpoint:human-verify` in a plan is
   an instruction to decide *well*, not to stop. Cross-audit it (§4), commit to an answer,
   record the evidence and the dissent, and proceed. Escalate only on genuine deadlock, and
   bring the split with you.

## 3b. `rtk` rewrites tool output — measured, and wider than first thought

`rtk` proxies shell tools and **re-renders their output**. It returns rc=0 with well-formed text,
so nothing looks wrong. Two classes measured:

- **`git log`** — silently drops merge commits. One lane's merge-base came back wrong; had it been
  used, every fence number in its report would have been false.
- **`grep`** — also rewritten. Found 2026-07-29 by a lane whose extractions were being altered
  underneath it. Another lane's `git diff` came back re-indented by two spaces, which blinded a
  `^-` removal-line matcher and produced a false "no removals" fence result.

- **`cargo`** — also rewritten, and this one is the worst of the four. It reports the pass count
  correctly and **strips `0 ignored` and `0 filtered out`** — the exact two fields §3.2 requires you
  to read back to catch a suite that exits 0 having run nothing. **So the proxy silently removes
  the evidence the anti-vacuity rule is built on.** Measured 2026-07-29.

- **`ls`** — also rewritten: it alters the size column and reorders entries. Found 2026-07-29 by
  `lane/22-remaining`. Anything that counts or sizes files from a directory listing is affected.

**Assume the list is incomplete.** Four tools were found in five days, each after a lane trusted it.
The safe posture is that *any* proxied tool may re-render, so reach for the absolute path first
rather than discovering the fifth member the hard way. The orchestrator hit this too on 2026-07-29:
`git log` reported an unchanged HEAD immediately after a 24-branch merge train had moved it, and the
truth came from `command git reflog` plus a filesystem check.

**Rule: anything load-bearing goes through `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/env cargo`
(absolute path), or `rtk proxy`.** That
means merge-bases, fence diffs, evidence extraction, panel-vote extraction, and any count you will
report. Convenience reads are fine unproxied; measurements are not.

**If a number will appear in your summary, it must come from an unproxied tool.**

### 3b-i. A known-negative assertion is SELF-PASSING on a dead instrument

**Measured 2026-07-29, and it is the sharpest instance of the class this program has produced.** An
unpiped `grep` was being rewritten by a hook: it reported **"9 matches in 7 files"** for a
*one-file* search whose true answer was **zero**. Now notice what that means for the shape of the
claim it was serving — the lane's central measurement was *"grep for a revoke surface returns
zero."*

**A broken grep produces a zero for free.** So does a typo'd path, a wrong flag, an unquoted glob
zsh ate, a search of the wrong tree, and a tool that silently returns nothing. **Every one of those
confirms a negative claim.**

So: **"X does not exist" is the single easiest assertion to pass without doing any work**, and this
program has now been wrong about at least three of them — a concept declared absent that existed
under other names, a capability declared unreachable that was reachable, and a "sole path" that had
three.

**Before you report ANY absence:**

1. **Prove the instrument alive on a known-positive in the same invocation** — search for something
   you are certain is there, and show a non-zero count.
2. **Use an unproxied tool** (`/usr/bin/grep`), and quote your globs — zsh eats `--include=*.rs`.
3. **Search for the CONCEPT, not one keyword.** Vocabulary differs: liveness machinery existed under
   `owner_pid` / `owner_is_live` while a grep for `stale` returned nothing and was reported as
   proof the concept was absent.
4. **State the search you ran**, so a reader can re-run it. An absence without its query is not a
   measurement.


## 4. Cross-audit panel (for checkpoints and any judgement call)

```bash
codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<q>"
gemini -p "<q>" -m gemini-3.1-pro-preview -o text --skip-trust
/Users/seandonahoe/.kimi-code/bin/kimi -p "<q>" --output-format text
```
Plus one internal adversarial pass arguing AGAINST the emerging consensus.

**Each of these silently drops a vote if you invoke it wrong** — the same defect class as a
self-passing gate: gemini returns nothing without `--skip-trust`; kimi bullet-prefixes and
indents, so an anchored `^PANEL_POSITION=` regex loses its vote (extract unanchored); codex
repeats its final block, so take the LAST match. Use kimi's absolute path — a Bash-tool shell
predates `.zshrc`'s PATH. A one-word probe passes despite all three; probe with a real question.

Take the majority, or the minority when it carries stronger evidence — and say which and why.

## 5. Honesty rules — these outrank finishing

- **Never weaken a test to reach green.** No `#[ignore]`, no `#[allow]`, no re-gating, no
  deleting a test, no raising a timeout. **A reported red is worth more than an engineered
  green.** If something is genuinely red, report it red with the output.
- **Never redefine success downward when you hit a wall.** An agent on this program invented
  a "termination state 4" to dodge an artifact it wrongly believed unobtainable. If a leg
  looks impossible, say so plainly and prove the impossibility — do not invent an exit.
- Severity policy: **CRITICAL/HIGH must be fixed, or disproved with executable evidence.**
  MEDIUM and below go to BACKLOG, non-blocking. Do not invent a stricter rule — that is what
  turned Phase 20 into a 74-plan loop lasting two weeks.
- If your phase's goal is not achieved, **say so**. Every phase that graded itself honestly on
  this program produced more value than one that claimed a green. Phase 21 declared its own
  goal NOT ACHIEVED twice and that was the correct, useful outcome.

## 6. Shared-file fence (two files only)

`crates/wcore-cli/src/lib.rs` and `crates/wcore-cli/src/main.rs` are touched by **every**
lane. There:
- additive edits only, minimal, in one contiguous block;
- no reformatting, no reordering, no drive-by cleanup of surrounding code;
- do not rename or re-sort existing registrations.

Everything else in your plan's `files_modified` is yours alone.

**Diff your work against the MERGE-BASE SHA, never against the branch name.** A gate written as
`git diff plan/f20-unified-audit-repair -- <paths>` re-reads that branch *as it is now*, so every
other lane merged since you branched is attributed to you. Lane 24d's fence gate reported 28
deletions it had never made, purely from this. Use:

```
BASE=$(git merge-base HEAD plan/f20-unified-audit-repair)
git diff "$BASE" -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
```

Capture `$BASE` **once**, at the start, and quote it. The integration branch moves under you while
you work — expect it to.

**A full-workspace run taken while other lanes are building is not a measurement.** Two known
contention artifacts on `hetzner-dsm`, both measured:
- **~20 `wcore-skills` watcher tests fail with `Too many open files` (EMFILE)** when several lanes
  run at once. Cause is `fs.inotify.max_user_instances`, not fds and not your change — the default
  128 was ~109-held with six lanes live. Raised to 512 at runtime on 2026-07-27 (resets on reboot;
  re-apply with `sysctl -w fs.inotify.max_user_instances=512` if it returns). The same suite passed
  **669/669 in isolation at the identical commit.**
- Wall-clock-budgeted binary tests go flaky under full-suite load — see `.planning/BACKLOG.md`.

So: if a full-workspace run shows a cluster of failures in one crate, **re-run that crate alone at
the same commit before reporting a regression.** Report the isolated number, and say which run each
figure came from.

**The Windows account is `SeanD`. `ssh SeanD@seandesktop` works.** Two lanes reported
`seandesktop` as an access blocker after trying `sean`, `seandonahoe`, `sdonahoe`, `wayland` and
`Administrator` — none of which exist. Verified working 2026-07-28: `ssh -o BatchMode=yes
SeanD@seandesktop 'hostname'` → `SeanDesktop`, rc=0. Every plan in this program already spells it
`SeanD@seandesktop`; use that spelling and **never report a host unreachable without first trying
the account the plans use.** A wrongly-reported blocker costs a Sean-reserved round trip for
nothing.

Note `hetzner-dsm` genuinely **cannot** reach `seandesktop` (`Permission denied (publickey)`) —
that one is real, and is a separate authorization request pending with Sean. Do not conflate the
two: your Mac reaches both hosts; the two hosts cannot reach each other.

### The Mac is NOT an ssh host — you are already on it

`sean-mac-arm64` is a **GitHub Actions runner label**, not a hostname. `ssh sean-mac-arm64`
(or `sean-mac`, or `seanmac`) fails with `Could not resolve hostname`; only `hetzner-dsm` and
`SeanD@seandesktop` are reachable by ssh. A lane correctly probed this rather than assuming, after
an orchestrator brief implied the Mac was an ssh target.

**Darwin work runs locally, where your shell already is.** That is why the §0 Darwin exception is
worded as an exception: compiling on the Mac is normally forbidden, and the carve-out is for one
crate and one named test where the behaviour under test is genuinely Darwin-only. A macOS leg that
needs a *full workspace build* is still forbidden — say so and leave it rather than reporting the
host unreachable, which is a different and false claim.

### Orchestrator messages do not override this file

If an orchestrator instruction conflicts with a rule here, this file wins and you should say so.
Precedent, 2026-07-29: an orchestrator told a lane to "rebase onto integration" when §0 forbids
`git rebase` outright. The lane merged instead, reached the same stated end — a branch that merges
cleanly — and flagged the conflict. That was the correct handling.

### On SeanDesktop, work on `D:\` — NOT `C:\` (Sean, 2026-07-29)

`C:` is 1862 GB with only **167 GB free**. `D:` is 7452 GB with **5413 GB free**, and `E:` is
1863 GB essentially untouched. **New working directories go under `D:\`.**

**We are why C: is full.** Lanes have been creating working directories at the *root of* `C:\` —
~50 of them (`ferrox-win` 174 GB, `wayland-dev` 147 GB, `ferrox-win-f0403`, `ferrox-win-p21`,
`wl-test-99`, `p22*`, `wl-uat-*`, `f2[0-8]*`, `C:\tmp`) totalling **512 GB**, of which **471 GB is
Rust `target/`**. The evidence directories are ~0 GB — the cost is entirely build output.

Rules:
- Do not create directories at the root of `C:\`. Use `D:\<lane-name>\`.
- **Never delete `C:\actions-runner-{core,ferrox,wayland}`** — three live self-hosted runner
  services. Check for running `Runner.Worker`/`cargo`/`rustc` before any cleanup; two CI jobs were
  mid-build when this was written.
- Clean up your own `target/` when your lane ends. Do not delete another lane's tree.

## 6a-i. A concurrency test self-passes when a PARTICIPANT NEVER STARTED

New shape, found 2026-07-29 by `lane/wal-followups` while proving SQLite WAL corruption.

Its first run failed to reproduce and **read as a clean pass**: 45,342 writes, `integrity_check=ok`,
both processes exited 0. The truth was that writer A had died at `open` with `database is locked`
(the schema migration's exclusive lock), so **only one writer ever ran** — and a concurrency defect
cannot appear with one participant. The run was not a negative result; it was not the experiment.

**Assert that every participant reached a START marker, not merely that the run exited.** The same
shape covers any test whose defect requires N simultaneous actors: contention, races, fleet
dispatch, lease contention, multi-writer anything. Exit status tells you nothing about how many
actors were present.

Generalises the existing rule: a known-negative is self-passing on a dead instrument, and *an actor
that never launched is a dead instrument*.

## 6a-ii. `/tmp` on hetzner is SHARED between lanes — your glob will catch their files

A lane on 2026-07-29 ran a post-fix check that reported its defect still present. The evidence
came from `/tmp/final-types.log` — **another lane's file**, caught by an over-broad glob. Many
lanes run on `hetzner-dsm` at once and they all share `/tmp`.

Write evidence to a path unique to your lane (`/tmp/<lane-name>-*` or inside your worktree), and
**scope every glob you read back.** A measurement that silently includes another lane's output is
not your measurement, and it can point either way — a false red as here, or a false green.

## 6b. A silent wait looks exactly like a hung agent — this has killed four lanes

A stream watchdog kills an agent after **600s with no output**. A polling loop that prints
nothing is indistinguishable from a stall, so it gets killed mid-run. Do NOT write:

```bash
until ./poll.sh | grep -q DONE; do sleep 90; done     # silent for however long it takes
```

Emit on every iteration, and bound the loop:

```bash
for i in $(seq 1 40); do
  if ./poll.sh | grep -q DONE; then echo "DONE after $((i*30))s"; break; fi
  echo "waiting: iteration $i, $(date +%H:%M:%S)"; sleep 30
done
```

Prefer several short polls you return between over one long blocking wait, and **commit and
push everything you have established before starting any long wait.** Treat a long poll as a
checkpoint boundary. Four lanes died mid-run on 2026-07-28; every one was recoverable only
because the worktree was checked before assuming, and one had **eleven files of uncommitted
work** one dropped connection from being lost.

### 6b-i. Commit a NOTES file inside your first 15 minutes — non-negotiable

**Two more lanes died on the night of 2026-07-28, and both lost EVERYTHING.** Not eleven files —
zero commits, zero dirty files, after more than an hour each. Both were in an investigation
phase: reading source, measuring call sites, sweeping for a pattern. Neither had written
anything down yet, because the writing-down was going to happen "at the end".

There is no partial credit for uncommitted reasoning. So:

- **Within your first 15 minutes, commit `<phase>-NOTES.md`** into your plan's evidence directory
  — what you have measured, what you concluded, what you still need to establish.
- **Append and re-commit after every measurement**, not at the end of the investigation. If you
  die at minute 90, the resume must start from minute 85.
- This applies **hardest** to pure-investigation work, which feels like it has nothing to commit.
  That feeling is exactly what made both lanes unrecoverable.

A measured refusal is a result. If your investigation concludes the thing is not buildable, that
conclusion — with the call sites that make it so — is the deliverable. Write it down and commit
it *before* you attempt the build, not after it fails.

### 6b-ii. Noting a gate defect without fixing the instrument is not a fix

**Measured 2026-07-29, and it is the cleanest demonstration this program has produced.** A lane
recorded an under-detection in its own harness — a console line wrap put a newline inside the
phrase its matcher searched for, so the matcher reported absence while the raw log contained the
string four times. It **wrote the defect up and moved on without repairing the harness**. Its next
lane hit **the identical defect again**, reporting `size_error=False` against a log containing the
error four times.

So:

- **When you find a defect in your own instrument, repair the instrument in the same lane.** A
  written-up instrument defect is a defect you have agreed to keep.
- **Give the repaired instrument a self-test with three assertions, not two:** known-positive
  passes, known-negative fails, **and the old broken matcher would have missed it**. That third
  assertion is the only one that proves the repair does anything — without it the self-test passes
  on the broken instrument too.

This is the eleventh recorded instance of an instrument carrying the defect class it hunts, and the
first proven to have **recurred because the earlier sighting was documented rather than fixed**.

**Two more measurement traps established the same day, both of which silently destroy a
result rather than failing loudly:**

- **Over ssh+PowerShell, every non-zero exit collapses to `1`.** Measured across 2, 3, 7, 100
  and 255 — all arrive as 1. A Windows leg cannot distinguish "failed" from "failed the way we
  predicted" over that transport. Write the real code to a file the caller reads, plus an
  explicit completion marker.
- **A suite can exit 0 while running zero tests.** `cargo test --test live_fs_acl` prints
  `test result: ok` having run **0 of 12** — they need `-- --ignored`. Assert the executed
  count; never trust the exit status alone.

## 7. Per plan, deliver

1. Execute every task in the plan, in order, with atomic commits.
2. Run the plan's own gates. Read the output. Do not claim a pass you did not read.
3. Live-exercise what the plan built (§3.1) and record the actual transcript/output.
4. Write `<PLAN-ID>-SUMMARY.md` next to the PLAN file: what landed, what the gates showed,
   the live evidence, deviations with reasons, anything still open, and an honest verdict on
   whether the plan's criteria were met.
5. Commit and push before starting the next plan. Never leave a plan's work uncommitted —
   agents on this program died mid-write repeatedly and partial state on disk is the norm.

## 8. Report back (final message)

Keep it under ~40 lines: per plan — landed / partial / blocked, the honest verdict, gate
results with real numbers, live evidence one-liner, any HIGH finding, your lane branch name
and HEAD SHA, and anything the orchestrator must serialize (protocol seams, contract
requests, shared-file edits). State clearly what you did NOT do.

---

## §3b-ii — hetzner injects a provider credential you did not set

**Added 2026-07-29 after `27-media-intake` nearly published a false live proof.**

`/root/.wayland/.env` on `hetzner-dsm` injects **`ANTHROPIC_API_KEY`** into the product's process
**regardless of what you `unset` in the shell.** A lane proving "provider X was selected" can
therefore be silently running on a different arm than the one it believes.

That lane's first live vision proof ran on **arm 1 (Anthropic)** while it believed it was proving
**arm 5 (Flux)**. It was caught only because the lane read the *resolver's own arm line* back out
of the log instead of trusting its environment setup.

**The rule:** if your claim depends on which provider, backend or credential was selected, **read
the selection back from the product's own output** and assert on it. Do not infer it from what you
exported or unset. An env var you did not set is not a hypothetical on this host — it is the
default.

This is the same family as the self-passing assertion: the environment you *think* you configured
is an unverified premise, and on this box it is a **false** one.
