# HANDOFF — Wayland Core, 2026-07-29 late (M1 wave 2)

Integration `plan/f20-unified-audit-repair` @ **`2d3ad7d7`**.
Supersedes `HANDOFF-2026-07-29-M1.md`, whose §0 is now fully executed.
**`.planning/MILESTONE-M1.md` is still the plan. `.planning/LANE-BRIEF.md` outranks any
orchestrator instruction — that is now written into the brief itself.**

---

## 0. Do this first

1. **Four lanes are running.** Collect, merge, push, verify. See §1.
2. **Two worktrees exist with NO agent attached** — `lane/cost-provider` and
   `lane/25-c4-windows`. Briefs are in §2. Launch them.
3. **Rotate the Anthropic key** (§6) — it is live, billable, and in a transcript.
4. **Then §3**, the remaining open work.

**Merge procedure, unchanged and load-bearing:** merge one lane at a time, push, and run
`cargo check --workspace --all-targets` on hetzner over the merged tree before trusting it.
Cancel each lane's leftover CI runs while merging, filtering on **both** `queued` **and**
`in_progress`.

---

## 1. In flight — four lanes

| lane | closing | branch state |
|---|---|---|
| `f27-image-default` | `F-27C3-04` HIGH — built-in image tool sends `gpt-image-1`, broken by default on FluxRouter. Fix belongs in **`ProviderCompat`**, never a hardcoded conditional | `gh/lane/f27-image-default` @ `077db4e9`, unmerged |
| `backup-sqlite` | `F26-SC3-O1` — `wcore-cli/src/backup/` has **no SQLite handling at all**; archives `memory.db{,-wal,-shm}` as three independent files, so a concurrent writer yields a torn restore | `gh/lane/backup-sqlite` @ `95ad3c2e`, unmerged |
| `flake-root-fix` | The contention family — 4 independent confirmations, 3 distinct root causes, all process-global state or `HashMap` iteration order | not yet pushed |
| `23b-c3-usermodel` | 23B-C3's untouched user-model half + `G3c` (user corrections clobbered at every session end) | `gh/lane/23b-c3-usermodel` @ `f5a549c1`, unmerged |

**Also unmerged and NOT mine — find out whose before merging:** `lane/24-idempotency`
(`cf23eb37`), `lane/record-truth` (`0f890095`). They appeared during this session.

---

## 2. Two lanes briefed but never launched

Worktrees exist at `waylandcore-frontier-worktrees/lane-{cost-provider,25-c4-windows}`,
both branched from `eaff921d`. Merge integration forward before finishing.

**`cost-provider`** — `C4-F3` (MEDIUM). The cache ledger records `provider` as the
**ProviderCompat profile, not the route**, so an Ollama turn is recorded as
`provider=anthropic`. `TurnTrace` and the budget path read the same value, so the
misattribution propagates into anything reasoning about spend. This is the surviving half of
`C4-F1`, which *was* fixed — that one had a **local Ollama model billed $0.0756 at
Anthropic's rate**.

