# TRIAGE — release 0.13.13 blocking set

**Lane:** L6 (read-only triage). **Base:** `origin/main` @ `509f4426b`, worktree `/root/L6-triage` on `hetzner-dsm`.
**Date:** 2026-09-04. **Author:** core lane, L6.

> This document PROPOSES. Re-milestoning, closing, labelling and merging are human decisions.
> No `gh issue edit` was run to produce it. Nothing outside this file was written.

---

## 1. How the blocking set was derived

`scripts/check-release-readiness.py` hardcodes the release it grades:

```
scripts/check-release-readiness.py:122
RELEASE_MILESTONE = "0.13.12"
```

**0.13.12 has already shipped.** Run as it stands on `509f4426b`, the gate prints:

```
OK: every `kind: defect` entry has zero core-owned criteria outstanding, and every criterion
handed to another lane names the ticket that carries it, which exists and is open.
```

exit 0. So **the release gate on the shipping branch currently certifies a release that is already
out and says nothing whatever about 0.13.13.** It is not passing because the tree is ready; it is
passing because it is pointed at the wrong release. (A sibling lane owns that constant. This lane
changed nothing.)

To get the real set the script was copied to `/root/scratch-L6-triage/crr-1313.py` with the single
line changed to `RELEASE_MILESTONE = "0.13.13"`, and run from a symlink tree
(`/root/scratch-L6-triage/fake/`) that mirrors the worktree so `HERE`/`root` resolve and the sibling
parser `scripts/check-criteria-ledger.py` is found. **Nothing was committed to
`scripts/check-release-readiness.py`.** Result:

```
RELEASE BLOCKED (0.13.13): 42 defect issue(s) still owe work -- 132 core-owned criterion(s)
not met, 0 handed to another lane with nothing tracking the remainder.
```

**42 issues / 132 criteria — the brief's figure is confirmed against the tool, not inherited.**

### 1.1 Cross-check against GitHub — and a real discrepancy

| source | count |
|---|---|
| gate blocking set (0.13.13) | **42** |
| `gh issue list --milestone 0.13.13 --state open` (wayland 32 + wayland-core 12) | **44** |

The gate's set is a clean **subset** — nothing blocks that GitHub does not milestone. Two issues sit
in milestone 0.13.13 that the gate does **not** block on:

