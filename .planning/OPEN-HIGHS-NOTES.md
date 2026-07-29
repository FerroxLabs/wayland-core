# OPEN-HIGHS — running notes

Lane `lane/open-highs`, base `75babf32`, worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-open-highs`.

Notes-first per LANE-BRIEF §6b-i. Appended after every measurement, not at the end.

---

## T+0 — orientation, established by reading the tree

Read: `LANE-BRIEF.md` (full), `HANDOFF-2026-07-29.md` §3, `MILESTONE-RC.md`, `ZOMBIE-PROBE.md`.

Worktree verified: `git rev-parse --show-toplevel` = the lane path, NOT
`/Users/seandonahoe/dev/waylandcore`. Branch `lane/open-highs` @ `75babf32`.

### Instrument check done up front (§3b-i)

First `grep -rn ... --include=*.rs` returned **`no matches found`** — zsh ate the unquoted glob.
That is exactly the failure mode §3b-i names: an unquoted glob returns zero, and **zero confirms
every absence claim for free**. Had I been asserting an absence I would have "proved" it with a
dead instrument on my first command. Re-run quoted, with a known-positive control in the same
invocation:

- known-positive `fn main` → **73** hits (instrument alive)
- `owner_is_live|owner_pid|process_is_live|is_process_alive` → 41 hits

All load-bearing greps in this lane use `/usr/bin/grep` with quoted globs and a known-positive.

---

## Target 1 — Zombie liveness, macOS binary-level unproven

**Status: state established, not yet measured.**

Verified present at this base (not assumed from the doc):

- `crates/wcore-types/src/process_liveness.rs` — exists (19.8K)
- `crates/wcore-types/tests/real_zombie.rs` — exists (16.0K)
- `git merge-base --is-ancestor 797d4889 HEAD` → **YES**, so the zombie-probe lane's work is in
  this tree.

`ZOMBIE-PROBE.md` §6 states the gap precisely and honestly: Linux and Windows are proven on real
hardware against a real corpse; **macOS is proven only at kernel-semantics + algorithm level, in
C.** The Rust translation executing on Darwin is unproven. It names the single closing command:

```
cargo test -p wcore-types --test real_zombie
```

**Two routes now exist that did not exist when that gap was written:**

1. **LANE-BRIEF §0 Darwin-behaviour exception** (added 2026-07-29) explicitly permits
   `cargo test -p <crate> --test <file>` on the Mac for platform behaviour only Darwin can
   demonstrate. That is verbatim the command above. I qualify: hetzner cannot prove this — no
   permitted host executes Darwin code.
2. **`sean-mac-arm64` self-hosted runner** — queried live via `gh api`:
   `{"id":34,"name":"sean-mac-arm64","os":"macOS","status":"online","busy":false}`, labels
   `self-hosted, macOS, ARM64`. Online and **idle**.

Route 2 is fenced for me: I may dispatch to the runner but must NOT touch `.github/workflows/*`
(two lanes hold those), so I can only dispatch a workflow that already targets macOS. Route 1 is
unfenced and is the cheaper, more direct measurement. **Plan: take Route 1, disclose its use per
§0.** Check for an existing macOS workflow as a corroborating second route.

**The trap I must not fall into (§3b-i, and the lane's own headline):** containment was working
the whole time and the probes could not tell a corpse from a live process. So a green
`real_zombie` on macOS proves nothing unless assertion 3 — *the old shape would have missed it* —
is confirmed to have actually executed and passed on Darwin. I must read back the executed count
and confirm the decoy discrimination, not the exit status. `cargo` is proxied by `rtk` and
**strips `0 ignored` / `0 filtered out`** — use `/usr/bin/env cargo` absolute, or `rtk proxy`.

Also to verify, not assume: that the macOS arm's raw-offset read (`p_stat`/`p_pid` at offsets
36/40, `sizeof=648`) still holds on this machine's OS version. `ZOMBIE-PROBE.md` says an ABI drift
degrades to `Indeterminate`, never a wrong answer — so a *pass* could in principle be a pass on a
degraded arm. Must confirm the corpse reads `Dead`, not `Indeterminate`.

## Target 2 — 23A resurrection hazard

**Status: not yet started.** Coordination constraint recorded up front: `lane/23a-c1-governed` is
live and owns the promotion feature that lifts quarantine. My scope is the **hazard analysis and
the guard**, not promotion. If they overlap I defer the overlap and say so rather than racing.

## Target 3 — grades resting on a dead instrument

**Status: not yet started.** The precedent is `BL-23B-H1` (`MILESTONE-RC.md` §2 row 4): a HIGH
downgraded to MEDIUM on a harness that pointed at a dead port with a placeholder key, so no run
ever dispatched a tool event, and **non-reaching runs were silently counted as successes**. That
is one instrument with two independent self-passing defects. Task: sweep current grades for the
same shape — specifically any re-grade justified by a *negative* result.

---

## Fence exposure so far

None. No files modified outside `.planning/`. `crates/wcore-cli/src/{lib,main}.rs` untouched.
`.github/workflows/*` untouched.