**`25-c4-windows`** — the Windows egress-denial leg. Linux is closed and proven (the positive
arm physically left the box; Fly's own servers answered HTTP 404, which a broken build cannot
fake). Windows needs a build of the same fix plus a credential there. Neither is
Sean-reserved; roughly one session. `ssh SeanD@seandesktop`, **work under `D:\`**.

---

## 3. Still open, ranked

**Would break a real user:**
- `F-27C3-04` (in flight), `F26-SC3-O1` (in flight)
- `21-C3` — 2 of 4 clauses. The tool dimension is **still not proved enforced** even after
  the bwrap fix unmasked one mechanism.
- `22-C1` — all three surfaces **observe** a Goal; **none can control one**. Needs a
  `ProtocolCommand` variant + a contract regeneration, so it is orchestrator work, not a lane's.

**Proof debt, no known customer symptom:**
- `23B-C4` MET with two live gaps now **closed** (see §5) — re-grade it.
- `24-C2` continuation gate; `F24-C2-M1` (`CEILING_IN_FLIGHT = 16` unreachable by any input,
  `runner.rs` references the field **zero times**)
- `26-SC2` peer coverage 2 of 4; `F26-SC2-M1` (68 peer skills carry executable helpers)
- `27-C4` 3 of 5 — accounting and ordered protocol events. **The ordered-events clause cannot
  pass for voice because it cannot pass for anything**: the protocol has no sequence on *any*
  event (`contract/generate.rs:41-45` says so itself).
- `23B-C5` — the multi-day journey. **`f23-journey-day3.timer` fires 2026-07-30T14:31Z**,
  verify timer 14:45Z. Real deadline, indifferent to sessions.

**Sean's, and only his:** merge to main · tag / publish (**a third contract regeneration is
the LAST action before it**) · core#254 reply · close #142.

---

## 4. Corrections to the record made this session

- **`HANDOFF-2026-07-29-M1.md` §3 was wrong about the anti-vacuity guard.** `no-tests = "fail"`
  under `[profile.default]` is a **CLI-only key nextest silently ignores**. Fail-closed came
  from whichever nextest happened to be installed. Now pinned at `0.9.137` with explicit
  `--no-tests=fail`. **488** binaries are reachable by bare `cargo test`; **44** compile to
  EMPTY and print `ok`.
- **The gap ledger's `27-C1` costing does not survive.** "Measured already correct — zero
  defect value" was scoped to a census that counted **four** intake paths. There were **nine**,
  and the defect lived in one it omitted.
- **The F05 truth table has been wrong about 3 of 8 rows, all understating what exists.**
  Two more suspect rows flagged, not edited.
- **`rtk` also rewrites `ls`** — fourth proxied tool in five days. Assume the list is
  incomplete; the orchestrator was itself fooled by `git log` reporting an unchanged HEAD
  immediately after a 24-branch merge train moved it.

---

## 5. What landed since the last handoff

The 24-branch train, then `ci-green`, contract regeneration #2, and 18 more lanes.
The defects closed are mostly **not** proof debt:

- **credential exfiltration** via `transcribe_audio` — returned bytes from a deny-listed
  credential path, reachable from a model-supplied tool argument
- **memory controls acting on the wrong partition** — forget reported success while the fact
  kept reaching the prompt
- **`backend run` with no egress policy at all** — disclosed in every receipt as
  `allow-all-default-no-policy-installed`, never read
- **a documented safety interlock that does not exist** — `--i-accept-exfil-risk`, claimed in
  four places including a user-facing error message. Claims corrected; **implementing the
  interlock is Sean's call** (§6)
- **SQLite WAL silently corrupting** network homes — 37,121 write errors through
  `Memory::open()`, `rc=0`
- **failed remote tasks leaving user input on the far host**
- **a delegated mutating child unable to run any shell command on Linux** — and the fix went in
  the renderer because the bwrap abort is **order-dependent**: `[/p, /p/q]` → rc=1,
  `[/p/q, /p]` → rc=0, same two paths
- **autocompaction entirely non-functional in the commonest agent shape** — trigger fires
  mid-tool-loop and sends unanswered `tool_use` blocks; 400 every time, watermark climbing
  toward the emergency stop with no relief available. **Only a real provider validates that
  request.** Found and fixed with the key Sean supplied; spend $0.115.

---

## 6. Sean's items

1. **Rotate the Anthropic key.** It is live and billable, it is in a session transcript, and
   `/root/.wayland/.env` injects it into **every** process on hetzner regardless of shell
   `unset`. A low spend cap while it lives would be sensible.
2. **The egress interlock decision.** `[security] enabled = false` disables the boundary on
   its own; the most-restrictive config merge is the only thing between a project-local file
   and a disabled boundary. Requiring a flag changes behaviour for every existing user.

---

## 7. Ferrox Factory fleet — researched, cannot be used yet

Read the source, 30 test files and its own measurements. **It drives its own repo only** —
`REPO_ROOT` and `REPO_KEY='core'` are hardcoded with no `--repo`. Four blockers, three of
which Ferrox's own backlog already names, plus `FF-B470` (HIGH, open): a run targeting another
project **bills Ferrox's paid roster** — six unauthorised paid calls already made on
2026-07-29.

Its own numbers matter more than the port: `ROADMAP.md:257-262` records that the 9-minute land
gate the milestone was designed around **"was never a measurement"** (real: 40s cold), and
`:270-274` that the premise **"measured false"**. `PROOF-v1.14.md:7-11` publishes the live
verdict as **NEGATIVE — false-green rate 0.333, 4 of 12 landed increments broken.**

Four ideas adopted here at zero cost: one land token per trunk; **verify after land, not only
before** (that flag being off by default is why 0.33 was invisible); the anti-loop rules;
width ~3-4 per contended resource. A full brief for the Ferrox session was delivered to Sean.

---

## 8. Housekeeping

- **Not deleted, may be live:** hetzner `wayland-f21bwo-m`; Windows `D:\lane-f21bwo` (0.4 GB).
  A duplicate lane instance held them. Verify nothing is running, then remove.
- **SeanDesktop:** 442 GB reclaimed today (C: 209.9 → 651.9 GB free). All 39 removed
  directories were Rust `target/`. **New work goes under `D:\`, never `C:\` root.** Never
  touch `C:\actions-runner-*` — three live runner services.
- `C:\tmp` (13 GB) left for Sean; it is not build output.