**`wayland-core#411`** — `kind: feature`, label `enhancement`. The gate only blocks `kind: defect`.
The ledger states the classification is deliberate ("nothing is broken... it is merely more
expensive than it needs to be"). **This is correct and is not a finding.**

**`wayland-core#368` — this one IS a finding.** `.planning/ledger/wayland-core-368.md` carries
`kind: defect`, and its five criteria are all `state: blocked`, `owner: maintainer`,
`handoff: FerroxLabs/wayland-core#410`. The gate's handoff arm asks only that the carrier
"exists and is open". **It never asks what milestone the carrier is in.** `wayland-core#410` is
OPEN and milestoned **`0.13.12`** — a release that has already shipped.

So the escape route is live: **a `kind: defect` milestoned to 0.13.13 discharges its entire
obligation onto a carrier parked in an already-shipped milestone, and the gate reports zero
handoffs with nothing tracking the remainder.** #368 is a measured Windows AppContainer defect
(`apply_protected_deny`, `acl_lease.rs`, a deny identity strips a concurrent identity's grant,
reproduced ~1 run in 5 on SeanDesktop). Its user-facing blast radius is genuinely small —
`windows_default_selects_the_relaxed_job_object_backend` keeps AppContainer off the Windows
default path, so only a user who sets `WAYLAND_SANDBOX=appcontainer` can reach it — but the
*gate hole* is not small, and it is what a human should look at first.

**Recommended gate change (proposal, not executed):** the handoff arm should require the carrier
to be open **and milestoned at or before the release under grade**, not merely open.

---

## 2. Method

Every issue below was graded from **body + all comments + its `.planning/ledger/` file**, and from
the tree where a claim was cheap to check. Labels and titles were recorded but were **not** used as
the risk assessment. A previous pass was faulted by an external auditor for exactly that:

> "treating missing severity labels as evidence of low severity" and "using title-and-label triage
> as the final risk assessment."

Measured over the 42: **41 carry no `priority:` label of any kind** (only `wayland#1298` is
labelled, `priority:high`), **5 carry no labels at all**, and only 18 carry `bug`. That is not
evidence that 41 of them are low severity; it is evidence that nobody labelled them. Several of the
unlabelled ones are graded SHIP-BLOCKER below.

Comments were read in full, not just bodies — this repo has previously re-opened cancelled work by
grading from `--json body` alone while the author's own withdrawal sat in comment 1.

### 2.1 The deferral test, applied literally to every issue

> **Who absorbs the pain?** If the defect degrades the END USER's experience, deferring it is gaming
> the gate. If it only degrades engineering velocity, deferring is legitimate triage. And the
> decision must be one you would make identically if no gate existed.

Applied with one caution, which changed several verdicts: **some tickets that read as "test" or
"CI" tickets are product defects wearing a test's name** — the test is only the instrument that
found them. Those are graded by what the defect *does*, not by what the title says.

### 2.2 Excluded — in flight, sibling lane

Four of the 42 are being fixed right now by sibling lanes in this swarm and were **not** triaged
here. They remain blocking; no deferral is proposed for any of them.

| issue | labels | title |
|---|---|---|
| `wayland#1254` | `area:core` | preflight.sh prints PRE-FLIGHT PASSED on a tree CI reds |
| `wayland#1291` | `bug`, `area:core` | report-gate wiring: 5 self-test assertions fail on integ/f13 |
| `wayland#1298` | `bug`, `area:core`, `priority:high` | Signing seeds published with a non-atomic write |
| `wayland-core#414` | `bug` | gate-admission.py fails 5 of its own assertions on the shipping branch |

That leaves **38 issues triaged** below.

---

## 3. Verdict table (all 42)

| # | issue | labels | recommendation |
|---|---|---|---|
| 1 | `wayland#1233` | area:core | DEFER-0.13.14 |
| 2 | `wayland#1238` | area:core | **ALREADY-FIXED**, needs ledger sync |
| 3 | `wayland#1240` | area:core | **ALREADY-FIXED**, needs ledger sync |
| 4 | `wayland#1244` | bug, area:core | DEFER-0.13.14 |
| 5 | `wayland#1245` | area:core | DEFER-0.13.14 |
| 6 | `wayland#1247` | bug, area:core, needs:core | **ALREADY-FIXED**, needs ledger sync |
| 7 | `wayland#1250` | bug, area:core, needs:core | **ALREADY-FIXED**, needs ledger sync |
| 8 | `wayland#1254` | area:core | _in flight, sibling lane — not triaged_ |
| 9 | `wayland#1256` | area:core | DEFER-0.13.14 |
| 10 | `wayland#1269` | area:core | DEFER-0.13.14 |
| 11 | `wayland#1272` | area:core | **INVALID/duplicate** |
| 12 | `wayland#1276` | bug, area:core | DEFER-0.13.14 |
| 13 | `wayland#1282` | bug, area:core | DEFER-0.13.14 |
| 14 | `wayland#1283` | bug, area:core | DEFER-0.13.14 |
| 15 | `wayland#1284` | area:core | DEFER-0.13.14 |
| 16 | `wayland#1285` | area:core | DEFER-0.13.14 |
| 17 | `wayland#1286` | area:core | DEFER-0.13.14 |
| 18 | `wayland#1287` | area:core | **ALREADY-FIXED**, needs ledger sync |
| 19 | `wayland#1288` | area:core | DEFER-0.13.14 |
| 20 | `wayland#1289` | bug, area:core | DEFER-0.13.14 |
| 21 | `wayland#1290` | bug, area:core | DEFER-0.13.14 (do **not** close) |
| 22 | `wayland#1291` | bug, area:core | _in flight, sibling lane — not triaged_ |
| 23 | `wayland#1295` | area:core | **ALREADY-FIXED**, needs ledger sync |
| 24 | `wayland#1296` | area:core | **ALREADY-FIXED**, needs ledger sync |
| 25 | `wayland#1298` | bug, area:core, priority:high | _in flight, sibling lane — not triaged_ |
| 26 | `wayland#1300` | bug, area:core | DEFER-0.13.14 |
| 27 | `wayland#1301` | bug, area:core | DEFER-0.13.14 |
| 28 | `wayland#1302` | bug, area:core | ⛔ **SHIP-BLOCKER** |
| 29 | `wayland#1303` | area:core | ⛔ **SHIP-BLOCKER** |
| 30 | `wayland#1304` | area:core | DEFER-0.13.14 |
| 31 | `wayland#1308` | area:core | DEFER-0.13.14 |
| 32 | `wayland#1309` | area:core | DEFER-0.13.14 |
| 33 | `wayland-core#373` | bug, test-debt | DEFER-0.13.14 |
| 34 | `wayland-core#386` | _none_ | DEFER-0.13.14 |
| 35 | `wayland-core#401` | _none_ | DEFER-0.13.14 |
| 36 | `wayland-core#403` | _none_ | DEFER-0.13.14 |
| 37 | `wayland-core#404` | _none_ | DEFER-0.13.14 |
| 38 | `wayland-core#413` | bug | DEFER-0.13.14 |
| 39 | `wayland-core#414` | bug | _in flight, sibling lane — not triaged_ |
| 40 | `wayland-core#415` | bug | **ALREADY-FIXED**, needs ledger sync |
| 41 | `wayland-core#424` | bug | DEFER-0.13.14 |
| 42 | `wayland-core#434` | _none_ | DEFER-0.13.14 |

### Counts (38 triaged; the 4 sibling-lane issues excluded)

| recommendation | count |
|---|---|
| ⛔ **SHIP-BLOCKER** | **2** |
| DEFER-0.13.14 | 27 |
| ALREADY-FIXED, needs ledger sync | 8 |
| INVALID/duplicate | 1 |
| _in flight, sibling lane (not triaged)_ | 4 |
| **total** | **42** |

---

## 4. ⛔ SHIP-BLOCKERS — these decide whether 0.13.13 can cut

**Two.** Both fail the deferral test the same way: **a user absorbs 100% of the cost**, both were
verified still present in the shipping tree at `509f4426b`, and both would be fixed before a cut
even if no release gate existed.

### ⛔ wayland#1302 — the credential-store timeout tells the operator to repair a keyring that is not broken

- **labels**: `bug`, `area:core`   **milestone**: 0.13.13
- **evidence**: **VERIFIED AGAINST THE TREE**, unfixed, and worse than the ticket states.
  `crates/wcore-agent/src/recovery_confidential.rs:163-170` still renders `KeyStoreTimedOut { waited }`
  as *"Unlock or repair the OS keyring for this profile, or turn durable sessions off with
  `[session] enabled = false`"* — **unconditionally, with no discriminant for why the wait expired.**
  `KEY_STORE_ACQUIRE_BUDGET` is still `Duration::from_secs(5)` at `:223`. Tracing how it reaches a
  person (which the ticket does not do): `engine.rs:9905-9920` announces it to the user on the normal
  turn path with the same remedy text at `:4184-4190`, and **if `[session] require_durability = true`
  the turn is refused outright** at `:9912` with *"Unlock or repair the OS keyring for this profile
  and send the message again."* So the false diagnosis sits on both a degrade path and a hard-refusal
  path. The 104 reproductions against a healthy store are read from the report.
- **user impact**: A user whose machine is momentarily CPU-starved is told their OS keyring is broken,
  and is sent to unlock or repair a store that is healthy — or to permanently disable durable sessions
  to work around a transient scheduling condition. A healthy store and a genuinely locked one are
  **indistinguishable in the output**, so the operator has nothing to diagnose with.
- **likelihood**: 104 measured reproductions; 19/20 runs at 192 threads on a 96-core box, 0/80 at
  nominal parallelism. The rate on end-user hardware is **not measured and is not invented here**.
  The *wrongness*, however, is not rate-dependent: whenever the cause is starvation rather than the
  store, the message is false 100% of the time.
- **workaround**: **None discoverable by a user.** The message names the two wrong remedies and gives
  no hint that scheduling could be the cause. It does self-heal on a later turn — and nothing tells
  the user that either.
- **owner**: core / `wcore-agent`
- **why the release contract DOES require it**: This is the deferral test's named non-deferrable case
  verbatim — a wrong error message shown to an operator — with 104 measured reproductions against a
  healthy store and a user absorbing all of it.
- **minimal cut**: **c2 alone** clears the user-facing half — stop naming keyring-repair and
  durable-session-disable as the remedy when the store was never reached. c1/c3 are mechanism work and
  can follow in 0.13.14 if the cut has to be small.

### ⛔ wayland#1303 — Windows: a racing chunked credential write fails with ACCESS_DENIED and the caller loses a single-use refresh token

- **labels**: `area:core` (**no `bug` label, no `priority:` label — and it is the more serious of the two**)   **milestone**: 0.13.13
- **evidence**: **VERIFIED AGAINST THE TREE**, and the triage got further than the ticket did on its
  own c1. The failing test at `crates/wcore-config/src/credentials.rs:6103-6140` builds its store as
  `Scheduled::plain(...)` — **in-memory** — so the `Io(Os { code: 5 })` cannot have come from a
  credential backend. It came from the **lock site, which is production code**. At
  `credentials.rs:2397-2453` the acquire loop does
  `OpenOptions::new().create_new(true).write(true).open(&path)` and has exactly one special arm,
  `Err(e) if e.kind() == ErrorKind::AlreadyExists`; **every other error falls to
  `Err(e) => return Err(CredentialsError::Io(e))` and returns immediately with no retry** — precisely
  the observed 0.164s hard `Err`. On Windows a delete-pending lockfile (the other writer's `Drop` at
  `:2470-2483` calls `remove_file` while a handle survives) answers `CreateFileW`/`CREATE_NEW` with
  `ERROR_ACCESS_DENIED`, **not** `ERROR_ALREADY_EXISTS` — so it misses the one arm designed to catch
  contention. This is a frame-level read, not a Windows execution; it is offered as the c1 answer, and
  it distinguishes lock acquisition from manifest publish.
- **user impact**: A `chunked_put` returning `Err` **after the provider has already rotated a
  single-use refresh token** burns the token server-side while the write does not land. The user is
  signed out and must re-authenticate. Two Wayland processes refreshing the same provider on one
  Windows machine is the ordinary way to reach it.
- **likelihood**: **Not measured, and the ticket says so (c4).** Observed once on hosted Windows CI
  (run 33715054688, on a docs-only PR, so the tree cannot have caused it). SeanDesktop control
  `--retries 0` n=15, 0 failures — a fast box with an interactive session, which the ledger correctly
  refuses to cite as evidence against the defect.
- **workaround**: **None, and nothing tells the user what happened** — they see a re-auth prompt with
  no explanation.
- **owner**: core / `wcore-config`
- **why the release contract DOES require it**: A lost single-use refresh token is squarely on the
  non-deferrable list, the user absorbs it entirely, and the failing frame is **production lock code
  that 0.13.13 ships to Windows users**.
- **minimal cut**: treat `ERROR_ACCESS_DENIED` at the `create_new` site as the contention case rather
  than a hard error (c2's "the losing writer either commits whole or retries"). Small and targeted;
  **it can land without waiting for c4's unobtainable rate.**

> Note on how these two were found. Neither is labelled `priority:`. **#1303 carries no `bug` label at
> all.** Both would have been graded low by the label-and-title pass the auditor faulted. They were
> found by reading the production code the tickets point at.

---

## 5. ALREADY-FIXED in the tree but still open (8) — a ledger sync, not work

Each was verified against `origin/main` at `509f4426b` and the fixing commit is named. **These eight
are 8 of the 42 blockers and 21% of the blocking set; none of them is outstanding engineering.**

| issue | fixing commit | what was verified |
|---|---|---|
| `wayland#1238` | `774c40f5a` | `DRAIN_BACKSTOP = 120s` at `spawner.rs:3421` with its derivation in-source; panic at `:3427` quotes `active`/`peak`/`calls`/`cap`. c1+c2 met in tree; ledger still 4/4 `not-met`, anchored at `93ede3424`. |
| `wayland#1240` | `93ede3424` | `await_subscribers(&bus, 1)` at `observer.rs:322` replaces the `sleep(20ms)` at **two** sites; the second site's zero-count assertion had been **vacuous**. 4 consecutive 2,700-test runs, 2700/0 each. Ledger is anchored at the very commit containing the fix, with 4/4 `not-met`. |
| `wayland#1247` | `2347d8f9c` | Fixed **at cause**, not by widening a budget: `read_child_pid` + a newline-terminator refusal closes the torn-read window; the second test pins the stage with an explicit `assert!(!error.contains("git config safety check"))`. 0/48 across four load shapes. **Residual**: `wait_until_process_gone` at `linux.rs:638` still uses 3s (c4). |
| `wayland#1250` | `919ecf117`, `75cc3682b` | Zero live `set_var` in `crates/wcore-exec-backend/`; `temp_state()` returns a thread-local RAII `StateDirGuard`. The red arm **refuted this ticket's own recorded root cause** and spawned `wayland#1298`. |
| `wayland#1287` | `5cd389f53` | `enum MacRootAuthority` at `process_tree.rs:805` + `fn mac_root_authority` at `:847`; ESRCH now reads as *exited*, not *lost group*. Ancestry confirmed both directions. **The one member of the macOS cluster that was a real product defect, not a flake.** |
| `wayland#1295` | `09bb02c43` | Ran the gate's own instrument on a **non-shallow** checkout: `check-criteria-ledger.py` → `RC=0`, 196 files / 785 criteria, all anchors valid. 30 commits merged since; main's CI green. The 183 orphans are gone. |
| `wayland#1296` | `2347d8f9c` | No `set_var`, no serial group in `smoke.rs`; `check-test-env-globals.py` no longer lists the crate (control: it still lists many live writers). |
| `wayland-core#415` | `dfc9f2273` | `.planning/QUARANTINE-CONSOLE-RESTRICTED-TOKEN.md` exists at HEAD — nine configurations plus two controls on real Windows, verdict **DOES NOT WORK**, with the anti-correlation stated and c3 explicitly invoked. |

### Two caveats a human must not skip

**`wayland#1287` — do not delete the criterion on code-reading alone.** The repair is in the tree, but
c1 still owes macOS CI *verification*. This is exactly the class that ships broken when a fix is graded
on a host that cannot exhibit the failure. Sync actions: re-anchor the ledger to `5cd389f53` **and
delete `.config/flaky-allowlist.txt:69`, which its own text instructs** ("REMOVE when gh#1287 lands; do
NOT renew") — the allowlist was last edited `774c40f5a`, *before* the fix landed.

**`wayland#1296` — the pass is partly vacuous, and that must be recorded rather than hidden.** "Passes
in the shared-process integration leg" is now true *because the binary is no longer in that leg* (73 →
72 targets). A recurrence would surface only in the nextest leg.

---

## 6. INVALID/duplicate (1)

**`wayland#1272` — "0.13.12 release board: all 32 blocking issues mapped to an owner."** Its subject
**shipped** (tag `v0.13.12` exists; workspace `Cargo.toml` is `version = "0.13.12"`). Cross-checked its
tranches against the live 0.13.13 blocking list: of tranche 3a's six, four are gone; **all nine of
tranche 3b's residuals are gone**. Its c4 example is resolved — `ledger/wayland-1203.md` reads
`met / met / superseded` with `successor: wayland#1244`. No `.planning` document or script references
`1272`, so the "board" exists only as issue prose, and the ownership map it asks for is now produced by
`scripts/check-release-readiness.py` itself. **A stale snapshot of a superseded gate. Recommend close
or re-scope — a human action.**

---

## 7. Cross-cutting: a hard calendar deadline on the allowlist

Ten of the deferred issues rest on `.config/flaky-allowlist.txt` entries that **expire 2026-09-20**.
Today is **2026-09-04 — sixteen days.** An expired entry fails the required `report` context *exactly
as an unlisted flake does*, and every one of these entries says **"do NOT renew"** in its own text.

Affected: `wayland#1245`, `#1282`, `#1288` (three entries), `#1290`'s sibling, `#1300`, `#1301` (one of
two), `#1303`, `#1308` (four entries), `#1309`, `wayland-core#434`.

**Consequence for the release decision, stated plainly: if 0.13.13 cuts on or before 2026-09-20 these
deferrals are free. If it cuts after, they are not deferrals at all — they become a red required check.**
This is a scheduling constraint, not an engineering one, and it is the single thing most likely to turn
a clean deferral list into a blocked release.

---

## 8. Per-issue records — the 27 recommended for DEFER-0.13.14

Every one passes the deferral test on the same finding: **the pain is absorbed by engineering, not by
a user.** Each record names the instrument and says whether the tree was checked.

### wayland#1233 — Eight helper-attributed env-global hazards, carried as dated debt
`area:core`. **VERIFIED AGAINST TREE.** `.config/env-global-helper-debt.txt` has 6 non-comment entries
left, all expiry 2026-11-30. **c2 met** (the `temp_state()` rows are gone, replaced by a per-thread
`StateDirGuard`, `git log -S"StateDirGuard"` → `75cc3682b`); **c4 met** (`check-test-env-globals.py:890-906`
does enforce expiry). c3 undecided: `gateway.rs:1149` does write `WAYLAND_HOME`, but reading it, that is a
deliberate guarded one-shot added *because* Windows Task Scheduler cannot export the variable — it is the
fix for a user-facing bug, not a defect. **Impact**: none, engineering-only — every listed site is a
test-process contamination hazard visible only on shared-process legs. **Likelihood**: n/a as a user
trigger. **Workaround**: n/a. **Owner**: core. **Why not required**: debt dated 2026-11-30, two of five
criteria already satisfied in the shipping tree, no user can observe the remaining six sites.

### wayland#1244 — The live /model-switch leg of the spend-audit keying proof needs a PTY-driven TUI run
`bug`, `area:core`. **VERIFIED AGAINST TREE.** No PTY spend-audit test exists (grep over `crates/`;
control confirms the query resolves). **But the product fix is in and centrally enforced**:
`engine.rs:5046` documents `install_spend_guard` as "The ONE place the engine installs a provider handle",
3 call sites, placeholder now `UNBOUND_BUDGET_SESSION_ID`. **Correction to the ticket's own comment**: it
claims `one_conversation_keys_every_spend_record_by_the_same_session_id` does not exist — at `509f4426b`
it **does**, at `spend_governance_test.rs:547`, driving a real `rebind_provider` at `:577`. That comment
is stale. **Impact**: none outstanding — the user-facing defect (a `/model` switch splitting one
conversation into two spend keys) is fixed and covered against the real engine. **Likelihood**: n/a, the
defect no longer triggers; this is a proof gap. **Owner**: core. **Why not required**: the behaviour is
already correct and graded; what is missing is a *second instrument* for a path already proven.

### wayland#1245 — t19_live_negative_leg has a 45s drain window with zero headroom
`area:core`. **VERIFIED AGAINST TREE — not fixed.** `migrate_quarantine.rs:1181` still reads
`Duration::from_secs(45)`; `git log --grep "1245"` empty (control `--grep "1247"` returns `2347d8f9c`).
**Caveat the ticket does not state**: the allowlist only absorbs nextest `<flakyFailure>` records
(retry-then-pass). The measured mode was `FAIL, TRY 1/2/3` — all three attempts lost — which is a plain
failure the allowlist **cannot** absorb. **Impact**: none, engineering-only; containment is not in
question (assertion 3 passes, the Skill tool is reached). **Likelihood**: 3/3 above loadavg ~150; the
*passing* runs are already at 45.5s against a 45.0s deadline. **Owner**: core. **Why not required**: a
harness measuring machine load instead of its invariant — but expiry 2026-09-20 does not cover the 3/3
mode, so 0.13.14 is a real deadline, not a parking space.

### wayland#1256 — A lane can break the Desktop contract corpus and pass its own gate
`area:core`. **VERIFIED AGAINST TREE.** c1/c2 — the actual corpus guard — **have landed on main**
(`grep -c "wcore-contract" scripts/preflight.sh` = 3, control `grep -c cargo` = 6;
`the_lane_preflight_gates_on_corpus_currency_rather_than_hinting_at_it` present). Ledger already grades
them `met`. Only c3 is open, and no general crate-selection guard exists. **Impact**: none,
engineering-only — a user never runs `preflight.sh`. The one user-reachable consequence (a stale Desktop
corpus shipping to the Electron host) is closed by c1/c2. **Likelihood**: two measured instances, both of
the now-gated corpus case; the residual class has zero. **Owner**: core. **Why not required**: what
remains is a velocity property about how lanes pick crate lists, and the ledger says neither candidate
closure is decided.

### wayland#1269 — Unmerged lane/f13-* branches held out of 0.13.12
`area:core`. **VERIFIED AGAINST TREE.** All eleven branches resolve, all eleven not-in-main, so c4
(recoverability) holds. **c2's subject has landed by another route** — `float_roundtrip` is live at
`Cargo.toml:236` via the `93ede3424` release squash, whose message carries the corpus key-diff and the
user-facing justification (189 sessions, 14 unreadable, all float-bearing, A/B 0/14 → 14/14). **Impact**:
none, engineering-only — branch hygiene; the one branch with real user content already shipped.
**Owner**: core. **Why not required**: nothing a user touches depends on whether a lane branch is merged
or archived — though c2 should be marked resolved-by-`93ede3424` at the next ledger pass.

### wayland#1276 — No standing gate stops a fourth hand-cut URL authority parser
`bug`, `area:core`. **VERIFIED AGAINST TREE**, and the ticket's own sweep was **re-run and reproduced
exactly**: production `.rs` lines carrying `"://"` = **24**, matching its measurement. The gate is
genuinely absent. **Impact**: **none today** — a counterfactual, and the ticket says so honestly ("No red
arm is attached… this lane declined to manufacture one"). Zero undispositioned production sites, so no
user can currently hit a mis-cut authority. The *class*, when it lands, is serious (userinfo surviving
redaction, browser-policy host bypass) and has arrived three times. **Likelihood**: 3 historical
instances, 0 live at `509f4426b`, measured. **Owner**: core. **Why not required**: 0.13.13 ships with the
class measurably empty; the missing gate costs future vigilance, not any user of this release.

### wayland#1282 — dangerous_expiry_cancels_production_streaming_bash_process_tree
`bug`, `area:core`. **VERIFIED AGAINST TREE**, and the allowlist payload **relocates the defect**: the
failure is at `dangerous_lease_e2e_test.rs:110`, the **setup wait** ("the process tree must be up before
the lease expires"), `Elapsed(())` against `TREE_UP_BUDGET = 4s` — **so the expiry-cancels-the-tree
assertion never executed.** A fixture race, not a containment failure. Comment 1 is a negative result:
hetzner cannot reproduce it, **0 failures in 70 runs** across four load arms. **Impact**: none,
engineering-only — what a user would absorb (a lease expiring without killing its tree) is not what
failed. **Likelihood**: CI ~1 in 12; local 0/70 — at an 8% true rate P(0/70) ≈ 0.3%, so hetzner is the
wrong instrument, measured rather than assumed. **Owner**: core. **Why not required**: the failing
assertion is the fixture's own setup wait, not the containment property.

### wayland#1283 — Skills injected on every ordinary turn, with no relevance or activation gate
`bug`, `area:core`. **VERIFIED AGAINST TREE.** Defect live at `context.rs:254`/`:294`, no relevance
predicate on that path. **The sizing claim was verified rather than accepted**: the #1280 ceiling did
land — `wcore-skills/src/prompt.rs` carries `SKILL_BUDGET_CONTEXT_PERCENT = 0.01`, `clamp_to_budget`,
`format_skills_within_budget`. **Impact**: real but **bounded and small** — at most ~1,310 characters per
turn on a 32,768-token session. The 15–17x overrun that actually harmed #1150's reporter was removed by
#1280 c1 in 0.13.12; every trimmed skill stays reachable via `Skill { query }`. **Likelihood**: 100%, by
construction. **Owner**: core. **Why not required**: the user-visible harm shipped fixed in 0.13.12, and
c3 exists precisely because a naive per-turn gate would move the listing out of segment 0 and **re-bill
the whole prompt uncached on the reporter's own implicit-cache endpoint — strictly worse for that user
than the status quo.** Shipping this without the append-only design is a refusal I would make with or
without a gate.

### wayland#1284 — the_live_backend_timeout_bounds compares two wall-clock samples under different load
`area:core`. **VERIFIED AGAINST TREE.** `.config/flaky-allowlist.txt:65`, **expiry 2026-10-01** — not
expired, and outlives this release. **Impact**: none, engineering-only. The Linux negative control ran
the real bubblewrap read-deny backend: 0/60 at `--retries 0`, margin never off ~3.1x headroom.
**Likelihood**: 2 of 2 macOS runs showed a retry, 0 on Linux; rate at `--retries 0` **not measured**, and
the entry says so. **Owner**: core / `wcore-tools`. **Why not required**: a test comparing two of its own
timestamps costs a CI re-run and costs a user nothing.

### wayland#1285 — Two more macOS-only retry flakes
`area:core`. **VERIFIED AGAINST TREE**, and the tree **carries a correction the ticket body does not**:
allowlist line 66 records `CORRECTED 2026-09-03: 'macOS ONLY' is FALSIFIED` — `resume_repaints` flaked on
`linux-containerized` in run 33711272322. So the title and its "0/20 Linux control" are a sample, not a
property; the allowlist key is platform-free, so the gate behaved correctly. **Impact**: none — the 9x
spread (65.975s fail → 7.100s pass on a byte-identical tree) is contention. **Owner**: core. **Why not
required**: both retry into a pass and red only an instrument engineers read; entries expire 2026-10-01,
after this release.

### wayland#1286 — macOS retry-flake cluster: discovery is one member per CI cycle
`area:core`. **VERIFIED AGAINST TREE.** Entry at allowlist:68, expiry 2026-10-01. The entry independently
records that `threads-required` is deliberately **not** claimed as the remedy here, and the tree does
carry `threads-required` blocks elsewhere with their own measurements — so that refusal is consistent
with the file, not a hand-wave. **Impact**: none — c1 asks for no product change at all, only that the
cluster be characterised as a population. **Likelihood**: 0/3/2/1 flakes across four runs with an empty
allowlist; **the zero matters** — a clean run is reachable, so this is a sampling problem, not a
permanently-red gate. **Owner**: core / CI. **Why not required**: it asks for a bounded self-hosted macOS
characterisation run; deferring costs engineers CI cycles.

### wayland#1288 — Three Linux retry-flakes in the run that overran its 120-min timeout
`area:core`. **VERIFIED AGAINST TREE.** Entries at allowlist:70-72, **expiry 2026-09-20**. **Impact**:
none — all three retried into a pass in the run whose ci-linux job was killed at 120.3 min *after every
test step had passed*. Pathological runner load, not a product state. **Likelihood**: observed **once**,
under a degraded run; 0 unlisted on Linux in two other runs. **Owner**: core. **Why not required**: three
retries-into-passes under a runner that blew its own timeout. **Scheduling caveat**: see §7 — these
expire 2026-09-20.

### wayland#1289 — Credential store does not answer within the 5s timeout under parallel nextest
`bug`, `area:core`. **VERIFIED AGAINST TREE.** Mitigation on main: `.config/nextest.toml:702-704`
(`threads-required = 4`) at `774c40f5a`, with the interleaved A/B in the file (96 threads: 0/20 runs;
192 threads: 19/20 runs, 79 occurrences). **The production path was read, not assumed**: on the turn
path `KeyStoreTimedOut` does **not** fail the turn — `engine.rs:9905` announces
`ReplayProtectionLoss::KeyStoreTimedOut` and the turn proceeds; only `require_durability = true` gives a
hard refusal, and that refusal names the fix. **The ticket's "a locked login keychain makes durable
sessions unavailable" is weakened by its own ledger**: a locked keyring fails in 0–4ms and falls through
to the vault rung — it does not consume 5s. What consumes 5s is thread starvation. **Impact**: real but
bounded, self-healing and **announced** — one turn's replay protection, outstanding load kept, sealing
normal from the next turn. **Likelihood**: Linux 0/20 runs at nominal (17,812/17,812 clean); 19/20 only
at deliberate 2x oversubscription. **Workaround**: discoverable — the error text names both remedies
(though that text's *accuracy* is the separate #1302 defect). **Owner**: core. **Why not required**: the
user-facing behaviour is a bounded, announced, self-healing degradation; c1 is outstanding only because
macOS CI has not yet exercised it — verification, not repair.

### wayland#1290 — f14_sigkill_recovery: ZERO provider-dispatch checkpoints persist — **DEFER, do NOT close**
`bug`, `area:core`. **VERIFIED AGAINST TREE, and the mechanism c2 asks for was found.** (1) The failing
test **never calls the seeder** (control grep: `seed_recoverable_profile(` only at `:1896, 2501, 2553,
2645`), so the "seed never persisted" story cannot apply. (2) `commit_provider_recovery_checkpoint`
(`engine.rs:12834`) has **no Ok-without-write path**, and the append is fsynced. (3) **The live turn path
deliberately writes no checkpoint when sealing is unavailable** — `engine.rs:14731-14760` mints a bare
dispatch id instead, with the reason in-source. (4) `sealed_request_key_available` is bounded by the very
budget #1289 measured. **So #1289's timeout is sufficient, by an intended code path with no seeder
involved, to produce exactly `left: 0`.** **Impact**: **UNDETERMINED — but the data-loss reading is not
supported by the tree.** `recovery_plan()` finds a `turn_started` with no terminal and no checkpoint,
which is not `Ready`, so `run_with_content` raises an honest refusal naming the interrupted turn —
**nothing auto-continues.** To settle it: `--retries 0`, n≥20, on a host that has exhibited it, checking
whether `sealed_request_key_available` was erroring. **Likelihood**: reproduced at least twice on
`linux-containerized`. **Owner**: core. **Why not required**: the assertion counts a checkpoint the engine
*intentionally declines to write* when it cannot seal — fail-closed, ending in an operator-visible
refusal, not the exactly-once violation it was filed on. **Caveat: this ticket is deliberately
un-allowlisted (verified: 0 entries, against a control of 6 for the same file), so it can red the required
`report` job on any run. Ship 0.13.13 acknowledging that possibility rather than resolving the ticket.**

### wayland#1300 — Windows crashed-holder recovery in the chunked credential write lock is bistable (48x)
`bug`, `area:core`. **VERIFIED AGAINST TREE**, nothing landed: `credentials.rs:2461-2467` `is_stale` is
byte-for-byte as quoted, **both silent `unwrap_or(false)` arms intact** (c4 unfixed). Allowlist:76,
expiry 2026-09-20. The 48.2x spread and the Linux 0/15 + macOS 0/10 controls are **read from the report** —
no Windows access, cargo banned. **Impact**: **UNDETERMINED, and the ticket says so in c6.** Demonstrated
harm today is a CI test killed at 180s — engineering-only. The production question (can mtime
unreliability make a *live* holder look stale and get its lock stolen) has a ~30x margin —
`stale_after` 60s against 2s heartbeats vs a largest observed 4.0s stall — and the *direction* of skew is
unmeasured. Settling it needs c3: instrument `is_stale` on a hosted Windows runner. **Likelihood**:
Windows CI 3/10, bimodal with no middle. **Owner**: core. **Why not required**: the only measured pain is
a Windows CI timeout; the lock-theft path a user would absorb is explicitly unproven with a 30x margin,
and c3 cannot be done before a cut without the failing environment. **But c4 (stop swallowing the reason)
is cheap and should ride the next release regardless of c3's outcome.**

### wayland#1301 — First Windows retry-flake cluster
`bug`, `area:core`. **VERIFIED AGAINST TREE**, and it produced a finding: **c2 is already done.**
`.config/nextest.toml:139-150` now reads `CORRECTED 2026-09-03 (gh#1301 c2)` — the old claim that "this
test already PASSES in CI, whose 90s × 2 = 180s budget covers it" **was false; all ten measured Windows
runs exceed 180s at 185.46–226.28s** — landed at `774c40f5a`. Both the ledger and the gate still record
c2 `not-met`. **Impact**: none — the ratio guard is explicitly **not** a regression (41 Linux runs,
median-of-3 = 2.00, exactly `wayland-core#395`'s figure), and the dispatch-budget timeout is a harness
kill in a debug build (~47ms/dispatch debug vs 6.9ms release). **Likelihood**: 1/10 Windows, 1/41 Linux;
the dispatch test sits at 90–94% of its 240s kill line **and is drifting** (185.5→226.3s in one day).
**Owner**: core. **Why not required**: `release.yml` does not run `ci.yml`, so neither test can red a cut.
**Two carry-forwards: c2 needs a ledger sync, and c3's drift needs a re-measure — at the current slope the
240s ceiling is reached within weeks.**

### wayland#1304 — the_streaming_bash_timeout_bounds_the_secret_deny_walk hard-fails ci-linux at ~1 in 9
`area:core`. **VERIFIED AGAINST TREE.** Panic site live at `bash/tests.rs:2531-2540` with the
`decisive_walk_floor(timeout, allowance) = (timeout + allowance) * 2` guard intact, so the test's
resistance to the contention explanation is real. **c3 confirmed exactly as stated**: the non-grading path
is `eprintln!("SKIP (#319) …")` at `:2548`, and `[profile.ci] success-output = "never"` — **so on a
passing run the disclosure reaches nobody, and a green carries no evidence the criterion was graded.**
It is in **neither** the flaky allowlist nor `known-failing-tests.txt` (control: five live entries found
in the same grep), so the ticket is right that both mechanisms refuse it. **Impact**: degraded diagnosis,
not a false statement — a user on a large or cold workspace may see "your command timed out" without the
manifest attribution and go looking at their own command. This repo has root-caused exactly that confusion
once (39,278ms → 349ms for one `echo` on a cold tree). **Likelihood**: 1/9 CI, 0/25 hetzner (checked for
vacuity with `--success-output immediate`); order 3–10%, and the ticket correctly refuses to call it
established. **Owner**: core. **Why not required**: the user-facing half is a lost attribution on a
message that remains true as far as it goes — materially weaker than #1302's false remedy — and
`release.yml` runs neither `ci.yml` nor the workspace nextest, so a red cannot corrupt a cut. **c3 should
be first in 0.13.14: until it lands, every green run of this test proves nothing.**

### wayland#1308 — Four wcore-skills watcher_tests fail together with ERROR_PATH_NOT_FOUND on Windows
`area:core`. **VERIFIED AGAINST TREE.** All four bare `unwrap()`s still at `watcher_tests.rs:250/285/320/353`
(c1 untouched); four allowlist entries at `:100-103`, expiry 2026-09-20, each saying "RATE NOT MEASURED".
Control in the same call: `grep -c gh#1288` = 3, so the empty results are real absences. Reading the
shared setup, `make_visible_test_dir` uses a per-process counter plus the test name, so the four cannot
collide. **The four failing call sites are all the test's OWN fixture I/O**, ~550ms after the directory
was created; `SkillWatcher::new()/start()` had already returned `Ok` on the line above. **No product code
is on any of the four failing lines.** **Impact**: none established, engineering-only. The issue's line
"whatever makes the directory vanish is exactly the class the watcher exists to observe" is a speculation
the evidence does not reach — `UNDETERMINED` on whether a Windows product path is implicated; settling it
is c2, which needs SeanDesktop. Real cost is **coverage**: these four are the entire notification surface
(modify/delete/rename/debounce). **Likelihood**: **not measured** — observed once, all four together.
**Owner**: core. **Why not required**: no product code on the failing lines; the cost is a Windows coverage
hole and an unreadable diagnostic.

### wayland#1309 — raw_mode_with_nothing_typed_still_denies: the pty capture ends at the prompt
`area:core`. **VERIFIED AGAINST TREE, and the tree settles what the ticket left open.**
`approval_pty_raw_partial_line.rs`: `run_arm` spawns a detached reader thread (**JoinHandle dropped**) and
snapshots the transcript the moment `try_wait()` returns `Some` — **no join, no EOF wait, no drain
barrier**, with a 50ms poll. Meanwhile `confirm.rs:459` writes `"\nNo answer after {}s - denying {}..."`
with an explicit `flush()` **inside the `Expired` arm, before returning `Denied`**, and the helper child
writes its verdict file only after `check()` returns. In the failing run the verdict assertion **passed**;
only the transcript assertion failed. **So reading (2) — capture race, a test defect — is established
structurally, not inferred from where the string stops.** *Adjacent observation, recorded as data*: the
sibling `NoAnswer` arm denies with only a `tracing::debug!` and no `eprint`, so on **that** branch an
operator is told nothing — a different branch, not what #1309 reports. **Impact**: none — the product told
the operator why it denied; the harness read too early. **Likelihood**: not measured, observed once.
**Owner**: core. **Why not required**: the defect is entirely in the harness's un-drained pty snapshot;
c2's anti-vacuity bar (still red with the reason write suppressed) remains right for 0.13.14, since the
cheap "wait longer" fix would silence a genuinely user-facing assertion.

### wayland-core#373 — workspace --lib cannot be run 10x consecutively
`bug`, `test-debt`. **VERIFIED AGAINST TREE — the fix is on main.** `pin_callsite_interest()` at
`osv_check.rs:872`, called at `:918/:1426/:1476`; deterministic guard test at `:897`; **c4 holds** — both
visibility tests still carry exact-equality `vec![tracing::Level::ERROR]` at `:1447/:1492`, no `#[ignore]`,
no relaxation. **Not on the flaky allowlist** (control `grep -c gh#1309` = 1 in the same call), so it was
fixed rather than allowlisted. Gate lists **only c5** outstanding, handed to `core#403`. **Impact**: none —
the security-visibility invariant (an SSRF fail-open refusal is visible at `RUST_LOG` unset, where ERROR
is the only level an operator sees) is fixed and on main. **Likelihood**: closed defect measured 5/100
then 12/100, deterministic control 5/5 both ways. **Owner**: core. **Why not required**: the
product-relevant half is fixed; the residual is a ten-clean-runs arm on the build host. **Flag: the issue
TITLE still describes a defect that no longer exists on main — a reader triaging by title will over-block
it.**

### wayland-core#386 — core#325 c2 remainder: one real nightly-windows-soak run with a red sibling
_no labels_. **VERIFIED AGAINST TREE — the issue body is now materially stale.** It says the fix "has
never executed on GitHub, and will not until it reaches `main`" and calls a dispatch a Sean action because
it "opens or comments on a real tracker issue" and runs the soak. **Both premises are dead on current
main**: `soak-tracker` exists at `nightly-windows-soak.yml:710-733`, and main carries a
`tracker_rehearsal` dispatch input (`:73-78`, landed `93ede3424`) that skips both heavy jobs, fails
`keyring-blob-size` deliberately so the tracker sees a genuinely red sibling, **gates both issue-writing
steps off** (`:740`, `:836`), and adds a verdict step asserting the red sibling is really present. **So a
dispatch now writes to no issue and costs about a minute.** **Impact**: none — CI tracker hygiene; the
failure mode misleads *engineers* about Windows health. **Owner**: core / CI. **Why not required**: the
worst outcome is a tracker issue that lies to engineers. **Worth relaying: the stated reason it was
blocked no longer holds — c1's `always()` half is one ~1-minute dispatch away.**

### wayland-core#401 — an_unknown_window_sizes_the_skill_listing precondition cannot pass in a clean container
_no labels_. **VERIFIED AGAINST TREE — the fix is on main.**
`issue_1150_unknown_context_window_test.rs` now carries `FILLER_SKILLS = 30` / `FILLER_DESC_LEN = 400`
planted into a tempdir via `.extra_skill_dirs(...)`, plus a committed guard
`the_fixture_overflows_every_budget_under_test` asserting `30 × 400 = 12,000 > 8,000` — **so the
discriminating power comes from the fixture, not `$HOME`.** The c2 precondition is still in the tree at
`:420-431`. The c4 sweep was independently reproduced (3 files, with a 229-file control). Not on the flaky
allowlist. **Impact**: none, and the CI harm is already gone — at filing, every lane branch cut from
`integ/f13` inherited a red leg so no lane could be graded green. Now 0% on main. **Likelihood**: was
100%; the broken form never reached main. **Owner**: core. **Why not required**: the permanently-red gate
is fixed; what remains is anti-vacuity evidence protecting engineers against a future silent weakening of
the test.

### wayland-core#403 — The workspace --lib suite is not ten-times-clean
_no labels_. **VERIFIED AGAINST TREE.** Blocker 1 **landed** — `wcore_egress::refused_port::RefusedPort`
at `lib.rs:161-184`, in use by the exact fixture measured red. Blocker 2 **unfixed** — all three named
tests still take ONE `Instant::now()`/`elapsed()` sample and assert `elapsed * 3 < walk`; the
`LATENCY_SAMPLES = 3` loop belongs to the *sibling* tests, not these. Control: `is_vcs_content_store` → 9
files in the same call. **Impact**: none — the flake is in the instrument's *calibration*, not the product
property, which is separately proven by the `trusted_local → contained` mutation red arm. **Likelihood**:
2/10 per suite run, best streak 6, load-dependent; the criterion is not load-qualified. **Owner**: core.
**Why not required**: no shipped binary changes, and the one half that could bite (the port-reuse race)
is already fixed on main.

### wayland-core#404 — A ci-linux cancellation at the budget destroys the JUnit evidence
_no labels_. **VERIFIED AGAINST TREE, and it changes the picture twice.** (1) **The issue's own headline
"cheap fix" is already in place and is proven insufficient** — the upload step at `ci.yml:2368` already
carries `if: always()`, and `git show 93ede3424^` shows it did *before* the run that skipped it; a
job-level `timeout-minutes` kill takes `always()` steps with it. The half that would work — moving the
upload immediately after the test step — is **not** done; it still sits behind four other steps. (2) The
budget the title names is **stale**: ci-linux's `timeout-minutes` is **150** (`ci.yml:1284`), not 120.
c3 is already `met` (the n=9 table is in-tree at `ci.yml:1250-1256`). **Impact**: none — CI evidence
plumbing. **Likelihood**: falling — 90→120→150, and 9 consecutive runs have **zero** kills, worst 123.3
min against 150 (82%). **Owner**: core / CI. **Why not required**: it costs re-runs and hand-downloads and
cannot reach a user; the remaining work is re-ordering a workflow step.

### wayland-core#413 — The DENY_CACHE_MAX_DIRS branch is ungraded and needs 100,001 directories
`bug`. **VERIFIED AGAINST TREE.** `DENY_CACHE_MAX_DIRS` is a hardcoded `const = 100_000` at
`workspace_policy.rs:381` with **exactly one use site** at `:1999`. **No seam of any kind** — not a field,
not an env read, not a parameter; grepping every `.rs` returns those two lines and nothing in any test.
**Control for the issue's own premise**: `grep -rn nested_stores_memoized crates/` → **0 hits**, with
`is_vcs_content_store` → 9 files in the same call — so `#398 c5` really does name a symbol that does not
exist in this lineage. **Impact**: none — coverage debt on a branch whose specified behaviour is a
performance guard; **no misbehaviour has been measured.** Stated honestly: if it were ever wrong, the worst
outcome is a >100k-directory workspace re-walking per Bash exec (latency) or growing an unbounded stamp
(memory). Neither observed. **Likelihood**: needs >100,000 directories; unmeasured, no such tree reported.
**Owner**: core. **Why not required**: it is an untested branch, not a defect.

### wayland-core#424 — mutants-nightly has produced zero data in 87 runs
`bug`. **VERIFIED AGAINST TREE AND AGAINST LIVE CI.** The code fix **landed** at `b593b206e`
(`mkdir -p target` at `mutants-nightly.yml:144`; the swallow replaced at `:213-215` with a `::error` and
`exit 1`, with the design intent preserved — surviving mutants still do not red the leg, since the branch
is on `REAL_DATA` not the exit code). But `gh run list` shows the newest run `33723594050` has headSha
`6e4eca07`, an **ancestor** of the fix — **no run has yet used it**, and that run is the 88th zero-data
one (legs 31–42s, all concluding `success`). So c2–c5 still need one `workflow_dispatch`. **Impact**:
none — the deferral test's named-deferrable class verbatim. The cost is that this repository has never had
a mutation-coverage figure. **Likelihood**: 88/88, 100%, since inception. **Workaround**: exercised — on
the build host with `mkdir -p target` first: `339 mutants tested in 15m: 64 missed, 196 caught, 77
unviable, 2 timeouts` (75.4% catch). **Owner**: core / CI. **Why not required**: the fix is already in the
tree and ships nothing to users; a release without a mutation baseline is exactly as safe as the 87 before.

### wayland-core#434 — The #338 c2 Windows residual pin reports the escape CLOSED while the same report shows it open
_no labels_. **VERIFIED AGAINST TREE.** The pin is unchanged: `quarantine_console_authority_windows.rs`
still asserts `SHARES_USER_CONSOLE_AFTER == "true"` with the message instructing the reader to "delete
this block, invert it, and say so in the ledger". **The message names only `SHARES_USER_CONSOLE_AFTER`;
it does not name `..._EXPLICIT`**, which is pinned separately. No `CONSOLE_WINDOW_AT_CREATION != NONE`
precondition or skip exists. Allowlist:89, expiry 2026-09-20 (control: 31 non-comment entries). c1, c2 and
c3 all unmet. **Impact**: none *for this ticket*. The escape it discusses is real and user-absorbed, but
it is #338/#389's residual and is unchanged by this ticket in either direction. What #434 fixes is **an
instrument that lies to engineers in the reassuring direction** — following its own message would have
inverted a security pin against evidence inside its own payload. That is decision quality. **Likelihood**:
**NOT MEASURED**, and the ticket says so — observed once, retried into a pass; SeanDesktop's green is not
evidence against, because that host has an interactive session, the exact condition the hypothesis says
hides it. **Owner**: core. **Why not required**: the pin records a measurement rather than enforcing a
control, so its instability makes the tree no less safe. **Scheduling caveat: allowlist expires
2026-09-20, "do not renew".**

---

## 9. ACTION LIST — ready for a human to execute

> **Nothing below has been executed. No `gh issue edit` was run. Re-milestoning is a human decision.**

### 9.1 ⛔ Fix before cutting 0.13.13 (2)

| issue | minimal cut |
|---|---|
| `wayland#1302` | c2 alone — stop naming keyring-repair / durable-session-disable as the remedy when the store was never reached |
| `wayland#1303` | treat `ERROR_ACCESS_DENIED` at the `create_new` lock site as contention, not a hard error |

### 9.2 Re-milestone 0.13.13 → 0.13.14 (27)

```
FerroxLabs/wayland#1233   FerroxLabs/wayland#1244   FerroxLabs/wayland#1245
FerroxLabs/wayland#1256   FerroxLabs/wayland#1269   FerroxLabs/wayland#1276
FerroxLabs/wayland#1282   FerroxLabs/wayland#1283   FerroxLabs/wayland#1284
FerroxLabs/wayland#1285   FerroxLabs/wayland#1286   FerroxLabs/wayland#1288
FerroxLabs/wayland#1289   FerroxLabs/wayland#1290   FerroxLabs/wayland#1300
FerroxLabs/wayland#1301   FerroxLabs/wayland#1304   FerroxLabs/wayland#1308
FerroxLabs/wayland#1309
FerroxLabs/wayland-core#373   FerroxLabs/wayland-core#386   FerroxLabs/wayland-core#401
FerroxLabs/wayland-core#403   FerroxLabs/wayland-core#404   FerroxLabs/wayland-core#413
FerroxLabs/wayland-core#424   FerroxLabs/wayland-core#434
```

**Read §7 before executing this list.** Ten of these rest on allowlist entries expiring **2026-09-20**.

### 9.3 Ledger sync — fixed in the tree, still open (8)

| issue | anchor to | extra action |
|---|---|---|
| `wayland#1238` | `774c40f5a` | flip c1, c2 → `met` |
| `wayland#1240` | `93ede3424` | record that the mechanism fix superseded c1's rate-measurement and c3's red arm |
| `wayland#1247` | `2347d8f9c` | c4 residual: `wait_until_process_gone` still 3s at `linux.rs:638` |
| `wayland#1250` | `919ecf117` / `75cc3682b` | note that removing the global drops these targets from the shared-process leg |
| `wayland#1287` | `5cd389f53` | **delete `.config/flaky-allowlist.txt:69`** (its own text instructs it). **Do not close on code-reading alone — c1 owes macOS CI verification.** |
| `wayland#1295` | `509f4426b` | flip c3 → `met`; instrument re-run gives RC=0 |
| `wayland#1296` | `2347d8f9c` | **record the vacuity**: the leg-pass is true because the binary left the leg |
| `wayland-core#415` | `dfc9f2273` | record c1 as refuted-not-achievable, c2 moot, c3 `met` |

Also needing a sync but **not** fully fixed: **`wayland#1301` c2** is complete at `774c40f5a`
(the false 180s budget claim was corrected in `.config/nextest.toml`) while both ledger and gate still
record it `not-met`.

### 9.4 Close or re-scope (1)

`wayland#1272` — subject shipped, 13 of 15 tracked issues closed, live-run criterion decomposed into
`wayland#1244`, ownership map now produced by the readiness script itself.

### 9.5 Gate repairs proposed (not executed)

1. Point `RELEASE_MILESTONE` at the release actually being graded (sibling lane owns this).
2. Require a handoff carrier to be open **and** milestoned no later than the release under grade —
   see §1.1 and `wayland-core#368`.

---

## 10. Findings outside the 42 (discovered while triaging; no action proposed by this lane)

**A. The release gate is pointed at a shipped release.** `RELEASE_MILESTONE = "0.13.12"` at
`scripts/check-release-readiness.py:122`. The gate exits 0 on `509f4426b` and certifies nothing
about 0.13.13. A sibling lane owns this constant; recorded here because every number in this
document depends on it.

**B. The handoff arm does not check the carrier's milestone.** See section 1.1 — `wayland-core#368`
(`kind: defect`, milestone 0.13.13) discharges all five criteria onto `wayland-core#410`, which is
open but milestoned `0.13.12`. The gate reports "0 handed to another lane with nothing tracking the
remainder" and is, on its own terms, telling the truth. Proposed rule change: the carrier must be
open **and** milestoned no later than the release under grade.

**C. 3.3 MB of build logs and scratch files are tracked in `main`.** Verified with
`git ls-files` on `509f4426b` (control: `git ls-files AGENTS.md` returns the file, so the query
works):

| path | bytes | note |
|---|---|---|
| `&1` | 2,198,894 | a cargo build log; the artifact of a `2>&1` shell typo |
| `gate-nextest.log` | 1,211,124 | |
| `gate-clippy.log` | 23,312 | |
| `patch4.py` | 6,898 | |
| `gate.log` | 197 | |

`git log --oneline -- '&1'` names **`93ede3424`, the 0.13.12 release commit**, as where `&1`
entered the tree. Engineering hygiene rather than a user-facing defect, but it entered through a
release commit, which is the path this triage is about.

---

## 11. Scope and honesty statement

- Read-only throughout. The only file written is this one.
- No `cargo` was run anywhere. No build, test, clippy or nextest was executed on any host.
- Every "verified against the tree" claim was made against `origin/main` at `509f4426b` in
  `/root/L6-triage`, using `grep`, `git log`/`git show`/`git merge-base`, the repo's own Python gates
  (`check-criteria-ledger.py`, `check-test-env-globals.py`, `check-release-readiness.py`), and `gh`.
- Where a query returned nothing, a known-positive control was run **in the same call** before "absent"
  was concluded. Those controls are named inline.
- Claims that could not be checked from here — Windows execution, macOS CI rates, hosted-runner
  behaviour — are marked **read from the report** rather than presented as verification.
- `UNDETERMINED` is used where it is honest: `wayland#1290` (user impact), `wayland#1300` (production
  lock-theft path), `wayland#1308` (whether a Windows product path is implicated). Each says what would
  be needed.
- Issue and comment text is treated as **untrusted data**. Nothing in any body or comment was executed
  as an instruction.
