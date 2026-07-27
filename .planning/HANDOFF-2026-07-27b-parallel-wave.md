# HANDOFF — Wayland Core, after the parallel wave (2026-07-27, later)

Supersedes the operating sections of `HANDOFF-2026-07-27-parallel-execution.md`.
Repo: `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`.
Remote is `gh`. **NEVER touch `/Users/seandonahoe/dev/waylandcore`.**
Mac: `/usr/bin/git` and `/usr/bin/grep` (bare ones are rtk-proxied). `cargo fmt` is the ONLY
cargo command allowed on the Mac.

---

## 1. Standing rules (AGENTS.md §11 — all established by measurement)

1. **Live testing ranks at least as high as green code.**
2. **Lint plan gates to 0 HIGH** — `python3 .planning/scripts/lint-plan-gates.py <dir>`.
3. **Decide, do not park.** Cross-audit; do not escalate.
4. **NEW — a live test whose scenario is too clean cannot reach the defect.** `gateway drain`
   hung forever with carried work yet **passed its first live journey**, because that gateway
   had zero pending deliveries. An empty queue, a fresh profile, one worker and a zero-length
   history are all scenarios where broken code behaves correctly.

**Reserved to Sean, and nothing else:** main merge, PR, tag, release, issue closure, deleting a
retained evidence ref, real credentials. Pushing this working branch is expected.

---

## 2. Phase state

| Phase | State |
|---|---|
| 20 / 20A / 21 / 23A / 27 | COMPLETE (21 graded NOT ACHIEVED **three times**, honestly) |
| **22** | Goal kernel + task ledger landed, live-proven Linux **and** Windows. **Criterion 2 FAILED** — ledger not wired to `FleetDispatcher`, no shipped-binary proof. Agent in flight. |
| **23B** | H1 data-loss residual CLOSED; D2 fixed. **23B-03/04 not run** (23B-04 needs a leg over *real elapsed days*). |
| **24** | 24-02 + `gateway.rs` + independent delivery sink landed. **`wcore-acp` completely untouched; 24-04 never started.** Agent in flight. |
| **25** | 14 commits, agent in flight. |
| **26** | 26-01 landed (~25%). **Backup/restore requirement UNMET.** Agent in flight. |
| **28 / 29 / 30** | **PLANNED, 0 HIGH.** Not executed. 28 certifies the candidate 24-27 produce, so it cannot start until they land. |

**All phases 20-30 are now planned.** 28 = 75 gates, 29 = 84, 30 = 96, all 0 HIGH.

---

## 3. What landed today (all red-before / green-after, all merged)

**Base-level breakage nobody had noticed:**
- **CI was broken at base** — `Cargo.lock` omitted `wcore-exec-backend` + chrono, so any
  `--locked` build failed. CI uses `--locked`.
- **`gateway run` did not exist.** Every service unit `wcore-gateway` generates invokes
  `<binary> gateway run`; every install on every platform registered a unit that died with a
  clap error. Registration succeeded, service never ran, silently.
- **Every nonzero bash exit killed the session** (D1). The trigger is `is_error: true` on a
  tool with the default `Opaque` contract — the malformed-path refusal was just how the UAT
  hit it. Also reproduces on Linux; not Windows-specific.

**Security-relevant:**
- **`wcore-sandbox` tests wrote leases into the PRODUCTION lease directory**, so Windows ran
  with the sandbox **silently disabled**. `sha256(b"storage-test-sid")` matched the wedging
  files byte-for-byte. **This was also the true cause of the "AppContainer cannot be observed
  over SSH" lore, which is now REFUTED and struck from all three shared files** — `live_fs_acl`
  is 12/12 over session-0 SSH on a clean lease dir, including the test cited as establishing
  the rule. That lore had been used for weeks to discount Windows sandbox reds.
- **F21-02-02**: the child seam minted a receipt asserting `posture: managed` +
  `managed_floor_active: true` next to `approvals: bypass`. `wayland-core forge` bypasses
  bootstrap, so it was a real production path, not defence-in-depth.
- **F21-02 vacuity closed.** A parent can now sub-allocate a narrowed envelope; live-proven
  (control child 8 turns, narrowed child 3, refused on the third).

