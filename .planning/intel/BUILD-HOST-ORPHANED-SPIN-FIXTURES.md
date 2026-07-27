# Orphaned busy-spin test fixtures burn cores on the shared build host

**Found:** 2026-07-27, hetzner-dsm, during the five-lane frontier wave.
**Severity:** MEDIUM (test hygiene, shared-infrastructure cost). **Not** a proven product defect
— read §3 before repeating this anywhere, because there is a tempting wrong reading.
**Assigned:** lane 22 (`wcore-swarm` is its crate and it is already in that file).

## 1. What was measured

Five processes on the build host with **PPID 1** — reparented to systemd, meaning the process
that spawned them died and abandoned them — alive **7 days 11 hours**, each consuming ~99% of
a core:

```
etimes    pid      ppid  args
646692    1748372  1     /bin/sh /tmp/.tmpYI3Wep/hung-git.sh
646566    1781322  1     /bin/sh /tmp/.tmpOkBYQu/hung-cleanup-git.sh
646566    1781339  1     /bin/sh /tmp/.tmpQCK9lY/hung-git.sh
646425    1853557  1     /bin/sh /tmp/.tmpANGN8k/hung-git.sh
646292    2022626  1     /bin/sh /tmp/.tmppzYX4z/hung-git.sh
```

That is roughly **five cores burned continuously for a week** on the 96-core box every lane
builds on. They were killed on discovery. Doing so is safe: PPID 1 with no test awaiting them.

Diagnosis context at the time: load1 = 167, but **swap 0B used, io-wait 0.0%, no processes in
uninterruptible sleep**. The box was CPU-saturated, not distressed — and a meaningful fraction
of that saturation was these five, not anyone's build.

## 2. Source

```
crates/wcore-swarm/src/worktree_tests/linux.rs:887   (while :; do :; done) &   # spawns a grandchild
crates/wcore-swarm/src/worktree_tests/linux.rs:921   while :; do :; done
```

Both simulate a hung `git` by **spinning**. A blocking wait (`sleep 2147483647` — portable to
plain `sh`; `sleep infinity` is GNU-only) simulates a hung process at least as faithfully at
**zero** CPU. Then an interrupted run leaves a harmless idle process instead of a permanent
core-burner.

## 3. The wrong reading, and why it is wrong

The tempting conclusion is "the product fails to reap process trees" — which would tie neatly
to issue #247 and to `2b662fe8 fix(sandbox): own and reap process trees`. **The evidence does
not support that**, and recording it would be a fabricated finding:

- `worktree_add_timeout_kills_tree_and_reports_preserved_residual` exists *precisely* to assert
  that the timeout kills the process **tree**. It deliberately spawns a grandchild, records the
  **grandchild's** pid, and asserts `wait_until_process_gone(pid)`.
- The far likelier cause of the orphans is that the test binaries were **SIGKILLed mid-run**.
  Agents on this program died constantly over exactly that window — spend limits, API transport
  errors, host saturation — and a SIGKILLed test harness reaps nothing.

So: a genuine test-hygiene defect with a real shared-infrastructure cost, and **no demonstrated
product defect**. Do not upgrade this to a reaping bug without an actual demonstration.

## 4. Worth checking while fixing it

Whether the product's kill reaps a **blocked** grandchild as reliably as a spinning one. If the
reap only ever worked because the process was scheduling frequently, *that* would be a real
product finding — and it would be worth having. Prove it either way rather than assuming the
change is behaviour-neutral.

## 5. Related, and still open

Windows can run with the sandbox **silently disabled** on an AppContainer ACL lease SID/profile
mismatch. Separately, a lane has reported that the standing "session-0 SSH cannot observe
AppContainer" lore may be false and the real cause a lease wedge — see
`.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md` once it lands. Both are containment-adjacent
and both are being tracked independently of this note.
