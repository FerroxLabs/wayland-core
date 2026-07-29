# Phase 25 Criterion 1 repair — NOTES (running log, lane `25-c1-cleanup`)

Started 2026-07-29. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-25-c1-cleanup`, branch
`lane/25-c1-cleanup`, base `fd22dbf4196b3114ec679b66d8c96efcb3707e1a` (= `lane/grade-25` HEAD,
verified with `/usr/bin/git rev-parse`).

**Mandate:** close the two named gaps in `25-PHASE-VERDICT.md` Criterion 1.

- **Gap 1b (priority, a LIVE DEFECT):** the ssh remote runner leaks its task root, including
  `input.bin` (the task's input bytes), on every failing task. Fix it and prove the leak gone
  with a before/after count on a real ssh target.
- **Gap 1a (never exercised):** cloud cancellation was never driven at any commit. Drive it.

Not mine: Criterion 4 (egress / secret denial) — lane `25-c4-egress` owns that.

---

## t0 — the defect located in source, before any build

`crates/wcore-exec-backend/src/backends/ssh.rs:186-205`, `const REMOTE_RUNNER`:

```sh
set -eu
...
setsid "$@" &
child=$!
echo "$child" > "$root/.pid"
wait "$child"          # <-- under `set -e`, a NON-ZERO status aborts HERE
status=$?
rm -rf "$root"         # <-- never reached when the task exits non-zero
exit "$status"
```

So: **every task that exits non-zero leaves `${TMPDIR:-/tmp}/wayland-f25-<nonce>/` on the far
end, containing `input.bin` (the task's input bytes) and `.pid`.** The success path cleans up;
only the failure path leaks. That matches `25-HOSTS-SUMMARY.md` FINDING 5 ("six such roots were
found on the node and purged by hand"), which is still `BACKLOG`/unfixed at this base.

### The fix I will NOT make, and why (recorded before I write the fix)

The obvious "robust" fix is `trap 'rm -rf "$root"' EXIT`. **That would be a regression, not a
fix.** When the controller is killed `-9` the runner `sh` dies on SIGHUP while the `setsid`
child deliberately survives — that is exactly the *unplanted* positive control Criterion 4's
no-orphan half rests on (`evidence/25h-ssh-orphan-ledger.txt`, pid 1170). An EXIT trap would
`rm -rf` the root out from under that surviving orphan, deleting `$root/.pid` — the **primary
signal** `REMOTE_SCAN` uses (`ssh.rs:471-476`). Criterion 4 is MET on that half; I am not
trading it for Criterion 1.

So the repair is scoped to the path where the child has **already exited**: take `wait` out of
`set -e`'s reach, keep the status, then clean up.

Cancellation is already covered: `REMOTE_KILL` (`ssh.rs:549`) ends with
`rm -rf "$root" 2>/dev/null || true`, and the wall-clock timeout path routes through
`self.cancel()` (`ssh.rs:332`).

## Still to establish

- [ ] Leak reproduced live on a real ssh far end at BASE (count > 0, `input.bin` present).
- [ ] Instrument proven alive: a known-positive (a planted root) must be counted, and a
      successful task must count 0 at the same commit — otherwise "0 roots" is free.
- [ ] Leak gone at FIXED commit (count 0) with the same failing task.
- [ ] Exit status still propagates after the fix (the failing task must still report its code).
- [ ] Cloud cancellation driven live: terminal state + receipt + post-cancel machine count.
