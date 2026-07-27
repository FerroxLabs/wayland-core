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
4. **NEW — a live test whose scenario is too clean cannot reach the defect, and a comparison
   must be taken while the two sides can still disagree.** `gateway drain` hung forever with
   carried work yet **passed its first live journey**, because that gateway had zero pending
   deliveries. Separately, a `SCANNER-AGREEMENT` gate printed `AGREE scanner=0 manual=0` while
   running **only after the reap**, when both sides are legitimately zero — it agreed
   enthusiastically while the scanner was structurally blind.
5. **NEW — a measurement that cannot be taken must never render as `0`.** Windows orphan
   detection reported a **measured zero while an orphan existed**, because `tasklist` has no
   command-line column. Worse than an error: a zero reads as proof and everything downstream
   banks it. **The first fix reproduced the identical bug** (`Win32_Process.CommandLine` returns
   NULL without privilege) — the instrument was never the defect, the *representable bad state*
   was. Fixed in the type: `Enumerated | CannotDetermine` with **no `count()`**, plus an
   instrument self-test (the scanner must see its own process, with its own command line).

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
| **25** | **COMPLETE and merged** (`cb131278`). 25-02/03/04 all landed; **nine HIGH** found and fixed. Criterion 3 MET on Linux; **Criterion 2 NOT MET** (attribution held through all 5 disruptions but against a separate *machine identity*, not a second physical host — needs an SSH key on one of Sean's machines, §5); **Criterion 4 NOT MET** (SSH and cloud orphan surfaces report `NOT MEASURED`, and those are exactly the two with no proven reaping mechanism). |
| **26** | 26-01 + most of 26-03 landed. Exact rollback from an uncatchable mid-flight `SIGKILL` proven with THREE controls (negative, handler, positive) so `fired=no` is a measurement not a missing probe; round trip `diff -r` empty at full fidelity, redacted diff exactly `credentials.toml`+`oauth`. **26-02 and 26-04 never started; 26-03 Task 4's two panels never run, so F26-03/F26-04 are UNCLAIMED.** **HIGH F26-03-D open:** on Windows `backup create` accepts a deep tree `backup restore` cannot restore (`os error 3`, past `MAX_PATH`) — the archive is silently unrestorable on the platform that made it. Reported red, not fixed; the fixture was deliberately NOT deleted to force a green. Agent in flight. |
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

**Lane 25 — nine HIGH, and its own description is the right one: "every one a false answer, not
a crash."** `plugin sign` wrote the signature where the verifier never looks · `plugin install`
had **no path at all** for a Wayland-native plugin · `plugin remove` could not remove a
marketplace install · both shipped templates were unusable while their smoke tests skipped ·
`node probe` reported a healthy node OFFLINE · `node probe` refreshed from the **controller's**
backends · the orphan scan could not see an orphan · the scanner counted itself · the Windows
measured zero. A surface that answers confidently and wrongly is the failure mode this codebase
produces; crashes are comparatively rare.

---

## 4. Gate defects — seven new shapes caught today

- `powershell-missing-script-exits-zero` — **`powershell -File <missing.ps1>; exit $LASTEXITCODE`
  exits 0.** A Windows gate whose script is absent PASSES. Every Windows gate in this program
  runs that way, so it is the highest-leverage instance of the self-passing bug here.


`.planning/scripts/lint-plan-gates.py` gained, today:
- `unquoted-pathspec-shell-dependent` — `SEAM="a b c" … -- $SEAM`. **zsh does not word-split**,
  so it is one pathspec matching nothing and `--quiet` exits 0. Printed `SEAM CLEAN` while
  `Cargo.lock` had 3 added lines. 7 instances fixed, one in **already-COMPLETE 23A**.
- `empty-equals-empty-passes` — `test "$(shasum X)" = "$(cat Y)"` passes when both files are
  absent. The default shape of every digest/tamper check.
- `grep-rc-prefixes-the-count`, `grep-c-exit-1-breaks-chain`, `backslash-s-not-portable`,
  `gate-is-broken-not-red` (runs the gate, reads stderr).

**The linter had the disease it hunts FOUR times today**, and the pattern never varied: a rule
written against the shape it hunts, with no matching test for the shape that is **correct**.
Twice it produced false negatives (`-\w*r\w*c` also matched `-certification` inside a *path*;
`grep-c` required a bare `grep` while every real gate uses `/usr/bin/grep`), and twice false
positives (flagging gates that had already taken its own advice). **Test both directions on
every rule**, and re-verify 28/29/30 at 0 HIGH over 255 gates after any linter change —
`python3 .planning/scripts/lint-plan-gates.py .planning/phases/2[89]* .planning/phases/30*`.

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

## 5b. Merge hazard — the shared `wcore-cli` fence is NOT always a clean union

`crates/wcore-cli/src/{lib,main}.rs` are touched by every lane, and the fence keeps edits
additive — but "additive" does not mean "unionable". Measured on lane/25:

- `lib.rs` conflicted on two `pub mod` declarations. Union is correct.
- `main.rs` conflicted **mid-match-arm**: the shared closing lines (`Ok(ExitCode::FAILURE) } },`)
  sit *after* the `>>>>>>>` marker, so taking both sides leaves the FIRST arm unclosed. A naive
  union produced a tree that `cargo fmt` reported as "unclosed delimiter".

**Resolve these by reading them, and verify with `cargo check -p wcore-cli --all-targets` on
hetzner — not with `fmt`.** `fmt` parsing is necessary and not sufficient.

---

## 6. Next

1. Land the three in-flight agents (22 dispatcher wiring, 24 `wcore-acp`+24-04, 26
   F26-03-D + import/hostile).
2. **23B-03** (nine-file feature, no partial credit) and **23B-04** (needs real elapsed days).
3. Then **28 → 29 → 30**, in order; 28 cannot start until 24-27 land.
4. Open MEDIUMs worth a sweep: parallel `wcore-agent` test isolation (a *different* 13-22 tests
   fail each run; serial is clean — three agents rediscovered this independently), `gateway
   status` reporting `deliveries_pending: 0` mid-flight, Windows orphan scanner reporting a
   **measured zero** because `tasklist` has no command lines.
5. `core#254` still needs Sean's maintainer decision; our lease fix may change its relevance.

**Reserved-to-Sean items now blocking specific criteria** (nothing else is waiting on him):
- **An SSH key on one of his machines** — the only thing blocking Phase 25 Criterion 2's
  cross-machine half. It is an authorization grant, not a technical gap. Exact commands in
  `25-03-NODE-EVIDENCE.md` §7.
- **A cloud account** for 25's cloud backend; the choice was made autonomously, the account is his.
- **The coordinated Core+Desktop digest re-pin** (`SEAM-REQUESTS/CONTRACT-DIGEST-RESTAMP.md`),
  batched with `F21-04-01.md`'s three items so the set costs one release.
- **24-04's terminal publication** and `core#254`.

**Verify what landed before redoing anything.** Agents die mid-write routinely; partial state is
the norm. Three separate claims I made today were wrong and had to be corrected by measurement —
check, do not assume.