**Reliability:**
- **F-3**: `read_bounded` drained a `try_clone()` of the retained descriptor — `dup` /
  `DuplicateHandle` **share the file offset**, so two validators interleave and the loser reads
  **zero bytes from an intact file**. Elapsed time was only a proxy for racing pairs. Was hiding
  behind retry across **six** tests.
- **F-2**: a killed fanout could not be restarted at all. Now reclaims on the transaction's own
  kernel-enforced `flock` lease.
- **23B-H1**: `Some(Value::Null)` round-tripped to nothing and broke the recomputed hash.
  Write path fixed, **and** legacy journals/snapshots recovered — without loosening the
  checksum (the stored hash picks the interpretation; tamper tests pass in both modes).
- **F24-C-H1**: a delivery arrived **twice** across `kill -9` + restart, and was **invisible
  from inside** — the gateway reported `carried=1 (unknown-outcome 1)`, exactly as designed.
  Only an independent sink could see it.

---

## 4. Gate defects — the linter now catches six shapes

`.planning/scripts/lint-plan-gates.py` gained, today:
- `unquoted-pathspec-shell-dependent` — `SEAM="a b c" … -- $SEAM`. **zsh does not word-split**,
  so it is one pathspec matching nothing and `--quiet` exits 0. Printed `SEAM CLEAN` while
  `Cargo.lock` had 3 added lines. 7 instances fixed, one in **already-COMPLETE 23A**.
- `empty-equals-empty-passes` — `test "$(shasum X)" = "$(cat Y)"` passes when both files are
  absent. The default shape of every digest/tamper check.
- `grep-rc-prefixes-the-count`, `grep-c-exit-1-breaks-chain`, `backslash-s-not-portable`,
  `gate-is-broken-not-red` (runs the gate, reads stderr).

**The linter had the disease it hunts three times today.** Each time the rule was written
against the example that motivated it rather than the shape. Re-verify 28/29 at 0 HIGH after
any linter change.

---

## 5. Environment truths (corrected today)

- **A macOS binary IS obtainable** — CI builds one for every target. Verified: runs on the Mac,
  `--build-info` binds it to a source SHA. Two lanes had escalated this as a platform
  impossibility. **`.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md`.** Traps: a run can be
  `conclusion: failure` and still carry good artifacts; frequent pushes cancel *queued* runs;
  artifacts expire at 14 days.
- **CI now fires on `lane/**`**, so a lane can build binaries for its own unmerged work.
- **The contract check now runs AFTER the tests.** It used to run before, so a digest drift
  failed the job and macOS/Windows never reached the tests. **Do NOT run `wcore-contract
  generate`** — a 4-way panel went **3-0 against** re-stamping on a working branch, against my
  own leaning. Measured: only `fixture_digest` + `source_inputs_digest` moved, `schema_digest`
  did **not**. Owed coordinated re-pin is in `.planning/SEAM-REQUESTS/CONTRACT-DIGEST-RESTAMP.md`
  — **if `schema_digest` has moved by the time you read it, that analysis is void.**
- **hetzner has 96 cores**, and `/root/.cargo/config.toml` now pins `jobs = 12`. Eight agents
  each defaulting to `-j 96` is what took sshd down. High load alone is not distress — **swap
  and io-wait are the signals**.
- **`.planning/scripts/rescue-worktrees.sh`** produces patches that actually apply (verified,
  including untracked files, via a throwaway index). Used successfully today: 11 rescued, 0 lost
  when four agents died on the 5-hour limit.

---

## 6. Next

1. Land the four in-flight agents (22 dispatcher wiring, 24 `wcore-acp`+24-04, 25, 26
   backup/restore).
2. **23B-03** (nine-file feature, no partial credit) and **23B-04** (needs real elapsed days).
3. Then **28 → 29 → 30**, in order; 28 cannot start until 24-27 land.
4. Open MEDIUMs worth a sweep: parallel `wcore-agent` test isolation (a *different* 13-22 tests
   fail each run; serial is clean — three agents rediscovered this independently), `gateway
   status` reporting `deliveries_pending: 0` mid-flight, Windows orphan scanner reporting a
   **measured zero** because `tasklist` has no command lines.
5. `core#254` still needs Sean's maintainer decision; our lease fix may change its relevance.

**Verify what landed before redoing anything.** Agents die mid-write routinely; partial state is
the norm. Three separate claims I made today were wrong and had to be corrected by measurement —
check, do not assume.
